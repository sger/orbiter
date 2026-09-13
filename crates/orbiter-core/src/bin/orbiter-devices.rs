//! List the iPhones this Mac can see, as JSON.
//!
//! Read-only: it reports what the local device transport finds and the pairing records that
//! already exist. It pairs nothing, installs nothing, and contacts no Apple service.

/// Print the discovery report and signal whether the device service was reachable at all.
///
/// # Exit status
///
/// `0` when the local device service answered, `1` when it did not — which distinguishes "no
/// iPhone is plugged in" from "this Mac cannot talk to iPhones", two situations that look the
/// same in the report alone.
///
/// # Panics
///
/// Panics only if a report that was just built cannot be serialised.
#[tokio::main]
async fn main() {
    let report = orbiter_core::devices::discover().await;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("serializable discovery")
    );
    if !report.service_available {
        std::process::exit(1);
    }
}
