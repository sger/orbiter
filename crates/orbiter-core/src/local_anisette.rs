//! Local macOS authentication support. No support server, subprocess, or secret export.
use isideload::{
    anisette::{AnisetteClientInfo, AnisetteData, AnisetteProvider},
    auth::grandslam::GrandSlam,
};
use rootcause::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
struct Material {
    otp: String,
    machine: String,
    identifier: String,
    description: String,
    local_user: String,
    #[serde(default)]
    cfnetwork: String,
    #[serde(default)]
    darwin: String,
}
impl Drop for Material {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.otp.zeroize();
        self.machine.zeroize();
        self.identifier.zeroize();
        self.local_user.zeroize();
    }
}
#[derive(Serialize)]
pub struct Status {
    pub available: bool,
    pub message: String,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn orbiter_local_anisette(buffer: *mut u8, capacity: usize, written: *mut usize) -> i32;
}
async fn read() -> Result<Material, String> {
    #[cfg(not(target_os = "macos"))]
    return Err("Local authentication is currently implemented for macOS only. Remote fallback is disabled.".into());
    #[cfg(target_os = "macos")]
    {
        static GATE: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
            std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(1)));
        let operation = async {
            let permit = GATE
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| "Local authentication worker is unavailable.")?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let mut bytes = zeroize::Zeroizing::new(vec![0_u8; 65_536]);
                let mut written = 0_usize;
                // SAFETY: Native bridge writes within this live buffer and catches Objective-C exceptions.
                let result = unsafe { orbiter_local_anisette(bytes.as_mut_ptr(), bytes.len(), &mut written) };
                if result != 0 || written == 0 || written > bytes.len() {
                    return Err("macOS could not provide local authentication data. Check your macOS Apple account setup; this OS version may require an updated local adapter. No remote fallback was used.".to_string());
                }
                serde_json::from_slice(&bytes[..written]).map_err(|_| "Invalid local authentication response.".into())
            }).await.map_err(|_| "Local authentication worker stopped.".to_string())?
        };
        tokio::time::timeout(std::time::Duration::from_secs(15), operation)
            .await
            .map_err(|_| {
                "Local authentication timed out. No remote fallback was used.".to_string()
            })?
    }
}

pub async fn check() -> Status {
    match read().await {
        Ok(_) => Status { available: true, message: "Local macOS authentication support is available. Apple account sign-in has not yet been verified.".into() },
        Err(message) => Status { available: false, message },
    }
}

// Upstream isideload PR #11: GSA rejects the legacy Xcode client token
// before processing account credentials. Keep the local machine description.
//
// AKDevice already terminates its description with an AuthKit client segment naming the calling
// process (`<com.apple.AuthKit/1 (com.orbiter.desktop/0.1.0)>`). Appending ours to that produced a
// two-client-segment X-Mme-Client-Info that no Apple client sends, so drop the process segment and
// keep only the machine and OS description.
fn client_info(description: &str, cfnetwork: &str, darwin: &str) -> AnisetteClientInfo {
    let machine = match description.find("<com.apple.AuthKit") {
        Some(segment) => description[..segment].trim_end(),
        None => description.trim_end(),
    };
    // Apple answered HTTP 429 with an edge block page, so the client must identify itself the way
    // this Mac's own networking stack does: a hardcoded CFNetwork from 2016 on macOS 26, paired
    // with an Xcode version header, is a combination no real machine sends.
    let version = |value: &str, fallback: &str| {
        let usable = value.len() <= 64
            && !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c.is_ascii_alphabetic());
        if usable {
            value.to_string()
        } else {
            fallback.to_string()
        }
    };
    AnisetteClientInfo {
        client_info: format!("{machine} <com.apple.AuthKit/1 (com.apple.akd/1.0)>"),
        user_agent: format!(
            "akd/1.0 CFNetwork/{} Darwin/{}",
            version(cfnetwork, "3860.600.21"),
            version(darwin, "25.5.0")
        ),
    }
}

