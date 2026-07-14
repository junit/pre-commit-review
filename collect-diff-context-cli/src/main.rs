use regex::Regex;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

// Core Constants and Defaults
const DEFAULT_MAX_DIFF_BYTES: usize = 200000;
const DEFAULT_INLINE_DIFF_BYTES: usize = 60000;
const DEFAULT_CONTEXT_QUERY_LIMIT: usize = 20;
const DEFAULT_GROUP_TARGET_BYTES: usize = 120000;
const DEFAULT_GROUP_HARD_BYTES: usize = 160000;

#[derive(Debug)]
enum AppError {
    GitError {
        cmd: String,
        details: String,
    },
    GitMissing {
        details: String,
        cmd: String,
        cwd: String,
    },
    IoError(std::io::Error),
    InvalidArgument(String),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::GitError { cmd, details } => {
                write!(f, "Git execution error (cmd: {}):\n{}", cmd, details)
            }
            AppError::GitMissing { details, cmd, cwd } => write!(
                f,
                "Git executable missing or invalid cwd: {}\nAttempted cmd: {}\nCwd: {}",
                details, cmd, cwd
            ),
            AppError::IoError(e) => write!(f, "I/O error: {}", e),
            AppError::InvalidArgument(s) => write!(f, "Invalid argument: {}", s),
        }
    }
}

struct CliArgs {
    source: Option<String>,
    path: Option<String>,
    group: Option<String>,
    include_diff: String,
    control_plane: bool,
    expect_scope: Option<String>,
}

impl CliArgs {
    fn parse() -> Result<Self, AppError> {
        let args: Vec<String> = env::args().collect();
        let mut source = None;
        let mut path = None;
        let mut group = None;
        let mut control_plane = false;
        let mut expect_scope = None;
        let mut include_diff =
            env::var("PRE_COMMIT_REVIEW_INCLUDE_DIFF").unwrap_or_else(|_| "auto".to_string());

        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--source" => {
                    if i + 1 < args.len() {
                        let val = &args[i + 1];
                        if val == "staged" || val == "unstaged" || val == "branch" {
                            source = Some(val.clone());
                        } else {
                            return Err(AppError::InvalidArgument(format!(
                                "invalid --source value: {}",
                                val
                            )));
                        }
                        i += 2;
                    } else {
                        return Err(AppError::InvalidArgument(
                            "missing value for --source".to_string(),
                        ));
                    }
                }
                "--plan-only" => {
                    include_diff = "never".to_string();
                    i += 1;
                }
                "--control-plane" => {
                    control_plane = true;
                    i += 1;
                }
                "--expect-scope" => {
                    if i + 1 < args.len() {
                        expect_scope = Some(args[i + 1].clone());
                        i += 2;
                    } else {
                        return Err(AppError::InvalidArgument(
                            "missing value for --expect-scope".to_string(),
                        ));
                    }
                }
                "--include-diff" => {
                    if i + 1 < args.len() {
                        let val = &args[i + 1];
                        if val == "auto" || val == "never" || val == "always" {
                            include_diff = val.clone();
                        } else {
                            return Err(AppError::InvalidArgument(format!(
                                "invalid --include-diff value: {}",
                                val
                            )));
                        }
                        i += 2;
                    } else {
                        return Err(AppError::InvalidArgument(
                            "missing value for --include-diff".to_string(),
                        ));
                    }
                }
                "--path" => {
                    if i + 1 < args.len() {
                        path = Some(args[i + 1].clone());
                        i += 2;
                    } else {
                        return Err(AppError::InvalidArgument(
                            "missing value for --path".to_string(),
                        ));
                    }
                }
                "--group" => {
                    if i + 1 < args.len() {
                        group = Some(args[i + 1].clone());
                        i += 2;
                    } else {
                        return Err(AppError::InvalidArgument(
                            "missing value for --group".to_string(),
                        ));
                    }
                }
                "-h" | "--help" => {
                    println!("Usage: collect_diff_context [--source staged|unstaged|branch] [--path PATH | --group GROUP_ID] [--plan-only | --include-diff auto|never|always] [--control-plane] [--expect-scope FINGERPRINT]");
                    println!();
                    println!("Collect read-only Git diff context for pre-commit review.");
                    println!();
                    println!("Options:");
                    println!("  --source SOURCE  Read from one diff source: staged, unstaged, or branch.");
                    println!(
                        "  --path PATH      Emit file-specific context for one changed path only."
                    );
                    println!(
                        "  --group GROUP_ID Emit group-specific context for one review group only."
                    );
                    println!("  --plan-only      Emit only planning metadata for the selected diff source; omit the global raw diff.");
                    println!("  --control-plane  Emit only the compact authoritative scope manifest and review work order.");
                    println!("  --expect-scope FINGERPRINT");
                    println!("                   Fail closed if the selected full diff scope no longer matches this fingerprint.");
                    println!("  --include-diff MODE");
                    println!("                   Control global diff inclusion for default output: auto, never, or always.");
                    println!("  -h, --help    Show this help.");
                    std::process::exit(0);
                }
                _ => {
                    return Err(AppError::InvalidArgument(format!(
                        "unknown argument: {}",
                        args[i]
                    )));
                }
            }
        }

        if path.is_some() && group.is_some() {
            return Err(AppError::InvalidArgument(
                "--path and --group are mutually exclusive".to_string(),
            ));
        }
        if control_plane && (path.is_some() || group.is_some()) {
            return Err(AppError::InvalidArgument(
                "--control-plane cannot be combined with --path or --group".to_string(),
            ));
        }

        if include_diff != "auto" && include_diff != "never" && include_diff != "always" {
            include_diff = "auto".to_string();
        }

        Ok(CliArgs {
            source,
            path,
            group,
            include_diff,
            control_plane,
            expect_scope,
        })
    }
}

#[derive(Debug, Clone)]
struct NameStatusEntry {
    status: String,
    path: String,
    old_path: Option<String>,
}

#[derive(Debug, Clone)]
struct NumstatEntry {
    add: String,
    del: String,
    path: String,
    old_path: Option<String>,
    path_spec: String,
}

#[derive(Debug, Clone, Serialize)]
struct ManifestUnit {
    unit_id: String,
    #[serde(rename = "path")]
    file_path: String,
    status: String,
    additions: usize,
    deletions: usize,
    diff_bytes: usize,
    risk_tags: Vec<String>,
    group_id: String,
    review_command: String,
    context_command: String,
    content_fingerprint: String,
}

#[derive(Debug, Clone, Serialize)]
struct ReviewGroup {
    group_id: String,
    risk: String,
    reason: String,
    diff_bytes: usize,
    files: Vec<String>,
    budget_status: String,
}

#[derive(Debug, Clone, Serialize)]
struct ReviewPlan {
    schema_version: usize,
    source: String,
    group_target_bytes: usize,
    group_hard_bytes: usize,
    manifest_units: usize,
    review_groups: usize,
    split_required_groups: usize,
    high_risk_units: usize,
    context_mode: String,
    state_snapshot_section: String,
    semantic_context_section: String,
    groups: Vec<PlanGroupEntry>,
    coverage_validation: CoverageValidation,
}

#[derive(Debug, Clone, Serialize)]
struct PlanGroupEntry {
    group_id: String,
    risk: String,
    reason: String,
    priority: usize,
    action: String,
    budget_status: String,
    diff_bytes: usize,
    required_units: Vec<String>,
    files: Vec<String>,
    review_commands: Vec<String>,
    context_mode: String,
    context_command: String,
    split_source: String,
    notes: String,
}

#[derive(Debug, Clone, Serialize)]
struct CoverageValidation {
    rule: &'static str,
    blocking_rule: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct ReducerState {
    schema_version: usize,
    state_kind: &'static str,
    source: String,
    status: &'static str,
    manifest_units: usize,
    review_groups: usize,
    reviewed_units: Vec<serde_json::Value>,
    pending_units: Vec<String>,
    needs_split_units: Vec<String>,
    group_results: Vec<serde_json::Value>,
    coverage_gaps: Vec<CoverageGap>,
    finding_merge: FindingMerge,
    dependency_checks: Vec<serde_json::Value>,
    test_recommendations: Vec<serde_json::Value>,
    final_verdict: &'static str,
    persistence_rule: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct CoverageGap {
    unit_id: String,
    group_id: String,
    risk_tags: String,
    coverage_status: String,
}

#[derive(Debug, Clone, Serialize)]
struct FindingMerge {
    deduplicated_findings: Vec<serde_json::Value>,
    blockers: Vec<serde_json::Value>,
    notes: Vec<serde_json::Value>,
}

struct Hunk {
    header: String,
    content: String,
    bytes: usize,
}

struct DependencyEntry {
    file: String,
    change: String,
    kind: String,
    detail: String,
}

// Render a best-effort shell-display token for human-copyable commands.
// Not a byte-perfect shell escaping format.
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.contains(['\t', '\n', '\r']) {
        let mut quoted = String::from("$'");
        for c in s.chars() {
            match c {
                '\\' => quoted.push_str("\\\\"),
                '\'' => quoted.push_str("\\'"),
                '\t' => quoted.push_str("\\t"),
                '\n' => quoted.push_str("\\n"),
                '\r' => quoted.push_str("\\r"),
                _ => quoted.push(c),
            }
        }
        quoted.push('\'');
        return quoted;
    }
    let mut quoted = String::new();
    for c in s.chars() {
        match c {
            ' ' | '\\' | '\'' | '"' | '$' | '`' | '&' | '*' | '(' | ')' | '|' | '<' | '>' | ';'
            | '!' | ',' | '?' | '[' | ']' | '{' | '}' | '^' | '~' | '#' | '=' | '\t' | '\n'
            | '\r' => {
                quoted.push('\\');
                quoted.push(c);
            }
            _ => quoted.push(c),
        }
    }
    quoted
}

// Helper to sanitize tab and newlines to preserve TSV layout sanity
fn sanitize_tsv_field(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

// Run an arbitrary command returning raw stdout bytes (preserving non-UTF8 binary outputs)
fn run_command_bytes(args: &[&str], cwd: &str) -> Result<Vec<u8>, AppError> {
    let mut cmd = Command::new(args[0]);
    cmd.args(&args[1..]);
    cmd.current_dir(cwd);

    let output = match cmd.output() {
        Ok(out) => out,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Err(AppError::GitMissing {
                    details: e.to_string(),
                    cmd: args.join(" "),
                    cwd: cwd.to_string(),
                });
            }
            return Err(AppError::IoError(e));
        }
    };

    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(AppError::GitError {
            cmd: args.join(" "),
            details: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

// Run a command with exact stdin bytes. This is used for Git's repository-native
// object hashing so the helper works with both SHA-1 and SHA-256 repositories.
fn run_command_bytes_with_stdin(
    args: &[&str],
    stdin_bytes: &[u8],
    cwd: &str,
) -> Result<Vec<u8>, AppError> {
    let mut cmd = Command::new(args[0]);
    cmd.args(&args[1..]);
    cmd.current_dir(cwd);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Err(AppError::GitMissing {
                    details: e.to_string(),
                    cmd: args.join(" "),
                    cwd: cwd.to_string(),
                });
            }
            return Err(AppError::IoError(e));
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(stdin_bytes).map_err(AppError::IoError)?;
    }
    let output = child.wait_with_output().map_err(AppError::IoError)?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(AppError::GitError {
            cmd: args.join(" "),
            details: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

// Run command returning lossy String representation for config logic
fn run_command_string(args: &[&str], cwd: &str) -> Result<String, AppError> {
    let bytes = run_command_bytes(args, cwd)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

// Git Helpers
fn git_rev_parse_toplevel() -> Result<String, AppError> {
    let out = run_command_string(&["git", "rev-parse", "--show-toplevel"], ".")?;
    Ok(out.trim().to_string())
}

fn git_has_staged_changes(cwd: &str) -> Result<bool, AppError> {
    let mut cmd = Command::new("git");
    cmd.args(["diff", "--cached", "--quiet", "--exit-code", "--", "."]);
    cmd.current_dir(cwd);
    match cmd.status() {
        Ok(status) => match status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            Some(code) => Err(AppError::GitError {
                cmd: "git diff --cached --quiet --exit-code -- .".to_string(),
                details: format!("unexpected exit code: {}", code),
            }),
            None => Err(AppError::GitError {
                cmd: "git diff --cached --quiet --exit-code -- .".to_string(),
                details: "process terminated by signal".to_string(),
            }),
        },
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Err(AppError::GitMissing {
                    details: e.to_string(),
                    cmd: "git diff --cached --quiet".to_string(),
                    cwd: cwd.to_string(),
                })
            } else {
                Err(AppError::IoError(e))
            }
        }
    }
}

fn git_has_unstaged_changes(cwd: &str) -> Result<bool, AppError> {
    let mut cmd = Command::new("git");
    cmd.args(["diff", "--quiet", "--exit-code", "--", "."]);
    cmd.current_dir(cwd);
    match cmd.status() {
        Ok(status) => match status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            Some(code) => Err(AppError::GitError {
                cmd: "git diff --quiet --exit-code -- .".to_string(),
                details: format!("unexpected exit code: {}", code),
            }),
            None => Err(AppError::GitError {
                cmd: "git diff --quiet --exit-code -- .".to_string(),
                details: "process terminated by signal".to_string(),
            }),
        },
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Err(AppError::GitMissing {
                    details: e.to_string(),
                    cmd: "git diff --quiet".to_string(),
                    cwd: cwd.to_string(),
                })
            } else {
                Err(AppError::IoError(e))
            }
        }
    }
}

fn git_has_diff_for_ref(ref_name: &str, cwd: &str) -> Result<bool, AppError> {
    let mut cmd = Command::new("git");
    let ref_expr = format!("{}...HEAD", ref_name);
    cmd.args(["diff", "--quiet", "--exit-code", &ref_expr, "--", "."]);
    cmd.current_dir(cwd);
    match cmd.status() {
        Ok(status) => match status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            Some(code) => Err(AppError::GitError {
                cmd: format!("git diff --quiet --exit-code {} -- .", ref_expr),
                details: format!("unexpected exit code: {}", code),
            }),
            None => Err(AppError::GitError {
                cmd: format!("git diff --quiet --exit-code {} -- .", ref_expr),
                details: "process terminated by signal".to_string(),
            }),
        },
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Err(AppError::GitMissing {
                    details: e.to_string(),
                    cmd: format!("git diff --quiet {}", ref_expr),
                    cwd: cwd.to_string(),
                })
            } else {
                Err(AppError::IoError(e))
            }
        }
    }
}

fn git_detect_base_branch(cwd: &str) -> String {
    let sym_ref = run_command_string(
        &[
            "git",
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
        cwd,
    );
    if let Ok(out) = sym_ref {
        let trimmed = out.trim();
        if let Some(stripped) = trimmed.strip_prefix("origin/") {
            return stripped.to_string();
        }
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    for branch in &["origin/main", "origin/master", "main", "master"] {
        let verify = run_command_string(&["git", "rev-parse", "--verify", "--quiet", branch], cwd);
        if verify.is_ok() {
            if let Some(stripped) = branch.strip_prefix("origin/") {
                return stripped.to_string();
            }
            return branch.to_string();
        }
    }

    "main".to_string()
}

fn git_get_head_sha(cwd: &str) -> String {
    let out = run_command_string(&["git", "rev-parse", "--short", "HEAD"], cwd);
    out.unwrap_or_else(|_| "unknown".to_string())
        .trim()
        .to_string()
}

fn git_get_head_oid(cwd: &str) -> String {
    let out = run_command_string(&["git", "rev-parse", "HEAD"], cwd);
    out.unwrap_or_else(|_| "unknown".to_string())
        .trim()
        .to_string()
}

fn git_get_branch_name(cwd: &str) -> String {
    let out = run_command_string(&["git", "branch", "--show-current"], cwd);
    out.unwrap_or_else(|_| "".to_string()).trim().to_string()
}

fn git_get_untracked_files(cwd: &str) -> String {
    let out = run_command_string(&["git", "ls-files", "--others", "--exclude-standard"], cwd);
    out.unwrap_or_else(|_| "".to_string()).trim().to_string()
}

fn unquote_git_path(s: &str) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let mut unquoted = String::new();
        let chars: Vec<char> = s[1..s.len() - 1].chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '\\' && i + 1 < chars.len() {
                match chars[i + 1] {
                    'a' => {
                        unquoted.push('\x07');
                        i += 2;
                    }
                    'b' => {
                        unquoted.push('\x08');
                        i += 2;
                    }
                    'f' => {
                        unquoted.push('\x0c');
                        i += 2;
                    }
                    'n' => {
                        unquoted.push('\n');
                        i += 2;
                    }
                    'r' => {
                        unquoted.push('\r');
                        i += 2;
                    }
                    't' => {
                        unquoted.push('\t');
                        i += 2;
                    }
                    'v' => {
                        unquoted.push('\x0b');
                        i += 2;
                    }
                    '\\' => {
                        unquoted.push('\\');
                        i += 2;
                    }
                    '"' => {
                        unquoted.push('"');
                        i += 2;
                    }
                    '?' => {
                        unquoted.push('?');
                        i += 2;
                    }
                    c if c.is_digit(8) => {
                        let mut octal_val: u32 = 0;
                        let mut digits = 0;
                        while i + 1 + digits < chars.len() && digits < 3 {
                            let next_c = chars[i + 1 + digits];
                            if next_c.is_digit(8) {
                                octal_val = octal_val * 8 + next_c.to_digit(8).unwrap();
                                digits += 1;
                            } else {
                                break;
                            }
                        }
                        if let Some(decoded_char) = std::char::from_u32(octal_val) {
                            unquoted.push(decoded_char);
                        } else {
                            unquoted.push(octal_val as u8 as char);
                        }
                        i += 1 + digits;
                    }
                    _ => {
                        unquoted.push(chars[i]);
                        i += 1;
                    }
                }
            } else {
                unquoted.push(chars[i]);
                i += 1;
            }
        }
        unquoted
    } else {
        s.to_string()
    }
}

