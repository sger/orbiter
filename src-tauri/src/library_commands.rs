use super::*;
use orbiter_core::{
    application::{installation::Acknowledgement, runtime::Runtime, signing::Retained},
    domain::identifiers::{AppId, ArtifactId, UsbDeviceId},
    library::{Expiry, Imported, Library, Opened, Snapshot},
    plan::WatchChoice,
};
/// The one library for this process.
///
/// Takes a handle from the application runtime rather than constructing a library: two libraries
/// over one directory would not share a lease table, and a file another operation was reading
/// could then be deleted out from under it.
///
/// # Errors
///
/// Fails only if the runtime is missing from Tauri's managed state, which would mean startup did
/// not complete.
pub fn storage(app: &tauri::AppHandle) -> Result<Library, String> {
    Ok(runtime(app)?.library())
}

/// The application runtime, as managed by Tauri.
///
/// # Errors
///
/// Fails if startup did not register it.
pub fn runtime(app: &tauri::AppHandle) -> Result<Runtime, String> {
    Ok(app.state::<Runtime>().inner().clone())
}
#[tauri::command]
pub async fn library_list(app: tauri::AppHandle) -> Result<Snapshot, String> {
    let library = storage(&app)?;
    tokio::task::spawn_blocking(move || library.snapshot())
        .await
        .map_err(|_| "Library worker stopped.")?
}
#[tauri::command]
pub async fn library_import(path: String, app: tauri::AppHandle) -> Result<Imported, String> {
    let library = storage(&app)?;
    tokio::task::spawn_blocking(move || library.import(&PathBuf::from(path)))
        .await
        .map_err(|_| "Import worker stopped.")?
}
/// Where the seven days stand for one saved build. The raw team identifier is hashed here, at the
/// boundary: the library has never seen one and must not start.
#[tauri::command]
pub async fn library_expiry(
    artifact_id: String,
    team_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<Option<Expiry>, String> {
    let library = storage(&app)?;
    let tag = team_id.as_deref().map(orbiter_core::renewal::tag);
    let artifact_id = ArtifactId::parse(&artifact_id)?;
    tokio::task::spawn_blocking(move || library.expiry(&artifact_id, tag.as_deref()))
        .await
        .map_err(|_| "Library worker stopped.")?
}
/// One app icon's bytes. Fetched per hash and cached in the window, so a library of many apps
/// does not re-send every icon on every refresh.
#[tauri::command]
pub async fn library_icon(sha: String, app: tauri::AppHandle) -> Result<Option<String>, String> {
    let library = storage(&app)?;
    tokio::task::spawn_blocking(move || library.icon(&sha))
        .await
        .map_err(|_| "Library worker stopped.")?
}
/// Delete managed files no record points at. Always on request; never on a timer or at startup.
#[tauri::command]
pub async fn library_reclaim(app: tauri::AppHandle) -> Result<u64, String> {
    // Held for the whole sweep: a file an installation is reading must not be reclaimed.
    let _gate = runtime(&app)?.installations().exclude()?;
    let library = storage(&app)?;
    tokio::task::spawn_blocking(move || library.reclaim())
        .await
        .map_err(|_| "Library worker stopped.")?
}
#[tauri::command]
pub async fn library_open(artifact_id: String, app: tauri::AppHandle) -> Result<Opened, String> {
    let library = storage(&app)?;
    let artifact_id = ArtifactId::parse(&artifact_id)?;
    tokio::task::spawn_blocking(move || library.open(&artifact_id))
        .await
        .map_err(|_| "Library worker stopped.")?
}
#[tauri::command]
pub async fn library_remove(
    app_id: String,
    artifact_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let installations = runtime(&app)?.installations();
    // Held for the whole removal: a file must not disappear while a review points at it, and an
    // installation must not begin against something that is being deleted.
    let _gate = installations.exclude()?;
    let app_id = AppId::parse(&app_id)?;
    let artifact_id = artifact_id.as_deref().map(ArtifactId::parse).transpose()?;
    // Explicit removal invalidates a review that pointed at what is about to go. A running
    // installation cannot be affected: it holds the gate taken above.
    installations.invalidate(&app_id, artifact_id.as_ref())?;
    let library = storage(&app)?;
    tokio::task::spawn_blocking(move || library.remove(&app_id, artifact_id.as_ref()))
        .await
        .map_err(|_| "Library worker stopped.")?
}
/// Review installing one saved artifact on one connected phone.
///
/// The artifact-based entry point every installation goes through, including the path-based
/// compatibility command, which imports first and then calls this. Binds the returned token to the
/// exact bytes and the exact phone, and holds a lease so the file cannot be removed underneath it.
///
/// # Errors
///
/// Returns the service's structured failure: the artifact missing or changed, the phone absent or
/// unverified, or another operation already running.
#[tauri::command]
pub async fn library_prepare_install(
    artifact_id: String,
    device_id: u32,
    app: tauri::AppHandle,
) -> Result<Review, String> {
    let artifact_id = ArtifactId::parse(&artifact_id)?;
    runtime(&app)?
        .installations()
        .prepare(&artifact_id, UsbDeviceId::new(device_id))
        .await
        .map_err(Into::into)
}

/// Reserve app identifiers and download profiles for one saved original.
///
/// **Contacts Apple and mutates remote state**, so it refuses without the acknowledgement shown
/// beside the control. A free personal team may register only ten identifiers per seven days and
/// an identifier can never be reused by another team.
///
/// # Errors
///
/// Returns the service's structured failure: a missing acknowledgement, an artifact that is
/// missing, changed, or already a signed build, or Apple's own refusal.
#[tauri::command]
pub async fn library_prepare_provisioning(
    artifact_id: String,
    acknowledged: bool,
    watch: String,
    app: tauri::AppHandle,
) -> Result<orbiter_core::accounts::Preparation, String> {
    let artifact_id = ArtifactId::parse(&artifact_id)?;
    runtime(&app)?
        .signing()
        .prepare(
            &artifact_id,
            Acknowledgement::from_request(acknowledged),
            WatchChoice::parse(&watch),
        )
        .await
        .map_err(Into::into)
}

/// Re-sign one saved original and keep the result beside it.
///
/// Local only: no Apple request is made here — it uses the certificate and profiles already
/// obtained. Streams counted progress over `progress`; a closed window does not stop the run.
///
/// The returned build's path points at the library's managed copy, not at the staging file, which
/// is removed only once the library has durably retained it.
///
/// # Errors
///
/// Returns the service's structured failure: an artifact missing, changed, or already signed; no
/// signed-in session, selected team or certificate; or a library that could not keep the output —
/// in which case the generated build is deliberately left on disk.
#[tauri::command]
pub async fn library_sign(
    artifact_id: String,
    watch: String,
    marker: String,
    progress: Channel<orbiter_core::signer::Progress>,
    app: tauri::AppHandle,
) -> Result<Retained, String> {
    let artifact_id = ArtifactId::parse(&artifact_id)?;
    let sink = Arc::new(move |step| {
        // A dropped channel means the window went away; the run finishes regardless.
        let _ = progress.send(step);
    });
    runtime(&app)?
        .signing()
        .sign(&artifact_id, WatchChoice::parse(&watch), &marker, sink)
        .await
        .map_err(Into::into)
}
