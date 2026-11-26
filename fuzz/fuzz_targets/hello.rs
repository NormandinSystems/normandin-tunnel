#![no_main]
//! The hello and the pairing-message envelope (bytes a rendezvous server relays).

use libfuzzer_sys::fuzz_target;
use tunnel_proto::hello::{Hello, PairingMessage, sanitize_device_name};

fuzz_target!(|data: &[u8]| {
    if let Ok(hello) = Hello::decode(data) {
        let name = sanitize_device_name(&hello.device_name);
        assert!(!name.is_empty());
        assert!(!name.chars().any(char::is_control));
    }
    let _ = PairingMessage::decode(data);
});
