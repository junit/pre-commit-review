use super::contract::{PositionEncoding, RustAnalyzerProjectModel};
use super::json_rpc::{InboundMessage, ResponseOutcome, ServerRequest};
use super::session::{ManagedLspSession, SessionError};
use super::snapshot::BoundCandidateSnapshot;
use serde_json::{json, Value};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Healthy,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustAnalyzerHandshake {
    pub position_encoding: PositionEncoding,
    pub readiness: Readiness,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustAnalyzerHandshakeError {
    pub code: &'static str,
    message: String,
}

impl RustAnalyzerHandshakeError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl std::fmt::Display for RustAnalyzerHandshakeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RustAnalyzerHandshakeError {}

pub fn initialize_and_gate(
    session: &mut ManagedLspSession,
    snapshot: &BoundCandidateSnapshot<'_>,
    model: &RustAnalyzerProjectModel,
    target_triple: &str,
) -> Result<RustAnalyzerHandshake, RustAnalyzerHandshakeError> {
    let root_uri = Url::from_directory_path(snapshot.root()).map_err(|_| {
        RustAnalyzerHandshakeError::new("provider-uri-invalid", "snapshot root URI is invalid")
    })?;
    let linked_project = model.linked_project_value().map_err(|_| {
        RustAnalyzerHandshakeError::new("provider-model-invalid", "linked project model invalid")
    })?;
    let initialize_params = json!({
        "processId": Value::Null,
        "rootUri": root_uri.clone(),
        "workspaceFolders": [{"uri": root_uri, "name": "candidate"}],
        "capabilities": {
            "general": {"positionEncodings": ["utf-8", "utf-16"]},
            "workspace": {"configuration": true},
            "textDocument": {"callHierarchy": {"dynamicRegistration": false}},
            "experimental": {"serverStatusNotification": true}
        },
        "initializationOptions": {
            "linkedProjects": [linked_project],
            "cargo": {
                "buildScripts": {"enable": false},
                "noDeps": true,
                "sysroot": null,
                "sysrootSrc": null,
                "target": target_triple
            },
            "procMacro": {"enable": false},
            "checkOnSave": false
        }
    });
    let initialize_id = session
        .send_request("initialize", initialize_params)
        .map_err(session_error)?;
    let capabilities = loop {
        match session.next_message().map_err(session_error)? {
            InboundMessage::Response(response) if response.id == initialize_id => {
                match response.outcome {
                    ResponseOutcome::Result(value) => break value,
                    ResponseOutcome::Error(_) => {
                        return Err(RustAnalyzerHandshakeError::new(
                            "provider-initialize-failed",
                            "rust-analyzer initialize request failed",
                        ));
                    }
                }
            }
            InboundMessage::Request(request) => {
                handle_server_request(session, &request).map_err(session_error)?;
            }
            InboundMessage::Notification(_) | InboundMessage::Response(_) => {}
        }
    };
    session
        .send_notification("initialized", json!({}))
        .map_err(session_error)?;

    let capabilities = capabilities.get("capabilities").ok_or_else(|| {
        RustAnalyzerHandshakeError::new(
            "provider-initialize-invalid",
            "initialize result capabilities missing",
        )
    })?;
    if !capabilities
        .get("callHierarchyProvider")
        .is_some_and(|value| !value.is_null() && value != &Value::Bool(false))
    {
        return Err(RustAnalyzerHandshakeError::new(
            "provider-capability-unavailable",
            "rust-analyzer call hierarchy capability is unavailable",
        ));
    }
    let position_encoding = parse_position_encoding(capabilities.get("positionEncoding"))?;
    let mut limitations = Vec::new();
    let readiness = loop {
        match session.next_message().map_err(session_error)? {
            InboundMessage::Notification(notification)
                if notification.method == "experimental/serverStatus" =>
            {
                let params = notification.params.ok_or_else(|| {
                    RustAnalyzerHandshakeError::new(
                        "provider-readiness-invalid",
                        "rust-analyzer readiness status is malformed",
                    )
                })?;
                let quiescent = params
                    .get("quiescent")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| {
                        RustAnalyzerHandshakeError::new(
                            "provider-readiness-invalid",
                            "rust-analyzer readiness status is malformed",
                        )
                    })?;
                if !quiescent {
                    continue;
                }
                match params.get("health").and_then(Value::as_str) {
                    Some("ok") => break Readiness::Healthy,
                    Some("warning") => {
                        limitations.push("rust-analyzer-readiness-warning".to_string());
                        break Readiness::Warning;
                    }
                    Some("error") => {
                        return Err(RustAnalyzerHandshakeError::new(
                            "provider-readiness-unavailable",
                            "rust-analyzer reports unhealthy readiness",
                        ));
                    }
                    _ => {
                        return Err(RustAnalyzerHandshakeError::new(
                            "provider-readiness-invalid",
                            "rust-analyzer readiness health is malformed",
                        ));
                    }
                }
            }
            InboundMessage::Request(request) => {
                handle_server_request(session, &request).map_err(session_error)?;
            }
            InboundMessage::Notification(_) | InboundMessage::Response(_) => {}
        }
    };
    Ok(RustAnalyzerHandshake {
        position_encoding,
        readiness,
        limitations,
    })
}

fn parse_position_encoding(
    value: Option<&Value>,
) -> Result<PositionEncoding, RustAnalyzerHandshakeError> {
    let Some(value) = value else {
        return Ok(PositionEncoding::Utf16);
    };
    let value = value.as_str().ok_or_else(|| {
        RustAnalyzerHandshakeError::new(
            "provider-position-encoding-invalid",
            "rust-analyzer position encoding is malformed",
        )
    })?;
    match value {
        "utf-8" => Ok(PositionEncoding::Utf8),
        "utf-16" => Ok(PositionEncoding::Utf16),
        _ => Err(RustAnalyzerHandshakeError::new(
            "provider-position-encoding-invalid",
            "rust-analyzer position encoding is unsupported",
        )),
    }
}

fn handle_server_request(
    session: &mut ManagedLspSession,
    request: &ServerRequest,
) -> Result<(), SessionError> {
    match request.method.as_str() {
        "workspace/configuration" => {
            let items = request
                .params
                .as_ref()
                .and_then(|params| params.get("items"))
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    SessionError::new(
                        "provider-server-request-invalid",
                        "configuration request malformed",
                    )
                })?;
            session.send_server_result(&request.id, Value::Array(vec![Value::Null; items.len()]))
        }
        "window/workDoneProgress/create" => session.send_server_result(&request.id, Value::Null),
        "workspace/applyEdit" => session.send_server_result(&request.id, json!({"applied": false})),
        "client/registerCapability" => {
            let registrations = request
                .params
                .as_ref()
                .and_then(|params| params.get("registrations"))
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    SessionError::new(
                        "provider-server-request-invalid",
                        "registration request malformed",
                    )
                })?;
            let all_allowed = registrations.iter().all(|registration| {
                registration
                    .get("method")
                    .and_then(Value::as_str)
                    .is_some_and(|method| method == "workspace/didChangeConfiguration")
            });
            if all_allowed {
                session.send_server_result(&request.id, Value::Null)
            } else {
                session.send_server_error(
                    &request.id,
                    -32601,
                    "dynamic registration is not allowed",
                )
            }
        }
        _ => session.send_server_error(&request.id, -32601, "unsupported server request"),
    }
}

fn session_error(error: SessionError) -> RustAnalyzerHandshakeError {
    RustAnalyzerHandshakeError::new(error.code, "rust-analyzer session operation failed")
}
