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
/// Replace a transport error with one sentence about what to do.
///
/// The underlying error is discarded rather than formatted: it can carry pairing and address
/// detail, and none of it helps someone whose phone is locked.
fn connection_error(_: IdeviceError) -> String {
    "Cannot communicate with the selected iPhone. Unlock it, check trust and the USB cable, and review again.".into()
}
/// Verified USB iPhone identity for an in-crate caller: its UDID and display name. The UDID is
/// deliberately not part of any type that crosses the IPC boundary.
pub(crate) async fn verified_identity(id: u32) -> Result<(String, String), String> {
    let phone = phone(id).await?;
    Ok((phone.raw.udid, phone.name))
}
/// Find one attached phone and read the properties an installation needs.
///
/// Uses the existing pairing record and never creates one, so preparing a review cannot cause a
/// Trust prompt on someone's device.
///
/// # Errors
///
/// Returns an actionable sentence if the device daemon is unreachable, no attached device has that
/// number, or the phone is locked or untrusted. The transport's own error never appears.
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
/// What is already installed under this bundle identifier, if anything.
///
/// Used to tell a person they are replacing an app rather than adding one.
///
/// # Errors
///
/// Returns a message if the phone's installation service cannot be reached. `Ok(None)` means
/// nothing is installed, which is an ordinary answer rather than a failure.
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
/// Copy the chosen IPA into a private directory, inspect the copy, and fingerprint it.
///
/// The snapshot is what gets installed. Binding a review to a copy rather than to a path is what
/// makes "only the reviewed artifact may be installed" enforceable: the original can be replaced
/// afterwards and the installation is unaffected. The returned directory owns the copy and removes
/// it when dropped.
///
/// # Errors
///
/// Returns a message if the source cannot be read or copied, exceeds the size limit, or is not a
/// valid IPA.
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
/// Parse an iOS version into three numeric components, or `None` if it is not one.
///
/// Compared numerically rather than as text, because `"10.0"` sorts before `"9.0"` as a string and
/// that would refuse installs on newer phones. `None` is treated as unverified rather than as
/// satisfied.
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
/// Whether an embedded profile authorises this exact phone.
///
/// Membership of the device allowlist, or a profile that explicitly provisions all devices.
/// Anything looser would let a build install where it was never authorised to run.
fn authorize(d: &plist::Dictionary, udid: &str) -> bool {
    d.get("ProvisionsAllDevices")
        .and_then(plist::Value::as_boolean)
        == Some(true)
        || d.get("ProvisionedDevices")
            .and_then(plist::Value::as_array)
            .is_some_and(|a| a.iter().any(|v| v.as_string() == Some(udid)))
}
/// What would stop this build installing on this phone, and what is worth saying about it anyway.
///
/// Returns blockers and notes separately: a blocker refuses the installation, a note is something
/// a person should know before agreeing to one. Covers encryption, minimum OS, device family,
/// profile expiry and whether the profile authorises this device at all.
///
/// Anything unverified is treated as a blocker rather than as satisfied: the absence of evidence
/// that a build will run is not evidence that it will.
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
/// Review installing one IPA on one phone, binding a token to both.
///
/// Snapshots the bytes privately, verifies the phone, and checks everything that would stop the
/// installation. The returned plan holds the snapshot, the device's verified identity and an
/// expiry; none of that is serialised, so the binding exists only in this process's memory.
///
/// Preparing writes nothing to the phone and contacts no Apple service.
///
/// # Errors
///
/// Returns a message if the file cannot be read or inspected, or if the phone is absent, locked or
/// untrusted.
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
    /// Treat a plain message as a failure whose effect on the device is not established.
    ///
    /// The conservative default: an error that has not said whether it reached the phone is
    /// assumed not to have proved anything either way.
    fn from(message: String) -> Self {
        Self {
            message,
            definite: false,
        }
    }
}
/// Classify a device error by whether it settles what happened on the phone.
///
/// The distinction that matters: an error raised before the install command was sent means nothing
/// was installed, while one raised after means the outcome is unknown. Reporting the second as a
/// failure would tell a person the app is not there when it may well be.
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
/// Bound one device operation in time, so a phone that stops answering does not hang the install.
///
/// # Errors
///
/// Returns the operation's own failure, or a timeout classified the same way — by whether the
/// install command had already been sent.
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
/// Journal a status, then announce it.
///
/// Durable before visible, always: a person must never see a stage the journal does not have, or a
/// crash immediately afterwards would recover to an earlier state than the one they were shown.
///
/// # Errors
///
/// Returns a failure if the journal cannot be written; the installation stops rather than
/// continuing without a durable record.
fn publish(
    status: &JobStatus,
    journal: &Path,
    notify: &impl Fn(JobStatus),
) -> Result<(), RunError> {
    job::save(journal, status).map_err(RunError::from)?;
    notify(status.clone());
    Ok(())
}
/// Stop here if cancellation was requested and is still honourable.
///
/// Called between steps up to the commit boundary. After the install command is sent there is
/// nothing to check: the outcome belongs to the device.
///
/// # Errors
///
/// Returns a cancellation failure when a stop has been asked for.
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
    /// Build a review bound to nothing, for testing the rules around it.
    ///
    /// The snapshot directory is real but empty and the device identity is synthetic, so this can
    /// never be handed to the real installer and made to touch a phone. It exists so the rules an
    /// installation service enforces — expiry, token matching, acknowledgement, cancellation — can
    /// be exercised without one.
    #[cfg(test)]
    pub(crate) fn synthetic(
        token: crate::domain::identifiers::ReviewToken,
        sha256: String,
        device_id: u32,
        artifact: Option<crate::library::Artifact>,
        lease: Option<crate::library::Lease>,
    ) -> Self {
        Self {
            review: Review {
                token,
                app_name: "Synthetic".into(),
                bundle_id: "test.library".into(),
                version: Some("1.0".into()),
                device_name: "Tester phone".into(),
                size_bytes: 1,
                sha256,
                existing_app: None,
                blockers: vec![],
                notes: vec![],
            },
            library_artifact: artifact,
            library_lease: lease,
            snapshot: tempfile::tempdir().expect("a temporary snapshot directory"),
            device_id,
            udid: format!("synthetic-udid-{device_id}"),
            created: Instant::now(),
        }
    }

    /// Whether this review is too old to authorise an installation.
    ///
    /// A review binds bytes and a device that were verified at a moment in time; after ten minutes
    /// the phone may have been unplugged or the file replaced, so it must be taken again.
    pub fn expired(&self) -> bool {
        self.created.elapsed() > Duration::from_secs(600)
    }
}
/// Transfer and install the reviewed build, reporting every stage.
///
/// Returns the terminal status rather than a `Result`: the outcome *is* the answer, and `Failed`,
/// `Cancelled` and `Unknown` are each distinct facts a caller must be able to tell apart.
///
/// The whole run is bounded in time. A failure to journal the final status appends to its message
/// instead of changing it — what the device did is not altered by Orbiter's bookkeeping.
///
/// # Cancellation
///
/// Honoured up to the moment iOS is asked to install, and refused afterwards.
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
/// Do the work of one installation: connect, transfer, verify, install.
///
/// Every stage change is journalled before being announced. The transferred bytes are hashed as
/// they are written and compared against the reviewed fingerprint before the install command is
/// sent, so a snapshot that changed under the operation is caught rather than installed.
///
/// Cleanup intent is journalled *before* any package bytes reach the phone, so an interrupted run
/// is known to have possibly left a staging file behind.
///
/// # Errors
///
/// Returns a failure carrying whether the install command had already been sent, which is what
/// decides between `Failed` and `Unknown`.
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
/// Checks the comparisons and classifications a review depends on.
mod tests {
    use super::*;
    #[test]
    /// iOS versions compare numerically, so 10.0 is newer than 9.0 rather than sorting before it.
    fn version_comparison_is_numeric() {
        assert!(version("18.10") > version("18.9"));
        assert_eq!(version("15"), version("15.0.0"));
        assert!(version("garbage").is_none());
    }
    #[test]
    /// A profile authorises a phone only if that phone is actually in its allowlist; a near match
    /// is not a match.
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
    /// A profile that explicitly provisions all devices authorises any phone, without needing an
    /// allowlist entry.
    fn explicit_all_devices_profile_is_recognized() {
        let mut d = plist::Dictionary::new();
        d.insert("ProvisionsAllDevices".into(), true.into());
        assert!(authorize(&d, "synthetic-device"));
    }
    #[test]
    /// A transport error never reaches a message: the fixed, actionable sentence does, so pairing
    /// detail cannot leak into something a person pastes into an issue.
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
/// Checks review binding and cancellation against synthetic IPAs, with no device involved.
mod fixture_tests {
    use super::*;
    use cms::{
        content_info::{CmsVersion, ContentInfo},
        signed_data::{EncapsulatedContentInfo, SignedData, SignerInfos},
    };
    use der::{Any, Encode, asn1::OctetString};
    /// A minimal, valid IPA that passes inspection.
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
    /// Replacing the chosen file after a review does not change what would be installed: the
    /// review is bound to a private snapshot, not to a path.
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
    /// A build for another device family, or one needing a newer iOS than the phone runs, blocks
    /// the installation rather than being attempted and failing on the device.
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
    /// A job cancelled before it starts never connects to the phone and transfers nothing, ending
    /// as `Cancelled` with no cleanup pending — there is nothing on the device to clean up.
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
    /// Record this installation in the library's history before it begins.
    ///
    /// Re-checks that the artifact's recorded hash still matches the reviewed bytes, so a review
    /// and a library record that have drifted apart are caught here rather than producing history
    /// attributed to the wrong build.
    ///
    /// Does nothing for a plan with no library artifact, which is how a path-based install with no
    /// saved version behaves.
    ///
    /// # Errors
    ///
    /// Returns a message if the bytes no longer match, or if the library cannot record the attempt
    /// — including when this review was already used, which would mean installing twice on one
    /// authorisation.
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
