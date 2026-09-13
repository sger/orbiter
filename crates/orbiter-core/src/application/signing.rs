//! Re-signing a saved build and keeping the result.
//!
//! This workflow used to live inside a Tauri command: pin the source, refuse a signed input,
//! choose an output directory, clean the marker, sign, retain, delete the staging file. None of it
//! could be driven by a test, and the rules were visible only by reading the command.
//!
//! # What contacts Apple
//!
//! [`SigningService::prepare`] does: it reserves app identifiers and downloads profiles, which are
//! remote mutations and take an acknowledgement. [`SigningService::sign`] does **not** — it uses
//! material already obtained and runs entirely on this Mac.
//!
//! # Ownership of the output
//!
//! Signing writes to a uniquely named staging file that this operation alone owns. The file is
//! removed only after the library has durably retained a copy, so a failure to record the result
//! leaves the generated build on disk rather than losing it. The IPA a person chose is only ever
//! read.
//!
//! # Concurrency
//!
//! The source artifact is leased for the whole run, so it cannot be removed or replaced while it
//! is being read. Signing is CPU-bound and runs on a blocking thread, never on the async runtime.

use std::{path::PathBuf, sync::Arc};

use super::ProgressSink;
use crate::{
    accounts::Accounts,
    domain::{
        errors::{ErrorCode, OperationError, OperationResult},
        identifiers::ArtifactId,
    },
    library::{Artifact, Library},
    plan::WatchChoice,
    signer::{Progress, Signed},
};

/// A signed build and the library record that now holds it.
#[derive(serde::Serialize)]
pub struct Retained {
    /// What signing did, including its log and the earliest profile expiry.
    pub signed: Signed,
    /// The saved version the output became, linked to the original it came from.
    pub artifact: Artifact,
}

/// Preparing provisioning, signing a saved original, and retaining the result.
///
/// Cheap to clone; clones share the same library and the same account session.
#[derive(Clone)]
pub struct SigningService {
    pub(super) library: Library,
    pub(super) accounts: Accounts,
    /// Where a run writes its output before the library takes a copy.
    staging: PathBuf,
    pub(super) guided: Arc<super::guided::State>,
}

impl SigningService {
    /// Build the service over a library, an account session, and a staging directory.
    pub fn new(library: Library, accounts: Accounts, staging: PathBuf) -> Self {
        Self {
            library,
            accounts,
            staging,
            guided: Arc::default(),
        }
    }

    /// Take a saved original for signing, refusing anything that is already a signed build.
    ///
    /// Returns the artifact, the path of its verified managed copy, and a lease that must be held
    /// for as long as the path is used.
    ///
    /// # Errors
    ///
    /// - [`ErrorCode::ArtifactMissing`] or [`ErrorCode::ArtifactChanged`] from the library.
    /// - [`ErrorCode::InvalidRequest`] when the artifact is a signed build: re-signing one would
    ///   stack signatures and identifier rewrites on top of each other.
    pub(super) async fn source(
        &self,
        artifact_id: &ArtifactId,
    ) -> OperationResult<(Artifact, PathBuf, crate::library::Lease)> {
        let library = self.library.clone();
        let wanted = artifact_id.clone();
        // Verifying the managed copy hashes the whole file; not work for a runtime thread.
        let (artifact, path, lease) = tokio::task::spawn_blocking(move || library.pin(&wanted))
            .await
            .map_err(|_| OperationError::new(ErrorCode::Internal, "Library worker stopped."))?
            .map_err(|cause| {
                if cause.contains("changed or is damaged") {
                    OperationError::artifact_changed(artifact_id)
                } else {
                    OperationError::artifact_missing(artifact_id)
                }
            })?;
        if artifact.source_id.is_some() {
            return Err(OperationError::new(
                ErrorCode::InvalidRequest,
                "Select an original version to sign.",
            ));
        }
        Ok((artifact, path, lease))
    }

    /// Reserve app identifiers and download profiles for a saved original.
    ///
    /// **Contacts Apple and mutates remote state.** Registering an identifier consumes one of a
    /// free personal team's ten per seven days and it can never be reused by another team, so this
    /// refuses without an acknowledgement and is never retried automatically.
    ///
    /// # Errors
    ///
    /// - [`ErrorCode::AcknowledgementRequired`] without an acknowledgement.
    /// - [`ErrorCode::ArtifactMissing`], [`ErrorCode::ArtifactChanged`] or
    ///   [`ErrorCode::InvalidRequest`] when the artifact is already a signed build.
    /// - [`ErrorCode::Internal`] carrying Apple's own explanation for a refusal, already redacted
    ///   of authentication detail.
    pub async fn prepare(
        &self,
        artifact_id: &ArtifactId,
        acknowledgement: super::installation::Acknowledgement,
        watch: WatchChoice,
        dylibs: Vec<std::path::PathBuf>,
    ) -> OperationResult<crate::accounts::Preparation> {
        let (_artifact, path, _lease) = self.source(artifact_id).await?;
        self.accounts
            .prepare_provisioning(
                path,
                acknowledgement == super::installation::Acknowledgement::Given,
                watch,
                dylibs,
            )
            .await
            .map_err(OperationError::internal)
    }

