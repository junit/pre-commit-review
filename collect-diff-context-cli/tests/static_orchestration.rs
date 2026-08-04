#[cfg(unix)]
use collect_diff_context_cli::review_scope::{
    open_authoritative_scope, ReviewSource, ScopeRequest,
};
use collect_diff_context_cli::static_analysis::contracts::{
    InvalidationReason, NotRunReason, OrchestrationArtifact, OrchestrationManifest,
    OrchestrationRun, OrchestrationStatus, StaticAnalysisEvidence,
};
#[cfg(unix)]
use collect_diff_context_cli::static_analysis::orchestration::{
    execute, prepare_orchestration, OrchestrationRequest,
};
use serde_json::{json, Value};
#[cfg(unix)]
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use tempfile::TempDir;

const SCOPE_FINGERPRINT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const MANIFEST_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PROFILE_SHA256: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const EXECUTABLE_SHA256: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const SNAPSHOT_SHA256: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const STDOUT_SHA256: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const STDERR_SHA256: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
const EXECUTION_ID: &str = "1111111111111111";
const REPORT_ID: &str = "2222222222222222";

fn valid_manifest() -> Value {
    let profile_path = if cfg!(windows) {
        r"C:\review\profiles\security.json"
    } else {
        "/opt/review/profiles/security.json"
    };
    json!({
        "schema_version": 1,
        "kind": "static_analysis_orchestration_manifest",
        "name": "trusted pre-commit analyzer set",
        "profiles": [
            {
                "profile_id": "security",
                "path": profile_path,
                "sha256": PROFILE_SHA256
            }
        ],
        "limits": {
            "max_execution_seconds": 600,
            "max_captured_output_bytes": 30000000,
            "max_findings": 5000,
            "max_snapshot_bytes": 536870912,
            "max_snapshot_files": 100000
        }
    })
}

fn scope() -> Value {
    json!({
        "source": "staged",
        "head": "0123456789abcdef0123456789abcdef01234567",
        "fingerprint": SCOPE_FINGERPRINT
    })
}

fn valid_execution() -> Value {
    json!({
        "schema_version": 1,
        "kind": "static_analysis_execution",
        "authoritative": true,
        "execution_id": EXECUTION_ID,
        "scope": scope(),
        "profile": {
            "profile_id": "bbbbbbbbbbbbbbbb",
            "sha256": PROFILE_SHA256,
            "name": "fixture profile",
            "output_format": "normalized-json",
            "success_exit_codes": [0],
            "limits": {
                "timeout_seconds": 30,
                "max_output_bytes": 1048576,
                "max_snapshot_bytes": 10485760,
                "max_snapshot_files": 1000
            },
            "repository_configuration": "disabled",
            "network_access": "offline-required"
        },
        "tool": {"name": "fixture", "version": "1.0"},
        "executable": {
            "name": "fixture-analyzer",
            "sha256": EXECUTABLE_SHA256,
            "path_policy": "absolute-explicit-outside-repository"
        },
        "snapshot": {
            "kind": "temporary-tracked-files",
            "sha256": SNAPSHOT_SHA256,
            "files": 1,
            "bytes": 9
        },
        "isolation": {
            "shell": false,
            "vcs_metadata": false,
            "environment": "allowlist",
            "source_tree": "read-only-temporary-snapshot",
            "original_repository_path": "not-exposed",
            "network": "best-effort-offline-profile-required"
        },
        "execution": {
            "status": "completed",
            "exit_code": 0,
            "duration_ms": 10,
            "stdout_bytes": 10,
            "stdout_sha256": STDOUT_SHA256,
            "stderr_bytes": 0,
            "stderr_sha256": STDERR_SHA256,
            "result_accepted": true,
            "failure_reason": null
        },
        "evidence": {"report_ids": [REPORT_ID]}
    })
}

fn valid_evidence() -> Value {
    json!({
        "schema_version": 1,
        "kind": "static_analysis_evidence",
        "authoritative": true,
        "scope": scope(),
        "reports": [
            {
                "report_id": REPORT_ID,
                "format": "normalized-json",
                "tool": {"name": "fixture", "version": "1.0"},
                "status": "completed",
                "trust": "controlled-execution",
                "scope_binding": "controlled-execution",
                "execution_id": EXECUTION_ID,
                "finding_count": 0
            }
        ],
        "counts": {
            "reports": 1,
            "input_findings": 0,
            "deduplicated_findings": 0,
            "mapped_to_units": 0,
            "added_line": 0,
            "blocking_candidates": 0,
            "priority_candidates": 0,
            "notes": 0,
            "outside_scope": 0
        },
        "findings": [],
        "truncated": false,
        "decision_contract": {
            "blocking": "verify",
            "non_blocking": "record",
            "verification": "independent",
            "finalization": "revalidate"
        }
    })
}

fn budget(initial: u64, consumed: u64) -> Value {
    json!({
        "initial": initial,
        "consumed": consumed,
        "remaining": initial - consumed
    })
}

fn valid_artifact() -> Value {
    json!({
        "schema_version": 1,
        "kind": "static_analysis_orchestration",
        "authoritative": true,
        "orchestration_id": "3333333333333333",
        "scope": scope(),
        "manifest": {
            "manifest_id": "aaaaaaaaaaaaaaaa",
            "name": "trusted pre-commit analyzer set",
            "sha256": MANIFEST_SHA256
        },
        "snapshot": {
            "snapshot_id": "dddddddddddddddd",
            "kind": "temporary-tracked-files",
            "sha256": SNAPSHOT_SHA256,
            "files": 1,
            "bytes": 9
        },
        "status": "completed",
        "budgets": {
            "execution_millis": budget(600000, 10),
            "captured_output_bytes": budget(30000000, 10),
            "findings": budget(5000, 0),
            "snapshot_files": budget(100000, 1),
            "snapshot_bytes": budget(536870912, 9)
        },
        "runs": [
            {
                "run_kind": "executed",
                "profile_id": "security",
                "execution": valid_execution()
            }
        ],
        "report_ids": [REPORT_ID],
        "finding_ids": []
    })
}

fn parse_manifest(value: Value) -> OrchestrationManifest {
    serde_json::from_value(value).unwrap()
}

fn parse_artifact(value: Value) -> OrchestrationArtifact {
    serde_json::from_value(value).unwrap()
}

fn parse_evidence(value: Value) -> StaticAnalysisEvidence {
    serde_json::from_value(value).unwrap()
}

