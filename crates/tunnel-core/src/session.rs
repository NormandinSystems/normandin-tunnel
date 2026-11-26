//! One session, from code to goodbye.
//!
//! A session is a single task that owns all of its state. It pairs through the
//! rendezvous server, connects the two devices, proves the connection belongs
//! to the party that knew the code (channel binding), waits for both people to
//! confirm the safety code, and then runs the symmetric transfer loop until
//! someone ends it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh::endpoint::{Connection, ConnectionError, RecvStream, SendStream, VarInt};
use iroh::{Endpoint, EndpointAddr, EndpointId};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tunnel_crypto::{ConnectionCode, PairUri, PakeState, Role, SessionSecrets};
use tunnel_proto::control::{
    CancelReason, Completed, ControlMessage, DeclineReason, ErrorCode, Offer, ResumeEntry,
    TransferId,
};
use tunnel_proto::data::close;
use tunnel_proto::hello::{DeviceType, Hello, PairingMessage, sanitize_device_name};
use tunnel_proto::limits;
use tunnel_proto::manifest::{ManifestPage, Paginator};
use tunnel_proto::version::SUPPORTED_VERSIONS;

use crate::error::{Error, ErrorView, Result};
use crate::events::{
    Direction, EndReason, Event, Issue, OfferView, OfferWarning, PathKind, PausedBy, PeerInfo,
    SessionState, Snapshot, TextView, TransferState, TransferView, device_type_name,
};
use crate::net::{self, RelayChoice};
use crate::pairing::Room;
use crate::pairing::{self, Paired};
use crate::pathsafe::Destination;
use crate::platform;
use crate::store::{HistoryEntry, PageStats, Store};
use crate::transfer::recv::{self, DownloadEnd, IncomingShared, Notice};
use crate::transfer::send::{OutgoingShared, SenderHub};
use crate::transfer::walk::{self, WalkResult};
use crate::transfer::{Counters, IssueLog, Run, reason};
use crate::wire;
use crate::{meeting, rendezvous};

/// Reply channel of a command.
pub(crate) type Reply<T> = oneshot::Sender<Result<T>>;

/// What the app can ask a session to do.
pub(crate) enum Command {
    ConfirmSafety {
        matches: bool,
        reply: Reply<()>,
    },
    SendPaths {
        paths: Vec<PathBuf>,
        reply: Reply<String>,
    },
    SendText {
        text: String,
        reply: Reply<()>,
    },
    RespondOffer {
        id: TransferId,
        accept: bool,
        folder: Option<PathBuf>,
        reply: Reply<()>,
    },
    Pause {
        id: TransferId,
        reply: Reply<()>,
    },
    Resume {
        id: TransferId,
        reply: Reply<()>,
    },
    Cancel {
        id: TransferId,
        reply: Reply<()>,
    },
    End {
        reply: Reply<()>,
    },
    /// Test hook: drop the connection as a network failure would.
    DropConnection,
}

/// How a session begins.
pub(crate) enum Start {
    /// "Show a code".
    Host,
    /// "Enter a code".
    Guest {
        code: ConnectionCode,
        /// The pairing server to use, or `None` to meet on the relay network.
        rendezvous_url: Option<String>,
    },
}

/// Publishes events and keeps the snapshot in step with them.
#[derive(Clone)]
pub(crate) struct Emitter {
    pub events: tokio::sync::broadcast::Sender<Event>,
    pub snapshot: Arc<Mutex<Snapshot>>,
}

impl Emitter {
    pub(crate) fn emit(&self, event: Event) {
        self.snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .apply(&event);
        let _ = self.events.send(event);
    }

    fn session(&self, session: SessionState) {
        self.emit(Event::Session { session });
    }
}

/// What a session needs from the app around it.
#[derive(Clone)]
pub(crate) struct Env {
    pub store: Store,
    pub emitter: Emitter,
    pub device_type: DeviceType,
    pub app_version: String,
    pub relay: Option<RelayChoice>,
    pub rendezvous_url: Option<String>,
    pub include_loopback: bool,
}

