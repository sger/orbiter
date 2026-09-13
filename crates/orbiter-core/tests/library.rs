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

const DAY: i64 = 86_400;
const NOW: i64 = 1_800_000_000;

fn at(unix: i64) -> std::time::SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(unix as u64)
}
/// A signed build with a real expiry, which the older fixture above deliberately leaves at zero.
fn dated(
    f: &tempfile::NamedTempFile,
    expires_unix: i64,
    team_tag: &str,
) -> orbiter_core::signer::Signed {
    orbiter_core::signer::Signed {
        path: f.path().to_string_lossy().into_owned(),
        identifier: "test.library".into(),
        expires: "2026-09-19T00:00:00Z".into(),
        expires_unix,
        bundles_signed: 1,
        removed: vec![],
        message: "signed".into(),
        log: vec![],
        team_tag: Some(team_tag.into()),
    }
}
/// Import an original, retain a signed build from it, and install that build.
fn installed(
    lib: &Library,
    content: &str,
    job: &str,
    udid: &str,
    expires_unix: i64,
) -> (String, String) {
    let source = fixture(content);
    let imported = lib.import(source.path()).unwrap();
    let build = fixture(&format!("{content} signed"));
    let artifact = lib
        .retain_signed(
            &imported.artifact_id,
            &dated(&build, expires_unix, "team-one"),
            "team-one".into(),
            "keep".into(),
            "test".into(),
        )
        .unwrap();
    lib.begin(&artifact, job, udid, "Tester phone").unwrap();
    lib.update(&status(job, Stage::Installed)).unwrap();
    (imported.artifact_id, artifact.id)
}

#[test]
fn an_import_alone_never_counts_down() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let f = fixture("one");
    let imported = lib.import(f.path()).unwrap();
    // A saved file is a fact about this Mac. Counting down from it would be Orbiter claiming an
    // installation it never performed.
    assert!(lib.snapshot_at(at(NOW)).unwrap().expiries.is_empty());
    assert!(
        lib.expiry_at(&imported.artifact_id, None, at(NOW))
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_countdown_begins_only_when_an_install_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let source = fixture("one");
    let imported = lib.import(source.path()).unwrap();
    let build = fixture("one signed");
    let artifact = lib
        .retain_signed(
            &imported.artifact_id,
            &dated(&build, NOW + 5 * DAY, "team-one"),
            "team-one".into(),
            "keep".into(),
            "test".into(),
        )
        .unwrap();
    lib.begin(&artifact, "job", "udid", "Tester phone").unwrap();
    for waiting in [Stage::Transferring, Stage::Installing] {
        lib.update(&status("job", waiting)).unwrap();
        assert!(lib.snapshot_at(at(NOW)).unwrap().expiries.is_empty());
    }
    lib.update(&status("job", Stage::Installed)).unwrap();
    let expiries = lib.snapshot_at(at(NOW)).unwrap().expiries;
    assert_eq!(expiries.len(), 1);
    assert!(expiries[0].sentence.contains("5 days"));
    assert_eq!(expiries[0].device_name, "Tester phone");
    assert!(!expiries[0].urgent);
}

#[test]
fn a_failed_or_cancelled_install_is_never_a_countdown() {
    for outcome in [Stage::Failed, Stage::Cancelled, Stage::Unknown] {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::new(dir.path().into());
        let source = fixture("one");
        let imported = lib.import(source.path()).unwrap();
        let build = fixture("one signed");
        let artifact = lib
            .retain_signed(
                &imported.artifact_id,
                &dated(&build, NOW + 5 * DAY, "team-one"),
                "team-one".into(),
                "keep".into(),
                "test".into(),
            )
            .unwrap();
        lib.begin(&artifact, "job", "udid", "Tester phone").unwrap();
        lib.update(&status("job", Stage::Transferring)).unwrap();
        lib.update(&status("job", outcome)).unwrap();
        assert!(
            lib.snapshot_at(at(NOW)).unwrap().expiries.is_empty(),
            "{outcome:?} is not an installation"
        );
    }
}

#[test]
fn the_librarys_countdown_rounds_down_like_every_other() {
    use orbiter_core::renewal::Standing;
    for (expires, expected) in [
        (NOW + 6 * DAY + DAY / 2, Standing::Valid { days: 6 }),
        (NOW + 7 * DAY, Standing::Valid { days: 7 }),
        (NOW + DAY - 1, Standing::ExpiresToday),
        (NOW, Standing::Expired { days: 0 }),
        (NOW - 2 * DAY, Standing::Expired { days: 2 }),
        (NOW - 40 * DAY, Standing::LongExpired),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::new(dir.path().into());
        installed(&lib, "one", "job", "udid", expires);
        let expiries = lib.snapshot_at(at(NOW)).unwrap().expiries;
        assert_eq!(expiries[0].standing, expected, "expiring at {expires}");
        // Only a build that has run out is news a person must act on.
        assert_eq!(
            expiries[0].urgent,
            !matches!(expected, Standing::Valid { .. })
        );
    }
}

