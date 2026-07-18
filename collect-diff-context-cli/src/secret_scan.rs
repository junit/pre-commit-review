use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

const FINDING_EXIT_CODE: i32 = 42;
const DEFAULT_SCANNER_TIMEOUT_MS: u64 = 30_000;
const MIN_SCANNER_TIMEOUT_MS: u64 = 50;
const MAX_SCANNER_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Clone)]
pub enum SecretScanError {
    ScannerUnavailable,
    ConfigUnavailable,
    TrustMetadataUnavailable,
    ScannerIntegrity,
    ScannerVersion,
    ScannerCapability,
    ScannerTimeout,
    ProcessIo,
    ScannerFailed(i32),
    InvalidReport,
    InvalidLocation,
    ResidualFinding,
    RedactionVerification,
}

impl SecretScanError {
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::ScannerUnavailable => "scanner-unavailable",
            Self::ConfigUnavailable => "config-unavailable",
            Self::TrustMetadataUnavailable => "trust-metadata-unavailable",
            Self::ScannerIntegrity => "scanner-integrity-failed",
            Self::ScannerVersion => "scanner-version-mismatch",
            Self::ScannerCapability => "scanner-capability-failed",
            Self::ScannerTimeout => "scanner-timeout",
            Self::ProcessIo => "scanner-process-io-failed",
            Self::ScannerFailed(_) => "scanner-execution-failed",
            Self::InvalidReport => "scanner-report-invalid",
            Self::InvalidLocation => "redaction-location-invalid",
            Self::ResidualFinding => "redaction-residual-finding",
            Self::RedactionVerification => "redaction-verification-failed",
        }
    }

    pub fn is_redaction_failure(&self) -> bool {
        matches!(
            self,
            Self::InvalidLocation | Self::ResidualFinding | Self::RedactionVerification
        )
    }
}

