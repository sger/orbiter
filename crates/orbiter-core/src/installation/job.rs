use serde::{Deserialize, Serialize};
use std::{io::Write, path::Path, sync::Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Installed | Self::Failed | Self::Cancelled | Self::Unknown
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatus {
    pub id: String,
    pub stage: Stage,
    pub message: String,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub device_percent: Option<u64>,
    pub cleanup_pending: bool,
}
struct Inner {
    stage: Stage,
    cancel: bool,
}
pub struct Control(Mutex<Inner>);
impl Default for Control {
    fn default() -> Self {
        Self(Mutex::new(Inner {
            stage: Stage::Preparing,
            cancel: false,
        }))
    }
}
impl Control {
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
    pub fn cancelled(&self) -> bool {
        self.0.lock().map(|c| c.cancel).unwrap_or(true)
    }
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
mod tests {
    use super::*;
    #[test]
    fn cancellation_cannot_cross_commit_boundary() {
        let c = Control::default();
        assert!(c.transition(Stage::Transferring));
        assert!(c.cancel());
        assert!(!c.transition(Stage::Installing));
        assert!(c.transition(Stage::Cancelled));
        assert!(!c.transition(Stage::Installed));
    }
    #[test]
    fn device_install_cannot_be_cancelled() {
        let c = Control::default();
        assert!(c.transition(Stage::Transferring));
        assert!(c.transition(Stage::Installing));
        assert!(!c.cancel());
        assert!(c.transition(Stage::Unknown));
        assert!(!c.transition(Stage::Installed));
    }
    #[test]
    fn recovery_never_invents_success() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("job.json");
        save(
            &p,
            &JobStatus {
                id: "synthetic".into(),
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
