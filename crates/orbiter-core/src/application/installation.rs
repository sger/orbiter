//! The installation workflow, owned in one place.
//!
//! This used to live inside a Tauri command: validating the review, taking the operation gate,
//! consuming the prepared plan, registering cancellation, recording the attempt, spawning the
//! worker, filtering status transitions and reconciling a crash. None of it could be run by a test
//! or a CLI, and the rules it enforced were visible only by reading the command top to bottom.
//!
//! # What a review authorises
//!
//! [`InstallationService::prepare`] binds a token to one exact artifact, its verified bytes, and
//! one verified phone. Installing checks the token, the expiry, and the artifact's hash again.
//! Matching by app name or bundle identifier is deliberately never enough: two builds of the same
//! app share both.
//!
//! # Concurrency
//!
//! One review or installation at a time, admitted by an async gate. Removal of a saved file takes
//! the same gate, so a file cannot be deleted while a review points at it, and an active
//! installation holds a [`crate::library::Lease`] on its artifact for its whole lifetime.
//!
//! # Locking
//!
//! `gate` (async) is taken first and may be held across `await`. `state` (blocking) is taken only
//! for short, synchronous updates and is never held across an `await`. The library's own locks are
//! taken beneath both; see [`crate::library`].
//!
//! # Cancellation
//!
//! Cancellable up to the moment iOS is asked to install, and not after: once the device has the
//! command, Orbiter cannot take it back, and pretending otherwise would report an outcome it does
//! not know. Cancelling the Rust task is not the same as cancelling the installation — the
//! distinction is [`crate::installation::job::Control`]'s, and it is preserved here.
//!
//! # Persistence and recovery
//!
//! Every stage change is journalled before it is announced, and recorded in the library's history.
//! High-frequency byte progress is deliberately transient. After a crash, an interrupted stage
//! becomes a non-committal outcome; success is never inferred.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use super::ProgressSink;
use crate::{
    domain::{
        errors::{ErrorCode, OperationError, OperationResult},
        identifiers::{AppId, ArtifactId, JobId, ReviewToken, UsbDeviceId},
    },
    installation::{
        self, PreparedInstall, Review,
        job::{self, Control, JobStatus, Stage},
    },
    library::Library,
};

/// Whether the person driving this has accepted the operation's stated consequences.
///
/// An enum rather than a `bool` because the call sites read as claims about the world:
/// `execute(token, Acknowledgement::Given, sink)` says what was actually established, where
/// `execute(token, true, sink)` says nothing at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Acknowledgement {
    /// The consequences were shown and accepted.
    Given,
    /// Nothing was accepted. Every operation that mutates a phone refuses this.
    Missing,
}

impl Acknowledgement {
    /// Build one from the boolean an IPC request carries.
    ///
    /// The wire format is a `bool`; this is the single place that becomes a decision.
    pub fn from_request(acknowledged: bool) -> Self {
        if acknowledged {
            Self::Given
        } else {
            Self::Missing
        }
    }

    /// Refuse unless the consequences were accepted.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::AcknowledgementRequired`] when nothing was accepted.
    fn require(self, what: &str) -> OperationResult<()> {
        match self {
            Self::Given => Ok(()),
            Self::Missing => Err(OperationError::acknowledgement_required(what.to_owned())),
        }
    }
}

/// What one installation is doing right now, as far as this process knows.
///
/// Separate from the durable journal on purpose: this is the live picture a reconnecting window
/// asks for, and the journal is what survives the process.
#[derive(Default)]
struct Active {
    /// A review that has been prepared and not yet consumed or discarded.
    prepared: Option<PreparedInstall>,
    /// The running installation's cancellation handle, present only while one runs.
    control: Option<Arc<Control>>,
    /// The most recent status, so a client that reconnects can be told where things stand.
    current: Option<JobStatus>,
}

/// Reviewing, installing, cancelling and recovering installations.
///
/// Cheap to clone; clones share the same gate, the same live state and the same library.
#[derive(Clone)]
pub struct InstallationService {
    library: Library,
    journal: PathBuf,
    /// Admits one review or installation at a time, and is also taken by library removal so a
    /// saved file cannot disappear from under a review. Async because it is held across the whole
    /// installation, which awaits the device.
    gate: Arc<tokio::sync::Mutex<()>>,
    /// Short-lived, synchronous updates only. Never held across an `await`.
    state: Arc<Mutex<Active>>,
}

