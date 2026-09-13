//! Review installing an IPA on a connected iPhone, and print what the review says.
//!
//! Read-only with respect to the phone: it verifies the file and the device and reports what an
//! installation would involve, then throws the review away. It transfers nothing and installs
//! nothing.

/// Review installing an IPA on a connected phone and print the review as JSON.
///
/// Prepares only: it reports what installing would do, including any blockers, and then discards
/// the review. **Nothing is transferred and nothing is installed**, so this is safe to run against
/// a real phone to see what Orbiter would say.
///
/// # Exit status
///
/// `0` when a review was produced, `1` when it could not be, `2` for wrong arguments.
///
/// # Panics
///
/// Panics only if a review that was just built cannot be serialised.
#[tokio::main]
async fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        eprintln!("Usage: orbiter-install-review <ipa> <transport-id>");
        std::process::exit(2)
    };
    let Some(id) = args
        .next()
        .and_then(|v| v.to_str().and_then(|v| v.parse().ok()))
    else {
        eprintln!("Provide the transport ID from orbiter-devices.");
        std::process::exit(2)
    };
    if args.next().is_some() {
        eprintln!("Too many arguments.");
        std::process::exit(2)
    }
    match orbiter_core::installation::prepare(path.into(), id).await {
        Ok(p) => println!(
            "{}",
            serde_json::to_string_pretty(&p.review).expect("serializable review")
        ),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    }
}
