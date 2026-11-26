//! What the engine tells the apps. Everything here serializes to JSON for the UI.

use serde::Serialize;
use tunnel_proto::hello::DeviceType;

use crate::error::ErrorView;

/// The other device, as shown on screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PeerInfo {
    /// Its name (already made safe to display).
    pub name: String,
    /// `desktop`, `phone`, `tablet` or `server`.
    pub device_type: &'static str,
    /// Its app version.
    pub app_version: String,
}

pub(crate) fn device_type_name(device_type: DeviceType) -> &'static str {
    match device_type {
        DeviceType::Desktop => "desktop",
        DeviceType::Phone => "phone",
        DeviceType::Tablet => "tablet",
        DeviceType::Server => "server",
    }
}

/// How data is travelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathKind {
    /// Not known yet.
    Unknown,
    /// Straight between the two devices.
    Direct,
    /// Through a relay server (sealed packets; may be slower).
    Relay,
}

/// Why a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// The person on this device ended it.
    Local,
    /// The other device ended it.
    Peer,
    /// Something went wrong.
    Error,
}

/// Where the session is in its life.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionState {
    /// No session.
    Idle,
    /// Asking the pairing service for a code.
    CreatingCode,
    /// Showing a code and waiting for the other device.
    ShowingCode {
        /// The code, e.g. `42-orbit-velvet-candle`.
        code: String,
        /// The QR-code link, `tunnel://pair?c=…&s=…`.
        uri: String,
        /// The link as an SVG QR code.
        qr_svg: String,
        /// When the code expires (ms since the Unix epoch).
        expires_at: i64,
    },
    /// Looking up the code that was entered.
    Joining,
    /// Both devices are in the room; exchanging keys.
    Securing,
    /// Keys agreed; connecting the devices to each other.
    Connecting,
    /// Connected; both people must compare the safety code.
    VerifySafety {
        /// The 6-digit code, formatted `418 293`.
        safety_code: String,
        /// The other device.
        peer: PeerInfo,
        /// This person tapped "Codes match".
        confirmed: bool,
        /// The other person tapped "Codes match".
        peer_confirmed: bool,
    },
    /// The session is open.
    Active {
        /// The other device.
        peer: PeerInfo,
        /// Direct or through a relay.
        path: PathKind,
    },
    /// The connection dropped; trying to restore it.
    Reconnecting {
        /// The other device.
        peer: PeerInfo,
    },
    /// The session is over.
    Ended {
        /// Why.
        reason: EndReason,
        /// The error, when `reason` is `error`.
        error: Option<ErrorView>,
        /// The other device, if the session got that far.
        peer: Option<PeerInfo>,
    },
}

/// Which way a transfer goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// This device sends.
    Send,
    /// This device receives.
    Receive,
}

/// Where a transfer is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    /// Listing the files to send.
    Preparing,
    /// Offered; waiting for the receiver to decide.
    Offered,
    /// Moving data.
    Active,
    /// Paused.
    Paused,
    /// Finished (see the counts for skipped or failed files).
    Done,
    /// The receiver said no.
    Declined,
    /// Cancelled by either side.
    Cancelled,
    /// Could not be completed.
    Failed,
}

impl TransferState {
    /// Whether the transfer is over.
    pub fn is_final(self) -> bool {
        matches!(
            self,
            TransferState::Done
                | TransferState::Declined
                | TransferState::Cancelled
                | TransferState::Failed
        )
    }
}

/// Who paused a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PausedBy {
    /// Nobody: it is running.
    None,
    /// This device.
    Local,
    /// The other device.
    Peer,
    /// This device ran out of disk space.
    DiskFull,
}

/// One file or folder that needs mentioning in the transfer details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    /// Path inside the transfer.
    pub path: String,
    /// `skipped`, `failed` or `renamed`.
    pub kind: &'static str,
    /// Stable reason code (translation key).
    pub reason: &'static str,
    /// Extra detail: the original name for renames, the OS error for failures.
    pub detail: Option<String>,
}

