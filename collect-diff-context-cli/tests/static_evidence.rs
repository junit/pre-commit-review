use collect_diff_context_cli::review_scope::{
    open_authoritative_scope, ReviewSource, ScopeRequest,
};
use collect_diff_context_cli::static_analysis::contracts::{EvidenceReport, StaticAnalysisInput};
use collect_diff_context_cli::static_analysis::contracts::{
    EvidenceScopeBinding, EvidenceTrust, OutputFormat,
};
use collect_diff_context_cli::static_analysis::evidence::{collect_evidence, CollectRequest};
use serde_json::json;
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git must start");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn staged_repository() -> (TempDir, String) {
    let repo = TempDir::new().unwrap();
    git(repo.path(), &["init", "-q"]);
    git(
        repo.path(),
        &["config", "user.email", "review@example.test"],
    );
    git(repo.path(), &["config", "user.name", "Review Test"]);
    fs::create_dir_all(repo.path().join("src")).unwrap();
    fs::write(
        repo.path().join("src/app.rs"),
        "pub fn value() -> u8 { 1 }\n",
    )
    .unwrap();
    git(repo.path(), &["add", "src/app.rs"]);
    git(repo.path(), &["commit", "-qm", "base"]);
    fs::write(
        repo.path().join("src/app.rs"),
        "pub fn value() -> u8 { 2 }\n",
    )
    .unwrap();
    git(repo.path(), &["add", "src/app.rs"]);
    let scope = open_authoritative_scope(ScopeRequest {
        repository: repo.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_fingerprint: None,
    })
    .unwrap();
    (repo, scope.fingerprint)
}

fn static_analysis_binary() -> &'static str {
    env!("CARGO_BIN_EXE_static-analysis-cli")
}

fn valid_input() -> serde_json::Value {
    json!({
        "schema_version": 1,
        "kind": "static_analysis_input",
        "scope_fingerprint": "0123456789abcdef0123456789abcdef01234567",
        "tool": {"name": "fixture", "version": "1.0"},
        "status": "completed",
        "findings": [{
            "rule_id": "R1",
            "message": "unsafe value",
            "path": "src/app.rs",
            "start_line": 7,
            "end_line": 8,
            "severity": "error",
            "category": "security",
            "confidence": "high",
            "baseline_state": "new"
        }]
    })
}

#[test]
fn contracts_accept_valid_normalized_input() {
    let input: StaticAnalysisInput = serde_json::from_value(valid_input()).unwrap();
    input.validate().unwrap();
}

#[test]
fn contracts_reject_unknown_input_fields() {
    let mut input = valid_input();
    input["unexpected"] = json!(true);
    assert!(serde_json::from_value::<StaticAnalysisInput>(input).is_err());
}

#[test]
fn contracts_reject_invalid_normalized_semantics() {
    let mut input = valid_input();
    input["kind"] = json!("wrong");
    let input: StaticAnalysisInput = serde_json::from_value(input).unwrap();
    assert!(input.validate().is_err());

    let mut input = valid_input();
    input["findings"][0]["end_line"] = json!(6);
    let input: StaticAnalysisInput = serde_json::from_value(input).unwrap();
    assert!(input.validate().is_err());
}

#[test]
fn contracts_leave_input_tool_text_for_normalization() {
    let mut input = valid_input();
    input["tool"]["name"] = json!("x".repeat(300));
    input["tool"]["version"] = json!("");
    let input: StaticAnalysisInput = serde_json::from_value(input).unwrap();
    input.validate().unwrap();
}

#[test]
fn contracts_require_execution_id_for_controlled_trust() {
    let report: EvidenceReport = serde_json::from_value(json!({
        "report_id": "0123456789abcdef",
        "format": "normalized-json",
        "tool": {"name": "fixture", "version": "1.0"},
        "status": "completed",
        "trust": "controlled-execution",
        "scope_binding": "controlled-execution",
        "execution_id": null,
        "finding_count": 0
    }))
    .unwrap();
    assert!(report.validate().is_err());
}

#[test]
fn parsing_normalized_json_deduplicates_findings() {
    let (repo, fingerprint) = staged_repository();
    let result = repo.path().join("normalized.json");
    let finding = json!({
        "rule_id": "R1",
        "message": "unsafe value",
        "path": "src/app.rs",
        "start_line": 1,
        "end_line": 1,
        "severity": "error",
        "category": "security",
        "confidence": "high",
        "baseline_state": "new"
    });
    fs::write(
        &result,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "static_analysis_input",
            "scope_fingerprint": fingerprint,
            "tool": {"name": "fixture", "version": "1.0"},
            "status": "completed",
            "findings": [finding.clone(), finding]
        }))
        .unwrap(),
    )
    .unwrap();

    let evidence = collect_evidence(CollectRequest {
        repository: repo.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_scope: fingerprint,
        result_paths: vec![result],
        asserted_result_scope: None,
        max_findings: 500,
        trust: EvidenceTrust::ExplicitInput,
        execution_id: None,
    })
    .unwrap();

    assert_eq!(evidence.reports[0].format, OutputFormat::NormalizedJson);
    assert_eq!(evidence.counts.input_findings, 2);
    assert_eq!(evidence.counts.deduplicated_findings, 1);
    assert_eq!(evidence.findings[0].path, "src/app.rs");
}

