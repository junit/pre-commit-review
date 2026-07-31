use super::{
    contract::{ArtifactError, ArtifactPackRecord, ArtifactRole, ProbeId, ProbeResult},
    pack::VerifiedPack,
};
use crate::trusted_runtime::{apply_base_environment, ManagedChild, PrivateRuntime};
use std::{
    io::Read,
    process::{Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const PROBE_DEADLINE: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(5);
#[cfg(not(test))]
const CAPTURE_JOIN_DEADLINE: Duration = Duration::from_secs(1);
#[cfg(test)]
const CAPTURE_JOIN_DEADLINE: Duration = Duration::from_millis(25);
const MAX_STDOUT_BYTES: usize = 64 * 1024;
const MAX_STDERR_BYTES: usize = 64 * 1024;
const MAX_TOTAL_OUTPUT_BYTES: usize = 96 * 1024;

const GITLEAKS_CAPABILITY_ARGUMENTS: &[&str] = &[
    "--ignore-gitleaks-allow",
    "--redact=100",
    "--exit-code=42",
    "--no-banner",
    "--no-color",
    "--log-level=error",
    "--max-decode-depth=5",
    "--report-format=json",
    "--report-path=-",
    "stdin",
];
const GITLEAKS_VERSION_ARGUMENTS: &[&str] = &["version"];
const RUST_ANALYZER_VERSION_ARGUMENTS: &[&str] = &["--version"];
const RUST_ANALYZER_CAPABILITY_ARGUMENTS: &[&str] = &["--help"];
const RUST_ANALYZER_HELP_PREFIX: &[u8] = b"rust-analyzerLSPserverfortheRustprogramminglanguage.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapabilityExpectation {
    CompactExact(&'static [u8]),
    RustAnalyzerHelpV1,
}

impl CapabilityExpectation {
    fn matches(self, stdout: &[u8]) -> bool {
        let compact = compact_ascii_whitespace(stdout);
        match self {
            Self::CompactExact(expected) => compact == expected,
            Self::RustAnalyzerHelpV1 => compact.starts_with(RUST_ANALYZER_HELP_PREFIX),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProbePlan {
    version_arguments: &'static [&'static str],
    capability_arguments: &'static [&'static str],
    capability_expectation: CapabilityExpectation,
}

struct ProbeOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
}

enum CaptureResult {
    Bytes(Vec<u8>),
    Limit,
    Read,
}

pub fn run_probes(
    verified: &VerifiedPack,
    record: &ArtifactPackRecord,
) -> Result<Vec<ProbeResult>, ArtifactError> {
    run_installed_probes(&verified.root().join(&record.executable.path), record)
}

pub fn run_installed_probes(
    executable: &std::path::Path,
    record: &ArtifactPackRecord,
) -> Result<Vec<ProbeResult>, ArtifactError> {
    record.validate()?;
    let plan = probe_plan(
        record.artifact_role,
        record.version_probe,
        record.capability_probe,
    )?;

    let version = run_probe(
        executable,
        &record.executable.sha256,
        plan.version_arguments,
        record,
    )?;
    if !version.status.success()
        || trim_ascii(&version.stdout) != record.expected_version.as_bytes()
    {
        return Err(error(
            "probe-version-output",
            "artifact version probe did not match the selected record",
        ));
    }

    let capability = run_probe(
        executable,
        &record.executable.sha256,
        plan.capability_arguments,
        record,
    )?;
    if !capability.status.success() || !plan.capability_expectation.matches(&capability.stdout) {
        return Err(error(
            "probe-capability-output",
            "artifact capability probe did not return the authorized result",
        ));
    }

    Ok(vec![
        ProbeResult {
            probe_id: record.version_probe,
            success: true,
            observed_version: Some(record.expected_version.clone()),
        },
        ProbeResult {
            probe_id: record.capability_probe,
            success: true,
            observed_version: None,
        },
    ])
}

fn probe_plan(
    role: ArtifactRole,
    version_probe: ProbeId,
    capability_probe: ProbeId,
) -> Result<ProbePlan, ArtifactError> {
    match (role, version_probe, capability_probe) {
        (ArtifactRole::Sanitizer, ProbeId::GitleaksVersionV1, ProbeId::GitleaksStdinJsonV1) => {
            Ok(ProbePlan {
                version_arguments: GITLEAKS_VERSION_ARGUMENTS,
                capability_arguments: GITLEAKS_CAPABILITY_ARGUMENTS,
                capability_expectation: CapabilityExpectation::CompactExact(b"[]"),
            })
        }
        (
            ArtifactRole::RepositoryContextProvider,
            ProbeId::RustAnalyzerVersionV1,
            ProbeId::RustAnalyzerStdioV1,
        ) => Ok(ProbePlan {
            version_arguments: RUST_ANALYZER_VERSION_ARGUMENTS,
            capability_arguments: RUST_ANALYZER_CAPABILITY_ARGUMENTS,
            capability_expectation: CapabilityExpectation::RustAnalyzerHelpV1,
        }),
        _ => Err(error(
            "probe-policy",
            "artifact probes are not implemented for the selected role",
        )),
    }
}

fn run_probe(
    executable: &std::path::Path,
    expected_sha256: &str,
    arguments: &[&str],
    record: &ArtifactPackRecord,
) -> Result<ProbeOutput, ArtifactError> {
    let runtime = PrivateRuntime::create(executable, expected_sha256).map_err(map_runtime_error)?;
    let mut command = Command::new(runtime.executable_path());
    command
        .args(arguments)
        .current_dir(runtime.target())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_base_environment(
        &mut command,
        &runtime,
        runtime.empty_path().as_os_str(),
        "artifact-probe",
        &record.pack_sha256,
    );

    let mut child = ManagedChild::spawn(command).map_err(map_runtime_error)?;
    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| probe_io("probe stdout was unavailable"))?;
    let stderr = child
        .child_mut()
        .stderr
        .take()
        .ok_or_else(|| probe_io("probe stderr was unavailable"))?;
    drop(child.child_mut().stdin.take());

    let (stdout_receiver, stdout_thread) = capture(stdout, MAX_STDOUT_BYTES);
    let (stderr_receiver, stderr_thread) = capture(stderr, MAX_STDERR_BYTES);
    let started = Instant::now();
    let mut status = None;
    let mut stdout = None;
    let mut stderr = None;
    let mut failure = None;

    while failure.is_none() && (status.is_none() || stdout.is_none() || stderr.is_none()) {
        receive_capture(&stdout_receiver, &mut stdout, &mut failure);
        receive_capture(&stderr_receiver, &mut stderr, &mut failure);
        if status.is_none() {
            status = child.try_wait().map_err(map_runtime_error)?;
        }
        if started.elapsed() >= PROBE_DEADLINE {
            failure = Some(error(
                "probe-timeout",
                "artifact probe exceeded its deadline",
            ));
            break;
        }
        if status.is_none() || stdout.is_none() || stderr.is_none() {
            thread::sleep(POLL_INTERVAL);
        }
    }

    if failure.is_some() || status.is_none() {
        child.terminate_and_wait().map_err(map_runtime_error)?;
    }
    let stdout_finished = finish_capture(&stdout_receiver, &mut stdout);
    let stderr_finished = finish_capture(&stderr_receiver, &mut stderr);
    join_capture(stdout_thread, stdout_finished)?;
    join_capture(stderr_thread, stderr_finished)?;
    if let Some(error) = failure {
        return Err(error);
    }

    let stdout = stdout.ok_or_else(|| probe_io("probe stdout was incomplete"))?;
    let stderr = stderr.ok_or_else(|| probe_io("probe stderr was incomplete"))?;
    if stdout.len().saturating_add(stderr.len()) > MAX_TOTAL_OUTPUT_BYTES {
        return Err(error(
            "probe-output-limit",
            "artifact probe exceeded its total output limit",
        ));
    }
    runtime.verify().map_err(map_runtime_error)?;
    Ok(ProbeOutput {
        status: status.ok_or_else(|| probe_io("probe status was unavailable"))?,
        stdout,
    })
}

fn capture<R: Read + Send + 'static>(
    mut reader: R,
    maximum: usize,
) -> (Receiver<CaptureResult>, JoinHandle<()>) {
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = sender.send(CaptureResult::Bytes(bytes));
                    return;
                }
                Ok(read) if bytes.len().saturating_add(read) <= maximum => {
                    bytes.extend_from_slice(&buffer[..read]);
                }
                Ok(_) => {
                    let _ = sender.send(CaptureResult::Limit);
                    return;
                }
                Err(_) => {
                    let _ = sender.send(CaptureResult::Read);
                    return;
                }
            }
        }
    });
    (receiver, handle)
}

