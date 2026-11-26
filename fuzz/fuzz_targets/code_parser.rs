#![no_main]
//! Typed connection codes and pairing links.

use libfuzzer_sys::fuzz_target;
use tunnel_crypto::{ConnectionCode, PairUri};

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(code) = text.parse::<ConnectionCode>() {
            // Whatever parses must print in canonical form and parse back.
            assert_eq!(code.to_string().parse::<ConnectionCode>().unwrap(), code);
        }
        let _ = PairUri::parse(text);
    }
});
