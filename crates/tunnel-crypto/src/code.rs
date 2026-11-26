//! Connection codes: `42-orbit-velvet-candle`.
//!
//! A code is `<nameplate>-<word>-<word>-<word>`. The nameplate (1 to 9999) is
//! public and only finds the place where the two devices meet: a room on a
//! rendezvous server, or, with no server, a meeting point on the relay network
//! whose address is derived from the nameplate (see [`meeting_key`]). The three
//! words are the secret: about 31 bits drawn with the operating system's secure
//! random generator. With one guess allowed per code, an attacker's chance is
//! about 1 in 2.2 billion.

use std::fmt;
use std::str::FromStr;

use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::wordlist::{self, WORD_COUNT};

/// Smallest nameplate a rendezvous server may hand out.
pub const NAMEPLATE_MIN: u16 = 1;
/// Largest nameplate a rendezvous server may hand out.
pub const NAMEPLATE_MAX: u16 = 9999;
/// Smallest nameplate a device picks for itself when no rendezvous server
/// assigns one. Self-chosen nameplates always have four digits.
pub const SELF_NAMEPLATE_MIN: u16 = 1000;
/// Domain separation for the meeting-point key.
pub const MEETING_SALT: &[u8] = b"normandin-tunnel/v1/meeting-point";
/// Number of secret words in a code.
pub const WORDS_PER_CODE: usize = 3;
/// Domain-separation prefix of the SPAKE2 password.
pub const PASSWORD_PREFIX: &str = "normandin-tunnel/v1";
/// URI scheme used in QR codes.
pub const URI_SCHEME: &str = "tunnel";

/// Why a typed code could not be understood.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodeError {
    /// Nothing was typed.
    #[error("the code is empty")]
    Empty,
    /// The code does not start with a number from 1 to 9999.
    #[error("the code must start with a number from 1 to 9999")]
    BadNameplate,
    /// One of the words is not in the word list.
    #[error("word {position} is not a code word")]
    UnknownWord {
        /// 1-based position of the bad word.
        position: usize,
    },
    /// The code does not have exactly three words.
    #[error("a code has 3 words, found {found}")]
    WrongWordCount {
        /// Number of words that were found.
        found: usize,
    },
    /// A `tunnel://` link could not be read.
    #[error("that link is not a Tunnel pairing link")]
    BadUri,
    /// The operating system could not provide random numbers.
    #[error("secure random numbers are unavailable")]
    Random,
}

/// A one-time connection code.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionCode {
    nameplate: u16,
    words: [u16; WORDS_PER_CODE],
}

impl ConnectionCode {
    /// Picks three random words for the nameplate the rendezvous server assigned.
    pub fn generate(nameplate: u16) -> Result<Self, CodeError> {
        check_nameplate(nameplate)?;
        let mut words = [0u16; WORDS_PER_CODE];
        for word in &mut words {
            *word = random_word_index()?;
        }
        Ok(Self { nameplate, words })
    }

    /// Picks a random four-digit nameplate and three random words, for
    /// pairing without a rendezvous server.
    pub fn generate_unassigned() -> Result<Self, CodeError> {
        let span = u32::from(NAMEPLATE_MAX - SELF_NAMEPLATE_MIN) + 1;
        let nameplate = SELF_NAMEPLATE_MIN + random_below(span)?;
        Self::generate(nameplate)
    }

    /// Builds a code from a nameplate and word-list indexes.
    pub fn from_parts(nameplate: u16, words: [u16; WORDS_PER_CODE]) -> Result<Self, CodeError> {
        check_nameplate(nameplate)?;
        for (i, word) in words.iter().enumerate() {
            if usize::from(*word) >= WORD_COUNT {
                return Err(CodeError::UnknownWord { position: i + 1 });
            }
        }
        Ok(Self { nameplate, words })
    }

    /// The public nameplate that finds the meeting room.
    pub fn nameplate(&self) -> u16 {
        self.nameplate
    }

    /// The three secret words.
    pub fn words(&self) -> [&'static str; WORDS_PER_CODE] {
        self.words.map(|index| wordlist::WORDS[usize::from(index)])
    }

