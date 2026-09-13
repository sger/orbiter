//! Device log capture, for answering "why did that screen fail?" with evidence.
//!
//! The iPhone's system log is the whole device's log: every app, every system service, and
//! whatever personal detail those happen to print. Orbiter does not want that and does not take
//! it. A capture is explicit, runs only while it is asked to, keeps only lines that mention the
//! app being diagnosed, holds them in memory, and writes nothing to disk.
//!
//! The transport is the syslog relay over usbmuxd. It carries the system and framework messages
//! about an app, but the app's own `NSLog`/`os_log` output may never reach it — modern iOS routes
//! those into the unified log (Console.app) instead. The absence of such a line here is therefore
//! not evidence the code did not run.
use idevice::{IdeviceError, IdeviceService, provider::UsbmuxdProvider, usbmuxd::UsbmuxdAddr};
use serde::Serialize;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// A capture stops on its own. Someone who starts one and walks away has not left the iPhone's
/// log streaming into this process indefinitely.
const MAX_DURATION: Duration = Duration::from_secs(5 * 60);
/// Enough to cover reproducing one failure, bounded so memory cannot grow without limit.
const MAX_LINES: usize = 500;
/// A single log line is a sentence or two. Anything longer is truncated rather than kept whole.
const MAX_LINE: usize = 600;

#[derive(Clone, Serialize)]
pub struct LogLine {
    pub text: String,
}

#[derive(Clone, Serialize)]
pub struct Summary {
    pub matched: usize,
    /// Lines the iPhone emitted that were read and discarded because they were about other apps.
    pub discarded: usize,
    pub message: String,
}

/// Reasons a capture is refused before the iPhone is contacted.
pub fn refusal(subjects: &[String]) -> Option<&'static str> {
    if subjects.is_empty() || subjects.iter().all(|subject| subject.trim().is_empty()) {
        return Some(
            "Sign a build first. A capture keeps only lines about the app being diagnosed, so it needs to know which app that is.",
        );
    }
    None
}

/// Whether a log line is about the app being diagnosed.
///
/// Case-insensitive substring: the system log names a process in several forms, and this is a
/// filter for keeping less, not a parser. `subjects` is what must appear, the first being the
/// signed build's identifier; `superseded` is the identifier of the build this one was made from.
///
/// Both apps are usually installed side by side and their processes share a name, so a line that
/// names the superseded build and not this one belongs to the other app and is dropped. The
/// rewritten identifier contains the original as a substring, so a line about this build names
/// both and survives.
fn about(line: &str, subjects: &[String], superseded: &[String]) -> bool {
    let line = line.to_ascii_lowercase();
    let mentions = |needles: &[String]| {
        needles
            .iter()
            .any(|needle| !needle.trim().is_empty() && line.contains(&needle.to_ascii_lowercase()))
    };
    if !mentions(subjects) {
        return false;
    }
    let this_build = subjects
        .first()
        .is_some_and(|identifier| line.contains(&identifier.to_ascii_lowercase()));
    !(mentions(superseded) && !this_build)
}

