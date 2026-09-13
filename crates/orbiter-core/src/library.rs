//! Local managed IPAs and an append-only history of installation attempts.
//! Only opaque device tags leave Rust; artifacts are resolved by ID, never caller paths.
use crate::{
    Report,
    installation::job::{JobStatus, Stage},
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, atomic::AtomicBool},
};

static STORE: Mutex<()> = Mutex::new(());
static PINS: Mutex<BTreeMap<PathBuf, usize>> = Mutex::new(BTreeMap::new());
/// Bumped whenever the manifest gains a field. Reading an older one is fine — its new fields are
/// simply unknown — but an older Orbiter must refuse a newer manifest rather than load it and
/// silently drop every field it does not know on its next save.
const SCHEMA: u32 = 2;
const MAX_IPA: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MANIFEST: u64 = 64 * 1024 * 1024;
type Result<T> = std::result::Result<T, String>;
fn now() -> i64 {
    crate::renewal::now_unix(std::time::SystemTime::now())
}
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct App {
    pub id: String,
    pub identifier: String,
    pub name: String,
    /// The icon's own SHA-256; the bytes live beside the artifacts, not in this file.
    ///
    /// They used to be a base64 data URL stored inline. A single icon may be 2 MiB, which is
    /// ~2.8 MiB of base64, against a 64 MiB manifest — so a library filled up at around twenty
    /// apps, and every refresh re-sent every icon to the window.
    #[serde(default)]
    pub icon_sha: Option<String>,
    pub added_unix: i64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub app_id: String,
    pub source_id: Option<String>,
    pub sha256: String,
    pub name: String,
    pub identifier: String,
    pub version: Option<String>,
    pub build: Option<String>,
    pub size_bytes: u64,
    pub added_unix: i64,
    pub expires: Option<String>,
    /// The same moment as `expires`. Both are taken from one chosen profile, never picked
    /// separately, so the date shown and the date counted can never describe different bundles.
    #[serde(default)]
    pub expires_unix: Option<i64>,
    pub team_tag: Option<String>,
    pub watch: Option<String>,
    pub marker: Option<String>,
    pub deleted: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub last_seen_unix: i64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub id: String,
    pub app_id: String,
    pub artifact_id: String,
    pub device_id: String,
    pub app_name: String,
    pub identifier: String,
    pub version: Option<String>,
    pub build: Option<String>,
    pub sha256: String,
    pub signed: bool,
    pub expires: Option<String>,
    #[serde(default)]
    pub expires_unix: Option<i64>,
    /// Copied from the artifact rather than joined back to it: a saved file can be removed while
    /// its history is kept, and a countdown that vanished because someone tidied up would be the
    /// same silent loss this whole module exists to prevent.
    #[serde(default)]
    pub team_tag: Option<String>,
    pub started_unix: i64,
    pub finished_unix: Option<i64>,
    pub stage: Stage,
    pub message: String,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    schema: u32,
    salt: String,
    apps: Vec<App>,
    artifacts: Vec<Artifact>,
    devices: Vec<Device>,
    attempts: Vec<Attempt>,
    #[serde(default)]
    pending_removals: Vec<String>,
    #[serde(default)]
    pending_icon_removals: Vec<String>,
}
impl Default for Manifest {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            salt: id(),
            apps: vec![],
            artifacts: vec![],
            devices: vec![],
            attempts: vec![],
            pending_removals: vec![],
            pending_icon_removals: vec![],
        }
    }
}
#[derive(Serialize)]
pub struct Snapshot {
    pub apps: Vec<App>,
    /// Every successful install with a known expiry, longest-lived first within each app.
    pub expiries: Vec<Expiry>,
    pub artifacts: Vec<Artifact>,
    pub devices: Vec<Device>,
    pub attempts: Vec<Attempt>,
    pub storage_bytes: u64,
    /// Bytes on disk that no record points at — left behind when a copy was interrupted. Counted
    /// and named rather than deleted: Orbiter removes files somebody asked it to remove.
    pub unreferenced_bytes: u64,
    pub storage_warning: Option<String>,
}
/// One successful installation and where its seven days stand.
///
/// The honest moment for a countdown is an install, not an import: a saved file is a fact about
/// this Mac, and counting down from it would be Orbiter claiming an installation it never
/// performed. So this exists only for an attempt that reached `Installed` and whose expiry is
/// actually known — unknown is not zero, and rendering "expired long ago" for a record written
/// before Orbiter kept epoch seconds would invent a fact.
///
/// The sentence is built by `renewal::line`, the same function the legacy record uses, so the
/// library page, the workspace and a screenshot of either cannot word the same fact differently.
#[derive(Clone, Serialize)]
pub struct Expiry {
    pub app_id: String,
    pub artifact_id: String,
    pub attempt_id: String,
    pub device_id: String,
    /// Named so a person with two testers can tell which phone the line is about.
    pub device_name: String,
    pub app_name: String,
    pub identifier: String,
    pub signed: bool,
    pub expires: Option<String>,
    pub expires_unix: i64,
    pub installed_unix: i64,
    pub standing: crate::renewal::Standing,
    pub bearing: crate::renewal::Bearing,
    pub sentence: String,
    pub urgent: bool,
}

