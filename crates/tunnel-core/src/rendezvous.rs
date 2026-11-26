//! Client for the rendezvous server.
//!
//! The server is assumed hostile. Nothing secret is sent to it: only the
//! nameplate and the opaque pairing messages, whose secrecy and integrity come
//! from SPAKE2, key confirmation and the sealed hello, not from this channel.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use tunnel_proto::hello::PairingMessage;
use tunnel_proto::rendezvous::{self as proto, ClientMessage, ErrorCode, ServerMessage};

use crate::error::{Error, Result};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
/// Largest message accepted from the server (a relayed 4 KiB body plus JSON).
const MAX_SERVER_MESSAGE: usize = 16 * 1024;

/// Something that happened in the room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RoomEvent {
    /// The other device arrived.
    PeerJoined,
    /// A pairing message from the other device.
    Message(PairingMessage),
}

/// An open connection to a rendezvous room.
pub(crate) struct Room {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

/// Checks a rendezvous address and returns it in canonical form plus its host.
///
/// `wss://` is required, except for loopback and private-network addresses
/// where plain `ws://` is allowed for testing and self-hosting on a LAN.
pub fn check_url(url: &str) -> Result<(String, String)> {
    let url = url.trim();
    let (secure, rest) = if let Some(rest) = url.strip_prefix("wss://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("ws://") {
        (false, rest)
    } else {
        return Err(Error::RendezvousAddressInvalid);
    };
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, proto::PATH.to_string()),
    };
    if !tunnel_crypto::code::is_valid_host(authority) || path.contains(['?', '#', ' ']) {
        return Err(Error::RendezvousAddressInvalid);
    }
    let path = if path == "/" {
        proto::PATH.to_string()
    } else {
        path
    };
    if !secure && !is_local_host(authority) {
        return Err(Error::RendezvousAddressInvalid);
    }
    let scheme = if secure { "wss" } else { "ws" };
    Ok((
        format!("{scheme}://{authority}{path}"),
        authority.to_ascii_lowercase(),
    ))
}

/// Builds the address for a host named in a pairing link.
pub fn url_for_host(host: &str) -> Result<String> {
    let scheme = if is_local_host(host) { "ws" } else { "wss" };
    check_url(&format!("{scheme}://{host}{}", proto::PATH)).map(|(url, _)| url)
}

fn host_without_port(authority: &str) -> &str {
    if let Some(end) = authority.find(']') {
        return authority[..end].trim_start_matches('[');
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => authority,
    }
}

