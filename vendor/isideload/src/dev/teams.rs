use crate::dev::{
    developer_session::DeveloperSession,
    device_type::{DeveloperDeviceType::*, dev_url},
};
use rootcause::prelude::*;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeveloperTeam {
    pub name: Option<String>,
    pub team_id: String,
    pub r#type: Option<String>,
    pub status: Option<String>,
    /// Orbiter: `type` alone does not separate a free Personal Team from a paid individual
    /// membership, and that difference decides seven-day profiles and capability removals.
    #[serde(default)]
    pub memberships: Vec<DeveloperMembership>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeveloperMembership {
    pub name: Option<String>,
    pub status: Option<String>,
    pub platform: Option<String>,
}

#[cfg_attr(feature = "wasm", async_trait::async_trait(?Send))]
#[cfg_attr(not(feature = "wasm"), async_trait::async_trait)]
pub trait TeamsApi {
    fn developer_session(&mut self) -> &mut DeveloperSession;

    async fn list_teams(&mut self) -> Result<Vec<DeveloperTeam>, Report> {
        let response: Vec<DeveloperTeam> = self
            .developer_session()
            .send_dev_request(&dev_url("listTeams", Any), None, "teams")
            .await
            .context("Failed to list developer teams")?;

        Ok(response)
    }
}

impl TeamsApi for DeveloperSession {
    fn developer_session(&mut self) -> &mut DeveloperSession {
        self
    }
}
