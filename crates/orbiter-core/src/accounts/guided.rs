//! Read-only account assessment for a reviewed signing sequence.
use super::{Accounts, UNAVAILABLE};
use crate::{
    domain::errors::{ErrorCode, OperationError, OperationResult},
    plan::{Target, TeamKind},
};

impl Accounts {
    /// Reserve the account for an entire preparation sequence, including local signing.
    /// Refuses concurrent mutations; the owned guard also survives a dropped IPC subscriber.
    pub(crate) fn operation(&self) -> OperationResult<tokio::sync::OwnedMutexGuard<()>> {
        self.1.clone().try_lock_owned().map_err(|_| {
            OperationError::operation_in_progress("Another account operation is running.")
        })
    }

    /// Read a session generation and known supported team without contacting Apple.
    /// Unknown membership and enterprise distribution are not signing targets in this flow.
    pub(crate) fn signing_context(&self) -> OperationResult<(String, Target)> {
        let mut inner = self
            .0
            .lock()
            .map_err(|_| OperationError::internal(UNAVAILABLE))?;
        inner.expire();
        let team = inner
            .view
            .teams
            .iter()
            .find(|team| Some(&team.id) == inner.view.selected_team.as_ref())
            .filter(|_| inner.session.is_some())
            .ok_or_else(|| {
                OperationError::new(
                    ErrorCode::AuthenticationRequired,
                    "Sign in and select a signing team.",
                )
            })?;
        let free = team.free.ok_or_else(|| {
            OperationError::new(
                ErrorCode::InvalidRequest,
                "Apple has not established this team's membership. Refresh teams before signing.",
            )
        })?;
        if team
            .membership
            .as_deref()
            .is_some_and(|label| label.to_ascii_lowercase().contains("enterprise"))
        {
            return Err(OperationError::new(
                ErrorCode::InvalidRequest,
                "Select a Personal Team or Apple Developer Program team. Enterprise distribution is not supported in this flow.",
            ));
        }
        Ok((
            inner.generation.clone(),
            Target {
                team_id: team.id.clone(),
                kind: if free {
                    TeamKind::Personal
                } else {
                    TeamKind::Paid
                },
                watch: Default::default(),
            },
        ))
    }