    /// Re-sign a saved original and keep the result as a new version beside it.
    ///
    /// Local only: no Apple request is made here. The output is written to a uniquely named
    /// staging file, retained by the library, and only then removed — so a failure to record the
    /// result leaves the build on disk with a message saying where it went.
    ///
    /// The returned [`Signed`] has its `path` rewritten to the managed copy, because the staging
    /// file no longer exists by the time a caller sees it.
    ///
    /// # Errors
    ///
    /// - [`ErrorCode::ArtifactMissing`], [`ErrorCode::ArtifactChanged`] or
    ///   [`ErrorCode::InvalidRequest`] when the artifact is already a signed build.
    /// - [`ErrorCode::AuthenticationRequired`] without a signed-in session, a selected team, or a
    ///   certificate.
    /// - [`ErrorCode::StorageWrite`] if the library cannot retain the output, in which case the
    ///   generated build is deliberately left where it was written.
    pub async fn sign(
        &self,
        artifact_id: &ArtifactId,
        watch: WatchChoice,
        marker: &str,
        dylibs: Vec<std::path::PathBuf>,
        progress: Arc<dyn ProgressSink<Progress>>,
    ) -> OperationResult<Retained> {
        let _gate = self.accounts.operation()?;
        self.sign_under_gate(artifact_id, watch, marker, dylibs, progress)
            .await
    }

    /// Sign and retain while the caller reserves the account for the whole sequence.
    /// Uses the same validation and durable retention as the public signing operation.
    pub(super) async fn sign_under_gate(
        &self,
        artifact_id: &ArtifactId,
        watch: WatchChoice,
        marker: &str,
        dylibs: Vec<std::path::PathBuf>,
        progress: Arc<dyn ProgressSink<Progress>>,
    ) -> OperationResult<Retained> {
        let (_source, path, lease) = self.source(artifact_id).await?;
        let cleaned = crate::signer::marker(marker);

        tracing::info!(operation = "signing", stage = "started");
        let signed = self
            .accounts
            .sign_ipa_under_gate(
                path,
                self.staging.clone(),
                watch,
                cleaned.clone(),
                dylibs,
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
                move |step| progress.send(step),
            )
            .await
            .inspect_err(
                |cause| tracing::info!(operation = "signing", stage = "failed", detail = %cause),
            )
            .map_err(classify_signing)?;

        // The same record the interface shows, so a terminal and a screenshot agree. These lines
        // are how a signing run is diagnosed after the fact.
        for line in &signed.log {
            tracing::info!(operation = "signing", detail = %line);
        }
        tracing::info!(
            operation = "signing",
            stage = "finished",
            bundles = signed.bundles_signed,
            removed = signed.removed.len()
        );

        let team_tag = signed.team_tag.clone().ok_or_else(|| {
            OperationError::new(
                ErrorCode::AuthenticationRequired,
                "Signing team metadata unavailable.",
            )
        })?;
        let library = self.library.clone();
        let source_id = artifact_id.clone();
        let watch_label = watch.label().to_owned();
        let marker_label = cleaned.unwrap_or_default();

        // Retaining copies and re-hashes the whole output; a blocking thread, not a runtime one.
        let retained = tokio::task::spawn_blocking(move || {
            let artifact = library.retain_signed(
                &source_id,
                &signed,
                team_tag,
                watch_label,
                marker_label,
            )?;
            // Durable before the staging file goes. `pin` verifies the managed copy and yields its
            // path; `open` would also re-inspect an archive that was just written.
            let (_, managed, _lease) = library.pin(&artifact.id)?;
            let _ = std::fs::remove_file(&signed.path);
            let mut signed = signed;
            signed.path = managed.to_string_lossy().into_owned();
            Ok::<_, String>(Retained { signed, artifact })
        })
        .await
        .map_err(|_| {
            OperationError::new(ErrorCode::Internal, "Signed artifact storage worker stopped.")
        })?
        .map_err(|cause| {
            OperationError::new(
                ErrorCode::StorageWrite,
                format!(
                    "Signing completed, but saving it to the library failed: {cause} The generated output has been retained."
                ),
            )
        })?;

        drop(lease);
        Ok(retained)
    }
}

/// Classify a signing failure that came back as a message.
///
/// The account layer still returns prose. The distinction worth preserving is "you are not signed
/// in far enough yet" from "something went wrong", because the first tells a person exactly which
/// step to go back to.
fn classify_signing(cause: String) -> OperationError {
    let missing = cause.contains("Sign in before")
        || cause.contains("Select the signing team")
        || cause.contains("Get a signing certificate");
    if missing {
        OperationError::new(ErrorCode::AuthenticationRequired, cause)
    } else {
        OperationError::internal(cause)
    }
}