fn quote_git_path(s: &str) -> String {
    let mut needs_quoting = false;
    for b in s.bytes() {
        if b == b'\t'
            || b == b'\n'
            || b == b'\r'
            || b == b'"'
            || b == b'\\'
            || !(32..127).contains(&b)
        {
            needs_quoting = true;
            break;
        }
    }
    if !needs_quoting {
        return s.to_string();
    }
    let mut quoted = String::new();
    quoted.push('"');
    for b in s.bytes() {
        match b {
            7 => quoted.push_str("\\a"),
            8 => quoted.push_str("\\b"),
            9 => quoted.push_str("\\t"),
            10 => quoted.push_str("\\n"),
            11 => quoted.push_str("\\v"),
            12 => quoted.push_str("\\f"),
            13 => quoted.push_str("\\r"),
            b'"' => quoted.push_str("\\\""),
            b'\\' => quoted.push_str("\\\\"),
            other => {
                if !(32..127).contains(&other) {
                    quoted.push_str(&format!("\\{:03o}", other));
                } else {
                    quoted.push(other as char);
                }
            }
        }
    }
    quoted.push('"');
    quoted
}

fn git_run_diff_bytes(
    mode: &str,
    selected_ref: &str,
    extra_args: &[&str],
    path: Option<&str>,
    cwd: &str,
) -> Result<Vec<u8>, AppError> {
    let mut args = vec![
        "git",
        "-c",
        "color.ui=false",
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames",
    ];
    for arg in extra_args {
        args.push(arg);
    }

    let ref_expr;
    if mode == "staged" {
        args.push("--cached");
    } else if mode == "branch" {
        ref_expr = format!("{}...HEAD", selected_ref);
        args.push(&ref_expr);
    }

    let unquoted_p;
    args.push("--");
    if let Some(p) = path {
        unquoted_p = unquote_git_path(p);
        args.push(&unquoted_p);
    } else {
        args.push(".");
    }

    run_command_bytes(&args, cwd)
}

fn git_run_diff_string(
    mode: &str,
    selected_ref: &str,
    extra_args: &[&str],
    path: Option<&str>,
    cwd: &str,
) -> Result<String, AppError> {
    let bytes = git_run_diff_bytes(mode, selected_ref, extra_args, path, cwd)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn append_fingerprint_field(material: &mut Vec<u8>, name: &str, value: &[u8]) {
    material.extend_from_slice(name.as_bytes());
    material.push(0);
    material.extend_from_slice(value.len().to_string().as_bytes());
    material.push(0);
    material.extend_from_slice(value);
    material.push(0);
}

fn git_hash_object_bytes(bytes: &[u8], cwd: &str) -> Result<String, AppError> {
    let out = run_command_bytes_with_stdin(&["git", "hash-object", "--stdin"], bytes, cwd)?;
    let oid = String::from_utf8_lossy(&out).trim().to_string();
    if oid.is_empty() {
        return Err(AppError::GitError {
            cmd: "git hash-object --stdin".to_string(),
            details: "Git returned an empty object id".to_string(),
        });
    }
    Ok(oid)
}

fn diff_fingerprint(
    mode: &str,
    selected_ref: &str,
    head_oid: &str,
    path: Option<&str>,
    identity_path: Option<&str>,
    cwd: &str,
) -> Result<String, AppError> {
    let diff_bytes = if mode == "none" {
        Vec::new()
    } else {
        // The full-scope fingerprint uses binary-safe, full-index output. Keep
        // per-unit framing on the ordinary helper diff because that is the
        // exact review unit emitted by both native and legacy implementations.
        let fingerprint_args: &[&str] = if path.is_none() {
            &["--binary", "--full-index"]
        } else {
            &[]
        };
        git_run_diff_bytes(mode, selected_ref, fingerprint_args, path, cwd)?
    };

    diff_fingerprint_from_bytes(
        mode,
        selected_ref,
        head_oid,
        identity_path.or(path),
        &diff_bytes,
        cwd,
    )
}

fn diff_fingerprint_from_bytes(
    mode: &str,
    selected_ref: &str,
    head_oid: &str,
    identity_path: Option<&str>,
    diff_bytes: &[u8],
    cwd: &str,
) -> Result<String, AppError> {
    let mut material = b"pre-commit-review-diff-fingerprint-v1\0".to_vec();
    append_fingerprint_field(&mut material, "source", mode.as_bytes());
    append_fingerprint_field(&mut material, "selected-ref", selected_ref.as_bytes());
    append_fingerprint_field(&mut material, "head", head_oid.as_bytes());
    if let Some(path) = identity_path {
        append_fingerprint_field(&mut material, "path", path.as_bytes());
    }
    append_fingerprint_field(&mut material, "diff", diff_bytes);
    git_hash_object_bytes(&material, cwd)
}

struct ScopeIdentity<'a> {
    source: &'a str,
    head: &'a str,
    base: &'a str,
    selected_ref: &'a str,
}

fn emit_authority_failure(
    scope: &ScopeIdentity<'_>,
    expected: Option<&str>,
    started: &str,
    observed: &str,
    reason: &str,
) {
    let payload = serde_json::json!({
        "schema_version": 1,
        "kind": "review_control_plane",
        "authoritative": false,
        "reason": reason,
        "source": scope.source,
        "head": scope.head,
        "base": scope.base,
        "selected_ref": scope.selected_ref,
        "expected_scope_fingerprint": expected,
        "collection_start_fingerprint": started,
        "observed_scope_fingerprint": observed,
        "recovery": "rerun --control-plane and discard all coverage recorded under the previous scope fingerprint"
    });
    println!("# Pre-Commit Review Control Plane\n");
    println!("## Review Control Plane JSON");
    println!("{}", serde_json::to_string(&payload).unwrap_or_default());
}

fn emit_control_plane(
    scope: &ScopeIdentity<'_>,
    scope_fingerprint: &str,
    self_exe: &str,
    manifest_units: &[ManifestUnit],
    groups: &[ReviewGroup],
) {
    let total_additions: usize = manifest_units.iter().map(|u| u.additions).sum();
    let total_deletions: usize = manifest_units.iter().map(|u| u.deletions).sum();
    let total_diff_bytes: usize = manifest_units.iter().map(|u| u.diff_bytes).sum();
    let high_risk_units = manifest_units
        .iter()
        .filter(|u| u.risk_tags.iter().any(|tag| tag == "high-risk"))
        .count();
    let split_required_groups = groups
        .iter()
        .filter(|g| g.budget_status == "split-required")
        .count();

    // Positional tuple schema keeps large manifests compact while preserving a
    // single, explicit field definition for consumers.
    let units: Vec<serde_json::Value> = manifest_units
        .iter()
        .map(|u| {
            serde_json::json!([
                u.file_path,
                u.status,
                u.additions,
                u.deletions,
                u.diff_bytes,
                u.risk_tags.join(";"),
                u.group_id,
                u.content_fingerprint
            ])
        })
        .collect();

    let compact_groups: Vec<serde_json::Value> = groups
        .iter()
        .map(|g| {
            let unit_indexes: Vec<usize> = manifest_units
                .iter()
                .enumerate()
                .filter(|(_, u)| u.group_id == g.group_id)
                .map(|(idx, _)| idx)
                .collect();
            serde_json::json!([
                g.group_id,
                g.risk,
                g.reason,
                g.diff_bytes,
                g.budget_status,
                unit_indexes
            ])
        })
        .collect();

    let mut work_order: Vec<serde_json::Value> = groups
        .iter()
        .map(|g| {
            let (priority, action) = if g.budget_status == "split-required" {
                (1, "split")
            } else if g.risk == "high" {
                (2, "review")
            } else if g.risk == "consistency" {
                (3, "review")
            } else {
                (4, "review")
            };
            serde_json::json!([priority, g.group_id, action])
        })
        .collect();
    work_order.sort_by(|a, b| {
        let a_priority = a
            .get(0)
            .and_then(|v| v.as_u64())
            .unwrap_or(usize::MAX as u64);
        let b_priority = b
            .get(0)
            .and_then(|v| v.as_u64())
            .unwrap_or(usize::MAX as u64);
        a_priority.cmp(&b_priority).then_with(|| {
            a.get(1)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .cmp(b.get(1).and_then(|v| v.as_str()).unwrap_or(""))
        })
    });

    let payload = serde_json::json!({
        "schema_version": 1,
        "kind": "review_control_plane",
        "authoritative": true,
        "source": scope.source,
        "head": scope.head,
        "base": scope.base,
        "selected_ref": scope.selected_ref,
        "scope_fingerprint": scope_fingerprint,
        "fingerprint_algorithm": "git-hash-object(binary-full-index-no-textconv)",
        "collection": {
            "start": scope_fingerprint,
            "end": scope_fingerprint
        },
        "counts": {
            "units": manifest_units.len(),
            "groups": groups.len(),
            "additions": total_additions,
            "deletions": total_deletions,
            "diff_bytes": total_diff_bytes,
            "high_risk_units": high_risk_units,
            "split_required_groups": split_required_groups
        },
        "command_templates": {
            "helper": self_exe,
            "source_args": ["--source", scope.source],
            "refresh_args": ["--control-plane"],
            "group_args": ["--group", "{group_id}", "--expect-scope", "{scope_fingerprint}"],
            "path_args": ["--path", "{path}", "--expect-scope", "{scope_fingerprint}"]
        },
        "unit_tuple_fields": ["path", "status", "additions", "deletions", "diff_bytes", "risk_tags", "group_id", "content_fingerprint"],
        "units": units,
        "group_tuple_fields": ["group_id", "risk", "reason", "diff_bytes", "budget_status", "unit_indexes"],
        "groups": compact_groups,
        "work_order_tuple_fields": ["priority", "group_id", "action"],
        "work_order": work_order,
        "coverage_contract": {
            "unit_id": "file:<unit path>",
            "initial_status": "pending",
            "completion": "every unit index is reviewed under this exact scope_fingerprint",
            "split_rule": "replace each unit in a split-required group with bounded review units before claiming coverage",
            "blocking_rule": "scope drift or any high-risk/needs-split coverage gap forces DO_NOT_COMMIT",
            "finalization": "rerun --control-plane and require unchanged scope_fingerprint, units, groups, and work_order"
        }
    });

    println!("# Pre-Commit Review Control Plane\n");
    println!("## Review Control Plane JSON");
    println!("{}", serde_json::to_string(&payload).unwrap_or_default());
}

fn git_show_ref_bytes(refspec: &str, cwd: &str) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .args(["show", refspec])
        .current_dir(cwd)
        .output()
        .ok()?;
    if output.status.success() {
        Some(output.stdout)
    } else {
        None
    }
}

fn file_content_for_diff_source(
    mode: &str,
    _selected_ref: &str,
    path: &str,
    repo_root: &str,
) -> String {
    let refspec;
    let bytes = match mode {
        "staged" => {
            refspec = format!(":{}", path);
            git_show_ref_bytes(&refspec, repo_root)
        }
        "branch" => {
            refspec = format!("HEAD:{}", path);
            git_show_ref_bytes(&refspec, repo_root)
        }
        "unstaged" => fs::read(Path::new(repo_root).join(path)).ok(),
        _ => None,
    }
    .or_else(|| fs::read(Path::new(repo_root).join(path)).ok())
    .unwrap_or_default();

    String::from_utf8_lossy(&bytes).into_owned()
}

fn is_test_like_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.starts_with("e2e/")
        || lower.starts_with("cypress/")
        || lower.starts_with("playwright/")
        || lower.starts_with("src/test/")
        || lower.contains("/test/")
        || lower.contains("/tests/")
        || lower.contains("/e2e/")
        || lower.contains("/cypress/")
        || lower.contains("/playwright/")
        || lower.contains("/__tests__/")
        || lower.contains("/src/test/")
        || lower.contains("/src/it/")
        || lower.contains("/src/integrationtest/")
        || lower.contains("/src/integration-test/")
        || lower.ends_with("test.java")
        || lower.ends_with("tests.java")
        || lower.ends_with("it.java")
        || lower.ends_with("itcase.java")
        || lower.ends_with("integrationtest.java")
        || lower.ends_with("spec.java")
        || lower.ends_with("test.kt")
        || lower.ends_with("tests.kt")
        || lower.ends_with("it.kt")
        || lower.ends_with("itcase.kt")
        || lower.ends_with("integrationtest.kt")
        || lower.ends_with("spec.kt")
        || lower.ends_with("test.groovy")
        || lower.ends_with("spec.groovy")
        || lower.ends_with("it.groovy")
        || lower.ends_with("integrationtest.groovy")
        || lower.ends_with("test.scala")
        || lower.ends_with("spec.scala")
        || lower.ends_with("it.scala")
        || lower.ends_with("integrationtest.scala")
        || lower.ends_with("test.ts")
        || lower.ends_with("spec.ts")
        || lower.ends_with("e2e.ts")
        || lower.ends_with("cy.ts")
        || lower.ends_with("test.tsx")
        || lower.ends_with("spec.tsx")
        || lower.ends_with("e2e.tsx")
        || lower.ends_with("cy.tsx")
        || lower.ends_with("test.js")
        || lower.ends_with("spec.js")
        || lower.ends_with("e2e.js")
        || lower.ends_with("cy.js")
        || lower.ends_with("test.jsx")
        || lower.ends_with("spec.jsx")
        || lower.ends_with("e2e.jsx")
        || lower.ends_with("cy.jsx")
        || lower.ends_with("_test.go")
        || lower.ends_with("_test.py")
        || lower.ends_with(".spec.py")
        || lower.starts_with("test_")
        || lower.contains("/test_")
}

