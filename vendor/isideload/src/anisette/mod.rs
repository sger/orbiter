// Orbiter: remote providers are deliberately excluded.

use crate::auth::grandslam::GrandSlam;
use plist::Dictionary;
use plist_macro::plist;
use reqwest::header::HeaderMap;
use rootcause::prelude::*;
use serde::Deserialize;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use tracing::warn;
use web_time::SystemTime;

#[derive(Deserialize, Debug, Clone)]
pub struct AnisetteClientInfo {
    pub client_info: String,
    pub user_agent: String,
}

#[derive(Debug, Clone)]
pub struct AnisetteData {
    machine_id: String,
    one_time_password: String,
    pub routing_info: String,
    _device_description: String,
    device_unique_identifier: String,
    _local_user_id: String,
    generated_at: SystemTime,
    /// True when the OS generated this material locally, so every request can cheaply take its own.
    single_use: bool,
}

// Some headers don't seem to be required. I guess not including them is technically more efficient soooo
impl AnisetteData {
    /// Construct data supplied by a local OS provider. No network or persistence.
    pub fn from_local(machine_id: String, one_time_password: String,
        device_unique_identifier: String, device_description: String, local_user_id: String) -> Self {
        Self { machine_id, one_time_password, routing_info: "0".into(),
            device_unique_identifier, _device_description: device_description,
            _local_user_id: local_user_id, generated_at: SystemTime::now(), single_use: true }
    }

    pub fn get_headers(&self) -> HashMap<String, String> {
        HashMap::from_iter([
            ("X-Apple-I-Client-Time".into(), plist::Date::from(std::time::SystemTime::now()).to_xml_format()),
            ("X-Apple-I-TimeZone".into(), "UTC".into()),
            ("X-Apple-Locale".into(), "en_US".into()),
            ("X-Apple-I-MD-RINFO".into(), self.routing_info.clone()),
            ("X-Apple-I-MD-LU".into(), self._local_user_id.clone()),
            ("X-Mme-Device-Id".into(), self.device_unique_identifier.clone()),
            ("X-Apple-I-MD".into(), self.one_time_password.clone()),
            ("X-Apple-I-MD-M".into(), self.machine_id.clone()),
        ])
    }

    pub fn get_header_map(&self) -> Result<HeaderMap, Report> {
        let headers_map = self.get_headers();
        let mut header_map = HeaderMap::new();

        for (key, value) in headers_map {
            header_map.insert(
                reqwest::header::HeaderName::from_bytes(key.as_bytes())?,
                reqwest::header::HeaderValue::from_str(&value)?,
            );
        }

        Ok(header_map)
    }

    pub fn get_client_provided_data(&self) -> Dictionary {
        let headers = self.get_headers();

        let mut cpd = plist!(dict {
            "bootstrap": true,
            "icscrec": true,
            "loc": "en_US",
            "pbe": false,
            "prkgen": true,
            "svct": "iCloud"
        });

        for (key, value) in headers {
            cpd.insert(key.to_string(), plist::Value::String(value));
        }

        // HTTP headers are strings; the GrandSlam plist uses an integer here.
        cpd.insert("X-Apple-I-MD-RINFO".into(), plist::Value::Integer(0_u64.into()));
        cpd
    }

    pub fn needs_refresh(&self) -> bool {
        if self.single_use {
            // X-Apple-I-MD is a one-time password. Replaying one across requests is what a real
            // client never does, and local generation costs a single native call.
            return true;
        }
        let elapsed = self.generated_at.elapsed();
        match elapsed {
            Ok(elapsed) => elapsed.as_secs() > 60,
            Err(_) => {
                warn!("Unable to determine anisette data age, treating as expired");
                true
            }
        }
    }
}

#[cfg_attr(feature = "wasm", async_trait::async_trait(?Send))]
#[cfg_attr(not(feature = "wasm"), async_trait::async_trait)]
pub trait AnisetteProvider {
    async fn get_anisette_data(&self) -> Result<AnisetteData, Report>;

    async fn get_client_info(&self) -> Result<AnisetteClientInfo, Report>;

    async fn provision(&mut self, gs: Arc<GrandSlam>) -> Result<(), Report>;

    fn needs_provisioning(&self) -> Result<bool, Report>;
}

#[derive(Clone)]
pub struct AnisetteDataGenerator {
    provider: Arc<RwLock<dyn AnisetteProvider + Send + Sync>>,
    data: Option<Arc<AnisetteData>>,
}

impl AnisetteDataGenerator {
    pub fn new(provider: Arc<RwLock<dyn AnisetteProvider + Send + Sync>>) -> Self {
        AnisetteDataGenerator {
            provider,
            data: None,
        }
    }

    pub async fn get_anisette_data(
        &mut self,
        gs: Arc<GrandSlam>,
    ) -> Result<Arc<AnisetteData>, Report> {
        if let Some(data) = &self.data
            && !data.needs_refresh()
        {
            return Ok(data.clone());
        }

        // trying to avoid locking as write unless necessary to promote concurrency
        let provider = self.provider.read().await;

        if provider.needs_provisioning()? {
            drop(provider);
            let mut provider_write = self.provider.write().await;
            provider_write.provision(gs).await?;
            drop(provider_write);

            let provider = self.provider.read().await;
            let data = provider.get_anisette_data().await?;
            let arc_data = Arc::new(data);
            self.data = Some(arc_data.clone());
            Ok(arc_data)
        } else {
            let data = provider.get_anisette_data().await?;
            let arc_data = Arc::new(data);
            self.data = Some(arc_data.clone());
            Ok(arc_data)
        }
    }

    pub async fn get_client_info(&self) -> Result<AnisetteClientInfo, Report> {
        let provider = self.provider.read().await;
        provider.get_client_info().await
    }
}