#[test]
fn contracts_accept_valid_manifest_and_completed_artifact() {
    parse_manifest(valid_manifest()).validate().unwrap();

    let artifact = parse_artifact(valid_artifact());
    let evidence = parse_evidence(valid_evidence());
    artifact.validate(&evidence).unwrap();
}

#[test]
fn contracts_reject_unknown_fields_relative_paths_and_invalid_hashes() {
    let mut value = valid_manifest();
    value["unexpected"] = json!(true);
    assert!(serde_json::from_value::<OrchestrationManifest>(value).is_err());

    let mut value = valid_manifest();
    value["profiles"][0]["path"] = json!("relative/profile.json");
    assert!(parse_manifest(value).validate().is_err());

    for invalid_hash in ["ABCDEF", "aaaaaaaaaaaaaaaa"] {
        let mut value = valid_manifest();
        value["profiles"][0]["sha256"] = json!(invalid_hash);
        assert!(parse_manifest(value).validate().is_err());
    }
}

#[test]
fn contracts_reject_profile_cardinality_duplicates_and_budget_bounds() {
    let mut value = valid_manifest();
    value["profiles"] = json!([]);
    assert!(parse_manifest(value).validate().is_err());

    let mut value = valid_manifest();
    value["profiles"] = Value::Array(
        (0..17)
            .map(|index| {
                json!({
                    "profile_id": format!("profile-{index}"),
                    "path": format!("/opt/review/profiles/{index}.json"),
                    "sha256": format!("{index:064x}")
                })
            })
            .collect(),
    );
    assert!(parse_manifest(value).validate().is_err());

    let mut value = valid_manifest();
    value["profiles"] = json!([
        {
            "profile_id": "security",
            "path": "/opt/review/profiles/security.json",
            "sha256": PROFILE_SHA256
        },
        {
            "profile_id": "security",
            "path": "/opt/review/profiles/types.json",
            "sha256": EXECUTABLE_SHA256
        }
    ]);
    assert!(parse_manifest(value).validate().is_err());

    let mut value = valid_manifest();
    value["profiles"] = json!([
        {
            "profile_id": "security",
            "path": "/opt/review/profiles/security.json",
            "sha256": PROFILE_SHA256
        },
        {
            "profile_id": "security-copy",
            "path": "/opt/review/profiles/security.json",
            "sha256": PROFILE_SHA256
        }
    ]);
    assert!(parse_manifest(value).validate().is_err());

    for (field, invalid) in [
        ("max_execution_seconds", json!(0)),
        ("max_captured_output_bytes", json!(100000001)),
        ("max_findings", json!(0)),
        ("max_snapshot_bytes", json!(1048575)),
        ("max_snapshot_files", json!(200001)),
    ] {
        let mut value = valid_manifest();
        value["limits"][field] = invalid;
        assert!(parse_manifest(value).validate().is_err(), "field {field}");
    }
}

#[test]
fn contracts_reject_invalid_run_unions_and_inconsistent_status() {
    let mut value = valid_artifact();
    value["runs"][0] = json!({
        "run_kind": "not-run",
        "profile_id": "security",
        "reason": "budget-exhausted",
        "execution": valid_execution()
    });
    assert!(serde_json::from_value::<OrchestrationArtifact>(value).is_err());

    let mut value = valid_artifact();
    value["status"] = json!("failed");
    let artifact = parse_artifact(value);
    let evidence = parse_evidence(valid_evidence());
    assert!(artifact.validate(&evidence).is_err());

    let mut value = valid_artifact();
    value["runs"].as_array_mut().unwrap().push(json!({
        "run_kind": "not-run",
        "profile_id": "security",
        "reason": "budget-exhausted"
    }));
    value["status"] = json!("partial");
    let artifact = parse_artifact(value);
    assert!(artifact.validate(&evidence).is_err());
}

#[test]
fn contracts_allow_empty_evidence_only_when_no_run_executed() {
    let mut artifact_value = valid_artifact();
    artifact_value["status"] = json!("failed");
    artifact_value["runs"] = json!([
        {
            "run_kind": "invalidated",
            "profile_id": "security",
            "reason": "snapshot-mutated"
        },
        {
            "run_kind": "not-run",
            "profile_id": "types",
            "reason": "shared-integrity-failure"
        }
    ]);
    artifact_value["report_ids"] = json!([]);

    let mut evidence_value = valid_evidence();
    evidence_value["reports"] = json!([]);
    evidence_value["counts"]["reports"] = json!(0);

    let artifact = parse_artifact(artifact_value);
    let evidence = parse_evidence(evidence_value.clone());
    artifact.validate(&evidence).unwrap();

    let artifact = parse_artifact(valid_artifact());
    let empty_evidence = parse_evidence(evidence_value);
    assert!(artifact.validate(&empty_evidence).is_err());
}

#[cfg(unix)]
fn git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn preflight_repository() -> TempDir {
    let repository = TempDir::new().unwrap();
    git(repository.path(), &["init", "-q"]);
    git(
        repository.path(),
        &["config", "user.email", "review@example.test"],
    );
    git(repository.path(), &["config", "user.name", "Review Test"]);
    fs::write(repository.path().join("candidate.txt"), "base\n").unwrap();
    git(repository.path(), &["add", "candidate.txt"]);
    git(repository.path(), &["commit", "-qm", "base"]);
    fs::write(repository.path().join("candidate.txt"), "candidate\n").unwrap();
    git(repository.path(), &["add", "candidate.txt"]);
    repository
}

#[cfg(unix)]
fn sha256_file(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

#[cfg(unix)]
fn marker_executable(directory: &Path, name: &str, marker: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join(name);
    fs::write(
        &path,
        format!("#!/bin/sh\nprintf executed > '{}'\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn write_preflight_profile(
    directory: &Path,
    name: &str,
    executable: &Path,
    executable_sha256: &str,
    repository_configuration: &str,
) -> (PathBuf, String) {
    let path = directory.join(format!("{name}.json"));
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "static_analysis_profile",
            "name": format!("{name} profile"),
            "tool": {"name": name, "version": "1.0"},
            "executable": {
                "path": executable.to_string_lossy(),
                "sha256": executable_sha256
            },
            "arguments": [],
            "output_format": "normalized-json",
            "success_exit_codes": [0],
            "limits": {
                "timeout_seconds": 30,
                "max_output_bytes": 1048576,
                "max_snapshot_bytes": 10485760,
                "max_snapshot_files": 1000
            },
            "repository_configuration": repository_configuration,
            "network_access": "offline-required"
        }))
        .unwrap(),
    )
    .unwrap();
    let hash = sha256_file(&path);
    (path, hash)
}

