#![cfg(feature = "test-fixture")]

use collect_diff_context_cli::artifacts::contract::sha256_bytes;
use serde_json::{json, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

use collect_diff_context_cli::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};

const HELP: &str = "Usage:\n  provider-baseline-sample-runner contract --target-root <absolute-path> --source-lock <absolute-path> --fixture-root <absolute-path> --runner-class <id> --output <absolute-path>\n  provider-baseline-sample-runner sample --target-root <absolute-path> --source-lock <absolute-path> --fixture-root <absolute-path> --runner-class <id>\n";
const EXPECTED_RUNNER_SHA256: &str = "PCR_PROVIDER_BASELINE_EXPECTED_RUNNER_SHA256";
const SOURCE_LOCK_SHA256: &str = "298bc6c0339fe2c58fd35bfbd53db285ea7ff34e40734a4f0c36ccb3fe60d862";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn current_platform() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "darwin-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "darwin-amd64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-amd64"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "windows-amd64"
    } else {
        panic!("unsupported test platform");
    }
}

fn hosted_runner_class() -> &'static str {
    match current_platform() {
        "darwin-amd64" => "github-hosted-macos-15-intel",
        "darwin-arm64" => "github-hosted-macos-14-arm64",
        "linux-amd64" => "github-hosted-ubuntu-24-x64",
        "windows-amd64" => "github-hosted-windows-2025-x64",
        _ => unreachable!(),
    }
}

fn hosted_runner_metadata() -> [(&'static str, &'static str); 5] {
    match current_platform() {
        "darwin-amd64" => [
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REPOSITORY", "junit/pre-commit-review"),
            ("RUNNER_OS", "macOS"),
            ("RUNNER_ARCH", "X64"),
            ("ImageOS", "macos15"),
        ],
        "darwin-arm64" => [
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REPOSITORY", "junit/pre-commit-review"),
            ("RUNNER_OS", "macOS"),
            ("RUNNER_ARCH", "ARM64"),
            ("ImageOS", "macos14"),
        ],
        "linux-amd64" => [
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REPOSITORY", "junit/pre-commit-review"),
            ("RUNNER_OS", "Linux"),
            ("RUNNER_ARCH", "X64"),
            ("ImageOS", "ubuntu24"),
        ],
        "windows-amd64" => [
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REPOSITORY", "junit/pre-commit-review"),
            ("RUNNER_OS", "Windows"),
            ("RUNNER_ARCH", "X64"),
            ("ImageOS", "win25"),
        ],
        _ => unreachable!(),
    }
}

fn hosted_runner_environment() -> serde_json::Map<String, Value> {
    let git = if cfg!(windows) { "git.exe" } else { "git" };
    let git = env::split_paths(&env::var_os("PATH").unwrap())
        .map(|directory| directory.join(git))
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| fs::canonicalize(candidate).ok())
        .unwrap();
    let mut environment = hosted_runner_metadata()
        .into_iter()
        .map(|(name, value)| (name.to_string(), json!(value)))
        .collect::<serde_json::Map<String, Value>>();
    environment.insert(
        "PATH".to_string(),
        json!(env::join_paths([git.parent().unwrap()])
            .unwrap()
            .to_string_lossy()),
    );
    environment.insert(
        "GIT_CONFIG_GLOBAL".to_string(),
        json!(if cfg!(windows) { "NUL" } else { "/dev/null" }),
    );
    environment.insert("GIT_CONFIG_NOSYSTEM".to_string(), json!("1"));
    environment.insert("GIT_TERMINAL_PROMPT".to_string(), json!("0"));
    environment.insert("LC_ALL".to_string(), json!("C"));
    for name in ["SystemRoot", "TMPDIR", "TMP", "TEMP"] {
        if let Ok(value) = env::var(name) {
            environment.insert(name.to_string(), json!(value));
        }
    }
    environment
}

