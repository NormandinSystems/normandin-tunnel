//! Shared helpers for the integration tests and the hostile-peer harness:
//! a local rendezvous server and real engine nodes talking over loopback.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tunnel_core::{
    Config, Event, OfferView, RelayChoice, SessionState, Settings, TransferState, TransferView,
    Tunnel,
};
use tunnel_rendezvous::Rendezvous;

pub mod evil;

/// How long any single step may take before a test fails.
pub const STEP: Duration = Duration::from_secs(60);

/// A rendezvous server on loopback.
pub struct LocalRendezvous {
    pub url: String,
    pub server: Arc<Rendezvous>,
    _task: tokio::task::JoinHandle<()>,
}

pub async fn rendezvous() -> LocalRendezvous {
    rendezvous_with(tunnel_rendezvous::Config::default()).await
}

pub async fn rendezvous_with(config: tunnel_rendezvous::Config) -> LocalRendezvous {
    let (addr, server, task) = tunnel_rendezvous::spawn_local(config)
        .await
        .expect("start rendezvous");
    LocalRendezvous {
        url: format!("ws://{addr}/v1"),
        server,
        _task: task,
    }
}

/// A relay server on loopback with a self-signed certificate, for pairing
/// without a rendezvous server.
pub struct LocalRelay {
    pub url: String,
    _server: Box<dyn std::any::Any + Send + Sync>,
}

pub async fn relay() -> LocalRelay {
    let (_map, url, server) = iroh::test_utils::run_relay_server_with(false)
        .await
        .expect("start relay");
    LocalRelay {
        url: url.to_string(),
        _server: Box::new(server),
    }
}

/// A device with no rendezvous server configured: it pairs through the relay.
pub fn serverless_node(relay: &LocalRelay, name: &str) -> Node {
    serverless_node_on(&[relay], name)
}

/// Like [`serverless_node`], with a choice of relays.
pub fn serverless_node_on(relays: &[&LocalRelay], name: &str) -> Node {
    open_node(
        Config {
            relay: Some(RelayChoice::Insecure(
                relays.iter().map(|relay| relay.url.clone()).collect(),
            )),
            rendezvous_url: None,
            include_loopback: true,
            ..Config::default()
        },
        name,
        |_| {},
    )
}

/// A device exactly as installed: no pairing server, the public relays.
/// Needs the internet.
pub fn public_node(name: &str) -> Node {
    open_node(
        Config {
            relay: None,
            rendezvous_url: None,
            include_loopback: true,
            ..Config::default()
        },
        name,
        |_| {},
    )
}

/// A device with neither a pairing server nor any relay.
pub fn isolated_node(name: &str) -> Node {
    open_node(
        Config {
            relay: Some(RelayChoice::Disabled),
            rendezvous_url: None,
            include_loopback: true,
            ..Config::default()
        },
        name,
        |_| {},
    )
}

/// One device: an engine with its own data folder and save folder.
pub struct Node {
    pub tunnel: Tunnel,
    pub events: broadcast::Receiver<Event>,
    pub data: tempfile::TempDir,
    pub save: tempfile::TempDir,
}

pub fn node(rendezvous_url: &str, name: &str) -> Node {
    node_with(rendezvous_url, name, |_| {})
}

/// Turns on engine logging for a test run when `TUNNEL_TEST_LOG` is set.
fn init_logging() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if let Ok(filter) = std::env::var("TUNNEL_TEST_LOG") {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
                .with_writer(std::io::stderr)
                .try_init();
        }
    });
}

pub fn node_with(rendezvous_url: &str, name: &str, adjust: impl FnOnce(&mut Settings)) -> Node {
    open_node(
        Config {
            relay: Some(RelayChoice::Disabled),
            rendezvous_url: Some(rendezvous_url.to_string()),
            include_loopback: true,
            ..Config::default()
        },
        name,
        adjust,
    )
}

fn open_node(config: Config, name: &str, adjust: impl FnOnce(&mut Settings)) -> Node {
    init_logging();
    let data = tempfile::tempdir().expect("data dir");
    let save = tempfile::tempdir().expect("save dir");
    let tunnel = Tunnel::open(Config {
        data_dir: data.path().to_path_buf(),
        ..config
    })
    .expect("open engine");
    let mut settings = tunnel.settings();
    settings.device_name = name.to_string();
    settings.save_dir = save.path().to_path_buf();
    adjust(&mut settings);
    tunnel.save_settings(settings).expect("save settings");
    let events = tunnel.subscribe();
    Node {
        tunnel,
        events,
        data,
        save,
    }
}

