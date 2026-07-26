use super::contracts::{
    EvidenceScope, EvidenceTrust, ExecutableRecord, ExecutionEvidenceLinks, ExecutionProfileRecord,
    ExecutionRecord, ExecutionStatus, FailureReason, IsolationRecord, OutputFormat, ReportStatus,
    RepositoryConfiguration, SnapshotRecord, StaticAnalysisEvidence, StaticAnalysisExecution,
    StaticAnalysisProfile, ToolIdentity,
};
use super::evidence::{collect_evidence, CollectRequest};
use crate::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
use crate::process_group::{configure_process_group, ProcessGroup};
use crate::review_scope::{
    open_authoritative_scope, revalidate_scope, AuthoritativeScope, ReviewSource, ScopeRequest,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const MAX_PROFILE_BYTES: u64 = 1_000_000;
const CAPTURE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct PreparedProfile {
    pub profile_id: String,
    pub profile: StaticAnalysisProfile,
    pub profile_path: PathBuf,
    pub profile_sha256: String,
    pub executable_path: PathBuf,
    pub executable_sha256: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ExecutionLimits {
    pub timeout: Duration,
    pub max_stream_output_bytes: usize,
    pub max_combined_output_bytes: usize,
}

pub(crate) trait Clock {
    fn now(&self) -> Duration;
}

pub(crate) struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub(crate) fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

#[derive(Debug, Clone)]
pub struct RunRequest {
    pub repository: PathBuf,
    pub source: ReviewSource,
    pub expected_scope: String,
    pub profile_path: PathBuf,
    pub expected_profile_sha256: String,
    pub allow_repository_configuration: bool,
    pub max_findings: usize,
}

#[derive(Debug)]
pub struct RunArtifact {
    pub execution: StaticAnalysisExecution,
    pub evidence: StaticAnalysisEvidence,
}

#[derive(Debug)]
pub struct ProcessOutcome {
    runtime: TempDir,
    stdout_path: PathBuf,
    pub status: ExecutionStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout_bytes: usize,
    pub stdout_sha256: String,
    pub stderr_bytes: usize,
    pub stderr_sha256: String,
    pub failure_reason: Option<FailureReason>,
}

impl ProcessOutcome {
    pub fn read_stdout(&self) -> Result<Vec<u8>, RunError> {
        fs::read(&self.stdout_path)
            .map_err(|error| RunError::new(format!("cannot read analyzer stdout: {error}")))
    }

    pub fn stdout_path(&self) -> &Path {
        &self.stdout_path
    }

    pub fn runtime_path(&self) -> &Path {
        self.runtime.path()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunError {
    message: String,
}

impl RunError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RunError {}

pub fn prepare_profile(
    repository: &Path,
    profile_path: &Path,
    expected_sha256: &str,
    allow_repository_configuration: bool,
) -> Result<PreparedProfile, RunError> {
    if !is_sha256(expected_sha256) {
        return Err(RunError::new(
            "--expect-profile-sha256 must be 64 lowercase hexadecimal characters",
        ));
    }
    if !profile_path.is_absolute() {
        return Err(RunError::new("--profile must be an absolute path"));
    }
    let repository = fs::canonicalize(repository)
        .map_err(|error| RunError::new(format!("cannot resolve repository: {error}")))?;
    let metadata = fs::metadata(profile_path)
        .map_err(|error| RunError::new(format!("cannot read static-analysis profile: {error}")))?;
    if !metadata.is_file() {
        return Err(RunError::new(
            "static-analysis profile must be a regular file",
        ));
    }
    if metadata.len() > MAX_PROFILE_BYTES {
        return Err(RunError::new(format!(
            "static-analysis profile exceeds {MAX_PROFILE_BYTES} bytes"
        )));
    }
    let raw_profile = read_bounded(profile_path, MAX_PROFILE_BYTES, "static-analysis profile")?;
    let profile_sha256 = sha256_bytes(&raw_profile);
    if profile_sha256 != expected_sha256 {
        return Err(RunError::new(
            "profile SHA256 does not match --expect-profile-sha256",
        ));
    }
    let profile: StaticAnalysisProfile = serde_json::from_slice(&raw_profile).map_err(|error| {
        RunError::new(format!(
            "static-analysis profile is not valid UTF-8 JSON: {error}"
        ))
    })?;
    profile
        .validate()
        .map_err(|error| RunError::new(error.to_string()))?;
    match profile.repository_configuration {
        RepositoryConfiguration::ExplicitlyTrusted if !allow_repository_configuration => {
            return Err(RunError::new(
                "profile requires separate --allow-repository-configuration authorization",
            ));
        }
        RepositoryConfiguration::Disabled if allow_repository_configuration => {
            return Err(RunError::new(
                "--allow-repository-configuration is valid only for an explicitly-trusted profile",
            ));
        }
        _ => {}
    }
    let configured_executable = Path::new(&profile.executable.path);
    if !configured_executable.is_absolute() {
        return Err(RunError::new("profile executable.path must be absolute"));
    }
    let executable_path = fs::canonicalize(configured_executable)
        .map_err(|error| RunError::new(format!("cannot resolve profile executable: {error}")))?;
    if path_is_within(&executable_path, &repository) {
        return Err(RunError::new(
            "executable must be outside the reviewed repository",
        ));
    }
    let executable_metadata = fs::metadata(&executable_path)
        .map_err(|error| RunError::new(format!("cannot resolve profile executable: {error}")))?;
    if !executable_metadata.is_file() || !is_executable(&executable_metadata) {
        return Err(RunError::new(
            "profile executable must be an executable regular file",
        ));
    }
    let (executable_sha256, _) = sha256_file(&executable_path, None)?;
    if executable_sha256 != profile.executable.sha256 {
        return Err(RunError::new(
            "executable SHA256 does not match the profile",
        ));
    }
    validate_arguments(&profile.arguments, &repository)?;
    let profile_path = fs::canonicalize(profile_path)
        .map_err(|error| RunError::new(format!("cannot resolve profile path: {error}")))?;
    Ok(PreparedProfile {
        profile_id: profile_sha256[..16].to_string(),
        profile,
        profile_path,
        profile_sha256,
        executable_path,
        executable_sha256,
    })
}

pub fn execute_prepared(
    prepared: &PreparedProfile,
    snapshot: &CandidateSnapshot,
    source: ReviewSource,
    scope_fingerprint: &str,
    limits: ExecutionLimits,
) -> Result<ProcessOutcome, RunError> {
    let clock = SystemClock::new();
    execute_prepared_with_clock(
        prepared,
        snapshot,
        source,
        scope_fingerprint,
        limits,
        &clock,
    )
}

pub(crate) fn execute_prepared_with_clock(
    prepared: &PreparedProfile,
    snapshot: &CandidateSnapshot,
    source: ReviewSource,
    scope_fingerprint: &str,
    limits: ExecutionLimits,
    clock: &dyn Clock,
) -> Result<ProcessOutcome, RunError> {
    if limits.timeout.is_zero() {
        return Err(RunError::new("execution timeout must be greater than zero"));
    }
    if limits.timeout > Duration::from_secs(prepared.profile.limits.timeout_seconds)
        || limits.max_stream_output_bytes > prepared.profile.limits.max_output_bytes
        || limits.max_combined_output_bytes
            > prepared.profile.limits.max_output_bytes.saturating_mul(2)
    {
        return Err(RunError::new(
            "execution limits cannot exceed the authorized profile limits",
        ));
    }
    if limits.max_combined_output_bytes == 0 {
        return Err(RunError::new(
            "combined execution output limit must be greater than zero",
        ));
    }
    let stream_capture_capacity = limits
        .max_stream_output_bytes
        .checked_add(1)
        .ok_or_else(|| RunError::new("execution output limit is too large"))?;
    verify_prepared_integrity(prepared, "before execution")?;
    snapshot
        .verify_unchanged()
        .map_err(|error| RunError::new(error.to_string()))?;

    let runtime = tempfile::tempdir()
        .map_err(|error| RunError::new(format!("cannot create analyzer runtime: {error}")))?;
    set_private_directory(runtime.path())?;
    let runtime_home = runtime.path().join("home");
    let runtime_tmp = runtime.path().join("tmp");
    fs::create_dir(&runtime_home)
        .and_then(|_| fs::create_dir(&runtime_tmp))
        .map_err(|error| RunError::new(format!("cannot create analyzer runtime: {error}")))?;
    set_private_directory(&runtime_home)?;
    set_private_directory(&runtime_tmp)?;
    let stdout_path = runtime.path().join("analyzer.stdout");
    let stderr_path = runtime.path().join("analyzer.stderr");
    let runtime_executable = materialize_pinned_executable(prepared, runtime.path())?;

    let mut command = Command::new(runtime_executable.path());
    command
        .args(&prepared.profile.arguments)
        .current_dir(snapshot.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear();
    apply_child_environment(
        &mut command,
        &runtime_home,
        &runtime_tmp,
        source,
        scope_fingerprint,
    );
    configure_process_group(&mut command).map_err(|error| {
        RunError::new(format!("cannot configure analyzer process group: {error}"))
    })?;
    let mut child = command
        .spawn()
        .map_err(|error| RunError::new(format!("cannot start trusted analyzer: {error}")))?;
    let start = clock.now();
    let process_group = match ProcessGroup::attach(&mut child) {
        Ok(process_group) => process_group,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::new(format!(
                "cannot attach analyzer process group: {error}"
            )));
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            process_group.terminate(&mut child);
            let _ = child.wait();
            return Err(RunError::new("cannot capture trusted analyzer output"));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            process_group.terminate(&mut child);
            let _ = child.wait();
            return Err(RunError::new("cannot capture trusted analyzer output"));
        }
    };
    let overflow = Arc::new(AtomicBool::new(false));
    let combined_remaining = Arc::new(Mutex::new(limits.max_combined_output_bytes));
    let stdout_capture = spawn_capture(
        stdout,
        stdout_path.clone(),
        stream_capture_capacity,
        Arc::clone(&overflow),
        Arc::clone(&combined_remaining),
    );
    let stderr_capture = spawn_capture(
        stderr,
        stderr_path.clone(),
        stream_capture_capacity,
        Arc::clone(&overflow),
        Arc::clone(&combined_remaining),
    );

    let mut forced_status = None;
    let exit_status = loop {
        if overflow.load(Ordering::Acquire) {
            forced_status = Some(ExecutionStatus::OutputLimit);
            process_group.terminate(&mut child);
            break child.wait().map_err(|error| {
                RunError::new(format!("cannot wait for trusted analyzer: {error}"))
            })?;
        }
        if clock.now().saturating_sub(start) >= limits.timeout {
            forced_status = Some(ExecutionStatus::Timeout);
            process_group.terminate(&mut child);
            break child.wait().map_err(|error| {
                RunError::new(format!("cannot wait for trusted analyzer: {error}"))
            })?;
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| RunError::new(format!("cannot inspect trusted analyzer: {error}")))?
        {
            process_group.terminate(&mut child);
            break status;
        }
        thread::sleep(Duration::from_millis(20));
    };

    finish_capture(stdout_capture, "stdout")?;
    finish_capture(stderr_capture, "stderr")?;
    if overflow.load(Ordering::Acquire) && forced_status.is_none() {
        forced_status = Some(ExecutionStatus::OutputLimit);
    }
    snapshot
        .verify_unchanged()
        .map_err(|error| RunError::new(error.to_string()))?;
    verify_prepared_integrity(prepared, "during execution")?;
    runtime_executable.verify(&prepared.executable_sha256)?;

    let duration_ms =
        u64::try_from(clock.now().saturating_sub(start).as_millis()).unwrap_or(u64::MAX);
    let (stdout_sha256, stdout_bytes) = sha256_file(&stdout_path, None)?;
    let (stderr_sha256, stderr_bytes) = sha256_file(&stderr_path, None)?;
    let observed_exit_code = process_exit_code(&exit_status);
    let (status, exit_code, failure_reason) = match forced_status {
        Some(ExecutionStatus::Timeout) => {
            (ExecutionStatus::Timeout, None, Some(FailureReason::Timeout))
        }
        Some(ExecutionStatus::OutputLimit) => (
            ExecutionStatus::OutputLimit,
            None,
            Some(FailureReason::OutputLimit),
        ),
        _ if observed_exit_code
            .is_some_and(|code| prepared.profile.success_exit_codes.contains(&code)) =>
        {
            (ExecutionStatus::Completed, observed_exit_code, None)
        }
        _ => (
            ExecutionStatus::Failed,
            observed_exit_code,
            Some(FailureReason::NonSuccessExit),
        ),
    };
    Ok(ProcessOutcome {
        runtime,
        stdout_path,
        status,
        exit_code,
        duration_ms,
        stdout_bytes,
        stdout_sha256,
        stderr_bytes,
        stderr_sha256,
        failure_reason,
    })
}

struct MaterializedExecutable {
    path: PathBuf,
}

impl MaterializedExecutable {
    fn path(&self) -> &Path {
        &self.path
    }

    fn verify(&self, expected_sha256: &str) -> Result<(), RunError> {
        let (observed_sha256, _) = sha256_file(&self.path, None)?;
        if observed_sha256 != expected_sha256 {
            return Err(RunError::new(
                "trusted analyzer executable changed during execution",
            ));
        }
        Ok(())
    }
}

impl Drop for MaterializedExecutable {
    fn drop(&mut self) {
        #[cfg(not(unix))]
        if let Ok(mut permissions) = fs::metadata(&self.path).map(|metadata| metadata.permissions())
        {
            permissions.set_readonly(false);
            let _ = fs::set_permissions(&self.path, permissions);
        }
    }
}

fn materialize_pinned_executable(
    prepared: &PreparedProfile,
    runtime: &Path,
) -> Result<MaterializedExecutable, RunError> {
    let mut file_name = OsString::from("trusted-analyzer");
    if let Some(extension) = prepared.executable_path.extension() {
        file_name.push(".");
        file_name.push(extension);
    }
    let path = runtime.join(file_name);
    let mut input = File::open(&prepared.executable_path).map_err(|error| {
        RunError::new(format!("cannot open trusted analyzer executable: {error}"))
    })?;
    let metadata = input.metadata().map_err(|error| {
        RunError::new(format!(
            "cannot inspect trusted analyzer executable: {error}"
        ))
    })?;
    if !metadata.is_file() || !is_executable(&metadata) {
        return Err(RunError::new(
            "profile executable must remain an executable regular file",
        ));
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            RunError::new(format!(
                "cannot materialize trusted analyzer executable: {error}"
            ))
        })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = input.read(&mut buffer).map_err(|error| {
            RunError::new(format!("cannot read trusted analyzer executable: {error}"))
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        output.write_all(&buffer[..read]).map_err(|error| {
            RunError::new(format!(
                "cannot materialize trusted analyzer executable: {error}"
            ))
        })?;
    }
    output.flush().map_err(|error| {
        RunError::new(format!(
            "cannot materialize trusted analyzer executable: {error}"
        ))
    })?;
    let observed_sha256 = format!("{:x}", digest.finalize());
    if observed_sha256 != prepared.executable_sha256 {
        return Err(RunError::new(
            "trusted analyzer executable changed before execution",
        ));
    }
    set_materialized_executable_permissions(&path)?;
    Ok(MaterializedExecutable { path })
}

#[cfg(unix)]
fn set_materialized_executable_permissions(path: &Path) -> Result<(), RunError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o500))
        .map_err(|error| RunError::new(format!("cannot secure trusted analyzer copy: {error}")))
}

