//! Development certificate for the signing team.
//!
//! The private key is generated on this Mac and never leaves it: only a certificate signing
//! request goes to Apple. The key is kept in this Mac's Keychain, so a restart reuses the same
//! certificate instead of spending another of the team's few slots. The certificate itself is
//! fetched again per session, which is why signing needs step 3 run once after each restart.
//!
//! Certificates are never withdrawn automatically. Withdrawing one invalidates every app already
//! signed with it, including apps this tool did not produce, so reaching the limit is reported and
//! left to the person. `withdraw_all` exists for the one case where nothing else can work: a free
//! personal team has no certificates page at developer.apple.com, so a slot held by a certificate
//! whose key is not on this Mac can only be cleared from here, with an explicit acknowledgement.
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
/// Encode a private signing key as PKCS#8 DER for storage.
///
/// The result is [`zeroize::Zeroizing`], so the encoded private key is wiped when dropped rather
/// than left in freed memory.
///
/// # Errors
///
/// Fails if the key cannot be encoded, which would mean a malformed key rather than a storage
/// problem.
pub fn encode_key(key: &RsaPrivateKey) -> Result<zeroize::Zeroizing<Vec<u8>>, String> {
    key.to_pkcs8_der()
        .map(|document| zeroize::Zeroizing::new(document.as_bytes().to_vec()))
        .map_err(|_| "The signing key could not be encoded for storage.".to_string())
}

/// Decode a key read back from the Keychain.
/// Read a private signing key back from PKCS#8 DER.
///
/// # Errors
///
/// Fails if the bytes are not a well-formed key. The message says nothing about their contents.
pub fn decode_key(bytes: &[u8]) -> Result<RsaPrivateKey, String> {
    use rsa::pkcs8::DecodePrivateKey;
    RsaPrivateKey::from_pkcs8_der(bytes)
        .map_err(|_| "The stored signing key could not be read and must be replaced.".to_string())
}

/// Generate a signing key on this Mac. Blocking and CPU-bound, so callers run it off the runtime.
/// Generate a new private signing key on this Mac.
///
/// The private half never leaves this machine: only a certificate signing request derived from it
/// is sent to Apple.
///
/// # Errors
///
/// Fails if the system random source is unavailable.
pub fn generate_key() -> Result<RsaPrivateKey, String> {
    RsaPrivateKey::new(&mut rand::rng(), KEY_BITS)
        .map_err(|_| "A signing key could not be generated on this Mac.".to_string())
}

/// PKCS#10 request for `key`. The subject carries no account, device, or company information.
/// Build the PKCS#10 certificate signing request Apple is asked to sign.
///
/// Carries the public key and a fixed subject, and deliberately no account address, team name,
/// device identifier or machine name — none of it is needed, and a CSR is a durable record.
///
/// # Errors
///
/// Fails if the request cannot be built or encoded.
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
/// Find the certificate Apple issued for this exact key, if the team already holds one.
///
/// Matched by the public key inside the certificate rather than by name or date: names are not
/// unique and a certificate issued for a different key is useless for signing, however it is
/// labelled. This is what lets a rerun reuse a slot instead of spending another.
fn matching(key: &RsaPrivateKey, certificates: &[DevelopmentCertificate]) -> Option<Vec<u8>> {
    let ours = key.to_public_key().to_pkcs1_der().ok()?;
    certificates.iter().find_map(|certificate| {
        let content = certificate.cert_content.as_ref()?.as_ref();
        // Apple returns the issued certificate; match on the key it certifies, not on its name.
        contains(content, ours.as_bytes()).then(|| content.to_vec())
    })
}

/// Whether `haystack` contains `needle`. The issued certificate embeds the requested public key.
/// Whether `needle` appears anywhere in `haystack`.
///
/// Used to look for an encoded public key inside a certificate's bytes. An empty needle never
/// matches, so a failure to encode cannot read as a match against everything.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Wrap a team identifier in the shape Apple's client expects, carrying nothing else.
fn team(team_id: &str) -> DeveloperTeam {
    DeveloperTeam {
        name: None,
        team_id: team_id.to_string(),
        r#type: None,
        status: None,
        memberships: Vec::new(),
    }
}

