use crate::candidate::{
    decode_git_quoted_path, hash_unstaged_path_bounded, read_git_blobs_batch_bounded,
    read_unstaged_path_bounded, unstaged_mode, unstaged_path_size, CandidateBytes, CandidateError,
    CandidatePresence, RepoPath,
};
use crate::git_policy::{output_bounded, GitOutputError};
use crate::impact_context::contracts::{Completeness, UnitStatus};
use crate::impact_context::index::budget::{
    IndexBudgetExhaustion, IndexBudgetTracker, IndexResource,
};
use crate::impact_context::index::model::{
    IndexLimitation, RepositoryLocator, RepositoryManifest, RepositoryManifestEntry,
};
use crate::review_scope::{AuthoritativeScope, ReviewSource};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const LOCATOR_DEADLINE: Duration = Duration::from_secs(5);

pub trait RepositoryManifestSource {
    fn scope_fingerprint(&self) -> &str;
    fn revalidate_scope_bounded(&self, deadline: Duration) -> Result<(), RepositoryManifestError>;
    fn source(&self) -> ReviewSource;
    fn repository_locator(&self) -> &RepositoryLocator;
    fn manifest_bounded(
        &self,
        budget: &mut IndexBudgetTracker,
    ) -> Result<RepositoryManifest, RepositoryManifestError>;
    fn read_bounded(
        &self,
        path: &RepoPath,
        maximum_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError>;
}

#[derive(Debug, Clone)]
pub struct GitRepositoryManifestSource {
    scope: AuthoritativeScope,
    repository_locator: RepositoryLocator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryManifestError {
    pub code: &'static str,
    pub message: String,
}

impl RepositoryManifestError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RepositoryManifestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RepositoryManifestError {}

#[derive(Debug, Clone)]
struct GitManifestRecord {
    path: RepoPath,
    mode: String,
    object_id: Option<String>,
    presence: CandidatePresence,
}

impl GitRepositoryManifestSource {
    pub fn new(scope: &AuthoritativeScope) -> Result<Self, RepositoryManifestError> {
        Self::new_bounded(scope, LOCATOR_DEADLINE)
    }

    pub fn new_bounded(
        scope: &AuthoritativeScope,
        deadline: Duration,
    ) -> Result<Self, RepositoryManifestError> {
        if !scope.authoritative {
            return Err(RepositoryManifestError::new(
                "index-scope-not-authoritative",
                "repository manifest requires an authoritative scope",
            ));
        }
        let started = Instant::now();
        let object_format = git_text(
            &scope.repository,
            &["rev-parse", "--show-object-format"],
            started,
            deadline,
            "cannot determine Git object format",
        )?;
        let base_tree = git_text(
            &scope.repository,
            &["rev-parse", "HEAD^{tree}"],
            started,
            deadline,
            "cannot determine opening tree",
        )?;
        let index_manifest_digest =
            if matches!(scope.source, ReviewSource::Staged | ReviewSource::Unstaged) {
                let records = list_index_records(&scope.repository, None, started, deadline)?;
                Some(digest_index_records(&object_format, &records))
            } else {
                None
            };
        let overlay_candidate_digest = digest_overlay(scope, index_manifest_digest.as_deref());
        let repository_locator = RepositoryLocator {
            source: scope.source,
            object_format,
            base_tree: Some(base_tree),
            index_manifest_digest,
            overlay_candidate_digest,
        };
        repository_locator.validate().map_err(|error| {
            RepositoryManifestError::new("index-locator-invalid", error.to_string())
        })?;
        Ok(Self {
            scope: scope.clone(),
            repository_locator,
        })
    }
}

impl RepositoryManifestSource for GitRepositoryManifestSource {
    fn scope_fingerprint(&self) -> &str {
        &self.scope.fingerprint
    }

