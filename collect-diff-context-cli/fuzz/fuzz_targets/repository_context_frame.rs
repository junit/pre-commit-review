#![no_main]

use collect_diff_context_cli::repository_context_provider::json_rpc::{
    parse_inbound, FrameDecoder, FrameLimits,
};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_CHUNK_BYTES: usize = 31;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }

    let limits = FrameLimits {
        max_header_bytes: 4096,
        max_frame_bytes: 16 * 1024,
        max_protocol_bytes: 32 * 1024,
        max_messages: 64,
    };
    let max_buffer_bytes = limits
        .max_header_bytes
        .checked_add(limits.max_frame_bytes)
        .expect("static frame limits fit usize");
    let mut decoder = FrameDecoder::new(limits).expect("static frame limits are valid");

    let mut offset = 0;
    while offset < data.len() {
        let step = usize::from(data[offset] % MAX_CHUNK_BYTES as u8).saturating_add(1);
        let end = offset.saturating_add(step).min(data.len());
        assert!(end > offset);
        let result = decoder.push(&data[offset..end]);
        assert!(decoder.buffered_bytes() <= max_buffer_bytes);
        let bodies = match result {
            Ok(bodies) => bodies,
            Err(_) => break,
        };
        for body in bodies {
            let _ = parse_inbound(&body);
        }
        offset = end;
    }
    assert!(decoder.buffered_bytes() <= max_buffer_bytes);
    let _ = decoder.finish();
});
