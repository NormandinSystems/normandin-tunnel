//! Messages on the control stream.
//!
//! After pairing, one QUIC connection carries a control stream for these
//! messages and one stream per file for data. Each control message is one
//! frame: a 4-byte big-endian length, then this enum encoded with `postcard`.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::frame::{self, FrameError};
use crate::limits;
use crate::manifest::ManifestPage;

/// 16 random bytes naming one transfer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TransferId(pub [u8; 16]);

impl TransferId {
    /// Lower-case hex, used for folder names and in the UI layer.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Parses the hex form produced by [`TransferId::to_hex`].
    pub fn from_hex(text: &str) -> Option<Self> {
        if text.len() != 32 || !text.is_ascii() {
            return None;
        }
        let mut id = [0u8; 16];
        for (i, byte) in id.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Self(id))
    }
}

impl fmt::Debug for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TransferId({})", self.to_hex())
    }
}

impl fmt::Display for TransferId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// A sender's announcement of a transfer. The manifest pages follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    /// Fresh random ID for this transfer.
    pub transfer_id: TransferId,
    /// Number of files.
    pub file_count: u64,
    /// Number of folders.
    pub folder_count: u64,
    /// Sum of all file sizes.
    pub total_bytes: u64,
    /// Number of [`ManifestPage`] messages that follow.
    pub manifest_pages: u32,
}

/// Why a receiver declined an offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeclineReason {
    /// The person said no.
    Declined,
    /// The offer or its manifest broke a rule (unsafe path, wrong totals...).
    Invalid,
    /// Too many offers are already waiting or running.
    Busy,
    /// Not enough free space at the destination.
    NoSpace,
}

/// Why a transfer was paused or cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CancelReason {
    /// The person asked for it.
    User,
    /// The disk is full.
    DiskFull,
    /// The destination can no longer be written.
    DestinationUnavailable,
    /// The session is ending.
    SessionEnding,
    /// The other side broke the protocol.
    ProtocolError,
    /// The transfer is not known to this device.
    Unknown,
}

/// Coded errors, each with a short plain-language message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    /// The channel-binding tag was wrong: this is not the device that knew the code.
    BindFailed,
    /// The person said the safety codes differ.
    SafetyMismatch,
    /// A message arrived that is not allowed in the current state.
    UnexpectedMessage,
    /// A frame or field broke a limit.
    LimitExceeded,
    /// The message could not be decoded.
    Malformed,
    /// No protocol version in common.
    VersionMismatch,
    /// Something failed inside the other device.
    Internal,
}

/// Verified progress of one unfinished file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeEntry {
    /// The file.
    pub file_id: u32,
    /// Bytes already safely on the receiver's disk.
    pub offset: u64,
}

/// Final tally of a transfer, sent by the receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Completed {
    /// The transfer.
    pub transfer_id: TransferId,
    /// Files verified and moved into place.
    pub received: u64,
    /// Files skipped (conflict rule, or unreadable on the sender).
    pub skipped: u64,
    /// Files that failed.
    pub failed: u64,
}

/// Everything that can be said on the control stream.
///
/// Variant order is the wire format. Append only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlMessage {
    /// Channel binding: must be the first frame each side sends.
    Bind {
        /// `HMAC(bind key, "bind" ‖ role ‖ A's endpoint ID ‖ B's endpoint ID ‖ T)`.
        mac: [u8; 32],
    },
    /// The person tapped "Codes match".
    SafetyConfirmed,
    /// A transfer is offered.
    Offer(Offer),
    /// Part of the manifest of an offered transfer.
    ManifestPage(ManifestPage),
    /// The receiver accepted (where it saves stays private to the receiver).
    Accept {
        /// The accepted transfer.
        transfer_id: TransferId,
    },
    /// The receiver declined.
    Decline {
        /// The declined transfer.
        transfer_id: TransferId,
        /// Why.
        reason: DeclineReason,
    },
    /// The receiver continues a transfer after a dropped connection.
    Resume {
        /// The transfer.
        transfer_id: TransferId,
        /// Verified byte offset of each unfinished file that has data on disk.
        files: Vec<ResumeEntry>,
    },
    /// Stop sending data for a transfer for now.
    Pause {
        /// The transfer.
        transfer_id: TransferId,
        /// Why.
        reason: CancelReason,
    },
    /// Continue a paused transfer.
    Unpause {
        /// The transfer.
        transfer_id: TransferId,
    },
    /// Stop a transfer for good.
    Cancel {
        /// The transfer.
        transfer_id: TransferId,
        /// Why.
        reason: CancelReason,
    },
    /// The receiver has dealt with every file.
    Completed(Completed),
    /// A text message or link (never opened or copied automatically).
    Text {
        /// UTF-8 text, at most 64 KiB.
        text: String,
    },
    /// "Remember this device": offers this device's long-term public key.
    TrustOffer {
        /// Long-term Ed25519 public key.
        device_key: [u8; 32],
    },
    /// "Remember this device": accepts and answers with this device's key.
    TrustAccept {
        /// Long-term Ed25519 public key.
        device_key: [u8; 32],
    },
    /// Clean close.
    Bye,
    /// A coded error; the session closes after it.
    Error {
        /// What went wrong.
        code: ErrorCode,
        /// Short plain-language text.
        message: String,
    },
}