impl InstallationService {
    /// Build the service over a library and the path of its durable journal.
    pub fn new(library: Library, journal: PathBuf) -> Self {
        Self {
            library,
            journal,
            gate: Arc::default(),
            state: Arc::default(),
        }
    }

    /// Take the live-state lock for one short update.
    ///
    /// # Errors
    ///
    /// Fails only on lock poisoning, which means a panic left the picture half-updated.
    fn state(&self) -> OperationResult<std::sync::MutexGuard<'_, Active>> {
        self.state.lock().map_err(|_| {
            OperationError::new(ErrorCode::Internal, "Installation state is unavailable.")
        })
    }

    /// Take the operation gate, refusing rather than queueing.
    ///
    /// Orbiter never queues installations: a person who clicks twice should be told the first one
    /// is still running, not silently given two.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::OperationInProgress`] when another operation holds it.
    fn admit(&self) -> OperationResult<tokio::sync::OwnedMutexGuard<()>> {
        Arc::clone(&self.gate).try_lock_owned().map_err(|_| {
            OperationError::operation_in_progress("Another installation operation is in progress.")
        })
    }

    /// The gate, for callers that must exclude installations while they work.
    ///
    /// Library removal takes this so a saved file cannot be deleted while a review points at it.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::OperationInProgress`] when an installation holds it.
    pub fn exclude(&self) -> OperationResult<tokio::sync::OwnedMutexGuard<()>> {
        Arc::clone(&self.gate).try_lock_owned().map_err(|_| {
            OperationError::operation_in_progress(
                "An installation operation is in progress. Wait before removing files.",
            )
        })
    }

    /// Review installing one saved artifact on one connected phone.
    ///
    /// Verifies the managed copy's bytes, snapshots them privately, verifies the phone, and
    /// returns a token bound to all of it. Holds a lease on the artifact for as long as the review
    /// stands, so the file cannot be removed or replaced underneath it.
    ///
    /// Any previously prepared review is discarded: there is one at a time, and a stale one
    /// pointing at a different artifact is exactly the confusion this guards against.
    ///
    /// # Errors
    ///
    /// - [`ErrorCode::OperationInProgress`] if an installation is running.
    /// - [`ErrorCode::ArtifactMissing`] or [`ErrorCode::ArtifactChanged`] from the library.
    /// - [`ErrorCode::DeviceUnavailable`] if the phone is absent, locked or unpaired.
    pub async fn prepare(
        &self,
        artifact_id: &ArtifactId,
        device: UsbDeviceId,
    ) -> OperationResult<Review> {
        let _gate = self.admit()?;
        self.state()?.prepared.take();

        let library = self.library.clone();
        let wanted = artifact_id.clone();
        // Hashing a managed IPA reads the whole file; it does not belong on a runtime thread.
        let (artifact, path, lease) = tokio::task::spawn_blocking(move || library.pin(&wanted))
            .await
            .map_err(|_| OperationError::new(ErrorCode::Internal, "Library worker stopped."))?
            .map_err(|cause| classify_library(cause, artifact_id))?;

        let mut plan = installation::prepare(path, device.get())
            .await
            .map_err(|cause| OperationError::new(ErrorCode::DeviceUnavailable, cause))?;
        if plan.review.sha256 != artifact.sha256 {
            return Err(OperationError::artifact_changed(artifact_id));
        }
        plan.library_artifact = Some(artifact);
        plan.library_lease = Some(lease);
        let review = plan.review.clone();
        self.state()?.prepared = Some(plan);
        Ok(review)
    }

    /// Forget a prepared review, releasing its lease.
    ///
    /// Does nothing when the token names a review that is not the current one, so a late call from
    /// a window that has moved on cannot cancel a newer review.
    ///
    /// # Errors
    ///
    /// Fails only on lock poisoning.
    pub fn discard(&self, token: &ReviewToken) -> OperationResult<()> {
        let mut state = self.state()?;
        if state
            .prepared
            .as_ref()
            .is_some_and(|plan| &plan.review.token == token)
        {
            state.prepared.take();
        }
        Ok(())
    }

    /// Drop a prepared review that points at something about to be deleted.
    ///
    /// Called by library removal, which holds [`Self::exclude`], so no installation can be running
    /// and the only thing to invalidate is a review sitting idle.
    ///
    /// # Errors
    ///
    /// Fails only on lock poisoning.
    pub fn invalidate(
        &self,
        app_id: &AppId,
        artifact_id: Option<&ArtifactId>,
    ) -> OperationResult<()> {
        let mut state = self.state()?;
        let affected = state
            .prepared
            .as_ref()
            .and_then(|plan| plan.library_artifact.as_ref())
            .is_some_and(|artifact| {
                &artifact.app_id == app_id
                    && artifact_id.is_none_or(|id| {
                        &artifact.id == id || artifact.source_id.as_ref() == Some(id)
                    })
            });
        if affected {
            state.prepared.take();
        }
        Ok(())
    }

    /// Install the reviewed artifact on the reviewed phone.
    ///
    /// Consumes the review, records the attempt in the library *before* any bytes move, and runs
    /// the installation, reporting each stage through `progress`.
    ///
    /// A failure to write history is never reported as a failed installation: once iOS says it
    /// installed the app, that is the outcome, and the note about history is appended to the
    /// message rather than replacing the result.
    ///
    /// # Cancellation
    ///
    /// [`Self::cancel`] stops this up to the moment iOS is asked to install. After that the
    /// request is refused and the operation runs to whatever conclusion the device reports.
    ///
    /// # Errors
    ///
    /// - [`ErrorCode::AcknowledgementRequired`] without an acknowledgement.
    /// - [`ErrorCode::OperationInProgress`] if another operation is running.
    /// - [`ErrorCode::ReviewStale`] if the token does not match, the review expired, or it carried
    ///   blockers.
    /// - [`ErrorCode::ArtifactChanged`] if the reviewed bytes no longer match the library record.
    /// - [`ErrorCode::OutcomeUnknown`] if the worker itself stopped; the recorded outcome and the
    ///   phone are then the only evidence.
    pub async fn execute(
        &self,
        token: &ReviewToken,
        acknowledgement: Acknowledgement,
        progress: Arc<dyn ProgressSink<JobStatus>>,
    ) -> OperationResult<JobStatus> {
        acknowledgement.require("Review and acknowledge the installation consequences first.")?;
        let _gate = self.admit()?;

        let plan = {
            let mut state = self.state()?;
            let plan = state
                .prepared
                .as_ref()
                .ok_or_else(|| OperationError::review_stale(None))?;
            if &plan.review.token != token || plan.expired() {
                return Err(OperationError::review_stale(Some(token)));
            }
            if !plan.review.blockers.is_empty() {
                return Err(OperationError::new(
                    ErrorCode::ReviewStale,
                    "Resolve the installation blockers before continuing.",
                ));
            }
            state
                .prepared
                .take()
                .ok_or_else(|| OperationError::review_stale(Some(token)))?
        };

        // Durable before anything moves: an attempt with no row would leave a successful install
        // with nowhere to report its outcome.
        plan.record_library_attempt(&self.library)
            .map_err(|cause| OperationError::new(ErrorCode::StorageWrite, cause))?;

        let control = Arc::new(Control::default());
        {
            let mut state = self.state()?;
            state.control = Some(Arc::clone(&control));
            state.current = Some(JobStatus {
                id: JobId::from_review(&plan.review.token),
                stage: Stage::Preparing,
                message: "Rechecking reviewed IPA and iPhone.".into(),
                transferred_bytes: 0,
                total_bytes: plan.review.size_bytes,
                device_percent: None,
                cleanup_pending: false,
            });
        }

        let outcome = self.run(plan, control, Arc::clone(&progress)).await;

        // The control handle belongs to a running installation and to nothing else.
        if let Ok(mut state) = self.state() {
            state.control = None;
        }
        outcome
    }

    /// Drive one installation to its conclusion, keeping live state and history in step.
    ///
    /// Split out of [`Self::execute`] so the gate and the control handle are released on every
    /// path, including the one where the worker itself stops.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::OutcomeUnknown`] if the worker task stopped without reporting.
    async fn run(
        &self,
        plan: PreparedInstall,
        control: Arc<Control>,
        progress: Arc<dyn ProgressSink<JobStatus>>,
    ) -> OperationResult<JobStatus> {
        let journal = self.journal.clone();
        let history = self.library.clone();
        let live = Arc::clone(&self.state);
        // Transfer progress arrives every 150 ms and the device reports its own percentage on top,
        // a few thousand events for one install. Only a stage change is durable state worth
        // recording; asking the library about every tick would re-read the whole manifest on the
        // same path that delivers progress to the window.
        let recorded = Mutex::new(None::<Stage>);

        let worker = tokio::spawn(async move {
            installation::execute(plan, control, journal, move |mut status| {
                let changed = recorded
                    .lock()
                    .map(|mut held| {
                        let changed = *held != Some(status.stage);
                        *held = Some(status.stage);
                        changed
                    })
                    .unwrap_or(true);
                if changed && let Err(error) = history.update(&status) {
                    // Appended, never substituted: iOS's verdict is the outcome, and a note about
                    // Orbiter's own bookkeeping must not be mistaken for a failed installation.
                    status.message.push_str(&format!(
                        " Installation history could not be saved: {error}"
                    ));
                }
                if let Ok(mut state) = live.lock() {
                    state.current = Some(status.clone());
                }
                progress.send(status);
            })
            .await
        })
        .await;

        match worker {
            Ok(status) => Ok(status),
            Err(_) => {
                // The worker stopped without reporting. Reconcile from the journal and say the
                // outcome is unknown, which is the truth: the phone may or may not have the app.
                let recovered = job::recover(&self.journal).unwrap_or(None);
                let _ = self.library.recover(recovered.as_ref());
                if let Ok(mut state) = self.state() {
                    state.current = recovered;
                }
                Err(OperationError::new(
                    ErrorCode::OutcomeUnknown,
                    "Installation worker stopped. Check the recorded outcome and the phone before retrying.",
                ))
            }
        }
    }

    /// Ask the running installation to stop.
    ///
    /// Returns `true` only if a cancellable installation accepted the request. Returns `false`
    /// when nothing is running, or when iOS already has the install command — at which point the
    /// outcome belongs to the device and Orbiter will report what it observes.
    ///
    /// # Errors
    ///
    /// Fails only on lock poisoning.
    pub fn cancel(&self) -> OperationResult<bool> {
        Ok(self
            .state()?
            .control
            .as_ref()
            .is_some_and(|control| control.cancel()))
    }

    /// Where the current or most recent installation stands.
    ///
    /// A client that reconnects — a reopened window, a page that was hidden — calls this to catch
    /// up, because progress events it missed are gone. While an installation is running this is
    /// the live picture; otherwise it is whatever the durable journal last recorded, so a result
    /// survives the window being closed.
    ///
    /// # Errors
    ///
    /// Fails on lock poisoning, or if the journal exists but cannot be read.
    pub fn status(&self) -> OperationResult<Option<JobStatus>> {
        let (running, current) = {
            let state = self.state()?;
            (state.control.is_some(), state.current.clone())
        };
        if running {
            return Ok(current);
        }
        job::recover(&self.journal)
            .map_err(OperationError::storage_read)
            .map(|recovered| recovered.or(current))
    }

    /// The journal this service writes.
    pub fn journal(&self) -> &Path {
        &self.journal
    }
}