impl fmt::Display for SecretScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScannerUnavailable => write!(f, "gitleaks executable is unavailable"),
            Self::ConfigUnavailable => write!(f, "trusted gitleaks config is unavailable"),
            Self::TrustMetadataUnavailable => {
                write!(f, "trusted gitleaks verification metadata is unavailable")
            }
            Self::ScannerIntegrity => write!(f, "bundled gitleaks integrity check failed"),
            Self::ScannerVersion => write!(f, "gitleaks version check failed"),
            Self::ScannerCapability => write!(f, "gitleaks capability check failed"),
            Self::ScannerTimeout => write!(f, "gitleaks execution timed out"),
            Self::ProcessIo => write!(f, "gitleaks process I/O failed"),
            Self::ScannerFailed(code) => write!(f, "gitleaks failed with exit code {code}"),
            Self::InvalidReport => write!(f, "gitleaks returned an invalid JSON report"),
            Self::InvalidLocation => write!(f, "gitleaks returned an invalid finding location"),
            Self::ResidualFinding => write!(f, "redacted output still contains a finding"),
            Self::RedactionVerification => {
                write!(f, "redacted output could not be verified by gitleaks")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionSummary {
    pub rule_id: String,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedOutput {
    pub content: String,
    pub redactions: Vec<RedactionSummary>,
    pub status: SecretScanStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretScanStatus {
    Clean,
    Redacted,
    Disabled,
    Unavailable(&'static str),
    RedactionFailed(&'static str),
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct GitleaksFinding {
    #[serde(rename = "RuleID")]
    rule_id: String,
    start_line: usize,
    end_line: usize,
    start_column: usize,
    end_column: usize,
    #[serde(rename = "Match")]
    matched: String,
}

#[derive(Debug)]
struct Scanner {
    executable: PathBuf,
    config: PathBuf,
    working_dir: PathBuf,
    timeout: Duration,
}

#[derive(Debug)]
struct ScannerCandidate {
    executable: PathBuf,
    bundled: bool,
}

static SCANNER: OnceLock<Result<Scanner, SecretScanError>> = OnceLock::new();

impl Scanner {
    fn discover() -> Result<Self, SecretScanError> {
        let timeout = scanner_timeout();
        let config = trusted_config_path().ok_or(SecretScanError::ConfigUnavailable)?;
        let working_dir = config
            .parent()
            .map(Path::to_path_buf)
            .ok_or(SecretScanError::ConfigUnavailable)?;

        let candidate = scanner_candidate()?;
        let version_file = trusted_script_file("gitleaks.version")
            .ok_or(SecretScanError::TrustMetadataUnavailable)?;
        if candidate.bundled {
            let manifest = trusted_script_file("gitleaks-binaries.sha256")
                .ok_or(SecretScanError::TrustMetadataUnavailable)?;
            verify_bundled_hash(&candidate.executable, &manifest)?;
        }
        verify_version(&candidate.executable, &version_file, timeout)?;

        let scanner = Self {
            executable: candidate.executable,
            config,
            working_dir,
            timeout,
        };
        scanner.verify_capability()?;
        Ok(scanner)
    }

    fn verify_capability(&self) -> Result<(), SecretScanError> {
        match self.scan("") {
            Ok(findings) if findings.is_empty() => Ok(()),
            Err(SecretScanError::ScannerTimeout) => Err(SecretScanError::ScannerTimeout),
            _ => Err(SecretScanError::ScannerCapability),
        }
    }

    fn scan(&self, input: &str) -> Result<Vec<GitleaksFinding>, SecretScanError> {
        let config = self.config.to_string_lossy();
        let args = [
            "--config",
            config.as_ref(),
            "--ignore-gitleaks-allow",
            "--exit-code=42",
            "--no-banner",
            "--no-color",
            "--log-level=error",
            "--max-decode-depth=5",
            "--report-format=json",
            "--report-path=-",
            "stdin",
        ];
        let (status, stdout) = run_scanner_process(
            &self.executable,
            &args,
            Some(&self.working_dir),
            input.as_bytes(),
            self.timeout,
        )?;
        let code = status.code().unwrap_or(-1);
        if code != 0 && code != FINDING_EXIT_CODE {
            return Err(SecretScanError::ScannerFailed(code));
        }

        serde_json::from_slice(&stdout).map_err(|_| SecretScanError::InvalidReport)
    }
}

fn scanner_timeout() -> Duration {
    let timeout_ms = env::var("PRE_COMMIT_REVIEW_GITLEAKS_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (MIN_SCANNER_TIMEOUT_MS..=MAX_SCANNER_TIMEOUT_MS).contains(value))
        .unwrap_or(DEFAULT_SCANNER_TIMEOUT_MS);
    Duration::from_millis(timeout_ms)
}

fn run_scanner_process(
    executable: &Path,
    args: &[&str],
    working_dir: Option<&Path>,
    input: &[u8],
    timeout: Duration,
) -> Result<(ExitStatus, Vec<u8>), SecretScanError> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(working_dir) = working_dir {
        command.current_dir(working_dir);
    }

    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            SecretScanError::ScannerUnavailable
        } else {
            SecretScanError::ProcessIo
        }
    })?;
    let mut child_stdin = child.stdin.take().ok_or(SecretScanError::ProcessIo)?;
    let mut child_stdout = child.stdout.take().ok_or(SecretScanError::ProcessIo)?;
    let mut child_stderr = child.stderr.take().ok_or(SecretScanError::ProcessIo)?;
    let input = input.to_vec();

    let writer = thread::spawn(move || child_stdin.write_all(&input));
    let stdout_reader = thread::spawn(move || {
        let mut output = Vec::new();
        child_stdout.read_to_end(&mut output).map(|_| output)
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        child_stderr.read_to_end(&mut output).map(|_| output)
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = writer.join();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(SecretScanError::ScannerTimeout);
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = writer.join();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(SecretScanError::ProcessIo);
            }
        }
    };

    writer
        .join()
        .map_err(|_| SecretScanError::ProcessIo)?
        .map_err(|_| SecretScanError::ProcessIo)?;
    let stdout = stdout_reader
        .join()
        .map_err(|_| SecretScanError::ProcessIo)?
        .map_err(|_| SecretScanError::ProcessIo)?;
    stderr_reader
        .join()
        .map_err(|_| SecretScanError::ProcessIo)?
        .map_err(|_| SecretScanError::ProcessIo)?;
    Ok((status, stdout))
}

#[derive(Debug)]
struct RedactionSpan {
    start: usize,
    end: usize,
    rules: BTreeSet<String>,
}

pub fn sanitize_for_model(input: &str) -> Result<SanitizedOutput, SecretScanError> {
    let scanner = SCANNER
        .get_or_init(Scanner::discover)
        .as_ref()
        .map_err(Clone::clone)?;
    sanitize_with(input, |value| scanner.scan(value))
}

pub fn sanitize_for_model_optional(input: &str) -> SanitizedOutput {
    if env::var("PRE_COMMIT_REVIEW_SECRET_SCAN").as_deref() == Ok("off") {
        return SanitizedOutput {
            content: input.to_string(),
            redactions: Vec::new(),
            status: SecretScanStatus::Disabled,
        };
    }
    match sanitize_for_model(input) {
        Ok(output) => output,
        Err(error) => {
            let status = if error.is_redaction_failure() {
                SecretScanStatus::RedactionFailed(error.reason_code())
            } else {
                SecretScanStatus::Unavailable(error.reason_code())
            };
            SanitizedOutput {
                content: input.to_string(),
                redactions: Vec::new(),
                status,
            }
        }
    }
}

fn sanitize_with<F>(input: &str, mut scan: F) -> Result<SanitizedOutput, SecretScanError>
where
    F: FnMut(&str) -> Result<Vec<GitleaksFinding>, SecretScanError>,
{
    let findings = scan(input)?;
    if findings.is_empty() {
        return Ok(SanitizedOutput {
            content: input.to_string(),
            redactions: Vec::new(),
            status: SecretScanStatus::Clean,
        });
    }

    let mut spans = Vec::with_capacity(findings.len());
    let mut summaries = Vec::with_capacity(findings.len());
    for finding in findings {
        let rule_id = sanitize_rule_id(&finding.rule_id);
        let (start, end) = finding_span(input, &finding)?;
        let mut rules = BTreeSet::new();
        rules.insert(rule_id.clone());
        spans.push(RedactionSpan { start, end, rules });
        summaries.push(RedactionSummary {
            rule_id,
            start_line: finding.start_line,
            end_line: finding.end_line,
        });
    }

    spans.sort_by_key(|span| (span.start, span.end));
    let mut merged: Vec<RedactionSpan> = Vec::with_capacity(spans.len());
    for span in spans {
        if let Some(previous) = merged.last_mut() {
            if span.start <= previous.end {
                previous.end = previous.end.max(span.end);
                previous.rules.extend(span.rules);
                continue;
            }
        }
        merged.push(span);
    }

    let mut content = input.to_string();
    for span in merged.into_iter().rev() {
        if !content.is_char_boundary(span.start) || !content.is_char_boundary(span.end) {
            return Err(SecretScanError::InvalidLocation);
        }
        let rule_list = span.rules.into_iter().collect::<Vec<_>>().join("+");
        let newline_count = input.as_bytes()[span.start..span.end]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count();
        let replacement = format!("[redacted:{rule_list}]{}", "\n".repeat(newline_count));
        content.replace_range(span.start..span.end, &replacement);
    }

    let residual = scan(&content).map_err(|_| SecretScanError::RedactionVerification)?;
    if !residual.is_empty() {
        return Err(SecretScanError::ResidualFinding);
    }

    summaries.sort_by(|left, right| {
        left.start_line
            .cmp(&right.start_line)
            .then_with(|| left.rule_id.cmp(&right.rule_id))
    });
    summaries.dedup();

    Ok(SanitizedOutput {
        content,
        redactions: summaries,
        status: SecretScanStatus::Redacted,
    })
}

fn finding_span(input: &str, finding: &GitleaksFinding) -> Result<(usize, usize), SecretScanError> {
    if finding.start_line == 0
        || finding.end_line < finding.start_line
        || finding.start_column == 0
        || finding.end_column == 0
    {
        return Err(SecretScanError::InvalidLocation);
    }

    let starts = line_starts(input.as_bytes());
    let start_line_index = finding.start_line - 1;
    let end_line_index = finding.end_line - 1;
    let start_line = *starts
        .get(start_line_index)
        .ok_or(SecretScanError::InvalidLocation)?;
    let end_line = *starts
        .get(end_line_index)
        .ok_or(SecretScanError::InvalidLocation)?;
    let start_line_end = line_end(input.as_bytes(), &starts, start_line_index);
    let end_line_end = line_end(input.as_bytes(), &starts, end_line_index);

    let reported_start = start_line.checked_add(finding.start_column - 1);
    let reported_end = end_line.checked_add(finding.end_column);
    if let (Some(start), Some(end)) = (reported_start, reported_end) {
        if start < end
            && start <= start_line_end
            && end <= end_line_end
            && input.get(start..end) == Some(finding.matched.as_str())
        {
            return Ok((start, end));
        }
    }

    // Gitleaks coordinates have changed under some output-redaction modes. Use
    // the locally captured Match only to validate/recover the span; this value
    // is never included in helper output or error messages.
    if finding.matched.is_empty() || start_line > end_line_end {
        return Err(SecretScanError::InvalidLocation);
    }
    let search_region = input
        .get(start_line..end_line_end)
        .ok_or(SecretScanError::InvalidLocation)?;
    let expected_start = reported_start.unwrap_or(start_line);
    search_region
        .match_indices(&finding.matched)
        .map(|(relative_start, matched)| {
            let start = start_line + relative_start;
            (start, start + matched.len())
        })
        .min_by_key(|(start, _)| start.abs_diff(expected_start))
        .ok_or(SecretScanError::InvalidLocation)
}

fn line_starts(input: &[u8]) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in input.iter().enumerate() {
        if *byte == b'\n' && index + 1 < input.len() {
            starts.push(index + 1);
        }
    }
    starts
}

fn line_end(input: &[u8], starts: &[usize], line_index: usize) -> usize {
    starts
        .get(line_index + 1)
        .map(|next| next.saturating_sub(1))
        .unwrap_or(input.len())
}

fn sanitize_rule_id(rule_id: &str) -> String {
    let sanitized: String = rule_id
        .chars()
        .take(80)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown-rule".to_string()
    } else {
        sanitized
    }
}

