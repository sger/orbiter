#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use orbiter_core::installation::{
    self, PreparedInstall, Review,
    job::{self, Control, JobStatus},
};
use std::sync::Mutex;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{Manager, State, ipc::Channel};
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

#[tauri::command]
async fn discover_signing_identities() -> Result<orbiter_core::signing::Inventory, String> {
    orbiter_core::signing::discover().await
}

#[derive(Default, Clone)]
struct Installations {
    gate: Arc<tokio::sync::Mutex<()>>,
    plan: Arc<Mutex<Option<PreparedInstall>>>,
    control: Arc<Mutex<Option<Arc<Control>>>>,
    current: Arc<Mutex<Option<JobStatus>>>,
}
struct EndInstall(Installations);
impl Drop for EndInstall {
    fn drop(&mut self) {
        if let Ok(mut control) = self.0.control.lock() {
            *control = None;
        }
    }
}
fn journal(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|_| "Cannot locate application storage.")?
        .join("last-install.json"))
}
#[tauri::command]
async fn prepare_install(
    path: String,
    device_id: u32,
    state: State<'_, Installations>,
) -> Result<Review, String> {
    let _gate = state
        .gate
        .clone()
        .try_lock_owned()
        .map_err(|_| "Another installation operation is in progress.")?;
    state
        .plan
        .lock()
        .map_err(|_| "Installation state unavailable.")?
        .take();
    let plan = installation::prepare(path.into(), device_id).await?;
    let review = plan.review.clone();
    *state
        .plan
        .lock()
        .map_err(|_| "Installation state unavailable.")? = Some(plan);
    Ok(review)
}
#[tauri::command]
fn discard_install(token: String, state: State<'_, Installations>) -> Result<(), String> {
    let mut plan = state
        .plan
        .lock()
        .map_err(|_| "Installation state unavailable.")?;
    if plan.as_ref().is_some_and(|p| p.review.token == token) {
        plan.take();
    }
    Ok(())
}
#[tauri::command]
async fn execute_install(
    token: String,
    acknowledged: bool,
    progress: Channel<JobStatus>,
    app: tauri::AppHandle,
    state: State<'_, Installations>,
) -> Result<JobStatus, String> {
    if !acknowledged {
        return Err("Review and acknowledge the installation consequences first.".into());
    }
    let _gate = state
        .gate
        .clone()
        .try_lock_owned()
        .map_err(|_| "Another installation operation is in progress.")?;
    let plan = {
        let mut saved = state
            .plan
            .lock()
            .map_err(|_| "Installation state unavailable.")?;
        let plan = saved
            .as_ref()
            .ok_or("Review the IPA and device before installing.")?;
        if plan.review.token != token || plan.expired() {
            return Err("Installation review is stale. Review again.".into());
        }
        if !plan.review.blockers.is_empty() {
            return Err("Resolve the installation blockers before continuing.".into());
        }
        saved.take().ok_or("Installation review is unavailable.")?
    };
    let location = journal(&app)?;
    let owned = state.inner().clone();
    let _end = EndInstall(owned.clone());
    let control = Arc::new(Control::default());
    *owned
        .control
        .lock()
        .map_err(|_| "Installation state unavailable.")? = Some(control.clone());
    *owned
        .current
        .lock()
        .map_err(|_| "Installation state unavailable.")? = Some(JobStatus {
        id: plan.review.token.clone(),
        stage: job::Stage::Preparing,
        message: "Rechecking reviewed IPA and iPhone.".into(),
        transferred_bytes: 0,
        total_bytes: plan.review.size_bytes,
        device_percent: None,
        cleanup_pending: false,
    });
    let current = owned.current.clone();
    let result = tokio::spawn(async move {
        installation::execute(plan, control, location, move |status| {
            if let Ok(mut current) = current.lock() {
                *current = Some(status.clone());
            }
            let _ = progress.send(status);
        })
        .await
    })
    .await;
    match result {
        Ok(status) => Ok(status),
        Err(_) => {
            let recovered = job::recover(&journal(&app)?)?;
            *owned
                .current
                .lock()
                .map_err(|_| "Installation state unavailable.")? = recovered;
            Err("Installation worker stopped. Check the recorded outcome and the phone before retrying.".into())
        }
    }
}
#[tauri::command]
fn cancel_install(state: State<'_, Installations>) -> Result<bool, String> {
    Ok(state
        .control
        .lock()
        .map_err(|_| "Installation state unavailable.")?
        .as_ref()
        .is_some_and(|c| c.cancel()))
}
#[tauri::command]
fn installation_status(
    app: tauri::AppHandle,
    state: State<'_, Installations>,
) -> Result<Option<JobStatus>, String> {
    if state
        .control
        .lock()
        .map_err(|_| "Installation state unavailable.")?
        .is_some()
    {
        return Ok(state
            .current
            .lock()
            .map_err(|_| "Installation state unavailable.")?
            .clone());
    }
    job::recover(&journal(&app)?)
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
        .manage(Installations::default())
        .invoke_handler(tauri::generate_handler![
            inspect_ipa,
            cancel_inspection,
            discover_devices,
            discover_signing_identities,
            prepare_install,
            discard_install,
            execute_install,
            cancel_install,
            installation_status
        ])
        .run(tauri::generate_context!())
        .expect("Unable to start Orbiter");
}