#[test]
fn parsing_sarif_records_embedded_scope() {
    let (repo, fingerprint) = staged_repository();
    let result = repo.path().join("result.sarif");
    fs::write(
        &result,
        serde_json::to_vec(&json!({
            "version": "2.1.0",
            "runs": [{
                "properties": {"preCommitReviewScopeFingerprint": fingerprint},
                "tool": {"driver": {"name": "fixture-sarif", "version": "2.0"}},
                "results": [{
                    "ruleId": "security/test",
                    "level": "error",
                    "message": {"text": "unsafe value"},
                    "locations": [{"physicalLocation": {
                        "artifactLocation": {"uri": "src/app.rs"},
                        "region": {"startLine": 1, "endLine": 1}
                    }}]
                }]
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let evidence = collect_evidence(CollectRequest {
        repository: repo.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_scope: fingerprint,
        result_paths: vec![result],
        asserted_result_scope: None,
        max_findings: 500,
        trust: EvidenceTrust::ExplicitInput,
        execution_id: None,
    })
    .unwrap();

    assert_eq!(evidence.reports[0].format, OutputFormat::Sarif);
    assert_eq!(
        evidence.reports[0].scope_binding,
        EvidenceScopeBinding::Embedded
    );
    assert_eq!(evidence.findings.len(), 1);
}

#[test]
fn parsing_rejects_malformed_json() {
    let (repo, fingerprint) = staged_repository();
    let result = repo.path().join("broken.json");
    fs::write(&result, b"{").unwrap();
    let error = collect_evidence(CollectRequest {
        repository: repo.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_scope: fingerprint,
        result_paths: vec![result],
        asserted_result_scope: None,
        max_findings: 500,
        trust: EvidenceTrust::ExplicitInput,
        execution_id: None,
    })
    .unwrap_err();
    assert!(error.to_string().contains("valid UTF-8 JSON"));
}

#[test]
fn parsing_normalized_json_requires_embedded_scope() {
    let (repo, fingerprint) = staged_repository();
    let result = repo.path().join("unbound.json");
    fs::write(
        &result,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "static_analysis_input",
            "tool": {"name": "fixture", "version": "1.0"},
            "status": "completed",
            "findings": []
        }))
        .unwrap(),
    )
    .unwrap();

    let error = collect_evidence(CollectRequest {
        repository: repo.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_scope: fingerprint.clone(),
        result_paths: vec![result],
        asserted_result_scope: Some(fingerprint),
        max_findings: 500,
        trust: EvidenceTrust::ExplicitInput,
        execution_id: None,
    })
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("normalized input must embed scope_fingerprint"));
}

#[test]
fn parsing_normalized_json_rejects_zero_line_numbers() {
    let (repo, fingerprint) = staged_repository();
    let result = repo.path().join("zero-line.json");
    fs::write(
        &result,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "static_analysis_input",
            "scope_fingerprint": fingerprint,
            "tool": {"name": "fixture", "version": "1.0"},
            "status": "completed",
            "findings": [{
                "rule_id": "R1",
                "message": "invalid location",
                "path": "src/app.rs",
                "start_line": 0,
                "end_line": 0,
                "severity": "warning",
                "category": "correctness",
                "confidence": "medium"
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let error = collect_evidence(CollectRequest {
        repository: repo.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_scope: fingerprint,
        result_paths: vec![result],
        asserted_result_scope: None,
        max_findings: 500,
        trust: EvidenceTrust::ExplicitInput,
        execution_id: None,
    })
    .unwrap_err();

    assert!(error.to_string().contains("positive integer"));
}

#[test]
fn parsing_rejects_invalid_request_fingerprints() {
    let (repo, fingerprint) = staged_repository();
    let result = repo.path().join("valid.json");
    fs::write(
        &result,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "static_analysis_input",
            "scope_fingerprint": fingerprint,
            "tool": {"name": "fixture", "version": "1.0"},
            "status": "completed",
            "findings": []
        }))
        .unwrap(),
    )
    .unwrap();

    let invalid_expected = collect_evidence(CollectRequest {
        repository: repo.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_scope: "invalid".to_string(),
        result_paths: vec![result.clone()],
        asserted_result_scope: None,
        max_findings: 500,
        trust: EvidenceTrust::ExplicitInput,
        execution_id: None,
    })
    .unwrap_err();
    assert_eq!(
        invalid_expected.to_string(),
        "--expect-scope is missing or invalid"
    );

    let invalid_assertion = collect_evidence(CollectRequest {
        repository: repo.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_scope: fingerprint,
        result_paths: vec![result],
        asserted_result_scope: Some("invalid".to_string()),
        max_findings: 500,
        trust: EvidenceTrust::ExplicitInput,
        execution_id: None,
    })
    .unwrap_err();
    assert_eq!(
        invalid_assertion.to_string(),
        "--result-scope is missing or invalid"
    );
}

#[test]
fn parsing_sarif_supports_explicit_scope_and_multiple_runs() {
    let (repo, fingerprint) = staged_repository();
    let result = repo.path().join("multi-run.sarif");
    let absolute_uri = format!("file://{}", repo.path().join("src/app.rs").display());
    fs::write(
        &result,
        serde_json::to_vec(&json!({
            "version": "2.1.0",
            "runs": [
                {
                    "tool": {"driver": {
                        "name": "security-tool",
                        "semanticVersion": "3.0.0",
                        "rules": [{
                            "id": "dynamic-eval",
                            "properties": {
                                "tags": ["security", "external/cwe/cwe-95"],
                                "precision": "high"
                            }
                        }]
                    }},
                    "results": [{
                        "ruleId": "dynamic-eval",
                        "level": "high",
                        "message": {"text": "Dynamic evaluation is unsafe."},
                        "locations": [{"physicalLocation": {
                            "artifactLocation": {"uri": absolute_uri},
                            "region": {"startLine": 1}
                        }}]
                    }]
                },
                {
                    "tool": {"driver": {"name": "compiler", "version": "1.0"}},
                    "invocations": [{"executionSuccessful": false}],
                    "results": [{
                        "ruleId": "type-error",
                        "level": "warning",
                        "properties": {"precision": "moderate"},
                        "message": "Type mismatch.",
                        "locations": [{"physicalLocation": {
                            "artifactLocation": {"uri": "./src/app.rs"},
                            "region": {"startLine": 1, "endLine": 1}
                        }}]
                    }]
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let evidence = collect_evidence(CollectRequest {
        repository: repo.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_scope: fingerprint.clone(),
        result_paths: vec![result],
        asserted_result_scope: Some(fingerprint),
        max_findings: 500,
        trust: EvidenceTrust::ExplicitInput,
        execution_id: None,
    })
    .unwrap();

    assert_eq!(evidence.reports.len(), 2);
    assert!(evidence
        .reports
        .iter()
        .all(|report| report.scope_binding == EvidenceScopeBinding::ExplicitAssertion));
    let security = evidence
        .findings
        .iter()
        .find(|finding| finding.rule_id == "dynamic-eval")
        .unwrap();
    assert_eq!(security.path, "src/app.rs");
    assert_eq!(
        security.severity,
        collect_diff_context_cli::static_analysis::contracts::Severity::Error
    );
    assert_eq!(
        security.confidence,
        collect_diff_context_cli::static_analysis::contracts::Confidence::High
    );
    assert_eq!(
        security.category,
        collect_diff_context_cli::static_analysis::contracts::FindingCategory::Security
    );
}

#[test]
fn parsing_collect_help_succeeds() {
    let output = Command::new(static_analysis_binary())
        .args(["collect", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--expect-scope"));
}

#[test]
fn parsing_collect_missing_result_is_actionable() {
    let output = Command::new(static_analysis_binary())
        .arg("collect")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr)
        .starts_with("collect_static_evidence: at least one --result is required"));
}

#[test]
fn parsing_collect_cli_renders_stable_marker() {
    let (repo, fingerprint) = staged_repository();
    let result = repo.path().join("normalized.json");
    fs::write(
        &result,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "static_analysis_input",
            "scope_fingerprint": fingerprint,
            "tool": {"name": "fixture", "version": "1.0"},
            "status": "completed",
            "findings": []
        }))
        .unwrap(),
    )
    .unwrap();

    let output = Command::new(static_analysis_binary())
        .current_dir(repo.path())
        .args([
            "collect",
            "--source",
            "staged",
            "--expect-scope",
            &fingerprint,
            "--result",
            result.to_str().unwrap(),
            "--helper",
            "/ignored/legacy/helper",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "collect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("# Pre-Commit Review Static Analysis Evidence\n\n"));
    assert!(stdout.contains("\n## Static Analysis Evidence JSON\n"));
}
