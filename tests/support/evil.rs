//! A hostile peer.
//!
//! Speaks the real protocol with the real cryptography, so it gets as far as a
//! malicious device with the right code would, and then misbehaves on purpose.
//! Built only from the protocol crates, never from the engine, so it cannot
//! accidentally inherit the engine's good manners.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, TransportAddr};
use tokio_tungstenite::tungstenite::Message;
use tunnel_crypto::{ConnectionCode, PakeState, Role, SessionSecrets};
use tunnel_proto::control::ControlMessage;
use tunnel_proto::data::{DataHeader, Get};
use tunnel_proto::frame;
use tunnel_proto::hello::{DeviceType, Hello, PairingMessage};
use tunnel_proto::limits;
use tunnel_proto::rendezvous::{ClientMessage, ServerMessage};
use tunnel_proto::version::ALPN_CONTROL;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const PATIENCE: Duration = Duration::from_secs(20);

/// Ways to misbehave during pairing.
#[derive(Debug, Clone, Default)]
pub struct Misbehave {
    /// Send a key-confirmation tag that is wrong.
    pub wrong_confirmation: bool,
    /// Send the host's own SPAKE2 message back at it.
    pub reflect_pake: bool,
    /// Send the host's own confirmation tag back at it.
    pub reflect_confirmation: bool,
    /// Send a channel-binding tag that is wrong.
    pub wrong_bind: bool,
    /// Claim to speak only these protocol versions.
    pub versions: Option<Vec<u16>>,
    /// Name to present.
    pub device_name: Option<String>,
}

/// A connected hostile peer (role B: it entered the code).
pub struct Evil {
    pub endpoint: Endpoint,
    pub connection: Connection,
    pub control_send: SendStream,
    pub control_recv: RecvStream,
    pub secrets: SessionSecrets,
}

async fn ws_send(socket: &mut Socket, message: &ClientMessage) {
    socket
        .send(Message::text(serde_json::to_string(message).unwrap()))
        .await
        .expect("ws send");
}

async fn ws_recv(socket: &mut Socket) -> Result<ServerMessage, String> {
    loop {
        match tokio::time::timeout(PATIENCE, socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                return serde_json::from_str(text.as_str()).map_err(|e| e.to_string());
            }
            Ok(Some(Ok(_))) => continue,
            Ok(_) => return Err("rendezvous closed".into()),
            Err(_) => return Err("rendezvous timed out".into()),
        }
    }
}

async fn ws_post(socket: &mut Socket, message: &PairingMessage) {
    let body = BASE64.encode(message.encode().unwrap());
    ws_send(socket, &ClientMessage::Post { body }).await;
}

async fn ws_pairing(socket: &mut Socket) -> Result<PairingMessage, String> {
    loop {
        match ws_recv(socket).await? {
            ServerMessage::Message { body } => {
                let bytes = BASE64.decode(body).map_err(|e| e.to_string())?;
                return PairingMessage::decode(&bytes).map_err(|e| e.to_string());
            }
            ServerMessage::PeerJoined | ServerMessage::Joined => continue,
            other => return Err(format!("{other:?}")),
        }
    }
}

