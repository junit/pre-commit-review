use collect_diff_context_cli::static_analysis::contracts::{
    OrchestrationArtifact, OrchestrationManifest, StaticAnalysisEvidence,
};
use serde_json::{json, Value};

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
