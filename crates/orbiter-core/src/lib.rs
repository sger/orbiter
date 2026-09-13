//! Reading an IPA and saying what is in it.
//!
//! Local inspection of an untrusted archive: the chosen file is opened read-only and every
//! structure inside it is treated as hostile until bounded. Nothing is extracted to disk, no
//! network is used, and no credential is touched — the one exception is app-icon conversion on
//! macOS, which is described on [`inspect`].
//!
//! # Bounds
//!
//! Archive size, entry count, per-entry size, total expanded size, compression ratio, property
//! list depth and size, and a shared read budget across the whole inspection. Each exists because
//! an IPA is a file someone else produced.
//!
//! # What this does not claim
//!
//! Nothing here verifies a signature or a certificate chain. A profile is parsed, not trusted, and
//! every finding says so.

pub mod accounts;
mod app_icon;
pub mod application;
pub mod certificates;
pub mod devices;
pub mod diagnostics;
pub mod domain;
pub mod installation;
pub mod keychain;
pub mod library;
pub mod local_anisette;
mod macho;
pub mod plan;
mod profile;
pub mod provisioning;
pub mod renewal;
pub mod signer;

use base64::Engine;
use plist::{Dictionary, Value};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use zip::ZipArchive;

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Cannot read this file. Choose a readable local IPA and try again.")]
    Io,
    #[error("Invalid or unsupported ZIP archive. Export a fresh IPA from your build system.")]
    Zip,
    #[error(
        "Unsafe archive paths, duplicate entries, symlinks, or special files. Export a standard IPA."
    )]
    UnsafeArchive,
    #[error(
        "Archive exceeds inspection limits. Use an IPA below 2 GiB with at most 50,000 entries and 8 GiB expanded data."
    )]
    Limits,
    #[error("Malformed or oversized property list. Export a fresh IPA.")]
    Plist,
    #[error("Expected exactly one main app in Payload with valid bundle metadata.")]
    MainApp,
    #[error("Malformed or unsupported Mach-O executable or signature.")]
    MachO,
    #[error("Cannot parse the embedded CMS provisioning profile.")]
    Profile,
    #[error("Inspection cancelled. The original IPA is unchanged.")]
    Cancelled,
}
#[derive(Debug, Serialize)]
pub struct Report {
    pub size_bytes: u64,
    pub main_path: String,
    pub bundles: Vec<Bundle>,
    pub findings: Vec<Finding>,
    pub icon_data_url: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct Bundle {
    pub path: String,
    pub kind: String,
    pub name: String,
    pub identifier: String,
    pub version: Option<String>,
    pub build: Option<String>,
    pub minimum_os: Option<String>,
    pub supported_platforms: Vec<String>,
    pub device_families: Vec<u64>,
    pub slices: Vec<macho::Slice>,
    pub profile: Option<profile::Profile>,
    pub issues: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct Finding {
    pub status: &'static str,
    pub title: String,
    pub detail: String,
    pub bundle: Option<String>,
}
/// Stop at this boundary if cancellation was requested.
///
/// Called between archive entries and inside read loops, so cancellation is prompt without ever
/// leaving a half-built report to be mistaken for a complete one.
///
/// # Errors
///
/// Returns [`Error::Cancelled`] when a stop has been asked for.
fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
/// Parse a property list from untrusted bytes, under strict bounds.
///
/// The event stream is bounded *before* the recursive value builder ever sees it — depth, element
/// counts, event count and total string and data size — because the builder is where a crafted
/// plist would otherwise turn into unbounded recursion or allocation. An unbalanced document is
/// rejected rather than partially accepted.
///
/// # Errors
///
/// Returns [`Error::Plist`] for anything oversized, too deep, malformed, unbalanced, or not a
/// dictionary at the top level.
pub(crate) fn parse_plist(b: &[u8]) -> Result<Dictionary> {
    if b.len() > 4 * 1024 * 1024 {
        return Err(Error::Plist);
    }
    // Bound event depth/count before the recursive Value builder sees untrusted data.
    use plist::stream::{Event, Reader};
    let mut depth = 0usize;
    let mut events = Vec::new();
    let mut bytes = 0usize;
    for event in Reader::new(Cursor::new(b)) {
        let event = event.map_err(|_| Error::Plist)?;
        match &event {
            Event::StartArray(n) | Event::StartDictionary(n) => {
                depth += 1;
                if depth > 64 || n.is_some_and(|n| n > 100_000) {
                    return Err(Error::Plist);
                }
            }
            Event::EndCollection => depth = depth.checked_sub(1).ok_or(Error::Plist)?,
            Event::String(s) => bytes += s.len(),
            Event::Data(d) => bytes += d.len(),
            _ => (),
        }
        if events.len() >= 100_000 || bytes > 8 * 1024 * 1024 {
            return Err(Error::Plist);
        }
        events.push(Ok(event));
    }
    if depth != 0 {
        return Err(Error::Plist);
    }
    Value::from_events(events)
        .map_err(|_| Error::Plist)?
        .into_dictionary()
        .ok_or(Error::Plist)
}
/// Read one string value from a property list, or `None` if it is absent or another type.
pub(crate) fn string(d: &Dictionary, k: &str) -> Option<String> {
    d.get(k).and_then(Value::as_string).map(str::to_owned)
}
/// Convert an entitlements dictionary into JSON, or an empty map when there is none.
///
/// Ordered, so two reports of the same build list entitlements in the same order and can be
/// compared by eye.
pub(crate) fn entitlements(v: Option<&Value>) -> BTreeMap<String, serde_json::Value> {
    v.and_then(Value::as_dictionary)
        .map(|d| d.iter().map(|(k, v)| (k.clone(), json_value(v))).collect())
        .unwrap_or_default()
}
/// Convert one property-list value to JSON, replacing anything non-textual with a placeholder.
///
/// Dates, binary data and other opaque values become a note rather than being rendered: an
/// entitlements listing is read by a person, and a wall of base64 in it helps nobody.
fn json_value(v: &Value) -> serde_json::Value {
    match v {
        Value::String(s) => s.clone().into(),
        Value::Boolean(b) => (*b).into(),
        Value::Array(a) => a.iter().map(json_value).collect(),
        Value::Dictionary(d) => d.iter().map(|(k, v)| (k.clone(), json_value(v))).collect(),
        Value::Integer(i) => i
            .as_signed()
            .map(serde_json::Value::from)
            .or_else(|| i.as_unsigned().map(serde_json::Value::from))
            .unwrap_or_default(),
        _ => "[non-text value omitted]".into(),
    }
}
// Reject ZIP64/multi-volume archives and bound directory allocation before ZipArchive.
/// Validate an archive's end-of-central-directory record before handing it to the ZIP reader.
///
/// Reads the tail directly to reject multi-volume and ZIP64 archives, to bound the declared entry
/// count and directory size, and to check that the directory's offset and length actually meet its
/// start. This runs first because the entry count decides how much the reader will allocate, and
/// a crafted header must not be able to choose that number.
///
/// Returns the declared entry count, which the caller compares against what the reader finds.
///
/// # Errors
///
/// Returns [`Error::Zip`] for a malformed or multi-volume archive, [`Error::Limits`] when a
/// declared count or size exceeds what inspection will attempt, and [`Error::Io`] if the tail
/// cannot be read.
fn preflight(f: &mut File, size: u64) -> Result<usize> {
    let n = size.min(65_557) as usize;
    f.seek(SeekFrom::End(-(n as i64))).map_err(|_| Error::Io)?;
    let mut tail = vec![0; n];
    f.read_exact(&mut tail).map_err(|_| Error::Io)?;
    let offset = (0..n.saturating_sub(21))
        .rev()
        .find(|&i| {
            tail[i..].starts_with(b"PK\x05\x06")
                && i + 22 + u16::from_le_bytes([tail[i + 20], tail[i + 21]]) as usize == n
        })
        .ok_or(Error::Zip)?;
    let b = &tail[offset..];
    let u16at = |i| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32at = |i| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    if u16at(4) != 0 || u16at(6) != 0 || u16at(8) != u16at(10) {
        return Err(Error::Zip);
    }
    if u16at(10) > 50_000 || u32at(12) > 32 * 1024 * 1024 || u32at(16) == u32::MAX {
        return Err(Error::Limits);
    }
    if offset >= 20 && tail[offset - 20..].starts_with(b"PK\x06\x07") {
        return Err(Error::Zip);
    }
    let end = size - n as u64 + offset as u64;
    if u32at(16) as u64 + u32at(12) as u64 != end {
        return Err(Error::Zip);
    }
    f.rewind().map_err(|_| Error::Io)?;
    Ok(u16at(10) as usize)
}
/// Whether an archive entry name is safe to reason about and to join onto a path.
///
/// Rejects absolute paths, traversal, Windows separators and drive-letter colons, control
/// characters, and components ending in a space or dot — the last because those are silently
/// normalised away on some filesystems, so two entries could become one file.
pub(crate) fn safe_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 4096
        && !n.starts_with('/')
        && !n.contains(['\\', ':', '\0'])
        && !n.chars().any(char::is_control)
        && n.trim_end_matches('/')
            .split('/')
            .all(|c| !c.is_empty() && c != "." && c != ".." && !c.ends_with([' ', '.']))
}
/// Read one archive entry into memory, bounded twice and charged against a shared budget.
///
/// The declared size is checked before decompression begins — that is the zip-bomb guard — and the
/// accumulated size is checked again as bytes arrive, because a declared size is only a claim. The
/// budget is shared across every read in one inspection, so no combination of individually legal
/// entries can add up to an unbounded total.
///
/// Cancellation is honoured between blocks.
///
/// # Errors
///
/// Returns [`Error::Limits`] when the entry or the budget would be exceeded, [`Error::Zip`] if the
/// entry is missing or corrupt, and [`Error::Cancelled`] if a stop was requested.
fn read<R: Read + Seek>(
    z: &mut ZipArchive<R>,
    name: &str,
    limit: u64,
    cancel: &AtomicBool,
    budget: &mut u64,
) -> Result<Vec<u8>> {
    check(cancel)?;
    let mut f = z.by_name(name).map_err(|_| Error::Zip)?;
    if f.size() > limit || f.size() > *budget {
        return Err(Error::Limits);
    }
    let mut bytes = Vec::new();
    let mut block = [0; 64 * 1024];
    loop {
        check(cancel)?;
        let n = f.read(&mut block).map_err(|_| Error::Zip)?;
        if n == 0 {
            break;
        }
        if bytes.len() as u64 + n as u64 > limit || n as u64 > *budget {
            return Err(Error::Limits);
        }
        *budget -= n as u64;
        bytes.extend_from_slice(&block[..n]);
    }
    Ok(bytes)
}
/// Local inspection: no network and no credential access, and the chosen IPA is only ever read.
///
/// The archive itself is read entirely in process, with nothing extracted to disk. The one
/// exception is the app icon: an Apple-optimised PNG cannot be decoded here, so on macOS
/// `app_icon` writes that single image to a temporary directory and asks the system converter to
/// re-encode it, bounded and with a timeout. Nothing else leaves this process, and an icon that
/// cannot be read is simply absent.
///
/// Progress describes completed boundaries, not a fabricated percentage.
pub fn inspect(
    path: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&'static str),
) -> Result<Report> {
    check(cancel)?;
    progress("Checking archive");
    let mut f = File::open(path).map_err(|_| Error::Io)?;
    let meta = f.metadata().map_err(|_| Error::Io)?;
    if !meta.is_file() {
        return Err(Error::Io);
    }
    let size = meta.len();
    if size > 2 * 1024 * 1024 * 1024 {
        return Err(Error::Limits);
    }
    let declared_entries = preflight(&mut f, size)?;
    let mut z = ZipArchive::new(f).map_err(|_| Error::Zip)?;
    if z.len() != declared_entries {
        return Err(Error::UnsafeArchive);
    }
    if z.len() > 50_000 {
        return Err(Error::Limits);
    }
    let mut names = BTreeSet::new();
    let mut normalized = BTreeSet::new();
    let mut total = 0u64;
    for i in 0..z.len() {
        check(cancel)?;
        let entry = z.by_index(i).map_err(|_| Error::Zip)?;
        let n = entry.name();
        let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
        if !safe_name(n)
            || !matches!(mode, 0 | 0o100000 | 0o040000)
            || !normalized.insert(n.trim_end_matches('/').to_lowercase())
        {
            return Err(Error::UnsafeArchive);
        }
        total = total.checked_add(entry.size()).ok_or(Error::Limits)?;
        if total > 8 * 1024 * 1024 * 1024
            || entry.size() > 1024 * 1024 * 1024
            || (entry.size() > 16 * 1024 * 1024
                && entry.size() / entry.compressed_size().max(1) > 1000)
        {
            return Err(Error::Limits);
        }
        names.insert(n.to_owned());
    }
    // All app/extension/framework directories must have a direct Info.plist.
    let mut roots = BTreeSet::new();
    for name in &names {
        let parts: Vec<_> = name.trim_end_matches('/').split('/').collect();
        for i in 1..parts.len() {
            if parts[i].ends_with(".app")
                || parts[i].ends_with(".appex")
                || parts[i].ends_with(".framework")
            {
                roots.insert(parts[..=i].join("/"));
            }
        }
    }
    let mains: Vec<_> = roots
        .iter()
        .filter(|p| p.starts_with("Payload/") && p.ends_with(".app") && p.split('/').count() == 2)
        .cloned()
        .collect();
    if mains.len() != 1 {
        return Err(Error::MainApp);
    }
    let main = mains[0].clone();
    if roots.len() > 512 {
        return Err(Error::Limits);
    }
    let mut bundles = Vec::new();
    let mut budget = 1024 * 1024 * 1024;
    let mut main_info = None;
    progress("Reading bundles and signatures");
    for root in roots {
        check(cancel)?;
        let info_name = format!("{root}/Info.plist");
        if !names.contains(&info_name) {
            return Err(Error::Plist);
        }
        let d = parse_plist(&read(
            &mut z,
            &info_name,
            4 * 1024 * 1024,
            cancel,
            &mut budget,
        )?)?;
        let id = string(&d, "CFBundleIdentifier")
            .filter(|s| !s.is_empty())
            .ok_or(Error::Plist)?;
        let name = string(&d, "CFBundleDisplayName")
            .or_else(|| string(&d, "CFBundleName"))
            .unwrap_or_else(|| id.clone());
        let mut b = Bundle {
            path: root.clone(),
            kind: if root == main {
                "Main app"
            } else if root.ends_with(".framework") {
                "Framework"
            } else if root.ends_with(".appex") {
                "Extension"
            } else if root.contains("/Watch/") {
                "Watch app"
            } else {
                "Nested app"
            }
            .into(),
            name,
            identifier: id,
            version: string(&d, "CFBundleShortVersionString"),
            build: string(&d, "CFBundleVersion"),
            minimum_os: string(&d, "MinimumOSVersion"),
            supported_platforms: d
                .get("CFBundleSupportedPlatforms")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_string)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            device_families: d
                .get("UIDeviceFamily")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_unsigned_integer).collect())
                .unwrap_or_default(),
            slices: Vec::new(),
            profile: None,
            issues: Vec::new(),
        };
        if let Some(exe) = string(&d, "CFBundleExecutable") {
            if !safe_name(&exe) || exe.contains('/') {
                return Err(Error::UnsafeArchive);
            }
            match read(
                &mut z,
                &format!("{root}/{exe}"),
                512 * 1024 * 1024,
                cancel,
                &mut budget,
            )
            .and_then(|bytes| macho::inspect(&bytes))
            {
                Ok(s) => b.slices = s,
                Err(e @ (Error::Cancelled | Error::Limits)) => return Err(e),
                Err(e) => b.issues.push(e.to_string()),
            }
        } else {
            b.issues
                .push("No executable declared; code compatibility is unverified.".into());
        }
        let profile_name = format!("{root}/embedded.mobileprovision");
        if names.contains(&profile_name) {
            match profile::inspect(&read(
                &mut z,
                &profile_name,
                4 * 1024 * 1024,
                cancel,
                &mut budget,
            )?) {
                Ok(p) => b.profile = Some(p),
                Err(e) => b.issues.push(e.to_string()),
            }
        } else if b.kind != "Framework" {
            b.issues
                .push("No embedded provisioning profile; provisioning is unverified.".into());
        }
        if root == main {
            main_info = Some(d);
        }
        bundles.push(b);
    }
    progress("Assessing compatibility");
    check(cancel)?;
    let mut findings=vec![Finding {status:"not_verified",title:"Signing identity required".into(),detail:"This inspection has not been matched to a signing certificate and provisioning profiles. Device eligibility and capability preservation cannot be established from this IPA alone.".into(),bundle:None}];
    for b in &bundles {
        let mut add = |status, title: &str, detail: &str| {
            findings.push(Finding {
                status,
                title: title.into(),
                detail: detail.into(),
                bundle: Some(b.identifier.clone()),
            })
        };
        for issue in &b.issues {
            add("not_verified", "Incomplete inspection", issue);
        }
        if b.slices.iter().any(|s| s.encrypted) {
            add(
                "unsupported",
                "Encrypted executable",
                "Obtain an unencrypted company build. This inspector does not decrypt executables.",
            );
        }
        if b.slices
            .iter()
            .any(|s| s.der_entitlements_present && !s.xml_entitlements_present)
        {
            add(
                "not_verified",
                "DER-only entitlements",
                "DER entitlement decoding is not implemented. No signing plan can be approved for this bundle.",
            );
        }
        if b.profile.as_ref().and_then(|p| p.expired) == Some(true) {
            add(
                "requires_configuration",
                "Embedded profile expired",
                "A newly issued profile and matching signature are required for installation.",
            );
        }
        let mut keys = BTreeSet::new();
        for s in &b.slices {
            keys.extend(s.entitlements.keys());
        }
        if let Some(p) = &b.profile {
            keys.extend(p.entitlements.keys());
        }
        for key in keys {
            let (title, detail) = match key.as_str() {
                "aps-environment" => (
                    "Push notifications",
                    "Verify target-team authorization, the APNs environment, and backend credentials for the resulting app identity.",
                ),
                "com.apple.developer.associated-domains" => (
                    "Associated domains",
                    "Verify target-team support and website association for universal links and web credentials.",
                ),
                "com.apple.developer.in-app-payments" => (
                    "Apple Pay",
                    "Verify that the selected team is authorized for every merchant identifier. Existing merchant authorization cannot be assumed to transfer.",
                ),
                "com.apple.security.application-groups" => (
                    "App groups",
                    "Verify group ownership and shared-container access. Changing groups may affect SDK behavior and existing data.",
                ),
                "keychain-access-groups" => (
                    "Keychain access",
                    "Verify team-prefixed access groups. Changing identity can prevent access to existing sessions and credentials.",
                ),
                "application-identifier"
                | "com.apple.developer.team-identifier"
                | "get-task-allow" => continue,
                _ => (
                    key.as_str(),
                    "This entitlement requires explicit review against the target provisioning profile; support is not yet verified.",
                ),
            };
            add("not_verified", title, detail);
        }
        if b.kind == "Watch app" {
            add(
                "not_verified",
                "Watch companion",
                "Coordinate companion identifiers, provisioning, and shared data. The Watch app will not be silently removed.",
            );
        }
    }
    let icon_data_url = if let Some(d) = main_info {
        icon(&mut z, &names, &main, &d, cancel, &mut budget)?
    } else {
        None
    };
    check(cancel)?;
    Ok(Report {
        size_bytes: size,
        main_path: main,
        bundles,
        findings,
        icon_data_url,
    })
}
/// Find and decode the main app's icon, or report that there is none.
///
/// Candidates are taken from the bundle's own declarations and tried largest-last-declared first,
/// then by scale suffix. Only names inside the main bundle are considered, and only ones that pass
/// [`safe_name`].
///
/// An icon that cannot be read — oversized, unreadable, or a format this decoder does not
/// understand — moves on to the next candidate. It never fails the inspection: an icon is
/// decoration, and an IPA with a broken one must still be inspectable and importable. Asset
/// catalogs are not decoded, so an app whose icon lives only there has none here.
///
/// # Errors
///
/// Returns [`Error::Cancelled`] only. Every other failure yields `Ok(None)`.
fn icon<R: Read + Seek>(
    z: &mut ZipArchive<R>,
    names: &BTreeSet<String>,
    root: &str,
    d: &Dictionary,
    cancel: &AtomicBool,
    budget: &mut u64,
) -> Result<Option<String>> {
    let declared = d
        .get("CFBundleIcons")
        .and_then(Value::as_dictionary)
        .and_then(|d| d.get("CFBundlePrimaryIcon"))
        .and_then(Value::as_dictionary)
        .and_then(|d| d.get("CFBundleIconFiles"))
        .or_else(|| d.get("CFBundleIconFiles"));
    let candidates: Vec<_> = declared
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_string).collect())
        .unwrap_or_default();
    for c in candidates.iter().rev() {
        if !safe_name(c) || c.contains('/') {
            continue;
        }
        for suffix in ["@3x.png", "@2x.png", ".png", ""] {
            let name = format!("{root}/{c}{suffix}");
            if !names.contains(&name) {
                continue;
            }
            // An icon is decoration. A member too large to read, or one this decoder does not
            // understand, moves on to the next candidate — it does not fail the inspection and
            // take the whole import with it. Only cancellation still stops everything.
            let bytes = match read(z, &name, 2 * 1024 * 1024, cancel, budget) {
                Ok(bytes) => bytes,
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(_) => continue,
            };
            if let Some(bytes) = app_icon::normalize(bytes) {
                return Ok(Some(format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                )));
            }
        }
    }
    Ok(None)
}
