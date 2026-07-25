use collect_diff_context_cli::static_analysis::contracts::{EvidenceReport, StaticAnalysisInput};
use serde_json::json;

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
