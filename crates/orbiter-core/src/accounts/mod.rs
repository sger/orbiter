//! Apple account sign-in, two-factor challenges, the developer session and team selection.
//!
//! This half owns *who Orbiter is signed in as*: authentication, the challenge flow, the
//! session's thirty-minute lifetime, which team is selected, and signing out. What that
//! session is then used **for** — registering a device, requesting a certificate, reserving
//! identifiers, signing — lives in [`provisioning`], because those are the operations that
//! change something at Apple and each needs its own acknowledgement.
//!
//! # Secrets
//!
//! A password and a verification code exist only for the duration of the request that uses
//! them and are zeroized afterwards; neither is ever stored, logged, or returned. The session
//! itself lives only in memory and expires.
//!
//! # Locking
//!
//! A blocking mutex guards the view and session, and is never held across an `await`. A
//! separate async gate admits one Apple-contacting operation at a time.
//!
//! Nothing in this file mutates a certificate, a profile, a device registration or an app
//! identifier; those all live in [`provisioning`].

mod guided;
pub mod provisioning;

use crate::domain::errors::{ErrorCode, OperationError, OperationResult};
use isideload::{
    auth::apple_account::{AppleAccount, TwoFactorCallbackParams, TwoFactorCallbackResponse},
    dev::{developer_session::DeveloperSession, teams::TeamsApi},
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use uuid::Uuid;

/// Configure TLS once, before any authentication request is made.
///
/// Installs the process-wide cryptography provider explicitly rather than leaving it to whichever
/// dependency first needs one, so the first sign-in cannot fail for want of it.
pub fn initialize() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        // reqwest rustls-no-provider requires explicit initialization, even with one provider.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let _ = isideload::init();
    });
}

const SESSION_LIFETIME: Duration = Duration::from_secs(30 * 60);
const LOGIN_DEADLINE: Duration = Duration::from_secs(10 * 60);
const UNAVAILABLE: &str = "Account state unavailable. Restart Orbiter.";

#[derive(Clone, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    #[default]
    SignedOut,
    SigningIn,
    TwoFactor,
    SignedIn,
    Failed,
}
#[derive(Clone, Serialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub kind: Option<String>,
    /// Whether this is a free Personal Team. `None` when Apple's answer does not establish it:
    /// the difference decides seven-day profiles and which capabilities survive re-signing, so it
    /// is never guessed.
    pub free: Option<bool>,
    /// Allowlisted membership label for display, e.g. "Apple Developer Program".
    pub membership: Option<String>,
}

/// Classify a team from Apple's membership list. A paid membership names a program; a free
/// account's Personal Team does not. Anything unrecognised stays unknown rather than assumed.
fn classify(team: &isideload::dev::teams::DeveloperTeam) -> (Option<bool>, Option<String>) {
    let named: Vec<String> = team
        .memberships
        .iter()
        .filter_map(|membership| membership.name.clone())
        .filter(|name| name.len() <= 120 && !name.chars().any(char::is_control))
        .collect();
    let label = named.first().cloned();
    let lowercase: Vec<String> = named.iter().map(|name| name.to_ascii_lowercase()).collect();
    // Observed on a live free account: Apple names the Personal Team membership
    // "Xcode Free Provisioning Program". It ends in "Program" like a paid one, so match the
    // free wording first.
    if lowercase
        .iter()
        .any(|name| name.contains("free provisioning"))
    {
        return (Some(true), label);
    }
    if lowercase
        .iter()
        .any(|name| name.contains("developer program") || name.contains("enterprise"))
    {
        return (Some(false), label);
    }
    // A Personal Team that reports no membership at all is still a free team.
    if named.is_empty() && team.r#type.as_deref() == Some("Individual") {
        return (Some(true), label);
    }
    (None, label)
}
#[derive(Clone, Serialize)]
pub struct Number {
    pub id: u32,
    pub label: String,
}
#[derive(Clone, Serialize)]
pub struct Challenge {
    pub id: String,
    pub sms: bool,
    pub unknown: bool,
    pub retry: bool,
    pub numbers: Vec<Number>,
}
#[derive(Clone, Serialize, Default)]
pub struct View {
    pub stage: Stage,
    pub account: Option<String>,
    pub teams: Vec<Team>,
    pub selected_team: Option<String>,
    pub challenge: Option<Challenge>,
    pub message: String,
    /// A redacted technical summary of the last failure, for someone to copy and send when the
    /// message alone does not say enough. Present only after a failure Orbiter could classify no
    /// further; cleared whenever a new attempt starts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}
#[derive(Deserialize)]
#[serde(tag = "action", content = "value", rename_all = "snake_case")]
pub enum Answer {
    Code(String),
    Sms(u32),
    Devices,
    Resend,
}
struct Session {
    developer: DeveloperSession,
    /// Signing key and certificate for this session. Memory only: never written to disk.
    identity: Option<crate::certificates::Identity>,
}
/// What provisioning produced: the plan it followed, what Apple registered, and the profiles.
#[derive(Clone, Serialize)]
pub struct Preparation {
    pub plan: crate::plan::Plan,
    pub app_ids: Vec<crate::provisioning::AppIdOutcome>,
    pub profiles: Vec<crate::provisioning::ProfileOutcome>,
}
/// A sign-in failure plus any Apple-imposed wait Orbiter must honour before retrying.
struct Failure {
    message: String,
    retry_after: Option<Duration>,
    /// The redacted shape of the upstream report. Kept because the classified `message` says
    /// nothing at all when the failure is one Orbiter does not recognise, and an unrecognised
    /// failure that leaves no trace cannot be fixed.
    diagnostic: Option<String>,
}
impl Failure {
    /// Classify an authentication failure and keep only what is safe to show.
    ///
    /// The upstream report is inspected for its *kind* and then discarded: Apple's own text can
    /// carry account state and server payloads, and a person needs to know which step to take
    /// rather than what the service said.
    fn new(message: String, error: &rootcause::Report) -> Self {
        let diagnostic = isideload::auth_diagnostic(error);
        // Logged as well as carried: a person who never presses the copy button still leaves a
        // trace behind for whoever looks at the terminal. The target is named explicitly so this
        // passes the filter the application installs, which selects on `orbiter`.
        tracing::warn!(target: "orbiter", operation = "sign-in", detail = %diagnostic);
        Self {
            retry_after: isideload::auth_throttle_delay(error),
            diagnostic: Some(diagnostic),
            message,
        }
    }
    /// A failure with a fixed message and no upstream report to classify.
    fn plain(message: String) -> Self {
        Self {
            message,
            retry_after: None,
            diagnostic: None,
        }
    }
}
#[derive(Default)]
struct Inner {
    generation: String,
    view: View,
    task: Option<tokio::task::AbortHandle>,
    response: Option<oneshot::Sender<TwoFactorCallbackResponse>>,
    session: Option<Session>,
    expires_at: Option<Instant>,
    /// Apple-side sign-in throttling. Kept across sign-out and expiry: it is not Orbiter state.
    retry_at: Option<Instant>,
    /// Profiles obtained for the selected team, held for the signer.
    ///
    /// Beside the team selection rather than inside the session, because that is the relationship
    /// that governs them: identifiers are derived from the team, so profiles for one team describe
    /// identifiers another will never produce. Changing the team discards them, and so does losing
    /// the session — see [`Inner::clear`].
    profiles: Vec<crate::provisioning::ProfileOutcome>,
}
#[derive(Clone, Default)]
pub struct Accounts(Arc<Mutex<Inner>>, Arc<tokio::sync::Mutex<()>>);

impl Inner {
    /// Forget the session and everything derived from it, leaving `message` on screen.
    ///
    /// Aborts any worker still waiting on a challenge and starts a new generation, so a late
    /// answer from the old one cannot be applied to whatever replaces it. The throttle deadline
    /// deliberately survives: it is Apple's state about this machine, not Orbiter's about a
    /// session, and clearing it would let a rejected password be retried immediately.
    fn clear(&mut self, message: &str) {
        self.generation = Uuid::new_v4().to_string();
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.response = None;
        self.session = None;
        self.profiles.clear();
        self.expires_at = None;
        self.view = View {
            message: message.into(),
            ..View::default()
        };
    }
    /// How long Apple's throttling still has to run, if it does.
    ///
    /// Clears a deadline that has passed, so a lapsed hold does not keep refusing sign-ins.
    fn throttle_remaining(&mut self) -> Option<Duration> {
        let deadline = self.retry_at?;
        match deadline.checked_duration_since(Instant::now()) {
            Some(remaining) if !remaining.is_zero() => Some(remaining),
            _ => {
                self.retry_at = None;
                None
            }
        }
    }
    /// Clear the session if it has outlived its thirty minutes.
    ///
    /// Called on every read, so a stale session is never reported as live and a signed-out view is
    /// what a person sees rather than an operation failing later for an unexplained reason.
    /// Point at a team, discarding provisioning obtained for a different one.
    ///
    /// Identifiers are derived from the team, so profiles fetched for the previous one describe
    /// identifiers this team will never produce. Signing would refuse them anyway, but it would
    /// refuse by naming a missing profile rather than the team change that caused it — and a stale
    /// profile is not worth keeping either way. Re-selecting the same team changes nothing.
    fn retarget(&mut self, team: String) {
        if self.view.selected_team.as_deref() != Some(team.as_str()) {
            self.profiles.clear();
            if let Some(session) = self.session.as_mut() {
                session.identity = None;
            }
        }
        self.view.selected_team = Some(team);
    }