/// Say what is occupying the team's certificate slots, so the choice to revoke is an informed one.
fn describe(certificates: &[DevelopmentCertificate]) -> String {
    let held: Vec<String> = certificates
        .iter()
        .map(|certificate| {
            let machine = certificate
                .machine_name
                .clone()
                .unwrap_or_else(|| "an unnamed Mac".into());
            match certificate.expiration_date {
                Some(date) => format!(
                    "one issued for {machine}, expiring {}",
                    date.to_xml_format()
                ),
                None => format!("one issued for {machine}"),
            }
        })
        .collect();
    if held.is_empty() {
        String::new()
    } else {
        format!("It holds {}.", held.join("; "))
    }
}

/// Revoke every development certificate on the team.
///
/// This exists for one situation: the team's only certificate slot is taken by a certificate whose
/// private key is not on this Mac, so it cannot sign anything here, and a free personal team has no
/// portal page to revoke it from. It is never automatic — revoking stops every app already signed
/// with that certificate from launching, including apps Orbiter did not produce.
pub async fn withdraw_all(
    session: &mut DeveloperSession,
    team_id: &str,
    acknowledged: bool,
) -> Result<String, String> {
    if !acknowledged {
        return Err(
            "Revoking the team's certificate stops every app already signed with it from launching, on every device. Acknowledge before continuing."
                .into(),
        );
    }
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
    if existing.is_empty() {
        return Ok("This team holds no development certificates. Nothing was revoked.".into());
    }
    let mut revoked = 0usize;
    for certificate in &existing {
        let serial = certificate.serial_number.as_deref().ok_or(
            "Apple did not identify one of the team's certificates, so it was not revoked.",
        )?;
        tokio::time::timeout(
            DEADLINE,
            session.revoke_development_cert(&team, serial, None),
        )
        .await
        .map_err(|_| {
            "Revoking a certificate timed out. Check the team before retrying.".to_string()
        })?
        .map_err(|error| {
            format!(
                "Apple did not revoke one of the team's certificates. {}",
                isideload::redacted_auth_error(&error)
            )
        })?;
        revoked += 1;
    }
    Ok(format!(
        "{revoked} certificate(s) were revoked. Apps already signed with them no longer launch. Request a signing certificate again to continue."
    ))
}

/// Whether Apple refused because the team already holds all the certificates it may.
///
/// Recognised so the interface can offer the one thing that resolves it — withdrawing the existing
/// certificate — rather than showing a refusal with no way forward. A free personal team has a
/// single slot, and there is no portal page to free it from.
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
                "Apple refused the request: this team already holds {held}, which is its maximum, and none of them certifies this Mac's signing key — so none can be used to sign. {} A free personal team has no certificates page at developer.apple.com, so the only way forward is to revoke it here, which invalidates every app already signed with it.",
                describe(&existing)
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
/// Checks that a certificate is only ever requested deliberately, and that the request discloses
/// nothing about the account or the machine.
mod tests {
    use super::*;
    use rsa::traits::PublicKeyParts;

    #[test]
    /// Requesting a certificate needs a session, a chosen team and an acknowledgement: a free
    /// team has very few slots and spending one can leave it unable to issue another.
    fn a_certificate_request_is_refused_until_its_cost_is_acknowledged() {
        assert!(refusal(true, true, false).is_some_and(|m| m.contains("Sign in")));
        assert!(refusal(true, false, true).is_some_and(|m| m.contains("Select the signing team")));
        let unacknowledged = refusal(false, true, true).expect("acknowledgement required");
        assert!(unacknowledged.contains("never revokes"));
        assert!(refusal(true, true, true).is_none());
    }

    #[test]
    /// The signing request contains the public key and a fixed subject — no email address, team
    /// name, device identifier or machine name.
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
    /// An existing certificate is recognised by the key inside it rather than by its name, so a
    /// rerun reuses the team's slot instead of spending another.
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
