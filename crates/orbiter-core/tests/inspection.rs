//! Inspection against deliberately hostile archives.
//!
//! An IPA is a file someone else produced, so most of these are refusals: traversal, case
//! collisions, symlinks, duplicate entries, oversized and deeply nested property lists. Each
//! asserts that inspection declines rather than partially succeeding, because a half-read report
//! is worse than none.

use orbiter_core::{Error, inspect};
use std::{
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
};
use zip::{ZipWriter, write::SimpleFileOptions};
/// An archive containing exactly the entries given, valid or not.
///
/// Deliberately writes whatever it is handed, including traversal paths and duplicate names, so a
/// test can present the malformed archives inspection has to refuse.
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
/// A minimal `Info.plist` declaring one bundle identifier and executable.
fn info(id: &str) -> Vec<u8> {
    format!(r#"<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{id}</string><key>CFBundleExecutable</key><string>App</string></dict></plist>"#).into_bytes()
}
/// A minimal 64-bit arm64 Mach-O with one load command, optionally marked encrypted.
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
/// Nested apps, extensions and frameworks are all found and described, each with its own
/// identifier, rather than only the main app being reported.
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
/// An entry name containing traversal, an absolute path, or a Windows separator is refused before
/// anything is read: a crafted IPA must not be able to name a file outside the archive.
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
/// Two entries differing only in case are refused, because on a case-insensitive filesystem they
/// would become one file and the second would silently replace the first.
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
/// A symlink entry is refused rather than followed, so an archive cannot reach outside itself.
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
/// A file that is not a valid archive is refused with a message saying so, rather than being
/// partially read.
fn rejects_bad_zip() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(b"not a zip").unwrap();
    assert!(matches!(
        inspect(f.path(), &AtomicBool::new(false), |_| {}),
        Err(Error::Zip)
    ));
}
#[test]
/// An archive with more or fewer than exactly one main app is refused: guessing which one was
/// meant would describe a build nobody asked about.
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
/// A bundle whose `Info.plist` is malformed or lacks an identifier is refused rather than
/// described with invented values.
fn malformed_metadata() {
    let f = fixture(vec![("Payload/A.app/Info.plist", b"bad plist".to_vec())]);
    assert!(matches!(
        inspect(f.path(), &AtomicBool::new(false), |_| {}),
        Err(Error::Plist)
    ));
}
#[test]
/// Cancellation takes effect at a boundary and leaves no partial report, and inspecting the same
/// archive again afterwards succeeds — a cancelled run poisons nothing.
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
/// An encrypted executable produces an unsupported finding: it cannot be re-signed, and saying so
/// during inspection is what stops a plan being built on it.
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
/// A property list beyond the size bound is refused before the recursive value builder sees it.
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
/// A bundle whose executable cannot be read is reported as not verified, never as compatible:
/// absence of evidence is not evidence the build will run.
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
/// A deeply nested property list is refused at the event stream, before recursion could exhaust
/// the stack.
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
/// Binary property lists are read as well as XML ones, since real IPAs ship both.
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
/// An archive whose central directory lists the same name twice is refused: which entry a reader
/// would get is not something to leave to chance.
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
/// An icon beyond the member size limit leaves the report without one, rather than failing the
/// whole inspection — which would make an IPA with a large icon impossible to import at all.
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
