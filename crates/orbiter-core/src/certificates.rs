//! Development certificate for the signing team.
//!
//! The private key is generated on this Mac and never leaves it: only a certificate signing
//! request goes to Apple. Nothing is persisted yet, so the key and certificate live as long as the
//! signed-in session and a restart needs a new certificate — which matters because a team allows
//! only a few active certificates at once.
//!
//! Certificates are never revoked automatically. Revoking one invalidates every app already signed
//! with it, including apps this tool did not produce, so reaching the limit is reported and left
//! to the person.
use isideload::dev::{
    certificates::{CertificatesApi, DevelopmentCertificate},
    developer_session::DeveloperSession,
    teams::DeveloperTeam,
};
use rsa::{
    RsaPrivateKey,
    pkcs1::EncodeRsaPublicKey,
    pkcs8::{EncodePrivateKey, LineEnding},
};
use serde::Serialize;
use std::time::Duration;

const KEY_BITS: usize = 2048;
const DEADLINE: Duration = Duration::from_secs(90);
/// Apple's developer error for "maximum number of certificates reached".
const MAX_CERTS: i64 = 7460;

/// A key held only in memory, with the certificate Apple issued for it.
#[derive(Clone)]
pub struct Identity {
    pub key: RsaPrivateKey,
    pub certificate: Vec<u8>,
}

#[derive(Clone, Serialize, Debug)]
pub struct Outcome {
    /// True when an existing certificate for this key was reused and nothing was written.
    pub reused: bool,
    pub expires: Option<String>,
    /// Active development certificates on the team, as Apple reported them.
    pub active: usize,
    pub message: String,
}

pub fn refusal(acknowledged: bool, team_selected: bool, signed_in: bool) -> Option<&'static str> {
    if !signed_in {
        return Some("Sign in to Apple before requesting a signing certificate.");
    }
    if !team_selected {
        return Some("Select the signing team the certificate belongs to.");
    }
    if !acknowledged {
        return Some(
            "Requesting a development certificate uses one of the team's few active certificate slots. Orbiter never revokes a certificate: revoking one would invalidate every app already signed with it, including apps Orbiter did not produce. Acknowledge before continuing.",
        );
    }
    None
}

/// Encode a key for the Keychain. PKCS#8 DER: the same encoding Apple's own tools use.
pub fn encode_key(key: &RsaPrivateKey) -> Result<zeroize::Zeroizing<Vec<u8>>, String> {
    key.to_pkcs8_der()
        .map(|document| zeroize::Zeroizing::new(document.as_bytes().to_vec()))
        .map_err(|_| "The signing key could not be encoded for storage.".to_string())
}

/// Decode a key read back from the Keychain.
pub fn decode_key(bytes: &[u8]) -> Result<RsaPrivateKey, String> {
    use rsa::pkcs8::DecodePrivateKey;
    RsaPrivateKey::from_pkcs8_der(bytes)
        .map_err(|_| "The stored signing key could not be read and must be replaced.".to_string())
}

/// Generate a signing key on this Mac. Blocking and CPU-bound, so callers run it off the runtime.
pub fn generate_key() -> Result<RsaPrivateKey, String> {
    RsaPrivateKey::new(&mut rand::rng(), KEY_BITS)
        .map_err(|_| "A signing key could not be generated on this Mac.".to_string())
}

/// PKCS#10 request for `key`. The subject carries no account, device, or company information.
pub fn certificate_request(key: &RsaPrivateKey) -> Result<String, String> {
    let pem = key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|_| "The signing key could not be encoded.".to_string())?;
    let pair = rcgen::KeyPair::from_pkcs8_pem_and_sign_algo(&pem, &rcgen::PKCS_RSA_SHA256)
        .map_err(|_| "The signing key could not be prepared for a request.".to_string())?;
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new())
        .map_err(|_| "The certificate request could not be prepared.".to_string())?;
    let mut name = rcgen::DistinguishedName::new();
    name.push(rcgen::DnType::CommonName, "Orbiter");
    params.distinguished_name = name;
    params
        .serialize_request(&pair)
        .and_then(|request| request.pem())
        .map_err(|_| "The certificate request could not be built.".to_string())
}

/// Find a certificate Apple already holds for this key, so a rerun writes nothing.
fn matching(key: &RsaPrivateKey, certificates: &[DevelopmentCertificate]) -> Option<Vec<u8>> {
    let ours = key.to_public_key().to_pkcs1_der().ok()?;
    certificates.iter().find_map(|certificate| {
        let content = certificate.cert_content.as_ref()?.as_ref();
        // Apple returns the issued certificate; match on the key it certifies, not on its name.
        contains(content, ours.as_bytes()).then(|| content.to_vec())
    })
}

/// Whether `haystack` contains `needle`. The issued certificate embeds the requested public key.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn team(team_id: &str) -> DeveloperTeam {
    DeveloperTeam {
        name: None,
        team_id: team_id.to_string(),
        r#type: None,
        status: None,
        memberships: Vec::new(),
    }
}

fn limit_reached(error: &rootcause::Report) -> bool {
    error.iter_reports().any(|cause| {
        matches!(
            cause.downcast_current_context::<isideload::SideloadError>(),
            Some(isideload::SideloadError::DeveloperError(MAX_CERTS, _))
        )
    })
}

