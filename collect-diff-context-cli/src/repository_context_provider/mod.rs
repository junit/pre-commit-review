#[cfg(feature = "test-fixture")]
pub mod baseline_fixture;
pub mod cli;
pub mod cli_contract;
pub mod contract;
pub mod json_rpc;
pub mod model;
pub mod rust_analyzer;
pub mod session;
pub mod snapshot;

use crate::candidate::snapshot::CandidateSnapshot;
use crate::impact_context::cache::file_facts::{
    open_regular_file_no_follow, opened_regular_file_fingerprint,
};
use crate::repository_context_provider::contract::{
    AuthorizedProviderProfile, ProviderCompleteness, ProviderExecutionRecord, ProviderIsolation,
    ProviderLimitation, ProviderMetrics, ProviderNetworkIsolation, RepositoryContextProviderReport,
    RepositoryContextProviderRequest, RepositoryContextProviderStatus, RustAnalyzerProjectModel,
    MAX_DEADLINE_MS,
};
use crate::repository_context_provider::rust_analyzer::{
    initialize_and_gate_with_position_encoding_preference, traverse_call_hierarchy,
    CallHierarchyTraversal, PositionEncodingPreference, Readiness, RustAnalyzerHandshakeError,
};
use crate::repository_context_provider::session::{ManagedLspSession, SessionLaunch};
use crate::repository_context_provider::snapshot::BoundCandidateSnapshot;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::provider_resources::{ProviderResourcePolicy, ResourceAccountingStatus};

const MAX_PROVIDER_PROFILE_BYTES: u64 = 1024 * 1024;
const MAX_PROVIDER_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    InvalidRequest,
    ProfileMismatch,
    StaleBinding,
    Cancelled,
    DeadlineExceeded,
    Preflight,
    Session,
    ReportInvalid,
}

impl ProviderError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest => "provider-request-invalid",
            Self::ProfileMismatch => "provider-profile-mismatch",
            Self::StaleBinding => "provider-stale-binding",
            Self::Cancelled => "provider-cancelled",
            Self::DeadlineExceeded => "provider-deadline-exceeded",
            Self::Preflight => "provider-preflight-failed",
            Self::Session => "provider-session-failed",
            Self::ReportInvalid => "provider-report-invalid",
        }
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ProviderError {}

pub struct ProviderInvocation<'a> {
    pub snapshot: &'a CandidateSnapshot,
    pub model: &'a RustAnalyzerProjectModel,
    pub request: &'a RepositoryContextProviderRequest,
    pub profile: &'a AuthorizedProviderProfile,
    pub cancellation: Arc<AtomicBool>,
}

pub struct ProviderRunMeasurement {
    pub report: RepositoryContextProviderReport,
    pub elapsed_ms: u64,
}

pub fn run_repository_context_provider(
    invocation: ProviderInvocation<'_>,
) -> Result<RepositoryContextProviderReport, ProviderError> {
    run_repository_context_provider_with_policy(
        invocation,
        ProviderResourcePolicy::production(),
        None,
    )
    .map(|measurement| measurement.report)
}

#[cfg(feature = "test-fixture")]
pub fn run_repository_context_provider_measured(
    invocation: ProviderInvocation<'_>,
) -> Result<ProviderRunMeasurement, ProviderError> {
    run_repository_context_provider_with_policy(
        invocation,
        ProviderResourcePolicy::production(),
        None,
    )
}

#[cfg(feature = "test-fixture")]
pub fn run_repository_context_provider_with_postflight_elapsed_ms(
    invocation: ProviderInvocation<'_>,
    elapsed_ms: u64,
) -> Result<RepositoryContextProviderReport, ProviderError> {
    run_repository_context_provider_with_policy(
        invocation,
        ProviderResourcePolicy::production(),
        Some(elapsed_ms),
    )
    .map(|measurement| measurement.report)
}

#[cfg(feature = "test-fixture")]
pub fn run_repository_context_provider_with_postflight_snapshot_hook(
    invocation: ProviderInvocation<'_>,
    hook: &dyn Fn(),
) -> Result<RepositoryContextProviderReport, ProviderError> {
    run_repository_context_provider_with_policy_and_position_encoding_preference(
        invocation,
        ProviderResourcePolicy::production(),
        PositionEncodingPreference::default(),
        None,
        None,
        Some(hook),
        None,
    )
    .map(|measurement| measurement.report)
}

