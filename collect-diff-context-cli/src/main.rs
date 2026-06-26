use regex::Regex;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

// Core Constants and Defaults
const DEFAULT_MAX_DIFF_BYTES: usize = 200000;
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
}

impl CliArgs {
    fn parse() -> Result<Self, AppError> {
        let args: Vec<String> = env::args().collect();
        let mut source = None;
        let mut path = None;
        let mut group = None;

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
                    println!("Usage: collect_diff_context [--source staged|unstaged|branch] [--path PATH | --group GROUP_ID]");
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

        Ok(CliArgs {
            source,
            path,
            group,
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
    add: usize,
    del: usize,
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

// Helper to escape path characters for shell commands (replicating printf %q)
fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
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
        Ok(status) => Ok(status.code() == Some(1)),
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
        Ok(status) => Ok(status.code() == Some(1)),
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
        Ok(status) => Ok(status.code() == Some(1)),
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

fn parse_name_status(output: &str) -> Vec<NameStatusEntry> {
    let mut entries = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('\t').collect();
        if parts.len() >= 2 {
            let status = parts[0].to_string();
            if (status.starts_with('R') || status.starts_with('C')) && parts.len() >= 3 {
                entries.push(NameStatusEntry {
                    status,
                    path: parts[2].to_string(),
                    old_path: Some(parts[1].to_string()),
                });
            } else {
                entries.push(NameStatusEntry {
                    status,
                    path: parts[1].to_string(),
                    old_path: None,
                });
            }
        }
    }
    entries
}

fn parse_numstat(output: &str) -> Vec<NumstatEntry> {
    let mut entries = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('\t').collect();
        if parts.len() >= 3 {
            let add = parts[0].parse::<usize>().unwrap_or(0);
            let del = parts[1].parse::<usize>().unwrap_or(0);
            entries.push(NumstatEntry {
                add,
                del,
                path_spec: parts[2].to_string(),
            });
        }
    }
    entries
}

fn lookup_numstat(entries: &[NumstatEntry], path: &str, old_path: Option<&str>) -> (usize, usize) {
    if let Some(old) = old_path {
        for entry in entries {
            if entry.path_spec.contains("=>")
                && entry.path_spec.contains(old)
                && entry.path_spec.contains(path)
            {
                return (entry.add, entry.del);
            }
        }
    } else {
        for entry in entries {
            if entry.path_spec == path {
                return (entry.add, entry.del);
            }
        }
    }
    (0, 0)
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
    let re_sig = Regex::new(r"^(export\s+)?(async\s+)?function\s|[A-Za-z0-9_$]+\s*\(|def\s.*\(|fn\s.*\(|func\s.*\(|class\s.*|interface\s.*|type\s.*\s(struct|interface)|struct\s.*|enum\s.*|impl\s.*").unwrap();
    let re_schema = Regex::new(r"(?i)^(alter\s+table|create\s+table|drop\s+table|create\s+index|drop\s+index|grant\s+|revoke\s+|add\s+column|drop\s+column)").unwrap();

    for line in diff.lines() {
        if let Some(stripped) = line.strip_prefix("+++ b/") {
            current_file = stripped.to_string();
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
                let mut safe_current = current_file.clone();
                let detail = clean.replace('\t', " ");
                safe_current = safe_current.replace('\t', " ");
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
                emit("signature", &mut entries);
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
    std::process::exit(0);
}

fn emit_diff_limited(diff: &str, max_bytes: usize) {
    let size = diff.len();
    println!("diff_bytes: {}", size);
    println!("max_diff_bytes: {}", max_bytes);
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
        println!("\n[diff truncated after {} bytes; inspect high-risk files with file-specific git diff commands before making safety claims]", max_bytes);
    }
    println!("```");
}

fn run_app() -> Result<(), AppError> {
    let args = CliArgs::parse()?;

    // Git top-level resolution
    let repo_root = match git_rev_parse_toplevel() {
        Ok(path) => path,
        Err(_) => {
            fail_no_repo();
            return Ok(());
        }
    };

    // Configuration from environment variables
    let max_diff_bytes = env::var("PRE_COMMIT_REVIEW_MAX_DIFF_BYTES")
        .ok()
        .and_then(|val| val.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_DIFF_BYTES);

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

    if args.path.is_some() && mode != "none" {
        review_limit_note =
            "file-specific diff for requested path; no other files included".to_string();
    }
    if args.group.is_some() && mode != "none" {
        review_limit_note =
            "group-specific diff for requested group; no other groups included".to_string();
    }

    let untracked_names = git_get_untracked_files(&repo_root);
    let mut unreviewed_note = "none".to_string();
    if mode == "staged" && unstaged_avail {
        unreviewed_note =
            "unstaged changes exist and were not reviewed as part of the staged commit candidate"
                .to_string();

        // Check for overlap
        let staged_list_out = run_command_string(
            &["git", "diff", "--cached", "--name-only", "--", "."],
            &repo_root,
        )
        .unwrap_or_default();
        let unstaged_list_out =
            run_command_string(&["git", "diff", "--name-only", "--", "."], &repo_root)
                .unwrap_or_default();

        let staged_set: HashSet<&str> = staged_list_out
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        let unstaged_set: HashSet<&str> = unstaged_list_out
            .lines()
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
    let global_name_status = if mode != "none" {
        git_run_diff_string(mode, &selected_ref, &["--name-status"], None, &repo_root)?
    } else {
        String::new()
    };
    let name_status_entries = parse_name_status(&global_name_status);

    // 2. Gather all numstat entries globally
    let global_numstat = if mode != "none" {
        git_run_diff_string(mode, &selected_ref, &["--numstat"], None, &repo_root)?
    } else {
        String::new()
    };
    let numstat_entries = parse_numstat(&global_numstat);

    // 3. Gather untracked files count/details
    let mut files_changed_str = "0 files, 0 insertions(+), 0 deletions(-)".to_string();
    if mode != "none" {
        let total_add: usize = numstat_entries.iter().map(|e| e.add).sum();
        let total_del: usize = numstat_entries.iter().map(|e| e.del).sum();
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
        let total = entry.add + entry.del;
        churn_list.push((total, entry.path_spec.clone(), entry.add, entry.del));
    }
    churn_list.sort_by(|a, b| b.0.cmp(&a.0)); // descending
    let top_churn_entries: Vec<String> = churn_list
        .iter()
        .take(5)
        .map(|item| format!("{} (+{}/-{})", item.1, item.2, item.3))
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
            current_file_in_diff = stripped.to_string();
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
    let mut content_risk_vec: Vec<String> = content_risk_files.into_iter().collect();
    content_risk_vec.sort();

    // Map files to path risk status
    let mut path_risk_files = Vec::new();
    let mut generated_files_list = Vec::new();
    let mut lock_files_list = Vec::new();
    let mut high_risk_candidates_set = HashSet::new();

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
            path_risk_files.push(path.clone());
            high_risk_candidates_set.insert(path.clone());
        }

        // Content risk also promotes to high-risk candidate
        if content_risk_vec.contains(path) {
            high_risk_candidates_set.insert(path.clone());
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
            generated_files_list.push(path.clone());
        }

        // Lockfile check
        if lockfile_regex.is_match(path) {
            lock_files_list.push(path.clone());
        }
    }

    path_risk_files.sort();
    generated_files_list.sort();
    lock_files_list.sort();

    let mut high_risk_candidates_vec: Vec<String> = high_risk_candidates_set.into_iter().collect();
    high_risk_candidates_vec.sort();

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
    let diff_truncated = if max_diff_bytes != 0 && diff_size > max_diff_bytes {
        review_limit_note = "partial diff output; inspect file list and prioritize risky files before making safety claims".to_string();
        "yes"
    } else {
        "no"
    };

    // Print Context Metadata Header
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
    println!("review_limits: {}", review_limit_note);
    println!("diff_truncated: {}", diff_truncated);
    println!("group_target_bytes: {}", group_target_bytes);
    println!("group_hard_bytes: {}", group_hard_bytes);
    println!("files_changed: {}", files_changed_str);
    println!("high_risk_candidates: {}", high_risk_candidates);
    println!("content_risk_candidates: {}", content_risk_candidates);
    println!("generated_like_files: {}", generated_like_files);
    println!("lock_files: {}", lock_files);
    println!("top_churn_files: {}", top_churn_files);
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

    // 6. Prints git status
    println!("## Status");
    if let Some(ref p) = args.path {
        let status_out = run_command_string(&["git", "status", "--short", "--", p], &repo_root)
            .unwrap_or_default();
        print!("{}", status_out);
    } else if args.group.is_some() {
        println!("group-specific status is emitted after group resolution");
    } else {
        let status_out =
            run_command_string(&["git", "status", "--short"], &repo_root).unwrap_or_default();
        print!("{}", status_out);
    }
    println!();

    if mode == "none" {
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

    for entry in &name_status_entries {
        let path = &entry.path;
        let old_path = entry.old_path.as_deref();

        let (add, del) = lookup_numstat(&numstat_entries, path, old_path);

        // Single file diff byte size (calculating raw bytes to prevent UTF-8 loss)
        let file_diff_bytes_vec =
            git_run_diff_bytes(mode, &selected_ref, &[], Some(path), &repo_root)?;
        let file_diff_bytes = file_diff_bytes_vec.len();

        let top_component = group_component_for_path(path);
        let safe_component = safe_group_component(&top_component);

        let mut risk_tags = Vec::new();
        let group_id;

        // Group assignment logic
        if high_risk_candidates_vec.contains(path) {
            risk_tags.push("high-risk".to_string());
            group_id = format!("high-risk-{}", safe_component);
            group_risk_map.insert(group_id.clone(), "high".to_string());
            group_reason_map.insert(group_id.clone(), "path-or-content-risk".to_string());
        } else if generated_files_list.contains(path) {
            risk_tags.push("generated-like".to_string());
            group_id = format!("consistency-{}", safe_component);
            if group_risk_map.get(&group_id).map(|s| s.as_str()) != Some("high") {
                group_risk_map.insert(group_id.clone(), "consistency".to_string());
                group_reason_map.insert(group_id.clone(), "generated-like".to_string());
            }
        } else if lock_files_list.contains(path) {
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

        let quoted_path = shell_quote(path);
        let review_command = match mode {
            "staged" => format!("git diff --cached -- {}", quoted_path),
            "unstaged" => format!("git diff -- {}", quoted_path),
            "branch" => {
                let ref_expr = format!("{}...HEAD", selected_ref);
                format!("git diff {} -- {}", shell_quote(&ref_expr), quoted_path)
            }
            _ => "unavailable".to_string(),
        };

        let context_command = format!(
            "{} --source {} --path {}",
            shell_quote(&self_exe),
            mode,
            quoted_path
        );

        // Update group properties
        *group_sizes.entry(group_id.clone()).or_insert(0) += file_diff_bytes;
        group_files_map
            .entry(group_id.clone())
            .or_default()
            .push(path.clone());
        group_commands_map
            .entry(group_id.clone())
            .or_default()
            .push(review_command.clone());

        manifest_units.push(ManifestUnit {
            unit_id: format!("file:{}", path),
            file_path: path.clone(),
            status: entry.status.clone(),
            additions: add,
            deletions: del,
            diff_bytes: file_diff_bytes,
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

    // Handle REQUEST_GROUP early exit
    if let Some(ref req_grp) = args.group {
        println!();
        emit_requested_group(
            req_grp,
            &manifest_units,
            &groups,
            mode,
            &selected_ref,
            max_diff_bytes,
            &repo_root,
        );
        return Ok(());
    }

    // Output stats and file lists for the main review mode
    println!("## Diff Stat");
    let diff_stat_out = git_run_diff_string(mode, &selected_ref, &["--stat"], None, &repo_root)?;
    print!("{}", diff_stat_out);
    println!();

    println!("## File List");
    print!("{}", global_name_status);
    println!();

    println!("## Numstat");
    print!("{}", global_numstat);
    println!();

    if let Some(ref req_path) = args.path {
        println!();
        println!("## Requested File Diff");
        println!("path: {}", req_path);

        let unit = manifest_units.iter().find(|u| u.file_path == *req_path);
        let r_cmd = unit.map(|u| u.review_command.clone()).unwrap_or_else(|| {
            let quoted_path = shell_quote(req_path);
            match mode {
                "staged" => format!("git diff --cached -- {}", quoted_path),
                "unstaged" => format!("git diff -- {}", quoted_path),
                "branch" => {
                    let ref_expr = format!("{}...HEAD", selected_ref);
                    format!("git diff {} -- {}", shell_quote(&ref_expr), quoted_path)
                }
                _ => "unavailable".to_string(),
            }
        });
        let c_cmd = unit.map(|u| u.context_command.clone()).unwrap_or_else(|| {
            format!(
                "{} --source {} --path {}",
                shell_quote(&self_exe),
                mode,
                shell_quote(req_path)
            )
        });

        println!("review_command: {}", r_cmd);
        println!("context_command: {}", c_cmd);

        let file_diff_bytes =
            git_run_diff_bytes(mode, &selected_ref, &[], Some(req_path), &repo_root)?;
        if file_diff_bytes.is_empty() {
            println!();
            println!("No diff available for requested path in the selected diff source.");
            return Ok(());
        }

        emit_diff_limited(&String::from_utf8_lossy(&file_diff_bytes), max_diff_bytes);
        return Ok(());
    }

    // Print Review Manifest (TSV) - protected with TSV sanitization
    println!("## Review Manifest");
    println!("unit_id\tpath\tstatus\tadditions\tdeletions\tdiff_bytes\trisk_tags\tgroup_id\treview_command\tcontext_command");
    for unit in &manifest_units {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            sanitize_tsv_field(&unit.unit_id),
            sanitize_tsv_field(&unit.file_path),
            sanitize_tsv_field(&unit.status),
            unit.additions,
            unit.deletions,
            unit.diff_bytes,
            sanitize_tsv_field(&unit.risk_tags.join(";")),
            sanitize_tsv_field(&unit.group_id),
            sanitize_tsv_field(&unit.review_command),
            sanitize_tsv_field(&unit.context_command)
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
    println!("parent_group_id\tunit_id\tpath\tsplit_kind\tdiff_bytes\thunk_header\treview_command");
    if !split_files.is_empty() {
        for (parent_group, path, r_cmd) in &split_files {
            let f_diff_bytes =
                git_run_diff_bytes(mode, &selected_ref, &[], Some(path), &repo_root)?;
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
            let f_diff_bytes =
                git_run_diff_bytes(mode, &selected_ref, &[], Some(path), &repo_root)?;
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

            // Execute git grep
            let mut grep_args = vec!["grep", "-n", "-I", "-E", "-e", query];

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
            if let Ok(out) = cmd.output() {
                let stdout_str = String::from_utf8_lossy(&out.stdout);

                for line in stdout_str.lines() {
                    if count >= context_query_limit {
                        break;
                    }
                    // parse format safely using splitn (protecting colon-rich matching text)
                    if mode == "branch" && line.starts_with("HEAD:") {
                        let parts: Vec<&str> = line.splitn(4, ':').collect();
                        if parts.len() >= 4 {
                            let file = parts[1];
                            let line_num = parts[2].parse::<usize>().unwrap_or(0);
                            let match_text = parts[3];

                            if file == ".pre-commit-review/context-queries" {
                                continue;
                            }

                            let safe_file = file.replace('\t', " ");
                            let safe_match_text = match_text.replace('\t', " ");
                            println!(
                                "{}\t{}\t{}\t{}",
                                safe_query, safe_file, line_num, safe_match_text
                            );
                            count += 1;
                        }
                    } else {
                        let parts: Vec<&str> = line.splitn(3, ':').collect();
                        if parts.len() >= 3 {
                            let file = parts[0];
                            let line_num = parts[1].parse::<usize>().unwrap_or(0);
                            let match_text = parts[2];

                            if file == ".pre-commit-review/context-queries" {
                                continue;
                            }

                            let safe_file = file.replace('\t', " ");
                            let safe_match_text = match_text.replace('\t', " ");
                            println!(
                                "{}\t{}\t{}\t{}",
                                safe_query, safe_file, line_num, safe_match_text
                            );
                            count += 1;
                        }
                    }
                }
            }

            if count == 0 {
                println!("{}\tnone\t0\tno matches", safe_query);
            }
        }
    }
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
        let staged_list_out = run_command_string(
            &["git", "diff", "--cached", "--name-only", "--", "."],
            &repo_root,
        )
        .unwrap_or_default();
        let unstaged_list_out =
            run_command_string(&["git", "diff", "--name-only", "--", "."], &repo_root)
                .unwrap_or_default();

        let staged_set: HashSet<&str> = staged_list_out
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        let unstaged_set: HashSet<&str> = unstaged_list_out
            .lines()
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

    // Limit/emit the actual global diff
    emit_diff_limited(&global_diff, max_diff_bytes);

    Ok(())
}

fn emit_requested_group(
    req_grp: &str,
    manifest_units: &[ManifestUnit],
    groups: &[ReviewGroup],
    mode: &str,
    selected_ref: &str,
    max_diff_bytes: usize,
    repo_root: &str,
) {
    let group = match groups.iter().find(|g| g.group_id == req_grp) {
        Some(g) => g,
        None => {
            println!("## Requested Group Diff");
            println!("group_id: {}", req_grp);
            println!();
            println!("No review group found for requested group in the selected diff source.");
            return;
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
            let f_diff_bytes =
                git_run_diff_bytes(mode, selected_ref, &[], Some(f), repo_root).unwrap_or_default();
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
            let f_diff_bytes =
                git_run_diff_bytes(mode, selected_ref, &[], Some(f), repo_root).unwrap_or_default();
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
        return;
    }

    let mut group_diff = String::new();
    for f in &group.files {
        let f_diff_bytes =
            git_run_diff_bytes(mode, selected_ref, &[], Some(f), repo_root).unwrap_or_default();
        group_diff.push_str(&String::from_utf8_lossy(&f_diff_bytes));
    }

    if group_diff.is_empty() {
        println!();
        println!("No diff available for requested group in the selected diff source.");
        return;
    }

    emit_diff_limited(&group_diff, max_diff_bytes);
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
                // If it is a GitError, print details and delegate to fail_no_repo()
                eprintln!(
                    "collect_diff_context: GitError under cmd: {}\nDetails: {}",
                    cmd, details
                );
                fail_no_repo();
            }
            AppError::IoError(e) => {
                eprintln!("collect_diff_context: IoError: {}", e);
                fail_no_repo();
            }
            AppError::GitMissing { details, cmd, cwd } => {
                eprintln!("collect_diff_context: Git executable missing or invalid cwd: {}\nAttempted cmd: {}\nCwd: {}", details, cmd, cwd);
                std::process::exit(1);
            }
        },
    }
}
