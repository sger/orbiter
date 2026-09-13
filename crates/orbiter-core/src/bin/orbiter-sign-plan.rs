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
            "Usage: orbiter-sign-plan <ipa> <team-id> [--personal|--paid] [--watch-remove|--watch-sign] [--inject <name>]..."
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
    let mut watch = orbiter_core::plan::WatchChoice::Undecided;
    let mut injected_dylibs = Vec::new();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--watch-remove" => watch = orbiter_core::plan::WatchChoice::Remove,
            "--watch-sign" => watch = orbiter_core::plan::WatchChoice::Sign,
            "--inject" => match args.next() {
                Some(name) => injected_dylibs.push(name),
                None => {
                    eprintln!("--inject needs a library name.");
                    std::process::exit(2)
                }
            },
            _ => {
                eprintln!("Expected --watch-remove, --watch-sign, or --inject <name>.");
                std::process::exit(2)
            }
        }
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
            injected_dylibs,
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
