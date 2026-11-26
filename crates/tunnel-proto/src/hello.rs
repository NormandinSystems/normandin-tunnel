//! The hello each device seals for the other during pairing, and the envelope
//! pairing messages travel in while they pass through the rendezvous server.

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::frame::FrameError;
use crate::limits;

/// What kind of device this is; picks the icon next to the device name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeviceType {
    /// Desktop or laptop computer.
    Desktop,
    /// Phone.
    Phone,
    /// Tablet.
    Tablet,
    /// Server, NAS or other headless machine (command-line app).
    Server,
}

/// Sealed with the mailbox key; tells the other device how to reach this one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Protocol versions this device speaks.
    pub versions: Vec<u16>,
    /// This session's endpoint ID (a fresh public key for one-off sessions).
    pub endpoint_id: [u8; 32],
    /// The relay this device is reachable through, if any.
    pub relay_url: Option<String>,
    /// Addresses this device may be reachable at directly.
    pub direct_addrs: Vec<SocketAddr>,
    /// Name the person gave this device.
    pub device_name: String,
    /// Kind of device.
    pub device_type: DeviceType,
    /// App version, for "please update" messages.
    pub app_version: String,
    /// Optional features, as a bit set (none defined in version 1).
    pub capabilities: u64,
}

impl Hello {
    /// Encodes the hello (to be sealed).
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        self.check_limits()?;
        postcard::to_allocvec(self).map_err(|_| FrameError::Malformed)
    }

    /// Decodes an opened hello and checks every limit.
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        let hello: Self = crate::frame::decode_payload(bytes)?;
        hello.check_limits()?;
        Ok(hello)
    }

    /// Checks field limits.
    pub fn check_limits(&self) -> Result<(), FrameError> {
        if self.versions.is_empty() || self.versions.len() > limits::MAX_VERSIONS {
            return Err(FrameError::Invalid("versions"));
        }
        if self.direct_addrs.len() > limits::MAX_DIRECT_ADDRS {
            return Err(FrameError::Invalid("too many addresses"));
        }
        if self
            .relay_url
            .as_ref()
            .is_some_and(|u| u.len() > limits::MAX_RELAY_URL_BYTES)
        {
            return Err(FrameError::Invalid("relay url too long"));
        }
        if self.device_name.chars().count() > limits::MAX_DEVICE_NAME_CHARS {
            return Err(FrameError::Invalid("device name too long"));
        }
        if self.app_version.len() > limits::MAX_APP_VERSION_BYTES {
            return Err(FrameError::Invalid("app version too long"));
        }
        Ok(())
    }
}

/// The few messages that pass through the rendezvous room, in order:
/// `Pake`, `Confirm`, `Hello` from each side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairingMessage {
    /// SPAKE2 message (33 bytes).
    Pake(Vec<u8>),
    /// Key-confirmation tag.
    Confirm([u8; 32]),
    /// The sealed [`Hello`].
    Hello(Vec<u8>),
}

impl PairingMessage {
    /// Encodes the message for the body of a rendezvous `post`.
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        let bytes = postcard::to_allocvec(self).map_err(|_| FrameError::Malformed)?;
        if bytes.len() > crate::rendezvous::MAX_BODY_BYTES {
            return Err(FrameError::TooLarge {
                len: bytes.len(),
                max: crate::rendezvous::MAX_BODY_BYTES,
            });
        }
        Ok(bytes)
    }

    /// Decodes the body of a rendezvous `message`.
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        if bytes.len() > crate::rendezvous::MAX_BODY_BYTES {
            return Err(FrameError::TooLarge {
                len: bytes.len(),
                max: crate::rendezvous::MAX_BODY_BYTES,
            });
        }
        crate::frame::decode_payload(bytes)
    }
}

/// Makes a device name safe to show: no control or bidirectional-override
/// characters, trimmed, bounded, never empty.
pub fn sanitize_device_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .take(limits::MAX_DEVICE_NAME_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "Unnamed device".to_string()
    } else {
        cleaned.to_string()
    }
}

/// Unicode bidirectional-text control characters, which can make a name read
/// differently from what it is.
pub fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Hello {
        Hello {
            versions: vec![1],
            endpoint_id: [5; 32],
            relay_url: Some("https://relay.example.com/".into()),
            direct_addrs: vec![
                "192.168.1.20:4433".parse().unwrap(),
                "[2001:db8::1]:4433".parse().unwrap(),
            ],
            device_name: "Michael's Laptop".into(),
            device_type: DeviceType::Desktop,
            app_version: "0.1.0".into(),
            capabilities: 0,
        }
    }

    #[test]
    fn hello_round_trips() {
        let h = hello();
        assert_eq!(Hello::decode(&h.encode().unwrap()).unwrap(), h);
    }

    #[test]
    fn full_hello_fits_in_a_rendezvous_message() {
        let mut h = hello();
        h.direct_addrs = vec![
            "[2001:db8:ffff:ffff:ffff:ffff:ffff:ffff]:65535"
                .parse()
                .unwrap();
            limits::MAX_DIRECT_ADDRS
        ];
        h.device_name = "é".repeat(limits::MAX_DEVICE_NAME_CHARS);
        h.relay_url = Some("x".repeat(limits::MAX_RELAY_URL_BYTES));
        h.versions = vec![u16::MAX; limits::MAX_VERSIONS];
        let sealed_len = h.encode().unwrap().len() + 16;
        let message = PairingMessage::Hello(vec![0; sealed_len]).encode().unwrap();
        assert!(
            message.len() <= crate::rendezvous::MAX_BODY_BYTES,
            "{}",
            message.len()
        );
    }

    #[test]
    fn hello_limits_are_enforced() {
        let mut h = hello();
        h.versions.clear();
        assert!(h.encode().is_err());
        let mut h = hello();
        h.direct_addrs = vec!["1.2.3.4:5".parse().unwrap(); limits::MAX_DIRECT_ADDRS + 1];
        assert!(h.encode().is_err());
        let mut h = hello();
        h.device_name = "x".repeat(limits::MAX_DEVICE_NAME_CHARS + 1);
        assert!(h.encode().is_err());
        assert!(Hello::decode(&postcard::to_allocvec(&h).unwrap()).is_err());
    }

    #[test]
    fn device_names_are_sanitized() {
        assert_eq!(
            sanitize_device_name("  Laptop\u{202E}gnp.exe\n "),
            "Laptopgnp.exe"
        );
        assert_eq!(sanitize_device_name("\u{0}\u{7}"), "Unnamed device");
        assert_eq!(
            sanitize_device_name(&"x".repeat(500)).chars().count(),
            limits::MAX_DEVICE_NAME_CHARS
        );
    }

    #[test]
    fn pairing_messages_round_trip() {
        for m in [
            PairingMessage::Pake(vec![1; 33]),
            PairingMessage::Confirm([2; 32]),
            PairingMessage::Hello(vec![3; 300]),
        ] {
            assert_eq!(PairingMessage::decode(&m.encode().unwrap()).unwrap(), m);
        }
        assert!(PairingMessage::decode(&[9, 9, 9]).is_err());
    }
}
