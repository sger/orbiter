//! The signer: produce a new IPA that a tester's own team can install.
//!
//! The original IPA is opened read-only and never written. Everything happens inside a temporary
//! directory that is removed when this returns, and the result is a new file the caller names.
//!
//! What it does, in order: extract the archive under the same limits inspection uses; drop the
//! bundles the reviewed plan left out; rewrite every bundle identifier, including the references
//! bundles hold to each other; install Apple's profile in each bundle that has one; sign every
//! bundle from the inside out; repackage.
//!
//! What it never does: choose entitlements. The entitlements written into each signature are the
//! ones inside Apple's own provisioning profile for that identifier — whatever Apple authorised,
//! and nothing else. A capability the plan said would be lost is lost because Apple did not grant
//! it, not because this code removed it.
use crate::{certificates::Identity, plan::Plan, provisioning::ProfileOutcome};
use apple_codesign::{SettingsScope, SigningSettings, UnifiedSigner};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// Progress boundaries, not a fabricated percentage.
pub const STAGES: [&str; 5] = [
    "Extracting the archive",
    "Rewriting identifiers",
    "Installing profiles",
    "Signing bundles",
    "Repackaging",
];

/// How far along a signing run is. Counts of real things — files, bundles — never a percentage
/// invented to look like movement.
#[derive(Clone, Copy, serde::Serialize)]
pub struct Progress {
    pub stage: &'static str,
    pub done: usize,
    pub total: usize,
}

/// A record of what the signer did, for a person checking whether it did anything at all.
///
/// It carries counts, sizes, and the identifiers already shown in the interface — never a path
/// from the person's disk, and never a device identifier.
#[derive(Default)]
struct Log {
    stage: &'static str,
    lines: Vec<String>,
}

impl Log {
    fn stage(&mut self, stage: &'static str) {
        self.stage = stage;
        self.lines.push(stage.to_string());
    }
    fn note(&mut self, line: String) {
        self.lines.push(format!("  {line}"));
    }
    /// Say which stage a failure happened in: "it did not work" is not a diagnosis.
    fn failed(&self, error: String) -> String {
        format!(
            "Signing failed while {}. {error}",
            self.stage.to_lowercase()
        )
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Signed {
    /// The new IPA. The original is untouched.
    pub path: String,
    pub identifier: String,
    /// Earliest profile expiry: when the app stops launching and must be signed again.
    pub expires: String,
    pub bundles_signed: usize,
    /// Bundles the plan left out of the build, by their original identifier.
    pub removed: Vec<String>,
    pub message: String,
    /// What each stage did, in order.
    pub log: Vec<String>,
}

/// Reasons signing is refused before the archive is opened.
pub fn refusal(plan: &Plan, profiles: &[ProfileOutcome], has_identity: bool) -> Option<String> {
    if !has_identity {
        return Some("Get a signing certificate before signing.".into());
    }
    if let Some(blocker) = plan.blockers.first() {
        return Some(format!("The plan cannot be signed. {blocker}"));
    }
    let missing: Vec<&str> = plan
        .bundles
        .iter()
        .filter(|bundle| bundle.consumes_app_id)
        .map(|bundle| bundle.new_identifier.as_str())
        .filter(|identifier| !profiles.iter().any(|p| p.identifier == *identifier))
        .collect();
    if let Some(first) = missing.first() {
        return Some(format!(
            "Apple has not returned a provisioning profile for {first}. Prepare identifiers and profiles again before signing."
        ));
    }
    None
}

fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Signing cancelled. The original IPA is unchanged.".into())
    } else {
        Ok(())
    }
}