    /// Clear the session if it has outlived its thirty minutes.
    ///
    /// Called on every read, so a stale session is never reported as live and a signed-out view is
    /// what a person sees rather than an operation failing later for an unexplained reason.
    fn expire(&mut self) {
        if self
            .expires_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.clear("The local account session expired. Sign in again.");
        }
    }
}

impl Accounts {
    /// The current sign-in state, expiring a session that has outlived its lifetime first.
    ///
    /// Local only: reports state this process already holds and contacts nothing.
    ///
    /// # Errors
    ///
    /// Fails only on lock poisoning.
    pub fn status(&self) -> OperationResult<View> {
        let mut inner = self
            .0
            .lock()
            .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
        inner.expire();
        Ok(inner.view.clone())
    }
    /// Forget the session.
    ///
    /// Local only. Nothing is revoked at Apple: a certificate this session obtained still exists
    /// and a signed build still works. Any two-factor prompt still waiting is cancelled.
    ///
    /// # Errors
    ///
    /// Fails only on lock poisoning.
    pub fn sign_out(&self) -> OperationResult<View> {
        let _gate = self.1.try_lock().map_err(|_| {
            OperationError::operation_in_progress("Wait for account preparation to finish.")
        })?;
        let mut inner = self
            .0
            .lock()
            .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
        inner.clear("Signed out locally. macOS manages its own authentication support data.");
        Ok(inner.view.clone())
    }
    /// Begin signing in to Apple.
    ///
    /// **Contacts Apple.** Validates consent and input before anything is sent or stored, then
    /// hands the credentials to a worker. The password exists only for that request and is
    /// zeroized afterwards; it is never written down, logged, or returned.
    ///
    /// Returns immediately with the next state — a challenge is answered through [`Self::answer`].
    ///
    /// # Errors
    ///
    /// Fails without consent, with empty input, while a sign-in is already running, or while
    /// Apple's throttling of this machine is still in force.
    pub fn start(&self, email: String, password: String, consent: bool) -> OperationResult<View> {
        let _gate = self.1.try_lock().map_err(|_| {
            OperationError::operation_in_progress("Wait for account preparation to finish.")
        })?;
        initialize();
        let password = zeroize::Zeroizing::new(password);
        if !consent {
            return Err("Confirm direct Apple authentication before signing in.".into());
        }
        let email = email.trim().to_string();
        if email.is_empty()
            || email.len() > 254
            || email.chars().any(char::is_control)
            || password.is_empty()
            || password.len() > 1024
        {
            return Err("Enter your Apple account and password in the application.".into());
        }
        let mut inner = self
            .0
            .lock()
            .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
        inner.expire();
        if inner.task.is_some() || inner.session.is_some() {
            return Err("Cancel or sign out before starting another account session.".into());
        }
        if let Some(remaining) = inner.throttle_remaining() {
            // Only the wait Apple itself asked for, so a rebuilt adapter can always be retried.
            inner.view = View {
                stage: Stage::Failed,
                message: format!(
                    "Apple asked to wait before another sign-in. Try again in about {} second(s); Apple did not check your password.",
                    remaining.as_secs().max(1)
                ),
                ..View::default()
            };
            return Ok(inner.view.clone());
        }
        let generation = Uuid::new_v4().to_string();
        inner.generation = generation.clone();
        inner.view = View {
            stage: Stage::SigningIn,
            message: "Checking local authentication support and signing in to Apple…".into(),
            ..View::default()
        };
        let manager = self.clone();
        let task = tokio::spawn(async move {
            let _guard = WorkerGuard(manager.clone(), generation.clone());
            let result =
                tokio::time::timeout(LOGIN_DEADLINE, manager.login(&generation, email, &password))
                    .await;
            match result {
                Ok(Ok((account, session, teams))) => {
                    manager.finish(&generation, account, session, teams)
                }
                Ok(Err(failure)) => manager.fail(
                    &generation,
                    &failure.message,
                    failure.retry_after,
                    failure.diagnostic,
                ),
                Err(_) => manager.fail(
                    &generation,
                    "Sign-in timed out. Check your connection and start again.",
                    None,
                    None,
                ),
            }
        });
        inner.task = Some(task.abort_handle());
        Ok(inner.view.clone())
    }
    /// Run one sign-in attempt against Apple, on a worker task.
    ///
    /// **Contacts Apple.** Holds the password only for the duration of the request and zeroizes it
    /// afterwards. A two-factor challenge suspends here until [`Self::answer`] supplies a reply or
    /// the session is cleared.
    ///
    /// Apple's throttling of this machine is recorded as a local hold so the next attempt is
    /// refused here rather than being sent and refused again — which is what makes throttling
    /// worse.
    async fn login(
        &self,
        generation: &str,
        email: String,
        password: &str,
    ) -> Result<(String, DeveloperSession, Vec<Team>), Failure> {
        let provider = crate::local_anisette::LocalProvider;
        let manager = self.clone();
        let id = generation.to_string();
        let mut account = AppleAccount::builder(&email)
            .anisette_provider(provider)
            .danger_debug(false)
            .build()
            .await
            .map_err(|error| {
                Failure::new(
                    format!(
                        "Sign-in setup failed before account verification. {}",
                        isideload::redacted_auth_error(&error)
                    ),
                    &error,
                )
            })?;
        account
            .login(password, move |params| {
                let manager = manager.clone();
                let id = id.clone();
                async move { Ok(manager.challenge(&id, params).await) }
            })
            .await
            .map_err(|error| {
                // Throttling, outages, and transport failures never reached the credentials;
                // saying "authentication failed" would send the user to reset a valid password.
                let prefix = if isideload::auth_error_is_inconclusive(&error) {
                    "Apple did not complete sign-in, and did not report your password or two-factor verification as wrong."
                } else {
                    "Apple account authentication or two-factor verification failed."
                };
                Failure::new(
                    format!("{prefix} {}", isideload::redacted_auth_error(&error)),
                    &error,
                )
            })?;
        let mut developer = DeveloperSession::from_account(&mut account)
            .await
            .map_err(|error| {
                // The static sentence alone hid which step failed and why. Keep the allowlisted
                // detail: it names the stage and any numeric Apple code, and never server text.
                Failure::new(
                    format!(
                        "Apple authentication completed, but developer access failed. {} If this account has never been used for development, accept the Apple Developer Agreement at developer.apple.com once, then sign in again.",
                        isideload::redacted_auth_error(&error)
                    ),
                    &error,
                )
            })?;
        let teams = read_teams(&mut developer).await.map_err(Failure::plain)?;
        Ok((email, developer, teams))
    }
    /// Present a two-factor challenge and wait for the person to answer it.
    ///
    /// Trusted phone numbers are masked before they reach the view. The wait ends when an answer
    /// arrives, when the session is cleared, or when the account generation moves on — a stale
    /// reply can never be applied to a newer prompt.
    async fn challenge(
        &self,
        generation: &str,
        params: TwoFactorCallbackParams,
    ) -> TwoFactorCallbackResponse {
        let (sender, receiver) = oneshot::channel();
        {
            let Ok(mut inner) = self.0.lock() else {
                return TwoFactorCallbackResponse::Abort;
            };
            if inner.generation != generation {
                return TwoFactorCallbackResponse::Abort;
            }
            inner.view.stage = Stage::TwoFactor;
            inner.view.message =
                "Complete two-factor authentication using your trusted device or phone.".into();
            inner.view.challenge = Some(Challenge {
                id: Uuid::new_v4().to_string(),
                sms: params.sms,
                unknown: params.unknown,
                retry: params.last_error.is_some(),
                numbers: params
                    .numbers
                    .iter()
                    .take(20)
                    .map(|n| Number {
                        id: n.id,
                        label: format!(
                            "Phone ending in {}",
                            n.last_two_digits
                                .chars()
                                .filter(char::is_ascii_digit)
                                .take(2)
                                .collect::<String>()
                        ),
                    })
                    .collect(),
            });
            inner.response = Some(sender);
        }
        receiver.await.unwrap_or(TwoFactorCallbackResponse::Abort)
    }
    /// Answer a two-factor challenge, or ask for the code another way.
    ///
    /// A verification code is treated exactly as a password: used once, zeroized, never stored. An
    /// answer to a challenge that is no longer current is refused rather than applied to whatever
    /// replaced it — which is what stops a stale reply consuming a fresh prompt.
    ///
    /// # Errors
    ///
    /// Fails if the challenge is unknown or superseded, if the input is empty, or if the session
    /// was cleared while the prompt was open.
    pub fn answer(&self, challenge_id: String, answer: Answer) -> OperationResult<View> {
        let mut inner = self
            .0
            .lock()
            .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
        let challenge = inner
            .view
            .challenge
            .as_ref()
            .filter(|c| c.id == challenge_id)
            .ok_or("This verification prompt expired. Check the current account status.")?;
        let response = match answer {
            Answer::Code(code)
                if !challenge.unknown
                    && code.len() == 6
                    && code.bytes().all(|b| b.is_ascii_digit()) =>
            {
                TwoFactorCallbackResponse::SubmitCode(code)
            }
            Answer::Sms(id) if challenge.numbers.iter().any(|n| n.id == id) => {
                TwoFactorCallbackResponse::SendSms(id)
            }
            Answer::Devices => TwoFactorCallbackResponse::SendToDevices,
            Answer::Resend if !challenge.unknown => TwoFactorCallbackResponse::ResendCode,
            _ => {
                return Err(
                    "Enter a six-digit code or choose an available verification method.".into(),
                );
            }
        };
        inner
            .response
            .take()
            .ok_or("Verification is no longer waiting for a response.")?
            .send(response)
            .map_err(|_| "Verification expired. Start sign-in again.")?;
        inner.view.challenge = None;
        inner.view.stage = Stage::SigningIn;
        inner.view.message = "Waiting for Apple verification…".into();
        Ok(inner.view.clone())
    }
    /// Record a completed sign-in: the account, its teams, and the session's deadline.
    ///
    /// Ignored if the account generation has moved on, so a worker that finishes after a sign-out
    /// cannot resurrect the session it was working on.
    fn finish(
        &self,
        generation: &str,
        account: String,
        developer: DeveloperSession,
        teams: Vec<Team>,
    ) {
        if let Ok(mut inner) = self.0.lock() {
            if inner.generation != generation {
                return;
            }
            inner.task = None;
            inner.response = None;
            inner.session = Some(Session {
                developer,
                identity: None,
            });
            inner.profiles.clear();
            inner.expires_at = Some(Instant::now() + SESSION_LIFETIME);
            inner.view = View {
                stage: Stage::SignedIn,
                account: Some(account),
                teams,
                message: "Signed in. Select the signing team this build should be re-signed for."
                    .into(),
                ..View::default()
            };
        }
    }
    /// Record a failed sign-in, optionally holding further attempts until Apple's throttling
    /// lapses.
    ///
    /// Ignored if the account generation has moved on. `message` is already redacted; nothing from
    /// Apple's own response reaches it.
    fn fail(
        &self,
        generation: &str,
        message: &str,
        retry_after: Option<Duration>,
        diagnostic: Option<String>,
    ) {
        if let Ok(mut inner) = self.0.lock() {
            if inner.generation != generation || inner.task.is_none() {
                return;
            }
            if let Some(wait) = retry_after {
                inner.retry_at = Some(Instant::now() + wait);
            }
            inner.task = None;
            inner.response = None;
            inner.session = None;
            inner.expires_at = None;
            inner.view = View {
                stage: Stage::Failed,
                message: message.into(),
                diagnostic,
                ..View::default()
            };
        }
    }
    /// Choose which of the account's teams to work with.
    ///
    /// Local only. Identifiers are derived from the team, so a plan prepared for one says nothing
    /// about another: changing the team discards any profiles already obtained, and the interface
    /// clears the preparation it was showing.
    ///
    /// # Errors
    ///
    /// Fails without a session, or if the identifier names no team this account belongs to.
    pub fn select_team(&self, id: String) -> OperationResult<View> {
        let _gate = self.1.try_lock().map_err(|_| {
            OperationError::operation_in_progress("Wait for account preparation to finish.")
        })?;
        let mut inner = self
            .0
            .lock()
            .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
        inner.expire();
        if inner.session.is_none() || !inner.view.teams.iter().any(|team| team.id == id) {
            return Err(OperationError::new(
                ErrorCode::AuthenticationRequired,
                "Select a team returned by the current Apple account session.",
            ));
        }
        inner.retarget(id);
        Ok(inner.view.clone())
    }
    /// Ask Apple for the account's teams again.
    ///
    /// **Contacts Apple** but mutates nothing. A session that can no longer be refreshed is
    /// cleared along with the team selection, so the interface shows signed-out rather than acting
    /// on a session that has quietly stopped working.
    ///
    /// # Errors
    ///
    /// Fails if a refresh is already running, or if the session cannot be refreshed — in which
    /// case the returned view already reflects being signed out.
    pub async fn refresh_teams(&self) -> OperationResult<View> {
        let _gate = self.1.try_lock().map_err(|_| {
            OperationError::operation_in_progress("Team refresh is already running.")
        })?;
        let (generation, mut developer) = {
            let mut inner = self
                .0
                .lock()
                .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
            inner.expire();
            let session = inner
                .session
                .as_ref()
                .ok_or("Sign in before refreshing teams.")?;
            (inner.generation.clone(), session.developer.clone())
        };
        let result =
            tokio::time::timeout(Duration::from_secs(60), read_teams(&mut developer)).await;
        let mut inner = self
            .0
            .lock()
            .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
        inner.expire();
        if inner.generation != generation {
            return Ok(inner.view.clone());
        }
        match result {
            Ok(Ok(teams)) => {
                if !teams.iter().any(|t| Some(&t.id) == inner.view.selected_team.as_ref()) { inner.view.selected_team = None; }
                inner.view.teams = teams;
            }
            _ => inner.clear("Developer session could not be refreshed. Sign in again; your team selection was cleared."),
        }
        Ok(inner.view.clone())
    }
}
/// Machine label Apple shows beside the certificate. The computer name, bounded and stripped of
/// anything that is not plain text, with a neutral fallback.
pub(super) fn hostname() -> String {
    let raw = std::env::var("HOST")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        .take(60)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        "Orbiter Mac".into()
    } else {
        cleaned
    }
}
struct WorkerGuard(Accounts, String);
impl Drop for WorkerGuard {
    /// Report a worker that stopped without finishing, so the interface does not wait forever.
    ///
    /// Does no I/O and cannot fail: it records a message against the generation it belongs to,
    /// which is ignored if that generation has already moved on.
    fn drop(&mut self) {
        self.0.fail(
            &self.1,
            "Authentication worker stopped. Start sign-in again.",
            None,
            None,
        );
    }
}
/// Turn Apple's team listing into the view's own shape.
///
/// Classifies free versus paid from the membership Apple reports, matching the free wording first
/// and treating anything unrecognised as undetermined rather than guessing. That answer decides
/// seven-day expiry and which capabilities survive re-signing, so a guess would be worse than an
/// admission.
async fn read_teams(developer: &mut DeveloperSession) -> Result<Vec<Team>, String> {
    let teams = developer.list_teams().await.map_err(|_| "Could not list developer teams. Check your developer account access and sign in again.")?;
    if teams.len() > 100 {
        return Err("The account returned too many teams for this preview.".into());
    }
    Ok(teams
        .into_iter()
        .map(|team| {
            let (free, membership) = classify(&team);
            Team {
                id: team.team_id,
                name: team.name.unwrap_or_else(|| "Unnamed team".into()),
                kind: team.r#type,
                free,
                membership,
            }
        })
        .collect())
}

