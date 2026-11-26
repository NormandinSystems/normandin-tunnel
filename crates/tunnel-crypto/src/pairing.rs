//! SPAKE2 pairing, the key schedule and everything derived from it.
//!
//! The flow, with the type that owns each stage:
//!
//! 1. [`PakeState::start`] — begin SPAKE2 with the connection code; send the
//!    returned message to the other device through the rendezvous server.
//! 2. [`PakeState::finish`] — combine with the other device's message to get the
//!    shared key `K`, the transcript hash `T` and the seven derived keys.
//! 3. [`Handshake`] — prove both sides hold the same key
//!    ([`Handshake::confirmation`] / [`Handshake::verify_confirmation`]) and then
//!    exchange the sealed hello ([`Handshake::seal`] / [`Handshake::open`]).
//! 4. [`Handshake::into_session`] — wipe `K`, the confirm keys and the mailbox
//!    keys; keep only what the live session needs ([`SessionSecrets`]): the
//!    channel-binding key, the safety-code key and the trust key.
//!
//! Secrets are wiped from memory when dropped and are never printed.

use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

use crate::code::ConnectionCode;

type HmacSha256 = Hmac<Sha256>;
type Key = Zeroizing<[u8; 32]>;

/// SPAKE2 identity of the device that showed the code.
pub const IDENTITY_A: &[u8] = b"tunnel-v1-A";
/// SPAKE2 identity of the device that entered the code.
pub const IDENTITY_B: &[u8] = b"tunnel-v1-B";
/// Domain separator of the transcript hash.
pub const TRANSCRIPT_LABEL: &[u8] = b"tunnel-v1-transcript";
/// Prefix of every HKDF label.
pub const LABEL_PREFIX: &str = "tunnel v1 ";
/// Length of a SPAKE2 message on the Ed25519 group (side byte + point).
pub const PAKE_MESSAGE_LEN: usize = 33;
/// Length of an HMAC-SHA256 tag.
pub const MAC_LEN: usize = 32;
/// Length of an endpoint (device session) public key.
pub const ENDPOINT_ID_LEN: usize = 32;
/// Most hello messages a side may seal or open; the protocol needs one.
pub const MAX_MAILBOX_MESSAGES: u64 = 4;

/// Which side of the pairing a device is on. Roles matter only during pairing;
/// the session afterwards is symmetric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// The device that chose "Show a code".
    A,
    /// The device that chose "Enter a code".
    B,
}

impl Role {
    /// The other side.
    pub fn peer(self) -> Role {
        match self {
            Role::A => Role::B,
            Role::B => Role::A,
        }
    }

    /// The byte that stands for this role inside MAC inputs.
    pub fn byte(self) -> u8 {
        match self {
            Role::A => b'A',
            Role::B => b'B',
        }
    }
}

/// Why pairing failed. Deliberately vague: every variant is shown to the person
/// as "That code didn't work".
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PairingError {
    /// The other device's SPAKE2 message was malformed.
    #[error("the pairing message is malformed")]
    BadMessage,
    /// The other device does not hold the same key (wrong code, or an attacker).
    #[error("the other device does not have the same code")]
    ConfirmationFailed,
    /// A sealed message was used before key confirmation succeeded.
    #[error("keys are not confirmed yet")]
    NotConfirmed,
    /// A sealed message failed authentication.
    #[error("a pairing message failed authentication")]
    BadSeal,
    /// More sealed messages than the protocol allows.
    #[error("too many pairing messages")]
    TooManyMessages,
}

/// An in-progress SPAKE2 exchange.
pub struct PakeState {
    spake: Spake2<Ed25519Group>,
    role: Role,
    nameplate: u16,
    own_message: Vec<u8>,
}

impl fmt::Debug for PakeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PakeState")
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

impl PakeState {
    /// Starts SPAKE2 for `role` with the code's password.
    ///
    /// Returns the state and the message to post to the rendezvous room.
    pub fn start(role: Role, code: &ConnectionCode) -> (Self, Vec<u8>) {
        let password = code.password();
        let password = Password::new(password.as_slice());
        let id_a = Identity::new(IDENTITY_A);
        let id_b = Identity::new(IDENTITY_B);
        let (spake, message) = match role {
            Role::A => Spake2::<Ed25519Group>::start_a(&password, &id_a, &id_b),
            Role::B => Spake2::<Ed25519Group>::start_b(&password, &id_a, &id_b),
        };
        let state = Self {
            spake,
            role,
            nameplate: code.nameplate(),
            own_message: message.clone(),
        };
        (state, message)
    }