/// Trim a kept line to something a person reads, without the trailing newline the relay sends.
fn tidy(line: &str) -> String {
    let line = line.trim_end_matches(['\n', '\r', '\0']);
    if line.chars().count() > MAX_LINE {
        let cut: String = line.chars().take(MAX_LINE).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

/// Where the local device daemon listens.
///
/// Always the machine's own socket. Any environment variable naming a remote daemon is ignored:
/// a device log is streamed from a phone plugged into *this* Mac, and honouring a redirect would
/// send a capture request somewhere a person did not choose.
fn address() -> UsbmuxdAddr {
    // The same narrow, local-only transport discovery and installation use.
    #[cfg(unix)]
    {
        UsbmuxdAddr::UnixSocket("/var/run/usbmuxd".into())
    }
    #[cfg(not(unix))]
    {
        UsbmuxdAddr::TcpSocket(std::net::SocketAddr::from(([127, 0, 0, 1], 27015)))
    }
}

/// Replace a transport error with one sentence about what to do.
///
/// The underlying error is deliberately discarded rather than formatted: it can carry pairing and
/// address detail, and none of it helps someone whose phone is locked.
fn connection_error(_: IdeviceError) -> String {
    "Cannot read the iPhone's log. Unlock it, check trust and the connection, and try again.".into()
}

/// Open a connection to one attached phone for log streaming.
///
/// # Errors
///
/// Fails if the device daemon is unreachable, if no attached device has that number, or if the
/// phone is locked or untrusted — all reported as the same actionable sentence, since the fix is
/// the same and the difference would only leak pairing detail.
async fn provider(device_id: u32) -> Result<UsbmuxdProvider, String> {
    let mut mux = address().connect(0).await.map_err(connection_error)?;
    let raw = mux
        .get_devices()
        .await
        .map_err(connection_error)?
        .into_iter()
        .find(|device| device.device_id == device_id)
        .ok_or("Selected iPhone disconnected. Select it again.")?;
    Ok(raw.to_provider(address(), "Orbiter"))
}

/// Stream the iPhone's log, keeping only what is about `subjects`, until cancelled or bounded out.
pub async fn capture(
    device_id: u32,
    subjects: Vec<String>,
    superseded: Vec<String>,
    cancel: Arc<AtomicBool>,
    mut sink: impl FnMut(LogLine),
) -> Result<Summary, String> {
    if let Some(refusal) = refusal(&subjects) {
        return Err(refusal.into());
    }
    let provider = provider(device_id).await?;
    let mut client = tokio::time::timeout(
        Duration::from_secs(15),
        idevice::services::syslog_relay::SyslogRelayClient::connect(&provider),
    )
    .await
    .map_err(|_| "Opening the iPhone's log service timed out.".to_string())?
    .map_err(connection_error)?;

    let started = Instant::now();
    let mut matched = 0usize;
    let mut discarded = 0usize;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(Summary {
                matched,
                discarded,
                message: "Capture stopped. Nothing was written to disk.".into(),
            });
        }
        if started.elapsed() >= MAX_DURATION {
            return Ok(Summary {
                matched,
                discarded,
                message: "Capture stopped after five minutes. Start it again to keep going.".into(),
            });
        }
        if matched >= MAX_LINES {
            return Ok(Summary {
                matched,
                discarded,
                message: format!(
                    "Capture stopped after {MAX_LINES} matching lines. Start it again to keep going."
                ),
            });
        }
        // A quiet iPhone must not block cancellation, so reading is bounded too.
        let line = match tokio::time::timeout(Duration::from_secs(2), client.next()).await {
            Err(_) => continue,
            Ok(Ok(line)) => line,
            Ok(Err(_)) => {
                return Ok(Summary {
                    matched,
                    discarded,
                    message: "The iPhone closed its log connection. Reconnect and try again."
                        .into(),
                });
            }
        };
        if about(&line, &subjects, &superseded) {
            matched += 1;
            sink(LogLine { text: tidy(&line) });
        } else {
            // Counted, never kept: this is the rest of the device's log passing through.
            discarded += 1;
        }
    }
}

#[cfg(test)]
/// Checks that a capture keeps only lines about the build it was asked about.
mod tests {
    use super::*;

    #[test]
    /// A capture with no subject is refused. Streaming a whole phone's log and calling it a
    /// diagnosis of one app would be both useless and a privacy problem.
    fn a_capture_needs_to_know_which_app_it_is_about() {
        assert!(refusal(&[]).is_some());
        assert!(refusal(&["   ".to_string()]).is_some());
        assert!(refusal(&["com.example.app".to_string()]).is_none());
    }

    #[test]
    /// Lines mentioning the signed build are kept; every other line the device emits is counted
    /// and discarded rather than retained.
    fn only_lines_about_the_app_are_kept_and_the_rest_of_the_device_is_not() {
        let subjects = vec!["com.example.app.ab12".to_string(), "Example".to_string()];
        let superseded = vec!["com.example.app".to_string()];
        assert!(about(
            "Sep 12 21:40 iPhone com.example.app.ab12[431]: refused",
            &subjects,
            &superseded
        ));
        // The system log names processes in several cases; the filter must not miss those.
        assert!(about(
            "... COM.EXAMPLE.APP.AB12 ...",
            &subjects,
            &superseded
        ));
        // A line carrying only the process name is this app's: the other is told apart by id.
        assert!(about("iPhone Example(WebKit)[7702]: ready", &subjects, &[]));
        // Someone else's messages, someone else's business.
        assert!(!about(
            "Sep 12 21:40 iPhone Messages[88]: delivered to a friend",
            &subjects,
            &superseded
        ));
        assert!(!about(
            "Sep 12 21:40 iPhone locationd[77]: fix",
            &subjects,
            &superseded
        ));
    }

    #[test]
    /// A line naming only the superseded identifier is dropped: with both builds installed, the
    /// capture must describe the one that was just signed.
    fn the_company_build_installed_beside_this_one_is_not_mistaken_for_it() {
        let subjects = vec!["com.example.app.ab12".to_string(), "Example".to_string()];
        let superseded = vec!["com.example.app".to_string()];
        // Both apps run a process called Example, so the identifier is what separates them.
        assert!(!about(
            "Data Usage for com.example.app on flow 1318991",
            &subjects,
            &superseded
        ));
        // The rewritten identifier contains the original, so this build's lines still match.
        assert!(about(
            "Data Usage for com.example.app.ab12 on flow 1318991",
            &subjects,
            &superseded
        ));
    }

    #[test]
    /// A kept line is bounded before it is forwarded, so one enormous log line cannot become the
    /// whole capture.
    fn a_kept_line_is_trimmed_rather_than_stored_whole() {
        assert_eq!(tidy("ready\n\u{0}"), "ready");
        let long = "x".repeat(MAX_LINE + 50);
        let kept = tidy(&long);
        assert!(kept.ends_with('…'));
        assert_eq!(kept.chars().count(), MAX_LINE + 1);
    }
}
