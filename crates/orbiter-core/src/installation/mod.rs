//! Install an unchanged, already-signed IPA after a read-only review.
pub mod job;
use crate::{Report, devices::address};
use idevice::{
    IdeviceError, IdeviceService,
    afc::{AfcClient, opcode::AfcFopenMode},
    installation_proxy::InstallationProxyClient,
    lockdown::LockdownClient,
    provider::{IdeviceProvider, UsbmuxdProvider},
    usbmuxd::{Connection, UsbmuxdDevice},
};
use job::{Control, JobStatus, Stage};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, time::timeout};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExistingApp {
    pub version: Option<String>,
    pub build: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct Review {
    /// Authorises installing exactly these bytes on exactly this phone, until it expires.
    pub token: crate::domain::identifiers::ReviewToken,
    pub app_name: String,
    pub bundle_id: String,
    pub version: Option<String>,
    pub device_name: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub existing_app: Option<ExistingApp>,
    pub blockers: Vec<String>,
    pub notes: Vec<String>,
}
/// Sensitive binding stays only in Rust memory; no Debug or serialization implementation.
pub struct PreparedInstall {
    pub review: Review,
    pub library_artifact: Option<crate::library::Artifact>,
    pub library_lease: Option<crate::library::Lease>,
    snapshot: tempfile::TempDir,
    device_id: u32,
    udid: String,
    created: Instant,
}
struct Phone {
    raw: UsbmuxdDevice,
    name: String,
    version: String,
    provider: UsbmuxdProvider,
}
fn connection_error(_: IdeviceError) -> String {
    "Cannot communicate with the selected iPhone. Unlock it, check trust and the USB cable, and review again.".into()
}
/// Verified USB iPhone identity for an in-crate caller: its UDID and display name. The UDID is
/// deliberately not part of any type that crosses the IPC boundary.
pub(crate) async fn verified_identity(id: u32) -> Result<(String, String), String> {
    let phone = phone(id).await?;
    Ok((phone.raw.udid, phone.name))
}
async fn phone(id: u32) -> Result<Phone, String> {
    timeout(Duration::from_secs(10), async {
        let mut mux = address().connect(0).await.map_err(connection_error)?;
        let raw = mux
            .get_devices()
            .await
            .map_err(connection_error)?
            .into_iter()
            .find(|d| d.device_id == id)
            .ok_or("Selected iPhone disconnected. Select it again.")?;
        if raw.connection_type != Connection::Usb {
            return Err(
                "This installation preview supports USB only. Connect the iPhone by cable.".into(),
            );
        }
        let provider = raw.to_provider(address(), "Orbiter");
        let mut client = LockdownClient::connect(&provider)
            .await
            .map_err(connection_error)?;
        let pair = provider
            .get_pairing_file()
            .await
            .map_err(connection_error)?;
        client
            .start_session(&pair)
            .await
            .map_err(connection_error)?;
        let product = client
            .get_value(Some("ProductType"), None)
            .await
            .map_err(connection_error)?;
        if !product.as_string().is_some_and(|s| s.starts_with("iPhone")) {
            return Err("Selected device is not a verified physical iPhone.".into());
        }
        let name = client
            .get_value(Some("DeviceName"), None)
            .await
            .map_err(connection_error)?
            .as_string()
            .unwrap_or("iPhone")
            .to_owned();
        let version = client
            .get_value(Some("ProductVersion"), None)
            .await
            .map_err(connection_error)?
            .as_string()
            .ok_or("Cannot read the iPhone's iOS version.")?
            .to_owned();
        Ok(Phone {
            raw,
            name,
            version,
            provider,
        })
    })
    .await
    .map_err(|_| "iPhone connection timed out. Unlock it and review again.".to_string())?
}
async fn existing(provider: &UsbmuxdProvider, bundle: &str) -> Result<Option<ExistingApp>, String> {
    timeout(Duration::from_secs(10), async {
        let mut proxy = InstallationProxyClient::connect(provider)
            .await
            .map_err(connection_error)?;
        let apps = proxy
            .get_apps(Some("Any"), Some(vec![bundle.into()]))
            .await
            .map_err(connection_error)?;
        Ok(apps.get(bundle).map(|v| ExistingApp {
            version: v
                .as_dictionary()
                .and_then(|d| d.get("CFBundleShortVersionString"))
                .and_then(plist::Value::as_string)
                .map(str::to_owned),
            build: v
                .as_dictionary()
                .and_then(|d| d.get("CFBundleVersion"))
                .and_then(plist::Value::as_string)
                .map(str::to_owned),
        }))
    })
    .await
    .map_err(|_| "Installed-app lookup timed out. No install was attempted.".to_string())?
}
fn snapshot(path: &Path) -> Result<(tempfile::TempDir, Report, String), String> {
    let mut source = std::fs::File::open(path).map_err(|_| "Cannot open the selected IPA.")?;
    let metadata = source
        .metadata()
        .map_err(|_| "Cannot inspect the selected file.")?;
    if !metadata.is_file() || metadata.len() > 2 * 1024 * 1024 * 1024 {
        return Err("Choose a regular IPA below 2 GiB.".into());
    }
    let dir = tempfile::Builder::new()
        .prefix("orbiter-install-")
        .tempdir()
        .map_err(|_| "Cannot create a private IPA snapshot.")?;
    let target = dir.path().join("artifact.ipa");
    let mut dest = std::fs::File::create(&target).map_err(|_| "Cannot create the IPA snapshot.")?;
    let mut hasher = Sha256::new();
    let mut buf = [0; 256 * 1024];
    let mut size = 0u64;
    loop {
        let n = source
            .read(&mut buf)
            .map_err(|_| "Cannot read the selected IPA.")?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > 2 * 1024 * 1024 * 1024 {
            return Err("IPA exceeds the snapshot limit.".into());
        }
        dest.write_all(&buf[..n])
            .map_err(|_| "Not enough space for the IPA snapshot.")?;
        hasher.update(&buf[..n]);
    }
    dest.sync_all()
        .map_err(|_| "Cannot save the IPA snapshot.")?;
    let report =
        crate::inspect(&target, &AtomicBool::new(false), |_| {}).map_err(|e| e.to_string())?;
    Ok((dir, report, format!("{:x}", hasher.finalize())))
}
fn version(s: &str) -> Option<[u32; 3]> {
    let parts: Vec<_> = s.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut out = [0; 3];
    for (i, p) in parts.into_iter().enumerate() {
        out[i] = p.parse().ok()?
    }
    Some(out)
}
fn authorize(d: &plist::Dictionary, udid: &str) -> bool {
    d.get("ProvisionsAllDevices")
        .and_then(plist::Value::as_boolean)
        == Some(true)
        || d.get("ProvisionedDevices")
            .and_then(plist::Value::as_array)
            .is_some_and(|a| a.iter().any(|v| v.as_string() == Some(udid)))
}
fn package_blockers(
    report: &Report,
    path: &Path,
    udid: &str,
    ios: &str,
) -> Result<Vec<String>, String> {
    let main = report
        .bundles
        .iter()
        .find(|b| b.path == report.main_path)
        .ok_or("Main app was not found.")?;
    let mut blockers = Vec::new();
    if !main.supported_platforms.iter().any(|p| p == "iPhoneOS") {
        blockers.push("The main bundle does not declare iPhoneOS support.".into())
    }
    if !main.device_families.contains(&1) {
        blockers.push("The main bundle does not declare iPhone support.".into())
    }
    if !main
        .slices
        .iter()
        .any(|s| s.architecture == "arm64" || s.architecture == "arm64e")
    {
        blockers.push("No supported arm64 iPhone executable was found.".into())
    }
    match (main.minimum_os.as_deref().and_then(version), version(ios)) {
        (Some(min), Some(current)) if current >= min => (),
        _ => blockers.push("The iPhone does not meet a verified minimum iOS requirement.".into()),
    }
    let file = std::fs::File::open(path).map_err(|_| "Cannot read snapshot.")?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| "Cannot read snapshot archive.")?;
    for b in &report.bundles {
        if !b.issues.is_empty() || b.slices.is_empty() {
            blockers.push(format!(
                "{}: bundle inspection is incomplete.",
                b.identifier
            ));
        }
        if b.slices.iter().any(|s| s.encrypted) {
            blockers.push(format!(
                "{}: encrypted executable; obtain a company development or Ad Hoc build.",
                b.identifier
            ));
        }
        if b.kind == "Framework" {
            continue;
        }
        let Some(p) = &b.profile else {
            blockers.push(format!("{}: missing provisioning profile.", b.identifier));
            continue;
        };
        if p.expired != Some(false) {
            blockers.push(format!(
                "{}: profile expired or expiration is unknown.",
                b.identifier
            ));
        }
        // Watch profiles target the Watch, not the phone. iOS validates all nested code during installation.
        if b.path != report.main_path && !b.supported_platforms.iter().any(|p| p == "iPhoneOS") {
            continue;
        }
        let entry = archive
            .by_name(&format!("{}/embedded.mobileprovision", b.path))
            .map_err(|_| "Cannot read embedded profile.")?;
        if entry.size() > 4 * 1024 * 1024 {
            return Err("Embedded profile exceeds the size limit.".into());
        }
        let mut bytes = Vec::new();
        entry
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Cannot read embedded profile.")?;
        let d =
            crate::profile::dictionary(&bytes).map_err(|_| "Cannot decode embedded profile.")?;
        if !authorize(&d, udid) {
            blockers.push(format!("{}: the embedded profile does not authorize this iPhone. Obtain a build provisioned for this device; changing Apple accounts alone will not fix it.",b.identifier));
        }
    }
    Ok(blockers)
}
pub async fn prepare(path: PathBuf, device_id: u32) -> Result<PreparedInstall, String> {
    let phone = phone(device_id).await?;
    let (dir, report, hash) = tokio::task::spawn_blocking(move || snapshot(&path))
        .await
        .map_err(|_| "Snapshot worker stopped.")??;
    let udid = phone.raw.udid.clone();
    let ios = phone.version.clone();
    let snap_path = dir.path().join("artifact.ipa");
    let (report, blockers) = tokio::task::spawn_blocking(move || {
        let b = package_blockers(&report, &snap_path, &udid, &ios)?;
        Ok::<_, String>((report, b))
    })
    .await
    .map_err(|_| "Preflight worker stopped.")??;
    let main = report
        .bundles
        .iter()
        .find(|b| b.path == report.main_path)
        .ok_or("Main app missing.")?;
    let installed = existing(&phone.provider, &main.identifier).await?;
    let mut notes=vec!["Installation will use the unchanged IPA. Its signatures and certificate trust have not been verified by Orbiter; iOS will validate the package.".into(),"Installation may replace an existing app with the same bundle ID. Data retention and runtime functionality are not guaranteed.".into(),"Cancel is available during transfer. Once the install command is sent, iOS may finish even if Orbiter closes or disconnects.".into()];
    if report.bundles.iter().any(|b| b.kind == "Watch app") {
        notes.push("The Watch app stays included. Authorization and behavior on a paired Watch have not been verified.".into());
    }
    Ok(PreparedInstall {
        library_artifact: None,
        library_lease: None,
        review: Review {
            token: crate::domain::identifiers::ReviewToken::new(uuid::Uuid::new_v4().to_string()),
            app_name: main.name.clone(),
            bundle_id: main.identifier.clone(),
            version: main.version.clone(),
            device_name: phone.name,
            size_bytes: report.size_bytes,
            sha256: hash,
            existing_app: installed,
            blockers,
            notes,
        },
        snapshot: dir,
        device_id,
        udid: phone.raw.udid,
        created: Instant::now(),
    })
}
struct RunError {
    message: String,
    definite: bool,
}
impl From<String> for RunError {
    fn from(message: String) -> Self {
        Self {
            message,
            definite: false,
        }
    }
}
fn transport_error(e: IdeviceError) -> RunError {
    // Classify locally; never return arbitrary device responses (which may contain identifiers).
    let text = e.to_string();
    let message = if text.contains("ApplicationVerificationFailed") || text.contains("0xe8008015") {
        "iOS rejected the signature or provisioning. Obtain a correctly signed build for this device."
    } else if text.contains("DeviceLocked") || text.contains("PasswordProtected") {
        "Unlock the iPhone and review the installation again."
    } else if text.contains("NoSpace") || text.contains("DiskFull") {
        "The iPhone does not have enough free space. Free storage and review again."
    } else if text.contains("DeveloperMode") {
        "Check Developer Mode on the iPhone before trying again."
    } else {
        "Device communication or installation failed. Check the iPhone, cable, signing, and available storage."
    };
    let definite = matches!(
        e,
        IdeviceError::UnknownErrorType(_)
            | IdeviceError::InstallationProxy(_)
            | IdeviceError::DeviceLocked
    );
    RunError {
        message: message.into(),
        definite,
    }
}
async fn limited<T>(
    future: impl std::future::Future<Output = Result<T, IdeviceError>>,
) -> Result<T, RunError> {
    timeout(Duration::from_secs(15), future)
        .await
        .map_err(|_| {
            RunError::from(
                "The iPhone stopped responding. Check the cable and device before retrying."
                    .to_string(),
            )
        })?
        .map_err(transport_error)
}
fn publish(
    status: &JobStatus,
    journal: &Path,
    notify: &impl Fn(JobStatus),
) -> Result<(), RunError> {
    job::save(journal, status).map_err(RunError::from)?;
    notify(status.clone());
    Ok(())
}
fn cancel_check(control: &Control) -> Result<(), RunError> {
    if control.cancelled() {
        Err("Transfer cancelled before installation was requested."
            .to_string()
            .into())
    } else {
        Ok(())
    }
}
impl PreparedInstall {
    pub fn expired(&self) -> bool {
        self.created.elapsed() > Duration::from_secs(600)
    }
}
pub async fn execute(
    plan: PreparedInstall,
    control: Arc<Control>,
    journal: PathBuf,
    notify: impl Fn(JobStatus) + Send + Sync,
) -> JobStatus {
    let mut status = JobStatus {
        id: crate::domain::identifiers::JobId::from_review(&plan.review.token),
        stage: Stage::Preparing,
        message: "Rechecking reviewed IPA and iPhone.".into(),
        transferred_bytes: 0,
        total_bytes: plan.review.size_bytes,
        device_percent: None,
        cleanup_pending: false,
    };
    let remote = format!("PublicStaging/Orbiter-{}.ipa", plan.review.token);
    let outcome = if let Err(e) = publish(&status, &journal, &notify) {
        Err(e)
    } else {
        timeout(
            Duration::from_secs(20 * 60),
            run(&plan, &control, &journal, &remote, &mut status, &notify),
        )
        .await
        .unwrap_or_else(|_| {
            Err(
                "Installation job timed out. Check the phone before retrying."
                    .to_string()
                    .into(),
            )
        })
    };
    match outcome {
        Ok(()) => {
            status.stage = Stage::Installed;
            status.message="iOS reported installation complete. Launch the app and test its features; runtime behavior is not verified.".into();
        }
        Err(e) => {
            status.stage = if status.stage == Stage::Installing {
                if e.definite {
                    Stage::Failed
                } else {
                    Stage::Unknown
                }
            } else if control.cancelled() {
                Stage::Cancelled
            } else {
                Stage::Failed
            };
            status.message = e.message;
            if status.stage == Stage::Unknown {
                status.message.push_str(" The install command may have reached iOS. The outcome is unknown; check the app on the phone before retrying.");
            }
        }
    }
    control.transition(status.stage);
    // Unknown outcomes may still be consuming the IPA. Never remove their staging package here.
    if status.cleanup_pending && status.stage != Stage::Unknown {
        status.cleanup_pending = timeout(Duration::from_secs(10), async {
            let target = phone(plan.device_id).await?;
            if target.raw.udid != plan.udid {
                return Err("Device changed.".to_string());
            }
            let mut afc = AfcClient::connect(&target.provider)
                .await
                .map_err(connection_error)?;
            afc.remove(&remote).await.map_err(connection_error)
        })
        .await
        .map_or(true, |r| r.is_err());
    }
    if status.cleanup_pending {
        status.message.push_str(" The temporary staging IPA may remain on the iPhone; Orbiter will not delete unrelated files.");
    }
    if job::save(&journal, &status).is_err() {
        status
            .message
            .push_str(" The final result could not be saved to the job journal.");
    }
    notify(status.clone());
    status
}
async fn run(
    plan: &PreparedInstall,
    control: &Control,
    journal: &Path,
    remote: &str,
    status: &mut JobStatus,
    notify: &(impl Fn(JobStatus) + Send + Sync),
) -> Result<(), RunError> {
    if !plan.review.blockers.is_empty() {
        return Err("This review contains installation blockers."
            .to_string()
            .into());
    }
    if plan.expired() {
        return Err("The installation review expired. Review the IPA again."
            .to_string()
            .into());
    }
    cancel_check(control)?;
    let target = phone(plan.device_id).await?;
    if target.raw.udid != plan.udid {
        return Err(
            "The connected device changed. Review again for the intended iPhone."
                .to_string()
                .into(),
        );
    }
    if existing(&target.provider, &plan.review.bundle_id).await? != plan.review.existing_app {
        return Err(
            "The installed app changed since review. Review again before replacing it."
                .to_string()
                .into(),
        );
    }
    let path = plan.snapshot.path().join("artifact.ipa");
    let check_path = path.clone();
    let udid = plan.udid.clone();
    let ios = target.version;
    let blockers = tokio::task::spawn_blocking(move || {
        let report = crate::inspect(&check_path, &AtomicBool::new(false), |_| {})
            .map_err(|e| e.to_string())?;
        package_blockers(&report, &check_path, &udid, &ios)
    })
    .await
    .map_err(|_| RunError::from("Recheck worker stopped.".to_string()))??;
    if !blockers.is_empty() {
        return Err("The IPA no longer passes preflight. Review again."
            .to_string()
            .into());
    }
    cancel_check(control)?;
    let mut afc = limited(AfcClient::connect(&target.provider)).await?;
    let mut proxy = limited(InstallationProxyClient::connect(&target.provider)).await?;
    let mut local = tokio::fs::File::open(&path)
        .await
        .map_err(|_| RunError::from("Cannot open the reviewed snapshot.".to_string()))?;
    if !control.transition(Stage::Transferring) {
        cancel_check(control)?;
        return Err("Invalid job transition.".to_string().into());
    }
    status.stage = Stage::Transferring;
    status.message =
        "Transferring the unchanged IPA to the iPhone. Cancellation is available.".into();
    publish(status, journal, notify)?;
    cancel_check(control)?;
    if limited(afc.get_file_info("PublicStaging")).await.is_err() {
        limited(afc.mk_dir("PublicStaging")).await?;
    }
    // Journal intent before any package bytes are written, for conservative crash recovery.
    status.cleanup_pending = true;
    publish(status, journal, notify)?;
    let mut file = limited(afc.open(remote, AfcFopenMode::WrOnly)).await?;
    let mut hash = Sha256::new();
    let mut buf = vec![0; 256 * 1024];
    let mut last = Instant::now();
    let transfer = async {
        loop {
            cancel_check(control)?;
            let n = local
                .read(&mut buf)
                .await
                .map_err(|_| RunError::from("Cannot read the reviewed snapshot.".to_string()))?;
            if n == 0 {
                break;
            }
            if status.transferred_bytes + n as u64 > status.total_bytes {
                return Err(
                    "The reviewed snapshot changed. Installation was not requested."
                        .to_string()
                        .into(),
                );
            }
            limited(file.write_entire(&buf[..n])).await?;
            hash.update(&buf[..n]);
            status.transferred_bytes += n as u64;
            if last.elapsed() > Duration::from_millis(150) {
                notify(status.clone());
                last = Instant::now();
            }
        }
        Ok::<(), RunError>(())
    }
    .await;
    let closed = limited(file.close()).await;
    transfer?;
    closed?;
    if status.transferred_bytes != status.total_bytes
        || format!("{:x}", hash.finalize()) != plan.review.sha256
    {
        return Err(
            "The reviewed snapshot changed. Installation was not requested."
                .to_string()
                .into(),
        );
    }
    cancel_check(control)?;
    // Atomically wins against cancellation, then persist the uncertain boundary before dispatch.
    if !control.transition(Stage::Installing) {
        cancel_check(control)?;
        return Err("Invalid job transition.".to_string().into());
    }
    status.stage = Stage::Installing;
    status.message =
        "iOS is installing. Cancellation is no longer available; keep the iPhone connected.".into();
    // If persistence fails before dispatch, the result is a definite local failure.
    publish(status, journal, notify).map_err(|mut e| {
        e.definite = true;
        e
    })?;
    let mut options = plist::Dictionary::new();
    options.insert(
        "CFBundleIdentifier".into(),
        plan.review.bundle_id.clone().into(),
    );
    let installing = status.clone();
    proxy
        .install_with_callback(
            remote,
            Some(plist::Value::Dictionary(options)),
            |(percent, ())| {
                let mut progress = installing.clone();
                progress.device_percent = Some(percent.min(100));
                notify(progress);
                async {}
            },
            (),
        )
        .await
        .map_err(transport_error)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_comparison_is_numeric() {
        assert!(version("18.10") > version("18.9"));
        assert_eq!(version("15"), version("15.0.0"));
        assert!(version("garbage").is_none());
    }
    #[test]
    fn membership_requires_actual_device() {
        let mut d = plist::Dictionary::new();
        d.insert(
            "ProvisionedDevices".into(),
            plist::Value::Array(vec!["synthetic-device".into()]),
        );
        assert!(authorize(&d, "synthetic-device"));
        assert!(!authorize(&d, "other-device"));
        assert!(!authorize(&plist::Dictionary::new(), "synthetic-device"));
    }
    #[test]
    fn explicit_all_devices_profile_is_recognized() {
        let mut d = plist::Dictionary::new();
        d.insert("ProvisionsAllDevices".into(), true.into());
        assert!(authorize(&d, "synthetic-device"));
    }
    #[test]
    fn raw_device_failures_are_not_exposed() {
        let e = transport_error(IdeviceError::UnknownErrorType(
            "ApplicationVerificationFailed secret-udid".into(),
        ));
        assert!(e.definite);
        assert!(!e.message.contains("secret-udid"));
        assert!(e.message.contains("provisioning"));
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;
    use cms::{
        content_info::{CmsVersion, ContentInfo},
        signed_data::{EncapsulatedContentInfo, SignedData, SignerInfos},
    };
    use der::{Any, Encode, asn1::OctetString};
    fn fixture() -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        let info=br#"<plist><dict><key>CFBundleIdentifier</key><string>test.synthetic</string><key>CFBundleExecutable</key><string>App</string><key>CFBundleSupportedPlatforms</key><array><string>iPhoneOS</string></array><key>UIDeviceFamily</key><array><integer>1</integer></array><key>MinimumOSVersion</key><string>15.0</string></dict></plist>"#;
        let profile=br#"<plist><dict><key>ExpirationDate</key><date>2099-01-01T00:00:00Z</date><key>ProvisionedDevices</key><array><string>synthetic-device</string></array></dict></plist>"#;
        let data = SignedData {
            version: CmsVersion::V1,
            digest_algorithms: Default::default(),
            encap_content_info: EncapsulatedContentInfo {
                econtent_type: "1.2.840.113549.1.7.1".parse().unwrap(),
                econtent: Some(
                    Any::encode_from(&OctetString::new(profile.to_vec()).unwrap()).unwrap(),
                ),
            },
            certificates: None,
            crls: None,
            signer_infos: SignerInfos(Default::default()),
        };
        let cms = ContentInfo {
            content_type: "1.2.840.113549.1.7.2".parse().unwrap(),
            content: Any::encode_from(&data).unwrap(),
        }
        .to_der()
        .unwrap();
        let mut exe = vec![0; 32];
        exe[0..4].copy_from_slice(&0xfeedfacf_u32.to_le_bytes());
        exe[4..8].copy_from_slice(&0x100000c_u32.to_le_bytes());
        {
            let mut z = zip::ZipWriter::new(&mut f);
            for (name, bytes) in [
                ("Payload/App.app/Info.plist", info.as_slice()),
                ("Payload/App.app/App", exe.as_slice()),
                ("Payload/App.app/embedded.mobileprovision", cms.as_slice()),
            ] {
                z.start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                z.write_all(bytes).unwrap();
            }
            z.finish().unwrap();
        }
        f
    }
    #[test]
    fn reviewed_snapshot_survives_original_changes() {
        let f = fixture();
        let before = std::fs::read(f.path()).unwrap();
        let (dir, r, hash) = snapshot(f.path()).unwrap();
        assert_eq!(std::fs::read(f.path()).unwrap(), before);
        std::fs::write(f.path(), b"changed source").unwrap();
        let snapshot = std::fs::read(dir.path().join("artifact.ipa")).unwrap();
        assert_eq!(snapshot, before);
        assert_eq!(hash, format!("{:x}", Sha256::digest(&snapshot)));
        assert!(
            package_blockers(
                &r,
                &dir.path().join("artifact.ipa"),
                "synthetic-device",
                "18.0"
            )
            .unwrap()
            .is_empty()
        );
    }
    #[test]
    fn mismatched_device_and_old_os_block_installation() {
        let f = fixture();
        let (dir, r, _) = snapshot(f.path()).unwrap();
        let blockers = package_blockers(
            &r,
            &dir.path().join("artifact.ipa"),
            "different-device",
            "14.0",
        )
        .unwrap();
        assert!(blockers.iter().any(|b| b.contains("does not authorize")));
        assert!(blockers.iter().any(|b| b.contains("minimum iOS")));
        assert!(!blockers.join(" ").contains("different-device"));
    }
    #[tokio::test]
    async fn cancelled_job_never_connects_or_transfers() {
        let f = fixture();
        let (dir, r, hash) = snapshot(f.path()).unwrap();
        let plan = PreparedInstall {
            library_artifact: None,
            library_lease: None,
            review: Review {
                token: crate::domain::identifiers::ReviewToken::new(
                    uuid::Uuid::new_v4().to_string(),
                ),
                app_name: "Synthetic".into(),
                bundle_id: "test.synthetic".into(),
                version: None,
                device_name: "Synthetic iPhone".into(),
                size_bytes: r.size_bytes,
                sha256: hash,
                existing_app: None,
                blockers: vec![],
                notes: vec![],
            },
            snapshot: dir,
            device_id: u32::MAX,
            udid: "never-connect".into(),
            created: Instant::now(),
        };
        let c = Arc::new(Control::default());
        assert!(c.cancel());
        let storage = tempfile::tempdir().unwrap();
        let result = execute(plan, c, storage.path().join("job.json"), |_| {}).await;
        assert_eq!(result.stage, Stage::Cancelled);
        assert_eq!(result.transferred_bytes, 0);
        assert!(!result.cleanup_pending);
    }
}

impl PreparedInstall {
    pub fn record_library_attempt(&self, library: &crate::library::Library) -> Result<(), String> {
        if let Some(artifact) = &self.library_artifact {
            if artifact.sha256 != self.review.sha256 {
                return Err(
                    "Reviewed bytes do not match the library artifact. Import it again.".into(),
                );
            }
            library.begin(
                artifact,
                &crate::domain::identifiers::JobId::from_review(&self.review.token),
                &self.udid,
                &self.review.device_name,
            )?;
        }
        Ok(())
    }
}
