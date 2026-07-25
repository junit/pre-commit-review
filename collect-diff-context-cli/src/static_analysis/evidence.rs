use super::contracts::{
    BaselineState, Confidence, DecisionContract, EvidenceCounts, EvidenceFinding, EvidenceReport,
    EvidenceScope, EvidenceScopeBinding, EvidenceTrust, FindingCategory, FindingDisposition,
    LineScope, OutputFormat, ReportStatus, Severity, StaticAnalysisEvidence, StaticAnalysisInput,
    ToolIdentity,
};
use crate::review_scope::{
    open_authoritative_scope, revalidate_scope, AuthoritativeScope, ReviewSource, ScopeRequest,
};
use percent_encoding::percent_decode_str;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_INPUT_BYTES: u64 = 10_000_000;
const MAX_INPUT_FINDINGS: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceError {
    message: String,
}

impl EvidenceError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for EvidenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for EvidenceError {}

#[derive(Debug, Clone)]
pub struct CollectRequest {
    pub repository: PathBuf,
    pub source: Option<ReviewSource>,
    pub expected_scope: String,
    pub result_paths: Vec<PathBuf>,
    pub asserted_result_scope: Option<String>,
    pub max_findings: usize,
    pub trust: EvidenceTrust,
    pub execution_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedFinding {
    tool: ToolIdentity,
    rule_id: String,
    message: String,
    path: String,
    start_line: Option<u32>,
    end_line: Option<u32>,
    severity: Severity,
    category: FindingCategory,
    confidence: Confidence,
    baseline_state: BaselineState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedReport {
    report_id: String,
    format: OutputFormat,
    tool: ToolIdentity,
    status: ReportStatus,
    scope_binding: EvidenceScopeBinding,
    finding_count: usize,
    findings: Vec<ParsedFinding>,
}

#[derive(Debug, Clone)]
struct MergedFinding {
    finding: ParsedFinding,
    report_ids: Vec<String>,
    completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FindingKey {
    tool_name: String,
    rule_id: String,
    message: String,
    path: String,
    start_line: Option<u32>,
    end_line: Option<u32>,
}

pub fn collect_evidence(request: CollectRequest) -> Result<StaticAnalysisEvidence, EvidenceError> {
    validate_request(&request)?;
    let scope = open_authoritative_scope(ScopeRequest {
        repository: request.repository.clone(),
        source: request.source,
        expected_fingerprint: Some(request.expected_scope.clone()),
    })
    .map_err(|error| EvidenceError::new(error.to_string()))?;

    let mut reports = Vec::new();
    for result_path in &request.result_paths {
        for report in parse_report_file(
            result_path,
            request.asserted_result_scope.as_deref(),
            &request.expected_scope,
            &scope.repository,
        )? {
            if let Some(existing) = reports
                .iter()
                .find(|existing: &&ParsedReport| existing.report_id == report.report_id)
            {
                if *existing != report {
                    return Err(EvidenceError::new(format!(
                        "report identifier collision: {}",
                        report.report_id
                    )));
                }
            } else {
                reports.push(report);
            }
        }
    }

    let input_findings = reports.iter().map(|report| report.finding_count).sum();
    if input_findings > MAX_INPUT_FINDINGS {
        return Err(EvidenceError::new(format!(
            "static results exceed the {MAX_INPUT_FINDINGS}-finding processing limit"
        )));
    }
    let merged = merge_findings(&reports);
    let mut evidence =
        build_preliminary_evidence(&request, &scope, reports, merged, input_findings)?;
    revalidate_scope(&scope).map_err(|error| EvidenceError::new(error.to_string()))?;
    evidence.scope = evidence_scope(&scope);
    Ok(evidence)
}

fn validate_request(request: &CollectRequest) -> Result<(), EvidenceError> {
    if request.result_paths.is_empty() {
        return Err(EvidenceError::new("at least one --result is required"));
    }
    if !is_scope_fingerprint(&request.expected_scope) {
        return Err(EvidenceError::new("--expect-scope is missing or invalid"));
    }
    if request
        .asserted_result_scope
        .as_deref()
        .is_some_and(|fingerprint| !is_scope_fingerprint(fingerprint))
    {
        return Err(EvidenceError::new("--result-scope is missing or invalid"));
    }
    if !(1..=5_000).contains(&request.max_findings) {
        return Err(EvidenceError::new(
            "--max-findings must be between 1 and 5000",
        ));
    }
    match request.trust {
        EvidenceTrust::ControlledExecution => {
            let execution_id = request.execution_id.as_deref().ok_or_else(|| {
                EvidenceError::new("controlled-execution trust requires --execution-id")
            })?;
            if execution_id.len() != 16
                || !execution_id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(EvidenceError::new(
                    "--execution-id must be 16 lowercase hexadecimal characters",
                ));
            }
        }
        EvidenceTrust::ExplicitInput if request.execution_id.is_some() => {
            return Err(EvidenceError::new(
                "--execution-id is valid only with controlled-execution trust",
            ));
        }
        EvidenceTrust::ExplicitInput => {}
    }
    Ok(())
}

fn is_scope_fingerprint(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_report_file(
    path: &Path,
    asserted_scope: Option<&str>,
    expected_scope: &str,
    repository: &Path,
) -> Result<Vec<ParsedReport>, EvidenceError> {
    let metadata = fs::metadata(path).map_err(|error| {
        EvidenceError::new(format!(
            "cannot read static result {}: {error}",
            display_name(path)
        ))
    })?;
    if !metadata.is_file() {
        return Err(EvidenceError::new(format!(
            "static result {} must be a regular file",
            display_name(path)
        )));
    }
    if metadata.len() > MAX_INPUT_BYTES {
        return Err(EvidenceError::new(format!(
            "static result {} exceeds the {MAX_INPUT_BYTES}-byte input limit",
            display_name(path)
        )));
    }
    let raw = fs::read(path).map_err(|error| {
        EvidenceError::new(format!(
            "cannot read static result {}: {error}",
            display_name(path)
        ))
    })?;
    let text = std::str::from_utf8(&raw).map_err(|error| {
        EvidenceError::new(format!(
            "static result {} is not valid UTF-8 JSON: {error}",
            display_name(path)
        ))
    })?;
    let payload: Value = serde_json::from_str(text).map_err(|error| {
        EvidenceError::new(format!(
            "static result {} is not valid UTF-8 JSON: {error}",
            display_name(path)
        ))
    })?;
    if payload.get("version").and_then(Value::as_str) == Some("2.1.0")
        && payload.get("runs").is_some_and(Value::is_array)
    {
        parse_sarif(
            &payload,
            &raw,
            path,
            asserted_scope,
            expected_scope,
            repository,
        )
    } else {
        parse_normalized(&payload, &raw, path, expected_scope, repository)
    }
}

fn parse_normalized(
    payload: &Value,
    raw: &[u8],
    path: &Path,
    expected_scope: &str,
    repository: &Path,
) -> Result<Vec<ParsedReport>, EvidenceError> {
    if payload.get("schema_version").and_then(Value::as_u64) == Some(1)
        && payload.get("kind").and_then(Value::as_str) == Some("static_analysis_input")
        && payload
            .get("scope_fingerprint")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(EvidenceError::new(format!(
            "{} normalized input must embed scope_fingerprint",
            display_name(path)
        )));
    }
    let input: StaticAnalysisInput = serde_json::from_value(payload.clone()).map_err(|_| {
        EvidenceError::new(format!(
            "{} is neither SARIF 2.1.0 nor static_analysis_input/v1",
            display_name(path)
        ))
    })?;
    input
        .validate()
        .map_err(|error| EvidenceError::new(error.to_string()))?;
    if input.scope_fingerprint != expected_scope {
        return Err(EvidenceError::new(format!(
            "{} scope fingerprint does not match the review scope",
            display_name(path)
        )));
    }
    let tool = ToolIdentity {
        name: clean_text(Some(&input.tool.name), "unknown-tool", 200),
        version: input
            .tool
            .version
            .as_deref()
            .map(|version| clean_text(Some(version), "", 100))
            .filter(|version| !version.is_empty()),
    };
    let findings = input
        .findings
        .into_iter()
        .map(|finding| ParsedFinding {
            tool: tool.clone(),
            rule_id: clean_text(Some(&finding.rule_id), "unknown-rule", 200),
            message: clean_text(Some(&finding.message), "Static analyzer finding.", 1_000),
            path: normalize_path(&finding.path, repository),
            start_line: finding.start_line,
            end_line: finding.end_line.or(finding.start_line),
            severity: finding.severity,
            category: finding.category,
            confidence: finding.confidence,
            baseline_state: finding.baseline_state,
        })
        .collect::<Vec<_>>();
    Ok(vec![ParsedReport {
        report_id: compact_report_id(raw, 0, &tool.name),
        format: OutputFormat::NormalizedJson,
        tool,
        status: input.status,
        scope_binding: EvidenceScopeBinding::Embedded,
        finding_count: findings.len(),
        findings,
    }])
}

fn parse_sarif(
    payload: &Value,
    raw: &[u8],
    path: &Path,
    asserted_scope: Option<&str>,
    expected_scope: &str,
    repository: &Path,
) -> Result<Vec<ParsedReport>, EvidenceError> {
    let runs = payload
        .get("runs")
        .and_then(Value::as_array)
        .ok_or_else(|| EvidenceError::new("SARIF runs must be an array"))?;
    let mut reports = Vec::new();
    for (run_index, run_value) in runs.iter().enumerate() {
        let run = run_value.as_object().ok_or_else(|| {
            EvidenceError::new(format!(
                "{} SARIF run {run_index} must be an object",
                display_name(path)
            ))
        })?;
        let scope_binding = resolve_sarif_scope(
            payload,
            run,
            asserted_scope,
            expected_scope,
            &format!("{} SARIF run {run_index}", display_name(path)),
        )?;
        let driver = run
            .get("tool")
            .and_then(Value::as_object)
            .and_then(|tool| tool.get("driver"))
            .and_then(Value::as_object);
        let tool = ToolIdentity {
            name: clean_text(
                driver
                    .and_then(|value| value.get("name"))
                    .and_then(Value::as_str),
                "unknown-sarif-tool",
                200,
            ),
            version: driver
                .and_then(|value| {
                    value
                        .get("semanticVersion")
                        .or_else(|| value.get("version"))
                })
                .and_then(Value::as_str)
                .map(|version| clean_text(Some(version), "", 100))
                .filter(|version| !version.is_empty()),
        };
        let status = if run
            .get("invocations")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("executionSuccessful").and_then(Value::as_bool) == Some(false)
                })
            }) {
            ReportStatus::Failed
        } else {
            ReportStatus::Completed
        };
        let rules = sarif_rules(driver);
        let mut findings = Vec::new();
        if let Some(results) = run.get("results").and_then(Value::as_array) {
            for (result_index, result_value) in results.iter().enumerate() {
                let Some(result) = result_value.as_object() else {
                    continue;
                };
                if result.get("baselineState").and_then(Value::as_str) == Some("absent") {
                    continue;
                }
                let rule_id = clean_text(
                    result.get("ruleId").and_then(Value::as_str),
                    &format!("result-{result_index}"),
                    200,
                );
                let rule = find_rule(&rules, result, &rule_id);
                let rule_properties = rule
                    .and_then(|value| value.get("properties"))
                    .and_then(Value::as_object);
                let result_properties = result.get("properties").and_then(Value::as_object);
                let tags = result_properties
                    .and_then(|properties| properties.get("tags"))
                    .or_else(|| rule_properties.and_then(|properties| properties.get("tags")))
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .map(|value| {
                                value
                                    .as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| value.to_string())
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let message = result.get("message").and_then(|value| {
                    value.as_str().or_else(|| {
                        value.as_object().and_then(|object| {
                            object
                                .get("text")
                                .or_else(|| object.get("markdown"))
                                .and_then(Value::as_str)
                        })
                    })
                });
                let message = clean_text(message, "Static analyzer finding.", 1_000);
                let default_configuration = rule
                    .and_then(|value| value.get("defaultConfiguration"))
                    .and_then(Value::as_object);
                let severity = normalize_severity(
                    result_properties
                        .and_then(|properties| properties.get("severity"))
                        .and_then(Value::as_str)
                        .or_else(|| result.get("level").and_then(Value::as_str))
                        .or_else(|| {
                            default_configuration
                                .and_then(|configuration| configuration.get("level"))
                                .and_then(Value::as_str)
                        }),
                );
                let confidence = normalize_confidence(
                    result_properties
                        .and_then(|properties| properties.get("precision"))
                        .and_then(Value::as_str)
                        .or_else(|| {
                            rule_properties
                                .and_then(|properties| properties.get("precision"))
                                .and_then(Value::as_str)
                        }),
                );
                let category = infer_category(&rule_id, &message, &tool.name, &tags);
                let baseline_state = match result.get("baselineState").and_then(Value::as_str) {
                    Some("new" | "updated") => BaselineState::New,
                    Some("unchanged") => BaselineState::Existing,
                    _ => BaselineState::Unknown,
                };
                let locations = result
                    .get("locations")
                    .and_then(Value::as_array)
                    .filter(|locations| !locations.is_empty());
                if let Some(locations) = locations {
                    for location in locations {
                        findings.push(sarif_finding(
                            location.as_object(),
                            repository,
                            &tool,
                            &rule_id,
                            &message,
                            severity,
                            category,
                            confidence,
                            baseline_state,
                        ));
                    }
                } else {
                    findings.push(sarif_finding(
                        None,
                        repository,
                        &tool,
                        &rule_id,
                        &message,
                        severity,
                        category,
                        confidence,
                        baseline_state,
                    ));
                }
            }
        }
        reports.push(ParsedReport {
            report_id: compact_report_id(raw, run_index, &tool.name),
            format: OutputFormat::Sarif,
            tool,
            status,
            scope_binding,
            finding_count: findings.len(),
            findings,
        });
    }
    if reports.is_empty() {
        return Err(EvidenceError::new(format!(
            "{} SARIF input contains no runs",
            display_name(path)
        )));
    }
    Ok(reports)
}

