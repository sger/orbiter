use orbiter_core::{
    installation::job::{JobStatus, Stage},
    library::Library,
};
use std::{fs, io::Write};
use zip::{ZipWriter, write::SimpleFileOptions};
fn fixture(content: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    let mut z = ZipWriter::new(&mut f);
    let info = br#"<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>test.library</string><key>CFBundleName</key><string>Library Test</string><key>CFBundleShortVersionString</key><string>preview-a</string><key>CFBundleVersion</key><string>build-z</string><key>CFBundleExecutable</key><string>App</string></dict></plist>"#;
    let mut macho = vec![0; 56];
    for (offset, value) in [
        (0, 0xfeedfacf_u32),
        (4, 0x100000c),
        (16, 1),
        (20, 24),
        (32, 0x2c),
        (36, 24),
    ] {
        macho[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (name, bytes) in [
        ("Payload/A.app/Info.plist", info.as_slice()),
        ("Payload/A.app/App", &macho),
        ("Payload/A.app/content", content.as_bytes()),
    ] {
        z.start_file(name, SimpleFileOptions::default()).unwrap();
        z.write_all(bytes).unwrap();
    }
    z.finish().unwrap();
    f
}
fn status(id: &str, stage: Stage) -> JobStatus {
    JobStatus {
        id: id.into(),
        stage,
        message: format!("{stage:?}"),
        transferred_bytes: 0,
        total_bytes: 0,
        device_percent: None,
        cleanup_pending: false,
    }
}
#[test]
fn deduplicates_bytes_preserves_original_and_keeps_same_version_different_content() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let first = fixture("one");
    let original = fs::read(first.path()).unwrap();
    let a = lib.import(first.path()).unwrap();
    let duplicate = lib.import(first.path()).unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(a.artifact_id, duplicate.artifact_id);
    let second = fixture("two");
    let b = lib.import(second.path()).unwrap();
    assert_eq!(a.app_id, b.app_id);
    assert_ne!(a.artifact_id, b.artifact_id);
    let restart = Library::new(dir.path().into());
    let snapshot = restart.snapshot().unwrap();
    assert_eq!(snapshot.apps.len(), 1);
    assert_eq!(snapshot.artifacts.len(), 2);
    assert_eq!(snapshot.artifacts[0].version, snapshot.artifacts[1].version);
    assert_eq!(snapshot.artifacts[0].build.as_deref(), Some("build-z"));
    assert_eq!(fs::read(first.path()).unwrap(), original);
    fs::remove_file(second.path()).unwrap();
    assert!(restart.open(&b.artifact_id).is_ok());
}
#[test]
fn verifies_artifacts_and_duplicate_import_repairs_damage() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let f = fixture("one");
    let a = lib.import(f.path()).unwrap();
    let opened = lib.open(&a.artifact_id).unwrap();
    fs::write(&opened.path, b"damage").unwrap();
    assert!(lib.open(&a.artifact_id).is_err());
    assert!(lib.import(f.path()).unwrap().duplicate);
    assert!(lib.open(&a.artifact_id).is_ok());
    fs::remove_file(opened.path).unwrap();
    assert!(lib.open(&a.artifact_id).is_err());
}
#[test]
fn leases_prevent_deletion_and_history_survives_version_removal() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let f = fixture("one");
    let imported = lib.import(f.path()).unwrap();
    let (a, path, lease) = lib.pin(&imported.artifact_id).unwrap();
    lib.begin(&a, "attempt", "verified-udid", "My phone")
        .unwrap();
    lib.update(&status("attempt", Stage::Failed)).unwrap();
    assert!(lib.remove(&a.app_id, Some(&a.id)).is_err());
    assert!(lib.remove(&a.app_id, None).is_err());
    assert!(path.exists());
    drop(lease);
    lib.remove(&a.app_id, Some(&a.id)).unwrap();
    assert!(!path.exists());
    let snapshot = lib.snapshot().unwrap();
    assert_eq!(snapshot.attempts.len(), 1);
    assert!(snapshot.artifacts[0].deleted);
    lib.remove(&a.app_id, None).unwrap();
    let snapshot = lib.snapshot().unwrap();
    assert!(snapshot.apps.is_empty());
    assert!(snapshot.attempts.is_empty());
    assert!(f.path().exists());
}
#[test]
fn separate_devices_variants_and_exact_outcomes_survive_restart_without_raw_identity() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let f = fixture("source");
    let imported = lib.import(f.path()).unwrap();
    let one = fixture("signed one");
    let two = fixture("signed two");
    let signed = |f: &tempfile::NamedTempFile, expires: &str| orbiter_core::signer::Signed {
        path: f.path().to_string_lossy().into_owned(),
        identifier: "test.library".into(),
        expires: expires.into(),
        expires_unix: 0,
        bundles_signed: 1,
        removed: vec![],
        message: "signed".into(),
        log: vec![],
        team_tag: Some("team".into()),
    };
    let a = lib
        .retain_signed(
            &imported.artifact_id,
            &signed(&one, "2020-01-01T00:00:00Z"),
            "one".into(),
            "keep".into(),
            "test".into(),
        )
        .unwrap();
    let b = lib
        .retain_signed(
            &imported.artifact_id,
            &signed(&two, "2099-01-01T00:00:00Z"),
            "two".into(),
            "remove".into(),
            "".into(),
        )
        .unwrap();
    lib.begin(&a, "a", "private-udid-one", "Same name").unwrap();
    lib.begin(&b, "b", "private-udid-two", "Same name").unwrap();
    lib.update(&status("unrelated", Stage::Installed)).unwrap();
    lib.update(&status("a", Stage::Installed)).unwrap();
    lib.update(&status("b", Stage::Cancelled)).unwrap();
    // Later unrelated events must not rewrite terminal outcomes.
    lib.update(&status("b", Stage::Installed)).unwrap();
    let restarted = Library::new(dir.path().into());
    let snapshot = restarted.snapshot().unwrap();
    assert_eq!(snapshot.devices.len(), 2);
    assert_eq!(snapshot.attempts[0].artifact_id, a.id);
    assert_eq!(snapshot.attempts[1].artifact_id, b.id);
    assert_ne!(
        snapshot.attempts[0].device_id,
        snapshot.attempts[1].device_id
    );
    assert_eq!(snapshot.attempts[0].stage, Stage::Installed);
    assert_eq!(snapshot.attempts[1].stage, Stage::Cancelled);
    assert_eq!(snapshot.artifacts[0].expires, None);
    assert_eq!(
        snapshot.attempts[0].expires.as_deref(),
        Some("2020-01-01T00:00:00Z")
    );
    assert_eq!(
        lib.device_tag("private-udid-one").unwrap(),
        restarted.device_tag("private-udid-one").unwrap()
    );
    let other = tempfile::tempdir().unwrap();
    assert_ne!(
        lib.device_tag("private-udid-one").unwrap(),
        Library::new(other.path().into())
            .device_tag("private-udid-one")
            .unwrap()
    );
    let raw = fs::read_to_string(dir.path().join("manifest.json")).unwrap();
    assert!(!raw.contains("private-udid"));
}
#[test]
fn interrupted_attempts_never_become_success_without_a_matching_terminal_journal() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let f = fixture("one");
    let imported = lib.import(f.path()).unwrap();
    let a = lib.open(&imported.artifact_id).unwrap().artifact;
    lib.begin(&a, "lost", "device", "Phone").unwrap();
    lib.begin(&a, "known", "device", "Renamed phone").unwrap();
    lib.recover(Some(&status("known", Stage::Installed)))
        .unwrap();
    let snapshot = lib.snapshot().unwrap();
    assert_eq!(snapshot.attempts[0].stage, Stage::Unknown);
    assert_eq!(snapshot.attempts[1].stage, Stage::Installed);
    assert_eq!(snapshot.devices.len(), 1);
    assert_eq!(snapshot.devices[0].name, "Renamed phone");
    lib.recover(None).unwrap();
    assert_eq!(lib.snapshot().unwrap().attempts[1].stage, Stage::Installed);
}
#[test]
fn invalid_imports_corrupt_manifest_and_failed_writes_do_not_reset_storage() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let f = fixture("one");
    let imported = lib.import(f.path()).unwrap();
    let invalid = tempfile::NamedTempFile::new().unwrap();
    assert!(lib.import(invalid.path()).is_err());
    assert_eq!(lib.snapshot().unwrap().artifacts.len(), 1);
    let manifest = dir.path().join("manifest.json");
    let saved = fs::read(&manifest).unwrap();
    let path = lib.open(&imported.artifact_id).unwrap().path;
    fs::write(&manifest, b"{broken").unwrap();
    assert!(lib.snapshot().is_err());
    assert!(lib.import(f.path()).is_err());
    assert!(std::path::Path::new(&path).exists());
    fs::write(&manifest, &saved).unwrap();
    // Directory permissions force the atomic publication to fail, even though reads still work.
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let result = lib.remove(&imported.app_id, Some(&imported.artifact_id));
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read(&manifest).unwrap(), saved);
    assert!(std::path::Path::new(&path).exists());
}
#[test]
fn pending_user_deletions_resume_after_crash_and_imported_bytes_are_not_automatically_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let f = fixture("one");
    let imported = lib.import(f.path()).unwrap();
    let opened = lib.open(&imported.artifact_id).unwrap();
    let manifest = dir.path().join("manifest.json");
    let mut m: serde_json::Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    m["artifacts"][0]["deleted"] = true.into();
    m["pending_removals"] = serde_json::json!([opened.artifact.sha256]);
    fs::write(&manifest, serde_json::to_vec(&m).unwrap()).unwrap();
    lib.recover(None).unwrap();
    assert!(!std::path::Path::new(&opened.path).exists());
    assert!(lib.snapshot().unwrap().artifacts[0].deleted);
    let orphan = dir.path().join("artifacts/unpublished.ipa");
    fs::write(&orphan, b"retained bytes").unwrap();
    lib.recover(None).unwrap();
    assert!(orphan.exists());
    assert_eq!(lib.snapshot().unwrap().storage_bytes, 14);
}