#[cfg(test)]
/// Checks that authentication fails legibly without echoing Apple, and that secrets never persist.
mod tests {
    use super::*;
    /// An account waiting on a two-factor challenge, with the channel its answer would go to.
    fn pending() -> (Accounts, oneshot::Receiver<TwoFactorCallbackResponse>) {
        let manager = Accounts::default();
        let (tx, rx) = oneshot::channel();
        {
            let mut inner = manager.0.lock().unwrap();
            inner.generation = "current".into();
            inner.view.stage = Stage::TwoFactor;
            inner.view.challenge = Some(Challenge {
                id: "challenge".into(),
                sms: false,
                unknown: false,
                retry: false,
                numbers: vec![Number {
                    id: 7,
                    label: "Phone ending in 12".into(),
                }],
            });
            inner.response = Some(tx);
        }
        (manager, rx)
    }
    #[test]
    /// A failure's diagnostic carries the classification and none of Apple's own text or
    /// attachments, which can include account state.
    fn authentication_diagnostic_excludes_server_messages_and_attachments() {
        let error = rootcause::report!(isideload::SideloadError::AuthWithMessage(
            -12345,
            "SECRET_EMAIL_PASSWORD".into()
        ))
        .attach("SECRET_TOKEN");
        let message = isideload::redacted_auth_error(&error.into_dynamic());
        assert!(message.contains("-12345"));
        assert!(!message.contains("SECRET"));
        let unknown = rootcause::report!("SECRET_UNTYPED_PAYLOAD");
        assert!(!isideload::redacted_auth_error(&unknown.into_dynamic()).contains("SECRET"));
    }
    #[test]
    /// A federated (managed) account is named as unsupported with what to do about it, without
    /// echoing the server's response.
    fn federated_login_error_is_actionable_without_exposing_server_payload() {
        let error = rootcause::report!(isideload::SideloadError::AuthWithMessage(
            -22320,
            "SECRET_FEDERATION_URL".into()
        ))
        .context("GrandSlam error during initial login request")
        .attach("SECRET_TOKEN");
        let message = isideload::redacted_auth_error(&error.into_dynamic());
        assert!(message.starts_with("Initial Apple login:"));
        assert!(message.contains("federated organization sign-in"));
        assert!(!message.contains("SECRET"));
    }

