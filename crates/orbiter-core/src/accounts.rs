//! Apple authentication only: no certificate, profile, device, or app mutations.
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
    /// Provisioning profiles fetched for this session's plan, held for the signer.
    profiles: Vec<crate::provisioning::ProfileOutcome>,
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
}
impl Failure {
    fn new(message: String, error: &rootcause::Report) -> Self {
        Self {
            retry_after: isideload::auth_throttle_delay(error),
            message,
        }
    }
    fn plain(message: String) -> Self {
        Self {
            message,
            retry_after: None,
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
}
#[derive(Clone, Default)]
pub struct Accounts(Arc<Mutex<Inner>>, Arc<tokio::sync::Mutex<()>>);

impl Inner {
    fn clear(&mut self, message: &str) {
        self.generation = Uuid::new_v4().to_string();
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.response = None;
        self.session = None;
        self.expires_at = None;
        self.view = View {
            message: message.into(),
            ..View::default()
        };
    }
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
    pub fn status(&self) -> Result<View, String> {
        let mut inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
        inner.expire();
        Ok(inner.view.clone())
    }
    pub fn sign_out(&self) -> Result<View, String> {
        let mut inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
        inner.clear("Signed out locally. macOS manages its own authentication support data.");
        Ok(inner.view.clone())
    }
    pub fn start(&self, email: String, password: String, consent: bool) -> Result<View, String> {
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
        let mut inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
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
                Ok(Err(failure)) => {
                    manager.fail(&generation, &failure.message, failure.retry_after)
                }
                Err(_) => manager.fail(
                    &generation,
                    "Sign-in timed out. Check your connection and start again.",
                    None,
                ),
            }
        });
        inner.task = Some(task.abort_handle());
        Ok(inner.view.clone())
    }
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
    pub fn answer(&self, challenge_id: String, answer: Answer) -> Result<View, String> {
        let mut inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
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
                profiles: Vec::new(),
            });
            inner.expires_at = Some(Instant::now() + SESSION_LIFETIME);
            inner.view = View {
                stage: Stage::SignedIn,
                account: Some(account),
                teams,
                message:
                    "Signed in. Select a signing team explicitly. Re-signing is not enabled yet."
                        .into(),
                ..View::default()
            };
        }
    }
    fn fail(&self, generation: &str, message: &str, retry_after: Option<Duration>) {
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
                ..View::default()
            };
        }
    }
    pub fn select_team(&self, id: String) -> Result<View, String> {
        let mut inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
        inner.expire();
        if inner.session.is_none() || !inner.view.teams.iter().any(|team| team.id == id) {
            return Err("Select a team returned by the current Apple account session.".into());
        }
        inner.view.selected_team = Some(id);
        Ok(inner.view.clone())
    }
    /// Register a connected iPhone on the selected team. The first Orbiter operation that writes
    /// to Apple: it requires an explicit acknowledgement and returns no device identifier.
    pub async fn register_device(
        &self,
        device_id: u32,
        acknowledged: bool,
    ) -> Result<crate::provisioning::Outcome, String> {
        let _gate = self
            .1
            .try_lock()
            .map_err(|_| "Another account operation is already running.")?;
        let (generation, mut developer, team, free) = {
            let mut inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
            inner.expire();
            let signed_in = inner.session.is_some();
            let selected = inner.view.selected_team.clone();
            if let Some(refusal) =
                crate::provisioning::refusal(acknowledged, selected.is_some(), signed_in)
            {
                return Err(refusal.into());
            }
            let team =
                selected.ok_or("Select the signing team that should register this iPhone.")?;
            let free = inner
                .view
                .teams
                .iter()
                .find(|candidate| candidate.id == team)
                .and_then(|candidate| candidate.free)
                // An unestablished membership is treated as the stricter free allowance.
                .unwrap_or(true);
            let session = inner
                .session
                .as_ref()
                .ok_or("Sign in before registering an iPhone.")?;
            (
                inner.generation.clone(),
                session.developer.clone(),
                team,
                free,
            )
        };
        // The identifier is read here and handed straight to Apple; it never reaches the view.
        let (udid, name) = crate::installation::verified_identity(device_id).await?;
        let outcome =
            crate::provisioning::register(&mut developer, &team, &udid, &name, free).await;
        let inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
        if inner.generation != generation {
            return Err("The account session changed during registration. Check the account at developer.apple.com before retrying.".into());
        }
        outcome
    }
    /// Register the plan's identifiers on the team and fetch their provisioning profiles.
    ///
    /// This is where Apple, not Orbiter, answers which capabilities the team may create: the
    /// returned App IDs report what Apple actually enabled.
    pub async fn prepare_provisioning(
        &self,
        path: std::path::PathBuf,
        acknowledged: bool,
        watch: crate::plan::WatchChoice,
    ) -> Result<Preparation, String> {
        let _gate = self
            .1
            .try_lock()
            .map_err(|_| "Another account operation is already running.")?;
        let (generation, mut developer, team_id, free) = {
            let mut inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
            inner.expire();
            let signed_in = inner.session.is_some();
            let selected = inner.view.selected_team.clone();
            if let Some(refusal) =
                crate::provisioning::app_id_refusal(acknowledged, selected.is_some(), signed_in)
            {
                return Err(refusal.into());
            }
            let team_id = selected.ok_or("Select the signing team to provision on.")?;
            let free = inner
                .view
                .teams
                .iter()
                .find(|candidate| candidate.id == team_id)
                .and_then(|candidate| candidate.free)
                .unwrap_or(true);
            let session = inner
                .session
                .as_ref()
                .ok_or("Sign in before provisioning.")?;
            (
                inner.generation.clone(),
                session.developer.clone(),
                team_id,
                free,
            )
        };
        let report = tokio::task::spawn_blocking(move || {
            crate::inspect(&path, &std::sync::atomic::AtomicBool::new(false), |_| {})
        })
        .await
        .map_err(|_| "Reading the IPA stopped.".to_string())?
        .map_err(|error| error.to_string())?;
        let plan = crate::plan::build(
            &report,
            &crate::plan::Target {
                team_id: team_id.clone(),
                kind: if free {
                    crate::plan::TeamKind::Personal
                } else {
                    crate::plan::TeamKind::Paid
                },
                watch,
            },
        );
        if !plan.blockers.is_empty() {
            // Nothing is written while the plan cannot be carried out.
            return Ok(Preparation {
                app_ids: vec![],
                profiles: vec![],
                plan,
            });
        }
        let mut app_ids = Vec::new();
        let mut profiles = Vec::new();
        for bundle in plan.bundles.iter().filter(|bundle| bundle.consumes_app_id) {
            let (app_id, outcome) = crate::provisioning::ensure_app_id(
                &mut developer,
                &team_id,
                &bundle.new_identifier,
                &bundle.name,
            )
            .await?;
            app_ids.push(outcome);
            profiles
                .push(crate::provisioning::fetch_profile(&mut developer, &team_id, &app_id).await?);
        }
        let mut inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
        if inner.generation != generation {
            return Err("The account session changed during provisioning. Check developer.apple.com before retrying.".into());
        }
        if let Some(session) = inner.session.as_mut() {
            session.profiles = profiles.clone();
        }
        Ok(Preparation {
            app_ids,
            profiles,
            plan,
        })
    }
    /// Sign the IPA with this session's certificate and the profiles Apple returned.
    ///
    /// The plan is rebuilt from the same inputs rather than remembered, so the build that is
    /// signed is the build that was reviewed: a different IPA, team, or Watch choice produces a
    /// different plan, and a plan whose profiles were never prepared is refused.
    pub async fn sign_ipa(
        &self,
        path: std::path::PathBuf,
        out_dir: std::path::PathBuf,
        watch: crate::plan::WatchChoice,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<crate::signer::Signed, String> {
        let _gate = self
            .1
            .try_lock()
            .map_err(|_| "Another account operation is already running.")?;
        let (team_id, free, identity, profiles) = {
            let mut inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
            inner.expire();
            let team_id = inner
                .view
                .selected_team
                .clone()
                .ok_or("Select the signing team before signing.")?;
            let free = inner
                .view
                .teams
                .iter()
                .find(|candidate| candidate.id == team_id)
                .and_then(|candidate| candidate.free)
                .unwrap_or(true);
            let session = inner.session.as_ref().ok_or("Sign in before signing.")?;
            let identity = session
                .identity
                .clone()
                .ok_or("Get a signing certificate before signing.")?;
            (team_id, free, identity, session.profiles.clone())
        };
        // Signing is local and CPU-bound: it reads and writes a whole app bundle and computes
        // hashes over every file, so it never runs on the async runtime's threads.
        tokio::task::spawn_blocking(move || {
            let report =
                crate::inspect(&path, &cancel, |_| {}).map_err(|error| error.to_string())?;
            let plan = crate::plan::build(
                &report,
                &crate::plan::Target {
                    team_id,
                    kind: if free {
                        crate::plan::TeamKind::Personal
                    } else {
                        crate::plan::TeamKind::Paid
                    },
                    watch,
                },
            );
            crate::signer::sign(
                &path,
                &out_dir,
                &plan,
                &profiles,
                &identity,
                &cancel,
                |_| {},
            )
        })
        .await
        .map_err(|_| "Signing stopped unexpectedly.".to_string())?
    }
    /// Reuse or obtain this session's development certificate for the selected team.
    pub async fn request_certificate(
        &self,
        acknowledged: bool,
    ) -> Result<crate::certificates::Outcome, String> {
        let _gate = self
            .1
            .try_lock()
            .map_err(|_| "Another account operation is already running.")?;
        let (generation, mut developer, team, existing, stored) = {
            let mut inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
            inner.expire();
            let signed_in = inner.session.is_some();
            let selected = inner.view.selected_team.clone();
            if let Some(refusal) =
                crate::certificates::refusal(acknowledged, selected.is_some(), signed_in)
            {
                return Err(refusal.into());
            }
            let team = selected.ok_or("Select the signing team the certificate belongs to.")?;
            let email = inner.view.account.clone().unwrap_or_default();
            let session = inner
                .session
                .as_ref()
                .ok_or("Sign in before requesting a signing certificate.")?;
            (
                inner.generation.clone(),
                session.developer.clone(),
                team.clone(),
                session
                    .identity
                    .as_ref()
                    .map(|identity| identity.key.clone()),
                crate::keychain::account(&email, &team),
            )
        };
        // The key persists in this Mac's Keychain, so a restart reuses the certificate Apple
        // already issued instead of spending another of the team's few certificate slots.
        let key = match existing {
            Some(key) => key,
            None => {
                let account = stored.clone();
                let loaded = tokio::task::spawn_blocking(move || crate::keychain::load(&account))
                    .await
                    .map_err(|_| "Reading the stored signing key stopped.".to_string())??;
                match loaded.as_deref().map(crate::certificates::decode_key) {
                    Some(Ok(key)) => key,
                    // A key that cannot be decoded is replaced rather than blocking the request.
                    _ => {
                        let key = tokio::task::spawn_blocking(crate::certificates::generate_key)
                            .await
                            .map_err(|_| "Signing key generation stopped.".to_string())??;
                        let encoded = crate::certificates::encode_key(&key)?;
                        let account = stored.clone();
                        tokio::task::spawn_blocking(move || {
                            crate::keychain::store(&account, &encoded)
                        })
                        .await
                        .map_err(|_| "Storing the signing key stopped.".to_string())??;
                        key
                    }
                }
            }
        };
        let machine = hostname();
        let (identity, outcome) =
            crate::certificates::ensure(&mut developer, &team, &machine, &key).await?;
        let mut inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
        if inner.generation != generation {
            return Err("The account session changed while the certificate was issued. Check developer.apple.com before requesting another.".into());
        }
        if let Some(session) = inner.session.as_mut() {
            session.identity = Some(identity);
        }
        Ok(outcome)
    }
    /// Remove this account and team's stored signing key from this Mac's Keychain.
    pub async fn forget_signing_key(&self) -> Result<String, String> {
        let stored = {
            let mut inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
            inner.expire();
            let team = inner
                .view
                .selected_team
                .clone()
                .ok_or("Select the team whose stored signing key should be removed.")?;
            let email = inner.view.account.clone().unwrap_or_default();
            if let Some(session) = inner.session.as_mut() {
                session.identity = None;
            }
            crate::keychain::account(&email, &team)
        };
        tokio::task::spawn_blocking(move || crate::keychain::forget(&stored))
            .await
            .map_err(|_| "Removing the stored signing key stopped.".to_string())??;
        Ok("The stored signing key was removed from this Mac's Keychain. The certificate Apple issued for it still exists: revoke it at developer.apple.com if it is no longer wanted.".into())
    }
    pub async fn refresh_teams(&self) -> Result<View, String> {
        let _gate = self
            .1
            .try_lock()
            .map_err(|_| "Team refresh is already running.")?;
        let (generation, mut developer) = {
            let mut inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
            inner.expire();
            let session = inner
                .session
                .as_ref()
                .ok_or("Sign in before refreshing teams.")?;
            (inner.generation.clone(), session.developer.clone())
        };
        let result =
            tokio::time::timeout(Duration::from_secs(60), read_teams(&mut developer)).await;
        let mut inner = self.0.lock().map_err(|_| UNAVAILABLE)?;
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
fn hostname() -> String {
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
    fn drop(&mut self) {
        self.0.fail(
            &self.1,
            "Authentication worker stopped. Start sign-in again.",
            None,
        );
    }
}
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
mod tests {
    use super::*;
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
    fn unsupported_additional_step_is_named_without_echoing_apple_text() {
        let error = rootcause::report!(isideload::SideloadError::UnsupportedStep(
            "SECRET_STEP_NAME".into()
        ))
        .into_dynamic();
        let message = isideload::redacted_auth_error(&error);
        assert!(message.contains("additional sign-in step"));
        assert!(!message.contains("SECRET"));
        assert!(!isideload::auth_error_is_inconclusive(&error));
    }
    #[test]
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
    async fn a_certificate_request_refuses_while_no_live_session_exists() {
        let manager = Accounts::default();
        assert!(
            manager
                .request_certificate(true)
                .await
                .is_err_and(|m| m.contains("Sign in"))
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
                .is_err_and(|m| m.contains("Sign in"))
        );
    }
    #[tokio::test]
    async fn device_registration_refuses_while_no_live_session_exists() {
        // Device transport ID 1 is never contacted: every refusal happens before the identifier
        // is read. Ordering of the individual refusals is covered in provisioning::tests.
        let manager = Accounts::default();
        assert!(
            manager
                .register_device(1, true)
                .await
                .is_err_and(|m| m.contains("Sign in"))
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
                .is_err_and(|m| m.contains("Sign in"))
        );
    }
    #[test]
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
    #[tokio::test]
    async fn sign_out_cancels_prompt_and_clears_account_team() {
        let (manager, rx) = pending();
        manager.0.lock().unwrap().view.selected_team = Some("OLDTEAM".into());
        let view = manager.sign_out().unwrap();
        assert!(rx.await.is_err());
        assert!(view.stage == Stage::SignedOut);
        assert!(view.selected_team.is_none());
        assert!(view.account.is_none());
        assert!(manager.select_team("OLDTEAM".into()).is_err());
        manager.fail("current", "Stale error must not reappear", None);
        assert!(manager.status().unwrap().stage == Stage::SignedOut);
    }
    #[tokio::test]
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
