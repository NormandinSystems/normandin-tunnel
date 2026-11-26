//! The Normandin Tunnel engine.
//!
//! One [`Tunnel`] per app. It owns the local store and at most one session at a
//! time; the desktop app and the command-line app are thin shells around it.
//!
//! ```no_run
//! # async fn demo() -> tunnel_core::Result<()> {
//! use tunnel_core::{Config, Event, SessionState, Tunnel};
//!
//! let tunnel = Tunnel::open(Config::default())?;
//! let mut events = tunnel.subscribe();
//! tunnel.create_code().await?;
//! while let Ok(event) = events.recv().await {
//!     if let Event::Session { session: SessionState::ShowingCode { code, .. } } = event {
//!         println!("Code: {code}");
//!     }
//! }
//! # Ok(()) }
//! ```
//!
//! The UI never touches the network or chooses raw paths: it calls the short
//! list of methods on [`Tunnel`] and listens to [`Event`]s.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, mpsc, oneshot};
use tunnel_crypto::PairUri;
use tunnel_proto::control::TransferId;
pub use tunnel_proto::hello::DeviceType;

pub mod error;
pub mod events;
mod meeting;
mod net;
mod pairing;
pub mod pathsafe;
pub mod platform;
mod rendezvous;
mod session;
pub mod settings;
pub mod store;
pub mod transfer;
mod wire;

pub use error::{Error, ErrorView, Result};
pub use events::{
    Direction, EndReason, Event, Issue, OfferView, OfferWarning, PathKind, PausedBy, PeerInfo,
    SessionState, Snapshot, TextView, TransferState, TransferView,
};
pub use net::RelayChoice;
pub use pathsafe::ConflictRule;
pub use rendezvous::check_url as check_rendezvous_url;
pub use settings::{HistoryRetention, Settings};
pub use store::HistoryEntry;

use session::{Command, Emitter, Env, Start};
use store::Store;

/// How an app sets the engine up.
#[derive(Debug, Clone)]
pub struct Config {
    /// Folder for the database. Defaults to the platform's app-data folder.
    pub data_dir: PathBuf,
    /// What kind of device this is.
    pub device_type: DeviceType,
    /// The app's version, shown to the other device.
    pub app_version: String,
    /// Overrides the relay setting (tests turn relays off).
    pub relay: Option<RelayChoice>,
    /// Overrides the rendezvous address setting.
    pub rendezvous_url: Option<String>,
    /// Offer loopback addresses to the other device. For tests and for two
    /// copies of the app on one machine.
    pub include_loopback: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: platform::default_data_dir(),
            device_type: DeviceType::Desktop,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            relay: None,
            rendezvous_url: None,
            include_loopback: false,
        }
    }
}

struct ActiveSession {
    commands: mpsc::Sender<Command>,
    task: tokio::task::JoinHandle<()>,
}

struct Inner {
    env: Env,
    session: Mutex<Option<ActiveSession>>,
}

/// The engine. Cheap to clone; all clones share one state.
#[derive(Clone)]
pub struct Tunnel {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnel").finish_non_exhaustive()
    }
}

/// What a typed or pasted code refers to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CodeCheck {
    /// The code in canonical form.
    pub code: String,
    /// A rendezvous host named by a pairing link that differs from the one in
    /// settings. The app must ask before using it.
    pub other_host: Option<String>,
}

impl Tunnel {
    /// Opens the store and gets ready. No network activity happens here.
    pub fn open(config: Config) -> Result<Self> {
        let store = Store::open(&config.data_dir)?;
        let settings = store.settings();
        let _ = store.prune_history(settings.history);
        let (events, _) = broadcast::channel(1024);
        let env = Env {
            store,
            emitter: Emitter {
                events,
                snapshot: Arc::new(Mutex::new(Snapshot::default())),
            },
            device_type: config.device_type,
            app_version: config.app_version,
            relay: config.relay,
            rendezvous_url: config.rendezvous_url,
            include_loopback: config.include_loopback,
        };
        Ok(Self {
            inner: Arc::new(Inner {
                env,
                session: Mutex::new(None),
            }),
        })
    }