fn sarif_rules(driver: Option<&Map<String, Value>>) -> Vec<&Map<String, Value>> {
    driver
        .and_then(|value| value.get("rules"))
        .and_then(Value::as_array)
        .map(|rules| rules.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

fn find_rule<'a>(
    rules: &'a [&Map<String, Value>],
    result: &Map<String, Value>,
    rule_id: &str,
) -> Option<&'a Map<String, Value>> {
    rules
        .iter()
        .copied()
        .find(|rule| rule.get("id").and_then(Value::as_str) == Some(rule_id))
        .or_else(|| {
            result
                .get("ruleIndex")
                .and_then(Value::as_u64)
                .and_then(|index| rules.get(index as usize).copied())
        })
}

#[allow(clippy::too_many_arguments)]
fn sarif_finding(
    location: Option<&Map<String, Value>>,
    repository: &Path,
    tool: &ToolIdentity,
    rule_id: &str,
    message: &str,
    severity: Severity,
    category: FindingCategory,
    confidence: Confidence,
    baseline_state: BaselineState,
) -> ParsedFinding {
    let physical = location
        .and_then(|value| value.get("physicalLocation"))
        .and_then(Value::as_object);
    let path = physical
        .and_then(|value| value.get("artifactLocation"))
        .and_then(Value::as_object)
        .and_then(|value| value.get("uri").or_else(|| value.get("uriBaseId")))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let region = physical
        .and_then(|value| value.get("region"))
        .and_then(Value::as_object);
    let start_line = region
        .and_then(|value| value.get("startLine"))
        .and_then(Value::as_u64)
        .filter(|line| *line > 0)
        .and_then(|line| u32::try_from(line).ok());
    let mut end_line = region
        .and_then(|value| value.get("endLine"))
        .and_then(Value::as_u64)
        .filter(|line| *line > 0)
        .and_then(|line| u32::try_from(line).ok())
        .or(start_line);
    if start_line.is_some() && end_line < start_line {
        end_line = start_line;
    }
    ParsedFinding {
        tool: tool.clone(),
        rule_id: rule_id.to_string(),
        message: message.to_string(),
        path: normalize_path(path, repository),
        start_line,
        end_line,
        severity,
        category,
        confidence,
        baseline_state,
    }
}

