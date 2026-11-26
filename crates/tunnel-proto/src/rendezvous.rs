//! Messages between a device and the rendezvous server.
//!
//! The rendezvous server only introduces two devices: it hands out a nameplate,
//! lets a second device join the room, and passes a handful of opaque pairing
//! messages between them. It never sees the code words, keys or files.
//!
//! Transport: WebSocket at `wss://<host>/v1`, one JSON text message per frame,
//! at most [`MAX_CLIENT_MESSAGE_BYTES`] bytes.

use serde::{Deserialize, Serialize};

/// WebSocket path of protocol version 1.
pub const PATH: &str = "/v1";
/// Health-check path (`GET`, answers `200 ok`).
pub const HEALTH_PATH: &str = "/healthz";
/// Largest JSON message a client may send.
pub const MAX_CLIENT_MESSAGE_BYTES: usize = 8 * 1024;
/// Largest pairing message, before base64.
pub const MAX_BODY_BYTES: usize = 4 * 1024;
/// Largest base64 `body` string (`MAX_BODY_BYTES` encoded with padding).
pub const MAX_BODY_BASE64_CHARS: usize = MAX_BODY_BYTES.div_ceil(3) * 4;
/// Members a room can hold.
pub const ROOM_MEMBERS: usize = 2;
/// Messages a room will carry in total.
pub const ROOM_MAX_MESSAGES: usize = 16;
/// Seconds a room (and its code) lives.
pub const ROOM_LIFETIME_SECS: u64 = 600;
/// Seconds a freed nameplate stays unused.
pub const NAMEPLATE_COOLDOWN_SECS: u64 = 600;
/// Rooms the server will hold at once.
pub const MAX_ROOMS: usize = 10_000;
/// `create` requests allowed per IP address per minute.
pub const CREATES_PER_MINUTE: u32 = 20;
/// `join` requests allowed per IP address per minute.
pub const JOINS_PER_MINUTE: u32 = 20;
/// Open connections allowed per IP address.
pub const CONNECTIONS_PER_IP: usize = 10;

/// What a device can say to the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    /// Open a new room; the server answers `created`.
    Create,
    /// Join the room behind a nameplate; the server answers `joined`.
    Join {
        /// The number at the start of the connection code.
        nameplate: u16,
    },
    /// Pass one opaque pairing message to the other device.
    Post {
        /// Base64 (standard alphabet, padded) of at most 4 KiB.
        body: String,
    },
    /// Leave and close the room.
    Close,
}

/// What the server can say to a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerMessage {
    /// The room exists.
    Created {
        /// The nameplate to show at the start of the code.
        nameplate: u16,
        /// Seconds until the room expires.
        expires_in: u64,
    },
    /// The device joined an existing room.
    Joined,
    /// The other device arrived.
    PeerJoined,
    /// A pairing message from the other device.
    Message {
        /// Base64 body exactly as the other device posted it.
        body: String,
    },
    /// The other device left; the room is closed.
    PeerLeft,
    /// Something went wrong; the connection closes after most errors.
    Error {
        /// Why.
        code: ErrorCode,
    },
}

/// Rendezvous error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// No room behind that nameplate.
    UnknownCode,
    /// The room already has two devices: someone used this code first.
    CodeInUse,
    /// The room's 10 minutes are over.
    Expired,
    /// Too many requests from this address.
    RateLimited,
    /// A message was larger than allowed, or the room's message count ran out.
    TooLarge,
    /// The message made no sense in the current state.
    BadRequest,
    /// The server is full.
    ServerBusy,
}

/// Checks that a `post` body is well-formed base64 of an allowed size, without
/// decoding it.
pub fn is_valid_body(body: &str) -> bool {
    let bytes = body.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_BODY_BASE64_CHARS || !bytes.len().is_multiple_of(4) {
        return false;
    }
    let padding = bytes.iter().rev().take_while(|b| **b == b'=').count();
    padding <= 2
        && bytes[..bytes.len() - padding]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/'))
}