impl ControlMessage {
    /// Encodes the message as one control frame.
    pub fn to_frame(&self) -> Result<Vec<u8>, FrameError> {
        self.check_limits()?;
        frame::encode(self, limits::MAX_CONTROL_FRAME)
    }

    /// Decodes a control-frame payload and checks every field limit.
    pub fn from_payload(payload: &[u8]) -> Result<Self, FrameError> {
        let message: Self = frame::decode_payload(payload)?;
        message.check_limits()?;
        Ok(message)
    }

    /// Checks the limits that framing alone cannot enforce.
    pub fn check_limits(&self) -> Result<(), FrameError> {
        match self {
            ControlMessage::Text { text } if text.len() > limits::MAX_TEXT_BYTES => {
                Err(FrameError::Invalid("text too long"))
            }
            ControlMessage::Error { message, .. } if message.len() > limits::MAX_REASON_BYTES => {
                Err(FrameError::Invalid("error text too long"))
            }
            ControlMessage::Offer(offer) => {
                if offer.file_count.saturating_add(offer.folder_count) > limits::HARD_MAX_ENTRIES {
                    return Err(FrameError::Invalid("too many entries"));
                }
                if offer.file_count + offer.folder_count == 0 {
                    return Err(FrameError::Invalid("empty offer"));
                }
                Ok(())
            }
            ControlMessage::ManifestPage(page) => {
                if page.entries.is_empty() {
                    return Err(FrameError::Invalid("empty manifest page"));
                }
                page.entries
                    .iter()
                    .try_for_each(|entry| entry.check_limits())
            }
            _ => Ok(()),
        }
    }

