//! The per-file data stream.
//!
//! For each file the receiver opens a bidirectional stream and sends one
//! [`Get`] frame. The sender answers with one [`DataHeader`] frame; if it is
//! [`DataHeader::Ok`], the file's bytes from `offset` to the end follow, then a
//! 32-byte trailer: the BLAKE3 hash of the *whole* file. Nothing may follow the
//! trailer.
//!
//! If the file changes or becomes unreadable part-way, the sender resets the
//! stream with one of the [`reset`] codes instead of sending a trailer.

use serde::{Deserialize, Serialize};

use crate::control::TransferId;
use crate::frame::{self, FrameError};
use crate::limits;

/// Length of the BLAKE3 trailer.
pub const TRAILER_LEN: usize = 32;

/// Receiver's request for one file from a byte offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Get {
    /// The accepted transfer the file belongs to.
    pub transfer_id: TransferId,
    /// The file.
    pub file_id: u32,
    /// First byte wanted; everything before it is already on the receiver's disk.
    pub offset: u64,
}

/// Why a sender will not serve a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// The transfer was never accepted, or is finished or cancelled.
    NotAccepted,
    /// No such file in the transfer, or it is a folder.
    UnknownFile,
    /// The offset is past the end of the file.
    BadOffset,
    /// The file can no longer be read on the sender.
    Unreadable,
    /// The file's size or modified time changed since the offer.
    Changed,
    /// The transfer is paused on the sender.
    Paused,
}

/// Sender's answer to a [`Get`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataHeader {
    /// Bytes follow.
    Ok {
        /// Total size of the file; `size - offset` bytes follow.
        size: u64,
        /// Echo of the requested offset.
        offset: u64,
    },
    /// No bytes follow; the stream ends.
    Refused(Refusal),
}

impl Get {
    /// Encodes the request as one frame.
    pub fn to_frame(&self) -> Result<Vec<u8>, FrameError> {
        frame::encode(self, limits::MAX_DATA_FRAME)
    }
}

impl DataHeader {
    /// Encodes the header as one frame.
    pub fn to_frame(&self) -> Result<Vec<u8>, FrameError> {
        frame::encode(self, limits::MAX_DATA_FRAME)
    }
}

/// QUIC stream reset / stop codes.
pub mod reset {
    /// The file changed while it was being sent.
    pub const CHANGED: u32 = 1;
    /// The file could not be read to the end.
    pub const UNREADABLE: u32 = 2;
    /// The transfer was paused.
    pub const PAUSED: u32 = 3;
    /// The transfer was cancelled.
    pub const CANCELLED: u32 = 4;
    /// The peer broke the protocol on this stream.
    pub const PROTOCOL: u32 = 5;
    /// The receiver could not write the data (disk full, folder gone).
    pub const WRITE_FAILED: u32 = 6;
}

/// QUIC connection close codes.
pub mod close {
    /// Normal end of session.
    pub const BYE: u32 = 0;
    /// Protocol error.
    pub const PROTOCOL: u32 = 1;
    /// Channel binding failed.
    pub const BIND_FAILED: u32 = 2;
    /// The safety codes did not match.
    pub const SAFETY_MISMATCH: u32 = 3;
    /// A connection from an unexpected device.
    pub const UNEXPECTED_PEER: u32 = 4;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_round_trips_and_is_small() {
        let get = Get {
            transfer_id: TransferId([9; 16]),
            file_id: u32::MAX,
            offset: u64::MAX,
        };
        let bytes = get.to_frame().unwrap();
        assert!(bytes.len() <= 64);
        let (decoded, used) = frame::decode::<Get>(&bytes, limits::MAX_DATA_FRAME)
            .unwrap()
            .unwrap();
        assert_eq!((decoded, used), (get, bytes.len()));
    }

    #[test]
    fn header_round_trips() {
        for header in [
            DataHeader::Ok {
                size: 10,
                offset: 3,
            },
            DataHeader::Refused(Refusal::NotAccepted),
            DataHeader::Refused(Refusal::Changed),
        ] {
            let bytes = header.to_frame().unwrap();
            let (decoded, _) = frame::decode::<DataHeader>(&bytes, limits::MAX_DATA_FRAME)
                .unwrap()
                .unwrap();
            assert_eq!(decoded, header);
        }
    }
}
