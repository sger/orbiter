//! Structured failures for the workflows this refactor owns.
//!
//! Every backend function used to return `Result<T, String>`, so a caller could not tell "this
//! artifact is gone" from "another operation is already running" without reading prose, and the
//! window distinguished failures by matching human-readable text. One sentence served as the
//! user's explanation, the developer's diagnosis and the frontend's control flow at once.
//!
//! An [`OperationError`] separates those three jobs:
//!
//! - [`ErrorCode`] is stable and machine-readable. The frontend switches on it; it never parses
//!   prose. Adding a variant is a deliberate change to a shared contract.
//! - [`OperationError::message`] is the sentence a person reads. It says what happened and what to
//!   do about it, and it is the only part shown on screen.
//! - [`OperationError::cause`] is an internal diagnostic for the log, absent unless it adds
//!   something the message does not.
//!
//! # Privacy
//!
//! No variant carries a credential, a raw UDID, an Apple authentication response or private key
//! material. Upstream errors that might are redacted before they reach a cause — see
//! `isideload::redacted_auth_error` — and the message is written for a screen that may be
//! photographed.

use std::fmt;

use super::identifiers::{ArtifactId, ReviewToken};

/// A stable, machine-readable classification of a failure.
///
/// Serialised in `snake_case` and matched by the frontend. The values are a contract: rename one
/// and the window stops recognising the failure it describes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// No saved artifact with that identity, or it has been removed.
    ArtifactMissing,
    /// The managed copy's bytes no longer hash to what the record says.
    ArtifactChanged,
    /// Another operation holds a resource this one needs.
    OperationInProgress,
    /// The installation review has expired, been consumed, or no longer matches.
    ReviewStale,
    /// A consequence has to be acknowledged before this may proceed.
    AcknowledgementRequired,
    /// No usable phone: absent, locked, unpaired, or not the reviewed one.
    DeviceUnavailable,
    /// The Apple account is not set up far enough: no session, an expired one, or no team chosen.
    ///
    /// One code for all three because they lead to the same place — back to the account step —
    /// and the message says which of them it is.
    AuthenticationRequired,
    /// Reading the library's own storage failed.
    StorageRead,
    /// Writing the library's own storage failed.
    StorageWrite,
    /// The stored manifest is unreadable or internally inconsistent.
    StorageCorrupt,
    /// The stored schema is newer than this build understands.
    StorageUnsupportedVersion,
    /// The operation was cancelled before it committed.
    Cancelled,
    /// iOS was asked to install and Orbiter never learned the result.
    OutcomeUnknown,
    /// The request itself was malformed.
    InvalidRequest,
    /// A failure with no more specific classification.
    Internal,
}

impl ErrorCode {
    /// The code as the string the frontend matches on.
    ///
    /// Kept beside the `serde` attribute deliberately: a log line and the IPC payload should not
    /// be able to spell the same code two ways.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ArtifactMissing => "artifact_missing",
            Self::ArtifactChanged => "artifact_changed",
            Self::OperationInProgress => "operation_in_progress",
            Self::ReviewStale => "review_stale",
            Self::AcknowledgementRequired => "acknowledgement_required",
            Self::DeviceUnavailable => "device_unavailable",
            Self::AuthenticationRequired => "authentication_required",
            Self::StorageRead => "storage_read",
            Self::StorageWrite => "storage_write",
            Self::StorageCorrupt => "storage_corrupt",
            Self::StorageUnsupportedVersion => "storage_unsupported_version",
            Self::Cancelled => "cancelled",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::InvalidRequest => "invalid_request",
            Self::Internal => "internal",
        }
    }
}

/// A failure with a code the frontend can act on, a sentence a person can read, and an optional
/// internal cause for the log.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize)]
pub struct OperationError {
    /// What kind of failure this is.
    pub code: ErrorCode,
    /// What to tell the person in front of the screen.
    pub message: String,
    /// Internal detail for diagnosis. Never rendered, and never a secret.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
}

impl OperationError {
    /// Build a failure from a code and the sentence a person will read.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            cause: None,
        }
    }

    /// Attach an internal diagnostic.
    ///
    /// The cause reaches the log, never the screen. Pass only what helps a developer and could be
    /// published without harm: never a credential, a raw UDID, or an upstream authentication body.
    pub fn because(mut self, cause: impl Into<String>) -> Self {
        self.cause = Some(cause.into());
        self
    }

    /// No saved artifact with that identity.
    pub fn artifact_missing(id: &ArtifactId) -> Self {
        Self::new(
            ErrorCode::ArtifactMissing,
            "This saved version is no longer in the library.",
        )
        .because(format!("artifact {id:?}"))
    }

    /// The managed copy's bytes no longer match the record.
    pub fn artifact_changed(id: &ArtifactId) -> Self {
        Self::new(
            ErrorCode::ArtifactChanged,
            "The saved IPA changed or is damaged. Import the original again before continuing.",
        )
        .because(format!("artifact {id:?}"))
    }

    /// Another operation holds a resource this one needs.
    pub fn operation_in_progress(what: &'static str) -> Self {
        Self::new(ErrorCode::OperationInProgress, what)
    }

    /// The review cannot authorise this installation any more.
    pub fn review_stale(token: Option<&ReviewToken>) -> Self {
        let error = Self::new(
            ErrorCode::ReviewStale,
            "Installation review is stale. Review again.",
        );
        match token {
            Some(token) => error.because(format!("review {token:?}")),
            None => error,
        }
    }

    /// A consequence has to be acknowledged first.
    pub fn acknowledgement_required(what: impl Into<String>) -> Self {
        Self::new(ErrorCode::AcknowledgementRequired, what)
    }

    /// Storage could not be read.
    pub fn storage_read(cause: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::StorageRead,
            "Cannot read the library. Check application storage permissions.",
        )
        .because(cause)
    }

    /// Storage could not be written.
    pub fn storage_write(cause: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::StorageWrite,
            "Cannot save to the library. Check disk space and permissions.",
        )
        .because(cause)
    }

    /// The operation was cancelled before it committed.
    pub fn cancelled(what: impl Into<String>) -> Self {
        Self::new(ErrorCode::Cancelled, what)
    }

    /// A failure with no better classification, carrying an existing message forward.
    ///
    /// The bridge for code that still returns `String`. Every use is a place this refactor has not
    /// reached yet, so it is worth being able to find them.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}

