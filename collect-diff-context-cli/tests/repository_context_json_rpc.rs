use collect_diff_context_cli::repository_context_provider::json_rpc::{
    encode_error, encode_notification, encode_request, encode_result, frame_json, parse_inbound,
    ClientResponse, CorrelationState, FrameDecoder, FrameLimits, InboundMessage, MessageLimits,
    ResponseOutcome, RpcErrorObject, ServerRequestId,
};
use serde_json::json;

fn frame_limits() -> FrameLimits {
    FrameLimits {
        max_header_bytes: 128,
        max_frame_bytes: 1024,
        max_protocol_bytes: 4096,
        max_messages: 16,
    }
}

fn message_limits() -> MessageLimits {
    MessageLimits {
        max_requests: 4,
        max_pending_requests: 2,
        max_messages: 8,
        max_notifications: 4,
        max_server_requests: 4,
        max_invalid_messages: 2,
    }
}

#[test]
fn frame_decoder_handles_every_split_point_multiple_frames_and_zero_body() {
    let first = frame_json(json!({"jsonrpc":"2.0","method":"one"})).unwrap();
    let second = frame_json(json!({"jsonrpc":"2.0","method":"two"})).unwrap();
    for split in 0..=first.len() {
        let mut decoder = FrameDecoder::new(frame_limits()).unwrap();
        let mut bodies = decoder.push(&first[..split]).unwrap();
        bodies.extend(decoder.push(&first[split..]).unwrap());
        assert_eq!(bodies.len(), 1, "split {split}");
        assert_eq!(bodies[0], br#"{"jsonrpc":"2.0","method":"one"}"#);
        decoder.finish().unwrap();
    }

    let mut decoder = FrameDecoder::new(frame_limits()).unwrap();
    let mut bodies = decoder.push(&first).unwrap();
    bodies.extend(decoder.push(&second).unwrap());
    assert_eq!(bodies.len(), 2);

    let mut zero = FrameDecoder::new(frame_limits()).unwrap();
    assert_eq!(
        zero.push(b"Content-Length: 0\r\n\r\n").unwrap(),
        vec![Vec::<u8>::new()]
    );
    zero.finish().unwrap();
}

#[test]
fn frame_decoder_rejects_bad_headers_eof_and_unsupported_transfer_framing() {
    for header in [
        b"Content-Length: 1\r\nContent-Length: 1\r\n\r\na".as_slice(),
        b"Content-Length: 1\r\nContent-Length: 2\r\n\r\na".as_slice(),
        b"X-Test: yes\r\n\r\na".as_slice(),
        b"Content-Length: -1\r\n\r\n".as_slice(),
        b"Content-Length: 999999999999999999999999\r\n\r\n".as_slice(),
        b"Content-Length: 1\n\na".as_slice(),
        b"Content-Length: 1\r\nContent-Type: text/plain\ninvalid\r\n\r\na".as_slice(),
        b"Transfer-Encoding: chunked\r\nContent-Length: 1\r\n\r\na".as_slice(),
        b"\r\n\r\n".as_slice(),
    ] {
        let mut decoder = FrameDecoder::new(frame_limits()).unwrap();
        assert!(decoder.push(header).is_err());
    }

    let mut decoder = FrameDecoder::new(frame_limits()).unwrap();
    decoder.push(b"Content-Length: 3\r\n\r\na").unwrap();
    assert!(decoder.finish().is_err());
}

#[test]
fn frame_decoder_enforces_header_body_protocol_and_message_limits() {
    let mut limits = frame_limits();
    limits.max_header_bytes = 8;
    let mut decoder = FrameDecoder::new(limits).unwrap();
    assert!(decoder.push(b"Content-Length: 1\r\n\r\na").is_err());

    let mut limits = frame_limits();
    limits.max_frame_bytes = 2;
    let mut decoder = FrameDecoder::new(limits).unwrap();
    assert!(decoder.push(b"Content-Length: 3\r\n\r\nabc").is_err());

    let mut limits = frame_limits();
    limits.max_protocol_bytes = 4;
    limits.max_frame_bytes = 4;
    let mut decoder = FrameDecoder::new(limits).unwrap();
    assert!(decoder.push(b"12345").is_err());

    let mut limits = frame_limits();
    limits.max_messages = 1;
    let mut decoder = FrameDecoder::new(limits).unwrap();
    let frame = frame_json(json!({"jsonrpc":"2.0","method":"x"})).unwrap();
    decoder.push(&frame).unwrap();
    assert!(decoder.push(&frame).is_err());
}

#[test]
fn messages_require_json_rpc_two_and_bound_envelopes() {
    let request =
        parse_inbound(br#"{"jsonrpc":"2.0","id":"srv","method":"work","params":{"ok":true}}"#)
            .unwrap();
    match request {
        InboundMessage::Request(request) => {
            assert_eq!(request.id, ServerRequestId::String("srv".to_string()));
            assert_eq!(request.method, "work");
        }
        _ => panic!("expected server request"),
    }

    let notification = parse_inbound(br#"{"jsonrpc":"2.0","method":"note"}"#).unwrap();
    assert!(matches!(notification, InboundMessage::Notification(_)));
    let response = parse_inbound(br#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#).unwrap();
    assert!(matches!(response, InboundMessage::Response(_)));

    for malformed in [
        br#"{}"#.as_slice(),
        br#"{"jsonrpc":"1.0","method":"x"}"#.as_slice(),
        br#"{"jsonrpc":"2.0","id":-1,"result":null}"#.as_slice(),
        br#"{"jsonrpc":"2.0","id":1,"result":null,"error":{"code":-1,"message":"x"}}"#.as_slice(),
        br#"{"jsonrpc":"2.0","id":1,"error":{"code":-1}}"#.as_slice(),
        br#"{"jsonrpc":"2.0","method":"x","params":1}"#.as_slice(),
        br#"{"jsonrpc":"2.0","method":"x","result":null}"#.as_slice(),
        br#"not-json"#.as_slice(),
    ] {
        assert!(parse_inbound(malformed).is_err());
    }
}

#[test]
fn correlation_state_bounds_pending_ids_and_rejects_unknown_or_duplicate_responses() {
    let mut state = CorrelationState::new(message_limits()).unwrap();
    let first = state.reserve_request("one").unwrap();
    let second = state.reserve_request("two").unwrap();
    assert_eq!(state.pending_len(), 2);
    assert!(state.reserve_request("three").is_err());

    let response = ClientResponse {
        id: second,
        outcome: ResponseOutcome::Result(json!(null)),
    };
    assert_eq!(
        state.accept_client_response(response.clone()).unwrap(),
        response
    );
    assert_eq!(state.pending_len(), 1);
    assert!(state.accept_client_response(response).is_err());
    assert!(state
        .accept_client_response(ClientResponse {
            id: 999,
            outcome: ResponseOutcome::Error(RpcErrorObject {
                code: -1,
                message: "unknown".to_string(),
                data: None,
            }),
        })
        .is_err());
    state
        .accept_client_response(ClientResponse {
            id: first,
            outcome: ResponseOutcome::Result(json!(true)),
        })
        .unwrap();
    assert_eq!(state.pending_len(), 0);
}

#[test]
fn encoders_emit_ascii_content_length_and_parse_back() {
    let request = encode_request(7, "work", Some(json!({"value":"é"}))).unwrap();
    assert!(request.starts_with(b"Content-Length: "));
    assert!(request.windows(2).any(|window| window == b"\r\n"));
    let body_start = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap()
        + 4;
    assert!(request[..body_start].is_ascii());
    assert!(matches!(
        parse_inbound(&request[body_start..]).unwrap(),
        InboundMessage::Request(_)
    ));

    let notification = encode_notification("note", None).unwrap();
    let notification_start = notification
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap()
        + 4;
    assert!(matches!(
        parse_inbound(&notification[notification_start..]).unwrap(),
        InboundMessage::Notification(_)
    ));
    let result = encode_result(7, json!(true)).unwrap();
    let error = encode_error(
        8,
        RpcErrorObject {
            code: -32601,
            message: "missing".to_string(),
            data: None,
        },
    )
    .unwrap();
    assert!(!result.is_empty());
    assert!(!error.is_empty());
    assert!(encode_request(9, "work", Some(json!(1))).is_err());
    assert!(encode_notification("note", Some(json!(true))).is_err());
}

#[test]
fn correlation_counters_reject_at_limits_without_wrapping() {
    let limits = MessageLimits {
        max_requests: 1,
        max_pending_requests: 1,
        max_messages: 4,
        max_notifications: 1,
        max_server_requests: 1,
        max_invalid_messages: 1,
    };
    let mut state = CorrelationState::new(limits).unwrap();
    state.observe_notification().unwrap();
    assert_eq!(
        state.observe_notification().unwrap_err().code,
        "provider-notification-limit"
    );
    state.observe_server_request().unwrap();
    state.observe_invalid().unwrap();
    assert_eq!(
        state.observe_invalid().unwrap_err().code,
        "provider-message-limit"
    );
}