/// Extract the archive, applying the same refusals inspection applies. Symlinks, special files,
/// absolute or traversing paths, and case-colliding duplicates are rejected rather than written.
fn extract(
    ipa: &Path,
    dest: &Path,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(Progress),
) -> Result<usize, String> {
    let file = fs::File::open(ipa).map_err(|_| "The IPA could not be opened.".to_string())?;
    let size = file
        .metadata()
        .map_err(|_| "The IPA could not be read.".to_string())?
        .len();
    if size > 2 * 1024 * 1024 * 1024 {
        return Err("The IPA is above the 2 GiB limit.".into());
    }
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|_| "The IPA is not a readable ZIP archive.".to_string())?;
    if archive.len() > 50_000 {
        return Err("The IPA has more entries than the 50,000 limit.".into());
    }
    let entries = archive.len();
    let mut seen = std::collections::BTreeSet::new();
    let mut written = 0u64;
    let mut files = 0usize;
    for index in 0..archive.len() {
        cancelled(cancel)?;
        let mut entry = archive
            .by_index(index)
            .map_err(|_| "The IPA could not be read.".to_string())?;
        let name = entry.name().to_string();
        let mode = entry.unix_mode();
        if !crate::safe_name(&name)
            || !matches!(mode.unwrap_or(0) & 0o170000, 0 | 0o100000 | 0o040000)
            || !seen.insert(name.trim_end_matches('/').to_lowercase())
        {
            return Err("The IPA contains unsafe paths, duplicates, or symlinks.".into());
        }
        let target = dest.join(&name);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|_| "The archive could not be extracted.")?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|_| "The archive could not be extracted.")?;
        }
        written = written.saturating_add(entry.size());
        if written > 8 * 1024 * 1024 * 1024 {
            return Err("The IPA expands beyond the 8 GiB limit.".into());
        }
        let mut out =
            fs::File::create(&target).map_err(|_| "The archive could not be extracted.")?;
        let mut block = [0u8; 64 * 1024];
        loop {
            cancelled(cancel)?;
            let read = entry
                .read(&mut block)
                .map_err(|_| "The archive could not be extracted.".to_string())?;
            if read == 0 {
                break;
            }
            out.write_all(&block[..read])
                .map_err(|_| "The archive could not be extracted.".to_string())?;
        }
        // Keep the executable bit the build system set; the signer needs it on Mach-O files.
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&target, fs::Permissions::from_mode(mode & 0o777));
        }
        files += 1;
        // Often enough to show movement, rarely enough not to flood the interface.
        if files.is_multiple_of(64) {
            progress(Progress {
                stage: STAGES[0],
                done: files,
                total: entries,
            });
        }
    }
    Ok(files)
}

/// Remove every bundle the plan left out, deepest first. This is how a Watch app is dropped: the
/// plan omitted it, so it is not in the build. Nothing is removed that the plan still lists.
fn prune(root: &Path, plan: &Plan) -> Result<Vec<String>, String> {
    let planned: std::collections::BTreeSet<&str> =
        plan.bundles.iter().map(|b| b.path.as_str()).collect();
    let mut removed = Vec::new();
    let mut directories = Vec::new();
    collect_bundle_directories(root, root, &mut directories)?;
    // Shallowest first: removing a container takes everything inside it, so a nested bundle is
    // already gone by the time its own turn comes and is not reported twice.
    directories.sort_by_key(|path| path.components().count());
    for absolute in directories {
        let relative = absolute
            .strip_prefix(root)
            .map_err(|_| "A bundle path left the working directory.")?
            .to_string_lossy()
            .to_string();
        if planned.contains(relative.as_str()) || !absolute.exists() {
            continue;
        }
        removed.push(relative);
        fs::remove_dir_all(&absolute)
            .map_err(|_| "A bundle the plan left out could not be removed.".to_string())?;
    }
    Ok(removed)
}

fn collect_bundle_directories(
    root: &Path,
    dir: &Path,
    found: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|_| "The extracted archive could not be read.".to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if (name.ends_with(".app") || name.ends_with(".appex") || name.ends_with(".framework"))
            && path.join("Info.plist").is_file()
            && path != root
        {
            found.push(path.clone());
        }
        collect_bundle_directories(root, &path, found)?;
    }
    Ok(())
}

/// Whether a key's value is a bundle identifier.
///
/// Matching on the key, not on the string, is the whole point. A framework whose identifier is
/// `VirtualStadiumDataSDK` carries that same word as `CFBundleExecutable` — the name of a file on
/// disk — and as `CFBundleName`. Rewriting by value alone renamed all three, leaving the plist
/// pointing at an executable that does not exist. `NSExtensionPointIdentifier` is excluded for the
/// same reason: it names one of Apple's extension points, not a bundle here.
fn identifier_key(key: &str) -> bool {
    key == "CFBundleIdentifier" || key.ends_with("BundleIdentifier")
}

