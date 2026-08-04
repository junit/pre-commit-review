#![cfg(feature = "test-fixture")]

use collect_diff_context_cli::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
use collect_diff_context_cli::repository_context_provider::contract::{
    AuthorizedProviderProfile, CallDirection, CandidateBinding, PositionEncoding, ProviderBinding,
    ProviderHardening, ProviderLimits, ProviderRange, ProviderRangeFormat,
    RepositoryContextProviderRequest, RepositoryContextProviderStatus, RustAnalyzerCrate,
    RustAnalyzerProjectModel, SeedKind, SeedSymbol,
};
use collect_diff_context_cli::repository_context_provider::rust_analyzer::{
    initialize_and_gate, traverse_call_hierarchy, CallHierarchyTraversal, Readiness,
    RustAnalyzerHandshakeError,
};
use collect_diff_context_cli::repository_context_provider::session::{
    ManagedLspSession, SessionLaunch,
};
use collect_diff_context_cli::repository_context_provider::snapshot::BoundCandidateSnapshot;
use collect_diff_context_cli::repository_context_provider::{
    run_repository_context_provider, run_repository_context_provider_measured,
    run_repository_context_provider_with_postflight_elapsed_ms,
    run_repository_context_provider_with_postflight_snapshot_hook, ProviderInvocation,
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
use std::time::{Duration, Instant};
use tempfile::TempDir;

static RESOURCE_INTENSIVE_RUNNER_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_resource_intensive_runner_test() -> std::sync::MutexGuard<'static, ()> {
    RESOURCE_INTENSIVE_RUNNER_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn public_runner_measurement_is_observed_after_final_validation() {
    let _guard = lock_resource_intensive_runner_test();
    let fixture = Fixture::new();
    let (mut request, profile) = fixture.runner_input();
    request.candidate.scope_fingerprint = digest('b');
    request.limits.deadline_ms = 5_000;

    let measured = run_repository_context_provider_measured(ProviderInvocation {
        snapshot: &fixture.snapshot,
        model: &fixture.model,
        request: &request,
        profile: &profile,
        cancellation: Arc::new(AtomicBool::new(false)),
    })
    .unwrap();

    assert_eq!(measured.elapsed_ms, measured.report.metrics.elapsed_ms);
    assert_eq!(
        measured.report.metrics.report_bytes,
        serde_json::to_vec(&measured.report).unwrap().len()
    );
    measured.report.validate().unwrap();
}

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
        Self::new_with_snapshot_noise(0)
    }

    fn new_with_snapshot_noise(snapshot_noise_files: usize) -> Self {
        let repository = TempDir::new().unwrap();
        git(repository.path(), &["init", "-q"]);
        fs::create_dir_all(repository.path().join("src")).unwrap();
        fs::write(
            repository.path().join("src/lib.rs"),
            b"pub fn seed() { caller(); }\npub fn caller() { seed(); }\npub fn callee() {}\n",
        )
        .unwrap();
        if snapshot_noise_files > 0 {
            let noise = repository.path().join("postflight-noise");
            fs::create_dir(&noise).unwrap();
            for index in 0..snapshot_noise_files {
                fs::write(noise.join(format!("{index:04}")), b"").unwrap();
            }
        }
        git(repository.path(), &["add", "--", "."]);
        let snapshot = CandidateSnapshot::materialize(
            repository.path(),
            ReviewSource::Staged,
            SnapshotLimits {
                max_files: snapshot_noise_files.saturating_add(10),
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

    fn run_graph(&self) -> CallHierarchyTraversal {
        self.run_graph_scenario("graph")
    }

    fn run_graph_scenario(&self, scenario: &str) -> CallHierarchyTraversal {
        let bound =
            BoundCandidateSnapshot::new(&self.snapshot, &self.model, &self.binding).unwrap();
        let profile_path = self.tools.path().join("profile.json");
        fs::write(&profile_path, b"fixture-profile").unwrap();
        let request_provider = ProviderBinding {
            kind: "rust-analyzer".to_string(),
            version: "fixture".to_string(),
            profile_path,
            profile_sha256: digest('3'),
            executable_path: self.executable.clone(),
            executable_sha256: self.executable_sha256.clone(),
            configuration_sha256: digest('4'),
            target_triple: self.model.target_triple.clone(),
            toolchain_mode: "none".to_string(),
        };
        let request = collect_diff_context_cli::repository_context_provider::contract::RepositoryContextProviderRequest {
            schema_version: 1,
            kind: "repository_context_provider_request".to_string(),
            candidate: self.binding.clone(),
            provider: request_provider,
            seeds: vec![SeedSymbol {
                changed_symbol_id: digest('5'),
                path: "src/lib.rs".to_string(),
                kind: SeedKind::Function,
                name: "seed".to_string(),
                symbol_range: ProviderRange {
                    format: ProviderRangeFormat::Utf8ByteColumnsEndExclusiveV1,
                    start_line: 1,
                    start_column: 1,
                    end_line: 1,
                    end_column: 27,
                    start_byte: 0,
                    end_byte: 26,
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
            }],
            directions: vec![CallDirection::Incoming, CallDirection::Outgoing],
            limits: graph_limits(),
        };
        request.validate().unwrap();
        let binding_digest = request.binding_digest(&self.model.algorithm).unwrap();
        let arguments = Box::leak(
            vec![
                scenario.to_string(),
                self.tools
                    .path()
                    .join(format!("{scenario}.log"))
                    .to_string_lossy()
                    .into_owned(),
            ]
            .into_boxed_slice(),
        );
        let limits = Box::leak(Box::new(graph_limits()));
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
        let handshake =
            initialize_and_gate(&mut session, &bound, &self.model, &self.model.target_triple)
                .unwrap();
        let result = traverse_call_hierarchy(
            &mut session,
            &bound,
            &request.seeds,
            &request.directions,
            limits,
            handshake.position_encoding,
            &binding_digest,
            "rust-analyzer",
            "fixture",
        )
        .unwrap();
        session.terminate();
        result
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
            seeds: vec![graph_seed()],
            directions: vec![CallDirection::Incoming, CallDirection::Outgoing],
            limits: graph_limits(),
        };
        (request, profile)
    }
}

fn graph_seed() -> SeedSymbol {
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
            end_column: 27,
            start_byte: 0,
            end_byte: 26,
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

fn graph_limits() -> ProviderLimits {
    ProviderLimits {
        deadline_ms: 2_000,
        max_depth: 2,
        max_seeds: 1,
        max_requests: 64,
        max_pending_requests: 1,
        max_messages: 256,
        max_notifications: 64,
        max_server_requests: 32,
        max_invalid_messages: 4,
        max_call_ranges: 64,
        max_header_bytes: 4096,
        max_frame_bytes: 64 * 1024,
        max_protocol_bytes: 512 * 1024,
        max_stderr_bytes: 1024,
        max_total_output_bytes: 2 * 1024 * 1024,
        max_source_file_bytes: 4096,
        max_source_bytes: 4096,
        max_nodes: 16,
        max_edges: 32,
        max_report_bytes: 64 * 1024,
    }
}

fn configure_large_report_request(request: &mut RepositoryContextProviderRequest) {
    request.candidate.scope_fingerprint = digest('8');
    request.seeds[0].name = "large-seed".to_string();
    request.directions = vec![CallDirection::Incoming, CallDirection::Outgoing];
    request.limits.deadline_ms = 20_000;
    request.limits.max_depth = 1;
    request.limits.max_requests = 16;
    request.limits.max_messages = 64;
    request.limits.max_call_ranges = 1_000;
    request.limits.max_frame_bytes = 4 * 1024 * 1024;
    request.limits.max_protocol_bytes = 16 * 1024 * 1024;
    request.limits.max_total_output_bytes = 16 * 1024 * 1024;
    request.limits.max_nodes = 1_001;
    request.limits.max_edges = 1_000;
    request.limits.max_report_bytes = 16 * 1024 * 1024;
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

#[test]
fn call_hierarchy_bfs_deduplicates_edges_and_is_deterministic() {
    let fixture = Fixture::new();
    let first = fixture.run_graph();
    let second = fixture.run_graph();
    assert_eq!(first, second);
    assert_eq!(first.seed_symbols.len(), 1);
    assert!(first
        .related_symbols
        .iter()
        .any(|symbol| symbol.name == "caller"));
    assert!(first
        .related_symbols
        .iter()
        .any(|symbol| symbol.name == "callee"));
    assert!(first
        .edges
        .iter()
        .any(|edge| { edge.from_symbol != edge.to_symbol && edge.call_site_path == "src/lib.rs" }));
    assert_eq!(
        first.edges.len(),
        first
            .edges
            .iter()
            .map(|edge| &edge.edge_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );
    assert!(first
        .edges
        .windows(2)
        .all(|edges| edges[0].edge_id < edges[1].edge_id));
}

#[test]
fn call_hierarchy_retries_transient_empty_seed_resolution() {
    let traversal = Fixture::new().run_graph_scenario("graph-transient-empty");
    assert_eq!(traversal.seed_symbols.len(), 1);
    assert!(!traversal.edges.is_empty());
    assert!(traversal
        .limitations
        .iter()
        .all(|limitation| limitation.code != "seed-unresolved"));
}

#[test]
fn graph_warning_does_not_select_large_report_from_a_scope_digest_prefix() {
    let mut fixture = Fixture::new();
    fixture.binding.scope_fingerprint = format!("8{}", "0".repeat(63));

    let traversal = fixture.run_graph_scenario("graph-warning");

    assert_eq!(traversal.seed_symbols.len(), 1);
    assert!(!traversal.edges.is_empty());
}

#[test]
fn public_runner_returns_bound_completed_report() {
    let fixture = Fixture::new();
    let (request, profile) = fixture.runner_input();
    request.validate().unwrap();
    let report = run_repository_context_provider(ProviderInvocation {
        snapshot: &fixture.snapshot,
        model: &fixture.model,
        request: &request,
        profile: &profile,
        cancellation: Arc::new(AtomicBool::new(false)),
    })
    .unwrap();
    assert_eq!(report.status, RepositoryContextProviderStatus::Completed);
    assert!(!report.seed_symbols.is_empty());
    assert!(!report.edges.is_empty());
    report.validate().unwrap();
    assert!(!serde_json::to_string(&report)
        .unwrap()
        .contains(fixture.snapshot.path().to_str().unwrap()));
    assert_eq!(
        report.metrics.report_bytes,
        serde_json::to_vec(&report).unwrap().len()
    );
}

#[cfg(unix)]
#[test]
fn public_runner_elapsed_includes_final_report_processing() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_resource_intensive_runner_test();
    let fixture = Fixture::new();
    let (mut request, profile) = fixture.runner_input();
    configure_large_report_request(&mut request);

    let baseline_executable = fixture.tools.path().join("large-report-preflight-only");
    fs::copy(&fixture.executable, &baseline_executable).unwrap();
    fs::set_permissions(&baseline_executable, fs::Permissions::from_mode(0o600)).unwrap();
    let mut baseline_request = request.clone();
    baseline_request.provider.executable_path = baseline_executable;
    let preflight_started = Instant::now();
    let preflight_error = run_repository_context_provider(ProviderInvocation {
        snapshot: &fixture.snapshot,
        model: &fixture.model,
        request: &baseline_request,
        profile: &profile,
        cancellation: Arc::new(AtomicBool::new(false)),
    })
    .unwrap_err();
    let preflight_elapsed = preflight_started.elapsed();
    assert_eq!(
        preflight_error,
        collect_diff_context_cli::repository_context_provider::ProviderError::Preflight
    );

    let started = Instant::now();
    let report = run_repository_context_provider(ProviderInvocation {
        snapshot: &fixture.snapshot,
        model: &fixture.model,
        request: &request,
        profile: &profile,
        cancellation: Arc::new(AtomicBool::new(false)),
    })
    .unwrap();
    let wall_elapsed = started.elapsed();

    assert_eq!(
        report.status,
        RepositoryContextProviderStatus::Completed,
        "limitations: {:?}",
        report.limitations
    );
    report.validate().unwrap();
    assert_eq!(report.related_symbols.len(), 1_000);
    assert_eq!(report.edges.len(), 1_000);
    assert_eq!(
        report.metrics.report_bytes,
        serde_json::to_vec(&report).unwrap().len()
    );
    let unaccounted = wall_elapsed
        .saturating_sub(preflight_elapsed)
        .saturating_sub(Duration::from_millis(report.metrics.elapsed_ms));
    assert!(
        unaccounted < Duration::from_millis(50),
        "final report work was not timed: wall={wall_elapsed:?} preflight={preflight_elapsed:?} report={}ms unaccounted={unaccounted:?}",
        report.metrics.elapsed_ms
    );
}

#[cfg(unix)]
#[test]
fn public_runner_honors_cancellation_during_final_report_processing() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::Ordering;

    let _guard = lock_resource_intensive_runner_test();
    let fixture = Fixture::new();
    let (mut request, mut profile) = fixture.runner_input();
    configure_large_report_request(&mut request);
    let wrapper = fixture.tools.path().join("final-report-provider");
    let marker = fixture.tools.path().join("final-report-started");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n'{}' \"$@\"\nstatus=$?\nprintf done > '{}'\nexit $status\n",
            fixture.executable.display(),
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let executable_sha256 = format!("{:x}", Sha256::digest(fs::read(&wrapper).unwrap()));
    profile.executable_sha256 = executable_sha256.clone();
    request.provider.executable_path = wrapper;
    request.provider.executable_sha256 = executable_sha256;
    request.provider.profile_sha256 = profile.sha256();
    fs::write(
        &request.provider.profile_path,
        serde_json::to_vec(&profile).unwrap(),
    )
    .unwrap();

    let cancellation = Arc::new(AtomicBool::new(false));
    let watched_cancellation = Arc::clone(&cancellation);
    let watcher = std::thread::spawn(move || {
        while !marker.exists() {
            std::thread::yield_now();
        }
        std::thread::sleep(Duration::from_millis(50));
        watched_cancellation.store(true, Ordering::Release);
    });
    let result = run_repository_context_provider(ProviderInvocation {
        snapshot: &fixture.snapshot,
        model: &fixture.model,
        request: &request,
        profile: &profile,
        cancellation,
    });
    watcher.join().unwrap();

    assert_eq!(
        result.unwrap_err(),
        collect_diff_context_cli::repository_context_provider::ProviderError::Cancelled
    );
}

#[cfg(unix)]
#[test]
fn public_runner_rejects_a_symlinked_provider_profile() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let (mut request, profile) = fixture.runner_input();
    let profile_link = fixture.tools.path().join("runner-profile-link.json");
    symlink(&request.provider.profile_path, &profile_link).unwrap();
    request.provider.profile_path = profile_link;

    let error = run_repository_context_provider(ProviderInvocation {
        snapshot: &fixture.snapshot,
        model: &fixture.model,
        request: &request,
        profile: &profile,
        cancellation: Arc::new(AtomicBool::new(false)),
    })
    .unwrap_err();

    assert_eq!(
        error,
        collect_diff_context_cli::repository_context_provider::ProviderError::Preflight
    );
}

#[cfg(unix)]
#[test]
fn public_runner_rejects_an_oversized_provider_executable_without_streaming_it() {
    let fixture = Fixture::new();
    let (mut request, profile) = fixture.runner_input();
    let oversized_executable = fixture.tools.path().join("oversized-provider");
    let file = fs::File::create(&oversized_executable).unwrap();
    file.set_len(512 * 1024 * 1024 + 1).unwrap();
    request.provider.executable_path = oversized_executable;

    let started = Instant::now();
    let error = run_repository_context_provider(ProviderInvocation {
        snapshot: &fixture.snapshot,
        model: &fixture.model,
        request: &request,
        profile: &profile,
        cancellation: Arc::new(AtomicBool::new(false)),
    })
    .unwrap_err();
    let elapsed = started.elapsed();

    assert_eq!(
        error,
        collect_diff_context_cli::repository_context_provider::ProviderError::Preflight
    );
    assert!(
        elapsed < Duration::from_millis(500),
        "oversized executable was streamed for {elapsed:?}"
    );
}

#[test]
fn public_runner_rejects_provider_metadata_changes_during_a_bounded_read() {
    use std::io::{Seek, SeekFrom, Write};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::Ordering;
    use std::sync::Barrier;

    let _guard = lock_resource_intensive_runner_test();
    let fixture = Fixture::new();
    let (mut request, mut profile) = fixture.runner_input();
    let wrapper = fixture.tools.path().join(if cfg!(windows) {
        "mutable-provider.exe"
    } else {
        "mutable-provider"
    });
    #[cfg(unix)]
    {
        fs::write(
            &wrapper,
            format!("#!/bin/sh\nexec '{}'\n", fixture.executable.display()),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[cfg(windows)]
    fs::copy(&fixture.executable, &wrapper).unwrap();
    let file = fs::OpenOptions::new().write(true).open(&wrapper).unwrap();
    file.set_len(file.metadata().unwrap().len().max(8 * 1024 * 1024))
        .unwrap();
    let executable_sha256 = format!("{:x}", Sha256::digest(fs::read(&wrapper).unwrap()));
    profile.executable_sha256 = executable_sha256.clone();
    request.provider.executable_path = wrapper.clone();
    request.provider.executable_sha256 = executable_sha256;
    request.provider.profile_sha256 = profile.sha256();
    request.limits.deadline_ms = 5_000;
    fs::write(
        &request.provider.profile_path,
        serde_json::to_vec(&profile).unwrap(),
    )
    .unwrap();

    let barrier = Arc::new(Barrier::new(2));
    let mutator_barrier = Arc::clone(&barrier);
    let stop = Arc::new(AtomicBool::new(false));
    let mutator_stop = Arc::clone(&stop);
    let mutator = std::thread::spawn(move || {
        let mut file = fs::OpenOptions::new().write(true).open(wrapper).unwrap();
        mutator_barrier.wait();
        while !mutator_stop.load(Ordering::Acquire) {
            file.seek(SeekFrom::End(-1)).unwrap();
            file.write_all(&[0]).unwrap();
        }
    });
    barrier.wait();
    let result = run_repository_context_provider(ProviderInvocation {
        snapshot: &fixture.snapshot,
        model: &fixture.model,
        request: &request,
        profile: &profile,
        cancellation: Arc::new(AtomicBool::new(false)),
    });
    stop.store(true, Ordering::Release);
    mutator.join().unwrap();

    assert_eq!(
        result.unwrap_err(),
        collect_diff_context_cli::repository_context_provider::ProviderError::Preflight
    );
}

#[cfg(unix)]
#[test]
fn public_runner_honors_cancellation_during_postflight_provider_reads() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::Ordering;

    let _guard = lock_resource_intensive_runner_test();
    let fixture = Fixture::new();
    let (mut request, mut profile) = fixture.runner_input();
    let wrapper = fixture.tools.path().join("postflight-provider");
    let marker = fixture.tools.path().join("postflight-started");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n'{}' \"$@\"\nprintf done > '{}'\n",
            fixture.executable.display(),
            marker.display()
        ),
    )
    .unwrap();
    let file = fs::OpenOptions::new().write(true).open(&wrapper).unwrap();
    file.set_len(8 * 1024 * 1024).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let executable_sha256 = format!("{:x}", Sha256::digest(fs::read(&wrapper).unwrap()));
    profile.executable_sha256 = executable_sha256.clone();
    request.provider.executable_path = wrapper;
    request.provider.executable_sha256 = executable_sha256;
    request.provider.profile_sha256 = profile.sha256();
    request.limits.deadline_ms = 5_000;
    fs::write(
        &request.provider.profile_path,
        serde_json::to_vec(&profile).unwrap(),
    )
    .unwrap();

    let cancellation = Arc::new(AtomicBool::new(false));
    let watched_cancellation = Arc::clone(&cancellation);
    let watcher = std::thread::spawn(move || {
        while !marker.exists() {
            std::thread::yield_now();
        }
        std::thread::sleep(Duration::from_millis(50));
        watched_cancellation.store(true, Ordering::Release);
    });
    let result = run_repository_context_provider(ProviderInvocation {
        snapshot: &fixture.snapshot,
        model: &fixture.model,
        request: &request,
        profile: &profile,
        cancellation,
    });
    watcher.join().unwrap();

    assert_eq!(
        result.unwrap_err(),
        collect_diff_context_cli::repository_context_provider::ProviderError::Cancelled
    );
}

#[cfg(unix)]
#[test]
fn public_runner_prioritizes_cancellation_after_stale_snapshot_validation() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::Ordering;

    let _guard = lock_resource_intensive_runner_test();
    let fixture = Fixture::new_with_snapshot_noise(1);
    let (mut request, mut profile) = fixture.runner_input();
    let wrapper = fixture.tools.path().join("stale-snapshot-provider");
    let mutation_marker = fixture.tools.path().join("stale-snapshot-mutation-ready");
    let mutation_done = fixture.tools.path().join("stale-snapshot-mutation-done");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n'{}' \"$@\"\nprintf ready > '{}'\nwhile [ ! -f '{}' ]; do :; done\n",
            fixture.executable.display(),
            mutation_marker.display(),
            mutation_done.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let executable_sha256 = format!("{:x}", Sha256::digest(fs::read(&wrapper).unwrap()));
    profile.executable_sha256 = executable_sha256.clone();
    request.provider.executable_path = wrapper;
    request.provider.executable_sha256 = executable_sha256;
    request.provider.profile_sha256 = profile.sha256();
    request.limits.deadline_ms = 30_000;
    fs::write(
        &request.provider.profile_path,
        serde_json::to_vec(&profile).unwrap(),
    )
    .unwrap();

    let cancellation = Arc::new(AtomicBool::new(false));
    let stale_file = fixture.snapshot.path().join("postflight-noise/0000");
    let watcher = std::thread::spawn(move || {
        while !mutation_marker.exists() {
            std::thread::yield_now();
        }
        fs::set_permissions(&stale_file, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&stale_file, b"changed").unwrap();
        fs::set_permissions(&stale_file, fs::Permissions::from_mode(0o444)).unwrap();
        fs::write(mutation_done, b"done").unwrap();
    });
    let postflight_cancellation = Arc::clone(&cancellation);
    let cancel_after_snapshot_validation = || {
        postflight_cancellation.store(true, Ordering::Release);
    };
    let result = run_repository_context_provider_with_postflight_snapshot_hook(
        ProviderInvocation {
            snapshot: &fixture.snapshot,
            model: &fixture.model,
            request: &request,
            profile: &profile,
            cancellation,
        },
        &cancel_after_snapshot_validation,
    );
    watcher.join().unwrap();

    assert_eq!(
        result.unwrap_err(),
        collect_diff_context_cli::repository_context_provider::ProviderError::Cancelled
    );
}

