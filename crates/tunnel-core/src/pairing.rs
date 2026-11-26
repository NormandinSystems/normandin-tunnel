//! Pairing: from a connection code to verified session secrets.
//!
//! Runs steps 1–7 of the pairing protocol through a [`Room`]: a room on a
//! rendezvous server, or a meeting point on the relay network when there is no
//! server. The steps and their guarantees are the same either way. See
//! `docs/PROTOCOL.md`.

use tunnel_crypto::{ConnectionCode, Handshake, PakeState, Role, SessionSecrets};
use tunnel_proto::hello::{Hello, PairingMessage};
use tunnel_proto::limits;
use tunnel_proto::version::{self, SUPPORTED_VERSIONS};

use crate::error::{Error, Result};
use crate::meeting;
use crate::rendezvous::{self, RoomEvent};

/// Where the two devices exchange their pairing messages. Whatever carries
/// them is assumed hostile: nothing here is trusted until key confirmation.
pub(crate) enum Room {
    /// A room on a rendezvous server.
    Server(Box<rendezvous::Room>),
    /// This device's own meeting point (it is showing the code).
    Host(meeting::Host),
    /// Another device's meeting point (this device typed the code).
    Guest(meeting::Guest),
}

impl Room {
    async fn post(&mut self, message: &PairingMessage) -> Result<()> {
        match self {
            Room::Server(room) => room.post(message).await,
            Room::Host(room) => room.post(message).await,
            Room::Guest(room) => room.post(message).await,
        }
    }

    async fn next(&mut self) -> Result<RoomEvent> {
        match self {
            Room::Server(room) => room.next().await,
            Room::Host(room) => room.next().await.map(RoomEvent::Message),
            Room::Guest(room) => room.next().await.map(RoomEvent::Message),
        }
    }

    /// Leaves, whatever happened. A meeting point goes off the network.
    pub(crate) async fn close(self) {
        match self {
            Room::Server(room) => room.close().await,
            Room::Host(room) => room.close().await,
            Room::Guest(room) => room.close().await,
        }
    }
}

/// The result of a successful pairing.
pub(crate) struct Paired {
    /// Which side this device was.
    pub role: Role,
    /// Channel-binding, safety-code and trust keys.
    pub secrets: SessionSecrets,
    /// What the other device said about itself.
    pub peer: Hello,
    /// The protocol version both sides will speak.
    #[allow(dead_code)]
    pub version: u16,
}

async fn expect_message(room: &mut Room) -> Result<PairingMessage> {
    loop {
        match room.next().await? {
            RoomEvent::Message(message) => return Ok(message),
            RoomEvent::PeerJoined => continue,
        }
    }
}

/// Host side (role A), after the code is on screen: waits for the other
/// device's SPAKE2 message. This is the only step that waits on a person.
pub(crate) async fn host_wait_for_peer(room: &mut Room, pake: PakeState) -> Result<Handshake> {
    match expect_message(room).await? {
        PairingMessage::Pake(message) => pake.finish(&message).map_err(|_| Error::CodeFailed),
        _ => Err(Error::CodeFailed),
    }
}

/// Guest side (role B), once in the room: reads A's SPAKE2 message and answers.
pub(crate) async fn guest_start(room: &mut Room, code: &ConnectionCode) -> Result<Handshake> {
    let host_message = match expect_message(room).await? {
        PairingMessage::Pake(message) => message,
        _ => return Err(Error::CodeFailed),
    };
    let (pake, own_message) = PakeState::start(Role::B, code);
    room.post(&PairingMessage::Pake(own_message)).await?;
    pake.finish(&host_message).map_err(|_| Error::CodeFailed)
}

/// Both sides, steps 5–7: key confirmation, then the sealed hello exchange.
///
/// Any failure here means the codes differed or someone interfered. Either way
/// the room is abandoned and the code is dead.
pub(crate) async fn confirm_and_exchange(
    room: &mut Room,
    mut handshake: Handshake,
    own_hello: &Hello,
) -> Result<Paired> {
    let exchange = async {
        room.post(&PairingMessage::Confirm(handshake.confirmation()))
            .await?;
        match expect_message(room).await? {
            PairingMessage::Confirm(tag) => {
                handshake.verify_confirmation(&tag).map_err(|_| {
                    tracing::debug!("the other device's key confirmation is wrong");
                    Error::CodeFailed
                })?;
            }
            _ => return Err(Error::CodeFailed),
        }

        let plain = own_hello.encode().map_err(Error::internal)?;
        let sealed = handshake.seal(&plain).map_err(Error::internal)?;
        room.post(&PairingMessage::Hello(sealed)).await?;
        let peer = match expect_message(room).await? {
            PairingMessage::Hello(sealed) => {
                let plain = handshake.open(&sealed).map_err(|_| Error::CodeFailed)?;
                Hello::decode(&plain).map_err(|_| Error::Protocol)?
            }
            _ => return Err(Error::CodeFailed),
        };
        Ok(peer)
    };
    let peer = tokio::time::timeout(limits::HELLO_TIMEOUT, exchange)
        .await
        .map_err(|_| Error::CodeFailed)??;

    let version =
        version::negotiate(SUPPORTED_VERSIONS, &peer.versions).ok_or(Error::VersionMismatch)?;
    let role = handshake.role();
    // K, the confirm keys and the mailbox keys are wiped here.
    let secrets = handshake.into_session().map_err(Error::internal)?;
    Ok(Paired {
        role,
        secrets,
        peer,
        version,
    })
}
