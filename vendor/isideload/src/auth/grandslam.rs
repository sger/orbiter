
use plist::Dictionary;
use plist_macro::plist_to_xml_string;
use plist_macro::pretty_print_dictionary;
#[cfg(not(feature = "wasm"))]
use reqwest::Certificate;
use reqwest::{
    ClientBuilder,
    header::{HeaderMap, HeaderValue},
};
use reqwest_middleware::ClientBuilder as MwClientBuilder;
use rootcause::prelude::*;
use tracing::debug;

use crate::{SideloadError, anisette::AnisetteClientInfo, util::plist::PlistDataExtract};

#[cfg(not(feature = "wasm"))]
const APPLE_ROOT: &[u8] = include_bytes!("./apple_root.der");
const URL_BAG: &str = "https://gsa.apple.com/grandslam/GsService2/lookup";

pub struct GrandSlam {
    pub client: reqwest_middleware::ClientWithMiddleware,
    pub client_info: AnisetteClientInfo,
    url_bag: Dictionary,
}

impl GrandSlam {
    /// Create a new GrandSlam instance
    ///
    /// # Arguments
    /// - `client`: The reqwest client to use for requests
    pub async fn new(
        client_info: AnisetteClientInfo,
        debug: bool,
        proxy_url: Option<String>,
    ) -> Result<Self, Report> {
        let client =
            Self::build_reqwest_client(debug, proxy_url).context("Failed to build HTTP client")?;
        let base_headers = Self::base_headers(&client_info, false)?;
        let url_bag = Self::fetch_url_bag(&client, base_headers).await?;
        Ok(Self {
            client,
            client_info,
            url_bag,
        })
    }

    /// Fetch the URL bag from GrandSlam and cache it
    pub async fn fetch_url_bag(
        client: &reqwest_middleware::ClientWithMiddleware,
        base_headers: HeaderMap,
    ) -> Result<Dictionary, Report> {
        debug!("Fetching URL bag from GrandSlam");
        let resp = client
            .get(URL_BAG)
            .headers(base_headers)
            .send()
            .await
            .context("Failed to fetch URL Bag")?;
        let resp = check_throttled(resp)
            .await?
            .error_for_status()
            .context("Apple service directory request failed")?
            .text()
            .await
            .context("Failed to read URL Bag response text")?;

        let dict: Dictionary =
            plist::from_bytes(resp.as_bytes()).context("Failed to parse URL Bag plist")?;
        let urls = dict
            .get("urls")
            .and_then(|v| v.as_dictionary())
            .cloned()
            .ok_or_else(|| report!("URL Bag plist missing 'urls' dictionary"))?;

        Ok(urls)
    }

    pub fn get_url(&self, key: &str) -> Result<String, Report> {
        apple_url_from_directory(&self.url_bag, key)
    }

    pub fn get(&self, url: &str) -> Result<reqwest_middleware::RequestBuilder, Report> {
        validate_apple_url(url)?;
        let builder = self
            .client
            .get(url)
            .headers(Self::base_headers(&self.client_info, false)?);

        Ok(builder)
    }

    pub fn get_sms(&self, url: &str) -> Result<reqwest_middleware::RequestBuilder, Report> {
        validate_apple_url(url)?;
        let builder = self
            .client
            .get(url)
            .headers(Self::base_headers(&self.client_info, true)?);

        Ok(builder)
    }

    pub fn put_sms(&self, url: &str) -> Result<reqwest_middleware::RequestBuilder, Report> {
        validate_apple_url(url)?;
        let builder = self
            .client
            .put(url)
            .headers(Self::base_headers(&self.client_info, true)?);

        Ok(builder)
    }

    pub fn post(&self, url: &str) -> Result<reqwest_middleware::RequestBuilder, Report> {
        validate_apple_url(url)?;
        let builder = self
            .client
            .post(url)
            .headers(Self::base_headers(&self.client_info, false)?);

        Ok(builder)
    }

    pub fn post_sms(&self, url: &str) -> Result<reqwest_middleware::RequestBuilder, Report> {
        validate_apple_url(url)?;
        let builder = self
            .client
            .post(url)
            .headers(Self::base_headers(&self.client_info, true)?);

        Ok(builder)
    }

