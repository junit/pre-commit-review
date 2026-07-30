pub mod cli;
pub mod cli_contract;
pub mod contract;
pub mod json_rpc;
pub mod model;
pub mod rust_analyzer;
pub mod session;
pub mod snapshot;

use crate::candidate::snapshot::CandidateSnapshot;
use crate::repository_context_provider::contract::{
    AuthorizedProviderProfile, ProviderCompleteness, ProviderExecutionRecord, ProviderIsolation,
    ProviderLimitation, ProviderMetrics, ProviderNetworkIsolation, RepositoryContextProviderReport,
    RepositoryContextProviderRequest, RepositoryContextProviderStatus, RustAnalyzerProjectModel,
};
use crate::repository_context_provider::rust_analyzer::{
    initialize_and_gate, traverse_call_hierarchy, CallHierarchyTraversal, Readiness,
    RustAnalyzerHandshakeError,
};
use crate::repository_context_provider::session::{ManagedLspSession, SessionLaunch};
use crate::repository_context_provider::snapshot::BoundCandidateSnapshot;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::provider_resources::{ProviderResourcePolicy, ResourceAccountingStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    InvalidRequest,
    ProfileMismatch,
    StaleBinding,
    Cancelled,
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

pub fn run_repository_context_provider(
    invocation: ProviderInvocation<'_>,
) -> Result<RepositoryContextProviderReport, ProviderError> {
    run_repository_context_provider_with_policy(invocation, ProviderResourcePolicy::production())
}

#[cfg(feature = "test-fixture")]
pub fn run_repository_context_provider_with_resource_policy(
    invocation: ProviderInvocation<'_>,
    policy: ProviderResourcePolicy,
) -> Result<RepositoryContextProviderReport, ProviderError> {
    run_repository_context_provider_with_policy(invocation, policy)
}

fn run_repository_context_provider_with_policy(
    invocation: ProviderInvocation<'_>,
    policy: ProviderResourcePolicy,
) -> Result<RepositoryContextProviderReport, ProviderError> {
    let started = Instant::now();
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
    let mut session = match ManagedLspSession::spawn_with_policy(launch, policy) {
        Ok(session) => session,
        Err(error) if error.code == "process-tree-rss-accounting-unavailable" => {
            let elapsed_ms = started.elapsed().as_millis() as u64;
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
            )?;
            return Ok(report);
        }
        Err(_) => return Err(ProviderError::Preflight),
    };
    let handshake = match initialize_and_gate(
        &mut session,
        &bound,
        invocation.model,
        &invocation.profile.target_triple,
    ) {
        Ok(handshake) => handshake,
        Err(error) => {
            session.terminate();
            check_cancelled(&invocation.cancellation)?;
            let status = status_for_handshake_error(&error);
            let report = empty_report(
                invocation.request,
                invocation.profile,
                invocation.model,
                status,
                error.code,
                session_metrics(&session, 0, started.elapsed().as_millis() as u64),
                started.elapsed().as_millis() as u64,
            )?;
            postflight(
                invocation.request,
                invocation.profile,
                invocation.model,
                invocation.snapshot,
            )?;
            return Ok(report);
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
            let report = empty_report(
                invocation.request,
                invocation.profile,
                invocation.model,
                status_for_session_error(error.code),
                error.code,
                session_metrics(&session, 0, started.elapsed().as_millis() as u64),
                started.elapsed().as_millis() as u64,
            )?;
            postflight(
                invocation.request,
                invocation.profile,
                invocation.model,
                invocation.snapshot,
            )?;
            return Ok(report);
        }
    };
    if let Err(error) = session.shutdown_and_reap() {
        if error.code == "provider-cancelled" {
            return Err(ProviderError::Cancelled);
        }
        let report = empty_report(
            invocation.request,
            invocation.profile,
            invocation.model,
            status_for_session_error(error.code),
            error.code,
            session_metrics(&session, 0, started.elapsed().as_millis() as u64),
            started.elapsed().as_millis() as u64,
        )?;
        postflight(
            invocation.request,
            invocation.profile,
            invocation.model,
            invocation.snapshot,
        )?;
        return Ok(report);
    }
    check_cancelled(&invocation.cancellation)?;
    postflight(
        invocation.request,
        invocation.profile,
        invocation.model,
        invocation.snapshot,
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
    Ok(report)
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
    let profile_bytes =
        read_file_digest(&request.provider.profile_path).map_err(|_| ProviderError::Preflight)?;
    if profile_bytes != profile.sha256() || request.provider.profile_sha256 != profile_bytes {
        return Err(ProviderError::ProfileMismatch);
    }
    let executable_bytes = read_file_digest(&request.provider.executable_path)
        .map_err(|_| ProviderError::Preflight)?;
    if executable_bytes != request.provider.executable_sha256
        || executable_bytes != profile.executable_sha256
    {
        return Err(ProviderError::Preflight);
    }
    Ok(())
}

fn read_file_digest(path: &std::path::Path) -> Result<String, std::io::Error> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn postflight(
    request: &RepositoryContextProviderRequest,
    profile: &AuthorizedProviderProfile,
    model: &RustAnalyzerProjectModel,
    snapshot: &CandidateSnapshot,
) -> Result<(), ProviderError> {
    snapshot
        .verify_unchanged()
        .map_err(|_| ProviderError::StaleBinding)?;
    model.validate().map_err(|_| ProviderError::StaleBinding)?;
    if model.digest != request.candidate.project_model_digest {
        return Err(ProviderError::StaleBinding);
    }
    let profile_digest = read_file_digest(&request.provider.profile_path)
        .map_err(|_| ProviderError::StaleBinding)?;
    if profile_digest != profile.sha256() {
        return Err(ProviderError::StaleBinding);
    }
    let executable = read_file_digest(&request.provider.executable_path)
        .map_err(|_| ProviderError::StaleBinding)?;
    if executable != profile.executable_sha256 {
        return Err(ProviderError::StaleBinding);
    }
    Ok(())
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
    let elapsed_ms = started.elapsed().as_millis() as u64;
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