#[cfg(unix)]
#[test]
fn public_runner_timing_excludes_regular_file_preflight() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_resource_intensive_runner_test();
    let fixture = Fixture::new();
    let (mut request, mut profile) = fixture.runner_input();
    let executable = fixture.tools.path().join("timed-provider");
    fs::copy(&fixture.executable, &executable).unwrap();
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&executable)
        .unwrap();
    file.set_len(8 * 1024 * 1024).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let executable_sha256 = format!("{:x}", Sha256::digest(fs::read(&executable).unwrap()));
    profile.executable_sha256 = executable_sha256.clone();
    request.provider.executable_path = executable;
    request.provider.executable_sha256 = executable_sha256;
    request.provider.profile_sha256 = profile.sha256();
    request.limits.deadline_ms = 10_000;
    fs::write(
        &request.provider.profile_path,
        serde_json::to_vec(&profile).unwrap(),
    )
    .unwrap();

    let started = Instant::now();
    let report = run_repository_context_provider(ProviderInvocation {
        snapshot: &fixture.snapshot,
        model: &fixture.model,
        request: &request,
        profile: &profile,
        cancellation: Arc::new(AtomicBool::new(false)),
    })
    .unwrap();
    let wall_elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap();

    assert_eq!(report.status, RepositoryContextProviderStatus::Completed);
    assert!(
        wall_elapsed_ms >= report.metrics.elapsed_ms.saturating_add(20),
        "preflight leaked into provider timing: wall={wall_elapsed_ms}ms report={}ms",
        report.metrics.elapsed_ms
    );
    assert_eq!(
        report.metrics.report_bytes,
        serde_json::to_vec(&report).unwrap().len()
    );
}

