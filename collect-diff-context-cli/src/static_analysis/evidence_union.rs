use super::contracts::{
    DecisionContract, EvidenceCounts, EvidenceScope, StaticAnalysisEvidence,
    StaticAnalysisExecution,
};
use super::orchestration::OrchestrationError;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub struct EvidenceRun {
    pub execution: StaticAnalysisExecution,
    pub evidence: StaticAnalysisEvidence,
}

pub fn union_evidence(
    scope: &EvidenceScope,
    runs: &mut [EvidenceRun],
    max_findings: usize,
) -> Result<StaticAnalysisEvidence, OrchestrationError> {
    let mut reports = Vec::new();
    let mut findings = Vec::new();
    let mut counts = empty_counts();
    let mut truncated = false;
    let decision_contract = runs
        .first()
        .map(|run| run.evidence.decision_contract.clone())
        .unwrap_or_else(empty_decision_contract);

    for run in runs {
        if run.execution.scope != *scope || run.evidence.scope != *scope {
            return Err(OrchestrationError::new(
                "evidence union scopes must match the orchestration scope",
            ));
        }
        let execution_id = run.execution.execution_id.clone();
        let mut report_ids = HashMap::new();
        for report in &mut run.evidence.reports {
            let source_report_id = report.report_id.clone();
            let combined_report_id =
                compact_hash("orchestration-report-v1", &execution_id, &source_report_id);
            if report_ids
                .insert(source_report_id, combined_report_id.clone())
                .is_some()
            {
                return Err(OrchestrationError::new(
                    "one evidence run contains duplicate report identifiers",
                ));
            }
            report.report_id = combined_report_id;
        }
        run.execution.evidence.report_ids = run
            .execution
            .evidence
            .report_ids
            .iter()
            .map(|source_report_id| {
                report_ids.get(source_report_id).cloned().ok_or_else(|| {
                    OrchestrationError::new(
                        "execution report link is missing from its source evidence",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut source_finding_ids = HashSet::new();
        for finding in &mut run.evidence.findings {
            if !source_finding_ids.insert(finding.finding_id.clone()) {
                return Err(OrchestrationError::new(
                    "one evidence run contains duplicate finding identifiers",
                ));
            }
            finding.finding_id = compact_hash(
                "orchestration-finding-v1",
                &execution_id,
                &finding.finding_id,
            );
            finding.report_ids = finding
                .report_ids
                .iter()
                .map(|source_report_id| {
                    report_ids.get(source_report_id).cloned().ok_or_else(|| {
                        OrchestrationError::new(
                            "finding report link is missing from its source evidence",
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
        }

        reports.extend(run.evidence.reports.iter().cloned());
        findings.extend(run.evidence.findings.iter().cloned());
        add_counts(&mut counts, &run.evidence.counts);
        truncated |= run.evidence.truncated;
    }

    if findings.len() > max_findings {
        findings.truncate(max_findings);
        truncated = true;
    }
    Ok(StaticAnalysisEvidence {
        schema_version: 1,
        kind: "static_analysis_evidence".to_string(),
        authoritative: true,
        scope: scope.clone(),
        reports,
        counts,
        findings,
        truncated,
        decision_contract,
    })
}

fn compact_hash(label: &str, execution_id: &str, source_id: &str) -> String {
    let mut digest = Sha256::new();
    for value in [label, execution_id, source_id] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())[..16].to_string()
}

fn empty_counts() -> EvidenceCounts {
    EvidenceCounts {
        reports: 0,
        input_findings: 0,
        deduplicated_findings: 0,
        mapped_to_units: 0,
        added_line: 0,
        blocking_candidates: 0,
        priority_candidates: 0,
        notes: 0,
        outside_scope: 0,
    }
}

fn add_counts(target: &mut EvidenceCounts, source: &EvidenceCounts) {
    target.reports = target.reports.saturating_add(source.reports);
    target.input_findings = target.input_findings.saturating_add(source.input_findings);
    target.deduplicated_findings = target
        .deduplicated_findings
        .saturating_add(source.deduplicated_findings);
    target.mapped_to_units = target
        .mapped_to_units
        .saturating_add(source.mapped_to_units);
    target.added_line = target.added_line.saturating_add(source.added_line);
    target.blocking_candidates = target
        .blocking_candidates
        .saturating_add(source.blocking_candidates);
    target.priority_candidates = target
        .priority_candidates
        .saturating_add(source.priority_candidates);
    target.notes = target.notes.saturating_add(source.notes);
    target.outside_scope = target.outside_scope.saturating_add(source.outside_scope);
}

fn empty_decision_contract() -> DecisionContract {
    DecisionContract {
        blocking:
            "blocking candidates require independent verification before they affect the verdict"
                .to_string(),
        non_blocking:
            "invalidated and not-run analyzers are unavailable verification, not clean results"
                .to_string(),
        verification: "preserve every available analyzer result with its execution provenance"
            .to_string(),
        finalization:
            "revalidate scope and authorization before releasing the orchestration artifact"
                .to_string(),
    }
}