    pub fn patch(&self, url: &str) -> Result<reqwest_middleware::RequestBuilder, Report> {
        validate_apple_url(url)?;
        let builder = self
            .client
            .patch(url)
            .headers(Self::base_headers(&self.client_info, false)?);

        Ok(builder)
    }

    pub async fn plist_request(
        &self,
        url: &str,
        body: &Dictionary,
        additional_headers: Option<HeaderMap>,
    ) -> Result<Dictionary, Report> {
        let resp = self
            .post(url)?
            .headers(additional_headers.unwrap_or_else(reqwest::header::HeaderMap::new))
            .body(plist_to_xml_string(body))
            .send()
            .await
            .context("Failed to send grandslam request")?;
        let resp = check_throttled(resp)
            .await?
            .error_for_status()
            .context("Received error response from grandslam")?
            .text()
            .await
            .context("Failed to read grandslam response as text")?;

        let dict: Dictionary = plist::from_bytes(resp.as_bytes())
            .context("Failed to parse grandslam response plist")
            .attach_with(|| resp.clone())?;

        let response_plist = dict
            .get("Response")
            .and_then(|v| v.as_dictionary())
            .cloned()
            .ok_or_else(|| {
                report!("grandslam response missing 'Response'")
                    .attach(pretty_print_dictionary(&dict))
            })?;

        Ok(response_plist)
    }

    fn base_headers(
        client_info: &AnisetteClientInfo,
        sms: bool,
    ) -> Result<reqwest::header::HeaderMap, Report> {
        let mut headers = reqwest::header::HeaderMap::new();
        if !sms {
            headers.insert("Content-Type", HeaderValue::from_static("text/x-xml-plist"));
            headers.insert("Accept", HeaderValue::from_static("text/x-xml-plist"));
        } else {
            headers.insert("Content-Type", HeaderValue::from_static("application/json"));
            headers.insert("Accept", HeaderValue::from_static("application/json"));
        }
        headers.insert(
            "X-Mme-Client-Info",
            HeaderValue::from_str(&client_info.client_info)?,
        );
        headers.insert(
            "User-Agent",
            HeaderValue::from_str(&client_info.user_agent)?,
        );
        // Paired with X-Apple-App-Info below: both name the Xcode authentication scope being
        // requested. Kept to match the reference implementations, since the HTTP 429 was an edge
        // connection-reuse block rather than anything about client identity.
        headers.insert(
            "X-Xcode-Version",
            HeaderValue::from_static("27.0 (27A5218g)"),
        );
        headers.insert(
            "X-Apple-App-Info",
            HeaderValue::from_static("com.apple.gs.xcode.auth"),
        );

        Ok(headers)
    }

    /// Build a reqwest client with the Apple root certificate
    ///
    /// # Arguments
    /// - `debug`: DANGER, If true, accept invalid certificates and enable verbose connection logging
    /// # Errors
    /// Returns an error if the reqwest client cannot be built
    pub fn build_reqwest_client(
        debug: bool,
        proxy_url: Option<String>,
    ) -> Result<reqwest_middleware::ClientWithMiddleware, Report> {
        if proxy_url.is_some() || debug { bail!("Authentication proxies and debug TLS are disabled."); }
        #[cfg(not(feature = "wasm"))]
        let cert = Certificate::from_der(APPLE_ROOT)?;
        #[cfg(not(feature = "wasm"))]
        let client = ClientBuilder::new()
            .no_proxy()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(30))
            // Apple's edge answers HTTP 429 with a block page to any request that reuses a pooled
            // keep-alive connection to GrandSlam: the first POST succeeds and every later one is
            // refused, whatever it contains. Take a fresh connection per request. Upstream's
            // `Connection: close` on the proof request was the same intent, one request too late.
            .pool_max_idle_per_host(0)
            .add_root_certificate(cert)
            .http1_title_case_headers()
            .danger_accept_invalid_certs(debug)
            .connection_verbose(debug)
            .build()?;
        #[cfg(feature = "wasm")]
        let client = ClientBuilder::new()
            .no_proxy()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(30)).build()?;

        let builder = MwClientBuilder::new(client);
        Ok(builder.build())
    }
}