    /// Finishes SPAKE2 with the other device's message and derives every key.
    ///
    /// Success here does not yet mean the codes matched: that is only known after
    /// [`Handshake::verify_confirmation`].
    pub fn finish(self, peer_message: &[u8]) -> Result<Handshake, PairingError> {
        if peer_message.len() != PAKE_MESSAGE_LEN {
            return Err(PairingError::BadMessage);
        }
        let Self {
            spake,
            role,
            nameplate,
            own_message,
        } = self;
        let shared = Zeroizing::new(
            spake
                .finish(peer_message)
                .map_err(|_| PairingError::BadMessage)?,
        );
        let (message_a, message_b) = match role {
            Role::A => (own_message.as_slice(), peer_message),
            Role::B => (peer_message, own_message.as_slice()),
        };
        let transcript = transcript_hash(nameplate, message_a, message_b);
        Ok(Handshake::derive(role, shared.as_slice(), transcript))
    }
}

/// `T = SHA-256(label ‖ nameplate ‖ len(A) ‖ A's message ‖ len(B) ‖ B's message)`.
///
/// The nameplate and both lengths are big-endian `u16`.
pub fn transcript_hash(nameplate: u16, message_a: &[u8], message_b: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(TRANSCRIPT_LABEL);
    hasher.update(nameplate.to_be_bytes());
    hasher.update((message_a.len() as u16).to_be_bytes());
    hasher.update(message_a);
    hasher.update((message_b.len() as u16).to_be_bytes());
    hasher.update(message_b);
    hasher.finalize().into()
}

/// The seven keys derived from `K`, in the order the protocol lists them.
struct KeySchedule {
    confirm_a: Key,
    confirm_b: Key,
    mailbox_a_to_b: Key,
    mailbox_b_to_a: Key,
    bind: Key,
    safety: Key,
    trust: Key,
}

impl KeySchedule {
    /// HKDF-SHA256 with input `K`, salt `T`, and one expand call per label.
    fn derive(shared: &[u8], transcript: &[u8; 32]) -> Self {
        let hkdf = Hkdf::<Sha256>::new(Some(transcript), shared);
        let expand = |label: &str| -> Key {
            let mut key = Zeroizing::new([0u8; 32]);
            let info = format!("{LABEL_PREFIX}{label}");
            hkdf.expand(info.as_bytes(), key.as_mut_slice())
                .expect("32 bytes is a valid HKDF-SHA256 output length");
            key
        };
        Self {
            confirm_a: expand("confirm A"),
            confirm_b: expand("confirm B"),
            mailbox_a_to_b: expand("mailbox A->B"),
            mailbox_b_to_a: expand("mailbox B->A"),
            bind: expand("bind"),
            safety: expand("safety"),
            trust: expand("trust"),
        }
    }
}

/// Keys and state for key confirmation and the sealed hello exchange.
pub struct Handshake {
    role: Role,
    transcript: [u8; 32],
    confirm_own: Key,
    confirm_peer: Key,
    mailbox_send: Key,
    mailbox_recv: Key,
    send_counter: u64,
    recv_counter: u64,
    confirmed: bool,
    session: SessionSecrets,
}

impl fmt::Debug for Handshake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handshake")
            .field("role", &self.role)
            .field("confirmed", &self.confirmed)
            .finish_non_exhaustive()
    }
}

impl Handshake {
    /// Derives the handshake from the SPAKE2 output. Exposed for test vectors;
    /// applications use [`PakeState::finish`].
    pub fn derive(role: Role, shared_key: &[u8], transcript: [u8; 32]) -> Self {
        let keys = KeySchedule::derive(shared_key, &transcript);
        let KeySchedule {
            confirm_a,
            confirm_b,
            mailbox_a_to_b,
            mailbox_b_to_a,
            bind,
            safety,
            trust,
        } = keys;
        let (confirm_own, confirm_peer, mailbox_send, mailbox_recv) = match role {
            Role::A => (confirm_a, confirm_b, mailbox_a_to_b, mailbox_b_to_a),
            Role::B => (confirm_b, confirm_a, mailbox_b_to_a, mailbox_a_to_b),
        };
        Self {
            role,
            transcript,
            confirm_own,
            confirm_peer,
            mailbox_send,
            mailbox_recv,
            send_counter: 0,
            recv_counter: 0,
            confirmed: false,
            session: SessionSecrets {
                role,
                transcript,
                bind,
                safety,
                trust,
            },
        }
    }