#[test]
fn the_copy_still_working_leads_and_the_dead_one_is_still_listed() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    installed(&lib, "one", "dead", "udid-one", NOW - 2 * DAY);
    installed(&lib, "one", "live", "udid-two", NOW + 5 * DAY);
    let expiries = lib.snapshot_at(at(NOW)).unwrap().expiries;
    // A re-sign installed on one tester's phone does not revive the copy on another's, so both
    // are reported — but the headline is the copy that still launches, because announcing
    // "stopped launching" while a working install exists would be a false alarm.
    assert_eq!(expiries.len(), 2);
    assert!(expiries[0].sentence.contains("5 days"));
    assert!(!expiries[0].urgent);
    assert!(expiries[1].urgent);
}

#[test]
fn a_version_that_was_never_installed_has_no_countdown_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let (original, signed) = installed(&lib, "one", "job", "udid", NOW + 5 * DAY);
    // Opening the original that produced the installed build is exactly when its expiry matters.
    assert!(
        lib.expiry_at(&original, None, at(NOW))
            .unwrap()
            .unwrap()
            .sentence
            .contains("5 days")
    );
    assert!(
        lib.expiry_at(&signed, None, at(NOW))
            .unwrap()
            .unwrap()
            .sentence
            .contains("5 days")
    );
    // A different original of the same app was never the thing that was installed.
    let other = fixture("two");
    let stranger = lib.import(other.path()).unwrap();
    assert!(
        lib.expiry_at(&stranger.artifact_id, None, at(NOW))
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_build_signed_for_another_team_gets_no_countdown() {
    use orbiter_core::renewal::Bearing;
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let (_, signed) = installed(&lib, "one", "job", "udid", NOW + 5 * DAY);
    let mine = lib
        .expiry_at(&signed, Some("team-one"), at(NOW))
        .unwrap()
        .unwrap();
    assert_eq!(mine.bearing, Bearing::SameApp);
    assert!(mine.sentence.contains("5 days"));
    let theirs = lib
        .expiry_at(&signed, Some("team-two"), at(NOW))
        .unwrap()
        .unwrap();
    assert_eq!(theirs.bearing, Bearing::OtherTeam);
    assert!(!theirs.sentence.contains("5 days"));
    assert!(!theirs.urgent);
}

#[test]
fn an_install_whose_expiry_was_never_recorded_is_silent_not_expired() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let f = fixture("one");
    let imported = lib.import(f.path()).unwrap();
    let artifact = lib.snapshot().unwrap().artifacts.pop().unwrap();
    assert_eq!(artifact.expires_unix, None);
    lib.begin(&artifact, "job", "udid", "Tester phone").unwrap();
    lib.update(&status("job", Stage::Installed)).unwrap();
    // Unknown is not zero. Reporting "expired long ago" here would invent a fact.
    assert!(lib.snapshot_at(at(NOW)).unwrap().expiries.is_empty());
    assert!(
        lib.expiry_at(&imported.artifact_id, None, at(NOW))
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_countdown_survives_removing_the_saved_file() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let (_, signed) = installed(&lib, "one", "job", "udid", NOW + 5 * DAY);
    let app_id = lib.snapshot().unwrap().apps[0].id.clone();
    lib.remove(&app_id, Some(&signed)).unwrap();
    // History is kept when a saved file is tidied away, and so is what it said about expiry: the
    // build is still on the tester's phone.
    let expiries = lib.snapshot_at(at(NOW)).unwrap().expiries;
    assert_eq!(expiries.len(), 1);
    assert!(expiries[0].sentence.contains("5 days"));
}

#[test]
fn a_library_from_an_older_orbiter_opens_and_a_newer_one_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    installed(&lib, "one", "job", "udid", NOW + 5 * DAY);
    let path = dir.path().join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    // What a version before this change left behind: schema 1, and no epoch anywhere.
    manifest["schema"] = 1.into();
    for list in ["artifacts", "attempts"] {
        for entry in manifest[list].as_array_mut().unwrap() {
            entry.as_object_mut().unwrap().remove("expires_unix");
        }
    }
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let snapshot = Library::new(dir.path().into())
        .snapshot_at(at(NOW))
        .unwrap();
    // It opens, and says nothing it cannot know rather than guessing at a date string.
    assert_eq!(snapshot.apps.len(), 1);
    assert!(snapshot.expiries.is_empty());
    // Reading normalises; the upgrade reaches disk on the next write.
    let upgraded = Library::new(dir.path().into());
    upgraded.device_tag("udid").unwrap();
    upgraded.remove(&snapshot.apps[0].id.clone(), None).unwrap();
    let after: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(after["schema"], 2);

    // A manifest from a version that knows more must be refused, not loaded and silently
    // stripped of every field this build does not understand.
    manifest["schema"] = 3.into();
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(Library::new(dir.path().into()).snapshot().is_err());
}

