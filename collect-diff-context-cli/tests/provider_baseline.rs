use collect_diff_context_cli::artifacts::contract::{
    canonical_json, sha256_bytes, ArtifactBaseline, ArtifactManifest,
};
use collect_diff_context_cli::artifacts::provider::{accept_p95, release_threshold_ms};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::Duration,
};

const SOURCE_LOCK_SHA256: &str = "298bc6c0339fe2c58fd35bfbd53db285ea7ff34e40734a4f0c36ccb3fe60d862";
const PLATFORMS: [&str; 4] = [
    "darwin-amd64",
    "darwin-arm64",
    "linux-amd64",
    "windows-amd64",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn fixture_root() -> PathBuf {
    repo_root().join("tests/fixtures/provider-release")
}

fn run_generator(fixture: &Path) -> Output {
    Command::new("python3")
        .arg(repo_root().join("scripts/generate_provider_manifest_update.py"))
        .arg("--fixture")
        .arg(fixture)
        .output()
        .unwrap()
}

fn run_generator_in_core_release(fixture: &Path) -> Output {
    Command::new("python3")
        .arg(repo_root().join("scripts/generate_provider_manifest_update.py"))
        .arg("--fixture")
        .arg(fixture)
        .env("PCR_CORE_RELEASE_JOB", "1")
        .output()
        .unwrap()
}

fn run_generator_in_named_core_workflow(fixture: &Path) -> Output {
    Command::new("python3")
        .arg(repo_root().join("scripts/generate_provider_manifest_update.py"))
        .arg("--fixture")
        .arg(fixture)
        .env("GITHUB_ACTIONS", "true")
        .env("GITHUB_WORKFLOW", "Release Multi-Platform Packs")
        .output()
        .unwrap()
}

fn copy_generator_fixture() -> tempfile::TempDir {
    let temporary = tempfile::tempdir().unwrap();
    for name in ["reviewed-baseline.json", "verified-publication.json"] {
        fs::copy(fixture_root().join(name), temporary.path().join(name)).unwrap();
    }
    temporary
}

fn mutate_json(path: &Path, mutate: impl FnOnce(&mut Value)) {
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    mutate(&mut value);
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn assert_rejected(fixture: &Path, expected_code: &str) {
    let output = run_generator(fixture);
    assert!(!output.status.success(), "generator unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(expected_code),
        "expected {expected_code} rejection, stderr: {stderr}"
    );
}

fn python_executable() -> String {
    let output = Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(output.status.success());
    fs::canonicalize(String::from_utf8(output.stdout).unwrap().trim())
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

fn fake_runner_executable() -> PathBuf {
    fs::canonicalize(env::current_exe().unwrap()).unwrap()
}

fn fake_runner_command(executable: &Path) -> Value {
    json!([
        executable,
        "--exact",
        "measurement_fake_runner_process",
        "--nocapture"
    ])
}

#[test]
fn measurement_fake_runner_process() {
    let Some(mode) = env::var_os("PCR_FAKE_RUNNER_MODE") else {
        return;
    };
    match mode.to_string_lossy().as_ref() {
        "sample" => fake_runner_sample(),
        "timeout" => fake_runner_timeout(),
        "descendant" => {
            thread::sleep(Duration::from_millis(800));
            fs::write(env::var_os("PCR_FAKE_MARKER").unwrap(), b"survived").unwrap();
        }
        value => panic!("unknown fake runner mode: {value}"),
    }
}

fn fake_runner_sample() {
    let state = PathBuf::from(env::var_os("PCR_FAKE_STATE").unwrap());
    let count = fs::read_to_string(&state)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    fs::write(&state, (count + 1).to_string()).unwrap();
    if count == 0 {
        if let (Some(original), Some(replacement)) = (
            env::var_os("PCR_FAKE_ORIGINAL_RUNNER"),
            env::var_os("PCR_FAKE_REPLACEMENT_RUNNER"),
        ) {
            fs::rename(replacement, original).unwrap();
        }
    }
    let mut sample: Value = serde_json::from_str(&env::var("PCR_FAKE_SAMPLE").unwrap()).unwrap();
    if count == 1 {
        if let Ok(field) = env::var("PCR_FAKE_DRIFT_FIELD") {
            sample[&field] = json!("0".repeat(64));
        }
    }
    let elapsed: Vec<u64> = serde_json::from_str(&env::var("PCR_FAKE_ELAPSED").unwrap()).unwrap();
    sample["elapsed_ms"] = json!(elapsed[count]);
    sample["peak_process_tree_rss_bytes"] = json!(268_435_456_u64 + count as u64);
    let mut bytes = serde_json::to_vec(&sample).unwrap();
    if env::var("PCR_FAKE_OUTPUT_MODE").as_deref() == Ok("noncanonical") {
        bytes.push(b'\n');
    }
    fs::write(
        env::var_os("PCR_PROVIDER_BASELINE_SAMPLE_OUTPUT").unwrap(),
        bytes,
    )
    .unwrap();
}

fn fake_runner_timeout() {
    if let Some(start) = env::var_os("PCR_FAKE_DESCENDANT_START") {
        let start = PathBuf::from(start);
        while !start.exists() {
            thread::sleep(Duration::from_millis(1));
        }
    }
    let mut child = Command::new(env::current_exe().unwrap());
    child
        .args(["--exact", "measurement_fake_runner_process", "--nocapture"])
        .env("PCR_FAKE_RUNNER_MODE", "descendant");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            child.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        child.creation_flags(0x0000_0200);
    }
    let mut descendant = child.spawn().unwrap();
    if let Some(ready) = env::var_os("PCR_FAKE_DESCENDANT_READY") {
        fs::write(ready, b"ready").unwrap();
    }
    descendant.wait().unwrap();
    thread::sleep(Duration::from_secs(60));
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

fn measurement_runner(
    samples: &[u64],
    identity_overrides: BTreeMap<&str, Value>,
    drift_field: Option<&str>,
) -> tempfile::TempDir {
    let temporary = tempfile::tempdir().unwrap();
    let runner = fake_runner_executable();
    let mut sample = json!({
        "schema_version": 1,
        "kind": "provider_baseline_sample",
        "platform_id": current_platform(),
        "pack_version": "2026.07.27-pcr.3",
        "pack_sha256": "1".repeat(64),
        "executable_sha256": "2".repeat(64),
        "source_lock_sha256": SOURCE_LOCK_SHA256,
        "profile_sha256": "3".repeat(64),
        "fixture_id": "single-crate",
        "fixture_sha256": "4".repeat(64),
        "request_sha256": "5".repeat(64),
        "runner_class": format!("local-{}", current_platform()),
        "toolchain": "rust-1.95.0-locked",
        "timing_scope": "provider-run-only-v1",
        "provisioning_included": false
    });
    for (field, value) in identity_overrides {
        sample[field] = value;
    }
    let mut environment = json!({
        "PCR_FAKE_RUNNER_MODE": "sample",
        "PCR_FAKE_ELAPSED": serde_json::to_string(samples).unwrap(),
        "PCR_FAKE_SAMPLE": serde_json::to_string(&sample).unwrap(),
        "PCR_FAKE_STATE": temporary.path().join("state").to_str().unwrap()
    });
    if let Some(field) = drift_field {
        environment["PCR_FAKE_DRIFT_FIELD"] = json!(field);
    }
    let runner_contract = json!({
        "schema_version": 1,
        "kind": "provider_baseline_runner",
        "command": fake_runner_command(&runner),
        "current_directory": temporary.path().to_str().unwrap(),
        "environment": environment,
        "expected": sample.as_object().unwrap().iter()
            .filter(|(field, _)| !["schema_version", "kind"].contains(&field.as_str()))
            .map(|(field, value)| (field.clone(), value.clone()))
            .collect::<serde_json::Map<String, Value>>()
    });
    fs::write(
        temporary.path().join("runner.json"),
        serde_json::to_vec(&runner_contract).unwrap(),
    )
    .unwrap();
    temporary
}

fn run_measurement(runner: &Path, samples: usize) -> Output {
    run_measurement_with_arguments(runner, samples, &["--evidence-only-local"])
}

fn run_reviewed_measurement(runner: &Path, samples: usize) -> Output {
    run_measurement_with_arguments(runner, samples, &[])
}

fn run_measurement_with_arguments(
    runner: &Path,
    samples: usize,
    additional_arguments: &[&str],
) -> Output {
    measurement_command(runner, samples, additional_arguments)
        .output()
        .unwrap()
}

fn measurement_command(runner: &Path, samples: usize, additional_arguments: &[&str]) -> Command {
    let mut command = Command::new(python_executable());
    command
        .arg(repo_root().join("scripts/measure_provider_baseline.py"))
        .arg("--runner")
        .arg(runner)
        .arg("--samples")
        .arg(samples.to_string())
        .args(additional_arguments);
    command
}

#[test]
fn measurement_cli_rejects_a_hosted_identity_from_an_arbitrary_runner() {
    let mut overrides = BTreeMap::new();
    overrides.insert("runner_class", json!(hosted_runner_class()));
    let fixture = measurement_runner(&[1; 21], overrides, None);

    let output = run_reviewed_measurement(&fixture.path().join("runner.json"), 20);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("runner-provenance"));
}

#[cfg(unix)]
#[test]
fn measurement_cli_executes_the_validated_runner_after_the_contract_path_is_replaced() {
    let fixture = measurement_runner(&[1; 21], BTreeMap::new(), None);
    let runner_executable = fixture.path().join("validated-rust-runner");
    let replacement = fixture.path().join("replacement-runner");
    fs::copy(fake_runner_executable(), &runner_executable).unwrap();
    fs::copy(python_executable(), &replacement).unwrap();
    mutate_json(&fixture.path().join("runner.json"), |runner| {
        runner["command"][0] = json!(runner_executable);
        runner["environment"]["PCR_FAKE_ORIGINAL_RUNNER"] =
            json!(fixture.path().join("validated-rust-runner"));
        runner["environment"]["PCR_FAKE_REPLACEMENT_RUNNER"] = json!(replacement);
    });

    let output = run_measurement(&fixture.path().join("runner.json"), 20);

    assert!(
        output.status.success(),
        "measurement followed the replaced contract path: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(fixture.path().join("state")).unwrap(),
        "21"
    );
}

fn set_runner_environment(fixture: &Path, key: &str, value: &str) {
    mutate_json(&fixture.join("runner.json"), |runner| {
        runner["environment"][key] = json!(value);
    });
}

#[test]
fn measurement_cli_uses_internal_elapsed_metrics_and_nearest_rank_output() {
    let elapsed = std::iter::once(999).chain(1..=20).collect::<Vec<_>>();
    let fixture = measurement_runner(&elapsed, BTreeMap::new(), None);
    let output = run_measurement(&fixture.path().join("runner.json"), 20);
    assert!(
        output.status.success(),
        "measurement failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.ends_with(b"\n"));
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(serde_json::to_vec(&envelope).unwrap(), output.stdout);
    assert_eq!(envelope["baseline_eligible"], false);
    let measurement = &envelope["measurement"];
    assert_eq!(measurement["pack_version"], "2026.07.27-pcr.3");
    assert_eq!(measurement["source_lock_sha256"], SOURCE_LOCK_SHA256);
    assert_eq!(
        measurement["samples_ms"],
        json!((1..=20).collect::<Vec<_>>())
    );
    assert_eq!(measurement["p95_ms"], 19);
    assert_eq!(measurement["peak_process_tree_rss_bytes"], 268435476u64);
    assert_eq!(measurement["timing_scope"], "provider-run-only-v1");
    assert_eq!(measurement["provisioning_included"], false);
}

#[test]
fn measurement_cli_rejects_sample_policy_identity_and_timing_drift() {
    let fixture = measurement_runner(&[1; 20], BTreeMap::new(), None);
    let output = run_measurement(&fixture.path().join("runner.json"), 19);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("measurement-samples"));

    for field in [
        "pack_sha256",
        "executable_sha256",
        "source_lock_sha256",
        "profile_sha256",
        "fixture_sha256",
        "request_sha256",
    ] {
        let elapsed = vec![1; 21];
        let fixture = measurement_runner(&elapsed, BTreeMap::new(), Some(field));
        let output = run_measurement(&fixture.path().join("runner.json"), 20);
        assert!(
            !output.status.success(),
            "measurement accepted drift in {field}"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("baseline-binding"));
    }

    for (field, replacement) in [
        ("runner_class", json!("different-runner")),
        ("provisioning_included", json!(true)),
    ] {
        let mut overrides = BTreeMap::new();
        overrides.insert(field, replacement);
        let fixture = measurement_runner(&[1; 21], overrides, None);
        let output = run_measurement(&fixture.path().join("runner.json"), 20);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("baseline-binding"));
    }

    let fixture = measurement_runner(
        &[
            1, 30_001, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        ],
        BTreeMap::new(),
        None,
    );
    let output = run_measurement(&fixture.path().join("runner.json"), 20);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("measurement-deadline"));
}

