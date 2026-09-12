//! Portal writes on the signed-in account's team. Registration is the first operation in Orbiter
//! that changes state at Apple rather than only reading it, so it is explicit, acknowledged, and
//! reports only what it did — never the device identifier it sent.
use isideload::dev::{
    app_ids::{AppId, AppIdsApi},
    developer_session::DeveloperSession,
    device_type::DeveloperDeviceType,
    devices::DevicesApi,
    teams::DeveloperTeam,
};
use serde::Serialize;
use std::time::Duration;

/// Apple limits a free Personal Team to three registered devices.
pub const PERSONAL_DEVICE_LIMIT: usize = 3;
const DEADLINE: Duration = Duration::from_secs(60);

#[derive(Clone, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Registration {
    /// The iPhone was already registered on this team; nothing was written.
    AlreadyRegistered,
    /// The iPhone was registered by this operation.
    Registered,
}

#[derive(Clone, Serialize, Debug)]
pub struct Outcome {
    pub registration: Registration,
    /// Devices on the team after the operation, as Apple reported them.
    pub team_devices: usize,
    pub message: String,
}

/// Reasons a registration is refused before any request is made.
pub fn refusal(acknowledged: bool, team_selected: bool, signed_in: bool) -> Option<&'static str> {
    if !signed_in {
        return Some("Sign in to Apple before registering an iPhone.");
    }
    if !team_selected {
        return Some("Select the signing team that should register this iPhone.");
    }
    if !acknowledged {
        return Some(
            "Registering writes this iPhone to the selected team at Apple. A free personal team allows three devices; a paid team consumes one of its 100 slots for the membership year, and removing the device later does not return that slot. Acknowledge before continuing.",
        );
    }
    None
}