    /// The SPAKE2 password: `normandin-tunnel/v1|<nameplate>|<w1>-<w2>-<w3>`.
    ///
    /// Both devices must derive byte-identical passwords, so the canonical
    /// spelling (lower case, single hyphens, decimal nameplate without leading
    /// zeros) is used no matter how the code was typed.
    pub fn password(&self) -> Zeroizing<Vec<u8>> {
        let words = self.words();
        Zeroizing::new(
            format!(
                "{PASSWORD_PREFIX}|{}|{}-{}-{}",
                self.nameplate, words[0], words[1], words[2]
            )
            .into_bytes(),
        )
    }
}

impl fmt::Display for ConnectionCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let words = self.words();
        write!(
            f,
            "{}-{}-{}-{}",
            self.nameplate, words[0], words[1], words[2]
        )
    }
}

/// Codes are secrets: debug output shows the nameplate only.
impl fmt::Debug for ConnectionCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ConnectionCode({}-***)", self.nameplate)
    }
}

impl FromStr for ConnectionCode {
    type Err = CodeError;

    /// Forgiving parser: any letter case, spaces or hyphens between parts, words
    /// may be shortened to their first three or more letters.
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let tokens: Vec<&str> = input
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();
        let Some((first, rest)) = tokens.split_first() else {
            return Err(CodeError::Empty);
        };
        if first.len() > 4 || !first.bytes().all(|b| b.is_ascii_digit()) {
            return Err(CodeError::BadNameplate);
        }
        let nameplate: u16 = first.parse().map_err(|_| CodeError::BadNameplate)?;
        check_nameplate(nameplate)?;

        let mut words = Vec::with_capacity(WORDS_PER_CODE);
        let mut i = 0;
        while i < rest.len() {
            // A hyphenated word ("yo-yo") arrives as two tokens: try the pair first.
            let joined = rest.get(i + 1).map(|next| format!("{}{}", rest[i], next));
            if let Some(index) = joined.as_deref().and_then(exact_word) {
                words.push(index);
                i += 2;
                continue;
            }
            match wordlist::lookup(rest[i]) {
                Some(index) => words.push(index),
                None => {
                    return Err(CodeError::UnknownWord {
                        position: words.len() + 1,
                    });
                }
            }
            i += 1;
        }
        let words: [u16; WORDS_PER_CODE] = words
            .try_into()
            .map_err(|w: Vec<u16>| CodeError::WrongWordCount { found: w.len() })?;
        Ok(Self { nameplate, words })
    }
}

fn exact_word(token: &str) -> Option<u16> {
    let token = wordlist::normalize(token);
    wordlist::WORDS
        .iter()
        .position(|word| word.contains('-') && wordlist::normalize(word) == token)
        .map(|index| index as u16)
}

fn check_nameplate(nameplate: u16) -> Result<(), CodeError> {
    if (NAMEPLATE_MIN..=NAMEPLATE_MAX).contains(&nameplate) {
        Ok(())
    } else {
        Err(CodeError::BadNameplate)
    }
}

/// Uniform index into the word list.
fn random_word_index() -> Result<u16, CodeError> {
    random_below(WORD_COUNT as u32)
}

/// Uniform number below `n` (at most 65,536), by rejection sampling on 16
/// random bits.
fn random_below(n: u32) -> Result<u16, CodeError> {
    let limit = (65_536 / n) * n;
    loop {
        let mut buf = [0u8; 2];
        crate::random_bytes(&mut buf).map_err(|_| CodeError::Random)?;
        let value = u32::from(u16::from_be_bytes(buf));
        if value < limit {
            return Ok((value % n) as u16);
        }
    }
}

/// The secret key of the meeting point for a nameplate, used when two devices
/// pair without a rendezvous server.
///
/// `HKDF-SHA256(salt = "normandin-tunnel/v1/meeting-point", ikm = nameplate in
/// decimal, info = "ed25519 secret key")`, 32 bytes.
///
/// This key is **not a secret**: it is computed from the public nameplate
/// alone, never from the words, so that the device typing the code can find
/// the device showing it. Anyone can derive it, exactly as anyone can reach a
/// rendezvous server. Whoever answers at the meeting point is therefore
/// untrusted, and proves nothing until SPAKE2 and key confirmation succeed.
pub fn meeting_key(nameplate: u16) -> [u8; 32] {
    let hkdf = Hkdf::<Sha256>::new(Some(MEETING_SALT), nameplate.to_string().as_bytes());
    let mut key = [0u8; 32];
    hkdf.expand(b"ed25519 secret key", &mut key)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    key
}