#[test]
fn measurement_cli_rejects_untrusted_runner_and_sample_boundaries() {
    let core_release = measurement_runner(&[1; 21], BTreeMap::new(), None);
    let output = Command::new("python3")
        .arg(repo_root().join("scripts/measure_provider_baseline.py"))
        .arg("--runner")
        .arg(core_release.path().join("runner.json"))
        .arg("--samples")
        .arg("20")
        .env("PCR_CORE_RELEASE_JOB", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("core-release-boundary"));

    let noncanonical_runner = measurement_runner(&[1; 21], BTreeMap::new(), None);
    let path = noncanonical_runner.path().join("runner.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b'\n');
    fs::write(&path, bytes).unwrap();
    let output = run_measurement(&path, 20);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("runner-contract"));

    let oversized_runner = measurement_runner(&[1; 21], BTreeMap::new(), None);
    let path = oversized_runner.path().join("runner.json");
    fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
    let output = run_measurement(&path, 20);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("runner-contract"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("outside its byte limit"));

    #[cfg(unix)]
    {
        let symlink_runner = measurement_runner(&[1; 21], BTreeMap::new(), None);
        let link = symlink_runner.path().join("runner-link.json");
        std::os::unix::fs::symlink(symlink_runner.path().join("runner.json"), &link).unwrap();
        let output = run_measurement(&link, 20);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("runner-contract"));
    }

    let relative_command = measurement_runner(&[1; 21], BTreeMap::new(), None);
    mutate_json(&relative_command.path().join("runner.json"), |runner| {
        runner["command"][0] = json!("python3");
    });
    let output = run_measurement(&relative_command.path().join("runner.json"), 20);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("runner-contract"));

    let invalid_environment = measurement_runner(&[1; 21], BTreeMap::new(), None);
    set_runner_environment(invalid_environment.path(), "INVALID=NAME", "value");
    let output = run_measurement(&invalid_environment.path().join("runner.json"), 20);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("runner environment is invalid"),
        "invalid environment name was not rejected during contract validation: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let noncanonical_sample = measurement_runner(&[1; 21], BTreeMap::new(), None);
    set_runner_environment(
        noncanonical_sample.path(),
        "PCR_FAKE_OUTPUT_MODE",
        "noncanonical",
    );
    let output = run_measurement(&noncanonical_sample.path().join("runner.json"), 20);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("sample-output"));
}

