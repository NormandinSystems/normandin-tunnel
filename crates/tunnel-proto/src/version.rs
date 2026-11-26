//! Protocol versions and negotiation.
//!
//! The protocol has its own number, separate from the app version. Each device
//! lists the versions it speaks in its hello; both use the highest one they
//! share. No shared version means "The other device needs an update".

/// The protocol version this build speaks natively.
pub const PROTOCOL_VERSION: u16 = 1;

/// Every protocol version this build can speak, newest first.
pub const SUPPORTED_VERSIONS: &[u16] = &[PROTOCOL_VERSION];

/// ALPN of the control connection for one-off sessions.
pub const ALPN_CONTROL: &[u8] = b"tunnel/control/1";

/// ALPN reserved for reconnecting trusted devices.
pub const ALPN_TRUSTED: &[u8] = b"tunnel/trusted/1";

/// Picks the highest version both sides support.
pub fn negotiate(ours: &[u16], theirs: &[u16]) -> Option<u16> {
    ours.iter().copied().filter(|v| theirs.contains(v)).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_highest_shared_version() {
        assert_eq!(negotiate(&[1], &[1]), Some(1));
        assert_eq!(negotiate(&[3, 2, 1], &[1, 2]), Some(2));
        assert_eq!(negotiate(&[2, 1], &[4, 3]), None);
        assert_eq!(negotiate(&[1], &[]), None);
    }
}