#[cfg(not(unix))]
fn set_materialized_executable_permissions(path: &Path) -> Result<(), RunError> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| RunError::new(format!("cannot secure trusted analyzer copy: {error}")))?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)
        .map_err(|error| RunError::new(format!("cannot secure trusted analyzer copy: {error}")))
}

pub fn run_analysis(request: RunRequest) -> Result<RunArtifact, RunError> {
    if !is_scope_fingerprint(&request.expected_scope) {
        return Err(RunError::new("--expect-scope is missing or invalid"));
    }
    if !(1..=5_000).contains(&request.max_findings) {
        return Err(RunError::new("--max-findings must be between 1 and 5000"));
    }
    let scope = open_authoritative_scope(ScopeRequest {
        repository: request.repository.clone(),
        source: Some(request.source),
        expected_fingerprint: Some(request.expected_scope.clone()),
    })
    .map_err(|error| RunError::new(error.to_string()))?;
    let repository = scope.repository.clone();
    let repository_state_before = repository_state_digest(&repository)?;
    let prepared = prepare_profile(
        &repository,
        &request.profile_path,
        &request.expected_profile_sha256,
        request.allow_repository_configuration,
    )?;
    let snapshot = CandidateSnapshot::materialize(
        &repository,
        request.source,
        SnapshotLimits {
            max_files: prepared.profile.limits.max_snapshot_files,
            max_bytes: prepared.profile.limits.max_snapshot_bytes,
        },
    )
    .map_err(|error| RunError::new(error.to_string()))?;
    let process = execute_prepared(
        &prepared,
        &snapshot,
        request.source,
        &request.expected_scope,
        ExecutionLimits {
            timeout: Duration::from_secs(prepared.profile.limits.timeout_seconds),
            max_stream_output_bytes: prepared.profile.limits.max_output_bytes,
            max_combined_output_bytes: prepared.profile.limits.max_output_bytes.saturating_mul(2),
        },
    )?;

    let artifact = build_run_artifact(
        &repository,
        request.source,
        &request.expected_scope,
        &prepared,
        &snapshot,
        &process,
        request.max_findings,
    )?;

    snapshot
        .verify_unchanged()
        .map_err(|error| RunError::new(error.to_string()))?;
    verify_prepared_integrity(&prepared, "during controlled execution")?;
    if repository_state_digest(&repository)? != repository_state_before {
        return Err(RunError::new(
            "reviewed repository state changed during controlled execution",
        ));
    }
    revalidate_scope(&scope).map_err(|error| {
        RunError::new(format!(
            "review scope changed during controlled execution: {error}"
        ))
    })?;
    let expected_evidence_scope = evidence_scope(&scope);
    if artifact.evidence.scope != expected_evidence_scope {
        return Err(RunError::new(
            "controlled evidence scope does not match the opening control plane",
        ));
    }
    Ok(artifact)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_run_artifact(
    repository: &Path,
    source: ReviewSource,
    expected_scope: &str,
    prepared: &PreparedProfile,
    snapshot: &CandidateSnapshot,
    process: &ProcessOutcome,
    max_findings: usize,
) -> Result<RunArtifact, RunError> {
    let mut final_status = process.status;
    let mut execution_id = compact_execution_id(expected_scope, prepared, process, final_status);
    let mut evidence = if final_status == ExecutionStatus::Completed {
        collect_completed_evidence(
            repository,
            source,
            expected_scope,
            prepared,
            process,
            &execution_id,
            max_findings,
        )
        .ok()
        .filter(|evidence| evidence_matches_profile(evidence, &prepared.profile))
    } else {
        None
    };
    if evidence.is_none() {
        if final_status == ExecutionStatus::Completed {
            final_status = ExecutionStatus::InvalidOutput;
            execution_id = compact_execution_id(expected_scope, prepared, process, final_status);
        }
        evidence = Some(collect_failure_evidence(
            repository,
            source,
            expected_scope,
            prepared,
            process,
            &execution_id,
            final_status,
            max_findings,
        )?);
    }
    let evidence = evidence.expect("assigned above");

    let mut report_ids = evidence
        .reports
        .iter()
        .map(|report| report.report_id.clone())
        .collect::<Vec<_>>();
    report_ids.sort();
    let failure_reason = if final_status == ExecutionStatus::InvalidOutput {
        Some(FailureReason::InvalidOutput)
    } else {
        process.failure_reason
    };
    let execution = StaticAnalysisExecution {
        schema_version: 1,
        kind: "static_analysis_execution".to_string(),
        authoritative: true,
        execution_id,
        scope: evidence.scope.clone(),
        profile: ExecutionProfileRecord {
            profile_id: prepared.profile_id.clone(),
            sha256: prepared.profile_sha256.clone(),
            name: prepared.profile.name.clone(),
            output_format: prepared.profile.output_format,
            success_exit_codes: prepared.profile.success_exit_codes.clone(),
            limits: prepared.profile.limits.clone(),
            repository_configuration: prepared.profile.repository_configuration,
            network_access: prepared.profile.network_access,
        },
        tool: prepared.profile.tool.clone(),
        executable: ExecutableRecord {
            name: prepared
                .executable_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "analyzer".to_string()),
            sha256: prepared.executable_sha256.clone(),
            path_policy: "absolute-explicit-outside-repository".to_string(),
        },
        snapshot: SnapshotRecord {
            kind: "temporary-tracked-files".to_string(),
            sha256: snapshot.sha256.clone(),
            files: snapshot.files,
            bytes: snapshot.bytes,
        },
        isolation: IsolationRecord {
            shell: false,
            vcs_metadata: false,
            environment: "allowlist".to_string(),
            source_tree: "read-only-temporary-snapshot".to_string(),
            original_repository_path: "not-exposed".to_string(),
            network: "best-effort-offline-profile-required".to_string(),
        },
        execution: ExecutionRecord {
            status: final_status,
            exit_code: process.exit_code,
            duration_ms: process.duration_ms,
            stdout_bytes: process.stdout_bytes,
            stdout_sha256: process.stdout_sha256.clone(),
            stderr_bytes: process.stderr_bytes,
            stderr_sha256: process.stderr_sha256.clone(),
            result_accepted: final_status == ExecutionStatus::Completed,
            failure_reason,
        },
        evidence: ExecutionEvidenceLinks { report_ids },
    };
    Ok(RunArtifact {
        execution,
        evidence,
    })
}