#[test]
fn public_runner_rejects_a_postflight_deadline_overrun() {
    let _guard = lock_resource_intensive_runner_test();
    let fixture = Fixture::new();
    let (mut request, profile) = fixture.runner_input();
    request.candidate.scope_fingerprint = digest('b');
    request.limits.deadline_ms = 5_000;

    let error = run_repository_context_provider_with_postflight_elapsed_ms(
        ProviderInvocation {
            snapshot: &fixture.snapshot,
            model: &fixture.model,
            request: &request,
            profile: &profile,
            cancellation: Arc::new(AtomicBool::new(false)),
        },
        request.limits.deadline_ms + 1,
    )
    .unwrap_err();

    assert_eq!(
        error,
        collect_diff_context_cli::repository_context_provider::ProviderError::DeadlineExceeded
    );
}

#[test]
fn public_runner_status_matrix_retains_no_facts_on_terminal_failures() {
    let fixture = Fixture::new();
    let (mut request, profile) = fixture.runner_input();
    request.candidate.scope_fingerprint = digest('a');
    request.limits.deadline_ms = 1_000;
    let report = run_repository_context_provider(ProviderInvocation {
        snapshot: &fixture.snapshot,
        model: &fixture.model,
        request: &request,
        profile: &profile,
        cancellation: Arc::new(AtomicBool::new(false)),
    })
    .unwrap();
    assert_eq!(report.status, RepositoryContextProviderStatus::Timeout);
    report.validate().unwrap();
    assert!(report.seed_symbols.is_empty());
    assert!(report.related_symbols.is_empty());
    assert!(report.edges.is_empty());
    assert!(
        report.metrics.elapsed_ms > request.limits.deadline_ms,
        "timeout elapsed time was truncated: report={}ms deadline={}ms",
        report.metrics.elapsed_ms,
        request.limits.deadline_ms
    );

    for (scenario, expected) in [
        ('b', RepositoryContextProviderStatus::InvalidOutput),
        ('c', RepositoryContextProviderStatus::InvalidOutput),
        ('d', RepositoryContextProviderStatus::Failed),
        ('e', RepositoryContextProviderStatus::Partial),
        ('f', RepositoryContextProviderStatus::Unavailable),
    ] {
        let fixture = Fixture::new();
        let (mut request, profile) = fixture.runner_input();
        request.candidate.scope_fingerprint = digest(scenario);
        request.limits.deadline_ms = 5_000;
        let report = run_repository_context_provider(ProviderInvocation {
            snapshot: &fixture.snapshot,
            model: &fixture.model,
            request: &request,
            profile: &profile,
            cancellation: Arc::new(AtomicBool::new(false)),
        })
        .unwrap();
        assert_eq!(report.status, expected, "scenario {scenario}");
        if expected != RepositoryContextProviderStatus::Partial {
            assert!(report.seed_symbols.is_empty());
            assert!(report.related_symbols.is_empty());
            assert!(report.edges.is_empty());
        } else {
            assert!(!report.seed_symbols.is_empty());
        }
    }
}

#[test]
fn public_runner_rejects_pre_cancelled_invocation() {
    let fixture = Fixture::new();
    let (request, profile) = fixture.runner_input();
    let cancellation = Arc::new(AtomicBool::new(true));
    let error = run_repository_context_provider(ProviderInvocation {
        snapshot: &fixture.snapshot,
        model: &fixture.model,
        request: &request,
        profile: &profile,
        cancellation,
    })
    .unwrap_err();
    assert_eq!(
        error,
        collect_diff_context_cli::repository_context_provider::ProviderError::Cancelled
    );
}

fn _unused_json_value() -> serde_json::Value {
    json!({})
}
