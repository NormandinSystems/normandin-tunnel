//! The Normandin Tunnel rendezvous server.
//!
//! It introduces two devices and nothing more: it hands out a nameplate, lets
//! one other device join the room behind it, and passes a handful of opaque
//! pairing messages between the two. It never sees code words, keys or files.
//!
//! Everything lives in memory. Nothing is written to disk, logs hold counts and
//! error codes only, and IP addresses exist only in the rate-limit table, where
//! they expire after ten minutes.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tunnel_proto::rendezvous::{self as proto, ClientMessage, ErrorCode, ServerMessage};

/// Server settings. The defaults are the protocol's published limits.
#[derive(Debug, Clone)]
pub struct Config {
    /// How long a room (and its code) lives.
    pub room_lifetime: Duration,
    /// How long a freed nameplate stays unused.
    pub nameplate_cooldown: Duration,
    /// Most rooms at once.
    pub max_rooms: usize,
    /// `create` requests per IP address per minute.
    pub creates_per_minute: u32,
    /// `join` requests per IP address per minute.
    pub joins_per_minute: u32,
    /// Open connections per IP address.
    pub connections_per_ip: usize,
    /// How long a connection may stay open without creating or joining a room.
    pub idle_timeout: Duration,
    /// Take the client address from the last `X-Forwarded-For` entry. Turn on
    /// only behind a reverse proxy you control (the bundled Caddy setup).
    pub trust_forwarded_for: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            room_lifetime: Duration::from_secs(proto::ROOM_LIFETIME_SECS),
            nameplate_cooldown: Duration::from_secs(proto::NAMEPLATE_COOLDOWN_SECS),
            max_rooms: proto::MAX_ROOMS,
            creates_per_minute: proto::CREATES_PER_MINUTE,
            joins_per_minute: proto::JOINS_PER_MINUTE,
            connections_per_ip: proto::CONNECTIONS_PER_IP,
            idle_timeout: Duration::from_secs(30),
            trust_forwarded_for: false,
        }
    }
}

/// Counters for the log line and `/healthz`. No message contents, no addresses.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Rooms open right now.
    pub rooms: usize,
    /// Connections open right now.
    pub connections: usize,
    /// Rooms created since start.
    pub created: u64,
    /// Rooms a second device joined since start.
    pub joined: u64,
    /// Errors sent since start.
    pub errors: u64,
}

type ConnId = u64;
type Outbox = mpsc::Sender<ServerMessage>;

struct Member {
    conn: ConnId,
    outbox: Outbox,
}

struct Room {
    created: Instant,
    members: Vec<Member>,
    /// Messages posted before the second device arrived.
    queued: Vec<String>,
    messages: usize,
    /// A second device has joined at some point: the code is used up.
    used: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Freed {
    Closed,
    Expired,
}

#[derive(Default)]
struct IpState {
    connections: usize,
    window_start: Option<Instant>,
    creates: u32,
    joins: u32,
    last_seen: Option<Instant>,
}

#[derive(Default)]
struct Inner {
    rooms: HashMap<u16, Room>,
    cooldown: HashMap<u16, (Instant, Freed)>,
    ips: HashMap<IpAddr, IpState>,
    next_conn: ConnId,
    stats: Stats,
}

/// Shared server state.
pub struct Rendezvous {
    config: Config,
    inner: Mutex<Inner>,
}

type Nameplates = std::ops::RangeInclusive<u16>;

fn low_range() -> Nameplates {
    1..=999
}

fn high_range() -> Nameplates {
    1000..=9999
}

const RATE_WINDOW: Duration = Duration::from_secs(60);
const IP_MEMORY: Duration = Duration::from_secs(600);
const OUTBOX_DEPTH: usize = 32;

impl Rendezvous {
    /// Creates the server state.
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new(Self {
            config,
            inner: Mutex::new(Inner::default()),
        })
    }

