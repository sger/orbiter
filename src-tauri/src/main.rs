#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{State, ipc::Channel};
#[derive(Default, Clone)]
struct Inspection {
    busy: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}
struct Release(Arc<AtomicBool>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
#[tauri::command]
async fn inspect_ipa(
    path: String,
    progress: Channel<&'static str>,
    state: State<'_, Inspection>,
) -> Result<orbiter_core::Report, String> {
    if state
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("An inspection is already running.".into());
    }
    state.cancel.store(false, Ordering::SeqCst);
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let _release = Release(state.busy.clone());
        tracing::info!(operation = "inspection", stage = "started");
        let result = orbiter_core::inspect(&PathBuf::from(path), &state.cancel, |stage| {
            let _ = progress.send(stage);
        });
        tracing::info!(
            operation = "inspection",
            success = result.is_ok(),
            stage = "finished"
        );
        result.map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "Inspection worker stopped. Retry with a fresh IPA.".to_string())?
}
#[tauri::command]
fn cancel_inspection(state: State<'_, Inspection>) {
    state.cancel.store(true, Ordering::SeqCst);
}
#[tauri::command]
async fn discover_devices() -> orbiter_core::devices::Discovery {
    orbiter_core::devices::discover().await
}
fn main() {
    use tracing_subscriber::prelude::*;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_target(false)
                .with_filter(
                    tracing_subscriber::filter::Targets::new()
                        .with_target("orbiter", tracing::Level::INFO),
                ),
        )
        .init();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Inspection::default())
        .invoke_handler(tauri::generate_handler![
            inspect_ipa,
            cancel_inspection,
            discover_devices
        ])
        .run(tauri::generate_context!())
        .expect("Unable to start Orbiter");
}
