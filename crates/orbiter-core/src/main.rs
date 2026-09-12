use std::{path::PathBuf, sync::atomic::AtomicBool};
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