    fn revalidate_scope_bounded(&self, deadline: Duration) -> Result<(), RepositoryManifestError> {
        crate::review_scope::revalidate_scope_bounded(&self.scope, deadline)
            .map_err(|error| RepositoryManifestError::new("index-scope-drift", error.to_string()))
    }

    fn source(&self) -> ReviewSource {
        self.scope.source
    }

    fn repository_locator(&self) -> &RepositoryLocator {
        &self.repository_locator
    }

    fn manifest_bounded(
        &self,
        budget: &mut IndexBudgetTracker,
    ) -> Result<RepositoryManifest, RepositoryManifestError> {
        budget.check_deadline().map_err(map_budget_error)?;
        let started = Instant::now();
        let deadline = budget.remaining_deadline();
        let mut records = match self.scope.source {
            ReviewSource::Staged | ReviewSource::Unstaged => {
                list_index_records(&self.scope.repository, None, started, deadline)?
            }
            ReviewSource::Branch => {
                list_tree_records(&self.scope.repository, None, started, deadline)?
            }
        };
        if self.scope.source == ReviewSource::Staged {
            add_staged_deletions(&self.scope, &mut records)?;
        }
        records.sort_by(|left, right| left.path.cmp(&right.path));

        let mut selected_records = Vec::new();
        let mut entries = Vec::new();
        let mut limitations = Vec::new();
        let mut truncated_entry = None;
        let mut manifest_truncated = false;
        for record in records {
            if let Err(error) = budget.consume(IndexResource::ManifestFiles, 1) {
                limitations.push(budget_limitation(error, None));
                manifest_truncated = true;
                break;
            }
            let record_bytes = record
                .path
                .as_str()
                .len()
                .saturating_add(record.mode.len())
                .saturating_add(record.object_id.as_deref().map(str::len).unwrap_or(0))
                .saturating_add(128);
            if let Err(error) = budget.consume(IndexResource::ManifestBytes, record_bytes) {
                let limitation = budget_limitation(error, Some(record.path.clone()));
                truncated_entry = Some(limited_entry(&record, limitation.code.clone()));
                limitations.push(limitation);
                manifest_truncated = true;
                break;
            }
            selected_records.push(record);
        }

        let maximum_file_bytes = budget.budget().max_file_bytes;
        let maximum_parse_bytes = budget.amount(IndexResource::ParseBytes).remaining;
        let staged_requests = if self.scope.source == ReviewSource::Unstaged {
            Vec::new()
        } else {
            selected_records
                .iter()
                .filter(|record| record.presence == CandidatePresence::Present)
                .filter_map(|record| {
                    record
                        .object_id
                        .as_ref()
                        .map(|object_id| (record.path.clone(), object_id.clone()))
                })
                .collect::<Vec<_>>()
        };
        let mut staged_contents = if staged_requests.is_empty() {
            BTreeMap::new()
        } else {
            read_git_blobs_batch_bounded(
                &self.scope.repository,
                &staged_requests,
                started,
                deadline,
                maximum_file_bytes,
                maximum_parse_bytes,
            )
            .map_err(|error| map_candidate_error("index-content-unavailable", error))?
        };

        for record in selected_records {
            match record.presence {
                CandidatePresence::Deleted | CandidatePresence::Gitlink => {
                    entries.push(completed_entry(&record, None, None));
                }
                CandidatePresence::Present if self.scope.source == ReviewSource::Unstaged => {
                    let repository_path = self.scope.repository.join(record.path.as_str());
                    let mode = unstaged_mode(&repository_path, &record.mode).map_err(|error| {
                        RepositoryManifestError::new(
                            "index-content-unavailable",
                            format!(
                                "cannot inspect tracked worktree path {}: {error}",
                                record.path.as_str()
                            ),
                        )
                    })?;
                    let mut worktree_record = record.clone();
                    worktree_record.mode = mode.clone();
                    if mode == "160000" {
                        worktree_record.presence = CandidatePresence::Gitlink;
                        entries.push(completed_entry(&worktree_record, None, None));
                        continue;
                    }
                    match unstaged_path_size(&repository_path, &mode) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            worktree_record.mode = "000000".to_string();
                            worktree_record.presence = CandidatePresence::Deleted;
                            entries.push(completed_entry(&worktree_record, None, None));
                        }
                        Err(error) => {
                            let limitation = content_limitation(
                                "index-content-unavailable",
                                worktree_record.path.clone(),
                                format!("cannot inspect tracked worktree content: {error}"),
                            );
                            entries
                                .push(unavailable_entry(&worktree_record, limitation.code.clone()));
                            limitations.push(limitation);
                        }
                        Ok(_) => match hash_unstaged_path_bounded(
                            &repository_path,
                            &mode,
                            &worktree_record.path,
                            started,
                            deadline,
                            maximum_file_bytes,
                            budget.amount(IndexResource::ParseBytes).remaining,
                        ) {
                            Ok((sha256, bytes)) => {
                                budget
                                    .consume(IndexResource::ParseBytes, bytes)
                                    .map_err(map_budget_error)?;
                                entries.push(completed_entry(
                                    &worktree_record,
                                    Some(sha256),
                                    Some(bytes),
                                ));
                            }
                            Err(error) => {
                                let (status, limitation) =
                                    candidate_limitation(&worktree_record.path, error);
                                entries.push(entry_with_status(
                                    &worktree_record,
                                    status,
                                    limitation.code.clone(),
                                ));
                                limitations.push(limitation);
                            }
                        },
                    }
                }
                CandidatePresence::Present => {
                    let result = staged_contents.remove(&record.path).ok_or_else(|| {
                        RepositoryManifestError::new(
                            "index-content-unavailable",
                            format!("missing batch content for {}", record.path.as_str()),
                        )
                    })?;
                    match result {
                        Ok(content) => {
                            budget
                                .consume(IndexResource::ParseBytes, content.bytes.len())
                                .map_err(map_budget_error)?;
                            entries.push(completed_entry(
                                &record,
                                Some(content.sha256),
                                Some(content.bytes.len()),
                            ));
                        }
                        Err(error) => {
                            let (status, limitation) = candidate_limitation(&record.path, error);
                            entries.push(entry_with_status(
                                &record,
                                status,
                                limitation.code.clone(),
                            ));
                            limitations.push(limitation);
                        }
                    }
                }
            }
            budget.check_deadline().map_err(map_budget_error)?;
        }
        if let Some(entry) = truncated_entry {
            entries.push(entry);
        }

