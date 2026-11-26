//! The iroh endpoint: creating it, describing how to reach it, and reaching
//! the other device.
//!
//! All iroh use is confined to this module and `session`, so an iroh upgrade
//! touches one place.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use iroh::endpoint::{Connection, QuicTransportConfig, VarInt, presets};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, SecretKey, TransportAddr, Watcher,
};
use tunnel_proto::hello::Hello;
use tunnel_proto::limits;
use tunnel_proto::version::ALPN_CONTROL;

use crate::error::{Error, Result};
use crate::events::PathKind;

/// Which relays to use when the two devices cannot reach each other directly.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RelayChoice {
    /// The default public relays.
    #[default]
    Default,
    /// No relays: direct connections only (tests, isolated networks).
    Disabled,
    /// Your own relays.
    Custom(Vec<String>),
    /// Relays started inside a test, whose self-signed certificates are
    /// accepted. Does not exist in release builds.
    #[cfg(feature = "test-utils")]
    Insecure(Vec<String>),
}

impl RelayChoice {
    /// Builds the choice from the "relay servers" setting (empty = default).
    pub fn from_urls(urls: &[String]) -> Self {
        if urls.is_empty() {
            RelayChoice::Default
        } else {
            RelayChoice::Custom(urls.to_vec())
        }
    }

    fn mode(&self) -> Result<RelayMode> {
        Ok(match self {
            RelayChoice::Default => iroh::endpoint::default_relay_mode(),
            RelayChoice::Disabled => RelayMode::Disabled,
            RelayChoice::Custom(urls) => RelayMode::custom(parse_relays(urls)?),
            #[cfg(feature = "test-utils")]
            RelayChoice::Insecure(urls) => RelayMode::custom(parse_relays(urls)?),
        })
    }

    /// Every relay this choice may use. A device that only knows another
    /// device's endpoint ID tries all of them.
    pub(crate) fn urls(&self) -> Result<Vec<RelayUrl>> {
        Ok(self.mode()?.relay_map().urls())
    }
}

fn parse_relays(urls: &[String]) -> Result<Vec<RelayUrl>> {
    urls.iter()
        .map(|url| url.parse::<RelayUrl>())
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| Error::Internal("invalid relay address".into()))
}

/// Per-stream receive window. Sized for fast links with high latency.
const STREAM_WINDOW: u32 = 16 * 1024 * 1024;
/// Per-connection receive window.
const CONNECTION_WINDOW: u32 = 64 * 1024 * 1024;
/// Streams the other device may have open at once: one control stream plus
/// every parallel download, with headroom.
const MAX_BIDI_STREAMS: u32 = 96;

/// Creates an endpoint with a fresh key.
///
/// One-off sessions use a new key every time, so relays cannot link one
/// session to the next. No address is published anywhere: the two devices
/// tell each other how to connect through the sealed hello.
pub(crate) async fn bind_endpoint(relay: &RelayChoice) -> Result<Endpoint> {
    let transport = QuicTransportConfig::builder()
        .max_concurrent_bidi_streams(VarInt::from_u32(MAX_BIDI_STREAMS))
        .max_concurrent_uni_streams(VarInt::from_u32(0))
        .stream_receive_window(VarInt::from_u32(STREAM_WINDOW))
        .receive_window(VarInt::from_u32(CONNECTION_WINDOW))
        .send_window(u64::from(CONNECTION_WINDOW))
        .build();
    builder(relay)?
        .secret_key(SecretKey::generate())
        .alpns(vec![ALPN_CONTROL.to_vec()])
        .transport_config(transport)
        .bind()
        .await
        .map_err(|err| Error::Internal(format!("could not start networking: {err}")))
}

/// Creates the meeting-point endpoint for a nameplate: the endpoint a device
/// showing a code listens on when there is no rendezvous server.
///
/// Its key is derived from the public nameplate (see
/// [`tunnel_crypto::code::meeting_key`]), so it identifies nobody. It only
/// carries the pairing messages and is closed as soon as pairing ends.
pub(crate) async fn bind_meeting_point(relay: &RelayChoice, nameplate: u16) -> Result<Endpoint> {
    let transport = QuicTransportConfig::builder()
        .max_concurrent_bidi_streams(VarInt::from_u32(1))
        .max_concurrent_uni_streams(VarInt::from_u32(0))
        .build();
    builder(relay)?
        .secret_key(meeting_secret(nameplate))
        .alpns(vec![tunnel_proto::meeting::ALPN.to_vec()])
        .transport_config(transport)
        .bind()
        .await
        .map_err(|err| Error::Internal(format!("could not start networking: {err}")))
}

fn meeting_secret(nameplate: u16) -> SecretKey {
    SecretKey::from_bytes(&tunnel_crypto::code::meeting_key(nameplate))
}