/// Replace old identifiers with new ones under identifier-bearing keys, at any depth.
///
/// That covers `CFBundleIdentifier` and every cross-reference a bundle holds to another —
/// `WKCompanionAppBundleIdentifier` on a Watch app, `WKAppBundleIdentifier` on a companion,
/// `NSExtension` attributes on an extension — and nothing else. Matches are whole strings, so an
/// identifier that merely begins with another is left alone.
fn rewrite(identifier_valued: bool, value: &mut plist::Value, map: &BTreeMap<String, String>) {
    match value {
        plist::Value::String(text) if identifier_valued => {
            if let Some(new) = map.get(text.as_str()) {
                *text = new.clone();
            }
        }
        plist::Value::Array(items) => items
            .iter_mut()
            .for_each(|item| rewrite(identifier_valued, item, map)),
        plist::Value::Dictionary(dictionary) => dictionary
            .iter_mut()
            .for_each(|(key, item)| rewrite(identifier_key(key), item, map)),
        _ => (),
    }
}

fn rewrite_info(
    path: &Path,
    identifier: &str,
    map: &BTreeMap<String, String>,
) -> Result<(), String> {
    let info = path.join("Info.plist");
    let bytes = fs::read(&info).map_err(|_| "A bundle has no readable Info.plist.".to_string())?;
    let mut value = plist::Value::from_reader(std::io::Cursor::new(&bytes))
        .map_err(|_| "A bundle's Info.plist could not be read.".to_string())?;
    rewrite(false, &mut value, map);
    if let Some(dictionary) = value.as_dictionary_mut() {
        // The bundle's own identifier is set outright: a build whose Info.plist disagreed with the
        // plan would be signed for an identifier it does not claim, and iOS would refuse it.
        dictionary.insert(
            "CFBundleIdentifier".into(),
            plist::Value::String(identifier.to_string()),
        );
    }
    let mut out = Vec::new();
    plist::to_writer_binary(&mut std::io::Cursor::new(&mut out), &value)
        .map_err(|_| "A bundle's Info.plist could not be written.".to_string())?;
    fs::write(&info, out).map_err(|_| "A bundle's Info.plist could not be written.".to_string())?;
    Ok(())
}

/// Build the signing settings for one bundle: this Mac's key, Apple's certificate, and the
/// entitlements out of Apple's profile for this identifier.
fn settings<'a>(
    identity: &'a SigningIdentity,
    entitlements: Option<&plist::Dictionary>,
) -> Result<SigningSettings<'a>, String> {
    use apple_codesign::cryptography::PrivateKey;
    let mut settings = SigningSettings::default();
    settings.set_signing_key(
        identity.key.as_key_info_signer(),
        identity.certificate.clone(),
    );
    settings.chain_apple_certificates();
    settings.set_team_id_from_signing_certificate();
    settings.set_for_notarization(false);
    // Each bundle is signed on its own, inside out, so nested bundles are never re-signed with
    // the wrong bundle's entitlements.
    settings.set_shallow(true);
    if let Some(entitlements) = entitlements {
        let mut xml = Vec::new();
        plist::to_writer_xml(&mut std::io::Cursor::new(&mut xml), entitlements)
            .map_err(|_| "The profile's entitlements could not be encoded.".to_string())?;
        let xml = String::from_utf8(xml)
            .map_err(|_| "The profile's entitlements are not valid text.".to_string())?;
        settings
            .set_entitlements_xml(SettingsScope::Main, xml)
            .map_err(|_| "The profile's entitlements were rejected.".to_string())?;
    }
    Ok(settings)
}

/// The key and certificate in the form the signer needs them.
pub struct SigningIdentity {
    key: apple_codesign::cryptography::InMemoryPrivateKey,
    certificate: x509_certificate::CapturedX509Certificate,
}

impl SigningIdentity {
    pub fn new(identity: &Identity) -> Result<Self, String> {
        use apple_codesign::cryptography::InMemoryPrivateKey;
        let encoded = crate::certificates::encode_key(&identity.key)?;
        let key = InMemoryPrivateKey::from_pkcs8_der(encoded.as_slice())
            .map_err(|_| "The signing key could not be prepared for signing.".to_string())?;
        let certificate =
            x509_certificate::CapturedX509Certificate::from_der(identity.certificate.clone())
                .map_err(|_| "Apple's certificate could not be read.".to_string())?;
        Ok(Self { key, certificate })
    }
}