/// The QR-code form of a code: `tunnel://pair?c=<code>&s=<rendezvous host>`.
#[derive(Clone, PartialEq, Eq)]
pub struct PairUri {
    /// The connection code.
    pub code: ConnectionCode,
    /// Rendezvous host the code was created on, if the link names one.
    pub rendezvous_host: Option<String>,
}

impl fmt::Debug for PairUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairUri")
            .field("code", &self.code)
            .field("rendezvous_host", &self.rendezvous_host)
            .finish()
    }
}

impl PairUri {
    /// Reads either a pairing link or a plain typed code.
    pub fn parse(input: &str) -> Result<Self, CodeError> {
        let input = input.trim();
        let prefix = format!("{URI_SCHEME}://");
        let is_link = input
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(&prefix));
        if !is_link {
            return Ok(Self {
                code: input.parse()?,
                rendezvous_host: None,
            });
        }
        let rest = &input[prefix.len()..];
        let (action, query) = rest.split_once('?').ok_or(CodeError::BadUri)?;
        if !action.trim_end_matches('/').eq_ignore_ascii_case("pair") {
            return Err(CodeError::BadUri);
        }
        let mut code = None;
        let mut host = None;
        for pair in query.split('&') {
            match pair.split_once('=') {
                Some(("c", value)) => code = Some(value.parse::<ConnectionCode>()?),
                Some(("s", value)) if is_valid_host(value) => {
                    host = Some(value.to_ascii_lowercase())
                }
                Some(("s", _)) => return Err(CodeError::BadUri),
                _ => {}
            }
        }
        Ok(Self {
            code: code.ok_or(CodeError::BadUri)?,
            rendezvous_host: host,
        })
    }
}

impl fmt::Display for PairUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{URI_SCHEME}://pair?c={}", self.code)?;
        if let Some(host) = &self.rendezvous_host {
            write!(f, "&s={host}")?;
        }
        Ok(())
    }
}

