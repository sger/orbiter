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
fn range(b: &[u8], o: usize, n: usize) -> Result<&[u8]> {
    b.get(o..o.checked_add(n).ok_or(Error::MachO)?)
        .ok_or(Error::MachO)
}
fn u32at(b: &[u8], o: usize, le: bool) -> Result<u32> {
    let v = range(b, o, 4)?.try_into().map_err(|_| Error::MachO)?;
    Ok(if le {
        u32::from_le_bytes(v)
    } else {
        u32::from_be_bytes(v)
    })
}
fn u64at(b: &[u8], o: usize, le: bool) -> Result<usize> {
    let v = range(b, o, 8)?.try_into().map_err(|_| Error::MachO)?;
    usize::try_from(if le {
        u64::from_le_bytes(v)
    } else {
        u64::from_be_bytes(v)
    })
    .map_err(|_| Error::MachO)
}
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
#[cfg(test)]
mod tests {
    use super::*;
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
    fn detects_encryption() {
        assert!(inspect(&thin_fixture(1)).unwrap()[0].encrypted);
        assert!(!inspect(&thin_fixture(0)).unwrap()[0].encrypted);
    }
    #[test]
    fn rejects_truncated_commands() {
        let b = thin_fixture(0);
        for n in 0..b.len() {
            assert!(inspect(&b[..n]).is_err());
        }
    }
    #[test]
    fn rejects_bad_fat_offsets() {
        let mut b = vec![0; 28];
        b[..4].copy_from_slice(&0xcafebabe_u32.to_be_bytes());
        b[4..8].copy_from_slice(&1_u32.to_be_bytes());
        assert!(inspect(&b).is_err());
    }
    #[test]
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
}
