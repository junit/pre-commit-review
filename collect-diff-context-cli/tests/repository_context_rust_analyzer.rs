#![cfg(feature = "test-fixture")]

use collect_diff_context_cli::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
use collect_diff_context_cli::repository_context_provider::contract::{
    CandidateBinding, PositionEncoding, ProviderLimits, RustAnalyzerCrate, RustAnalyzerProjectModel,
};
use collect_diff_context_cli::repository_context_provider::rust_analyzer::{
    initialize_and_gate, Readiness, RustAnalyzerHandshakeError,
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
    assert!(output.status.success());
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
            cfg: vec!["unix".to_string()],
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

    fn run(
        &self,
        scenario: &str,
    ) -> Result<(PositionEncoding, Readiness), RustAnalyzerHandshakeError> {
        let bound =
            BoundCandidateSnapshot::new(&self.snapshot, &self.model, &self.binding).unwrap();
        let log = self.tools.path().join(format!("{scenario}.log"));
        let arguments = Box::leak(
            vec![scenario.to_string(), log.to_string_lossy().into_owned()].into_boxed_slice(),
        );
        let limits = Box::leak(Box::new(ProviderLimits {
            deadline_ms: if scenario == "readiness-hang" {
                80
            } else {
                2_000
            },
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
            max_stderr_bytes: 1024,
            max_total_output_bytes: 2 * 1024 * 1024,
            max_source_file_bytes: 4096,
            max_source_bytes: 4096,
            max_nodes: 16,
            max_edges: 16,
            max_report_bytes: 64 * 1024,
        }));
        let launch = SessionLaunch {
            snapshot: &bound,
            executable: &self.executable,
            executable_sha256: &self.executable_sha256,
            arguments,
            source: ReviewSource::Staged,
            scope_fingerprint: &self.binding.scope_fingerprint,
            limits,
            cancellation: Arc::new(AtomicBool::new(false)),
        };
        let mut session = ManagedLspSession::spawn(launch).unwrap();
        let result =
            initialize_and_gate(&mut session, &bound, &self.model, &self.model.target_triple);
        session.terminate();
        result.map(|handshake| (handshake.position_encoding, handshake.readiness))
    }
}

#[test]
fn handshake_accepts_ready_server_and_uses_utf8_encoding() {
    let result = Fixture::new().run("readiness-ok").unwrap();
    assert_eq!(result.0, PositionEncoding::Utf8);
    assert_eq!(result.1, Readiness::Healthy);
}

#[test]
fn handshake_warning_is_degraded_but_usable() {
    let result = Fixture::new().run("readiness-warning").unwrap();
    assert_eq!(result.1, Readiness::Warning);
}

#[test]
fn handshake_maps_capability_and_protocol_failures_without_facts() {
    assert_eq!(
        Fixture::new().run("missing-capability").unwrap_err().code,
        "provider-capability-unavailable"
    );
    assert_eq!(
        Fixture::new().run("initialize-error").unwrap_err().code,
        "provider-initialize-failed"
    );
    assert_eq!(
        Fixture::new().run("unknown-encoding").unwrap_err().code,
        "provider-position-encoding-invalid"
    );
    assert_eq!(
        Fixture::new().run("readiness-error").unwrap_err().code,
        "provider-readiness-unavailable"
    );
    assert_eq!(
        Fixture::new().run("readiness-hang").unwrap_err().code,
        "provider-timeout"
    );
}

#[test]
fn handshake_default_encoding_is_utf16() {
    let result = Fixture::new().run("readiness-default-encoding").unwrap();
    assert_eq!(result.0, PositionEncoding::Utf16);
}

#[test]
fn handshake_services_positional_configuration_and_rejects_mixed_registration() {
    assert_eq!(
        Fixture::new().run("readiness-config-requests").unwrap().1,
        Readiness::Healthy
    );
    assert_eq!(
        Fixture::new().run("registration-disallowed").unwrap().1,
        Readiness::Healthy
    );
}

fn _unused_json_value() -> serde_json::Value {
    json!({})
}
