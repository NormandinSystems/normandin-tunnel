//! The Normandin Tunnel wire format.
//!
//! This crate defines every byte two devices (and the rendezvous server) exchange:
//! message types, framing, hard size limits and version negotiation. It does no
//! networking and never touches the disk, so every decoder here can be fuzzed in
//! isolation.
//!
//! * [`control`] — messages on the control stream of a session.
//! * [`data`] — the per-file data stream: request, header, bytes, hash trailer.
//! * [`manifest`] — what a sender says it is about to send.
//! * [`hello`] — what each device tells the other during pairing.
//! * [`rendezvous`] — JSON messages between a device and the rendezvous server.
//! * [`frame`] — length-prefixed framing with size checks before allocation.
//! * [`limits`] — every hard limit in one place.
//!
//! Wire stability: `postcard` encodes enum variants by position, so variants and
//! struct fields must never be reordered or removed within a protocol version.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod control;
pub mod data;
pub mod frame;
pub mod hello;
pub mod limits;
pub mod manifest;
pub mod meeting;
pub mod rendezvous;
pub mod version;

pub use control::{CancelReason, ControlMessage, DeclineReason, ErrorCode, Offer, TransferId};
pub use frame::FrameError;
pub use manifest::{EntryKind, ManifestEntry, ManifestPage};
pub use version::{ALPN_CONTROL, PROTOCOL_VERSION, SUPPORTED_VERSIONS};