#[cfg(unix)]
fn write_preflight_manifest(
    directory: &Path,
    profiles: &[(&str, &Path, &str)],
) -> (PathBuf, String) {
    let path = directory.join("manifest.json");
    let profile_values = profiles
        .iter()
        .map(|(profile_id, path, sha256)| {
            json!({
                "profile_id": profile_id,
                "path": path.to_string_lossy(),
                "sha256": sha256
            })
        })
        .collect::<Vec<_>>();
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "static_analysis_orchestration_manifest",
            "name": "trusted fixture analyzers",
            "profiles": profile_values,
            "limits": {
                "max_execution_seconds": 60,
                "max_captured_output_bytes": 10485760,
                "max_findings": 100,
                "max_snapshot_bytes": 10485760,
                "max_snapshot_files": 1000
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let hash = sha256_file(&path);
    (path, hash)
}

#[cfg(unix)]
fn preflight_request(
    repository: &Path,
    manifest_path: &Path,
    manifest_sha256: &str,
    allow_repository_configuration: bool,
) -> OrchestrationRequest {
    OrchestrationRequest {
        repository: repository.to_path_buf(),
        source: ReviewSource::Staged,
        expected_scope: SCOPE_FINGERPRINT.to_string(),
        manifest_path: manifest_path.to_path_buf(),
        expected_manifest_sha256: manifest_sha256.to_string(),
        allow_repository_configuration,
    }
}

#[cfg(unix)]
#[test]
fn preflight_rejects_manifest_hash_and_contract_before_execution() {
    let repository = preflight_repository();
    let fixtures = TempDir::new().unwrap();
    let marker = fixtures.path().join("executed.marker");
    let executable = marker_executable(fixtures.path(), "analyzer.sh", &marker);
    let executable_sha256 = sha256_file(&executable);
    let (profile, profile_sha256) = write_preflight_profile(
        fixtures.path(),
        "security",
        &executable,
        &executable_sha256,
        "disabled",
    );
    let (manifest, manifest_sha256) =
        write_preflight_manifest(fixtures.path(), &[("security", &profile, &profile_sha256)]);

    let wrong_hash = "0".repeat(64);
    assert!(prepare_orchestration(&preflight_request(
        repository.path(),
        &manifest,
        &wrong_hash,
        false,
    ))
    .is_err());

    let mut invalid_manifest: Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    invalid_manifest["limits"]["max_execution_seconds"] = json!(0);
    fs::write(&manifest, serde_json::to_vec(&invalid_manifest).unwrap()).unwrap();
    let invalid_manifest_sha256 = sha256_file(&manifest);
    assert!(prepare_orchestration(&preflight_request(
        repository.path(),
        &manifest,
        &invalid_manifest_sha256,
        false,
    ))
    .is_err());
    assert!(!marker.exists());

    fs::write(
        &manifest,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "static_analysis_orchestration_manifest",
            "name": "duplicate fixtures",
            "profiles": [
                {"profile_id": "security", "path": profile.to_string_lossy(), "sha256": profile_sha256},
                {"profile_id": "security-copy", "path": profile.to_string_lossy(), "sha256": profile_sha256}
            ],
            "limits": {
                "max_execution_seconds": 60,
                "max_captured_output_bytes": 10485760,
                "max_findings": 100,
                "max_snapshot_bytes": 10485760,
                "max_snapshot_files": 1000
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let duplicate_sha256 = sha256_file(&manifest);
    assert!(prepare_orchestration(&preflight_request(
        repository.path(),
        &manifest,
        &duplicate_sha256,
        false,
    ))
    .is_err());
    assert!(!marker.exists());

    assert_ne!(manifest_sha256, duplicate_sha256);
}

#[cfg(unix)]
#[test]
fn preflight_rejects_any_profile_or_entrypoint_before_execution() {
    let repository = preflight_repository();
    let fixtures = TempDir::new().unwrap();
    let first_marker = fixtures.path().join("first.marker");
    let second_marker = fixtures.path().join("second.marker");
    let first = marker_executable(fixtures.path(), "first.sh", &first_marker);
    let second = marker_executable(fixtures.path(), "second.sh", &second_marker);
    let first_hash = sha256_file(&first);
    let second_hash = sha256_file(&second);
    let (first_profile, first_profile_hash) =
        write_preflight_profile(fixtures.path(), "first", &first, &first_hash, "disabled");
    let (second_profile, _second_profile_hash) =
        write_preflight_profile(fixtures.path(), "second", &second, &second_hash, "disabled");

    let wrong_profile_hash = "0".repeat(64);
    let (manifest, manifest_sha256) = write_preflight_manifest(
        fixtures.path(),
        &[
            ("first", &first_profile, &first_profile_hash),
            ("second", &second_profile, &wrong_profile_hash),
        ],
    );
    assert!(prepare_orchestration(&preflight_request(
        repository.path(),
        &manifest,
        &manifest_sha256,
        false,
    ))
    .is_err());
    assert!(!first_marker.exists() && !second_marker.exists());

    let mut invalid_profile: Value =
        serde_json::from_slice(&fs::read(&second_profile).unwrap()).unwrap();
    invalid_profile["unexpected"] = json!(true);
    fs::write(
        &second_profile,
        serde_json::to_vec(&invalid_profile).unwrap(),
    )
    .unwrap();
    let invalid_profile_hash = sha256_file(&second_profile);
    let (manifest, manifest_sha256) = write_preflight_manifest(
        fixtures.path(),
        &[
            ("first", &first_profile, &first_profile_hash),
            ("second", &second_profile, &invalid_profile_hash),
        ],
    );
    assert!(prepare_orchestration(&preflight_request(
        repository.path(),
        &manifest,
        &manifest_sha256,
        false,
    ))
    .is_err());
    assert!(!first_marker.exists() && !second_marker.exists());

    let (second_profile, second_profile_hash) = write_preflight_profile(
        fixtures.path(),
        "second",
        &second,
        &"0".repeat(64),
        "disabled",
    );
    let (manifest, manifest_sha256) = write_preflight_manifest(
        fixtures.path(),
        &[
            ("first", &first_profile, &first_profile_hash),
            ("second", &second_profile, &second_profile_hash),
        ],
    );
    assert!(prepare_orchestration(&preflight_request(
        repository.path(),
        &manifest,
        &manifest_sha256,
        false,
    ))
    .is_err());
    assert!(!first_marker.exists() && !second_marker.exists());
}

#[cfg(unix)]
#[test]
fn preflight_requires_manifest_level_repository_configuration_authority() {
    let repository = preflight_repository();
    let fixtures = TempDir::new().unwrap();
    let marker = fixtures.path().join("trusted.marker");
    let executable = marker_executable(fixtures.path(), "trusted.sh", &marker);
    let executable_sha256 = sha256_file(&executable);
    let (profile, profile_sha256) = write_preflight_profile(
        fixtures.path(),
        "trusted",
        &executable,
        &executable_sha256,
        "explicitly-trusted",
    );
    let (manifest, manifest_sha256) =
        write_preflight_manifest(fixtures.path(), &[("trusted", &profile, &profile_sha256)]);
    assert!(prepare_orchestration(&preflight_request(
        repository.path(),
        &manifest,
        &manifest_sha256,
        false,
    ))
    .is_err());
    assert!(!marker.exists());

    let prepared = prepare_orchestration(&preflight_request(
        repository.path(),
        &manifest,
        &manifest_sha256,
        true,
    ))
    .unwrap();
    assert_eq!(prepared.profiles[0].profile_id, "trusted");
    assert!(!marker.exists());
}

#[cfg(unix)]
#[test]
fn preflight_rejects_unused_repository_configuration_authority() {
    let repository = preflight_repository();
    let fixtures = TempDir::new().unwrap();
    let marker = fixtures.path().join("disabled.marker");
    let executable = marker_executable(fixtures.path(), "disabled.sh", &marker);
    let executable_sha256 = sha256_file(&executable);
    let (profile, profile_sha256) = write_preflight_profile(
        fixtures.path(),
        "disabled",
        &executable,
        &executable_sha256,
        "disabled",
    );
    let (manifest, manifest_sha256) =
        write_preflight_manifest(fixtures.path(), &[("disabled", &profile, &profile_sha256)]);

    let error = prepare_orchestration(&preflight_request(
        repository.path(),
        &manifest,
        &manifest_sha256,
        true,
    ))
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("valid only when at least one profile is explicitly trusted"));
    assert!(!marker.exists());
}

#[cfg(unix)]
#[test]
fn preflight_revalidation_rejects_manifest_profile_and_entrypoint_drift() {
    for drift in ["manifest", "profile", "entrypoint"] {
        let repository = preflight_repository();
        let fixtures = TempDir::new().unwrap();
        let marker = fixtures.path().join(format!("{drift}.marker"));
        let executable = marker_executable(fixtures.path(), "analyzer.sh", &marker);
        let executable_sha256 = sha256_file(&executable);
        let (profile, profile_sha256) = write_preflight_profile(
            fixtures.path(),
            "security",
            &executable,
            &executable_sha256,
            "disabled",
        );
        let (manifest, manifest_sha256) =
            write_preflight_manifest(fixtures.path(), &[("security", &profile, &profile_sha256)]);
        let prepared = prepare_orchestration(&preflight_request(
            repository.path(),
            &manifest,
            &manifest_sha256,
            false,
        ))
        .unwrap();

        let changed = match drift {
            "manifest" => &manifest,
            "profile" => &profile,
            "entrypoint" => &executable,
            _ => unreachable!(),
        };
        fs::OpenOptions::new()
            .append(true)
            .open(changed)
            .unwrap()
            .write_all(b"\n")
            .unwrap();

        assert!(prepared.revalidate().is_err(), "drift={drift}");
        assert!(!marker.exists());
    }
}

#[cfg(unix)]
fn write_execution_profile(
    directory: &Path,
    name: &str,
    executable: &Path,
    arguments: &[&Path],
) -> (PathBuf, String) {
    let path = directory.join(format!("{name}-execution.json"));
    let argument_values = arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "static_analysis_profile",
            "name": format!("{name} execution profile"),
            "tool": {"name": name, "version": "1.0"},
            "executable": {
                "path": executable.to_string_lossy(),
                "sha256": sha256_file(executable)
            },
            "arguments": argument_values,
            "output_format": "normalized-json",
            "success_exit_codes": [0],
            "limits": {
                "timeout_seconds": 30,
                "max_output_bytes": 1048576,
                "max_snapshot_bytes": 10485760,
                "max_snapshot_files": 1000
            },
            "repository_configuration": "disabled",
            "network_access": "offline-required"
        }))
        .unwrap(),
    )
    .unwrap();
    let hash = sha256_file(&path);
    (path, hash)
}

