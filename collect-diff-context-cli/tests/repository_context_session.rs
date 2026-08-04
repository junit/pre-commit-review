#![cfg(feature = "test-fixture")]

use collect_diff_context_cli::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
use collect_diff_context_cli::repository_context_provider::contract::{
    CandidateBinding, ProviderLimits, RustAnalyzerCrate, RustAnalyzerProjectModel,
};
use collect_diff_context_cli::repository_context_provider::session::{
    ManagedLspSession, SessionLaunch,
};
use collect_diff_context_cli::repository_context_provider::snapshot::BoundCandidateSnapshot;
use collect_diff_context_cli::review_scope::ReviewSource;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tempfile::TempDir;

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {arguments:?} failed");
}

struct Fixture {
    _repository: TempDir,
    snapshot: CandidateSnapshot,
    model: RustAnalyzerProjectModel,
    binding: CandidateBinding,
    tools: TempDir,
    executable: PathBuf,
    executable_sha256: String,
}

struct LaunchOptions<'a> {
    scenario: &'a str,
    log: &'a Path,
    deadline_ms: u64,
    max_stderr_bytes: usize,
    cancellation: Arc<AtomicBool>,
    extra: Option<&'a str>,
}

impl Fixture {
    fn new() -> Self {
        let repository = TempDir::new().unwrap();
        git(repository.path(), &["init", "-q"]);
        fs::create_dir_all(repository.path().join("src")).unwrap();
        fs::write(repository.path().join("src/lib.rs"), b"pub fn seed() {}\n").unwrap();
        git(repository.path(), &["add", "--", "."]);
        let snapshot = CandidateSnapshot::materialize(
            repository.path(),
            ReviewSource::Staged,
            SnapshotLimits {
                max_files: 10,
                max_bytes: 10_000,
            },
        )
        .unwrap();
        let mut model = RustAnalyzerProjectModel {
            schema_version: 1,
            algorithm: "rust-analyzer-linked-project-v1".to_string(),
            digest: digest('0'),
            target_triple: "x86_64-unknown-linux-gnu".to_string(),
            crates: vec![RustAnalyzerCrate {
                crate_id: "app".to_string(),
                root_module: "src/lib.rs".to_string(),
                edition: "2021".to_string(),
                dependencies: Vec::new(),
            }],
            cfg: Vec::new(),
            env: BTreeMap::new(),
            limitations: Vec::new(),
        };
        model.digest = model.canonical_sha256();
        let binding = CandidateBinding {
            source: ReviewSource::Staged,
            scope_fingerprint: digest('1'),
            candidate_digest: digest('2'),
            snapshot_root: fs::canonicalize(snapshot.path()).unwrap(),
            snapshot_sha256: snapshot.sha256.clone(),
            snapshot_files: snapshot.files,
            snapshot_bytes: snapshot.bytes,
            project_model_digest: model.digest.clone(),
        };
        let executable = PathBuf::from(env!("CARGO_BIN_EXE_repository-context-provider-fixture"));
        let executable_sha256 = format!("{:x}", Sha256::digest(fs::read(&executable).unwrap()));
        Self {
            _repository: repository,
            snapshot,
            model,
            binding,
            tools: TempDir::new().unwrap(),
            executable,
            executable_sha256,
        }
    }

    fn launch<'a>(
        &'a self,
        scenario: &'a str,
        log: &'a Path,
        deadline_ms: u64,
        max_stderr_bytes: usize,
        bound: &'a BoundCandidateSnapshot<'a>,
    ) -> SessionLaunch<'a> {
        self.launch_with_options(
            bound,
            LaunchOptions {
                scenario,
                log,
                deadline_ms,
                max_stderr_bytes,
                cancellation: Arc::new(AtomicBool::new(false)),
                extra: None,
            },
        )
    }

    fn launch_with_options<'a>(
        &'a self,
        bound: &'a BoundCandidateSnapshot<'a>,
        options: LaunchOptions<'a>,
    ) -> SessionLaunch<'a> {
        let mut argument_values = vec![
            options.scenario.to_string(),
            options.log.to_string_lossy().into_owned(),
        ];
        if let Some(extra) = options.extra {
            argument_values.push(extra.to_string());
        }
        let arguments = Box::leak(argument_values.into_boxed_slice());
        let limits = Box::leak(Box::new(ProviderLimits {
            deadline_ms: options.deadline_ms,
            max_depth: 1,
            max_seeds: 1,
            max_requests: 16,
            max_pending_requests: 1,
            max_messages: 64,
            max_notifications: 16,
            max_server_requests: 16,
            max_invalid_messages: 4,
            max_call_ranges: 16,
            max_header_bytes: 4096,
            max_frame_bytes: 64 * 1024,
            max_protocol_bytes: 256 * 1024,
            max_stderr_bytes: options.max_stderr_bytes,
            max_total_output_bytes: 2 * 1024 * 1024,
            max_source_file_bytes: 4096,
            max_source_bytes: 4096,
            max_nodes: 16,
            max_edges: 16,
            max_report_bytes: 64 * 1024,
        }));
        SessionLaunch {
            snapshot: bound,
            executable: &self.executable,
            executable_sha256: &self.executable_sha256,
            arguments,
            source: ReviewSource::Staged,
            scope_fingerprint: &self.binding.scope_fingerprint,
            limits,
            cancellation: options.cancellation,
        }
    }
}