/// Reuse or obtain a development certificate for `key`.
pub async fn ensure(
    session: &mut DeveloperSession,
    team_id: &str,
    machine_name: &str,
    key: &RsaPrivateKey,
) -> Result<(Identity, Outcome), String> {
    let team = team(team_id);
    let existing = tokio::time::timeout(DEADLINE, session.list_ios_certs(&team))
        .await
        .map_err(|_| "Listing the team's certificates timed out.".to_string())?
        .map_err(|error| {
            format!(
                "The team's certificates could not be listed. {}",
                isideload::redacted_auth_error(&error)
            )
        })?;
    if let Some(certificate) = matching(key, &existing) {
        return Ok((
            Identity {
                key: key.clone(),
                certificate,
            },
            Outcome {
                reused: true,
                expires: None,
                active: existing.len(),
                message: "An existing certificate for this Mac's signing key was reused. Nothing was written.".into(),
            },
        ));
    }
    let request = certificate_request(key)?;
    let submitted = tokio::time::timeout(
        DEADLINE,
        session.submit_development_csr(&team, request, machine_name.to_string(), None),
    )
    .await
    .map_err(|_| "The certificate request timed out. Check developer.apple.com before retrying: it may still have been issued.".to_string())?;
    let submitted = match submitted {
        Ok(submitted) => submitted,
        Err(error) if limit_reached(&error) => {
            let held = match existing.len() {
                1 => "one active development certificate".to_string(),
                other => format!("{other} active development certificates"),
            };
            return Err(format!(
                "Apple refused the request: this team already holds {held}, which is its maximum. Revoke one at developer.apple.com if it is no longer in use — Orbiter will not revoke it, because that invalidates every app already signed with it."
            ));
        }
        Err(error) => {
            return Err(format!(
                "Apple did not issue a certificate. {}",
                isideload::redacted_auth_error(&error)
            ));
        }
    };
    let issued = tokio::time::timeout(DEADLINE, session.list_ios_certs(&team))
        .await
        .map_err(|_| {
            "Reading the issued certificate timed out. Check developer.apple.com before retrying."
                .to_string()
        })?
        .map_err(|error| {
            format!(
                "The issued certificate could not be read back. {}",
                isideload::redacted_auth_error(&error)
            )
        })?;
    let certificate = issued
        .iter()
        .find(|candidate| candidate.certificate_id.as_deref() == Some(&submitted.cert_request_id))
        .and_then(|candidate| candidate.cert_content.as_ref())
        .map(|content| content.as_ref().to_vec())
        .ok_or("Apple accepted the request but did not return the certificate. Check developer.apple.com before requesting another.")?;
    let expires = issued
        .iter()
        .find(|candidate| candidate.certificate_id.as_deref() == Some(&submitted.cert_request_id))
        .and_then(|candidate| candidate.expiration_date.as_ref())
        .map(|date| date.to_xml_format());
    Ok((
        Identity {
            key: key.clone(),
            certificate,
        },
        Outcome {
            reused: false,
            expires,
            active: issued.len(),
            message: "A development certificate was issued for this Mac's signing key. The private key stays on this Mac and is not saved: restarting Orbiter needs a new certificate.".into(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::traits::PublicKeyParts;

    #[test]
    fn a_certificate_request_is_refused_until_its_cost_is_acknowledged() {
        assert!(refusal(true, true, false).is_some_and(|m| m.contains("Sign in")));
        assert!(refusal(true, false, true).is_some_and(|m| m.contains("Select the signing team")));
        let unacknowledged = refusal(false, true, true).expect("acknowledgement required");
        assert!(unacknowledged.contains("never revokes"));
        assert!(refusal(true, true, true).is_none());
    }

    #[test]
    fn the_request_carries_this_key_and_no_account_or_device_information() {
        // Generating a real 2048-bit key is slow but this is the only place it is exercised.
        let key = generate_key().expect("key");
        let pem = certificate_request(&key).expect("request");
        assert!(pem.starts_with("-----BEGIN CERTIFICATE REQUEST-----"));
        let body = pem.replace('\n', "");
        for leaked in ["@", "iPhone", "UDID", "Apple ID"] {
            assert!(!body.contains(leaked));
        }
        assert_eq!(key.size(), KEY_BITS / 8);
    }

    #[test]
    fn an_existing_certificate_is_matched_by_the_key_it_certifies() {
        let key = generate_key().expect("key");
        let public = key.to_public_key().to_pkcs1_der().expect("public key");
        let certificate = |content: Option<Vec<u8>>| DevelopmentCertificate {
            name: None,
            certificate_id: None,
            serial_number: None,
            machine_id: None,
            machine_name: Some("Another Mac".into()),
            cert_content: content.map(Into::into),
            certificate_platform: None,
            certificate_type: None,
            status: None,
            status_code: None,
            expiration_date: None,
        };
        // A certificate for somebody else's key is not ours, whatever it is named.
        assert!(matching(&key, &[certificate(Some(vec![9; 512])), certificate(None)]).is_none());
        let mut issued = vec![1, 2, 3];
        issued.extend_from_slice(public.as_bytes());
        assert_eq!(
            matching(&key, &[certificate(Some(issued.clone()))]),
            Some(issued)
        );
    }
}