fn collect_completed_evidence(
    repository: &Path,
    source: ReviewSource,
    expected_scope: &str,
    prepared: &PreparedProfile,
    process: &ProcessOutcome,
    execution_id: &str,
    max_findings: usize,
) -> Result<StaticAnalysisEvidence, RunError> {
    collect_evidence(CollectRequest {
        repository: repository.to_path_buf(),
        source: Some(source),
        expected_scope: expected_scope.to_string(),
        result_paths: vec![process.stdout_path().to_path_buf()],
        asserted_result_scope: (prepared.profile.output_format == OutputFormat::Sarif)
            .then(|| expected_scope.to_string()),
        max_findings,
        trust: EvidenceTrust::ControlledExecution,
        execution_id: Some(execution_id.to_string()),
    })
    .map_err(|error| RunError::new(error.to_string()))
}

#[allow(clippy::too_many_arguments)]
fn collect_failure_evidence(
    repository: &Path,
    source: ReviewSource,
    expected_scope: &str,
    prepared: &PreparedProfile,
    process: &ProcessOutcome,
    execution_id: &str,
    final_status: ExecutionStatus,
    max_findings: usize,
) -> Result<StaticAnalysisEvidence, RunError> {
    #[derive(Serialize)]
    struct FailureInput<'a> {
        schema_version: u8,
        kind: &'static str,
        scope_fingerprint: &'a str,
        tool: &'a ToolIdentity,
        status: ReportStatus,
        findings: &'static [()],
    }

    let result_path = process.runtime_path().join("failed-result.json");
    let report_status = if final_status == ExecutionStatus::Timeout {
        ReportStatus::Timeout
    } else {
        ReportStatus::Failed
    };
    let payload = FailureInput {
        schema_version: 1,
        kind: "static_analysis_input",
        scope_fingerprint: expected_scope,
        tool: &prepared.profile.tool,
        status: report_status,
        findings: &[],
    };
    fs::write(
        &result_path,
        serde_json::to_vec(&payload).map_err(|error| {
            RunError::new(format!("cannot serialize failure evidence: {error}"))
        })?,
    )
    .map_err(|error| RunError::new(format!("cannot write failure evidence: {error}")))?;
    collect_evidence(CollectRequest {
        repository: repository.to_path_buf(),
        source: Some(source),
        expected_scope: expected_scope.to_string(),
        result_paths: vec![result_path],
        asserted_result_scope: None,
        max_findings,
        trust: EvidenceTrust::ControlledExecution,
        execution_id: Some(execution_id.to_string()),
    })
    .map_err(|error| RunError::new(format!("cannot create bounded failure evidence: {error}")))
}