#[cfg(feature = "test-fixture")]
pub fn run_repository_context_provider_with_postflight_and_finalization_hooks(
    invocation: ProviderInvocation<'_>,
    postflight_hook: &dyn Fn(),
    finalization_hook: &dyn Fn(Instant),
) -> Result<ProviderRunMeasurement, ProviderError> {
    run_repository_context_provider_with_policy_and_position_encoding_preference(
        invocation,
        ProviderResourcePolicy::production(),
        PositionEncodingPreference::default(),
        None,
        None,
        Some(postflight_hook),
        Some(finalization_hook),
    )
}

#[cfg(feature = "test-fixture")]
pub fn run_repository_context_provider_with_preflight_and_finalization_hooks(
    invocation: ProviderInvocation<'_>,
    preflight_hook: &dyn Fn(),
    finalization_hook: &dyn Fn(Instant),
) -> Result<RepositoryContextProviderReport, ProviderError> {
    run_repository_context_provider_with_policy_and_position_encoding_preference(
        invocation,
        ProviderResourcePolicy::production(),
        PositionEncodingPreference::default(),
        Some(preflight_hook),
        None,
        None,
        Some(finalization_hook),
    )
    .map(|measurement| measurement.report)
}

#[cfg(feature = "test-fixture")]
pub fn run_repository_context_provider_with_position_encoding_preference(
    invocation: ProviderInvocation<'_>,
    preferred_encoding: contract::PositionEncoding,
) -> Result<RepositoryContextProviderReport, ProviderError> {
    run_repository_context_provider_with_policy_and_position_encoding_preference(
        invocation,
        ProviderResourcePolicy::production(),
        PositionEncodingPreference::preferred(preferred_encoding),
        None,
        None,
        None,
        None,
    )
    .map(|measurement| measurement.report)
}

#[cfg(feature = "test-fixture")]
pub fn run_repository_context_provider_with_resource_policy(
    invocation: ProviderInvocation<'_>,
    policy: ProviderResourcePolicy,
) -> Result<RepositoryContextProviderReport, ProviderError> {
    run_repository_context_provider_with_policy(invocation, policy, None)
        .map(|measurement| measurement.report)
}

fn run_repository_context_provider_with_policy(
    invocation: ProviderInvocation<'_>,
    policy: ProviderResourcePolicy,
    postflight_elapsed_floor_ms: Option<u64>,
) -> Result<ProviderRunMeasurement, ProviderError> {
    run_repository_context_provider_with_policy_and_position_encoding_preference(
        invocation,
        policy,
        PositionEncodingPreference::default(),
        None,
        postflight_elapsed_floor_ms,
        None,
        None,
    )
}