#[cfg(unix)]
fn source_analyzer(directory: &Path, name: &str, mutate_snapshot: bool) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join(format!("{name}-analyzer.sh"));
    let mutation = if mutate_snapshot {
        "chmod u+w candidate.txt\nprintf 'mutated\\n' > candidate.txt\n"
    } else {
        ""
    };
    let body = format!(
        "#!/bin/sh\nset -eu\nlog_path=$1\nprintf '%s\\t%s\\t%s\\n' \"$PWD\" \"$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT\" \"$(cat candidate.txt)\" >> \"$log_path\"\n{mutation}printf '%s\\n' '{{\"schema_version\":1,\"kind\":\"static_analysis_input\",\"scope_fingerprint\":\"'\"$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT\"'\",\"tool\":{{\"name\":\"{name}\",\"version\":\"1.0\"}},\"status\":\"completed\",\"findings\":[]}}'\n"
    );
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn execution_request(
    repository: &Path,
    manifest_path: &Path,
    manifest_sha256: &str,
) -> OrchestrationRequest {
    let scope = open_authoritative_scope(ScopeRequest {
        repository: repository.to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_fingerprint: None,
    })
    .unwrap();
    OrchestrationRequest {
        repository: repository.to_path_buf(),
        source: ReviewSource::Staged,
        expected_scope: scope.fingerprint,
        manifest_path: manifest_path.to_path_buf(),
        expected_manifest_sha256: manifest_sha256.to_string(),
        allow_repository_configuration: false,
    }
}