#[derive(Serialize)]
pub struct Imported {
    pub app_id: String,
    pub artifact_id: String,
    pub duplicate: bool,
}
#[derive(Serialize)]
pub struct Opened {
    pub artifact: Artifact,
    pub report: Report,
    pub path: String,
}

/// Keeps a reviewed/signing artifact alive until its operation ends.
pub struct Lease {
    path: PathBuf,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut pins) = PINS.lock()
            && let Some(count) = pins.get_mut(&self.path)
        {
            *count -= 1;
            if *count == 0 {
                pins.remove(&self.path);
            }
        }
    }
}
#[derive(Clone)]
pub struct Library {
    root: PathBuf,
}
impl Library {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    fn manifest_path(&self) -> PathBuf {
        self.root.join("manifest.json")
    }
    fn read(&self) -> Result<Manifest> {
        let path = self.manifest_path();
        if !path.exists() {
            // Only files named the way this library names them count as "managed files remain".
            // An import stages its copy in the same directory before touching the manifest, and a
            // half-written temporary is not evidence that somebody lost their library.
            let managed = match fs::read_dir(self.root.join("artifacts")) {
                Ok(entries) => entries
                    .filter_map(|entry| entry.ok())
                    .any(|entry| managed_name(&entry.file_name())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(_) => return Err("Cannot read library directory.".into()),
            };
            if managed {
                return Err("Library manifest is missing but managed files remain. Restore the manifest from a backup; files have not been removed.".into());
            }
            return Ok(Manifest::default());
        }
        if fs::metadata(&path)
            .map_err(|_| "Cannot inspect library storage.")?
            .len()
            > MAX_MANIFEST
        {
            return Err("Library manifest exceeds its supported size.".into());
        }
        let mut bytes = vec![];
        fs::File::open(path)
            .map_err(|_| "Cannot read library storage.")?
            .take(MAX_MANIFEST + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Cannot read library storage.")?;
        if bytes.len() as u64 > MAX_MANIFEST {
            return Err("Library manifest exceeds its supported size.".into());
        }
        let mut m: Manifest = serde_json::from_slice(&bytes).map_err(|_| "Library storage is corrupt. Your files have not been removed; restore the manifest from a backup.")?;
        if m.schema == 0 || m.schema > SCHEMA {
            return Err("This library requires a different Orbiter version.".into());
        }
        // An older manifest opens as it is; the upgrade lands on the next save. Nothing is
        // backfilled — an expiry Orbiter never recorded stays unknown rather than being guessed
        // at by parsing a display date back into a number.
        m.schema = SCHEMA;
        if uuid::Uuid::parse_str(&m.salt).is_err() {
            return Err("Invalid library device salt.".into());
        }
        for a in &m.artifacts {
            if !valid_hash(&a.sha256) {
                return Err("Invalid library artifact hash.".into());
            }
        }
        if m.apps
            .iter()
            .filter_map(|a| a.icon_sha.as_deref())
            .chain(m.pending_icon_removals.iter().map(String::as_str))
            .any(|hash| !valid_hash(hash))
        {
            return Err("Invalid library icon hash.".into());
        }
        let unique = |ids: Vec<&str>| {
            let count = ids.len();
            ids.into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == count
        };
        if !unique(m.apps.iter().map(|a| a.id.as_str()).collect())
            || !unique(m.artifacts.iter().map(|a| a.id.as_str()).collect())
            || !unique(m.devices.iter().map(|a| a.id.as_str()).collect())
            || !unique(m.attempts.iter().map(|a| a.id.as_str()).collect())
        {
            return Err(
                "Library contains duplicate record identities. Restore its manifest from a backup."
                    .into(),
            );
        }
        if m.artifacts.iter().any(|a| {
            !m.apps.iter().any(|app| app.id == a.app_id)
                || a.source_id.as_ref().is_some_and(|source| {
                    !m.artifacts
                        .iter()
                        .any(|s| s.id == *source && s.app_id == a.app_id && s.source_id.is_none())
                })
        }) || m.attempts.iter().any(|a| {
            !m.artifacts
                .iter()
                .any(|f| f.id == a.artifact_id && f.app_id == a.app_id)
                || !m.devices.iter().any(|d| d.id == a.device_id)
        }) || m.pending_removals.iter().any(|h| !valid_hash(h))
        {
            return Err(
                "Library contains broken record relationships. Restore its manifest from a backup."
                    .into(),
            );
        }
        Ok(m)
    }
    fn save(&self, m: &Manifest) -> Result<()> {
        fs::create_dir_all(&self.root)
            .map_err(|_| "Cannot create library storage. Check disk space and permissions.")?;
        let mut temp = tempfile::NamedTempFile::new_in(&self.root)
            .map_err(|_| "Cannot write library storage. Check disk space and permissions.")?;
        serde_json::to_writer(&mut temp, m)
            .map_err(|_| "Cannot write library metadata. Check disk space.")?;
        if temp
            .as_file()
            .metadata()
            .map_err(|_| "Cannot inspect library metadata.")?
            .len()
            > MAX_MANIFEST
        {
            return Err(
                "Library metadata is full. Export a backup before removing old entries.".into(),
            );
        }
        temp.flush()
            .and_then(|_| temp.as_file().sync_all())
            .map_err(|_| "Cannot flush library storage. Check disk space.")?;
        temp.persist(self.manifest_path())
            .map_err(|_| "Cannot commit library metadata. Check permissions.")?;
        Ok(())
    }
    fn file(&self, hash: &str) -> Result<PathBuf> {
        if !valid_hash(hash) {
            return Err("Invalid library artifact hash.".into());
        }
        Ok(self.root.join("artifacts").join(format!("{hash}.ipa")))
    }
    fn icon_file(&self, hash: &str) -> Result<PathBuf> {
        if !valid_hash(hash) {
            return Err("Invalid library icon hash.".into());
        }
        Ok(self.root.join("icons").join(format!("{hash}.png")))
    }
    /// Save an icon's bytes under their own hash and return it. Identical icons share one file.
    fn keep_icon(&self, data_url: &str) -> Result<String> {
        let encoded = data_url
            .strip_prefix("data:image/png;base64,")
            .ok_or("Unsupported icon encoding.")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| "Unreadable icon.".to_string())?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let path = self.icon_file(&hash)?;
        if path.exists() {
            return Ok(hash);
        }
        let parent = path.parent().ok_or("Invalid library storage location.")?;
        fs::create_dir_all(parent).map_err(|_| "Cannot create library icon storage.")?;
        let mut temp =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| "Cannot write a library icon.")?;
        temp.write_all(&bytes)
            .and_then(|_| temp.flush())
            .map_err(|_| "Cannot write a library icon.")?;
        temp.persist(path)
            .map_err(|_| "Cannot save a library icon.")?;
        Ok(hash)
    }
    /// The icon bytes for a hash, as the data URL the window can render.
    pub fn icon(&self, hash: &str) -> Result<Option<String>> {
        let path = self.icon_file(hash)?;
        // An icon is decoration. A missing or unreadable one is a placeholder, never an error.
        let Ok(bytes) = fs::read(&path) else {
            return Ok(None);
        };
        if bytes.len() > 4 * 1024 * 1024 {
            return Ok(None);
        }
        Ok(Some(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )))
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.snapshot_at(std::time::SystemTime::now())
    }
    /// The clock is a parameter so a test can stand on a day boundary; nothing else passes one.
    pub fn snapshot_at(&self, now: std::time::SystemTime) -> Result<Snapshot> {
        let _lock = STORE.lock().map_err(|_| "Library is unavailable.")?;
        let m = self.read()?;
        let expiries = expiries(&m, now);
        let mut storage_bytes = 0;
        let mut unreferenced_bytes = 0;
        let kept: BTreeSet<&str> = m
            .artifacts
            .iter()
            .filter(|a| !a.deleted)
            .map(|a| a.sha256.as_str())
            .collect();
        match fs::read_dir(self.root.join("artifacts")) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|_| "Cannot read library storage usage.")?;
                    let metadata = entry
                        .metadata()
                        .map_err(|_| "Cannot read library file size.")?;
                    if metadata.is_file() {
                        storage_bytes += metadata.len();
                        if !unreferenced(&entry.file_name(), &kept) {
                            continue;
                        }
                        unreferenced_bytes += metadata.len();
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("Cannot read library storage usage.".into()),
        }
        Ok(Snapshot {
            storage_warning: (!m.pending_removals.is_empty() || !m.pending_icon_removals.is_empty()).then(|| "Some requested file removals are still pending. Check disk permissions and restart Orbiter to retry cleanup.".into()),
            storage_bytes,
            unreferenced_bytes,
            expiries,
            apps: m.apps,
            artifacts: m.artifacts,
            devices: m.devices,
            attempts: m.attempts,
        })
    }
    pub fn import(&self, source: &Path) -> Result<Imported> {
        // Copying, hashing and inspecting up to 2 GiB happens before the lock is taken. Holding
        // it across all of that froze every other library call — including the list the window
        // refreshes — for the length of the copy, and none of it touches the manifest: it writes
        // one temporary file that nothing else can see until `publish` renames it.
        let (temp, hash, report) = self.stage(source)?;
        let _lock = STORE.lock().map_err(|_| "Library is unavailable.")?;
        let mut m = self.read()?;
        if !self.manifest_path().exists() {
            self.save(&m)?;
        }
        if let Some(a) = m
            .artifacts
            .iter()
            .find(|a| a.sha256 == hash && a.source_id.is_none() && !a.deleted)
        {
            // A repeat import also repairs a missing or damaged managed copy.
            self.publish(temp, &hash)?;
            return Ok(Imported {
                app_id: a.app_id.clone(),
                artifact_id: a.id.clone(),
                duplicate: true,
            });
        }
        let main = report
            .bundles
            .iter()
            .find(|b| b.path == report.main_path)
            .ok_or("IPA has no main app.")?;
        if main.identifier.is_empty() {
            return Err("IPA has no bundle identifier to group in the library.".into());
        }
        let app_id = if let Some(app) = m.apps.iter_mut().find(|a| a.identifier == main.identifier)
        {
            app.name = main.name.clone();
            if let Some(icon) = &report.icon_data_url {
                app.icon_sha = self.keep_icon(icon).ok();
            }
            app.id.clone()
        } else {
            let app_id = id();
            m.apps.push(App {
                id: app_id.clone(),
                identifier: main.identifier.clone(),
                name: main.name.clone(),
                icon_sha: report
                    .icon_data_url
                    .as_deref()
                    .and_then(|icon| self.keep_icon(icon).ok()),
                added_unix: now(),
            });
            app_id
        };
        let artifact = from_report(&report, app_id.clone(), hash.clone())?;
        let result = Imported {
            app_id,
            artifact_id: artifact.id.clone(),
            duplicate: false,
        };
        self.publish(temp, &hash)?;
        m.artifacts.push(artifact);
        if let Err(error) = self.save(&m) {
            // The bytes are on disk but nothing references them. Removing what this operation
            // itself just published is an operation cleaning up after its own failure — not the
            // sweep of unreferenced files that Orbiter deliberately never performs on its own.
            if !m
                .artifacts
                .iter()
                .any(|a| !a.deleted && a.sha256 == hash && a.id != result.artifact_id)
                && let Ok(path) = self.file(&hash)
            {
                let _ = fs::remove_file(path);
            }
            return Err(error);
        }
        Ok(result)
    }
    fn stage(&self, source: &Path) -> Result<(tempfile::NamedTempFile, String, Report)> {
        let metadata = fs::metadata(source)
            .map_err(|_| "IPA is missing or unreadable. Choose a readable IPA.")?;
        if !metadata.is_file() || metadata.len() > MAX_IPA {
            return Err("Choose a regular IPA file smaller than 2 GiB.".into());
        }
        fs::create_dir_all(self.root.join("artifacts"))
            .map_err(|_| "Cannot create library storage. Check disk space and permissions.")?;
        let mut temp = tempfile::NamedTempFile::new_in(self.root.join("artifacts"))
            .map_err(|_| "Cannot create managed IPA. Check disk space.")?;
        let mut input = fs::File::open(source)
            .map_err(|_| "Cannot read selected IPA.")?
            .take(MAX_IPA + 1);
        let size = std::io::copy(&mut input, &mut temp)
            .map_err(|_| "Cannot copy IPA. Check free disk space and source availability.")?;
        if size > MAX_IPA {
            return Err("IPA grew beyond the 2 GiB limit during import.".into());
        }
        temp.flush()
            .and_then(|_| temp.as_file().sync_all())
            .map_err(|_| "Cannot flush managed IPA. Check disk space.")?;
        let hash = hash_file(temp.path())?;
        let report = crate::inspect(temp.path(), &AtomicBool::new(false), |_| {})
            .map_err(|e| e.to_string())?;
        Ok((temp, hash, report))
    }
    fn publish(&self, temp: tempfile::NamedTempFile, hash: &str) -> Result<()> {
        let dest = self.file(hash)?;
        // Avoid replacing an artifact currently in use. Identical valid bytes need no write.
        if dest.exists() && hash_file(&dest).is_ok_and(|actual| actual == hash) {
            return Ok(());
        }
        if PINS
            .lock()
            .map_err(|_| "Library is unavailable.")?
            .contains_key(&dest)
        {
            return Err("This artifact is in use; retry after the operation finishes.".into());
        }
        temp.persist(dest)
            .map_err(|_| "Cannot save managed IPA. Check disk space and permissions.")?;
        Ok(())
    }
    pub fn pin(&self, artifact_id: &str) -> Result<(Artifact, PathBuf, Lease)> {
        let _lock = STORE.lock().map_err(|_| "Library is unavailable.")?;
        let m = self.read()?;
        let a = m
            .artifacts
            .iter()
            .find(|a| a.id == artifact_id && !a.deleted)
            .cloned()
            .ok_or("This library artifact was removed.")?;
        let path = self.file(&a.sha256)?;
        if hash_file(&path)? != a.sha256 {
            return Err(
                "Managed IPA changed or is damaged. Import the original again before continuing."
                    .into(),
            );
        }
        *PINS
            .lock()
            .map_err(|_| "Library is unavailable.")?
            .entry(path.clone())
            .or_default() += 1;
        Ok((a, path.clone(), Lease { path }))
    }
    /// Where the seven days stand for the build on screen, if it was ever installed.
    ///
    /// "On screen" is an artifact, not a bundle identifier: the workspace is opened from one, so
    /// the app half of the question is settled by construction and only the team can differ. A
    /// signed build made from this original counts as the same build — opening the original to
    /// re-sign it is exactly when its expiry is worth knowing — but a different original of the
    /// same app does not, because it was never the thing that was installed.
    pub fn expiry(&self, artifact_id: &str, team_tag: Option<&str>) -> Result<Option<Expiry>> {
        self.expiry_at(artifact_id, team_tag, std::time::SystemTime::now())
    }
    pub fn expiry_at(
        &self,
        artifact_id: &str,
        team_tag: Option<&str>,
        now: std::time::SystemTime,
    ) -> Result<Option<Expiry>> {
        let _lock = STORE.lock().map_err(|_| "Library is unavailable.")?;
        let m = self.read()?;
        let lineage: BTreeSet<&str> = m
            .artifacts
            .iter()
            .filter(|a| a.id == artifact_id || a.source_id.as_deref() == Some(artifact_id))
            .map(|a| a.id.as_str())
            .collect();
        Ok(expiries(&m, now)
            .into_iter()
            .find(|e| lineage.contains(e.artifact_id.as_str()))
            .map(|mut found| {
                // The only thing that can still be wrong is the team: it changes while the
                // workspace is open, and a countdown about a build signed for someone else is
                // worse than silence.
                found.bearing = match (team_tag, attempt_team(&m, &found.attempt_id)) {
                    (Some(asked), Some(held)) if asked != held => {
                        crate::renewal::Bearing::OtherTeam
                    }
                    _ => crate::renewal::Bearing::SameApp,
                };
                found.sentence =
                    crate::renewal::line(&found.app_name, found.standing, found.bearing);
                found.urgent = crate::renewal::urgent(found.standing, found.bearing);
                found
            }))
    }
    /// Compatibility for older path-based clients. Resolve a managed path back to its exact
    /// retained artifact; external paths are imported as originals before any operation.
    pub fn resolve_or_import(&self, path: &Path) -> Result<String> {
        {
            let _lock = STORE.lock().map_err(|_| "Library is unavailable.")?;
            let m = self.read()?;
            for artifact in m.artifacts.iter().rev().filter(|a| !a.deleted) {
                if self.file(&artifact.sha256)? == path {
                    return Ok(artifact.id.clone());
                }
            }
        }
        Ok(self.import(path)?.artifact_id)
    }
    pub fn open(&self, artifact_id: &str) -> Result<Opened> {
        let (artifact, path, _lease) = self.pin(artifact_id)?;
        let report =
            crate::inspect(&path, &AtomicBool::new(false), |_| {}).map_err(|e| e.to_string())?;
        if artifact.source_id.is_none()
            && let Some(icon) = &report.icon_data_url
            && let Ok(hash) = self.keep_icon(icon)
        {
            let _lock = STORE.lock().map_err(|_| "Library is unavailable.")?;
            let mut m = self.read()?;
            // Comparing two hashes rather than two multi-megabyte strings, which is what this
            // was doing on every open.
            if let Some(app) = m.apps.iter_mut().find(|a| a.id == artifact.app_id)
                && app.icon_sha.as_deref() != Some(hash.as_str())
            {
                app.icon_sha = Some(hash);
                self.save(&m)?;
            }
        }
        Ok(Opened {
            artifact,
            report,
            path: path.to_string_lossy().into_owned(),
        })
    }
    pub fn retain_signed(
        &self,
        source_id: &str,
        signed: &crate::signer::Signed,
        team_tag: String,
        watch: String,
        marker: String,
    ) -> Result<Artifact> {
        let _lock = STORE.lock().map_err(|_| "Library is unavailable.")?;
        let mut m = self.read()?;
        let source = m
            .artifacts
            .iter()
            .find(|a| a.id == source_id && !a.deleted && a.source_id.is_none())
            .cloned()
            .ok_or("Original version is unavailable.")?;
        let (temp, hash, report) = self.stage(Path::new(&signed.path))?;
        let mut artifact = from_report(&report, source.app_id, hash.clone())?;
        artifact.source_id = Some(source.id);
        artifact.expires = (!signed.expires.is_empty()).then(|| signed.expires.clone());
        artifact.expires_unix = (signed.expires_unix > 0).then_some(signed.expires_unix);
        artifact.team_tag = Some(team_tag);
        artifact.watch = Some(watch);
        artifact.marker = Some(marker);
        self.publish(temp, &hash)?;
        m.artifacts.push(artifact.clone());
        self.save(&m)?;
        Ok(artifact)
    }
    pub fn remove(&self, app_id: &str, artifact_id: Option<&str>) -> Result<()> {
        let _lock = STORE.lock().map_err(|_| "Library is unavailable.")?;
        let mut m = self.read()?;
        if !m.apps.iter().any(|a| a.id == app_id) {
            return Err("App is not in this library.".into());
        }
        if let Some(id) = artifact_id
            && !m
                .artifacts
                .iter()
                .any(|a| a.id == id && a.app_id == app_id && !a.deleted)
        {
            return Err("Version is not in this app.".into());
        }
        let matches = |a: &Artifact| {
            a.app_id == app_id
                && artifact_id.is_none_or(|id| a.id == id || a.source_id.as_deref() == Some(id))
        };
        let paths: Vec<PathBuf> = m
            .artifacts
            .iter()
            .filter(|a| matches(a) && !a.deleted)
            .map(|a| self.file(&a.sha256))
            .collect::<Result<_>>()?;
        {
            let pins = PINS.lock().map_err(|_| "Library is unavailable.")?;
            if paths.iter().any(|p| pins.contains_key(p)) {
                return Err(
                    "An artifact is in use. Wait for its operation to finish before removing it."
                        .into(),
                );
            }
        }
        let hashes: Vec<String> =
            m.artifacts
                .iter()
                .filter(|a| {
                    matches(a)
                        && !a.deleted
                        && !m.artifacts.iter().any(|other| {
                            !other.deleted && !matches(other) && other.sha256 == a.sha256
                        })
                })
                .map(|a| a.sha256.clone())
                .collect();
        m.pending_removals.extend(hashes);
        if artifact_id.is_some() {
            for a in &mut m.artifacts {
                if matches(a) {
                    a.deleted = true;
                }
            }
        } else {
            // The app's icon goes with it, unless another app happens to have the same one.
            let icon = m
                .apps
                .iter()
                .find(|a| a.id == app_id)
                .and_then(|a| a.icon_sha.clone());
            m.apps.retain(|a| a.id != app_id);
            if let Some(icon) = icon
                && !m.apps.iter().any(|a| a.icon_sha.as_deref() == Some(&icon))
            {
                m.pending_icon_removals.push(icon);
            }
            m.artifacts.retain(|a| a.app_id != app_id);
            m.attempts.retain(|a| a.app_id != app_id);
        }
        // Commit the user's removal before deleting bytes. Failed metadata writes leave files intact.
        self.save(&m)?;
        self.finish_removals(&mut m)
    }
    fn finish_removals(&self, m: &mut Manifest) -> Result<()> {
        if m.pending_removals.is_empty() && m.pending_icon_removals.is_empty() {
            return Ok(());
        }
        let mut icons = Vec::new();
        for hash in &m.pending_icon_removals {
            // A repeat import may have reintroduced the same icon after interrupted cleanup.
            if m.apps.iter().any(|a| a.icon_sha.as_deref() == Some(hash)) {
                continue;
            }
            match fs::remove_file(self.icon_file(hash)?) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                // An icon is decoration; failing to delete one must not stop the cleanup that
                // matters, so it is kept on the list and tried again next time.
                Err(_) => icons.push(hash.clone()),
            }
        }
        m.pending_icon_removals = icons;
        if m.pending_removals.is_empty() {
            return self.save(m);
        }
        for hash in &m.pending_removals {
            // A repeat import may have reintroduced these bytes after interrupted cleanup.
            if m.artifacts.iter().any(|a| !a.deleted && a.sha256 == *hash) {
                continue;
            }
            let path = self.file(hash)?;
            if PINS
                .lock()
                .map_err(|_| "Library is unavailable.")?
                .contains_key(&path)
            {
                return Err(
                    "Removed file is still in use. Retry cleanup after the operation finishes."
                        .into(),
                );
            }
            match fs::remove_file(path) { Ok(()) => {}, Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}, Err(_) => return Err("Removal was recorded, but some files could not be deleted. Check permissions and restart Orbiter to retry cleanup.".into()) }
        }
        m.pending_removals.clear();
        self.save(m)
    }
    /// Delete managed files no record points at, and say how many bytes went.
    ///
    /// Only ever on request. Startup cleanup finishes removals a person already asked for; bytes
    /// left behind by an interrupted copy are reported and kept until someone says otherwise,
    /// because a tool that deletes files nobody asked it to delete cannot be trusted with any.
    pub fn reclaim(&self) -> Result<u64> {
        let _lock = STORE.lock().map_err(|_| "Library is unavailable.")?;
        let m = self.read()?;
        let kept: BTreeSet<&str> = m
            .artifacts
            .iter()
            .filter(|a| !a.deleted)
            .map(|a| a.sha256.as_str())
            .collect();
        let pins = PINS.lock().map_err(|_| "Library is unavailable.")?;
        let mut freed = 0;
        let entries = match fs::read_dir(self.root.join("artifacts")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(_) => return Err("Cannot read library storage.".into()),
        };
        for entry in entries {
            let entry = entry.map_err(|_| "Cannot read library storage.")?;
            if !unreferenced(&entry.file_name(), &kept) {
                continue;
            }
            let path = entry.path();
            if pins.contains_key(&path) || !path.is_file() {
                continue;
            }
            let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            match fs::remove_file(&path) {
                Ok(()) => freed += size,
                Err(_) => {
                    return Err(
                        "Some unreferenced files could not be removed. Check permissions.".into(),
                    );
                }
            }
        }
        Ok(freed)
    }
    pub fn device_tag(&self, udid: &str) -> Result<String> {
        let _lock = STORE.lock().map_err(|_| "Library is unavailable.")?;
        let m = self.read()?;
        if !self.manifest_path().exists() {
            self.save(&m)?;
        }
        Ok(tag(&m.salt, udid))
    }
    pub fn begin(&self, artifact: &Artifact, job_id: &str, udid: &str, name: &str) -> Result<()> {
        let _lock = STORE.lock().map_err(|_| "Library is unavailable.")?;
        let mut m = self.read()?;
        if !m.artifacts.iter().any(|a| {
            a.id == artifact.id
                && a.app_id == artifact.app_id
                && a.sha256 == artifact.sha256
                && !a.deleted
        }) {
            return Err("Reviewed artifact is no longer available in the library.".into());
        }
        if udid.is_empty() {
            return Err("Verified device identity is missing.".into());
        }
        if m.attempts.iter().any(|a| a.id == job_id) {
            return Err("This installation attempt was already recorded. Review again.".into());
        }
        let device_id = tag(&m.salt, udid);
        if let Some(device) = m.devices.iter_mut().find(|d| d.id == device_id) {
            device.name = name.into();
            device.last_seen_unix = now();
        } else {
            m.devices.push(Device {
                id: device_id.clone(),
                name: name.into(),
                last_seen_unix: now(),
            });
        }
        m.attempts.push(Attempt {
            id: job_id.into(),
            app_id: artifact.app_id.clone(),
            artifact_id: artifact.id.clone(),
            device_id,
            app_name: artifact.name.clone(),
            identifier: artifact.identifier.clone(),
            version: artifact.version.clone(),
            build: artifact.build.clone(),
            sha256: artifact.sha256.clone(),
            signed: artifact.source_id.is_some(),
            expires: artifact.expires.clone(),
            expires_unix: artifact.expires_unix,
            team_tag: artifact.team_tag.clone(),
            started_unix: now(),
            finished_unix: None,
            stage: Stage::Preparing,
            message: "Preparing installation.".into(),
        });
        self.save(&m)
    }
    pub fn update(&self, status: &JobStatus) -> Result<()> {
        let _lock = STORE.lock().map_err(|_| "Library is unavailable.")?;
        let mut m = self.read()?;
        let Some(attempt) = m.attempts.iter_mut().find(|a| a.id == status.id) else {
            return Ok(());
        };
        if attempt.stage.terminal() {
            return Ok(());
        }
        // Bytes/progress are transient; only durable state transitions belong in the library.
        if attempt.stage == status.stage {
            return Ok(());
        }
        attempt.stage = status.stage;
        attempt.message = status.message.clone();
        if status.stage.terminal() {
            attempt.finished_unix = Some(now());
        }
        self.save(&m)
    }
    /// Called once on process startup, before any new jobs can run.
    pub fn recover(&self, journal: Option<&JobStatus>) -> Result<()> {
        let _lock = STORE.lock().map_err(|_| "Library is unavailable.")?;
        let mut m = self.read()?;
        let mut changed = false;
        for attempt in &mut m.attempts {
            if !attempt.stage.terminal() {
                if let Some(status) = journal.filter(|s| s.id == attempt.id && s.stage.terminal()) {
                    attempt.stage = status.stage;
                    attempt.message = status.message.clone();
                } else {
                    attempt.stage = Stage::Unknown;
                    attempt.message = "Orbiter stopped before the result was recorded. Check the app on the device before retrying.".into();
                }
                attempt.finished_unix = Some(now());
                changed = true;
            }
        }
        if changed {
            self.save(&m)?;
        }
        self.finish_removals(&mut m)
    }
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}
fn hash_file(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "Managed IPA is missing or unreadable. Import the original again.")?;
    if !metadata.is_file() || metadata.len() > MAX_IPA {
        return Err("Invalid managed IPA file.".into());
    }
    let mut file = fs::File::open(path).map_err(|_| "Cannot read managed IPA.")?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|_| "Cannot read managed IPA.")?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > MAX_IPA {
            return Err("Managed IPA exceeds its size limit.".into());
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn tag(salt: &str, udid: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(salt);
    hash.update([0]);
    hash.update(udid);
    format!("{:x}", hash.finalize())
}
fn from_report(report: &Report, app_id: String, hash: String) -> Result<Artifact> {
    let main = report
        .bundles
        .iter()
        .find(|b| b.path == report.main_path)
        .ok_or("IPA has no main app.")?;
    // The build stops working when its first profile does, and the date shown and the seconds
    // counted come from that one profile rather than being chosen independently.
    let earliest = report
        .bundles
        .iter()
        .filter_map(|b| b.profile.as_ref())
        .filter_map(|p| Some((p.expires_unix?, p.expires_at.clone()?)))
        .min_by_key(|(unix, _)| *unix);
    Ok(Artifact {
        id: id(),
        app_id,
        source_id: None,
        sha256: hash,
        name: main.name.clone(),
        identifier: main.identifier.clone(),
        version: main.version.clone(),
        build: main.build.clone(),
        size_bytes: report.size_bytes,
        added_unix: now(),
        expires: earliest.as_ref().map(|(_, text)| text.clone()),
        expires_unix: earliest.as_ref().map(|(unix, _)| *unix),
        team_tag: None,
        watch: None,
        marker: None,
        deleted: false,
    })
}

fn attempt_team<'a>(m: &'a Manifest, attempt_id: &str) -> Option<&'a str> {
    m.attempts
        .iter()
        .find(|a| a.id == attempt_id)?
        .team_tag
        .as_deref()
}

