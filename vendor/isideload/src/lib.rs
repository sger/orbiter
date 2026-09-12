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

/// Orbiter diagnostic: expose only error categories and numeric service/status codes.
/// Never format a report, its attachments, URLs, or server-provided messages.
pub fn redacted_auth_error(report: &Report) -> String {
    let detail = redacted_auth_error_detail(report);
    let stage = report.iter_reports().find_map(|cause| {
        let context = cause.downcast_current_context::<&str>().copied()
            .or_else(|| cause.downcast_current_context::<String>().map(String::as_str));
        match context {
            Some("Failed to send initial login request" | "GrandSlam error during initial login request") => Some("Initial Apple login"),
            Some("Failed to send proof login request" | "GrandSlam error during proof login request") => Some("Apple password verification"),
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
            _ => {}
        }
        if let Some(error) = cause.downcast_current_context::<SideloadError>() {
            match error {
                SideloadError::AuthWithMessage(-22320, _) => return "Apple requires federated organization sign-in. Orbiter does not yet support that flow; retrying the password or entering a Microsoft Authenticator code here will not complete it.".into(),
                SideloadError::AuthWithMessage(code, _) => return format!("Apple authentication error {code}."),
                SideloadError::RateLimited(retry, shape) => {
                    let wait = match retry {
                        Some(seconds) => format!(" Apple asked to wait {seconds} seconds."),
                        None => " Apple sent no Retry-After.".into(),
                    };
                    return format!("Apple returned HTTP 429 (too many requests) with {shape}.{wait} Apple is throttling sign-ins from this Mac or account; this response does not establish whether the password or two-factor verification is valid.");
                }
                SideloadError::UnsupportedStep(_) => return "Apple requires an additional sign-in step that Orbiter does not support. Accounts that sign in through an organization's identity provider, or that must be repaired or updated at appleid.apple.com, cannot complete this flow yet.".into(),
                SideloadError::DeveloperError(code, _) => return format!("Apple developer error {code}."),
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