/// Messages from the control-stream reader to the session loop.
enum Inbound {
    Message(Box<ControlMessage>),
    /// The peer sent something that cannot be decoded or breaks a limit.
    Malformed,
    Closed(Closed),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Closed {
    /// The other device said goodbye.
    Bye,
    /// The other device closed with an error code.
    PeerError(u32),
    /// The connection was lost.
    Lost,
}

/// The tasks and channels tied to one live connection.
struct Link {
    connection: Connection,
    /// Small, urgent messages from the session loop. Never blocks.
    control: mpsc::UnboundedSender<ControlMessage>,
    /// Bulk messages (manifest pages), with backpressure.
    bulk: mpsc::Sender<ControlMessage>,
    inbound: mpsc::Receiver<Inbound>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Link {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Results of background work, delivered to the session loop.
enum Internal {
    Walked {
        id: TransferId,
        result: Result<WalkResult>,
    },
    PagesSent {
        id: TransferId,
        sent: bool,
    },
    Downloaded {
        id: TransferId,
        end: DownloadEnd,
    },
}

struct Meter {
    last_bytes: u64,
    last_at: Instant,
    speed: f64,
}

impl Meter {
    fn new() -> Self {
        Self {
            last_bytes: 0,
            last_at: Instant::now(),
            speed: 0.0,
        }
    }

    /// Smoothed bytes per second.
    fn update(&mut self, bytes: u64) -> u64 {
        let elapsed = self.last_at.elapsed().as_secs_f64();
        if elapsed >= 0.2 {
            let instant = bytes.saturating_sub(self.last_bytes) as f64 / elapsed;
            self.speed = if self.speed == 0.0 {
                instant
            } else {
                self.speed * 0.7 + instant * 0.3
            };
            self.last_bytes = bytes;
            self.last_at = Instant::now();
        }
        self.speed as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutPhase {
    Preparing,
    /// Offer sent, manifest pages on their way.
    Offering,
    Offered,
    Active,
    Final,
}

struct Outgoing {
    view: TransferView,
    shared: Arc<OutgoingShared>,
    phase: OutPhase,
    cancel_walk: Arc<AtomicBool>,
    task: Option<JoinHandle<()>>,
    meter: Meter,
    local_paused: bool,
    peer_paused: bool,
    /// Sent again after a reconnect, in case it was lost.
    last_word: Option<ControlMessage>,
}

enum InPhase {
    /// Manifest pages are still arriving.
    Manifest {
        next_page: u32,
        next_file_id: u32,
        bytes_seen: u64,
    },
    /// Complete and waiting for the person.
    Pending,
    Active {
        shared: Arc<IncomingShared>,
        run: watch::Sender<Run>,
        task: JoinHandle<()>,
    },
    Final,
}

struct Incoming {
    offer: Offer,
    stats: PageStats,
    phase: InPhase,
    view: Option<TransferView>,
    meter: Meter,
    local_paused: bool,
    peer_paused: bool,
    disk_full: bool,
    cancelled: bool,
    last_word: Option<ControlMessage>,
}

/// Why the session loop stopped.
enum Exit {
    Local,
    Peer,
    Failed(Error),
}

struct Session {
    env: Env,
    endpoint: Endpoint,
    role: Role,
    secrets: SessionSecrets,
    endpoint_a: [u8; 32],
    endpoint_b: [u8; 32],
    peer: PeerInfo,
    peer_addr: EndpointAddr,
    peer_id: EndpointId,
    connections: watch::Sender<Option<Connection>>,
    link: Option<Link>,
    hub: Arc<SenderHub>,
    outgoing: HashMap<TransferId, Outgoing>,
    incoming: HashMap<TransferId, Incoming>,
    internal_tx: mpsc::Sender<Internal>,
    internal_rx: mpsc::Receiver<Internal>,
    notices_tx: mpsc::Sender<(TransferId, Notice)>,
    notices_rx: mpsc::Receiver<(TransferId, Notice)>,
    next_text: u64,
    path: PathKind,
}

/// Runs a whole session. Returns when it is over.
pub(crate) async fn run(env: Env, start: Start, mut commands: mpsc::Receiver<Command>) {
    let slot: Arc<Mutex<Option<Endpoint>>> = Arc::new(Mutex::new(None));
    let established = tokio::select! {
        result = establish(&env, start, slot.clone()) => result,
        () = wait_for_end(&mut commands) => Err(Error::Cancelled),
    };
    let mut session = match established {
        Ok(session) => session,
        Err(error) => {
            let endpoint = slot.lock().unwrap_or_else(|p| p.into_inner()).take();
            if let Some(endpoint) = endpoint {
                endpoint.close().await;
            }
            env.emitter.session(ended(&error, None));
            return;
        }
    };

    let exit = match session.verify_safety(&mut commands).await {
        Ok(()) => session.run_active(&mut commands).await,
        Err(exit) => exit,
    };
    session.teardown(exit).await;
}

fn ended(error: &Error, peer: Option<PeerInfo>) -> SessionState {
    match error {
        Error::Cancelled => SessionState::Ended {
            reason: EndReason::Local,
            error: None,
            peer,
        },
        Error::PeerEnded => SessionState::Ended {
            reason: EndReason::Peer,
            error: None,
            peer,
        },
        other => SessionState::Ended {
            reason: EndReason::Error,
            error: Some(other.view()),
            peer,
        },
    }
}

/// During pairing only "end" makes sense; everything else is refused.
async fn wait_for_end(commands: &mut mpsc::Receiver<Command>) {
    loop {
        match commands.recv().await {
            None => return,
            Some(Command::End { reply }) => {
                let _ = reply.send(Ok(()));
                return;
            }
            Some(command) => refuse(command, Error::NoSession),
        }
    }
}

fn refuse(command: Command, error: Error) {
    match command {
        Command::ConfirmSafety { reply, .. }
        | Command::SendText { reply, .. }
        | Command::RespondOffer { reply, .. }
        | Command::Pause { reply, .. }
        | Command::Resume { reply, .. }
        | Command::Cancel { reply, .. }
        | Command::End { reply } => {
            let _ = reply.send(Err(error));
        }
        Command::SendPaths { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        Command::DropConnection => {}
    }
}

/// The pairing link as an SVG QR code. It uses `currentColor`, so the app's
/// theme decides its colour.
fn qr_svg(text: &str) -> String {
    use qrcode::render::svg;
    qrcode::QrCode::new(text.as_bytes())
        .map(|code| {
            code.render::<svg::Color<'_>>()
                .min_dimensions(240, 240)
                .quiet_zone(false)
                .dark_color(svg::Color("currentColor"))
                .light_color(svg::Color("none"))
                .build()
        })
        .unwrap_or_default()
}

/// The pairing server to use, if any: the one the engine was started with,
/// else the one in settings. `None` (the default) means the devices meet on
/// the relay network instead.
pub(crate) fn configured_rendezvous(env: &Env) -> Option<String> {
    env.rendezvous_url
        .clone()
        .or_else(|| Some(env.store.settings().rendezvous_url))
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
}

/// Pairing, connecting and channel binding.
async fn establish(env: &Env, start: Start, slot: Arc<Mutex<Option<Endpoint>>>) -> Result<Session> {
    let settings = env.store.settings();
    let relay = env
        .relay
        .clone()
        .unwrap_or_else(|| RelayChoice::from_urls(&settings.relay_urls));

    env.emitter.session(match &start {
        Start::Host => SessionState::CreatingCode,
        Start::Guest { .. } => SessionState::Joining,
    });

    let endpoint = net::bind_endpoint(&relay).await?;
    *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(endpoint.clone());
    // Work out how we can be reached while the pairing proceeds.
    let reach = {
        let endpoint = endpoint.clone();
        let relay = relay.clone();
        let include_loopback = env.include_loopback;
        tokio::spawn(async move { net::reachability(&endpoint, &relay, include_loopback).await })
    };

    let (mut room, handshake) = match start {
        Start::Host => {
            // With a pairing server, it assigns the nameplate and holds our
            // first message. Without one, this device picks the nameplate and
            // waits at the meeting point derived from it.
            let (room, pake, code, host, expires_in) = match configured_rendezvous(env) {
                Some(configured) => {
                    let (url, host) = crate::rendezvous::check_url(&configured)?;
                    let mut room = rendezvous::Room::connect(&url).await?;
                    let (nameplate, expires_in) = room.create().await?;
                    let code = ConnectionCode::generate(nameplate).map_err(Error::internal)?;
                    let (pake, message) = PakeState::start(Role::A, &code);
                    room.post(&PairingMessage::Pake(message)).await?;
                    (
                        Room::Server(Box::new(room)),
                        pake,
                        code,
                        Some(host),
                        expires_in,
                    )
                }
                None => {
                    let code = ConnectionCode::generate_unassigned().map_err(Error::internal)?;
                    let (pake, message) = PakeState::start(Role::A, &code);
                    let greeting = PairingMessage::Pake(message);
                    let room = meeting::Host::open(&relay, code.nameplate(), &greeting).await?;
                    let lifetime = limits::CODE_LIFETIME.as_secs();
                    (Room::Host(room), pake, code, None, lifetime)
                }
            };
            let mut room = room;

            let uri = PairUri {
                code: code.clone(),
                rendezvous_host: host,
            }
            .to_string();
            let lifetime = Duration::from_secs(expires_in.min(limits::CODE_LIFETIME.as_secs()));
            env.emitter.session(SessionState::ShowingCode {
                code: code.to_string(),
                qr_svg: qr_svg(&uri),
                uri,
                expires_at: platform::now_ms() + lifetime.as_millis() as i64,
            });
            // A server keeps the room a few seconds past the lifetime it
            // announced; our own meeting point closes exactly on time.
            let grace = match room {
                Room::Server(_) => Duration::from_secs(5),
                _ => Duration::ZERO,
            };
            let waiting = pairing::host_wait_for_peer(&mut room, pake);
            let waited = tokio::time::timeout(lifetime + grace, waiting).await;
            let handshake = match waited {
                Ok(Ok(handshake)) => handshake,
                Ok(Err(error)) => {
                    room.close().await;
                    return Err(error);
                }
                Err(_) => {
                    room.close().await;
                    return Err(Error::CodeExpired);
                }
            };
            (room, handshake)
        }
        Start::Guest {
            code,
            rendezvous_url,
        } => {
            let mut room = match rendezvous_url {
                Some(url) => {
                    let mut room = rendezvous::Room::connect(&url).await?;
                    room.join(code.nameplate()).await?;
                    Room::Server(Box::new(room))
                }
                None => {
                    Room::Guest(meeting::Guest::join(&endpoint, &relay, code.nameplate()).await?)
                }
            };
            let handshake = match pairing::guest_start(&mut room, &code).await {
                Ok(handshake) => handshake,
                Err(error) => {
                    room.close().await;
                    return Err(error);
                }
            };
            (room, handshake)
        }
    };
    env.emitter.session(SessionState::Securing);

    let (relay_url, direct_addrs) = reach.await.map_err(Error::internal)?;
    let own_hello = Hello {
        versions: SUPPORTED_VERSIONS.to_vec(),
        endpoint_id: *endpoint.id().as_bytes(),
        relay_url,
        direct_addrs,
        device_name: sanitize_device_name(&settings.device_name),
        device_type: env.device_type,
        app_version: env
            .app_version
            .chars()
            .take(limits::MAX_APP_VERSION_BYTES)
            .collect(),
        capabilities: 0,
    };
    let paired = pairing::confirm_and_exchange(&mut room, handshake, &own_hello).await;
    // Step 7: both leave the room, whatever happened.
    room.close().await;
    let Paired {
        role,
        secrets,
        peer: peer_hello,
        ..
    } = paired?;

    env.emitter.session(SessionState::Connecting);
    let own_id = *endpoint.id().as_bytes();
    if peer_hello.endpoint_id == own_id {
        return Err(Error::Protocol);
    }
    let peer_addr = net::peer_addr(&peer_hello)?;
    let peer_id = peer_addr.id;
    let (endpoint_a, endpoint_b) = match role {
        Role::A => (own_id, peer_hello.endpoint_id),
        Role::B => (peer_hello.endpoint_id, own_id),
    };
    // Step 8: B dials A; A accepts only the endpoint it was told about.
    let connection = match role {
        Role::A => net::accept_from(&endpoint, peer_id, limits::DIAL_TIMEOUT).await?,
        Role::B => net::dial(&endpoint, peer_addr.clone(), limits::DIAL_TIMEOUT).await?,
    };
    // Step 9: channel binding.
    let streams = bind(&connection, role, &secrets, &endpoint_a, &endpoint_b).await?;

    let (internal_tx, internal_rx) = mpsc::channel(64);
    let (notices_tx, notices_rx) = mpsc::channel(16);
    let (connections, _) = watch::channel(None);
    let hub = SenderHub::new(env.store.clone());
    let mut session = Session {
        env: env.clone(),
        endpoint,
        role,
        secrets,
        endpoint_a,
        endpoint_b,
        peer: PeerInfo {
            name: sanitize_device_name(&peer_hello.device_name),
            device_type: device_type_name(peer_hello.device_type),
            app_version: peer_hello
                .app_version
                .chars()
                .filter(|c| !c.is_control())
                .collect(),
        },
        peer_addr,
        peer_id,
        connections,
        link: None,
        hub,
        outgoing: HashMap::new(),
        incoming: HashMap::new(),
        internal_tx,
        internal_rx,
        notices_tx,
        notices_rx,
        next_text: 0,
        path: PathKind::Unknown,
    };
    session.attach(connection, streams);
    Ok(session)
}

/// Exchanges the `Bind` frames on a new connection's control stream.
///
/// B opens the stream and sends first; A answers. A wrong tag means the QUIC
/// peer is not the party that knew the code: the connection is closed.
async fn bind(
    connection: &Connection,
    role: Role,
    secrets: &SessionSecrets,
    endpoint_a: &[u8; 32],
    endpoint_b: &[u8; 32],
) -> Result<(SendStream, RecvStream)> {
    let own = ControlMessage::Bind {
        mac: secrets.bind_tag(role, endpoint_a, endpoint_b),
    };
    let frame = own.to_frame().map_err(Error::internal)?;
    let exchange = async {
        let (mut send, mut recv) = match role {
            Role::B => {
                let (mut send, recv) = connection
                    .open_bi()
                    .await
                    .map_err(|_| Error::PeerUnreachable)?;
                send.write_all(&frame)
                    .await
                    .map_err(|_| Error::PeerUnreachable)?;
                (send, recv)
            }
            Role::A => connection
                .accept_bi()
                .await
                .map_err(|_| Error::PeerUnreachable)?,
        };
        let payload = wire::read_payload(&mut recv, limits::MAX_CONTROL_FRAME)
            .await
            .map_err(|_| Error::BindFailed)?;
        let tag = match ControlMessage::from_payload(&payload) {
            Ok(ControlMessage::Bind { mac }) => mac,
            _ => return Err(Error::BindFailed),
        };
        if !secrets.verify_bind(endpoint_a, endpoint_b, &tag) {
            return Err(Error::BindFailed);
        }
        if role == Role::A {
            send.write_all(&frame)
                .await
                .map_err(|_| Error::PeerUnreachable)?;
        }
        Ok((send, recv))
    };
    match tokio::time::timeout(limits::BIND_TIMEOUT, exchange).await {
        Ok(Ok(streams)) => Ok(streams),
        Ok(Err(error)) => {
            if error == Error::BindFailed {
                connection.close(VarInt::from_u32(close::BIND_FAILED), b"");
            }
            Err(error)
        }
        Err(_) => {
            connection.close(VarInt::from_u32(close::BIND_FAILED), b"");
            Err(Error::BindFailed)
        }
    }
}

async fn read_control(
    mut recv: RecvStream,
    connection: Connection,
    inbound: mpsc::Sender<Inbound>,
) {
    loop {
        let item = match wire::read_payload(&mut recv, limits::MAX_CONTROL_FRAME).await {
            Ok(payload) => match ControlMessage::from_payload(&payload) {
                Ok(message) => Inbound::Message(Box::new(message)),
                Err(_) => Inbound::Malformed,
            },
            Err(wire::WireError::Frame(_)) => Inbound::Malformed,
            Err(_) => {
                // Give the close reason a moment to arrive, then classify it.
                let reason =
                    tokio::time::timeout(Duration::from_secs(2), connection.closed()).await;
                let closed = match reason {
                    Ok(ConnectionError::ApplicationClosed(close)) => {
                        match u32::try_from(close.error_code.into_inner()).unwrap_or(u32::MAX) {
                            close::BYE => Closed::Bye,
                            code @ (close::PROTOCOL
                            | close::BIND_FAILED
                            | close::SAFETY_MISMATCH
                            | close::UNEXPECTED_PEER) => Closed::PeerError(code),
                            _ => Closed::Lost,
                        }
                    }
                    Ok(_) => Closed::Lost,
                    // The stream ended but the connection is alive: a goodbye.
                    Err(_) => Closed::Bye,
                };
                let _ = inbound.send(Inbound::Closed(closed)).await;
                return;
            }
        };
        let stop = matches!(item, Inbound::Malformed);
        if inbound.send(item).await.is_err() || stop {
            return;
        }
    }
}

async fn write_control(
    mut send: SendStream,
    mut control: mpsc::UnboundedReceiver<ControlMessage>,
    mut bulk: mpsc::Receiver<ControlMessage>,
) {
    loop {
        let message = tokio::select! {
            biased;
            message = control.recv() => message,
            message = bulk.recv() => message,
        };
        let Some(message) = message else { break };
        let last = matches!(message, ControlMessage::Bye | ControlMessage::Error { .. });
        let Ok(frame) = message.to_frame() else {
            continue;
        };
        if send.write_all(&frame).await.is_err() {
            break;
        }
        if last {
            let _ = send.finish();
            let _ = tokio::time::timeout(Duration::from_secs(3), send.stopped()).await;
            break;
        }
    }
}

fn error_view(code: &'static str, message: &str) -> ErrorView {
    ErrorView {
        code,
        message: message.to_string(),
        shortfall: None,
    }
}

impl Session {
    /// Starts the per-connection tasks.
    fn attach(&mut self, connection: Connection, (send, recv): (SendStream, RecvStream)) {
        let (control, control_rx) = mpsc::unbounded_channel();
        let (bulk, bulk_rx) = mpsc::channel(2);
        let (inbound_tx, inbound) = mpsc::channel(16);
        let reader = tokio::spawn(read_control(recv, connection.clone(), inbound_tx));
        let writer = tokio::spawn(write_control(send, control_rx, bulk_rx));
        let acceptor = {
            let connection = connection.clone();
            let hub = self.hub.clone();
            tokio::spawn(async move {
                // Every further stream the peer opens is a request for a file.
                while let Ok((send, recv)) = connection.accept_bi().await {
                    tokio::spawn(hub.clone().serve(send, recv));
                }
            })
        };
        self.path = net::path_kind(&connection);
        self.connections.send_replace(Some(connection.clone()));
        self.link = Some(Link {
            connection,
            control,
            bulk,
            inbound,
            tasks: vec![reader, writer, acceptor],
        });
    }

    fn send(&self, message: ControlMessage) {
        if let Some(link) = &self.link {
            let _ = link.control.send(message);
        }
    }

    fn emit(&self, event: Event) {
        self.env.emitter.emit(event);
    }

    fn safety_state(&self, confirmed: bool, peer_confirmed: bool) -> SessionState {
        SessionState::VerifySafety {
            safety_code: self
                .secrets
                .safety_code(&self.endpoint_a, &self.endpoint_b)
                .to_string(),
            peer: self.peer.clone(),
            confirmed,
            peer_confirmed,
        }
    }

    fn active_state(&self) -> SessionState {
        SessionState::Active {
            peer: self.peer.clone(),
            path: self.path,
        }
    }

    async fn next_inbound(link: &mut Option<Link>) -> Inbound {
        match link {
            Some(link) => link
                .inbound
                .recv()
                .await
                .unwrap_or(Inbound::Closed(Closed::Lost)),
            None => std::future::pending().await,
        }
    }

    fn protocol_violation(&self, code: ErrorCode) -> Exit {
        self.send(ControlMessage::Error {
            code,
            message: String::new(),
        });
        Exit::Failed(Error::Protocol)
    }

    /// Step 10: both people must confirm the safety code before anything else.
    async fn verify_safety(
        &mut self,
        commands: &mut mpsc::Receiver<Command>,
    ) -> std::result::Result<(), Exit> {
        let (mut confirmed, mut peer_confirmed) = (false, false);
        self.env
            .emitter
            .session(self.safety_state(confirmed, peer_confirmed));
        while !(confirmed && peer_confirmed) {
            tokio::select! {
                command = commands.recv() => match command {
                    None => return Err(Exit::Local),
                    Some(Command::ConfirmSafety { matches: true, reply }) => {
                        if !confirmed {
                            confirmed = true;
                            self.send(ControlMessage::SafetyConfirmed);
                        }
                        let _ = reply.send(Ok(()));
                    }
                    Some(Command::ConfirmSafety { matches: false, reply }) => {
                        self.send(ControlMessage::Error {
                            code: ErrorCode::SafetyMismatch,
                            message: String::new(),
                        });
                        let _ = reply.send(Ok(()));
                        return Err(Exit::Failed(Error::SafetyMismatch));
                    }
                    Some(Command::End { reply }) => {
                        let _ = reply.send(Ok(()));
                        return Err(Exit::Local);
                    }
                    Some(other) => refuse(other, Error::NoSession),
                },
                inbound = Self::next_inbound(&mut self.link) => match inbound {
                    Inbound::Message(message) => match *message {
                        ControlMessage::SafetyConfirmed if !peer_confirmed => peer_confirmed = true,
                        ControlMessage::Bye => return Err(Exit::Peer),
                        ControlMessage::Error { code: ErrorCode::SafetyMismatch, .. } => {
                            return Err(Exit::Failed(Error::SafetyMismatch));
                        }
                        ControlMessage::Error { .. } => return Err(Exit::Failed(Error::Protocol)),
                        // Nothing else is allowed before both sides confirm.
                        _ => return Err(self.protocol_violation(ErrorCode::UnexpectedMessage)),
                    },
                    Inbound::Malformed => return Err(self.protocol_violation(ErrorCode::Malformed)),
                    Inbound::Closed(Closed::Bye) => return Err(Exit::Peer),
                    Inbound::Closed(Closed::PeerError(close::SAFETY_MISMATCH)) => {
                        return Err(Exit::Failed(Error::SafetyMismatch));
                    }
                    Inbound::Closed(_) => return Err(Exit::Failed(Error::ConnectionLost)),
                },
            }
            if !(confirmed && peer_confirmed) {
                self.env
                    .emitter
                    .session(self.safety_state(confirmed, peer_confirmed));
            }
        }
        self.env.emitter.session(self.active_state());
        Ok(())
    }

    /// The symmetric session: either side can send, pause, cancel or end.
    async fn run_active(&mut self, commands: &mut mpsc::Receiver<Command>) -> Exit {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    None => return Exit::Local,
                    Some(Command::End { reply }) => {
                        let _ = reply.send(Ok(()));
                        return Exit::Local;
                    }
                    Some(command) => self.handle_command(command).await,
                },
                inbound = Self::next_inbound(&mut self.link) => match inbound {
                    Inbound::Message(message) => {
                        if let Err(exit) = self.handle_message(*message).await {
                            return exit;
                        }
                    }
                    Inbound::Malformed => return self.protocol_violation(ErrorCode::Malformed),
                    Inbound::Closed(Closed::Bye) => return Exit::Peer,
                    Inbound::Closed(Closed::PeerError(close::SAFETY_MISMATCH)) => {
                        return Exit::Failed(Error::SafetyMismatch);
                    }
                    Inbound::Closed(Closed::PeerError(_)) => return Exit::Failed(Error::Protocol),
                    Inbound::Closed(Closed::Lost) => {
                        if let Err(exit) = self.reconnect(commands).await {
                            return exit;
                        }
                    }
                },
                Some(internal) = self.internal_rx.recv() => self.handle_internal(internal).await,
                Some((id, notice)) = self.notices_rx.recv() => self.handle_notice(id, notice),
                _ = tick.tick() => self.report_progress(),
            }
        }
    }

    // ----- commands -----

    async fn handle_command(&mut self, command: Command) {
        match command {
            Command::SendPaths { paths, reply } => {
                let _ = reply.send(self.send_paths(paths));
            }
            Command::SendText { text, reply } => {
                let _ = reply.send(self.send_text(text));
            }
            Command::RespondOffer {
                id,
                accept,
                folder,
                reply,
            } => {
                let result = if accept {
                    self.accept_offer(id, folder).await
                } else {
                    self.decline_offer(id)
                };
                let _ = reply.send(result);
            }
            Command::Pause { id, reply } => {
                let _ = reply.send(self.set_paused(id, true));
            }
            Command::Resume { id, reply } => {
                let _ = reply.send(self.set_paused(id, false));
            }
            Command::Cancel { id, reply } => {
                let _ = reply.send(self.cancel_transfer(id).await);
            }
            Command::ConfirmSafety { reply, .. } => {
                let _ = reply.send(Ok(()));
            }
            Command::DropConnection => {
                if let Some(link) = &self.link {
                    link.connection.close(VarInt::from_u32(99), b"test");
                }
            }
            Command::End { reply } => {
                let _ = reply.send(Ok(()));
            }
        }
    }

    fn running_transfers(&self) -> usize {
        self.outgoing
            .values()
            .filter(|t| t.phase != OutPhase::Final)
            .count()
            + self
                .incoming
                .values()
                .filter(|t| matches!(t.phase, InPhase::Active { .. }))
                .count()
    }

    fn send_text(&mut self, text: String) -> Result<()> {
        if text.len() > limits::MAX_TEXT_BYTES {
            return Err(Error::TextTooLong);
        }
        if text.trim().is_empty() {
            return Err(Error::NothingToSend);
        }
        if self.link.is_none() {
            return Err(Error::ConnectionLost);
        }
        self.send(ControlMessage::Text { text: text.clone() });
        self.push_text(false, text);
        Ok(())
    }

    fn push_text(&mut self, from_peer: bool, text: String) {
        self.next_text += 1;
        let message = TextView {
            id: self.next_text,
            from_peer,
            text,
            at: platform::now_ms(),
        };
        self.emit(Event::Text { message });
    }

    fn send_paths(&mut self, paths: Vec<PathBuf>) -> Result<String> {
        if paths.is_empty() {
            return Err(Error::NothingToSend);
        }
        if self.running_transfers() >= limits::MAX_ACTIVE_TRANSFERS {
            return Err(Error::TooManyTransfers);
        }
        let mut raw = [0u8; 16];
        tunnel_crypto::random_bytes(&mut raw).map_err(Error::internal)?;
        let id = TransferId(raw);
        let shared = self.hub.register(id);
        let cancel_walk = Arc::new(AtomicBool::new(false));

        let label = paths
            .first()
            .and_then(|p| p.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let view = new_view(
            id,
            Direction::Send,
            TransferState::Preparing,
            label,
            paths.len() as u64,
        );
        self.emit(Event::Transfer {
            transfer: view.clone(),
        });

        let task = {
            let store = self.env.store.clone();
            let cancel = cancel_walk.clone();
            let results = self.internal_tx.clone();
            tokio::spawn(async move {
                let walked = tokio::task::spawn_blocking(move || {
                    walk::walk(&store, id, &paths, limits::DEFAULT_MAX_ENTRIES, &cancel)
                })
                .await
                .unwrap_or_else(|err| Err(Error::internal(err)));
                let _ = results.send(Internal::Walked { id, result: walked }).await;
            })
        };
        self.outgoing.insert(
            id,
            Outgoing {
                view,
                shared,
                phase: OutPhase::Preparing,
                cancel_walk,
                task: Some(task),
                meter: Meter::new(),
                local_paused: false,
                peer_paused: false,
                last_word: None,
            },
        );
        Ok(id.to_hex())
    }

    async fn accept_offer(&mut self, id: TransferId, folder: Option<PathBuf>) -> Result<()> {
        if self.running_transfers() >= limits::MAX_ACTIVE_TRANSFERS {
            return Err(Error::TooManyTransfers);
        }
        let Some(incoming) = self.incoming.get(&id) else {
            return Err(Error::UnknownTransfer);
        };
        if !matches!(incoming.phase, InPhase::Pending) {
            return Err(Error::UnknownTransfer);
        }
        let settings = self.env.store.settings();
        let folder = folder.unwrap_or_else(|| settings.save_dir.clone());
        let total = incoming.offer.total_bytes;

        // Checked now, before any download: writable folder, enough space, and
        // a partial folder we can create.
        let partial_name = id.to_hex();
        let prepared = tokio::task::spawn_blocking(move || -> Result<(Destination, PathBuf)> {
            let dest = Destination::open(&folder).map_err(|_| Error::DestinationNotWritable)?;
            let shortfall = space_shortfall(dest.root(), total);
            if shortfall > 0 {
                return Err(Error::NoSpace { shortfall });
            }
            let partial = dest
                .ensure_folder(&[".tunnel-partial", &partial_name])
                .map_err(|_| Error::DestinationNotWritable)?;
            Ok((dest, partial))
        })
        .await
        .map_err(Error::internal)??;
        let (dest, partial) = prepared;

        let shared = Arc::new(IncomingShared {
            id,
            store: self.env.store.clone(),
            dest: dest.clone(),
            partial,
            rule: settings.conflict_rule,
            keep_modified: settings.keep_modified_times,
            keep_executable: settings.keep_executable,
            mark_downloaded: settings.mark_downloaded,
            counters: Counters::default(),
            issues: IssueLog::default(),
            disk: recv::DiskWork::default(),
        });
        // Names that had to change are listed up front in the details.
        if let Ok(renamed) = self.env.store.incoming_renamed(id, 200) {
            for (original, saved) in renamed {
                shared.issues.renamed(&saved, reason::UNSAFE_NAME, original);
            }
        }
        let (run, run_rx) = watch::channel(Run::Running);
        let task = {
            let shared = shared.clone();
            let connections = self.connections.subscribe();
            let notices = self.notices_tx.clone();
            let results = self.internal_tx.clone();
            tokio::spawn(async move {
                let end = recv::run_download(shared, connections, run_rx, notices).await;
                let _ = results.send(Internal::Downloaded { id, end }).await;
            })
        };

        let names = self.env.store.incoming_names(id, 1).unwrap_or_default();
        let incoming = self.incoming.get_mut(&id).expect("checked above");
        let mut view = new_view(
            id,
            Direction::Receive,
            TransferState::Active,
            names
                .into_iter()
                .next()
                .unwrap_or_default()
                .trim_end_matches('/')
                .to_string(),
            1,
        );
        view.files_total = incoming.offer.file_count;
        view.folders_total = incoming.offer.folder_count;
        view.bytes_total = incoming.offer.total_bytes;
        view.folder = Some(platform::display_path(dest.root()));
        incoming.view = Some(view.clone());
        incoming.phase = InPhase::Active { shared, run, task };
        incoming.meter = Meter::new();

        self.send(ControlMessage::Accept { transfer_id: id });
        self.emit(Event::OfferClosed { id: id.to_hex() });
        self.emit(Event::Transfer { transfer: view });
        Ok(())
    }

    fn decline_offer(&mut self, id: TransferId) -> Result<()> {
        let Some(incoming) = self.incoming.get_mut(&id) else {
            return Err(Error::UnknownTransfer);
        };
        if !matches!(incoming.phase, InPhase::Pending) {
            return Err(Error::UnknownTransfer);
        }
        let word = ControlMessage::Decline {
            transfer_id: id,
            reason: DeclineReason::Declined,
        };
        incoming.phase = InPhase::Final;
        incoming.last_word = Some(word.clone());
        self.env.store.forget_transfer(id);
        self.send(word);
        self.emit(Event::OfferClosed { id: id.to_hex() });
        Ok(())
    }

    fn set_paused(&mut self, id: TransferId, paused: bool) -> Result<()> {
        let message = if paused {
            ControlMessage::Pause {
                transfer_id: id,
                reason: CancelReason::User,
            }
        } else {
            ControlMessage::Unpause { transfer_id: id }
        };
        if let Some(outgoing) = self.outgoing.get_mut(&id) {
            if outgoing.phase != OutPhase::Active {
                return Err(Error::UnknownTransfer);
            }
            outgoing.local_paused = paused;
            outgoing.shared.paused.store(paused, Ordering::Relaxed);
            refresh_outgoing(outgoing);
            let view = outgoing.view.clone();
            self.send(message);
            self.emit(Event::Transfer { transfer: view });
            return Ok(());
        }
        if let Some(incoming) = self.incoming.get_mut(&id) {
            if !matches!(incoming.phase, InPhase::Active { .. }) {
                return Err(Error::UnknownTransfer);
            }
            incoming.local_paused = paused;
            if !paused {
                incoming.disk_full = false;
            }
            let view = refresh_incoming(incoming);
            self.send(message);
            if let Some(view) = view {
                self.emit(Event::Transfer { transfer: view });
            }
            return Ok(());
        }
        Err(Error::UnknownTransfer)
    }

    async fn cancel_transfer(&mut self, id: TransferId) -> Result<()> {
        let word = ControlMessage::Cancel {
            transfer_id: id,
            reason: CancelReason::User,
        };
        if self
            .outgoing
            .get(&id)
            .is_some_and(|t| t.phase != OutPhase::Final)
        {
            self.send(word.clone());
            self.finish_outgoing(id, TransferState::Cancelled, None, Some(word));
            return Ok(());
        }
        match self.incoming.get(&id).map(|t| &t.phase) {
            Some(InPhase::Pending | InPhase::Manifest { .. }) => {
                self.send(word.clone());
                self.drop_offer(id, Some(word));
                Ok(())
            }
            Some(InPhase::Active { .. }) => {
                self.send(word.clone());
                self.finish_incoming(id, TransferState::Cancelled, None, Some(word))
                    .await;
                Ok(())
            }
            _ => Err(Error::UnknownTransfer),
        }
    }

    // ----- messages from the other device -----

    async fn handle_message(&mut self, message: ControlMessage) -> std::result::Result<(), Exit> {
        tracing::debug!(message = message.name(), "control message received");
        match message {
            ControlMessage::Bind { .. } | ControlMessage::SafetyConfirmed => {
                return Err(self.protocol_violation(ErrorCode::UnexpectedMessage));
            }
            ControlMessage::Bye => return Err(Exit::Peer),
            ControlMessage::Error {
                code: ErrorCode::SafetyMismatch,
                ..
            } => {
                return Err(Exit::Failed(Error::SafetyMismatch));
            }
            ControlMessage::Error { .. } => return Err(Exit::Failed(Error::Protocol)),
            ControlMessage::Text { text } => self.push_text(true, text),
            ControlMessage::Offer(offer) => self.on_offer(offer),
            ControlMessage::ManifestPage(page) => self.on_page(page).await,
            ControlMessage::Accept { transfer_id } => self.on_accept(transfer_id),
            ControlMessage::Resume { transfer_id, .. } => self.on_resume(transfer_id),
            ControlMessage::Decline {
                transfer_id,
                reason,
            } => self.on_decline(transfer_id, reason),
            ControlMessage::Pause { transfer_id, .. } => self.on_peer_pause(transfer_id, true),
            ControlMessage::Unpause { transfer_id } => self.on_peer_pause(transfer_id, false),
            ControlMessage::Cancel { transfer_id, .. } => self.on_cancel(transfer_id).await,
            ControlMessage::Completed(completed) => self.on_completed(completed),
            // "Remember this device" arrives with trusted devices; until then
            // the offer is simply left unanswered.
            ControlMessage::TrustOffer { .. } | ControlMessage::TrustAccept { .. } => {}
        }
        Ok(())
    }

    fn on_offer(&mut self, offer: Offer) {
        let id = offer.transfer_id;
        let decline = |reason| ControlMessage::Decline {
            transfer_id: id,
            reason,
        };
        if self.incoming.contains_key(&id) || self.outgoing.contains_key(&id) {
            // A reused ID is never legitimate.
            self.send(decline(DeclineReason::Invalid));
            return;
        }
        let waiting = self
            .incoming
            .values()
            .filter(|t| matches!(t.phase, InPhase::Manifest { .. } | InPhase::Pending))
            .count();
        if waiting >= limits::MAX_PENDING_OFFERS {
            self.send(decline(DeclineReason::Busy));
            return;
        }
        let entries = offer.file_count.saturating_add(offer.folder_count);
        let max_entries = self.env.store.settings().max_entries;
        let pages_plausible =
            offer.manifest_pages >= 1 && u64::from(offer.manifest_pages) <= entries;
        if entries == 0 || entries > max_entries || !pages_plausible {
            self.send(decline(DeclineReason::Invalid));
            self.emit(Event::OfferBlocked {
                peer: self.peer.clone(),
            });
            return;
        }
        self.incoming.insert(
            id,
            Incoming {
                offer,
                stats: PageStats::default(),
                phase: InPhase::Manifest {
                    next_page: 0,
                    next_file_id: 0,
                    bytes_seen: 0,
                },
                view: None,
                meter: Meter::new(),
                local_paused: false,
                peer_paused: false,
                disk_full: false,
                cancelled: false,
                last_word: None,
            },
        );
    }

    /// Rejects an offer outright: the manifest broke a rule.
    fn block_offer(&mut self, id: TransferId) {
        let word = ControlMessage::Decline {
            transfer_id: id,
            reason: DeclineReason::Invalid,
        };
        self.send(word.clone());
        self.env.store.forget_transfer(id);
        if let Some(incoming) = self.incoming.get_mut(&id) {
            incoming.phase = InPhase::Final;
            incoming.last_word = Some(word);
        }
        self.emit(Event::OfferBlocked {
            peer: self.peer.clone(),
        });
    }

    async fn on_page(&mut self, page: ManifestPage) {
        let id = page.transfer_id;
        let Some(incoming) = self.incoming.get_mut(&id) else {
            return;
        };
        let InPhase::Manifest {
            next_page,
            next_file_id,
            bytes_seen,
        } = &mut incoming.phase
        else {
            // Pages for a transfer that is not expecting any are ignored.
            return;
        };
        let page_bytes: u64 = page
            .entries
            .iter()
            .map(|e| tunnel_proto::manifest::encoded_size(e) as u64)
            .sum();
        let count = page.entries.len() as u64;
        let total_entries = incoming.offer.file_count + incoming.offer.folder_count;
        let over = u64::from(*next_file_id) + count > total_entries
            || *bytes_seen + page_bytes > limits::MAX_MANIFEST_BYTES;
        if page.page != *next_page || over {
            self.block_offer(id);
            return;
        }
        let first = *next_file_id;
        *next_page += 1;
        *next_file_id += count as u32;
        *bytes_seen += page_bytes;
        let pages_done = *next_page;

        let store = self.env.store.clone();
        let stored = tokio::task::spawn_blocking(move || {
            store.insert_incoming_page(id, first, &page.entries)
        })
        .await;
        let stats = match stored {
            Ok(Ok(stats)) => stats,
            _ => {
                self.block_offer(id);
                return;
            }
        };
        let incoming = self.incoming.get_mut(&id).expect("present above");
        incoming.stats.add(&stats);
        if pages_done < incoming.offer.manifest_pages {
            return;
        }
        // The manifest is complete: our own totals must match the offer.
        let offer = &incoming.offer;
        let matches = incoming.stats.files == offer.file_count
            && incoming.stats.folders == offer.folder_count
            && incoming.stats.bytes == offer.total_bytes;
        if !matches {
            self.block_offer(id);
            return;
        }
        incoming.phase = InPhase::Pending;
        let view = self.offer_view(id).await;
        if let Some(offer) = view {
            self.emit(Event::Offer { offer });
        }
    }

    async fn offer_view(&self, id: TransferId) -> Option<OfferView> {
        let incoming = self.incoming.get(&id)?;
        let settings = self.env.store.settings();
        let stats = incoming.stats;
        let total = incoming.offer.total_bytes;
        let folder = settings.save_dir.clone();
        let store = self.env.store.clone();
        let (names, shortfall, low_space) = tokio::task::spawn_blocking(move || {
            let names = store.incoming_names(id, 20).unwrap_or_default();
            let probe = nearest_existing(&folder);
            let free = fs4::available_space(&probe).unwrap_or(u64::MAX);
            let needed = space_needed(total);
            (
                names,
                needed.saturating_sub(free),
                free != u64::MAX && total > free / 2,
            )
        })
        .await
        .ok()?;
        let mut warnings = Vec::new();
        if stats.double_extensions > 0 {
            warnings.push(OfferWarning::DoubleExtensions {
                count: stats.double_extensions,
            });
        }
        if stats.executables > 0 {
            warnings.push(OfferWarning::Executables {
                count: stats.executables,
            });
        }
        if stats.bidi > 0 {
            warnings.push(OfferWarning::BidiNames { count: stats.bidi });
        }
        if stats.renamed > 0 {
            warnings.push(OfferWarning::Renamed {
                count: stats.renamed,
            });
        }
        if low_space && shortfall == 0 {
            warnings.push(OfferWarning::LowSpace);
        }
        Some(OfferView {
            id: id.to_hex(),
            peer: self.peer.clone(),
            files: stats.files,
            folders: stats.folders,
            bytes: total,
            names,
            warnings,
            folder: platform::display_path(&settings.save_dir),
            shortfall,
            ask_where: settings.ask_where_to_save,
        })
    }

    fn on_accept(&mut self, id: TransferId) {
        let Some(outgoing) = self.outgoing.get_mut(&id) else {
            return;
        };
        // `Offering` counts too: the receiver can only accept once it has the
        // whole manifest, so the pages have arrived even if this side has not
        // yet noticed that the last one left.
        if !matches!(outgoing.phase, OutPhase::Offered | OutPhase::Offering) {
            return;
        }
        outgoing.phase = OutPhase::Active;
        outgoing.shared.accepted.store(true, Ordering::Relaxed);
        outgoing.meter = Meter::new();
        refresh_outgoing(outgoing);
        let view = outgoing.view.clone();
        self.emit(Event::Transfer { transfer: view });
    }

    fn on_resume(&mut self, id: TransferId) {
        match self.outgoing.get(&id).map(|t| t.phase) {
            // The Accept was lost with the old connection.
            Some(OutPhase::Offered | OutPhase::Offering) => self.on_accept(id),
            Some(OutPhase::Active) => {}
            _ => self.send(ControlMessage::Cancel {
                transfer_id: id,
                reason: CancelReason::Unknown,
            }),
        }
    }

    fn on_decline(&mut self, id: TransferId, reason: DeclineReason) {
        if !self
            .outgoing
            .get(&id)
            .is_some_and(|t| matches!(t.phase, OutPhase::Offered | OutPhase::Offering))
        {
            return;
        }
        let error = match reason {
            DeclineReason::Declined => None,
            DeclineReason::Invalid => Some(error_view(
                "declined_invalid",
                "The other device could not accept this selection.",
            )),
            DeclineReason::Busy => Some(error_view(
                "declined_busy",
                "The other device has too many transfers waiting. Try again in a moment.",
            )),
            DeclineReason::NoSpace => Some(error_view(
                "declined_no_space",
                "The other device does not have enough free space.",
            )),
        };
        self.finish_outgoing(id, TransferState::Declined, error, None);
    }

    fn on_peer_pause(&mut self, id: TransferId, paused: bool) {
        if let Some(outgoing) = self.outgoing.get_mut(&id) {
            if outgoing.phase == OutPhase::Active {
                outgoing.peer_paused = paused;
                refresh_outgoing(outgoing);
                let view = outgoing.view.clone();
                self.emit(Event::Transfer { transfer: view });
            }
        } else if let Some(incoming) = self.incoming.get_mut(&id) {
            incoming.peer_paused = paused;
            if let Some(view) = refresh_incoming(incoming) {
                self.emit(Event::Transfer { transfer: view });
            }
        }
    }

    async fn on_cancel(&mut self, id: TransferId) {
        if self
            .outgoing
            .get(&id)
            .is_some_and(|t| t.phase != OutPhase::Final)
        {
            self.finish_outgoing(id, TransferState::Cancelled, None, None);
            return;
        }
        match self.incoming.get(&id).map(|t| &t.phase) {
            Some(InPhase::Pending | InPhase::Manifest { .. }) => self.drop_offer(id, None),
            Some(InPhase::Active { .. }) => {
                self.finish_incoming(id, TransferState::Cancelled, None, None)
                    .await;
            }
            _ => {}
        }
    }

    fn on_completed(&mut self, completed: Completed) {
        let id = completed.transfer_id;
        let Some(outgoing) = self.outgoing.get_mut(&id) else {
            return;
        };
        if outgoing.phase != OutPhase::Active {
            return;
        }
        outgoing.view.files_done = completed.received;
        outgoing.view.files_skipped = completed.skipped;
        outgoing.view.files_failed = completed.failed;
        outgoing.view.bytes_done = outgoing.view.bytes_total;
        self.finish_outgoing(id, TransferState::Done, None, None);
    }

    // ----- background results -----

    async fn handle_internal(&mut self, internal: Internal) {
        match internal {
            Internal::Walked { id, result } => self.on_walked(id, result),
            Internal::PagesSent { id, sent } => {
                let Some(outgoing) = self.outgoing.get_mut(&id) else {
                    return;
                };
                if outgoing.phase != OutPhase::Offering {
                    return;
                }
                if sent {
                    outgoing.phase = OutPhase::Offered;
                } else {
                    let word = ControlMessage::Cancel {
                        transfer_id: id,
                        reason: CancelReason::SessionEnding,
                    };
                    self.send(word.clone());
                    self.finish_outgoing(
                        id,
                        TransferState::Failed,
                        Some(Error::ConnectionLost.view()),
                        Some(word),
                    );
                }
            }
            Internal::Downloaded { id, end } => {
                if end == DownloadEnd::Finished
                    && self.incoming.get(&id).is_some_and(|t| !t.cancelled)
                {
                    self.finish_incoming(id, TransferState::Done, None, None)
                        .await;
                }
            }
        }
    }

    fn on_walked(&mut self, id: TransferId, result: Result<WalkResult>) {
        let Some(outgoing) = self.outgoing.get_mut(&id) else {
            return;
        };
        if outgoing.phase != OutPhase::Preparing {
            return;
        }
        let walked = match result {
            Ok(walked) => walked,
            Err(error) => {
                let state = if error == Error::Cancelled {
                    TransferState::Cancelled
                } else {
                    TransferState::Failed
                };
                let error = (state == TransferState::Failed).then(|| error.view());
                self.finish_outgoing(id, state, error, None);
                return;
            }
        };
        let Some(link) = &self.link else {
            self.finish_outgoing(
                id,
                TransferState::Failed,
                Some(Error::ConnectionLost.view()),
                None,
            );
            return;
        };
        for issue in &walked.skipped {
            outgoing
                .shared
                .issues
                .push(&issue.path, issue.kind, issue.reason, None);
        }
        outgoing.view.label = walked.label.clone();
        outgoing.view.top_level = walked.top_level;
        outgoing.view.files_total = walked.files;
        outgoing.view.folders_total = walked.folders;
        outgoing.view.bytes_total = walked.bytes;
        outgoing.view.files_skipped = walked.skipped.len() as u64;
        outgoing.view.state = TransferState::Offered;
        outgoing.phase = OutPhase::Offering;
        refresh_outgoing(outgoing);

        let _ = link.control.send(ControlMessage::Offer(Offer {
            transfer_id: id,
            file_count: walked.files,
            folder_count: walked.folders,
            total_bytes: walked.bytes,
            manifest_pages: walked.pages,
        }));
        // Send the manifest in pages, with backpressure.
        let store = self.env.store.clone();
        let bulk = link.bulk.clone();
        let results = self.internal_tx.clone();
        outgoing.task = Some(tokio::spawn(async move {
            let sent = send_manifest(&store, id, &bulk).await;
            let _ = results.send(Internal::PagesSent { id, sent }).await;
        }));
        let view = outgoing.view.clone();
        self.emit(Event::Transfer { transfer: view });
    }

    fn handle_notice(&mut self, id: TransferId, notice: Notice) {
        let Notice::DiskFull = notice;
        let Some(incoming) = self.incoming.get_mut(&id) else {
            return;
        };
        if incoming.disk_full {
            return;
        }
        incoming.disk_full = true;
        let view = refresh_incoming(incoming);
        self.send(ControlMessage::Pause {
            transfer_id: id,
            reason: CancelReason::DiskFull,
        });
        if let Some(view) = view {
            self.emit(Event::Transfer { transfer: view });
        }
    }

    /// Publishes progress for running transfers and watches the network path.
    fn report_progress(&mut self) {
        let mut updates = Vec::new();
        for outgoing in self
            .outgoing
            .values_mut()
            .filter(|t| t.phase == OutPhase::Active)
        {
            let before = (outgoing.view.bytes_done, outgoing.view.speed);
            refresh_outgoing(outgoing);
            if before != (outgoing.view.bytes_done, outgoing.view.speed) {
                updates.push(outgoing.view.clone());
            }
        }
        for incoming in self
            .incoming
            .values_mut()
            .filter(|t| matches!(t.phase, InPhase::Active { .. }))
        {
            let before = incoming
                .view
                .as_ref()
                .map(|v| (v.bytes_done, v.files_done, v.speed));
            let view = refresh_incoming(incoming);
            if before != view.as_ref().map(|v| (v.bytes_done, v.files_done, v.speed)) {
                updates.extend(view);
            }
        }
        for transfer in updates {
            self.emit(Event::Transfer { transfer });
        }
        if let Some(link) = &self.link {
            let path = net::path_kind(&link.connection);
            if path != self.path {
                self.path = path;
                self.env.emitter.session(self.active_state());
            }
        }
    }

    // ----- endings -----

    fn finish_outgoing(
        &mut self,
        id: TransferId,
        state: TransferState,
        error: Option<ErrorView>,
        last_word: Option<ControlMessage>,
    ) {
        let Some(outgoing) = self.outgoing.get_mut(&id) else {
            return;
        };
        if outgoing.phase == OutPhase::Final {
            return;
        }
        outgoing.cancel_walk.store(true, Ordering::Relaxed);
        if let Some(task) = outgoing.task.take() {
            task.abort();
        }
        outgoing.phase = OutPhase::Final;
        outgoing.last_word = last_word;
        refresh_outgoing(outgoing);
        outgoing.view.state = state;
        outgoing.view.paused_by = PausedBy::None;
        outgoing.view.speed = 0;
        outgoing.view.eta_secs = None;
        outgoing.view.error = error;
        let view = outgoing.view.clone();
        self.hub.remove(id);
        self.env.store.forget_transfer(id);
        self.record_history(&view, vec![view.label.clone()]);
        self.emit(Event::Transfer { transfer: view });
    }

    /// Ends an accepted incoming transfer: stops the workers, removes the
    /// partial folder and reports. Files already moved into place stay.
    async fn finish_incoming(
        &mut self,
        id: TransferId,
        state: TransferState,
        error: Option<ErrorView>,
        last_word: Option<ControlMessage>,
    ) {
        let Some(incoming) = self.incoming.get_mut(&id) else {
            return;
        };
        let InPhase::Active { shared, run, task } =
            std::mem::replace(&mut incoming.phase, InPhase::Final)
        else {
            return;
        };
        incoming.cancelled = state != TransferState::Done;
        let _ = run.send(Run::Cancelled);
        if state != TransferState::Done {
            // Let workers stop cleanly so no partial file is being written.
            let mut task = task;
            if tokio::time::timeout(Duration::from_secs(10), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        // Writer threads outlive a dropped worker for a moment: wait until
        // every partial file is closed before removing the folder.
        shared.disk.idle(Duration::from_secs(10)).await;
        let partial = shared.partial.clone();
        let _ = tokio::task::spawn_blocking(move || recv::remove_partial(&partial)).await;

        let mut view = incoming
            .view
            .clone()
            .unwrap_or_else(|| new_view(id, Direction::Receive, state, String::new(), 1));
        fill_incoming(&mut view, &shared);
        view.state = state;
        view.paused_by = PausedBy::None;
        view.speed = 0;
        view.eta_secs = None;
        view.error = error;
        if state == TransferState::Done {
            view.bytes_done = view.bytes_total;
        }
        incoming.view = Some(view.clone());

        let word = match state {
            TransferState::Done => Some(ControlMessage::Completed(Completed {
                transfer_id: id,
                received: view.files_done,
                skipped: view.files_skipped,
                failed: view.files_failed,
            })),
            _ => last_word,
        };
        if state == TransferState::Done
            && let Some(word) = &word
        {
            self.send(word.clone());
        }
        if let Some(incoming) = self.incoming.get_mut(&id) {
            incoming.last_word = word;
        }
        let names = self.env.store.incoming_names(id, 5).unwrap_or_default();
        self.env.store.forget_transfer(id);
        self.record_history(&view, names);
        self.emit(Event::Transfer { transfer: view });
    }

    /// Forgets an offer that was never accepted.
    fn drop_offer(&mut self, id: TransferId, last_word: Option<ControlMessage>) {
        if let Some(incoming) = self.incoming.get_mut(&id) {
            incoming.phase = InPhase::Final;
            incoming.last_word = last_word;
        }
        self.env.store.forget_transfer(id);
        self.emit(Event::OfferClosed { id: id.to_hex() });
    }

    fn record_history(&self, view: &TransferView, names: Vec<String>) {
        let result = match view.state {
            TransferState::Done if view.files_failed > 0 || view.files_skipped > 0 => "partial",
            TransferState::Done => "done",
            TransferState::Declined => "declined",
            TransferState::Cancelled => "cancelled",
            _ => "failed",
        };
        let entry = HistoryEntry {
            id: 0,
            at: platform::now_ms(),
            peer: self.peer.name.clone(),
            direction: match view.direction {
                Direction::Send => "send".into(),
                Direction::Receive => "receive".into(),
            },
            files: if view.state == TransferState::Done {
                view.files_done
            } else {
                view.files_total
            },
            folders: view.folders_total,
            bytes: view.bytes_total,
            result: result.into(),
            names: names.into_iter().filter(|n| !n.is_empty()).collect(),
            folder: view.folder.clone(),
        };
        let _ = self
            .env
            .store
            .add_history(&self.env.store.settings(), entry);
    }

    // ----- reconnecting -----

    /// The connection dropped: for up to two minutes B redials A, both repeat
    /// `Bind` with the session's bind key, and running transfers continue from
    /// what is already on disk.
    async fn reconnect(
        &mut self,
        commands: &mut mpsc::Receiver<Command>,
    ) -> std::result::Result<(), Exit> {
        tracing::debug!(role = ?self.role, "connection lost; reconnecting");
        self.link = None;
        self.connections.send_replace(None);
        self.env.emitter.session(SessionState::Reconnecting {
            peer: self.peer.clone(),
        });
        let deadline = tokio::time::Instant::now() + limits::RECONNECT_WINDOW;

        let attempt = async {
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return Err(Error::ConnectionLost);
                }
                let connection = match self.role {
                    Role::A => net::accept_from(&self.endpoint, self.peer_id, remaining).await,
                    Role::B => {
                        let patience = remaining.min(Duration::from_secs(15));
                        net::dial(&self.endpoint, self.peer_addr.clone(), patience).await
                    }
                };
                let Ok(connection) = connection else {
                    tokio::time::sleep(Duration::from_secs(1).min(remaining)).await;
                    continue;
                };
                match bind(
                    &connection,
                    self.role,
                    &self.secrets,
                    &self.endpoint_a,
                    &self.endpoint_b,
                )
                .await
                {
                    Ok(streams) => return Ok((connection, streams)),
                    Err(Error::BindFailed) => return Err(Error::BindFailed),
                    Err(_) => continue,
                }
            }
        };
        let outcome = tokio::select! {
            outcome = attempt => outcome,
            () = wait_for_end(commands) => return Err(Exit::Local),
        };
        let (connection, streams) = outcome.map_err(Exit::Failed)?;
        tracing::debug!(role = ?self.role, "reconnected");
        self.attach(connection, streams);

        // Tell the other side where every transfer stands. Messages in flight
        // when the connection dropped may never have arrived.
        let mut words: Vec<ControlMessage> = Vec::new();
        let mut interrupted: Vec<TransferId> = Vec::new();
        for (id, outgoing) in &mut self.outgoing {
            outgoing.peer_paused = false;
            match outgoing.phase {
                OutPhase::Offering => interrupted.push(*id),
                OutPhase::Active if outgoing.local_paused => {
                    words.push(ControlMessage::Pause {
                        transfer_id: *id,
                        reason: CancelReason::User,
                    });
                }
                OutPhase::Final => words.extend(outgoing.last_word.clone()),
                _ => {}
            }
        }
        let mut broken_offers: Vec<TransferId> = Vec::new();
        for (id, incoming) in &mut self.incoming {
            incoming.peer_paused = false;
            match &incoming.phase {
                InPhase::Manifest { .. } => broken_offers.push(*id),
                InPhase::Active { shared, .. } => {
                    let files = recv::partial_offsets(&shared.partial, 1000)
                        .into_iter()
                        .map(|(file_id, offset)| ResumeEntry { file_id, offset })
                        .collect();
                    words.push(ControlMessage::Resume {
                        transfer_id: *id,
                        files,
                    });
                    if incoming.local_paused || incoming.disk_full {
                        words.push(ControlMessage::Pause {
                            transfer_id: *id,
                            reason: CancelReason::User,
                        });
                    }
                    let _ = refresh_incoming(incoming);
                }
                InPhase::Final => words.extend(incoming.last_word.clone()),
                InPhase::Pending => {}
            }
        }
        for id in interrupted {
            let word = ControlMessage::Cancel {
                transfer_id: id,
                reason: CancelReason::SessionEnding,
            };
            words.push(word.clone());
            self.finish_outgoing(
                id,
                TransferState::Failed,
                Some(Error::ConnectionLost.view()),
                Some(word),
            );
        }
        for id in broken_offers {
            let word = ControlMessage::Decline {
                transfer_id: id,
                reason: DeclineReason::Invalid,
            };
            words.push(word.clone());
            self.drop_offer(id, Some(word));
        }
        for word in words {
            self.send(word);
        }
        self.env.emitter.session(self.active_state());
        Ok(())
    }

    /// Ends the session: says goodbye, stops every task, leaves no partial
    /// data behind and closes the network endpoint.
    async fn teardown(mut self, exit: Exit) {
        let (reason, error) = match &exit {
            Exit::Local => (EndReason::Local, None),
            Exit::Peer => (EndReason::Peer, None),
            Exit::Failed(error) => (EndReason::Error, Some(error.view())),
        };
        let interrupted = match &exit {
            Exit::Failed(error) => error.view(),
            Exit::Local => Error::Cancelled.view(),
            Exit::Peer => Error::PeerEnded.view(),
        };

        let outgoing: Vec<TransferId> = self
            .outgoing
            .iter()
            .filter(|(_, t)| t.phase != OutPhase::Final)
            .map(|(id, _)| *id)
            .collect();
        for id in outgoing {
            self.finish_outgoing(id, TransferState::Failed, Some(interrupted.clone()), None);
        }
        let incoming: Vec<(TransferId, bool)> = self
            .incoming
            .iter()
            .filter(|(_, t)| !matches!(t.phase, InPhase::Final))
            .map(|(id, t)| (*id, matches!(t.phase, InPhase::Active { .. })))
            .collect();
        for (id, active) in incoming {
            if active {
                self.finish_incoming(id, TransferState::Failed, Some(interrupted.clone()), None)
                    .await;
            } else {
                self.drop_offer(id, None);
            }
        }

        if let Some(link) = self.link.take() {
            match &exit {
                Exit::Failed(Error::SafetyMismatch) | Exit::Failed(Error::Protocol) => {}
                _ => {
                    let _ = link.control.send(ControlMessage::Bye);
                }
            }
            // Give queued goodbyes and errors a moment to leave.
            tokio::time::sleep(Duration::from_millis(150)).await;
            let code = match &exit {
                Exit::Failed(Error::SafetyMismatch) => close::SAFETY_MISMATCH,
                Exit::Failed(Error::Protocol) => close::PROTOCOL,
                _ => close::BYE,
            };
            link.connection.close(VarInt::from_u32(code), b"");
        }
        self.connections.send_replace(None);
        self.secrets.wipe();
        self.env.store.forget_all_transfers();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.endpoint.close()).await;
        self.env.emitter.session(SessionState::Ended {
            reason,
            error,
            peer: Some(self.peer.clone()),
        });
    }
}

/// Reads the stored entries back and sends them as manifest pages, cut exactly
/// as they were counted for the offer.
async fn send_manifest(store: &Store, id: TransferId, bulk: &mpsc::Sender<ControlMessage>) -> bool {
    let mut paginator = Paginator::new(id);
    let mut after: i64 = -1;
    loop {
        let batch = {
            let store = store.clone();
            tokio::task::spawn_blocking(move || store.outgoing_batch(id, after, 2000)).await
        };
        let Ok(Ok(batch)) = batch else { return false };
        let Some(last) = batch.last() else { break };
        after = i64::from(last.file_id);
        for entry in &batch {
            if let Some(page) = paginator.push(entry.to_manifest())
                && bulk.send(ControlMessage::ManifestPage(page)).await.is_err()
            {
                return false;
            }
        }
    }
    match paginator.finish() {
        Some(page) => bulk.send(ControlMessage::ManifestPage(page)).await.is_ok(),
        None => true,
    }
}

fn new_view(
    id: TransferId,
    direction: Direction,
    state: TransferState,
    label: String,
    top_level: u64,
) -> TransferView {
    TransferView {
        id: id.to_hex(),
        direction,
        state,
        paused_by: PausedBy::None,
        label,
        top_level,
        files_total: 0,
        folders_total: 0,
        bytes_total: 0,
        bytes_done: 0,
        files_done: 0,
        files_skipped: 0,
        files_failed: 0,
        speed: 0,
        eta_secs: None,
        folder: None,
        issues: Vec::new(),
        issues_truncated: false,
        error: None,
        started_at: platform::now_ms(),
    }
}

fn eta(bytes_total: u64, bytes_done: u64, speed: u64) -> Option<u64> {
    (speed > 0 && bytes_total > bytes_done).then(|| (bytes_total - bytes_done).div_ceil(speed))
}

/// Brings an outgoing transfer's view up to date with its counters and flags.
fn refresh_outgoing(outgoing: &mut Outgoing) {
    let view = &mut outgoing.view;
    let (issues, truncated): (Vec<Issue>, bool) = outgoing.shared.issues.snapshot();
    view.issues = issues;
    view.issues_truncated = truncated;
    if outgoing.phase != OutPhase::Active {
        return;
    }
    view.bytes_done = outgoing
        .shared
        .bytes_sent
        .load(Ordering::Relaxed)
        .min(view.bytes_total);
    view.files_done = outgoing
        .shared
        .files_sent
        .load(Ordering::Relaxed)
        .min(view.files_total);
    view.paused_by = if outgoing.local_paused {
        PausedBy::Local
    } else if outgoing.peer_paused {
        PausedBy::Peer
    } else {
        PausedBy::None
    };
    if view.paused_by == PausedBy::None {
        view.state = TransferState::Active;
        view.speed = outgoing.meter.update(view.bytes_done);
        view.eta_secs = eta(view.bytes_total, view.bytes_done, view.speed);
    } else {
        view.state = TransferState::Paused;
        view.speed = 0;
        view.eta_secs = None;
    }
}

fn fill_incoming(view: &mut TransferView, shared: &IncomingShared) {
    let counters = &shared.counters;
    view.bytes_done = (counters.bytes() + counters.dropped()).min(view.bytes_total);
    view.files_done = counters.files();
    view.files_skipped = counters.skipped();
    view.files_failed = counters.failed();
    let (issues, truncated) = shared.issues.snapshot();
    view.issues = issues;
    view.issues_truncated = truncated;
}

/// Brings an incoming transfer's view and its workers' run state up to date.
fn refresh_incoming(incoming: &mut Incoming) -> Option<TransferView> {
    let InPhase::Active { shared, run, .. } = &incoming.phase else {
        return incoming.view.clone();
    };
    let view = incoming.view.as_mut()?;
    fill_incoming(view, shared);
    view.paused_by = if incoming.disk_full {
        PausedBy::DiskFull
    } else if incoming.local_paused {
        PausedBy::Local
    } else if incoming.peer_paused {
        PausedBy::Peer
    } else {
        PausedBy::None
    };
    let wanted = if view.paused_by == PausedBy::None {
        Run::Running
    } else {
        Run::Paused
    };
    run.send_if_modified(|current| {
        let changed = *current != wanted && *current != Run::Cancelled;
        if changed {
            *current = wanted;
        }
        changed
    });
    if wanted == Run::Running {
        view.state = TransferState::Active;
        view.speed = incoming.meter.update(shared.counters.bytes());
        view.eta_secs = eta(view.bytes_total, view.bytes_done, view.speed);
    } else {
        view.state = TransferState::Paused;
        view.speed = 0;
        view.eta_secs = None;
    }
    Some(view.clone())
}

/// Free space required for a transfer: its size plus 1% plus 100 MB.
pub(crate) fn space_needed(total: u64) -> u64 {
    total
        .saturating_add(total / 100)
        .saturating_add(100 * 1024 * 1024)
}

fn space_shortfall(folder: &std::path::Path, total: u64) -> u64 {
    match fs4::available_space(folder) {
        Ok(free) => space_needed(total).saturating_sub(free),
        // If the platform cannot say, do not block the transfer on a guess.
        Err(_) => 0,
    }
}

/// The closest ancestor of `path` that exists, for free-space queries on a
/// folder that has not been created yet.
fn nearest_existing(path: &std::path::Path) -> PathBuf {
    path.ancestors()
        .find(|p| p.exists())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_rule_is_size_plus_one_percent_plus_100_mb() {
        assert_eq!(space_needed(0), 100 * 1024 * 1024);
        assert_eq!(
            space_needed(10_000_000_000),
            10_000_000_000 + 100_000_000 + 104_857_600
        );
        assert_eq!(space_needed(u64::MAX), u64::MAX);
    }

    #[test]
    fn eta_needs_speed() {
        assert_eq!(eta(1000, 0, 0), None);
        assert_eq!(eta(1000, 0, 100), Some(10));
        assert_eq!(eta(1000, 950, 100), Some(1));
        assert_eq!(eta(1000, 1000, 100), None);
    }
}
