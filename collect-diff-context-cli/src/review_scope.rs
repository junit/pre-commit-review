use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
}

impl ScopeError {
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
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

pub fn revalidate_scope(scope: &AuthoritativeScope) -> Result<(), ScopeError> {
    let observed = open_authoritative_scope(ScopeRequest {
        repository: scope.repository.clone(),
        source: Some(scope.source),
        expected_fingerprint: Some(scope.fingerprint.clone()),
    })?;

    if observed.head != scope.head
        || observed.base != scope.base
        || observed.selected_ref != scope.selected_ref
        || observed.units != scope.units
        || observed.groups != scope.groups
        || observed.work_order != scope.work_order
    {
        return Err(ScopeError::new(
            "review scope structure changed during revalidation",
        ));
    }
    Ok(())
}
