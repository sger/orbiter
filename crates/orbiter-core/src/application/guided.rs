//! Reviewed preparation built on the existing account and signing services.
//! Plans are short-lived, single-use capabilities. Private device identity and account generation
//! remain in memory; the library lease spans assessment, portal writes, signing, and retention.
use super::{
    ProgressSink,
    signing::{Retained, SigningService},
};
use crate::{
    domain::{
        errors::{ErrorCode, OperationError, OperationResult},
        identifiers::ArtifactId,
    },
    plan::{Plan, WatchChoice},
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Actions whose consequences require separate acknowledgements on one review screen.
#[derive(Clone, Debug, Serialize)]
pub struct PreparationReview {
    pub token: String,
    pub artifact_id: ArtifactId,
    pub device_name: String,
    pub registration: bool,
    pub certificate: bool,
    pub provisioning: bool,
    pub plan: Plan,
    pub marker: String,
}

/// Explicit consent for each class of remote mutation, not a generic bypass flag.
#[derive(Deserialize)]
pub struct Consents {
    pub registration: bool,
    pub certificate: bool,
    pub provisioning: bool,
}

impl Consents {
    /// Refuse any action whose displayed consequences have not been acknowledged.
    fn validate(&self, review: &PreparationReview) -> OperationResult<()> {
        if (review.registration && !self.registration)
            || (review.certificate && !self.certificate)
            || !self.provisioning
        {
            return Err(OperationError::acknowledgement_required(
                "Acknowledge each required preparation action before continuing.",
            ));
        }
        Ok(())
    }
}

/// The backend-owned phase of a preparation sequence.
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparationStage {
    #[default]
    Idle,
    Checking,
    Registration,
    Certificate,
    Provisioning,
    Signing,
    Complete,
    Failed,
}

/// A progress snapshot survives navigation and channel disconnection in the current process.
#[derive(Clone, Serialize, Default)]
pub struct PreparationStatus {
    pub stage: PreparationStage,
    pub message: String,
    pub completed: Vec<String>,
    pub artifact_id: Option<ArtifactId>,
    pub source_artifact_id: Option<ArtifactId>,
}

/// Private binding for one reviewed plan. No Debug or Serialize: it contains device identity.
struct Pending {
    review: PreparationReview,
    generation: String,
    udid: String,
    device_id: u32,
    watch: WatchChoice,
    /// Absolute paths of libraries to inject, carried from review so the acknowledged plan and the
    /// signed build agree on exactly what is added.
    dylibs: Vec<std::path::PathBuf>,
    created: Instant,
    _lease: crate::library::Lease,
}

impl Pending {
    /// Validate time, account generation, team, and blockers before any remote mutation.
    fn validate_context(&self, generation: &str, team: &str) -> OperationResult<()> {
        if self.created.elapsed() > Duration::from_secs(600)
            || generation != self.generation
            || team != self.review.plan.team_id
            || !self.review.plan.blockers.is_empty()
        {
            Err(stale())
        } else {
            Ok(())
        }
    }
}

/// Shared state of the signing service, not an independent installation/job queue.
#[derive(Default)]
pub(super) struct State {
    pending: Mutex<Option<Pending>>,
    status: Mutex<PreparationStatus>,
}

/// A conservative error used whenever the reviewed inputs or resource requirements changed.
fn stale() -> OperationError {
    OperationError::new(
        ErrorCode::ReviewStale,
        "Preparation changed or expired. Review the required actions again.",
    )
}

impl SigningService {
    /// Inspect a managed original and list account resources without remote mutations.
    /// Fails for unsupported teams, inaccessible devices, blocked packages, or unreadable resources.
    pub async fn review_preparation(
        &self,
        artifact_id: &ArtifactId,
        device_id: u32,
        watch: WatchChoice,
        marker: &str,
        dylibs: Vec<std::path::PathBuf>,
    ) -> OperationResult<PreparationReview> {
        let _gate = self.accounts.operation()?;
        self.discard_preparation(None)?;
        let (artifact, path, lease) = self.source(artifact_id).await?;
        let checked = crate::installation::prepare(path.clone(), device_id).await?;
        if checked.review.readiness == crate::installation::Readiness::Blocked {
            return Err(OperationError::new(
                ErrorCode::InvalidRequest,
                checked.review.blockers.join(" "),
            ));
        }
        let device_name = checked.review.device_name.clone();
        drop(checked);
        let (udid, _) = crate::installation::verified_identity(device_id).await?;
        let (generation, mut target) = self.accounts.signing_context()?;
        target.watch = watch;
        target.injected_dylibs = dylibs
            .iter()
            .filter_map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect();
        let report = tokio::task::spawn_blocking(move || {
            crate::inspect(&path, &std::sync::atomic::AtomicBool::new(false), |_| {})
        })
        .await
        .map_err(|_| OperationError::internal("Inspection worker stopped."))?
        .map_err(|_| OperationError::internal("Cannot inspect the saved original."))?;
        let plan = crate::plan::build(&report, &target);
        // Watch/capability blockers are shown without spending any portal resources.
        let (registration, certificate) = if plan.blockers.is_empty() {
            self.accounts.signing_resources(&udid).await?
        } else {
            (false, false)
        };
        let provisioning = certificate || !self.accounts.profiles_ready(&plan, &udid)?;
        let review = PreparationReview {
            token: uuid::Uuid::new_v4().to_string(),
            artifact_id: artifact.id,
            device_name,
            registration,
            certificate,
            provisioning,
            plan,
            marker: crate::signer::marker(marker).unwrap_or_default(),
        };
        *self
            .guided
            .pending
            .lock()
            .map_err(|_| OperationError::internal("Preparation state unavailable."))? =
            Some(Pending {
                review: review.clone(),
                generation,
                udid,
                device_id,
                watch,
                dylibs,
                created: Instant::now(),
                _lease: lease,
            });
        Ok(review)
    }

    /// Release an unused plan's lease; a stale client cannot discard a newer token.
    pub fn discard_preparation(&self, token: Option<&str>) -> OperationResult<()> {
        let mut pending = self
            .guided
            .pending
            .lock()
            .map_err(|_| OperationError::internal("Preparation state unavailable."))?;
        if token.is_none()
            || pending
                .as_ref()
                .is_some_and(|p| Some(p.review.token.as_str()) == token)
        {
            pending.take();
        }
        Ok(())
    }

    /// Read the latest preparation progress without polling Apple or changing any resource.
    pub fn preparation_status(&self) -> OperationResult<PreparationStatus> {
        self.guided
            .status
            .lock()
            .map(|status| status.clone())
            .map_err(|_| OperationError::internal("Preparation status unavailable."))
    }

    /// Record a stage and emit it to an optional window; losing the subscriber never stops work.
    fn preparation_progress(
        &self,
        stage: PreparationStage,
        message: &str,
        completed: &[String],
        artifact_id: Option<ArtifactId>,
        sink: &dyn ProgressSink<PreparationStatus>,
    ) {
        let status = PreparationStatus {
            stage,
            message: message.into(),
            completed: completed.to_vec(),
            artifact_id,
            source_artifact_id: self
                .guided
                .status
                .lock()
                .ok()
                .and_then(|status| status.source_artifact_id.clone()),
        };
        if let Ok(mut current) = self.guided.status.lock() {
            *current = status.clone();
        }
        sink.send(status);
    }

    /// Execute only the acknowledged plan under one account reservation. Never retries mutations.
    /// Re-verifies the phone and resources, consumes the token, and retains the exact signed output.
    /// Partial completion remains in status; an explicit fresh review is required after failure.
    pub async fn execute_preparation(
        &self,
        token: &str,
        consents: Consents,
        sink: Arc<dyn ProgressSink<PreparationStatus>>,
    ) -> OperationResult<Retained> {
        let _gate = self.accounts.operation()?;
        let pending = {
            let mut slot = self
                .guided
                .pending
                .lock()
                .map_err(|_| OperationError::internal("Preparation state unavailable."))?;
            let pending = slot.as_ref().ok_or_else(stale)?;
            if pending.review.token != token {
                return Err(stale());
            }
            consents.validate(&pending.review)?;
            slot.take().ok_or_else(stale)?
        };
        *self
            .guided
            .status
            .lock()
            .map_err(|_| OperationError::internal("Preparation status unavailable."))? =
            PreparationStatus {
                source_artifact_id: Some(pending.review.artifact_id.clone()),
                ..PreparationStatus::default()
            };
        let mut completed = Vec::new();
        self.preparation_progress(
            PreparationStage::Checking,
            "Verifying the reviewed inputs…",
            &completed,
            None,
            sink.as_ref(),
        );
        let result = async {
            let (generation, target) = self.accounts.signing_context()?;
            pending.validate_context(&generation, &target.team_id)?;
            // By identity rather than by the number the review was prepared with: that number
            // changes when the phone moves between a cable and Wi-Fi, and the phone has not.
            crate::installation::confirm_identity(&pending.udid).await?;
            let udid = pending.udid.clone();
            // Re-hash before any portal mutation, retaining the original lease throughout.
            let (_, _, _verified) = self.source(&pending.review.artifact_id).await?;
            let (registration, certificate) = self.accounts.signing_resources(&udid).await?;
            let provisioning =
                certificate || !self.accounts.profiles_ready(&pending.review.plan, &udid)?;
            if (registration && !pending.review.registration)
                || (certificate && !pending.review.certificate)
                || (provisioning && !pending.review.provisioning)
            {
                return Err(stale());
            }
            if registration {
                self.preparation_progress(
                    PreparationStage::Registration,
                    "Registering this iPhone…",
                    &completed,
                    None,
                    sink.as_ref(),
                );
                self.accounts
                    .register_device_under_gate(pending.device_id, consents.registration)
                    .await?;
                completed.push("iPhone registered".into());
            } else {
                completed.push("Registered iPhone reused".into());
            }
            if certificate {
                self.preparation_progress(
                    PreparationStage::Certificate,
                    "Obtaining the development certificate…",
                    &completed,
                    None,
                    sink.as_ref(),
                );
                self.accounts
                    .request_certificate_under_gate(consents.certificate)
                    .await?;
                completed.push("Development certificate obtained".into());
            } else {
                completed.push("Existing signing certificate reused".into());
            }
            if provisioning {
                self.preparation_progress(
                    PreparationStage::Provisioning,
                    "Preparing identifiers and profiles…",
                    &completed,
                    None,
                    sink.as_ref(),
                );
                let (_, path, _lease) = self.source(&pending.review.artifact_id).await?;
                let prepared = self
                    .accounts
                    .prepare_provisioning_under_gate(
                        path,
                        consents.provisioning,
                        pending.watch,
                        pending.dylibs.clone(),
                    )
                    .await?;
                if !prepared.plan.blockers.is_empty() {
                    return Err(OperationError::new(
                        ErrorCode::InvalidRequest,
                        prepared.plan.blockers.join(" "),
                    ));
                }
                completed.push("Profiles prepared".into());
            } else {
                completed.push("Existing profiles reused".into());
            }
            self.preparation_progress(
                PreparationStage::Signing,
                "Signing the saved original…",
                &completed,
                None,
                sink.as_ref(),
            );
            let service = self.clone();
            let progress_sink = sink.clone();
            let done = completed.clone();
            let progress = Arc::new(move |step: crate::signer::Progress| {
                service.preparation_progress(
                    PreparationStage::Signing,
                    &format!("{} · {} / {}", step.stage, step.done, step.total),
                    &done,
                    None,
                    progress_sink.as_ref(),
                );
            });
            self.sign_under_gate(
                &pending.review.artifact_id,
                pending.watch,
                &pending.review.marker,
                pending.dylibs.clone(),
                progress,
            )
            .await
        }
        .await;
        match &result {
            Ok(retained) => self.preparation_progress(
                PreparationStage::Complete,
                "Signed build saved. Review installation next.",
                &completed,
                Some(retained.artifact.id.clone()),
                sink.as_ref(),
            ),
            Err(error) => self.preparation_progress(
                PreparationStage::Failed,
                &error.message,
                &completed,
                None,
                sink.as_ref(),
            ),
        }
        result
    }
}

#[cfg(test)]
/// Tests the capability boundary without credentials or a connected phone.
mod tests {
    use super::*;
    use crate::{
        accounts::Accounts,
        library::Library,
        plan::{Target, TeamKind},
    };

    /// Create a real managed original and a reviewed plan with a live artifact lease.
    fn pending(library: &Library) -> Pending {
        let file = crate::library::tests_support::fixture("guided");
        let imported = library.import(file.path()).unwrap();
        let opened = library.open(&imported.artifact_id).unwrap();
        let (_, _, lease) = library.pin(&imported.artifact_id).unwrap();
        let plan = crate::plan::build(
            &opened.report,
            &Target {
                team_id: "TEAM".into(),
                kind: TeamKind::Personal,
                watch: WatchChoice::Remove,
                injected_dylibs: Vec::new(),
            },
        );
        Pending {
            review: PreparationReview {
                token: "token".into(),
                artifact_id: imported.artifact_id,
                device_name: "Test phone".into(),
                registration: true,
                certificate: true,
                provisioning: true,
                plan,
                marker: "test".into(),
            },
            generation: "session".into(),
            udid: "private-identity".into(),
            device_id: 1,
            watch: WatchChoice::Remove,
            dylibs: Vec::new(),
            created: Instant::now(),
            _lease: lease,
        }
    }

    #[test]
    /// Each remote action needs its own acknowledgement; reused resources need none.
    fn acknowledgements_match_required_actions() {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::new(dir.path().join("library"));
        let mut pending = pending(&library);
        for consents in [
            Consents {
                registration: false,
                certificate: true,
                provisioning: true,
            },
            Consents {
                registration: true,
                certificate: false,
                provisioning: true,
            },
            Consents {
                registration: true,
                certificate: true,
                provisioning: false,
            },
        ] {
            assert_eq!(
                consents.validate(&pending.review).unwrap_err().code,
                ErrorCode::AcknowledgementRequired
            );
        }
        pending.review.registration = false;
        pending.review.certificate = false;
        assert!(
            Consents {
                registration: false,
                certificate: false,
                provisioning: true
            }
            .validate(&pending.review)
            .is_ok()
        );
    }

    #[test]
    /// Changed accounts, teams, expired reviews and blocked plans cannot authorize mutations.
    fn stale_context_is_refused_and_private_identity_never_serializes() {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::new(dir.path().join("library"));
        let mut pending = pending(&library);
        pending.review.plan.blockers.clear();
        assert!(pending.validate_context("session", "TEAM").is_ok());
        assert!(pending.validate_context("another-session", "TEAM").is_err());
        assert!(pending.validate_context("session", "OTHER").is_err());
        pending.created = Instant::now() - Duration::from_secs(601);
        assert!(pending.validate_context("session", "TEAM").is_err());
        pending.created = Instant::now();
        pending
            .review
            .plan
            .blockers
            .push("Watch choice required".into());
        assert!(pending.validate_context("session", "TEAM").is_err());
        let json = serde_json::to_string(&pending.review).unwrap();
        assert!(!json.contains("private-identity"));
        assert!(!json.contains("session"));
        let opened = library.open(&pending.review.artifact_id).unwrap();
        assert!(
            library
                .remove(&opened.artifact.app_id, Some(&opened.artifact.id))
                .is_err()
        );
        drop(pending);
        assert!(
            library
                .remove(&opened.artifact.app_id, Some(&opened.artifact.id))
                .is_ok()
        );
    }

    #[tokio::test]
    /// Stale clients cannot consume or discard a newer plan, and refusal performs no device I/O.
    async fn tokens_and_acknowledgements_are_checked_before_consumption() {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::new(dir.path().join("library"));
        let pending = pending(&library);
        let service = SigningService::new(library, Accounts::default(), dir.path().join("signed"));
        *service.guided.pending.lock().unwrap() = Some(pending);
        service.discard_preparation(Some("old-token")).unwrap();
        assert!(service.guided.pending.lock().unwrap().is_some());
        let result = service
            .execute_preparation(
                "old-token",
                Consents {
                    registration: true,
                    certificate: true,
                    provisioning: true,
                },
                Arc::new(crate::application::Discard),
            )
            .await;
        assert_eq!(result.err().unwrap().code, ErrorCode::ReviewStale);
        let result = service
            .execute_preparation(
                "token",
                Consents {
                    registration: false,
                    certificate: false,
                    provisioning: false,
                },
                Arc::new(crate::application::Discard),
            )
            .await;
        assert_eq!(
            result.err().unwrap().code,
            ErrorCode::AcknowledgementRequired
        );
        assert!(service.guided.pending.lock().unwrap().is_some());
        service.discard_preparation(Some("token")).unwrap();
        assert!(service.guided.pending.lock().unwrap().is_none());
    }

    #[tokio::test]
    /// The whole sequence reserves account mutations, not just individual Apple requests.
    async fn account_changes_are_refused_during_preparation() {
        let accounts = Accounts::default();
        let guard = accounts.operation().unwrap();
        assert_eq!(
            accounts.sign_out().err().unwrap().code,
            ErrorCode::OperationInProgress
        );
        assert_eq!(
            accounts.select_team("TEAM".into()).err().unwrap().code,
            ErrorCode::OperationInProgress
        );
        assert_eq!(
            accounts.request_certificate(true).await.unwrap_err().code,
            ErrorCode::OperationInProgress
        );
        drop(guard);
        assert!(accounts.sign_out().is_ok());
    }
}
