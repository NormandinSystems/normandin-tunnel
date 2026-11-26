//! Length-prefixed framing.
//!
//! Every message is a 4-byte big-endian length followed by a `postcard`-encoded
//! value. The length is checked against a hard limit *before* anything is
//! allocated, so a hostile peer cannot make us reserve memory by lying about it.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Size of the length prefix.
pub const HEADER_LEN: usize = 4;

/// A frame could not be produced or understood.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// The frame is larger than the limit for this stream.
    #[error("frame of {len} bytes exceeds the {max}-byte limit")]
    TooLarge {
        /// Declared or produced length.
        len: usize,
        /// The limit that applies.
        max: usize,
    },
    /// A frame with no payload.
    #[error("empty frame")]
    Empty,
    /// The payload is not a valid message (includes unknown message types).
    #[error("malformed message")]
    Malformed,
    /// Bytes were left over after the message.
    #[error("trailing bytes after message")]
    TrailingBytes,
    /// A field inside the message breaks a limit.
    #[error("message field out of range: {0}")]
    Invalid(&'static str),
}

/// Validates a length prefix and returns the payload length to read.
pub fn payload_len(header: [u8; HEADER_LEN], max: usize) -> Result<usize, FrameError> {
    let len = u32::from_be_bytes(header) as usize;
    if len == 0 {
        return Err(FrameError::Empty);
    }
    if len > max {
        return Err(FrameError::TooLarge { len, max });
    }
    Ok(len)
}

/// Encodes `message` as one frame (length prefix included).
pub fn encode<T: Serialize>(message: &T, max: usize) -> Result<Vec<u8>, FrameError> {
    let mut frame = vec![0u8; HEADER_LEN];
    frame = postcard::to_extend(message, frame).map_err(|_| FrameError::Malformed)?;
    let len = frame.len() - HEADER_LEN;
    if len > max {
        return Err(FrameError::TooLarge { len, max });
    }
    frame[..HEADER_LEN].copy_from_slice(&(len as u32).to_be_bytes());
    Ok(frame)
}

/// Decodes a frame payload (without its length prefix).
///
/// Unknown message types and trailing bytes are errors: the session must close.
pub fn decode_payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T, FrameError> {
    let (message, rest) =
        postcard::take_from_bytes::<T>(payload).map_err(|_| FrameError::Malformed)?;
    if !rest.is_empty() {
        return Err(FrameError::TrailingBytes);
    }
    Ok(message)
}

/// Decodes one complete frame from the start of `bytes`, returning the message
/// and how many bytes it used. Returns `Ok(None)` when more bytes are needed.
pub fn decode<T: DeserializeOwned>(
    bytes: &[u8],
    max: usize,
) -> Result<Option<(T, usize)>, FrameError> {
    let Some(header) = bytes.first_chunk::<HEADER_LEN>() else {
        return Ok(None);
    };
    let len = payload_len(*header, max)?;
    let Some(payload) = bytes.get(HEADER_LEN..HEADER_LEN + len) else {
        return Ok(None);
    };
    Ok(Some((decode_payload(payload)?, HEADER_LEN + len)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::ControlMessage;
    use crate::limits::MAX_CONTROL_FRAME;

    #[test]
    fn round_trip() {
        let frame = encode(&ControlMessage::SafetyConfirmed, MAX_CONTROL_FRAME).unwrap();
        let (message, used) = decode::<ControlMessage>(&frame, MAX_CONTROL_FRAME)
            .unwrap()
            .unwrap();
        assert_eq!(message, ControlMessage::SafetyConfirmed);
        assert_eq!(used, frame.len());
    }

    #[test]
    fn incomplete_frames_ask_for_more() {
        let frame = encode(&ControlMessage::Bye, MAX_CONTROL_FRAME).unwrap();
        for cut in 0..frame.len() {
            assert_eq!(
                decode::<ControlMessage>(&frame[..cut], MAX_CONTROL_FRAME),
                Ok(None)
            );
        }
    }

    #[test]
    fn oversized_length_is_rejected_before_reading() {
        let header = (MAX_CONTROL_FRAME as u32 + 1).to_be_bytes();
        assert!(matches!(
            payload_len(header, MAX_CONTROL_FRAME),
            Err(FrameError::TooLarge { .. })
        ));
        assert_eq!(
            payload_len([0; 4], MAX_CONTROL_FRAME),
            Err(FrameError::Empty)
        );
        assert!(matches!(
            payload_len([0xff; 4], MAX_CONTROL_FRAME),
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[test]
    fn unknown_message_type_is_an_error() {
        // Variant index 200 does not exist.
        assert_eq!(
            decode_payload::<ControlMessage>(&[200, 1]),
            Err(FrameError::Malformed)
        );
    }

    #[test]
    fn trailing_bytes_are_an_error() {
        let mut frame = encode(&ControlMessage::Bye, MAX_CONTROL_FRAME).unwrap();
        frame.push(0);
        let len = (frame.len() - HEADER_LEN) as u32;
        frame[..HEADER_LEN].copy_from_slice(&len.to_be_bytes());
        assert_eq!(
            decode::<ControlMessage>(&frame, MAX_CONTROL_FRAME),
            Err(FrameError::TrailingBytes)
        );
    }
}