/// Accepts `host`, `host:port` and bracketed IPv6; nothing that could smuggle a
/// path, credentials or a different scheme into the rendezvous address.
pub fn is_valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample() -> ConnectionCode {
        ConnectionCode::from_parts(42, [0, 1, 2]).unwrap()
    }

    #[test]
    fn self_chosen_nameplates_have_four_digits() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let code = ConnectionCode::generate_unassigned().unwrap();
            assert!((SELF_NAMEPLATE_MIN..=NAMEPLATE_MAX).contains(&code.nameplate()));
            seen.insert(code.nameplate());
        }
        // 200 draws from 9,000 values: a stuck generator would repeat.
        assert!(seen.len() > 150);
    }

    #[test]
    fn meeting_key_depends_only_on_the_nameplate() {
        // Fixed vector, cross-checked by docs/test_vectors.py.
        assert_eq!(
            hex::encode(meeting_key(42)),
            include_str!("vectors/meeting_key_42.hex").trim()
        );
        assert_ne!(meeting_key(42), meeting_key(43));
        assert_ne!(meeting_key(4), meeting_key(42));
        let a = ConnectionCode::from_parts(4821, [0, 1, 2]).unwrap();
        let b = ConnectionCode::from_parts(4821, [9, 8, 7]).unwrap();
        assert_eq!(meeting_key(a.nameplate()), meeting_key(b.nameplate()));
    }

    #[test]
    fn displays_in_canonical_form() {
        assert_eq!(sample().to_string(), "42-aardvark-abandoned-abbreviate");
        assert_eq!(
            sample().password().as_slice(),
            b"normandin-tunnel/v1|42|aardvark-abandoned-abbreviate"
        );
    }

    #[test]
    fn debug_never_shows_the_words() {
        let debug = format!("{:?}", sample());
        assert!(!debug.contains("aardvark"));
        assert!(debug.contains("42"));
    }

    #[test]
    fn parsing_is_forgiving() {
        let expected = sample();
        for typed in [
            "42-aardvark-abandoned-abbreviate",
            "42 aardvark abandoned abbreviate",
            "  42 - AARDVARK - Abandoned -  abbreviate ",
            "42-aar-aba-abb",
            "0042-aardvark-abandoned-abbreviate",
            "42_aardvark.abandoned,abbreviate",
        ] {
            assert_eq!(
                typed.parse::<ConnectionCode>().as_ref(),
                Ok(&expected),
                "{typed}"
            );
        }
    }

    #[test]
    fn hyphenated_word_round_trips() {
        let yoyo = wordlist::lookup("yoyo").unwrap();
        let code = ConnectionCode::from_parts(7, [yoyo, 0, yoyo]).unwrap();
        let text = code.to_string();
        assert_eq!(text, "7-yo-yo-aardvark-yo-yo");
        assert_eq!(text.parse::<ConnectionCode>().unwrap(), code);
        assert_eq!(
            "7 yoyo aardvark yo yo".parse::<ConnectionCode>().unwrap(),
            code
        );
    }

    #[test]
    fn rejects_bad_codes() {
        let parse = |s: &str| s.parse::<ConnectionCode>();
        assert_eq!(parse(""), Err(CodeError::Empty));
        assert_eq!(parse(" - "), Err(CodeError::Empty));
        assert_eq!(
            parse("aardvark-abandoned-abbreviate"),
            Err(CodeError::BadNameplate)
        );
        assert_eq!(
            parse("0-aardvark-abandoned-abbreviate"),
            Err(CodeError::BadNameplate)
        );
        assert_eq!(
            parse("10000-aardvark-abandoned-abbreviate"),
            Err(CodeError::BadNameplate)
        );
        assert_eq!(
            parse("42-aardvark-abandoned"),
            Err(CodeError::WrongWordCount { found: 2 })
        );
        assert_eq!(
            parse("42-aardvark-abandoned-abbreviate-abdomen"),
            Err(CodeError::WrongWordCount { found: 4 })
        );
        assert_eq!(
            parse("42-aardvark-xxxxxx-abbreviate"),
            Err(CodeError::UnknownWord { position: 2 })
        );
        assert!(ConnectionCode::from_parts(0, [0, 0, 0]).is_err());
        assert!(ConnectionCode::from_parts(1, [0, 0, 1296]).is_err());
    }

    #[test]
    fn generated_codes_are_valid_and_vary() {
        let a = ConnectionCode::generate(1).unwrap();
        let b = ConnectionCode::generate(1).unwrap();
        let c = ConnectionCode::generate(1).unwrap();
        assert!(a != b || b != c, "three identical codes in a row");
        assert_eq!(a.to_string().parse::<ConnectionCode>().unwrap(), a);
        assert!(ConnectionCode::generate(0).is_err());
    }

    #[test]
    fn pairing_links_round_trip() {
        let uri = PairUri {
            code: sample(),
            rendezvous_host: Some("rendezvous.example.com".into()),
        };
        let text = uri.to_string();
        assert_eq!(
            text,
            "tunnel://pair?c=42-aardvark-abandoned-abbreviate&s=rendezvous.example.com"
        );
        assert_eq!(PairUri::parse(&text).unwrap(), uri);
        let plain = PairUri::parse("42 aar aba abb").unwrap();
        assert_eq!(plain.code, sample());
        assert_eq!(plain.rendezvous_host, None);
    }

    #[test]
    fn pairing_links_reject_tricks() {
        for bad in [
            "tunnel://pair",
            "tunnel://open?c=42-aardvark-abandoned-abbreviate",
            "tunnel://pair?s=example.com",
            "tunnel://pair?c=42-aardvark-abandoned-abbreviate&s=evil.com/path",
            "tunnel://pair?c=42-aardvark-abandoned-abbreviate&s=user@evil.com",
            "tunnel://pair?c=42-aardvark-abandoned-abbreviate&s=",
        ] {
            assert!(PairUri::parse(bad).is_err(), "{bad}");
        }
    }

    proptest! {
        #[test]
        fn display_then_parse_is_identity(
            nameplate in NAMEPLATE_MIN..=NAMEPLATE_MAX,
            w in proptest::array::uniform3(0u16..1296),
        ) {
            let code = ConnectionCode::from_parts(nameplate, w).unwrap();
            prop_assert_eq!(code.to_string().parse::<ConnectionCode>().unwrap(), code.clone());
            prop_assert_eq!(code.to_string().to_uppercase().replace('-', " ").parse::<ConnectionCode>().unwrap(), code);
        }

        #[test]
        fn parser_never_panics(input in "\\PC{0,64}") {
            let _ = input.parse::<ConnectionCode>();
            let _ = PairUri::parse(&input);
        }
    }
}
