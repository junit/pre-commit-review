#[cfg(unix)]
use collect_diff_context_cli::review_scope::ReviewSource;
use collect_diff_context_cli::static_analysis::contracts::{
    OrchestrationArtifact, OrchestrationManifest, StaticAnalysisEvidence,
};
#[cfg(unix)]
use collect_diff_context_cli::static_analysis::orchestration::{
    prepare_orchestration, OrchestrationRequest,
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
    json!({
        "schema_version": 1,
        "kind": "static_analysis_orchestration_manifest",
        "name": "trusted pre-commit analyzer set",
        "profiles": [
            {
                "profile_id": "security",
                "path": "/opt/review/profiles/security.json",
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