    /// This device's role.
    pub fn role(&self) -> Role {
        self.role
    }

    /// The transcript hash `T`.
    pub fn transcript(&self) -> &[u8; 32] {
        &self.transcript
    }

    /// This device's key-confirmation tag: `HMAC(own confirm key, "confirm" ‖ T)`.
    pub fn confirmation(&self) -> [u8; MAC_LEN] {
        confirmation_tag(&self.confirm_own, &self.transcript)
    }

    /// Checks the other device's key-confirmation tag in constant time.
    ///
    /// A failure means the codes differ (a typo, an expired code, or someone
    /// guessing). The caller must close the room: the code is dead.
    pub fn verify_confirmation(&mut self, tag: &[u8]) -> Result<(), PairingError> {
        let expected = confirmation_tag(&self.confirm_peer, &self.transcript);
        if tag.len() == MAC_LEN && bool::from(expected.as_slice().ct_eq(tag)) {
            self.confirmed = true;
            Ok(())
        } else {
            Err(PairingError::ConfirmationFailed)
        }
    }

    /// Seals a hello message for the other device (XChaCha20-Poly1305).
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, PairingError> {
        if !self.confirmed {
            return Err(PairingError::NotConfirmed);
        }
        if self.send_counter >= MAX_MAILBOX_MESSAGES {
            return Err(PairingError::TooManyMessages);
        }
        let aad = mailbox_aad(&self.transcript, self.role);
        let cipher = XChaCha20Poly1305::new(self.mailbox_send.as_slice().into());
        let nonce = mailbox_nonce(self.send_counter);
        let sealed = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| PairingError::BadSeal)?;
        self.send_counter += 1;
        Ok(sealed)
    }

    /// Opens a hello message sealed by the other device.
    pub fn open(&mut self, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, PairingError> {
        if !self.confirmed {
            return Err(PairingError::NotConfirmed);
        }
        if self.recv_counter >= MAX_MAILBOX_MESSAGES {
            return Err(PairingError::TooManyMessages);
        }
        let aad = mailbox_aad(&self.transcript, self.role.peer());
        let cipher = XChaCha20Poly1305::new(self.mailbox_recv.as_slice().into());
        let nonce = mailbox_nonce(self.recv_counter);
        let plain = cipher
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: sealed,
                    aad: &aad,
                },
            )
            .map_err(|_| PairingError::BadSeal)?;
        self.recv_counter += 1;
        Ok(Zeroizing::new(plain))
    }

    /// Ends the handshake: the confirm and mailbox keys are wiped and only the
    /// secrets the live session needs are returned.
    pub fn into_session(self) -> Result<SessionSecrets, PairingError> {
        if !self.confirmed {
            return Err(PairingError::NotConfirmed);
        }
        Ok(self.session)
    }
}

fn keyed_mac(key: &Key) -> HmacSha256 {
    <HmacSha256 as Mac>::new_from_slice(key.as_slice()).expect("HMAC accepts any key length")
}

fn confirmation_tag(key: &Key, transcript: &[u8; 32]) -> [u8; MAC_LEN] {
    let mut mac = keyed_mac(key);
    mac.update(b"confirm");
    mac.update(transcript);
    mac.finalize().into_bytes().into()
}

/// Associated data of a sealed hello: `T ‖ sender role byte`.
fn mailbox_aad(transcript: &[u8; 32], sender: Role) -> [u8; 33] {
    let mut aad = [0u8; 33];
    aad[..32].copy_from_slice(transcript);
    aad[32] = sender.byte();
    aad
}

/// 24-byte nonce: 16 zero bytes, then the per-direction counter (big-endian).
/// Each direction has its own key, so a counter can never repeat under a key.
fn mailbox_nonce(counter: u64) -> [u8; 24] {
    let mut nonce = [0u8; 24];
    nonce[16..].copy_from_slice(&counter.to_be_bytes());
    nonce
}

/// The secrets that outlive the handshake.
pub struct SessionSecrets {
    role: Role,
    transcript: [u8; 32],
    bind: Key,
    safety: Key,
    trust: Key,
}