/// Apple answers HTTP 429 while throttling sign-ins. `error_for_status` would discard Apple's
/// Retry-After, so classify it here instead: throttling is not an authentication rejection.
async fn check_throttled(response: reqwest::Response) -> Result<reqwest::Response, Report> {
    if response.status().as_u16() != 429 {
        return Ok(response);
    }
    let retry = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok());
    // Whether Apple's own service answered (a GrandSlam plist, with its numeric code) or an edge
    // block page did, separates account throttling from this client being refused. Only the shape
    // and the numeric code are kept; the body itself is never retained or shown.
    let body = response.text().await.unwrap_or_default();
    let shape = throttle_body_shape(&body);
    Err(report!(SideloadError::RateLimited(retry, shape)).into_dynamic())
}

/// Classify a throttled response body into an allowlisted label. Never returns server text.
fn throttle_body_shape(body: &str) -> &'static str {
    let trimmed = body.trim_start();
    if trimmed.is_empty() {
        return "no body";
    }
    if let Ok(dict) = plist::from_bytes::<Dictionary>(body.as_bytes()) {
        let status = match dict.get("Response").and_then(|v| v.as_dictionary()) {
            Some(response) => response.get("Status").and_then(|v| v.as_dictionary()),
            None => dict.get("Status").and_then(|v| v.as_dictionary()),
        };
        return match status.and_then(|status| status.get_signed_integer("ec").ok()) {
            Some(0) | None => "a GrandSlam plist without an error code",
            Some(-20101) => "a GrandSlam plist carrying authentication code -20101",
            Some(-22320) => "a GrandSlam plist carrying authentication code -22320",
            Some(_) => "a GrandSlam plist carrying an authentication code",
        };
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return "a JSON body";
    }
    if trimmed.starts_with('<') {
        return "an HTML or XML body that is not a GrandSlam plist";
    }
    "an unrecognized body"
}

pub trait GrandSlamErrorChecker {
    fn check_grandslam_error(self) -> Result<Dictionary, Report<SideloadError>>;
}

impl GrandSlamErrorChecker for Dictionary {
    fn check_grandslam_error(self) -> Result<Self, Report<SideloadError>> {
        let result = match self.get("Status") {
            Some(plist::Value::Dictionary(d)) => d,
            _ => &self,
        };

        if result.get_signed_integer("ec").unwrap_or(0) != 0 {
            bail!(SideloadError::AuthWithMessage(
                result.get_signed_integer("ec").unwrap_or(-1),
                result.get_str("em").unwrap_or("Unknown error").to_string(),
            ))
        }

        Ok(self)
    }
}


// Orbiter: never send authentication payloads to support servers, redirects, or proxies.
pub fn validate_apple_url(value: &str) -> Result<(), Report> {
    let url = reqwest::Url::parse(value)?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some()
        || url.port_or_known_default() != Some(443)
        || !(host == "apple.com" || host.ends_with(".apple.com")) {
        bail!("Authentication destination is not an allowed Apple HTTPS endpoint.");
    }
    Ok(())
}


/// Resolve only the endpoint being used. Unrelated directory metadata is not a request.
pub fn apple_url_from_directory(urls: &Dictionary, key: &str) -> Result<String, Report> {
    let url = urls.get_string(key).context("Unable to find key in URL bag")?;
    validate_apple_url(&url)?;
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::throttle_body_shape;

    #[test]
    fn throttled_body_shapes_are_allowlisted_and_carry_no_server_text() {
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>Response</key><dict><key>Status</key><dict>
<key>ec</key><integer>-20101</integer><key>em</key><string>SECRET_SERVER_TEXT</string>
</dict></dict></dict></plist>"#;
        let shape = throttle_body_shape(plist);
        assert_eq!(shape, "a GrandSlam plist carrying authentication code -20101");
        assert!(!shape.contains("SECRET"));
        assert_eq!(
            throttle_body_shape("<html><body>SECRET_BLOCK_PAGE</body></html>"),
            "an HTML or XML body that is not a GrandSlam plist"
        );
        assert_eq!(
            throttle_body_shape("{\"error\": \"SECRET\"}"),
            "a JSON body"
        );
        assert_eq!(throttle_body_shape("   "), "no body");
        assert_eq!(throttle_body_shape("SECRET"), "an unrecognized body");
    }
}
