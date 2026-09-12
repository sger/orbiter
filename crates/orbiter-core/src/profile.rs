use crate::{Error, Result, entitlements, parse_plist, string};
use cms::{content_info::ContentInfo, signed_data::SignedData};
use der::{Decode, asn1::OctetString};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub struct Profile {
    pub name: Option<String>,
    pub team_name: Option<String>,
    pub team_id: Option<String>,
    pub expires_at: Option<String>,
    pub expired: Option<bool>,
    pub device_count: Option<usize>,
    pub distribution: String,
    pub entitlements: BTreeMap<String, serde_json::Value>,
    pub trust: &'static str,
}
pub(crate) fn dictionary(bytes: &[u8]) -> Result<plist::Dictionary> {
    // Parse the CMS envelope, not an XML substring. This does not verify CMS signatures.
    let content = ContentInfo::from_der(bytes).map_err(|_| Error::Profile)?;
    if content.content_type.to_string() != "1.2.840.113549.1.7.2" {
        return Err(Error::Profile);
    }
    let signed: SignedData = content.content.decode_as().map_err(|_| Error::Profile)?;
    if signed.encap_content_info.econtent_type.to_string() != "1.2.840.113549.1.7.1" {
        return Err(Error::Profile);
    }
    let octets: OctetString = signed
        .encap_content_info
        .econtent
        .ok_or(Error::Profile)?
        .decode_as()
        .map_err(|_| Error::Profile)?;
    parse_plist(octets.as_bytes())
}
pub fn inspect(bytes: &[u8]) -> Result<Profile> {
    from_dictionary(dictionary(bytes)?)
}
#[cfg(test)]
fn from_plist(bytes: &[u8]) -> Result<Profile> {
    from_dictionary(parse_plist(bytes)?)
}
fn from_dictionary(d: plist::Dictionary) -> Result<Profile> {
    let ent = entitlements(d.get("Entitlements"));
    let devices = d
        .get("ProvisionedDevices")
        .and_then(plist::Value::as_array)
        .map(Vec::len);
    let all = d
        .get("ProvisionsAllDevices")
        .and_then(plist::Value::as_boolean)
        .unwrap_or(false);
    let debug = ent
        .get("get-task-allow")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let expiry = d.get("ExpirationDate").and_then(plist::Value::as_date);
    Ok(Profile {
        name: string(&d, "Name"),
        team_name: string(&d, "TeamName"),
        team_id: d
            .get("TeamIdentifier")
            .and_then(plist::Value::as_array)
            .and_then(|a| a.first())
            .and_then(plist::Value::as_string)
            .map(str::to_owned),
        expires_at: expiry.map(|d| d.to_xml_format()),
        expired: expiry.map(|d| std::time::SystemTime::from(d) < std::time::SystemTime::now()),
        device_count: devices,
        distribution: if all {
            "Enterprise-like"
        } else if devices.is_some() {
            if debug {
                "Development-like"
            } else {
                "Ad Hoc-like"
            }
        } else {
            "No device allowlist (distribution unverified)"
        }
        .into(),
        entitlements: ent,
        trust: "Parsed only; CMS signature and certificate trust not verified",
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts_device_identifiers() {
        let p=from_plist(br#"<plist version="1.0"><dict><key>ProvisionedDevices</key><array><string>SECRET-UDID</string></array><key>Entitlements</key><dict><key>get-task-allow</key><false/></dict></dict></plist>"#).unwrap();
        assert_eq!(p.device_count, Some(1));
        assert_eq!(p.distribution, "Ad Hoc-like");
        assert!(!serde_json::to_string(&p).unwrap().contains("SECRET-UDID"));
    }
    #[test]
    fn rejects_xml_outside_cms() {
        assert!(inspect(b"junk<plist><dict/></plist>").is_err());
    }
    #[test]
    fn parses_cms_envelope_without_claiming_trust() {
        use cms::{
            content_info::CmsVersion,
            signed_data::{EncapsulatedContentInfo, SignerInfos},
        };
        use der::{Any, Encode};
        let xml = br#"<plist><dict><key>Name</key><string>Synthetic</string><key>ExpirationDate</key><date>2000-01-01T00:00:00Z</date></dict></plist>"#;
        let signed = SignedData {
            version: CmsVersion::V1,
            digest_algorithms: Default::default(),
            encap_content_info: EncapsulatedContentInfo {
                econtent_type: "1.2.840.113549.1.7.1".parse().unwrap(),
                econtent: Some(Any::encode_from(&OctetString::new(xml.to_vec()).unwrap()).unwrap()),
            },
            certificates: None,
            crls: None,
            signer_infos: SignerInfos(Default::default()),
        };
        let envelope = ContentInfo {
            content_type: "1.2.840.113549.1.7.2".parse().unwrap(),
            content: Any::encode_from(&signed).unwrap(),
        }
        .to_der()
        .unwrap();
        let p = inspect(&envelope).unwrap();
        assert_eq!(p.name.as_deref(), Some("Synthetic"));
        assert_eq!(p.expired, Some(true));
        assert!(p.trust.contains("not verified"));
        for n in 0..envelope.len() {
            assert!(inspect(&envelope[..n]).is_err());
        }
    }
}