fn reviewed_contract(temporary: &Path, executable: &Path) -> PathBuf {
    let target_root = temporary.join("target");
    let fixture_root = temporary.join("fixture");
    fs::create_dir(&target_root).unwrap();
    fs::create_dir(&fixture_root).unwrap();
    fs::write(fixture_root.join("lib.rs"), b"pub fn seed() {}\n").unwrap();
    let source_lock = temporary.join("source-lock.json");
    fs::write(&source_lock, b"{}").unwrap();
    let environment = hosted_runner_environment();
    assert!(!environment.contains_key(EXPECTED_RUNNER_SHA256));
    let contract = json!({
        "schema_version": 1,
        "kind": "provider_baseline_runner",
        "command": [
            executable,
            "sample",
            "--target-root",
            target_root,
            "--source-lock",
            source_lock,
            "--fixture-root",
            fixture_root,
            "--runner-class",
            hosted_runner_class()
        ],
        "current_directory": temporary,
        "environment": environment,
        "expected": {
            "platform_id": current_platform(),
            "pack_version": "2026.07.27-pcr.3",
            "pack_sha256": "1".repeat(64),
            "executable_sha256": "2".repeat(64),
            "source_lock_sha256": SOURCE_LOCK_SHA256,
            "profile_sha256": "3".repeat(64),
            "fixture_id": "single-crate",
            "fixture_sha256": "4".repeat(64),
            "request_sha256": "5".repeat(64),
            "runner_class": hosted_runner_class(),
            "toolchain": "rust-1.95.0-locked",
            "timing_scope": "provider-run-only-v1",
            "provisioning_included": false
        }
    });
    let path = temporary.join("runner.json");
    fs::write(&path, serde_json::to_vec(&contract).unwrap()).unwrap();
    path
}

fn reviewed_measurement_command(contract: &Path) -> Command {
    let mut command = Command::new("python3");
    command
        .arg(repo_root().join("scripts/measure_provider_baseline.py"))
        .arg("--runner")
        .arg(contract)
        .arg("--samples")
        .arg("20");
    for (name, value) in hosted_runner_metadata() {
        command.env(name, value);
    }
    command
}

fn run_reviewed_measurement(contract: &Path, runner_sha256: &str) -> Output {
    reviewed_measurement_command(contract)
        .env(EXPECTED_RUNNER_SHA256, runner_sha256)
        .output()
        .unwrap()
}

fn local_runner_class() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "local-darwin-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "local-darwin-amd64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "local-linux-amd64"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "local-windows-amd64"
    } else {
        panic!("unsupported test platform");
    }
}

fn assert_fixture_structure_rejected(populate: impl FnOnce(&Path)) {
    let temporary = tempfile::tempdir().unwrap();
    let fixture_root = temporary.path().join("fixture");
    fs::create_dir(&fixture_root).unwrap();
    populate(&fixture_root);
    let source_lock = temporary.path().join("source-lock.json");
    fs::write(&source_lock, b"{}").unwrap();
    let output_path = temporary.path().join("runner.json");

    let output = Command::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"))
        .args([
            "contract",
            "--target-root",
            temporary.path().to_str().unwrap(),
            "--source-lock",
            source_lock.to_str().unwrap(),
            "--fixture-root",
            fixture_root.to_str().unwrap(),
            "--runner-class",
            local_runner_class(),
            "--output",
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fixture-structure-policy"),
        "fixture was not rejected before provider bindings were read: {stderr}"
    );
    assert!(!output_path.exists());
}

#[test]
fn runner_binary_has_a_strict_help_contract() {
    let output = Command::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), HELP);
    assert!(output.stderr.is_empty());
}