    /// Short name for logs (never includes contents).
    pub fn name(&self) -> &'static str {
        match self {
            ControlMessage::Bind { .. } => "Bind",
            ControlMessage::SafetyConfirmed => "SafetyConfirmed",
            ControlMessage::Offer(_) => "Offer",
            ControlMessage::ManifestPage(_) => "ManifestPage",
            ControlMessage::Accept { .. } => "Accept",
            ControlMessage::Decline { .. } => "Decline",
            ControlMessage::Resume { .. } => "Resume",
            ControlMessage::Pause { .. } => "Pause",
            ControlMessage::Unpause { .. } => "Unpause",
            ControlMessage::Cancel { .. } => "Cancel",
            ControlMessage::Completed(_) => "Completed",
            ControlMessage::Text { .. } => "Text",
            ControlMessage::TrustOffer { .. } => "TrustOffer",
            ControlMessage::TrustAccept { .. } => "TrustAccept",
            ControlMessage::Bye => "Bye",
            ControlMessage::Error { .. } => "Error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{EntryKind, ManifestEntry};
    use proptest::prelude::*;

    #[test]
    fn transfer_id_hex_round_trips() {
        let id = TransferId([0xab; 16]);
        assert_eq!(id.to_hex(), "ab".repeat(16));
        assert_eq!(TransferId::from_hex(&id.to_hex()), Some(id));
        assert_eq!(TransferId::from_hex("zz"), None);
        assert_eq!(TransferId::from_hex(&"g".repeat(32)), None);
        assert_eq!(TransferId::from_hex(&"é".repeat(16)), None);
    }

    #[test]
    fn variant_tags_are_stable() {
        // These bytes are the wire format. If this test changes, the protocol
        // version must change with it.
        let tag = |m: &ControlMessage| postcard::to_allocvec(m).unwrap()[0];
        assert_eq!(tag(&ControlMessage::Bind { mac: [0; 32] }), 0);
        assert_eq!(tag(&ControlMessage::SafetyConfirmed), 1);
        assert_eq!(
            tag(&ControlMessage::Accept {
                transfer_id: TransferId([0; 16])
            }),
            4
        );
        assert_eq!(
            tag(&ControlMessage::Text {
                text: String::new()
            }),
            11
        );
        assert_eq!(tag(&ControlMessage::Bye), 14);
        assert_eq!(
            tag(&ControlMessage::Error {
                code: ErrorCode::Internal,
                message: String::new()
            }),
            15
        );
    }

    #[test]
    fn oversized_fields_are_rejected() {
        let text = ControlMessage::Text {
            text: "x".repeat(limits::MAX_TEXT_BYTES + 1),
        };
        assert!(text.to_frame().is_err());
        let payload = postcard::to_allocvec(&text).unwrap();
        assert!(ControlMessage::from_payload(&payload).is_err());

        let offer = ControlMessage::Offer(Offer {
            transfer_id: TransferId([1; 16]),
            file_count: limits::HARD_MAX_ENTRIES,
            folder_count: 1,
            total_bytes: 0,
            manifest_pages: 1,
        });
        assert!(offer.check_limits().is_err());
        let empty = ControlMessage::Offer(Offer {
            transfer_id: TransferId([1; 16]),
            file_count: 0,
            folder_count: 0,
            total_bytes: 0,
            manifest_pages: 0,
        });
        assert!(empty.check_limits().is_err());
    }

    fn arb_entry() -> impl Strategy<Value = ManifestEntry> {
        (
            any::<u32>(),
            any::<bool>(),
            proptest::collection::vec("\\PC{0,24}", 1..5),
            0u64..limits::MAX_FILE_SIZE,
            any::<i64>(),
            any::<bool>(),
        )
            .prop_map(|(file_id, is_file, path, size, modified_ms, executable)| {
                ManifestEntry {
                    file_id,
                    kind: if is_file {
                        EntryKind::File
                    } else {
                        EntryKind::Folder
                    },
                    path,
                    size: if is_file { size } else { 0 },
                    modified_ms,
                    executable,
                }
            })
    }

    fn arb_message() -> impl Strategy<Value = ControlMessage> {
        let id = any::<[u8; 16]>().prop_map(TransferId);
        prop_oneof![
            any::<[u8; 32]>().prop_map(|mac| ControlMessage::Bind { mac }),
            Just(ControlMessage::SafetyConfirmed),
            (
                id.clone(),
                1u64..1000,
                0u64..1000,
                any::<u64>(),
                any::<u32>()
            )
                .prop_map(
                    |(transfer_id, file_count, folder_count, total_bytes, manifest_pages)| {
                        ControlMessage::Offer(Offer {
                            transfer_id,
                            file_count,
                            folder_count,
                            total_bytes,
                            manifest_pages,
                        })
                    }
                ),
            (
                id.clone(),
                any::<u32>(),
                proptest::collection::vec(arb_entry(), 1..8)
            )
                .prop_map(|(transfer_id, page, entries)| {
                    ControlMessage::ManifestPage(ManifestPage {
                        transfer_id,
                        page,
                        entries,
                    })
                }),
            id.clone()
                .prop_map(|transfer_id| ControlMessage::Accept { transfer_id }),
            id.clone()
                .prop_map(|transfer_id| ControlMessage::Unpause { transfer_id }),
            (
                id,
                proptest::collection::vec((any::<u32>(), any::<u64>()), 0..8)
            )
                .prop_map(|(transfer_id, files)| ControlMessage::Resume {
                    transfer_id,
                    files: files
                        .into_iter()
                        .map(|(file_id, offset)| ResumeEntry { file_id, offset })
                        .collect(),
                }),
            "\\PC{0,200}".prop_map(|text| ControlMessage::Text { text }),
            Just(ControlMessage::Bye),
        ]
    }

    proptest! {
        #[test]
        fn encode_then_decode_returns_the_original(message in arb_message()) {
            let frame = message.to_frame().unwrap();
            let (decoded, used) = frame::decode::<ControlMessage>(&frame, limits::MAX_CONTROL_FRAME)
                .unwrap()
                .unwrap();
            prop_assert_eq!(used, frame.len());
            prop_assert_eq!(decoded, message);
        }

        #[test]
        fn decoder_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _ = ControlMessage::from_payload(&bytes);
            let _ = frame::decode::<ControlMessage>(&bytes, limits::MAX_CONTROL_FRAME);
        }
    }
}