fn evidence_matches_profile(
    evidence: &StaticAnalysisEvidence,
    profile: &StaticAnalysisProfile,
) -> bool {
    !evidence.reports.is_empty()
        && evidence.reports.iter().all(|report| {
            report.format == profile.output_format
                && report.tool == profile.tool
                && report.status == ReportStatus::Completed
        })
}

fn compact_execution_id(
    expected_scope: &str,
    prepared: &PreparedProfile,
    process: &ProcessOutcome,
    status: ExecutionStatus,
) -> String {
    let mut digest = Sha256::new();
    for value in [
        expected_scope,
        &prepared.profile_sha256,
        &prepared.executable_sha256,
        &process.stdout_sha256,
        execution_status_name(status),
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())[..16].to_string()
}

fn execution_status_name(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Completed => "completed",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Timeout => "timeout",
        ExecutionStatus::OutputLimit => "output-limit",
        ExecutionStatus::InvalidOutput => "invalid-output",
    }
}

fn evidence_scope(scope: &AuthoritativeScope) -> EvidenceScope {
    EvidenceScope {
        source: scope.source,
        head: scope.head.clone(),
        fingerprint: scope.fingerprint.clone(),
    }
}

fn is_scope_fingerprint(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::static_analysis::contracts::{
        ExecutableAuthorization, NetworkAccess, OutputFormat, ProfileLimits,
        RepositoryConfiguration, StaticAnalysisProfile, ToolIdentity,
    };
    use std::collections::VecDeque;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;

    struct SequenceClock {
        values: Mutex<VecDeque<Duration>>,
        last: Mutex<Duration>,
    }

    impl SequenceClock {
        fn new(values: impl IntoIterator<Item = Duration>) -> Self {
            Self {
                values: Mutex::new(values.into_iter().collect()),
                last: Mutex::new(Duration::ZERO),
            }
        }
    }

    impl Clock for SequenceClock {
        fn now(&self) -> Duration {
            let next = self.values.lock().unwrap().pop_front();
            if let Some(value) = next {
                *self.last.lock().unwrap() = value;
                value
            } else {
                *self.last.lock().unwrap()
            }
        }
    }

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

    #[test]
    fn budgets_use_deterministic_clock_for_effective_timeout() {
        let repository = tempfile::tempdir().unwrap();
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

        let fixtures = tempfile::tempdir().unwrap();
        let executable_path = fixtures.path().join("slow.sh");
        fs::write(&executable_path, "#!/bin/sh\nsleep 10\n").unwrap();
        fs::set_permissions(&executable_path, fs::Permissions::from_mode(0o755)).unwrap();
        let (executable_sha256, _) = sha256_file(&executable_path, None).unwrap();
        let profile = StaticAnalysisProfile {
            schema_version: 1,
            kind: "static_analysis_profile".to_string(),
            name: "deterministic clock profile".to_string(),
            tool: ToolIdentity {
                name: "slow".to_string(),
                version: Some("1.0".to_string()),
            },
            executable: ExecutableAuthorization {
                path: executable_path.to_string_lossy().into_owned(),
                sha256: executable_sha256,
            },
            arguments: Vec::new(),
            output_format: OutputFormat::NormalizedJson,
            success_exit_codes: vec![0],
            limits: ProfileLimits {
                timeout_seconds: 5,
                max_output_bytes: 1024,
                max_snapshot_bytes: 10_485_760,
                max_snapshot_files: 1000,
            },
            repository_configuration: RepositoryConfiguration::Disabled,
            network_access: NetworkAccess::OfflineRequired,
        };
        let profile_path = fixtures.path().join("profile.json");
        fs::write(&profile_path, serde_json::to_vec(&profile).unwrap()).unwrap();
        let (profile_sha256, _) = sha256_file(&profile_path, None).unwrap();
        let prepared =
            prepare_profile(repository.path(), &profile_path, &profile_sha256, false).unwrap();
        let snapshot = CandidateSnapshot::materialize(
            repository.path(),
            ReviewSource::Staged,
            SnapshotLimits {
                max_files: 1000,
                max_bytes: 10_485_760,
            },
        )
        .unwrap();
        let clock = SequenceClock::new([
            Duration::ZERO,
            Duration::from_secs(2),
            Duration::from_secs(2),
        ]);

        let outcome = execute_prepared_with_clock(
            &prepared,
            &snapshot,
            ReviewSource::Staged,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            ExecutionLimits {
                timeout: Duration::from_secs(1),
                max_stream_output_bytes: 1024,
                max_combined_output_bytes: 2048,
            },
            &clock,
        )
        .unwrap();

        assert_eq!(outcome.status, ExecutionStatus::Timeout);
        assert_eq!(outcome.duration_ms, 2_000);
    }

    #[test]
    fn capture_shutdown_timeout_does_not_block_on_join() {
        let (reader, writer) = UnixStream::pair().unwrap();
        let output = tempfile::NamedTempFile::new().unwrap();
        let capture = spawn_capture(
            reader,
            output.path().to_path_buf(),
            1024,
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(1024)),
        );
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(finish_capture(capture, "stdout"));
        });

        let result = receiver
            .recv_timeout(CAPTURE_SHUTDOWN_TIMEOUT + Duration::from_secs(1))
            .expect("capture shutdown must return after its timeout")
            .unwrap_err();

        assert!(result.to_string().contains("capture did not terminate"));
        drop(writer);
    }
}