fn run_repository_context_provider_with_policy_and_position_encoding_preference(
    invocation: ProviderInvocation<'_>,
    policy: ProviderResourcePolicy,
    position_encoding_preference: PositionEncodingPreference,
    preflight_hook: Option<&dyn Fn()>,
    postflight_elapsed_floor_ms: Option<u64>,
    postflight_snapshot_hook: Option<&dyn Fn()>,
    finalization_hook: Option<&dyn Fn(Instant)>,
) -> Result<ProviderRunMeasurement, ProviderError> {
    invocation
        .profile
        .validate()
        .map_err(|_| ProviderError::ProfileMismatch)?;
    invocation
        .request
        .validate()
        .map_err(|_| ProviderError::InvalidRequest)?;
    invocation
        .profile
        .validate_request(invocation.request)
        .map_err(|_| ProviderError::ProfileMismatch)?;
    invocation
        .model
        .validate()
        .map_err(|_| ProviderError::InvalidRequest)?;
    if invocation.model.target_triple != invocation.profile.target_triple
        || invocation.request.candidate.project_model_digest != invocation.model.digest
    {
        return Err(ProviderError::InvalidRequest);
    }
    check_cancelled(&invocation.cancellation)?;
    let bound = BoundCandidateSnapshot::new(
        invocation.snapshot,
        invocation.model,
        &invocation.request.candidate,
    )
    .map_err(|_| ProviderError::StaleBinding)?;
    preflight_files(invocation.request, invocation.profile, invocation.snapshot)?;
    check_cancelled(&invocation.cancellation)?;
    if let Some(hook) = preflight_hook {
        hook();
    }

    let limits = &invocation.request.limits;
    let launch = SessionLaunch {
        snapshot: &bound,
        executable: &invocation.request.provider.executable_path,
        executable_sha256: &invocation.request.provider.executable_sha256,
        arguments: &invocation.profile.arguments,
        source: invocation.request.candidate.source,
        scope_fingerprint: &invocation.request.candidate.scope_fingerprint,
        limits,
        cancellation: Arc::clone(&invocation.cancellation),
    };
    let started = Instant::now();
    let mut session = match ManagedLspSession::spawn_with_policy(launch, policy) {
        Ok(session) => session,
        Err(error) if error.code == "process-tree-rss-accounting-unavailable" => {
            let elapsed_ms = elapsed_ms(started);
            let report = empty_report(
                invocation.request,
                invocation.profile,
                invocation.model,
                RepositoryContextProviderStatus::Failed,
                error.code,
                unavailable_resource_metrics(policy, elapsed_ms),
                elapsed_ms,
            )?;
            postflight(
                invocation.request,
                invocation.profile,
                invocation.model,
                invocation.snapshot,
                &invocation.cancellation,
                started,
                limits.deadline_ms,
                false,
                postflight_elapsed_floor_ms,
                postflight_snapshot_hook,
            )?;
            return finalize_report(
                report,
                &invocation.cancellation,
                started,
                limits.deadline_ms,
                finalization_hook,
            );
        }
        Err(_) => return Err(ProviderError::Preflight),
    };
    let handshake = match initialize_and_gate_with_position_encoding_preference(
        &mut session,
        &bound,
        invocation.model,
        &invocation.profile.target_triple,
        position_encoding_preference,
    ) {
        Ok(handshake) => handshake,
        Err(error) => {
            session.terminate();
            check_cancelled(&invocation.cancellation)?;
            let status = status_for_handshake_error(&error);
            let elapsed_ms = elapsed_ms(started);
            let report = empty_report(
                invocation.request,
                invocation.profile,
                invocation.model,
                status,
                error.code,
                session_metrics(&session, 0, elapsed_ms),
                elapsed_ms,
            )?;
            postflight(
                invocation.request,
                invocation.profile,
                invocation.model,
                invocation.snapshot,
                &invocation.cancellation,
                started,
                limits.deadline_ms,
                status == RepositoryContextProviderStatus::Timeout,
                postflight_elapsed_floor_ms,
                postflight_snapshot_hook,
            )?;
            return finalize_report(
                report,
                &invocation.cancellation,
                started,
                limits.deadline_ms,
                finalization_hook,
            );
        }
    };

    let binding_digest = invocation
        .request
        .binding_digest(&invocation.model.algorithm)
        .map_err(|_| ProviderError::InvalidRequest)?;
    let traversal = match traverse_call_hierarchy(
        &mut session,
        &bound,
        &invocation.request.seeds,
        &invocation.request.directions,
        limits,
        handshake.position_encoding,
        &binding_digest,
        &invocation.profile.provider_kind,
        &invocation.profile.provider_version,
    ) {
        Ok(traversal) => traversal,
        Err(error) => {
            session.terminate();
            if error.code == "provider-cancelled" {
                return Err(ProviderError::Cancelled);
            }
            check_cancelled(&invocation.cancellation)?;
            let elapsed_ms = elapsed_ms(started);
            let status = status_for_session_error(error.code);
            let report = empty_report(
                invocation.request,
                invocation.profile,
                invocation.model,
                status,
                error.code,
                session_metrics(&session, 0, elapsed_ms),
                elapsed_ms,
            )?;
            postflight(
                invocation.request,
                invocation.profile,
                invocation.model,
                invocation.snapshot,
                &invocation.cancellation,
                started,
                limits.deadline_ms,
                status == RepositoryContextProviderStatus::Timeout,
                postflight_elapsed_floor_ms,
                postflight_snapshot_hook,
            )?;
            return finalize_report(
                report,
                &invocation.cancellation,
                started,
                limits.deadline_ms,
                finalization_hook,
            );
        }
    };
    if let Err(error) = session.shutdown_and_reap() {
        if error.code == "provider-cancelled" {
            return Err(ProviderError::Cancelled);
        }
        let elapsed_ms = elapsed_ms(started);
        let status = status_for_session_error(error.code);
        let report = empty_report(
            invocation.request,
            invocation.profile,
            invocation.model,
            status,
            error.code,
            session_metrics(&session, 0, elapsed_ms),
            elapsed_ms,
        )?;
        postflight(
            invocation.request,
            invocation.profile,
            invocation.model,
            invocation.snapshot,
            &invocation.cancellation,
            started,
            limits.deadline_ms,
            status == RepositoryContextProviderStatus::Timeout,
            postflight_elapsed_floor_ms,
            postflight_snapshot_hook,
        )?;
        return finalize_report(
            report,
            &invocation.cancellation,
            started,
            limits.deadline_ms,
            finalization_hook,
        );
    }
    check_cancelled(&invocation.cancellation)?;
    postflight(
        invocation.request,
        invocation.profile,
        invocation.model,
        invocation.snapshot,
        &invocation.cancellation,
        started,
        limits.deadline_ms,
        false,
        postflight_elapsed_floor_ms,
        postflight_snapshot_hook,
    )?;

    let report = report_from_traversal(
        invocation.request,
        invocation.profile,
        invocation.model,
        handshake.readiness,
        handshake.limitations,
        traversal,
        handshake.position_encoding,
        &session,
        started,
    )?;
    finalize_report(
        report,
        &invocation.cancellation,
        started,
        limits.deadline_ms,
        finalization_hook,
    )
}

