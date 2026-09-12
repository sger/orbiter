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