#[test]
fn reviewed_measurement_rejects_a_same_name_runner_with_a_different_digest() {
    let temporary = tempfile::tempdir().unwrap();
    let executable_name = if cfg!(windows) {
        "provider-baseline-sample-runner.exe"
    } else {
        "provider-baseline-sample-runner"
    };
    let fake_runner = temporary.path().join(executable_name);
    fs::copy(env::current_exe().unwrap(), &fake_runner).unwrap();
    let contract = reviewed_contract(temporary.path(), &fake_runner);
    let real_runner = Path::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"));
    let real_runner_sha256 = sha256_bytes(&fs::read(real_runner).unwrap());

    let output = run_reviewed_measurement(&contract, &real_runner_sha256);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("runner-provenance"),
        "same-name runner was not rejected at the provenance boundary: {stderr}"
    );
}

#[test]
fn core_release_measurement_accepts_the_cargo_built_runner_digest_at_provenance() {
    let temporary = tempfile::tempdir().unwrap();
    let real_runner = Path::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"));
    let contract = reviewed_contract(temporary.path(), real_runner);
    let real_runner_sha256 = sha256_bytes(&fs::read(real_runner).unwrap());

    let output = reviewed_measurement_command(&contract)
        .env(EXPECTED_RUNNER_SHA256, &real_runner_sha256)
        .env("GITHUB_WORKFLOW", "Release Multi-Platform Packs")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("core-release-boundary"),
        "core release did not reach reviewed runner validation: {stderr}"
    );
    assert!(
        !stderr.contains("runner-provenance"),
        "Cargo-built runner failed its provenance boundary: {stderr}"
    );
    assert!(
        stderr.contains("runner-execution"),
        "invalid target did not fail after provenance validation: {stderr}"
    );
}

#[test]
fn reviewed_measurement_requires_the_digest_outside_the_runner_contract() {
    let temporary = tempfile::tempdir().unwrap();
    let real_runner = Path::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"));
    let contract_path = reviewed_contract(temporary.path(), real_runner);
    let real_runner_sha256 = sha256_bytes(&fs::read(real_runner).unwrap());

    for invalid in [None, Some("not-a-sha256")] {
        let mut command = reviewed_measurement_command(&contract_path);
        if let Some(value) = invalid {
            command.env(EXPECTED_RUNNER_SHA256, value);
        } else {
            command.env_remove(EXPECTED_RUNNER_SHA256);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("runner-provenance"));
    }

    let mut contract: Value = serde_json::from_slice(&fs::read(&contract_path).unwrap()).unwrap();
    contract["environment"][EXPECTED_RUNNER_SHA256] = json!(real_runner_sha256.clone());
    fs::write(&contract_path, serde_json::to_vec(&contract).unwrap()).unwrap();
    let output = run_reviewed_measurement(&contract_path, &real_runner_sha256);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("runner-provenance"));
    assert!(stderr.contains("contract cannot declare"));
}

#[test]
fn reviewed_measurement_rejects_a_mixed_case_contract_trust_key() {
    let temporary = tempfile::tempdir().unwrap();
    let real_runner = Path::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"));
    let contract_path = reviewed_contract(temporary.path(), real_runner);
    let real_runner_sha256 = sha256_bytes(&fs::read(real_runner).unwrap());
    let mut contract: Value = serde_json::from_slice(&fs::read(&contract_path).unwrap()).unwrap();
    contract["environment"]["pCr_PrOvIdEr_BaSeLiNe_ExPeCtEd_RuNnEr_ShA256"] =
        json!(real_runner_sha256.clone());
    fs::write(&contract_path, serde_json::to_vec(&contract).unwrap()).unwrap();

    let output = run_reviewed_measurement(&contract_path, &real_runner_sha256);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("runner-provenance") && stderr.contains("contract cannot declare"),
        "mixed-case trust key escaped contract policy: {stderr}"
    );
}