/// Classify a library failure that happened while resolving an artifact.
///
/// The library still returns messages rather than codes; this is where they become a code the
/// frontend can act on. The distinction that matters is "gone" from "changed": the first means
/// re-import is pointless, the second means it is the fix.
fn classify_library(cause: String, artifact_id: &ArtifactId) -> OperationError {
    if cause.contains("changed or is damaged") {
        OperationError::artifact_changed(artifact_id)
    } else if cause.contains("was removed") {
        OperationError::artifact_missing(artifact_id)
    } else {
        OperationError::new(ErrorCode::StorageRead, cause)
    }
}

#[cfg(test)]
/// Checks the rules an installation must obey before it may touch a phone.
///
/// None of these need Apple credentials or a connected device: every case stops at or before the
/// point where the transport would be used, which is exactly where the rules live.
mod tests {
    use super::*;
    use crate::application::{Discard, runtime::Runtime};

    /// A runtime over a throwaway directory, plus its temporary directory so it outlives the test.
    fn runtime() -> (tempfile::TempDir, Runtime) {
        let dir = tempfile::tempdir().expect("a temporary storage directory");
        let runtime = Runtime::new(dir.path());
        (dir, runtime)
    }

    /// A sink that records nothing, for tests that assert on outcomes rather than on updates.
    fn discard() -> Arc<dyn ProgressSink<JobStatus>> {
        Arc::new(Discard)
    }

