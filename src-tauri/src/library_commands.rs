use super::*;
use orbiter_core::{
    application::runtime::Runtime,
    domain::identifiers::{AppId, ArtifactId, ArtifactId as SourceId, UsbDeviceId},
    library::{Artifact, Expiry, Imported, Library, Opened, Snapshot},
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

#[tauri::command]
pub async fn library_prepare_provisioning(
    artifact_id: String,
    acknowledged: bool,
    watch: String,
    app: tauri::AppHandle,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<orbiter_core::accounts::Preparation, String> {
    let library = storage(&app)?;
    let artifact_id = ArtifactId::parse(&artifact_id)?;
    let (artifact, path, _lease) = tokio::task::spawn_blocking(move || library.pin(&artifact_id))
        .await
        .map_err(|_| "Library worker stopped.")??;
    if artifact.source_id.is_some() {
        return Err("Select an original version to sign.".into());
    }
    state
        .prepare_provisioning(
            path,
            acknowledged,
            orbiter_core::plan::WatchChoice::parse(&watch),
        )
        .await
}
#[derive(serde::Serialize)]
pub struct SavedSigned {
    pub signed: orbiter_core::signer::Signed,
    artifact: Artifact,
}
#[tauri::command]
pub async fn library_sign(
    artifact_id: String,
    watch: String,
    marker: String,
    progress: Channel<orbiter_core::signer::Progress>,
    app: tauri::AppHandle,
    state: State<'_, orbiter_core::accounts::Accounts>,
) -> Result<SavedSigned, String> {
    let library = storage(&app)?;
    let worker_library = library.clone();
    let artifact_id = SourceId::parse(&artifact_id)?;
    let source_id = artifact_id.clone();
    let (source, path, _lease) =
        tokio::task::spawn_blocking(move || worker_library.pin(&source_id))
            .await
            .map_err(|_| "Library worker stopped.")??;
    if source.source_id.is_some() {
        return Err("Select an original version to sign.".into());
    }
    let out_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| "Cannot locate application storage.")?
        .join("signed");
    let cleaned_marker = orbiter_core::signer::marker(&marker);
    tracing::info!(operation = "signing", stage = "started");
    let signed = state
        .sign_ipa(
            path,
            out_dir,
            orbiter_core::plan::WatchChoice::parse(&watch),
            cleaned_marker.clone(),
            Arc::new(AtomicBool::new(false)),
            move |step| {
                let _ = progress.send(step);
            },
        )
        .await
        .inspect_err(
            |error| tracing::info!(operation = "signing", stage = "failed", detail = %error),
        )?;
    // The same record the interface shows, so a terminal and a screenshot agree. These lines are
    // how a signing run is diagnosed after the fact; without them a failure is only ever "it did
    // not work" by the time anyone asks.
    for line in &signed.log {
        tracing::info!(operation = "signing", detail = %line);
    }
    tracing::info!(
        operation = "signing",
        stage = "finished",
        bundles = signed.bundles_signed,
        removed = signed.removed.len()
    );
    tokio::task::spawn_blocking(move || {
        let artifact = library.retain_signed(
            &artifact_id,
            &signed,
            signed
                .team_tag
                .clone()
                .ok_or("Signing team metadata unavailable.")?,
            orbiter_core::plan::WatchChoice::parse(&watch).label().into(),
            cleaned_marker.unwrap_or_default(),
        ).map_err(|e| format!("Signing completed, but saving it to the library failed: {e} The generated output has been retained."))?;
        // The library is durable before removing this operation's staging output. `pin` is what
        // is wanted here and `open` is not: both verify the managed copy's hash, but `open` also
        // re-inspects the whole archive, and re-reading a 200 MB IPA that was just written to
        // learn a path it already knows is a minute of nothing.
        let (_, path, _lease) = library.pin(&artifact.id)?;
        let _ = std::fs::remove_file(&signed.path);
        let mut signed = signed;
        signed.path = path.to_string_lossy().into_owned();
        tracing::info!(operation = "signing", stage = "retained", artifact = %artifact.id);
        Ok(SavedSigned { signed, artifact })
    })
    .await
    .map_err(|_| "Signed artifact storage worker stopped.")?
}
