use super::contracts::{
    BudgetAmount, BudgetRecord, EvidenceScope, InvalidationReason, ManifestIdentity, NotRunReason,
    OrchestrationArtifact, OrchestrationManifest, OrchestrationRun, OrchestrationSnapshot,
    OrchestrationStatus, ProfileLimits, RepositoryConfiguration, StaticAnalysisEvidence,
    StaticAnalysisProfile,
};
use super::evidence_union::{union_evidence, EvidenceRun};
use super::executor::{
    build_run_artifact, execute_prepared_with_clock, prepare_profile, repository_state_digest,
    sha256_file, verify_prepared_integrity, Clock, ExecutionLimits, PreparedProfile,
    ProcessOutcome, SystemClock,
};
use crate::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
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
const MAX_SOURCE_FINDINGS: usize = 5_000;

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
    pub(crate) fn new(message: impl Into<String>) -> Self {
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
    let mut has_explicitly_trusted_profile = false;
    for profile_ref in &manifest.profiles {
        let profile_path = Path::new(&profile_ref.path);
        let repository_configuration =
            profile_repository_configuration(profile_path, &profile_ref.sha256)?;
        has_explicitly_trusted_profile |=
            repository_configuration == RepositoryConfiguration::ExplicitlyTrusted;
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
    if request.allow_repository_configuration && !has_explicitly_trusted_profile {
        return Err(OrchestrationError::new(
            "--allow-repository-configuration is valid only when at least one profile is explicitly trusted",
        ));
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
    let clock = SystemClock::new();
    execute_with_clock(request, &clock)
}

pub(crate) fn execute_with_clock(
    request: OrchestrationRequest,
    clock: &dyn Clock,
) -> Result<OrchestrationOutput, OrchestrationError> {
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
    let mut budgets = BudgetLedger::new(&prepared);
    budgets.record_snapshot(&snapshot);

    let mut runs = Vec::with_capacity(prepared.profiles.len());
    let mut artifacts = Vec::new();
    let mut shared_integrity_failed = false;
    let mut budget_exhausted = false;
    for profile in &prepared.profiles {
        if shared_integrity_failed {
            runs.push(OrchestrationRun::NotRun {
                profile_id: profile.profile_id.clone(),
                reason: NotRunReason::SharedIntegrityFailure,
            });
            continue;
        }
        if budget_exhausted {
            runs.push(OrchestrationRun::NotRun {
                profile_id: profile.profile_id.clone(),
                reason: NotRunReason::BudgetExhausted,
            });
            continue;
        }
        let Some(execution_limits) = budgets.effective_limits(&profile.prepared.profile.limits)
        else {
            runs.push(OrchestrationRun::NotRun {
                profile_id: profile.profile_id.clone(),
                reason: NotRunReason::BudgetExhausted,
            });
            budget_exhausted = true;
            continue;
        };
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
        let process = match execute_prepared_with_clock(
            &profile.prepared,
            &snapshot,
            request.source,
            &request.expected_scope,
            execution_limits,
            clock,
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
        budgets.consume(&process);
        let artifact = build_run_artifact(
            &repository,
            request.source,
            &request.expected_scope,
            &profile.prepared,
            &snapshot,
            &process,
            MAX_SOURCE_FINDINGS,
        )
        .map_err(|error| OrchestrationError::new(error.to_string()))?;
        runs.push(OrchestrationRun::Executed {
            profile_id: profile.profile_id.clone(),
            execution: Box::new(artifact.execution.clone()),
        });
        artifacts.push(artifact);
    }

    let combined_scope = evidence_scope(&scope);
    let mut evidence_runs = artifacts
        .into_iter()
        .map(|artifact| EvidenceRun {
            execution: artifact.execution,
            evidence: artifact.evidence,
        })
        .collect::<Vec<_>>();
    let evidence = union_evidence(
        &combined_scope,
        &mut evidence_runs,
        prepared.manifest.limits.max_findings,
    )?;
    let mut updated_executions = evidence_runs.iter();
    for run in &mut runs {
        if let OrchestrationRun::Executed { execution, .. } = run {
            let updated = updated_executions.next().ok_or_else(|| {
                OrchestrationError::new("executed run count does not match evidence run count")
            })?;
            **execution = updated.execution.clone();
        }
    }
    if updated_executions.next().is_some() {
        return Err(OrchestrationError::new(
            "evidence run count does not match executed run count",
        ));
    }
    budgets.record_findings(evidence.counts.deduplicated_findings);
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
    let budget_record = budgets.record();
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
        budgets: budget_record,
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

struct BudgetLedger {
    initial_millis: u64,
    remaining_millis: u64,
    initial_output_bytes: usize,
    remaining_output_bytes: usize,
    finding_limit: usize,
    finding_consumed: usize,
    snapshot_file_limit: usize,
    snapshot_files_consumed: usize,
    snapshot_byte_limit: u64,
    snapshot_bytes_consumed: u64,
}

impl BudgetLedger {
    fn new(prepared: &PreparedOrchestration) -> Self {
        let initial_millis = prepared
            .manifest
            .limits
            .max_execution_seconds
            .saturating_mul(1_000);
        let initial_output_bytes =
            usize::try_from(prepared.manifest.limits.max_captured_output_bytes)
                .expect("manifest output limit fits usize");
        Self {
            initial_millis,
            remaining_millis: initial_millis,
            initial_output_bytes,
            remaining_output_bytes: initial_output_bytes,
            finding_limit: prepared.manifest.limits.max_findings,
            finding_consumed: 0,
            snapshot_file_limit: prepared.manifest.limits.max_snapshot_files,
            snapshot_files_consumed: 0,
            snapshot_byte_limit: prepared.manifest.limits.max_snapshot_bytes,
            snapshot_bytes_consumed: 0,
        }
    }

    fn effective_limits(&self, profile: &ProfileLimits) -> Option<ExecutionLimits> {
        if self.remaining_millis == 0 || self.remaining_output_bytes == 0 {
            return None;
        }
        let timeout_millis = self
            .remaining_millis
            .min(profile.timeout_seconds.saturating_mul(1_000));
        let max_combined_output_bytes = self
            .remaining_output_bytes
            .min(profile.max_output_bytes.saturating_mul(2));
        if timeout_millis == 0 || max_combined_output_bytes == 0 {
            return None;
        }
        Some(ExecutionLimits {
            timeout: Duration::from_millis(timeout_millis),
            max_stream_output_bytes: profile.max_output_bytes,
            max_combined_output_bytes,
        })
    }

    fn consume(&mut self, outcome: &ProcessOutcome) {
        self.remaining_millis = self.remaining_millis.saturating_sub(outcome.duration_ms);
        let captured = outcome.stdout_bytes.saturating_add(outcome.stderr_bytes);
        self.remaining_output_bytes = self.remaining_output_bytes.saturating_sub(captured);
    }

    fn record_findings(&mut self, total_independent: usize) {
        self.finding_consumed = total_independent.min(self.finding_limit);
    }

    fn record_snapshot(&mut self, snapshot: &CandidateSnapshot) {
        self.snapshot_files_consumed = snapshot.files;
        self.snapshot_bytes_consumed = snapshot.bytes;
    }

    fn record(&self) -> BudgetRecord {
        BudgetRecord {
            execution_millis: BudgetAmount {
                initial: self.initial_millis,
                consumed: self.initial_millis.saturating_sub(self.remaining_millis),
                remaining: self.remaining_millis,
            },
            captured_output_bytes: BudgetAmount {
                initial: self.initial_output_bytes as u64,
                consumed: self
                    .initial_output_bytes
                    .saturating_sub(self.remaining_output_bytes) as u64,
                remaining: self.remaining_output_bytes as u64,
            },
            findings: BudgetAmount {
                initial: self.finding_limit as u64,
                consumed: self.finding_consumed as u64,
                remaining: self.finding_limit.saturating_sub(self.finding_consumed) as u64,
            },
            snapshot_files: BudgetAmount {
                initial: self.snapshot_file_limit as u64,
                consumed: self.snapshot_files_consumed as u64,
                remaining: self
                    .snapshot_file_limit
                    .saturating_sub(self.snapshot_files_consumed) as u64,
            },
            snapshot_bytes: BudgetAmount {
                initial: self.snapshot_byte_limit,
                consumed: self.snapshot_bytes_consumed,
                remaining: self
                    .snapshot_byte_limit
                    .saturating_sub(self.snapshot_bytes_consumed),
            },
        }
    }
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