#[cfg(unix)]
#[test]
fn shared_snapshot_is_materialized_once_for_all_profiles() {
    let repository = preflight_repository();
    let fixtures = TempDir::new().unwrap();
    let log = fixtures.path().join("snapshot.log");
    let first = source_analyzer(fixtures.path(), "first", false);
    let second = source_analyzer(fixtures.path(), "second", false);
    let (first_profile, first_hash) =
        write_execution_profile(fixtures.path(), "first", &first, &[&log]);
    let (second_profile, second_hash) =
        write_execution_profile(fixtures.path(), "second", &second, &[&log]);
    let (manifest, manifest_sha256) = write_preflight_manifest(
        fixtures.path(),
        &[
            ("first", &first_profile, &first_hash),
            ("second", &second_profile, &second_hash),
        ],
    );

    let output = execute(execution_request(
        repository.path(),
        &manifest,
        &manifest_sha256,
    ))
    .unwrap();

    assert_eq!(output.orchestration.status, OrchestrationStatus::Completed);
    assert_eq!(output.orchestration.runs.len(), 2);
    for run in &output.orchestration.runs {
        let OrchestrationRun::Executed { execution, .. } = run else {
            panic!("expected executed run: {run:?}");
        };
        assert_eq!(
            execution.snapshot.sha256,
            output.orchestration.snapshot.sha256
        );
        assert_eq!(
            execution.snapshot.files,
            output.orchestration.snapshot.files
        );
        assert_eq!(
            execution.snapshot.bytes,
            output.orchestration.snapshot.bytes
        );
    }
    assert_eq!(output.evidence.reports.len(), 2);

    let observations = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|line| line.split('\t').map(str::to_string).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0][0], observations[1][0]);
    assert_eq!(observations[0][1], observations[1][1]);
    assert_eq!(observations[0][1], output.orchestration.scope.fingerprint);
    assert_eq!(observations[0][2], "candidate");
    assert_eq!(observations[1][2], "candidate");
}

#[cfg(unix)]
#[test]
fn shared_snapshot_mutation_invalidates_current_and_stops_remaining_profiles() {
    let repository = preflight_repository();
    let fixtures = TempDir::new().unwrap();
    let log = fixtures.path().join("mutation.log");
    let mutating = source_analyzer(fixtures.path(), "mutating", true);
    let later = source_analyzer(fixtures.path(), "later", false);
    let (mutating_profile, mutating_hash) =
        write_execution_profile(fixtures.path(), "mutating", &mutating, &[&log]);
    let (later_profile, later_hash) =
        write_execution_profile(fixtures.path(), "later", &later, &[&log]);
    let (manifest, manifest_sha256) = write_preflight_manifest(
        fixtures.path(),
        &[
            ("mutating", &mutating_profile, &mutating_hash),
            ("later", &later_profile, &later_hash),
        ],
    );

    let output = execute(execution_request(
        repository.path(),
        &manifest,
        &manifest_sha256,
    ))
    .unwrap();

    assert_eq!(output.orchestration.status, OrchestrationStatus::Failed);
    assert!(matches!(
        &output.orchestration.runs[0],
        OrchestrationRun::Invalidated {
            profile_id,
            reason: InvalidationReason::SnapshotMutated
        } if profile_id == "mutating"
    ));
    assert!(matches!(
        &output.orchestration.runs[1],
        OrchestrationRun::NotRun {
            profile_id,
            reason: NotRunReason::SharedIntegrityFailure
        } if profile_id == "later"
    ));
    assert!(output.evidence.reports.is_empty());
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 1);
}

#[cfg(unix)]
fn write_budget_profile(
    directory: &Path,
    name: &str,
    executable: &Path,
    arguments: &[&Path],
    timeout_seconds: u64,
    max_output_bytes: usize,
) -> (PathBuf, String) {
    let path = directory.join(format!("{name}-budget.json"));
    let argument_values = arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "static_analysis_profile",
            "name": format!("{name} budget profile"),
            "tool": {"name": name, "version": "1.0"},
            "executable": {
                "path": executable.to_string_lossy(),
                "sha256": sha256_file(executable)
            },
            "arguments": argument_values,
            "output_format": "normalized-json",
            "success_exit_codes": [0],
            "limits": {
                "timeout_seconds": timeout_seconds,
                "max_output_bytes": max_output_bytes,
                "max_snapshot_bytes": 10485760,
                "max_snapshot_files": 1000
            },
            "repository_configuration": "disabled",
            "network_access": "offline-required"
        }))
        .unwrap(),
    )
    .unwrap();
    let hash = sha256_file(&path);
    (path, hash)
}

