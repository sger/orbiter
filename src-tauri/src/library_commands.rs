use super::*;
use orbiter_core::library::{Artifact, Imported, Library, Opened, Snapshot};
pub fn storage(app: &tauri::AppHandle) -> Result<Library, String> {
    Ok(Library::new(
        app.path()
            .app_data_dir()
            .map_err(|_| "Cannot locate application storage.")?
            .join("library"),
    ))
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
#[tauri::command]
pub async fn library_open(artifact_id: String, app: tauri::AppHandle) -> Result<Opened, String> {
    let library = storage(&app)?;
    tokio::task::spawn_blocking(move || library.open(&artifact_id))
        .await
        .map_err(|_| "Library worker stopped.")?
}
#[tauri::command]
pub async fn library_remove(
    app_id: String,
    artifact_id: Option<String>,
    app: tauri::AppHandle,
    state: State<'_, Installations>,
) -> Result<(), String> {
    let _gate = state
        .gate
        .clone()
        .try_lock_owned()
        .map_err(|_| "An installation operation is in progress. Wait before removing files.")?;
    {
        let mut saved = state
            .plan
            .lock()
            .map_err(|_| "Installation state unavailable.")?;
        if saved
            .as_ref()
            .and_then(|p| p.library_artifact.as_ref())
            .is_some_and(|a| {
                a.app_id == app_id
                    && artifact_id
                        .as_ref()
                        .is_none_or(|id| a.id == *id || a.source_id.as_ref() == Some(id))
            })
        {
            // Explicit removal cancels a pending review; executing jobs hold the gate above.
            saved.take();
        }
    }
    let library = storage(&app)?;
    tokio::task::spawn_blocking(move || library.remove(&app_id, artifact_id.as_deref()))
        .await
        .map_err(|_| "Library worker stopped.")?
}
#[tauri::command]
pub async fn library_prepare_install(
    artifact_id: String,
    device_id: u32,
    app: tauri::AppHandle,
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
    let library = storage(&app)?;
    let (artifact, path, lease) = tokio::task::spawn_blocking(move || library.pin(&artifact_id))
        .await
        .map_err(|_| "Library worker stopped.")??;
    let mut plan = installation::prepare(path, device_id).await?;
    if plan.review.sha256 != artifact.sha256 {
        return Err("Managed IPA changed. Import the original again.".into());
    }
    plan.library_artifact = Some(artifact);
    plan.library_lease = Some(lease);
    let review = plan.review.clone();
    *state
        .plan
        .lock()
        .map_err(|_| "Installation state unavailable.")? = Some(plan);
    Ok(review)
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
        .await?;
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
        // The library is durable before removing this operation's staging output.
        let opened = library.open(&artifact.id)?;
        let _ = std::fs::remove_file(&signed.path);
        let mut signed = signed;
        signed.path = opened.path;
        Ok(SavedSigned { signed, artifact })
    })
    .await
    .map_err(|_| "Signed artifact storage worker stopped.")?
}
