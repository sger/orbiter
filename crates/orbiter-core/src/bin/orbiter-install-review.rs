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