fn configured_test_hint_for_path(
    path: &str,
    content: &str,
    repo_root: &str,
) -> Option<[String; 5]> {
    let hints_path = Path::new(repo_root).join(".pre-commit-review/test-hints");
    let file = File::open(hints_path).ok()?;
    let reader = BufReader::new(file);
    for line_result in reader.lines() {
        let line = line_result.ok()?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 7 {
            continue;
        }
        let rule_id = parts[0].trim();
        let path_regex = parts[1].trim();
        let content_regex = parts[2].trim();
        let test_kind = parts[3].trim();
        let dependency = parts[4].trim();
        let confidence = parts[5].trim();
        let hint = parts[6..].join(" ").trim().to_string();

        if rule_id.is_empty()
            || test_kind.is_empty()
            || dependency.is_empty()
            || confidence.is_empty()
            || hint.is_empty()
        {
            continue;
        }

        let path_match = !path_regex.is_empty()
            && Regex::new(path_regex)
                .map(|re| re.is_match(path))
                .unwrap_or(false);
        let content_match = !content_regex.is_empty()
            && Regex::new(content_regex)
                .map(|re| re.is_match(content))
                .unwrap_or(false);
        if path_match || content_match {
            return Some([
                rule_id.to_string(),
                confidence.to_string(),
                test_kind.to_string(),
                dependency.to_string(),
                hint,
            ]);
        }
    }
    None
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn path_indicates_jvm_integration(lower_path: &str) -> bool {
    lower_path.contains("/src/it/")
        || lower_path.contains("/src/integrationtest/")
        || lower_path.contains("/src/integration-test/")
        || lower_path.ends_with("it.java")
        || lower_path.ends_with("itcase.java")
        || lower_path.ends_with("integrationtest.java")
        || lower_path.ends_with("it.kt")
        || lower_path.ends_with("itcase.kt")
        || lower_path.ends_with("integrationtest.kt")
        || lower_path.ends_with("it.groovy")
        || lower_path.ends_with("integrationtest.groovy")
        || lower_path.ends_with("it.scala")
        || lower_path.ends_with("integrationtest.scala")
}

fn classify_test_hint(
    path: &str,
    content: &str,
) -> (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
) {
    let lower_path = path.to_ascii_lowercase();
    let lower_content = content.to_ascii_lowercase();

    if contains_any(
        &lower_content,
        &[
            "org.testcontainers",
            "@testcontainers",
            "@container",
            "testcontainers-go",
        ],
    ) {
        (
            "testcontainers",
            "high",
            "container-integration",
            "docker-or-testcontainers",
            "Requires Docker/Testcontainers; do not treat failure in a sandbox as a pure code failure without environment evidence.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "dockercomposecontainer",
            "docker-compose",
            "docker compose",
            "compose.yml",
            "compose.yaml",
        ],
    ) {
        (
            "docker-compose-test",
            "high",
            "compose-backed-integration",
            "docker-compose-runtime",
            "Uses Docker Compose or compose-backed services; verify in an environment with Docker and required service images.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "wiremockserver",
            "wiremockextension",
            "@autoconfigurewiremock",
            "com.github.tomakehurst.wiremock",
            "wiremock.org",
        ],
    ) {
        (
            "wiremock-test",
            "high",
            "http-stub-integration",
            "wiremock-runtime",
            "Uses WireMock HTTP stubs; sandbox failures may reflect port/runtime setup rather than the changed code.",
        )
    } else if contains_any(
        &lower_content,
        &["org.mockserver", "mockservercontainer", "clientandserver"],
    ) {
        (
            "mockserver-test",
            "high",
            "http-stub-integration",
            "mockserver-runtime",
            "Uses MockServer or its container runtime; verify with the required local or CI service setup.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "@autoconfigurestubrunner",
            "stubrunner",
            "spring-cloud-contract",
            "org.springframework.cloud.contract",
        ],
    ) {
        (
            "spring-cloud-contract",
            "high",
            "contract-integration",
            "spring-cloud-contract-runtime",
            "Uses Spring Cloud Contract or Stub Runner; may require generated stubs, broker settings, or CI contract artifacts.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "jdbc:",
            "r2dbc:",
            "spring.datasource.url",
            "datasource.url",
            "postgresql",
            "mysql",
            "mariadb",
            "oracle.jdbc",
            "mongodb://",
            "redis://",
            "spring.redis",
            "spring.data.redis",
            "kafka.bootstrap",
            "bootstrap.servers",
            "spring.kafka",
            "elasticsearch",
            "opensearch",
            "rabbitmq",
            "amqp://",
            "localstack",
            "minio",
        ],
    ) {
        (
            "external-service-config",
            "high",
            "service-backed-integration",
            "database-cache-broker-or-search-service",
            "References database, cache, broker, search, or object-storage service configuration; run with the expected local profile or CI services.",
        )
    } else if contains_any(
        &lower_content,
        &["@quarkustest", "@quarkusintegrationtest", "io.quarkus.test"],
    ) {
        (
            "quarkus-test-context",
            "high",
            "quarkus-integration",
            "quarkus-test-runtime",
            "Loads a Quarkus test context; may require Quarkus profiles, dev services, containers, or CI runtime support.",
        )
    } else if contains_any(&lower_content, &["@micronauttest", "io.micronaut.test"]) {
        (
            "micronaut-test-context",
            "high",
            "micronaut-integration",
            "micronaut-test-runtime",
            "Loads a Micronaut test context; may require application context configuration or service-backed test resources.",
        )
    } else if content.contains("@SpringBootTest") {
        (
            "spring-boot-context",
            "high",
            "spring-boot-integration",
            "spring-context",
            "Loads a Spring Boot application context; may require local profiles, DB, middleware, or CI-provided services.",
        )
    } else if content.contains("@DataJpaTest")
        || content.contains("@JdbcTest")
        || content.contains("@JooqTest")
        || content.contains("@MybatisTest")
    {
        (
            "spring-data-slice",
            "high",
            "data-slice-integration",
            "database-or-spring-test-slice",
            "Loads a data test slice; may require an embedded or configured database.",
        )
    } else if content.contains("@WebMvcTest") || content.contains("@AutoConfigureMockMvc") {
        (
            "spring-web-slice",
            "high",
            "spring-web-slice",
            "spring-test-context",
            "Loads a Spring web test slice; usually narrower than full integration but not a pure unit test.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "@activeprofiles",
            "spring_profiles_active",
            "quarkus.test.profile",
            "micronaut.environments",
        ],
    ) {
        (
            "jvm-test-profile",
            "high",
            "profile-backed-test",
            "maven-gradle-or-framework-profile",
            "Selects framework test profiles or environments; use the matching Maven/Gradle profile or CI profile configuration.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "@tag(\"integration\")",
            "@tag(\"e2e\")",
            "@tag(\"contract\")",
            "@tag(\"slow\")",
            "@category(integrationtest",
            "@category(e2etest",
        ],
    ) {
        (
            "junit-integration-tag",
            "high",
            "tagged-jvm-integration",
            "junit-tag-or-category-selection",
            "Uses JUnit integration/e2e/contract tags; run with the tag expression and environment expected by the project.",
        )
    } else if path_indicates_jvm_integration(&lower_path) {
        (
            "jvm-integration-naming",
            "medium",
            "jvm-integration-by-convention",
            "maven-failsafe-or-gradle-integration-profile",
            "Path or class name follows common JVM integration-test conventions such as *IT or src/integrationTest; run the project integration-test profile if available.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "pytest.mark.integration",
            "pytest.mark.e2e",
            "pytest.mark.contract",
            "pytest.mark.system",
            "pytest.mark.django_db",
            "pytest.mark.db",
            "pytest.mark.redis",
            "pytest.mark.kafka",
            "pytest.mark.elasticsearch",
        ],
    ) {
        (
            "pytest-env-marker",
            "high",
            "pytest-marked-integration",
            "pytest-marker-or-service-runtime",
            "Uses pytest markers that usually select integration/e2e/database/service tests; run with the matching marker and required services.",
        )
    } else if contains_any(&lower_content, &["@playwright/test", "playwright/test"])
        || lower_path.ends_with(".pw.ts")
        || lower_path.ends_with(".pw.js")
    {
        (
            "playwright-e2e",
            "high",
            "browser-e2e",
            "browser-runtime-and-app-server",
            "Uses Playwright; requires browser runtime and usually a running app server or configured webServer.",
        )
    } else if lower_path.contains("/cypress/")
        || lower_path.ends_with(".cy.ts")
        || lower_path.ends_with(".cy.tsx")
        || lower_path.ends_with(".cy.js")
        || lower_path.ends_with(".cy.jsx")
        || contains_any(&lower_content, &["cy.visit(", "cypress."])
    {
        (
            "cypress-e2e",
            "high",
            "browser-e2e",
            "browser-runtime-and-app-server",
            "Uses Cypress; requires browser runtime and usually a running app server.",
        )
    } else if (lower_path.contains("/e2e/")
        || lower_path.contains(".e2e.")
        || lower_path.contains("/integration/"))
        && contains_any(&lower_content, &["vitest", "jest", "describe(", "test("])
    {
        (
            "node-e2e-or-integration",
            "medium",
            "node-e2e-or-integration",
            "node-runtime-and-possibly-app-server",
            "Path/content follows common Node e2e or integration-test conventions; verify with the project test script and required runtime services.",
        )
    } else if contains_any(
        &lower_content,
        &[
            "//go:build integration",
            "//go:build e2e",
            "//go:build docker",
            "// +build integration",
            "// +build e2e",
            "// +build docker",
        ],
    ) {
        (
            "go-integration-build-tag",
            "high",
            "go-tagged-integration",
            "go-build-tags-and-service-runtime",
            "Uses Go integration/e2e/docker build tags; run go test with the matching tags and required services.",
        )
    } else if lower_path.ends_with("_test.go")
        && (lower_path.contains("integration") || lower_path.contains("/e2e/"))
    {
        (
            "go-integration-naming",
            "medium",
            "go-integration-by-convention",
            "go-test-selection-or-service-runtime",
            "Go test path suggests integration coverage; check project docs for tags, env vars, or service dependencies.",
        )
    } else if lower_content.contains("#[ignore]") {
        (
            "rust-ignored-test",
            "medium",
            "rust-ignored-or-slow-test",
            "cargo-test-ignored-selection",
            "Rust ignored tests are not run by default and often need explicit `cargo test -- --ignored` plus external setup.",
        )
    } else if lower_path.ends_with(".rs")
        && (lower_path.starts_with("tests/")
            || lower_path.contains("/tests/")
            || lower_path.contains("/integration/"))
    {
        (
            "rust-integration-path",
            "low",
            "rust-integration-by-convention",
            "cargo-test-selection-or-project-specific-runtime",
            "Rust test path follows Cargo integration-test layout; treat as a planning hint and verify whether external setup is required.",
        )
    } else {
        (
            "no-known-env-heavy-marker",
            "low",
            "unit-or-unknown",
            "not-proven-isolated",
            "No known env-heavy marker detected; this is not proof of unit-test isolation. Prefer the narrowest focused test command for this file.",
        )
    }
}

fn emit_test_selection_hints(
    name_status_entries: &[NameStatusEntry],
    mode: &str,
    selected_ref: &str,
    repo_root: &str,
) {
    println!("## Test Selection Hints");
    println!("path\trule_id\tconfidence\ttest_kind\tenvironment_dependency\thint");
    let mut emitted = false;
    for entry in name_status_entries {
        let path = &entry.path;
        if !is_test_like_path(path) {
            continue;
        }
        let content = file_content_for_diff_source(mode, selected_ref, path, repo_root);
        if let Some([rule_id, confidence, kind, dependency, hint]) =
            configured_test_hint_for_path(path, &content, repo_root)
        {
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                sanitize_tsv_field(path),
                sanitize_tsv_field(&rule_id),
                sanitize_tsv_field(&confidence),
                sanitize_tsv_field(&kind),
                sanitize_tsv_field(&dependency),
                sanitize_tsv_field(&hint)
            );
            emitted = true;
            continue;
        }
        let (rule_id, confidence, kind, dependency, hint) = classify_test_hint(path, &content);
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            sanitize_tsv_field(path),
            sanitize_tsv_field(rule_id),
            sanitize_tsv_field(confidence),
            sanitize_tsv_field(kind),
            sanitize_tsv_field(dependency),
            sanitize_tsv_field(hint)
        );
        emitted = true;
    }
    if !emitted {
        println!("none\tnone\tnone\tnone\tnone\tno changed test files detected");
    }
}

// Thread-Safe OnceLock Classifiers for Tier-1 Quality
fn get_path_risk_regexes() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        vec![
            Regex::new(r"(?i)(^|/|[_-])(auth|authentication|permission|permissions|security|oauth|session|sessions|jwt|token|tokens|acl|rbac)(/|[_\.-]|$)").unwrap(),
            Regex::new(r"(?i)(^|/)(db|database|sql)/.*(migration|migrations|schema)").unwrap(),
            Regex::new(r"(?i)(^|/)(migration|migrations)(/|$)").unwrap(),
            Regex::new(r"(?i)(^|/)(payment|payments|billing|invoice|invoices|checkout)(/|[_\.-]|$)").unwrap(),
            Regex::new(r"(?i)(^|/)(config|configs|deploy|deployment|infra|infrastructure|terraform|k8s|kubernetes|docker|\.github/workflows)(/|$)").unwrap(),
            Regex::new(r"(?i)(^|/|[_-])(concurrency|async|retry|queue|worker|scheduler|delete|deletion|destroy|destructive)(/|[_\.-]|$)").unwrap(),
            Regex::new(r"(?i)(^|/|[_-])(crypto|cryptographic|encrypt|decrypt|hash|hashing|sha|sha256|md5|rsa|aes|tls|ssl|cert|certificate|bcrypt|argon2)(/|[_\.-]|$)").unwrap(),
            Regex::new(r"(?i)(^|/|[_-])(secret|secrets|credential|credentials|api[_-]?key|apikey|vault|keychain)(/|[_\.-]|$)").unwrap(),
            Regex::new(r"(?i)(^|/|[_-])(cors|csrf|xss|sanitize|sanitizer|escape)(/|[_\.-]|$)").unwrap(),
            Regex::new(r"(?i)(^|/|[_-])(role|roles|admin|superuser|root|sudo|policy|policies)(/|[_\.-]|$)").unwrap(),
            Regex::new(r"(?i)(^|/|[_-])(exec|eval|spawn|subprocess|shell|command|cmd)(/|[_\.-]|$)").unwrap(),
            Regex::new(r"(?i)(^|/|[_-])(upload|download|attachment|attachments|file|files)(/|[_\.-]|$)").unwrap(),
            Regex::new(r"(?i)(^|/|[_-])(env|environment|settings|configure)(/|[_\.-]|$)").unwrap(),
        ]
    })
}

fn get_content_risk_regexes() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        vec![
            Regex::new(r"(?i)(authorization|authenticate|authentication|permission|permissions|is_admin|oauth|jwt|session|token|secret|password|credential)").unwrap(),
            Regex::new(r"(?i)(alter\s+table|drop\s+table|delete\s+from|truncate\s+table|grant\s+|revoke\s+)").unwrap(),
            Regex::new(r"(?i)(payment|billing|invoice|checkout|refund)").unwrap(),
            Regex::new(r"(?i)(retry|timeout|queue|worker|scheduler|transaction)").unwrap(),
            Regex::new(r"(?i)(crypto\.|createcipher|hashlib\.|sha256|sha512|md5|bcrypt\.compare|argon2|aes|rsa|x509|tls|ssl)").unwrap(),
            Regex::new(r"(?i)(process\.env\.[a-z0-9_]*(secret|token|key|password)|os\.environ.*(secret|token|key|password)|api[_-]?key|secret[_-]?key|private[_-]?key)").unwrap(),
            Regex::new(r"(?i)(eval\s*\(|exec\s*\(|subprocess\.|child_process|spawn\s*\(|system\s*\()").unwrap(),
            Regex::new(r"(?i)(cors|csrf|xss|sanitize|sanitizer|escapehtml|escape_html)").unwrap(),
            Regex::new(r"(?i)(fs\.unlink|os\.remove|drop\s+database|grant\s+all|chmod\s+777|sudo\s)").unwrap(),
        ]
    })
}

fn get_generated_regexes() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        vec![
            Regex::new(r"(?i)(^|/)(__snapshots__|snapshots|generated|vendor|vendors|dist|build|coverage)(/|$)").unwrap(),
            Regex::new(r"(?i)(\.snap|\.snapshot|\.generated\.|_generated\.|\.min\.(js|css))$").unwrap(),
        ]
    })
}

fn get_lockfile_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(^|/)(package-lock\.json|npm-shrinkwrap\.json|yarn\.lock|pnpm-lock\.yaml|poetry\.lock|pipfile\.lock|cargo\.lock|gemfile\.lock|composer\.lock|go\.sum)$").unwrap()
    })
}

fn load_custom_regexes(path: &Path) -> Vec<Regex> {
    let mut regexes = Vec::new();
    if let Ok(file) = File::open(path) {
        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Ok(re) = Regex::new(trimmed) {
                regexes.push(re);
            } else {
                eprintln!(
                    "Warning: invalid custom regex in {}: {}",
                    path.display(),
                    trimmed
                );
            }
        }
    }
    regexes
}

fn group_component_for_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() >= 2 {
        let first = parts[0];
        let second = parts[1];
        if second == "migration"
            || second == "migrations"
            || second == "schema"
            || second == "schemas"
        {
            return format!("{}-{}", first, second);
        }
        first.to_string()
    } else {
        path.to_string()
    }
}

fn safe_group_component(component: &str) -> String {
    component
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn parse_name_status_z(bytes: &[u8]) -> Vec<NameStatusEntry> {
    let mut entries = Vec::new();
    let mut parts = bytes.split(|&b| b == 0);
    while let Some(status_bytes) = parts.next() {
        if status_bytes.is_empty() {
            continue;
        }
        let status = String::from_utf8_lossy(status_bytes).into_owned();
        if status.starts_with('R') || status.starts_with('C') {
            let src_bytes = match parts.next() {
                Some(b) => b,
                None => break,
            };
            let dest_bytes = match parts.next() {
                Some(b) => b,
                None => break,
            };
            entries.push(NameStatusEntry {
                status,
                path: String::from_utf8_lossy(dest_bytes).into_owned(),
                old_path: Some(String::from_utf8_lossy(src_bytes).into_owned()),
            });
        } else {
            let path_bytes = match parts.next() {
                Some(b) => b,
                None => break,
            };
            entries.push(NameStatusEntry {
                status,
                path: String::from_utf8_lossy(path_bytes).into_owned(),
                old_path: None,
            });
        }
    }
    entries
}

fn parse_numstat_z(bytes: &[u8]) -> Vec<NumstatEntry> {
    let mut entries = Vec::new();
    let mut parts = bytes.split(|&b| b == 0);
    while let Some(first_part) = parts.next() {
        if first_part.is_empty() {
            continue;
        }
        if let Some(first_tab) = first_part.iter().position(|&b| b == b'\t') {
            let add_bytes = &first_part[..first_tab];
            let rest = &first_part[first_tab + 1..];
            if let Some(second_tab) = rest.iter().position(|&b| b == b'\t') {
                let del_bytes = &rest[..second_tab];
                let path_bytes = &rest[second_tab + 1..];

                let add_str = String::from_utf8_lossy(add_bytes);
                let del_str = String::from_utf8_lossy(del_bytes);
                let add = add_str.trim().to_string();
                let del = del_str.trim().to_string();

                if path_bytes.is_empty() {
                    // Rename!
                    let src_bytes = match parts.next() {
                        Some(b) => b,
                        None => break,
                    };
                    let dest_bytes = match parts.next() {
                        Some(b) => b,
                        None => break,
                    };
                    let src = String::from_utf8_lossy(src_bytes).into_owned();
                    let dest = String::from_utf8_lossy(dest_bytes).into_owned();
                    let path_spec = format!("{} => {}", src, dest);
                    entries.push(NumstatEntry {
                        add,
                        del,
                        path: dest,
                        old_path: Some(src),
                        path_spec,
                    });
                } else {
                    let path = String::from_utf8_lossy(path_bytes).into_owned();
                    entries.push(NumstatEntry {
                        add,
                        del,
                        path: path.clone(),
                        old_path: None,
                        path_spec: path,
                    });
                }
            }
        }
    }
    entries
}

fn lookup_numstat(
    entries: &[NumstatEntry],
    path: &str,
    old_path: Option<&str>,
) -> (String, String) {
    if let Some(old) = old_path {
        for entry in entries {
            if let Some(entry_old) = &entry.old_path {
                if entry_old == old && entry.path == path {
                    return (entry.add.clone(), entry.del.clone());
                }
            }
        }
    } else {
        for entry in entries {
            if entry.path == path && entry.old_path.is_none() {
                return (entry.add.clone(), entry.del.clone());
            }
        }
    }
    ("0".to_string(), "0".to_string())
}

fn split_diff_into_hunks(diff: &str) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let mut current_header = String::new();
    let mut current_content = String::new();
    let mut current_bytes = 0;

    for line in diff.lines() {
        if line.starts_with("@@ ") {
            if !current_header.is_empty() {
                hunks.push(Hunk {
                    header: current_header.clone(),
                    content: current_content.clone(),
                    bytes: current_bytes,
                });
            }
            current_header = line.to_string();
            current_content = line.to_string() + "\n";
            current_bytes = line.len() + 1; // +1 for newline
        } else if !current_header.is_empty() {
            current_content.push_str(line);
            current_content.push('\n');
            current_bytes += line.len() + 1;
        }
    }

    if !current_header.is_empty() {
        hunks.push(Hunk {
            header: current_header,
            content: current_content,
            bytes: current_bytes,
        });
    }

    hunks
}