fn resolve_sarif_scope(
    payload: &Value,
    run: &Map<String, Value>,
    asserted_scope: Option<&str>,
    expected_scope: &str,
    label: &str,
) -> Result<EvidenceScopeBinding, EvidenceError> {
    let automation_properties = run
        .get("automationDetails")
        .and_then(Value::as_object)
        .and_then(|value| value.get("properties"));
    let property_sources = [
        payload.get("properties"),
        run.get("properties"),
        automation_properties,
    ];
    let keys = [
        "preCommitReviewScopeFingerprint",
        "pre-commit-review/scopeFingerprint",
        "scope_fingerprint",
    ];
    let embedded = property_sources
        .iter()
        .filter_map(|value| value.and_then(Value::as_object))
        .find_map(|properties| {
            keys.iter()
                .find_map(|key| properties.get(*key).and_then(Value::as_str))
        });
    if let Some(observed) = embedded {
        if observed != expected_scope {
            return Err(EvidenceError::new(format!(
                "{label} scope fingerprint does not match the review scope"
            )));
        }
        return Ok(EvidenceScopeBinding::Embedded);
    }
    if let Some(observed) = asserted_scope {
        if observed != expected_scope {
            return Err(EvidenceError::new(
                "--result-scope fingerprint does not match the review scope",
            ));
        }
        return Ok(EvidenceScopeBinding::ExplicitAssertion);
    }
    Err(EvidenceError::new(format!(
        "{label} has no embedded scope fingerprint; pass --result-scope only when you can assert its snapshot"
    )))
}