/// Message for a team that is already at the Personal Team device limit.
pub fn at_personal_limit(devices: usize) -> Option<String> {
    (devices >= PERSONAL_DEVICE_LIMIT).then(|| {
        format!(
            "This personal team already has {devices} of its {PERSONAL_DEVICE_LIMIT} devices registered. Remove a device from the account at developer.apple.com before registering another."
        )
    })
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

/// Register `udid` on `team_id` if it is not registered already.
///
/// The identifier and the device name are sent to Apple because registration cannot happen
/// without them; neither is returned, logged, or persisted by Orbiter. `free` narrows the
/// pre-checks to the Personal Team allowance.
pub async fn register(
    session: &mut DeveloperSession,
    team_id: &str,
    udid: &str,
    name: &str,
    free: bool,
) -> Result<Outcome, String> {
    let team = team(team_id);
    let existing = tokio::time::timeout(
        DEADLINE,
        session.list_devices(&team, DeveloperDeviceType::Ios),
    )
    .await
    .map_err(|_| {
        "Listing the team's devices timed out. Check your connection and try again.".to_string()
    })?
    .map_err(|error| {
        format!(
            "The team's devices could not be listed. {}",
            isideload::redacted_auth_error(&error)
        )
    })?;
    if existing.iter().any(|device| device.device_number == udid) {
        return Ok(Outcome {
            registration: Registration::AlreadyRegistered,
            team_devices: existing.len(),
            message: "This iPhone is already registered on the selected team. Nothing was written."
                .into(),
        });
    }
    if free && let Some(refusal) = at_personal_limit(existing.len()) {
        return Err(refusal);
    }
    tokio::time::timeout(DEADLINE, session.add_device(&team, name, udid, DeveloperDeviceType::Ios))
        .await
        .map_err(|_| "Registering the iPhone timed out. Check the account at developer.apple.com before trying again: the request may still have been applied.".to_string())?
        .map_err(|error| {
            format!(
                "Apple did not register this iPhone. {}",
                isideload::redacted_auth_error(&error)
            )
        })?;
    Ok(Outcome {
        registration: Registration::Registered,
        team_devices: existing.len() + 1,
        message: "This iPhone is now registered on the selected team.".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_refused_until_it_is_acknowledged_on_a_chosen_team() {
        assert!(refusal(true, true, false).is_some_and(|m| m.contains("Sign in")));
        assert!(refusal(true, false, true).is_some_and(|m| m.contains("Select the signing team")));
        let unacknowledged = refusal(false, true, true).expect("acknowledgement required");
        // The consequence is stated before the write, not after it.
        assert!(unacknowledged.contains("three devices"));
        assert!(unacknowledged.contains("membership year"));
        assert!(refusal(true, true, true).is_none());
    }

    #[test]
    fn the_personal_team_device_limit_is_reported_before_writing() {
        assert!(at_personal_limit(PERSONAL_DEVICE_LIMIT - 1).is_none());
        let full = at_personal_limit(PERSONAL_DEVICE_LIMIT).expect("limit reached");
        assert!(full.contains("developer.apple.com"));
    }

    #[test]
    fn provisioning_is_refused_until_the_identifier_budget_is_acknowledged() {
        assert!(app_id_refusal(true, true, false).is_some_and(|m| m.contains("Sign in")));
        assert!(
            app_id_refusal(true, false, true)
                .is_some_and(|m| m.contains("Select the signing team"))
        );
        let unacknowledged = app_id_refusal(false, true, true).expect("acknowledgement required");
        assert!(unacknowledged.contains("ten identifiers per seven days"));
        assert!(app_id_refusal(true, true, true).is_none());
    }

    #[test]
    fn only_capabilities_apple_enabled_are_reported_and_unknown_keys_are_not_invented() {
        let mut features = plist::Dictionary::new();
        features.insert("APG3427HIY".into(), plist::Value::Boolean(true));
        features.insert("SOMETHINGNEW".into(), plist::Value::Boolean(true));
        // A capability Apple reports as disabled is not a capability the build has.
        features.insert("OM633U5T5G".into(), plist::Value::Boolean(false));
        let app_id = AppId {
            app_id_id: "id".into(),
            identifier: "com.example.app".into(),
            name: "App".into(),
            features,
            expiration_date: None,
        };
        let capabilities = enabled_capabilities(&app_id);
        assert!(capabilities.contains(&"App groups".to_string()));
        assert!(capabilities.contains(&"an unnamed capability".to_string()));
        assert!(!capabilities.contains(&"Apple Pay".to_string()));
    }

    #[test]
    fn the_team_request_carries_only_the_team_identifier() {
        let team = team("T8B3X5UL5W");
        assert_eq!(team.team_id, "T8B3X5UL5W");
        assert!(team.name.is_none() && team.status.is_none() && team.memberships.is_empty());
    }
}

/// Reasons provisioning is refused before any identifier is registered.
pub fn app_id_refusal(
    acknowledged: bool,
    team_selected: bool,
    signed_in: bool,
) -> Option<&'static str> {
    if !signed_in {
        return Some("Sign in to Apple before provisioning.");
    }
    if !team_selected {
        return Some("Select the signing team to provision on.");
    }
    if !acknowledged {
        return Some(
            "Provisioning registers new app identifiers on the team and downloads their profiles. A free personal team may register only ten identifiers per seven days, and an identifier cannot be reused by another team afterwards. Acknowledge before continuing.",
        );
    }
    None
}

/// Apple's opaque feature identifiers, for the few whose meaning is documented by use.
fn feature_label(key: &str) -> &'static str {
    match key {
        "APG3427HIY" => "App groups",
        "push" | "APNS" => "Push notifications",
        "IAD53UNK2F" => "Associated domains",
        "OM633U5T5G" => "Apple Pay",
        _ => "an unnamed capability",
    }
}

#[derive(Clone, Serialize, Debug)]
pub struct AppIdOutcome {
    pub identifier: String,
    pub created: bool,
    /// Capabilities Apple actually enabled on this App ID, labelled where the key is known.
    pub capabilities: Vec<String>,
    /// App IDs this team may still register in the current seven-day window, when Apple says.
    pub remaining: Option<i64>,
}

#[derive(Clone, Serialize, Debug)]
pub struct ProfileOutcome {
    pub identifier: String,
    pub expires: String,
    /// The same moment as `expires`, as seconds since the epoch, so nothing downstream has to
    /// parse a date string back out of a human-readable one.
    pub expires_unix: i64,
    pub uuid: String,
    /// Profile bytes for the signer. Not serialised into the interface.
    #[serde(skip)]
    pub encoded: Vec<u8>,
}

/// Register `identifier` on the team, or reuse Apple's existing App ID for it.
pub async fn ensure_app_id(
    session: &mut DeveloperSession,
    team_id: &str,
    identifier: &str,
    name: &str,
) -> Result<(AppId, AppIdOutcome), String> {
    let team = team(team_id);
    let listed = tokio::time::timeout(
        DEADLINE,
        session.list_app_ids(&team, DeveloperDeviceType::Ios),
    )
    .await
    .map_err(|_| "Listing the team's app identifiers timed out.".to_string())?
    .map_err(|error| {
        format!(
            "The team's app identifiers could not be listed. {}",
            isideload::redacted_auth_error(&error)
        )
    })?;
    if let Some(existing) = listed
        .app_ids
        .iter()
        .find(|candidate| candidate.identifier == identifier)
    {
        let outcome = AppIdOutcome {
            identifier: identifier.to_string(),
            created: false,
            capabilities: enabled_capabilities(existing),
            remaining: listed.available_quantity,
        };
        return Ok((existing.clone(), outcome));
    }
    if listed.available_quantity == Some(0) {
        return Err(format!(
            "This team cannot register another app identifier right now: Apple reports none of its {} remaining in the current seven-day window. Wait for the window to pass, or remove an unused identifier at developer.apple.com.",
            listed
                .max_quantity
                .map(|max| max.to_string())
                .unwrap_or_else(|| "allowed".into())
        ));
    }
    let created = tokio::time::timeout(
        DEADLINE,
        session.add_app_id(&team, name, identifier, DeveloperDeviceType::Ios),
    )
    .await
    .map_err(|_| "Registering the app identifier timed out. Check developer.apple.com before retrying: it may still have been registered.".to_string())?
    .map_err(|error| {
        format!(
            "Apple did not register the app identifier {identifier}. {}",
            isideload::redacted_auth_error(&error)
        )
    })?;
    let outcome = AppIdOutcome {
        identifier: identifier.to_string(),
        created: true,
        capabilities: enabled_capabilities(&created),
        remaining: listed.available_quantity.map(|left| left - 1),
    };
    Ok((created, outcome))
}

/// Capabilities Apple reports as enabled, as labels. Apple decides these, not the plan.
fn enabled_capabilities(app_id: &AppId) -> Vec<String> {
    let mut labels: Vec<String> = app_id
        .features
        .iter()
        .filter(|(_, value)| value.as_boolean() == Some(true))
        .map(|(key, _)| feature_label(key).to_string())
        .collect();
    labels.sort();
    labels.dedup();
    labels
}

/// The team provisioning profile for an App ID: it authorises the team's registered devices and,
/// on a free personal team, expires in seven days.
pub async fn fetch_profile(
    session: &mut DeveloperSession,
    team_id: &str,
    app_id: &AppId,
) -> Result<ProfileOutcome, String> {
    let team = team(team_id);
    let profile = tokio::time::timeout(
        DEADLINE,
        session.download_team_provisioning_profile(&team, app_id, DeveloperDeviceType::Ios),
    )
    .await
    .map_err(|_| "Downloading the provisioning profile timed out.".to_string())?
    .map_err(|error| {
        format!(
            "Apple did not return a provisioning profile for {}. {}",
            app_id.identifier,
            isideload::redacted_auth_error(&error)
        )
    })?;
    Ok(ProfileOutcome {
        identifier: app_id.identifier.clone(),
        expires: profile.date_expire.to_xml_format(),
        expires_unix: crate::renewal::now_unix(std::time::SystemTime::from(profile.date_expire)),
        uuid: profile.uuid,
        encoded: profile.encoded_profile.as_ref().to_vec(),
    })
}
