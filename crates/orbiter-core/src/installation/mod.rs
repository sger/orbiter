//! Install an unchanged, already-signed IPA after a read-only review.
pub mod job;
use crate::{
    Report,
    devices::{Transport, address, transport},
};
use idevice::{
    IdeviceError, IdeviceService,
    afc::{AfcClient, opcode::AfcFopenMode},
    installation_proxy::InstallationProxyClient,
    lockdown::LockdownClient,
    provider::{IdeviceProvider, UsbmuxdProvider},
    usbmuxd::UsbmuxdDevice,
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
/// The next safe action established by local package and device checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Direct,
    NeedsSigning,
    Blocked,
}

/// A machine-readable package issue; messages are for display only.
#[derive(Clone, Debug, Serialize)]
pub struct InstallIssue {
    pub code: &'static str,
    pub message: String,
    pub signing_may_resolve: bool,
}

impl InstallIssue {
    /// Construct an issue without inferring its classification from prose.
    fn new(code: &'static str, message: impl Into<String>, signing_may_resolve: bool) -> Self {
        Self {
            code,
            message: message.into(),
            signing_may_resolve,
        }
    }
}

/// Classify checks conservatively: any non-signing issue blocks the guided path.
pub fn readiness(issues: &[InstallIssue]) -> Readiness {
    if issues.is_empty() {
        Readiness::Direct
    } else if issues.iter().all(|issue| issue.signing_may_resolve) {
        Readiness::NeedsSigning
    } else {
        Readiness::Blocked
    }
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
    pub issues: Vec<InstallIssue>,
    pub readiness: Readiness,
    pub notes: Vec<String>,
}
/// Sensitive binding stays only in Rust memory; no Debug or serialization implementation.
pub struct PreparedInstall {
    pub review: Review,
    pub library_artifact: Option<crate::library::Artifact>,
    pub library_lease: Option<crate::library::Lease>,
    snapshot: tempfile::TempDir,
    /// How the phone was reached when the review was made. Kept so a move between the cable and
    /// Wi-Fi can be mentioned rather than silently changing how long the transfer takes.
    connection: Transport,
    udid: String,
    created: Instant,
}
struct Phone {
    raw: UsbmuxdDevice,
    name: String,
    version: String,
    provider: UsbmuxdProvider,
    /// How this Mac is reaching the phone right now. Not fixed for the life of an operation: a
    /// cable can be pulled while the phone stays on Wi-Fi, and the work carries on over Wi-Fi.
    connection: Transport,
}
/// How long resolving one phone may take, covering several lockdown round-trips plus a TLS
/// handshake. It is a ceiling rather than a delay, so allowing for Wi-Fi costs a cable nothing —
/// and the previous ten seconds, sized for a cable, is where a Wi-Fi phone would first give up.
const RESOLVE_DEADLINE: Duration = Duration::from_secs(25);
/// How to find a phone among the transports the device daemon reports.
///
/// The distinction matters because the two are not equally durable. A transport number belongs to
/// one connection and changes when a phone moves between the cable and Wi-Fi; the phone's own
/// identity does not. So the number is used only for first contact — it is all the window has for
/// a phone nobody has identified yet — and everything afterwards looks the phone up by identity,
/// which is what makes an operation survive the cable being pulled.
enum Locate<'a> {
    /// The transport number the window offered.
    Number(u32),
    /// The phone itself, by the identifier read from it during the review.
    Identity(&'a str),
}
/// Replace a transport error with one sentence about what to do.
///
/// The underlying error is discarded rather than formatted: it can carry pairing and address
/// detail, and none of it helps someone whose phone is locked.
fn connection_error(_: IdeviceError) -> String {
    "Cannot communicate with the selected iPhone. Unlock it, check trust and the USB cable, and review again.".into()
}
/// Verified iPhone identity for an in-crate caller: its UDID and display name. The UDID is
/// deliberately not part of any type that crosses the IPC boundary.
pub(crate) async fn verified_identity(id: u32) -> Result<(String, String), String> {
    let phone = phone(Locate::Number(id)).await?;
    Ok((phone.raw.udid, phone.name))
}
/// Confirm the phone a review was prepared for is still reachable, by either transport.
///
/// # Errors
///
/// Returns an actionable sentence if it cannot be reached, is locked, or is untrusted.
pub(crate) async fn confirm_identity(udid: &str) -> Result<(), String> {
    phone(Locate::Identity(udid)).await.map(|_| ())
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
async fn phone(locate: Locate<'_>) -> Result<Phone, String> {
    timeout(RESOLVE_DEADLINE, async {
        let mut mux = address().connect(0).await.map_err(connection_error)?;
        let mut matches: Vec<UsbmuxdDevice> = mux
            .get_devices()
            .await
            .map_err(connection_error)?
            .into_iter()
            .filter(|d| match locate {
                Locate::Number(id) => d.device_id == id,
                Locate::Identity(udid) => d.udid == udid,
            })
            .collect();
        // One phone can answer on two transports. Prefer the cable, as discovery does, so an
        // operation does not quietly move to the slower connection while both are available.
        matches.sort_by_key(|d| transport(&d.connection_type).rank());
        let raw = matches.into_iter().next().ok_or(match locate {
            Locate::Number(_) => "Selected iPhone disconnected. Select it again.",
            Locate::Identity(_) => {
                "The iPhone this was reviewed for is no longer reachable, over either a cable or Wi-Fi. Reconnect it and review again."
            }
        })?;
        let connection = transport(&raw.connection_type);
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
            connection,
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
fn package_issues(
    report: &Report,
    path: &Path,
    udid: &str,
    ios: &str,
) -> Result<Vec<InstallIssue>, String> {
    let main = report
        .bundles
        .iter()
        .find(|b| b.path == report.main_path)
        .ok_or("Main app was not found.")?;
    let mut blockers = Vec::new();
    if !main.supported_platforms.iter().any(|p| p == "iPhoneOS") {
        blockers.push(InstallIssue::new(
            "platform",
            "The main bundle does not declare iPhoneOS support.",
            false,
        ))
    }
    if !main.device_families.contains(&1) {
        blockers.push(InstallIssue::new(
            "device_family",
            "The main bundle does not declare iPhone support.",
            false,
        ))
    }
    if !main
        .slices
        .iter()
        .any(|s| s.architecture == "arm64" || s.architecture == "arm64e")
    {
        blockers.push(InstallIssue::new(
            "architecture",
            "No supported arm64 iPhone executable was found.",
            false,
        ))
    }
    match (main.minimum_os.as_deref().and_then(version), version(ios)) {
        (Some(min), Some(current)) if current >= min => (),
        _ => blockers.push(InstallIssue::new(
            "minimum_os",
            "The iPhone does not meet a verified minimum iOS requirement.",
            false,
        )),
    }
    let file = std::fs::File::open(path).map_err(|_| "Cannot read snapshot.")?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| "Cannot read snapshot archive.")?;
    for b in &report.bundles {
        if !b.issues.is_empty() || b.slices.is_empty() {
            blockers.push(InstallIssue::new(
                "inspection",
                format!("{}: bundle inspection is incomplete.", b.identifier),
                false,
            ));
        }
        if b.slices.iter().any(|s| s.encrypted) {
            blockers.push(InstallIssue::new(
                "encryption",
                format!(
                    "{}: encrypted executable; obtain an unencrypted development or Ad Hoc build.",
                    b.identifier
                ),
                false,
            ));
        }
        if b.kind == "Framework" {
            continue;
        }
        let Some(p) = &b.profile else {
            blockers.push(InstallIssue::new(
                "profile_missing",
                format!("{}: missing provisioning profile.", b.identifier),
                true,
            ));
            continue;
        };
        if p.expired != Some(false) {
            blockers.push(InstallIssue::new(
                "profile_expiry",
                format!(
                    "{}: profile expired or expiration is unknown.",
                    b.identifier
                ),
                true,
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
            blockers.push(InstallIssue::new("profile_device", format!("{}: the embedded profile does not authorize this iPhone. Signing must obtain a profile for this device.", b.identifier), true));
        }
    }
    Ok(blockers)
}
/// Keep the execution gate and older callers on the same checks as the guided review.
/// Returns display messages, or an inspection/read failure; performs no mutations.
fn package_blockers(
    report: &Report,
    path: &Path,
    udid: &str,
    ios: &str,
) -> Result<Vec<String>, String> {
    Ok(package_issues(report, path, udid, ios)?
        .into_iter()
        .map(|issue| issue.message)
        .collect())
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
    let phone = phone(Locate::Number(device_id)).await?;
    let (dir, report, hash) = tokio::task::spawn_blocking(move || snapshot(&path))
        .await
        .map_err(|_| "Snapshot worker stopped.")??;
    let udid = phone.raw.udid.clone();
    let ios = phone.version.clone();
    let snap_path = dir.path().join("artifact.ipa");
    let (report, issues) = tokio::task::spawn_blocking(move || {
        let b = package_issues(&report, &snap_path, &udid, &ios)?;
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
            readiness: readiness(&issues),
            blockers: issues.iter().map(|issue| issue.message.clone()).collect(),
            issues,
            notes,
        },
        snapshot: dir,
        connection: phone.connection,
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
/// How long one device operation may take over a cable, where a 256 KiB write is milliseconds.
const USB_OPERATION: Duration = Duration::from_secs(15);
/// The same, over Wi-Fi. A write that would be instant on a cable can sit behind other traffic on
/// a busy network, and the cable's ceiling would abort a transfer that was merely slow.
const NETWORK_OPERATION: Duration = Duration::from_secs(60);
/// How long iOS may install without reporting any progress before Orbiter stops waiting.
///
/// This number is a judgement, not a measurement. iOS reports progress every few seconds while it
/// is working, so three minutes of silence means something has gone wrong rather than that the app
/// is large. It is deliberately generous, because giving up early on a working install and
/// reporting an unknown outcome is worse than waiting a little longer.
const INSTALL_SILENCE: Duration = Duration::from_secs(180);
/// How often that silence is checked. Short enough to notice promptly, long enough to be free.
const SILENCE_CHECK: Duration = Duration::from_secs(10);
/// The backstop for a whole job, whatever the transport.
///
/// One value rather than one per transport, because the transport can change mid-job: a phone
/// reviewed on a cable and then unplugged would otherwise inherit a cable-sized budget for a
/// Wi-Fi transfer. The per-operation ceilings and the silence watchdog do the real work; this only
/// stops a job that has gone wrong in a way neither of them noticed from running forever.
const JOB_DEADLINE: Duration = Duration::from_secs(45 * 60);

/// How long one device operation may take, for the connection it is running over.
fn operation_deadline(connection: Transport) -> Duration {
    match connection {
        Transport::Usb => USB_OPERATION,
        // An unmodelled transport is given the same room as Wi-Fi: it may well be a slow one, and
        // aborting early would be guessing against it.
        Transport::Network | Transport::Unknown => NETWORK_OPERATION,
    }
}
/// What to say when iOS stops reporting progress during an installation.
///
/// Never phrased as a failure. The install command reached the phone, so the app may be installed;
/// what is lost is Orbiter's ability to watch it, which is a different thing to report.
fn silent_install(connection: Transport) -> &'static str {
    match connection {
        Transport::Network => {
            "iOS stopped reporting progress for three minutes. The Wi-Fi connection to the iPhone was probably lost."
        }
        _ => "iOS stopped reporting progress for three minutes.",
    }
}
/// What to say when a phone stops answering, for the connection it stopped answering over.
///
/// Sending someone to check a cable that is not plugged in is sending them to fix the wrong thing.
fn unresponsive(connection: Transport) -> &'static str {
    match connection {
        Transport::Usb => {
            "The iPhone stopped responding. Check the cable and device before retrying."
        }
        Transport::Network => {
            "The iPhone stopped responding over Wi-Fi. Check that it is awake, unlocked, and on the same network as this Mac before retrying."
        }
        Transport::Unknown => {
            "The iPhone stopped responding. Check that it is awake and still connected before retrying."
        }
    }
}
/// Bound one device operation in time, so a phone that stops answering does not hang the install.
///
/// # Errors
///
/// Returns the operation's own failure, or a timeout classified the same way — by whether the
/// install command had already been sent.
async fn limited<T>(
    connection: Transport,
    future: impl std::future::Future<Output = Result<T, IdeviceError>>,
) -> Result<T, RunError> {
    timeout(operation_deadline(connection), future)
        .await
        .map_err(|_| RunError::from(unresponsive(connection).to_string()))?
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
                issues: vec![],
                readiness: Readiness::Direct,
                notes: vec![],
            },
            library_artifact: artifact,
            library_lease: lease,
            snapshot: tempfile::tempdir().expect("a temporary snapshot directory"),
            connection: Transport::Usb,
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
            JOB_DEADLINE,
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
            // Also by identity. Resolving by number meant cleanup always failed after a phone
            // moved between the cable and Wi-Fi, leaving the staging copy behind for no reason.
            let target = phone(Locate::Identity(&plan.udid)).await?;
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
/// What to say as a transfer starts, given how the phone was reached at review and how it is
/// being reached now.
///
/// A phone found by identity is the same phone however it is connected, so a changed connection
/// is not a reason to refuse. It is a reason to *say so*: moving to Wi-Fi changes how long the
/// transfer takes, and someone watching a progress bar that has slowed down deserves to know it
/// is the connection rather than the phone.
fn transfer_message(reviewed: Transport, now: Transport) -> &'static str {
    if reviewed == now {
        return "Transferring the unchanged IPA to the iPhone. Cancellation is available.";
    }
    match now {
        Transport::Network => {
            "The iPhone is now on Wi-Fi rather than a cable. Transferring the unchanged IPA, which will take longer. Cancellation is available."
        }
        Transport::Usb => {
            "The iPhone is now on a cable rather than Wi-Fi. Transferring the unchanged IPA. Cancellation is available."
        }
        Transport::Unknown => {
            "The iPhone is reachable a different way than when it was reviewed. Transferring the unchanged IPA. Cancellation is available."
        }
    }
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
    // Located by identity, not by the number the review was prepared with: unplugging the cable
    // changes that number, and a review belongs to a phone rather than to a connection. The
    // identity is the same thing the old equality check proved, now enforced by the lookup.
    let target = phone(Locate::Identity(&plan.udid)).await?;
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
    let mut afc = limited(target.connection, AfcClient::connect(&target.provider)).await?;
    let mut proxy = limited(
        target.connection,
        InstallationProxyClient::connect(&target.provider),
    )
    .await?;
    let mut local = tokio::fs::File::open(&path)
        .await
        .map_err(|_| RunError::from("Cannot open the reviewed snapshot.".to_string()))?;
    if !control.transition(Stage::Transferring) {
        cancel_check(control)?;
        return Err("Invalid job transition.".to_string().into());
    }
    status.stage = Stage::Transferring;
    status.message = transfer_message(plan.connection, target.connection).into();
    publish(status, journal, notify)?;
    cancel_check(control)?;
    if limited(target.connection, afc.get_file_info("PublicStaging"))
        .await
        .is_err()
    {
        limited(target.connection, afc.mk_dir("PublicStaging")).await?;
    }
    // Journal intent before any package bytes are written, for conservative crash recovery.
    status.cleanup_pending = true;
    publish(status, journal, notify)?;
    let mut file = limited(target.connection, afc.open(remote, AfcFopenMode::WrOnly)).await?;
    // Copied out so the transfer block does not hold a borrow of the phone for its whole run.
    let connection = target.connection;
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
            limited(connection, file.write_entire(&buf[..n])).await?;
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
    let closed = limited(target.connection, file.close()).await;
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
    // iOS reports progress as it works. That reporting is the only sign the phone is still there:
    // this call has no deadline of its own, so without a watchdog a connection that dies here
    // leaves the window saying "iOS is installing" until the whole job times out. Over a cable
    // that is nearly unreachable; over Wi-Fi it is a walk out of range.
    let heard = std::sync::Mutex::new(Instant::now());
    let install = proxy.install_with_callback(
        remote,
        Some(plist::Value::Dictionary(options)),
        |(percent, ())| {
            if let Ok(mut at) = heard.lock() {
                *at = Instant::now();
            }
            let mut progress = installing.clone();
            progress.device_percent = Some(percent.min(100));
            notify(progress);
            async {}
        },
        (),
    );
    tokio::pin!(install);
    loop {
        tokio::select! {
            result = &mut install => return result.map(|_| ()).map_err(transport_error),
            () = tokio::time::sleep(SILENCE_CHECK) => {
                // A poisoned lock means a panicking callback, not a silent phone; treating that
                // as "still working" would hang, so it counts as silence.
                let quiet = heard.lock().map_or(INSTALL_SILENCE, |at| at.elapsed());
                if quiet >= INSTALL_SILENCE {
                    // Deliberately not `definite`: the install command reached iOS, so it may
                    // well have finished. Saying it failed would tell someone the app is not
                    // there when it may be sitting on their Home Screen.
                    return Err(RunError::from(silent_install(connection).to_string()));
                }
            }
        }
    }
}
#[cfg(test)]
/// Checks the comparisons and classifications a review depends on.
mod tests {
    use super::*;
    #[test]
    /// A device operation gets the time its connection needs. Fifteen seconds is right for a
    /// cable, where a 256 KiB write is milliseconds, and would abort a Wi-Fi transfer that was
    /// only slow.
    fn an_operation_is_given_the_time_its_connection_needs() {
        assert_eq!(operation_deadline(Transport::Usb), USB_OPERATION);
        assert_eq!(operation_deadline(Transport::Network), NETWORK_OPERATION);
        // An unmodelled transport is given room rather than judged against a cable.
        assert_eq!(operation_deadline(Transport::Unknown), NETWORK_OPERATION);
        assert!(NETWORK_OPERATION > USB_OPERATION);
        // The backstop must outlast any single operation by a wide margin, or it would be the
        // thing that fires and the specific message would never be seen.
        assert!(JOB_DEADLINE > NETWORK_OPERATION * 10);
    }

    #[test]
    /// Nobody is told to check a cable that is not plugged in.
    fn a_phone_on_wifi_is_not_told_to_check_its_cable() {
        assert!(unresponsive(Transport::Usb).contains("cable"));
        let wifi = unresponsive(Transport::Network);
        assert!(!wifi.contains("cable"));
        assert!(wifi.contains("network"));
        assert!(!unresponsive(Transport::Unknown).contains("cable"));
    }

    #[test]
    /// iOS going quiet mid-install is never reported as a failure, over any connection.
    ///
    /// The install command reached the phone, so the app may be installed. Calling that a failure
    /// would tell someone the app is not there while it sits on their Home Screen — and the whole
    /// point of the unknown outcome is that Orbiter does not guess.
    fn a_silent_install_is_never_called_a_failure() {
        for connection in [Transport::Usb, Transport::Network, Transport::Unknown] {
            let message = silent_install(connection);
            assert!(!message.contains("failed"));
            assert!(!message.contains("not installed"));
            // Carried as indefinite, which is what turns it into an unknown outcome rather than
            // a failed one where the stage is decided.
            assert!(!RunError::from(message.to_string()).definite);
        }
        assert!(silent_install(Transport::Network).contains("Wi-Fi"));
        // The wording names the wait, so the constant and the sentence must not drift apart.
        assert_eq!(INSTALL_SILENCE, Duration::from_secs(3 * 60));
        assert!(silent_install(Transport::Usb).contains("three minutes"));
        // Silence is checked often enough to be noticed well inside the wait.
        assert!(SILENCE_CHECK * 4 < INSTALL_SILENCE);
    }

    #[test]
    /// A phone reached the same way it was reviewed produces no remark; a phone that has moved
    /// between a cable and Wi-Fi is still installed to, and is said to have moved.
    ///
    /// It must never be refused: the review is bound to the phone, and the phone has not changed.
    fn a_changed_connection_is_mentioned_rather_than_refused() {
        for same in [Transport::Usb, Transport::Network, Transport::Unknown] {
            let plain = transfer_message(same, same);
            assert!(plain.starts_with("Transferring the unchanged IPA"));
            assert!(!plain.contains("now on"));
        }
        // Moving to Wi-Fi is the one that changes what a person should expect.
        let slower = transfer_message(Transport::Usb, Transport::Network);
        assert!(slower.contains("now on Wi-Fi"));
        assert!(slower.contains("take longer"));
        // And back again, without claiming it will take longer.
        let faster = transfer_message(Transport::Network, Transport::Usb);
        assert!(faster.contains("now on a cable"));
        assert!(!faster.contains("take longer"));
        // Every wording keeps the two things that are true whatever the connection.
        for message in [
            slower,
            faster,
            transfer_message(Transport::Usb, Transport::Unknown),
        ] {
            assert!(message.contains("unchanged IPA"));
            assert!(message.contains("Cancellation is available"));
        }
    }
    #[test]
    /// Profile issues may require signing, but any unsupported package issue prevents that route.
    fn guided_readiness_does_not_treat_all_blockers_as_signable() {
        assert_eq!(readiness(&[]), Readiness::Direct);
        let profile = InstallIssue::new("profile_missing", "No profile", true);
        assert_eq!(
            readiness(std::slice::from_ref(&profile)),
            Readiness::NeedsSigning
        );
        assert_eq!(
            readiness(&[profile, InstallIssue::new("encryption", "Encrypted", false)]),
            Readiness::Blocked
        );
    }

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
                issues: vec![],
                readiness: Readiness::Direct,
                notes: vec![],
            },
            snapshot: dir,
            connection: Transport::Usb,
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