#[test]
fn reviewed_measurement_rejects_environment_outside_the_hosted_policy() {
    let temporary = tempfile::tempdir().unwrap();
    let real_runner = Path::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"));
    let contract_path = reviewed_contract(temporary.path(), real_runner);
    let real_runner_sha256 = sha256_bytes(&fs::read(real_runner).unwrap());
    let mut contract: Value = serde_json::from_slice(&fs::read(&contract_path).unwrap()).unwrap();
    contract["environment"]["UNREVIEWED_ENVIRONMENT_VARIABLE"] = json!("injected");
    fs::write(&contract_path, serde_json::to_vec(&contract).unwrap()).unwrap();

    let output = run_reviewed_measurement(&contract_path, &real_runner_sha256);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("runner-provenance") && stderr.contains("environment policy"),
        "unreviewed environment variable escaped hosted provenance policy: {stderr}"
    );
}

#[test]
fn reviewed_measurement_requires_the_exact_hosted_environment_policy() {
    let temporary = tempfile::tempdir().unwrap();
    let real_runner = Path::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"));
    let contract_path = reviewed_contract(temporary.path(), real_runner);
    let real_runner_sha256 = sha256_bytes(&fs::read(real_runner).unwrap());
    let mut contract: Value = serde_json::from_slice(&fs::read(&contract_path).unwrap()).unwrap();
    contract["environment"]
        .as_object_mut()
        .unwrap()
        .remove("GIT_CONFIG_NOSYSTEM");
    fs::write(&contract_path, serde_json::to_vec(&contract).unwrap()).unwrap();

    let output = run_reviewed_measurement(&contract_path, &real_runner_sha256);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("runner-provenance") && stderr.contains("environment policy"),
        "incomplete hosted environment policy reached runner execution: {stderr}"
    );
}

#[test]
fn measurement_contract_rejects_case_folded_environment_duplicates() {
    let temporary = tempfile::tempdir().unwrap();
    let real_runner = Path::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"));
    let contract_path = reviewed_contract(temporary.path(), real_runner);
    let real_runner_sha256 = sha256_bytes(&fs::read(real_runner).unwrap());
    let mut contract: Value = serde_json::from_slice(&fs::read(&contract_path).unwrap()).unwrap();
    contract["environment"]["PATH"] = json!("/first");
    contract["environment"]["Path"] = json!("/second");
    fs::write(&contract_path, serde_json::to_vec(&contract).unwrap()).unwrap();

    let output = run_reviewed_measurement(&contract_path, &real_runner_sha256);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("runner-contract") && stderr.contains("case-folded"),
        "case-folded environment duplicates escaped contract policy: {stderr}"
    );
}

#[test]
fn runner_rejects_a_hosted_identity_without_matching_github_metadata() {
    let temporary = tempfile::tempdir().unwrap();
    let source_lock = temporary.path().join("source-lock.json");
    fs::write(&source_lock, b"{}").unwrap();
    let output_path = temporary.path().join("runner.json");
    let hosted_class = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "github-hosted-macos-14-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "github-hosted-macos-15-intel"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "github-hosted-ubuntu-24-x64"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "github-hosted-windows-2025-x64"
    } else {
        panic!("unsupported test platform");
    };
    let output = Command::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"))
        .args([
            "contract",
            "--target-root",
            temporary.path().to_str().unwrap(),
            "--source-lock",
            source_lock.to_str().unwrap(),
            "--fixture-root",
            temporary.path().to_str().unwrap(),
            "--runner-class",
            hosted_class,
            "--output",
            output_path.to_str().unwrap(),
        ])
        .env_remove("GITHUB_ACTIONS")
        .env_remove("RUNNER_OS")
        .env_remove("RUNNER_ARCH")
        .env_remove("ImageOS")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("runner-binding"));
    assert!(stderr.contains("hosted runner metadata differs"));
    assert!(!output_path.exists());
}

