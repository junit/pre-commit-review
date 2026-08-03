use std::io::{Read, Seek, SeekFrom, Write};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::process_group::{configure_process_group, ProcessGroup};

pub(crate) const MAX_GIT_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

pub(crate) fn configure_read_only(command: &mut Command) {
    command
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "core.fsmonitor")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "core.untrackedCache")
        .env("GIT_CONFIG_VALUE_1", "false")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    #[cfg(not(windows))]
    command.env("GIT_CONFIG_GLOBAL", "/dev/null");
    #[cfg(windows)]
    command.env("GIT_CONFIG_GLOBAL", "NUL");
}

#[derive(Debug)]
pub(crate) enum GitOutputError {
    Io(std::io::Error),
    DeadlineExceeded,
    OutputLimitExceeded,
}

pub(crate) fn output_bounded(
    command: &mut Command,
    timeout: Duration,
) -> Result<Output, GitOutputError> {
    output_bounded_inner(command, None, timeout)
}

pub(crate) fn output_bounded_with_stdin(
    command: &mut Command,
    stdin_bytes: &[u8],
    timeout: Duration,
) -> Result<Output, GitOutputError> {
    output_bounded_inner(command, Some(stdin_bytes), timeout)
}

fn output_bounded_inner(
    command: &mut Command,
    stdin_bytes: Option<&[u8]>,
    timeout: Duration,
) -> Result<Output, GitOutputError> {
    configure_read_only(command);
    if timeout.is_zero() {
        return Err(GitOutputError::DeadlineExceeded);
    }

    let started = Instant::now();
    let stdin = if let Some(stdin_bytes) = stdin_bytes {
        let mut stdin = tempfile::tempfile().map_err(GitOutputError::Io)?;
        stdin.write_all(stdin_bytes).map_err(GitOutputError::Io)?;
        stdin.seek(SeekFrom::Start(0)).map_err(GitOutputError::Io)?;
        Some(stdin)
    } else {
        None
    };
    command
        .stdin(stdin.map_or_else(Stdio::null, Stdio::from))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_process_group(command).map_err(GitOutputError::Io)?;
    if started.elapsed() >= timeout {
        return Err(GitOutputError::DeadlineExceeded);
    }
    let mut child = command.spawn().map_err(GitOutputError::Io)?;
    let process_group = match ProcessGroup::attach(&mut child) {
        Ok(process_group) => process_group,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GitOutputError::Io(error));
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            process_group.terminate(&mut child);
            let _ = child.wait();
            return Err(GitOutputError::Io(std::io::Error::other(
                "cannot capture Git stdout",
            )));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            process_group.terminate(&mut child);
            let _ = child.wait();
            return Err(GitOutputError::Io(std::io::Error::other(
                "cannot capture Git stderr",
            )));
        }
    };
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout_capture = spawn_bounded_capture(stdout, Arc::clone(&overflow));
    let stderr_capture = spawn_bounded_capture(stderr, Arc::clone(&overflow));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                process_group.terminate(&mut child);
                break status;
            }
            Ok(None) if overflow.load(Ordering::Acquire) => {
                terminate_process_group(&process_group, &mut child);
                finish_bounded_capture(stdout_capture)?;
                finish_bounded_capture(stderr_capture)?;
                return Err(GitOutputError::OutputLimitExceeded);
            }
            Ok(None) if started.elapsed() >= timeout => {
                terminate_process_group(&process_group, &mut child);
                let _ = finish_bounded_capture(stdout_capture);
                let _ = finish_bounded_capture(stderr_capture);
                return Err(GitOutputError::DeadlineExceeded);
            }
            Ok(None) => {
                let remaining = timeout.saturating_sub(started.elapsed());
                thread::sleep(remaining.min(Duration::from_millis(2)));
            }
            Err(error) => {
                terminate_process_group(&process_group, &mut child);
                let _ = finish_bounded_capture(stdout_capture);
                let _ = finish_bounded_capture(stderr_capture);
                return Err(GitOutputError::Io(error));
            }
        }
    };

    let stdout_bytes = finish_bounded_capture(stdout_capture)?;
    let stderr_bytes = finish_bounded_capture(stderr_capture)?;
    if overflow.load(Ordering::Acquire) {
        return Err(GitOutputError::OutputLimitExceeded);
    }
    if started.elapsed() >= timeout {
        return Err(GitOutputError::DeadlineExceeded);
    }
    Ok(Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    })
}

fn spawn_bounded_capture<R>(
    mut input: R,
    overflow: Arc<AtomicBool>,
) -> JoinHandle<Result<Vec<u8>, std::io::Error>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut bytes = Vec::with_capacity(MAX_GIT_OUTPUT_BYTES.min(64 * 1024));
        input
            .by_ref()
            .take((MAX_GIT_OUTPUT_BYTES as u64).saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_GIT_OUTPUT_BYTES {
            overflow.store(true, Ordering::Release);
        }
        Ok(bytes)
    })
}

fn finish_bounded_capture(
    capture: JoinHandle<Result<Vec<u8>, std::io::Error>>,
) -> Result<Vec<u8>, GitOutputError> {
    capture
        .join()
        .map_err(|_| GitOutputError::Io(std::io::Error::other("Git capture thread panicked")))?
        .map_err(GitOutputError::Io)
}

fn terminate_process_group(process_group: &ProcessGroup, child: &mut std::process::Child) {
    process_group.terminate(child);
    let _ = child.wait();
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn bounded_output_reports_the_capture_limit_before_its_independent_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("oversized-output");
        fs::write(&output, vec![0_u8; MAX_GIT_OUTPUT_BYTES + 1]).unwrap();

        let error =
            output_bounded(Command::new("cat").arg(output), Duration::from_secs(5)).unwrap_err();

        assert!(matches!(error, GitOutputError::OutputLimitExceeded));
    }
}
