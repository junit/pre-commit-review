use serde_json::{Map, Value};
use std::collections::BTreeSet;

const MAX_METHOD_BYTES: usize = 256;
const MAX_STRING_BYTES: usize = 4 * 1024;
const MAX_VALUE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError {
    pub code: &'static str,
    message: String,
}

impl ProtocolError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProtocolError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLimits {
    pub max_header_bytes: usize,
    pub max_frame_bytes: usize,
    pub max_protocol_bytes: usize,
    pub max_messages: usize,
}

impl FrameLimits {
    fn validate(self) -> Result<(), ProtocolError> {
        if self.max_header_bytes == 0
            || self.max_frame_bytes == 0
            || self.max_protocol_bytes == 0
            || self.max_messages == 0
            || self.max_frame_bytes > self.max_protocol_bytes
        {
            return Err(ProtocolError::new(
                "provider-frame-limits-invalid",
                "JSON-RPC frame limits are invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct FrameDecoder {
    limits: FrameLimits,
    buffer: Vec<u8>,
    expected_body: Option<usize>,
    protocol_bytes: usize,
    messages: usize,
}

impl FrameDecoder {
    pub fn new(limits: FrameLimits) -> Result<Self, ProtocolError> {
        limits.validate()?;
        Ok(Self {
            limits,
            buffer: Vec::new(),
            expected_body: None,
            protocol_bytes: 0,
            messages: 0,
        })
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, ProtocolError> {
        self.protocol_bytes = self
            .protocol_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| {
                ProtocolError::new(
                    "provider-frame-limit",
                    "JSON-RPC protocol bytes exceeded the limit",
                )
            })?;
        if self.protocol_bytes > self.limits.max_protocol_bytes {
            return Err(ProtocolError::new(
                "provider-frame-limit",
                "JSON-RPC protocol bytes exceeded the limit",
            ));
        }
        let buffer_limit = self
            .limits
            .max_header_bytes
            .checked_add(self.limits.max_frame_bytes)
            .ok_or_else(|| {
                ProtocolError::new(
                    "provider-frame-limit",
                    "JSON-RPC frame buffer exceeded the limit",
                )
            })?;
        if self
            .buffer
            .len()
            .checked_add(bytes.len())
            .is_none_or(|value| value > buffer_limit)
        {
            return Err(ProtocolError::new(
                "provider-frame-limit",
                "JSON-RPC frame buffer exceeded the limit",
            ));
        }
        self.buffer.extend_from_slice(bytes);
        self.drain_frames()
    }

    pub fn finish(self) -> Result<(), ProtocolError> {
        if self.expected_body.is_some() || !self.buffer.is_empty() {
            return Err(ProtocolError::new(
                "provider-frame-eof",
                "JSON-RPC stream ended with a partial frame",
            ));
        }
        Ok(())
    }

    pub fn buffered_bytes(&self) -> usize {
        self.buffer.len()
    }

    fn drain_frames(&mut self) -> Result<Vec<Vec<u8>>, ProtocolError> {
        let mut frames = Vec::new();
        loop {
            if let Some(body_len) = self.expected_body {
                if self.buffer.len() < body_len {
                    break;
                }
                let body = self.buffer.drain(..body_len).collect::<Vec<_>>();
                self.expected_body = None;
                self.messages = self.messages.checked_add(1).ok_or_else(|| {
                    ProtocolError::new(
                        "provider-frame-limit",
                        "JSON-RPC message count exceeded the limit",
                    )
                })?;
                if self.messages > self.limits.max_messages {
                    return Err(ProtocolError::new(
                        "provider-frame-limit",
                        "JSON-RPC message count exceeded the limit",
                    ));
                }
                frames.push(body);
                continue;
            }

            let delimiter = find_header_end(&self.buffer);
            let header_candidate =
                delimiter.map_or(self.buffer.as_slice(), |end| &self.buffer[..end]);
            if contains_bare_lf(header_candidate) {
                return Err(ProtocolError::new(
                    "provider-frame-header-invalid",
                    "JSON-RPC headers must use CRLF line endings",
                ));
            }
            let Some(delimiter) = delimiter else {
                if self.buffer.len() > self.limits.max_header_bytes {
                    return Err(ProtocolError::new(
                        "provider-frame-header-limit",
                        "JSON-RPC header exceeded the limit",
                    ));
                }
                break;
            };
            let header_bytes = delimiter + 4;
            if header_bytes > self.limits.max_header_bytes {
                return Err(ProtocolError::new(
                    "provider-frame-header-limit",
                    "JSON-RPC header exceeded the limit",
                ));
            }
            let body_len = parse_content_length(&self.buffer[..delimiter])?;
            if body_len > self.limits.max_frame_bytes {
                return Err(ProtocolError::new(
                    "provider-frame-limit",
                    "JSON-RPC frame body exceeded the limit",
                ));
            }
            self.buffer.drain(..header_bytes);
            self.expected_body = Some(body_len);
        }
        Ok(frames)
    }
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn contains_bare_lf(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .enumerate()
        .any(|(index, byte)| *byte == b'\n' && (index == 0 || bytes[index - 1] != b'\r'))
}

fn parse_content_length(header: &[u8]) -> Result<usize, ProtocolError> {
    let header = std::str::from_utf8(header).map_err(|_| {
        ProtocolError::new(
            "provider-frame-header-invalid",
            "JSON-RPC header is not valid ASCII",
        )
    })?;
    let mut content_length = None;
    for line in header.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            return Err(ProtocolError::new(
                "provider-frame-header-invalid",
                "JSON-RPC header is malformed",
            ));
        };
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some()
                || value.is_empty()
                || !value.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(ProtocolError::new(
                    "provider-frame-header-invalid",
                    "JSON-RPC Content-Length is malformed or duplicated",
                ));
            }
            let parsed = value.parse::<usize>().map_err(|_| {
                ProtocolError::new(
                    "provider-frame-header-invalid",
                    "JSON-RPC Content-Length is out of range",
                )
            })?;
            content_length = Some(parsed);
        } else if name.eq_ignore_ascii_case("content-type") {
            if value.is_empty()
                || !value.is_ascii()
                || value.bytes().any(|byte| byte.is_ascii_control())
            {
                return Err(ProtocolError::new(
                    "provider-frame-header-invalid",
                    "JSON-RPC Content-Type is malformed",
                ));
            }
        } else {
            return Err(ProtocolError::new(
                "provider-frame-header-invalid",
                "JSON-RPC header contains an unsupported field",
            ));
        }
    }
    content_length.ok_or_else(|| {
        ProtocolError::new(
            "provider-frame-header-invalid",
            "JSON-RPC Content-Length is missing",
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientResponse {
    pub id: u64,
    pub outcome: ResponseOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseOutcome {
    Result(Value),
    Error(RpcErrorObject),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerRequestId {
    Number(u64),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRequest {
    pub id: ServerRequestId,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerNotification {
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundMessage {
    Response(ClientResponse),
    Request(ServerRequest),
    Notification(ServerNotification),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcErrorObject {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

pub fn parse_inbound(bytes: &[u8]) -> Result<InboundMessage, ProtocolError> {
    if bytes.len() > MAX_VALUE_BYTES {
        return Err(ProtocolError::new(
            "provider-message-limit",
            "JSON-RPC message exceeded the limit",
        ));
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| {
        ProtocolError::new("provider-message-invalid", "JSON-RPC message is malformed")
    })?;
    let object = value.as_object().ok_or_else(|| {
        ProtocolError::new(
            "provider-message-invalid",
            "JSON-RPC message must be an object",
        )
    })?;
    if object.get("jsonrpc") != Some(&Value::String("2.0".to_string())) {
        return Err(ProtocolError::new(
            "provider-message-invalid",
            "JSON-RPC message must use version 2.0",
        ));
    }
    if let Some(method) = object.get("method") {
        if object.contains_key("result") || object.contains_key("error") {
            return Err(ProtocolError::new(
                "provider-message-invalid",
                "JSON-RPC request contains response fields",
            ));
        }
        let method = bounded_method(method)?;
        let params = bounded_params(object.get("params"))?;
        if let Some(id) = object.get("id") {
            return Ok(InboundMessage::Request(ServerRequest {
                id: parse_server_id(id)?,
                method,
                params,
            }));
        }
        return Ok(InboundMessage::Notification(ServerNotification {
            method,
            params,
        }));
    }

    let id = object.get("id").and_then(Value::as_u64).ok_or_else(|| {
        ProtocolError::new(
            "provider-message-invalid",
            "JSON-RPC response ID is invalid",
        )
    })?;
    let result = object.get("result");
    let error = object.get("error");
    match (result, error) {
        (Some(result), None) => Ok(InboundMessage::Response(ClientResponse {
            id,
            outcome: ResponseOutcome::Result(bounded_value(Some(result))?.unwrap_or(Value::Null)),
        })),
        (None, Some(error)) => Ok(InboundMessage::Response(ClientResponse {
            id,
            outcome: ResponseOutcome::Error(parse_error(error)?),
        })),
        _ => Err(ProtocolError::new(
            "provider-message-invalid",
            "JSON-RPC response must contain exactly one result or error",
        )),
    }
}

fn bounded_method(value: &Value) -> Result<String, ProtocolError> {
    let method = value.as_str().ok_or_else(|| {
        ProtocolError::new("provider-message-invalid", "JSON-RPC method is invalid")
    })?;
    if method.is_empty() || method.len() > MAX_METHOD_BYTES || method.chars().any(char::is_control)
    {
        return Err(ProtocolError::new(
            "provider-message-limit",
            "JSON-RPC method exceeded the limit",
        ));
    }
    Ok(method.to_string())
}

fn parse_server_id(value: &Value) -> Result<ServerRequestId, ProtocolError> {
    if let Some(id) = value.as_u64() {
        return Ok(ServerRequestId::Number(id));
    }
    if let Some(id) = value.as_str() {
        if id.is_empty() || id.len() > MAX_STRING_BYTES || id.chars().any(char::is_control) {
            return Err(ProtocolError::new(
                "provider-message-limit",
                "JSON-RPC server request ID exceeded the limit",
            ));
        }
        return Ok(ServerRequestId::String(id.to_string()));
    }
    Err(ProtocolError::new(
        "provider-message-invalid",
        "JSON-RPC server request ID is invalid",
    ))
}

fn bounded_value(value: Option<&Value>) -> Result<Option<Value>, ProtocolError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let encoded = serde_json::to_vec(value).map_err(|_| {
        ProtocolError::new(
            "provider-message-invalid",
            "JSON-RPC value cannot be encoded",
        )
    })?;
    if encoded.len() > MAX_VALUE_BYTES {
        return Err(ProtocolError::new(
            "provider-message-limit",
            "JSON-RPC value exceeded the limit",
        ));
    }
    Ok(Some(value.clone()))
}

fn bounded_params(value: Option<&Value>) -> Result<Option<Value>, ProtocolError> {
    if value.is_some_and(|value| !value.is_array() && !value.is_object()) {
        return Err(ProtocolError::new(
            "provider-message-invalid",
            "JSON-RPC params must be an object or array",
        ));
    }
    bounded_value(value)
}

fn parse_error(value: &Value) -> Result<RpcErrorObject, ProtocolError> {
    let object = value.as_object().ok_or_else(|| {
        ProtocolError::new(
            "provider-message-invalid",
            "JSON-RPC error object is invalid",
        )
    })?;
    let code = object.get("code").and_then(Value::as_i64).ok_or_else(|| {
        ProtocolError::new("provider-message-invalid", "JSON-RPC error code is invalid")
    })?;
    let message = object
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ProtocolError::new(
                "provider-message-invalid",
                "JSON-RPC error message is invalid",
            )
        })?;
    if message.is_empty()
        || message.len() > MAX_STRING_BYTES
        || message.chars().any(char::is_control)
    {
        return Err(ProtocolError::new(
            "provider-message-limit",
            "JSON-RPC error message exceeded the limit",
        ));
    }
    Ok(RpcErrorObject {
        code,
        message: message.to_string(),
        data: bounded_value(object.get("data"))?,
    })
}

pub fn frame_json(value: Value) -> Result<Vec<u8>, ProtocolError> {
    let body = serde_json::to_vec(&value).map_err(|_| {
        ProtocolError::new(
            "provider-message-invalid",
            "JSON-RPC value cannot be encoded",
        )
    })?;
    if body.len() > 4 * 1024 * 1024 {
        return Err(ProtocolError::new(
            "provider-frame-limit",
            "JSON-RPC frame body exceeded the limit",
        ));
    }
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut frame = header.into_bytes();
    frame.extend_from_slice(&body);
    Ok(frame)
}

pub fn encode_request(
    id: u64,
    method: &str,
    params: Option<Value>,
) -> Result<Vec<u8>, ProtocolError> {
    let mut object = rpc_object(method)?;
    object.insert("id".to_string(), Value::Number(id.into()));
    if let Some(params) = bounded_params(params.as_ref())? {
        object.insert("params".to_string(), params);
    }
    frame_json(Value::Object(object))
}

pub fn encode_notification(method: &str, params: Option<Value>) -> Result<Vec<u8>, ProtocolError> {
    let mut object = rpc_object(method)?;
    if let Some(params) = bounded_params(params.as_ref())? {
        object.insert("params".to_string(), params);
    }
    frame_json(Value::Object(object))
}

pub fn encode_result(id: u64, result: Value) -> Result<Vec<u8>, ProtocolError> {
    frame_json(serde_json::json!({"jsonrpc":"2.0","id":id,"result":result}))
}

pub fn encode_error(id: u64, error: RpcErrorObject) -> Result<Vec<u8>, ProtocolError> {
    validate_error(&error)?;
    let mut object = Map::new();
    object.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
    object.insert("id".to_string(), Value::Number(id.into()));
    let mut error_object = Map::new();
    error_object.insert("code".to_string(), Value::Number(error.code.into()));
    error_object.insert("message".to_string(), Value::String(error.message));
    if let Some(data) = error.data {
        error_object.insert("data".to_string(), data);
    }
    object.insert("error".to_string(), Value::Object(error_object));
    frame_json(Value::Object(object))
}

fn rpc_object(method: &str) -> Result<Map<String, Value>, ProtocolError> {
    let method = bounded_method(&Value::String(method.to_string()))?;
    Ok(Map::from_iter([
        ("jsonrpc".to_string(), Value::String("2.0".to_string())),
        ("method".to_string(), Value::String(method)),
    ]))
}

fn validate_error(error: &RpcErrorObject) -> Result<(), ProtocolError> {
    if error.message.is_empty()
        || error.message.len() > MAX_STRING_BYTES
        || error.message.chars().any(char::is_control)
    {
        return Err(ProtocolError::new(
            "provider-message-limit",
            "JSON-RPC error message exceeded the limit",
        ));
    }
    bounded_value(error.data.as_ref())?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageLimits {
    pub max_requests: usize,
    pub max_pending_requests: usize,
    pub max_messages: usize,
    pub max_notifications: usize,
    pub max_server_requests: usize,
    pub max_invalid_messages: usize,
}

impl MessageLimits {
    fn validate(self) -> Result<(), ProtocolError> {
        if self.max_requests == 0
            || self.max_pending_requests == 0
            || self.max_messages == 0
            || self.max_notifications == 0
            || self.max_server_requests == 0
            || self.max_invalid_messages == 0
            || self.max_pending_requests > self.max_requests
        {
            return Err(ProtocolError::new(
                "provider-message-limits-invalid",
                "JSON-RPC message limits are invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct CorrelationState {
    limits: MessageLimits,
    pending: BTreeSet<u64>,
    next_id: u64,
    requests: usize,
    messages: usize,
    notifications: usize,
    server_requests: usize,
    invalid: usize,
}

impl CorrelationState {
    pub fn new(limits: MessageLimits) -> Result<Self, ProtocolError> {
        limits.validate()?;
        Ok(Self {
            limits,
            pending: BTreeSet::new(),
            next_id: 1,
            requests: 0,
            messages: 0,
            notifications: 0,
            server_requests: 0,
            invalid: 0,
        })
    }

    pub fn reserve_request(&mut self, method: &str) -> Result<u64, ProtocolError> {
        bounded_method(&Value::String(method.to_string()))?;
        if self.requests >= self.limits.max_requests {
            return Err(ProtocolError::new(
                "provider-request-limit",
                "JSON-RPC request count exceeded the limit",
            ));
        }
        if self.pending.len() >= self.limits.max_pending_requests {
            return Err(ProtocolError::new(
                "provider-pending-limit",
                "JSON-RPC pending request count exceeded the limit",
            ));
        }
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or_else(|| {
            ProtocolError::new("provider-request-limit", "JSON-RPC request ID overflowed")
        })?;
        self.requests += 1;
        self.pending.insert(id);
        Ok(id)
    }

    pub fn accept_client_response(
        &mut self,
        response: ClientResponse,
    ) -> Result<ClientResponse, ProtocolError> {
        self.observe_message()?;
        if !self.pending.remove(&response.id) {
            increment_bounded(
                &mut self.invalid,
                self.limits.max_invalid_messages,
                "provider-invalid-limit",
                "JSON-RPC invalid message count exceeded the limit",
            )?;
            return Err(ProtocolError::new(
                "provider-response-id-invalid",
                "JSON-RPC response ID is unknown or completed",
            ));
        }
        Ok(response)
    }

    pub fn observe_server_request(&mut self) -> Result<(), ProtocolError> {
        self.observe_message()?;
        increment_bounded(
            &mut self.server_requests,
            self.limits.max_server_requests,
            "provider-server-request-limit",
            "JSON-RPC server request count exceeded the limit",
        )
    }

    pub fn observe_notification(&mut self) -> Result<(), ProtocolError> {
        self.observe_message()?;
        increment_bounded(
            &mut self.notifications,
            self.limits.max_notifications,
            "provider-notification-limit",
            "JSON-RPC notification count exceeded the limit",
        )
    }

    pub fn observe_invalid(&mut self) -> Result<(), ProtocolError> {
        self.observe_message()?;
        increment_bounded(
            &mut self.invalid,
            self.limits.max_invalid_messages,
            "provider-invalid-limit",
            "JSON-RPC invalid message count exceeded the limit",
        )
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    fn observe_message(&mut self) -> Result<(), ProtocolError> {
        increment_bounded(
            &mut self.messages,
            self.limits.max_messages,
            "provider-message-limit",
            "JSON-RPC message count exceeded the limit",
        )
    }
}

fn increment_bounded(
    counter: &mut usize,
    maximum: usize,
    code: &'static str,
    message: &'static str,
) -> Result<(), ProtocolError> {
    if *counter >= maximum {
        return Err(ProtocolError::new(code, message));
    }
    *counter += 1;
    Ok(())
}