fn merge_findings(reports: &[ParsedReport]) -> Vec<MergedFinding> {
    let mut merged = HashMap::<FindingKey, MergedFinding>::new();
    for report in reports {
        for finding in &report.findings {
            let key = FindingKey {
                tool_name: finding.tool.name.clone(),
                rule_id: finding.rule_id.clone(),
                message: finding.message.clone(),
                path: finding.path.clone(),
                start_line: finding.start_line,
                end_line: finding.end_line,
            };
            if let Some(existing) = merged.get_mut(&key) {
                if !existing.report_ids.contains(&report.report_id) {
                    existing.report_ids.push(report.report_id.clone());
                }
                if severity_order(finding.severity) > severity_order(existing.finding.severity) {
                    existing.finding.severity = finding.severity;
                }
                if confidence_order(finding.confidence)
                    > confidence_order(existing.finding.confidence)
                {
                    existing.finding.confidence = finding.confidence;
                }
                if existing.finding.category == FindingCategory::Unknown
                    && finding.category != FindingCategory::Unknown
                {
                    existing.finding.category = finding.category;
                }
                if finding.baseline_state == BaselineState::New {
                    existing.finding.baseline_state = BaselineState::New;
                } else if existing.finding.baseline_state == BaselineState::Unknown
                    && finding.baseline_state == BaselineState::Existing
                {
                    existing.finding.baseline_state = BaselineState::Existing;
                }
                existing.completed |= report.status == ReportStatus::Completed;
            } else {
                merged.insert(
                    key,
                    MergedFinding {
                        finding: finding.clone(),
                        report_ids: vec![report.report_id.clone()],
                        completed: report.status == ReportStatus::Completed,
                    },
                );
            }
        }
    }
    let mut findings = merged.into_values().collect::<Vec<_>>();
    findings.sort_by(|left, right| {
        left.finding
            .path
            .cmp(&right.finding.path)
            .then_with(|| {
                left.finding
                    .start_line
                    .unwrap_or(0)
                    .cmp(&right.finding.start_line.unwrap_or(0))
            })
            .then_with(|| left.finding.tool.name.cmp(&right.finding.tool.name))
            .then_with(|| left.finding.rule_id.cmp(&right.finding.rule_id))
            .then_with(|| left.finding.message.cmp(&right.finding.message))
    });
    findings
}