    /// Current counters.
    pub fn stats(&self) -> Stats {
        let inner = self.lock();
        Stats {
            rooms: inner.rooms.len(),
            ..inner.stats
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves plain data behind; keep serving.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The HTTP router: the WebSocket endpoint and the health check.
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route(proto::PATH, get(ws_handler))
            .route(proto::HEALTH_PATH, get(|| async { "ok" }))
            .with_state(self.clone())
    }

    /// Serves until the listener fails. Also runs the expiry sweeper.
    pub async fn serve(self: Arc<Self>, listener: tokio::net::TcpListener) -> std::io::Result<()> {
        let sweeper = tokio::spawn(self.clone().sweep_forever());
        let app = self
            .router()
            .into_make_service_with_connect_info::<SocketAddr>();
        let result = axum::serve(listener, app).await;
        sweeper.abort();
        result
    }

    async fn sweep_forever(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        loop {
            tick.tick().await;
            self.sweep(Instant::now());
        }
    }

    /// Expires rooms, cooldowns and remembered addresses.
    fn sweep(&self, now: Instant) {
        let mut expired = Vec::new();
        {
            let mut guard = self.lock();
            let inner = &mut *guard;
            let lifetime = self.config.room_lifetime;
            let dead: Vec<u16> = inner
                .rooms
                .iter()
                .filter(|(_, room)| now.duration_since(room.created) >= lifetime)
                .map(|(nameplate, _)| *nameplate)
                .collect();
            for nameplate in dead {
                if let Some(room) = inner.rooms.remove(&nameplate) {
                    inner.cooldown.insert(nameplate, (now, Freed::Expired));
                    expired.extend(room.members.into_iter().map(|m| m.outbox));
                }
            }
            let cooldown = self.config.nameplate_cooldown;
            inner
                .cooldown
                .retain(|_, (freed, _)| now.duration_since(*freed) < cooldown);
            inner.ips.retain(|_, ip| {
                ip.connections > 0
                    || ip
                        .last_seen
                        .is_some_and(|seen| now.duration_since(seen) < IP_MEMORY)
            });
            inner.stats.errors += expired.len() as u64;
        }
        for outbox in expired {
            let _ = outbox.try_send(ServerMessage::Error {
                code: ErrorCode::Expired,
            });
        }
    }

    fn connect(&self, ip: IpAddr) -> Result<ConnId, ErrorCode> {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let limit = self.config.connections_per_ip;
        let state = inner.ips.entry(ip).or_default();
        if state.connections >= limit {
            inner.stats.errors += 1;
            return Err(ErrorCode::RateLimited);
        }
        state.connections += 1;
        state.last_seen = Some(Instant::now());
        inner.next_conn += 1;
        inner.stats.connections += 1;
        Ok(inner.next_conn)
    }

    fn disconnect(&self, ip: IpAddr, conn: ConnId, nameplate: Option<u16>) {
        if let Some(nameplate) = nameplate {
            self.leave(nameplate, conn);
        }
        let mut inner = self.lock();
        if let Some(state) = inner.ips.get_mut(&ip) {
            state.connections = state.connections.saturating_sub(1);
            state.last_seen = Some(Instant::now());
        }
        inner.stats.connections = inner.stats.connections.saturating_sub(1);
    }

    /// Counts one `create` or `join` against the address's per-minute budget.
    fn rate_limit(&self, inner: &mut Inner, ip: IpAddr, join: bool) -> Result<(), ErrorCode> {
        let now = Instant::now();
        let state = inner.ips.entry(ip).or_default();
        if state
            .window_start
            .is_none_or(|start| now.duration_since(start) >= RATE_WINDOW)
        {
            state.window_start = Some(now);
            state.creates = 0;
            state.joins = 0;
        }
        let (count, limit) = if join {
            (&mut state.joins, self.config.joins_per_minute)
        } else {
            (&mut state.creates, self.config.creates_per_minute)
        };
        if *count >= limit {
            return Err(ErrorCode::RateLimited);
        }
        *count += 1;
        Ok(())
    }

    fn create(&self, ip: IpAddr, conn: ConnId, outbox: &Outbox) -> Result<u16, ErrorCode> {
        let mut guard = self.lock();
        let inner = &mut *guard;
        self.rate_limit(inner, ip, false)?;
        if inner.rooms.len() >= self.config.max_rooms {
            return Err(ErrorCode::ServerBusy);
        }
        let nameplate = pick_nameplate(inner).ok_or(ErrorCode::ServerBusy)?;
        inner.rooms.insert(
            nameplate,
            Room {
                created: Instant::now(),
                members: vec![Member {
                    conn,
                    outbox: outbox.clone(),
                }],
                queued: Vec::new(),
                messages: 0,
                used: false,
            },
        );
        inner.stats.created += 1;
        Ok(nameplate)
    }

    fn join(
        &self,
        ip: IpAddr,
        conn: ConnId,
        nameplate: u16,
        outbox: &Outbox,
    ) -> Result<(), ErrorCode> {
        let mut guard = self.lock();
        let inner = &mut *guard;
        self.rate_limit(inner, ip, true)?;
        let Some(room) = inner.rooms.get_mut(&nameplate) else {
            return Err(match inner.cooldown.get(&nameplate) {
                Some((_, Freed::Expired)) => ErrorCode::Expired,
                _ => ErrorCode::UnknownCode,
            });
        };
        if room.used || room.members.len() >= proto::ROOM_MEMBERS {
            return Err(ErrorCode::CodeInUse);
        }
        room.used = true;
        // Order matters: `joined` first, then everything the first device queued.
        let _ = outbox.try_send(ServerMessage::Joined);
        for body in room.queued.drain(..) {
            let _ = outbox.try_send(ServerMessage::Message { body });
        }
        for member in &room.members {
            let _ = member.outbox.try_send(ServerMessage::PeerJoined);
        }
        room.members.push(Member {
            conn,
            outbox: outbox.clone(),
        });
        inner.stats.joined += 1;
        Ok(())
    }

    fn post(&self, nameplate: u16, conn: ConnId, body: String) -> Result<(), ErrorCode> {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let room = inner.rooms.get_mut(&nameplate).ok_or(ErrorCode::Expired)?;
        if room.messages >= proto::ROOM_MAX_MESSAGES {
            return Err(ErrorCode::TooLarge);
        }
        room.messages += 1;
        match room.members.iter().find(|m| m.conn != conn) {
            Some(peer) => {
                // A full outbox means the peer is not reading: drop the room.
                peer.outbox
                    .try_send(ServerMessage::Message { body })
                    .map_err(|_| ErrorCode::ServerBusy)
            }
            None if !room.used => {
                room.queued.push(body);
                Ok(())
            }
            None => Err(ErrorCode::BadRequest),
        }
    }

    /// Removes a device from its room. The room closes and the code is dead.
    fn leave(&self, nameplate: u16, conn: ConnId) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let Some(room) = inner.rooms.get(&nameplate) else {
            return;
        };
        if !room.members.iter().any(|m| m.conn == conn) {
            return;
        }
        if let Some(room) = inner.rooms.remove(&nameplate) {
            inner
                .cooldown
                .insert(nameplate, (Instant::now(), Freed::Closed));
            for member in room.members.iter().filter(|m| m.conn != conn) {
                let _ = member.outbox.try_send(ServerMessage::PeerLeft);
            }
        }
    }