#[test]
fn session_preserves_lifecycle_and_gracefully_reaps_fake_server() {
    let fixture = Fixture::new();
    let log = fixture.tools.path().join("lifecycle.log");
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch("lifecycle", &log, 2_000, 1_024, &bound);
    let mut session = ManagedLspSession::spawn(launch).unwrap();

    let id = session
        .send_request("initialize", json!({"jsonrpc":"2.0"}))
        .unwrap();
    let response = session.next_message().unwrap();
    assert!(
        matches!(response, collect_diff_context_cli::repository_context_provider::json_rpc::InboundMessage::Response(response) if response.id == id)
    );
    session.send_notification("initialized", json!({})).unwrap();
    session.shutdown_and_reap().unwrap();

    assert_eq!(
        fs::read_to_string(log).unwrap(),
        "initialize\ninitialized\nshutdown\nexit\n"
    );
}

#[test]
fn session_handles_server_request_interleaving_without_dropping_it() {
    let fixture = Fixture::new();
    let log = fixture.tools.path().join("interleave.log");
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch("config-requests", &log, 2_000, 1_024, &bound);
    let mut session = ManagedLspSession::spawn(launch).unwrap();
    let initialize_id = session.send_request("initialize", json!({})).unwrap();
    let request = session.next_message().unwrap();
    let request = match request {
        collect_diff_context_cli::repository_context_provider::json_rpc::InboundMessage::Request(request) => request,
        _ => panic!("expected server request"),
    };
    session
        .send_server_result(&request.id, json!([null]))
        .unwrap();
    let response = session.next_message().unwrap();
    assert!(
        matches!(response, collect_diff_context_cli::repository_context_provider::json_rpc::InboundMessage::Response(response) if response.id == initialize_id)
    );
    session.terminate();
}

#[test]
fn session_bounds_stderr_to_limit_plus_one() {
    let fixture = Fixture::new();
    let log = fixture.tools.path().join("stderr.log");
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch("stderr-flood", &log, 2_000, 32, &bound);
    let mut session = ManagedLspSession::spawn(launch).unwrap();
    let error = session.next_message().unwrap_err();
    assert_eq!(error.code, "provider-stderr-limit");
    session.terminate();
    assert_eq!(session.metrics().stderr_bytes, 33);
}

#[test]
fn session_deadline_returns_timeout_and_reaps() {
    let fixture = Fixture::new();
    let log = fixture.tools.path().join("hang.log");
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch("hang", &log, 50, 1_024, &bound);
    let mut session = ManagedLspSession::spawn(launch).unwrap();
    let error = session.next_message().unwrap_err();
    assert_eq!(error.code, "provider-timeout");
    session.terminate();
}

#[test]
fn session_decodes_split_frames_without_blocking_or_loss() {
    let fixture = Fixture::new();
    let log = fixture.tools.path().join("split.log");
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch("split-frame", &log, 2_000, 1_024, &bound);
    let mut session = ManagedLspSession::spawn(launch).unwrap();
    let id = session.send_request("initialize", json!({})).unwrap();
    let response = session.next_message().unwrap();
    assert!(
        matches!(response, collect_diff_context_cli::repository_context_provider::json_rpc::InboundMessage::Response(response) if response.id == id)
    );
    session.terminate();
}

#[test]
fn session_rejects_malformed_frames_and_unknown_response_ids() {
    let fixture = Fixture::new();
    let log = fixture.tools.path().join("malformed.log");
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch("malformed-frame", &log, 2_000, 1_024, &bound);
    let mut session = ManagedLspSession::spawn(launch).unwrap();
    assert_eq!(
        session.next_message().unwrap_err().code,
        "provider-frame-header-invalid"
    );
    session.terminate();

    let log = fixture.tools.path().join("unknown.log");
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch("unknown-id", &log, 2_000, 1_024, &bound);
    let mut session = ManagedLspSession::spawn(launch).unwrap();
    session.send_request("initialize", json!({})).unwrap();
    assert_eq!(
        session.next_message().unwrap_err().code,
        "provider-response-id-invalid"
    );
    session.terminate();
}

#[test]
fn session_cancellation_interrupts_waiting_reader() {
    let fixture = Fixture::new();
    let log = fixture.tools.path().join("cancel.log");
    let token = Arc::new(AtomicBool::new(false));
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch_with_options(
        &bound,
        LaunchOptions {
            scenario: "hang",
            log: &log,
            deadline_ms: 2_000,
            max_stderr_bytes: 1_024,
            cancellation: Arc::clone(&token),
            extra: None,
        },
    );
    let mut session = ManagedLspSession::spawn(launch).unwrap();
    token.store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(
        session.next_message().unwrap_err().code,
        "provider-cancelled"
    );
    session.terminate();
}

#[cfg(unix)]
#[test]
fn session_drop_terminates_fixture_descendants() {
    let fixture = Fixture::new();
    let log = fixture.tools.path().join("descendant.log");
    let marker = fixture.tools.path().join("descendant.marker");
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch_with_options(
        &bound,
        LaunchOptions {
            scenario: "spawn-descendant",
            log: &log,
            deadline_ms: 2_000,
            max_stderr_bytes: 1_024,
            cancellation: Arc::new(AtomicBool::new(false)),
            extra: Some(marker.to_str().unwrap()),
        },
    );
    let session = ManagedLspSession::spawn(launch).unwrap();
    drop(session);
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(!marker.exists());
}