fn generate_dependency_summary(diff: &str) -> Vec<DependencyEntry> {
    let mut entries = Vec::new();
    let mut current_file = String::new();

    let re_import = Regex::new(r"(?i)^(import\s.*|from\s.*\simport\s.*|.*require\(.+\).*|use\s.*;|package\s.*|#include\s.*)$").unwrap();
    let re_export = Regex::new(r"^(export\s.*|pub\s.*)$").unwrap();
    let re_sig = Regex::new(r"^(?:(?:(?:export\s+|async\s+|pub\s+|static\s+)*function\s+[A-Za-z0-9_$]+\s*\()|(?:(?:export\s+|pub\s+)*(?:class|struct|interface|enum|impl|type)\s+[A-Za-z0-9_$]+)|(?:def\s+[A-Za-z0-9_]+\s*\()|(?:fn\s+[A-Za-z0-9_]+\s*\()|(?:func\s+[A-Za-z0-9_]+\s*\()|(?:[A-Za-z0-9_$]+\s+[A-Za-z0-9_$]+\s*\()|(?:[A-Za-z0-9_$]+\s*\(\s*\)\s*\{))").unwrap();
    let re_schema = Regex::new(r"(?i)^(alter\s+table|create\s+table|drop\s+table|create\s+index|drop\s+index|grant\s+|revoke\s+|add\s+column|drop\s+column)").unwrap();

    for line in diff.lines() {
        if let Some(stripped) = line.strip_prefix("+++ b/") {
            current_file = unquote_git_path(stripped);
            continue;
        } else if let Some(stripped) = line.strip_prefix("+++ \"b/") {
            let unquoted = unquote_git_path(&format!("\"{}", stripped));
            current_file = unquoted.strip_prefix("b/").unwrap_or(&unquoted).to_string();
            continue;
        } else if line.starts_with("+++ ") {
            current_file = String::new();
            continue;
        }

        if (line.starts_with('+') || line.starts_with('-'))
            && !line.starts_with("+++")
            && !line.starts_with("---")
        {
            if current_file.is_empty() {
                continue;
            }
            let change = if line.starts_with('+') {
                "added"
            } else {
                "removed"
            };
            let raw_content = &line[1..];
            let clean = raw_content.trim();
            if clean.is_empty() {
                continue;
            }

            let emit = |kind: &str, entries: &mut Vec<DependencyEntry>| {
                let safe_current = quote_git_path(&current_file);
                let detail = clean.replace('\t', " ");
                entries.push(DependencyEntry {
                    file: safe_current,
                    change: change.to_string(),
                    kind: kind.to_string(),
                    detail,
                });
            };

            if re_import.is_match(clean) {
                emit("import", &mut entries);
            }
            if re_export.is_match(clean) {
                emit("export", &mut entries);
            }
            if re_sig.is_match(clean) {
                let is_control_flow = {
                    let s = clean.trim();
                    s.starts_with("if ")
                        || s.starts_with("if(")
                        || s.starts_with("while ")
                        || s.starts_with("while(")
                        || s.starts_with("for ")
                        || s.starts_with("for(")
                        || s.starts_with("switch ")
                        || s.starts_with("switch(")
                        || s.starts_with("catch ")
                        || s.starts_with("catch(")
                        || s.starts_with("return ")
                        || s.starts_with("return(")
                        || s.starts_with("else ")
                        || s.starts_with("else{")
                        || s.starts_with("else {")
                        || s.starts_with("elif ")
                        || s.starts_with("elif(")
                        || s.starts_with("gsub(")
                        || s.starts_with("printf ")
                        || s.starts_with("printf(")
                        || s.starts_with("print ")
                        || s.starts_with("print(")
                };
                if !is_control_flow {
                    emit("signature", &mut entries);
                }
            }
            if re_schema.is_match(clean) {
                emit("schema", &mut entries);
            }
        }
    }
    entries
}

fn fail_no_repo() {
    println!("# Pre-Commit Review Diff Context\n");
    println!("repository: not a git repository");
    println!("diff_source: unavailable");
    println!("review_limits: no local repository access");
    println!();
    println!("No diff available. Stage your changes or provide a diff to review.");
    // Exit 0 intentionally: downstream consumers (Skill / reducer) expect
    // structured stdout even when no repository is found. A non-zero exit here
    // would cause the consumer to discard the diagnostic output. The output
    // content itself ("not a git repository") signals the error condition.
    std::process::exit(0);
}

fn emit_diff_limited(diff: &str, max_bytes: usize, inline_diff_bytes: usize) {
    let size = diff.len();
    println!("diff_bytes: {}", size);
    println!("max_diff_bytes: {}", max_bytes);
    println!("inline_diff_bytes: {}", inline_diff_bytes);
    println!("diff_output: inline");
    println!();
    println!("## Diff");
    println!("```diff");
    if max_bytes == 0 || size <= max_bytes {
        print!("{}", diff);
    } else {
        let mut byte_count = 0;
        for c in diff.chars() {
            let char_len = c.len_utf8();
            if byte_count + char_len > max_bytes {
                break;
            }
            print!("{}", c);
            byte_count += char_len;
        }
        println!("\n[diff truncated after {} bytes; inspect high-risk files with helper-emitted context commands before making safety claims]", max_bytes);
    }
    println!("```");
}

fn emit_diff_omitted(diff_size: usize, max_bytes: usize, inline_diff_bytes: usize, reason: &str) {
    println!("diff_bytes: {}", diff_size);
    println!("max_diff_bytes: {}", max_bytes);
    println!("inline_diff_bytes: {}", inline_diff_bytes);
    println!("diff_output: omitted");
    println!("diff_omitted_reason: {}", reason);
    println!();
    println!("## Diff Loading Instructions");
    println!("Global raw diff omitted from the gateway output so Review Plan JSON, Review Manifest JSONL, and Coverage Ledger Template remain visible to the model.");
    println!("Use helper-emitted context_command values for group/path loading; do not rebuild review scope with direct git commands.");
}

fn build_review_plan(
    manifest_units: &[ManifestUnit],
    groups: &[ReviewGroup],
    group_commands_map: &HashMap<String, Vec<String>>,
    mode: &str,
    self_exe: &str,
    group_target_bytes: usize,
    group_hard_bytes: usize,
) -> (ReviewPlan, usize, usize) {
    let mut plan_groups = Vec::new();
    let mut high_risk_units = 0;
    let mut split_required_groups = 0;

    for g in groups {
        let req_units: Vec<String> = manifest_units
            .iter()
            .filter(|u| u.group_id == g.group_id)
            .map(|u| u.unit_id.clone())
            .collect();

        let r_cmds_escaped = group_commands_map
            .get(&g.group_id)
            .cloned()
            .unwrap_or_default();
        let context_command = format!(
            "{} --source {} --group {}",
            shell_quote(self_exe),
            mode,
            shell_quote(&g.group_id)
        );

        let mut priority = 4;
        let mut action = "review".to_string();
        let mut split_source = "none".to_string();
        let mut notes = "review-complete-group-before-coverage-validation".to_string();

        if g.budget_status == "split-required" {
            action = "split".to_string();
            split_source = "Split Suggestions and Split Unit Diff Preview".to_string();
            notes = "replace-with-split-suggestions-before-review".to_string();
            priority = 1;
            split_required_groups += 1;
        } else if g.budget_status == "over-target" {
            if g.risk == "high" {
                priority = 2;
            } else if g.risk == "consistency" {
                priority = 3;
            }
        } else if g.risk == "high" {
            priority = 2;
        } else if g.risk == "consistency" {
            priority = 3;
        }

        if g.risk == "high" {
            high_risk_units += g.files.len();
        }

        plan_groups.push(PlanGroupEntry {
            group_id: g.group_id.clone(),
            risk: g.risk.clone(),
            reason: g.reason.clone(),
            priority,
            action,
            budget_status: g.budget_status.clone(),
            diff_bytes: g.diff_bytes,
            required_units: req_units,
            files: g.files.clone(),
            review_commands: r_cmds_escaped,
            context_mode: "group".to_string(),
            context_command,
            split_source,
            notes,
        });
    }

    plan_groups.sort_by(|a, b| {
        let p_cmp = a.priority.cmp(&b.priority);
        if p_cmp == std::cmp::Ordering::Equal {
            a.group_id.cmp(&b.group_id)
        } else {
            p_cmp
        }
    });

    (
        ReviewPlan {
            schema_version: 1,
            source: mode.to_string(),
            group_target_bytes,
            group_hard_bytes,
            manifest_units: manifest_units.len(),
            review_groups: groups.len(),
            split_required_groups,
            high_risk_units,
            context_mode: "group".to_string(),
            state_snapshot_section: "Reducer State Snapshot Template".to_string(),
            semantic_context_section: "Semantic Context Queries".to_string(),
            groups: plan_groups,
            coverage_validation: CoverageValidation {
                rule: "manifest_units - reviewed_units must be empty before claiming full review",
                blocking_rule: "high-risk or needs-split coverage gaps force DO_NOT_COMMIT",
            },
        },
        high_risk_units,
        split_required_groups,
    )
}