impl Evil {
    /// Joins the session behind `code`. Returns an error string when the
    /// victim (correctly) shuts the attempt down.
    pub async fn join(
        rendezvous_url: &str,
        code: &str,
        misbehave: Misbehave,
    ) -> Result<Evil, String> {
        let code: ConnectionCode = code.parse().map_err(|e| format!("{e}"))?;
        let (mut socket, _) = tokio_tungstenite::connect_async(rendezvous_url)
            .await
            .map_err(|e| e.to_string())?;
        ws_send(
            &mut socket,
            &ClientMessage::Join {
                nameplate: code.nameplate(),
            },
        )
        .await;

        let PairingMessage::Pake(host_pake) = ws_pairing(&mut socket).await? else {
            return Err("expected the host's SPAKE2 message".into());
        };
        let (pake, own_pake) = PakeState::start(Role::B, &code);
        let posted = if misbehave.reflect_pake {
            host_pake.clone()
        } else {
            own_pake
        };
        ws_post(&mut socket, &PairingMessage::Pake(posted)).await;
        let mut handshake = pake.finish(&host_pake).map_err(|e| e.to_string())?;

        let PairingMessage::Confirm(host_tag) = ws_pairing(&mut socket).await? else {
            return Err("expected the host's confirmation".into());
        };
        let own_tag = if misbehave.wrong_confirmation {
            [0x5a; 32]
        } else if misbehave.reflect_confirmation {
            host_tag
        } else {
            handshake.confirmation()
        };
        ws_post(&mut socket, &PairingMessage::Confirm(own_tag)).await;
        handshake
            .verify_confirmation(&host_tag)
            .map_err(|e| e.to_string())?;

        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(SecretKey::generate())
            .alpns(vec![ALPN_CONTROL.to_vec()])
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await
            .map_err(|e| e.to_string())?;
        let direct_addrs: Vec<SocketAddr> = endpoint
            .bound_sockets()
            .into_iter()
            .filter(SocketAddr::is_ipv4)
            .map(|bound| SocketAddr::new(Ipv4Addr::LOCALHOST.into(), bound.port()))
            .collect();
        let hello = Hello {
            versions: misbehave.versions.clone().unwrap_or_else(|| vec![1]),
            endpoint_id: *endpoint.id().as_bytes(),
            relay_url: None,
            direct_addrs,
            device_name: misbehave
                .device_name
                .clone()
                .unwrap_or_else(|| "Mallory".into()),
            device_type: DeviceType::Desktop,
            app_version: "6.6.6".into(),
            capabilities: 0,
        };
        let sealed = handshake
            .seal(&hello.encode().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        ws_post(&mut socket, &PairingMessage::Hello(sealed)).await;
        let PairingMessage::Hello(host_sealed) = ws_pairing(&mut socket).await? else {
            return Err("expected the host's hello".into());
        };
        let host_hello = Hello::decode(&handshake.open(&host_sealed).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let _ = socket.close(None).await;
        let secrets = handshake.into_session().map_err(|e| e.to_string())?;

        let host_id = EndpointId::from_bytes(&host_hello.endpoint_id).map_err(|e| e.to_string())?;
        let addr = EndpointAddr::from_parts(
            host_id,
            host_hello
                .direct_addrs
                .iter()
                .copied()
                .map(TransportAddr::Ip),
        );
        let connection = tokio::time::timeout(PATIENCE, endpoint.connect(addr, ALPN_CONTROL))
            .await
            .map_err(|_| "dial timed out".to_string())?
            .map_err(|e| e.to_string())?;

        let (endpoint_a, endpoint_b) = (host_hello.endpoint_id, *endpoint.id().as_bytes());
        let mut mac = secrets.bind_tag(Role::B, &endpoint_a, &endpoint_b);
        if misbehave.wrong_bind {
            mac[0] ^= 0xff;
        }
        let (mut control_send, mut control_recv) =
            connection.open_bi().await.map_err(|e| e.to_string())?;
        let bind = ControlMessage::Bind { mac }.to_frame().unwrap();
        control_send
            .write_all(&bind)
            .await
            .map_err(|e| e.to_string())?;
        let reply = read_control(&mut control_recv)
            .await
            .ok_or("no Bind from the host")?;
        let ControlMessage::Bind { mac: host_mac } = reply else {
            return Err(format!("expected Bind, got {}", reply.name()));
        };
        if !secrets.verify_bind(&endpoint_a, &endpoint_b, &host_mac) {
            return Err("the host's Bind did not verify".into());
        }
        Ok(Evil {
            endpoint,
            connection,
            control_send,
            control_recv,
            secrets,
        })
    }

    pub async fn send(&mut self, message: &ControlMessage) {
        let frame = frame::encode(message, usize::MAX).expect("encode");
        let _ = self.control_send.write_all(&frame).await;
    }

    pub async fn send_raw(&mut self, bytes: &[u8]) {
        let _ = self.control_send.write_all(bytes).await;
    }

    /// The next control message, or `None` once the victim closed or went quiet.
    pub async fn recv(&mut self) -> Option<ControlMessage> {
        read_control(&mut self.control_recv).await
    }

    /// Reads until a message the closure accepts arrives.
    pub async fn recv_until<T>(
        &mut self,
        mut pick: impl FnMut(&ControlMessage) -> Option<T>,
    ) -> Option<T> {
        loop {
            let message = self.recv().await?;
            if let Some(found) = pick(&message) {
                return Some(found);
            }
        }
    }

    /// Completes the safety step as an attacker who simply clicks "match".
    pub async fn confirm_safety(&mut self) {
        self.send(&ControlMessage::SafetyConfirmed).await;
    }

    /// Waits for the victim to request a file.
    pub async fn next_get(&mut self) -> Option<(SendStream, RecvStream, Get)> {
        let (send, mut recv) = tokio::time::timeout(PATIENCE, self.connection.accept_bi())
            .await
            .ok()?
            .ok()?;
        let mut header = [0u8; 4];
        recv.read_exact(&mut header).await.ok()?;
        let len = frame::payload_len(header, limits::MAX_DATA_FRAME).ok()?;
        let mut payload = vec![0u8; len];
        recv.read_exact(&mut payload).await.ok()?;
        let get: Get = frame::decode_payload(&payload).ok()?;
        Some((send, recv, get))
    }

    /// Asks the victim for a file, as a receiver would. Returns its answer and
    /// however many body bytes followed.
    pub async fn get(&mut self, get: Get) -> Option<(DataHeader, usize)> {
        let (mut send, mut recv) = self.connection.open_bi().await.ok()?;
        send.write_all(&get.to_frame().unwrap()).await.ok()?;
        let _ = send.finish();
        let mut header = [0u8; 4];
        tokio::time::timeout(PATIENCE, recv.read_exact(&mut header))
            .await
            .ok()?
            .ok()?;
        let len = frame::payload_len(header, limits::MAX_DATA_FRAME).ok()?;
        let mut payload = vec![0u8; len];
        recv.read_exact(&mut payload).await.ok()?;
        let header: DataHeader = frame::decode_payload(&payload).ok()?;
        let body = tokio::time::timeout(PATIENCE, recv.read_to_end(64 * 1024 * 1024))
            .await
            .ok()?
            .unwrap_or_default();
        Some((header, body.len()))
    }

    /// True once the victim has closed the connection.
    pub async fn closed(&self) -> bool {
        tokio::time::timeout(PATIENCE, self.connection.closed())
            .await
            .is_ok()
    }
}

async fn read_control(recv: &mut RecvStream) -> Option<ControlMessage> {
    let read = async {
        let mut header = [0u8; 4];
        recv.read_exact(&mut header).await.ok()?;
        let len = frame::payload_len(header, limits::MAX_CONTROL_FRAME).ok()?;
        let mut payload = vec![0u8; len];
        recv.read_exact(&mut payload).await.ok()?;
        ControlMessage::from_payload(&payload).ok()
    };
    tokio::time::timeout(PATIENCE, read).await.ok().flatten()
}

/// Sends a file's header, body and trailer on a data stream, exactly as given.
pub async fn serve(
    send: &mut SendStream,
    header: DataHeader,
    body: &[u8],
    trailer: &[u8],
    extra: &[u8],
) {
    let _ = send.write_all(&header.to_frame().unwrap()).await;
    let _ = send.write_all(body).await;
    let _ = send.write_all(trailer).await;
    let _ = send.write_all(extra).await;
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(5), send.stopped()).await;
}
