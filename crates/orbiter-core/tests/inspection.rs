use orbiter_core::{Error, inspect};
use std::{
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
};
use zip::{ZipWriter, write::SimpleFileOptions};
fn fixture(entries: Vec<(&str, Vec<u8>)>) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    {
        let mut z = ZipWriter::new(&mut file);
        for (name, data) in entries {
            z.start_file(name, SimpleFileOptions::default()).unwrap();
            z.write_all(&data).unwrap();
        }
        z.finish().unwrap();
    }
    file
}
fn info(id: &str) -> Vec<u8> {
    format!(r#"<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{id}</string><key>CFBundleExecutable</key><string>App</string></dict></plist>"#).into_bytes()
}
fn macho(crypt: u32) -> Vec<u8> {
    let mut b = vec![0; 56];
    for (o, v) in [
        (0, 0xfeedfacf_u32),
        (4, 0x100000c),
        (16, 1),
        (20, 24),
        (32, 0x2c),
        (36, 24),
        (48, crypt),
    ] {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    b
}
#[test]
fn nested_bundle_inventory() {
    let f = fixture(vec![
        ("Payload/A.app/Info.plist", info("test.app")),
        ("Payload/A.app/App", macho(0)),
        ("Payload/A.app/Watch/W.app/Info.plist", info("test.watch")),
        ("Payload/A.app/Watch/W.app/App", macho(0)),
        (
            "Payload/A.app/PlugIns/E.appex/Info.plist",
            info("test.extension"),
        ),
        ("Payload/A.app/PlugIns/E.appex/App", macho(0)),
        (
            "Payload/A.app/Frameworks/F.framework/Info.plist",
            info("test.framework"),
        ),
        ("Payload/A.app/Frameworks/F.framework/App", macho(0)),
    ]);
    let bytes = std::fs::read(f.path()).unwrap();
    let r = inspect(f.path(), &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(r.bundles.len(), 4);
    assert!(r.bundles.iter().any(|b| b.kind == "Watch app"));
    assert!(r.bundles.iter().any(|b| b.kind == "Extension"));
    assert!(r.bundles.iter().any(|b| b.kind == "Framework"));
    assert_eq!(std::fs::read(f.path()).unwrap(), bytes);
    assert!(!r.findings.iter().any(|f| f.status == "preserved"));
}
#[test]
fn rejects_traversal_and_windows_paths() {
    for n in [
        "../evil",
        "/absolute",
        "Payload/../evil",
        "C:/evil",
        "Payload\\evil",
        "Payload/A.app/../evil",
    ] {
        let f = fixture(vec![(n, vec![])]);
        assert!(
            matches!(
                inspect(f.path(), &AtomicBool::new(false), |_| {}),
                Err(Error::UnsafeArchive)
            ),
            "{n}"
        );
    }
}
#[test]
fn rejects_case_collisions() {
    let f = fixture(vec![
        ("Payload/A.app/Info.plist", info("a")),
        ("Payload/A.app/info.plist", info("a")),
    ]);
    assert!(matches!(
        inspect(f.path(), &AtomicBool::new(false), |_| {}),
        Err(Error::UnsafeArchive)
    ));
}
#[test]
fn rejects_symlinks() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    {
        let mut z = ZipWriter::new(&mut f);
        z.add_symlink(
            "Payload/A.app/link",
            "/etc/passwd",
            SimpleFileOptions::default(),
        )
        .unwrap();
        z.finish().unwrap();
    }
    assert!(matches!(
        inspect(f.path(), &AtomicBool::new(false), |_| {}),
        Err(Error::UnsafeArchive)
    ));
}
#[test]
fn rejects_bad_zip() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(b"not a zip").unwrap();
    assert!(matches!(
        inspect(f.path(), &AtomicBool::new(false), |_| {}),
        Err(Error::Zip)
    ));
}
#[test]
fn rejects_multiple_main_apps() {
    let f = fixture(vec![
        ("Payload/A.app/Info.plist", info("a")),
        ("Payload/B.app/Info.plist", info("b")),
    ]);
    assert!(matches!(
        inspect(f.path(), &AtomicBool::new(false), |_| {}),
        Err(Error::MainApp)
    ));
}
#[test]
fn malformed_metadata() {
    let f = fixture(vec![("Payload/A.app/Info.plist", b"bad plist".to_vec())]);
    assert!(matches!(
        inspect(f.path(), &AtomicBool::new(false), |_| {}),
        Err(Error::Plist)
    ));
}
#[test]
fn cancellation_at_stage_boundary_and_retry() {
    let f = fixture(vec![
        ("Payload/A.app/Info.plist", info("a")),
        ("Payload/A.app/App", macho(0)),
    ]);
    let c = AtomicBool::new(false);
    assert!(matches!(
        inspect(f.path(), &c, |s| {
            if s == "Reading bundles and signatures" {
                c.store(true, Ordering::Relaxed)
            }
        }),
        Err(Error::Cancelled)
    ));
    c.store(false, Ordering::Relaxed);
    assert!(inspect(f.path(), &c, |_| {}).is_ok());
}
#[test]
fn encrypted_build_has_hard_blocker() {
    let f = fixture(vec![
        ("Payload/A.app/Info.plist", info("a")),
        ("Payload/A.app/App", macho(1)),
    ]);
    let r = inspect(f.path(), &AtomicBool::new(false), |_| {}).unwrap();
    assert!(
        r.findings
            .iter()
            .any(|f| f.status == "unsupported" && f.title == "Encrypted executable")
    );
}
#[test]
fn oversized_plist_rejected() {
    let f = fixture(vec![(
        "Payload/A.app/Info.plist",
        vec![0; 4 * 1024 * 1024 + 1],
    )]);
    assert!(matches!(
        inspect(f.path(), &AtomicBool::new(false), |_| {}),
        Err(Error::Limits)
    ));
}
#[test]
fn corrupt_executable_is_never_compatible() {
    let f = fixture(vec![
        ("Payload/A.app/Info.plist", info("a")),
        ("Payload/A.app/App", vec![0; 64]),
    ]);
    let r = inspect(f.path(), &AtomicBool::new(false), |_| {}).unwrap();
    assert!(!r.bundles[0].issues.is_empty());
    assert!(
        r.findings
            .iter()
            .any(|f| f.title == "Incomplete inspection")
    );
}
#[test]
fn rejects_deeply_nested_plist() {
    let xml = format!(
        "<plist><dict><key>x</key>{}<string>x</string>{}</dict></plist>",
        "<array>".repeat(200),
        "</array>".repeat(200)
    );
    let f = fixture(vec![("Payload/A.app/Info.plist", xml.into_bytes())]);
    assert!(matches!(
        inspect(f.path(), &AtomicBool::new(false), |_| {}),
        Err(Error::Plist)
    ));
}
#[test]
fn binary_plist_supported() {
    let mut bytes = Vec::new();
    let mut d = plist::Dictionary::new();
    d.insert("CFBundleIdentifier".into(), "test.binary".into());
    d.insert("CFBundleExecutable".into(), "App".into());
    plist::Value::Dictionary(d)
        .to_writer_binary(&mut bytes)
        .unwrap();
    let f = fixture(vec![
        ("Payload/A.app/Info.plist", bytes),
        ("Payload/A.app/App", macho(0)),
    ]);
    assert_eq!(
        inspect(f.path(), &AtomicBool::new(false), |_| {})
            .unwrap()
            .bundles[0]
            .identifier,
        "test.binary"
    );
}
#[test]
fn rejects_duplicate_central_directory_names() {
    let f = fixture(vec![
        ("Payload/A.app/Info.plist", info("a")),
        ("Payload/B.app/Info.plist", info("b")),
    ]);
    let mut bytes = std::fs::read(f.path()).unwrap();
    let from = b"Payload/B.app/Info.plist";
    let to = b"Payload/A.app/Info.plist";
    for i in 0..=bytes.len() - from.len() {
        if &bytes[i..i + from.len()] == from {
            bytes[i..i + to.len()].copy_from_slice(to);
        }
    }
    std::fs::write(f.path(), bytes).unwrap();
    assert!(matches!(
        inspect(f.path(), &AtomicBool::new(false), |_| {}),
        Err(Error::UnsafeArchive)
    ));
}

#[test]
fn an_icon_too_large_to_read_is_absent_rather_than_fatal() {
    // A declared icon over the 2 MiB member limit used to fail the whole inspection, so an IPA
    // with a large icon could not be inspected or imported at all.
    let huge = vec![0x41u8; 3 * 1024 * 1024];
    let report = inspect(
        fixture(vec![
            (
                "Payload/A.app/Info.plist",
                br#"<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>test.icon</string><key>CFBundleName</key><string>Icon Test</string><key>CFBundleExecutable</key><string>App</string><key>CFBundleIconFiles</key><array><string>Icon</string></array></dict></plist>"#.to_vec(),
            ),
            ("Payload/A.app/App", macho(0)),
            ("Payload/A.app/Icon.png", huge),
        ])
        .path(),
        &AtomicBool::new(false),
        |_| {},
    )
    .expect("an unreadable icon is not a reason to refuse the archive");
    assert_eq!(report.icon_data_url, None);
    assert_eq!(report.bundles.len(), 1);
}
