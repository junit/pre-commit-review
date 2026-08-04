#![cfg(feature = "test-fixture")]

use collect_diff_context_cli::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
use collect_diff_context_cli::provider_resources::{
    ProviderResourcePolicy, ResourceAccountingStatus, PRODUCTION_PROCESS_TREE_RSS_LIMIT_BYTES,
};
use collect_diff_context_cli::repository_context_provider::contract::{
    AuthorizedProviderProfile, CallDirection, CandidateBinding, ProviderBinding, ProviderHardening,
    ProviderLimits, ProviderRange, ProviderRangeFormat, RepositoryContextProviderRequest,
    RepositoryContextProviderStatus, RustAnalyzerCrate, RustAnalyzerProjectModel, SeedKind,
    SeedSymbol,
};
use collect_diff_context_cli::repository_context_provider::session::{
    ManagedLspSession, SessionLaunch,
};
use collect_diff_context_cli::repository_context_provider::snapshot::BoundCandidateSnapshot;
use collect_diff_context_cli::repository_context_provider::{
    run_repository_context_provider_with_resource_policy, ProviderInvocation,
};
use collect_diff_context_cli::review_scope::ReviewSource;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

const TEST_RSS_LIMIT: u64 = 32 * 1024 * 1024;

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
        bound: &'a BoundCandidateSnapshot<'a>,
        scenario: &str,
        log: &Path,
        extra: Option<&str>,
    ) -> SessionLaunch<'a> {
        let mut arguments = vec![scenario.to_string(), log.to_string_lossy().into_owned()];
        if let Some(extra) = extra {
            arguments.push(extra.to_string());
        }
        let arguments = Box::leak(arguments.into_boxed_slice());
        let limits = Box::leak(Box::new(ProviderLimits {
            deadline_ms: 5_000,
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
        SessionLaunch {
            snapshot: bound,
            executable: &self.executable,
            executable_sha256: &self.executable_sha256,
            arguments,
            source: ReviewSource::Staged,
            scope_fingerprint: &self.binding.scope_fingerprint,
            limits,
            cancellation: Arc::new(AtomicBool::new(false)),
        }
    }

    fn runner_input(&self) -> (RepositoryContextProviderRequest, AuthorizedProviderProfile) {
        let mut profile = AuthorizedProviderProfile {
            schema_version: 1,
            kind: "repository_context_provider_profile".to_string(),
            provider_kind: "rust-analyzer".to_string(),
            provider_version: "fixture".to_string(),
            executable_sha256: self.executable_sha256.clone(),
            configuration_sha256: digest('0'),
            target_triple: self.model.target_triple.clone(),
            toolchain_mode: "none".to_string(),
            arguments: Vec::new(),
            hardening: ProviderHardening {
                cargo_build_scripts: false,
                cargo_no_deps: true,
                cargo_sysroot: None,
                cargo_sysroot_src: None,
                proc_macro: false,
                check_on_save: false,
                workspace_discovery: false,
                empty_path: true,
                server_status_notification: true,
            },
            maximum_limits: ProviderLimits::maximum(),
        };
        profile.configuration_sha256 = profile.canonical_configuration_sha256();
        let profile_path = self.tools.path().join("runner-profile.json");
        fs::write(&profile_path, serde_json::to_vec(&profile).unwrap()).unwrap();
        let request = RepositoryContextProviderRequest {
            schema_version: 1,
            kind: "repository_context_provider_request".to_string(),
            candidate: self.binding.clone(),
            provider: ProviderBinding {
                kind: profile.provider_kind.clone(),
                version: profile.provider_version.clone(),
                profile_path,
                profile_sha256: profile.sha256(),
                executable_path: self.executable.clone(),
                executable_sha256: profile.executable_sha256.clone(),
                configuration_sha256: profile.configuration_sha256.clone(),
                target_triple: profile.target_triple.clone(),
                toolchain_mode: profile.toolchain_mode.clone(),
            },
            seeds: vec![seed()],
            directions: vec![CallDirection::Incoming, CallDirection::Outgoing],
            limits: ProviderLimits {
                deadline_ms: 5_000,
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
            },
        };
        (request, profile)
    }
}

fn seed() -> SeedSymbol {
    SeedSymbol {
        changed_symbol_id: digest('5'),
        path: "src/lib.rs".to_string(),
        kind: SeedKind::Function,
        name: "seed".to_string(),
        symbol_range: ProviderRange {
            format: ProviderRangeFormat::Utf8ByteColumnsEndExclusiveV1,
            start_line: 1,
            start_column: 1,
            end_line: 1,
            end_column: 22,
            start_byte: 0,
            end_byte: 21,
        },
        selection_range: ProviderRange {
            format: ProviderRangeFormat::Utf8ByteColumnsEndExclusiveV1,
            start_line: 1,
            start_column: 8,
            end_line: 1,
            end_column: 12,
            start_byte: 7,
            end_byte: 11,
        },
        query_byte: 8,
    }
}

#[test]
fn rss_limit_terminates_descendants_without_retaining_output() {
    let fixture = Fixture::new();
    let log = fixture.tools.path().join("rss.log");
    let marker = fixture.tools.path().join("rss.marker");
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch(
        &bound,
        "spawn-descendant-rss",
        &log,
        Some(marker.to_str().unwrap()),
    );
    let policy =
        ProviderResourcePolicy::for_test(TEST_RSS_LIMIT, Duration::from_millis(10)).unwrap();
    let mut session = ManagedLspSession::spawn_with_resource_policy(launch, policy).unwrap();

    let error = session.next_message().unwrap_err();
    assert_eq!(error.code, "process-tree-rss-limit");
    assert!(session.metrics().process_tree_peak_rss_bytes > TEST_RSS_LIMIT);
    assert!(session.metrics().process_tree_sample_interval_ms <= 100);
    assert_eq!(
        session.metrics().process_tree_accounting,
        ResourceAccountingStatus::Available
    );
    session.terminate();
    #[cfg(unix)]
    let descendant_pid = fs::read_to_string(&log).unwrap().parse::<i32>().unwrap();
    for _ in 0..20 {
        #[cfg(unix)]
        if !process_exists(descendant_pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    #[cfg(unix)]
    assert!(!process_exists(descendant_pid));
    assert!(!marker.exists());
}

#[cfg(unix)]
fn process_exists(pid: i32) -> bool {
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[test]
fn test_resource_policy_cannot_raise_the_production_rss_limit() {
    assert!(ProviderResourcePolicy::for_test(
        PRODUCTION_PROCESS_TREE_RSS_LIMIT_BYTES + 1,
        Duration::from_millis(10),
    )
    .is_err());
}

#[test]
fn test_resource_policy_rejects_submillisecond_interval_overflow() {
    assert!(ProviderResourcePolicy::for_test(
        TEST_RSS_LIMIT,
        Duration::from_millis(100) + Duration::from_nanos(1),
    )
    .is_err());
}

#[test]
fn normal_session_reports_bounded_process_tree_metrics() {
    let fixture = Fixture::new();
    let log = fixture.tools.path().join("lifecycle.log");
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch(&bound, "lifecycle", &log, None);
    let policy =
        ProviderResourcePolicy::for_test(512 * 1024 * 1024, Duration::from_millis(10)).unwrap();
    let mut session = ManagedLspSession::spawn_with_resource_policy(launch, policy).unwrap();

    let id = session
        .send_request("initialize", json!({"jsonrpc":"2.0"}))
        .unwrap();
    let response = session.next_message().unwrap();
    assert!(
        matches!(response, collect_diff_context_cli::repository_context_provider::json_rpc::InboundMessage::Response(response) if response.id == id)
    );
    session.send_notification("initialized", json!({})).unwrap();
    session.shutdown_and_reap().unwrap();

    assert!(session.metrics().process_tree_peak_rss_bytes > 0);
    assert!(session.metrics().process_tree_sample_interval_ms <= 100);
    assert_eq!(
        session.metrics().process_tree_accounting,
        ResourceAccountingStatus::Available
    );
}

#[test]
fn shutdown_rechecks_rss_after_the_monitor_terminates_the_child() {
    let fixture = Fixture::new();
    let log = fixture.tools.path().join("shutdown-rss.log");
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch(&bound, "lifecycle-rss-after-exit", &log, None);
    let policy =
        ProviderResourcePolicy::for_test(TEST_RSS_LIMIT, Duration::from_millis(10)).unwrap();
    let mut session = ManagedLspSession::spawn_with_resource_policy(launch, policy).unwrap();

    let id = session
        .send_request("initialize", json!({"jsonrpc":"2.0"}))
        .unwrap();
    let response = session.next_message().unwrap();
    assert!(
        matches!(response, collect_diff_context_cli::repository_context_provider::json_rpc::InboundMessage::Response(response) if response.id == id)
    );
    session.send_notification("initialized", json!({})).unwrap();

    let error = session.shutdown_and_reap().unwrap_err();
    assert_eq!(error.code, "process-tree-rss-limit");
    assert!(session.metrics().process_tree_peak_rss_bytes > TEST_RSS_LIMIT);
}

#[test]
fn root_exit_does_not_stop_accounting_for_a_live_rss_descendant() {
    let fixture = Fixture::new();
    let log = fixture.tools.path().join("root-exit-rss.log");
    let gate = fixture.tools.path().join("root-exit-rss.gate");
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch(
        &bound,
        "root-exit-descendant-rss",
        &log,
        Some(gate.to_str().unwrap()),
    );
    let policy =
        ProviderResourcePolicy::for_test(TEST_RSS_LIMIT, Duration::from_millis(10)).unwrap();
    let mut session = ManagedLspSession::spawn_with_resource_policy(launch, policy).unwrap();
    fs::write(gate, b"release root fixture").unwrap();

    let error = session.next_message().unwrap_err();
    assert_eq!(error.code, "process-tree-rss-limit");
    assert!(session.metrics().process_tree_peak_rss_bytes > TEST_RSS_LIMIT);
}

#[cfg(target_os = "linux")]
#[test]
fn rss_limit_tracks_a_descendant_that_escapes_the_original_process_group() {
    let fixture = Fixture::new();
    let log = fixture.tools.path().join("detached-rss.log");
    let marker = fixture.tools.path().join("detached-rss.marker");
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch(
        &bound,
        "spawn-detached-descendant-rss",
        &log,
        Some(marker.to_str().unwrap()),
    );
    let policy =
        ProviderResourcePolicy::for_test(TEST_RSS_LIMIT, Duration::from_millis(10)).unwrap();
    let mut session = ManagedLspSession::spawn_with_resource_policy(launch, policy).unwrap();

    let error = session.next_message().unwrap_err();
    assert_eq!(error.code, "process-tree-rss-limit");
    session.terminate();
    let descendant_pid = fs::read_to_string(&log).unwrap().parse::<i32>().unwrap();
    for _ in 0..20 {
        if !process_exists(descendant_pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!process_exists(descendant_pid));
    assert!(!marker.exists());
}

#[test]
fn unavailable_accounting_fails_the_session_gate() {
    let fixture = Fixture::new();
    let log = fixture.tools.path().join("unavailable.log");
    let bound =
        BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding).unwrap();
    let launch = fixture.launch(&bound, "hang", &log, None);
    let policy = ProviderResourcePolicy::unavailable_for_test(Duration::from_millis(10)).unwrap();

    let error = ManagedLspSession::spawn_with_resource_policy(launch, policy)
        .err()
        .expect("unavailable accounting must reject the session");

    assert_eq!(error.code, "process-tree-rss-accounting-unavailable");
}

#[test]
fn public_runner_preserves_unavailable_accounting_as_a_failed_report() {
    let fixture = Fixture::new();
    let (request, profile) = fixture.runner_input();
    request.validate().unwrap();
    profile.validate_request(&request).unwrap();
    fixture.model.validate().unwrap();
    let policy = ProviderResourcePolicy::unavailable_for_test(Duration::from_millis(10)).unwrap();

    let report = run_repository_context_provider_with_resource_policy(
        ProviderInvocation {
            snapshot: &fixture.snapshot,
            model: &fixture.model,
            request: &request,
            profile: &profile,
            cancellation: Arc::new(AtomicBool::new(false)),
        },
        policy,
    )
    .unwrap();

    assert_eq!(report.status, RepositoryContextProviderStatus::Failed);
    assert_eq!(
        report.limitations[0].code,
        "process-tree-rss-accounting-unavailable"
    );
    assert!(report.seed_symbols.is_empty());
    assert!(report.related_symbols.is_empty());
    assert!(report.edges.is_empty());
    assert_eq!(
        report.metrics.process_tree_accounting,
        ResourceAccountingStatus::Unavailable
    );
}

#[test]
fn public_runner_releases_no_facts_after_process_tree_rss_limit() {
    let fixture = Fixture::new();
    let (mut request, profile) = fixture.runner_input();
    request.candidate.scope_fingerprint = digest('7');
    request.validate().unwrap();
    profile.validate_request(&request).unwrap();
    fixture.model.validate().unwrap();
    let policy =
        ProviderResourcePolicy::for_test(TEST_RSS_LIMIT, Duration::from_millis(10)).unwrap();

    let report = run_repository_context_provider_with_resource_policy(
        ProviderInvocation {
            snapshot: &fixture.snapshot,
            model: &fixture.model,
            request: &request,
            profile: &profile,
            cancellation: Arc::new(AtomicBool::new(false)),
        },
        policy,
    )
    .unwrap();

    assert_eq!(report.status, RepositoryContextProviderStatus::Failed);
    assert_eq!(report.limitations[0].code, "process-tree-rss-limit");
    assert!(report.seed_symbols.is_empty());
    assert!(report.related_symbols.is_empty());
    assert!(report.edges.is_empty());
    assert!(report.metrics.process_tree_peak_rss_bytes > TEST_RSS_LIMIT);
    assert!(report.metrics.process_tree_sample_interval_ms <= 100);
    assert_eq!(
        report.metrics.process_tree_accounting,
        ResourceAccountingStatus::Available
    );
}
