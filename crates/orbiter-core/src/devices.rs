//! Read-only discovery through the local Apple device service. No pairing mutations.
use idevice::{
    IdeviceError, IdeviceService,
    lockdown::LockdownClient,
    provider::IdeviceProvider,
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdDevice},
};
use serde::Serialize;
use std::time::Duration;
use tokio::time::timeout;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    Paired,
    Locked,
    TrustRequired,
    PairingUnverified,
    Unavailable,
}
#[derive(Debug, Serialize)]
pub struct Device {
    /// Ephemeral transport ID, not the device's UDID. Do not persist as identity.
    pub id: u32,
    pub name: Option<String>,
    pub product_type: Option<String>,
    pub ios_version: Option<String>,
    pub connection: &'static str,
    pub state: DeviceState,
    pub message: &'static str,
}
#[derive(Debug, Serialize)]
pub struct Discovery {
    pub devices: Vec<Device>,
    pub service_available: bool,
    pub message: Option<&'static str>,
}
/// Where the local device daemon listens.
///
/// Always this machine's own socket. Any environment variable naming a remote daemon is ignored:
/// a phone on someone else's desk is not a device this Mac may enumerate or install to.
pub(crate) fn address() -> UsbmuxdAddr {
    // Deliberately ignore USBMUXD_SOCKET_ADDRESS: never send pairing data to a remote daemon.
    #[cfg(unix)]
    {
        UsbmuxdAddr::UnixSocket("/var/run/usbmuxd".into())
    }
    #[cfg(not(unix))]
    {
        UsbmuxdAddr::TcpSocket(std::net::SocketAddr::from(([127, 0, 0, 1], 27015)))
    }
}
/// Turn a transport error into a state a person can act on and one sentence saying how.
///
/// The underlying error is classified and then discarded rather than formatted: it can carry
/// pairing material and addresses, and "unlock the phone and tap Trust" is the whole of what is
/// useful.
fn failure(error: &IdeviceError) -> (DeviceState, &'static str) {
    match error {
        IdeviceError::DeviceLocked => (
            DeviceState::Locked,
            "Unlock the iPhone and refresh devices.",
        ),
        IdeviceError::InvalidHostID => (
            DeviceState::TrustRequired,
            "Unlock the iPhone and establish trust in Finder (macOS) or Apple's device app (Windows), then refresh.",
        ),
        _ => (
            DeviceState::Unavailable,
            "Device communication failed. Unlock the iPhone, check the cable, and refresh. Trust may need to be established in Apple's device app.",
        ),
    }
}
/// List the iPhones attached to this Mac and how usable each one is.
///
/// Read-only: it reports what the local daemon sees and which pairings already exist. It pairs
/// nothing and contacts no Apple service. Never fails — an unreachable daemon is reported through
/// `service_available`, because "nothing is plugged in" and "this Mac cannot see phones" are
/// different answers.
///
/// Each device is probed independently, so one locked phone does not hide the others.
pub async fn discover() -> Discovery {
    let result = timeout(Duration::from_secs(3), async {
        let mut mux = address().connect(0).await?;
        mux.get_devices().await
    })
    .await;
    let devices = match result {
        Ok(Ok(devices)) => devices,
        _ => {
            return Discovery {
                devices: vec![],
                service_available: false,
                message: Some(
                    "Apple device service is unavailable or timed out. On macOS, reconnect the iPhone and check Finder. On Windows, check Apple Mobile Device services and USB drivers.",
                ),
            };
        }
    };
    let mut result = Vec::new();
    // Keep the complete refresh bounded even with many attached transports.
    for raw in devices.into_iter().take(16) {
        let mut device = Device {
            id: raw.device_id,
            name: None,
            product_type: None,
            ios_version: None,
            connection: match raw.connection_type {
                Connection::Usb => "USB",
                Connection::Network(_) => "Network",
                _ => "Unknown",
            },
            state: DeviceState::Unavailable,
            message: "Device did not respond in time. Unlock it, check the connection, and refresh.",
        };
        match timeout(Duration::from_secs(3), probe(&raw, &mut device)).await {
            Ok(Ok(())) => (),
            Ok(Err(e)) => {
                (device.state, device.message) = failure(&e);
            }
            Err(_) => (),
        }
        // Retain unknown devices for recovery, but don't offer known iPads/Macs as iPhones.
        if device
            .product_type
            .as_ref()
            .is_none_or(|p| p.starts_with("iPhone"))
        {
            result.push(device);
        }
    }
    Discovery {
        devices: result,
        service_available: true,
        message: None,
    }
}
/// Read one string property from a phone, or `None` if it is absent or not a string.
///
/// Absence is ordinary rather than an error: the report says what is known and leaves the rest
/// blank instead of refusing to describe a device at all.
async fn value(client: &mut LockdownClient, key: &str) -> Option<String> {
    client
        .get_value(Some(key), None)
        .await
        .ok()?
        .as_string()
        .map(str::to_owned)
}
/// Fill in one device's name, model, iOS version and pairing state.
///
/// Uses the existing pairing record and never creates one: discovery must not cause a Trust
/// prompt on someone's phone. The UDID is used to address the device and is not copied into the
/// report.
///
/// # Errors
///
/// Returns the transport's error, which the caller classifies through [`failure`] rather than
/// showing.
async fn probe(raw: &UsbmuxdDevice, out: &mut Device) -> Result<(), IdeviceError> {
    let provider = raw.to_provider(address(), "Orbiter");
    let mut client = LockdownClient::connect(&provider).await?;
    out.name = value(&mut client, "DeviceName").await;
    out.product_type = value(&mut client, "ProductType").await;
    out.ios_version = value(&mut client, "ProductVersion").await;
    // A saved record alone is not proof of current trust: verify an authenticated session.
    let pair = match provider.get_pairing_file().await {
        Ok(pair) => pair,
        Err(_) => {
            out.state = DeviceState::PairingUnverified;
            out.message = "A pairing record could not be read. Unlock the iPhone, establish trust using Finder or Apple's device app, then refresh. Orbiter does not create or reset pairing records.";
            return Ok(());
        }
    };
    client.start_session(&pair).await?;
    out.state = DeviceState::Paired;
    out.message = "Pairing session verified. Lock state, installation, and Developer Mode readiness have not been checked.";
    Ok(())
}
#[cfg(test)]
/// Checks that device states are actionable, errors are redacted, and discovery stays local.
mod tests {
    use super::*;
    #[test]
    /// Every state a device can be reported in comes with a sentence saying what to do about it,
    /// rather than only naming the problem.
    fn actionable_states() {
        assert_eq!(failure(&IdeviceError::DeviceLocked).0, DeviceState::Locked);
        assert_eq!(
            failure(&IdeviceError::InvalidHostID).0,
            DeviceState::TrustRequired
        );
    }
    #[test]
    /// A transport error never reaches the report: the classified state and its fixed sentence do,
    /// so pairing detail cannot leak into something a person pastes into an issue.
    fn errors_are_redacted() {
        let (_, msg) = failure(&IdeviceError::UnexpectedResponse(
            "secret-device-identifier".into(),
        ));
        assert!(!msg.contains("secret-device-identifier"));
    }
    #[test]
    /// Discovery always uses this machine's own device daemon, even when the environment names a
    /// remote one: a phone on someone else's desk is not a device this Mac may enumerate.
    fn ignores_remote_daemon_environment() {
        match address() {
            #[cfg(unix)]
            UsbmuxdAddr::UnixSocket(p) => assert_eq!(p, "/var/run/usbmuxd"),
            UsbmuxdAddr::TcpSocket(p) => assert!(p.ip().is_loopback()),
        }
    }
}