#[test]
fn runner_rejects_unbounded_fixture_trees_before_provider_bindings() {
    assert_fixture_structure_rejected(|fixture| {
        fs::write(fixture.join("oversized.rs"), vec![b'x'; 64 * 1024 + 1]).unwrap();
    });

    assert_fixture_structure_rejected(|fixture| {
        let mut directory = fixture.to_path_buf();
        for _ in 0..17 {
            directory.push("d");
        }
        fs::create_dir_all(directory).unwrap();
    });

    assert_fixture_structure_rejected(|fixture| {
        for index in 0..65 {
            fs::create_dir(fixture.join(format!("directory-{index:02}"))).unwrap();
        }
    });

    assert_fixture_structure_rejected(|fixture| {
        let component = "p".repeat(50);
        let directory = fixture.join(&component).join(&component).join(&component);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("{}.rs", "f".repeat(50))),
            b"fixture\n",
        )
        .unwrap();
    });
}

#[test]
fn runner_requires_the_target_distribution_manifest_before_receipts() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repository_context_provider/real/single_crate");
    let source_lock = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../third_party_artifacts/sources/rust-analyzer-2026-07-27.json");
    let output_path = temporary.path().join("runner.json");

    let output = Command::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"))
        .args([
            "contract",
            "--target-root",
            temporary.path().to_str().unwrap(),
            "--source-lock",
            source_lock.to_str().unwrap(),
            "--fixture-root",
            fixture_root.to_str().unwrap(),
            "--runner-class",
            local_runner_class(),
            "--output",
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("target distribution manifest is unavailable"),
        "runner did not verify the target manifest first: {stderr}"
    );
    assert!(!output_path.exists());
}

#[test]
fn runner_sample_observes_its_total_deadline_before_provider_bindings() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture_root = temporary.path().join("fixture");
    fs::create_dir(&fixture_root).unwrap();
    fs::write(fixture_root.join("lib.rs"), b"pub fn seed() {}\n").unwrap();
    let source_lock = temporary.path().join("source-lock.json");
    fs::write(&source_lock, b"{}").unwrap();
    let output_path = temporary.path().join("sample.json");

    let output = Command::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"))
        .args([
            "sample",
            "--target-root",
            temporary.path().to_str().unwrap(),
            "--source-lock",
            source_lock.to_str().unwrap(),
            "--fixture-root",
            fixture_root.to_str().unwrap(),
            "--runner-class",
            local_runner_class(),
        ])
        .env("PCR_PROVIDER_BASELINE_SAMPLE_OUTPUT", &output_path)
        .env("PCR_PROVIDER_BASELINE_TEST_DEADLINE_MS", "0")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("runner-deadline"),
        "runner did not enforce its total deadline first: {stderr}"
    );
    assert!(!output_path.exists());

    let output = Command::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"))
        .args([
            "sample",
            "--target-root",
            temporary.path().to_str().unwrap(),
            "--source-lock",
            source_lock.to_str().unwrap(),
            "--fixture-root",
            fixture_root.to_str().unwrap(),
            "--runner-class",
            local_runner_class(),
        ])
        .env("PCR_PROVIDER_BASELINE_SAMPLE_OUTPUT", &output_path)
        .env("PCR_PROVIDER_BASELINE_TEST_DEADLINE_MS", "30001")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("runner-arguments"));
    assert!(stderr.contains("test deadline override is invalid"));
}

