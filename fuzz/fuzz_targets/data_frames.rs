#![no_main]
//! The request and header frames of a file stream.

use libfuzzer_sys::fuzz_target;
use tunnel_proto::data::{DataHeader, Get};
use tunnel_proto::{frame, limits};

fuzz_target!(|data: &[u8]| {
    let _ = frame::decode::<Get>(data, limits::MAX_DATA_FRAME);
    let _ = frame::decode::<DataHeader>(data, limits::MAX_DATA_FRAME);
    let _ = frame::decode_payload::<Get>(data);
    let _ = frame::decode_payload::<DataHeader>(data);
});