/// Where the meeting point for a nameplate can be reached: its endpoint ID
/// through every relay in use. Whichever relay the other device is connected
/// to delivers the first packet; the rest go nowhere.
pub(crate) fn meeting_addr(relay: &RelayChoice, nameplate: u16) -> Result<EndpointAddr> {
    let id = meeting_secret(nameplate).public();
    let addrs: Vec<TransportAddr> = relay
        .urls()?
        .into_iter()
        .map(TransportAddr::Relay)
        .collect();
    Ok(EndpointAddr::from_parts(id, addrs))
}

fn builder(relay: &RelayChoice) -> Result<iroh::endpoint::Builder> {
    let builder = Endpoint::builder(presets::Minimal).relay_mode(relay.mode()?);
    #[cfg(feature = "test-utils")]
    if matches!(relay, RelayChoice::Insecure(_)) {
        return Ok(builder.ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify()));
    }
    Ok(builder)
}

/// How this endpoint can be reached, for the hello.
///
/// Waits briefly for the relay connection (so the relay address can be
/// included) but never blocks pairing on it: on a network without internet the
/// local addresses are enough.
pub(crate) async fn reachability(
    endpoint: &Endpoint,
    relay: &RelayChoice,
    include_loopback: bool,
) -> (Option<String>, Vec<SocketAddr>) {
    if *relay != RelayChoice::Disabled {
        let _ = tokio::time::timeout(Duration::from_secs(4), endpoint.online()).await;
    }
    let mut watcher = endpoint.watch_addr();
    let mut addr = watcher.get();
    if addr.ip_addrs().next().is_none() && addr.relay_urls().next().is_none() {
        // Local addresses are discovered just after binding; give it a moment.
        if let Ok(Ok(updated)) =
            tokio::time::timeout(Duration::from_secs(2), watcher.updated()).await
        {
            addr = updated;
        }
    }
    let relay_url = addr.relay_urls().next().map(|url| url.to_string());
    let mut direct: Vec<SocketAddr> = addr.ip_addrs().copied().collect();
    if include_loopback {
        for bound in endpoint.bound_sockets() {
            let loopback: IpAddr = if bound.is_ipv4() {
                Ipv4Addr::LOCALHOST.into()
            } else {
                Ipv6Addr::LOCALHOST.into()
            };
            direct.push(SocketAddr::new(loopback, bound.port()));
        }
    }
    direct.sort();
    direct.dedup();
    direct.truncate(limits::MAX_DIRECT_ADDRS);
    let relay_url = relay_url.filter(|url| url.len() <= limits::MAX_RELAY_URL_BYTES);
    (relay_url, direct)
}

/// Turns the other device's hello into something iroh can dial.
pub(crate) fn peer_addr(hello: &Hello) -> Result<EndpointAddr> {
    let id = EndpointId::from_bytes(&hello.endpoint_id).map_err(|_| Error::Protocol)?;
    let mut addrs: Vec<TransportAddr> = hello
        .direct_addrs
        .iter()
        .copied()
        .map(TransportAddr::Ip)
        .collect();
    if let Some(url) = &hello.relay_url {
        addrs.push(TransportAddr::Relay(
            url.parse::<RelayUrl>().map_err(|_| Error::Protocol)?,
        ));
    }
    if addrs.is_empty() {
        return Err(Error::PeerUnreachable);
    }
    Ok(EndpointAddr::from_parts(id, addrs))
}

/// Dials the other device.
pub(crate) async fn dial(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    timeout: Duration,
) -> Result<Connection> {
    match tokio::time::timeout(timeout, endpoint.connect(addr, ALPN_CONTROL)).await {
        Ok(Ok(connection)) => Ok(connection),
        Ok(Err(_)) | Err(_) => Err(Error::PeerUnreachable),
    }
}

/// Waits for the other device to dial in. Connections from any other endpoint
/// ID are closed and ignored: during this window exactly one device is welcome.
pub(crate) async fn accept_from(
    endpoint: &Endpoint,
    expected: EndpointId,
    timeout: Duration,
) -> Result<Connection> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let incoming = match tokio::time::timeout_at(deadline, endpoint.accept()).await {
            Ok(Some(incoming)) => incoming,
            Ok(None) => return Err(Error::Internal("networking stopped".into())),
            Err(_) => return Err(Error::PeerUnreachable),
        };
        let connection = match tokio::time::timeout_at(deadline, incoming).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(_)) => continue,
            Err(_) => return Err(Error::PeerUnreachable),
        };
        if connection.remote_id() == expected {
            return Ok(connection);
        }
        connection.close(
            VarInt::from_u32(tunnel_proto::data::close::UNEXPECTED_PEER),
            b"",
        );
    }
}

/// Whether data currently flows directly or through a relay.
pub(crate) fn path_kind(connection: &Connection) -> PathKind {
    let paths = connection.paths();
    let selected = paths.iter().find(|path| path.is_selected());
    match selected {
        Some(path) if path.is_relay() => PathKind::Relay,
        Some(path) if path.is_ip() => PathKind::Direct,
        _ => PathKind::Unknown,
    }
}
