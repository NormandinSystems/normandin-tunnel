//! Pairing without a rendezvous server.
//!
//! The device showing a code opens a *meeting point*: an endpoint on the relay
//! network whose key is derived from the code's public nameplate. The device
//! typing the code derives the same key and dials it through the relays. The
//! two then exchange exactly the messages a rendezvous room would carry.
//!
//! The meeting point is as untrusted as a rendezvous server. Its key follows
//! from four public digits, so anyone can dial it, and anyone can pretend to
//! be it. Neither helps an attacker: the three words never enter the key, the
//! messages are SPAKE2 messages that reveal nothing about the words, and the
//! host answers exactly one device, once. A wrong answer ends the code. The
//! session itself runs on separate endpoints with fresh random keys, tied to
//! the pairing by the channel binding.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use iroh::endpoint::{Connection, ConnectionError, RecvStream, SendStream, VarInt};
use iroh::{Endpoint, Watcher};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;
use tunnel_proto::frame;
use tunnel_proto::hello::PairingMessage;
use tunnel_proto::meeting::{self, close};

use crate::error::{Error, Result};
use crate::net::{self, RelayChoice};
use crate::wire;

/// How long one pairing frame may take to arrive once pairing is under way.
const FRAME_TIMEOUT: Duration = Duration::from_secs(45);
/// How long to wait for the last pairing message to be delivered before
/// closing the connection.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the QUIC handshake of an arriving device may take.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// One device at the meeting point: its connection and the pairing stream.
struct Peer {
    connection: Connection,
    send: SendStream,
    recv: RecvStream,
}

impl Peer {
    async fn write(&mut self, message: &PairingMessage) -> Result<()> {
        wire::write_message(&mut self.send, message, meeting::MAX_FRAME)
            .await
            .map_err(|error| {
                tracing::debug!(%error, "could not send a pairing message");
                closed_error(&self.connection)
            })
    }

    async fn read(&mut self) -> Result<PairingMessage> {
        wire::read_message(&mut self.recv, meeting::MAX_FRAME, FRAME_TIMEOUT)
            .await
            .map_err(|error| {
                tracing::debug!(%error, "no pairing message arrived");
                closed_error(&self.connection)
            })
    }

    /// Ends the pairing stream without cutting off the last message: closing
    /// a QUIC connection discards whatever has not been delivered yet, so wait
    /// (briefly) until the other side has received everything we sent.
    async fn close(mut self) {
        let _ = self.send.finish();
        let _ = tokio::time::timeout(DELIVERY_TIMEOUT, self.send.stopped()).await;
        self.connection.close(VarInt::from_u32(close::DONE), b"");
    }
}

/// Why pairing stopped, judged by how the other side closed the connection.
fn closed_error(connection: &Connection) -> Error {
    match connection.close_reason() {
        Some(ConnectionError::ApplicationClosed(closed)) => {
            match u32::try_from(closed.error_code.into_inner()).unwrap_or(u32::MAX) {
                close::IN_USE => Error::CodeInUse,
                close::BUSY => Error::RendezvousBusy,
                _ => Error::CodeFailed,
            }
        }
        _ => Error::CodeFailed,
    }
}

/// The first answer to the host's greeting, with the device that sent it.
struct Answer {
    peer: Peer,
    message: PairingMessage,
}

/// The host side: the open meeting point.
pub(crate) struct Host {
    endpoint: Endpoint,
    answers: mpsc::Receiver<Answer>,
    acceptor: JoinHandle<()>,
    peer: Option<Peer>,
}

/// The guest side: a connection to someone's meeting point.
pub(crate) struct Guest {
    peer: Peer,
}

impl Host {
    /// Opens the meeting point for `nameplate` and starts greeting arriving
    /// devices with `greeting` (the host's SPAKE2 message).
    ///
    /// Returns once a relay is reachable, so that a code is only ever shown
    /// when it can actually be used.
    pub(crate) async fn open(
        relay: &RelayChoice,
        nameplate: u16,
        greeting: &PairingMessage,
    ) -> Result<Self> {
        if relay.urls()?.is_empty() {
            return Err(Error::RendezvousUnreachable);
        }
        let greeting = frame::encode(greeting, meeting::MAX_FRAME).map_err(Error::internal)?;
        let endpoint = net::bind_meeting_point(relay, nameplate).await?;
        if tokio::time::timeout(meeting::ONLINE_TIMEOUT, endpoint.online())
            .await
            .is_err()
        {
            endpoint.close().await;
            return Err(Error::RendezvousUnreachable);
        }
        let (sender, answers) = mpsc::channel(1);
        let acceptor = tokio::spawn(greet_arrivals(endpoint.clone(), greeting, sender));
        Ok(Self {
            endpoint,
            answers,
            acceptor,
            peer: None,
        })
    }

    /// Waits for the next pairing message.
    ///
    /// The first call waits for a device to arrive and answer the greeting.
    /// That device is the only one this code will ever talk to.
    pub(crate) async fn next(&mut self) -> Result<PairingMessage> {
        if let Some(peer) = self.peer.as_mut() {
            return peer.read().await;
        }
        match self.answers.recv().await {
            Some(Answer { peer, message }) => {
                self.peer = Some(peer);
                Ok(message)
            }
            None => Err(Error::Internal("the meeting point stopped".into())),
        }
    }

