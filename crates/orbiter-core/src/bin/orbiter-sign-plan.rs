//! Read-only: prints what re-signing would change. It signs nothing and contacts no Apple service.
use std::{path::PathBuf, sync::atomic::AtomicBool};
/// Print what re-signing the given IPA under the given team would change.
///
/// Pure local computation: it signs nothing, writes nothing and contacts no Apple service. Useful
/// for seeing which capabilities a team would lose, and what a Watch app would cost, before
/// deciding anything.
///
/// # Exit status
///
/// `0` when a plan was produced, `1` when the IPA could not be inspected, `2` for wrong arguments.
///
/// # Panics
///
/// Panics only if a plan that was just built cannot be serialised.
fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(team)) = (args.next(), args.next()) else {
        eprintln!(
            "Usage: orbiter-sign-plan <ipa> <team-id> [--personal|--paid] [--watch-remove|--watch-sign]"
        );
        std::process::exit(2)
    };
    let kind = match args.next().as_deref() {
        None | Some("--personal") => orbiter_core::plan::TeamKind::Personal,
        Some("--paid") => orbiter_core::plan::TeamKind::Paid,
        Some(_) => {
            eprintln!("Expected --personal or --paid.");
            std::process::exit(2)
        }
    };
    let watch = match args.next().as_deref() {
        None => orbiter_core::plan::WatchChoice::Undecided,
        Some("--watch-remove") => orbiter_core::plan::WatchChoice::Remove,
        Some("--watch-sign") => orbiter_core::plan::WatchChoice::Sign,
        Some(_) => {
            eprintln!("Expected --watch-remove or --watch-sign.");
            std::process::exit(2)
        }
    };
    if args.next().is_some() {
        eprintln!("Too many arguments.");
        std::process::exit(2)
    }
    let report = match orbiter_core::inspect(&PathBuf::from(path), &AtomicBool::new(false), |_| {})
    {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1)
        }
    };
    let plan = orbiter_core::plan::build(
        &report,
        &orbiter_core::plan::Target {
            team_id: team,
            kind,
            watch,
        },
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&plan).expect("serializable plan")
    );
    // A plan with blockers is not a signable plan; say so in the exit status too.
    if !plan.blockers.is_empty() {
        std::process::exit(1)
    }
}
