#[allow(dead_code)]
mod support;

use collect_diff_context_cli::repository_context_provider::contract::RustAnalyzerProjectModel;
use collect_diff_context_cli::review_scope::ReviewSource;
use std::error::Error;
use std::process::{Command, Output};
use support::GitRepo;

#[cfg(all(feature = "test-fixture", unix))]
use collect_diff_context_cli::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
#[cfg(all(feature = "test-fixture", unix))]
use collect_diff_context_cli::repository_context_provider::cli_contract::{
    ProviderRegistry, ProviderRegistryEntry, ProviderRunRequest,
};
#[cfg(all(feature = "test-fixture", unix))]
use collect_diff_context_cli::repository_context_provider::contract::{
    AuthorizedProviderProfile, CallDirection, ProviderHardening, ProviderLimits, ProviderRange,
    ProviderRangeFormat, RepositoryContextProviderReport, RepositoryContextProviderStatus,
    SeedKind, SeedSymbol,
};
#[cfg(all(feature = "test-fixture", unix))]
use collect_diff_context_cli::repository_context_provider::model::{
    build_linked_project_model, ProviderModelLimits,
};
#[cfg(all(feature = "test-fixture", unix))]
use sha2::{Digest, Sha256};
#[cfg(all(feature = "test-fixture", unix))]
use std::fs;
#[cfg(all(feature = "test-fixture", unix))]
use std::os::unix::fs::PermissionsExt;
#[cfg(all(feature = "test-fixture", unix))]
use std::path::{Path, PathBuf};
#[cfg(all(feature = "test-fixture", unix))]
use tempfile::TempDir;

fn provider_cli(repository: &GitRepo, arguments: &[&str]) -> Result<Output, Box<dyn Error>> {
    Ok(
        Command::new(env!("CARGO_BIN_EXE_repository-context-provider-cli"))
            .args(arguments)
            .current_dir(repository.path())
            .output()?,
    )
}

#[test]
fn help_and_parser_failures_are_stable() -> Result<(), Box<dyn Error>> {
    let repository = GitRepo::new()?;
    let help = provider_cli(&repository, &["--help"])?;
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout)?;
    assert!(help.contains("repository-context-provider-cli model"));
    assert!(help.contains("repository-context-provider-cli run"));

    let model_help = provider_cli(&repository, &["model", "--help"])?;
    assert!(model_help.status.success());
    let model_help = String::from_utf8(model_help.stdout)?;
    assert!(model_help.contains("--source <staged|unstaged|branch>"));
    assert!(model_help.contains("--max-model-files"));
    assert!(model_help.contains("--max-model-bytes"));

    let run_help = provider_cli(&repository, &["run", "--help"])?;
    assert!(run_help.status.success());
    assert!(String::from_utf8(run_help.stdout)?.contains("--expect-registry-sha256"));

    let scope = "a".repeat(64);
    let invalid_cases = vec![
        vec!["unknown".to_string()],
        vec!["model".to_string(), "--unknown".to_string()],
        vec![
            "model".to_string(),
            "--source".to_string(),
            "staged".to_string(),
        ],
        vec![
            "model".to_string(),
            "--expect-scope".to_string(),
            scope.clone(),
        ],
        vec![
            "model".to_string(),
            "--source".to_string(),
            "working-tree".to_string(),
            "--expect-scope".to_string(),
            scope.clone(),
        ],
        vec![
            "model".to_string(),
            "--source".to_string(),
            "staged".to_string(),
            "--expect-scope".to_string(),
            "A".repeat(64),
        ],
        vec![
            "model".to_string(),
            "--source".to_string(),
            "staged".to_string(),
            "--source=staged".to_string(),
            "--expect-scope".to_string(),
            scope.clone(),
        ],
        vec![
            "model".to_string(),
            "--source".to_string(),
            "staged".to_string(),
            "--expect-scope".to_string(),
            scope.clone(),
            "--max-model-files=0".to_string(),
        ],
        vec![
            "model".to_string(),
            "--source".to_string(),
            "staged".to_string(),
            "--expect-scope".to_string(),
            scope.clone(),
            "--max-model-bytes".to_string(),
        ],
        vec![
            "run".to_string(),
            "--source".to_string(),
            "staged".to_string(),
            "--expect-scope".to_string(),
            scope,
            "--registry".to_string(),
            "relative/registry.json".to_string(),
            "--expect-registry-sha256".to_string(),
            "b".repeat(64),
            "--provider-id".to_string(),
            "local".to_string(),
            "--model".to_string(),
            "/tmp/model.json".to_string(),
            "--expect-model-sha256".to_string(),
            "c".repeat(64),
            "--request".to_string(),
            "/tmp/request.json".to_string(),
        ],
    ];
    for arguments in invalid_cases {
        let arguments = arguments.iter().map(String::as_str).collect::<Vec<_>>();
        let output = provider_cli(&repository, &arguments)?;
        assert_eq!(output.status.code(), Some(2), "arguments: {arguments:?}");
        assert!(output.stdout.is_empty(), "arguments: {arguments:?}");
        let stderr = String::from_utf8(output.stderr)?;
        assert!(
            stderr.starts_with("repository-context-provider-cli: provider-cli-"),
            "arguments: {arguments:?}, stderr: {stderr}"
        );
        assert!(stderr.len() <= 512);
    }
    Ok(())
}