pub(crate) fn repository_state_digest(repository: &Path) -> Result<String, RunError> {
    let commands: [&[&str]; 3] = [
        &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
        &["diff", "--no-ext-diff", "--no-textconv", "--binary"],
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--binary",
        ],
    ];
    let mut digest = Sha256::new();
    for arguments in commands {
        update_digest_from_git(repository, arguments, &mut digest)?;
        digest.update([0]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn update_digest_from_git(
    repository: &Path,
    arguments: &[&str],
    digest: &mut Sha256,
) -> Result<(), RunError> {
    let mut stderr = tempfile::tempfile()
        .map_err(|error| RunError::new(format!("cannot capture Git state: {error}")))?;
    let stderr_child = stderr
        .try_clone()
        .map_err(|error| RunError::new(format!("cannot capture Git state: {error}")))?;
    let mut command = Command::new("git");
    crate::git_policy::configure_read_only(&mut command);
    command
        .args(arguments)
        .current_dir(repository)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr_child));
    let mut child = command
        .spawn()
        .map_err(|error| RunError::new(format!("cannot inspect Git repository state: {error}")))?;
    let mut stdout = child.stdout.take().ok_or_else(|| {
        let _ = child.kill();
        RunError::new("cannot hash Git repository state")
    })?;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = match stdout.read(&mut buffer) {
            Ok(read) => read,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::new(format!(
                    "cannot hash Git repository state: {error}"
                )));
            }
        };
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let status = child
        .wait()
        .map_err(|error| RunError::new(format!("cannot wait for Git state: {error}")))?;
    if !status.success() {
        stderr.seek(SeekFrom::Start(0)).ok();
        let mut detail = Vec::new();
        Read::by_ref(&mut stderr)
            .take(500)
            .read_to_end(&mut detail)
            .ok();
        return Err(RunError::new(format!(
            "Git repository-state command failed: {}",
            bounded_process_detail(&detail)
        )));
    }
    Ok(())
}

