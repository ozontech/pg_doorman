#![no_main]

use libfuzzer_sys::fuzz_target;
use bytes::BytesMut;

fuzz_target!(|data: &[u8]| {
    // Test parse_startup with arbitrary input
    let bytes = BytesMut::from(data);
    let _ = pg_doorman::messages::parse_startup(bytes);
});