#[test]
fn model_emits_one_compact_deterministic_digest_bound_value() -> Result<(), Box<dyn Error>> {
    let repository = GitRepo::new()?;
    repository.commit_file("README.md", b"base\n")?;
    repository.write(
        "Cargo.toml",
        b"[package]\nname = \"cli-model\"\nedition = \"2021\"\n",
    )?;
    repository.write("src/lib.rs", b"pub fn cli_model() {}\n")?;
    repository.git(["add", "--", "Cargo.toml", "src/lib.rs"])?;
    let scope = repository.scope(ReviewSource::Staged)?;
    let arguments = [
        "model",
        "--source=staged",
        "--expect-scope",
        &scope.fingerprint,
        "--max-model-files=64",
        "--max-model-bytes",
        "65536",
    ];

    let first = provider_cli(&repository, &arguments)?;
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(first.stderr.is_empty());
    let model: RustAnalyzerProjectModel = serde_json::from_slice(&first.stdout)?;
    model.validate()?;
    assert_eq!(model.digest, model.canonical_sha256());
    assert_eq!(
        model
            .crates
            .iter()
            .map(|item| item.root_module.as_str())
            .collect::<Vec<_>>(),
        vec!["src/lib.rs"]
    );
    assert!(model
        .limitations
        .windows(2)
        .all(|items| items[0] < items[1]));
    assert_eq!(
        first.stdout,
        format!("{}\n", serde_json::to_string(&model)?).as_bytes()
    );
    assert!(
        !String::from_utf8_lossy(&first.stdout).contains(&repository.path().display().to_string())
    );

    let repeated = provider_cli(&repository, &arguments)?;
    assert!(repeated.status.success());
    assert!(repeated.stderr.is_empty());
    assert_eq!(first.stdout, repeated.stdout);
    Ok(())
}

#[test]
fn model_rejects_scope_drift_without_stdout() -> Result<(), Box<dyn Error>> {
    let repository = GitRepo::new()?;
    repository.commit_file("README.md", b"base\n")?;
    repository.write(
        "Cargo.toml",
        b"[package]\nname = \"drift\"\nedition = \"2021\"\n",
    )?;
    repository.write("src/lib.rs", b"pub fn drift() {}\n")?;
    repository.git(["add", "--", "Cargo.toml", "src/lib.rs"])?;
    let scope = repository.scope(ReviewSource::Staged)?;
    repository.write("src/lib.rs", b"pub fn changed_after_scope() {}\n")?;
    repository.git(["add", "--", "src/lib.rs"])?;

    let output = provider_cli(
        &repository,
        &[
            "model",
            "--source",
            "staged",
            "--expect-scope",
            &scope.fingerprint,
        ],
    )?;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr)?.contains("provider-cli-scope-invalid"));
    Ok(())
}

#[cfg(all(feature = "test-fixture", unix))]
struct CliRunFixture {
    repository: GitRepo,
    assets: TempDir,
    scope_fingerprint: String,
    snapshot_sha256: String,
    model: RustAnalyzerProjectModel,
    profile: AuthorizedProviderProfile,
    registry_path: PathBuf,
    registry_sha256: String,
    profile_path: PathBuf,
    executable_path: PathBuf,
    model_path: PathBuf,
    model_file_sha256: String,
    request_path: PathBuf,
}