    #[test]
    /// Rejected credentials produce Orbiter's own sentence, never Apple's — which can differ by
    /// account state and reveal more than whether the password was right.
    fn rejected_credentials_are_explained_without_apple_text() {
        for code in [-20101, -22406] {
            let error = rootcause::report!(isideload::SideloadError::AuthWithMessage(
                code,
                "SECRET_SERVER_TEXT".into()
            ))
            .into_dynamic();
            let message = isideload::redacted_auth_error(&error);
            assert!(message.contains("did not accept this account and password"));
            assert!(message.contains("appleid.apple.com"));
            assert!(!message.contains("SECRET"));
            // A rejection is conclusive: it must not be softened into "Apple did not complete".
            assert!(!isideload::auth_error_is_inconclusive(&error));
        }
        let wrong_code = rootcause::report!(isideload::SideloadError::AuthWithMessage(
            -21669,
            "SECRET".into()
        ))
        .into_dynamic();
        assert!(isideload::redacted_auth_error(&wrong_code).contains("verification code"));
    }
    #[test]
    /// Only recognised authentication stages are accepted. An unfamiliar one is refused rather
    /// than being carried into a state machine that does not model it.
    fn authentication_stages_are_allowlisted() {
        for (context, expected) in [
            (
                "Failed to send initial login request",
                "Initial Apple login:",
            ),
            (
                "Failed to send proof login request",
                "Apple password verification:",
            ),
            (
                "Failed to send app token request",
                "Developer session authorization:",
            ),
        ] {
            let error = rootcause::report!("SECRET_RESPONSE").context(context);
            let message = isideload::redacted_auth_error(&error.into_dynamic());
            assert!(message.starts_with(expected));
            assert!(!message.contains("SECRET"));
        }
    }