/// Every install worth counting down, longest-lived first within each app.
///
/// Longest-lived rather than most recent: the same app can be on two testers' phones, and a
/// re-sign installed on one does not revive the copy on the other. Leading with the copy that is
/// still working is the truthful headline, and because every qualifying attempt is returned, the
/// phone whose copy has already died is still there to be shown beside its own device.
fn expiries(m: &Manifest, now: std::time::SystemTime) -> Vec<Expiry> {
    let mut found: Vec<Expiry> = m
        .attempts
        .iter()
        .filter(|a| a.stage == Stage::Installed)
        .filter_map(|a| {
            let expires_unix = a.expires_unix?;
            let standing = crate::renewal::standing_at(expires_unix, now);
            // The library page names the app in its own row, so nothing there is about another
            // build. `expiry_at` re-decides this for the workspace, where the team can differ.
            let bearing = crate::renewal::Bearing::SameApp;
            Some(Expiry {
                app_id: a.app_id.clone(),
                artifact_id: a.artifact_id.clone(),
                attempt_id: a.id.clone(),
                device_id: a.device_id.clone(),
                device_name: m
                    .devices
                    .iter()
                    .find(|d| d.id == a.device_id)
                    .map(|d| d.name.clone())
                    .unwrap_or_default(),
                app_name: a.app_name.clone(),
                identifier: a.identifier.clone(),
                signed: a.signed,
                expires: a.expires.clone(),
                expires_unix,
                installed_unix: a.finished_unix.unwrap_or(a.started_unix),
                standing,
                bearing,
                sentence: crate::renewal::line(&a.app_name, standing, bearing),
                urgent: crate::renewal::urgent(standing, bearing),
            })
        })
        .collect();
    found.sort_by(|a, b| {
        b.expires_unix
            .cmp(&a.expires_unix)
            .then(b.installed_unix.cmp(&a.installed_unix))
    });
    found
}

/// A managed artifact file whose hash no live record mentions. Anything not named `<sha256>.ipa`
/// is not Orbiter's to judge and is left alone.
fn unreferenced(name: &std::ffi::OsStr, kept: &BTreeSet<&str>) -> bool {
    match managed_hash(name) {
        Some(hash) => !kept.contains(hash),
        None => false,
    }
}

/// `<sha256>.ipa` — the only shape this library gives a managed file.
fn managed_hash(name: &std::ffi::OsStr) -> Option<&str> {
    let hash = name.to_str()?.strip_suffix(".ipa")?;
    valid_hash(hash).then_some(hash)
}
fn managed_name(name: &std::ffi::OsStr) -> bool {
    managed_hash(name).is_some()
}