    fn count_error(&self) {
        self.lock().stats.errors += 1;
    }
}

/// "Nameplates are picked at random from 1 to 999 while fewer than half are in
/// use, otherwise from 1000 to 9999." Nameplates in cooldown count as in use.
fn pick_nameplate(inner: &Inner) -> Option<u16> {
    let taken = |n: &u16| inner.rooms.contains_key(n) || inner.cooldown.contains_key(n);
    let low_in_use = low_range().filter(|n| taken(n)).count();
    let range = if low_in_use < low_range().count() / 2 {
        low_range()
    } else {
        high_range()
    };
    let span = u32::from(*range.end() - *range.start()) + 1;
    for _ in 0..64 {
        let candidate = *range.start() + (random_u32() % span) as u16;
        if !taken(&candidate) {
            return Some(candidate);
        }
    }
    range
        .clone()
        .find(|n| !taken(n))
        .or_else(|| high_range().find(|n| !taken(n)))
}

fn random_u32() -> u32 {
    let mut buf = [0u8; 4];
    // Nameplates are public; if the OS generator fails, fall back to the clock
    // rather than refuse service.
    if getrandom::fill(&mut buf).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        return nanos;
    }
    u32::from_be_bytes(buf)
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(server): State<Arc<Rendezvous>>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let ip = client_ip(&server.config, remote, &headers);
    ws.max_message_size(proto::MAX_CLIENT_MESSAGE_BYTES)
        .max_frame_size(proto::MAX_CLIENT_MESSAGE_BYTES)
        .on_upgrade(move |socket| connection(server, socket, ip))
}

