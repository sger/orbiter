//! Workflows, expressed once, in terms of the domain rather than of any user interface.
//!
//! A service here owns the coordination that used to live inside Tauri commands: which operations
//! may run at once, what a review authorises, when an attempt becomes durable, and what recovery
//! is allowed to conclude. Nothing in this module names `tauri`, an `AppHandle`, a `State` or a
//! `Channel`, so the same workflow can be driven by a command, a CLI binary, or a test with a fake
//! clock and no phone attached.
//!
//! Progress leaves through [`ProgressSink`], which the caller supplies. Tauri adapts it to a
//! channel; a test collects it into a vector.

pub mod guided;
pub mod installation;
pub mod ports;
pub mod runtime;
pub mod signing;

/// Somewhere for a running operation to report progress.
///
/// Implementations must be cheap and must not block: a sink is called from inside the operation,
/// including from its transfer loop, so anything slow here slows the work itself.
///
/// A sink that has gone away — a closed window, a dropped subscription — is not an error and must
/// not stop the operation. The work continues and its durable record is written regardless; see
/// [`crate::application::runtime`] for what a client does after reconnecting.
pub trait ProgressSink<T>: Send + Sync {
    /// Deliver one progress update, discarding it if nobody is listening.
    fn send(&self, update: T);
}

impl<T, F> ProgressSink<T> for F
where
    F: Fn(T) + Send + Sync,
{
    /// Treat any `Fn(T)` as a sink, so a closure or a channel's send method can be passed directly.
    fn send(&self, update: T) {
        self(update);
    }
}

/// A sink that drops everything, for callers with nothing to show.
///
/// Used by CLI entry points and by tests that assert on the durable record rather than on the
/// stream of updates.
pub struct Discard;

impl<T> ProgressSink<T> for Discard {
    /// Ignore the update.
    fn send(&self, _update: T) {}
}