fn is_local_host(authority: &str) -> bool {
    let host = host_without_port(authority);
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(IpAddr::V6(ip)) => {
            ip.is_loopback()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(Error::internal)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

fn map_code(code: ErrorCode) -> Error {
    match code {
        ErrorCode::UnknownCode => Error::CodeFailed,
        ErrorCode::CodeInUse => Error::CodeInUse,
        ErrorCode::Expired => Error::CodeExpired,
        ErrorCode::RateLimited | ErrorCode::ServerBusy => Error::RendezvousBusy,
        ErrorCode::TooLarge | ErrorCode::BadRequest => {
            Error::Internal("the pairing service refused a message".into())
        }
    }
}

impl Room {
    /// Connects to the rendezvous server.
    pub(crate) async fn connect(url: &str) -> Result<Self> {
        let (url, _) = check_url(url)?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_SERVER_MESSAGE))
            .max_frame_size(Some(MAX_SERVER_MESSAGE));
        let connector = Connector::Rustls(tls_config()?);
        let connecting = tokio_tungstenite::connect_async_tls_with_config(
            url,
            Some(config),
            true,
            Some(connector),
        );
        match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
            Ok(Ok((socket, _response))) => Ok(Self { socket }),
            Ok(Err(_)) | Err(_) => Err(Error::RendezvousUnreachable),
        }
    }

    async fn send(&mut self, message: &ClientMessage) -> Result<()> {
        let json = serde_json::to_string(message).map_err(Error::internal)?;
        self.socket
            .send(Message::text(json))
            .await
            .map_err(|_| Error::RendezvousUnreachable)
    }

    /// Reads the next server message, or `None` when the server closed.
    async fn recv(&mut self) -> Result<Option<ServerMessage>> {
        loop {
            match self.socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    return serde_json::from_str(text.as_str()).map(Some).map_err(|_| {
                        Error::Internal("the pairing service sent something unreadable".into())
                    });
                }
                Some(Ok(Message::Close(_))) | None => return Ok(None),
                Some(Ok(_)) => continue,
                Some(Err(_)) => return Err(Error::RendezvousUnreachable),
            }
        }
    }

    /// Opens a new room. Returns the nameplate and the room's lifetime in seconds.
    pub(crate) async fn create(&mut self) -> Result<(u16, u64)> {
        self.send(&ClientMessage::Create).await?;
        match tokio::time::timeout(REPLY_TIMEOUT, self.recv()).await {
            Ok(Ok(Some(ServerMessage::Created {
                nameplate,
                expires_in,
            }))) => Ok((nameplate, expires_in)),
            Ok(Ok(Some(ServerMessage::Error { code }))) => Err(map_code(code)),
            Ok(Err(err)) => Err(err),
            _ => Err(Error::RendezvousUnreachable),
        }
    }

    /// Joins the room behind a nameplate.
    pub(crate) async fn join(&mut self, nameplate: u16) -> Result<()> {
        self.send(&ClientMessage::Join { nameplate }).await?;
        match tokio::time::timeout(REPLY_TIMEOUT, self.recv()).await {
            Ok(Ok(Some(ServerMessage::Joined))) => Ok(()),
            Ok(Ok(Some(ServerMessage::Error { code }))) => Err(map_code(code)),
            Ok(Err(err)) => Err(err),
            _ => Err(Error::RendezvousUnreachable),
        }
    }

    /// Posts one pairing message to the other device.
    pub(crate) async fn post(&mut self, message: &PairingMessage) -> Result<()> {
        let bytes = message.encode().map_err(Error::internal)?;
        self.send(&ClientMessage::Post {
            body: BASE64.encode(bytes),
        })
        .await
    }

    /// Waits for the next thing to happen in the room.
    ///
    /// The other device leaving, the room expiring, or anything malformed all
    /// end pairing: the code is dead.
    pub(crate) async fn next(&mut self) -> Result<RoomEvent> {
        match self.recv().await? {
            Some(ServerMessage::PeerJoined) => Ok(RoomEvent::PeerJoined),
            Some(ServerMessage::Message { body }) => {
                if !proto::is_valid_body(&body) {
                    return Err(Error::CodeFailed);
                }
                let bytes = BASE64.decode(body).map_err(|_| Error::CodeFailed)?;
                PairingMessage::decode(&bytes)
                    .map(RoomEvent::Message)
                    .map_err(|_| Error::CodeFailed)
            }
            Some(ServerMessage::PeerLeft) => Err(Error::CodeFailed),
            Some(ServerMessage::Error { code }) => Err(map_code(code)),
            Some(ServerMessage::Created { .. } | ServerMessage::Joined) => Err(Error::CodeFailed),
            None => Err(Error::RendezvousUnreachable),
        }
    }

    /// Leaves the room; the server deletes it.
    pub(crate) async fn close(mut self) {
        let _ = self.send(&ClientMessage::Close).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), self.socket.close(None)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_checked() {
        assert_eq!(
            check_url("wss://rendezvous.example.com/v1").unwrap(),
            (
                "wss://rendezvous.example.com/v1".to_string(),
                "rendezvous.example.com".to_string()
            )
        );
        assert_eq!(
            check_url(" wss://Example.com ").unwrap().0,
            "wss://Example.com/v1"
        );
        assert_eq!(
            check_url("wss://example.com:8443/").unwrap().0,
            "wss://example.com:8443/v1"
        );
        assert_eq!(
            check_url("ws://127.0.0.1:8080/v1").unwrap().1,
            "127.0.0.1:8080"
        );
        assert!(check_url("ws://localhost:8080").is_ok());
        assert!(check_url("ws://192.168.1.10:8080/v1").is_ok());
        assert!(check_url("ws://[::1]:8080/v1").is_ok());
        for bad in [
            "",
            "https://example.com/v1",
            "example.com",
            "ws://example.com/v1",
            "ws://8.8.8.8/v1",
            "wss://user@example.com/v1",
            "wss://example.com/v1?x=1",
            "wss:///v1",
            "wss://exa mple.com/v1",
        ] {
            assert_eq!(
                check_url(bad),
                Err(Error::RendezvousAddressInvalid),
                "{bad}"
            );
        }
    }

    #[test]
    fn link_hosts_become_addresses() {
        assert_eq!(
            url_for_host("rendezvous.example.com").unwrap(),
            "wss://rendezvous.example.com/v1"
        );
        assert_eq!(
            url_for_host("127.0.0.1:9000").unwrap(),
            "ws://127.0.0.1:9000/v1"
        );
        assert!(url_for_host("bad host").is_err());
    }
}