/// Entitlements Apple authorised for one identifier, read out of its profile.
fn entitlements(profile: &ProfileOutcome) -> Result<plist::Dictionary, String> {
    let dictionary = crate::profile::dictionary(&profile.encoded).map_err(|_| {
        format!(
            "Apple's profile for {} could not be read.",
            profile.identifier
        )
    })?;
    dictionary
        .get("Entitlements")
        .and_then(plist::Value::as_dictionary)
        .cloned()
        .ok_or_else(|| {
            format!(
                "Apple's profile for {} carries no entitlements.",
                profile.identifier
            )
        })
}

/// Sign the IPA. Returns the new file; the original is never opened for writing.
pub fn sign(
    ipa: &Path,
    out_dir: &Path,
    plan: &Plan,
    profiles: &[ProfileOutcome],
    identity: &Identity,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<Signed, String> {
    let mut log = Log::default();
    match run(
        ipa,
        out_dir,
        plan,
        profiles,
        identity,
        cancel,
        &mut progress,
        &mut log,
    ) {
        Ok(mut signed) => {
            signed.log = log.lines;
            Ok(signed)
        }
        Err(error) => Err(log.failed(error)),
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    ipa: &Path,
    out_dir: &Path,
    plan: &Plan,
    profiles: &[ProfileOutcome],
    identity: &Identity,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(Progress),
    log: &mut Log,
) -> Result<Signed, String> {
    log.stage("Checking the plan");
    if let Some(refusal) = refusal(plan, profiles, true) {
        return Err(refusal);
    }
    let identity = SigningIdentity::new(identity)?;
    fs::create_dir_all(out_dir)
        .map_err(|_| "The output directory could not be created.".to_string())?;
    let work = tempfile::Builder::new()
        .prefix("orbiter-sign-")
        .tempdir_in(out_dir)
        .map_err(|_| "A working directory could not be created.".to_string())?;
    let root = work.path();

    log.stage(STAGES[0]);
    let extracted = extract(ipa, root, cancel, progress)?;
    log.note(format!(
        "{extracted} file(s), {} MB",
        fs::metadata(ipa)
            .map(|m| m.len() / 1024 / 1024)
            .unwrap_or(0)
    ));

    progress(Progress {
        stage: STAGES[1],
        done: 0,
        total: plan.bundles.len(),
    });
    log.stage(STAGES[1]);
    let removed = prune(root, plan)?;
    for path in &removed {
        log.note(format!("removed {path}"));
    }
    let map: BTreeMap<String, String> = plan
        .bundles
        .iter()
        .map(|bundle| (bundle.identifier.clone(), bundle.new_identifier.clone()))
        .collect();
    for bundle in &plan.bundles {
        cancelled(cancel)?;
        rewrite_info(&root.join(&bundle.path), &bundle.new_identifier, &map)?;
    }
    log.note(format!(
        "{} bundle(s) rewritten, main identifier now {}",
        plan.bundles.len(),
        plan.new_main_identifier
    ));

    log.stage(STAGES[2]);
    let carrying: Vec<_> = plan
        .bundles
        .iter()
        .filter(|bundle| bundle.consumes_app_id)
        .collect();
    for (index, bundle) in carrying.iter().enumerate() {
        progress(Progress {
            stage: STAGES[2],
            done: index,
            total: carrying.len(),
        });
        let profile = profiles
            .iter()
            .find(|profile| profile.identifier == bundle.new_identifier)
            .ok_or_else(|| format!("No profile was prepared for {}.", bundle.new_identifier))?;
        fs::write(
            root.join(&bundle.path).join("embedded.mobileprovision"),
            &profile.encoded,
        )
        .map_err(|_| "A provisioning profile could not be installed in the build.".to_string())?;
        log.note(format!(
            "{} expires {}",
            bundle.new_identifier, profile.expires
        ));
    }

    log.stage(STAGES[3]);
    // Inside out: a container's signature seals its nested bundles, so they must be final first.
    let mut order: Vec<_> = plan.bundles.iter().collect();
    order.sort_by_key(|bundle| std::cmp::Reverse(bundle.path.split('/').count()));
    for (index, bundle) in order.iter().enumerate() {
        cancelled(cancel)?;
        progress(Progress {
            stage: STAGES[3],
            done: index,
            total: order.len(),
        });
        let profile = profiles
            .iter()
            .find(|profile| profile.identifier == bundle.new_identifier);
        let entitlements = match profile {
            Some(profile) => Some(entitlements(profile)?),
            None => None,
        };
        let signer = UnifiedSigner::new(settings(&identity, entitlements.as_ref())?);
        signer
            .sign_path_in_place(root.join(&bundle.path))
            .map_err(|error| {
                format!(
                    "{} could not be signed. {}",
                    bundle.name,
                    signing_failure(&error)
                )
            })?;
        log.note(format!(
            "signed {} as {}",
            bundle.kind.to_lowercase(),
            bundle.new_identifier
        ));
    }

    log.stage(STAGES[4]);
    let name = ipa
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| "app".into());
    let output = out_dir.join(format!("{name}-{}.ipa", plan.team_id));
    repackage(root, &output, cancel, progress)?;
    log.note(format!(
        "{} MB written",
        fs::metadata(&output)
            .map(|meta| meta.len() / 1024 / 1024)
            .unwrap_or(0)
    ));

    let expires = profiles
        .iter()
        .map(|profile| profile.expires.clone())
        .min()
        .unwrap_or_default();
    Ok(Signed {
        path: output.to_string_lossy().to_string(),
        identifier: plan.new_main_identifier.clone(),
        expires,
        bundles_signed: order.len(),
        removed,
        message: "A signed IPA was produced. The original IPA is unchanged.".into(),
        log: Vec::new(),
    })
}

/// Signing failures are this Mac's own, not a server's, but they are still not written for a
/// person to read. Keep the shape, drop the detail.
fn signing_failure(error: &apple_codesign::AppleCodesignError) -> &'static str {
    match error {
        apple_codesign::AppleCodesignError::UnrecognizedPathType => {
            "The bundle is not in a form this signer recognises."
        }
        apple_codesign::AppleCodesignError::Io(_) => {
            "A file in the extracted build could not be read or written."
        }
        _ => "The signature could not be produced.",
    }
}