fn run_app() -> Result<(), AppError> {
    let args = CliArgs::parse()?;

    // Git top-level resolution
    let repo_root = match git_rev_parse_toplevel() {
        Ok(path) => path,
        Err(_) => {
            fail_no_repo(); // exits the process; never returns
            unreachable!();
        }
    };

    // Configuration from environment variables
    let max_diff_bytes = env::var("PRE_COMMIT_REVIEW_MAX_DIFF_BYTES")
        .ok()
        .and_then(|val| val.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_DIFF_BYTES);

    let inline_diff_bytes = env::var("PRE_COMMIT_REVIEW_INLINE_DIFF_BYTES")
        .ok()
        .and_then(|val| val.parse::<usize>().ok())
        .unwrap_or(DEFAULT_INLINE_DIFF_BYTES);

    let context_query_limit = env::var("PRE_COMMIT_REVIEW_CONTEXT_QUERY_LIMIT")
        .ok()
        .and_then(|val| val.parse::<usize>().ok())
        .unwrap_or(DEFAULT_CONTEXT_QUERY_LIMIT);

    let mut group_target_bytes = env::var("PRE_COMMIT_REVIEW_GROUP_TARGET_BYTES")
        .ok()
        .and_then(|val| val.parse::<usize>().ok())
        .unwrap_or(DEFAULT_GROUP_TARGET_BYTES);

    let group_hard_bytes = env::var("PRE_COMMIT_REVIEW_GROUP_HARD_BYTES")
        .ok()
        .and_then(|val| val.parse::<usize>().ok())
        .unwrap_or(DEFAULT_GROUP_HARD_BYTES);

    if group_target_bytes > group_hard_bytes {
        group_target_bytes = group_hard_bytes;
    }

    // Git state detection
    let branch = git_get_branch_name(&repo_root);
    let head_sha = git_get_head_sha(&repo_root);
    let head_oid = git_get_head_oid(&repo_root);
    let base = git_detect_base_branch(&repo_root);

    let staged_avail = git_has_staged_changes(&repo_root)?;
    let unstaged_avail = git_has_unstaged_changes(&repo_root)?;

    // Select base ref
    let mut selected_ref = String::new();
    let mut branch_mode_avail = false;
    let mut source_description = "none".to_string();
    let mut review_limit_note =
        "no diff found in staged, unstaged, or branch-vs-base comparisons".to_string();

    let remote_ref = format!("origin/{}", base);
    if run_command_string(
        &["git", "rev-parse", "--verify", "--quiet", &remote_ref],
        &repo_root,
    )
    .is_ok()
    {
        selected_ref = remote_ref;
        branch_mode_avail = git_has_diff_for_ref(&selected_ref, &repo_root)?;
        source_description = format!("branch vs base via git diff {}...HEAD", selected_ref);
        review_limit_note = format!("full diff available from local {}; remote freshness not verified because git fetch was not run", selected_ref);
    } else if run_command_string(
        &["git", "rev-parse", "--verify", "--quiet", &base],
        &repo_root,
    )
    .is_ok()
    {
        selected_ref = base.clone();
        branch_mode_avail = git_has_diff_for_ref(&selected_ref, &repo_root)?;
        source_description = format!("branch vs local base via git diff {}...HEAD", selected_ref);
        review_limit_note =
            "full local branch-vs-base diff available unless truncated by helper output limit"
                .to_string();
    }

    // Resolve active diff mode
    let mut mode = "none";

    if let Some(ref req_src) = args.source {
        if req_src == "staged" {
            mode = "staged";
            source_description = "staged changes via git diff --cached".to_string();
            review_limit_note =
                "full staged diff available unless truncated by helper output limit".to_string();
        } else if req_src == "unstaged" {
            mode = "unstaged";
            source_description = "unstaged changes via git diff".to_string();
            review_limit_note =
                "full unstaged diff available unless truncated by helper output limit".to_string();
        } else if req_src == "branch" && !selected_ref.is_empty() {
            mode = "branch";
        }
    } else {
        // Auto detection order
        if staged_avail {
            mode = "staged";
            source_description = "staged changes via git diff --cached".to_string();
            review_limit_note =
                "full staged diff available unless truncated by helper output limit".to_string();
        } else if unstaged_avail {
            mode = "unstaged";
            source_description = "unstaged changes via git diff".to_string();
            review_limit_note =
                "full unstaged diff available unless truncated by helper output limit".to_string();
        } else if branch_mode_avail {
            mode = "branch";
        }
    }

    let selected_diff_available = match mode {
        "staged" => staged_avail,
        "unstaged" => unstaged_avail,
        "branch" => branch_mode_avail,
        _ => false,
    };
    if args.control_plane && !selected_diff_available {
        mode = "none";
        selected_ref.clear();
    }

    // Staged and unstaged diffs do not use the detected branch base. Treating
    // that unrelated ref as part of the scope made fingerprints vary across
    // helper implementations (and when origin/* moved) despite identical
    // commit candidates.
    if mode == "staged" || mode == "unstaged" {
        selected_ref.clear();
    }

    let scope_identity = ScopeIdentity {
        source: mode,
        head: &head_oid,
        base: &base,
        selected_ref: &selected_ref,
    };

    if args.control_plane && mode == "none" {
        emit_authority_failure(
            &scope_identity,
            args.expect_scope.as_deref(),
            "",
            "",
            "no_diff_available",
        );
        return Ok(());
    }

    if args.path.is_some() && mode != "none" {
        review_limit_note =
            "file-specific diff for requested path; no other files included".to_string();
    }
    if args.group.is_some() && mode != "none" {
        review_limit_note =
            "group-specific diff for requested group; no other groups included".to_string();
    }

    // The fingerprint always covers the complete selected source, even for a
    // later --path/--group projection. This makes child review results safely
    // comparable with the authoritative parent manifest.
    let defer_output_for_authority = args.control_plane || args.expect_scope.is_some();
    let collection_start_fingerprint = if defer_output_for_authority {
        diff_fingerprint(mode, &selected_ref, &head_oid, None, None, &repo_root)?
    } else {
        String::new()
    };
    if let Some(ref expected) = args.expect_scope {
        if expected != &collection_start_fingerprint {
            emit_authority_failure(
                &scope_identity,
                Some(expected),
                &collection_start_fingerprint,
                &collection_start_fingerprint,
                "expected_scope_mismatch_before_collection",
            );
            return Ok(());
        }
    }

    let untracked_names = git_get_untracked_files(&repo_root);
    let mut unreviewed_note = "none".to_string();
    if mode == "staged" && unstaged_avail {
        unreviewed_note =
            "unstaged changes exist and were not reviewed as part of the staged commit candidate"
                .to_string();

        // Check for overlap
        let staged_list_bytes = run_command_bytes(
            &["git", "diff", "--cached", "--name-only", "-z", "--", "."],
            &repo_root,
        )?;
        let unstaged_list_bytes =
            run_command_bytes(&["git", "diff", "--name-only", "-z", "--", "."], &repo_root)?;

        let staged_list_out = String::from_utf8_lossy(&staged_list_bytes);
        let unstaged_list_out = String::from_utf8_lossy(&unstaged_list_bytes);

        let staged_set: HashSet<&str> = staged_list_out
            .split('\0')
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        let unstaged_set: HashSet<&str> = unstaged_list_out
            .split('\0')
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        let overlap: Vec<&str> = staged_set.intersection(&unstaged_set).cloned().collect();
        if !overlap.is_empty() {
            let mut overlap_sorted = overlap.clone();
            overlap_sorted.sort();
            unreviewed_note = format!(
                "unstaged changes touch files also staged for commit; actual working tree behavior may differ from reviewed commit candidate: {}",
                overlap_sorted.join(",")
            );
        }
    }

    if !untracked_names.is_empty() {
        if unreviewed_note == "none" {
            unreviewed_note = "untracked files exist but are not part of git diff; stage them or provide file content to review".to_string();
        } else {
            unreviewed_note = format!(
                "{}; untracked files exist but are not part of git diff",
                unreviewed_note
            );
        }
    }

    // Executable path for context commands
    let self_exe = env::var("PRE_COMMIT_REVIEW_HELPER_PATH").unwrap_or_else(|_| {
        env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("collect_diff_context"))
            .to_string_lossy()
            .to_string()
    });

    // 1. Gather all name-status changes globally
    let global_name_status_bytes = if mode != "none" {
        git_run_diff_bytes(
            mode,
            &selected_ref,
            &["--name-status", "-z"],
            None,
            &repo_root,
        )?
    } else {
        Vec::new()
    };
    let name_status_entries = parse_name_status_z(&global_name_status_bytes);

    // 2. Gather all numstat entries globally
    let global_numstat_bytes = if mode != "none" {
        git_run_diff_bytes(mode, &selected_ref, &["--numstat", "-z"], None, &repo_root)?
    } else {
        Vec::new()
    };
    let numstat_entries = parse_numstat_z(&global_numstat_bytes);

    // 3. Gather untracked files count/details
    let mut files_changed_str = "0 files, 0 insertions(+), 0 deletions(-)".to_string();
    if mode != "none" {
        let total_add: usize = numstat_entries
            .iter()
            .map(|e| e.add.parse::<usize>().unwrap_or(0))
            .sum();
        let total_del: usize = numstat_entries
            .iter()
            .map(|e| e.del.parse::<usize>().unwrap_or(0))
            .sum();
        files_changed_str = format!(
            "{} files, {} insertions(+), {} deletions(-)",
            name_status_entries.len(),
            total_add,
            total_del
        );
    }

    // 4. Calculate top-churn (top 5 files by total add+del)
    let mut churn_list = Vec::new();
    for entry in &numstat_entries {
        let add_val = entry.add.parse::<usize>().unwrap_or(0);
        let del_val = entry.del.parse::<usize>().unwrap_or(0);
        let total = add_val + del_val;
        churn_list.push((total, entry.path_spec.clone(), add_val, del_val));
    }
    churn_list.sort_by(|a, b| {
        let cmp = b.0.cmp(&a.0);
        if cmp == std::cmp::Ordering::Equal {
            b.1.cmp(&a.1)
        } else {
            cmp
        }
    }); // descending
    let top_churn_entries: Vec<String> = churn_list
        .iter()
        .take(5)
        .map(|item| format!("{} (+{}/-{})", quote_git_path(&item.1), item.2, item.3))
        .collect();
    let top_churn_files = if top_churn_entries.is_empty() {
        "none".to_string()
    } else {
        top_churn_entries.join(", ")
    };

    // 5. Gather classifiers
    let path_risk_regexes = get_path_risk_regexes();
    let content_risk_regexes = get_content_risk_regexes();
    let generated_regexes = get_generated_regexes();
    let lockfile_regex = get_lockfile_regex();

    // Custom regexes
    let custom_risk_paths = load_custom_regexes(
        Path::new(&repo_root)
            .join(".pre-commit-review/risk-paths")
            .as_path(),
    );
    let custom_risk_content = load_custom_regexes(
        Path::new(&repo_root)
            .join(".pre-commit-review/risk-content")
            .as_path(),
    );

    // Write global diff to memory to parse content risk and dependency summary (preserving raw byte size)
    let global_diff_bytes = if mode != "none" {
        git_run_diff_bytes(mode, &selected_ref, &[], None, &repo_root)?
    } else {
        Vec::new()
    };
    let global_diff = String::from_utf8_lossy(&global_diff_bytes).into_owned();

    // Calculate content-risk candidates
    let mut content_risk_files = HashSet::new();
    let mut current_file_in_diff = String::new();
    for line in global_diff.lines() {
        if let Some(stripped) = line.strip_prefix("+++ b/") {
            current_file_in_diff = unquote_git_path(stripped);
            continue;
        } else if let Some(stripped) = line.strip_prefix("+++ \"b/") {
            let unquoted = unquote_git_path(&format!("\"{}", stripped));
            current_file_in_diff = unquoted.strip_prefix("b/").unwrap_or(&unquoted).to_string();
            continue;
        } else if line.starts_with("+++ ") {
            current_file_in_diff = String::new();
            continue;
        }
        if (line.starts_with('+') || line.starts_with('-'))
            && !line.starts_with("+++")
            && !line.starts_with("---")
        {
            if current_file_in_diff.is_empty() {
                continue;
            }
            let raw_content = &line[1..];
            let lower_line = raw_content.to_lowercase();

            // Standard risk content regexes
            let mut is_risk = false;
            for re in content_risk_regexes {
                if re.is_match(&lower_line) || re.is_match(raw_content) {
                    is_risk = true;
                    break;
                }
            }
            // Custom risk content regexes
            if !is_risk {
                for re in &custom_risk_content {
                    if re.is_match(raw_content) {
                        is_risk = true;
                        break;
                    }
                }
            }
            if is_risk {
                content_risk_files.insert(current_file_in_diff.clone());
            }
        }
    }
    let mut content_risk_vec_raw: Vec<String> = content_risk_files.into_iter().collect();
    content_risk_vec_raw.sort();

    // Map files to path risk status
    let mut path_risk_files_raw = Vec::new();
    let mut generated_files_list_raw = Vec::new();
    let mut lock_files_list_raw = Vec::new();
    let mut high_risk_candidates_set_raw = HashSet::new();

    for entry in &name_status_entries {
        let path = &entry.path;

        // Path risk check
        let mut is_path_risk = false;
        for re in path_risk_regexes {
            if re.is_match(path) {
                is_path_risk = true;
                break;
            }
        }
        if !is_path_risk {
            for re in &custom_risk_paths {
                if re.is_match(path) {
                    is_path_risk = true;
                    break;
                }
            }
        }
        if is_path_risk {
            path_risk_files_raw.push(path.clone());
            high_risk_candidates_set_raw.insert(path.clone());
        }

        // Content risk also promotes to high-risk candidate
        if content_risk_vec_raw.contains(path) {
            high_risk_candidates_set_raw.insert(path.clone());
        }

        // Generated check
        let mut is_gen = false;
        for re in generated_regexes {
            if re.is_match(path) {
                is_gen = true;
                break;
            }
        }
        if is_gen {
            generated_files_list_raw.push(path.clone());
        }

        // Lockfile check
        if lockfile_regex.is_match(path) {
            lock_files_list_raw.push(path.clone());
        }
    }

    path_risk_files_raw.sort();
    generated_files_list_raw.sort();
    lock_files_list_raw.sort();

    let mut high_risk_candidates_vec_raw: Vec<String> =
        high_risk_candidates_set_raw.into_iter().collect();
    high_risk_candidates_vec_raw.sort();

    // Create display quoted lists
    let _path_risk_files: Vec<String> = path_risk_files_raw
        .iter()
        .map(|p| quote_git_path(p))
        .collect();
    let generated_files_list: Vec<String> = generated_files_list_raw
        .iter()
        .map(|p| quote_git_path(p))
        .collect();
    let lock_files_list: Vec<String> = lock_files_list_raw
        .iter()
        .map(|p| quote_git_path(p))
        .collect();
    let mut high_risk_candidates_vec: Vec<String> = high_risk_candidates_vec_raw
        .iter()
        .map(|p| quote_git_path(p))
        .collect();
    high_risk_candidates_vec.sort();
    let mut content_risk_vec: Vec<String> = content_risk_vec_raw
        .iter()
        .map(|p| quote_git_path(p))
        .collect();
    content_risk_vec.sort();

    let high_risk_candidates = if high_risk_candidates_vec.is_empty() {
        "none".to_string()
    } else {
        high_risk_candidates_vec.join(", ")
    };
    let content_risk_candidates = if content_risk_vec.is_empty() {
        "none".to_string()
    } else {
        content_risk_vec.join(", ")
    };
    let generated_like_files = if generated_files_list.is_empty() {
        "none".to_string()
    } else {
        generated_files_list.join(", ")
    };
    let lock_files = if lock_files_list.is_empty() {
        "none".to_string()
    } else {
        lock_files_list.join(", ")
    };

    // Calculate truncation metadata (based on accurate raw byte size)
    let diff_size = global_diff_bytes.len();
    if args.path.is_none() && max_diff_bytes != 0 && diff_size > max_diff_bytes {
        review_limit_note = "partial diff output; inspect file list and prioritize risky files before making safety claims".to_string();
    }
    let (diff_output_decision, diff_omitted_reason) = if diff_size == 0 {
        ("omitted".to_string(), "no diff available".to_string())
    } else {
        match args.include_diff.as_str() {
            "always" => ("inline".to_string(), "none".to_string()),
            "never" => ("omitted".to_string(), "plan-only mode".to_string()),
            "auto" => {
                if inline_diff_bytes == 0 || diff_size <= inline_diff_bytes {
                    ("inline".to_string(), "none".to_string())
                } else {
                    (
                        "omitted".to_string(),
                        format!(
                            "global diff exceeds inline budget ({} > {})",
                            diff_size, inline_diff_bytes
                        ),
                    )
                }
            }
            other => (
                "omitted".to_string(),
                format!("invalid include-diff mode coerced to plan-only: {}", other),
            ),
        }
    };

    // Path responses are bounded to the requested unit. Scoped responses are
    // emitted only after the full-scope end check, so cache every Git-derived
    // projection beforehand to avoid post-check snapshot mixing.
    let requested_path_raw = args.path.as_deref().map(unquote_git_path);
    let requested_path_diff_bytes = if let Some(ref raw_path) = requested_path_raw {
        git_run_diff_bytes(mode, &selected_ref, &[], Some(raw_path), &repo_root)?
    } else {
        Vec::new()
    };
    let requested_path_name_status = if let Some(ref raw_path) = requested_path_raw {
        parse_name_status_z(&git_run_diff_bytes(
            mode,
            &selected_ref,
            &["--name-status", "-z"],
            Some(raw_path),
            &repo_root,
        )?)
    } else {
        Vec::new()
    };
    let requested_path_numstat = if let Some(ref raw_path) = requested_path_raw {
        parse_numstat_z(&git_run_diff_bytes(
            mode,
            &selected_ref,
            &["--numstat", "-z"],
            Some(raw_path),
            &repo_root,
        )?)
    } else {
        Vec::new()
    };
    let scoped_path_status = if args.expect_scope.is_some() {
        if let Some(ref raw_path) = requested_path_raw {
            run_command_string(&["git", "status", "--short", "--", raw_path], &repo_root)?
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    let requested_path_stat = if let Some(ref raw_path) = requested_path_raw {
        git_run_diff_string(mode, &selected_ref, &["--stat"], Some(raw_path), &repo_root)?
    } else {
        String::new()
    };
    let path_files_changed = if args.path.is_some() {
        let additions: usize = requested_path_numstat
            .iter()
            .map(|entry| entry.add.parse::<usize>().unwrap_or(0))
            .sum();
        let deletions: usize = requested_path_numstat
            .iter()
            .map(|entry| entry.del.parse::<usize>().unwrap_or(0))
            .sum();
        format!(
            "{} files, {} insertions(+), {} deletions(-)",
            requested_path_name_status.len(),
            additions,
            deletions
        )
    } else {
        files_changed_str.clone()
    };
    let requested_path_display = requested_path_raw.as_deref().map(quote_git_path);
    let path_candidate = |paths: &[String]| -> String {
        match (&requested_path_raw, &requested_path_display) {
            (Some(raw), Some(display)) if paths.contains(raw) => display.clone(),
            _ => "none".to_string(),
        }
    };
    let path_high_risk_candidates = path_candidate(&high_risk_candidates_vec_raw);
    let path_content_risk_candidates = path_candidate(&content_risk_vec_raw);
    let path_generated_like_files = path_candidate(&generated_files_list_raw);
    let path_lock_files = path_candidate(&lock_files_list_raw);
    let path_top_churn_files =
        if let (Some(raw), Some(display)) = (&requested_path_raw, &requested_path_display) {
            let (add, del) = lookup_numstat(&requested_path_numstat, raw, None);
            if requested_path_numstat.is_empty() {
                "none".to_string()
            } else {
                format!("{} (+{}/-{})", display, add, del)
            }
        } else {
            top_churn_files.clone()
        };
    let header_diff_size = if args.path.is_some() {
        requested_path_diff_bytes.len()
    } else {
        diff_size
    };
    let header_diff_truncated = if max_diff_bytes != 0 && header_diff_size > max_diff_bytes {
        "yes"
    } else {
        "no"
    };
    if args.path.is_some() && header_diff_truncated == "yes" {
        review_limit_note =
            "partial requested file diff output; rerun with a larger bounded limit before claiming file coverage"
                .to_string();
    }
    let (header_diff_output, header_diff_omitted_reason) = if args.path.is_some() {
        if header_diff_size == 0 {
            ("omitted", "no diff available")
        } else {
            ("inline", "none")
        }
    } else {
        (diff_output_decision.as_str(), diff_omitted_reason.as_str())
    };

    let emit_context_header = || -> Result<(), AppError> {
        println!("# Pre-Commit Review Diff Context\n");
        println!("repository: {}", repo_root);
        println!(
            "branch: {}",
            if branch.is_empty() {
                "detached-or-unknown"
            } else {
                &branch
            }
        );
        println!("head: {}", head_sha);
        println!("detected_base: {}", base);
        println!("diff_source: {}", source_description);
        if let Some(ref p) = args.path {
            println!("requested_path: {}", p);
        }
        if let Some(ref g) = args.group {
            println!("requested_group: {}", g);
        }
        if let Some(ref s) = args.source {
            println!("requested_source: {}", s);
        }
        if args.expect_scope.is_some() {
            println!("scope_fingerprint: {}", collection_start_fingerprint);
        }
        println!("review_limits: {}", review_limit_note);
        println!("diff_truncated: {}", header_diff_truncated);
        println!("inline_diff_bytes: {}", inline_diff_bytes);
        println!("diff_output: {}", header_diff_output);
        if header_diff_output == "omitted" {
            println!("diff_omitted_reason: {}", header_diff_omitted_reason);
        }
        println!("diff_loading: use helper-emitted context_command values; do not rebuild review scope with direct git commands");
        println!("group_target_bytes: {}", group_target_bytes);
        println!("group_hard_bytes: {}", group_hard_bytes);
        println!("files_changed: {}", path_files_changed);
        println!(
            "high_risk_candidates: {}",
            if args.path.is_some() {
                &path_high_risk_candidates
            } else {
                &high_risk_candidates
            }
        );
        println!(
            "content_risk_candidates: {}",
            if args.path.is_some() {
                &path_content_risk_candidates
            } else {
                &content_risk_candidates
            }
        );
        println!(
            "generated_like_files: {}",
            if args.path.is_some() {
                &path_generated_like_files
            } else {
                &generated_like_files
            }
        );
        println!(
            "lock_files: {}",
            if args.path.is_some() {
                &path_lock_files
            } else {
                &lock_files
            }
        );
        println!("top_churn_files: {}", path_top_churn_files);
        println!(
            "staged_changes: {}",
            if staged_avail { "yes" } else { "no" }
        );
        println!(
            "unstaged_changes: {}",
            if unstaged_avail { "yes" } else { "no" }
        );
        println!(
            "untracked_files: {}",
            if !untracked_names.is_empty() {
                "yes"
            } else {
                "no"
            }
        );
        println!("unreviewed_changes: {}", unreviewed_note);
        println!();

        println!("## Status");
        if let Some(ref p) = args.path {
            if args.expect_scope.is_some() {
                print!("{}", scoped_path_status);
            } else {
                let raw_path = unquote_git_path(p);
                let status_out =
                    run_command_string(&["git", "status", "--short", "--", &raw_path], &repo_root)?;
                print!("{}", status_out);
            }
        } else if args.group.is_some() {
            println!("group-specific status is emitted after group resolution");
        } else {
            let status_out = run_command_string(&["git", "status", "--short"], &repo_root)?;
            print!("{}", status_out);
        }
        println!();
        Ok(())
    };

    if !defer_output_for_authority {
        emit_context_header()?;
    }

    if mode == "none" && !defer_output_for_authority {
        println!("No diff available. Stage your changes or provide a diff to review.");
        return Ok(());
    }

    // 7. Resolve Manifest Units
    let mut manifest_units = Vec::new();
    let mut group_sizes: HashMap<String, usize> = HashMap::new();
    let mut group_files_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut group_risk_map: HashMap<String, String> = HashMap::new();
    let mut group_reason_map: HashMap<String, String> = HashMap::new();
    let mut group_commands_map: HashMap<String, Vec<String>> = HashMap::new();
    // Keep the exact bytes used to size and fingerprint each manifest unit.
    // Scoped group/path projections must emit this cache after the full-scope
    // end fingerprint succeeds; re-running git diff afterwards would reopen a
    // TOCTOU window and could mix a newer index into an authoritative review.
    let mut unit_diff_cache: HashMap<String, Vec<u8>> = HashMap::new();

    for entry in &name_status_entries {
        let path = &entry.path;
        let old_path = entry.old_path.as_deref();

        let display_path = quote_git_path(path);

        let (add, del) = lookup_numstat(&numstat_entries, path, old_path);

        // Single file diff byte size (calculating raw bytes to prevent UTF-8 loss)
        let path_is_requested = requested_path_raw.as_deref() == Some(path.as_str());
        let file_diff_bytes_vec = if path_is_requested {
            requested_path_diff_bytes.clone()
        } else {
            git_run_diff_bytes(mode, &selected_ref, &[], Some(path), &repo_root)?
        };
        let file_diff_bytes = file_diff_bytes_vec.len();
        // Select the raw path while retaining the display-quoted manifest
        // token as the cross-implementation fingerprint identity.
        let content_fingerprint = diff_fingerprint_from_bytes(
            mode,
            &selected_ref,
            &head_oid,
            Some(&display_path),
            &file_diff_bytes_vec,
            &repo_root,
        )?;
        let top_component = group_component_for_path(&display_path);
        let safe_component = safe_group_component(&top_component);

        let mut risk_tags = Vec::new();
        let group_id;

        // Group assignment logic
        if high_risk_candidates_vec_raw.contains(path) {
            risk_tags.push("high-risk".to_string());
            group_id = format!("high-risk-{}", safe_component);
            if !group_risk_map.contains_key(&group_id) {
                group_risk_map.insert(group_id.clone(), "high".to_string());
                group_reason_map.insert(group_id.clone(), "path-or-content-risk".to_string());
            }
        } else if generated_files_list_raw.contains(path) {
            risk_tags.push("generated-like".to_string());
            group_id = format!("consistency-{}", safe_component);
            if group_risk_map.get(&group_id).map(|s| s.as_str()) != Some("high") {
                group_risk_map.insert(group_id.clone(), "consistency".to_string());
                group_reason_map.insert(group_id.clone(), "generated-like".to_string());
            }
        } else if lock_files_list_raw.contains(path) {
            risk_tags.push("lockfile".to_string());
            group_id = "consistency-lockfiles".to_string();
            if group_risk_map.get(&group_id).map(|s| s.as_str()) != Some("high") {
                group_risk_map.insert(group_id.clone(), "consistency".to_string());
                group_reason_map.insert(group_id.clone(), "lockfile".to_string());
            }
        } else {
            risk_tags.push("medium".to_string());
            group_id = format!("module-{}", safe_component);
            if !group_risk_map.contains_key(&group_id) {
                group_risk_map.insert(group_id.clone(), "medium".to_string());
                group_reason_map.insert(group_id.clone(), "module".to_string());
            }
        }

        // Commands operate on the raw path. The manifest keeps Git's quoted
        // display token as its stable identity, but passing that token back to
        // Git would look for a filename containing literal quote characters.
        let quoted_path = shell_quote(path);
        let review_command = match mode {
            "staged" => format!("git diff --cached --no-textconv -- {}", quoted_path),
            "unstaged" => format!("git diff --no-textconv -- {}", quoted_path),
            "branch" => {
                let ref_expr = format!("{}...HEAD", selected_ref);
                format!(
                    "git diff --no-textconv {} -- {}",
                    shell_quote(&ref_expr),
                    quoted_path
                )
            }
            _ => "unavailable".to_string(),
        };

        let context_command = format!(
            "{} --source {} --path {}",
            shell_quote(&self_exe),
            mode,
            quoted_path
        );

        let requested_path_matches = path_is_requested;
        let requested_group_matches = args.group.as_deref() == Some(group_id.as_str());
        if requested_path_matches || requested_group_matches {
            unit_diff_cache.insert(display_path.clone(), file_diff_bytes_vec);
        }

        // Update group properties
        *group_sizes.entry(group_id.clone()).or_insert(0) += file_diff_bytes;
        group_files_map
            .entry(group_id.clone())
            .or_default()
            .push(display_path.clone());
        group_commands_map
            .entry(group_id.clone())
            .or_default()
            .push(review_command.clone());

        manifest_units.push(ManifestUnit {
            unit_id: format!("file:{}", display_path),
            file_path: display_path.clone(),
            status: entry.status.clone(),
            additions: add.parse::<usize>().unwrap_or(0),
            deletions: del.parse::<usize>().unwrap_or(0),
            diff_bytes: file_diff_bytes,
            content_fingerprint,
            risk_tags,
            group_id,
            review_command,
            context_command,
        });
    }

    // Resolves Group structures
    let mut groups = Vec::new();
    for (group_id, files) in &group_files_map {
        let size = group_sizes.get(group_id).cloned().unwrap_or(0);
        let budget_status = if size > group_hard_bytes {
            "split-required".to_string()
        } else if size > group_target_bytes {
            "over-target".to_string()
        } else {
            "ok".to_string()
        };

        groups.push(ReviewGroup {
            group_id: group_id.clone(),
            risk: group_risk_map
                .get(group_id)
                .cloned()
                .unwrap_or_else(|| "medium".to_string()),
            reason: group_reason_map
                .get(group_id)
                .cloned()
                .unwrap_or_else(|| "module".to_string()),
            diff_bytes: size,
            files: files.clone(),
            budget_status,
        });
    }
    // Sort groups deterministically by group_id
    groups.sort_by(|a, b| a.group_id.cmp(&b.group_id));

    if defer_output_for_authority {
        let collection_end_fingerprint =
            diff_fingerprint(mode, &selected_ref, &head_oid, None, None, &repo_root)?;
        if collection_end_fingerprint != collection_start_fingerprint {
            emit_authority_failure(
                &scope_identity,
                args.expect_scope.as_deref(),
                &collection_start_fingerprint,
                &collection_end_fingerprint,
                "scope_changed_during_collection",
            );
            return Ok(());
        }
        if let Some(ref expected) = args.expect_scope {
            if expected != &collection_end_fingerprint {
                emit_authority_failure(
                    &scope_identity,
                    Some(expected),
                    &collection_start_fingerprint,
                    &collection_end_fingerprint,
                    "expected_scope_mismatch_after_collection",
                );
                return Ok(());
            }
        }

        if args.control_plane {
            emit_control_plane(
                &scope_identity,
                &collection_end_fingerprint,
                &self_exe,
                &manifest_units,
                &groups,
            );
            return Ok(());
        }

        emit_context_header()?;
        if mode == "none" {
            println!("No diff available. Stage your changes or provide a diff to review.");
            return Ok(());
        }
    }

    // Handle REQUEST_GROUP early exit
    if let Some(ref req_grp) = args.group {
        println!();
        emit_requested_group(
            req_grp,
            &manifest_units,
            &groups,
            &unit_diff_cache,
            mode,
            max_diff_bytes,
            inline_diff_bytes,
        )?;
        return Ok(());
    }

    // Output stats and file lists for the main review mode
    println!("## Diff Stat");
    let diff_stat_out = if args.path.is_some() {
        requested_path_stat
    } else {
        git_run_diff_string(mode, &selected_ref, &["--stat"], None, &repo_root)?
    };
    print!("{}", diff_stat_out);
    println!();

    println!("## File List");
    let output_name_status = if args.path.is_some() {
        &requested_path_name_status
    } else {
        &name_status_entries
    };
    for entry in output_name_status {
        let disp_path = quote_git_path(&entry.path);
        if let Some(ref old) = entry.old_path {
            let disp_old = quote_git_path(old);
            println!("{}\t{}\t{}", entry.status, disp_old, disp_path);
        } else {
            println!("{}\t{}", entry.status, disp_path);
        }
    }
    println!();

    println!("## Numstat");
    let output_numstat = if args.path.is_some() {
        &requested_path_numstat
    } else {
        &numstat_entries
    };
    for entry in output_numstat {
        let disp_spec = quote_git_path(&entry.path_spec);
        println!("{}\t{}\t{}", entry.add, entry.del, disp_spec);
    }
    println!();

    if let Some(ref req_path) = args.path {
        println!();
        println!("## Requested File Diff");
        println!("path: {}", req_path);

        let raw_req_path = unquote_git_path(req_path);
        let unit = manifest_units
            .iter()
            .find(|u| u.file_path == *req_path || unquote_git_path(&u.file_path) == raw_req_path);
        let r_cmd = unit.map(|u| u.review_command.clone()).unwrap_or_else(|| {
            let quoted_path = shell_quote(&raw_req_path);
            match mode {
                "staged" => format!("git diff --cached --no-textconv -- {}", quoted_path),
                "unstaged" => format!("git diff --no-textconv -- {}", quoted_path),
                "branch" => {
                    let ref_expr = format!("{}...HEAD", selected_ref);
                    format!(
                        "git diff --no-textconv {} -- {}",
                        shell_quote(&ref_expr),
                        quoted_path
                    )
                }
                _ => "unavailable".to_string(),
            }
        });
        let c_cmd = unit.map(|u| u.context_command.clone()).unwrap_or_else(|| {
            format!(
                "{} --source {} --path {}",
                shell_quote(&self_exe),
                mode,
                shell_quote(&raw_req_path)
            )
        });

        println!("review_command: {}", r_cmd);
        println!("context_command: {}", c_cmd);

        let cache_key = unit.map(|u| u.file_path.as_str()).unwrap_or(req_path);
        let file_diff_bytes = unit_diff_cache.get(cache_key).cloned().unwrap_or_default();
        if file_diff_bytes.is_empty() {
            println!();
            println!("No diff available for requested path in the selected diff source.");
            return Ok(());
        }

        emit_diff_limited(
            &String::from_utf8_lossy(&file_diff_bytes),
            max_diff_bytes,
            inline_diff_bytes,
        );
        return Ok(());
    }

    let (plan, high_risk_units, split_required_groups) = build_review_plan(
        &manifest_units,
        &groups,
        &group_commands_map,
        mode,
        &self_exe,
        group_target_bytes,
        group_hard_bytes,
    );

    let compact_plan = diff_size > 0 && diff_output_decision == "omitted";

    if compact_plan {
        println!("## Review Manifest JSONL");
        for unit in &manifest_units {
            if let Ok(json) = serde_json::to_string(unit) {
                println!("{}", json);
            }
        }
        println!();

        println!("## Review Groups JSONL");
        for g in &groups {
            if let Ok(json) = serde_json::to_string(g) {
                println!("{}", json);
            }
        }
        println!();

        println!("## Review Plan JSON");
        println!("{}", serde_json::to_string(&plan).unwrap_or_default());
        println!();

        println!("## Split Suggestions");
        println!(
            "parent_group_id\tunit_id\tpath\tsplit_kind\tdiff_bytes\thunk_header\treview_command"
        );
        let mut emitted_split = false;
        for g in &groups {
            if g.budget_status == "split-required" {
                for f in &g.files {
                    let unit = match manifest_units.iter().find(|u| u.file_path == *f) {
                        Some(u) => u,
                        None => continue,
                    };
                    let raw_f = unquote_git_path(f);
                    let f_diff_bytes =
                        git_run_diff_bytes(mode, &selected_ref, &[], Some(&raw_f), &repo_root)?;
                    let f_diff = String::from_utf8_lossy(&f_diff_bytes);
                    let hunks = split_diff_into_hunks(&f_diff);
                    if hunks.is_empty() {
                        println!(
                            "{}\tfile:{}\t{}\tfile\t0\tnone\t{}",
                            sanitize_tsv_field(&g.group_id),
                            sanitize_tsv_field(f),
                            sanitize_tsv_field(f),
                            sanitize_tsv_field(&unit.review_command)
                        );
                    } else {
                        for (h_idx, hunk) in hunks.iter().enumerate() {
                            let clean_header = hunk.header.replace('\t', " ");
                            println!(
                                "{}\thunk:{}:{}\t{}\thunk\t{}\t{}\t{}",
                                sanitize_tsv_field(&g.group_id),
                                sanitize_tsv_field(f),
                                h_idx + 1,
                                sanitize_tsv_field(f),
                                hunk.bytes,
                                sanitize_tsv_field(&clean_header),
                                sanitize_tsv_field(&unit.review_command)
                            );
                        }
                    }
                    emitted_split = true;
                }
            }
        }
        if !emitted_split {
            println!("none\tnone\tnone\tnone\t0\tnone\tnone");
        }
        println!();

        println!("## Coverage Ledger Template");
        println!("unit_id\tgroup_id\tpath\tcoverage_status\tcoverage_mode\tnotes");
        for unit in &manifest_units {
            let is_split = groups
                .iter()
                .find(|g| g.group_id == unit.group_id)
                .map(|g| g.budget_status == "split-required")
                .unwrap_or(false);
            if is_split {
                println!(
                    "{}\t{}\t{}\tneeds-split\treplace-with-split-suggestions\tsplit-required group",
                    sanitize_tsv_field(&unit.unit_id),
                    sanitize_tsv_field(&unit.group_id),
                    sanitize_tsv_field(&unit.file_path)
                );
            } else {
                println!(
                    "{}\t{}\t{}\tpending\tfile-review\trecord group result before final verdict",
                    sanitize_tsv_field(&unit.unit_id),
                    sanitize_tsv_field(&unit.group_id),
                    sanitize_tsv_field(&unit.file_path)
                );
            }
        }
        println!();

        let mut coverage_gaps = Vec::new();
        let mut needs_split_units = Vec::new();
        let mut pending_units = Vec::new();
        for unit in &manifest_units {
            let is_split = groups
                .iter()
                .find(|g| g.group_id == unit.group_id)
                .map(|g| g.budget_status == "split-required")
                .unwrap_or(false);
            let status = if is_split {
                needs_split_units.push(unit.unit_id.clone());
                "needs-split"
            } else {
                "pending"
            };
            pending_units.push(unit.unit_id.clone());
            coverage_gaps.push(CoverageGap {
                unit_id: unit.unit_id.clone(),
                group_id: unit.group_id.clone(),
                risk_tags: unit.risk_tags.join(";"),
                coverage_status: status.to_string(),
            });
        }
        let reducer_state = ReducerState {
            schema_version: 1,
            state_kind: "reducer_state_snapshot",
            source: mode.to_string(),
            status: "pending_group_reviews",
            manifest_units: manifest_units.len(),
            review_groups: groups.len(),
            reviewed_units: vec![],
            pending_units,
            needs_split_units,
            group_results: vec![],
            coverage_gaps,
            finding_merge: FindingMerge {
                deduplicated_findings: vec![],
                blockers: vec![],
                notes: vec![],
            },
            dependency_checks: vec![],
            test_recommendations: vec![],
            final_verdict: "blocked_until_coverage_validation_passes",
            persistence_rule: "carry this compact state forward after each group result; update reviewed_units, pending_units, group_results, coverage_gaps, and finding_merge before reducer finalization",
        };
        println!("## Reducer State Snapshot Template");
        println!(
            "{}",
            serde_json::to_string(&reducer_state).unwrap_or_default()
        );
        println!();

        let needs_split_units_cnt = manifest_units
            .iter()
            .filter(|u| {
                groups
                    .iter()
                    .any(|g| g.group_id == u.group_id && g.budget_status == "split-required")
            })
            .count();
        println!("## Coverage Validation Checklist");
        println!("manifest_units: {}", manifest_units.len());
        println!("review_groups: {}", groups.len());
        println!("split_required_groups: {}", split_required_groups);
        println!("needs_split_units: {}", needs_split_units_cnt);
        println!("high_risk_units: {}", high_risk_units);
        println!("validation_rule: manifest_units - reviewed_units must be empty before claiming full review");
        println!("blocking_rule: high-risk or needs-split coverage gaps force DO_NOT_COMMIT");
        println!();
    } else {
        // Print Review Manifest (TSV) - protected with TSV sanitization
        println!("## Review Manifest");
        println!("unit_id\tpath\tstatus\tadditions\tdeletions\tdiff_bytes\trisk_tags\tgroup_id\treview_command\tcontext_command\tcontent_fingerprint");
        for unit in &manifest_units {
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                sanitize_tsv_field(&unit.unit_id),
                sanitize_tsv_field(&unit.file_path),
                sanitize_tsv_field(&unit.status),
                unit.additions,
                unit.deletions,
                unit.diff_bytes,
                sanitize_tsv_field(&unit.risk_tags.join(";")),
                sanitize_tsv_field(&unit.group_id),
                sanitize_tsv_field(&unit.review_command),
                sanitize_tsv_field(&unit.context_command),
                sanitize_tsv_field(&unit.content_fingerprint)
            );
        }
        println!();

        // Print Review Manifest JSONL
        println!("## Review Manifest JSONL");
        for unit in &manifest_units {
            if let Ok(json) = serde_json::to_string(unit) {
                println!("{}", json);
            }
        }
        println!();

        // Print Review Groups (TSV)
        println!("## Review Groups");
        println!("group_id\trisk\treason\tdiff_bytes\tfiles\tbudget_status");
        for g in &groups {
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                sanitize_tsv_field(&g.group_id),
                sanitize_tsv_field(&g.risk),
                sanitize_tsv_field(&g.reason),
                g.diff_bytes,
                sanitize_tsv_field(&g.files.join(";")),
                sanitize_tsv_field(&g.budget_status)
            );
        }
        println!();

        // Print Review Groups JSONL
        println!("## Review Groups JSONL");
        for g in &groups {
            if let Ok(json) = serde_json::to_string(g) {
                println!("{}", json);
            }
        }
        println!();

        // Build and emit Review Plan JSON
        let mut plan_groups = Vec::new();
        let mut high_risk_units = 0;
        let mut split_required_groups = 0;

        for g in &groups {
            let req_units: Vec<String> = manifest_units
                .iter()
                .filter(|u| u.group_id == g.group_id)
                .map(|u| u.unit_id.clone())
                .collect();

            let files_escaped: Vec<String> = g.files.clone();
            let r_cmds_escaped = group_commands_map
                .get(&g.group_id)
                .cloned()
                .unwrap_or_default();
            let context_command = format!(
                "{} --source {} --group {}",
                shell_quote(&self_exe),
                mode,
                shell_quote(&g.group_id)
            );

            let mut priority = 4;
            let mut action = "review".to_string();
            let mut split_source = "none".to_string();
            let mut notes = "review-complete-group-before-coverage-validation".to_string();

            if g.budget_status == "split-required" {
                action = "split".to_string();
                split_source = "Split Suggestions and Split Unit Diff Preview".to_string();
                notes = "replace-with-split-suggestions-before-review".to_string();
                priority = 1;
                split_required_groups += 1;
            } else if g.budget_status == "over-target" {
                if g.risk == "high" {
                    priority = 2;
                } else if g.risk == "consistency" {
                    priority = 3;
                }
            } else if g.risk == "high" {
                priority = 2;
            } else if g.risk == "consistency" {
                priority = 3;
            }

            if g.risk == "high" {
                high_risk_units += g.files.len();
            }

            plan_groups.push(PlanGroupEntry {
                group_id: g.group_id.clone(),
                risk: g.risk.clone(),
                reason: g.reason.clone(),
                priority,
                action,
                budget_status: g.budget_status.clone(),
                diff_bytes: g.diff_bytes,
                required_units: req_units,
                files: files_escaped,
                review_commands: r_cmds_escaped,
                context_mode: "group".to_string(),
                context_command,
                split_source,
                notes,
            });
        }

        // Sort plan groups by priority ascending, then group_id
        plan_groups.sort_by(|a, b| {
            let p_cmp = a.priority.cmp(&b.priority);
            if p_cmp == std::cmp::Ordering::Equal {
                a.group_id.cmp(&b.group_id)
            } else {
                p_cmp
            }
        });

        let plan = ReviewPlan {
            schema_version: 1,
            source: mode.to_string(),
            group_target_bytes,
            group_hard_bytes,
            manifest_units: manifest_units.len(),
            review_groups: groups.len(),
            split_required_groups,
            high_risk_units,
            context_mode: "group".to_string(),
            state_snapshot_section: "Reducer State Snapshot Template".to_string(),
            semantic_context_section: "Semantic Context Queries".to_string(),
            groups: plan_groups,
            coverage_validation: CoverageValidation {
                rule: "manifest_units - reviewed_units must be empty before claiming full review",
                blocking_rule: "high-risk or needs-split coverage gaps force DO_NOT_COMMIT",
            },
        };

        println!("## Review Plan JSON");
        println!("{}", serde_json::to_string(&plan).unwrap_or_default());
        println!();

        // 8. Generate and emit Split Suggestions
        let mut split_files = Vec::new();
        for g in &groups {
            if g.budget_status == "split-required" {
                for f in &g.files {
                    let r_cmd = manifest_units
                        .iter()
                        .find(|u| u.file_path == *f)
                        .map(|u| u.review_command.clone())
                        .unwrap_or_default();
                    split_files.push((g.group_id.clone(), f.clone(), r_cmd));
                }
            }
        }

        println!("## Split Suggestions");
        println!(
            "parent_group_id\tunit_id\tpath\tsplit_kind\tdiff_bytes\thunk_header\treview_command"
        );
        if !split_files.is_empty() {
            for (parent_group, path, r_cmd) in &split_files {
                let raw_path = unquote_git_path(path);
                let f_diff_bytes =
                    git_run_diff_bytes(mode, &selected_ref, &[], Some(&raw_path), &repo_root)?;
                let f_diff = String::from_utf8_lossy(&f_diff_bytes);
                let hunks = split_diff_into_hunks(&f_diff);
                if hunks.is_empty() {
                    println!(
                        "{}\tfile:{}\t{}\tfile\t0\tnone\t{}",
                        sanitize_tsv_field(parent_group),
                        sanitize_tsv_field(path),
                        sanitize_tsv_field(path),
                        sanitize_tsv_field(r_cmd)
                    );
                } else {
                    for (h_idx, hunk) in hunks.iter().enumerate() {
                        let clean_header = hunk.header.replace('\t', " ");
                        println!(
                            "{}\thunk:{}:{}\t{}\thunk\t{}\t{}\t{}",
                            sanitize_tsv_field(parent_group),
                            sanitize_tsv_field(path),
                            h_idx + 1,
                            sanitize_tsv_field(path),
                            hunk.bytes,
                            sanitize_tsv_field(&clean_header),
                            sanitize_tsv_field(r_cmd)
                        );
                    }
                }
            }
        } else {
            println!("none\tnone\tnone\tnone\t0\tnone\tnone");
        }
        println!();

        // Emit Split Unit Diff Previews
        println!("## Split Unit Diff Preview");
        if !split_files.is_empty() {
            for (parent_group, path, _) in &split_files {
                let raw_path = unquote_git_path(path);
                let f_diff_bytes =
                    git_run_diff_bytes(mode, &selected_ref, &[], Some(&raw_path), &repo_root)?;
                let f_diff = String::from_utf8_lossy(&f_diff_bytes);
                let hunks = split_diff_into_hunks(&f_diff);
                for (h_idx, hunk) in hunks.iter().enumerate() {
                    println!("unit_id: hunk:{}:{}", path, h_idx + 1);
                    println!("parent_group_id: {}", parent_group);
                    println!("```diff");
                    print!("{}", hunk.content);
                    println!("```");
                }
            }
        } else {
            println!("none");
        }
        println!();

        // Coverage Ledger Template
        println!("## Coverage Ledger Template");
        println!("unit_id\tgroup_id\tpath\tcoverage_status\tcoverage_mode\tnotes");
        for unit in &manifest_units {
            let is_split = groups
                .iter()
                .find(|g| g.group_id == unit.group_id)
                .map(|g| g.budget_status == "split-required")
                .unwrap_or(false);
            if is_split {
                println!(
                    "{}\t{}\t{}\tneeds-split\treplace-with-split-suggestions\tsplit-required group",
                    sanitize_tsv_field(&unit.unit_id),
                    sanitize_tsv_field(&unit.group_id),
                    sanitize_tsv_field(&unit.file_path)
                );
            } else {
                println!(
                    "{}\t{}\t{}\tpending\tfile-review\trecord group result before final verdict",
                    sanitize_tsv_field(&unit.unit_id),
                    sanitize_tsv_field(&unit.group_id),
                    sanitize_tsv_field(&unit.file_path)
                );
            }
        }
        println!();

        // Group Review Result Template
        println!("## Group Review Result Template");
        for g in &groups {
            let req_units: Vec<String> = manifest_units
                .iter()
                .filter(|u| u.group_id == g.group_id)
                .map(|u| u.unit_id.clone())
                .collect();
            let coverage = if g.budget_status == "split-required" {
                "needs-split"
            } else {
                "pending"
            };
            let gr_json = serde_json::json!({
                "group_id": g.group_id,
                "required_units": req_units,
                "reviewed_units": Vec::<serde_json::Value>::new(),
                "coverage": coverage,
                "findings": Vec::<serde_json::Value>::new(),
                "contract_changes": Vec::<serde_json::Value>::new(),
                "dependencies_to_check": Vec::<serde_json::Value>::new(),
                "tests_recommended": Vec::<serde_json::Value>::new(),
            });
            if let Ok(json_str) = serde_json::to_string(&gr_json) {
                println!("{}", json_str);
            }
        }
        println!();

        // Reducer State Snapshot Template
        let mut coverage_gaps = Vec::new();
        let mut needs_split_units = Vec::new();
        let mut pending_units = Vec::new();

        for unit in &manifest_units {
            let is_split = groups
                .iter()
                .find(|g| g.group_id == unit.group_id)
                .map(|g| g.budget_status == "split-required")
                .unwrap_or(false);

            let status = if is_split {
                needs_split_units.push(unit.unit_id.clone());
                "needs-split"
            } else {
                "pending"
            };
            pending_units.push(unit.unit_id.clone());

            let risk_tag_str = unit.risk_tags.join(";");
            coverage_gaps.push(CoverageGap {
                unit_id: unit.unit_id.clone(),
                group_id: unit.group_id.clone(),
                risk_tags: risk_tag_str,
                coverage_status: status.to_string(),
            });
        }

        let reducer_state = ReducerState {
        schema_version: 1,
        state_kind: "reducer_state_snapshot",
        source: mode.to_string(),
        status: "pending_group_reviews",
        manifest_units: manifest_units.len(),
        review_groups: groups.len(),
        reviewed_units: vec![],
        pending_units,
        needs_split_units,
        group_results: vec![],
        coverage_gaps,
        finding_merge: FindingMerge {
            deduplicated_findings: vec![],
            blockers: vec![],
            notes: vec![],
        },
        dependency_checks: vec![],
        test_recommendations: vec![],
        final_verdict: "blocked_until_coverage_validation_passes",
        persistence_rule: "carry this compact state forward after each group result; update reviewed_units, pending_units, group_results, coverage_gaps, and finding_merge before reducer finalization",
    };

        println!("## Reducer State Snapshot Template");
        println!(
            "{}",
            serde_json::to_string(&reducer_state).unwrap_or_default()
        );
        println!();

        // Coverage Validation Checklist
        println!("## Coverage Validation Checklist");
        let needs_split_units_cnt = manifest_units
            .iter()
            .filter(|u| {
                groups
                    .iter()
                    .any(|g| g.group_id == u.group_id && g.budget_status == "split-required")
            })
            .count();

        println!("manifest_units: {}", manifest_units.len());
        println!("review_groups: {}", groups.len());
        println!("split_required_groups: {}", split_required_groups);
        println!("needs_split_units: {}", needs_split_units_cnt);
        println!("high_risk_units: {}", high_risk_units);
        println!("validation_rule: manifest_units - reviewed_units must be empty before claiming full review");
        println!("blocking_rule: high-risk or needs-split coverage gaps force DO_NOT_COMMIT");
        println!();

        // Full Review Execution Plan
        println!("## Full Review Execution Plan");
        println!("step\taction\tgroup_id\trisk\tbudget_status\tunits\tnotes");
        for (step_idx, entry) in plan.groups.iter().enumerate() {
            let req_units_raw: Vec<String> = manifest_units
                .iter()
                .filter(|u| u.group_id == entry.group_id)
                .map(|u| u.unit_id.clone())
                .collect();

            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                step_idx + 1,
                sanitize_tsv_field(&entry.action),
                sanitize_tsv_field(&entry.group_id),
                sanitize_tsv_field(&entry.risk),
                sanitize_tsv_field(&entry.budget_status),
                sanitize_tsv_field(&req_units_raw.join(";")),
                sanitize_tsv_field(&entry.notes)
            );
        }
        println!();

        // Group Review Work Packets
        println!("## Group Review Work Packets");
        for entry in &plan.groups {
            let req_units_raw: Vec<String> = manifest_units
                .iter()
                .filter(|u| u.group_id == entry.group_id)
                .map(|u| u.unit_id.clone())
                .collect();

            let file_review_cmds: Vec<String> = manifest_units
                .iter()
                .filter(|u| u.group_id == entry.group_id)
                .map(|u| u.review_command.clone())
                .collect();

            println!("---");
            println!("group_id: {}", entry.group_id);
            println!("risk: {}", entry.risk);
            println!("budget_status: {}", entry.budget_status);
            println!("required_units: {}", req_units_raw.join(";"));
            println!("review_commands: {}", file_review_cmds.join(" ; "));

            let context_command = format!(
                "{} --source {} --group {}",
                shell_quote(&self_exe),
                mode,
                shell_quote(&entry.group_id)
            );
            println!("context_command: {}", context_command);

            let split_source_val = if entry.budget_status == "split-required" {
                "Split Suggestions and Split Unit Diff Preview"
            } else {
                "none"
            };
            println!("split_source: {}", split_source_val);
        }
        println!();

        // Reducer Finalization Template
        println!("## Reducer Finalization Template");
        let rf_json = serde_json::json!({
            "coverage_validation": "required",
            "manifest_units": manifest_units.len(),
            "review_groups": groups.len(),
            "high_risk_units": high_risk_units,
            "coverage_gaps": Vec::<serde_json::Value>::new(),
            "finding_merge": {
                "deduplicated_findings": Vec::<serde_json::Value>::new(),
                "blockers": Vec::<serde_json::Value>::new(),
                "notes": Vec::<serde_json::Value>::new(),
            },
            "cross_file_reduction": "required_after_coverage_validation",
            "dependency_checks": Vec::<serde_json::Value>::new(),
            "test_recommendations": Vec::<serde_json::Value>::new(),
            "residual_risks": Vec::<serde_json::Value>::new(),
            "final_verdict": "blocked_until_coverage_validation_passes",
        });
        if let Ok(json_str) = serde_json::to_string(&rf_json) {
            println!("{}", json_str);
        }
        println!();
    }

    // Dependency Summary
    println!("## Dependency Summary");
    println!("file\tchange\tkind\tdetail");
    let dep_entries = generate_dependency_summary(&global_diff);
    if dep_entries.is_empty() {
        println!("none\tnone\tnone\tnone");
    } else {
        for entry in &dep_entries {
            println!(
                "{}\t{}\t{}\t{}",
                sanitize_tsv_field(&entry.file),
                sanitize_tsv_field(&entry.change),
                sanitize_tsv_field(&entry.kind),
                sanitize_tsv_field(&entry.detail)
            );
        }
    }
    println!();

    // Semantic Context Queries - protected against colons in file paths and matches using splitn
    println!("## Semantic Context Queries");
    println!("query\tfile\tline\tmatch");

    let queries_file = Path::new(&repo_root).join(".pre-commit-review/context-queries");
    let custom_queries = if queries_file.exists() {
        let mut list = Vec::new();
        if let Ok(file) = File::open(&queries_file) {
            let reader = BufReader::new(file);
            for line in reader.lines().map_while(Result::ok) {
                let trimmed = line.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    list.push(trimmed.to_string());
                }
            }
        }
        list
    } else {
        Vec::new()
    };

    if custom_queries.is_empty() {
        println!("none\tnone\t0\tno context queries configured");
    } else {
        for query in &custom_queries {
            let safe_query = query.replace('\t', " ");

            // Execute git grep with NUL delimiters for path and line numbers
            let mut grep_args = vec!["grep", "-n", "-z", "-I", "-E", "-e", query];

            let ref_expr;
            if mode == "staged" {
                grep_args.push("--cached");
            } else if mode == "branch" {
                ref_expr = "HEAD".to_string();
                grep_args.push(&ref_expr);
            }
            grep_args.push("--");
            grep_args.push(".");

            let mut cmd = Command::new("git");
            cmd.args(&grep_args);
            cmd.current_dir(&repo_root);

            let mut count = 0;
            match cmd.output() {
                Ok(out) => {
                    let status_code = out.status.code();
                    if out.status.success() {
                        // exit 0: matches found, parse output
                        // NOTE: git grep -z replaces field separators (file:line:match)
                        // with NUL bytes, but records are still newline-separated.
                        // This means filenames containing literal newlines would be
                        // mis-parsed. This is an accepted limitation matching the
                        // legacy shell behavior.
                        for line_bytes in out.stdout.split(|&b| b == b'\n') {
                            if line_bytes.is_empty() {
                                continue;
                            }
                            if count >= context_query_limit {
                                break;
                            }
                            if let Some(first_nul) = line_bytes.iter().position(|&b| b == 0) {
                                let file_bytes = &line_bytes[..first_nul];
                                let rest = &line_bytes[first_nul + 1..];
                                if let Some(second_nul) = rest.iter().position(|&b| b == 0) {
                                    let line_num_bytes = &rest[..second_nul];
                                    let match_bytes = &rest[second_nul + 1..];

                                    let file_str = String::from_utf8_lossy(file_bytes);
                                    let line_num_str = String::from_utf8_lossy(line_num_bytes);
                                    let match_str = String::from_utf8_lossy(match_bytes);

                                    let file_parsed =
                                        if mode == "branch" && file_str.starts_with("HEAD:") {
                                            file_str.strip_prefix("HEAD:").unwrap().to_string()
                                        } else {
                                            file_str.into_owned()
                                        };

                                    if file_parsed == ".pre-commit-review/context-queries" {
                                        continue;
                                    }

                                    let line_num = line_num_str.parse::<usize>().unwrap_or(0);
                                    let safe_file = file_parsed.replace('\t', " ");
                                    let safe_match_text = match_str.replace('\t', " ");

                                    println!(
                                        "{}\t{}\t{}\t{}",
                                        safe_query, safe_file, line_num, safe_match_text
                                    );
                                    count += 1;
                                }
                            }
                        }
                    } else if status_code == Some(1) {
                        // exit 1: no matches found — this is normal, not an error
                    } else {
                        // exit >1: actual error (bad regex, permission denied, etc.)
                        return Err(AppError::GitError {
                            cmd: format!("git grep {:?}", grep_args),
                            details: String::from_utf8_lossy(&out.stderr).into_owned(),
                        });
                    }
                }
                Err(e) => {
                    return Err(AppError::IoError(e));
                }
            }

            if count == 0 {
                println!("{}\tnone\t0\tno matches", safe_query);
            }
        }
    }
    println!();

    emit_test_selection_hints(&name_status_entries, mode, &selected_ref, &repo_root);
    println!();

    // Suggested Review Queue
    println!("## Suggested Review Queue");
    let mut has_queue_items = false;
    for path in &high_risk_candidates_vec {
        println!("high-risk: {}", path);
        has_queue_items = true;
    }
    for item in &top_churn_entries {
        println!("top-churn: {}", item);
        has_queue_items = true;
    }
    for path in &generated_files_list {
        println!("generated-like consistency check: {}", path);
        has_queue_items = true;
    }
    for path in &lock_files_list {
        println!("lockfile consistency check: {}", path);
        has_queue_items = true;
    }
    if !has_queue_items {
        println!("none");
    }

    // Staged Files with Unstaged Changes Too
    if mode == "staged" && unstaged_avail {
        let staged_list_bytes = run_command_bytes(
            &["git", "diff", "--cached", "--name-only", "-z", "--", "."],
            &repo_root,
        )?;
        let unstaged_list_bytes =
            run_command_bytes(&["git", "diff", "--name-only", "-z", "--", "."], &repo_root)?;

        let staged_list_out = String::from_utf8_lossy(&staged_list_bytes);
        let unstaged_list_out = String::from_utf8_lossy(&unstaged_list_bytes);

        let staged_set: HashSet<&str> = staged_list_out
            .split('\0')
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        let unstaged_set: HashSet<&str> = unstaged_list_out
            .split('\0')
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        let mut overlap: Vec<&str> = staged_set.intersection(&unstaged_set).cloned().collect();
        if !overlap.is_empty() {
            overlap.sort();
            println!();
            println!("## Staged Files With Unstaged Changes Too");
            for f in overlap {
                println!("{}", f);
            }
        }
    }

    // Limit/emit the actual global diff only when the gateway budget allows it.
    if diff_output_decision == "inline" {
        emit_diff_limited(&global_diff, max_diff_bytes, inline_diff_bytes);
    } else {
        emit_diff_omitted(
            diff_size,
            max_diff_bytes,
            inline_diff_bytes,
            &diff_omitted_reason,
        );
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_requested_group(
    req_grp: &str,
    manifest_units: &[ManifestUnit],
    groups: &[ReviewGroup],
    unit_diff_cache: &HashMap<String, Vec<u8>>,
    mode: &str,
    max_diff_bytes: usize,
    inline_diff_bytes: usize,
) -> Result<(), AppError> {
    let group = match groups.iter().find(|g| g.group_id == req_grp) {
        Some(g) => g,
        None => {
            println!("## Requested Group Diff");
            println!("group_id: {}", req_grp);
            println!();
            println!("No review group found for requested group in the selected diff source.");
            return Ok(());
        }
    };

    println!("## Requested Group Files");
    println!("status\tpath\tunit_id\treview_command");
    for unit in manifest_units {
        if unit.group_id == group.group_id {
            println!(
                "{}\t{}\t{}\t{}",
                unit.status, unit.file_path, unit.unit_id, unit.review_command
            );
        }
    }
    println!();

    println!("## Requested Group Diff");
    println!("group_id: {}", group.group_id);
    println!("risk: {}", group.risk);
    println!("budget_status: {}", group.budget_status);
    println!("diff_bytes: {}", group.diff_bytes);

    let req_units: Vec<String> = manifest_units
        .iter()
        .filter(|u| u.group_id == group.group_id)
        .map(|u| u.unit_id.clone())
        .collect();
    println!("required_units: {}", req_units.join(";"));
    println!("files: {}", group.files.join(";"));

    let self_exe = env::var("PRE_COMMIT_REVIEW_HELPER_PATH").unwrap_or_else(|_| {
        env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("collect_diff_context"))
            .to_string_lossy()
            .to_string()
    });
    let context_command = format!(
        "{} --source {} --group {}",
        shell_quote(&self_exe),
        mode,
        shell_quote(&group.group_id)
    );
    println!("context_command: {}", context_command);

    if group.budget_status == "split-required" {
        println!();
        println!("Group exceeds hard review budget; use split suggestions instead of reviewing it as one group.");
        println!();
        println!("## Split Suggestions");
        println!(
            "parent_group_id\tunit_id\tpath\tsplit_kind\tdiff_bytes\thunk_header\treview_command"
        );
        for f in &group.files {
            let unit = match manifest_units.iter().find(|u| u.file_path == *f) {
                Some(u) => u,
                None => continue,
            };
            let f_diff_bytes = unit_diff_cache.get(f).cloned().unwrap_or_default();
            let f_diff = String::from_utf8_lossy(&f_diff_bytes);
            let hunks = split_diff_into_hunks(&f_diff);
            if hunks.is_empty() {
                println!(
                    "{}\tfile:{}\t{}\tfile\t0\tnone\t{}",
                    group.group_id, f, f, unit.review_command
                );
            } else {
                for (h_idx, hunk) in hunks.iter().enumerate() {
                    let clean_header = hunk.header.replace('\t', " ");
                    println!(
                        "{}\thunk:{}:{}\t{}\thunk\t{}\t{}\t{}",
                        group.group_id,
                        f,
                        h_idx + 1,
                        f,
                        hunk.bytes,
                        clean_header,
                        unit.review_command
                    );
                }
            }
        }
        println!();
        println!("## Split Unit Diff Preview");
        for f in &group.files {
            let f_diff_bytes = unit_diff_cache.get(f).cloned().unwrap_or_default();
            let f_diff = String::from_utf8_lossy(&f_diff_bytes);
            let hunks = split_diff_into_hunks(&f_diff);
            for (h_idx, hunk) in hunks.iter().enumerate() {
                println!("unit_id: hunk:{}:{}", f, h_idx + 1);
                println!("parent_group_id: {}", group.group_id);
                println!("```diff");
                print!("{}", hunk.content);
                println!("```");
            }
        }
        return Ok(());
    }

    let mut group_diff = String::new();
    for f in &group.files {
        if let Some(f_diff_bytes) = unit_diff_cache.get(f) {
            group_diff.push_str(&String::from_utf8_lossy(f_diff_bytes));
        }
    }

    if group_diff.is_empty() {
        println!();
        println!("No diff available for requested group in the selected diff source.");
        return Ok(());
    }

    emit_diff_limited(&group_diff, max_diff_bytes, inline_diff_bytes);
    Ok(())
}

