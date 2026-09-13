//! Identifiers that cannot be mistaken for one another.
//!
//! Every identifier in this codebase used to be a `String`, so `remove(app_id, artifact_id)` and
//! `expiry(artifact_id)` accepted each other's arguments without complaint. The types here are
//! newtypes over `String` with no mutual conversions: turning one into another takes a deliberate
//! call that names both kinds, which is the point.
//!
//! Three device identities exist and only one of them may ever be written down:
//!
//! - [`UsbDeviceId`] is the ephemeral usbmuxd number. It changes between connections and means
//!   nothing after the cable is unplugged.
//! - The raw UDID is the phone's permanent hardware identity. It is handled in memory to talk to
//!   the device and to derive the tag below, and it is never persisted or serialised. There is
//!   deliberately no type for it here; it stays a `&str` that crosses as few functions as possible.
//! - [`RememberedDeviceId`] is a salted hash of the UDID, stable for one library and meaningless
//!   outside it. This is the one the library stores.
//!
//! # Serialised form
//!
//! Every type here is `#[serde(transparent)]`, so a record written before these types existed
//! reads back unchanged and a record written now is readable by any JSON consumer. The refactor
//! must not rewrite a single stored identifier.

use serde::{Deserialize, Serialize};
use std::fmt;

use super::errors::{ErrorCode, OperationError};

/// The longest identifier this process will accept from outside it.
///
/// Every identifier Orbiter generates is a UUID or a hex digest, so the real values are well under
/// this. The bound exists so a malformed request cannot make the backend carry an arbitrary string
/// into a filename, a log line or a manifest.
const MAX_LENGTH: usize = 200;