#[cfg(unix)]
#[test]
fn bounded_runner_snapshot_terminates_a_hanging_git_tree() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let git = temporary.path().join("git");
    let marker = temporary.path().join("git-descendant.marker");
    fs::write(
        &git,
        format!(
            "#!/bin/sh\n(sleep 0.8; touch '{}') &\nwait\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    let started = Instant::now();

    let result = CandidateSnapshot::materialize_staged_bounded(
        temporary.path(),
        &git,
        SnapshotLimits {
            max_files: 64,
            max_bytes: 256 * 1024,
        },
        Duration::from_millis(100),
    );

    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
    thread::sleep(Duration::from_millis(900));
    assert!(!marker.exists(), "hanging Git descendant survived deadline");
}

#[test]
fn bounded_snapshot_deadline_cleans_a_read_only_temporary_tree() {
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn snapshot_is_read_only(root: &Path, _deep_path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(root).is_ok_and(|metadata| metadata.permissions().mode() & 0o222 == 0)
        }
        #[cfg(windows)]
        {
            fs::OpenOptions::new()
                .write(true)
                .open(root.join(_deep_path))
                .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
        }
    }

    let temporary = tempfile::tempdir().unwrap();
    let git = PathBuf::from(env!("CARGO_BIN_EXE_repository-context-provider-fixture"));
    let snapshot_paths = (0..12)
        .map(|branch| format!("{branch:02}/{}deadline-cleanup-token", "d/".repeat(60)))
        .collect::<Vec<_>>();
    let deep_path = snapshot_paths[0].clone();
    let mut index_records = Vec::new();
    for path in &snapshot_paths {
        index_records.extend_from_slice(format!("100644 {} 0\t{path}", "1".repeat(40)).as_bytes());
        index_records.push(0);
    }
    let first_blob_marker = temporary.path().join("first-blob-complete");
    let observer_marker = temporary.path().join("snapshot-observer-ready");
    fs::write(
        temporary.path().join(".snapshot-index-records"),
        index_records,
    )
    .unwrap();
    let temp_root = env::temp_dir();
    let existing = fs::read_dir(&temp_root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<HashSet<_>>();
    let mut exercised_read_only_deadline = false;
    let mut deadline_failures = 0;
    let mut successes = 0;
    let mut observed_candidates = 0;
    let mut observed_read_only = 0;
    let mut last_error = String::new();

    for deadline_ms in 1..=500 {
        let _ = fs::remove_file(&first_blob_marker);
        let _ = fs::remove_file(&observer_marker);
        let watched_existing = existing.clone();
        let watched_temp_root = temp_root.clone();
        let watched_deep_path = PathBuf::from(&deep_path);
        let watched_observer_marker = observer_marker.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let watched_stop = Arc::clone(&stop);
        let watcher = thread::spawn(move || {
            while !watched_stop.load(Ordering::Acquire) {
                for entry in fs::read_dir(&watched_temp_root)
                    .unwrap()
                    .filter_map(Result::ok)
                {
                    let candidate = entry.path();
                    if !entry.file_name().to_string_lossy().starts_with(".tmp")
                        || watched_existing.contains(&candidate)
                        || !candidate.join(&watched_deep_path).exists()
                    {
                        continue;
                    }
                    fs::write(&watched_observer_marker, b"ready").unwrap();
                    let mut saw_read_only = false;
                    while !watched_stop.load(Ordering::Acquire) && candidate.exists() {
                        if snapshot_is_read_only(&candidate, &watched_deep_path) {
                            saw_read_only = true;
                            break;
                        }
                        thread::yield_now();
                    }
                    return Some((candidate, saw_read_only));
                }
                thread::yield_now();
            }
            None
        });

        let result = CandidateSnapshot::materialize_staged_bounded(
            temporary.path(),
            &git,
            SnapshotLimits {
                max_files: snapshot_paths.len(),
                max_bytes: 1024,
            },
            Duration::from_millis(deadline_ms),
        );
        let deadline_failed = result.is_err();
        if deadline_failed {
            deadline_failures += 1;
            last_error = result.as_ref().unwrap_err().to_string();
        } else {
            successes += 1;
        }
        drop(result);
        stop.store(true, Ordering::Release);
        let observed = watcher.join().unwrap();
        if observed.is_some() {
            observed_candidates += 1;
        }
        if observed.as_ref().is_some_and(|(_, read_only)| *read_only) {
            observed_read_only += 1;
        }

        if let Some((snapshot_root, true)) = observed.filter(|_| deadline_failed) {
            let leaked = snapshot_root.exists();
            exercised_read_only_deadline = true;
            assert!(!leaked, "deadline leaked a read-only snapshot tree");
            break;
        }
    }

    assert!(
        exercised_read_only_deadline,
        "test did not reach the post-hardening deadline window: failures={deadline_failures} successes={successes} candidates={observed_candidates} read_only={observed_read_only} last_error={last_error}"
    );
}

#[cfg(windows)]
#[test]
fn baseline_runner_rejects_source_lock_change_time_drift_during_read() {
    use std::io::{Seek, SeekFrom, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};

    let temporary = tempfile::tempdir().unwrap();
    let fixture_root = temporary.path().join("fixture");
    fs::create_dir(&fixture_root).unwrap();
    fs::write(fixture_root.join("lib.rs"), b"pub fn seed() {}\n").unwrap();
    let source_lock = temporary.path().join("source-lock.json");
    let mut source_lock_bytes = vec![b' '; 900 * 1024];
    source_lock_bytes[0] = b'{';
    *source_lock_bytes.last_mut().unwrap() = b'}';
    fs::write(&source_lock, source_lock_bytes).unwrap();

    let barrier = Arc::new(Barrier::new(2));
    let mutator_barrier = Arc::clone(&barrier);
    let stop = Arc::new(AtomicBool::new(false));
    let mutator_stop = Arc::clone(&stop);
    let mutated_source_lock = source_lock.clone();
    let mutator = thread::spawn(move || {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(mutated_source_lock)
            .unwrap();
        mutator_barrier.wait();
        while !mutator_stop.load(Ordering::Acquire) {
            file.seek(SeekFrom::Start(1024)).unwrap();
            file.write_all(b" ").unwrap();
        }
    });
    barrier.wait();
    let output = Command::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"))
        .args([
            "contract",
            "--target-root",
            temporary.path().to_str().unwrap(),
            "--source-lock",
            source_lock.to_str().unwrap(),
            "--fixture-root",
            fixture_root.to_str().unwrap(),
            "--runner-class",
            local_runner_class(),
            "--output",
            temporary.path().join("runner.json").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    stop.store(true, Ordering::Release);
    mutator.join().unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("provider binding file changed while it was read"),
        "source-lock metadata drift was not rejected: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
#[test]
fn measurement_rejects_runner_change_time_drift_during_read() {
    use std::os::windows::io::AsRawHandle;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};
    use windows_sys::Win32::Storage::FileSystem::{
        FileBasicInfo, GetFileInformationByHandleEx, SetFileInformationByHandle, FILE_BASIC_INFO,
    };

    let temporary = tempfile::tempdir().unwrap();
    let fake_runner = temporary.path().join("provider-baseline-sample-runner.exe");
    fs::copy(env::current_exe().unwrap(), &fake_runner).unwrap();
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&fake_runner)
        .unwrap();
    file.set_len(128 * 1024 * 1024).unwrap();
    let contract_path = reviewed_contract(temporary.path(), &fake_runner);
    let mut contract: Value = serde_json::from_slice(&fs::read(&contract_path).unwrap()).unwrap();
    contract["command"][9] = json!(local_runner_class());
    contract["expected"]["runner_class"] = json!(local_runner_class());
    fs::write(&contract_path, serde_json::to_vec(&contract).unwrap()).unwrap();

    let barrier = Arc::new(Barrier::new(2));
    let mutator_barrier = Arc::clone(&barrier);
    let stop = Arc::new(AtomicBool::new(false));
    let mutator_stop = Arc::clone(&stop);
    let mutator = thread::spawn(move || {
        let handle = file.as_raw_handle() as _;
        let size = u32::try_from(std::mem::size_of::<FILE_BASIC_INFO>()).unwrap();
        let mut information = std::mem::MaybeUninit::<FILE_BASIC_INFO>::zeroed();
        let succeeded = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileBasicInfo,
                information.as_mut_ptr().cast(),
                size,
            )
        };
        assert_ne!(succeeded, 0);
        let mut information = unsafe { information.assume_init() };
        let original_change_time = information.ChangeTime;
        mutator_barrier.wait();
        let mut offset = 1_i64;
        while !mutator_stop.load(Ordering::Acquire) {
            information.ChangeTime = original_change_time.saturating_add(offset);
            let succeeded = unsafe {
                SetFileInformationByHandle(
                    handle,
                    FileBasicInfo,
                    (&raw const information).cast(),
                    size,
                )
            };
            assert_ne!(succeeded, 0);
            offset = if offset == 1 { 2 } else { 1 };
        }
    });
    barrier.wait();
    let output = Command::new("python3")
        .arg(repo_root().join("scripts/measure_provider_baseline.py"))
        .arg("--runner")
        .arg(&contract_path)
        .arg("--samples")
        .arg("20")
        .arg("--evidence-only-local")
        .output()
        .unwrap();
    stop.store(true, Ordering::Release);
    mutator.join().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("runner-contract") && stderr.contains("changed while it was read"),
        "measurement accepted runner ChangeTime drift: {stderr}"
    );
}

