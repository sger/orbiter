//! The desktop shell: Tauri startup, dependency construction, and the IPC surface.
//!
//! Every command here is an adapter. It decodes its inputs, validates identifiers at this
//! boundary, calls an application service, and maps the result — including turning a structured
//! backend failure into the `{ code, message }` the window switches on. No workflow lives in this
//! file; that is the point of [`orbiter_core::application`].
//!
//! # Acknowledgements
//!
//! Commands that mutate something outside this Mac — an Apple account, an iPhone — take their own
//! acknowledgement and refuse without it. Nothing here spends one of a free team's small, mostly
//! irreversible allowances on Orbiter's initiative.
//!
//! # Startup
//!
//! One [`Runtime`] is built from the platform's application data directory, reconciled against
//! whatever the last run left behind, and registered. Everything else takes a handle to it.

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
    /// Let the next inspection start, whether this one finished, failed or panicked.
    ///
    /// Clearing the flag by `Drop` rather than at the end of the command is what makes a panicking
    /// inspection leave the app usable instead of permanently busy.
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
/// Inspect one local IPA and report what is in it.
///
/// Purely local: the chosen file is read and never written, no Apple service is contacted, and no
/// credential is touched. Progress names completed boundaries rather than a fabricated percentage.
/// One inspection at a time; a second is refused rather than queued.
///
/// # Errors
///
/// Fails if another inspection is running, if the archive is malformed or exceeds an inspection
/// limit, or if [`cancel_inspection`] was called.
#[tauri::command]
async fn inspect_ipa(
    path: String,
    progress: Channel<&'static str>,
    state: State<'_, Inspection>,
) -> Result<orbiter_core::Report, Failure> {
    if state
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(Failure::from("An inspection is already running."));
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
        // Inspection's own error already classifies itself: cancelled, a limit, or a malformed
        // archive each reach the window as a different code.
        result.map_err(orbiter_core::domain::errors::OperationError::from)
    })
    .await
    .map_err(|_| internal("Inspection worker stopped. Retry with a fresh IPA."))?
    .map_err(Into::into)
}
/// Ask the running inspection to stop at its next boundary.
///
/// Takes effect between archive entries, so it is prompt without leaving a half-read report.
/// Calling it when nothing is running is harmless.
#[tauri::command]
fn cancel_inspection(state: State<'_, Inspection>) {
    state.cancel.store(true, Ordering::SeqCst);
}
/// List the iPhones this Mac can see right now.
///
/// Read-only: it reports what the local device service finds and which pairings already exist. It
/// pairs nothing and contacts no Apple service. Never fails — an unreachable device service is
/// part of the report, because "no iPhone is plugged in" and "this Mac cannot talk to iPhones" are
/// different answers and a person needs to tell them apart.
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
fn renewal_file(app: &tauri::AppHandle) -> Result<PathBuf, Failure> {
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
) -> Result<Option<orbiter_core::renewal::Status>, Failure> {
    Ok(orbiter_core::renewal::status(
        &renewal_file(&app)?,
        team_id.as_deref(),
        identifier.as_deref(),
        std::time::SystemTime::now(),
    ))
}
/// Forget every remembered build. Nothing on any phone changes.
#[tauri::command]
fn renewal_forget(app: tauri::AppHandle) -> Result<(), Failure> {
    Ok(orbiter_core::renewal::forget(&renewal_file(&app)?)?)
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
) -> Result<orbiter_core::diagnostics::Summary, Failure> {
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
    Ok(result?)
}
/// Stop the running device-log capture at its next line.
///
/// Harmless when no capture is running. Nothing was written to disk, so there is nothing to clean
/// up.
#[tauri::command]
fn stop_device_log(state: State<'_, LogCapture>) {
    state.cancel.store(true, Ordering::SeqCst);
}
/// What the Apple account session currently is: signed out, awaiting a challenge, or signed in.
///
/// Local only — it reports state this process already holds and contacts nothing. Reading it also
/// expires a session that has passed its lifetime, so a stale view is never returned.
#[tauri::command]
fn account_status(
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::View, Failure> {
    Ok(state.status()?)
}
/// Sign in to Apple with an email, a password, and explicit consent.
///
/// **Contacts Apple.** The password exists only for the duration of this request and is zeroized
/// afterwards; it is never stored, logged, or returned. Consent is required because the request
/// goes to Apple directly using this Mac's own authentication support.
///
/// Returns the next state: signed in, or a two-factor challenge to answer.
///
/// # Errors
///
/// Fails on rejected credentials, on Apple throttling this Mac (in which case a local hold is
/// applied before retrying), or when local authentication support is unavailable. Apple's own
/// responses are redacted before they appear in any message.
#[tauri::command]
async fn account_sign_in(
    email: String,
    password: String,
    consent: bool,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::View, Failure> {
    Ok(state.start(email, password, consent)?)
}
/// Answer a two-factor challenge, or ask for the code to be sent another way.
///
/// A verification code is treated exactly as a password: used once, zeroized, never stored.
/// Answering a challenge that is no longer the current one is refused rather than applied to
/// whatever challenge replaced it. The Apple request itself is made by the waiting sign-in.
///
/// # Errors
///
/// Fails on an unknown challenge, or if the session was cleared while the challenge was open.
#[tauri::command]
fn account_answer(
    challenge_id: String,
    answer: orbiter_core::accounts::Answer,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::View, Failure> {
    Ok(state.answer(challenge_id, answer)?)
}
/// Sign out, clearing the session from memory.
///
/// Local only. Nothing is revoked at Apple: a certificate this session obtained still exists, and
/// a signed build already produced still works. Signing out only forgets the session.
#[tauri::command]
fn account_sign_out(
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::View, Failure> {
    Ok(state.sign_out()?)
}
/// Choose which of the signed-in account's teams to work with.
///
/// Local only. Changing the team invalidates any prepared provisioning, because identifiers are
/// derived from the team and a plan made for one says nothing about another.
///
/// # Errors
///
/// Fails if no session exists or the identifier names no team the account belongs to.
#[tauri::command]
fn account_select_team(
    id: String,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::View, Failure> {
    Ok(state.select_team(id)?)
}
/// Register a connected iPhone on the selected team.
///
/// **Contacts Apple and mutates remote state**, so it refuses without an acknowledgement: a free
/// personal team may register only three devices and a registration cannot be undone from here.
/// Returns no device identifier — the phone's UDID is used to make the request and never returned
/// or stored.
///
/// # Errors
///
/// Fails without an acknowledgement, a session, a selected team or a verified phone, and when
/// Apple refuses — including when the team's device allowance is already full.
#[tauri::command]
async fn account_register_device(
    device_id: u32,
    acknowledged: bool,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::provisioning::Outcome, Failure> {
    Ok(state.register_device(device_id, acknowledged).await?)
}
/// Obtain a development certificate for the selected team, reusing one where possible.
///
/// **Contacts Apple and may mutate remote state**, so it refuses without an acknowledgement: a
/// free personal team has very few certificate slots and issuing one can leave the team unable to
/// issue another. The private key is generated on this Mac and stays in its Keychain; only a
/// certificate signing request is sent.
///
/// # Errors
///
/// Fails without an acknowledgement or a session, and when Apple refuses — notably when the team
/// already holds an active certificate, which [`account_withdraw_certificates`] exists to resolve.
#[tauri::command]
async fn account_request_certificate(
    acknowledged: bool,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::certificates::Outcome, Failure> {
    Ok(state.request_certificate(acknowledged).await?)
}
/// Reserve app identifiers and download profiles for an IPA chosen by path.
///
/// The path-based compatibility entry point: it imports the file into the library first, then runs
/// the same artifact-based preparation as [`library_prepare_provisioning`], so the two cannot
/// diverge.
///
/// **Contacts Apple and mutates remote state**; see the artifact-based command for the
/// acknowledgement and allowances involved.
///
/// # Errors
///
/// Returns the same failures as [`library_prepare_provisioning`], plus any failure to read or
/// import the chosen file.
#[tauri::command]
async fn account_prepare_provisioning(
    path: String,
    acknowledged: bool,
    watch: String,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::Preparation, Failure> {
    state
        .prepare_provisioning(
            std::path::PathBuf::from(path),
            acknowledged,
            orbiter_core::plan::WatchChoice::parse(&watch),
        )
        .await
        .map_err(Into::into)
}
/// Withdraw the selected team's development certificates. The interface offers this only when the
/// team's slots are full and none of them can sign on this Mac.
#[tauri::command]
async fn account_withdraw_certificates(
    acknowledged: bool,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<String, Failure> {
    tracing::info!(operation = "certificate-withdrawal", stage = "started");
    let result = state.withdraw_certificates(acknowledged).await;
    tracing::info!(
        operation = "certificate-withdrawal",
        stage = "finished",
        success = result.is_ok()
    );
    Ok(result?)
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
) -> Result<orbiter_core::signer::Signed, Failure> {
    let library = storage(&app)?;
    let artifact_id =
        tokio::task::spawn_blocking(move || library.resolve_or_import(&PathBuf::from(path)))
            .await
            .map_err(|_| "Library worker stopped.")??;
    Ok(
        library_sign(artifact_id.into_inner(), watch, marker, progress, app)
            .await?
            .signed,
    )
}
/// Remove this Mac's stored signing key from the Keychain.
///
/// Local only. The certificate Apple issued for that key still exists and still counts against the
/// team's allowance; this forgets the private half, which is why the message says so rather than
/// implying the slot was freed.
///
/// # Errors
///
/// Fails if no key is stored for the current account and team, or if the Keychain refuses.
#[tauri::command]
async fn account_forget_signing_key(
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<String, Failure> {
    Ok(state.forget_signing_key().await?)
}
/// Ask Apple for the account's teams again.
///
/// **Contacts Apple** but mutates nothing. A session that can no longer be refreshed is cleared
/// along with the team selection, so the interface shows a signed-out state rather than acting on
/// a session that has quietly stopped working.
///
/// # Errors
///
/// Fails if a refresh is already running, or if the session cannot be refreshed — in which case
/// the returned view already reflects being signed out.
#[tauri::command]
async fn account_refresh_teams(
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::View, Failure> {
    Ok(state.refresh_teams().await?)
}
/// Build the runtime, register the IPC surface, and run the desktop application.
///
/// Startup order matters: the runtime is constructed and reconciled against whatever the last run
/// left behind *before* any command can be invoked, so no installation can begin against state
/// that has not been recovered. A library that cannot be reconciled is reported and the
/// application still opens — refusing to start would leave a person with no way to see why.
///
/// # Panics
///
/// Panics if Tauri itself cannot start, which means the webview or the application context is
/// unavailable and there is nothing to degrade to.
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
            // The account session is the runtime's, registered so `State<Accounts>` resolves to
            // the same instance the signing service holds. Two sessions would mean signing never
            // saw the one a person had signed in to.
            app.manage(runtime.accounts());
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
