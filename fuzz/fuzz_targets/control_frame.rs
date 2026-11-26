#![no_main]
//! Control-stream frames, including manifest pages: decoders must never panic
//! and whatever decodes must encode back to something that decodes the same.

use libfuzzer_sys::fuzz_target;
use tunnel_proto::control::ControlMessage;
use tunnel_proto::{frame, limits};

fuzz_target!(|data: &[u8]| {
    let _ = frame::decode::<ControlMessage>(data, limits::MAX_CONTROL_FRAME);
    if let Ok(message) = ControlMessage::from_payload(data) {
        let encoded = message.to_frame().expect("a decoded message encodes");
        let (again, used) = frame::decode::<ControlMessage>(&encoded, limits::MAX_CONTROL_FRAME)
            .expect("re-decodes")
            .expect("complete frame");
        assert_eq!(used, encoded.len());
        assert_eq!(again, message);
    }
});
