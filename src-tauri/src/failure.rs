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