#[test]
fn measurement_cli_keeps_local_evidence_out_of_reviewed_baselines() {
    let rejected = measurement_runner(&[1; 21], BTreeMap::new(), None);
    let output = run_reviewed_measurement(&rejected.path().join("runner.json"), 20);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("baseline-binding"));

    let evidence = measurement_runner(&[1; 21], BTreeMap::new(), None);
    let output = run_measurement(&evidence.path().join("runner.json"), 20);
    assert!(
        output.status.success(),
        "local evidence failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(serde_json::to_vec(&envelope).unwrap(), output.stdout);
    assert_eq!(envelope["schema_version"], 1);
    assert_eq!(envelope["kind"], "provider_baseline_local_evidence");
    assert_eq!(envelope["baseline_eligible"], false);
    assert_eq!(envelope["reason"], "non-hosted-runner");
    assert_eq!(
        envelope["measurement"]["runner_class"],
        format!("local-{}", current_platform())
    );
    assert_eq!(envelope["measurement"]["p95_ms"], 1);
}

#[cfg(unix)]
#[test]
fn measurement_timeout_terminates_runner_descendants() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = measurement_runner(&[1; 21], BTreeMap::new(), None);
    let marker = fixture.path().join("descendant.marker");
    let descendant_start = fixture.path().join("descendant.start");
    let descendant_ready = fixture.path().join("descendant.ready");
    let process_snapshot = fixture.path().join("process.snapshot");
    let fake_ps = fixture.path().join("ps");
    fs::write(
        &fake_ps,
        b"#!/bin/sh\n\"$PCR_FAKE_REAL_PS\" \"$@\" > \"$PCR_FAKE_PS_SNAPSHOT\"\n: > \"$PCR_FAKE_DESCENDANT_START\"\nwhile [ ! -e \"$PCR_FAKE_DESCENDANT_READY\" ]; do :; done\n\"$PCR_FAKE_REAL_CAT\" \"$PCR_FAKE_PS_SNAPSHOT\"\n",
    )
    .unwrap();
    fs::set_permissions(&fake_ps, fs::Permissions::from_mode(0o755)).unwrap();
    set_runner_environment(fixture.path(), "PCR_FAKE_RUNNER_MODE", "timeout");
    set_runner_environment(fixture.path(), "PCR_FAKE_MARKER", marker.to_str().unwrap());
    set_runner_environment(
        fixture.path(),
        "PCR_FAKE_DESCENDANT_START",
        descendant_start.to_str().unwrap(),
    );
    set_runner_environment(
        fixture.path(),
        "PCR_FAKE_DESCENDANT_READY",
        descendant_ready.to_str().unwrap(),
    );

    let mut command = measurement_command(
        &fixture.path().join("runner.json"),
        20,
        &["--evidence-only-local", "--runner-timeout-seconds", "0.2"],
    );
    let output = command
        .env("PATH", fixture.path())
        .env("PCR_FAKE_REAL_PS", "/bin/ps")
        .env("PCR_FAKE_REAL_CAT", "/bin/cat")
        .env("PCR_FAKE_PS_SNAPSHOT", &process_snapshot)
        .env("PCR_FAKE_DESCENDANT_START", &descendant_start)
        .env("PCR_FAKE_DESCENDANT_READY", &descendant_ready)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("runner-timeout"));
    thread::sleep(Duration::from_millis(1_100));
    assert!(
        !marker.exists(),
        "runner descendant survived measurement timeout"
    );

    let output = run_measurement_with_arguments(
        &fixture.path().join("runner.json"),
        20,
        &["--runner-timeout-seconds", "0.2"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("runner-timeout-policy"));
}