fn scanner_candidate() -> Result<ScannerCandidate, SecretScanError> {
    if let Some(path) = env::var_os("PRE_COMMIT_REVIEW_GITLEAKS_BIN") {
        let executable = PathBuf::from(path);
        return (executable.is_absolute() && executable.is_file())
            .then_some(ScannerCandidate {
                executable,
                bundled: false,
            })
            .ok_or(SecretScanError::ScannerUnavailable);
    }

    let binary_name = bundled_binary_name();
    for script_dir in script_dir_candidates() {
        let executable = script_dir.join("bin").join(&binary_name);
        if executable.is_file() {
            return Ok(ScannerCandidate {
                executable,
                bundled: true,
            });
        }
    }

    Err(SecretScanError::ScannerUnavailable)
}

fn trusted_config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("PRE_COMMIT_REVIEW_GITLEAKS_CONFIG") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }

    script_dir_candidates()
        .into_iter()
        .map(|script_dir| {
            script_dir
                .join("..")
                .join("references")
                .join("security")
                .join("gitleaks.toml")
        })
        .find(|candidate| candidate.is_file())
}

fn script_dir_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(helper) = env::var_os("PRE_COMMIT_REVIEW_HELPER_PATH") {
        if let Some(script_dir) = Path::new(&helper).parent() {
            candidates.push(script_dir.to_path_buf());
            return candidates;
        }
    }
    if let Ok(current_exe) = env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            if exe_dir.file_name().and_then(|name| name.to_str()) == Some("bin") {
                if let Some(script_dir) = exe_dir.parent() {
                    candidates.push(script_dir.to_path_buf());
                }
            }
        }
    }
    if candidates.is_empty() {
        candidates.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("scripts"),
        );
    }
    candidates.dedup();
    candidates
}

