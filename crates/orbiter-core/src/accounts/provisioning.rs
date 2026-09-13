//! Everything that reaches Apple's developer account, and the local signing that follows it.
//!
//! Split out of the session module deliberately. Authentication and a session's lifetime are one
//! responsibility; registering a device, spending a certificate slot, reserving app identifiers
//! and signing a build are another, and every function here is worth being able to find by what it
//! touches rather than by where it happened to be written.
//!
//! # What contacts Apple
//!
//! Every method in this file except [`Accounts::sign_ipa`] and [`Accounts::forget_signing_key`]
//! sends a request to Apple's developer service, and all but the listing ones **mutate** a remote
//! resource: a registered device, an issued certificate, a reserved app identifier. Signing is
//! purely local — it uses material already obtained — and forgetting the signing key touches only
//! this Mac's Keychain.
//!
//! # Acknowledgements
//!
//! Each remote mutation takes its own acknowledgement and refuses without one. A free personal
//! team's allowances are small and mostly irreversible — three devices, ten identifiers per seven
//! days, one certificate slot, and an identifier no other team can ever reuse — so nothing here
//! spends one on Orbiter's own initiative.
//!
//! # Retries
//!
//! There are none. A lost response does not prove a mutation failed, and a blind retry against a
//! team with one certificate slot is how that slot ends up deadlocked. A failure is reported with
//! what to check, and the person decides.

use super::{Accounts, Preparation, UNAVAILABLE, hostname};
use crate::domain::errors::{ErrorCode, OperationError, OperationResult};