    #[tokio::test]
    /// Installing without the acknowledgement shown beside the review is refused, and refused
    /// before anything else is checked — so a missing acknowledgement can never be bypassed by
    /// also having a stale token.
    async fn an_installation_without_an_acknowledgement_is_refused_first() {
        let (_dir, runtime) = runtime();
        let error = runtime
            .installations()
            .execute(
                &ReviewToken::new("never-prepared"),
                Acknowledgement::Missing,
                discard(),
            )
            .await
            .expect_err("an unacknowledged installation is refused");
        assert_eq!(error.code, ErrorCode::AcknowledgementRequired);
    }

    #[tokio::test]
    /// Installing with no review at all is refused as stale rather than proceeding: a token that
    /// authorises nothing must not be treated as authorising the last thing prepared.
    async fn an_installation_with_no_review_is_refused() {
        let (_dir, runtime) = runtime();
        let error = runtime
            .installations()
            .execute(
                &ReviewToken::new("never-prepared"),
                Acknowledgement::Given,
                discard(),
            )
            .await
            .expect_err("an installation without a review is refused");
        assert_eq!(error.code, ErrorCode::ReviewStale);
    }

    #[tokio::test]
    /// Reviewing an artifact the library does not hold is refused as missing, not as a storage
    /// failure: re-importing fixes one of those and is pointless for the other.
    async fn reviewing_an_unknown_artifact_says_it_is_missing() {
        let (_dir, runtime) = runtime();
        // `Review` deliberately has no `Debug`, so match rather than unwrap the error out.
        match runtime
            .installations()
            .prepare(&ArtifactId::new("not-in-this-library"), UsbDeviceId::new(1))
            .await
        {
            Err(error) => assert_eq!(error.code, ErrorCode::ArtifactMissing),
            Ok(_) => panic!("an unknown artifact must not be reviewable"),
        }
    }

