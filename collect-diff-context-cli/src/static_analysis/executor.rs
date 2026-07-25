use super::contracts::{
    ExecutionStatus, FailureReason, RepositoryConfiguration, StaticAnalysisProfile,
};
use super::snapshot::CandidateSnapshot;
use crate::review_scope::ReviewSource;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
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
    pub max_output_bytes: usize,
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
    if limits.timeout.is_zero() {
        return Err(RunError::new("execution timeout must be greater than zero"));
    }
    if limits.timeout > Duration::from_secs(prepared.profile.limits.timeout_seconds)
        || limits.max_output_bytes > prepared.profile.limits.max_output_bytes
    {
        return Err(RunError::new(
            "execution limits cannot exceed the authorized profile limits",
        ));
    }
    let capture_capacity = limits
        .max_output_bytes
        .checked_add(1)
        .ok_or_else(|| RunError::new("execution output limit is too large"))?;
    verify_prepared_integrity(prepared, "before execution")?;
    snapshot
        .verify_unchanged()
        .map_err(|error| RunError::new(error.to_string()))?;

    let runtime = tempfile::tempdir()
        .map_err(|error| RunError::new(format!("cannot create analyzer runtime: {error}")))?;
    let runtime_home = runtime.path().join("home");
    let runtime_tmp = runtime.path().join("tmp");
    fs::create_dir(&runtime_home)
        .and_then(|_| fs::create_dir(&runtime_tmp))
        .map_err(|error| RunError::new(format!("cannot create analyzer runtime: {error}")))?;
    set_private_directory(&runtime_home)?;
    set_private_directory(&runtime_tmp)?;
    let stdout_path = runtime.path().join("analyzer.stdout");
    let stderr_path = runtime.path().join("analyzer.stderr");

    let mut command = Command::new(&prepared.executable_path);
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
    configure_process_group(&mut command)?;
    let start = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|error| RunError::new(format!("cannot start trusted analyzer: {error}")))?;
    let process_group = match ProcessGroup::attach(&mut child) {
        Ok(process_group) => process_group,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
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
    let stdout_capture = spawn_capture(
        stdout,
        stdout_path.clone(),
        capture_capacity,
        Arc::clone(&overflow),
    );
    let stderr_capture = spawn_capture(
        stderr,
        stderr_path.clone(),
        capture_capacity,
        Arc::clone(&overflow),
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
        if start.elapsed() >= limits.timeout {
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

    let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
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

fn verify_prepared_integrity(prepared: &PreparedProfile, phase: &str) -> Result<(), RunError> {
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
                "profile arguments must not expose the reviewed repository path",
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

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), RunError> {
    Ok(())
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
) -> CaptureHandle {
    let (sender, receiver) = mpsc::channel();
    let thread = thread::spawn(move || {
        let result = capture_stream(&mut stream, &path, capacity, &overflow)
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
        let remaining = capacity.saturating_sub(written);
        let saved = read.min(remaining);
        if saved > 0 {
            output.write_all(&buffer[..saved]).map_err(|error| {
                RunError::new(format!("cannot capture trusted analyzer output: {error}"))
            })?;
            written += saved;
        }
        if read > remaining || written == capacity {
            overflow.store(true, Ordering::Release);
        }
    }
    Ok(())
}

fn finish_capture(capture: CaptureHandle, stream_name: &str) -> Result<(), RunError> {
    let result = capture.receiver.recv_timeout(CAPTURE_SHUTDOWN_TIMEOUT);
    let joined = capture
        .thread
        .join()
        .map_err(|_| RunError::new(format!("analyzer {stream_name} capture panicked")));
    joined?;
    result
        .map_err(|_| RunError::new(format!("analyzer {stream_name} capture did not terminate")))?
        .map_err(RunError::new)
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) -> Result<(), RunError> {
    use std::os::unix::process::CommandExt;
    // SAFETY: this closure calls only async-signal-safe setpgid before exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    Ok(())
}

#[cfg(windows)]
fn configure_process_group(command: &mut Command) -> Result<(), RunError> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    Ok(())
}

#[cfg(unix)]
struct ProcessGroup {
    process_group_id: i32,
}

#[cfg(unix)]
impl ProcessGroup {
    fn attach(child: &mut Child) -> Result<Self, RunError> {
        let process_group_id = i32::try_from(child.id())
            .map_err(|_| RunError::new("analyzer process id exceeds i32"))?;
        Ok(Self { process_group_id })
    }

    fn terminate(&self, child: &mut Child) {
        // SAFETY: the process group id was created for this child immediately before exec.
        unsafe {
            libc::killpg(self.process_group_id, libc::SIGKILL);
        }
        let _ = child.kill();
    }
}

#[cfg(windows)]
struct ProcessGroup {
    job: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl ProcessGroup {
    fn attach(child: &mut Child) -> Result<Self, RunError> {
        use std::ffi::c_void;
        use std::mem::size_of;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        // SAFETY: Windows handles are checked for null and owned until Drop.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(RunError::new("cannot create analyzer Job Object"));
            }
            let mut information: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &mut information as *mut _ as *mut c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
                || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0
            {
                CloseHandle(job);
                let _ = child.kill();
                return Err(RunError::new("cannot assign analyzer to Job Object"));
            }
            Ok(Self { job })
        }
    }

    fn terminate(&self, child: &mut Child) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        // SAFETY: self.job is a live Job Object handle owned by this guard.
        unsafe {
            TerminateJobObject(self.job, 1);
        }
        let _ = child.kill();
    }
}

#[cfg(windows)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        // SAFETY: self.job is owned by this guard and closed exactly once.
        unsafe {
            CloseHandle(self.job);
        }
    }
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
