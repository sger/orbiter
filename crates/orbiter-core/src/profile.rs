//! Reading an embedded provisioning profile.
//!
//! A `.mobileprovision` is a CMS envelope wrapping a property list. This parses the envelope
//! properly rather than hunting for an XML substring inside it, and reports what the profile
//! claims — **without** verifying the CMS signature or any certificate chain. Nothing here decides
//! that a profile is trustworthy; `trust` says so in as many words, and every caller repeats it.
//!
//! # Privacy
//!
//! `ProvisionedDevices` lists real UDIDs. Only their count is kept, so a report can be pasted into
//! an issue without publishing the hardware identity of someone's phone.

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
    /// The same moment as `expires_at`, as seconds since the epoch, so nothing downstream parses a
    /// human-readable date back out of a display string. Zero is never written: unknown is `None`.
    pub expires_unix: Option<i64>,
    pub expired: Option<bool>,
    pub device_count: Option<usize>,
    pub distribution: String,
    pub entitlements: BTreeMap<String, serde_json::Value>,
    pub trust: &'static str,
}
/// Unwrap the CMS envelope and return the property list inside it.
///
/// Checks both content-type OIDs rather than trusting the structure, so a file that merely
/// contains a plist is rejected instead of being read as a profile.
///
/// # Errors
///
/// Returns [`Error::Profile`] for anything that is not a signed-data envelope wrapping data, and
/// [`Error::Plist`] if the payload is not a bounded, well-formed property list.
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
/// Describe an embedded provisioning profile.
///
/// # Errors
///
/// Returns [`Error::Profile`] or [`Error::Plist`]; see [`dictionary`].
pub fn inspect(bytes: &[u8]) -> Result<Profile> {
    from_dictionary(dictionary(bytes)?)
}
#[cfg(test)]
/// Read a profile from a bare property list, skipping the CMS envelope.
///
/// Test-only: it lets a case describe the payload directly instead of building a signed envelope
/// around it. Production code always goes through [`inspect`], which requires the envelope.
///
/// # Errors
///
/// Returns [`Error::Plist`] if the bytes are not a bounded, well-formed property list.
fn from_plist(bytes: &[u8]) -> Result<Profile> {
    from_dictionary(parse_plist(bytes)?)
}
/// Build the description from an already-parsed profile payload.
///
/// Classifies distribution from what the profile carries rather than from its name: a device
/// allowlist plus `get-task-allow` reads as development, an allowlist without it as ad hoc, and
/// `ProvisionsAllDevices` as enterprise-like. Each label ends in "-like" because this is an
/// inference from contents, not a statement about how the profile was issued.
///
/// Expiry is carried in two forms — the displayable string and epoch seconds — taken from one
/// value, so the date shown and the date counted can never disagree.
///
/// # Errors
///
/// Currently infallible, but returns `Result` so a future validity check does not change every
/// caller.
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
        expires_unix: expiry
            .map(|d| crate::renewal::now_unix(std::time::SystemTime::from(d)))
            .filter(|unix| *unix > 0),
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
/// Checks that a profile is parsed from its real envelope and that device identities stay out.
mod tests {
    use super::*;
    #[test]
    /// A profile's device allowlist is reduced to a count: the serialised description must not
    /// contain a UDID, because reports are pasted into issues.
    fn redacts_device_identifiers() {
        let p=from_plist(br#"<plist version="1.0"><dict><key>ProvisionedDevices</key><array><string>SECRET-UDID</string></array><key>Entitlements</key><dict><key>get-task-allow</key><false/></dict></dict></plist>"#).unwrap();
        assert_eq!(p.device_count, Some(1));
        assert_eq!(p.distribution, "Ad Hoc-like");
        assert!(!serde_json::to_string(&p).unwrap().contains("SECRET-UDID"));
    }
    #[test]
    /// A file that merely contains a property list is not a profile. Accepting one would mean
    /// anything could claim to be provisioning.
    fn rejects_xml_outside_cms() {
        assert!(inspect(b"junk<plist><dict/></plist>").is_err());
    }
    #[test]
    /// A real signed-data envelope is parsed, its expiry is recognised, the result still says
    /// trust was not verified — and every truncation of those same bytes is refused rather than
    /// partially read.
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
