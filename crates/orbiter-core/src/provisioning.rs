//! Portal writes on the signed-in account's team. Registration is the first operation in Orbiter
//! that changes state at Apple rather than only reading it, so it is explicit, acknowledged, and
//! reports only what it did — never the device identifier it sent.
use isideload::dev::{
    developer_session::DeveloperSession, device_type::DeveloperDeviceType, devices::DevicesApi,
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
    fn the_team_request_carries_only_the_team_identifier() {
        let team = team("T8B3X5UL5W");
        assert_eq!(team.team_id, "T8B3X5UL5W");
        assert!(team.name.is_none() && team.status.is_none() && team.memberships.is_empty());
    }
}
