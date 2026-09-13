//! Bounded, read-only Mach-O load-command and embedded entitlement inspection.
use crate::{Error, Result};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub struct Slice {
    pub architecture: String,
    pub encrypted: bool,
    pub entitlements: BTreeMap<String, serde_json::Value>,
    pub xml_entitlements_present: bool,
    pub der_entitlements_present: bool,
}
/// Borrow `n` bytes at offset `o`, or refuse if that range is not entirely inside `b`.
///
/// Every read of this executable goes through here, including the addition of offset and length,
/// which is checked for overflow. The offsets come from the file being inspected, so each one is a
/// claim rather than a fact.
///
/// # Errors
///
/// Returns [`Error::MachO`] when the range would run past the end of the slice or the offset
/// arithmetic overflows.
fn range(b: &[u8], o: usize, n: usize) -> Result<&[u8]> {
    b.get(o..o.checked_add(n).ok_or(Error::MachO)?)
        .ok_or(Error::MachO)
}
/// Read a 32-bit integer at `o` in the file's own byte order.
///
/// # Errors
///
/// Returns [`Error::MachO`] if those four bytes are not inside the slice.
fn u32at(b: &[u8], o: usize, le: bool) -> Result<u32> {
    let v = range(b, o, 4)?.try_into().map_err(|_| Error::MachO)?;
    Ok(if le {
        u32::from_le_bytes(v)
    } else {
        u32::from_be_bytes(v)
    })
}
/// Read a 64-bit integer at `o` in the file's own byte order, as a `usize`.
///
/// # Errors
///
/// Returns [`Error::MachO`] if those eight bytes are not inside the slice, or if the value does
/// not fit a `usize` on this platform — a 64-bit offset from an untrusted file must not be
/// truncated into a smaller one that happens to be in range.
fn u64at(b: &[u8], o: usize, le: bool) -> Result<usize> {
    let v = range(b, o, 8)?.try_into().map_err(|_| Error::MachO)?;
    usize::try_from(if le {
        u64::from_le_bytes(v)
    } else {
        u64::from_be_bytes(v)
    })
    .map_err(|_| Error::MachO)
}
/// Describe every architecture slice in a Mach-O executable.
///
/// Handles both a single slice and a fat binary. For a fat binary each slice's declared offset and
/// length are checked against the real file before being read, so a crafted header cannot make the
/// reader look outside it.
///
/// # Errors
///
/// Returns [`Error::MachO`] for a malformed header, a truncated load command, a slice that does
/// not fit inside the file, or a signature whose offsets do not agree with its contents.
pub fn inspect(b: &[u8]) -> Result<Vec<Slice>> {
    let magic = u32at(b, 0, false)?;
    match magic {
        0xcafebabe | 0xbebafeca | 0xcafebabf | 0xbfbafeca => {
            let le = matches!(magic, 0xbebafeca | 0xbfbafeca);
            let wide = matches!(magic, 0xcafebabf | 0xbfbafeca);
            let n = u32at(b, 4, le)? as usize;
            if n == 0 || n > 32 {
                return Err(Error::MachO);
            }
            let mut result = Vec::new();
            let stride = if wide { 32 } else { 20 };
            let table_end = 8 + n * stride;
            range(b, 0, table_end)?;
            let mut ranges = Vec::new();
            for i in 0..n {
                let base = 8 + i * stride;
                let (offset, size) = if wide {
                    (u64at(b, base + 8, le)?, u64at(b, base + 16, le)?)
                } else {
                    (
                        u32at(b, base + 8, le)? as usize,
                        u32at(b, base + 12, le)? as usize,
                    )
                };
                let end = offset.checked_add(size).ok_or(Error::MachO)?;
                if offset < table_end || ranges.iter().any(|&(a, z)| offset < z && end > a) {
                    return Err(Error::MachO);
                }
                ranges.push((offset, end));
                result.push(thin(range(b, offset, size)?)?);
            }
            Ok(result)
        }
        _ => Ok(vec![thin(b)?]),
    }
}
/// Describe one architecture slice: its architecture, whether it is encrypted, and its
/// entitlements.
///
/// Walks the load commands once, bounding each by the size it declares. Encryption is read from
/// the encryption-info command, because an encrypted executable cannot be re-signed and saying so
/// early is the whole point of noticing.
///
/// # Errors
///
/// Returns [`Error::MachO`] for an unrecognised header, a load command that claims a size the file
/// does not have, or a command count that would run past the end.
fn thin(b: &[u8]) -> Result<Slice> {
    let magic = u32at(b, 0, false)?;
    let le = matches!(magic, 0xcefaedfe | 0xcffaedfe);
    if !matches!(magic, 0xfeedface | 0xfeedfacf | 0xcefaedfe | 0xcffaedfe) {
        return Err(Error::MachO);
    }
    let wide = matches!(magic, 0xfeedfacf | 0xcffaedfe);
    let cpu = u32at(b, 4, le)?;
    let sub = u32at(b, 8, le)? & 0xffffff;
    let architecture = match (cpu, sub) {
        (0x100000c, 2) => "arm64e".into(),
        (0x100000c, _) => "arm64".into(),
        (0x200000c, _) => "arm64_32".into(),
        (12, _) => "arm".into(),
        (0x1000007, _) => "x86_64".into(),
        (7, _) => "i386".into(),
        _ => format!("unknown ({cpu:#x})"),
    };
    let count = u32at(b, 16, le)? as usize;
    let mut cursor = if wide { 32 } else { 28 };
    let commands_end = cursor + u32at(b, 20, le)? as usize;
    range(b, 0, commands_end)?;
    if count > 65536 {
        return Err(Error::MachO);
    }
    let mut out = Slice {
        architecture,
        encrypted: false,
        entitlements: BTreeMap::new(),
        xml_entitlements_present: false,
        der_entitlements_present: false,
    };
    let mut signature_seen = false;
    for _ in 0..count {
        let cmd = u32at(b, cursor, le)?;
        let len = u32at(b, cursor + 4, le)? as usize;
        if len < 8 || cursor + len > commands_end {
            return Err(Error::MachO);
        }
        if matches!(cmd, 0x21 | 0x2c) {
            if len < if cmd == 0x2c { 24 } else { 20 } {
                return Err(Error::MachO);
            }
            out.encrypted |= u32at(b, cursor + 16, le)? != 0;
        }
        if cmd == 0x1d {
            if len < 16 || signature_seen {
                return Err(Error::MachO);
            }
            signature_seen = true;
            let o = u32at(b, cursor + 8, le)? as usize;
            let n = u32at(b, cursor + 12, le)? as usize;
            signature(range(b, o, n)?, &mut out)?;
        }
        cursor += len;
    }
    if cursor != commands_end {
        return Err(Error::MachO);
    }
    Ok(out)
}
/// Read a code signature's embedded entitlements into `out`.
///
/// Records whether XML entitlements were present, whether DER ones were, and the decoded XML. DER
/// entitlements are noticed but not decoded — a bundle carrying only those is marked as not fully
/// inspected rather than described as having none, because "not read" and "not there" lead to
/// opposite conclusions.
///
/// # Errors
///
/// Returns [`Error::MachO`] when a blob's declared offset or length does not fit inside the
/// signature, and [`Error::Plist`] when an entitlements payload is malformed or oversized.
fn signature(b: &[u8], out: &mut Slice) -> Result<()> {
    if u32at(b, 0, false)? != 0xfade0cc0 {
        return Err(Error::MachO);
    }
    let len = u32at(b, 4, false)? as usize;
    let b = range(b, 0, len)?;
    let count = u32at(b, 8, false)? as usize;
    if count > 1024 {
        return Err(Error::MachO);
    }
    range(b, 12, count * 8)?;
    for i in 0..count {
        let offset = u32at(b, 16 + i * 8, false)? as usize;
        if offset < 12 + count * 8 {
            return Err(Error::MachO);
        }
        let magic = u32at(b, offset, false)?;
        let size = u32at(b, offset + 4, false)? as usize;
        if size < 8 {
            return Err(Error::MachO);
        }
        let blob = range(b, offset, size)?;
        match magic {
            0xfade7171 => {
                if out.xml_entitlements_present || size > 4 * 1024 * 1024 {
                    return Err(Error::MachO);
                }
                let dict = crate::parse_plist(&blob[8..])?;
                out.entitlements = crate::entitlements(Some(&plist::Value::Dictionary(dict)));
                out.xml_entitlements_present = true;
            }
            0xfade7172 => out.der_entitlements_present = true,
            _ => (),
        }
    }
    Ok(())
}
/// Add a load command that makes this executable load `install_path` at launch — `LC_LOAD_DYLIB`,
/// or `LC_LOAD_WEAK_DYLIB` when `weak`.
///
/// The command is written into the zero padding between the end of the load-command list and the
/// first byte of section content the commands describe; only `ncmds` and `sizeofcmds` in the header
/// grow to cover it. No segment or section file offset moves, so nothing downstream needs
/// relocating and a fat binary's slice table stays exact — each slice is patched inside its own
/// region and keeps its length. A slice that already loads `install_path` is left untouched, so
/// re-signing an already-injected build does not stack a second command.
///
/// # Errors
///
/// Returns [`Error::MachO`] for a malformed header, an encrypted slice (which cannot be re-signed),
/// or a slice whose header padding is too small to hold the command — corrupting the binary to make
/// it fit is never preferable to refusing.
pub fn add_load_dylib(bytes: &[u8], install_path: &str, weak: bool) -> Result<Vec<u8>> {
    let magic = u32at(bytes, 0, false)?;
    match magic {
        0xcafebabe | 0xbebafeca | 0xcafebabf | 0xbfbafeca => {
            let le = matches!(magic, 0xbebafeca | 0xbfbafeca);
            let wide = matches!(magic, 0xcafebabf | 0xbfbafeca);
            let n = u32at(bytes, 4, le)? as usize;
            if n == 0 || n > 32 {
                return Err(Error::MachO);
            }
            let stride = if wide { 32 } else { 20 };
            let table_end = 8 + n * stride;
            range(bytes, 0, table_end)?;
            let mut slices = Vec::new();
            let mut ranges = Vec::new();
            for i in 0..n {
                let base = 8 + i * stride;
                let (offset, size) = if wide {
                    (u64at(bytes, base + 8, le)?, u64at(bytes, base + 16, le)?)
                } else {
                    (
                        u32at(bytes, base + 8, le)? as usize,
                        u32at(bytes, base + 12, le)? as usize,
                    )
                };
                let end = offset.checked_add(size).ok_or(Error::MachO)?;
                if offset < table_end || ranges.iter().any(|&(a, z)| offset < z && end > a) {
                    return Err(Error::MachO);
                }
                ranges.push((offset, end));
                slices.push((offset, size));
            }
            let mut out = bytes.to_vec();
            for (offset, size) in slices {
                // `add_thin` returns the slice at exactly its original length, so the fat table's
                // recorded offsets and sizes stay correct without any fixup.
                let patched = add_thin(range(bytes, offset, size)?, install_path, weak)?;
                out[offset..offset + size].copy_from_slice(&patched);
            }
            Ok(out)
        }
        _ => add_thin(bytes, install_path, weak),
    }
}
/// Add a dylib load command to one architecture slice, writing it into the slice's own header
/// padding. The returned bytes are the same length as `b`: only padding is consumed, and only
/// `ncmds` and `sizeofcmds` change.
fn add_thin(b: &[u8], install_path: &str, weak: bool) -> Result<Vec<u8>> {
    const LC_SEGMENT: u32 = 0x1;
    const LC_SEGMENT_64: u32 = 0x19;
    const LC_LOAD_DYLIB: u32 = 0xc;
    const LC_LOAD_WEAK_DYLIB: u32 = 0x8000_0018;

    let magic = u32at(b, 0, false)?;
    let le = matches!(magic, 0xcefaedfe | 0xcffaedfe);
    if !matches!(magic, 0xfeedface | 0xfeedfacf | 0xcefaedfe | 0xcffaedfe) {
        return Err(Error::MachO);
    }
    let wide = matches!(magic, 0xfeedfacf | 0xcffaedfe);
    let count = u32at(b, 16, le)? as usize;
    let sizeofcmds = u32at(b, 20, le)? as usize;
    let header = if wide { 32 } else { 28 };
    let commands_end = header + sizeofcmds;
    range(b, 0, commands_end)?;
    if count > 65536 {
        return Err(Error::MachO);
    }
    // Walk the commands once: refuse an encrypted slice, skip if the path is already loaded, and
    // find where section content begins — the lowest non-zero section file offset, which is the
    // first byte the header padding must not overwrite.
    let mut cursor = header;
    let mut content_start: Option<usize> = None;
    for _ in 0..count {
        let cmd = u32at(b, cursor, le)?;
        let len = u32at(b, cursor + 4, le)? as usize;
        if len < 8 || cursor + len > commands_end {
            return Err(Error::MachO);
        }
        if matches!(cmd, 0x21 | 0x2c) {
            if len < if cmd == 0x2c { 24 } else { 20 } {
                return Err(Error::MachO);
            }
            if u32at(b, cursor + 16, le)? != 0 {
                return Err(Error::MachO);
            }
        }
        if matches!(cmd, LC_LOAD_DYLIB | LC_LOAD_WEAK_DYLIB) {
            if len < 24 {
                return Err(Error::MachO);
            }
            let name_off = u32at(b, cursor + 8, le)? as usize;
            if name_off < len {
                let field = &b[cursor + name_off..cursor + len];
                let name = field.split(|&c| c == 0).next().unwrap_or(field);
                if name == install_path.as_bytes() {
                    return Ok(b.to_vec());
                }
            }
        }
        if cmd == LC_SEGMENT_64 {
            if len < 72 {
                return Err(Error::MachO);
            }
            let nsects = u32at(b, cursor + 64, le)? as usize;
            for s in 0..nsects {
                let sec = cursor + 72 + s * 80;
                if sec + 80 > cursor + len {
                    return Err(Error::MachO);
                }
                let off = u32at(b, sec + 48, le)? as usize;
                if off != 0 {
                    content_start = Some(content_start.map_or(off, |m| m.min(off)));
                }
            }
        }
        if cmd == LC_SEGMENT {
            if len < 56 {
                return Err(Error::MachO);
            }
            let nsects = u32at(b, cursor + 48, le)? as usize;
            for s in 0..nsects {
                let sec = cursor + 56 + s * 68;
                if sec + 68 > cursor + len {
                    return Err(Error::MachO);
                }
                let off = u32at(b, sec + 40, le)? as usize;
                if off != 0 {
                    content_start = Some(content_start.map_or(off, |m| m.min(off)));
                }
            }
        }
        cursor += len;
    }
    if cursor != commands_end {
        return Err(Error::MachO);
    }
    let content_start = content_start.ok_or(Error::MachO)?;

    // A `dylib_command`: six 32-bit words then the null-terminated path, the whole command padded
    // to an 8-byte multiple so what follows it stays aligned.
    let name = install_path.as_bytes();
    let cmdsize = (24 + name.len() + 1 + 7) & !7;
    if content_start < commands_end || commands_end + cmdsize > content_start {
        return Err(Error::MachO);
    }
    let word = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let mut command = vec![0u8; cmdsize];
    command[0..4].copy_from_slice(&word(if weak {
        LC_LOAD_WEAK_DYLIB
    } else {
        LC_LOAD_DYLIB
    }));
    command[4..8].copy_from_slice(&word(cmdsize as u32));
    command[8..12].copy_from_slice(&word(24)); // name offset within the command
    command[12..16].copy_from_slice(&word(2)); // timestamp
    command[16..20].copy_from_slice(&word(0x1_0000)); // current version 1.0.0
    command[20..24].copy_from_slice(&word(0x1_0000)); // compatibility version 1.0.0
    command[24..24 + name.len()].copy_from_slice(name);

    let mut out = b.to_vec();
    out[commands_end..commands_end + cmdsize].copy_from_slice(&command);
    out[16..20].copy_from_slice(&word(count as u32 + 1));
    out[20..24].copy_from_slice(&word((sizeofcmds + cmdsize) as u32));
    Ok(out)
}
#[cfg(test)]
/// Checks that a crafted executable cannot make the reader look outside the file, and that
/// encryption and entitlements are read from where they actually are.
mod tests {
    use super::*;
    /// A minimal 64-bit Mach-O with one load command, optionally marked as encrypted.
    fn thin_fixture(crypt: u32) -> Vec<u8> {
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
    /// An executable whose encryption-info command says it is encrypted is reported as such,
    /// because an encrypted binary cannot be re-signed and the plan must refuse early.
    fn detects_encryption() {
        assert!(inspect(&thin_fixture(1)).unwrap()[0].encrypted);
        assert!(!inspect(&thin_fixture(0)).unwrap()[0].encrypted);
    }
    #[test]
    /// A load command that claims more bytes than the file holds is refused rather than read up
    /// to the end of whatever happens to follow it.
    fn rejects_truncated_commands() {
        let b = thin_fixture(0);
        for n in 0..b.len() {
            assert!(inspect(&b[..n]).is_err());
        }
    }
    #[test]
    /// A fat header whose slice offset or length points outside the file is refused: the offsets
    /// come from the file being inspected and are claims, not facts.
    fn rejects_bad_fat_offsets() {
        let mut b = vec![0; 28];
        b[..4].copy_from_slice(&0xcafebabe_u32.to_be_bytes());
        b[4..8].copy_from_slice(&1_u32.to_be_bytes());
        assert!(inspect(&b).is_err());
    }
    #[test]
    /// Entitlements are read from inside the code signature, and a blob whose declared offset or
    /// length does not fit is refused instead of being read from adjacent bytes.
    fn reads_signature_entitlements_and_rejects_bad_offsets() {
        let xml = br#"<plist><dict><key>aps-environment</key><string>production</string><key>get-task-allow</key><false/></dict></plist>"#;
        let length = 28 + xml.len();
        let mut signature = Vec::new();
        for word in [
            0xfade0cc0_u32,
            length as u32,
            1,
            5,
            20,
            0xfade7171,
            (xml.len() + 8) as u32,
        ] {
            signature.extend_from_slice(&word.to_be_bytes());
        }
        signature.extend_from_slice(xml);
        let mut b = vec![0; 48];
        for (o, v) in [
            (0, 0xfeedfacf_u32),
            (4, 0x100000c),
            (16, 1),
            (20, 16),
            (32, 0x1d),
            (36, 16),
            (40, 48),
            (44, length as u32),
        ] {
            b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&signature);
        let s = inspect(&b).unwrap();
        assert_eq!(s[0].entitlements["aps-environment"], "production");
        assert_eq!(s[0].entitlements["get-task-allow"], false);
        b[64..68].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(inspect(&b).is_err());
    }
    /// A 64-bit slice with one `__TEXT` segment whose single section starts at `section_offset`,
    /// leaving header padding after the load commands for an injected command to land in.
    fn injectable(section_offset: u32) -> Vec<u8> {
        let mut b = vec![0u8; 512];
        // header + one LC_SEGMENT_64 (152 bytes: 72-byte command + one 80-byte section).
        for (o, v) in [
            (0, 0xfeedfacf_u32),
            (4, 0x100000c),
            (16, 1),   // ncmds
            (20, 152), // sizeofcmds
            (32, 0x19),
            (36, 152),
        ] {
            b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        b[96..100].copy_from_slice(&1_u32.to_le_bytes()); // nsects
        // section_64 begins at 104; its file offset field is at +48.
        b[152..156].copy_from_slice(&section_offset.to_le_bytes());
        b
    }
    /// Names of every dylib the slice loads, so a test can confirm the command really landed.
    fn loaded(b: &[u8]) -> Vec<String> {
        let count = u32::from_le_bytes(b[16..20].try_into().unwrap()) as usize;
        let mut cursor = 32;
        let mut out = Vec::new();
        for _ in 0..count {
            let cmd = u32::from_le_bytes(b[cursor..cursor + 4].try_into().unwrap());
            let len = u32::from_le_bytes(b[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
            if matches!(cmd, 0xc | 0x8000_0018) {
                let off =
                    u32::from_le_bytes(b[cursor + 8..cursor + 12].try_into().unwrap()) as usize;
                let field = &b[cursor + off..cursor + len];
                let name = field.split(|&c| c == 0).next().unwrap_or(field);
                out.push(String::from_utf8_lossy(name).into_owned());
            }
            cursor += len;
        }
        out
    }
    #[test]
    /// An injected command lands in the header padding, the header counts grow to cover it, the file
    /// keeps its length, and the binary still parses and now loads the named library.
    fn appends_load_command_into_padding() {
        let path = "@executable_path/Frameworks/tweak.dylib";
        let out = add_load_dylib(&injectable(256), path, false).unwrap();
        assert_eq!(out.len(), 512);
        assert_eq!(u32::from_le_bytes(out[16..20].try_into().unwrap()), 2);
        assert!(inspect(&out).is_ok());
        assert_eq!(loaded(&out), vec![path.to_string()]);
    }
    #[test]
    /// A slice whose section content begins right after the load commands has no room, and is
    /// refused rather than corrupted.
    fn refuses_when_padding_too_small() {
        assert!(add_load_dylib(&injectable(184), "x.dylib", false).is_err());
    }
    #[test]
    /// Injecting a path the slice already loads is a no-op, so re-signing an injected build never
    /// stacks a second copy of the command.
    fn skips_when_already_present() {
        let path = "@executable_path/Frameworks/tweak.dylib";
        let once = add_load_dylib(&injectable(256), path, false).unwrap();
        let twice = add_load_dylib(&once, path, false).unwrap();
        assert_eq!(once, twice);
        assert_eq!(loaded(&twice).len(), 1);
    }
    #[test]
    /// Every slice of a fat binary is patched inside its own region; the slice table is untouched
    /// and both slices load the library afterwards.
    fn patches_every_fat_slice() {
        let slice = injectable(256);
        let (a, z) = (4096usize, 8192usize);
        let mut b = vec![0u8; z + slice.len()];
        b[0..4].copy_from_slice(&0xcafebabe_u32.to_be_bytes());
        b[4..8].copy_from_slice(&2_u32.to_be_bytes());
        for (i, offset) in [a, z].into_iter().enumerate() {
            let base = 8 + i * 20;
            b[base + 8..base + 12].copy_from_slice(&(offset as u32).to_be_bytes());
            b[base + 12..base + 16].copy_from_slice(&(slice.len() as u32).to_be_bytes());
            b[offset..offset + slice.len()].copy_from_slice(&slice);
        }
        let path = "@executable_path/Frameworks/tweak.dylib";
        let out = add_load_dylib(&b, path, false).unwrap();
        assert_eq!(out.len(), b.len());
        assert_eq!(inspect(&out).unwrap().len(), 2);
        for offset in [a, z] {
            assert_eq!(
                loaded(&out[offset..offset + slice.len()]),
                vec![path.to_string()]
            );
        }
    }
}