impl fmt::Debug for SessionSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionSecrets")
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

impl SessionSecrets {
    /// This device's role during pairing.
    pub fn role(&self) -> Role {
        self.role
    }

    /// The transcript hash `T`.
    pub fn transcript(&self) -> &[u8; 32] {
        &self.transcript
    }

    /// Channel-binding tag sent by `sender` as the first frame of the control
    /// stream: `HMAC(bind key, "bind" ‖ sender role ‖ A's endpoint ID ‖ B's
    /// endpoint ID ‖ T)`.
    ///
    /// It proves the QUIC peer is the same party that knew the code.
    pub fn bind_tag(
        &self,
        sender: Role,
        endpoint_a: &[u8; ENDPOINT_ID_LEN],
        endpoint_b: &[u8; ENDPOINT_ID_LEN],
    ) -> [u8; MAC_LEN] {
        let mut mac = keyed_mac(&self.bind);
        mac.update(b"bind");
        mac.update(&[sender.byte()]);
        mac.update(endpoint_a);
        mac.update(endpoint_b);
        mac.update(&self.transcript);
        mac.finalize().into_bytes().into()
    }

    /// Checks the other device's channel-binding tag in constant time.
    pub fn verify_bind(
        &self,
        endpoint_a: &[u8; ENDPOINT_ID_LEN],
        endpoint_b: &[u8; ENDPOINT_ID_LEN],
        tag: &[u8],
    ) -> bool {
        let expected = self.bind_tag(self.role.peer(), endpoint_a, endpoint_b);
        tag.len() == MAC_LEN && bool::from(expected.as_slice().ct_eq(tag))
    }

    /// The 6-digit safety code both screens show.
    pub fn safety_code(
        &self,
        endpoint_a: &[u8; ENDPOINT_ID_LEN],
        endpoint_b: &[u8; ENDPOINT_ID_LEN],
    ) -> SafetyCode {
        let mut mac = keyed_mac(&self.safety);
        mac.update(b"safety");
        mac.update(endpoint_a);
        mac.update(endpoint_b);
        let tag = mac.finalize().into_bytes();
        let mut first = [0u8; 8];
        first.copy_from_slice(&tag[..8]);
        SafetyCode((u64::from_be_bytes(first) % 1_000_000) as u32)
    }

    /// The pair secret kept only if both people choose "Remember this device".
    pub fn trust_key(&self) -> Zeroizing<[u8; 32]> {
        self.trust.clone()
    }

    /// Wipes the channel-binding key early (the session has ended).
    pub fn wipe(&mut self) {
        self.bind.zeroize();
        self.safety.zeroize();
        self.trust.zeroize();
    }
}

/// The 6-digit number both people compare, shown as `418 293`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SafetyCode(u32);

impl SafetyCode {
    /// The six digits without a space.
    pub fn digits(&self) -> String {
        format!("{:06}", self.0)
    }
}

