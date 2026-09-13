#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod library_commands;
use library_commands::*;
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
fn renewal_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|_| "Cannot locate application storage.")?
        .join("renewal.json"))
}
/// Where the seven days stand for the team and build on screen, if anything is known.
#[tauri::command]
fn renewal_status(
    team_id: Option<String>,
    identifier: Option<String>,
    app: tauri::AppHandle,
) -> Result<Option<orbiter_core::renewal::Status>, String> {
    Ok(orbiter_core::renewal::status(
        &renewal_file(&app)?,
        team_id.as_deref(),
        identifier.as_deref(),
        std::time::SystemTime::now(),
    ))
}
/// Forget every remembered build. Nothing on any phone changes.
#[tauri::command]
fn renewal_forget(app: tauri::AppHandle) -> Result<(), String> {
    orbiter_core::renewal::forget(&renewal_file(&app)?)
}
#[tauri::command]
async fn prepare_install(
    path: String,
    device_id: u32,
    app: tauri::AppHandle,
    state: State<'_, Installations>,
) -> Result<Review, String> {
    let library = storage(&app)?;
    let artifact_id =
        tokio::task::spawn_blocking(move || library.resolve_or_import(&PathBuf::from(path)))
            .await
            .map_err(|_| "Library worker stopped.")??;
    library_prepare_install(artifact_id, device_id, app, state).await
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
    accounts: State<'_, orbiter_core::accounts::Accounts>,
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
    // Captured before the plan moves into the worker: what was actually put on the phone, for
    // the renewal record written only if the install reports success.
    let library = storage(&app)?;
    let library_backed = plan.library_artifact.is_some();
    plan.record_library_attempt(&library)?;
    let identifier = plan.review.bundle_id.clone();
    let app_name = plan.review.app_name.clone();
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
    let history = library.clone();
    let result = tokio::spawn(async move {
        installation::execute(plan, control, location, move |mut status| {
            if let Err(error) = history.update(&status) {
                status.message.push_str(&format!(
                    " Installation history could not be saved: {error}"
                ));
            }
            if let Ok(mut current) = current.lock() {
                *current = Some(status.clone());
            }
            let _ = progress.send(status);
        })
        .await
    })
    .await;
    match result {
        Ok(mut status) => {
            if let Err(error) = library.update(&status) {
                status.message.push_str(&format!(
                    " Installation history could not be saved: {error}"
                ));
            }
            // The moment the seven days start mattering: the build is on a phone. A failed or
            // cancelled install leaves the waiting record untouched, so a later attempt still has
            // it, and a build that never installed is never counted down.
            if !library_backed
                && status.stage == job::Stage::Installed
                && let Some(mut record) = accounts.take_pending_renewal(&identifier)
            {
                record.app_name = app_name.clone();
                record.installed_unix =
                    orbiter_core::renewal::now_unix(std::time::SystemTime::now());
                if let Err(error) = renewal_file(&app)
                    .and_then(|path| orbiter_core::renewal::remember(&path, record))
                {
                    // Never fail a completed install over a note about when it expires.
                    tracing::warn!(operation = "renewal", stage = "not-recorded", detail = %error);
                }
            }
            Ok(status)
        }
        Err(_) => {
            let recovered = job::recover(&journal(&app)?)?;
            library.recover(recovered.as_ref())?;
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
#[derive(Default, Clone)]
struct LogCapture {
    gate: Arc<tokio::sync::Mutex<()>>,
    cancel: Arc<AtomicBool>,
}
/// Stream the iPhone's log for one app. Only lines about `subjects` are kept, and the capture
/// stops on request, after five minutes, or after its line budget.
#[tauri::command]
async fn start_device_log(
    device_id: u32,
    subjects: Vec<String>,
    superseded: Vec<String>,
    progress: Channel<orbiter_core::diagnostics::LogLine>,
    state: State<'_, LogCapture>,
) -> Result<orbiter_core::diagnostics::Summary, String> {
    let _gate = state
        .gate
        .clone()
        .try_lock_owned()
        .map_err(|_| "A log capture is already running.")?;
    state.cancel.store(false, Ordering::SeqCst);
    tracing::info!(operation = "device-log", stage = "started");
    let result = orbiter_core::diagnostics::capture(
        device_id,
        subjects,
        superseded,
        state.cancel.clone(),
        move |line| {
            let _ = progress.send(line);
        },
    )
    .await;
    // Line contents are the device's, not Orbiter's to record: only the counts are logged.
    tracing::info!(
        operation = "device-log",
        stage = "finished",
        success = result.is_ok(),
        matched = result.as_ref().map(|s| s.matched).unwrap_or(0)
    );
    result
}
#[tauri::command]
fn stop_device_log(state: State<'_, LogCapture>) {
    state.cancel.store(true, Ordering::SeqCst);
}
#[tauri::command]
fn account_status(
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::View, String> {
    state.status()
}
#[tauri::command]
async fn account_sign_in(
    email: String,
    password: String,
    consent: bool,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::View, String> {
    state.start(email, password, consent)
}
#[tauri::command]
fn account_answer(
    challenge_id: String,
    answer: orbiter_core::accounts::Answer,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::View, String> {
    state.answer(challenge_id, answer)
}
#[tauri::command]
fn account_sign_out(
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::View, String> {
    state.sign_out()
}
#[tauri::command]
fn account_select_team(
    id: String,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::View, String> {
    state.select_team(id)
}
#[tauri::command]
async fn account_register_device(
    device_id: u32,
    acknowledged: bool,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::provisioning::Outcome, String> {
    state.register_device(device_id, acknowledged).await
}
#[tauri::command]
async fn account_request_certificate(
    acknowledged: bool,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::certificates::Outcome, String> {
    state.request_certificate(acknowledged).await
}
#[tauri::command]
async fn account_prepare_provisioning(
    path: String,
    acknowledged: bool,
    watch: String,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::Preparation, String> {
    state
        .prepare_provisioning(
            std::path::PathBuf::from(path),
            acknowledged,
            orbiter_core::plan::WatchChoice::parse(&watch),
        )
        .await
}
/// Withdraw the selected team's development certificates. The interface offers this only when the
/// team's slots are full and none of them can sign on this Mac.
#[tauri::command]
async fn account_withdraw_certificates(
    acknowledged: bool,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<String, String> {
    tracing::info!(operation = "certificate-withdrawal", stage = "started");
    let result = state.withdraw_certificates(acknowledged).await;
    tracing::info!(
        operation = "certificate-withdrawal",
        stage = "finished",
        success = result.is_ok()
    );
    result
}
/// Sign the selected IPA for the signed-in account's team. The signed build is written into the
/// application's own storage; the IPA the person chose is only ever read.
#[tauri::command]
async fn account_sign_ipa(
    path: String,
    watch: String,
    marker: String,
    progress: tauri::ipc::Channel<orbiter_core::signer::Progress>,
    app: tauri::AppHandle,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::signer::Signed, String> {
    let library = storage(&app)?;
    let artifact_id =
        tokio::task::spawn_blocking(move || library.resolve_or_import(&PathBuf::from(path)))
            .await
            .map_err(|_| "Library worker stopped.")??;
    Ok(
        library_sign(artifact_id, watch, marker, progress, app, state)
            .await?
            .signed,
    )
}
#[tauri::command]
async fn account_forget_signing_key(
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<String, String> {
    state.forget_signing_key().await
}
#[tauri::command]
async fn account_refresh_teams(
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::View, String> {
    state.refresh_teams().await
}
fn main() {
    use tracing_subscriber::prelude::*;
    // Never format upstream authentication reports: they may contain credentials.
    orbiter_core::accounts::initialize();
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
        .manage(orbiter_core::accounts::Accounts::default())
        .manage(Inspection::default())
        .manage(Installations::default())
        .manage(LogCapture::default())
        .setup(|app| {
            let result = storage(app.handle()).and_then(|library| {
                let recovered = job::recover(&journal(app.handle())?).unwrap_or_else(|_| {
                    tracing::warn!(operation = "library-recovery", "Unreadable installation journal; interrupted library attempts will be marked unknown.");
                    None
                });
                library.recover(recovered.as_ref())
            });
            if let Err(error) = result {
                tracing::warn!(operation = "library-recovery", detail = %error);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            library_list,
            library_import,
            library_open,
            library_remove,
            library_prepare_install,
            library_prepare_provisioning,
            library_sign,
            account_status,
            account_sign_in,
            account_answer,
            account_sign_out,
            account_select_team,
            account_refresh_teams,
            account_register_device,
            account_request_certificate,
            account_prepare_provisioning,
            account_sign_ipa,
            renewal_status,
            renewal_forget,
            account_withdraw_certificates,
            account_forget_signing_key,
            inspect_ipa,
            cancel_inspection,
            discover_devices,
            prepare_install,
            discard_install,
            execute_install,
            cancel_install,
            installation_status,
            start_device_log,
            stop_device_log
        ])
        .run(tauri::generate_context!())
        .expect("Unable to start Orbiter");
}
