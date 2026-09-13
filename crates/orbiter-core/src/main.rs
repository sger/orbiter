//! Inspect one IPA and print the report as JSON.
//!
//! The whole inspection pipeline without the desktop shell: useful for looking at a build from a
//! terminal, and for checking that a failure is the inspector's rather than the interface's.
//! Reads the file and nothing else — no network, no credentials, no writes.

use std::{path::PathBuf, sync::atomic::AtomicBool};
/// Inspect the IPA named by the single argument and print its report.
///
/// # Exit status
///
/// `0` on success, `1` if the archive cannot be inspected, `2` if the arguments are wrong.
/// Cancellation is never requested here, so the inspection always runs to completion.
///
/// # Panics
///
/// Panics only if a report that was just built cannot be serialised, which would mean a bug in
/// this crate rather than anything about the file.
fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        eprintln!("Usage: cargo run -p orbiter-core -- <file.ipa>");
        std::process::exit(2)
    };
    if args.next().is_some() {
        eprintln!("Expected exactly one IPA path.");
        std::process::exit(2)
    }
    match orbiter_core::inspect(&PathBuf::from(path), &AtomicBool::new(false), |_| {}) {
        Ok(r) => println!(
            "{}",
            serde_json::to_string_pretty(&r).expect("serializable report")
        ),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    }
}
