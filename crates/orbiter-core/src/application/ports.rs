//! The seams between Orbiter's workflows and the world outside this process.
//!
//! Three narrow traits, not one backend interface. Each exists because a specific rule could not
//! otherwise be tested without a phone, an Apple account, or a ten-minute wait:
//!
//! - [`Clock`] — so a review's expiry can be stood on exactly rather than waited for.
//! - [`Reviewer`] — so binding a review to an artifact and a device can be exercised without one.
//! - [`Installer`] — so cancellation, a dropped subscriber, a failed history write and crash
//!   recovery can each be driven deliberately.
//!
//! What is deliberately **not** here: the library, the signer, the plan. Those are local,
//! deterministic and already testable with a temporary directory and a synthetic IPA; wrapping
//! them in traits would add indirection and remove nothing from a test's path.
//!
//! # Contracts
//!
//! Every implementation must be `Send + Sync` and usable from a multi-threaded runtime. An
//! [`Installer`] must not panic: it reports an outcome, and `Failed`, `Cancelled` and `Unknown`
//! are three distinct facts a caller relies on being able to tell apart.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use crate::installation::{
    PreparedInstall,
    job::{Control, JobStatus},
};

/// A source of the current time.
///
/// Injected so review expiry is a value a test can choose rather than a wall-clock wait. Nothing
/// here is used for the seven-day countdown, which takes its clock as an argument already.
pub trait Clock: Send + Sync {
    /// The current instant.
    fn now(&self) -> SystemTime;
}

/// The real clock.
pub struct SystemClock;

impl Clock for SystemClock {
    /// The operating system's current time.
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Preparing an installation review against a connected phone.
///
/// The one operation that needs a device before anything is installed: it verifies the phone,
/// snapshots the bytes and binds the two together.
pub trait Reviewer: Send + Sync {
    /// Verify the phone, snapshot the file, and bind a review to both.
    ///
    /// # Errors
    ///
    /// Returns a sentence a person can act on when the file cannot be read or inspected, or the
    /// phone is absent, locked or untrusted. The transport's own error must never appear in it.
    fn prepare(
        &self,
        path: PathBuf,
        device_id: u32,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<PreparedInstall, String>> + Send + '_>>;
}

/// Carrying one reviewed installation out on a phone.
///
/// # Cancellation
///
/// An implementation honours `control` up to the moment iOS is asked to install and refuses it
/// afterwards — see [`Control`]. Cancelling the calling task is not the same as cancelling the
/// installation, and an implementation must not treat it as such.
///
/// # Persistence
///
/// Each stage is written to `journal` *before* it reaches `notify`, so a client can never be shown
/// a stage that a crash would then lose.
///
/// # Subscribers
///
/// `notify` may go nowhere. A dropped subscriber is not an error and must not stop the work or
/// change what is recorded.
pub trait Installer: Send + Sync {
    /// Run the installation and report its terminal outcome.
    ///
    /// Returns a status rather than a `Result`: the outcome *is* the answer, and `Failed`,
    /// `Cancelled` and `Unknown` are distinct facts. `Unknown` specifically means iOS was asked and
    /// Orbiter did not learn the result — it is never used for a failure that happened earlier.
    fn install(
        &self,
        plan: PreparedInstall,
        control: Arc<Control>,
        journal: PathBuf,
        notify: Arc<dyn Fn(JobStatus) + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn Future<Output = JobStatus> + Send>>;
}

/// The real reviewer: a connected phone and the local device transport.
pub struct DeviceReviewer;

impl Reviewer for DeviceReviewer {
    /// Prepare a review through [`crate::installation::prepare`].
    fn prepare(
        &self,
        path: PathBuf,
        device_id: u32,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<PreparedInstall, String>> + Send + '_>> {
        Box::pin(crate::installation::prepare(path, device_id))
    }
}

/// The real installer: transfer to the phone over whichever connection it is on, and ask iOS to
/// install.
pub struct DeviceInstaller;

impl Installer for DeviceInstaller {
    /// Run the installation through [`crate::installation::execute`].
    fn install(
        &self,
        plan: PreparedInstall,
        control: Arc<Control>,
        journal: PathBuf,
        notify: Arc<dyn Fn(JobStatus) + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn Future<Output = JobStatus> + Send>> {
        Box::pin(async move {
            crate::installation::execute(plan, control, journal, move |status| notify(status)).await
        })
    }
}

/// How long a review authorises an installation for.
///
/// A review binds bytes and a phone that were verified at one moment. After this the phone may
/// have been unplugged or the file replaced, so it must be taken again rather than trusted.
pub const REVIEW_LIFETIME: Duration = Duration::from_secs(600);
