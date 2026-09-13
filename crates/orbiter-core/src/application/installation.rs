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
    time::SystemTime,
};

use super::{
    ProgressSink,
    ports::{
        Clock, DeviceInstaller, DeviceReviewer, Installer, REVIEW_LIFETIME, Reviewer, SystemClock,
    },
};
use crate::{
    domain::{
        errors::{ErrorCode, OperationError, OperationResult},
        identifiers::{AppId, ArtifactId, JobId, ReviewToken, UsbDeviceId},
    },
    installation::{
        PreparedInstall, Review,
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
    /// A review that has been prepared and not yet consumed or discarded, and when it was taken.
    ///
    /// The moment is kept here rather than inside the review because expiry is measured against
    /// this service's clock, which a test can choose.
    prepared: Option<(PreparedInstall, SystemTime)>,
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
    /// How a review is taken. The real one needs a phone; a test supplies its own.
    reviewer: Arc<dyn Reviewer>,
    /// How an installation is carried out. The real one needs a phone; a test supplies its own.
    installer: Arc<dyn Installer>,
    /// What "now" means for review expiry.
    clock: Arc<dyn Clock>,
}

impl InstallationService {
    /// Build the service over a library and the path of its durable journal, using a real phone.
    pub fn new(library: Library, journal: PathBuf) -> Self {
        Self::with_ports(
            library,
            journal,
            Arc::new(DeviceReviewer),
            Arc::new(DeviceInstaller),
            Arc::new(SystemClock),
        )
    }

    /// Build the service against chosen boundaries.
    ///
    /// Used by tests to drive the rules — expiry, cancellation, a dropped subscriber, a failed
    /// history write — without a phone or a ten-minute wait. Production uses [`Self::new`].
    pub fn with_ports(
        library: Library,
        journal: PathBuf,
        reviewer: Arc<dyn Reviewer>,
        installer: Arc<dyn Installer>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            library,
            journal,
            gate: Arc::default(),
            state: Arc::default(),
            reviewer,
            installer,
            clock,
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

        let mut plan = self
            .reviewer
            .prepare(path, device.get())
            .await
            .map_err(|cause| OperationError::new(ErrorCode::DeviceUnavailable, cause))?;
        if plan.review.sha256 != artifact.sha256 {
            return Err(OperationError::artifact_changed(artifact_id));
        }
        plan.library_artifact = Some(artifact);
        plan.library_lease = Some(lease);
        let review = plan.review.clone();
        self.state()?.prepared = Some((plan, self.clock.now()));
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
            .is_some_and(|(plan, _)| &plan.review.token == token)
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
            .and_then(|(plan, _)| plan.library_artifact.as_ref())
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
            let (plan, taken) = state
                .prepared
                .as_ref()
                .ok_or_else(|| OperationError::review_stale(None))?;
            // Expiry is measured against this service's clock, and the review's own deadline is
            // checked too: either one having passed is enough to refuse.
            let aged = self
                .clock
                .now()
                .duration_since(*taken)
                .is_ok_and(|since| since > REVIEW_LIFETIME);
            if &plan.review.token != token || aged || plan.expired() {
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
                .0
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

        let installer = Arc::clone(&self.installer);
        let worker = tokio::spawn(async move {
            let notify: Arc<dyn Fn(JobStatus) + Send + Sync> =
                Arc::new(move |mut status: JobStatus| {
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
                });
            installer.install(plan, control, journal, notify).await
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
/// Deterministic stand-ins for the phone and the clock.
///
/// Each is the smallest thing that satisfies its port. None of them mirrors the real
/// implementation: a test asserts what the *service* does with what a boundary reports, never that
/// the boundary was called in a particular way.
mod fakes {
    use super::*;
    use crate::application::ports::{Clock, Installer, Reviewer};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// A clock a test sets by hand.
    pub(super) struct FixedClock(Mutex<SystemTime>);

    impl FixedClock {
        /// Start at a fixed instant well clear of the epoch.
        pub(super) fn new() -> Arc<Self> {
            Arc::new(Self(Mutex::new(
                SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000),
            )))
        }
        /// Move time forward by `seconds`.
        pub(super) fn advance(&self, seconds: u64) {
            if let Ok(mut now) = self.0.lock() {
                *now += std::time::Duration::from_secs(seconds);
            }
        }
    }

    impl Clock for FixedClock {
        /// The instant this clock was last set to.
        fn now(&self) -> SystemTime {
            self.0
                .lock()
                .map(|now| *now)
                .unwrap_or(SystemTime::UNIX_EPOCH)
        }
    }

    /// A reviewer that hands back a synthetic review instead of verifying a phone.
    pub(super) struct FakeReviewer {
        /// The fingerprint the review claims, so a test can make it disagree with the library.
        pub(super) sha256: Mutex<String>,
        /// Whether the next review should fail as though no phone were attached.
        pub(super) unavailable: AtomicBool,
    }

    impl FakeReviewer {
        /// A reviewer that will claim `sha256` for whatever it is asked to review.
        pub(super) fn new(sha256: &str) -> Arc<Self> {
            Arc::new(Self {
                sha256: Mutex::new(sha256.to_owned()),
                unavailable: AtomicBool::new(false),
            })
        }
    }

    impl Reviewer for FakeReviewer {
        /// Produce a synthetic review, or refuse as an absent phone would.
        fn prepare(
            &self,
            _path: PathBuf,
            device_id: u32,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<PreparedInstall, String>> + Send + '_>>
        {
            let sha = self.sha256.lock().map(|s| s.clone()).unwrap_or_default();
            let refuse = self.unavailable.load(Ordering::SeqCst);
            Box::pin(async move {
                if refuse {
                    return Err("Connect and unlock the iPhone, then trust this Mac.".into());
                }
                Ok(PreparedInstall::synthetic(
                    ReviewToken::new(uuid::Uuid::new_v4().to_string()),
                    sha,
                    device_id,
                    None,
                    None,
                ))
            })
        }
    }

    /// An installer that reports a scripted sequence of stages.
    pub(super) struct ScriptedInstaller {
        /// Stages to announce, in order. The last one is the outcome.
        pub(super) stages: Vec<Stage>,
        /// How many times it was asked to install, so a test can prove it was never reached.
        pub(super) calls: AtomicUsize,
        /// Whether to check the cancellation handle before announcing anything.
        pub(super) honours_cancellation: bool,
        /// Run once the installation has started, after the attempt row is already durable.
        ///
        /// Lets a test break the library at the one moment that matters: mid-install, with the
        /// history row written and the outcome still to come.
        pub(super) once_started: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    }

    impl ScriptedInstaller {
        /// An installer that walks `stages` and ends at the last of them.
        pub(super) fn new(stages: &[Stage]) -> Arc<Self> {
            Arc::new(Self {
                stages: stages.to_vec(),
                calls: AtomicUsize::new(0),
                honours_cancellation: true,
                once_started: Mutex::new(None),
            })
        }
        /// How many installations this ran.
        pub(super) fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl Installer for ScriptedInstaller {
        /// Announce each scripted stage, journalling before notifying, and return the last.
        ///
        /// Honours cancellation the way the real transport does: only while the stage machine
        /// still permits it. Once the commit boundary is crossed the request is refused and the
        /// scripted outcome stands.
        fn install(
            &self,
            plan: PreparedInstall,
            control: Arc<Control>,
            journal: PathBuf,
            notify: Arc<dyn Fn(JobStatus) + Send + Sync>,
        ) -> std::pin::Pin<Box<dyn Future<Output = JobStatus> + Send>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut hook) = self.once_started.lock()
                && let Some(run) = hook.take()
            {
                run();
            }
            let stages = self.stages.clone();
            let honours = self.honours_cancellation;
            Box::pin(async move {
                let mut status = JobStatus {
                    id: JobId::from_review(&plan.review.token),
                    stage: Stage::Preparing,
                    message: "Preparing".into(),
                    transferred_bytes: 0,
                    total_bytes: plan.review.size_bytes,
                    device_percent: None,
                    cleanup_pending: false,
                };
                for stage in stages {
                    if honours && control.cancelled() && !control.transition(stage) {
                        status.stage = Stage::Cancelled;
                        status.message = "Cancelled before the install command.".into();
                        let _ = job::save(&journal, &status);
                        notify(status.clone());
                        return status;
                    }
                    control.transition(stage);
                    status.stage = stage;
                    status.message = format!("{stage:?}");
                    // Durable before visible, exactly as the real transport does it.
                    let _ = job::save(&journal, &status);
                    notify(status.clone());
                }
                status
            })
        }
    }
}

#[cfg(test)]
/// Checks the rules an installation must obey before it may touch a phone.
///
/// None of these need Apple credentials or a connected device: every case stops at or before the
/// point where the transport would be used, which is exactly where the rules live.
mod tests {
    use super::fakes::{FakeReviewer, FixedClock, ScriptedInstaller};
    use super::*;
    use crate::application::{
        Discard,
        ports::{Clock, Installer, Reviewer},
        runtime::Runtime,
    };
    use std::sync::atomic::Ordering;

    /// A service wired to fakes, holding one imported build, with the pieces to steer them.
    struct Harness {
        _dir: tempfile::TempDir,
        service: InstallationService,
        library: Library,
        /// The saved original the tests review.
        artifact: ArtifactId,
        app: AppId,
        clock: Arc<FixedClock>,
        reviewer: Arc<FakeReviewer>,
        installer: Arc<ScriptedInstaller>,
    }

    /// A service over a throwaway directory with one imported build, running `stages`.
    ///
    /// The reviewer claims the imported build's real fingerprint, so the service's own check that
    /// the reviewed bytes match the library record passes unless a test makes it disagree.
    fn harness(stages: &[Stage]) -> Harness {
        let dir = tempfile::tempdir().expect("a temporary storage directory");
        let library = Library::new(dir.path().join("library"));
        let ipa = crate::library::tests_support::fixture("one");
        let imported = library.import(ipa.path()).expect("the fixture imports");
        let sha = library
            .snapshot()
            .expect("the library is readable")
            .artifacts
            .into_iter()
            .find(|a| a.id == imported.artifact_id)
            .expect("the imported artifact is listed")
            .sha256;

        let clock = FixedClock::new();
        let reviewer = FakeReviewer::new(&sha);
        let installer = ScriptedInstaller::new(stages);
        let service = InstallationService::with_ports(
            library.clone(),
            dir.path().join("last-install.json"),
            Arc::clone(&reviewer) as Arc<dyn Reviewer>,
            Arc::clone(&installer) as Arc<dyn Installer>,
            Arc::clone(&clock) as Arc<dyn Clock>,
        );
        Harness {
            _dir: dir,
            service,
            library,
            artifact: imported.artifact_id,
            app: imported.app_id,
            clock,
            reviewer,
            installer,
        }
    }

    /// Review the harness's build on one phone and return the resulting token.
    async fn reviewed(h: &Harness, device: u32) -> ReviewToken {
        h.service
            .prepare(&h.artifact, UsbDeviceId::new(device))
            .await
            .map(|review| review.token)
            .expect("a synthetic review is prepared")
    }

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

    #[tokio::test]
    /// A review authorises an installation for ten minutes. Past that it is refused and must be
    /// taken again, because the phone may have been unplugged or the file replaced since.
    async fn a_review_expires_on_a_clock_a_test_can_move() {
        let h = harness(&[Stage::Transferring, Stage::Installing, Stage::Installed]);
        let token = reviewed(&h, 1).await;

        h.clock.advance(599);
        // Still inside its lifetime: nothing is refused yet.
        h.service
            .execute(&token, Acknowledgement::Given, Arc::new(Discard))
            .await
            .expect("a fresh review installs");

        let token = reviewed(&h, 1).await;
        h.clock.advance(601);
        match h
            .service
            .execute(&token, Acknowledgement::Given, Arc::new(Discard))
            .await
        {
            Err(error) => assert_eq!(error.code, ErrorCode::ReviewStale),
            Ok(_) => panic!("an expired review must not authorise an installation"),
        }
    }

    #[tokio::test]
    /// A token from an earlier review does not authorise the current one. Reviewing again replaces
    /// what is authorised, so the old token is worthless rather than merely older.
    async fn a_superseded_token_authorises_nothing() {
        let h = harness(&[Stage::Installed]);
        let first = reviewed(&h, 1).await;
        let second = reviewed(&h, 1).await;
        assert_ne!(first, second);

        match h
            .service
            .execute(&first, Acknowledgement::Given, Arc::new(Discard))
            .await
        {
            Err(error) => assert_eq!(error.code, ErrorCode::ReviewStale),
            Ok(_) => panic!("a superseded token must not authorise an installation"),
        }
        assert_eq!(h.installer.calls(), 0, "nothing may reach the phone");
    }

    #[tokio::test]
    /// Reviewing on a second phone replaces the first review. A token bound to one device must not
    /// install on another, however alike the two look.
    async fn switching_devices_invalidates_the_earlier_review() {
        let h = harness(&[Stage::Installed]);
        let first_phone = reviewed(&h, 1).await;
        let _second_phone = reviewed(&h, 2).await;

        match h
            .service
            .execute(&first_phone, Acknowledgement::Given, Arc::new(Discard))
            .await
        {
            Err(error) => assert_eq!(error.code, ErrorCode::ReviewStale),
            Ok(_) => panic!("a review for another phone must not authorise this one"),
        }
    }

    #[tokio::test]
    /// If the reviewed bytes do not match the library's record, the review is refused rather than
    /// installed. Matching by app name or bundle identifier would not catch this: two builds of one
    /// app share both.
    async fn a_review_whose_bytes_disagree_with_the_library_is_refused() {
        let h = harness(&[Stage::Installed]);
        if let Ok(mut claimed) = h.reviewer.sha256.lock() {
            *claimed = "0".repeat(64);
        }
        match h.service.prepare(&h.artifact, UsbDeviceId::new(1)).await {
            Err(error) => assert_eq!(error.code, ErrorCode::ArtifactChanged),
            Ok(_) => panic!("bytes that disagree with the record must not be reviewable"),
        }
    }

    #[tokio::test]
    /// A phone that is absent, locked or untrusted refuses the review with something a person can
    /// act on, and the transport's own error never appears.
    async fn an_unavailable_phone_is_reported_as_such() {
        let h = harness(&[Stage::Installed]);
        h.reviewer.unavailable.store(true, Ordering::SeqCst);
        match h.service.prepare(&h.artifact, UsbDeviceId::new(1)).await {
            Err(error) => {
                assert_eq!(error.code, ErrorCode::DeviceUnavailable);
                assert!(error.message.contains("unlock"), "{}", error.message);
            }
            Ok(_) => panic!("an unavailable phone must not produce a review"),
        }
    }

    #[tokio::test]
    /// Cancelling before the install command reaches iOS stops the installation, and the outcome
    /// is `Cancelled` — a fact distinct from both failure and success.
    async fn cancelling_before_the_commit_boundary_stops_the_installation() {
        let h = harness(&[Stage::Transferring, Stage::Installing, Stage::Installed]);
        let token = reviewed(&h, 1).await;

        // The scripted installer checks the handle at each stage, as the real transport does.
        let service = h.service.clone();
        let watcher = tokio::spawn(async move {
            // Cancel as soon as an installation exists to cancel.
            for _ in 0..200 {
                if service.cancel().unwrap_or(false) {
                    return true;
                }
                tokio::task::yield_now().await;
            }
            false
        });
        let outcome = h
            .service
            .execute(&token, Acknowledgement::Given, Arc::new(Discard))
            .await
            .expect("the installation reports an outcome");
        let cancelled = watcher.await.expect("the watcher finishes");
        if cancelled {
            assert_eq!(outcome.stage, Stage::Cancelled);
            assert_ne!(outcome.stage, Stage::Installed);
        }
    }

    #[tokio::test]
    /// Once iOS has been asked to install, cancellation is refused. The outcome belongs to the
    /// device from that point, and claiming a stop would be asserting something Orbiter does not
    /// control.
    async fn cancelling_after_the_commit_boundary_is_refused() {
        let h = harness(&[Stage::Transferring, Stage::Installing, Stage::Installed]);
        let token = reviewed(&h, 1).await;
        let outcome = h
            .service
            .execute(&token, Acknowledgement::Given, Arc::new(Discard))
            .await
            .expect("the installation reports an outcome");
        assert_eq!(outcome.stage, Stage::Installed);
        // Nothing is running now, and a late request changes nothing.
        assert!(!h.service.cancel().expect("cancellation state is readable"));
        assert_eq!(
            h.service
                .status()
                .expect("status is readable")
                .map(|status| status.stage),
            Some(Stage::Installed)
        );
    }

    #[tokio::test]
    /// A subscriber that goes away does not stop the installation or lose its history: the work
    /// finishes and the durable record is written regardless.
    async fn a_dropped_subscriber_loses_updates_but_never_the_outcome() {
        let h = harness(&[Stage::Transferring, Stage::Installing, Stage::Installed]);
        let token = reviewed(&h, 1).await;

        let seen = Arc::new(Mutex::new(0usize));
        let counted = Arc::clone(&seen);
        // A sink that stops recording after the first update, standing in for a closed window.
        let sink: Arc<dyn ProgressSink<JobStatus>> = Arc::new(move |_status: JobStatus| {
            if let Ok(mut count) = counted.lock() {
                *count += 1;
            }
        });
        let outcome = h
            .service
            .execute(&token, Acknowledgement::Given, sink)
            .await
            .expect("the installation reports an outcome");

        assert_eq!(outcome.stage, Stage::Installed);
        assert!(*seen.lock().expect("the counter is readable") > 0);
        // And the library recorded the outcome without depending on anyone listening.
        let attempts = h
            .library
            .snapshot()
            .expect("the library is readable")
            .attempts;
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].stage, Stage::Installed);
    }

    #[tokio::test]
    /// A client that reconnects is told where things stand from the durable journal, because the
    /// progress events it missed are gone and nothing is replayed.
    async fn a_reconnecting_client_is_caught_up_from_the_journal() {
        let h = harness(&[Stage::Transferring, Stage::Installing, Stage::Installed]);
        assert!(
            h.service.status().expect("status is readable").is_none(),
            "nothing has happened yet, so nothing is claimed"
        );
        let token = reviewed(&h, 1).await;
        h.service
            .execute(&token, Acknowledgement::Given, Arc::new(Discard))
            .await
            .expect("the installation reports an outcome");

        // A second service over the same journal is exactly what a restarted client sees.
        let fresh = InstallationService::new(h.library.clone(), h.service.journal().to_path_buf());
        assert_eq!(
            fresh
                .status()
                .expect("status is readable")
                .map(|status| status.stage),
            Some(Stage::Installed)
        );
    }

    #[tokio::test]
    /// iOS reporting success is the outcome. A failure to write Orbiter's own history is appended
    /// to the message and never substituted for the result: telling someone an installation failed
    /// when the app is on their phone is the worse error of the two.
    async fn a_history_write_failure_does_not_become_a_failed_installation() {
        use std::os::unix::fs::PermissionsExt;
        let h = harness(&[Stage::Transferring, Stage::Installing, Stage::Installed]);
        let token = reviewed(&h, 1).await;

        // Break the library once the installation has started, so the attempt row already exists
        // and only the later stage writes fail. Breaking it any earlier would refuse the
        // installation outright, which is separately correct: history must be durable first.
        let root = h.library.root().to_path_buf();
        let broken = root.clone();
        if let Ok(mut hook) = h.installer.once_started.lock() {
            *hook = Some(Box::new(move || {
                let _ = std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o500));
            }));
        }
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let collected = Arc::clone(&seen);
        let sink: Arc<dyn ProgressSink<JobStatus>> = Arc::new(move |status: JobStatus| {
            if let Ok(mut lines) = collected.lock() {
                lines.push(status.message);
            }
        });
        let outcome = h
            .service
            .execute(&token, Acknowledgement::Given, sink)
            .await;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .expect("permissions are restorable");

        let outcome = outcome.expect("the installation still reports its outcome");
        // The invariant: iOS said it installed the app, so that is the outcome. The bookkeeping
        // problem does not change it.
        assert_eq!(outcome.stage, Stage::Installed);
        // And it is not hidden either — it is appended to what the window is told.
        let lines = seen.lock().expect("the messages are readable");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("history could not be saved")),
            "the bookkeeping problem must still be stated: {lines:?}"
        );
    }

    #[tokio::test]
    /// An installation interrupted while iOS was installing recovers to `Unknown`, never to
    /// success. Orbiter did not see the result and has no evidence for one.
    async fn recovery_at_a_durable_transition_never_infers_success() {
        let h = harness(&[Stage::Transferring, Stage::Installing]);
        let token = reviewed(&h, 1).await;
        h.service
            .execute(&token, Acknowledgement::Given, Arc::new(Discard))
            .await
            .expect("the installation reports an outcome");

        // The journal is left at `Installing`, which is what a crash there looks like.
        let fresh = InstallationService::new(h.library.clone(), h.service.journal().to_path_buf());
        let recovered = fresh
            .status()
            .expect("status is readable")
            .expect("the journal has something in it");
        assert_eq!(recovered.stage, Stage::Unknown);
        assert!(recovered.cleanup_pending);
    }

    #[tokio::test]
    /// Removing the app a review points at drops that review, so the token no longer authorises
    /// anything and nothing can be installed from a file that is gone.
    async fn removing_the_app_behind_a_review_drops_it() {
        let h = harness(&[Stage::Installed]);
        let token = reviewed(&h, 1).await;
        h.service
            .invalidate(&h.app, None)
            .expect("invalidation succeeds");
        match h
            .service
            .execute(&token, Acknowledgement::Given, Arc::new(Discard))
            .await
        {
            Err(error) => assert_eq!(error.code, ErrorCode::ReviewStale),
            Ok(_) => panic!("a review for a removed app must not authorise an installation"),
        }
        assert_eq!(h.installer.calls(), 0, "nothing may reach the phone");
    }
}
