//! IPC for the saved-IPA library.
//!
//! Adapters, like every command in this crate: decode, validate identifiers at the boundary, call
//! an application service or the library, map the result. Commands that touch managed files take
//! the installation gate first, so nothing can be deleted or reclaimed while a review points at it
//! or an installation is reading it.
//!
//! Nothing here uninstalls anything from a phone. Removing a saved file removes a copy on this
//! Mac; the app on a tester's device is unaffected.

use super::*;
use crate::failure::Failure;
use orbiter_core::{
    application::{installation::Acknowledgement, runtime::Runtime, signing::Retained},
    domain::identifiers::{AppId, ArtifactId, RememberedDeviceId, UsbDeviceId},
    library::{Expiry, Imported, Library, Opened, Refresh, Snapshot},
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
pub fn storage(app: &tauri::AppHandle) -> Result<Library, Failure> {
    Ok(runtime(app)?.library())
}

/// The application runtime, as managed by Tauri.
///
/// # Errors
///
/// Fails if startup did not register it.
pub fn runtime(app: &tauri::AppHandle) -> Result<Runtime, Failure> {
    Ok(app.state::<Runtime>().inner().clone())
}
#[tauri::command]
/// Everything the library holds: apps, saved versions, remembered devices, history, expiry and
/// storage use.
///
/// Read-only and local. Returns the whole picture in one payload because the window renders it as
/// one screen; icons are deliberately *not* included, and are fetched per hash by
/// [`library_icon`] so a refresh does not re-send them.
///
/// # Errors
///
/// Fails if the manifest is unreadable, damaged, or written by a newer Orbiter. A damaged library
/// is reported rather than replaced with an empty one: silently starting over would look like
/// every saved build had vanished.
pub async fn library_list(app: tauri::AppHandle) -> Result<Snapshot, Failure> {
    let library = storage(&app)?;
    tokio::task::spawn_blocking(move || library.snapshot())
        .await
        .map_err(|_| "Library worker stopped.")?
        .map_err(Into::into)
}
#[tauri::command]
/// Copy a local IPA into the library and record it as a saved version.
///
/// The chosen file is only ever read. Bytes decide identity: importing the same file twice returns
/// the existing version and repairs a damaged managed copy, while different bytes are a new
/// version even when the version and build labels match.
///
/// Copying, hashing and inspecting happen on a blocking thread and outside the metadata lock, so a
/// multi-gigabyte import does not freeze the rest of the library.
///
/// # Errors
///
/// Fails if the file is missing, unreadable, larger than 2 GiB, not a valid IPA, or if the library
/// cannot be written.
pub async fn library_import(path: String, app: tauri::AppHandle) -> Result<Imported, Failure> {
    let library = storage(&app)?;
    tokio::task::spawn_blocking(move || library.import(&PathBuf::from(path)))
        .await
        .map_err(|_| "Import worker stopped.")?
        .map_err(Into::into)
}
/// Where the seven days stand for one saved build. The raw team identifier is hashed here, at the
/// boundary: the library has never seen one and must not start.
#[tauri::command]
pub async fn library_expiry(
    artifact_id: String,
    team_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<Option<Expiry>, Failure> {
    let library = storage(&app)?;
    let tag = team_id.as_deref().map(orbiter_core::renewal::tag);
    let artifact_id = ArtifactId::parse(&artifact_id)?;
    tokio::task::spawn_blocking(move || library.expiry(&artifact_id, tag.as_deref()))
        .await
        .map_err(|_| "Library worker stopped.")?
        .map_err(Into::into)
}
/// What a re-sign of an expiring build would start from: the original it was made from, the watch
/// decision and marker it was signed under, and the phone it went to.
///
/// Read-only and local. Answering does not begin anything — the window uses it to open the review
/// screen with the previous answers filled in, and that screen still asks for every acknowledgement
/// it asked for the first time. `None` means the original is gone, so there is nothing to offer.
#[tauri::command]
pub async fn library_refresh(
    artifact_id: String,
    app: tauri::AppHandle,
) -> Result<Option<Refresh>, Failure> {
    let library = storage(&app)?;
    let artifact_id = ArtifactId::parse(&artifact_id)?;
    tokio::task::spawn_blocking(move || library.refresh(&artifact_id))
        .await
        .map_err(|_| "Library worker stopped.")?
        .map_err(Into::into)
}
/// The library's tag for the phone currently on the cable, so the window can say whether it is the
/// one a build was installed to.
///
/// The UDID is read inside the core crate and never crosses this boundary; what comes back is a
/// salted hash that means nothing outside this library. Reports an error rather than a guess when
/// the phone cannot be identified: "cannot tell" and "a different phone" are not the same answer.
#[tauri::command]
pub async fn library_device_tag(
    device_id: u32,
    app: tauri::AppHandle,
) -> Result<RememberedDeviceId, Failure> {
    runtime(&app)?
        .remembered_device(UsbDeviceId::new(device_id))
        .await
        .map_err(Into::into)
}
/// One app icon's bytes. Fetched per hash and cached in the window, so a library of many apps
/// does not re-send every icon on every refresh.
#[tauri::command]
pub async fn library_icon(sha: String, app: tauri::AppHandle) -> Result<Option<String>, Failure> {
    let library = storage(&app)?;
    tokio::task::spawn_blocking(move || library.icon(&sha))
        .await
        .map_err(|_| "Library worker stopped.")?
        .map_err(Into::into)
}
/// Delete managed files no record points at. Always on request; never on a timer or at startup.
#[tauri::command]
pub async fn library_reclaim(app: tauri::AppHandle) -> Result<u64, Failure> {
    // Held for the whole sweep: a file an installation is reading must not be reclaimed.
    let _gate = runtime(&app)?.installations().exclude()?;
    let library = storage(&app)?;
    tokio::task::spawn_blocking(move || library.reclaim())
        .await
        .map_err(|_| "Library worker stopped.")?
        .map_err(Into::into)
}
#[tauri::command]
/// Verify a saved version and return its metadata, inspection report and managed path.
///
/// Re-hashes the managed copy before reporting anything, so a file that changed on disk is refused
/// rather than described. Opening an original also refreshes the app's cached icon.
///
/// # Errors
///
/// Fails if the artifact is unknown, has been removed, or no longer matches its recorded hash.
pub async fn library_open(artifact_id: String, app: tauri::AppHandle) -> Result<Opened, Failure> {
    let library = storage(&app)?;
    let artifact_id = ArtifactId::parse(&artifact_id)?;
    tokio::task::spawn_blocking(move || library.open(&artifact_id))
        .await
        .map_err(|_| "Library worker stopped.")?
        .map_err(Into::into)
}
#[tauri::command]
/// Remove one saved version, or an entire app with its history.
///
/// Removing a version removes that original and the signed builds made from it while keeping their
/// installation history as tombstones; removing an app removes its files *and* its history, which
/// is why the interface asks first. **Nothing is uninstalled from any phone.**
///
/// Holds the installation gate for the whole removal and invalidates a review that pointed at what
/// is going, so a file cannot disappear from under an operation.
///
/// # Errors
///
/// Fails if an installation is running, if the app or version is not in this library, or if a file
/// is currently leased by another operation.
pub async fn library_remove(
    app_id: String,
    artifact_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<(), Failure> {
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
        .map_err(Into::into)
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
) -> Result<Review, Failure> {
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
    dylibs: Vec<String>,
    app: tauri::AppHandle,
) -> Result<orbiter_core::accounts::Preparation, Failure> {
    let artifact_id = ArtifactId::parse(&artifact_id)?;
    runtime(&app)?
        .signing()
        .prepare(
            &artifact_id,
            Acknowledgement::from_request(acknowledged),
            WatchChoice::parse(&watch),
            dylibs.into_iter().map(std::path::PathBuf::from).collect(),
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
    dylibs: Vec<String>,
    progress: Channel<orbiter_core::signer::Progress>,
    app: tauri::AppHandle,
) -> Result<Retained, Failure> {
    let artifact_id = ArtifactId::parse(&artifact_id)?;
    let sink = Arc::new(move |step| {
        // A dropped channel means the window went away; the run finishes regardless.
        let _ = progress.send(step);
    });
    runtime(&app)?
        .signing()
        .sign(
            &artifact_id,
            WatchChoice::parse(&watch),
            &marker,
            dylibs.into_iter().map(std::path::PathBuf::from).collect(),
            sink,
        )
        .await
        .map_err(Into::into)
}

/// Read-only preparation assessment. The token binds all displayed actions to these inputs.
/// Returns structured failures without registering devices, creating certificates or identifiers.
#[tauri::command]
pub async fn library_review_preparation(
    artifact_id: String,
    device_id: u32,
    watch: String,
    marker: String,
    dylibs: Vec<String>,
    app: tauri::AppHandle,
) -> Result<orbiter_core::application::guided::PreparationReview, Failure> {
    runtime(&app)?
        .signing()
        .review_preparation(
            &ArtifactId::parse(&artifact_id)?,
            device_id,
            WatchChoice::parse(&watch),
            &marker,
            dylibs.into_iter().map(std::path::PathBuf::from).collect(),
        )
        .await
        .map_err(Into::into)
}

/// Execute an explicitly acknowledged preparation, independent of the subscribing page.
/// The spawned task owns the work; a lost IPC response never triggers an automatic retry.
#[tauri::command]
pub async fn library_execute_preparation(
    token: String,
    consents: orbiter_core::application::guided::Consents,
    progress: Channel<orbiter_core::application::guided::PreparationStatus>,
    app: tauri::AppHandle,
) -> Result<Retained, Failure> {
    let service = runtime(&app)?.signing();
    let sink = Arc::new(move |step| {
        let _ = progress.send(step);
    });
    tauri::async_runtime::spawn(
        async move { service.execute_preparation(&token, consents, sink).await },
    )
    .await
    .map_err(|_| {
        crate::failure::internal(
            "Preparation worker stopped. Check Apple account resources before retrying.",
        )
    })?
    .map_err(Into::into)
}

/// Release a reviewed preparation without mutating Apple or the phone.
#[tauri::command]
pub fn library_discard_preparation(token: String, app: tauri::AppHandle) -> Result<(), Failure> {
    runtime(&app)?
        .signing()
        .discard_preparation(Some(&token))
        .map_err(Into::into)
}

/// Read preparation progress after navigating away or reconnecting a window.
#[tauri::command]
pub fn library_preparation_status(
    app: tauri::AppHandle,
) -> Result<orbiter_core::application::guided::PreparationStatus, Failure> {
    runtime(&app)?
        .signing()
        .preparation_status()
        .map_err(Into::into)
}