    #[tokio::test]
    /// Discarding a token that is not the current review leaves that review alone, so a late call
    /// from a window that has moved on cannot cancel a newer one.
    async fn discarding_a_foreign_token_leaves_the_current_review_alone() {
        let (_dir, runtime) = runtime();
        let service = runtime.installations();
        service
            .discard(&ReviewToken::new("someone-elses"))
            .expect("discarding an unknown token is not an error");
    }

    #[tokio::test]
    /// The gate admits one operation: while a removal holds it, an installation is refused rather
    /// than queued, because a person who clicks twice should be told the first is still running.
    async fn one_operation_at_a_time_and_the_second_is_refused_not_queued() {
        let (_dir, runtime) = runtime();
        let service = runtime.installations();
        let held = service.exclude().expect("the gate is free");
        let error = service
            .execute(&ReviewToken::new("any"), Acknowledgement::Given, discard())
            .await
            .expect_err("a second operation is refused");
        assert_eq!(error.code, ErrorCode::OperationInProgress);
        drop(held);
        // And it is available again immediately afterwards.
        assert!(service.exclude().is_ok());
    }

    #[tokio::test]
    /// Cancelling when nothing is running reports that nothing was cancelled, rather than
    /// claiming a stop that did not happen.
    async fn cancelling_nothing_says_so() {
        let (_dir, runtime) = runtime();
        assert!(
            !runtime
                .installations()
                .cancel()
                .expect("cancellation state is readable")
        );
    }

    #[tokio::test]
    /// With no installation ever run and no journal on disk, the status a reconnecting client
    /// receives is "nothing", not a fabricated idle result.
    async fn a_reconnecting_client_is_told_nothing_rather_than_a_guess() {
        let (_dir, runtime) = runtime();
        assert!(
            runtime
                .installations()
                .status()
                .expect("status is readable")
                .is_none()
        );
    }

    #[tokio::test]
    /// Removing the artifact a review points at invalidates that review. The review is dropped,
    /// its lease released, and the token no longer authorises anything.
    async fn removing_the_reviewed_artifact_invalidates_the_review() {
        let (_dir, runtime) = runtime();
        let service = runtime.installations();
        let library = runtime.library();
        let ipa = crate::library::tests_support::fixture("one");
        let imported = library.import(ipa.path()).expect("the fixture imports");

        service
            .invalidate(&imported.app_id, Some(&imported.artifact_id))
            .expect("invalidation is not an error when there is no review");
        // And with a review in place it would be dropped; there is nothing to install afterwards.
        let error = service
            .execute(
                &ReviewToken::new("anything"),
                Acknowledgement::Given,
                discard(),
            )
            .await
            .expect_err("no review authorises an installation");
        assert_eq!(error.code, ErrorCode::ReviewStale);
    }
}