    #[test]
    /// Apple throttling this machine is reported as throttling, not as a wrong password — which
    /// would send someone to reset a password that was correct.
    fn throttled_sign_in_is_not_reported_as_a_rejected_password() {
        let throttled = rootcause::report!(isideload::SideloadError::RateLimited(
            Some(900),
            "a GrandSlam plist carrying an authentication code"
        ))
        .attach("SECRET_RESPONSE")
        .into_dynamic();
        assert!(isideload::auth_error_is_inconclusive(&throttled));
        assert_eq!(
            isideload::auth_throttle_delay(&throttled),
            Some(Duration::from_secs(900))
        );
        let message = isideload::redacted_auth_error(&throttled);
        assert!(message.contains("429"));
        assert!(message.contains("GrandSlam plist"));
        assert!(message.contains("900 seconds"));
        assert!(!message.contains("SECRET"));

        // Without a Retry-After, Orbiter debounces rather than inventing a long lockout of its
        // own: a guessed wait would block retrying a corrected adapter.
        let bare = rootcause::report!(isideload::SideloadError::RateLimited(None, "no body"))
            .into_dynamic();
        assert_eq!(
            isideload::auth_throttle_delay(&bare),
            Some(Duration::from_secs(60))
        );
        // Apple's own wait is honoured, and bounded.
        let hour = rootcause::report!(isideload::SideloadError::RateLimited(
            Some(99_999),
            "no body"
        ))
        .into_dynamic();
        assert_eq!(
            isideload::auth_throttle_delay(&hour),
            Some(Duration::from_secs(3600))
        );

        let rejected = rootcause::report!(isideload::SideloadError::AuthWithMessage(
            -20101,
            "x".into()
        ))
        .into_dynamic();
        assert!(!isideload::auth_error_is_inconclusive(&rejected));
        assert!(isideload::auth_throttle_delay(&rejected).is_none());
    }
    #[test]
    /// An additional step Orbiter cannot perform is named as unsupported, without repeating what
    /// Apple said about it.
    fn unsupported_additional_step_is_named_without_echoing_apple_text() {
        let error = rootcause::report!(isideload::SideloadError::UnsupportedStep(
            "SECRET_STEP_NAME".into()
        ))
        .into_dynamic();
        let message = isideload::redacted_auth_error(&error);
        assert!(message.contains("additional sign-in step"));
        assert!(!message.contains("SECRET"));
        assert!(!isideload::auth_error_is_inconclusive(&error));

        // A plain token naming the step has no room to carry anything but the step, so it is
        // repeated: knowing which step Apple asked for is what makes the gap fixable.
        let named = rootcause::report!(isideload::SideloadError::UnsupportedStep(
            "federatedAuth".into()
        ))
        .into_dynamic();
        assert!(isideload::redacted_auth_error(&named).contains("\"federatedAuth\""));
        assert!(isideload::auth_diagnostic(&named).contains("UnsupportedStep(federatedAuth)"));
    }
    #[test]
    /// Apple answering the password step without the fields it needs is named, and the federated
    /// organisation account that produces that shape is named as the likely reason.
    ///
    /// This case previously produced the bare "could not complete this step" fallback with no
    /// stage, which told a person nothing about where to look.
    fn a_login_response_without_password_fields_names_the_federated_account() {
        let error = rootcause::report!("SECRET_PLIST_BODY")
            .context("Failed to parse initial login response")
            .into_dynamic();
        let message = isideload::redacted_auth_error(&error);
        assert!(message.starts_with("Initial Apple login:"));
        assert!(message.contains("identity provider"));
        assert!(message.contains("personal Apple ID"));
        assert!(!message.contains("SECRET"));
        assert!(!message.contains("could not complete this step"));
    }