/// Declare a newtype over `String` with the shared accessors, formatting and serde behaviour.
///
/// A macro rather than a generic wrapper: a generic `Id<Tag>` would make every identifier the same
/// type to `rustc` if the tag were ever elided, and the whole purpose is that they are not.
macro_rules! identifier {
    ($(#[$meta:meta])* $name:ident, $label:literal) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wrap an existing identifier, whatever its shape.
            ///
            /// Used when reading a value this process already produced — from the manifest, or
            /// from another part of the core. Values arriving from outside should go through the
            /// validating constructor on the type instead, where one exists.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Accept an identifier arriving from outside this process.
            ///
            /// Tauri commands and CLI arguments go through this rather than [`Self::new`]: an
            /// identifier that reaches a filename or a manifest should have been checked at the
            /// edge, once, rather than trusted everywhere.
            ///
            /// # Errors
            ///
            /// Returns [`ErrorCode::InvalidRequest`] for an empty value, one longer than 200
            /// bytes, or one containing control characters or path separators.
            pub fn parse(value: &str) -> Result<Self, OperationError> {
                let value = value.trim();
                if value.is_empty()
                    || value.len() > MAX_LENGTH
                    || value.contains(['/', '\\', '\0'])
                    || value.chars().any(char::is_control)
                {
                    return Err(OperationError::new(
                        ErrorCode::InvalidRequest,
                        concat!("The ", $label, " in this request is not valid."),
                    ));
                }
                Ok(Self(value.to_owned()))
            }

            /// The identifier as a string slice, for comparison, hashing and display.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume the wrapper and return the inner string.
            ///
            /// Deliberately not `From<Self> for String`: an explicit call is easy to search for
            /// when asking which code still needs the untyped form.
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            /// Write the identifier exactly as stored, with no decoration.
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            /// Show the kind alongside the value, so a log line says which identifier it is.
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", $label, self.0)
            }
        }

        impl AsRef<str> for $name {
            /// Borrow the identifier as a string slice.
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

identifier!(
    /// One app in the library, grouping every saved version that shares a bundle identifier.
    ///
    /// Generated once on first import and never regenerated: it is what installation history is
    /// attributed to, so a new value would orphan the history rather than move it.
    AppId,
    "AppId"
);

identifier!(
    /// One saved IPA — an imported original, or a signed build produced from one.
    ///
    /// Distinct bytes are always a distinct artifact, even when the version and build labels match.
    ArtifactId,
    "ArtifactId"
);

identifier!(
    /// One installation attempt, from the moment it is recorded to its terminal outcome.
    ///
    /// The same value as the [`ReviewToken`] that authorised it, so the durable journal and the
    /// library's history describe one event rather than two. [`JobId::from_review`] is the only
    /// way to make that connection, and it is deliberately one-directional.
    JobId,
    "JobId"
);

identifier!(
    /// Authorisation to install one exact artifact on one exact device.
    ///
    /// Bound in memory to a private snapshot of the reviewed bytes, their SHA-256, and the
    /// verified device. It expires, and it is consumed by the installation it authorises.
    ReviewToken,
    "ReviewToken"
);

identifier!(
    /// A phone the library remembers: a salted hash of its UDID, stable within one library.
    ///
    /// Deriving it needs both the library's salt and the raw UDID, so it cannot be produced by
    /// accident and cannot be reversed into the hardware identity it stands for.
    RememberedDeviceId,
    "RememberedDeviceId"
);

impl JobId {
    /// Take the identity of the review that authorised this installation.
    ///
    /// The durable journal keys on the job and the library's history keys on the attempt; sharing
    /// one value is what lets recovery match a recorded outcome to the attempt it belongs to.
    /// There is no inverse: a job id must not be usable to authorise a fresh installation.
    pub fn from_review(token: &ReviewToken) -> Self {
        Self(token.0.clone())
    }
}

/// The ephemeral usbmuxd number for a connected phone.
///
/// Valid only for as long as the device stays attached, and meaningless across reconnections —
/// which is exactly why it is a separate type from [`RememberedDeviceId`]. Storing one would
/// record something that stops being true the moment a cable is unplugged.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UsbDeviceId(u32);

impl UsbDeviceId {
    /// Wrap a number reported by the local device transport.
    pub fn new(value: u32) -> Self {
        Self(value)
    }

    /// The number the transport expects back.
    pub fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for UsbDeviceId {
    /// Write the bare number, as the transport's own diagnostics do.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
/// Checks that the identifier types keep their stored shape and stay distinct from one another.
mod tests {
    use super::*;

    #[test]
    /// A typed identifier serialises as the bare string it wraps, so records written before these
    /// types existed read back unchanged and records written now stay plain JSON.
    fn the_stored_form_is_the_bare_string() {
        let artifact = ArtifactId::new("6f1c-…");
        assert_eq!(
            serde_json::to_string(&artifact).expect("an identifier serialises"),
            "\"6f1c-…\""
        );
        let read: ArtifactId =
            serde_json::from_str("\"6f1c-…\"").expect("an identifier round-trips");
        assert_eq!(read, artifact);
        // And inside a record, the field is indistinguishable from the String it replaced.
        #[derive(Serialize)]
        struct Row {
            app_id: AppId,
        }
        assert_eq!(
            serde_json::to_string(&Row {
                app_id: AppId::new("app-1")
            })
            .expect("a record serialises"),
            r#"{"app_id":"app-1"}"#
        );
    }

    #[test]
    /// A job takes the identity of the review that authorised it, which is what lets crash
    /// recovery match a journalled outcome to the attempt it belongs to.
    fn a_job_carries_the_identity_of_its_review() {
        let token = ReviewToken::new("review-1");
        assert_eq!(JobId::from_review(&token).as_str(), token.as_str());
    }

    #[test]
    /// Debug output names the kind, so a log line says which identifier it is rather than leaving
    /// a bare string for a reader to guess at.
    fn debug_output_names_the_kind() {
        assert_eq!(format!("{:?}", AppId::new("x")), "AppId(x)");
        assert_eq!(
            format!("{:?}", RememberedDeviceId::new("y")),
            "RememberedDeviceId(y)"
        );
    }
}
