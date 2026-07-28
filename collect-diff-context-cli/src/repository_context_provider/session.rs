use super::contract::ProviderLimits;
use super::json_rpc::{
    encode_error, encode_notification, encode_request, frame_json, parse_inbound, ClientResponse,
    CorrelationState, FrameDecoder, FrameLimits, InboundMessage, MessageLimits, ResponseOutcome,
    RpcErrorObject, ServerRequestId,
};
use super::snapshot::BoundCandidateSnapshot;
use crate::review_scope::ReviewSource;
use crate::trusted_runtime::{
    apply_base_environment, ManagedChild, PrivateRuntime, TrustedRuntimeError,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{ChildStdin, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionError {
    pub code: &'static str,
    message: String,
}

impl SessionError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }

    fn from_runtime(error: TrustedRuntimeError) -> Self {
        Self::new(error.code, "trusted provider runtime operation failed")
    }
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SessionError {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionMetrics {
    pub messages: usize,
    pub requests: usize,
    pub notifications: usize,
    pub server_requests: usize,
    pub invalid_messages: usize,
    pub stderr_bytes: usize,
    pub stderr_sha256: String,
    pub total_output_bytes: usize,
}

pub struct SessionLaunch<'a> {
    pub snapshot: &'a BoundCandidateSnapshot<'a>,
    pub executable: &'a Path,
    pub executable_sha256: &'a str,
    pub arguments: &'a [String],
    pub source: ReviewSource,
    pub scope_fingerprint: &'a str,
    pub limits: &'a ProviderLimits,
    pub cancellation: Arc<AtomicBool>,
}

#[derive(Debug)]
enum ReaderEvent {
    Frame(Vec<u8>),
    Error(&'static str),
    Eof,
}

#[derive(Debug, Clone)]
struct StderrSummary {
    bytes: usize,
    sha256: String,
}

#[derive(Clone)]
struct OutputBudget {
    maximum: usize,
    overflow: Arc<AtomicBool>,
    total: Arc<AtomicUsize>,
}

impl OutputBudget {
    fn observe(&self, bytes: usize) -> bool {
        let total = self.total.fetch_add(bytes, Ordering::AcqRel) + bytes;
        if total > self.maximum {
            self.overflow.store(true, Ordering::Release);
            false
        } else {
            true
        }
    }
}

pub struct ManagedLspSession {
    _runtime: PrivateRuntime,
    child: ManagedChild,
    stdin: Option<ChildStdin>,
    stdout_events: Receiver<ReaderEvent>,
    stderr_summary: Receiver<StderrSummary>,
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    stdout_overflow: Arc<AtomicBool>,
    stderr_overflow: Arc<AtomicBool>,
    output_overflow: Arc<AtomicBool>,
    total_output: Arc<AtomicUsize>,
    correlation: CorrelationState,
    cancellation: Arc<AtomicBool>,
    deadline: Instant,
    metrics: SessionMetrics,
}

impl ManagedLspSession {
    pub fn spawn(launch: SessionLaunch<'_>) -> Result<Self, SessionError> {
        launch
            .limits
            .validate()
            .map_err(|_| SessionError::new("provider-limits-invalid", "provider limits invalid"))?;
        if launch.limits.deadline_ms == 0 {
            return Err(SessionError::new(
                "provider-deadline-invalid",
                "provider deadline must be positive",
            ));
        }
        let runtime = PrivateRuntime::create(launch.executable, launch.executable_sha256)
            .map_err(SessionError::from_runtime)?;
        let mut command = std::process::Command::new(runtime.executable_path());
        command
            .args(launch.arguments)
            .current_dir(launch.snapshot.root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        apply_base_environment(
            &mut command,
            &runtime,
            runtime.empty_path().as_os_str(),
            launch.source.as_str(),
            launch.scope_fingerprint,
        );
        command
            .env("CARGO_NET_OFFLINE", "true")
            .env("RUSTUP_AUTO_INSTALL", "0")
            .env("CARGO_TARGET_DIR", runtime.target())
            .env("RUST_ANALYZER cargo.buildScripts.enable", "false")
            .env("RUST_ANALYZER cargo.noDeps", "true")
            .env("RUST_ANALYZER procMacro.enable", "false")
            .env("RUST_ANALYZER checkOnSave.enable", "false");

        let mut child = ManagedChild::spawn(command).map_err(SessionError::from_runtime)?;
        let stdin = child.child_mut().stdin.take().ok_or_else(|| {
            SessionError::new("provider-stdin-missing", "provider stdin unavailable")
        })?;
        let stdout = child.child_mut().stdout.take().ok_or_else(|| {
            SessionError::new("provider-stdout-missing", "provider stdout unavailable")
        })?;
        let stderr = child.child_mut().stderr.take().ok_or_else(|| {
            SessionError::new("provider-stderr-missing", "provider stderr unavailable")
        })?;

        let stdout_overflow = Arc::new(AtomicBool::new(false));
        let stderr_overflow = Arc::new(AtomicBool::new(false));
        let output_overflow = Arc::new(AtomicBool::new(false));
        let total_output = Arc::new(AtomicUsize::new(0));
        let (stdout_sender, stdout_events) =
            mpsc::sync_channel(launch.limits.max_messages.clamp(1, 64));
        let output_budget = OutputBudget {
            maximum: launch.limits.max_total_output_bytes,
            overflow: Arc::clone(&output_overflow),
            total: Arc::clone(&total_output),
        };
        let stdout_thread = Some(spawn_stdout_reader(
            stdout,
            stdout_sender,
            FrameLimits {
                max_header_bytes: launch.limits.max_header_bytes,
                max_frame_bytes: launch.limits.max_frame_bytes,
                max_protocol_bytes: launch.limits.max_protocol_bytes,
                max_messages: launch.limits.max_messages,
            },
            Arc::clone(&stdout_overflow),
            output_budget.clone(),
        ));
        let (stderr_sender, stderr_summary) = mpsc::sync_channel(1);
        let stderr_thread = Some(spawn_stderr_reader(
            stderr,
            launch.limits.max_stderr_bytes,
            stderr_sender,
            Arc::clone(&stderr_overflow),
            output_budget,
        ));
        let correlation = CorrelationState::new(MessageLimits {
            max_requests: launch.limits.max_requests,
            max_pending_requests: launch.limits.max_pending_requests,
            max_messages: launch.limits.max_messages,
            max_notifications: launch.limits.max_notifications,
            max_server_requests: launch.limits.max_server_requests,
            max_invalid_messages: launch.limits.max_invalid_messages,
        })
        .map_err(|_| SessionError::new("provider-limits-invalid", "provider limits invalid"))?;
        Ok(Self {
            _runtime: runtime,
            child,
            stdin: Some(stdin),
            stdout_events,
            stderr_summary,
            stdout_thread,
            stderr_thread,
            stdout_overflow,
            stderr_overflow,
            output_overflow,
            total_output,
            correlation,
            cancellation: launch.cancellation,
            deadline: Instant::now() + Duration::from_millis(launch.limits.deadline_ms),
            metrics: SessionMetrics::default(),
        })
    }

    pub fn send_request(&mut self, method: &str, params: Value) -> Result<u64, SessionError> {
        self.send_request_optional(method, Some(params))
    }

    fn send_request_optional(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<u64, SessionError> {
        self.check_limits()?;
        let id = self
            .correlation
            .reserve_request(method)
            .map_err(protocol_error)?;
        let frame = encode_request(id, method, params).map_err(protocol_error)?;
        self.write_frame(&frame)?;
        self.metrics.requests += 1;
        Ok(id)
    }

    pub fn send_notification(&mut self, method: &str, params: Value) -> Result<(), SessionError> {
        self.send_notification_optional(method, Some(params))
    }

    fn send_notification_optional(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), SessionError> {
        self.check_limits()?;
        let frame = encode_notification(method, params).map_err(protocol_error)?;
        self.write_frame(&frame)
    }

    pub fn send_server_result(
        &mut self,
        id: &ServerRequestId,
        value: Value,
    ) -> Result<(), SessionError> {
        self.write_server_response(id, Some(value), None)
    }

    pub fn send_server_error(
        &mut self,
        id: &ServerRequestId,
        code: i64,
        message: &str,
    ) -> Result<(), SessionError> {
        self.write_server_response(
            id,
            None,
            Some(RpcErrorObject {
                code,
                message: message.chars().take(4_096).collect(),
                data: None,
            }),
        )
    }

    pub fn next_message(&mut self) -> Result<InboundMessage, SessionError> {
        let event = loop {
            self.check_limits()?;
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_default();
            let poll = remaining.min(Duration::from_millis(10));
            match self.stdout_events.recv_timeout(poll) {
                Ok(event) => break event,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(SessionError::new(
                        "provider-child-exited",
                        "provider output stream ended",
                    ));
                }
            }
        };
        let body = match event {
            ReaderEvent::Frame(body) => body,
            ReaderEvent::Error(code) => {
                self.record_invalid()?;
                return Err(SessionError::new(code, "provider output invalid"));
            }
            ReaderEvent::Eof => {
                return Err(SessionError::new(
                    "provider-child-eof",
                    "provider output stream ended",
                ))
            }
        };
        let message = match parse_inbound(&body) {
            Ok(message) => message,
            Err(error) => {
                self.record_invalid()?;
                return Err(protocol_error(error));
            }
        };
        self.metrics.messages += 1;
        match message {
            InboundMessage::Response(response) => {
                match self.correlation.accept_client_response(response) {
                    Ok(response) => Ok(InboundMessage::Response(response)),
                    Err(error) => {
                        if error.code == "provider-response-id-invalid" {
                            self.metrics.invalid_messages += 1;
                        }
                        Err(protocol_error(error))
                    }
                }
            }
            InboundMessage::Request(request) => {
                self.correlation
                    .observe_server_request()
                    .map_err(protocol_error)?;
                self.metrics.server_requests += 1;
                Ok(InboundMessage::Request(request))
            }
            InboundMessage::Notification(notification) => {
                self.correlation
                    .observe_notification()
                    .map_err(protocol_error)?;
                self.metrics.notifications += 1;
                Ok(InboundMessage::Notification(notification))
            }
        }
    }

    pub fn shutdown_and_reap(&mut self) -> Result<(), SessionError> {
        let shutdown_id = self.send_request_optional("shutdown", None)?;
        loop {
            match self.next_message()? {
                InboundMessage::Response(ClientResponse { id, outcome }) if id == shutdown_id => {
                    if matches!(outcome, ResponseOutcome::Error(_)) {
                        return Err(SessionError::new(
                            "provider-shutdown-failed",
                            "provider shutdown request failed",
                        ));
                    }
                    break;
                }
                InboundMessage::Request(request) => {
                    self.send_server_error(&request.id, -32601, "unsupported server request")?;
                }
                _ => {}
            }
        }
        self.send_notification_optional("exit", None)?;
        self.stdin.take();
        loop {
            if self
                .child
                .try_wait()
                .map_err(SessionError::from_runtime)?
                .is_some()
            {
                self.join_readers();
                return Ok(());
            }
            if self.cancellation.load(Ordering::Acquire) {
                self.terminate();
                return Err(SessionError::new(
                    "provider-cancelled",
                    "provider operation cancelled",
                ));
            }
            if Instant::now() >= self.deadline {
                self.terminate();
                return Err(SessionError::new(
                    "provider-timeout",
                    "provider deadline exceeded",
                ));
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn terminate(&mut self) {
        self.stdin.take();
        let _ = self.child.terminate_and_wait();
        self.join_readers();
    }

    pub fn metrics(&self) -> &SessionMetrics {
        &self.metrics
    }

    fn write_server_response(
        &mut self,
        id: &ServerRequestId,
        result: Option<Value>,
        error: Option<RpcErrorObject>,
    ) -> Result<(), SessionError> {
        let mut object = Map::new();
        object.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
        object.insert("id".to_string(), server_id_value(id));
        if let Some(result) = result {
            object.insert("result".to_string(), result);
        }
        if let Some(error) = error {
            let frame = encode_error(0, error).map_err(protocol_error)?;
            let body_start = frame
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .ok_or_else(|| {
                    SessionError::new("provider-frame-invalid", "provider frame invalid")
                })?
                + 4;
            let mut response: Value =
                serde_json::from_slice(&frame[body_start..]).map_err(|_| {
                    SessionError::new("provider-frame-invalid", "provider frame invalid")
                })?;
            if let Some(map) = response.as_object_mut() {
                map.insert("id".to_string(), server_id_value(id));
            }
            return self.write_frame(&frame_json(response).map_err(protocol_error)?);
        }
        self.write_frame(&frame_json(Value::Object(object)).map_err(protocol_error)?)
    }

    fn write_frame(&mut self, frame: &[u8]) -> Result<(), SessionError> {
        let stdin = self.stdin.as_mut().ok_or_else(|| {
            SessionError::new("provider-stdin-closed", "provider stdin is closed")
        })?;
        stdin
            .write_all(frame)
            .and_then(|_| stdin.flush())
            .map_err(|_| {
                SessionError::new("provider-write-failed", "provider request write failed")
            })
    }

    fn check_limits(&mut self) -> Result<(), SessionError> {
        if self.cancellation.load(Ordering::Acquire) {
            return Err(SessionError::new(
                "provider-cancelled",
                "provider operation cancelled",
            ));
        }
        if self.stdout_overflow.load(Ordering::Acquire)
            || self.output_overflow.load(Ordering::Acquire)
        {
            return Err(SessionError::new(
                "provider-output-limit",
                "provider output exceeded the limit",
            ));
        }
        if self.stderr_overflow.load(Ordering::Acquire) {
            self.refresh_stderr_metrics();
            return Err(SessionError::new(
                "provider-stderr-limit",
                "provider stderr exceeded the limit",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(SessionError::new(
                "provider-timeout",
                "provider deadline exceeded",
            ));
        }
        Ok(())
    }

    fn refresh_stderr_metrics(&mut self) {
        if let Ok(summary) = self.stderr_summary.try_recv() {
            self.metrics.stderr_bytes = summary.bytes;
            self.metrics.stderr_sha256 = summary.sha256;
        }
    }

    fn join_readers(&mut self) {
        if let Some(thread) = self.stdout_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
        if let Ok(summary) = self.stderr_summary.try_recv() {
            self.metrics.stderr_bytes = summary.bytes;
            self.metrics.stderr_sha256 = summary.sha256;
        }
        self.metrics.total_output_bytes = self.total_output.load(Ordering::Acquire);
    }

    fn record_invalid(&mut self) -> Result<(), SessionError> {
        self.correlation.observe_invalid().map_err(protocol_error)?;
        self.metrics.invalid_messages += 1;
        Ok(())
    }
}

impl Drop for ManagedLspSession {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn spawn_stdout_reader(
    mut stdout: ChildStdout,
    sender: SyncSender<ReaderEvent>,
    limits: FrameLimits,
    overflow: Arc<AtomicBool>,
    output_budget: OutputBudget,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut decoder = match FrameDecoder::new(limits) {
            Ok(decoder) => decoder,
            Err(_) => {
                let _ = sender.try_send(ReaderEvent::Error("provider-frame-limits-invalid"));
                return;
            }
        };
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            let read = match stdout.read(&mut buffer) {
                Ok(read) => read,
                Err(_) => {
                    let _ = sender.try_send(ReaderEvent::Error("provider-read-failed"));
                    return;
                }
            };
            if read == 0 {
                if decoder.finish().is_err() {
                    let _ = sender.try_send(ReaderEvent::Error("provider-frame-eof"));
                } else {
                    let _ = sender.try_send(ReaderEvent::Eof);
                }
                return;
            }
            let frames = match decoder.push(&buffer[..read]) {
                Ok(frames) => frames,
                Err(error) => {
                    let _ = sender.try_send(ReaderEvent::Error(error.code));
                    return;
                }
            };
            for frame in frames {
                if !output_budget.observe(frame.len()) {
                    return;
                }
                match sender.try_send(ReaderEvent::Frame(frame)) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                        overflow.store(true, Ordering::Release);
                        return;
                    }
                }
            }
        }
    })
}

fn spawn_stderr_reader(
    mut stderr: impl Read + Send + 'static,
    max_stderr_bytes: usize,
    sender: SyncSender<StderrSummary>,
    overflow: Arc<AtomicBool>,
    output_budget: OutputBudget,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut retained = Vec::with_capacity(max_stderr_bytes.saturating_add(1));
        let mut digest = Sha256::new();
        let mut total = 0_usize;
        let mut buffer = [0_u8; 8 * 1024];
        while let Ok(read) = stderr.read(&mut buffer) {
            if read == 0 {
                break;
            }
            total = total.saturating_add(read);
            output_budget.observe(read);
            digest.update(&buffer[..read]);
            if retained.len() < max_stderr_bytes.saturating_add(1) {
                let remaining = max_stderr_bytes.saturating_add(1) - retained.len();
                retained.extend_from_slice(&buffer[..read.min(remaining)]);
            }
            if total > max_stderr_bytes {
                overflow.store(true, Ordering::Release);
            }
        }
        let _ = sender.try_send(StderrSummary {
            bytes: retained.len(),
            sha256: format!("{:x}", digest.finalize()),
        });
    })
}

fn protocol_error(error: super::json_rpc::ProtocolError) -> SessionError {
    SessionError::new(error.code, "provider JSON-RPC protocol operation failed")
}

fn server_id_value(id: &ServerRequestId) -> Value {
    match id {
        ServerRequestId::Number(value) => Value::Number((*value).into()),
        ServerRequestId::String(value) => Value::String(value.clone()),
    }
}