impl fmt::Display for OperationError {
    /// Write the user-facing sentence alone.
    ///
    /// The code and cause are deliberately absent: this is what reaches a screen when an error is
    /// formatted, and neither of those belongs there.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OperationError {}

impl From<OperationError> for String {
    /// Reduce a structured failure to the sentence a person reads.
    ///
    /// The compatibility bridge for command signatures that still return `Result<_, String>`. It
    /// loses the code, so a command that has been converted should return the error itself.
    fn from(error: OperationError) -> Self {
        error.message
    }
}

impl From<String> for OperationError {
    /// Carry a message from code that has not been classified yet.
    ///
    /// Makes `?` work on the many internal helpers that still return a sentence, so converting a
    /// workflow is a signature change rather than a rewrite of every call site. The result is
    /// [`ErrorCode::Internal`], which is the honest answer: this failure has no classification.
    ///
    /// Sites whose code a caller actually acts on construct the error directly instead.
    fn from(message: String) -> Self {
        Self::internal(message)
    }
}

impl From<&str> for OperationError {
    /// Carry a borrowed message, as above.
    fn from(message: &str) -> Self {
        Self::internal(message)
    }
}

impl From<crate::Error> for OperationError {
    /// Classify an inspection failure.
    ///
    /// Inspection's own messages already say what to do about each case, so they are carried
    /// through as the user-facing sentence rather than replaced.
    fn from(error: crate::Error) -> Self {
        let code = match error {
            crate::Error::Cancelled => ErrorCode::Cancelled,
            crate::Error::Io => ErrorCode::StorageRead,
            _ => ErrorCode::InvalidRequest,
        };
        Self::new(code, error.to_string())
    }
}

/// The result type for workflows this refactor owns.
pub type OperationResult<T> = Result<T, OperationError>;

#[cfg(test)]
/// Checks that failures stay classifiable, readable, and free of anything secret.
mod tests {
    use super::*;

    #[test]
    /// The wire form carries a stable code and the readable sentence, and omits the cause entirely
    /// when there is none — so the frontend switches on `code` and never parses prose.
    fn the_wire_form_is_a_code_and_a_sentence() {
        let error = OperationError::new(ErrorCode::ReviewStale, "Review again.");
        let json = serde_json::to_string(&error).expect("an error serialises");
        assert_eq!(json, r#"{"code":"review_stale","message":"Review again."}"#);
        assert_eq!(ErrorCode::ReviewStale.as_str(), "review_stale");
    }

    #[test]
    /// Every code's serialised spelling matches `as_str`, so a log line and an IPC payload cannot
    /// describe the same failure two different ways.
    fn every_code_spells_itself_one_way() {
        for code in [
            ErrorCode::ArtifactMissing,
            ErrorCode::ArtifactChanged,
            ErrorCode::OperationInProgress,
            ErrorCode::ReviewStale,
            ErrorCode::AcknowledgementRequired,
            ErrorCode::DeviceUnavailable,
            ErrorCode::AuthenticationRequired,
            ErrorCode::StorageRead,
            ErrorCode::StorageWrite,
            ErrorCode::StorageCorrupt,
            ErrorCode::StorageUnsupportedVersion,
            ErrorCode::Cancelled,
            ErrorCode::OutcomeUnknown,
            ErrorCode::InvalidRequest,
            ErrorCode::Internal,
        ] {
            let json = serde_json::to_string(&code).expect("a code serialises");
            assert_eq!(json, format!("\"{}\"", code.as_str()));
        }
    }

    #[test]
    /// Formatting an error yields only the sentence meant for a screen: the internal cause stays
    /// in the log, where a photograph of the window cannot reach it.
    fn displaying_an_error_shows_only_what_a_person_should_read() {
        let error = OperationError::storage_write("EACCES at /Users/someone/Library/…");
        assert_eq!(
            error.to_string(),
            "Cannot save to the library. Check disk space and permissions."
        );
        assert!(error.cause.is_some());
        assert!(!error.to_string().contains("/Users/"));
    }
}