fn build_preliminary_evidence(
    request: &CollectRequest,
    scope: &AuthoritativeScope,
    reports: Vec<ParsedReport>,
    merged: Vec<MergedFinding>,
    input_findings: usize,
) -> Result<StaticAnalysisEvidence, EvidenceError> {
    let unit_ids = scope
        .units
        .iter()
        .map(|unit| {
            (
                normalize_path(&unit.path, &scope.repository),
                unit.unit_id.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut findings = merged
        .into_iter()
        .map(|mut merged| {
            merged.report_ids.sort();
            let manifest_unit_id = unit_ids.get(&merged.finding.path).cloned();
            let (line_scope, disposition) = if manifest_unit_id.is_some() {
                (LineScope::Unknown, FindingDisposition::Note)
            } else {
                (LineScope::OutsideScope, FindingDisposition::OutsideScope)
            };
            EvidenceFinding {
                finding_id: compact_finding_id(&merged.finding),
                report_ids: merged.report_ids,
                tool: merged.finding.tool,
                rule_id: merged.finding.rule_id,
                message: merged.finding.message,
                path: merged.finding.path,
                start_line: merged.finding.start_line,
                end_line: merged.finding.end_line,
                severity: merged.finding.severity,
                category: merged.finding.category,
                confidence: merged.finding.confidence,
                baseline_state: merged.finding.baseline_state,
                manifest_unit_id,
                line_scope,
                disposition,
                blocking_candidate: false,
            }
        })
        .collect::<Vec<_>>();
    let counts = evidence_counts(&reports, input_findings, &findings);
    let truncated = findings.len() > request.max_findings;
    findings.truncate(request.max_findings);
    let mut report_values = reports
        .into_iter()
        .map(|report| EvidenceReport {
            report_id: report.report_id,
            format: report.format,
            tool: report.tool,
            status: report.status,
            trust: request.trust,
            scope_binding: if request.trust == EvidenceTrust::ControlledExecution {
                EvidenceScopeBinding::ControlledExecution
            } else {
                report.scope_binding
            },
            execution_id: request.execution_id.clone(),
            finding_count: report.finding_count,
        })
        .collect::<Vec<_>>();
    report_values.sort_by(|left, right| left.report_id.cmp(&right.report_id));
    for report in &report_values {
        report
            .validate()
            .map_err(|error| EvidenceError::new(error.to_string()))?;
    }
    Ok(StaticAnalysisEvidence {
        schema_version: 1,
        kind: "static_analysis_evidence".to_string(),
        authoritative: true,
        scope: evidence_scope(scope),
        reports: report_values,
        counts,
        findings,
        truncated,
        decision_contract: DecisionContract {
            blocking: "blocking-candidate findings require independent finding verification and normally force DO_NOT_COMMIT when confirmed".to_string(),
            non_blocking: "historical, unbaselined unchanged, maintainability-only, failed-report, and outside-scope findings cannot block by themselves".to_string(),
            verification: "trace every blocking or priority candidate to the changed execution point before final severity and verdict selection".to_string(),
            finalization: "expand truncated evidence before claiming complete static review, disposition every material candidate, and require the final control-plane fingerprint to match this evidence scope".to_string(),
        },
    })
}

fn evidence_counts(
    reports: &[ParsedReport],
    input_findings: usize,
    findings: &[EvidenceFinding],
) -> EvidenceCounts {
    EvidenceCounts {
        reports: reports.len(),
        input_findings,
        deduplicated_findings: findings.len(),
        mapped_to_units: findings
            .iter()
            .filter(|finding| finding.manifest_unit_id.is_some())
            .count(),
        added_line: findings
            .iter()
            .filter(|finding| finding.line_scope == LineScope::Added)
            .count(),
        blocking_candidates: findings
            .iter()
            .filter(|finding| finding.disposition == FindingDisposition::BlockingCandidate)
            .count(),
        priority_candidates: findings
            .iter()
            .filter(|finding| finding.disposition == FindingDisposition::PriorityCandidate)
            .count(),
        notes: findings
            .iter()
            .filter(|finding| finding.disposition == FindingDisposition::Note)
            .count(),
        outside_scope: findings
            .iter()
            .filter(|finding| finding.disposition == FindingDisposition::OutsideScope)
            .count(),
    }
}

fn evidence_scope(scope: &AuthoritativeScope) -> EvidenceScope {
    EvidenceScope {
        source: scope.source,
        head: scope.head.clone(),
        fingerprint: scope.fingerprint.clone(),
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn clean_text(value: Option<&str>, fallback: &str, limit: usize) -> String {
    let source = value.filter(|text| !text.is_empty()).unwrap_or(fallback);
    let without_nul = source.replace('\0', "");
    let collapsed = without_nul.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = if collapsed.is_empty() {
        fallback
    } else {
        &collapsed
    };
    normalized.chars().take(limit).collect()
}

fn normalize_path(value: &str, repository: &Path) -> String {
    let decoded = percent_decode_str(value)
        .decode_utf8_lossy()
        .trim()
        .replace('\\', "/");
    let mut path = decoded.as_str();
    if let Some(file_path) = path.strip_prefix("file://") {
        path = file_path;
    }
    if path.starts_with('/') && path.as_bytes().get(2) == Some(&b':') {
        path = &path[1..];
    }
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        let normalized_repository =
            fs::canonicalize(repository).unwrap_or_else(|_| repository.to_path_buf());
        let normalized_candidate =
            fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
        if let Ok(relative) = normalized_candidate.strip_prefix(&normalized_repository) {
            return relative.to_string_lossy().replace('\\', "/");
        }
        return normalized_candidate.to_string_lossy().replace('\\', "/");
    }
    let path = path.trim_start_matches("./");
    if path.is_empty() {
        "unknown".to_string()
    } else {
        path.to_string()
    }
}

fn compact_report_id(raw: &[u8], run_index: usize, tool_name: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(raw);
    digest.update([0]);
    digest.update(run_index.to_string().as_bytes());
    digest.update([0]);
    digest.update(tool_name.as_bytes());
    digest.update([0]);
    format!("{:x}", digest.finalize())[..16].to_string()
}

fn compact_finding_id(finding: &ParsedFinding) -> String {
    let mut digest = Sha256::new();
    for value in [
        finding.tool.name.clone(),
        finding.rule_id.clone(),
        finding.message.clone(),
        finding.path.clone(),
        finding
            .start_line
            .map(|line| line.to_string())
            .unwrap_or_else(|| "None".to_string()),
        finding
            .end_line
            .map(|line| line.to_string())
            .unwrap_or_else(|| "None".to_string()),
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())[..16].to_string()
}

fn normalize_severity(value: Option<&str>) -> Severity {
    match value
        .unwrap_or("unknown")
        .to_ascii_lowercase()
        .replace('_', "-")
        .as_str()
    {
        "fatal" | "critical" => Severity::Critical,
        "high" | "error" => Severity::Error,
        "medium" | "warning" => Severity::Warning,
        "low" | "info" | "information" | "note" => Severity::Note,
        "none" => Severity::None,
        _ => Severity::Unknown,
    }
}

fn normalize_confidence(value: Option<&str>) -> Confidence {
    match value
        .unwrap_or("unknown")
        .to_ascii_lowercase()
        .replace('_', "-")
        .as_str()
    {
        "veryhigh" | "very-high" => Confidence::VeryHigh,
        "high" => Confidence::High,
        "moderate" | "medium" => Confidence::Medium,
        "low" => Confidence::Low,
        _ => Confidence::Unknown,
    }
}

fn infer_category(
    rule_id: &str,
    message: &str,
    tool_name: &str,
    tags: &[String],
) -> FindingCategory {
    let corpus = format!("{rule_id} {message} {tool_name} {}", tags.join(" ")).to_lowercase();
    let classifiers = [
        (
            FindingCategory::Privacy,
            &["privacy", "pii", "personal-data"][..],
        ),
        (
            FindingCategory::Security,
            &[
                "security",
                "cwe-",
                "owasp",
                "injection",
                "xss",
                "ssrf",
                "auth",
                "vulnerability",
            ][..],
        ),
        (
            FindingCategory::Build,
            &[
                "compiler",
                "compile",
                "type-check",
                "typecheck",
                "type-error",
                "type error",
                "rustc",
                "tsc",
                "mypy",
                "pyright",
                "javac",
            ][..],
        ),
        (
            FindingCategory::Data,
            &["data-loss", "migration", "database", "corruption"][..],
        ),
        (
            FindingCategory::Compatibility,
            &["compatibility", "breaking", "api-contract"][..],
        ),
        (
            FindingCategory::Reliability,
            &["reliability", "deadlock", "race-condition", "resource-leak"][..],
        ),
        (
            FindingCategory::Performance,
            &["performance", "complexity", "n+1"][..],
        ),
        (
            FindingCategory::Correctness,
            &[
                "correctness",
                "null-deref",
                "use-after-free",
                "logic-error",
                "bug",
            ][..],
        ),
        (
            FindingCategory::Maintainability,
            &["maintainability", "style", "format", "documentation"][..],
        ),
    ];
    classifiers
        .iter()
        .find(|(_, needles)| needles.iter().any(|needle| corpus.contains(needle)))
        .map(|(category, _)| *category)
        .unwrap_or(FindingCategory::Unknown)
}

fn severity_order(value: Severity) -> u8 {
    match value {
        Severity::Unknown => 0,
        Severity::None => 1,
        Severity::Note => 2,
        Severity::Warning => 3,
        Severity::Error => 4,
        Severity::Critical => 5,
    }
}

fn confidence_order(value: Confidence) -> u8 {
    match value {
        Confidence::Unknown => 0,
        Confidence::Low => 1,
        Confidence::Medium => 2,
        Confidence::High => 3,
        Confidence::VeryHigh => 4,
    }
}