impl Accounts {
    /// Register a connected iPhone on the selected team. The first Orbiter operation that writes
    /// to Apple: it requires an explicit acknowledgement and returns no device identifier.
    pub async fn register_device(
        &self,
        device_id: u32,
        acknowledged: bool,
    ) -> OperationResult<crate::provisioning::Outcome> {
        let _gate = self.1.try_lock().map_err(|_| {
            OperationError::operation_in_progress("Another account operation is already running.")
        })?;
        let (generation, mut developer, team, free) = {
            let mut inner = self
                .0
                .lock()
                .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
            inner.expire();
            let signed_in = inner.session.is_some();
            let selected = inner.view.selected_team.clone();
            if let Some(refusal) =
                crate::provisioning::refusal(acknowledged, selected.is_some(), signed_in)
            {
                return Err(refusal);
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
            let session = inner.session.as_ref().ok_or_else(|| {
                OperationError::new(
                    ErrorCode::AuthenticationRequired,
                    "Sign in before registering an iPhone.",
                )
            })?;
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
        let inner = self
            .0
            .lock()
            .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
        if inner.generation != generation {
            return Err("The account session changed during registration. Check the account at developer.apple.com before retrying.".into());
        }
        outcome.map_err(OperationError::from)
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
    ) -> OperationResult<Preparation> {
        let _gate = self.1.try_lock().map_err(|_| {
            OperationError::operation_in_progress("Another account operation is already running.")
        })?;
        let (generation, mut developer, team_id, free) = {
            let mut inner = self
                .0
                .lock()
                .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
            inner.expire();
            let signed_in = inner.session.is_some();
            let selected = inner.view.selected_team.clone();
            if let Some(refusal) =
                crate::provisioning::app_id_refusal(acknowledged, selected.is_some(), signed_in)
            {
                return Err(refusal);
            }
            let team_id = selected.ok_or("Select the signing team to provision on.")?;
            let free = inner
                .view
                .teams
                .iter()
                .find(|candidate| candidate.id == team_id)
                .and_then(|candidate| candidate.free)
                .unwrap_or(true);
            let session = inner.session.as_ref().ok_or_else(|| {
                OperationError::new(
                    ErrorCode::AuthenticationRequired,
                    "Sign in before provisioning.",
                )
            })?;
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
        let mut inner = self
            .0
            .lock()
            .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
        if inner.generation != generation {
            return Err("The account session changed during provisioning. Check developer.apple.com before retrying.".into());
        }
        inner.profiles = profiles.clone();
        Ok(Preparation {
            app_ids,
            profiles,
            plan,
        })
    }
    /// Withdraw the selected team's development certificates at Apple. Explicit, acknowledged,
    /// never automatic: every app already signed with them stops launching.
    pub async fn withdraw_certificates(&self, acknowledged: bool) -> OperationResult<String> {
        let _gate = self.1.try_lock().map_err(|_| {
            OperationError::operation_in_progress("Another account operation is already running.")
        })?;
        let (mut developer, team_id) = {
            let mut inner = self
                .0
                .lock()
                .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
            inner.expire();
            let team_id = inner
                .view
                .selected_team
                .clone()
                .ok_or("Select the signing team whose certificate this applies to.")?;
            let session = inner.session.as_ref().ok_or("Sign in first.")?;
            (session.developer.clone(), team_id)
        };
        let message =
            crate::certificates::withdraw_all(&mut developer, &team_id, acknowledged).await?;
        // This session's identity, if any, rests on a certificate that no longer exists.
        if let Ok(mut inner) = self.0.lock() {
            if let Some(session) = inner.session.as_mut() {
                session.identity = None;
            }
            // The profiles were fetched under a certificate that has just been withdrawn.
            inner.profiles.clear();
        }
        Ok(message)
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
        // Already cleaned by `signer::marker`; `None` leaves every display name alone.
        marker: Option<String>,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        progress: impl FnMut(crate::signer::Progress) + Send + 'static,
    ) -> Result<crate::signer::Signed, String> {
        let _gate = self.1.try_lock().map_err(|_| {
            OperationError::operation_in_progress("Another account operation is already running.")
        })?;
        let (team_id, free, identity, profiles) = {
            let mut inner = self
                .0
                .lock()
                .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
            inner.expire();
            let team_id = inner.view.selected_team.clone().ok_or_else(|| {
                OperationError::new(
                    ErrorCode::AuthenticationRequired,
                    "Select the signing team before signing.",
                )
            })?;
            let free = inner
                .view
                .teams
                .iter()
                .find(|candidate| candidate.id == team_id)
                .and_then(|candidate| candidate.free)
                .unwrap_or(true);
            let session = inner.session.as_ref().ok_or_else(|| {
                OperationError::new(ErrorCode::AuthenticationRequired, "Sign in before signing.")
            })?;
            let identity = session.identity.clone().ok_or_else(|| {
                OperationError::new(
                    ErrorCode::AuthenticationRequired,
                    "Get a signing certificate before signing.",
                )
            })?;
            (team_id, free, identity, inner.profiles.clone())
        };
        let team_tag = crate::renewal::tag(&team_id);
        // Signing is local and CPU-bound: it reads and writes a whole app bundle and computes
        // hashes over every file, so it never runs on the async runtime's threads.
        let mut signed = tokio::task::spawn_blocking(move || {
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
                marker.as_deref(),
                &cancel,
                progress,
            )
        })
        .await
        .map_err(|_| "Signing stopped unexpectedly.".to_string())??;
        // The team the build was signed for, so the library can tell a countdown about this
        // build apart from one about a build signed for somebody else.
        signed.team_tag = Some(team_tag);
        Ok(signed)
    }
    /// Reuse or obtain this session's development certificate for the selected team.
    pub async fn request_certificate(
        &self,
        acknowledged: bool,
    ) -> OperationResult<crate::certificates::Outcome> {
        let _gate = self.1.try_lock().map_err(|_| {
            OperationError::operation_in_progress("Another account operation is already running.")
        })?;
        let (generation, mut developer, team, existing, stored) = {
            let mut inner = self
                .0
                .lock()
                .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
            inner.expire();
            let signed_in = inner.session.is_some();
            let selected = inner.view.selected_team.clone();
            if let Some(refusal) =
                crate::certificates::refusal(acknowledged, selected.is_some(), signed_in)
            {
                return Err(refusal);
            }
            let team = selected.ok_or("Select the signing team the certificate belongs to.")?;
            let email = inner.view.account.clone().unwrap_or_default();
            let session = inner.session.as_ref().ok_or_else(|| {
                OperationError::new(
                    ErrorCode::AuthenticationRequired,
                    "Sign in before requesting a signing certificate.",
                )
            })?;
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
        let mut inner = self
            .0
            .lock()
            .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
        if inner.generation != generation {
            return Err("The account session changed while the certificate was issued. Check developer.apple.com before requesting another.".into());
        }
        if let Some(session) = inner.session.as_mut() {
            session.identity = Some(identity);
        }
        Ok(outcome)
    }
    /// Remove this account and team's stored signing key from this Mac's Keychain.
    pub async fn forget_signing_key(&self) -> OperationResult<String> {
        let stored = {
            let mut inner = self
                .0
                .lock()
                .map_err(|_| OperationError::new(ErrorCode::Internal, UNAVAILABLE))?;
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
}
