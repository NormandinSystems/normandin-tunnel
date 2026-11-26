//! Errors, each with a stable code the apps translate into plain language.

use serde::Serialize;

/// Everything that can go wrong, grouped by what the person should do next.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The typed text is not a code.
    #[error("That doesn't look like a code. A code looks like 4821-orbit-velvet-candle.")]
    CodeInvalid,
    /// Wrong code, or key confirmation failed.
    #[error("That code didn't work. Codes work once, so ask for a new one.")]
    CodeFailed,
    /// The code's ten minutes are over.
    #[error("This code expired. Make a new one.")]
    CodeExpired,
    /// Pairing without a server: nothing answered for this code's number.
    #[error("No device is showing that code right now. Check the number, or ask for a new code.")]
    CodeNotFound,
    /// Another device used the code first.
    #[error(
        "Someone already used this code. If it wasn't your friend, end the session and start over."
    )]
    CodeInUse,
    /// The rendezvous server could not be reached.
    #[error("Can't reach the pairing network. Check your internet connection.")]
    RendezvousUnreachable,
    /// The rendezvous server refused for now.
    #[error("The pairing service is busy. Wait a minute and try again.")]
    RendezvousBusy,
    /// The rendezvous address in settings or in a link is not usable.
    #[error("The pairing service address isn't valid. Check Settings > Advanced.")]
    RendezvousAddressInvalid,
    /// The two devices could not reach each other.
    #[error("Couldn't connect to the other device. A firewall may be blocking the connection.")]
    PeerUnreachable,
    /// No protocol version in common.
    #[error("The other device needs an update. Update both to the latest Tunnel.")]
    VersionMismatch,
    /// The device that connected is not the one that knew the code.
    #[error("The connection could not be verified, so it was closed. Start over with a new code.")]
    BindFailed,
    /// One of the two people said the safety codes differ.
    #[error("The safety codes didn't match, so the session was ended.")]
    SafetyMismatch,
    /// The connection dropped and could not be restored.
    #[error("The connection was lost.")]
    ConnectionLost,
    /// The other device ended the session.
    #[error("The other device ended the session.")]
    PeerEnded,
    /// The other device broke the protocol.
    #[error("The other device sent something unexpected, so the session was closed.")]
    Protocol,
    /// A session is already running.
    #[error("A session is already open. End it before starting another.")]
    SessionBusy,
    /// There is no session, or it is not in the right state.
    #[error("There is no open session for that.")]
    NoSession,
    /// The transfer does not exist (any more).
    #[error("That transfer is no longer available.")]
    UnknownTransfer,
    /// Not enough free space at the destination.
    #[error("Not enough space. Free up {shortfall} bytes or choose another folder.")]
    NoSpace {
        /// Bytes missing.
        shortfall: u64,
    },
    /// The destination folder cannot be written.
    #[error("Can't save to that folder. Choose another one.")]
    DestinationNotWritable,
    /// Too many transfers at once.
    #[error("Too many transfers are running. Wait for one to finish.")]
    TooManyTransfers,
    /// Nothing sendable was selected.
    #[error("There is nothing to send in that selection.")]
    NothingToSend,
    /// The selection has more entries than the limit.
    #[error("That selection has too many files for one transfer.")]
    TooManyEntries,
    /// The text is longer than 64 KiB.
    #[error("That text is too long to send as a message. Send it as a file instead.")]
    TextTooLong,
    /// The session was cancelled by the person.
    #[error("Cancelled.")]
    Cancelled,
    /// A file-system error.
    #[error("A file error occurred: {0}")]
    Io(String),
    /// Something unexpected inside the app.
    #[error("Something went wrong inside Tunnel: {0}")]
    Internal(String),
}

impl Error {
    /// Stable machine-readable code, used as the translation key in the apps.
    pub fn code(&self) -> &'static str {
        match self {
            Error::CodeInvalid => "code_invalid",
            Error::CodeFailed => "code_failed",
            Error::CodeExpired => "code_expired",
            Error::CodeNotFound => "code_not_found",
            Error::CodeInUse => "code_in_use",
            Error::RendezvousUnreachable => "rendezvous_unreachable",
            Error::RendezvousBusy => "rendezvous_busy",
            Error::RendezvousAddressInvalid => "rendezvous_address_invalid",
            Error::PeerUnreachable => "peer_unreachable",
            Error::VersionMismatch => "version_mismatch",
            Error::BindFailed => "bind_failed",
            Error::SafetyMismatch => "safety_mismatch",
            Error::ConnectionLost => "connection_lost",
            Error::PeerEnded => "peer_ended",
            Error::Protocol => "protocol",
            Error::SessionBusy => "session_busy",
            Error::NoSession => "no_session",
            Error::UnknownTransfer => "unknown_transfer",
            Error::NoSpace { .. } => "no_space",
            Error::DestinationNotWritable => "destination_not_writable",
            Error::TooManyTransfers => "too_many_transfers",
            Error::NothingToSend => "nothing_to_send",
            Error::TooManyEntries => "too_many_entries",
            Error::TextTooLong => "text_too_long",
            Error::Cancelled => "cancelled",
            Error::Io(_) => "io",
            Error::Internal(_) => "internal",
        }
    }

    /// The serializable form sent to the apps.
    pub fn view(&self) -> ErrorView {
        ErrorView {
            code: self.code(),
            message: self.to_string(),
            shortfall: match self {
                Error::NoSpace { shortfall } => Some(*shortfall),
                _ => None,
            },
        }
    }

    pub(crate) fn io(err: impl std::fmt::Display) -> Self {
        Error::Io(err.to_string())
    }

    pub(crate) fn internal(err: impl std::fmt::Display) -> Self {
        Error::Internal(err.to_string())
    }
}

/// An error as the apps see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorView {
    /// Stable code (translation key).
    pub code: &'static str,
    /// English text, used when no translation exists.
    pub message: String,
    /// Bytes of free space missing, for `no_space`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortfall: Option<u64>,
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.view().serialize(serializer)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(err: rusqlite::Error) -> Self {
        Error::Internal(format!("database: {err}"))
    }
}

/// Shorthand result type.
pub type Result<T, E = Error> = std::result::Result<T, E>;