fn bounded_process_detail(value: &[u8]) -> String {
    let detail = String::from_utf8_lossy(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let detail = detail.chars().take(500).collect::<String>();
    if detail.is_empty() {
        "unknown Git error".to_string()
    } else {
        detail
    }
}

pub(crate) fn verify_prepared_integrity(
    prepared: &PreparedProfile,
    phase: &str,
) -> Result<(), RunError> {
    let (profile_sha256, _) = sha256_file(&prepared.profile_path, None)?;
    if profile_sha256 != prepared.profile_sha256 {
        return Err(RunError::new(format!(
            "static-analysis profile changed {phase}"
        )));
    }
    let (executable_sha256, _) = sha256_file(&prepared.executable_path, None)?;
    if executable_sha256 != prepared.executable_sha256 {
        return Err(RunError::new(format!(
            "trusted analyzer executable changed {phase}"
        )));
    }
    Ok(())
}

fn read_bounded(path: &Path, limit: u64, label: &str) -> Result<Vec<u8>, RunError> {
    let mut input =
        File::open(path).map_err(|error| RunError::new(format!("cannot read {label}: {error}")))?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut input)
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| RunError::new(format!("cannot read {label}: {error}")))?;
    if bytes.len() as u64 > limit {
        return Err(RunError::new(format!("{label} exceeds {limit} bytes")));
    }
    Ok(bytes)
}