fn trusted_script_file(name: &str) -> Option<PathBuf> {
    script_dir_candidates()
        .into_iter()
        .map(|script_dir| script_dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn verify_version(
    executable: &Path,
    version_file: &Path,
    timeout: Duration,
) -> Result<(), SecretScanError> {
    let expected =
        fs::read_to_string(version_file).map_err(|_| SecretScanError::TrustMetadataUnavailable)?;
    let (status, stdout) = run_scanner_process(executable, &["version"], None, &[], timeout)
        .map_err(|error| match error {
            SecretScanError::ScannerTimeout => SecretScanError::ScannerTimeout,
            _ => SecretScanError::ScannerVersion,
        })?;
    let actual = String::from_utf8_lossy(&stdout);
    if status.success() && actual.trim() == expected.trim() {
        Ok(())
    } else {
        Err(SecretScanError::ScannerVersion)
    }
}

fn verify_bundled_hash(executable: &Path, manifest: &Path) -> Result<(), SecretScanError> {
    let name = executable
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(SecretScanError::ScannerIntegrity)?;
    let contents =
        fs::read_to_string(manifest).map_err(|_| SecretScanError::TrustMetadataUnavailable)?;
    let expected = contents
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?, fields.next()?))
        })
        .find_map(|(hash, entry)| (entry == name).then_some(hash))
        .ok_or(SecretScanError::ScannerIntegrity)?;

    let mut file = File::open(executable).map_err(|_| SecretScanError::ScannerIntegrity)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| SecretScanError::ScannerIntegrity)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual == expected {
        Ok(())
    } else {
        Err(SecretScanError::ScannerIntegrity)
    }
}