    /// List devices and certificates and load an existing local key, without any portal writes.
    /// Caller holds the operation guard. Returns whether registration/certificate creation is needed.
    /// Read failures stop planning; no key is generated and no resource is silently replaced.
    pub(crate) async fn signing_resources(&self, udid: &str) -> OperationResult<(bool, bool)> {
        use isideload::dev::{
            device_type::DeveloperDeviceType, devices::DevicesApi, teams::DeveloperTeam,
        };
        let (_, target) = self.signing_context()?;
        let (mut developer, key, stored) = {
            let inner = self
                .0
                .lock()
                .map_err(|_| OperationError::internal(UNAVAILABLE))?;
            let session = inner.session.as_ref().ok_or("Sign in first.")?;
            (
                session.developer.clone(),
                session
                    .identity
                    .as_ref()
                    .map(|identity| identity.key.clone()),
                crate::keychain::account(
                    inner.view.account.as_deref().unwrap_or_default(),
                    &target.team_id,
                ),
            )
        };
        let team = DeveloperTeam {
            name: None,
            team_id: target.team_id.clone(),
            r#type: None,
            status: None,
            memberships: vec![],
        };
        let devices = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            developer.list_devices(&team, DeveloperDeviceType::Ios),
        )
        .await
        .map_err(|_| OperationError::internal("Listing registered devices timed out."))?
        .map_err(|_| {
            OperationError::internal(
                "Could not verify registered devices at Apple. Refresh preparation to retry.",
            )
        })?;
        let key = if key.is_some() {
            key
        } else {
            let bytes = tokio::task::spawn_blocking(move || crate::keychain::load(&stored))
                .await
                .map_err(|_| OperationError::internal("Reading the signing key stopped."))??;
            bytes
                .as_deref()
                .map(crate::certificates::decode_key)
                .transpose()?
        };
        let identity = crate::certificates::lookup(&mut developer, &target.team_id, key).await?;
        let certificate_needed = identity.is_none();
        let mut inner = self
            .0
            .lock()
            .map_err(|_| OperationError::internal(UNAVAILABLE))?;
        if let Some(session) = inner.session.as_mut() {
            session.identity = identity;
        }
        Ok((
            !devices.iter().any(|device| device.device_number == udid),
            certificate_needed,
        ))
    }
    /// Check cached profiles against the exact signing certificate, identifiers, device and clock.
    /// Local only; unknown expiration, malformed profiles or missing certificate bindings require
    /// fresh provisioning. Watch profiles target a separate device and are not reused here.
    pub(crate) fn profiles_ready(
        &self,
        plan: &crate::plan::Plan,
        udid: &str,
    ) -> OperationResult<bool> {
        let inner = self
            .0
            .lock()
            .map_err(|_| OperationError::internal(UNAVAILABLE))?;
        let Some(identity) = inner
            .session
            .as_ref()
            .and_then(|session| session.identity.as_ref())
        else {
            return Ok(false);
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| OperationError::internal("System clock is unavailable."))?
            .as_secs() as i64;
        Ok(plan
            .bundles
            .iter()
            .filter(|bundle| bundle.consumes_app_id)
            .all(|bundle| {
                if bundle.kind == "Watch app" {
                    return false;
                }
                inner.profiles.iter().any(|profile| {
                    if profile.identifier != bundle.new_identifier
                        || profile.expires_unix <= now + 60
                    {
                        return false;
                    }
                    let Ok(dictionary) = crate::profile::dictionary(&profile.encoded) else {
                        return false;
                    };
                    profile_binding_matches(&dictionary, &identity.certificate, udid)
                })
            }))
    }
}

/// Require both the reviewed device and the current certificate in a cached profile.
/// Empty or malformed claims are not evidence that a profile can be reused.
fn profile_binding_matches(dictionary: &plist::Dictionary, certificate: &[u8], udid: &str) -> bool {
    !certificate.is_empty()
        && !udid.is_empty()
        && dictionary
            .get("ProvisionedDevices")
            .and_then(plist::Value::as_array)
            .is_some_and(|devices| devices.iter().any(|value| value.as_string() == Some(udid)))
        && dictionary
            .get("DeveloperCertificates")
            .and_then(plist::Value::as_array)
            .is_some_and(|certificates| {
                certificates
                    .iter()
                    .any(|value| value.as_data() == Some(certificate))
            })
}

#[cfg(test)]
/// Exercise the fail-closed membership gate independently of an Apple session.
mod tests {
    use super::*;
    #[test]
    /// A profile for the same name but a different phone or key is never reused.
    fn cached_profiles_require_device_and_certificate_identity() {
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert(
            "ProvisionedDevices".into(),
            plist::Value::Array(vec![plist::Value::String("phone-one".into())]),
        );
        dictionary.insert(
            "DeveloperCertificates".into(),
            plist::Value::Array(vec![plist::Value::Data(vec![1, 2, 3])]),
        );
        assert!(profile_binding_matches(
            &dictionary,
            &[1, 2, 3],
            "phone-one"
        ));
        assert!(!profile_binding_matches(
            &dictionary,
            &[1, 2, 3],
            "phone-two"
        ));
        assert!(!profile_binding_matches(
            &dictionary,
            &[4, 5, 6],
            "phone-one"
        ));
        assert!(!profile_binding_matches(&dictionary, &[], "phone-one"));
        dictionary.remove("DeveloperCertificates");
        assert!(!profile_binding_matches(
            &dictionary,
            &[1, 2, 3],
            "phone-one"
        ));
    }

    #[test]
    /// A missing account cannot be treated as a free or paid membership by default.
    fn signed_out_has_no_signing_context() {
        assert_eq!(
            Accounts::default().signing_context().err().unwrap().code,
            ErrorCode::AuthenticationRequired
        );
    }
}