#[cfg(windows)]
#[test]
fn measurement_timeout_terminates_runner_descendants() {
    let fixture = measurement_runner(&[1; 21], BTreeMap::new(), None);
    let marker = fixture.path().join("descendant.marker");
    set_runner_environment(fixture.path(), "PCR_FAKE_RUNNER_MODE", "timeout");
    set_runner_environment(fixture.path(), "PCR_FAKE_MARKER", marker.to_str().unwrap());

    let output = run_measurement_with_arguments(
        &fixture.path().join("runner.json"),
        20,
        &["--evidence-only-local", "--runner-timeout-seconds", "0.2"],
    );

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("runner-timeout"));
    thread::sleep(Duration::from_millis(1_100));
    assert!(
        !marker.exists(),
        "runner descendant survived measurement timeout"
    );
}

#[test]
fn release_threshold_uses_checked_integer_ceiling_policy() {
    assert_eq!(release_threshold_ms(1001).unwrap(), 1502);
    assert!(accept_p95(1502, 1001).unwrap());
    assert!(!accept_p95(1503, 1001).unwrap());
}

#[test]
fn release_threshold_rejects_arithmetic_overflow() {
    let error = release_threshold_ms(u64::MAX).unwrap_err();
    assert_eq!(error.code, "baseline-threshold-overflow");

    let error = accept_p95(1, u64::MAX).unwrap_err();
    assert_eq!(error.code, "baseline-threshold-overflow");
}

