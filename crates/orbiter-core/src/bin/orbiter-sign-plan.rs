//! Read-only: prints what re-signing would change. It signs nothing and contacts no Apple service.
use std::{path::PathBuf, sync::atomic::AtomicBool};
fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(team)) = (args.next(), args.next()) else {
        eprintln!("Usage: orbiter-sign-plan <ipa> <team-id> [--personal|--paid]");
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