/// A transfer as the apps see it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransferView {
    /// Transfer ID (hex).
    pub id: String,
    /// Send or receive.
    pub direction: Direction,
    /// Current state.
    pub state: TransferState,
    /// Who paused it, if anyone.
    pub paused_by: PausedBy,
    /// A short label: the single top-level name, or the first of several.
    pub label: String,
    /// Number of top-level items selected.
    pub top_level: u64,
    /// Files in the transfer.
    pub files_total: u64,
    /// Folders in the transfer.
    pub folders_total: u64,
    /// Bytes in the transfer.
    pub bytes_total: u64,
    /// Bytes moved so far.
    pub bytes_done: u64,
    /// Files finished so far.
    pub files_done: u64,
    /// Files skipped.
    pub files_skipped: u64,
    /// Files failed.
    pub files_failed: u64,
    /// Current speed in bytes per second.
    pub speed: u64,
    /// Estimated seconds left, when known.
    pub eta_secs: Option<u64>,
    /// Where received files are being saved.
    pub folder: Option<String>,
    /// Skipped, failed and renamed items (capped; see `issues_truncated`).
    pub issues: Vec<Issue>,
    /// More issues exist than are listed.
    pub issues_truncated: bool,
    /// Why the transfer failed, if it did.
    pub error: Option<ErrorView>,
    /// When the transfer started (ms since the Unix epoch).
    pub started_at: i64,
}

/// A warning shown on an incoming offer. Worked out by the receiver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OfferWarning {
    /// The offer contains programs or scripts.
    Executables {
        /// How many.
        count: u64,
    },
    /// Programs hiding behind a harmless-looking extension (`report.pdf.exe`).
    DoubleExtensions {
        /// How many.
        count: u64,
    },
    /// Names with hidden text-direction characters.
    BidiNames {
        /// How many.
        count: u64,
    },
    /// Names that had to be changed to be safe on this device.
    Renamed {
        /// How many.
        count: u64,
    },
    /// The offer will use most of the remaining space.
    LowSpace,
}

/// An incoming offer waiting for a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OfferView {
    /// Transfer ID (hex).
    pub id: String,
    /// Who is sending.
    pub peer: PeerInfo,
    /// Files offered.
    pub files: u64,
    /// Folders offered.
    pub folders: u64,
    /// Total size in bytes.
    pub bytes: u64,
    /// The first 20 names.
    pub names: Vec<String>,
    /// Warnings.
    pub warnings: Vec<OfferWarning>,
    /// Where it would be saved by default.
    pub folder: String,
    /// Bytes missing at the default folder (0 when there is room).
    pub shortfall: u64,
    /// Ask the person where to save (the "ask each time" setting).
    pub ask_where: bool,
}

/// A text message in the session feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TextView {
    /// Position in the feed.
    pub id: u64,
    /// `true` when the other device sent it.
    pub from_peer: bool,
    /// The text. Never opened or copied automatically.
    pub text: String,
    /// When (ms since the Unix epoch).
    pub at: i64,
}

/// Everything the engine reports.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The session moved to a new state.
    Session {
        /// The new state.
        session: SessionState,
    },
    /// An offer arrived and needs a decision.
    Offer {
        /// The offer.
        offer: OfferView,
    },
    /// A pending offer went away (answered, withdrawn or session over).
    OfferClosed {
        /// Transfer ID (hex).
        id: String,
    },
    /// The other device offered something unsafe and it was refused outright.
    OfferBlocked {
        /// Who sent it.
        peer: PeerInfo,
    },
    /// A transfer changed (state or progress).
    Transfer {
        /// The transfer.
        transfer: TransferView,
    },
    /// A text message was sent or received.
    Text {
        /// The message.
        message: TextView,
    },
}

/// The whole current picture, so an app can redraw from scratch.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    /// Session state.
    pub session: SessionState,
    /// Offers waiting for a decision.
    pub offers: Vec<OfferView>,
    /// Transfers of this session, oldest first.
    pub transfers: Vec<TransferView>,
    /// Text messages of this session, oldest first.
    pub texts: Vec<TextView>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            session: SessionState::Idle,
            offers: Vec::new(),
            transfers: Vec::new(),
            texts: Vec::new(),
        }
    }
}

impl Snapshot {
    /// Folds an event into the picture.
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::Session { session } => self.session = session.clone(),
            Event::Offer { offer } => {
                self.offers.retain(|o| o.id != offer.id);
                self.offers.push(offer.clone());
            }
            Event::OfferClosed { id } => self.offers.retain(|o| &o.id != id),
            Event::OfferBlocked { .. } => {}
            Event::Transfer { transfer } => {
                match self.transfers.iter_mut().find(|t| t.id == transfer.id) {
                    Some(existing) => *existing = transfer.clone(),
                    None => self.transfers.push(transfer.clone()),
                }
            }
            Event::Text { message } => {
                self.texts.push(message.clone());
                // Bound memory: keep the most recent messages.
                if self.texts.len() > 500 {
                    let excess = self.texts.len() - 500;
                    self.texts.drain(..excess);
                }
            }
        }
    }
}