fn sha256_bytes(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

pub(crate) fn sha256_file(path: &Path, limit: Option<u64>) -> Result<(String, usize), RunError> {
    let mut input = File::open(path)
        .map_err(|error| RunError::new(format!("cannot hash {}: {error}", display_name(path))))?;
    let mut digest = Sha256::new();
    let mut total = 0_usize;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = input.read(&mut buffer).map_err(|error| {
            RunError::new(format!("cannot hash {}: {error}", display_name(path)))
        })?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read)
            .ok_or_else(|| RunError::new("file byte count overflow"))?;
        if let Some(limit) = limit.filter(|limit| total as u64 > *limit) {
            return Err(RunError::new(format!(
                "{} exceeds the {limit}-byte limit",
                display_name(path)
            )));
        }
        digest.update(&buffer[..read]);
    }
    Ok((format!("{:x}", digest.finalize()), total))
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn path_is_within(path: &Path, parent: &Path) -> bool {
    path.strip_prefix(parent).is_ok()
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    true
}

fn validate_arguments(arguments: &[String], repository: &Path) -> Result<(), RunError> {
    let repository_text = repository.to_string_lossy();
    for argument in arguments {
        if argument.contains(repository_text.as_ref()) {
            return Err(RunError::new(
                "profile arguments must not reference paths inside the reviewed repository",
            ));
        }
        let candidate = Path::new(argument);
        if candidate.is_absolute() {
            let normalized =
                fs::canonicalize(candidate).or_else(|_| normalize_absolute(candidate))?;
            if path_is_within(&normalized, repository) {
                return Err(RunError::new(
                    "profile arguments must not reference paths inside the reviewed repository",
                ));
            }
        }
    }
    Ok(())
}

