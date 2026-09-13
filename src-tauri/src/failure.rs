//! Turning a backend failure into something the window can act on.
//!
//! The frontend used to tell failures apart by matching human-readable text, so rewording a
//! sentence could change what the interface did. A [`Failure`] carries a stable `code` alongside
//! the sentence, and React switches on the code.
//!
//! # Privacy
//!
//! Only `code` and `message` cross to the window. The internal `cause` is logged here and never
//! serialised: it is written for a developer, and the window may be photographed.

use orbiter_core::domain::errors::{ErrorCode, OperationError};

/// A backend failure, as the window receives it.
///
/// Serialises as `{ "code": "review_stale", "message": "…" }`. The code is a contract with the
/// frontend; changing one is a change to both sides.
#[derive(serde::Serialize)]
pub struct Failure {
    /// Stable, machine-readable classification.
    pub code: ErrorCode,
    /// The sentence a person reads. The only human-readable text that crosses.
    pub message: String,
}

impl From<OperationError> for Failure {
    /// Drop the internal cause into the log and keep what the window may see.
    fn from(error: OperationError) -> Self {
        if let Some(cause) = &error.cause {
            tracing::info!(code = error.code.as_str(), detail = %cause, "operation failed");
        }
        Self {
            code: error.code,
            message: error.message,
        }
    }
}

impl From<String> for Failure {
    /// Carry a message from a workflow this refactor has not reached yet.
    ///
    /// Everything still returning `Result<_, String>` arrives here and is classified as
    /// [`ErrorCode::Internal`]. Each use is a place the structured errors have yet to reach, so
    /// it is worth being able to find them.
    fn from(message: String) -> Self {
        Self {
            code: ErrorCode::Internal,
            message,
        }
    }
}

impl From<&str> for Failure {
    /// Carry a borrowed message, as above.
    fn from(message: &str) -> Self {
        Self::from(message.to_owned())
    }
}

/// A failure with no better classification.
///
/// Used by the few adapters that still bridge string-returning code.
pub fn internal(message: impl Into<String>) -> Failure {
    Failure {
        code: ErrorCode::Internal,
        message: message.into(),
    }
}

#[cfg(test)]
/// Checks what crosses to the window and what stays in the log.
mod tests {
    use super::*;

    #[test]
    /// The wire form is a stable code and the sentence a person reads, and nothing else. The
    /// internal cause is logged, never serialised: it can name a path on someone's disk, and the
    /// window may be photographed.
    fn only_the_code_and_the_sentence_cross_to_the_window() {
        let failure = Failure::from(OperationError::storage_write(
            "EACCES at /Users/someone/Library/Application Support",
        ));
        let json = serde_json::to_string(&failure).expect("a failure serialises");
        assert_eq!(
            json,
            r#"{"code":"storage_write","message":"Cannot save to the library. Check disk space and permissions."}"#
        );
        assert!(!json.contains("/Users/"));
        assert!(!json.contains("EACCES"));
    }

    #[test]
    /// Every code reaches the window spelled exactly as the frontend's own union expects, so a
    /// renamed variant is a compile-time change on one side and a visible failure on the other,
    /// rather than a silently unmatched string.
    fn every_code_crosses_with_its_documented_spelling() {
        for (code, expected) in [
            (ErrorCode::ArtifactMissing, "artifact_missing"),
            (ErrorCode::ArtifactChanged, "artifact_changed"),
            (ErrorCode::OperationInProgress, "operation_in_progress"),
            (ErrorCode::ReviewStale, "review_stale"),
            (
                ErrorCode::AcknowledgementRequired,
                "acknowledgement_required",
            ),
            (ErrorCode::DeviceUnavailable, "device_unavailable"),
            (ErrorCode::AuthenticationRequired, "authentication_required"),
            (ErrorCode::StorageRead, "storage_read"),
            (ErrorCode::StorageWrite, "storage_write"),
            (ErrorCode::StorageCorrupt, "storage_corrupt"),
            (
                ErrorCode::StorageUnsupportedVersion,
                "storage_unsupported_version",
            ),
            (ErrorCode::Cancelled, "cancelled"),
            (ErrorCode::OutcomeUnknown, "outcome_unknown"),
            (ErrorCode::InvalidRequest, "invalid_request"),
            (ErrorCode::Internal, "internal"),
        ] {
            let failure = Failure::from(OperationError::new(code, "anything"));
            let json = serde_json::to_string(&failure).expect("a failure serialises");
            assert!(
                json.contains(&format!(r#""code":"{expected}""#)),
                "{code:?} crossed as {json}"
            );
        }
    }

    #[test]
    /// A workflow this refactor has not reached yet still crosses as a failure the window can
    /// render, classified as `internal` — which is the honest answer: nobody has classified it.
    fn an_unclassified_message_still_crosses_legibly() {
        let failure = Failure::from("Something specific went wrong.".to_owned());
        assert_eq!(failure.code, ErrorCode::Internal);
        assert_eq!(failure.message, "Something specific went wrong.");
    }
}