fn client_ip(config: &Config, remote: SocketAddr, headers: &HeaderMap) -> IpAddr {
    if config.trust_forwarded_for
        && let Some(forwarded) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
        && let Some(ip) = forwarded
            .rsplit(',')
            .next()
            .and_then(|last| last.trim().parse().ok())
    {
        return ip;
    }
    remote.ip()
}

async fn connection(server: Arc<Rendezvous>, socket: WebSocket, ip: IpAddr) {
    let (mut sink, mut stream) = socket.split();
    let conn = match server.connect(ip) {
        Ok(conn) => conn,
        Err(code) => {
            let _ = sink.send(to_frame(&ServerMessage::Error { code })).await;
            let _ = sink.close().await;
            return;
        }
    };

    let (outbox, mut outgoing) = mpsc::channel::<ServerMessage>(OUTBOX_DEPTH);
    let writer = tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            let last = matches!(
                message,
                ServerMessage::Error { .. } | ServerMessage::PeerLeft
            );
            if sink.send(to_frame(&message)).await.is_err() || last {
                break;
            }
        }
        let _ = sink.close().await;
    });

    let mut nameplate: Option<u16> = None;
    let deadline =
        tokio::time::Instant::now() + server.config.room_lifetime + Duration::from_secs(5);
    loop {
        let wait = if nameplate.is_some() {
            deadline
        } else {
            tokio::time::Instant::now() + server.config.idle_timeout
        };
        let frame = tokio::select! {
            frame = stream.next() => frame,
            _ = tokio::time::sleep_until(wait.min(deadline)) => None,
            _ = outbox.closed() => None,
        };
        let text = match frame {
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            Some(Ok(Message::Binary(_))) => {
                send_error(&server, &outbox, ErrorCode::BadRequest);
                break;
            }
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
        };
        let result = match proto::parse_client(text.as_str()) {
            Err(code) => Err(code),
            Ok(ClientMessage::Create) if nameplate.is_none() => {
                server.create(ip, conn, &outbox).map(|assigned| {
                    nameplate = Some(assigned);
                    let _ = outbox.try_send(ServerMessage::Created {
                        nameplate: assigned,
                        expires_in: server.config.room_lifetime.as_secs(),
                    });
                })
            }
            Ok(ClientMessage::Join { nameplate: wanted }) if nameplate.is_none() => server
                .join(ip, conn, wanted, &outbox)
                .map(|()| nameplate = Some(wanted)),
            Ok(ClientMessage::Post { body }) => match nameplate {
                Some(room) => server.post(room, conn, body),
                None => Err(ErrorCode::BadRequest),
            },
            Ok(ClientMessage::Close) => break,
            Ok(ClientMessage::Create | ClientMessage::Join { .. }) => Err(ErrorCode::BadRequest),
        };
        if let Err(code) = result {
            send_error(&server, &outbox, code);
            break;
        }
    }

    server.disconnect(ip, conn, nameplate);
    drop(outbox);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
}

fn send_error(server: &Rendezvous, outbox: &Outbox, code: ErrorCode) {
    server.count_error();
    let _ = outbox.try_send(ServerMessage::Error { code });
}

fn to_frame(message: &ServerMessage) -> Message {
    let json = serde_json::to_string(message).expect("server messages always serialize");
    Message::Text(json.into())
}

