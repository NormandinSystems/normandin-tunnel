#![no_main]
//! JSON a client sends to the rendezvous server.

use libfuzzer_sys::fuzz_target;
use tunnel_proto::rendezvous;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = rendezvous::parse_client(text);
        let _ = rendezvous::is_valid_body(text);
    }
});