#[test]
fn runner_contract_and_real_sample_bind_actual_provider_metrics() {
    let Some(target_root) = env::var_os("PCR_REAL_PROVIDER_TARGET_ROOT") else {
        eprintln!("PCR_REAL_PROVIDER_TARGET_ROOT is not set; skipping real baseline sample");
        return;
    };
    let temporary = tempfile::tempdir().unwrap();
    let contract_path = temporary.path().join("runner.json");
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repository_context_provider/real/single_crate");
    let source_lock = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../third_party_artifacts/sources/rust-analyzer-2026-07-27.json");
    let platform = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "darwin-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "darwin-amd64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-amd64"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "windows-amd64"
    } else {
        panic!("unsupported test platform");
    };
    let runner_class = format!("local-{platform}");

    let contract_output = Command::new(env!("CARGO_BIN_EXE_provider-baseline-sample-runner"))
        .args([
            "contract",
            "--target-root",
            target_root.to_str().unwrap(),
            "--source-lock",
            source_lock.to_str().unwrap(),
            "--fixture-root",
            fixture_root.to_str().unwrap(),
            "--runner-class",
            &runner_class,
            "--output",
            contract_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        contract_output.status.success(),
        "contract failed: {}",
        String::from_utf8_lossy(&contract_output.stderr)
    );
    assert!(contract_output.stdout.is_empty());
    let contract: Value = serde_json::from_slice(&fs::read(&contract_path).unwrap()).unwrap();
    assert_eq!(contract["kind"], "provider_baseline_runner");
    assert_eq!(contract["expected"]["runner_class"], runner_class);
    assert_eq!(contract["expected"]["platform_id"], platform);
    assert_eq!(contract["expected"]["pack_version"], "2026.07.27-pcr.3");

    let sample_path = temporary.path().join("sample.json");
    let command = contract["command"].as_array().unwrap();
    let mut sample_command = Command::new(command[0].as_str().unwrap());
    sample_command.args(command[1..].iter().map(|item| item.as_str().unwrap()));
    sample_command.current_dir(contract["current_directory"].as_str().unwrap());
    sample_command.env_clear();
    for (key, value) in contract["environment"].as_object().unwrap() {
        sample_command.env(key, value.as_str().unwrap());
    }
    sample_command.env("PCR_PROVIDER_BASELINE_SAMPLE_OUTPUT", &sample_path);
    let sample_output = sample_command.output().unwrap();
    assert!(
        sample_output.status.success(),
        "sample failed: {}",
        String::from_utf8_lossy(&sample_output.stderr)
    );
    assert!(sample_output.stdout.is_empty());
    let sample: Value = serde_json::from_slice(&fs::read(&sample_path).unwrap()).unwrap();
    assert_eq!(sample["kind"], "provider_baseline_sample");
    for (field, expected) in contract["expected"].as_object().unwrap() {
        assert_eq!(&sample[field], expected, "binding differs: {field}");
    }
    assert!(sample["elapsed_ms"].as_u64().unwrap() > 0);
    assert!(sample["peak_process_tree_rss_bytes"].as_u64().unwrap() > 0);
}
