//! Codes and cryptography for Normandin Tunnel.
//!
//! This crate turns a short one-time connection code into strong session keys and
//! exposes only the handful of operations the rest of the app needs:
//!
//! * [`code`] — generating, formatting and forgivingly parsing connection codes.
//! * [`wordlist`] — the fixed EFF word list codes are made of, plus autocomplete.
//! * [`pairing`] — SPAKE2, the key schedule, key confirmation, the sealed "hello"
//!   mailbox, channel binding and the 6-digit safety code.
//!
//! The cipher suite is fixed for protocol version 1: SPAKE2 on the Ed25519 group,
//! HKDF-SHA256, HMAC-SHA256 and XChaCha20-Poly1305. Changing any of it means a new
//! protocol version. See `docs/PROTOCOL.md` for the byte-exact definitions and the
//! test vectors checked by this crate's tests.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod code;
pub mod pairing;
pub mod wordlist;
mod wordlist_data;

pub use code::{CodeError, ConnectionCode, PairUri};
pub use pairing::{Handshake, PairingError, PakeState, Role, SafetyCode, SessionSecrets};

/// Fills `buf` from the operating system's secure random generator.
///
/// Every random value in Tunnel (code words, transfer IDs, nonces) comes from here.
pub fn random_bytes(buf: &mut [u8]) -> Result<(), RandomError> {
    getrandom::fill(buf).map_err(|_| RandomError)
}

/// The operating system could not provide secure random bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the operating system's secure random generator is unavailable")]
pub struct RandomError;