    /// Subscribes to everything the engine reports from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.env.emitter.events.subscribe()
    }

    /// The whole current picture, for drawing from scratch.
    pub fn snapshot(&self) -> Snapshot {
        self.inner
            .env
            .emitter
            .snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    // ----- settings and history -----

    /// The current settings.
    pub fn settings(&self) -> Settings {
        self.inner.env.store.settings()
    }

    /// Saves settings; returns them as stored.
    pub fn save_settings(&self, settings: Settings) -> Result<Settings> {
        // Empty means "no pairing server", which is the default.
        let address = settings.rendezvous_url.trim();
        if !address.is_empty() && rendezvous::check_url(address).is_err() {
            return Err(Error::RendezvousAddressInvalid);
        }
        let saved = self.inner.env.store.save_settings(settings)?;
        let _ = self.inner.env.store.prune_history(saved.history);
        Ok(saved)
    }

    /// Recent transfers, newest first.
    pub fn history(&self, limit: u32) -> Result<Vec<HistoryEntry>> {
        self.inner.env.store.history(limit)
    }

    /// Deletes all history.
    pub fn clear_history(&self) -> Result<()> {
        self.inner.env.store.clear_history()
    }

    // ----- sessions -----

    fn start(&self, start: Start) -> Result<()> {
        let mut slot = self.inner.session.lock().unwrap_or_else(|p| p.into_inner());
        if slot
            .as_ref()
            .is_some_and(|session| !session.task.is_finished())
        {
            return Err(Error::SessionBusy);
        }
        // A new session starts with a clean picture.
        *self
            .inner
            .env
            .emitter
            .snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Snapshot::default();
        let (commands, receiver) = mpsc::channel(32);
        let task = tokio::spawn(session::run(self.inner.env.clone(), start, receiver));
        *slot = Some(ActiveSession { commands, task });
        Ok(())
    }

    /// "Show a code": makes a one-time code and waits for the other device.
    /// The code arrives as a [`SessionState::ShowingCode`] event.
    pub async fn create_code(&self) -> Result<()> {
        self.start(Start::Host)
    }

    /// Checks typed or pasted text without starting anything.
    pub fn check_code(&self, input: &str) -> Result<CodeCheck> {
        let parsed = PairUri::parse(input).map_err(|_| Error::CodeInvalid)?;
        let own_host = self.rendezvous_host();
        let other_host = parsed
            .rendezvous_host
            .filter(|host| Some(host) != own_host.as_ref());
        Ok(CodeCheck {
            code: parsed.code.to_string(),
            other_host,
        })
    }

    /// "Enter a code": joins the session behind a code.
    ///
    /// With no pairing server set (the default) the two devices meet on the
    /// relay network. If the text is a pairing link naming a rendezvous host
    /// other than the one in settings, that host is used only when
    /// `allow_other_host` is true (the app must have asked the person first).
    pub async fn join_code(&self, input: &str, allow_other_host: bool) -> Result<()> {
        let parsed = PairUri::parse(input).map_err(|_| Error::CodeInvalid)?;
        let own_host = self.rendezvous_host();
        let rendezvous_url = match parsed.rendezvous_host {
            Some(host) if Some(&host) != own_host.as_ref() => {
                if !allow_other_host {
                    return Err(Error::RendezvousAddressInvalid);
                }
                Some(rendezvous::url_for_host(&host)?)
            }
            _ => match self.rendezvous_url() {
                Some(configured) => Some(rendezvous::check_url(&configured)?.0),
                None => None,
            },
        };
        self.start(Start::Guest {
            code: parsed.code,
            rendezvous_url,
        })
    }

    /// The pairing server in use, if any: the one this engine was started
    /// with, else the one in settings. `None` means pairing without a server.
    fn rendezvous_url(&self) -> Option<String> {
        session::configured_rendezvous(&self.inner.env)
    }

    fn rendezvous_host(&self) -> Option<String> {
        self.rendezvous_url()
            .and_then(|url| rendezvous::check_url(&url).ok())
            .map(|(_, host)| host)
    }

    fn commands(&self) -> Result<mpsc::Sender<Command>> {
        let slot = self.inner.session.lock().unwrap_or_else(|p| p.into_inner());
        match slot.as_ref() {
            Some(session) if !session.task.is_finished() => Ok(session.commands.clone()),
            _ => Err(Error::NoSession),
        }
    }

    async fn ask<T>(&self, build: impl FnOnce(oneshot::Sender<Result<T>>) -> Command) -> Result<T> {
        let commands = self.commands()?;
        let (reply, answer) = oneshot::channel();
        commands
            .send(build(reply))
            .await
            .map_err(|_| Error::NoSession)?;
        answer.await.map_err(|_| Error::NoSession)?
    }

    /// The person compared the safety codes. `false` ends the session.
    pub async fn confirm_safety(&self, matches: bool) -> Result<()> {
        self.ask(|reply| Command::ConfirmSafety { matches, reply })
            .await
    }

    /// Offers files and folders to the other device. Returns the transfer ID.
    pub async fn send_paths(&self, paths: Vec<PathBuf>) -> Result<String> {
        self.ask(|reply| Command::SendPaths { paths, reply }).await
    }

    /// Sends a text message or link.
    pub async fn send_text(&self, text: String) -> Result<()> {
        self.ask(|reply| Command::SendText { text, reply }).await
    }

    /// Accepts or declines an offer. `folder` overrides the save folder.
    pub async fn respond_offer(
        &self,
        id: &str,
        accept: bool,
        folder: Option<PathBuf>,
    ) -> Result<()> {
        let id = TransferId::from_hex(id).ok_or(Error::UnknownTransfer)?;
        if folder.as_ref().is_some_and(|folder| !folder.is_absolute()) {
            return Err(Error::DestinationNotWritable);
        }
        self.ask(|reply| Command::RespondOffer {
            id,
            accept,
            folder,
            reply,
        })
        .await
    }

    /// Pauses a transfer.
    pub async fn pause(&self, id: &str) -> Result<()> {
        let id = TransferId::from_hex(id).ok_or(Error::UnknownTransfer)?;
        self.ask(|reply| Command::Pause { id, reply }).await
    }

    /// Continues a paused transfer.
    pub async fn resume(&self, id: &str) -> Result<()> {
        let id = TransferId::from_hex(id).ok_or(Error::UnknownTransfer)?;
        self.ask(|reply| Command::Resume { id, reply }).await
    }

    /// Cancels a transfer or withdraws an offer.
    pub async fn cancel(&self, id: &str) -> Result<()> {
        let id = TransferId::from_hex(id).ok_or(Error::UnknownTransfer)?;
        self.ask(|reply| Command::Cancel { id, reply }).await
    }

    /// Ends the session (or cancels pairing) and waits until it is fully closed.
    pub async fn end_session(&self) {
        let session = self
            .inner
            .session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        let Some(session) = session else { return };
        let (reply, answer) = oneshot::channel();
        if session.commands.send(Command::End { reply }).await.is_ok() {
            let _ = answer.await;
        }
        let _ = tokio::time::timeout(std::time::Duration::from_secs(15), session.task).await;
    }

    /// Whether a session (including pairing) is running.
    pub fn has_session(&self) -> bool {
        let slot = self.inner.session.lock().unwrap_or_else(|p| p.into_inner());
        slot.as_ref()
            .is_some_and(|session| !session.task.is_finished())
    }

    /// Clears the picture of a finished session, back to "no session".
    pub fn dismiss(&self) {
        if !self.has_session() {
            let emitter = &self.inner.env.emitter;
            *emitter.snapshot.lock().unwrap_or_else(|p| p.into_inner()) = Snapshot::default();
            let _ = emitter.events.send(Event::Session {
                session: SessionState::Idle,
            });
        }
    }

    /// Words starting with `prefix`, for code autocomplete.
    pub fn complete_word(prefix: &str, limit: usize) -> Vec<&'static str> {
        tunnel_crypto::wordlist::complete(prefix, limit.min(20))
    }

    /// Test hook: drops the connection the way a network failure would.
    #[doc(hidden)]
    pub async fn debug_drop_connection(&self) {
        if let Ok(commands) = self.commands() {
            let _ = commands.send(Command::DropConnection).await;
        }
    }
}