fn check_cancelled(cancellation: &Arc<AtomicBool>) -> Result<(), ProviderError> {
    if cancellation.load(Ordering::Acquire) {
        Err(ProviderError::Cancelled)
    } else {
        Ok(())
    }
}

fn preflight_files(
    request: &RepositoryContextProviderRequest,
    profile: &AuthorizedProviderProfile,
    snapshot: &CandidateSnapshot,
) -> Result<(), ProviderError> {
    let snapshot_root = fs::canonicalize(snapshot.path()).map_err(|_| ProviderError::Preflight)?;
    for path in [
        &request.provider.profile_path,
        &request.provider.executable_path,
    ] {
        let canonical = fs::canonicalize(path).map_err(|_| ProviderError::Preflight)?;
        if canonical.starts_with(&snapshot_root) {
            return Err(ProviderError::Preflight);
        }
    }
    let profile_bytes = read_file_digest(
        &request.provider.profile_path,
        MAX_PROVIDER_PROFILE_BYTES,
        || Ok(()),
        ProviderError::Preflight,
    )?;
    if profile_bytes != profile.sha256() || request.provider.profile_sha256 != profile_bytes {
        return Err(ProviderError::ProfileMismatch);
    }
    let executable_bytes = read_file_digest(
        &request.provider.executable_path,
        MAX_PROVIDER_EXECUTABLE_BYTES,
        || Ok(()),
        ProviderError::Preflight,
    )?;
    if executable_bytes != request.provider.executable_sha256
        || executable_bytes != profile.executable_sha256
    {
        return Err(ProviderError::Preflight);
    }
    Ok(())
}

