//! Normalize embedded app icons for the webview. Icon failures never block IPA inspection.
const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// Turn an icon read from an IPA into a PNG a web view can render, or decide it cannot be.
///
/// A standard PNG is returned unchanged. An Apple-optimised one (`CgBI`) has reordered channels
/// and premultiplied alpha that no ordinary decoder understands, so on macOS it is re-encoded by
/// the system converter; elsewhere there is nothing to do it with.
///
/// Returns `None` for anything else — a truncated file, an asset-catalog reference, an unexpected
/// format. `None` always means "there is no icon to show", never "inspection failed": an icon is
/// decoration, and an IPA with an unreadable one must still be inspectable.
pub fn normalize(bytes: Vec<u8>) -> Option<Vec<u8>> {
    if !bytes.starts_with(SIGNATURE) {
        return None;
    }
    if bytes.get(12..16) == Some(b"IHDR") {
        return Some(bytes);
    }
    if bytes.get(12..16) != Some(b"CgBI") {
        return None;
    }
    optimized(&bytes)
}
/// No converter for Apple-optimised icons off macOS, so such an icon simply has no rendering.
#[cfg(not(target_os = "macos"))]
fn optimized(_: &[u8]) -> Option<Vec<u8>> {
    None
}
/// Re-encode an Apple-optimised PNG using the system image converter.
///
/// The one subprocess inspection starts, and the only file it writes. The header is parsed and
/// the dimensions bounded *before* the bytes are handed over, so an arbitrary compressed image
/// cannot be pushed through the converter; the child runs with all three standard streams closed,
/// is killed after three seconds, and its output is size-bounded and re-validated as a real PNG
/// before being returned.
///
/// The original IPA's path is never passed: the image is copied into a private temporary
/// directory first, which is removed when that directory is dropped.
///
/// Returns `None` on any failure, including a timeout — see [`normalize`] for why that is never
/// escalated to an error.
#[cfg(target_os = "macos")]
fn optimized(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::{
        fs,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    // Inspect the bounded chunk header before giving a compressed image to macOS.
    let cgbi_size = u32::from_be_bytes(bytes.get(8..12)?.try_into().ok()?) as usize;
    let header = 20usize.checked_add(cgbi_size)?;
    if bytes.get(header + 4..header + 8)? != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes.get(header + 8..header + 12)?.try_into().ok()?);
    let height = u32::from_be_bytes(bytes.get(header + 12..header + 16)?.try_into().ok()?);
    if width == 0 || height == 0 || width > 1024 || height > 1024 {
        return None;
    }
    let temp = tempfile::tempdir().ok()?;
    let input = temp.path().join("embedded.png");
    let output = temp.path().join("display.png");
    fs::write(&input, bytes).ok()?;
    // Apple's image converter understands CgBI channel order and premultiplied alpha.
    // No shell, user-controlled executable, or original IPA path is passed to it.
    let mut child = Command::new("/usr/bin/sips")
        .args(["-s", "format", "png"])
        .arg(&input)
        .arg("--out")
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    if fs::metadata(&output).ok()?.len() > 2 * 1024 * 1024 {
        return None;
    }
    let result = fs::read(output).ok()?;
    (result.starts_with(SIGNATURE) && result.get(12..16) == Some(b"IHDR")).then_some(result)
}
#[cfg(test)]
/// Checks that unreadable icons are ignored and that a real optimised icon converts.
mod tests {
    use super::*;
    #[test]
    /// Bytes that are not a PNG, and a `CgBI` header with nothing after it, both yield no icon
    /// rather than a panic or a partial image.
    fn invalid_and_truncated_icons_are_ignored() {
        assert!(normalize(vec![0; 32]).is_none());
        assert!(normalize(b"\x89PNG\r\n\x1a\n\0\0\0\x04CgBI".to_vec()).is_none());
    }
    #[cfg(target_os = "macos")]
    #[test]
    /// A real Apple-optimised icon comes back as a standard PNG with an `IHDR` chunk and its
    /// original dimensions, proving the converter ran rather than the bytes being passed through.
    fn apple_optimized_icon_becomes_a_standard_png() {
        let bytes = include_bytes!("../tests/fixtures/optimized-icon.png");
        let image = normalize(bytes.to_vec()).expect("macOS converts the synthetic CgBI icon");
        assert_eq!(image.get(12..16), Some(b"IHDR".as_slice()));
        assert_eq!(u32::from_be_bytes(image[16..20].try_into().unwrap()), 1);
    }
}
