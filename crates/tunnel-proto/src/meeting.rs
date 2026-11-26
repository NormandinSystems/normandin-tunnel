//! Pairing without a rendezvous server: the meeting-point connection.
//!
//! The device showing a code listens on the relay network under an endpoint
//! key derived from the code's public nameplate. The device typing the code
//! derives the same key, dials it, and the two exchange the same
//! [`PairingMessage`](crate::hello::PairingMessage)s a rendezvous room would
//! carry, as length-prefixed frames on one stream. Nothing here is trusted:
//! see `docs/PROTOCOL.md`, "Pairing without a server".

use std::time::Duration;

/// ALPN of the meeting-point connection.
pub const ALPN: &[u8] = b"tunnel/pair/1";

/// Largest pairing frame, the same limit a rendezvous room applies.
pub const MAX_FRAME: usize = crate::rendezvous::MAX_BODY_BYTES;

/// Devices that may be connected to a meeting point before one of them
/// answers. More than this are turned away as "busy".
pub const MAX_WAITING: usize = 8;

/// How long a connected device has to answer the host's first message before
/// it is dropped to make room for others. Dropping it does not use up the code:
/// it never made a guess.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(20);

/// How long the device typing the code waits for the meeting point to answer.
/// Through a relay this normally takes a second or two; after this long the
/// person is told that no device is showing that code.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(12);

/// How long the device showing the code waits to reach a relay before giving up.
pub const ONLINE_TIMEOUT: Duration = Duration::from_secs(15);

/// Application close codes of the meeting-point connection.
pub mod close {
    /// Pairing is over (either way) and this connection is no longer needed.
    pub const DONE: u32 = 0;
    /// Another device already answered this code.
    pub const IN_USE: u32 = 1;
    /// Too many devices are waiting at this meeting point.
    pub const BUSY: u32 = 2;
    /// The device did not answer in time.
    pub const TOO_SLOW: u32 = 3;
}