fn receive_capture(
    receiver: &Receiver<CaptureResult>,
    destination: &mut Option<Vec<u8>>,
    failure: &mut Option<ArtifactError>,
) {
    if destination.is_some() || failure.is_some() {
        return;
    }
    match receiver.try_recv() {
        Ok(CaptureResult::Bytes(bytes)) => *destination = Some(bytes),
        Ok(CaptureResult::Limit) => {
            *failure = Some(error(
                "probe-output-limit",
                "artifact probe exceeded an output limit",
            ));
        }
        Ok(CaptureResult::Read) | Err(TryRecvError::Disconnected) => {
            *failure = Some(probe_io("artifact probe output could not be read"));
        }
        Err(TryRecvError::Empty) => {}
    }
}

fn finish_capture(receiver: &Receiver<CaptureResult>, destination: &mut Option<Vec<u8>>) -> bool {
    if destination.is_some() {
        return true;
    }
    match receiver.recv_timeout(CAPTURE_JOIN_DEADLINE) {
        Ok(CaptureResult::Bytes(bytes)) => {
            *destination = Some(bytes);
            true
        }
        Ok(CaptureResult::Limit | CaptureResult::Read) | Err(RecvTimeoutError::Disconnected) => {
            true
        }
        Err(RecvTimeoutError::Timeout) => false,
    }
}