    #[test]
    /// Two upstream variants used to fall through to the generic fallback. Each now says which
    /// part of sign-in to look at.
    fn previously_unclassified_variants_say_where_to_look() {
        let parse = rootcause::report!(isideload::SideloadError::PlistParseError(
            "SECRET_BODY".into()
        ))
        .into_dynamic();
        assert!(isideload::redacted_auth_error(&parse).contains("property list"));
        assert!(!isideload::redacted_auth_error(&parse).contains("SECRET"));

        let anisette =
            rootcause::report!(isideload::SideloadError::AnisetteNotProvisioned).into_dynamic();
        assert!(isideload::redacted_auth_error(&anisette).contains("Local macOS"));
    }

    #[test]
    /// The diagnostic reproduces this crate's own source literals, because they describe Orbiter's
    /// code, and reduces everything that could quote Apple to a fixed label.
    fn a_diagnostic_keeps_source_literals_and_withholds_everything_else() {
        let error = rootcause::report!(isideload::SideloadError::AuthWithMessage(
            -22320,
            "SECRET_SERVER_TEXT".into()
        ))
        .context("Failed to parse initial login response")
        .attach("SECRET_ATTACHMENT")
        .into_dynamic();
        let diagnostic = isideload::auth_diagnostic(&error);
        // The literal is Orbiter's own wording about its own code, so it is shown whole.
        assert!(diagnostic.contains("Failed to parse initial login response"));
        // Apple's numeric code is traceable; the text it came with is not read at all.
        assert!(diagnostic.contains("AuthWithMessage(-22320)"));
        assert!(!diagnostic.contains("SECRET"));

        // An interpolated context could carry anything, so only its presence is reported.
        let interpolated = rootcause::report!(format!(
            "Unsupported SRP protocol selected: {}",
            "SECRET_PROTO"
        ))
        .into_dynamic();
        let withheld = isideload::auth_diagnostic(&interpolated);
        assert!(withheld.contains("interpolated detail withheld"));
        assert!(!withheld.contains("SECRET"));
    }

    #[test]
    /// A diagnostic stays something a person can paste into a message, however deep the chain.
    fn a_diagnostic_is_bounded() {
        let mut error: rootcause::Report =
            rootcause::report!("Failed to parse initial login response").into_dynamic();
        for _ in 0..200 {
            error = error
                .context("Failed to parse initial login response")
                .into_dynamic();
        }
        let diagnostic = isideload::auth_diagnostic(&error);
        assert!(diagnostic.len() <= 601, "{}", diagnostic.len());
    }