        limitations.sort_by(limitation_order);
        let completeness = if manifest_truncated
            || entries
                .iter()
                .any(|entry| entry.status != UnitStatus::Completed)
        {
            Completeness::Partial
        } else {
            Completeness::Complete
        };
        let digest = digest_manifest(
            &self.repository_locator,
            &entries,
            completeness,
            &limitations,
        );
        let manifest = RepositoryManifest {
            locator: self.repository_locator.clone(),
            digest,
            entries,
            completeness,
            limitations,
        };
        manifest.validate().map_err(|error| {
            RepositoryManifestError::new("index-manifest-invalid", error.to_string())
        })?;
        Ok(manifest)
    }

    fn read_bounded(
        &self,
        path: &RepoPath,
        maximum_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError> {
        let started = Instant::now();
        match self.scope.source {
            ReviewSource::Unstaged => {
                let record = list_index_records(
                    &self.scope.repository,
                    Some(path),
                    started,
                    LOCATOR_DEADLINE,
                )
                .map_err(|error| CandidateError::new(error.to_string()))?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    CandidateError::new(format!(
                        "repository path is not tracked: {}",
                        path.as_str()
                    ))
                })?;
                let repository_path = self.scope.repository.join(path.as_str());
                let mode = unstaged_mode(&repository_path, &record.mode).map_err(|error| {
                    CandidateError::new(format!(
                        "cannot inspect tracked path {}: {error}",
                        path.as_str()
                    ))
                })?;
                if mode == "160000" {
                    return Err(CandidateError::new(format!(
                        "repository path is a gitlink: {}",
                        path.as_str()
                    )));
                }
                let bytes = read_unstaged_path_bounded(
                    &repository_path,
                    &mode,
                    path,
                    maximum_bytes,
                    started,
                    LOCATOR_DEADLINE,
                )?;
                let sha256 = format!("{:x}", Sha256::digest(&bytes));
                let binary = bytes.iter().take(8192).any(|byte| *byte == 0);
                Ok(CandidateBytes {
                    bytes,
                    sha256,
                    binary,
                })
            }
            ReviewSource::Staged | ReviewSource::Branch => {
                let record = if self.scope.source == ReviewSource::Staged {
                    list_index_records(
                        &self.scope.repository,
                        Some(path),
                        started,
                        LOCATOR_DEADLINE,
                    )
                } else {
                    list_tree_records(
                        &self.scope.repository,
                        Some(path),
                        started,
                        LOCATOR_DEADLINE,
                    )
                }
                .map_err(|error| CandidateError::new(error.to_string()))?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    CandidateError::new(format!(
                        "repository path is not present: {}",
                        path.as_str()
                    ))
                })?;
                let object_id = record.object_id.ok_or_else(|| {
                    CandidateError::new(format!(
                        "repository path has no readable object: {}",
                        path.as_str()
                    ))
                })?;
                let requests = vec![(path.clone(), object_id)];
                let mut contents = read_git_blobs_batch_bounded(
                    &self.scope.repository,
                    &requests,
                    started,
                    LOCATOR_DEADLINE,
                    maximum_bytes,
                    maximum_bytes,
                )?;
                contents.remove(path).ok_or_else(|| {
                    CandidateError::new("Git batch reader omitted the requested path")
                })?
            }
        }
    }
}