fn bundled_binary_name() -> String {
    let os = match env::consts::OS {
        "macos" => "darwin",
        "windows" => "windows",
        other => other,
    };
    let arch = match env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    let suffix = if env::consts::OS == "windows" {
        ".exe"
    } else {
        ""
    };
    format!("gitleaks-{os}-{arch}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn finding(
        rule_id: &str,
        start_line: usize,
        end_line: usize,
        start_column: usize,
        end_column: usize,
        matched: &str,
    ) -> GitleaksFinding {
        GitleaksFinding {
            rule_id: rule_id.to_string(),
            start_line,
            end_line,
            start_column,
            end_column,
            matched: matched.to_string(),
        }
    }

    #[test]
    fn redacts_single_line_match_using_inclusive_byte_columns() {
        let input = "header\n+token = glpat-example-value\nfooter\n";
        let mut calls = 0;
        let output = sanitize_with(input, |_| {
            calls += 1;
            if calls == 1 {
                Ok(vec![finding(
                    "gitlab-pat",
                    2,
                    2,
                    10,
                    28,
                    "glpat-example-value",
                )])
            } else {
                Ok(Vec::new())
            }
        })
        .expect("sanitization should succeed");

        assert_eq!(
            output.content,
            "header\n+token = [redacted:gitlab-pat]\nfooter\n"
        );
        assert_eq!(calls, 2);
    }

    #[test]
    fn redacts_multiline_and_merges_overlapping_findings() {
        let input = "before\n+secret=(first\n+second)\nafter\n";
        let mut calls = 0;
        let output = sanitize_with(input, |_| {
            calls += 1;
            if calls == 1 {
                Ok(vec![
                    finding("rule-a", 2, 3, 10, 7, "first\n+second"),
                    finding("rule-b", 2, 2, 10, 14, "first"),
                ])
            } else {
                Ok(Vec::new())
            }
        })
        .expect("sanitization should succeed");

        assert_eq!(
            output.content,
            "before\n+secret=([redacted:rule-a+rule-b]\n)\nafter\n"
        );
        assert_eq!(output.content.lines().count(), input.lines().count());
    }

    #[test]
    fn recovers_a_shifted_location_from_the_local_match() {
        let input = "+token = glpat-example-value, keep punctuation\n";
        let mut calls = 0;
        let output = sanitize_with(input, |_| {
            calls += 1;
            if calls == 1 {
                Ok(vec![finding(
                    "gitlab-pat",
                    1,
                    1,
                    11,
                    29,
                    "glpat-example-value",
                )])
            } else {
                Ok(Vec::new())
            }
        })
        .expect("a shifted scanner location should be recovered");

        assert_eq!(
            output.content,
            "+token = [redacted:gitlab-pat], keep punctuation\n"
        );
    }

    #[test]
    fn rejects_invalid_locations() {
        let input = "only one line\n";
        let error = sanitize_with(input, |_| Ok(vec![finding("rule", 2, 2, 1, 2, "missing")]))
            .expect_err("invalid locations must be reported");
        assert!(matches!(error, SecretScanError::InvalidLocation));
    }

    #[test]
    fn rejects_residual_findings_after_redaction() {
        let input = "token-value\n";
        let error = sanitize_with(input, |_| {
            Ok(vec![finding("rule", 1, 1, 1, 11, "token-value")])
        })
        .expect_err("residual findings must be reported");
        assert!(matches!(error, SecretScanError::ResidualFinding));
    }

    #[test]
    fn verifies_and_rejects_bundled_binary_hashes() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be available")
            .as_nanos();
        let root =
            env::temp_dir().join(format!("gitleaks-integrity-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&root).expect("fixture directory should be created");
        let executable = root.join("gitleaks-test");
        let manifest = root.join("manifest.sha256");
        fs::write(&executable, b"trusted scanner").expect("fixture binary should be written");

        let mut hasher = Sha256::new();
        hasher.update(b"trusted scanner");
        fs::write(
            &manifest,
            format!("{:x}  gitleaks-test\n", hasher.finalize()),
        )
        .expect("fixture manifest should be written");
        verify_bundled_hash(&executable, &manifest).expect("matching bundled binary should pass");

        fs::write(&executable, b"tampered scanner").expect("fixture binary should be replaced");
        assert!(matches!(
            verify_bundled_hash(&executable, &manifest),
            Err(SecretScanError::ScannerIntegrity)
        ));
        fs::remove_dir_all(root).expect("fixture directory should be removed");
    }

    #[test]
    fn sanitizes_rule_ids_before_rendering() {
        assert_eq!(sanitize_rule_id("rule with/slashes"), "rule_with_slashes");
        assert_eq!(sanitize_rule_id(""), "unknown-rule");
    }
}