/// Parses a client message, enforcing the size limit first.
pub fn parse_client(text: &str) -> Result<ClientMessage, ErrorCode> {
    if text.len() > MAX_CLIENT_MESSAGE_BYTES {
        return Err(ErrorCode::TooLarge);
    }
    let message: ClientMessage = serde_json::from_str(text).map_err(|_| ErrorCode::BadRequest)?;
    if let ClientMessage::Post { body } = &message
        && !is_valid_body(body)
    {
        return Err(if body.len() > MAX_BODY_BASE64_CHARS {
            ErrorCode::TooLarge
        } else {
            ErrorCode::BadRequest
        });
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn json_shapes_match_the_specification() {
        let json = |m: &ClientMessage| serde_json::to_string(m).unwrap();
        assert_eq!(json(&ClientMessage::Create), r#"{"type":"create"}"#);
        assert_eq!(
            json(&ClientMessage::Join { nameplate: 42 }),
            r#"{"type":"join","nameplate":42}"#
        );
        assert_eq!(
            json(&ClientMessage::Post {
                body: "AAAA".into()
            }),
            r#"{"type":"post","body":"AAAA"}"#
        );
        assert_eq!(json(&ClientMessage::Close), r#"{"type":"close"}"#);
        let json = |m: &ServerMessage| serde_json::to_string(m).unwrap();
        assert_eq!(
            json(&ServerMessage::Created {
                nameplate: 7,
                expires_in: 600
            }),
            r#"{"type":"created","nameplate":7,"expires_in":600}"#
        );
        assert_eq!(
            json(&ServerMessage::PeerJoined),
            r#"{"type":"peer_joined"}"#
        );
        assert_eq!(
            json(&ServerMessage::Error {
                code: ErrorCode::CodeInUse
            }),
            r#"{"type":"error","code":"code_in_use"}"#
        );
    }

    #[test]
    fn client_messages_are_validated() {
        assert_eq!(
            parse_client(r#"{"type":"create"}"#),
            Ok(ClientMessage::Create)
        );
        assert_eq!(
            parse_client(r#"{"type":"nope"}"#),
            Err(ErrorCode::BadRequest)
        );
        assert_eq!(
            parse_client(r#"{"type":"join"}"#),
            Err(ErrorCode::BadRequest)
        );
        assert_eq!(
            parse_client(r#"{"type":"join","nameplate":70000}"#),
            Err(ErrorCode::BadRequest)
        );
        assert_eq!(
            parse_client(r#"{"type":"join","nameplate":1,"extra":1}"#),
            Err(ErrorCode::BadRequest)
        );
        assert_eq!(
            parse_client(r#"{"type":"post","body":"not base64!"}"#),
            Err(ErrorCode::BadRequest)
        );
        assert_eq!(
            parse_client(r#"{"type":"post","body":""}"#),
            Err(ErrorCode::BadRequest)
        );
        let big = format!(
            r#"{{"type":"post","body":"{}"}}"#,
            "A".repeat(MAX_BODY_BASE64_CHARS + 4)
        );
        assert_eq!(parse_client(&big), Err(ErrorCode::TooLarge));
        let huge = format!(
            r#"{{"type":"post","body":"{}"}}"#,
            "A".repeat(MAX_CLIENT_MESSAGE_BYTES)
        );
        assert_eq!(parse_client(&huge), Err(ErrorCode::TooLarge));
        let ok = format!(
            r#"{{"type":"post","body":"{}"}}"#,
            "A".repeat(MAX_BODY_BASE64_CHARS)
        );
        assert!(parse_client(&ok).is_ok());
    }

    #[test]
    fn body_validation() {
        assert!(is_valid_body("AAAA"));
        assert!(is_valid_body("AA=="));
        assert!(is_valid_body("ab+/"));
        assert!(!is_valid_body("A==="));
        assert!(!is_valid_body("AAA"));
        assert!(!is_valid_body("AA=A"));
        assert!(!is_valid_body("AA A"));
    }

    proptest! {
        #[test]
        fn parser_never_panics(text in "\\PC{0,300}") {
            let _ = parse_client(&text);
        }
    }
}