fn list_index_records(
    repository: &Path,
    path: Option<&RepoPath>,
    started: Instant,
    deadline: Duration,
) -> Result<Vec<GitManifestRecord>, RepositoryManifestError> {
    let mut command = Command::new("git");
    command
        .current_dir(repository)
        .args(["ls-files", "--stage", "-z", "--"]);
    if let Some(path) = path {
        command.arg(path.as_str());
    }
    let output = output_bounded(
        &mut command,
        remaining_deadline(started, deadline, "index-deadline-exhausted")?,
    )
    .map_err(|error| map_git_error(error, "cannot list index entries"))?;
    if !output.status.success() {
        return Err(git_failure("cannot list index entries", &output.stderr));
    }
    parse_git_records(&output.stdout, false)
}

fn list_tree_records(
    repository: &Path,
    path: Option<&RepoPath>,
    started: Instant,
    deadline: Duration,
) -> Result<Vec<GitManifestRecord>, RepositoryManifestError> {
    let mut command = Command::new("git");
    command
        .current_dir(repository)
        .args(["ls-tree", "-rz", "HEAD", "--"]);
    if let Some(path) = path {
        command.arg(path.as_str());
    }
    let output = output_bounded(
        &mut command,
        remaining_deadline(started, deadline, "index-deadline-exhausted")?,
    )
    .map_err(|error| map_git_error(error, "cannot list tree entries"))?;
    if !output.status.success() {
        return Err(git_failure("cannot list tree entries", &output.stderr));
    }
    parse_git_records(&output.stdout, true)
}