#[cfg(all(feature = "test-fixture", unix))]
impl CliRunFixture {
    fn new(scenario: &str, deadline_ms: u64) -> Result<Self, Box<dyn Error>> {
        let repository = GitRepo::new()?;
        repository.commit_file("README.md", b"base\n")?;
        repository.write(
            "Cargo.toml",
            b"[package]\nname = \"provider-cli\"\nedition = \"2021\"\n",
        )?;
        repository.write(
            "src/lib.rs",
            b"pub fn seed() { caller(); }\npub fn caller() { seed(); }\npub fn callee() {}\n",
        )?;
        repository.git(["add", "--", "Cargo.toml", "src/lib.rs"])?;
        let scope = repository.scope(ReviewSource::Staged)?;
        let snapshot = CandidateSnapshot::materialize(
            repository.path(),
            ReviewSource::Staged,
            SnapshotLimits {
                max_files: 64,
                max_bytes: 64 * 1024,
            },
        )?;
        let model = build_linked_project_model(&snapshot, ProviderModelLimits::default())?;
        let snapshot_sha256 = snapshot.sha256.clone();

        let assets = TempDir::new()?;
        let executable_path = assets.path().join("fake-rust-analyzer");
        let fixture_binary =
            PathBuf::from(env!("CARGO_BIN_EXE_repository-context-provider-fixture"));
        fs::write(
            &executable_path,
            format!(
                "#!/bin/sh\nexec '{}' '{}'\n",
                fixture_binary.display(),
                scenario
            ),
        )?;
        fs::set_permissions(&executable_path, fs::Permissions::from_mode(0o755))?;
        let executable_path = fs::canonicalize(executable_path)?;
        let executable_sha256 = file_sha256(&executable_path)?;

        let mut profile = AuthorizedProviderProfile {
            schema_version: 1,
            kind: "repository_context_provider_profile".to_string(),
            provider_kind: "rust-analyzer".to_string(),
            provider_version: "fixture-1".to_string(),
            executable_sha256: executable_sha256.clone(),
            configuration_sha256: "0".repeat(64),
            target_triple: model.target_triple.clone(),
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
        profile.validate()?;
        let profile_path = assets.path().join("profile.json");
        let profile_bytes = serde_json::to_vec(&profile)?;
        fs::write(&profile_path, &profile_bytes)?;
        let profile_path = fs::canonicalize(profile_path)?;
        assert_eq!(profile.sha256(), sha256(&profile_bytes));

        let registry = ProviderRegistry {
            schema_version: 1,
            kind: "repository_context_provider_registry".to_string(),
            entries: vec![ProviderRegistryEntry {
                provider_id: "fixture-local".to_string(),
                provider_kind: profile.provider_kind.clone(),
                provider_version: profile.provider_version.clone(),
                target_triple: profile.target_triple.clone(),
                profile_path: profile_path.clone(),
                profile_sha256: profile.sha256(),
                executable_path: executable_path.clone(),
                executable_sha256,
                configuration_sha256: profile.configuration_sha256.clone(),
                toolchain_mode: profile.toolchain_mode.clone(),
            }],
        };
        registry.validate()?;
        let registry_path = assets.path().join("registry.json");
        let registry_bytes = serde_json::to_vec(&registry)?;
        fs::write(&registry_path, &registry_bytes)?;
        let registry_path = fs::canonicalize(registry_path)?;
        let registry_sha256 = sha256(&registry_bytes);

        let model_path = assets.path().join("model.json");
        let model_bytes = serde_json::to_vec(&model)?;
        fs::write(&model_path, &model_bytes)?;
        let model_path = fs::canonicalize(model_path)?;
        let model_file_sha256 = sha256(&model_bytes);

        let run_request = ProviderRunRequest {
            schema_version: 1,
            kind: "repository_context_provider_run_request".to_string(),
            seeds: vec![graph_seed()],
            directions: vec![CallDirection::Incoming, CallDirection::Outgoing],
            limits: graph_limits(deadline_ms),
        };
        run_request.validate_against(&profile.maximum_limits)?;
        let request_path = assets.path().join("request.json");
        fs::write(&request_path, serde_json::to_vec(&run_request)?)?;
        let request_path = fs::canonicalize(request_path)?;

        Ok(Self {
            repository,
            assets,
            scope_fingerprint: scope.fingerprint,
            snapshot_sha256,
            model,
            profile,
            registry_path,
            registry_sha256,
            profile_path,
            executable_path,
            model_path,
            model_file_sha256,
            request_path,
        })
    }

    fn arguments(&self) -> Vec<String> {
        self.arguments_for_scope(&self.scope_fingerprint)
    }

    fn arguments_for_scope(&self, scope_fingerprint: &str) -> Vec<String> {
        vec![
            "run".to_string(),
            "--source".to_string(),
            "staged".to_string(),
            "--expect-scope".to_string(),
            scope_fingerprint.to_string(),
            "--registry".to_string(),
            self.registry_path.display().to_string(),
            "--expect-registry-sha256".to_string(),
            self.registry_sha256.clone(),
            "--provider-id".to_string(),
            "fixture-local".to_string(),
            "--model".to_string(),
            self.model_path.display().to_string(),
            "--expect-model-sha256".to_string(),
            self.model_file_sha256.clone(),
            "--request".to_string(),
            self.request_path.display().to_string(),
        ]
    }

    fn run(&self) -> Result<Output, Box<dyn Error>> {
        run_provider_arguments(&self.repository, &self.arguments())
    }
}

#[cfg(all(feature = "test-fixture", unix))]
fn run_provider_arguments(
    repository: &GitRepo,
    arguments: &[String],
) -> Result<Output, Box<dyn Error>> {
    let arguments = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    provider_cli(repository, &arguments)
}

#[cfg(all(feature = "test-fixture", unix))]
fn graph_seed() -> SeedSymbol {
    SeedSymbol {
        changed_symbol_id: "5".repeat(64),
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

#[cfg(all(feature = "test-fixture", unix))]
fn graph_limits(deadline_ms: u64) -> ProviderLimits {
    ProviderLimits {
        deadline_ms,
        max_depth: 2,
        max_seeds: 1,
        max_requests: 64,
        max_pending_requests: 1,
        max_messages: 256,
        max_notifications: 64,
        max_server_requests: 32,
        max_invalid_messages: 4,
        max_call_ranges: 64,
        max_header_bytes: 4_096,
        max_frame_bytes: 64 * 1_024,
        max_protocol_bytes: 512 * 1_024,
        max_stderr_bytes: 1_024,
        max_total_output_bytes: 2 * 1_024 * 1_024,
        max_source_file_bytes: 4_096,
        max_source_bytes: 4_096,
        max_nodes: 16,
        max_edges: 32,
        max_report_bytes: 64 * 1_024,
    }
}

#[cfg(all(feature = "test-fixture", unix))]
fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(all(feature = "test-fixture", unix))]
fn file_sha256(path: &Path) -> Result<String, Box<dyn Error>> {
    Ok(sha256(&fs::read(path)?))
}

#[cfg(all(feature = "test-fixture", unix))]
fn assert_authorization_rejected(
    fixture: &CliRunFixture,
    arguments: Vec<String>,
    expected_code: &str,
) -> Result<(), Box<dyn Error>> {
    let output = run_provider_arguments(&fixture.repository, &arguments)?;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.starts_with(&format!(
            "repository-context-provider-cli: {expected_code}:"
        )),
        "unexpected stderr: {stderr}"
    );
    assert!(stderr.len() <= 512);
    assert!(!stderr.contains(&fixture.repository.path().display().to_string()));
    assert!(!stderr.contains(&fixture.assets.path().display().to_string()));
    Ok(())
}

#[cfg(all(feature = "test-fixture", unix))]
#[test]
fn run_binds_registry_profile_executable_model_request_scope_and_snapshot(
) -> Result<(), Box<dyn Error>> {
    let fixture = CliRunFixture::new("graph", 2_000)?;
    let output = fixture.run()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let report: RepositoryContextProviderReport = serde_json::from_slice(&output.stdout)?;
    report.validate()?;
    assert_eq!(report.status, RepositoryContextProviderStatus::Completed);
    assert_eq!(
        report.candidate.scope_fingerprint,
        fixture.scope_fingerprint
    );
    assert_eq!(report.candidate.snapshot_sha256, fixture.snapshot_sha256);
    assert_eq!(report.candidate.project_model_digest, fixture.model.digest);
    assert_eq!(report.provider.profile_sha256, fixture.profile.sha256());
    assert_eq!(
        report.provider.executable_sha256,
        fixture.profile.executable_sha256
    );
    assert_eq!(
        report.provider.configuration_sha256,
        fixture.profile.configuration_sha256
    );
    assert!(!report.seed_symbols.is_empty());
    assert!(!report.edges.is_empty());
    let encoded = String::from_utf8(output.stdout)?;
    assert!(!encoded.contains(&fixture.repository.path().display().to_string()));
    assert!(!encoded.contains(&fixture.assets.path().display().to_string()));
    assert!(!encoded.contains("Content-Length"));
    Ok(())
}

#[cfg(all(feature = "test-fixture", unix))]
#[test]
fn run_rejects_every_authorized_input_drift_without_a_report() -> Result<(), Box<dyn Error>> {
    let fixture = CliRunFixture::new("graph", 2_000)?;
    fs::write(&fixture.registry_path, b"{}")?;
    assert_authorization_rejected(
        &fixture,
        fixture.arguments(),
        "provider-cli-registry-invalid",
    )?;

    let fixture = CliRunFixture::new("graph", 2_000)?;
    fs::write(&fixture.profile_path, b"{}")?;
    assert_authorization_rejected(
        &fixture,
        fixture.arguments(),
        "provider-cli-profile-invalid",
    )?;

    let fixture = CliRunFixture::new("graph", 2_000)?;
    fs::write(&fixture.executable_path, b"#!/bin/sh\nexit 99\n")?;
    fs::set_permissions(&fixture.executable_path, fs::Permissions::from_mode(0o755))?;
    assert_authorization_rejected(
        &fixture,
        fixture.arguments(),
        "provider-cli-executable-invalid",
    )?;

    let fixture = CliRunFixture::new("graph", 2_000)?;
    fs::write(&fixture.model_path, b"{}")?;
    assert_authorization_rejected(&fixture, fixture.arguments(), "provider-cli-model-invalid")?;

    let fixture = CliRunFixture::new("graph", 2_000)?;
    fs::write(&fixture.request_path, b"{}")?;
    assert_authorization_rejected(
        &fixture,
        fixture.arguments(),
        "provider-cli-request-invalid",
    )?;

    let fixture = CliRunFixture::new("graph", 2_000)?;
    fixture
        .repository
        .write("src/lib.rs", b"pub fn changed_after_scope() {}\n")?;
    fixture.repository.git(["add", "--", "src/lib.rs"])?;
    assert_authorization_rejected(&fixture, fixture.arguments(), "provider-cli-scope-invalid")?;

    let fixture = CliRunFixture::new("graph", 2_000)?;
    fixture
        .repository
        .git(["rm", "-q", "--cached", "src/lib.rs"])?;
    let scope = fixture.repository.scope(ReviewSource::Staged)?;
    assert_authorization_rejected(
        &fixture,
        fixture.arguments_for_scope(&scope.fingerprint),
        "provider-cli-binding-invalid",
    )?;
    Ok(())
}

#[cfg(all(feature = "test-fixture", unix))]
#[test]
fn run_renders_the_complete_provider_status_matrix_without_child_text() -> Result<(), Box<dyn Error>>
{
    for (scenario, deadline_ms, expected) in [
        ("graph", 2_000, RepositoryContextProviderStatus::Completed),
        (
            "graph-warning",
            2_000,
            RepositoryContextProviderStatus::Partial,
        ),
        (
            "missing-capability",
            2_000,
            RepositoryContextProviderStatus::Unavailable,
        ),
        (
            "readiness-hang",
            100,
            RepositoryContextProviderStatus::Timeout,
        ),
        (
            "unknown-encoding",
            2_000,
            RepositoryContextProviderStatus::InvalidOutput,
        ),
        (
            "initialize-error",
            2_000,
            RepositoryContextProviderStatus::Failed,
        ),
    ] {
        let fixture = CliRunFixture::new(scenario, deadline_ms)?;
        let output = fixture.run()?;
        assert!(
            output.status.success(),
            "scenario {scenario}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let report: RepositoryContextProviderReport = serde_json::from_slice(&output.stdout)?;
        report.validate()?;
        assert_eq!(report.status, expected, "scenario {scenario}");
        let encoded = String::from_utf8(output.stdout)?;
        assert!(!encoded.contains("fixture initialize failure"));
        assert!(!encoded.contains("Content-Length"));
        assert!(!encoded.contains(&fixture.assets.path().display().to_string()));
    }
    Ok(())
}
