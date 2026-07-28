#![no_main]

use collect_diff_context_cli::repository_context_provider::json_rpc::{
    parse_inbound, CorrelationState, InboundMessage, MessageLimits, ProtocolError,
};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 1024 * 1024;

fn message_counted<T>(result: &Result<T, ProtocolError>) -> bool {
    result
        .as_ref()
        .err()
        .is_none_or(|error| error.code != "provider-message-limit")
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }

    let limits = MessageLimits {
        max_requests: 4,
        max_pending_requests: 4,
        max_messages: 64,
        max_notifications: 16,
        max_server_requests: 16,
        max_invalid_messages: 8,
    };
    let mut state = CorrelationState::new(limits).expect("static message limits are valid");
    for method in ["fuzz/one", "fuzz/two", "fuzz/three", "fuzz/four"] {
        state
            .reserve_request(method)
            .expect("four static requests fit the limits");
    }

    let mut messages = 0;
    let mut notifications = 0;
    let mut server_requests = 0;
    let mut invalid = 0;
    for line in data
        .split(|byte| *byte == b'\n')
        .take(limits.max_messages.saturating_add(1))
    {
        match parse_inbound(line) {
            Ok(InboundMessage::Response(response)) => {
                let result = state.accept_client_response(response);
                if message_counted(&result) {
                    messages += 1;
                }
                if result
                    .as_ref()
                    .err()
                    .is_some_and(|error| error.code == "provider-response-id-invalid")
                {
                    invalid += 1;
                }
            }
            Ok(InboundMessage::Request(_)) => {
                let result = state.observe_server_request();
                if message_counted(&result) {
                    messages += 1;
                }
                if result.is_ok() {
                    server_requests += 1;
                }
            }
            Ok(InboundMessage::Notification(_)) => {
                let result = state.observe_notification();
                if message_counted(&result) {
                    messages += 1;
                }
                if result.is_ok() {
                    notifications += 1;
                }
            }
            Err(_) => {
                let result = state.observe_invalid();
                if message_counted(&result) {
                    messages += 1;
                }
                if result.is_ok() {
                    invalid += 1;
                }
            }
        }

        assert!(messages <= limits.max_messages);
        assert!(notifications <= limits.max_notifications);
        assert!(server_requests <= limits.max_server_requests);
        assert!(invalid <= limits.max_invalid_messages);
        assert!(state.pending_len() <= limits.max_pending_requests);
    }
});