    #[test]
    /// A failure carries its diagnostic to the window, and signing out takes it away again — a
    /// diagnostic outliving the failure it describes would be attached to the wrong attempt.
    fn a_diagnostic_reaches_the_view_and_does_not_outlive_its_failure() {
        let manager = Accounts::default();
        manager.0.lock().unwrap().generation = "current".into();
        manager.0.lock().unwrap().task = Some(
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async { tokio::spawn(std::future::pending::<()>()).abort_handle() }),
        );
        manager.fail(
            "current",
            "Apple account authentication failed.",
            None,
            Some("Failed to parse initial login response".into()),
        );
        let view = manager.0.lock().unwrap().view.clone();
        assert!(view.stage == Stage::Failed);
        assert_eq!(
            view.diagnostic.as_deref(),
            Some("Failed to parse initial login response")
        );
        assert!(manager.sign_out().unwrap().diagnostic.is_none());
    }

    #[test]
    /// While a throttle hold is in force, further attempts are refused locally rather than sent —
    /// sending them is what extends the throttling.
    fn apple_throttling_blocks_further_attempts_until_it_lapses() {
        let manager = Accounts::default();
        manager.0.lock().unwrap().generation = "current".into();
        manager.0.lock().unwrap().task = Some(
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async { tokio::spawn(std::future::pending::<()>()).abort_handle() }),
        );
        manager.fail(
            "current",
            "Apple returned HTTP 429.",
            Some(Duration::from_secs(600)),
            None,
        );
        let view = manager
            .start("test@example.invalid".into(), "synthetic".into(), true)
            .unwrap();
        assert!(view.stage == Stage::Failed);
        assert!(view.message.contains("wait"));
        // No worker may be started while Apple is throttling.
        assert!(manager.0.lock().unwrap().task.is_none());

        // Signing out is local and must not clear Apple's throttle.
        manager.sign_out().unwrap();
        assert!(manager.0.lock().unwrap().retry_at.is_some());
        manager.0.lock().unwrap().retry_at = Some(Instant::now() - Duration::from_secs(1));
        assert!(manager.0.lock().unwrap().throttle_remaining().is_none());
    }
    #[test]
    /// Free versus paid is read from the membership Apple reports, and anything unrecognised is
    /// undetermined rather than guessed — that answer decides seven-day expiry and which
    /// capabilities survive re-signing.
    fn team_membership_decides_free_versus_paid_and_never_guesses() {
        use isideload::dev::teams::{DeveloperMembership, DeveloperTeam};
        let team = |kind: &str, memberships: Vec<&str>| DeveloperTeam {
            name: Some("Team".into()),
            team_id: "T8B3X5UL5W".into(),
            r#type: Some(kind.into()),
            status: Some("active".into()),
            memberships: memberships
                .into_iter()
                .map(|name| DeveloperMembership {
                    name: Some(name.into()),
                    status: Some("active".into()),
                    platform: None,
                })
                .collect(),
        };
        // Observed live: a free account's Personal Team reports this membership, which ends in
        // "Program" and must not be read as a paid one.
        assert_eq!(
            classify(&team("Individual", vec!["Xcode Free Provisioning Program"])),
            (
                Some(true),
                Some("Xcode Free Provisioning Program".to_string())
            )
        );
        // A Personal Team reporting no membership at all is free too.
        assert_eq!(classify(&team("Individual", vec![])).0, Some(true));
        // A paid membership names its program, whatever the team type says.
        assert_eq!(
            classify(&team("Individual", vec!["Apple Developer Program"])),
            (Some(false), Some("Apple Developer Program".into()))
        );
        assert_eq!(
            classify(&team(
                "Company/Organization",
                vec!["Apple Developer Enterprise Program"]
            ))
            .0,
            Some(false)
        );
        // An organisation team with no membership list is not evidence of a free team.
        assert_eq!(classify(&team("Company/Organization", vec![])).0, None);
        // Hostile or oversized labels are dropped rather than displayed.
        let hostile = classify(&team("Individual", vec!["bad\nname"]));
        assert_eq!(hostile, (Some(true), None));
    }
    #[test]
    /// The machine label sent to Apple is plain, bounded text and never empty, so a certificate is
    /// not named after whatever a person called their Mac.
    fn the_machine_label_is_plain_text_and_never_empty() {
        // Apple shows this beside the certificate; it must not carry control characters or grow
        // without bound, and it must survive an unset environment.
        unsafe { std::env::set_var("HOSTNAME", "Spiros\u{7} Mac\n") };
        assert_eq!(hostname(), "Spiros Mac");
        unsafe { std::env::set_var("HOSTNAME", "!!!") };
        assert_eq!(hostname(), "Orbiter Mac");
        unsafe { std::env::set_var("HOSTNAME", "x".repeat(200)) };
        assert_eq!(hostname().len(), 60);
    }
    #[tokio::test]
    /// Requesting a certificate without a live session is refused locally rather than attempted.
    async fn a_certificate_request_refuses_while_no_live_session_exists() {
        let manager = Accounts::default();
        assert!(
            manager
                .request_certificate(true)
                .await
                .is_err_and(|error| error.code == ErrorCode::AuthenticationRequired
                    && error.message.contains("Sign in"))
        );
        {
            let mut inner = manager.0.lock().unwrap();
            inner.view.stage = Stage::SignedIn;
            inner.view.selected_team = Some("T8B3X5UL5W".into());
        }
        // Again: a view claiming to be signed in is not a session.
        assert!(
            manager
                .request_certificate(true)
                .await
                .is_err_and(|error| error.code == ErrorCode::AuthenticationRequired
                    && error.message.contains("Sign in"))
        );
    }
    #[tokio::test]
    /// Registering a device without a live session is refused locally rather than attempted.
    async fn device_registration_refuses_while_no_live_session_exists() {
        // Device transport ID 1 is never contacted: every refusal happens before the identifier
        // is read. Ordering of the individual refusals is covered in provisioning::tests.
        let manager = Accounts::default();
        assert!(
            manager
                .register_device(1, true)
                .await
                .is_err_and(|error| error.code == ErrorCode::AuthenticationRequired
                    && error.message.contains("Sign in"))
        );
        {
            let mut inner = manager.0.lock().unwrap();
            inner.view.stage = Stage::SignedIn;
            inner.view.teams = vec![Team {
                id: "T8B3X5UL5W".into(),
                name: "Personal".into(),
                kind: Some("Individual".into()),
                free: Some(true),
                membership: None,
            }];
            inner.view.selected_team = Some("T8B3X5UL5W".into());
            inner.expires_at = Some(Instant::now() + SESSION_LIFETIME);
        }
        // A view that says signed in is not a session: the session itself is what authorises a
        // portal write, so a stale or forged view cannot reach Apple.
        assert!(
            manager
                .register_device(1, true)
                .await
                .is_err_and(|error| error.code == ErrorCode::AuthenticationRequired
                    && error.message.contains("Sign in"))
        );
    }
    #[test]
    /// A session past its lifetime is cleared on the next read, along with the team selection, so
    /// nothing acts on a session that has quietly stopped working.
    fn local_expiry_clears_account_and_team_selection() {
        let manager = Accounts::default();
        {
            let mut inner = manager.0.lock().unwrap();
            inner.view.stage = Stage::SignedIn;
            inner.view.account = Some("test@example.invalid".into());
            inner.view.selected_team = Some("OLDTEAM".into());
            inner.expires_at = Some(Instant::now() - Duration::from_secs(1));
        }
        let view = manager.status().unwrap();
        assert!(view.stage == Stage::SignedOut);
        assert!(view.account.is_none());
        assert!(view.selected_team.is_none());
        assert!(view.message.contains("expired"));
    }
    #[test]
    /// Consent and input are checked before anything is sent or stored, so a refused sign-in never
    /// puts a password anywhere.
    fn consent_and_input_validation_precede_any_worker_or_storage() {
        let manager = Accounts::default();
        assert!(
            manager
                .start("test@example.invalid".into(), "synthetic".into(), false)
                .is_err()
        );
        assert!(
            manager
                .start("test@example.invalid".into(), "".into(), true)
                .is_err()
        );
        assert!(manager.0.lock().unwrap().task.is_none());
        assert!(manager.status().unwrap().stage == Stage::SignedOut);
    }
    #[tokio::test]
    /// An answer to a superseded challenge, or an empty one, leaves the current prompt intact
    /// instead of consuming it — otherwise a stale reply would cancel a fresh code.
    async fn stale_and_invalid_verification_does_not_consume_prompt() {
        let (manager, rx) = pending();
        assert!(
            manager
                .answer("old".into(), Answer::Code("123456".into()))
                .is_err()
        );
        assert!(
            manager
                .answer("challenge".into(), Answer::Code("abc123".into()))
                .is_err()
        );
        assert!(manager.answer("challenge".into(), Answer::Sms(99)).is_err());
        assert!(manager.answer("challenge".into(), Answer::Sms(7)).is_ok());
        assert!(matches!(
            rx.await.unwrap(),
            TwoFactorCallbackResponse::SendSms(7)
        ));
        assert!(
            manager
                .answer("challenge".into(), Answer::Code("123456".into()))
                .is_err()
        );
    }
    #[test]
    /// Choosing a different team discards profiles obtained for the previous one, and losing the
    /// session discards them too.
    ///
    /// Identifiers are derived from the team, so those profiles describe identifiers the new team
    /// will never produce. Re-selecting the same team is not a change and leaves them alone.
    fn changing_the_team_invalidates_prepared_provisioning() {
        let profile = || crate::provisioning::ProfileOutcome {
            identifier: "com.example.app.team1".into(),
            expires: "2099-01-01T00:00:00Z".into(),
            expires_unix: 4_070_908_800,
            uuid: "synthetic".into(),
            encoded: vec![],
        };
        let mut inner = Inner {
            generation: "one".into(),
            view: View::default(),
            task: None,
            response: None,
            session: None,
            expires_at: None,
            retry_at: None,
            profiles: vec![profile()],
        };
        inner.view.selected_team = Some("TEAM1".into());

        inner.retarget("TEAM1".into());
        assert_eq!(inner.profiles.len(), 1, "the same team is not a change");

        inner.retarget("TEAM2".into());
        assert!(
            inner.profiles.is_empty(),
            "profiles for the previous team must not survive the change"
        );

        // And signing out discards them as well: they were obtained under that session.
        inner.profiles = vec![profile()];
        inner.clear("Signed out locally.");
        assert!(inner.profiles.is_empty());
        assert_eq!(inner.view.selected_team, None);
    }

    #[tokio::test]
    /// Signing out cancels a waiting two-factor prompt and clears the account and team, leaving no
    /// worker still expecting an answer.
    async fn sign_out_cancels_prompt_and_clears_account_team() {
        let (manager, rx) = pending();
        manager.0.lock().unwrap().view.selected_team = Some("OLDTEAM".into());
        let view = manager.sign_out().unwrap();
        assert!(rx.await.is_err());
        assert!(view.stage == Stage::SignedOut);
        assert!(view.selected_team.is_none());
        assert!(view.account.is_none());
        assert!(manager.select_team("OLDTEAM".into()).is_err());
        manager.fail("current", "Stale error must not reappear", None, None);
        assert!(manager.status().unwrap().stage == Stage::SignedOut);
    }
    #[tokio::test]
    /// Trusted phone numbers reach the view masked, and no raw upstream error is ever serialised
    /// into it.
    async fn trusted_numbers_are_masked_and_raw_errors_are_not_serialized() {
        let manager = Accounts::default();
        manager.0.lock().unwrap().generation = "current".into();
        let cloned = manager.clone();
        let task = tokio::spawn(async move {
            cloned
                .challenge(
                    "current",
                    TwoFactorCallbackParams {
                        last_error: Some("SECRET_SERVER_DETAIL".into()),
                        unknown: false,
                        sms: true,
                        selected_number_id: Some(1),
                        numbers: vec![isideload::auth::apple_account::TrustedNumber {
                            id: 1,
                            number_with_dial_code: "+1 555 123 1212".into(),
                            last_two_digits: "12".into(),
                            push_mode: "sms".into(),
                        }],
                    },
                )
                .await
        });
        tokio::task::yield_now().await;
        let json = serde_json::to_string(&manager.status().unwrap()).unwrap();
        assert!(!json.contains("SECRET_SERVER_DETAIL"));
        assert!(!json.contains("555"));
        assert!(json.contains("Phone ending in 12"));
        manager.sign_out().unwrap();
        assert!(matches!(
            task.await.unwrap(),
            TwoFactorCallbackResponse::Abort
        ));
    }
}