/// An IPA whose main bundle declares a one-pixel PNG icon, so the icon path is exercised.
fn with_icon(content: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    let mut z = ZipWriter::new(&mut f);
    let info = br#"<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>test.library</string><key>CFBundleName</key><string>Library Test</string><key>CFBundleExecutable</key><string>App</string><key>CFBundleIconFiles</key><array><string>Icon</string></array></dict></plist>"#;
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
    let png: Vec<u8> = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=")
            .unwrap()
    };
    for (name, bytes) in [
        ("Payload/A.app/Info.plist", info.as_slice()),
        ("Payload/A.app/App", &macho),
        ("Payload/A.app/Icon.png", &png),
        ("Payload/A.app/content", content.as_bytes()),
    ] {
        z.start_file(name, SimpleFileOptions::default()).unwrap();
        z.write_all(bytes).unwrap();
    }
    z.finish().unwrap();
    f
}

#[test]
fn an_icon_is_kept_beside_the_artifacts_and_never_inside_the_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let f = with_icon("one");
    lib.import(f.path()).unwrap();

    let app = lib.snapshot().unwrap().apps.pop().unwrap();
    let sha = app.icon_sha.expect("the declared icon was read");
    // A 2 MiB icon is ~2.8 MiB of base64. Storing that inline capped the library at around
    // twenty apps and re-sent every icon on every refresh.
    let raw = fs::read_to_string(dir.path().join("manifest.json")).unwrap();
    assert!(!raw.contains("data:image/png"));
    assert!(raw.contains(&sha));
    assert!(
        dir.path()
            .join("icons")
            .join(format!("{sha}.png"))
            .is_file()
    );

    let restarted = Library::new(dir.path().into());
    assert!(
        restarted
            .icon(&sha)
            .unwrap()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    // A missing or unreadable icon is a placeholder, never an error.
    fs::remove_file(dir.path().join("icons").join(format!("{sha}.png"))).unwrap();
    assert_eq!(restarted.icon(&sha).unwrap(), None);
    assert!(restarted.icon("not-a-hash").is_err());

    // Removing the app takes its icon with it.
    let f2 = with_icon("one");
    lib.import(f2.path()).unwrap();
    let app_id = lib.snapshot().unwrap().apps[0].id.clone();
    lib.remove(&app_id, None).unwrap();
    assert!(!dir.path().join("icons").join(format!("{sha}.png")).exists());
    assert!(lib.snapshot().unwrap().storage_warning.is_none());
}

#[test]
fn unreferenced_bytes_are_named_and_only_removed_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let f = fixture("one");
    let imported = lib.import(f.path()).unwrap();
    let kept = lib.open(&imported.artifact_id).unwrap().path;

    // What an interrupted copy leaves behind: bytes under a valid name that nothing points at.
    let stray = dir
        .path()
        .join("artifacts")
        .join(format!("{}.ipa", "a".repeat(64)));
    fs::write(&stray, b"unreferenced").unwrap();
    // And something that is not Orbiter's to judge, which must survive either way.
    let foreign = dir.path().join("artifacts").join("notes.txt");
    fs::write(&foreign, b"not mine").unwrap();

    let snapshot = lib.snapshot().unwrap();
    assert_eq!(snapshot.unreferenced_bytes, 12);
    // Startup cleanup finishes removals a person asked for. It does not sweep.
    lib.recover(None).unwrap();
    assert!(stray.exists());

    let freed = lib.reclaim().unwrap();
    assert_eq!(freed, 12);
    assert!(!stray.exists());
    assert!(foreign.exists());
    assert!(std::path::Path::new(&kept).exists());
    assert_eq!(lib.snapshot().unwrap().unreferenced_bytes, 0);
    // Nothing left to reclaim is not a failure.
    assert_eq!(lib.reclaim().unwrap(), 0);
}

#[test]
fn an_import_that_cannot_be_recorded_leaves_no_bytes_behind() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path().into());
    let first = fixture("one");
    lib.import(first.path()).unwrap();
    let before = lib.snapshot().unwrap().storage_bytes;

    // The manifest write fails after the bytes are published.
    let second = fixture("two");
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let result = lib.import(second.path());
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());

    // An operation cleans up after its own failure, so it leaves no orphan for a person to
    // wonder about — while bytes from an interrupted run still stay put until they are asked for.
    let after = lib.snapshot().unwrap();
    assert_eq!(after.storage_bytes, before);
    assert_eq!(after.unreferenced_bytes, 0);
    assert_eq!(after.artifacts.len(), 1);
}