impl fmt::Display for SafetyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:03} {:03}", self.0 / 1000, self.0 % 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(text: &str) -> ConnectionCode {
        text.parse().unwrap()
    }

    fn pair(code_a: &str, code_b: &str) -> (Handshake, Handshake) {
        let (state_a, msg_a) = PakeState::start(Role::A, &code(code_a));
        let (state_b, msg_b) = PakeState::start(Role::B, &code(code_b));
        assert_eq!(msg_a.len(), PAKE_MESSAGE_LEN);
        assert_eq!(msg_b.len(), PAKE_MESSAGE_LEN);
        (
            state_a.finish(&msg_b).unwrap(),
            state_b.finish(&msg_a).unwrap(),
        )
    }

    const GOOD: &str = "42-aardvark-abandoned-abbreviate";

    #[test]
    fn same_code_confirms_and_exchanges_hellos() {
        let (mut a, mut b) = pair(GOOD, "42 AAR aba abb");
        assert_eq!(a.transcript(), b.transcript());
        let (tag_a, tag_b) = (a.confirmation(), b.confirmation());
        assert_ne!(tag_a, tag_b, "each side has its own confirm key");
        a.verify_confirmation(&tag_b).unwrap();
        b.verify_confirmation(&tag_a).unwrap();

        let sealed = a.seal(b"hello from A").unwrap();
        assert_eq!(b.open(&sealed).unwrap().as_slice(), b"hello from A");
        let sealed = b.seal(b"hello from B").unwrap();
        assert_eq!(a.open(&sealed).unwrap().as_slice(), b"hello from B");

        let (a, b) = (a.into_session().unwrap(), b.into_session().unwrap());
        let (id_a, id_b) = ([1u8; 32], [2u8; 32]);
        assert_eq!(a.safety_code(&id_a, &id_b), b.safety_code(&id_a, &id_b));
        assert!(b.verify_bind(&id_a, &id_b, &a.bind_tag(Role::A, &id_a, &id_b)));
        assert!(a.verify_bind(&id_a, &id_b, &b.bind_tag(Role::B, &id_a, &id_b)));
        assert_eq!(a.trust_key().as_slice(), b.trust_key().as_slice());
    }

    #[test]
    fn different_codes_fail_confirmation() {
        for wrong in [
            "42-aardvark-abandoned-abdomen",
            "43-aardvark-abandoned-abbreviate",
            "42-abandoned-aardvark-abbreviate",
        ] {
            let (mut a, mut b) = pair(GOOD, wrong);
            let (tag_a, tag_b) = (a.confirmation(), b.confirmation());
            assert_eq!(
                a.verify_confirmation(&tag_b),
                Err(PairingError::ConfirmationFailed)
            );
            assert_eq!(
                b.verify_confirmation(&tag_a),
                Err(PairingError::ConfirmationFailed)
            );
            assert_eq!(a.seal(b"x").unwrap_err(), PairingError::NotConfirmed);
            assert!(a.into_session().is_err());
        }
    }

    #[test]
    fn a_reflected_confirmation_is_rejected() {
        let (mut a, _b) = pair(GOOD, GOOD);
        let own = a.confirmation();
        assert_eq!(
            a.verify_confirmation(&own),
            Err(PairingError::ConfirmationFailed)
        );
        assert_eq!(
            a.verify_confirmation(&own[..31]),
            Err(PairingError::ConfirmationFailed)
        );
        assert_eq!(
            a.verify_confirmation(&[]),
            Err(PairingError::ConfirmationFailed)
        );
    }

    #[test]
    fn malformed_pake_messages_are_rejected() {
        let (state, own) = PakeState::start(Role::A, &code(GOOD));
        assert_eq!(state.finish(&[]).unwrap_err(), PairingError::BadMessage);
        let (state, _) = PakeState::start(Role::A, &code(GOOD));
        assert_eq!(
            state.finish(&[0u8; 64]).unwrap_err(),
            PairingError::BadMessage
        );
        // A message from the same side (a reflection) must not be accepted.
        let (state, _) = PakeState::start(Role::A, &code(GOOD));
        assert_eq!(state.finish(&own).unwrap_err(), PairingError::BadMessage);
    }

    #[test]
    fn sealed_messages_resist_tampering_replay_and_reflection() {
        let (mut a, mut b) = pair(GOOD, GOOD);
        let (tag_a, tag_b) = (a.confirmation(), b.confirmation());
        a.verify_confirmation(&tag_b).unwrap();
        b.verify_confirmation(&tag_a).unwrap();

        let sealed = a.seal(b"hello").unwrap();
        let mut tampered = sealed.clone();
        tampered[0] ^= 1;
        assert_eq!(b.open(&tampered).unwrap_err(), PairingError::BadSeal);
        // A's own message reflected back at A does not open.
        assert_eq!(a.open(&sealed).unwrap_err(), PairingError::BadSeal);
        assert_eq!(b.open(&sealed).unwrap().as_slice(), b"hello");
        // Replaying the same message fails: the counter has moved on.
        assert_eq!(b.open(&sealed).unwrap_err(), PairingError::BadSeal);
    }

    #[test]
    fn mailbox_is_limited() {
        let (mut a, mut b) = pair(GOOD, GOOD);
        let (tag_a, tag_b) = (a.confirmation(), b.confirmation());
        a.verify_confirmation(&tag_b).unwrap();
        b.verify_confirmation(&tag_a).unwrap();
        for _ in 0..MAX_MAILBOX_MESSAGES {
            a.seal(b"x").unwrap();
        }
        assert_eq!(a.seal(b"x").unwrap_err(), PairingError::TooManyMessages);
    }

    #[test]
    fn bind_tag_depends_on_role_and_endpoints() {
        let (mut a, mut b) = pair(GOOD, GOOD);
        let (tag_a, tag_b) = (a.confirmation(), b.confirmation());
        a.verify_confirmation(&tag_b).unwrap();
        b.verify_confirmation(&tag_a).unwrap();
        let (a, b) = (a.into_session().unwrap(), b.into_session().unwrap());
        let (id_a, id_b, id_c) = ([1u8; 32], [2u8; 32], [3u8; 32]);
        let from_a = a.bind_tag(Role::A, &id_a, &id_b);
        // A's own tag reflected back at A is refused.
        assert!(!a.verify_bind(&id_a, &id_b, &from_a));
        // A different endpoint (someone in the middle) is refused.
        assert!(!b.verify_bind(&id_c, &id_b, &from_a));
        assert!(!b.verify_bind(&id_a, &id_c, &from_a));
        assert!(!b.verify_bind(&id_a, &id_b, &from_a[..31]));
        assert!(b.verify_bind(&id_a, &id_b, &from_a));
    }

    #[test]
    fn safety_code_formats_as_two_groups() {
        assert_eq!(SafetyCode(418_293).to_string(), "418 293");
        assert_eq!(SafetyCode(7).to_string(), "000 007");
        assert_eq!(SafetyCode(7).digits(), "000007");
    }

    /// Fixed vectors, also published in docs/PROTOCOL.md. They were produced by
    /// an independent Python implementation (hashlib/hmac); see
    /// docs/PROTOCOL.md for the script.
    mod vectors {
        use super::*;

        const SHARED: [u8; 32] = [0x11; 32];
        const NAMEPLATE: u16 = 42;

        fn messages() -> (Vec<u8>, Vec<u8>) {
            let mut a = vec![b'A'];
            a.extend(0u8..32);
            let mut b = vec![b'B'];
            b.extend(32u8..64);
            (a, b)
        }

        fn ids() -> ([u8; 32], [u8; 32]) {
            ([0xA1; 32], [0xB2; 32])
        }

        fn confirmed(role: Role) -> Handshake {
            let (a, b) = messages();
            let transcript = transcript_hash(NAMEPLATE, &a, &b);
            let mut hs = Handshake::derive(role, &SHARED, transcript);
            let peer = Handshake::derive(role.peer(), &SHARED, transcript);
            hs.verify_confirmation(&peer.confirmation()).unwrap();
            hs
        }

        #[test]
        fn transcript() {
            let (a, b) = messages();
            assert_eq!(
                hex::encode(transcript_hash(NAMEPLATE, &a, &b)),
                include_str!("vectors/transcript.hex").trim()
            );
        }

        #[test]
        fn confirmations() {
            assert_eq!(
                hex::encode(confirmed(Role::A).confirmation()),
                include_str!("vectors/confirm_a.hex").trim()
            );
            assert_eq!(
                hex::encode(confirmed(Role::B).confirmation()),
                include_str!("vectors/confirm_b.hex").trim()
            );
        }

        #[test]
        fn bind_tags() {
            let (id_a, id_b) = ids();
            let session = confirmed(Role::A).into_session().unwrap();
            assert_eq!(
                hex::encode(session.bind_tag(Role::A, &id_a, &id_b)),
                include_str!("vectors/bind_a.hex").trim()
            );
            assert_eq!(
                hex::encode(session.bind_tag(Role::B, &id_a, &id_b)),
                include_str!("vectors/bind_b.hex").trim()
            );
        }

        #[test]
        fn safety_and_trust() {
            let (id_a, id_b) = ids();
            let session = confirmed(Role::B).into_session().unwrap();
            assert_eq!(
                session.safety_code(&id_a, &id_b).digits(),
                include_str!("vectors/safety.txt").trim()
            );
            assert_eq!(
                hex::encode(session.trust_key().as_slice()),
                include_str!("vectors/trust.hex").trim()
            );
        }

        #[test]
        fn sealed_hello() {
            let mut a = confirmed(Role::A);
            let sealed = a.seal(b"hello").unwrap();
            assert_eq!(
                hex::encode(&sealed),
                include_str!("vectors/sealed_hello_a.hex").trim()
            );
            let mut b = confirmed(Role::B);
            assert_eq!(b.open(&sealed).unwrap().as_slice(), b"hello");
        }
    }
}