fn main() {
    match run_app() {
        Ok(_) => {}
        Err(e) => match e {
            AppError::InvalidArgument(msg) => {
                eprintln!("collect_diff_context: {}", msg);
                std::process::exit(2);
            }
            AppError::GitError { cmd, details } => {
                eprintln!(
                    "collect_diff_context: git command failed\ncmd: {}\n{}",
                    cmd, details
                );
                std::process::exit(1);
            }
            AppError::IoError(e) => {
                eprintln!("collect_diff_context: I/O error: {}", e);
                std::process::exit(1);
            }
            AppError::GitMissing { details, cmd, cwd } => {
                eprintln!(
                    "collect_diff_context: git missing: {}\ncmd: {}\ncwd: {}",
                    details, cmd, cwd
                );
                std::process::exit(127);
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_tsv_field() {
        assert_eq!(sanitize_tsv_field("hello\tworld"), "hello world");
        assert_eq!(sanitize_tsv_field("line1\nline2"), "line1 line2");
        assert_eq!(sanitize_tsv_field("cr\rhere"), "cr here");
        assert_eq!(sanitize_tsv_field("no special chars"), "no special chars");
        assert_eq!(sanitize_tsv_field(""), "");
        assert_eq!(
            sanitize_tsv_field("mixed\ttab\nand\rnewline"),
            "mixed tab and newline"
        );
    }

    #[test]
    fn test_shell_quote_simple() {
        assert_eq!(shell_quote("simple"), "simple");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn test_shell_quote_special_chars() {
        assert_eq!(shell_quote("hello world"), "hello\\ world");
        assert_eq!(shell_quote("it's"), "it\\'s");
        assert_eq!(shell_quote("a\tb"), "$'a\\tb'");
        assert_eq!(shell_quote("a\nb"), "$'a\\nb'");
        assert_eq!(shell_quote("$HOME"), "\\$HOME");
    }

    #[test]
    fn test_quote_git_path_no_quoting() {
        assert_eq!(quote_git_path("simple.txt"), "simple.txt");
        assert_eq!(quote_git_path("src/main.rs"), "src/main.rs");
        assert_eq!(quote_git_path("file-name_v2.0.txt"), "file-name_v2.0.txt");
    }

    #[test]
    fn test_quote_git_path_special_chars() {
        assert_eq!(quote_git_path("hello\tworld.txt"), "\"hello\\tworld.txt\"");
        assert_eq!(quote_git_path("line\nbreak.txt"), "\"line\\nbreak.txt\"");
        assert_eq!(quote_git_path("file\"name.txt"), "\"file\\\"name.txt\"");
    }

    #[test]
    fn test_unquote_git_path_passthrough() {
        assert_eq!(unquote_git_path("simple.txt"), "simple.txt");
        assert_eq!(unquote_git_path("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn test_unquote_git_path_quoted() {
        assert_eq!(
            unquote_git_path("\"hello\\tworld.txt\""),
            "hello\tworld.txt"
        );
        assert_eq!(unquote_git_path("\"line\\nbreak.txt\""), "line\nbreak.txt");
        assert_eq!(unquote_git_path("\"file\\\"name.txt\""), "file\"name.txt");
    }

    #[test]
    fn test_quote_unquote_roundtrip() {
        let test_paths = vec![
            "simple.txt",
            "path with spaces.txt",
            "tab\there.txt",
            "new\nline.txt",
            "quote\"mark.txt",
            "backslash\\here.txt",
            "src/normal/path.rs",
        ];
        for path in test_paths {
            let quoted = quote_git_path(path);
            let unquoted = unquote_git_path(&quoted);
            assert_eq!(unquoted, path, "Roundtrip failed for: {:?}", path);
        }
    }

    #[test]
    fn test_parse_name_status_z_basic() {
        let bytes = b"M\0file.txt\0";
        let entries = parse_name_status_z(bytes);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, "M");
        assert_eq!(entries[0].path, "file.txt");
        assert!(entries[0].old_path.is_none());
    }

    #[test]
    fn test_parse_name_status_z_rename() {
        let bytes = b"R100\0old.txt\0new.txt\0";
        let entries = parse_name_status_z(bytes);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, "R100");
        assert_eq!(entries[0].path, "new.txt");
        assert_eq!(entries[0].old_path.as_deref(), Some("old.txt"));
    }

    #[test]
    fn test_parse_name_status_z_multiple() {
        let bytes = b"M\0a.txt\0A\0b.txt\0D\0c.txt\0";
        let entries = parse_name_status_z(bytes);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].status, "M");
        assert_eq!(entries[0].path, "a.txt");
        assert_eq!(entries[1].status, "A");
        assert_eq!(entries[1].path, "b.txt");
        assert_eq!(entries[2].status, "D");
        assert_eq!(entries[2].path, "c.txt");
    }

    #[test]
    fn test_parse_name_status_z_copy() {
        let bytes = b"C100\0src.txt\0dest.txt\0";
        let entries = parse_name_status_z(bytes);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, "C100");
        assert_eq!(entries[0].path, "dest.txt");
        assert_eq!(entries[0].old_path.as_deref(), Some("src.txt"));
    }

    #[test]
    fn test_parse_name_status_z_empty() {
        let entries = parse_name_status_z(b"");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_numstat_z_basic() {
        let bytes = b"10\t5\tfile.txt\0";
        let entries = parse_numstat_z(bytes);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].add, "10");
        assert_eq!(entries[0].del, "5");
        assert_eq!(entries[0].path, "file.txt");
        assert!(entries[0].old_path.is_none());
    }

    #[test]
    fn test_parse_numstat_z_rename() {
        let bytes = b"3\t2\t\0old.txt\0new.txt\0";
        let entries = parse_numstat_z(bytes);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].add, "3");
        assert_eq!(entries[0].del, "2");
        assert_eq!(entries[0].path, "new.txt");
        assert_eq!(entries[0].old_path.as_deref(), Some("old.txt"));
    }

    #[test]
    fn test_parse_numstat_z_binary() {
        let bytes = b"-\t-\tbinary.png\0";
        let entries = parse_numstat_z(bytes);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].add, "-");
        assert_eq!(entries[0].del, "-");
        assert_eq!(entries[0].path, "binary.png");
    }

    #[test]
    fn test_parse_numstat_z_empty() {
        let entries = parse_numstat_z(b"");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_group_component_for_path() {
        assert_eq!(group_component_for_path("src/main.rs"), "src");
        assert_eq!(group_component_for_path("README.md"), "README.md");
        assert_eq!(group_component_for_path("deeply/nested/file.txt"), "deeply");
    }

    #[test]
    fn test_safe_group_component() {
        assert_eq!(safe_group_component("normal"), "normal");
        assert_eq!(safe_group_component("has space"), "has_space");
        assert_eq!(safe_group_component("UPPER"), "UPPER");
        assert_eq!(safe_group_component("special!@#chars"), "special___chars");
    }

    #[test]
    fn test_lookup_numstat_found() {
        let entries = vec![NumstatEntry {
            add: "10".to_string(),
            del: "5".to_string(),
            path: "file.txt".to_string(),
            old_path: None,
            path_spec: "file.txt".to_string(),
        }];
        let (add, del) = lookup_numstat(&entries, "file.txt", None);
        assert_eq!(add, "10");
        assert_eq!(del, "5");
    }

    #[test]
    fn test_lookup_numstat_not_found() {
        let entries = vec![NumstatEntry {
            add: "10".to_string(),
            del: "5".to_string(),
            path: "file.txt".to_string(),
            old_path: None,
            path_spec: "file.txt".to_string(),
        }];
        let (add, del) = lookup_numstat(&entries, "other.txt", None);
        assert_eq!(add, "0");
        assert_eq!(del, "0");
    }

    #[test]
    fn test_lookup_numstat_rename() {
        let entries = vec![NumstatEntry {
            add: "3".to_string(),
            del: "2".to_string(),
            path: "new.txt".to_string(),
            old_path: Some("old.txt".to_string()),
            path_spec: "old.txt => new.txt".to_string(),
        }];
        let (add, del) = lookup_numstat(&entries, "new.txt", Some("old.txt"));
        assert_eq!(add, "3");
        assert_eq!(del, "2");
    }

    #[test]
    fn test_split_diff_into_hunks() {
        let diff = "diff --git a/file.txt b/file.txt\n--- a/file.txt\n+++ b/file.txt\n@@ -1,3 +1,4 @@\n line1\n+added\n line2\n line3\n@@ -10,3 +11,3 @@\n line10\n-old\n+new\n line12\n";
        let hunks = split_diff_into_hunks(diff);
        assert_eq!(hunks.len(), 2);
        assert!(hunks[0].header.contains("@@ -1,3 +1,4 @@"));
        assert!(hunks[1].header.contains("@@ -10,3 +11,3 @@"));
    }

    #[test]
    fn test_split_diff_into_hunks_empty() {
        let hunks = split_diff_into_hunks("");
        assert!(hunks.is_empty());
    }
}
