use super::contracts::{
    BudgetAmount, BudgetRecord, DecisionContract, EvidenceCounts, EvidenceScope,
    InvalidationReason, ManifestIdentity, NotRunReason, OrchestrationArtifact,
    OrchestrationManifest, OrchestrationRun, OrchestrationSnapshot, OrchestrationStatus,
    RepositoryConfiguration, StaticAnalysisEvidence, StaticAnalysisProfile,
};
use super::executor::{
    build_run_artifact, execute_prepared, prepare_profile, repository_state_digest, sha256_file,
    verify_prepared_integrity, ExecutionLimits, PreparedProfile, RunArtifact,
};
use super::snapshot::{CandidateSnapshot, SnapshotLimits};
use crate::review_scope::{
    open_authoritative_scope, revalidate_scope, AuthoritativeScope, ReviewSource, ScopeRequest,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_MANIFEST_BYTES: u64 = 1_000_000;
const MAX_PROFILE_BYTES: u64 = 1_000_000;

#[derive(Debug, Clone)]
pub struct OrchestrationRequest {
    pub repository: PathBuf,
    pub source: ReviewSource,
    pub expected_scope: String,
    pub manifest_path: PathBuf,
    pub expected_manifest_sha256: String,
    pub allow_repository_configuration: bool,
}

#[derive(Debug, Clone)]
pub struct PreparedManifestProfile {
    pub profile_id: String,
    pub prepared: PreparedProfile,
}

#[derive(Debug, Clone)]
pub struct PreparedOrchestration {
    pub manifest: OrchestrationManifest,
    pub manifest_path: PathBuf,
    pub manifest_sha256: String,
    pub manifest_id: String,
    pub profiles: Vec<PreparedManifestProfile>,
}

#[derive(Debug)]
pub struct OrchestrationOutput {
    pub orchestration: OrchestrationArtifact,
    pub evidence: StaticAnalysisEvidence,
}

impl PreparedOrchestration {
    pub fn revalidate(&self) -> Result<(), OrchestrationError> {
        let (manifest_sha256, _) = sha256_file(&self.manifest_path, Some(MAX_MANIFEST_BYTES))
            .map_err(|error| OrchestrationError::new(error.to_string()))?;
        if manifest_sha256 != self.manifest_sha256 {
            return Err(OrchestrationError::new(
                "static-analysis orchestration manifest changed after preflight",
            ));
        }
        for profile in &self.profiles {
            verify_prepared_integrity(&profile.prepared, "after orchestration preflight")
                .map_err(|error| OrchestrationError::new(error.to_string()))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestrationError {
    message: String,
}

impl OrchestrationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for OrchestrationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OrchestrationError {}

pub fn prepare_orchestration(
    request: &OrchestrationRequest,
) -> Result<PreparedOrchestration, OrchestrationError> {
    if !is_scope_fingerprint(&request.expected_scope) {
        return Err(OrchestrationError::new(
            "--expect-scope is missing or invalid",
        ));
    }
    if !is_sha256(&request.expected_manifest_sha256) {
        return Err(OrchestrationError::new(
            "--expect-manifest-sha256 must be 64 lowercase hexadecimal characters",
        ));
    }
    if !request.manifest_path.is_absolute() {
        return Err(OrchestrationError::new(
            "--manifest must be an absolute path",
        ));
    }
    let repository = fs::canonicalize(&request.repository)
        .map_err(|error| OrchestrationError::new(format!("cannot resolve repository: {error}")))?;
    let manifest_bytes = read_bounded(
        &request.manifest_path,
        MAX_MANIFEST_BYTES,
        "static-analysis orchestration manifest",
    )?;
    let manifest_sha256 = sha256_bytes(&manifest_bytes);
    if manifest_sha256 != request.expected_manifest_sha256 {
        return Err(OrchestrationError::new(
            "manifest SHA256 does not match --expect-manifest-sha256",
        ));
    }
    let manifest: OrchestrationManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|error| {
            OrchestrationError::new(format!(
                "static-analysis orchestration manifest is not valid UTF-8 JSON: {error}"
            ))
        })?;
    manifest
        .validate()
        .map_err(|error| OrchestrationError::new(error.to_string()))?;

    let mut profiles = Vec::with_capacity(manifest.profiles.len());
    for profile_ref in &manifest.profiles {
        let profile_path = Path::new(&profile_ref.path);
        let repository_configuration =
            profile_repository_configuration(profile_path, &profile_ref.sha256)?;
        let allow_profile_configuration = request.allow_repository_configuration
            && repository_configuration == RepositoryConfiguration::ExplicitlyTrusted;
        let prepared = prepare_profile(
            &repository,
            profile_path,
            &profile_ref.sha256,
            allow_profile_configuration,
        )
        .map_err(|error| OrchestrationError::new(error.to_string()))?;
        profiles.push(PreparedManifestProfile {
            profile_id: profile_ref.profile_id.clone(),
            prepared,
        });
    }

    let manifest_path = fs::canonicalize(&request.manifest_path).map_err(|error| {
        OrchestrationError::new(format!(
            "cannot resolve static-analysis orchestration manifest: {error}"
        ))
    })?;
    let prepared = PreparedOrchestration {
        manifest,
        manifest_path,
        manifest_id: manifest_sha256[..16].to_string(),
        manifest_sha256,
        profiles,
    };
    prepared.revalidate()?;
    Ok(prepared)
}

pub fn execute(request: OrchestrationRequest) -> Result<OrchestrationOutput, OrchestrationError> {
    let prepared = prepare_orchestration(&request)?;
    let scope = open_authoritative_scope(ScopeRequest {
        repository: request.repository.clone(),
        source: Some(request.source),
        expected_fingerprint: Some(request.expected_scope.clone()),
    })
    .map_err(|error| OrchestrationError::new(error.to_string()))?;
    let repository = scope.repository.clone();
    let repository_state_before = repository_state_digest(&repository)
        .map_err(|error| OrchestrationError::new(error.to_string()))?;
    let snapshot = CandidateSnapshot::materialize(
        &repository,
        request.source,
        effective_snapshot_limits(&prepared),
    )
    .map_err(|error| OrchestrationError::new(error.to_string()))?;

    let mut runs = Vec::with_capacity(prepared.profiles.len());
    let mut artifacts = Vec::new();
    let mut shared_integrity_failed = false;
    for profile in &prepared.profiles {
        if shared_integrity_failed {
            runs.push(OrchestrationRun::NotRun {
                profile_id: profile.profile_id.clone(),
                reason: NotRunReason::SharedIntegrityFailure,
            });
            continue;
        }
        if let Err(error) = snapshot.verify_unchanged() {
            runs.push(OrchestrationRun::Invalidated {
                profile_id: profile.profile_id.clone(),
                reason: InvalidationReason::SnapshotMutated,
            });
            shared_integrity_failed = true;
            if !is_snapshot_integrity_error(&error.to_string()) {
                return Err(OrchestrationError::new(error.to_string()));
            }
            continue;
        }
        let process = match execute_prepared(
            &profile.prepared,
            &snapshot,
            request.source,
            &request.expected_scope,
            ExecutionLimits {
                timeout: Duration::from_secs(profile.prepared.profile.limits.timeout_seconds),
                max_output_bytes: profile.prepared.profile.limits.max_output_bytes,
            },
        ) {
            Ok(process) => process,
            Err(error) if is_snapshot_integrity_error(&error.to_string()) => {
                runs.push(OrchestrationRun::Invalidated {
                    profile_id: profile.profile_id.clone(),
                    reason: InvalidationReason::SnapshotMutated,
                });
                shared_integrity_failed = true;
                continue;
            }
            Err(error) => return Err(OrchestrationError::new(error.to_string())),
        };
        let artifact = build_run_artifact(
            &repository,
            request.source,
            &request.expected_scope,
            &profile.prepared,
            &snapshot,
            &process,
            prepared.manifest.limits.max_findings,
        )
        .map_err(|error| OrchestrationError::new(error.to_string()))?;
        runs.push(OrchestrationRun::Executed {
            profile_id: profile.profile_id.clone(),
            execution: Box::new(artifact.execution.clone()),
        });
        artifacts.push(artifact);
    }

    let evidence = combine_evidence(&scope, &artifacts, prepared.manifest.limits.max_findings);
    prepared.revalidate()?;
    if repository_state_digest(&repository)
        .map_err(|error| OrchestrationError::new(error.to_string()))?
        != repository_state_before
    {
        return Err(OrchestrationError::new(
            "reviewed repository state changed during static-analysis orchestration",
        ));
    }
    revalidate_scope(&scope).map_err(|error| {
        OrchestrationError::new(format!(
            "review scope changed during static-analysis orchestration: {error}"
        ))
    })?;

    let status = orchestration_status(&runs);
    let budgets = provisional_budget_record(&prepared, &snapshot, &artifacts, &evidence);
    let report_ids = evidence
        .reports
        .iter()
        .map(|report| report.report_id.clone())
        .collect::<Vec<_>>();
    let finding_ids = evidence
        .findings
        .iter()
        .map(|finding| finding.finding_id.clone())
        .collect::<Vec<_>>();
    let orchestration_id = orchestration_id(&request, &prepared, &snapshot, &runs);
    let orchestration = OrchestrationArtifact {
        schema_version: 1,
        kind: "static_analysis_orchestration".to_string(),
        authoritative: true,
        orchestration_id,
        scope: evidence_scope(&scope),
        manifest: ManifestIdentity {
            manifest_id: prepared.manifest_id.clone(),
            name: prepared.manifest.name.clone(),
            sha256: prepared.manifest_sha256.clone(),
        },
        snapshot: OrchestrationSnapshot {
            snapshot_id: snapshot.snapshot_id.clone(),
            kind: "temporary-tracked-files".to_string(),
            sha256: snapshot.sha256.clone(),
            files: snapshot.files,
            bytes: snapshot.bytes,
        },
        status,
        budgets,
        runs,
        report_ids,
        finding_ids,
    };
    orchestration
        .validate(&evidence)
        .map_err(|error| OrchestrationError::new(error.to_string()))?;
    Ok(OrchestrationOutput {
        orchestration,
        evidence,
    })
}

fn effective_snapshot_limits(prepared: &PreparedOrchestration) -> SnapshotLimits {
    SnapshotLimits {
        max_files: prepared
            .profiles
            .iter()
            .map(|item| item.prepared.profile.limits.max_snapshot_files)
            .chain(std::iter::once(prepared.manifest.limits.max_snapshot_files))
            .min()
            .expect("manifest contains at least one profile"),
        max_bytes: prepared
            .profiles
            .iter()
            .map(|item| item.prepared.profile.limits.max_snapshot_bytes)
            .chain(std::iter::once(prepared.manifest.limits.max_snapshot_bytes))
            .min()
            .expect("manifest contains at least one profile"),
    }
}

fn combine_evidence(
    scope: &AuthoritativeScope,
    artifacts: &[RunArtifact],
    max_findings: usize,
) -> StaticAnalysisEvidence {
    let mut reports = Vec::new();
    let mut findings = Vec::new();
    let mut counts = EvidenceCounts {
        reports: 0,
        input_findings: 0,
        deduplicated_findings: 0,
        mapped_to_units: 0,
        added_line: 0,
        blocking_candidates: 0,
        priority_candidates: 0,
        notes: 0,
        outside_scope: 0,
    };
    let mut truncated = false;
    for artifact in artifacts {
        reports.extend(artifact.evidence.reports.iter().cloned());
        findings.extend(artifact.evidence.findings.iter().cloned());
        counts.reports = counts
            .reports
            .saturating_add(artifact.evidence.counts.reports);
        counts.input_findings = counts
            .input_findings
            .saturating_add(artifact.evidence.counts.input_findings);
        counts.deduplicated_findings = counts
            .deduplicated_findings
            .saturating_add(artifact.evidence.counts.deduplicated_findings);
        counts.mapped_to_units = counts
            .mapped_to_units
            .saturating_add(artifact.evidence.counts.mapped_to_units);
        counts.added_line = counts
            .added_line
            .saturating_add(artifact.evidence.counts.added_line);
        counts.blocking_candidates = counts
            .blocking_candidates
            .saturating_add(artifact.evidence.counts.blocking_candidates);
        counts.priority_candidates = counts
            .priority_candidates
            .saturating_add(artifact.evidence.counts.priority_candidates);
        counts.notes = counts.notes.saturating_add(artifact.evidence.counts.notes);
        counts.outside_scope = counts
            .outside_scope
            .saturating_add(artifact.evidence.counts.outside_scope);
        truncated |= artifact.evidence.truncated;
    }
    if findings.len() > max_findings {
        findings.truncate(max_findings);
        truncated = true;
    }
    StaticAnalysisEvidence {
        schema_version: 1,
        kind: "static_analysis_evidence".to_string(),
        authoritative: true,
        scope: evidence_scope(scope),
        reports,
        counts,
        findings,
        truncated,
        decision_contract: artifacts
            .first()
            .map(|artifact| artifact.evidence.decision_contract.clone())
            .unwrap_or_else(empty_decision_contract),
    }
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

fn orchestration_status(runs: &[OrchestrationRun]) -> OrchestrationStatus {
    let accepted = runs
        .iter()
        .filter(|run| match run {
            OrchestrationRun::Executed { execution, .. } => execution.execution.result_accepted,
            _ => false,
        })
        .count();
    if accepted == runs.len() {
        OrchestrationStatus::Completed
    } else if accepted > 0 {
        OrchestrationStatus::Partial
    } else {
        OrchestrationStatus::Failed
    }
}

fn provisional_budget_record(
    prepared: &PreparedOrchestration,
    snapshot: &CandidateSnapshot,
    artifacts: &[RunArtifact],
    evidence: &StaticAnalysisEvidence,
) -> BudgetRecord {
    let execution_initial = prepared
        .manifest
        .limits
        .max_execution_seconds
        .saturating_mul(1_000);
    let execution_consumed = artifacts
        .iter()
        .map(|artifact| artifact.execution.execution.duration_ms)
        .fold(0_u64, u64::saturating_add)
        .min(execution_initial);
    let output_initial = prepared.manifest.limits.max_captured_output_bytes;
    let output_consumed = artifacts
        .iter()
        .map(|artifact| {
            (artifact.execution.execution.stdout_bytes as u64)
                .saturating_add(artifact.execution.execution.stderr_bytes as u64)
        })
        .fold(0_u64, u64::saturating_add)
        .min(output_initial);
    let findings_initial = prepared.manifest.limits.max_findings as u64;
    let findings_consumed = (evidence.counts.deduplicated_findings as u64).min(findings_initial);
    BudgetRecord {
        execution_millis: budget_amount(execution_initial, execution_consumed),
        captured_output_bytes: budget_amount(output_initial, output_consumed),
        findings: budget_amount(findings_initial, findings_consumed),
        snapshot_files: budget_amount(
            prepared.manifest.limits.max_snapshot_files as u64,
            snapshot.files as u64,
        ),
        snapshot_bytes: budget_amount(prepared.manifest.limits.max_snapshot_bytes, snapshot.bytes),
    }
}

fn budget_amount(initial: u64, consumed: u64) -> BudgetAmount {
    BudgetAmount {
        initial,
        consumed,
        remaining: initial.saturating_sub(consumed),
    }
}

fn orchestration_id(
    request: &OrchestrationRequest,
    prepared: &PreparedOrchestration,
    snapshot: &CandidateSnapshot,
    runs: &[OrchestrationRun],
) -> String {
    let mut digest = Sha256::new();
    for value in [
        request.expected_scope.as_str(),
        prepared.manifest_sha256.as_str(),
        snapshot.sha256.as_str(),
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    for run in runs {
        let (profile_id, terminal, execution_id) = match run {
            OrchestrationRun::Executed {
                profile_id,
                execution,
            } => (
                profile_id.as_str(),
                "executed",
                execution.execution_id.as_str(),
            ),
            OrchestrationRun::NotRun { profile_id, reason } => (
                profile_id.as_str(),
                match reason {
                    NotRunReason::BudgetExhausted => "not-run/budget-exhausted",
                    NotRunReason::SharedIntegrityFailure => "not-run/shared-integrity-failure",
                },
                "",
            ),
            OrchestrationRun::Invalidated { profile_id, .. } => {
                (profile_id.as_str(), "invalidated/snapshot-mutated", "")
            }
        };
        for value in [profile_id, terminal, execution_id] {
            digest.update(value.as_bytes());
            digest.update([0]);
        }
    }
    format!("{:x}", digest.finalize())[..16].to_string()
}

fn evidence_scope(scope: &AuthoritativeScope) -> EvidenceScope {
    EvidenceScope {
        source: scope.source,
        head: scope.head.clone(),
        fingerprint: scope.fingerprint.clone(),
    }
}

fn is_snapshot_integrity_error(message: &str) -> bool {
    message.starts_with("analysis snapshot ")
}

fn profile_repository_configuration(
    path: &Path,
    expected_sha256: &str,
) -> Result<RepositoryConfiguration, OrchestrationError> {
    let bytes = read_bounded(path, MAX_PROFILE_BYTES, "static-analysis profile")?;
    if sha256_bytes(&bytes) != expected_sha256 {
        return Err(OrchestrationError::new(
            "profile SHA256 does not match the orchestration manifest",
        ));
    }
    let profile: StaticAnalysisProfile = serde_json::from_slice(&bytes).map_err(|error| {
        OrchestrationError::new(format!(
            "static-analysis profile is not valid UTF-8 JSON: {error}"
        ))
    })?;
    profile
        .validate()
        .map_err(|error| OrchestrationError::new(error.to_string()))?;
    Ok(profile.repository_configuration)
}

fn read_bounded(path: &Path, limit: u64, label: &str) -> Result<Vec<u8>, OrchestrationError> {
    let metadata = fs::metadata(path)
        .map_err(|error| OrchestrationError::new(format!("cannot read {label}: {error}")))?;
    if !metadata.is_file() {
        return Err(OrchestrationError::new(format!(
            "{label} must be a regular file"
        )));
    }
    let mut input = File::open(path)
        .map_err(|error| OrchestrationError::new(format!("cannot read {label}: {error}")))?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut input)
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| OrchestrationError::new(format!("cannot read {label}: {error}")))?;
    if bytes.len() as u64 > limit {
        return Err(OrchestrationError::new(format!(
            "{label} exceeds {limit} bytes"
        )));
    }
    Ok(bytes)
}

fn sha256_bytes(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_scope_fingerprint(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