fn normalize_absolute(path: &Path) -> Result<PathBuf, RunError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(value) => normalized.push(value.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    if !normalized.is_absolute() {
        return Err(RunError::new("cannot validate profile argument path"));
    }
    Ok(normalized)
}

fn apply_child_environment(
    command: &mut Command,
    runtime_home: &Path,
    runtime_tmp: &Path,
    source: ReviewSource,
    scope_fingerprint: &str,
) {
    #[cfg(unix)]
    let default_path = "/bin:/usr/bin";
    #[cfg(windows)]
    let default_path = r"C:\Windows\System32;C:\Windows";
    command
        .env("PATH", default_path)
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("HOME", runtime_home)
        .env("TMPDIR", runtime_tmp)
        .env("TMP", runtime_tmp)
        .env("TEMP", runtime_tmp)
        .env("NO_COLOR", "1")
        .env("PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT", scope_fingerprint)
        .env("PRE_COMMIT_REVIEW_SOURCE", source.as_str())
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .env("NO_PROXY", "");
    #[cfg(windows)]
    for name in ["SystemRoot", "WINDIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), RunError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| RunError::new(format!("cannot secure analyzer runtime: {error}")))
}

#[cfg(windows)]
fn set_private_directory(path: &Path) -> Result<(), RunError> {
    crate::windows_acl::restrict_tree_private(path)
        .map_err(|error| RunError::new(format!("cannot secure analyzer runtime: {error}")))
}

struct CaptureHandle {
    receiver: mpsc::Receiver<Result<(), String>>,
    thread: thread::JoinHandle<()>,
}

fn spawn_capture(
    mut stream: impl Read + Send + 'static,
    path: PathBuf,
    capacity: usize,
    overflow: Arc<AtomicBool>,
    combined_remaining: Arc<Mutex<usize>>,
) -> CaptureHandle {
    let (sender, receiver) = mpsc::channel();
    let thread = thread::spawn(move || {
        let result = capture_stream(&mut stream, &path, capacity, &overflow, &combined_remaining)
            .map_err(|error| error.to_string());
        if result.is_err() {
            overflow.store(true, Ordering::Release);
        }
        let _ = sender.send(result);
    });
    CaptureHandle { receiver, thread }
}

fn capture_stream(
    stream: &mut impl Read,
    path: &Path,
    capacity: usize,
    overflow: &AtomicBool,
    combined_remaining: &Mutex<usize>,
) -> Result<(), RunError> {
    let mut output = File::create(path)
        .map_err(|error| RunError::new(format!("cannot create analyzer capture: {error}")))?;
    let mut written = 0_usize;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = stream.read(&mut buffer).map_err(|error| {
            RunError::new(format!("cannot capture trusted analyzer output: {error}"))
        })?;
        if read == 0 {
            break;
        }
        let stream_remaining = capacity.saturating_sub(written);
        let stream_allowed = read.min(stream_remaining);
        let saved = {
            let mut remaining = combined_remaining
                .lock()
                .map_err(|_| RunError::new("combined output budget lock is poisoned"))?;
            let saved = stream_allowed.min(*remaining);
            *remaining -= saved;
            saved
        };
        if saved > 0 {
            output.write_all(&buffer[..saved]).map_err(|error| {
                RunError::new(format!("cannot capture trusted analyzer output: {error}"))
            })?;
            written += saved;
        }
        if read > saved || written == capacity {
            overflow.store(true, Ordering::Release);
        }
    }
    Ok(())
}

fn finish_capture(capture: CaptureHandle, stream_name: &str) -> Result<(), RunError> {
    let result = match capture.receiver.recv_timeout(CAPTURE_SHUTDOWN_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            return Err(RunError::new(format!(
                "analyzer {stream_name} capture did not terminate"
            )))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let _ = capture.thread.join();
            return Err(RunError::new(format!(
                "analyzer {stream_name} capture channel disconnected"
            )));
        }
    };
    let joined = capture
        .thread
        .join()
        .map_err(|_| RunError::new(format!("analyzer {stream_name} capture panicked")));
    joined?;
    result.map_err(RunError::new)
}

#[cfg(unix)]
fn process_exit_code(status: &ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|signal| -signal))
}

#[cfg(not(unix))]
fn process_exit_code(status: &ExitStatus) -> Option<i32> {
    status.code()
}