/// Write the signed tree back into an IPA. Permissions are carried over from the files on disk,
/// so executables stay executable.
fn repackage(
    root: &Path,
    output: &Path,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(Progress),
) -> Result<(), String> {
    let file =
        fs::File::create(output).map_err(|_| "The signed IPA could not be created.".to_string())?;
    let mut writer = zip::ZipWriter::new(file);
    let mut files = Vec::new();
    collect_files(root, &mut files)?;
    files.sort();
    let total = files.len();
    for (index, path) in files.into_iter().enumerate() {
        cancelled(cancel)?;
        if index.is_multiple_of(64) {
            progress(Progress {
                stage: STAGES[4],
                done: index,
                total,
            });
        }
        let name = path
            .strip_prefix(root)
            .map_err(|_| "A file left the working directory.")?
            .to_string_lossy()
            .to_string();
        #[allow(unused_mut)]
        let mut options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path)
                .map_err(|_| "A signed file could not be read.".to_string())?
                .permissions()
                .mode();
            options = options.unix_permissions(mode & 0o777);
        }
        writer
            .start_file(name, options)
            .map_err(|_| "The signed IPA could not be written.".to_string())?;
        let bytes = fs::read(&path).map_err(|_| "A signed file could not be read.".to_string())?;
        writer
            .write_all(&bytes)
            .map_err(|_| "The signed IPA could not be written.".to_string())?;
    }
    writer
        .finish()
        .map_err(|_| "The signed IPA could not be closed.".to_string())?;
    Ok(())
}