#[cfg(test)]
/// Checks the rules signing enforces before it uses a certificate or contacts Apple.
///
/// None of these need credentials: each stops at or before the point a session would be required,
/// which is where the rules being checked actually live.
mod tests {
    use super::*;
    use crate::application::{Discard, runtime::Runtime};

    /// A runtime over a throwaway directory, plus the directory so it outlives the test.
    fn runtime() -> (tempfile::TempDir, Runtime) {
        let dir = tempfile::tempdir().expect("a temporary storage directory");
        let runtime = Runtime::new(dir.path());
        (dir, runtime)
    }

    #[tokio::test]
    /// Signing an artifact the library does not hold is refused as missing rather than attempted,
    /// so no session is touched and no certificate slot is spent on a build that does not exist.
    async fn an_unknown_artifact_is_refused_before_anything_is_spent() {
        let (_dir, runtime) = runtime();
        // `Retained` carries profile-derived material and deliberately has no `Debug`, so match
        // rather than unwrap the error out of the result.
        match runtime
            .signing()
            .sign(
                &ArtifactId::new("not-in-this-library"),
                WatchChoice::Sign,
                "test",
                Vec::new(),
                Arc::new(Discard),
            )
            .await
        {
            Err(error) => assert_eq!(error.code, ErrorCode::ArtifactMissing),
            Ok(_) => panic!("an unknown artifact must not be signable"),
        }
    }

    #[tokio::test]
    /// A build that is already signed cannot be signed again. Re-signing one would stack a second
    /// identifier rewrite and a second signature on top of the first.
    async fn a_signed_build_cannot_be_signed_again() {
        let (_dir, runtime) = runtime();
        let library = runtime.library();
        let ipa = crate::library::tests_support::fixture("one");
        let imported = library.import(ipa.path()).expect("the fixture imports");

        let output = crate::library::tests_support::fixture("one signed");
        let signed = Signed {
            team_tag: Some("team".into()),
            path: output.path().to_string_lossy().into_owned(),
            identifier: "test.library".into(),
            expires: "2099-01-01T00:00:00Z".into(),
            expires_unix: 4_070_908_800,
            bundles_signed: 1,
            removed: vec![],
            message: "signed".into(),
            log: vec![],
        };
        let retained = library
            .retain_signed(
                &imported.artifact_id,
                &signed,
                "team".into(),
                "sign".into(),
                "test".into(),
            )
            .expect("the signed build is retained");

        match runtime
            .signing()
            .sign(
                &retained.id,
                WatchChoice::Sign,
                "test",
                Vec::new(),
                Arc::new(Discard),
            )
            .await
        {
            Err(error) => assert_eq!(error.code, ErrorCode::InvalidRequest),
            Ok(_) => panic!("a signed build must not be signable again"),
        }
    }

    #[tokio::test]
    /// Preparing provisioning without an acknowledgement is refused. Reserving an app identifier
    /// spends one of a free team's ten per seven days and can never be undone, so it is never done
    /// on Orbiter's own initiative.
    async fn provisioning_without_an_acknowledgement_is_refused() {
        let (_dir, runtime) = runtime();
        let library = runtime.library();
        let ipa = crate::library::tests_support::fixture("one");
        let imported = library.import(ipa.path()).expect("the fixture imports");

        match runtime
            .signing()
            .prepare(
                &imported.artifact_id,
                super::super::installation::Acknowledgement::Missing,
                WatchChoice::Sign,
                Vec::new(),
            )
            .await
        {
            // The account layer still returns prose here, so what is asserted is that the request
            // was refused rather than sent.
            Err(error) => assert!(
                error.message.contains("acknowledge") || error.message.contains("Sign in"),
                "unexpected refusal: {}",
                error.message
            ),
            Ok(_) => panic!("unacknowledged provisioning must be refused"),
        }
    }

    #[tokio::test]
    /// Signing before sign-in, team selection and a certificate are in place is classified as
    /// needing authentication and names the step to go back to, rather than reporting a generic
    /// failure a person cannot act on.
    async fn signing_without_a_session_names_the_missing_step() {
        let (_dir, runtime) = runtime();
        let library = runtime.library();
        let ipa = crate::library::tests_support::fixture("one");
        let imported = library.import(ipa.path()).expect("the fixture imports");

        match runtime
            .signing()
            .sign(
                &imported.artifact_id,
                WatchChoice::Sign,
                "test",
                Vec::new(),
                Arc::new(Discard),
            )
            .await
        {
            Err(error) => {
                assert_eq!(error.code, ErrorCode::AuthenticationRequired);
                // Whichever step is missing, the message names it rather than saying only that
                // something went wrong: "before signing" is the shape all three refusals share.
                assert!(
                    error.message.contains("before signing"),
                    "a refusal must name the step to go back to, got: {}",
                    error.message
                );
            }
            Ok(_) => panic!("signing without a session must be refused"),
        }
    }
}
