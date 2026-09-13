//! One installation's durable journal and its cancellation rules.
//!
//! Two things live here, both about honesty of outcome. [`Control`] enforces that an installation
//! can be stopped only up to the moment iOS is asked to install. [`recover`] decides what an
//! interrupted installation is allowed to be called afterwards, and the answer is never "success".
//!
//! # Privacy
//!
//! The journal contains no path, device identifier or pairing material — only a stage, a message
//! and byte counts.

use serde::{Deserialize, Serialize};
use std::{io::Write, path::Path, sync::Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Where one installation is, from a stage machine whose transitions are deliberately one-way.
///
/// The three non-terminal stages describe work in progress; the four terminal ones are outcomes
/// and are never overwritten. `Unknown` is what an interrupted install becomes: iOS was asked and
/// Orbiter did not see the answer, which is a different fact from either success or failure and
/// is kept distinct from both.
pub enum Stage {
    Preparing,
    Transferring,
    Installing,
    Installed,
    Failed,
    Cancelled,
    Unknown,
}
impl Stage {
    /// Whether this stage is an outcome rather than work in progress.
    ///
    /// A terminal stage is final: recovery and later events both refuse to move a job out of one,
    /// so a late message cannot turn a recorded failure into a success.
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Installed | Self::Failed | Self::Cancelled | Self::Unknown
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
/// One installation's current stage and progress, as journalled and as sent to the window.
///
/// Contains no path, no device identifier and no pairing material: the journal survives on disk
/// and is deliberately not a record of which phone was involved.
pub struct JobStatus {
    /// The installation this describes, which is also the token of the review that authorised it.
    pub id: crate::domain::identifiers::JobId,
    pub stage: Stage,
    pub message: String,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub device_percent: Option<u64>,
    pub cleanup_pending: bool,
}
/// The mutable half of [`Control`], guarded by its mutex.
struct Inner {
    /// The stage the installation has actually reached.
    stage: Stage,
    /// Whether a stop has been requested and is still honourable.
    cancel: bool,
}
/// Cancellation and stage transitions for one installation, shared with its worker.
///
/// The rule this type exists to enforce: an installation may be stopped up to the moment iOS is
/// asked to install, and not after. Once the device has the command, Orbiter cannot take it back,
/// and a "cancelled" result would be a claim about something it does not control.
pub struct Control(Mutex<Inner>);
impl Default for Control {
    /// Start at [`Stage::Preparing`] with no cancellation requested.
    fn default() -> Self {
        Self(Mutex::new(Inner {
            stage: Stage::Preparing,
            cancel: false,
        }))
    }
}
impl Control {
    /// Request that the installation stop, if it still can.
    ///
    /// Returns `true` only while the work is still cancellable — before the install command
    /// reaches the device. Returns `false` afterwards, and on a poisoned lock, both of which mean
    /// the same thing to a caller: do not tell anyone this was stopped.
    pub fn cancel(&self) -> bool {
        let Ok(mut c) = self.0.lock() else {
            return false;
        };
        if matches!(c.stage, Stage::Preparing | Stage::Transferring) {
            c.cancel = true;
            true
        } else {
            false
        }
    }
    /// Whether a stop has been requested.
    ///
    /// A poisoned lock reports `true`, which stops the work: continuing to install while unable to
    /// read the cancellation state is the worse of the two failures.
    pub fn cancelled(&self) -> bool {
        self.0.lock().map(|c| c.cancel).unwrap_or(true)
    }
    /// Move to the next stage if the move is legal, and report whether it happened.
    ///
    /// Refuses any transition the stage machine does not allow, and refuses to move *into* work
    /// once a stop has been requested — so a cancellation cannot be overtaken by the very step it
    /// was meant to prevent.
    ///
    /// Returning `false` is not an error: the caller reads it and stops.
    pub fn transition(&self, next: Stage) -> bool {
        let Ok(mut c) = self.0.lock() else {
            return false;
        };
        let valid = matches!(
            (c.stage, next),
            (
                Stage::Preparing,
                Stage::Transferring | Stage::Failed | Stage::Cancelled
            ) | (
                Stage::Transferring,
                Stage::Installing | Stage::Failed | Stage::Cancelled
            ) | (
                Stage::Installing,
                Stage::Installed | Stage::Failed | Stage::Unknown
            )
        );
        if !valid || (c.cancel && matches!(next, Stage::Transferring | Stage::Installing)) {
            return false;
        }
        c.stage = next;
        true
    }
}
/// Atomic journal replacement. Contains no paths, device identifiers, or pairing material.
pub fn save(path: &Path, status: &JobStatus) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid job journal location.")?;
    std::fs::create_dir_all(parent).map_err(|_| "Cannot create private job storage.")?;
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| "Cannot create job journal.")?;
    serde_json::to_writer(&mut temp, status).map_err(|_| "Cannot encode job journal.")?;
    temp.flush().map_err(|_| "Cannot flush job journal.")?;
    temp.as_file()
        .sync_all()
        .map_err(|_| "Cannot sync job journal.")?;
    temp.persist(path).map_err(|_| "Cannot save job journal.")?;
    Ok(())
}
/// Read the journal after a restart and turn an interrupted installation into an honest outcome.
///
/// An install interrupted *while iOS was installing* becomes [`Stage::Unknown`]: the device may or
/// may not have the app, and Orbiter has no evidence either way. One interrupted before the
/// install command becomes [`Stage::Failed`], because nothing was committed. Neither is ever
/// upgraded to success, and a stage that was already terminal is returned untouched.
///
/// Both non-terminal cases set `cleanup_pending`, because a staging file may remain on the phone.
/// The corrected status is written back before being returned, so a second crash reaches the same
/// conclusion rather than reconsidering.
///
/// # Errors
///
/// Returns a message if the journal exists but is oversized or malformed, which disables automatic
/// retry rather than guessing at what it said.
pub fn recover(path: &Path) -> Result<Option<JobStatus>, String> {
    if !path.exists() {
        return Ok(None);
    }
    if std::fs::metadata(path)
        .map_err(|_| "Cannot inspect job journal.")?
        .len()
        > 16 * 1024
    {
        return Err("Invalid job journal; automatic retry is disabled.".into());
    }
    let bytes = std::fs::read(path)
        .map_err(|_| "Cannot read job journal. Check application storage permissions.")?;
    if bytes.len() > 16 * 1024 {
        return Err("Invalid job journal; automatic retry is disabled.".into());
    }
    let mut status: JobStatus = serde_json::from_slice(&bytes)
        .map_err(|_| "Invalid job journal; automatic retry is disabled.")?;
    match status.stage {
        Stage::Installing => {
            status.stage = Stage::Unknown;
            status.message="Orbiter stopped while iOS was installing. The outcome is unknown. Check the app on the phone before trying again.".into();
            status.cleanup_pending = true;
        }
        Stage::Preparing | Stage::Transferring => {
            status.stage = Stage::Failed;
            status.message="Installation was interrupted before the install command. Review the IPA again. A staging file may remain on the phone.".into();
            status.cleanup_pending = true;
        }
        _ => return Ok(Some(status)),
    }
    save(path, &status)?;
    Ok(Some(status))
}
#[cfg(test)]
/// Checks the two rules that keep an installation's outcome honest: cancellation cannot cross the
/// commit boundary, and recovery never invents success.
mod tests {
    use super::*;
    #[test]
    /// A stop requested during transfer prevents the install command from being sent, and the
    /// resulting `Cancelled` outcome cannot then be overwritten by a late `Installed`.
    fn cancellation_cannot_cross_commit_boundary() {
        let c = Control::default();
        assert!(c.transition(Stage::Transferring));
        assert!(c.cancel());
        assert!(!c.transition(Stage::Installing));
        assert!(c.transition(Stage::Cancelled));
        assert!(!c.transition(Stage::Installed));
    }
    #[test]
    /// Once iOS has been asked to install, cancellation is refused: the outcome belongs to the
    /// device, and `Unknown` is the honest answer rather than a claimed stop.
    fn device_install_cannot_be_cancelled() {
        let c = Control::default();
        assert!(c.transition(Stage::Transferring));
        assert!(c.transition(Stage::Installing));
        assert!(!c.cancel());
        assert!(c.transition(Stage::Unknown));
        assert!(!c.transition(Stage::Installed));
    }
    #[test]
    /// A journal left at `Installing` recovers to `Unknown` with cleanup pending, and recovering
    /// twice reaches the same conclusion rather than reconsidering it.
    fn recovery_never_invents_success() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("job.json");
        save(
            &p,
            &JobStatus {
                id: crate::domain::identifiers::JobId::new("synthetic"),
                stage: Stage::Installing,
                message: "Installing".into(),
                transferred_bytes: 1,
                total_bytes: 1,
                device_percent: Some(100),
                cleanup_pending: true,
            },
        )
        .unwrap();
        let s = recover(&p).unwrap().unwrap();
        assert_eq!(s.stage, Stage::Unknown);
        assert!(s.cleanup_pending);
        assert_eq!(recover(&p).unwrap().unwrap().stage, Stage::Unknown);
    }
}
