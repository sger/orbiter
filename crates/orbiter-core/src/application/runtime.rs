//! The application runtime: the object that owns Orbiter's shared, long-lived state.
//!
//! Before this existed, coordination lived in two process-global `static`s inside the library and
//! in whatever Tauri happened to be managing. Nothing owned it, nothing could be constructed twice
//! for a test, and a command could quietly build a second `Library` that did not share the first
//! one's lease table — so a file could be deleted while an installation was reading it.
//!
//! A [`Runtime`] is constructed once at startup from a storage directory, and everything that
//! needs shared state takes a handle to it. Constructing a second one in a test is deliberate and
//! isolated: it has its own directory, its own locks and its own leases.
//!
//! # State ownership
//!
//! | State | Owner | Shared how |
//! |---|---|---|
//! | Library metadata lock, lease table | [`crate::library::Library`] | `Arc` inside the library; cloning shares it |
//! | Installation gate, prepared review, live status | [`InstallationService`] | `Arc` inside the service; cloning shares it |
//! | Account session, teams, certificate | [`Accounts`] | `Arc` inside; cloning shares it |
//! | Installation journal path | [`Runtime`] | Copied where needed; it is a path, not state |
//!
//! # Locking
//!
//! The runtime itself holds no lock. Every lock in Orbiter belongs to the component that owns the
//! state it guards, and the ordering rules are documented there.

use std::path::{Path, PathBuf};

use crate::{
    accounts::Accounts,
    application::{installation::InstallationService, signing::SigningService},
    domain::identifiers::{RememberedDeviceId, UsbDeviceId},
    library::Library,
};

/// Orbiter's shared, long-lived state.
///
/// Cheap to clone: every field is either a path or an `Arc`-backed handle, so a clone refers to
/// the same locks and the same leases rather than to a second copy of them.
#[derive(Clone)]
pub struct Runtime {
    /// The one library for this storage directory.
    library: Library,
    /// The one installation service, holding the gate every install and removal contends for.
    installations: InstallationService,
    /// The one signing service, sharing this runtime's library and account session.
    signing: SigningService,
    /// The one account session: who Orbiter is signed in as, and which team is selected.
    accounts: Accounts,
    /// Where the current installation's durable journal lives.
    journal: PathBuf,
}

impl Runtime {
    /// Build the runtime over an application storage directory.
    ///
    /// Creates nothing on disk; the directory is touched the first time something is written.
    /// Call this once per process, at startup, and clone the result.
    ///
    /// `storage` is the platform's per-application data directory — `app_data_dir()` under Tauri,
    /// a temporary directory in a test.
    pub fn new(storage: &Path) -> Self {
        let library = Library::new(storage.join("library"));
        let journal = storage.join("last-install.json");
        let accounts = Accounts::default();
        Self {
            installations: InstallationService::new(library.clone(), journal.clone()),
            signing: SigningService::new(library.clone(), accounts.clone(), storage.join("signed")),
            accounts,
            library,
            journal,
        }
    }

    /// The one library for this runtime.
    ///
    /// Returns a handle sharing the same coordination state, never a fresh library: two libraries
    /// over one directory would not see each other's leases.
    pub fn library(&self) -> Library {
        self.library.clone()
    }

    /// The one installation service for this runtime.
    ///
    /// Returns a handle sharing the same gate and live state. Library removal also takes that
    /// gate, which is what stops a saved file disappearing while a review points at it.
    pub fn installations(&self) -> InstallationService {
        self.installations.clone()
    }

    /// The one signing service for this runtime.
    ///
    /// Shares this runtime's library and account session, so a build it retains is visible to the
    /// same library every other service sees.
    pub fn signing(&self) -> SigningService {
        self.signing.clone()
    }

    /// The one account session for this runtime.
    ///
    /// Authentication, two-factor challenges, session lifetime and team selection. What a session
    /// is *used for* lives in [`SigningService`] and [`crate::accounts::provisioning`].
    pub fn accounts(&self) -> Accounts {
        self.accounts.clone()
    }

