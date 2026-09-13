#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod failure;
mod library_commands;
use failure::{Failure, internal};
use library_commands::*;
use orbiter_core::{
    application::{
        installation::{Acknowledgement, InstallationService},
        runtime::Runtime,
    },
    domain::identifiers::{ReviewToken, UsbDeviceId},
    installation::{Review, job::JobStatus},
};
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

/// The one installation service for this process.
///
/// # Errors
///
/// Fails if the runtime is missing from managed state, which would mean startup did not complete.
fn installations(app: &tauri::AppHandle) -> Result<InstallationService, Failure> {
    Ok(runtime(app)?.installations())
}

/// Where the legacy renewal record lives.
///
/// Written by earlier versions of Orbiter and never written again; still read so an existing
/// record can be shown as the legacy note it is, and cleared on request.
///
/// # Errors
///
/// Fails if the platform's application data directory cannot be located.
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
/// Review installing an IPA chosen by path.
///
/// The path-based compatibility entry point: it imports the file into the library first, then runs
/// exactly the same artifact-based review as [`library_prepare_install`]. It is deliberately not a
/// second implementation — a path-based install with weaker review or history rules is the kind of
/// shortcut that makes the two diverge.
///
/// # Errors
///
/// Returns the service's structured failure, mapped for IPC: an unreadable IPA, a phone that is
/// absent or unverified, or another operation already running.
#[tauri::command]
async fn prepare_install(
    path: String,
    device_id: u32,
    app: tauri::AppHandle,
) -> Result<Review, Failure> {
    let library = storage(&app)?;
    let artifact_id =
        tokio::task::spawn_blocking(move || library.resolve_or_import(&PathBuf::from(path)))
            .await
            .map_err(|_| internal("Library worker stopped."))?
            .map_err(internal)?;
    Ok(installations(&app)?
        .prepare(&artifact_id, UsbDeviceId::new(device_id))
        .await?)
}

/// Forget a prepared review and release the lease it held.
///
/// Idempotent, and ignores a token that is not the current review, so a late call from a window
/// that has moved on cannot discard a newer one. Nothing on any phone changes.
///
/// # Errors
///
/// Fails only if installation state is unavailable.
#[tauri::command]
fn discard_install(token: String, app: tauri::AppHandle) -> Result<(), Failure> {
    let token = ReviewToken::parse(&token)?;
    Ok(installations(&app)?.discard(&token)?)
}

/// Install the reviewed artifact on the reviewed phone.
///
/// Requires the acknowledgement shown with the review. Streams stage changes over `progress`; a
/// dropped subscription does not stop the installation or lose its history, and
/// [`installation_status`] catches a reconnecting client up.
///
/// # Errors
///
/// Returns the service's structured failure: a missing acknowledgement, a stale review, a changed
/// artifact, another operation running, or an unknown outcome if the worker itself stopped.
#[tauri::command]
async fn execute_install(
    token: String,
    acknowledged: bool,
    progress: Channel<JobStatus>,
    app: tauri::AppHandle,
) -> Result<JobStatus, Failure> {
    let token = ReviewToken::parse(&token)?;
    let sink = Arc::new(move |status: JobStatus| {
        // A closed window is not a reason to stop installing.
        let _ = progress.send(status);
    });
    Ok(installations(&app)?
        .execute(&token, Acknowledgement::from_request(acknowledged), sink)
        .await?)
}

/// Ask the running installation to stop.
///
/// Returns `true` only if a cancellable installation accepted. Once iOS has been asked to install,
/// cancellation is refused: the outcome belongs to the device from that point on.
///
/// # Errors
///
/// Fails only if installation state is unavailable.
#[tauri::command]
fn cancel_install(app: tauri::AppHandle) -> Result<bool, Failure> {
    Ok(installations(&app)?.cancel()?)
}

/// Where the current or most recent installation stands.
///
/// Called when a client reconnects — a reopened window, a page that was hidden — because progress
/// events it missed are gone. Returns the live picture while an installation runs, and otherwise
/// the durable journal, so a result survives the window being closed.
///
/// # Errors
///
/// Fails if installation state is unavailable or the journal cannot be read.
#[tauri::command]
fn installation_status(app: tauri::AppHandle) -> Result<Option<JobStatus>, Failure> {
    Ok(installations(&app)?.status()?)
}

/// One device-log capture at a time, with its own stop flag.
///
/// Separate from the installation gate on purpose: capturing a log is read-only and may run
/// alongside anything else. Only a second capture is refused.
#[derive(Default, Clone)]
struct LogCapture {
    /// Admits one capture at a time, refusing rather than queueing.
    gate: Arc<tokio::sync::Mutex<()>>,
    /// Set by [`stop_device_log`]; the capture checks it between lines.
    cancel: Arc<AtomicBool>,
}

/// Stream the iPhone's log for one app. Only lines about `subjects` are kept, and the capture
/// stops on request, after five minutes, or after its line budget.
///
/// Nothing is written to disk: lines are filtered in memory and forwarded to `progress`. Lines
/// that mention only the superseded identifier are counted and discarded, so the capture describes
/// the signed build rather than the one it replaced.
///
/// # Errors
///
/// Fails if a capture is already running, or if the device's log service cannot be reached.
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
    Ok(library_sign(
        artifact_id.into_inner(),
        watch,
        marker,
        progress,
        app,
        state,
    )
    .await?
    .signed)
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
        .manage(LogCapture::default())
        .setup(|app| {
            // One runtime for the process, built before any command can run. Everything that
            // needs shared state takes a handle to this rather than constructing its own.
            let storage = app
                .path()
                .app_data_dir()
                .map_err(|_| "Cannot locate application storage.")?;
            let runtime = Runtime::new(&storage);
            if let Err(error) = runtime.recover() {
                // A library that cannot be reconciled is reported, not fatal: the app still opens
                // and says what is wrong rather than refusing to start.
                tracing::warn!(operation = "library-recovery", detail = %error);
            }
            app.manage(runtime);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            library_list,
            library_import,
            library_open,
            library_remove,
            library_prepare_install,
            library_expiry,
            library_icon,
            library_reclaim,
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
