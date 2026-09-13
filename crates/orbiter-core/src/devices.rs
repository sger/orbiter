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
/// How this Mac reaches a phone.
///
/// A named answer rather than display text, because behaviour depends on it — timeouts, the
/// wording of a recovery instruction, and what a person is told an installation will cost. Text
/// meant for a person is written where it is shown; deciding anything by matching it is how a
/// reworded sentence silently changes what the application does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// A cable.
    Usb,
    /// Wi-Fi, relayed by this Mac's own device daemon to the phone's address.
    Network,
    /// The daemon named a transport this build does not model. Reported rather than guessed.
    Unknown,
}
impl Transport {
    /// Which transport to prefer when a phone offers more than one. Lower wins.
    ///
    /// A cable is faster and does not stop working when someone walks out of range, so it is
    /// always chosen over Wi-Fi. An unmodelled transport is last: it may work, and it is not
    /// something to select on a phone's behalf.
    fn rank(self) -> u8 {
        match self {
            Self::Usb => 0,
            Self::Network => 1,
            Self::Unknown => 2,
        }
    }
}
#[derive(Debug, Serialize)]
pub struct Device {
    /// Ephemeral transport ID, not the device's UDID. Do not persist as identity.
    pub id: u32,
    pub name: Option<String>,
    pub product_type: Option<String>,
    pub ios_version: Option<String>,
    /// The transport this entry uses, and which any operation on it will use.
    pub connection: Transport,
    /// Another transport the same phone is reachable on, when there is one.
    ///
    /// Present so the window can say a phone is also on Wi-Fi without listing it twice. It names
    /// only the kind of connection — no address, and no identifier tying the two entries together
    /// beyond what is already shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alternate: Option<Transport>,
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
/// How many transports the daemon's answer is read up to. A phone reachable two ways occupies two.
const MAX_TRANSPORTS: usize = 32;
/// How many phones are probed in one refresh, after collapsing each phone's transports into one.
const MAX_PHONES: usize = 16;

/// Name the transport the daemon reported.
fn transport(connection: &Connection) -> Transport {
    match connection {
        Connection::Usb => Transport::Usb,
        Connection::Network(_) => Transport::Network,
        _ => Transport::Unknown,
    }
}

/// Collapse each phone's transports into the one to use and another it is also reachable on.
///
/// A phone that is plugged in *and* on Wi-Fi is two entries to the daemon: two transport numbers,
/// one UDID. Listing both would make three phones read as six, and would invite someone to pick
/// the slower one by accident. So each phone appears once, on its preferred transport, and says
/// what else it is reachable on.
///
/// `entries` is `(udid, transport number, transport)` in the daemon's order, which is preserved
/// between phones and irrelevant within one — the preference decides that. The UDID is used to
/// group and is then dropped: it does not reach the report.
fn collapse(entries: Vec<(String, u32, Transport)>) -> Vec<(u32, Transport, Option<Transport>)> {
    let mut phones: Vec<(String, Vec<(u32, Transport)>)> = Vec::new();
    for (udid, id, transport) in entries {
        match phones.iter_mut().find(|(known, _)| *known == udid) {
            Some((_, seen)) => seen.push((id, transport)),
            None => phones.push((udid, vec![(id, transport)])),
        }
    }
    phones
        .into_iter()
        .filter_map(|(_, mut seen)| {
            // Stable, so two transports of equal preference keep the daemon's order.
            seen.sort_by_key(|(_, transport)| transport.rank());
            let &(id, chosen) = seen.first()?;
            let alternate = seen
                .iter()
                .map(|&(_, transport)| transport)
                .find(|&transport| transport != chosen);
            Some((id, chosen, alternate))
        })
        .collect()
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
    // Bounded twice, for two different reasons: the daemon's answer is read up to a limit so a
    // strange reply cannot become unbounded work, and the collapsed phones are limited again so
    // the probe budget is spent per phone rather than per cable.
    let entries = devices
        .iter()
        .take(MAX_TRANSPORTS)
        .map(|raw| {
            (
                raw.udid.clone(),
                raw.device_id,
                transport(&raw.connection_type),
            )
        })
        .collect();
    let mut result = Vec::new();
    for (id, connection, alternate) in collapse(entries).into_iter().take(MAX_PHONES) {
        let Some(raw) = devices.iter().find(|raw| raw.device_id == id) else {
            continue;
        };
        let mut device = Device {
            id,
            name: None,
            product_type: None,
            ios_version: None,
            connection,
            alternate,
            state: DeviceState::Unavailable,
            message: "Device did not respond in time. Unlock it, check the connection, and refresh.",
        };
        match timeout(Duration::from_secs(3), probe(raw, &mut device)).await {
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
    /// A phone that is plugged in and on Wi-Fi is one phone. It is offered once, on the cable,
    /// and says it is also reachable over Wi-Fi.
    fn a_phone_on_two_transports_is_offered_once_and_prefers_the_cable() {
        let collapsed = collapse(vec![
            // The daemon's order puts Wi-Fi first here; the preference must still win.
            ("phone-a".into(), 7, Transport::Network),
            ("phone-a".into(), 3, Transport::Usb),
        ]);
        assert_eq!(
            collapsed,
            vec![(3, Transport::Usb, Some(Transport::Network))]
        );
    }

    #[test]
    /// A phone reachable one way says so, rather than implying a second connection exists.
    fn a_phone_on_one_transport_names_no_alternative() {
        assert_eq!(
            collapse(vec![("phone-a".into(), 3, Transport::Network)]),
            vec![(3, Transport::Network, None)]
        );
        // Two of the same transport is not an alternative either — it is one way of connecting.
        assert_eq!(
            collapse(vec![
                ("phone-a".into(), 3, Transport::Usb),
                ("phone-a".into(), 4, Transport::Usb),
            ]),
            vec![(3, Transport::Usb, None)]
        );
    }

    #[test]
    /// Different phones stay different, in the order the daemon gave them — collapsing is about
    /// one phone's connections, and must never merge two phones or reorder the list.
    fn separate_phones_are_never_merged_and_keep_their_order() {
        let collapsed = collapse(vec![
            ("phone-b".into(), 9, Transport::Network),
            ("phone-a".into(), 3, Transport::Usb),
            ("phone-b".into(), 2, Transport::Usb),
        ]);
        assert_eq!(
            collapsed,
            vec![
                (2, Transport::Usb, Some(Transport::Network)),
                (3, Transport::Usb, None),
            ]
        );
    }

    #[test]
    /// A transport this build does not model is reported and is chosen last: it may well work,
    /// and it is not something to select on someone's behalf while a known one is available.
    fn an_unmodelled_transport_is_reported_but_never_preferred() {
        assert_eq!(
            transport(&Connection::Unknown("future".into())),
            Transport::Unknown
        );
        let collapsed = collapse(vec![
            ("phone-a".into(), 1, Transport::Unknown),
            ("phone-a".into(), 2, Transport::Network),
        ]);
        assert_eq!(
            collapsed,
            vec![(2, Transport::Network, Some(Transport::Unknown))]
        );
        // Alone, it is still offered rather than hidden.
        assert_eq!(
            collapse(vec![("phone-a".into(), 1, Transport::Unknown)]),
            vec![(1, Transport::Unknown, None)]
        );
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