    /// Where the current installation's durable journal lives.
    ///
    /// The journal records one installation's stage so a crash can be reconciled at startup. It is
    /// deliberately a single file: Orbiter runs one installation at a time.
    pub fn journal(&self) -> &Path {
        &self.journal
    }

    /// The library's tag for an attached phone, so a remembered device can be recognised.
    ///
    /// The two halves of this question live in different places on purpose: the transport knows
    /// which phone is on the cable, and only the library holds the salt that turns its identity
    /// into the tag written in history. This is the one place they meet, and it is here rather
    /// than in either of them so the raw UDID never leaves the crate — it is read, hashed, and
    /// dropped inside this function. The number returned is meaningless outside this library.
    ///
    /// Used to answer one question: is the phone connected now the phone that expiring build was
    /// installed to? A wrong answer there sends someone to re-sign for the wrong tester, so the
    /// caller is expected to treat a failure as "cannot tell" rather than as "no".
    ///
    /// # Errors
    ///
    /// Returns the device layer's own sentence if no attached phone has that number, or if it is
    /// locked, untrusted, or unreachable; and the library's if the manifest cannot be read.
    pub async fn remembered_device(
        &self,
        device: UsbDeviceId,
    ) -> Result<RememberedDeviceId, String> {
        let (udid, _) = crate::installation::verified_identity(device.get()).await?;
        let library = self.library.clone();
        tokio::task::spawn_blocking(move || library.device_tag(&udid))
            .await
            .map_err(|_| "Library worker stopped.".to_string())?
    }

    /// Reconcile durable state with reality after a restart.
    ///
    /// Reads the installation journal, downgrades an interrupted stage to a non-committal outcome,
    /// and lets the library apply the same conclusion to its own history and finish any file
    /// removals a person had already asked for.
    ///
    /// Never infers success: an installation interrupted while iOS was installing becomes
    /// `Unknown`, not `Installed`, because Orbiter did not see the result and has no evidence for
    /// one. Nothing is restarted or queued.
    ///
    /// # Errors
    ///
    /// Returns the library's own failure if its manifest cannot be read or written. An unreadable
    /// journal is *not* an error: it degrades to "nothing recorded", so a damaged file cannot stop
    /// the application from starting.
    pub fn recover(&self) -> Result<(), String> {
        let journal = crate::installation::job::recover(&self.journal).unwrap_or(None);
        self.library.recover(journal.as_ref())
    }
}

#[cfg(test)]
/// Checks that the runtime shares one library rather than handing out independent copies.
mod tests {
    use super::*;

    #[test]
    /// Two handles taken from one runtime share a lease table, so a file another operation is
    /// using cannot be deleted through the second handle. Before the runtime existed, each
    /// command built its own library and this protection silently did not apply.
    fn handles_from_one_runtime_share_their_leases() {
        let dir = tempfile::tempdir().expect("a temporary storage directory");
        let runtime = Runtime::new(dir.path());
        let first = runtime.library();
        let second = runtime.library();

        let ipa = crate::library::tests_support::fixture("one");
        let imported = first.import(ipa.path()).expect("the fixture imports");
        let (_, _, lease) = first.pin(&imported.artifact_id).expect("the artifact pins");

        assert!(
            second
                .remove(&imported.app_id, Some(&imported.artifact_id))
                .is_err(),
            "a leased artifact must be refused to a second handle on the same library"
        );
        drop(lease);
        second
            .remove(&imported.app_id, Some(&imported.artifact_id))
            .expect("removal succeeds once the lease is gone");
    }

    #[test]
    /// Separate runtimes are isolated: one test's library cannot block another's, which is what
    /// the process-global locks used to make impossible.
    fn separate_runtimes_do_not_share_state() {
        let one = tempfile::tempdir().expect("a temporary storage directory");
        let two = tempfile::tempdir().expect("a temporary storage directory");
        assert_ne!(
            Runtime::new(one.path()).journal(),
            Runtime::new(two.path()).journal()
        );
    }
}