#[test]
fn release_threshold_rejects_a_zero_baseline() {
    let error = release_threshold_ms(0).unwrap_err();
    assert_eq!(error.code, "baseline-threshold-range");
}

#[test]
fn synthetic_reviewed_baseline_is_canonical_and_policy_valid() {
    let path = fixture_root().join("reviewed-baseline.json");
    let bytes = fs::read(path).unwrap();
    assert!(!bytes.ends_with(b"\n"));
    let baseline: ArtifactBaseline = serde_json::from_slice(&bytes).unwrap();
    baseline.validate().unwrap();
    assert_eq!(canonical_json(&baseline).unwrap(), bytes);
    assert_eq!(baseline.source_lock_sha256, SOURCE_LOCK_SHA256);
    assert_eq!(baseline.measurements.len(), PLATFORMS.len());
    assert_eq!(
        baseline
            .measurements
            .iter()
            .map(|measurement| measurement.platform_id.as_str())
            .collect::<Vec<_>>(),
        PLATFORMS
    );
    for measurement in &baseline.measurements {
        assert_eq!(measurement.toolchain, "rust-1.95.0-locked");
        assert_eq!(measurement.timing_scope, "provider-run-only-v1");
        assert!(!measurement.provisioning_included);
    }
}

#[test]
fn generator_emits_a_four_platform_review_candidate_without_mutating_manifest() {
    let manifest_path = repo_root().join("third_party_artifacts/manifest.json");
    let manifest_before = fs::read(&manifest_path).unwrap();
    let baseline_bytes = fs::read(fixture_root().join("reviewed-baseline.json")).unwrap();
    let publication: Value = serde_json::from_slice(
        &fs::read(fixture_root().join("verified-publication.json")).unwrap(),
    )
    .unwrap();

    let output = run_generator(&fixture_root());
    assert!(
        output.status.success(),
        "generator failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.ends_with(b"\n"));
    let candidate: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(serde_json::to_vec(&candidate).unwrap(), output.stdout);
    assert_eq!(candidate["kind"], "provider_manifest_update_candidate");
    assert_eq!(candidate["synthetic_fixture_only"], true);
    assert_eq!(
        candidate["quality_baseline_sha256"],
        sha256_bytes(&baseline_bytes)
    );
    assert_eq!(candidate["platforms"].as_array().unwrap().len(), 4);
    assert_eq!(
        candidate["manifest_candidate"]["packs"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let manifest: ArtifactManifest =
        serde_json::from_value(candidate["manifest_candidate"].clone()).unwrap();
    manifest.validate().unwrap();

    for (generated, published) in candidate["platforms"]
        .as_array()
        .unwrap()
        .iter()
        .zip(publication["platforms"].as_array().unwrap())
    {
        assert_eq!(generated["platform_id"], published["platform_id"]);
        assert_eq!(
            generated["pack_asset_name"],
            published["subjects"][0]["name"]
        );
        assert_eq!(generated["pack_sha256"], published["subjects"][0]["sha256"]);
        assert_eq!(
            generated["pack_manifest_asset_name"],
            published["subjects"][1]["name"]
        );
        assert_eq!(
            generated["pack_manifest_sha256"],
            published["subjects"][1]["sha256"]
        );
        assert_eq!(
            generated["sbom_asset_name"],
            published["subjects"][2]["name"]
        );
        assert_eq!(generated["sbom_sha256"], published["subjects"][2]["sha256"]);
        assert_eq!(
            generated["executable_sha256"],
            published["executable"]["sha256"]
        );
        assert_eq!(generated["source_lock_sha256"], SOURCE_LOCK_SHA256);
        assert_eq!(
            generated["quality_baseline_sha256"],
            candidate["quality_baseline_sha256"]
        );
    }

    assert_eq!(fs::read(manifest_path).unwrap(), manifest_before);
}

#[test]
fn generator_refuses_unpublished_or_incompletely_attested_platforms() {
    let unpublished = copy_generator_fixture();
    mutate_json(
        &unpublished.path().join("verified-publication.json"),
        |value| value["platforms"][0]["published"] = json!(false),
    );
    assert_rejected(unpublished.path(), "publication-state");

    let missing_attestation = copy_generator_fixture();
    mutate_json(
        &missing_attestation.path().join("verified-publication.json"),
        |value| {
            value["platforms"][0]["subjects"][0]
                .as_object_mut()
                .unwrap()
                .remove("attestation");
        },
    );
    assert_rejected(missing_attestation.path(), "attestation-contract");
}

#[test]
fn generator_refuses_missing_internal_manifest_or_sbom_digests() {
    for role in ["manifest", "sbom"] {
        let fixture = copy_generator_fixture();
        mutate_json(&fixture.path().join("verified-publication.json"), |value| {
            let subject = value["platforms"][0]["subjects"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|subject| subject["role"] == role)
                .unwrap();
            subject.as_object_mut().unwrap().remove("sha256");
        });
        assert_rejected(fixture.path(), "publication-digest");
    }
}

#[test]
fn generator_refuses_source_lock_and_every_baseline_binding_drift() {
    let source_lock = copy_generator_fixture();
    mutate_json(
        &source_lock.path().join("verified-publication.json"),
        |value| value["source_lock_sha256"] = json!("0".repeat(64)),
    );
    assert_rejected(source_lock.path(), "source-lock-binding");

    let mutations = [
        ("pack_sha256", "0".repeat(64)),
        ("executable_sha256", "1".repeat(64)),
        ("runner_sha256", "0".repeat(64)),
        ("profile_sha256", "2".repeat(64)),
        ("fixture_id", "different-fixture".to_string()),
        ("fixture_sha256", "3".repeat(64)),
        ("request_sha256", "4".repeat(64)),
        ("runner_class", "different-runner".to_string()),
        ("toolchain", "different-toolchain".to_string()),
        ("timing_scope", "different-scope".to_string()),
    ];
    for (field, replacement) in mutations {
        let fixture = copy_generator_fixture();
        mutate_json(&fixture.path().join("reviewed-baseline.json"), |value| {
            value["measurements"][0][field] = json!(replacement)
        });
        assert_rejected(fixture.path(), "baseline-binding");
    }

    let provisioning = copy_generator_fixture();
    mutate_json(
        &provisioning.path().join("reviewed-baseline.json"),
        |value| value["measurements"][0]["provisioning_included"] = json!(true),
    );
    assert_rejected(provisioning.path(), "baseline-binding");

    for (field, replacement) in [
        ("source_lock_sha256", "5".repeat(64)),
        ("pack_version", "2026.07.27-pcr.changed".to_string()),
    ] {
        let fixture = copy_generator_fixture();
        mutate_json(&fixture.path().join("reviewed-baseline.json"), |value| {
            value[field] = json!(replacement)
        });
        assert_rejected(fixture.path(), "baseline-binding");
    }
}

#[test]
fn generator_rejects_matching_non_hosted_runner_bindings() {
    let fixture = copy_generator_fixture();
    mutate_json(&fixture.path().join("verified-publication.json"), |value| {
        value["platforms"][2]["baseline_binding"]["runner_class"] = json!("local-linux-amd64");
    });
    mutate_json(&fixture.path().join("reviewed-baseline.json"), |value| {
        value["measurements"][2]["runner_class"] = json!("local-linux-amd64");
    });

    assert_rejected(fixture.path(), "baseline-runner-class-policy");
}

#[test]
fn generator_refuses_noncanonical_publication_or_baseline_bytes() {
    let publication = copy_generator_fixture();
    let path = publication.path().join("verified-publication.json");
    let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    assert_rejected(publication.path(), "canonical-json");

    let baseline = copy_generator_fixture();
    let path = baseline.path().join("reviewed-baseline.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b'\n');
    fs::write(path, bytes).unwrap();
    assert_rejected(baseline.path(), "canonical-json");
}

#[test]
fn generator_rejects_malformed_list_entries_with_stable_contract_errors() {
    let platform = copy_generator_fixture();
    mutate_json(
        &platform.path().join("verified-publication.json"),
        |value| value["platforms"][0] = json!("not-an-object"),
    );
    assert_rejected(platform.path(), "publication-contract");

    let subject = copy_generator_fixture();
    mutate_json(&subject.path().join("verified-publication.json"), |value| {
        value["platforms"][0]["subjects"][0] = json!("not-an-object")
    });
    assert_rejected(subject.path(), "attestation-contract");

    let measurement = copy_generator_fixture();
    mutate_json(
        &measurement.path().join("reviewed-baseline.json"),
        |value| value["measurements"][0] = json!("not-an-object"),
    );
    assert_rejected(measurement.path(), "baseline-binding");
}

#[test]
fn generator_refuses_a_boolean_nearest_rank_p95() {
    let fixture = copy_generator_fixture();
    mutate_json(&fixture.path().join("reviewed-baseline.json"), |value| {
        value["measurements"][0]["samples_ms"] = json!(vec![1; 20]);
        value["measurements"][0]["p95_ms"] = json!(true);
    });
    assert_rejected(fixture.path(), "baseline-binding");
}

#[test]
fn core_release_context_cannot_generate_or_rewrite_the_candidate() {
    let manifest_path = repo_root().join("third_party_artifacts/manifest.json");
    let manifest_before = fs::read(&manifest_path).unwrap();
    let output = run_generator_in_core_release(&fixture_root());
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("core-release-boundary"));
    let named_workflow = run_generator_in_named_core_workflow(&fixture_root());
    assert!(!named_workflow.status.success());
    assert!(String::from_utf8_lossy(&named_workflow.stderr).contains("core-release-boundary"));
    assert_eq!(fs::read(manifest_path).unwrap(), manifest_before);

    let workflow = fs::read_to_string(repo_root().join(".github/workflows/release.yml")).unwrap();
    assert!(!workflow.contains("generate_provider_manifest_update.py"));
}