impl Node {
    /// Waits for an event the closure recognises.
    pub async fn wait<T>(&mut self, what: &str, mut pick: impl FnMut(&Event) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + STEP;
        loop {
            let event = match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Ok(Ok(event)) => event,
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    panic!("events closed waiting for {what}")
                }
                Err(_) => panic!(
                    "timed out waiting for {what}; state: {:?}",
                    self.tunnel.snapshot().session
                ),
            };
            if let Some(found) = pick(&event) {
                return found;
            }
        }
    }

    pub async fn wait_session<T>(
        &mut self,
        what: &str,
        mut pick: impl FnMut(&SessionState) -> Option<T>,
    ) -> T {
        self.wait(what, |event| match event {
            Event::Session { session } => pick(session),
            _ => None,
        })
        .await
    }

    pub async fn wait_code(&mut self) -> String {
        self.wait_session("a code", |s| match s {
            SessionState::ShowingCode { code, .. } => Some(code.clone()),
            SessionState::Ended { error, .. } => {
                panic!("session ended instead of showing a code: {error:?}")
            }
            _ => None,
        })
        .await
    }

    pub async fn wait_safety(&mut self) -> String {
        self.wait_session("the safety code", |s| match s {
            SessionState::VerifySafety { safety_code, .. } => Some(safety_code.clone()),
            SessionState::Ended { error, .. } => {
                panic!("session ended before the safety code: {error:?}")
            }
            _ => None,
        })
        .await
    }

    pub async fn wait_active(&mut self) {
        self.wait_session("an open session", |s| match s {
            SessionState::Active { .. } => Some(()),
            SessionState::Ended { error, .. } => panic!("session ended before opening: {error:?}"),
            _ => None,
        })
        .await
    }

    /// Waits for the session to end and returns the error code, if any.
    pub async fn wait_ended(&mut self) -> Option<String> {
        self.wait_session("the session to end", |s| match s {
            SessionState::Ended { error, .. } => Some(error.as_ref().map(|e| e.code.to_string())),
            _ => None,
        })
        .await
    }

    pub async fn wait_offer(&mut self) -> OfferView {
        self.wait("an offer", |event| match event {
            Event::Offer { offer } => Some(offer.clone()),
            _ => None,
        })
        .await
    }

    /// Waits until the transfer reaches a final state.
    ///
    /// Looks at the current picture first: an earlier wait for something else
    /// may already have read past the transfer's last event.
    pub async fn wait_transfer_end(&mut self, id: &str) -> TransferView {
        let finished = |tunnel: &Tunnel| {
            tunnel
                .snapshot()
                .transfers
                .into_iter()
                .find(|t| t.id == id && t.state.is_final())
        };
        if let Some(done) = finished(&self.tunnel) {
            return done;
        }
        self.wait("the transfer to finish", |event| match event {
            Event::Transfer { transfer } if transfer.id == id && transfer.state.is_final() => {
                Some(transfer.clone())
            }
            _ => None,
        })
        .await
    }

    pub async fn wait_transfer_state(&mut self, id: &str, state: TransferState) -> TransferView {
        self.wait("a transfer state", |event| match event {
            Event::Transfer { transfer } if transfer.id == id && transfer.state == state => {
                Some(transfer.clone())
            }
            _ => None,
        })
        .await
    }

    pub async fn wait_text(&mut self) -> String {
        self.wait("a text", |event| match event {
            Event::Text { message } if message.from_peer => Some(message.text.clone()),
            _ => None,
        })
        .await
    }

    pub fn saved(&self, rel: &str) -> PathBuf {
        self.save.path().join(rel)
    }
}

/// Pairs two nodes all the way to an open session.
pub async fn pair(a: &mut Node, b: &mut Node) {
    a.tunnel.create_code().await.expect("create code");
    let code = a.wait_code().await;
    b.tunnel.join_code(&code, false).await.expect("join code");
    let (safety_a, safety_b) = tokio::join!(a.wait_safety(), b.wait_safety());
    assert_eq!(safety_a, safety_b, "both screens show the same safety code");
    a.tunnel.confirm_safety(true).await.expect("confirm a");
    b.tunnel.confirm_safety(true).await.expect("confirm b");
    tokio::join!(a.wait_active(), b.wait_active());
}

/// Sends `paths` from `sender`, accepts on `receiver`, and waits for both ends.
pub async fn transfer(
    sender: &mut Node,
    receiver: &mut Node,
    paths: Vec<PathBuf>,
) -> (TransferView, TransferView) {
    let id = sender.tunnel.send_paths(paths).await.expect("send paths");
    let offer = receiver.wait_offer().await;
    assert_eq!(offer.id, id);
    receiver
        .tunnel
        .respond_offer(&id, true, None)
        .await
        .expect("accept");
    let received = receiver.wait_transfer_end(&id).await;
    let sent = sender.wait_transfer_end(&id).await;
    (sent, received)
}

/// Deterministic pseudo-random bytes (xorshift), so large files need no storage.
pub fn pattern(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.to_le_bytes());
    }
    out.truncate(len);
    out
}

pub fn write_file(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(path, bytes).expect("write file");
}

pub fn hash_file(path: &Path) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    let mut file =
        std::fs::File::open(path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    std::io::copy(&mut file, &mut hasher).expect("hash file");
    hasher.finalize()
}

/// Every file under `root` as (relative path with `/`, size), sorted.
pub fn tree(root: &Path) -> Vec<(String, u64)> {
    fn visit(root: &Path, dir: &Path, out: &mut Vec<(String, u64)>) {
        for entry in std::fs::read_dir(dir).expect("read dir").flatten() {
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .expect("inside root")
                .to_string_lossy()
                .replace('\\', "/");
            let metadata = std::fs::symlink_metadata(&path).expect("metadata");
            if metadata.is_dir() {
                out.push((format!("{rel}/"), 0));
                visit(root, &path, out);
            } else {
                out.push((rel, metadata.len()));
            }
        }
    }
    let mut out = Vec::new();
    visit(root, root, &mut out);
    out.sort();
    out
}
