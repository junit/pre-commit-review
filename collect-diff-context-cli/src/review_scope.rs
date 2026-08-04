use crate::git_policy::{configure_read_only, output_bounded, GitOutputError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewSource {
    Staged,
    Unstaged,
    Branch,
}

impl ReviewSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Unstaged => "unstaged",
            Self::Branch => "branch",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "staged" => Some(Self::Staged),
            "unstaged" => Some(Self::Unstaged),
            "branch" => Some(Self::Branch),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScopeRequest {
    pub repository: PathBuf,
    pub source: Option<ReviewSource>,
    pub expected_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopeUnit {
    pub unit_id: String,
    pub path: String,
    pub status: String,
    pub additions: usize,
    pub deletions: usize,
    pub diff_bytes: usize,
    pub risk_tags: Vec<String>,
    pub group_id: String,
    pub review_command: String,
    pub context_command: String,
    pub content_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopeGroup {
    pub group_id: String,
    pub risk: String,
    pub reason: String,
    pub diff_bytes: usize,
    pub files: Vec<String>,
    pub budget_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkOrderEntry {
    pub priority: u8,
    pub group_id: String,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoritativeScope {
    pub authoritative: bool,
    pub repository: PathBuf,
    pub source: ReviewSource,
    pub head: String,
    pub base: String,
    pub selected_ref: String,
    pub fingerprint: String,
    pub collection_start: String,
    pub collection_end: String,
    pub units: Vec<ScopeUnit>,
    pub groups: Vec<ScopeGroup>,
    pub work_order: Vec<WorkOrderEntry>,
}

pub(crate) struct ScopeParts {
    pub repository: PathBuf,
    pub source: ReviewSource,
    pub head: String,
    pub base: String,
    pub selected_ref: String,
    pub fingerprint: String,
    pub collection_start: String,
    pub collection_end: String,
    pub units: Vec<ScopeUnit>,
    pub groups: Vec<ScopeGroup>,
}

impl AuthoritativeScope {
    pub(crate) fn from_parts(parts: ScopeParts) -> Self {
        let mut work_order = parts
            .groups
            .iter()
            .map(|group| {
                let (priority, action) = if group.budget_status == "split-required" {
                    (1, "split")
                } else if group.risk == "high" {
                    (2, "review")
                } else if group.risk == "consistency" {
                    (3, "review")
                } else {
                    (4, "review")
                };
                WorkOrderEntry {
                    priority,
                    group_id: group.group_id.clone(),
                    action: action.to_string(),
                }
            })
            .collect::<Vec<_>>();
        work_order.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left.group_id.cmp(&right.group_id))
        });

        Self {
            authoritative: true,
            repository: parts.repository,
            source: parts.source,
            head: parts.head,
            base: parts.base,
            selected_ref: parts.selected_ref,
            fingerprint: parts.fingerprint,
            collection_start: parts.collection_start,
            collection_end: parts.collection_end,
            units: parts.units,
            groups: parts.groups,
            work_order,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeError {
    reason: String,
    deadline_exceeded: bool,
}

impl ScopeError {
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            deadline_exceeded: false,
        }
    }

    pub(crate) fn deadline(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            deadline_exceeded: true,
        }
    }

    pub(crate) fn is_deadline_exceeded(&self) -> bool {
        self.deadline_exceeded
    }
}

impl std::fmt::Display for ScopeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for ScopeError {}

pub fn open_authoritative_scope(request: ScopeRequest) -> Result<AuthoritativeScope, ScopeError> {
    crate::app::open_authoritative_scope_impl(request)
}

pub fn open_authoritative_scope_bounded(
    request: ScopeRequest,
    deadline: std::time::Duration,
) -> Result<AuthoritativeScope, ScopeError> {
    crate::app::open_authoritative_scope_impl_bounded(request, deadline)
}

pub fn revalidate_scope(scope: &AuthoritativeScope) -> Result<(), ScopeError> {
    revalidate_scope_bounded(scope, std::time::Duration::MAX)
}

pub fn revalidate_scope_bounded(
    scope: &AuthoritativeScope,
    deadline: std::time::Duration,
) -> Result<(), ScopeError> {
    crate::app::revalidate_authoritative_scope_impl_bounded(scope, deadline)
}

pub fn added_lines(
    repository: &Path,
    source: ReviewSource,
    selected_ref: &str,
    path: &str,
) -> Result<BTreeSet<u32>, ScopeError> {
    parse_added_lines(&diff_for_path(
        repository,
        source,
        selected_ref,
        path,
        std::time::Duration::MAX,
    )?)
}

pub fn changed_ranges(
    repository: &Path,
    source: ReviewSource,
    selected_ref: &str,
    path: &str,
) -> Result<Vec<crate::candidate::ChangedRange>, ScopeError> {
    changed_ranges_bounded(
        repository,
        source,
        selected_ref,
        path,
        std::time::Duration::MAX,
    )
}

pub(crate) fn changed_ranges_bounded(
    repository: &Path,
    source: ReviewSource,
    selected_ref: &str,
    path: &str,
    timeout: std::time::Duration,
) -> Result<Vec<crate::candidate::ChangedRange>, ScopeError> {
    parse_changed_ranges(&diff_for_path(
        repository,
        source,
        selected_ref,
        path,
        timeout,
    )?)
}

fn diff_for_path(
    repository: &Path,
    source: ReviewSource,
    selected_ref: &str,
    path: &str,
    timeout: std::time::Duration,
) -> Result<Vec<u8>, ScopeError> {
    let mut command = Command::new("git");
    configure_read_only(&mut command);
    command.current_dir(repository).args([
        "-c",
        "color.ui=false",
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames",
        "--unified=0",
    ]);
    match source {
        ReviewSource::Staged => {
            command.arg("--cached");
        }
        ReviewSource::Unstaged => {}
        ReviewSource::Branch => {
            if selected_ref.is_empty() {
                return Err(ScopeError::new("branch scope is missing selected_ref"));
            }
            command.arg(format!("{selected_ref}...HEAD"));
        }
    }
    command.arg("--").arg(crate::app::unquote_git_path(path));
    let output = output_bounded(&mut command, timeout).map_err(|error| match error {
        GitOutputError::DeadlineExceeded => ScopeError::deadline(format!(
            "candidate deadline exceeded while mapping changed lines for {path}"
        )),
        GitOutputError::OutputLimitExceeded => ScopeError::new(format!(
            "Git output exceeded the {}-byte capture limit while mapping changed lines for {path}",
            crate::git_policy::MAX_GIT_OUTPUT_BYTES
        )),
        GitOutputError::Io(error) => {
            ScopeError::new(format!("cannot map changed lines for {path}: {error}"))
        }
    })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let detail = detail.chars().take(500).collect::<String>();
        return Err(ScopeError::new(format!(
            "cannot map changed lines for {path}: {}",
            if detail.is_empty() {
                "git diff failed"
            } else {
                &detail
            }
        )));
    }
    Ok(output.stdout)
}

#[derive(Debug, Clone, Copy)]
struct HunkCursor {
    next_new_line: u32,
    remaining_old: u64,
    remaining_new: u64,
}

fn parse_added_lines(diff: &[u8]) -> Result<BTreeSet<u32>, ScopeError> {
    let mut added = BTreeSet::new();
    let mut hunk = None;
    for line in diff.split(|byte| *byte == b'\n') {
        if line.starts_with(b"@@ -") {
            hunk = parse_hunk_header(line)?;
            continue;
        }
        let Some(mut cursor) = hunk else {
            continue;
        };
        match line.first().copied() {
            Some(b'+') if cursor.remaining_new > 0 => {
                added.insert(cursor.next_new_line);
                cursor.next_new_line = cursor
                    .next_new_line
                    .checked_add(1)
                    .ok_or_else(|| ScopeError::new("added line number exceeds u32"))?;
                cursor.remaining_new -= 1;
            }
            Some(b'-') if cursor.remaining_old > 0 => cursor.remaining_old -= 1,
            Some(b' ') if cursor.remaining_old > 0 && cursor.remaining_new > 0 => {
                cursor.remaining_old -= 1;
                cursor.remaining_new -= 1;
                cursor.next_new_line = cursor
                    .next_new_line
                    .checked_add(1)
                    .ok_or_else(|| ScopeError::new("added line number exceeds u32"))?;
            }
            Some(b'\\') => {}
            _ => {}
        }
        hunk = (cursor.remaining_old != 0 || cursor.remaining_new != 0).then_some(cursor);
    }
    Ok(added)
}

fn parse_changed_ranges(diff: &[u8]) -> Result<Vec<crate::candidate::ChangedRange>, ScopeError> {
    let mut ranges = Vec::new();
    for line in diff.split(|byte| *byte == b'\n') {
        if !line.starts_with(b"@@ -") {
            continue;
        }
        let (_, _, new_start, new_count) = parse_hunk_ranges(line)?;
        if new_count == 0 {
            let anchor = new_start.max(1);
            ranges.push(crate::candidate::ChangedRange {
                start_line: anchor,
                end_line: anchor,
                deletion_anchor: true,
            });
            continue;
        }
        let end_line = u64::from(new_start)
            .checked_add(new_count - 1)
            .and_then(|line| u32::try_from(line).ok())
            .ok_or_else(|| ScopeError::new("changed range exceeds u32"))?;
        ranges.push(crate::candidate::ChangedRange {
            start_line: new_start,
            end_line,
            deletion_anchor: false,
        });
    }
    Ok(ranges)
}

fn parse_hunk_header(line: &[u8]) -> Result<Option<HunkCursor>, ScopeError> {
    if !line.starts_with(b"@@ -") {
        return Ok(None);
    }
    let (old_start, old_count, new_start, new_count) = parse_hunk_ranges(line)?;
    let _ = old_start;
    Ok(Some(HunkCursor {
        next_new_line: new_start,
        remaining_old: old_count,
        remaining_new: new_count,
    }))
}

fn parse_hunk_ranges(line: &[u8]) -> Result<(u32, u64, u32, u64), ScopeError> {
    let header = std::str::from_utf8(line)
        .map_err(|_| ScopeError::new("git diff emitted a non-UTF-8 hunk header"))?;
    let mut fields = header.split_whitespace();
    if fields.next() != Some("@@") {
        return Err(ScopeError::new("git diff emitted an invalid hunk header"));
    }
    let old_range = fields
        .next()
        .and_then(|value| value.strip_prefix('-'))
        .ok_or_else(|| ScopeError::new("git diff emitted an invalid old hunk range"))?;
    let new_range = fields
        .next()
        .and_then(|value| value.strip_prefix('+'))
        .ok_or_else(|| ScopeError::new("git diff emitted an invalid new hunk range"))?;
    let (old_start, old_count) = parse_hunk_range(old_range)?;
    let (new_start, new_count) = parse_hunk_range(new_range)?;
    Ok((old_start, old_count, new_start, new_count))
}

fn parse_hunk_range(value: &str) -> Result<(u32, u64), ScopeError> {
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    let start = start
        .parse::<u32>()
        .map_err(|_| ScopeError::new("git diff emitted an invalid hunk line number"))?;
    let count = count
        .parse::<u64>()
        .map_err(|_| ScopeError::new("git diff emitted an invalid hunk line count"))?;
    Ok((start, count))
}