fn join_capture(handle: JoinHandle<()>, reader_finished: bool) -> Result<(), ArtifactError> {
    if !reader_finished {
        drop(handle);
        return Ok(());
    }
    handle
        .join()
        .map_err(|_| probe_io("artifact probe output reader failed"))
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

fn compact_ascii_whitespace(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect()
}

fn map_runtime_error(error: crate::trusted_runtime::TrustedRuntimeError) -> ArtifactError {
    ArtifactError::new(error.code, "artifact probe runtime validation failed")
}

fn probe_io(message: &'static str) -> ArtifactError {
    error("probe-io", message)
}

fn error(code: &'static str, message: &'static str) -> ArtifactError {
    ArtifactError::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_analyzer_probe_plan_uses_version_and_lsp_help_flags() {
        let plan = probe_plan(
            ArtifactRole::RepositoryContextProvider,
            ProbeId::RustAnalyzerVersionV1,
            ProbeId::RustAnalyzerStdioV1,
        )
        .unwrap();
        assert_eq!(plan.version_arguments, ["--version"]);
        assert_eq!(plan.capability_arguments, ["--help"]);
        assert!(plan
            .capability_expectation
            .matches(b"rust-analyzer\n  LSP server for the Rust programming language.\n"));
        assert!(!plan.capability_expectation.matches(b"unrelated help"));
    }

    #[cfg(unix)]
    #[test]
    fn capture_shutdown_timeout_does_not_authorize_a_join() {
        use std::os::unix::net::UnixStream;

        let (reader, writer) = UnixStream::pair().unwrap();
        let (receiver, handle) = capture(reader, 64);
        let mut destination = None;
        assert!(!finish_capture(&receiver, &mut destination));
        drop(handle);
        drop(writer);
    }
}