    pub(crate) async fn post(&mut self, message: &PairingMessage) -> Result<()> {
        match self.peer.as_mut() {
            Some(peer) => peer.write(message).await,
            None => Err(Error::Internal("no device has answered yet".into())),
        }
    }

    /// Ends pairing and takes the meeting point off the network.
    pub(crate) async fn close(mut self) {
        self.acceptor.abort();
        if let Some(peer) = self.peer.take() {
            peer.close().await;
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), self.endpoint.close()).await;
    }
}

impl Drop for Host {
    /// A cancelled or expired code must not stay reachable: if the room is
    /// dropped without `close`, the endpoint is still shut down.
    fn drop(&mut self) {
        self.acceptor.abort();
        if self.endpoint.is_closed() {
            return;
        }
        let endpoint = self.endpoint.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { endpoint.close().await });
        }
    }
}

/// Greets every device that reaches the meeting point and hands the first one
/// to answer to the host. Everyone after that is told the code is in use.
async fn greet_arrivals(endpoint: Endpoint, greeting: Vec<u8>, answers: mpsc::Sender<Answer>) {
    let greeting: Arc<[u8]> = greeting.into();
    let claimed = Arc::new(AtomicBool::new(false));
    let waiting = Arc::new(Semaphore::new(meeting::MAX_WAITING));
    // Connections still shaking hands, or about to be turned away, are
    // bounded too: a flood is refused outright instead of piling up.
    let arriving = Arc::new(Semaphore::new(4 * meeting::MAX_WAITING));
    while let Some(incoming) = endpoint.accept().await {
        let Ok(_arrival) = arriving.clone().try_acquire_owned() else {
            incoming.refuse();
            continue;
        };
        let greeting = greeting.clone();
        let claimed = claimed.clone();
        let waiting = waiting.clone();
        let answers = answers.clone();
        tokio::spawn(async move {
            let _arrival = _arrival;
            let Ok(Ok(connection)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, incoming).await else {
                return;
            };
            if claimed.load(Ordering::SeqCst) {
                connection.close(VarInt::from_u32(close::IN_USE), b"");
                return;
            }
            // A device that connects and says nothing must not keep others out
            // for long, and only so many may wait at once.
            let Ok(_place) = waiting.try_acquire_owned() else {
                connection.close(VarInt::from_u32(close::BUSY), b"");
                return;
            };
            let greeted = async {
                let (mut send, mut recv) = connection.open_bi().await.ok()?;
                send.write_all(&greeting).await.ok()?;
                let message: PairingMessage =
                    wire::read_message(&mut recv, meeting::MAX_FRAME, meeting::ANSWER_TIMEOUT)
                        .await
                        .ok()?;
                Some((send, recv, message))
            };
            match greeted.await {
                Some((send, recv, message)) => {
                    // The first answer is the one attempt this code allows.
                    if claimed.swap(true, Ordering::SeqCst) {
                        connection.close(VarInt::from_u32(close::IN_USE), b"");
                        return;
                    }
                    let peer = Peer {
                        connection,
                        send,
                        recv,
                    };
                    let _ = answers.send(Answer { peer, message }).await;
                }
                None => connection.close(VarInt::from_u32(close::TOO_SLOW), b""),
            }
        });
    }
}

impl Guest {
    /// Dials the meeting point for `nameplate` from this device's session
    /// endpoint and waits for the host to open the pairing stream.
    pub(crate) async fn join(
        endpoint: &Endpoint,
        relay: &RelayChoice,
        nameplate: u16,
    ) -> Result<Self> {
        let addr = net::meeting_addr(relay, nameplate)?;
        if addr.addrs.is_empty() {
            return Err(Error::RendezvousUnreachable);
        }
        let dialing = endpoint.connect(addr, meeting::ALPN);
        let connection = match tokio::time::timeout(meeting::DIAL_TIMEOUT, dialing).await {
            Ok(Ok(connection)) => connection,
            // Nobody answered. If this device reached a relay, the code is
            // wrong or gone; if not, the network is the problem.
            Ok(Err(_)) | Err(_) => {
                let online = endpoint.watch_addr().get().relay_urls().next().is_some();
                return Err(if online {
                    Error::CodeNotFound
                } else {
                    Error::RendezvousUnreachable
                });
            }
        };
        let (send, recv) =
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, connection.accept_bi()).await {
                Ok(Ok(streams)) => streams,
                Ok(Err(_)) | Err(_) => {
                    let error = closed_error(&connection);
                    connection.close(VarInt::from_u32(close::DONE), b"");
                    return Err(error);
                }
            };
        Ok(Self {
            peer: Peer {
                connection,
                send,
                recv,
            },
        })
    }

    pub(crate) async fn next(&mut self) -> Result<PairingMessage> {
        self.peer.read().await
    }

    pub(crate) async fn post(&mut self, message: &PairingMessage) -> Result<()> {
        self.peer.write(message).await
    }

    pub(crate) async fn close(self) {
        self.peer.close().await;
    }
}