fn parse_git_records(
    bytes: &[u8],
    tree: bool,
) -> Result<Vec<GitManifestRecord>, RepositoryManifestError> {
    let mut records = Vec::new();
    for record in bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| {
                RepositoryManifestError::new(
                    "index-git-output-invalid",
                    "Git record is missing a path",
                )
            })?;
        let metadata = std::str::from_utf8(&record[..tab]).map_err(|_| {
            RepositoryManifestError::new(
                "index-git-output-invalid",
                "Git record metadata is not UTF-8",
            )
        })?;
        let path = std::str::from_utf8(&record[tab + 1..]).map_err(|_| {
            RepositoryManifestError::new(
                "index-repository-path-invalid",
                "repository path is not UTF-8",
            )
        })?;
        let path = RepoPath::new(path).map_err(|error| {
            RepositoryManifestError::new("index-repository-path-invalid", error.to_string())
        })?;
        let fields = metadata.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err(RepositoryManifestError::new(
                "index-git-output-invalid",
                "Git record has invalid metadata",
            ));
        }
        let (mode, object_id) = if tree {
            if fields[1] != "blob" && !(fields[0] == "160000" && fields[1] == "commit") {
                return Err(RepositoryManifestError::new(
                    "index-git-output-invalid",
                    "tree record has an unsupported object type",
                ));
            }
            (fields[0], fields[2])
        } else {
            if fields[2] != "0" {
                return Err(RepositoryManifestError::new(
                    "index-unmerged-entry",
                    format!("index path is unmerged: {}", path.as_str()),
                ));
            }
            (fields[0], fields[1])
        };
        records.push(GitManifestRecord {
            path,
            mode: mode.to_string(),
            object_id: Some(object_id.to_string()),
            presence: if mode == "160000" {
                CandidatePresence::Gitlink
            } else {
                CandidatePresence::Present
            },
        });
    }
    records.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(records)
}

fn add_staged_deletions(
    scope: &AuthoritativeScope,
    records: &mut Vec<GitManifestRecord>,
) -> Result<(), RepositoryManifestError> {
    for unit in &scope.units {
        if !unit.status.starts_with('D') {
            continue;
        }
        let decoded = decode_git_quoted_path(&unit.path);
        let path = RepoPath::new(decoded).map_err(|error| {
            RepositoryManifestError::new("index-repository-path-invalid", error.to_string())
        })?;
        if records.iter().any(|record| record.path == path) {
            continue;
        }
        records.push(GitManifestRecord {
            path,
            mode: "000000".to_string(),
            object_id: None,
            presence: CandidatePresence::Deleted,
        });
    }
    Ok(())
}

fn completed_entry(
    record: &GitManifestRecord,
    content_sha256: Option<String>,
    content_bytes: Option<usize>,
) -> RepositoryManifestEntry {
    RepositoryManifestEntry {
        path: record.path.clone(),
        mode: record.mode.clone(),
        presence: record.presence,
        content_sha256,
        content_bytes,
        language: language_for_path(&record.path),
        status: UnitStatus::Completed,
        limitation_codes: Vec::new(),
    }
}

fn limited_entry(record: &GitManifestRecord, code: String) -> RepositoryManifestEntry {
    entry_with_status(record, UnitStatus::BudgetExhausted, code)
}

fn unavailable_entry(record: &GitManifestRecord, code: String) -> RepositoryManifestEntry {
    entry_with_status(record, UnitStatus::Unavailable, code)
}

fn entry_with_status(
    record: &GitManifestRecord,
    status: UnitStatus,
    code: String,
) -> RepositoryManifestEntry {
    RepositoryManifestEntry {
        path: record.path.clone(),
        mode: record.mode.clone(),
        presence: record.presence,
        content_sha256: None,
        content_bytes: None,
        language: language_for_path(&record.path),
        status,
        limitation_codes: vec![code],
    }
}

fn language_for_path(path: &RepoPath) -> Option<String> {
    let value = path.as_str();
    if value.ends_with(".rs") {
        Some("rust".to_string())
    } else if value.ends_with(".toml") {
        Some("toml".to_string())
    } else if value.ends_with(".json") {
        Some("json".to_string())
    } else if value.ends_with(".yaml") || value.ends_with(".yml") {
        Some("yaml".to_string())
    } else if value.ends_with(".sh") {
        Some("shell".to_string())
    } else {
        None
    }
}