#[cfg(unix)]
fn write_budget_manifest(
    directory: &Path,
    profiles: &[(&str, &Path, &str)],
    max_execution_seconds: u64,
    max_captured_output_bytes: u64,
    max_findings: usize,
) -> (PathBuf, String) {
    let path = directory.join("budget-manifest.json");
    let profile_values = profiles
        .iter()
        .map(|(profile_id, path, sha256)| {
            json!({
                "profile_id": profile_id,
                "path": path.to_string_lossy(),
                "sha256": sha256
            })
        })
        .collect::<Vec<_>>();
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "static_analysis_orchestration_manifest",
            "name": "budget fixture analyzers",
            "profiles": profile_values,
            "limits": {
                "max_execution_seconds": max_execution_seconds,
                "max_captured_output_bytes": max_captured_output_bytes,
                "max_findings": max_findings,
                "max_snapshot_bytes": 10485760,
                "max_snapshot_files": 1000
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let hash = sha256_file(&path);
    (path, hash)
}

#[cfg(unix)]
fn raw_output_analyzer(directory: &Path, name: &str, bytes: usize) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join(format!("{name}-raw-output.sh"));
    fs::write(
        &path,
        format!("#!/bin/sh\nhead -c {bytes} /dev/zero | tr '\\000' x\n"),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn slow_analyzer(directory: &Path, name: &str, seconds: u64) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join(format!("{name}-slow.sh"));
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nsleep {seconds}\nprintf '%s\\n' '{{\"schema_version\":1,\"kind\":\"static_analysis_input\",\"scope_fingerprint\":\"'\"$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT\"'\",\"tool\":{{\"name\":\"{name}\",\"version\":\"1.0\"}},\"status\":\"completed\",\"findings\":[]}}'\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn finding_analyzer(directory: &Path, name: &str, findings: usize) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join(format!("{name}-findings.sh"));
    let finding_values = (0..findings)
        .map(|index| {
            json!({
                "rule_id": format!("{name}-R{index}"),
                "message": format!("{name} finding {index}"),
                "path": "candidate.txt",
                "start_line": 1,
                "end_line": 1,
                "severity": "warning",
                "category": "correctness",
                "confidence": "high",
                "baseline_state": "new"
            })
        })
        .collect::<Vec<_>>();
    let template = serde_json::to_string(&json!({
        "schema_version": 1,
        "kind": "static_analysis_input",
        "scope_fingerprint": "__SCOPE__",
        "tool": {"name": name, "version": "1.0"},
        "status": "completed",
        "findings": finding_values
    }))
    .unwrap();
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"${{0}}\" >/dev/null\nprintf '%s\\n' '{}' | sed \"s/__SCOPE__/$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT/\"\n",
            template.replace('\'', "'\\''")
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn budgets_enforce_cumulative_output_and_stop_remaining_profiles() {
    let repository = preflight_repository();
    let fixtures = TempDir::new().unwrap();
    let later_log = fixtures.path().join("later-output.log");
    let overflowing = raw_output_analyzer(fixtures.path(), "overflowing", 1025);
    let later = source_analyzer(fixtures.path(), "later-output", false);
    let (overflow_profile, overflow_hash) =
        write_budget_profile(fixtures.path(), "overflowing", &overflowing, &[], 30, 1024);
    let (later_profile, later_hash) = write_budget_profile(
        fixtures.path(),
        "later-output",
        &later,
        &[&later_log],
        30,
        1048576,
    );
    let (manifest, manifest_hash) = write_budget_manifest(
        fixtures.path(),
        &[
            ("overflowing", &overflow_profile, &overflow_hash),
            ("later-output", &later_profile, &later_hash),
        ],
        60,
        1025,
        100,
    );

    let output = execute(execution_request(
        repository.path(),
        &manifest,
        &manifest_hash,
    ))
    .unwrap();

    assert!(matches!(
        &output.orchestration.runs[0],
        OrchestrationRun::Executed { execution, .. }
            if execution.execution.status
                == collect_diff_context_cli::static_analysis::contracts::ExecutionStatus::OutputLimit
    ));
    assert!(matches!(
        &output.orchestration.runs[1],
        OrchestrationRun::NotRun {
            reason: NotRunReason::BudgetExhausted,
            ..
        }
    ));
    assert_eq!(
        output.orchestration.budgets.captured_output_bytes,
        serde_json::from_value(budget(1025, 1025)).unwrap()
    );
    assert!(!later_log.exists());
}

#[cfg(unix)]
#[test]
fn budgets_apply_effective_timeout_and_stop_remaining_profiles() {
    let repository = preflight_repository();
    let fixtures = TempDir::new().unwrap();
    let later_log = fixtures.path().join("later-time.log");
    let slow = slow_analyzer(fixtures.path(), "slow", 2);
    let later = source_analyzer(fixtures.path(), "later-time", false);
    let (slow_profile, slow_hash) =
        write_budget_profile(fixtures.path(), "slow", &slow, &[], 5, 1048576);
    let (later_profile, later_hash) = write_budget_profile(
        fixtures.path(),
        "later-time",
        &later,
        &[&later_log],
        30,
        1048576,
    );
    let (manifest, manifest_hash) = write_budget_manifest(
        fixtures.path(),
        &[
            ("slow", &slow_profile, &slow_hash),
            ("later-time", &later_profile, &later_hash),
        ],
        1,
        10485760,
        100,
    );

    let output = execute(execution_request(
        repository.path(),
        &manifest,
        &manifest_hash,
    ))
    .unwrap();

    assert!(matches!(
        &output.orchestration.runs[0],
        OrchestrationRun::Executed { execution, .. }
            if execution.execution.status
                == collect_diff_context_cli::static_analysis::contracts::ExecutionStatus::Timeout
    ));
    assert!(matches!(
        &output.orchestration.runs[1],
        OrchestrationRun::NotRun {
            reason: NotRunReason::BudgetExhausted,
            ..
        }
    ));
    assert_eq!(
        output.orchestration.budgets.execution_millis,
        serde_json::from_value(budget(1000, 1000)).unwrap()
    );
    assert!(!later_log.exists());
}

#[cfg(unix)]
#[test]
fn budgets_record_findings_and_shared_snapshot_exactly_once() {
    let repository = preflight_repository();
    let fixtures = TempDir::new().unwrap();
    let first = finding_analyzer(fixtures.path(), "first-findings", 2);
    let second = finding_analyzer(fixtures.path(), "second-findings", 2);
    let (first_profile, first_hash) =
        write_budget_profile(fixtures.path(), "first-findings", &first, &[], 30, 1048576);
    let (second_profile, second_hash) = write_budget_profile(
        fixtures.path(),
        "second-findings",
        &second,
        &[],
        30,
        1048576,
    );
    let (manifest, manifest_hash) = write_budget_manifest(
        fixtures.path(),
        &[
            ("first-findings", &first_profile, &first_hash),
            ("second-findings", &second_profile, &second_hash),
        ],
        60,
        10485760,
        3,
    );

    let output = execute(execution_request(
        repository.path(),
        &manifest,
        &manifest_hash,
    ))
    .unwrap();

    assert_eq!(output.evidence.counts.deduplicated_findings, 4);
    assert_eq!(output.evidence.findings.len(), 3);
    assert!(output.evidence.truncated);
    assert_eq!(
        output.orchestration.budgets.findings,
        serde_json::from_value(budget(3, 3)).unwrap()
    );
    assert_eq!(
        output.orchestration.budgets.snapshot_files.consumed,
        output.orchestration.snapshot.files as u64
    );
    assert_eq!(
        output.orchestration.budgets.snapshot_bytes.consumed,
        output.orchestration.snapshot.bytes
    );
}

#[cfg(unix)]
fn scheduler_analyzer(directory: &Path, name: &str, behavior: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join(format!("{name}-scheduler.sh"));
    let action = match behavior {
        "success" => format!(
            "printf '%s\\n' '{{\"schema_version\":1,\"kind\":\"static_analysis_input\",\"scope_fingerprint\":\"'\"$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT\"'\",\"tool\":{{\"name\":\"{name}\",\"version\":\"1.0\"}},\"status\":\"completed\",\"findings\":[]}}'\n"
        ),
        "failed" => "exit 7\n".to_string(),
        "timeout" => "sleep 2\n".to_string(),
        "output-limit" => "head -c 1025 /dev/zero | tr '\\000' x\n".to_string(),
        "invalid-output" => "printf 'not-json\\n'\n".to_string(),
        _ => panic!("unknown scheduler behavior: {behavior}"),
    };
    fs::write(
        &path,
        format!("#!/bin/sh\nset -eu\nlog_path=$1\nprintf '{name}\\n' >> \"$log_path\"\n{action}"),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn scheduler_continues_tool_local_failures_in_manifest_order() {
    use collect_diff_context_cli::static_analysis::contracts::ExecutionStatus;

    let repository = preflight_repository();
    let fixtures = TempDir::new().unwrap();
    let log = fixtures.path().join("scheduler.log");
    let cases = [
        ("failed", "failed", 30, 4096),
        ("timeout", "timeout", 1, 4096),
        ("output-limit", "output-limit", 30, 1024),
        ("invalid-output", "invalid-output", 30, 4096),
        ("accepted", "success", 30, 4096),
    ];
    let mut profile_paths = Vec::new();
    for (name, behavior, timeout, output) in cases {
        let analyzer = scheduler_analyzer(fixtures.path(), name, behavior);
        let (profile, hash) =
            write_budget_profile(fixtures.path(), name, &analyzer, &[&log], timeout, output);
        profile_paths.push((name.to_string(), profile, hash));
    }
    let manifest_profiles = profile_paths
        .iter()
        .map(|(name, path, hash)| (name.as_str(), path.as_path(), hash.as_str()))
        .collect::<Vec<_>>();
    let (manifest, manifest_hash) =
        write_budget_manifest(fixtures.path(), &manifest_profiles, 10, 10485760, 100);

    let output = execute(execution_request(
        repository.path(),
        &manifest,
        &manifest_hash,
    ))
    .unwrap();

    let statuses = output
        .orchestration
        .runs
        .iter()
        .map(|run| match run {
            OrchestrationRun::Executed { execution, .. } => execution.execution.status,
            other => panic!("expected executed run: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        vec![
            ExecutionStatus::Failed,
            ExecutionStatus::Timeout,
            ExecutionStatus::OutputLimit,
            ExecutionStatus::InvalidOutput,
            ExecutionStatus::Completed,
        ]
    );
    assert_eq!(output.orchestration.status, OrchestrationStatus::Partial);
    assert_eq!(output.evidence.reports.len(), 5);
    let manifest_order = [
        "failed",
        "timeout",
        "output-limit",
        "invalid-output",
        "accepted",
    ];
    let observed = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let observed_positions = observed
        .iter()
        .map(|name| manifest_order.iter().position(|item| item == name).unwrap())
        .collect::<Vec<_>>();
    assert!(observed_positions.windows(2).all(|pair| pair[0] < pair[1]));
    for required in ["failed", "output-limit", "invalid-output", "accepted"] {
        assert!(observed.iter().any(|item| item == required));
    }
}

#[cfg(unix)]
#[test]
fn scheduler_reports_failed_when_no_analyzer_result_is_accepted() {
    let repository = preflight_repository();
    let fixtures = TempDir::new().unwrap();
    let log = fixtures.path().join("failed-scheduler.log");
    let failed = scheduler_analyzer(fixtures.path(), "failed-only", "failed");
    let invalid = scheduler_analyzer(fixtures.path(), "invalid-only", "invalid-output");
    let (failed_profile, failed_hash) =
        write_budget_profile(fixtures.path(), "failed-only", &failed, &[&log], 30, 4096);
    let (invalid_profile, invalid_hash) =
        write_budget_profile(fixtures.path(), "invalid-only", &invalid, &[&log], 30, 4096);
    let (manifest, manifest_hash) = write_budget_manifest(
        fixtures.path(),
        &[
            ("failed-only", &failed_profile, &failed_hash),
            ("invalid-only", &invalid_profile, &invalid_hash),
        ],
        60,
        10485760,
        100,
    );

    let output = execute(execution_request(
        repository.path(),
        &manifest,
        &manifest_hash,
    ))
    .unwrap();

    assert_eq!(output.orchestration.status, OrchestrationStatus::Failed);
    assert_eq!(output.evidence.reports.len(), 2);
    assert_eq!(output.evidence.counts.blocking_candidates, 0);
}

#[cfg(unix)]
fn drifting_analyzer(
    directory: &Path,
    name: &str,
    target: &Path,
    repository_drift: bool,
) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join(format!("{name}-drift.sh"));
    let action = if repository_drift {
        format!("printf 'drift\\n' >> '{}'\n", target.display())
    } else {
        "printf '\\n' >> \"$1\"\n".to_string()
    };
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nset -eu\n{action}printf '%s\\n' '{{\"schema_version\":1,\"kind\":\"static_analysis_input\",\"scope_fingerprint\":\"'\"$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT\"'\",\"tool\":{{\"name\":\"{name}\",\"version\":\"1.0\"}},\"status\":\"completed\",\"findings\":[]}}'\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn scheduler_releases_no_artifact_after_authorization_or_repository_drift() {
    for drift in ["manifest", "profile", "entrypoint", "repository"] {
        let repository = preflight_repository();
        let fixtures = TempDir::new().unwrap();
        let placeholder = fixtures.path().join("placeholder");
        fs::write(&placeholder, "placeholder\n").unwrap();
        let analyzer = drifting_analyzer(
            fixtures.path(),
            drift,
            &repository.path().join("candidate.txt"),
            drift == "repository",
        );
        let argument_target = match drift {
            "entrypoint" => analyzer.as_path(),
            _ => placeholder.as_path(),
        };
        let (profile, profile_hash) = write_budget_profile(
            fixtures.path(),
            drift,
            &analyzer,
            &[argument_target],
            30,
            4096,
        );
        let (manifest, manifest_hash) = write_budget_manifest(
            fixtures.path(),
            &[(drift, &profile, &profile_hash)],
            60,
            10485760,
            100,
        );
        if drift == "manifest" {
            let analyzer = drifting_analyzer(fixtures.path(), drift, &manifest, false);
            let (rewritten_profile, rewritten_hash) =
                write_budget_profile(fixtures.path(), drift, &analyzer, &[&manifest], 30, 4096);
            let (rewritten_manifest, rewritten_manifest_hash) = write_budget_manifest(
                fixtures.path(),
                &[(drift, &rewritten_profile, &rewritten_hash)],
                60,
                10485760,
                100,
            );
            assert!(execute(execution_request(
                repository.path(),
                &rewritten_manifest,
                &rewritten_manifest_hash,
            ))
            .is_err());
            continue;
        }
        if drift == "profile" {
            let analyzer = drifting_analyzer(fixtures.path(), drift, &profile, false);
            let (rewritten_profile, rewritten_hash) =
                write_budget_profile(fixtures.path(), drift, &analyzer, &[&profile], 30, 4096);
            let (rewritten_manifest, rewritten_manifest_hash) = write_budget_manifest(
                fixtures.path(),
                &[(drift, &rewritten_profile, &rewritten_hash)],
                60,
                10485760,
                100,
            );
            assert!(execute(execution_request(
                repository.path(),
                &rewritten_manifest,
                &rewritten_manifest_hash,
            ))
            .is_err());
            continue;
        }
        assert!(execute(execution_request(
            repository.path(),
            &manifest,
            &manifest_hash,
        ))
        .is_err());
    }
}

#[cfg(unix)]
fn duplicate_finding_analyzer(directory: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join("duplicate-finding-analyzer.sh");
    fs::write(
        &path,
        "#!/bin/sh\nprintf '%s\\n' '{\"schema_version\":1,\"kind\":\"static_analysis_input\",\"scope_fingerprint\":\"'\"$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT\"'\",\"tool\":{\"name\":\"duplicate-tool\",\"version\":\"1.0\"},\"status\":\"completed\",\"findings\":[{\"rule_id\":\"DUP001\",\"message\":\"same semantic finding\",\"path\":\"candidate.txt\",\"start_line\":1,\"end_line\":1,\"severity\":\"warning\",\"category\":\"correctness\",\"confidence\":\"high\",\"baseline_state\":\"new\"}]}'\n",
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn write_duplicate_tool_profile(
    directory: &Path,
    profile_name: &str,
    executable: &Path,
) -> (PathBuf, String) {
    let path = directory.join(format!("{profile_name}.json"));
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "static_analysis_profile",
            "name": profile_name,
            "tool": {"name": "duplicate-tool", "version": "1.0"},
            "executable": {
                "path": executable.to_string_lossy(),
                "sha256": sha256_file(executable)
            },
            "arguments": [],
            "output_format": "normalized-json",
            "success_exit_codes": [0],
            "limits": {
                "timeout_seconds": 30,
                "max_output_bytes": 4096,
                "max_snapshot_bytes": 10485760,
                "max_snapshot_files": 1000
            },
            "repository_configuration": "disabled",
            "network_access": "offline-required"
        }))
        .unwrap(),
    )
    .unwrap();
    let hash = sha256_file(&path);
    (path, hash)
}

#[cfg(unix)]
#[test]
fn evidence_union_namespaces_raw_duplicates_without_semantic_merging() {
    let repository = preflight_repository();
    let fixtures = TempDir::new().unwrap();
    let analyzer = duplicate_finding_analyzer(fixtures.path());
    let (first_profile, first_hash) =
        write_duplicate_tool_profile(fixtures.path(), "first duplicate profile", &analyzer);
    let (second_profile, second_hash) =
        write_duplicate_tool_profile(fixtures.path(), "second duplicate profile", &analyzer);
    let (manifest, manifest_hash) = write_budget_manifest(
        fixtures.path(),
        &[
            ("first", &first_profile, &first_hash),
            ("second", &second_profile, &second_hash),
        ],
        60,
        10485760,
        100,
    );

    let output = execute(execution_request(
        repository.path(),
        &manifest,
        &manifest_hash,
    ))
    .unwrap();

    assert_eq!(output.evidence.reports.len(), 2);
    assert_eq!(output.evidence.findings.len(), 2);
    assert_eq!(output.evidence.counts.reports, 2);
    assert_eq!(output.evidence.counts.input_findings, 2);
    assert_eq!(output.evidence.counts.deduplicated_findings, 2);
    assert_ne!(
        output.evidence.reports[0].report_id,
        output.evidence.reports[1].report_id
    );
    assert_ne!(
        output.evidence.findings[0].finding_id,
        output.evidence.findings[1].finding_id
    );
    assert_eq!(
        output.evidence.findings[0].message,
        output.evidence.findings[1].message
    );
    assert_eq!(
        output.evidence.findings[0].path,
        output.evidence.findings[1].path
    );
    assert_eq!(
        output.evidence.findings[0].start_line,
        output.evidence.findings[1].start_line
    );
    assert_eq!(
        output.evidence.findings[0].severity,
        output.evidence.findings[1].severity
    );
    for (report, finding) in output
        .evidence
        .reports
        .iter()
        .zip(&output.evidence.findings)
    {
        assert_eq!(finding.report_ids, vec![report.report_id.clone()]);
    }
    let execution_ids = output
        .orchestration
        .runs
        .iter()
        .map(|run| match run {
            OrchestrationRun::Executed { execution, .. } => execution.execution_id.clone(),
            other => panic!("expected executed run: {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        output
            .evidence
            .reports
            .iter()
            .map(|report| report.execution_id.clone().unwrap())
            .collect::<Vec<_>>(),
        execution_ids
    );
}

#[cfg(unix)]
#[test]
fn evidence_union_truncates_only_after_ordered_independent_union() {
    let repository = preflight_repository();
    let fixtures = TempDir::new().unwrap();
    let analyzer = duplicate_finding_analyzer(fixtures.path());
    let (first_profile, first_hash) =
        write_duplicate_tool_profile(fixtures.path(), "first truncated profile", &analyzer);
    let (second_profile, second_hash) =
        write_duplicate_tool_profile(fixtures.path(), "second truncated profile", &analyzer);
    let (manifest, manifest_hash) = write_budget_manifest(
        fixtures.path(),
        &[
            ("first", &first_profile, &first_hash),
            ("second", &second_profile, &second_hash),
        ],
        60,
        10485760,
        1,
    );

    let output = execute(execution_request(
        repository.path(),
        &manifest,
        &manifest_hash,
    ))
    .unwrap();

    assert_eq!(output.evidence.counts.deduplicated_findings, 2);
    assert_eq!(output.evidence.findings.len(), 1);
    assert!(output.evidence.truncated);
    assert_eq!(output.orchestration.budgets.findings.initial, 1);
    assert_eq!(output.orchestration.budgets.findings.consumed, 1);
    assert_eq!(output.orchestration.budgets.findings.remaining, 0);
    let first_execution_id = match &output.orchestration.runs[0] {
        OrchestrationRun::Executed { execution, .. } => execution.execution_id.as_str(),
        other => panic!("expected executed run: {other:?}"),
    };
    assert_eq!(
        output.evidence.reports[0].execution_id.as_deref(),
        Some(first_execution_id)
    );
    assert_eq!(
        output.evidence.findings[0].report_ids,
        vec![output.evidence.reports[0].report_id.clone()]
    );
}