fn collect_files(dir: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir)
        .map_err(|_| "The signed build could not be read.".to_string())?
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, found)?;
        } else if path.is_file() {
            found.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{BundlePlan, TeamKind};

    fn bundle(path: &str, identifier: &str, new_identifier: &str, app_id: bool) -> BundlePlan {
        BundlePlan {
            path: path.into(),
            kind: "Main app".into(),
            name: "App".into(),
            identifier: identifier.into(),
            new_identifier: new_identifier.into(),
            consumes_app_id: app_id,
            capabilities: Vec::new(),
        }
    }

    fn plan(bundles: Vec<BundlePlan>, blockers: Vec<String>) -> Plan {
        Plan {
            team_id: "ABCDE12345".into(),
            team_kind: TeamKind::Personal,
            main_identifier: "com.company.app".into(),
            new_main_identifier: "com.company.app.abcd1234".into(),
            app_ids_required: bundles.iter().filter(|b| b.consumes_app_id).count(),
            bundles,
            blockers,
            consequences: Vec::new(),
        }
    }

    fn profile(identifier: &str) -> ProfileOutcome {
        ProfileOutcome {
            identifier: identifier.into(),
            expires: "2026-09-19T00:00:00Z".into(),
            uuid: "uuid".into(),
            encoded: Vec::new(),
        }
    }

    #[test]
    fn signing_is_refused_without_a_certificate_a_signable_plan_or_every_profile() {
        let one = plan(
            vec![bundle(
                "Payload/App.app",
                "com.company.app",
                "com.company.app.abcd1234",
                true,
            )],
            vec![],
        );
        assert!(
            refusal(&one, &[profile("com.company.app.abcd1234")], false)
                .is_some_and(|m| m.contains("signing certificate"))
        );
        // A plan that cannot be carried out is never signed part-way.
        let blocked = plan(vec![], vec!["An executable is encrypted.".into()]);
        assert!(refusal(&blocked, &[], true).is_some_and(|m| m.contains("encrypted")));
        // A bundle without Apple's profile cannot be signed for a device, so it is refused here
        // rather than producing an IPA that fails to install.
        assert!(refusal(&one, &[], true).is_some_and(|m| m.contains("com.company.app.abcd1234")));
        assert!(refusal(&one, &[profile("com.company.app.abcd1234")], true).is_none());
    }

    #[test]
    fn identifiers_are_replaced_whole_and_cross_references_travel_with_them() {
        let mut map = BTreeMap::new();
        map.insert(
            "com.company.app".to_string(),
            "com.company.app.ab".to_string(),
        );
        let mut value = plist::Value::Dictionary({
            let mut d = plist::Dictionary::new();
            d.insert("CFBundleIdentifier".into(), "com.company.app".into());
            // The Watch app's pointer back at the phone app has to move with it.
            d.insert(
                "WKCompanionAppBundleIdentifier".into(),
                "com.company.app".into(),
            );
            // A different identifier that merely starts with the old one is not this bundle.
            d.insert("Unrelated".into(), "com.company.apple".into());
            // A framework whose identifier is also its executable's filename: the file on disk
            // keeps its name, so this value must not move with the identifier.
            d.insert("CFBundleExecutable".into(), "com.company.app".into());
            d.insert("CFBundleName".into(), "com.company.app".into());
            // Apple's own extension point, which is not a bundle in this build.
            d.insert(
                "NSExtensionPointIdentifier".into(),
                "com.company.app".into(),
            );
            d.insert(
                "NSExtension".into(),
                plist::Value::Dictionary({
                    let mut inner = plist::Dictionary::new();
                    inner.insert("WKAppBundleIdentifier".into(), "com.company.app".into());
                    inner
                }),
            );
            d
        });
        rewrite(false, &mut value, &map);
        let dictionary = value.as_dictionary().expect("dictionary");
        assert_eq!(
            dictionary
                .get("WKCompanionAppBundleIdentifier")
                .and_then(|v| v.as_string()),
            Some("com.company.app.ab")
        );
        assert_eq!(
            dictionary.get("Unrelated").and_then(|v| v.as_string()),
            Some("com.company.apple")
        );
        for untouched in [
            "CFBundleExecutable",
            "CFBundleName",
            "NSExtensionPointIdentifier",
        ] {
            assert_eq!(
                dictionary.get(untouched).and_then(|v| v.as_string()),
                Some("com.company.app"),
                "{untouched} does not hold a bundle identifier and must not be rewritten"
            );
        }
        assert_eq!(
            dictionary
                .get("NSExtension")
                .and_then(plist::Value::as_dictionary)
                .and_then(|inner| inner.get("WKAppBundleIdentifier"))
                .and_then(plist::Value::as_string),
            Some("com.company.app.ab")
        );
    }

    fn write_bundle(root: &Path, path: &str, identifier: &str) {
        let dir = root.join(path);
        std::fs::create_dir_all(&dir).expect("bundle directory");
        let mut info = plist::Dictionary::new();
        info.insert("CFBundleIdentifier".into(), identifier.into());
        plist::to_file_binary(dir.join("Info.plist"), &info).expect("Info.plist");
    }

    #[test]
    fn bundles_the_plan_left_out_are_removed_with_everything_inside_them() {
        let root = tempfile::tempdir().expect("working directory");
        write_bundle(root.path(), "Payload/App.app", "com.company.app");
        write_bundle(
            root.path(),
            "Payload/App.app/Watch/W.app",
            "com.company.app.watch",
        );
        write_bundle(
            root.path(),
            "Payload/App.app/Watch/W.app/PlugIns/E.appex",
            "com.company.app.watch.ext",
        );
        let plan = plan(
            vec![bundle(
                "Payload/App.app",
                "com.company.app",
                "com.company.app.abcd1234",
                true,
            )],
            vec![],
        );
        let removed = prune(root.path(), &plan).expect("prune");
        // The Watch app is reported once, not once per bundle buried inside it.
        assert_eq!(removed, vec!["Payload/App.app/Watch/W.app".to_string()]);
        assert!(!root.path().join("Payload/App.app/Watch/W.app").exists());
        assert!(root.path().join("Payload/App.app/Info.plist").exists());
    }

    #[test]
    fn the_bundles_own_identifier_is_set_even_when_its_plist_disagrees() {
        let root = tempfile::tempdir().expect("working directory");
        write_bundle(root.path(), "Payload/App.app", "com.stale.identifier");
        rewrite_info(
            &root.path().join("Payload/App.app"),
            "com.company.app.abcd1234",
            &BTreeMap::new(),
        )
        .expect("rewrite");
        let value = plist::Value::from_file(root.path().join("Payload/App.app/Info.plist"))
            .expect("Info.plist");
        assert_eq!(
            value
                .as_dictionary()
                .and_then(|d| d.get("CFBundleIdentifier"))
                .and_then(plist::Value::as_string),
            Some("com.company.app.abcd1234")
        );
    }

    #[test]
    fn an_archive_with_unsafe_paths_is_refused_rather_than_written_to_disk() {
        let root = tempfile::tempdir().expect("working directory");
        let archive = root.path().join("bad.ipa");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).expect("archive"));
        writer
            .start_file("../escape.txt", zip::write::SimpleFileOptions::default())
            .expect("entry");
        writer.write_all(b"x").expect("write");
        writer.finish().expect("finish");
        let dest = root.path().join("out");
        std::fs::create_dir_all(&dest).expect("destination");
        let error =
            extract(&archive, &dest, &AtomicBool::new(false), &mut |_| {}).expect_err("refused");
        assert!(error.contains("unsafe paths"));
        assert!(!root.path().join("escape.txt").exists());
    }

    #[test]
    fn repackaging_keeps_the_executable_bit_so_ios_can_run_the_binary() {
        let root = tempfile::tempdir().expect("working directory");
        let payload = root.path().join("tree/Payload/App.app");
        std::fs::create_dir_all(&payload).expect("tree");
        std::fs::write(payload.join("App"), b"executable").expect("executable");
        std::fs::write(payload.join("Info.plist"), b"plist").expect("plist");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(payload.join("App"), std::fs::Permissions::from_mode(0o755))
                .expect("permissions");
        }
        let output = root.path().join("signed.ipa");
        repackage(
            &root.path().join("tree"),
            &output,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .expect("repackage");
        let mut archive =
            zip::ZipArchive::new(std::fs::File::open(&output).expect("open")).expect("archive");
        let names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).expect("entry").name().to_string())
            .collect();
        assert!(names.contains(&"Payload/App.app/App".to_string()));
        let entry = archive.by_name("Payload/App.app/App").expect("executable");
        assert_eq!(entry.unix_mode().map(|mode| mode & 0o111), Some(0o111));
    }
}

#[cfg(test)]
mod log_tests {
    use super::*;

    #[test]
    fn a_failure_says_which_stage_it_happened_in() {
        let mut log = Log::default();
        log.stage(STAGES[3]);
        let message = log.failed("The signature could not be produced.".into());
        assert!(message.contains("signing bundles"));
        assert!(message.contains("The signature could not be produced."));
    }
}