fn candidate_limitation(path: &RepoPath, error: CandidateError) -> (UnitStatus, IndexLimitation) {
    let (status, code) = match error.budget_limitation_code() {
        Some("file-byte-budget-exhausted") => (
            UnitStatus::BudgetExhausted,
            "index-file-byte-budget-exhausted",
        ),
        Some("total-byte-budget-exhausted") => (
            UnitStatus::BudgetExhausted,
            "index-parse-byte-budget-exhausted",
        ),
        Some("deadline-exhausted") => (UnitStatus::BudgetExhausted, "index-deadline-exhausted"),
        _ => (UnitStatus::Unavailable, "index-content-unavailable"),
    };
    (
        status,
        content_limitation(code, path.clone(), error.to_string()),
    )
}

fn content_limitation(code: &'static str, path: RepoPath, reason: String) -> IndexLimitation {
    IndexLimitation {
        code: code.to_string(),
        path: Some(path),
        symbol_id: None,
        reason,
        interpretation: "repository index content is incomplete for this path".to_string(),
    }
}

fn budget_limitation(error: IndexBudgetExhaustion, path: Option<RepoPath>) -> IndexLimitation {
    IndexLimitation {
        code: error.code().to_string(),
        path,
        symbol_id: None,
        reason: error.to_string(),
        interpretation: "repository manifest collection stopped at a declared budget".to_string(),
    }
}

fn map_budget_error(error: IndexBudgetExhaustion) -> RepositoryManifestError {
    RepositoryManifestError::new(error.code(), error.to_string())
}

fn map_candidate_error(code: &'static str, error: CandidateError) -> RepositoryManifestError {
    RepositoryManifestError::new(code, error.to_string())
}

fn limitation_order(left: &IndexLimitation, right: &IndexLimitation) -> std::cmp::Ordering {
    limitation_key(left).cmp(&limitation_key(right))
}

fn limitation_key(limitation: &IndexLimitation) -> (&str, &str, &str, &str, &str) {
    (
        limitation.code.as_str(),
        limitation.path.as_ref().map(RepoPath::as_str).unwrap_or(""),
        limitation.symbol_id.as_deref().unwrap_or(""),
        limitation.reason.as_str(),
        limitation.interpretation.as_str(),
    )
}

fn digest_index_records(object_format: &str, records: &[GitManifestRecord]) -> String {
    let mut digest = Sha256::new();
    digest_component(&mut digest, b"repository-index-manifest/v1");
    digest_component(&mut digest, object_format.as_bytes());
    for record in records {
        digest_component(&mut digest, record.path.as_str().as_bytes());
        digest_component(&mut digest, record.mode.as_bytes());
        digest_optional(&mut digest, record.object_id.as_deref());
    }
    format!("{:x}", digest.finalize())
}

fn digest_overlay(scope: &AuthoritativeScope, index_digest: Option<&str>) -> String {
    let mut digest = Sha256::new();
    digest_component(&mut digest, b"repository-candidate-overlay/v1");
    digest_component(&mut digest, scope.source.as_str().as_bytes());
    digest_component(&mut digest, scope.fingerprint.as_bytes());
    digest_optional(&mut digest, index_digest);
    format!("{:x}", digest.finalize())
}