fn read_file_digest(
    path: &std::path::Path,
    maximum_bytes: u64,
    mut check_runtime: impl FnMut() -> Result<(), ProviderError>,
    read_error: ProviderError,
) -> Result<String, ProviderError> {
    check_runtime()?;
    let opened = open_regular_file_no_follow(path);
    check_runtime()?;
    let mut file = opened.map_err(|_| read_error.clone())?;
    let fingerprint = opened_regular_file_fingerprint(&file);
    check_runtime()?;
    let before = fingerprint.map_err(|_| read_error.clone())?;
    if before.size() > maximum_bytes {
        return Err(read_error);
    }
    let mut input = (&mut file).take(maximum_bytes.saturating_add(1));
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        check_runtime()?;
        let read_result = input.read(&mut buffer);
        check_runtime()?;
        let read = read_result.map_err(|_| read_error.clone())?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(read as u64);
        if size > maximum_bytes {
            return Err(read_error);
        }
        digest.update(&buffer[..read]);
    }
    check_runtime()?;
    let fingerprint = opened_regular_file_fingerprint(&file);
    check_runtime()?;
    let after = fingerprint.map_err(|_| read_error.clone())?;
    if before != after || size != before.size() {
        return Err(read_error);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[allow(clippy::too_many_arguments)]
fn postflight(
    request: &RepositoryContextProviderRequest,
    profile: &AuthorizedProviderProfile,
    model: &RustAnalyzerProjectModel,
    snapshot: &CandidateSnapshot,
    cancellation: &Arc<AtomicBool>,
    started: Instant,
    deadline_ms: u64,
    internal_timeout: bool,
    elapsed_floor_ms: Option<u64>,
    snapshot_hook: Option<&dyn Fn()>,
) -> Result<(), ProviderError> {
    let check_runtime = || {
        check_runtime_deadline(cancellation, started, deadline_ms, internal_timeout)?;
        elapsed_floor_ms.map_or(Ok(()), |elapsed_ms| {
            ensure_elapsed_within_deadline(elapsed_ms, deadline_ms, internal_timeout)
        })
    };
    check_runtime()?;
    let snapshot_validation = snapshot.verify_unchanged();
    if let Some(hook) = snapshot_hook {
        hook();
    }
    check_runtime()?;
    snapshot_validation.map_err(|_| ProviderError::StaleBinding)?;
    let model_validation = model.validate();
    check_runtime()?;
    model_validation.map_err(|_| ProviderError::StaleBinding)?;
    if model.digest != request.candidate.project_model_digest {
        check_runtime()?;
        return Err(ProviderError::StaleBinding);
    }
    let profile_digest = read_file_digest(
        &request.provider.profile_path,
        MAX_PROVIDER_PROFILE_BYTES,
        check_runtime,
        ProviderError::StaleBinding,
    );
    check_runtime()?;
    let profile_digest = profile_digest?;
    if profile_digest != profile.sha256() {
        check_runtime()?;
        return Err(ProviderError::StaleBinding);
    }
    let executable = read_file_digest(
        &request.provider.executable_path,
        MAX_PROVIDER_EXECUTABLE_BYTES,
        check_runtime,
        ProviderError::StaleBinding,
    );
    check_runtime()?;
    let executable = executable?;
    if executable != profile.executable_sha256 {
        check_runtime()?;
        return Err(ProviderError::StaleBinding);
    }
    Ok(())
}

fn check_runtime_deadline(
    cancellation: &Arc<AtomicBool>,
    started: Instant,
    deadline_ms: u64,
    internal_timeout: bool,
) -> Result<(), ProviderError> {
    check_cancelled(cancellation)?;
    ensure_elapsed_within_deadline(unbounded_elapsed_ms(started), deadline_ms, internal_timeout)
}

fn ensure_elapsed_within_deadline(
    elapsed_ms: u64,
    deadline_ms: u64,
    internal_timeout: bool,
) -> Result<(), ProviderError> {
    let hard_deadline_ms = if internal_timeout {
        MAX_DEADLINE_MS
    } else {
        deadline_ms
    };
    if elapsed_ms > hard_deadline_ms {
        Err(ProviderError::DeadlineExceeded)
    } else {
        Ok(())
    }
}

fn status_for_handshake_error(
    error: &RustAnalyzerHandshakeError,
) -> RepositoryContextProviderStatus {
    if error.code == "provider-timeout" {
        RepositoryContextProviderStatus::Timeout
    } else if error.code.contains("capability") || error.code.contains("readiness-unavailable") {
        RepositoryContextProviderStatus::Unavailable
    } else if error.code.contains("invalid") {
        RepositoryContextProviderStatus::InvalidOutput
    } else {
        RepositoryContextProviderStatus::Failed
    }
}

fn status_for_session_error(code: &str) -> RepositoryContextProviderStatus {
    if code == "provider-timeout" {
        RepositoryContextProviderStatus::Timeout
    } else if code.contains("invalid") || code.contains("frame") || code.contains("message") {
        RepositoryContextProviderStatus::InvalidOutput
    } else {
        RepositoryContextProviderStatus::Failed
    }
}

fn base_provider_record(
    request: &RepositoryContextProviderRequest,
    profile: &AuthorizedProviderProfile,
    model: &RustAnalyzerProjectModel,
    encoding: Option<contract::PositionEncoding>,
) -> ProviderExecutionRecord {
    ProviderExecutionRecord {
        kind: profile.provider_kind.clone(),
        version: profile.provider_version.clone(),
        profile_sha256: request.provider.profile_sha256.clone(),
        executable_sha256: request.provider.executable_sha256.clone(),
        configuration_sha256: request.provider.configuration_sha256.clone(),
        target_triple: profile.target_triple.clone(),
        toolchain_mode: profile.toolchain_mode.clone(),
        project_model_algorithm: model.algorithm.clone(),
        negotiated_encoding: encoding,
    }
}

fn empty_report(
    request: &RepositoryContextProviderRequest,
    profile: &AuthorizedProviderProfile,
    model: &RustAnalyzerProjectModel,
    status: RepositoryContextProviderStatus,
    limitation_code: &str,
    metrics: ProviderMetrics,
    elapsed_ms: u64,
) -> Result<RepositoryContextProviderReport, ProviderError> {
    let mut limitations = Vec::new();
    if !limitation_code.is_empty() {
        limitations.push(ProviderLimitation {
            code: limitation_code.to_string(),
            message: "rust-analyzer execution did not produce retained facts".to_string(),
            changed_symbol_id: None,
            path: None,
        });
    }
    let query_completeness = match status {
        RepositoryContextProviderStatus::Unavailable
        | RepositoryContextProviderStatus::Timeout
        | RepositoryContextProviderStatus::InvalidOutput
        | RepositoryContextProviderStatus::Failed => ProviderCompleteness::Unavailable,
        RepositoryContextProviderStatus::Completed => ProviderCompleteness::Complete,
        RepositoryContextProviderStatus::Partial => ProviderCompleteness::Partial,
    };
    let mut report = RepositoryContextProviderReport {
        schema_version: 1,
        kind: "repository_context_provider_report".to_string(),
        candidate: (&request.candidate).into(),
        provider: base_provider_record(request, profile, model, None),
        status,
        index_completeness: ProviderCompleteness::Unknown,
        query_completeness,
        seed_symbols: Vec::new(),
        related_symbols: Vec::new(),
        edges: Vec::new(),
        limitations,
        isolation: ProviderIsolation {
            network: ProviderNetworkIsolation::BestEffortOffline,
            shell_enabled: false,
            original_repository_access: false,
        },
        metrics: ProviderMetrics {
            elapsed_ms,
            ..metrics
        },
    };
    report.metrics.report_bytes = serde_json::to_vec(&report)
        .map_err(|_| ProviderError::ReportInvalid)?
        .len();
    report
        .validate()
        .map_err(|_| ProviderError::ReportInvalid)?;
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn report_from_traversal(
    request: &RepositoryContextProviderRequest,
    profile: &AuthorizedProviderProfile,
    model: &RustAnalyzerProjectModel,
    readiness: Readiness,
    readiness_limitations: Vec<String>,
    traversal: CallHierarchyTraversal,
    encoding: contract::PositionEncoding,
    session: &ManagedLspSession,
    started: Instant,
) -> Result<RepositoryContextProviderReport, ProviderError> {
    let mut limitations = traversal.limitations;
    for code in readiness_limitations {
        limitations.push(ProviderLimitation {
            code,
            message: "rust-analyzer reported degraded readiness".to_string(),
            changed_symbol_id: None,
            path: None,
        });
    }
    limitations.sort();
    limitations.dedup();
    let status = if readiness == Readiness::Warning || !limitations.is_empty() {
        RepositoryContextProviderStatus::Partial
    } else {
        RepositoryContextProviderStatus::Completed
    };
    let query_completeness = if status == RepositoryContextProviderStatus::Completed {
        ProviderCompleteness::Complete
    } else {
        ProviderCompleteness::Partial
    };
    let elapsed_ms = elapsed_ms(started);
    let session_metrics = session.metrics();
    let mut report = RepositoryContextProviderReport {
        schema_version: 1,
        kind: "repository_context_provider_report".to_string(),
        candidate: (&request.candidate).into(),
        provider: base_provider_record(request, profile, model, Some(encoding)),
        status,
        index_completeness: ProviderCompleteness::Unknown,
        query_completeness,
        seed_symbols: traversal.seed_symbols,
        related_symbols: traversal.related_symbols,
        edges: traversal.edges,
        limitations,
        isolation: ProviderIsolation {
            network: ProviderNetworkIsolation::BestEffortOffline,
            shell_enabled: false,
            original_repository_access: false,
        },
        metrics: ProviderMetrics {
            requests: session_metrics.requests,
            messages: session_metrics.messages,
            notifications: session_metrics.notifications,
            server_requests: session_metrics.server_requests,
            invalid_messages: session_metrics.invalid_messages,
            call_ranges: 0,
            protocol_bytes: 0,
            stderr_bytes: session_metrics.stderr_bytes,
            source_bytes: traversal.source_bytes,
            nodes: 0,
            edges: 0,
            report_bytes: 0,
            elapsed_ms,
            process_tree_peak_rss_bytes: session_metrics.process_tree_peak_rss_bytes,
            process_tree_sample_interval_ms: session_metrics.process_tree_sample_interval_ms,
            process_tree_accounting: session_metrics.process_tree_accounting,
        },
    };
    report.metrics.nodes = report.seed_symbols.len() + report.related_symbols.len();
    report.metrics.edges = report.edges.len();
    report.metrics.call_ranges = report.edges.len();
    report.metrics.report_bytes = serde_json::to_vec(&report)
        .map_err(|_| ProviderError::ReportInvalid)?
        .len();
    report
        .validate()
        .map_err(|_| ProviderError::ReportInvalid)?;
    Ok(report)
}

fn finalize_report(
    mut report: RepositoryContextProviderReport,
    cancellation: &Arc<AtomicBool>,
    started: Instant,
    deadline_ms: u64,
    finalization_hook: Option<&dyn Fn(Instant)>,
) -> Result<ProviderRunMeasurement, ProviderError> {
    let internal_timeout = report.status == RepositoryContextProviderStatus::Timeout;
    let hard_deadline_ms = if internal_timeout {
        MAX_DEADLINE_MS
    } else {
        deadline_ms
    };
    check_runtime_deadline(cancellation, started, deadline_ms, internal_timeout)?;
    if let Some(hook) = finalization_hook {
        hook(started);
        check_runtime_deadline(cancellation, started, deadline_ms, internal_timeout)?;
    }
    let observed_elapsed_ms = unbounded_elapsed_ms(started);
    report.metrics.elapsed_ms = observed_elapsed_ms.min(hard_deadline_ms);
    stabilize_report_size(
        &mut report,
        cancellation,
        started,
        deadline_ms,
        internal_timeout,
    )?;

    let validation = report.validate();
    check_runtime_deadline(cancellation, started, deadline_ms, internal_timeout)?;
    validation.map_err(|_| ProviderError::ReportInvalid)?;

    loop {
        let observed_elapsed_ms = unbounded_elapsed_ms(started);
        check_runtime_deadline(cancellation, started, deadline_ms, internal_timeout)?;
        report.metrics.elapsed_ms = observed_elapsed_ms.min(hard_deadline_ms);
        let serialized = serde_json::to_vec(&report);
        check_runtime_deadline(cancellation, started, deadline_ms, internal_timeout)?;
        let serialized_bytes = serialized.map_err(|_| ProviderError::ReportInvalid)?.len();
        if serialized_bytes > contract::MAX_REPORT_BYTES {
            return Err(ProviderError::ReportInvalid);
        }
        if serialized_bytes != report.metrics.report_bytes {
            report.metrics.report_bytes = serialized_bytes;
            continue;
        }

        let finalized_elapsed_ms = unbounded_elapsed_ms(started);
        check_runtime_deadline(cancellation, started, deadline_ms, internal_timeout)?;
        if decimal_digits(finalized_elapsed_ms) != decimal_digits(report.metrics.elapsed_ms) {
            continue;
        }
        report.metrics.elapsed_ms = finalized_elapsed_ms;
        let validation = report.validate();
        check_runtime_deadline(cancellation, started, deadline_ms, internal_timeout)?;
        validation.map_err(|_| ProviderError::ReportInvalid)?;
        let measured_elapsed_ms = unbounded_elapsed_ms(started);
        ensure_elapsed_within_deadline(measured_elapsed_ms, deadline_ms, internal_timeout)?;
        if decimal_digits(measured_elapsed_ms) != decimal_digits(finalized_elapsed_ms) {
            continue;
        }
        report.metrics.elapsed_ms = measured_elapsed_ms;
        return Ok(ProviderRunMeasurement {
            report,
            elapsed_ms: measured_elapsed_ms,
        });
    }
}

fn decimal_digits(value: u64) -> u32 {
    value.checked_ilog10().unwrap_or(0) + 1
}

fn stabilize_report_size(
    report: &mut RepositoryContextProviderReport,
    cancellation: &Arc<AtomicBool>,
    started: Instant,
    deadline_ms: u64,
    internal_timeout: bool,
) -> Result<(), ProviderError> {
    report.metrics.report_bytes = 0;
    loop {
        let serialized = serde_json::to_vec(report);
        check_runtime_deadline(cancellation, started, deadline_ms, internal_timeout)?;
        let serialized_bytes = serialized.map_err(|_| ProviderError::ReportInvalid)?.len();
        if serialized_bytes > contract::MAX_REPORT_BYTES {
            return Err(ProviderError::ReportInvalid);
        }
        if serialized_bytes == report.metrics.report_bytes {
            return Ok(());
        }
        report.metrics.report_bytes = serialized_bytes;
    }
}

fn session_metrics(
    session: &ManagedLspSession,
    source_bytes: usize,
    elapsed_ms: u64,
) -> ProviderMetrics {
    let metrics = session.metrics();
    ProviderMetrics {
        requests: metrics.requests,
        messages: metrics.messages,
        notifications: metrics.notifications,
        server_requests: metrics.server_requests,
        invalid_messages: metrics.invalid_messages,
        call_ranges: 0,
        protocol_bytes: 0,
        stderr_bytes: metrics.stderr_bytes,
        source_bytes,
        nodes: 0,
        edges: 0,
        report_bytes: 0,
        elapsed_ms,
        process_tree_peak_rss_bytes: metrics.process_tree_peak_rss_bytes,
        process_tree_sample_interval_ms: metrics.process_tree_sample_interval_ms,
        process_tree_accounting: metrics.process_tree_accounting,
    }
}

fn unavailable_resource_metrics(
    policy: ProviderResourcePolicy,
    elapsed_ms: u64,
) -> ProviderMetrics {
    ProviderMetrics {
        requests: 0,
        messages: 0,
        notifications: 0,
        server_requests: 0,
        invalid_messages: 0,
        call_ranges: 0,
        protocol_bytes: 0,
        stderr_bytes: 0,
        source_bytes: 0,
        nodes: 0,
        edges: 0,
        report_bytes: 0,
        elapsed_ms,
        process_tree_peak_rss_bytes: 0,
        process_tree_sample_interval_ms: policy.interval_ms(),
        process_tree_accounting: ResourceAccountingStatus::Unavailable,
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    bounded_elapsed_ms(unbounded_elapsed_ms(started))
}

fn unbounded_elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn bounded_elapsed_ms(elapsed_ms: u64) -> u64 {
    elapsed_ms.min(MAX_DEADLINE_MS)
}

#[cfg(test)]
mod tests {
    use super::bounded_elapsed_ms;
    use crate::repository_context_provider::contract::MAX_DEADLINE_MS;

    #[test]
    fn elapsed_metrics_are_bounded_for_safe_timeout_reports() {
        assert_eq!(bounded_elapsed_ms(MAX_DEADLINE_MS + 5_000), MAX_DEADLINE_MS);
        assert_eq!(bounded_elapsed_ms(123), 123);
    }
}
