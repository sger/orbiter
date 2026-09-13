use idevice::IdeviceError;
use rootcause::{
    hooks::{Hooks, context_formatter::ContextFormatterHook},
    prelude::*,
};

pub mod anisette;
pub mod auth;
pub mod dev;
pub mod sideload;
pub mod util;

#[derive(Debug, thiserror::Error)]
pub enum SideloadError {
    #[error("Auth error {0}: {1}")]
    AuthWithMessage(i64, String),

    #[error("Plist parse error: {0}")]
    PlistParseError(String),

    #[error("Failed to get anisette data, anisette not provisioned")]
    AnisetteNotProvisioned,

    #[error("Developer error {0}: {1}")]
    DeveloperError(i64, String),

    /// Apple answered HTTP 429: Apple's Retry-After in seconds when it sent one, plus an
    /// allowlisted label for the shape of the response body.
    #[error("Apple rate limited this request")]
    RateLimited(Option<u64>, &'static str),

    /// Apple asked for a sign-in step this adapter does not implement.
    #[error("Unsupported additional authentication step")]
    UnsupportedStep(String),

    #[error("Invalid bundle: {0}")]
    InvalidBundle(String),

    #[error("{0}")]
    IdeviceError(#[from] IdeviceError),
}

// The default reqwest error formatter sucks and provides no info
struct ReqwestErrorFormatter;

impl ContextFormatterHook<reqwest::Error> for ReqwestErrorFormatter {
    fn display(
        &self,
        report: rootcause::ReportRef<'_, reqwest::Error, markers::Uncloneable, markers::Local>,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        writeln!(f, "{}", report.format_current_context_unhooked())?;
        let mut source = report.current_context_error_source();
        while let Some(s) = source {
            writeln!(f, "Caused by: {:?}", s)?;
            source = s.source();
        }
        Ok(())
    }
}

pub fn init() -> Result<(), Report> {
    Hooks::new()
        .context_formatter::<reqwest::Error, _>(ReqwestErrorFormatter)
        .install()
        .context("Failed to install error reporting hooks")?;

    Ok(())
}

/// An additional-sign-in-step name, if it is safe to repeat.
///
/// Apple sends this as a short token naming a step (`trustedDeviceSecondaryAuth`, `repair`). It is
/// still server-provided, so it is repeated only when its shape leaves no room to carry anything
/// else: letters only, and short. Anything else is withheld, which loses a little traceability and
/// risks nothing.
fn safe_step_token(step: &str) -> Option<&str> {
    (!step.is_empty() && step.len() <= 40 && step.chars().all(|c| c.is_ascii_alphabetic()))
        .then_some(step)
}

/// Orbiter diagnostic: expose only error categories and numeric service/status codes.
/// Never format a report, its attachments, URLs, or server-provided messages.
pub fn redacted_auth_error(report: &Report) -> String {
    let detail = redacted_auth_error_detail(report);
    // Which step an unusable account stopped at is not the point: the account is the answer, and
    // the step name only gets between a person and it. The diagnostic still carries the step.
    if auth_error_is_unsupported_account(report) {
        return detail;
    }
    let stage = report.iter_reports().find_map(|cause| {
        let context = cause.downcast_current_context::<&str>().copied()
            .or_else(|| cause.downcast_current_context::<String>().map(String::as_str));
        match context {
            Some("Failed to send initial login request" | "GrandSlam error during initial login request") => Some("Initial Apple login"),
            Some("Failed to parse initial login response") => Some("Initial Apple login"),
            Some("Failed to send proof login request" | "GrandSlam error during proof login request") => Some("Apple password verification"),
            Some("Failed to parse proof login response") => Some("Apple password verification"),
            Some("Failed to complete trusted device 2FA") => Some("Trusted-device verification request"),
            Some("Failed to verify trusted device 2FA") => Some("Trusted-device code verification"),
            Some("Failed to complete SMS 2FA") => Some("SMS verification request"),
            Some("Failed to send app token request" | "GrandSlam error during app token request") => Some("Developer session authorization"),
            _ => None,
        }
    });
    match stage {
        Some(stage) => format!("{stage}: {detail}"),
        None => detail,
    }
}

fn redacted_auth_error_detail(report: &Report) -> String {
    let mut fallback = "The authentication adapter could not complete this step.";
    for cause in report.iter_reports() {
        let context = cause.downcast_current_context::<&str>().copied()
            .or_else(|| cause.downcast_current_context::<String>().map(String::as_str));
        match context {
            Some("Authentication destination is not an allowed Apple HTTPS endpoint.") => fallback = "The Apple service directory contains a destination rejected by the direct-Apple policy.",
            Some("Failed to parse URL Bag plist") => fallback = "Apple's service directory response could not be parsed.",
            Some("URL Bag plist missing 'urls' dictionary") => fallback = "Apple's service directory response is missing its endpoint list.",
            Some("Local authentication support unavailable.") => fallback = "Local macOS authentication support failed.",
            Some("Apple's sign-in response failed verification.") => fallback = "Apple's sign-in response failed verification. Orbiter stopped instead of trusting it.",
            Some("Apple's sign-in response could not be read.") => fallback = "Apple's sign-in response could not be read by this adapter.",
            // Apple answered the SRP init request without the fields a password check needs. An
            // account that signs in through an organisation's identity provider has no Apple
            // password to verify and produces exactly this shape, so it is named as the likely
            // reason — likely, because this response alone does not prove it.
            Some("Failed to parse initial login response") => fallback = "Apple answered without the password fields this step needs and reported no error of its own, which is how an account whose password lives at an organisation's identity provider — a federated or Managed Apple ID — answers: there is no Apple password for Orbiter to check. Retyping the password or entering a verification code cannot complete it. Sign in with a personal Apple ID instead.",
            Some("Failed to parse proof login response") => fallback = "Apple's answer to the password check was not in the shape this adapter expects.",
            _ => {}
        }
        if let Some(error) = cause.downcast_current_context::<SideloadError>() {
            match error {
                SideloadError::AuthWithMessage(-22320, _) => return "Apple requires federated organization sign-in. Orbiter does not yet support that flow; retrying the password or entering a Microsoft Authenticator code here will not complete it.".into(),
                // Allowlisted meanings for the codes a person can act on. The numeric code is
                // kept for traceability; Apple's own text is still never shown.
                SideloadError::AuthWithMessage(-20101 | -22406, _) => return "Apple did not accept this account and password. Check the password by signing in at appleid.apple.com, then retype it here — the field is cleared after every attempt. A Managed Apple ID that signs in through an organization's identity provider cannot be used here at all.".into(),
                SideloadError::AuthWithMessage(-21669, _) => return "Apple did not accept that verification code. Request a new code and try again.".into(),
                SideloadError::AuthWithMessage(code, _) => return format!("Apple authentication error {code}."),
                SideloadError::RateLimited(retry, shape) => {
                    let wait = match retry {
                        Some(seconds) => format!(" Apple asked to wait {seconds} seconds."),
                        None => " Apple sent no Retry-After.".into(),
                    };
                    return format!("Apple returned HTTP 429 (too many requests) with {shape}.{wait} Apple is throttling sign-ins from this Mac or account; this response does not establish whether the password or two-factor verification is valid.");
                }
                SideloadError::UnsupportedStep(step) => {
                    let named = match safe_step_token(step) {
                        Some(step) => format!(" Apple named the step \"{step}\"."),
                        None => String::new(),
                    };
                    return format!("Apple requires an additional sign-in step that Orbiter does not support.{named} Accounts that sign in through an organization's identity provider, or that must be repaired or updated at appleid.apple.com, cannot complete this flow yet.");
                }
                SideloadError::DeveloperError(code, _) => return format!("Apple developer error {code}."),
                // Both previously fell through to the generic fallback, which said nothing about
                // where to look.
                SideloadError::PlistParseError(_) => return "Apple's response was not the property list this adapter expects.".into(),
                SideloadError::AnisetteNotProvisioned => return "Local macOS authentication support is not provisioned on this Mac.".into(),
                _ => {}
            }
        }
        if let Some(error) = cause.downcast_current_context::<reqwest::Error>() {
            if error.is_timeout() { return "The Apple request timed out.".into(); }
            if error.is_connect() { return "Could not establish a direct TLS connection to Apple.".into(); }
            if error.status().map(|status| status.as_u16()) == Some(429) {
                return "Apple returned HTTP 429 (too many requests). Apple is throttling sign-ins from this Mac or account; this response does not establish whether the password or two-factor verification is valid.".into();
            }
            if error.status().map(|status| status.as_u16()) == Some(503) {
                return "Apple returned HTTP 503 (service unavailable). This response does not establish whether the password or two-factor verification is valid.".into();
            }
            if let Some(status) = error.status() { return format!("Apple returned HTTP {}.", status.as_u16()); }
            return "The Apple network request failed.".into();
        }
    }
    fallback.into()
}

/// How many links of a failure chain a diagnostic keeps. Enough to show where a sign-in stopped
/// and what it was doing, bounded so one report cannot become an unbounded string.
const MAX_DIAGNOSTIC_LINKS: usize = 12;
/// Upper bound on a whole diagnostic, so it stays something a person can paste into a message.
const MAX_DIAGNOSTIC_BYTES: usize = 600;

/// Orbiter diagnostic: a redacted summary of the *whole* failure chain, for someone to copy and
/// send when a sign-in fails in a way Orbiter does not recognise.
///
/// [`redacted_auth_error`] answers "what should I do about this" and deliberately says nothing
/// when the failure is unclassified. This answers "what happened", which is what makes an
/// unclassified failure fixable instead of invisible.
///
/// # What may appear, and why that is safe
///
/// * A context that downcasts to `&'static str` is a literal written in this crate's own source,
///   such as `.context("Failed to parse initial login response")`. It describes Orbiter's code
///   rather than Apple's answer, so it is reproduced verbatim.
/// * A context that downcasts to [`String`] was built by interpolation and may therefore carry
///   server-provided text, so only a fixed label is emitted in its place.
/// * A [`SideloadError`] contributes its variant name and, where Apple sent one, the numeric
///   code. The message payload — which can carry account state — is never read.
/// * A [`reqwest::Error`] contributes its kind and HTTP status. The URL and body are never read.
///
/// Any other context type contributes a fixed label, so a type this function does not know about
/// cannot disclose anything by default. Attachments are never walked at all.
pub fn auth_diagnostic(report: &Report) -> String {
    let mut links: Vec<String> = Vec::new();
    for cause in report.iter_reports() {
        if links.len() == MAX_DIAGNOSTIC_LINKS {
            links.push("…".into());
            break;
        }
        // A source literal is this crate's own words about its own code.
        if let Some(literal) = cause.downcast_current_context::<&str>().copied() {
            links.push(literal.to_string());
        } else if cause.downcast_current_context::<String>().is_some() {
            // Built with interpolation, so it may quote Apple. Its presence is the whole report.
            links.push("interpolated detail withheld".into());
        } else if let Some(error) = cause.downcast_current_context::<SideloadError>() {
            links.push(match error {
                SideloadError::AuthWithMessage(code, _) => format!("AuthWithMessage({code})"),
                SideloadError::DeveloperError(code, _) => format!("DeveloperError({code})"),
                SideloadError::RateLimited(Some(seconds), shape) => {
                    format!("RateLimited(retry after {seconds}s, {shape})")
                }
                SideloadError::RateLimited(None, shape) => {
                    format!("RateLimited(no Retry-After, {shape})")
                }
                SideloadError::UnsupportedStep(step) => match safe_step_token(step) {
                    Some(step) => format!("UnsupportedStep({step})"),
                    None => "UnsupportedStep".into(),
                },
                SideloadError::PlistParseError(_) => "PlistParseError".into(),
                SideloadError::AnisetteNotProvisioned => "AnisetteNotProvisioned".into(),
                SideloadError::InvalidBundle(_) => "InvalidBundle".into(),
                SideloadError::IdeviceError(_) => "IdeviceError".into(),
            });
        } else if let Some(missing) =
            cause.downcast_current_context::<crate::util::plist::MissingPlistValue>()
        {
            // The key is a constant this crate chose at the call site, so naming it says which
            // field Apple omitted without repeating anything Apple sent.
            links.push(match safe_step_token(&missing.key) {
                Some(key) => format!("missing {} '{key}'", missing.kind),
                None => format!("missing {}", missing.kind),
            });
        } else if let Some(error) = cause.downcast_current_context::<reqwest::Error>() {
            let kind = if error.is_timeout() {
                "timeout"
            } else if error.is_connect() {
                "connect"
            } else if error.is_body() || error.is_decode() {
                "body"
            } else {
                "request"
            };
            links.push(match error.status() {
                Some(status) => format!("reqwest {kind} (HTTP {})", status.as_u16()),
                None => format!("reqwest {kind}"),
            });
        } else {
            links.push("unrecognised error type".into());
        }
    }
    if links.is_empty() {
        return "no diagnostic available".into();
    }
    let mut joined = links.join(" ← ");
    if joined.len() > MAX_DIAGNOSTIC_BYTES {
        // Truncate on a character boundary: a diagnostic is text a person reads, not bytes.
        let cut = (0..=MAX_DIAGNOSTIC_BYTES)
            .rev()
            .find(|at| joined.is_char_boundary(*at))
            .unwrap_or(0);
        joined.truncate(cut);
        joined.push('…');
    }
    joined
}

/// Apple throttling (HTTP 429) detector. Returns the wait to honour before retrying: Apple's own
/// Retry-After when it sent one, otherwise a short debounce, because a guessed long wait would
/// lock the user out of an account Apple may not be throttling. `None` means not a throttling
/// response.
pub fn auth_throttle_delay(report: &Report) -> Option<std::time::Duration> {
    /// Apple sent no Retry-After: block a double submission, nothing more.
    const UNSTATED_THROTTLE_WAIT: u64 = 60;
    const MAX_THROTTLE_WAIT: u64 = 60 * 60;
    for cause in report.iter_reports() {
        if let Some(SideloadError::RateLimited(retry, _)) =
            cause.downcast_current_context::<SideloadError>()
        {
            let seconds = retry
                .unwrap_or(UNSTATED_THROTTLE_WAIT)
                .clamp(1, MAX_THROTTLE_WAIT);
            return Some(std::time::Duration::from_secs(seconds));
        }
        if let Some(error) = cause.downcast_current_context::<reqwest::Error>()
            && error.status().map(|status| status.as_u16()) == Some(429)
        {
            return Some(std::time::Duration::from_secs(UNSTATED_THROTTLE_WAIT));
        }
    }
    None
}

/// True when the account itself is one Orbiter cannot sign in, whatever the credentials are.
///
/// Three shapes mean this, and all three lead to the same answer — use a personal Apple ID:
///
/// * Apple's own federation code, `-22320`.
/// * An additional sign-in step this adapter does not implement.
/// * Apple accepting the request and answering without the password-verification fields, while
///   reporting no error of its own. An account whose password lives at an organisation's identity
///   provider has nothing for the password handshake to verify and answers exactly like this.
///
/// Kept separate from [`auth_error_is_inconclusive`], which means Apple never looked at the
/// credentials. This means Apple looked and there was no password to look at — a conclusive
/// answer, but not a rejected one, and a person must not be sent to reset a password over it.
pub fn auth_error_is_unsupported_account(report: &Report) -> bool {
    let mut missing_login_fields = false;
    let mut at_initial_login = false;
    for cause in report.iter_reports() {
        if let Some(error) = cause.downcast_current_context::<SideloadError>() {
            match error {
                SideloadError::AuthWithMessage(-22320, _) | SideloadError::UnsupportedStep(_) => {
                    return true;
                }
                // Apple reporting any other code of its own means it did evaluate the request,
                // so the shape below is not what happened.
                SideloadError::AuthWithMessage(..) => return false,
                _ => {}
            }
        }
        if cause
            .downcast_current_context::<crate::util::plist::MissingPlistValue>()
            .is_some()
        {
            missing_login_fields = true;
        }
        if cause.downcast_current_context::<&str>().copied()
            == Some("Failed to parse initial login response")
        {
            at_initial_login = true;
        }
    }
    missing_login_fields && at_initial_login
}

/// True when Apple never evaluated the credentials: throttling, service errors, or transport
/// failures. Such a report must not be presented as an authentication rejection.
pub fn auth_error_is_inconclusive(report: &Report) -> bool {
    if auth_throttle_delay(report).is_some() {
        return true;
    }
    report.iter_reports().any(|cause| {
        cause
            .downcast_current_context::<reqwest::Error>()
            .is_some_and(|error| {
                error.is_timeout()
                    || error.is_connect()
                    || error.status().is_some_and(|status| status.is_server_error())
            })
    })
}