fn digest_manifest(
    locator: &RepositoryLocator,
    entries: &[RepositoryManifestEntry],
    completeness: Completeness,
    limitations: &[IndexLimitation],
) -> String {
    let mut digest = Sha256::new();
    digest_component(&mut digest, b"repository-manifest/v1");
    digest_component(&mut digest, locator.source.as_str().as_bytes());
    digest_component(&mut digest, locator.object_format.as_bytes());
    digest_optional(&mut digest, locator.base_tree.as_deref());
    digest_optional(&mut digest, locator.index_manifest_digest.as_deref());
    digest_component(&mut digest, locator.overlay_candidate_digest.as_bytes());
    for entry in entries {
        digest_component(&mut digest, entry.path.as_str().as_bytes());
        digest_component(&mut digest, entry.mode.as_bytes());
        digest_component(
            &mut digest,
            match entry.presence {
                CandidatePresence::Present => b"present",
                CandidatePresence::Deleted => b"deleted",
                CandidatePresence::Gitlink => b"gitlink",
            },
        );
        digest_optional(&mut digest, entry.content_sha256.as_deref());
        match entry.content_bytes {
            Some(bytes) => {
                digest.update([1]);
                digest_component(&mut digest, &(bytes as u64).to_be_bytes());
            }
            None => digest.update([0]),
        }
        digest_optional(&mut digest, entry.language.as_deref());
        digest_component(&mut digest, unit_status(entry.status));
        for code in &entry.limitation_codes {
            digest_component(&mut digest, code.as_bytes());
        }
    }
    digest_component(
        &mut digest,
        match completeness {
            Completeness::Complete => b"complete",
            Completeness::Partial => b"partial",
            Completeness::Unavailable => b"unavailable",
        },
    );
    for limitation in limitations {
        let (code, path, symbol_id, reason, interpretation) = limitation_key(limitation);
        for value in [code, path, symbol_id, reason, interpretation] {
            digest_component(&mut digest, value.as_bytes());
        }
    }
    format!("{:x}", digest.finalize())
}

fn unit_status(status: UnitStatus) -> &'static [u8] {
    match status {
        UnitStatus::Completed => b"completed",
        UnitStatus::Partial => b"partial",
        UnitStatus::Unsupported => b"unsupported",
        UnitStatus::BudgetExhausted => b"budget-exhausted",
        UnitStatus::Unavailable => b"unavailable",
    }
}

fn digest_optional(digest: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            digest.update([1]);
            digest_component(digest, value.as_bytes());
        }
        None => digest.update([0]),
    }
}

fn digest_component(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn git_text(
    repository: &Path,
    arguments: &[&str],
    started: Instant,
    deadline: Duration,
    context: &'static str,
) -> Result<String, RepositoryManifestError> {
    let mut command = Command::new("git");
    command.current_dir(repository).args(arguments);
    let output = output_bounded(
        &mut command,
        remaining_deadline(started, deadline, "index-locator-deadline-exhausted")?,
    )
    .map_err(|error| map_git_error(error, context))?;
    if !output.status.success() {
        return Err(git_failure(context, &output.stderr));
    }
    let value = std::str::from_utf8(&output.stdout)
        .map_err(|_| RepositoryManifestError::new("index-git-output-invalid", context))?
        .trim();
    if value.is_empty() {
        return Err(RepositoryManifestError::new(
            "index-git-output-invalid",
            context,
        ));
    }
    Ok(value.to_string())
}

fn remaining_deadline(
    started: Instant,
    deadline: Duration,
    code: &'static str,
) -> Result<Duration, RepositoryManifestError> {
    let remaining = deadline.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        Err(RepositoryManifestError::new(
            code,
            "repository index deadline exhausted",
        ))
    } else {
        Ok(remaining)
    }
}

fn map_git_error(error: GitOutputError, context: &'static str) -> RepositoryManifestError {
    match error {
        GitOutputError::DeadlineExceeded => {
            RepositoryManifestError::new("index-deadline-exhausted", context)
        }
        GitOutputError::OutputLimitExceeded => {
            RepositoryManifestError::new("index-git-output-limit-exhausted", context)
        }
        GitOutputError::Io(error) => {
            RepositoryManifestError::new("index-git-unavailable", format!("{context}: {error}"))
        }
    }
}

fn git_failure(context: &'static str, stderr: &[u8]) -> RepositoryManifestError {
    let detail = String::from_utf8_lossy(stderr)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    RepositoryManifestError::new(
        "index-git-failed",
        format!(
            "{context}: {}",
            if detail.is_empty() {
                "git failed"
            } else {
                &detail
            }
        ),
    )
}