/// Binds to `127.0.0.1` on a free port and serves in the background. For tests
/// and local development.
pub async fn spawn_local(
    config: Config,
) -> std::io::Result<(SocketAddr, Arc<Rendezvous>, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let server = Rendezvous::new(config);
    let serving = server.clone();
    let handle = tokio::spawn(async move {
        let _ = serving.serve(listener).await;
    });
    Ok((addr, server, handle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Arc<Rendezvous> {
        Rendezvous::new(Config::default())
    }

    fn outbox() -> (Outbox, mpsc::Receiver<ServerMessage>) {
        mpsc::channel(OUTBOX_DEPTH)
    }

    const IP: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 1));

    #[test]
    fn nameplates_stay_low_until_half_are_used() {
        let s = server();
        let (tx, _rx) = outbox();
        let mut inner = s.lock();
        for n in 1..=498u16 {
            inner.rooms.insert(
                n,
                Room {
                    created: Instant::now(),
                    members: vec![Member {
                        conn: 0,
                        outbox: tx.clone(),
                    }],
                    queued: vec![],
                    messages: 0,
                    used: false,
                },
            );
        }
        for _ in 0..50 {
            let n = pick_nameplate(&inner).unwrap();
            assert!(low_range().contains(&n) && n > 498);
        }
        inner.cooldown.insert(499, (Instant::now(), Freed::Closed));
        inner.cooldown.insert(500, (Instant::now(), Freed::Closed));
        for _ in 0..50 {
            assert!(high_range().contains(&pick_nameplate(&inner).unwrap()));
        }
    }

    #[test]
    fn create_and_join_are_rate_limited_per_address() {
        let s = server();
        let (tx, _rx) = outbox();
        for i in 0..proto::CREATES_PER_MINUTE {
            s.create(IP, u64::from(i), &tx).unwrap();
        }
        assert_eq!(s.create(IP, 99, &tx), Err(ErrorCode::RateLimited));
        let other = IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, 2));
        assert!(s.create(other, 100, &tx).is_ok());
        for _ in 0..proto::JOINS_PER_MINUTE {
            assert_eq!(s.join(IP, 200, 9999, &tx), Err(ErrorCode::UnknownCode));
        }
        assert_eq!(s.join(IP, 200, 9999, &tx), Err(ErrorCode::RateLimited));
    }

    #[test]
    fn connections_are_limited_per_address() {
        let s = server();
        let conns: Vec<_> = (0..proto::CONNECTIONS_PER_IP)
            .map(|_| s.connect(IP).unwrap())
            .collect();
        assert_eq!(s.connect(IP), Err(ErrorCode::RateLimited));
        s.disconnect(IP, conns[0], None);
        assert!(s.connect(IP).is_ok());
    }

    #[test]
    fn rooms_are_capped() {
        let s = Rendezvous::new(Config {
            max_rooms: 2,
            creates_per_minute: 100,
            ..Config::default()
        });
        let (tx, _rx) = outbox();
        s.create(IP, 1, &tx).unwrap();
        s.create(IP, 2, &tx).unwrap();
        assert_eq!(s.create(IP, 3, &tx), Err(ErrorCode::ServerBusy));
    }

    #[test]
    fn queued_messages_reach_the_second_device_in_order() {
        let s = server();
        let (tx_a, mut rx_a) = outbox();
        let (tx_b, mut rx_b) = outbox();
        let nameplate = s.create(IP, 1, &tx_a).unwrap();
        s.post(nameplate, 1, "AAAA".into()).unwrap();
        s.post(nameplate, 1, "BBBB".into()).unwrap();
        s.join(IP, 2, nameplate, &tx_b).unwrap();
        assert_eq!(rx_b.try_recv().unwrap(), ServerMessage::Joined);
        assert_eq!(
            rx_b.try_recv().unwrap(),
            ServerMessage::Message {
                body: "AAAA".into()
            }
        );
        assert_eq!(
            rx_b.try_recv().unwrap(),
            ServerMessage::Message {
                body: "BBBB".into()
            }
        );
        assert_eq!(rx_a.try_recv().unwrap(), ServerMessage::PeerJoined);
        s.post(nameplate, 2, "CCCC".into()).unwrap();
        assert_eq!(
            rx_a.try_recv().unwrap(),
            ServerMessage::Message {
                body: "CCCC".into()
            }
        );
    }

    #[test]
    fn a_third_device_is_refused_and_the_pair_is_unaffected() {
        let s = server();
        let (tx_a, mut rx_a) = outbox();
        let (tx_b, _rx_b) = outbox();
        let (tx_c, _rx_c) = outbox();
        let nameplate = s.create(IP, 1, &tx_a).unwrap();
        s.join(IP, 2, nameplate, &tx_b).unwrap();
        assert_eq!(s.join(IP, 3, nameplate, &tx_c), Err(ErrorCode::CodeInUse));
        assert_eq!(rx_a.try_recv().unwrap(), ServerMessage::PeerJoined);
        s.post(nameplate, 2, "AAAA".into()).unwrap();
        assert_eq!(
            rx_a.try_recv().unwrap(),
            ServerMessage::Message {
                body: "AAAA".into()
            }
        );
    }

    #[test]
    fn a_room_carries_at_most_sixteen_messages() {
        let s = server();
        let (tx_a, _rx_a) = outbox();
        let (tx_b, _rx_b) = outbox();
        let nameplate = s.create(IP, 1, &tx_a).unwrap();
        s.join(IP, 2, nameplate, &tx_b).unwrap();
        for i in 0..proto::ROOM_MAX_MESSAGES {
            s.post(nameplate, 1 + (i as u64 % 2), "AAAA".into())
                .unwrap();
        }
        assert_eq!(
            s.post(nameplate, 1, "AAAA".into()),
            Err(ErrorCode::TooLarge)
        );
    }

    #[test]
    fn leaving_closes_the_room_and_rests_the_nameplate() {
        let s = server();
        let (tx_a, _rx_a) = outbox();
        let (tx_b, mut rx_b) = outbox();
        let nameplate = s.create(IP, 1, &tx_a).unwrap();
        s.join(IP, 2, nameplate, &tx_b).unwrap();
        s.leave(nameplate, 1);
        assert_eq!(rx_b.try_recv().unwrap(), ServerMessage::Joined);
        assert_eq!(rx_b.try_recv().unwrap(), ServerMessage::PeerLeft);
        assert_eq!(s.stats().rooms, 0);
        assert_eq!(s.join(IP, 3, nameplate, &tx_b), Err(ErrorCode::UnknownCode));
        // The freed nameplate is not handed out again during the cooldown.
        let inner = s.lock();
        assert!(inner.cooldown.contains_key(&nameplate));
    }

    #[test]
    fn rooms_expire_and_say_so() {
        let s = Rendezvous::new(Config {
            room_lifetime: Duration::from_secs(1),
            ..Config::default()
        });
        let (tx_a, mut rx_a) = outbox();
        let nameplate = s.create(IP, 1, &tx_a).unwrap();
        s.sweep(Instant::now());
        assert_eq!(s.stats().rooms, 1);
        s.sweep(Instant::now() + Duration::from_secs(2));
        assert_eq!(s.stats().rooms, 0);
        assert_eq!(
            rx_a.try_recv().unwrap(),
            ServerMessage::Error {
                code: ErrorCode::Expired
            }
        );
        assert_eq!(s.join(IP, 2, nameplate, &tx_a), Err(ErrorCode::Expired));
    }

    #[test]
    fn addresses_are_forgotten_after_ten_minutes() {
        let s = server();
        let conn = s.connect(IP).unwrap();
        s.disconnect(IP, conn, None);
        s.sweep(Instant::now() + Duration::from_secs(60));
        assert!(s.lock().ips.contains_key(&IP));
        s.sweep(Instant::now() + IP_MEMORY + Duration::from_secs(1));
        assert!(s.lock().ips.is_empty());
    }

    #[test]
    fn forwarded_for_is_used_only_when_trusted() {
        let remote: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "6.6.6.6, 203.0.113.9".parse().unwrap());
        let trusting = Config {
            trust_forwarded_for: true,
            ..Config::default()
        };
        assert_eq!(
            client_ip(&trusting, remote, &headers),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
        assert_eq!(client_ip(&Config::default(), remote, &headers), remote.ip());
        headers.insert("x-forwarded-for", "garbage".parse().unwrap());
        assert_eq!(client_ip(&trusting, remote, &headers), remote.ip());
    }
}