pub struct LocalProvider;
#[async_trait::async_trait]
impl AnisetteProvider for LocalProvider {
    async fn get_anisette_data(&self) -> Result<AnisetteData, Report> {
        let data = read()
            .await
            .map_err(|_| rootcause::report!("Local authentication support unavailable."))?;
        Ok(AnisetteData::from_local(
            data.machine.clone(),
            data.otp.clone(),
            data.identifier.clone(),
            data.description.clone(),
            data.local_user.clone(),
        ))
    }
    async fn get_client_info(&self) -> Result<AnisetteClientInfo, Report> {
        // Resolve local material before AppleAccount::build can make its first request.
        let data = read()
            .await
            .map_err(|_| rootcause::report!("Local authentication support unavailable."))?;
        Ok(client_info(
            &data.description,
            &data.cfnetwork,
            &data.darwin,
        ))
    }
    fn needs_provisioning(&self) -> Result<bool, Report> {
        Ok(false)
    }
    async fn provision(&mut self, _: Arc<GrandSlam>) -> Result<(), Report> {
        Err(rootcause::report!(
            "Remote provisioning is disabled; macOS manages local authentication support."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authentication_client_uses_compatible_daemon_identifier() {
        // AKDevice returns the calling process in a trailing AuthKit segment; only one such
        // segment may reach Apple.
        for description in [
            "<TestMac> <macOS;26.5;test>",
            "<TestMac> <macOS;26.5;test> <com.apple.AuthKit/1 (com.orbiter.desktop/0.1.0)>",
            "<TestMac> <macOS;26.5;test> <com.apple.AuthKit/1 ((null)/1.0)>",
        ] {
            let info = client_info(description, "3860.600.21", "25.5.0");
            assert_eq!(
                info.client_info,
                "<TestMac> <macOS;26.5;test> <com.apple.AuthKit/1 (com.apple.akd/1.0)>"
            );
            assert_eq!(info.client_info.matches("com.apple.AuthKit").count(), 1);
            assert!(!info.client_info.contains("com.apple.dt.Xcode"));
            assert!(!info.client_info.contains("orbiter"));
            assert_eq!(
                info.user_agent,
                "akd/1.0 CFNetwork/3860.600.21 Darwin/25.5.0"
            );
        }
    }

    #[test]
    fn user_agent_reports_live_os_versions_and_rejects_unusable_values() {
        // The stale hardcoded CFNetwork build is gone; only plausible version text is accepted.
        let live = client_info("<TestMac> <macOS;26.5;test>", "4000.1.2", "26.0.0");
        assert_eq!(live.user_agent, "akd/1.0 CFNetwork/4000.1.2 Darwin/26.0.0");
        assert!(!live.user_agent.contains("808.1.4"));
        for (network, darwin) in [
            ("", ""),
            ("3860.600.21 evil\r\nX-Injected: 1", "25.5.0"),
            (&"9".repeat(65), "25.5.0"),
        ] {
            let info = client_info("<TestMac> <macOS;26.5;test>", network, darwin);
            assert!(info.user_agent.starts_with("akd/1.0 CFNetwork/"));
            assert!(!info.user_agent.contains("Injected"));
            assert!(
                !info.user_agent.contains(char::is_whitespace)
                    || info.user_agent.matches(' ').count() == 2
            );
        }
    }

    #[test]
    fn local_authentication_request_preserves_plist_types_and_metadata() {
        let data = AnisetteData::from_local(
            "machine".into(),
            "otp".into(),
            "device".into(),
            "description".into(),
            "local-user".into(),
        );
        let cpd = data.get_client_provided_data();
        let mut xml = Vec::new();
        plist::to_writer_xml(&mut xml, &cpd).unwrap();
        let decoded: plist::Dictionary = plist::from_bytes(&xml).unwrap();
        for (key, expected) in [
            ("bootstrap", true),
            ("icscrec", true),
            ("pbe", false),
            ("prkgen", true),
        ] {
            assert_eq!(
                decoded.get(key).and_then(plist::Value::as_boolean),
                Some(expected)
            );
        }
        assert_eq!(decoded["X-Apple-I-MD-RINFO"].as_unsigned_integer(), Some(0));
        assert_eq!(decoded["X-Apple-I-MD-LU"].as_string(), Some("local-user"));
        assert_eq!(decoded["X-Mme-Device-Id"].as_string(), Some("device"));
        assert_eq!(decoded["X-Apple-I-MD"].as_string(), Some("otp"));
        assert_eq!(decoded["X-Apple-I-MD-M"].as_string(), Some("machine"));
        assert_eq!(decoded["X-Apple-I-TimeZone"].as_string(), Some("UTC"));
        assert!(
            plist::Date::from_xml_format(decoded["X-Apple-I-Client-Time"].as_string().unwrap())
                .is_ok()
        );
        assert!(!decoded.contains_key("X-Apple-I-SRL-NO"));
        assert!(data.get_header_map().is_ok());
    }

    #[test]
    fn locally_generated_authentication_material_is_never_replayed() {
        // Apple answered HTTP 429 on the proof request while accepting the init request that
        // carried the same one-time password. Local material must be regenerated per request.
        let data = AnisetteData::from_local(
            "machine".into(),
            "otp".into(),
            "device".into(),
            "description".into(),
            "local-user".into(),
        );
        assert!(data.needs_refresh());
    }
    #[test]
    fn only_direct_apple_https_destinations_are_allowed() {
        use isideload::auth::grandslam::validate_apple_url;
        for url in [
            "https://gsa.apple.com/grandslam/GsService2/lookup",
            "https://developerservices2.apple.com/services/QH65B2/listTeams.action",
        ] {
            assert!(validate_apple_url(url).is_ok());
        }
        for url in [
            "http://gsa.apple.com",
            "https://apple.com.attacker.invalid",
            "https://attackerapple.com",
            "https://gsa.apple.com:8443",
            "https://user:password@gsa.apple.com",
            "https://127.0.0.1",
            "https://ani.stikstore.app",
        ] {
            assert!(validate_apple_url(url).is_err());
        }
    }
    #[test]
    fn unused_directory_entries_do_not_block_valid_apple_endpoints() {
        use isideload::auth::grandslam::apple_url_from_directory;
        let mut urls = plist::Dictionary::new();
        urls.insert("login".into(), "https://gsa.apple.com/auth".into());
        urls.insert("unused".into(), "http://unrelated.invalid/metadata".into());
        assert_eq!(
            apple_url_from_directory(&urls, "login").unwrap(),
            "https://gsa.apple.com/auth"
        );
        assert!(apple_url_from_directory(&urls, "unused").is_err());
        assert!(apple_url_from_directory(&urls, "missing").is_err());
    }
    #[test]
    fn authentication_initialization_configures_the_tls_provider() {
        crate::accounts::initialize();
        assert!(GrandSlam::build_reqwest_client(false, None).is_ok());
    }
    #[test]
    fn proxy_and_insecure_tls_configuration_are_rejected() {
        assert!(
            GrandSlam::build_reqwest_client(false, Some("https://proxy.invalid".into())).is_err()
        );
        assert!(GrandSlam::build_reqwest_client(true, None).is_err());
    }
    #[tokio::test]
    async fn account_builder_requires_an_explicit_local_provider() {
        assert!(
            isideload::auth::apple_account::AppleAccount::builder("test@example.invalid")
                .build()
                .await
                .is_err()
        );
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "Direct Apple directory connectivity only; no account login or credentials"]
    async fn native_apple_directory_connectivity() {
        crate::accounts::initialize();
        let result =
            isideload::auth::apple_account::AppleAccount::builder("unused@example.invalid")
                .anisette_provider(LocalProvider)
                .build()
                .await;
        if let Err(error) = result {
            panic!("{}", isideload::redacted_auth_error(&error));
        }
    }
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "Reads local macOS authentication support; emits no material and sends no account credentials"]
    async fn native_local_authentication_support() {
        assert!(
            check().await.available,
            "Local authentication support unavailable"
        );
    }

    /// Apple's edge refused every request that reused a pooled connection to GrandSlam, with an
    /// HTTP 429 block page, so a sign-in died on its second request whatever it contained. Two
    /// consecutive requests must both reach Apple's authentication service. Credential-free: the
    /// address does not exist, so Apple answers with an authentication error rather than a login.
    ///
    /// Run this sparingly. Repeated GrandSlam POSTs from one machine trip a volume limit that
    /// then refuses real sign-ins from the same network for a while, including their first
    /// request. Investigation bursts belong behind that same restraint.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "Direct Apple connectivity only; synthetic address, no credentials"]
    async fn consecutive_requests_are_not_refused_by_apple_edge() {
        use isideload::auth::grandslam::GrandSlam;
        crate::accounts::initialize();
        let provider = LocalProvider;
        let info = provider.get_client_info().await.unwrap();
        let grandslam = GrandSlam::new(info, false, None).await.unwrap();
        let url = grandslam.get_url("gsService").unwrap();
        for attempt in 0..2 {
            let data = provider.get_anisette_data().await.unwrap();
            let mut header = plist::Dictionary::new();
            header.insert("Version".into(), "1.0.1".into());
            let mut request = plist::Dictionary::new();
            request.insert("A2k".into(), plist::Value::Data(vec![7u8; 256]));
            request.insert(
                "cpd".into(),
                plist::Value::Dictionary(data.get_client_provided_data()),
            );
            request.insert("o".into(), "init".into());
            request.insert(
                "ps".into(),
                plist::Value::Array(vec!["s2k".into(), "s2k_fo".into()]),
            );
            request.insert("u".into(), "orbiter.probe.unused@example.invalid".into());
            let mut body = plist::Dictionary::new();
            body.insert("Header".into(), plist::Value::Dictionary(header));
            body.insert("Request".into(), plist::Value::Dictionary(request));
            let result = grandslam.plist_request(&url, &body, None).await;
            // Apple's service answering at all is the assertion; which authentication error it
            // returns for a synthetic address is Apple's business and varies.
            if let Err(error) = result {
                let message = isideload::redacted_auth_error(&error);
                assert!(
                    !message.contains("429"),
                    "request {attempt} was refused by Apple's edge: {message}"
                );
            }
        }
    }
}
