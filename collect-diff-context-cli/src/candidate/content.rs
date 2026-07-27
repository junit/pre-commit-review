use crate::git_policy::{configure_read_only, output_bounded, GitOutputError};
use crate::review_scope::{AuthoritativeScope, ReviewSource};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RepoPath(String);

impl<'de> Deserialize<'de> for RepoPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let path = String::deserialize(deserializer)?;
        Self::new(path).map_err(serde::de::Error::custom)
    }
}

impl RepoPath {
    pub fn new(path: impl Into<String>) -> Result<Self, CandidateError> {
        let path = path.into();
        if path.is_empty() {
            return Err(CandidateError::new("repository path is empty"));
        }
        if path.len() > 4096 {
            return Err(CandidateError::new("repository path exceeds 4096 bytes"));
        }
        if path.as_bytes().contains(&0) {
            return Err(CandidateError::new("repository path contains NUL"));
        }
        let windows_prefix =
            path.as_bytes().get(1).is_some_and(|byte| *byte == b':') || path.starts_with("\\\\");
        if Path::new(&path).is_absolute()
            || path.contains('\\')
            || windows_prefix
            || Path::new(&path).components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
            || path.split(['/', '\\']).any(|component| component == "..")
        {
            return Err(CandidateError::new(
                "repository path must stay within the repository",
            ));
        }
        Ok(Self(path))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn decode_git_quoted_path(path: &str) -> String {
    if path.len() < 2 || !path.starts_with('"') || !path.ends_with('"') {
        return path.to_string();
    }

    let mut decoded = Vec::new();
    let bytes = &path.as_bytes()[1..path.len() - 1];
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' || index + 1 >= bytes.len() {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        match bytes[index + 1] {
            b'a' => decoded.push(7),
            b'b' => decoded.push(8),
            b'f' => decoded.push(12),
            b'n' => decoded.push(b'\n'),
            b'r' => decoded.push(b'\r'),
            b't' => decoded.push(b'\t'),
            b'v' => decoded.push(11),
            b'\\' => decoded.push(b'\\'),
            b'"' => decoded.push(b'"'),
            b'?' => decoded.push(b'?'),
            value if (b'0'..=b'7').contains(&value) => {
                let mut octal_value: u16 = 0;
                let mut digits = 0;
                while index + 1 + digits < bytes.len() && digits < 3 {
                    let next = bytes[index + 1 + digits];
                    if !(b'0'..=b'7').contains(&next) {
                        break;
                    }
                    octal_value = octal_value * 8 + u16::from(next - b'0');
                    digits += 1;
                }
                decoded.push(octal_value as u8);
                index += digits.saturating_sub(1);
            }
            _ => decoded.push(bytes[index]),
        }
        index += 2;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CandidatePresence {
    Present,
    Deleted,
    Gitlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidateFile {
    pub path: RepoPath,
    pub mode: String,
    pub content_identity: Option<String>,
    pub presence: CandidatePresence,
    pub manifest_unit_id: Option<String>,
    pub change_status: Option<String>,
    pub changed_ranges: Vec<ChangedRange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChangedRange {
    pub start_line: u32,
    pub end_line: u32,
    pub deletion_anchor: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateBytes {
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub binary: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CandidateOpenLimits {
    pub deadline: Duration,
    pub max_changed_files: usize,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
}

impl CandidateOpenLimits {
    fn unbounded() -> Self {
        Self {
            deadline: Duration::MAX,
            max_changed_files: usize::MAX,
            max_file_bytes: usize::MAX,
            max_total_bytes: usize::MAX,
        }
    }
}

pub trait CandidateContent {
    fn scope_fingerprint(&self) -> &str;
    fn candidate_digest(&self) -> &str;
    fn source(&self) -> ReviewSource;
    fn files(&self) -> &[CandidateFile];
    fn read_bounded(
        &self,
        path: &RepoPath,
        max_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError>;

    fn read(&self, path: &RepoPath) -> Result<CandidateBytes, CandidateError> {
        self.read_bounded(path, usize::MAX)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateErrorKind {
    Unavailable,
    ByteLimitExceeded,
    TotalByteLimitExceeded,
    ChangedFileLimitExceeded,
    DeadlineExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateError {
    reason: String,
    kind: CandidateErrorKind,
}

impl CandidateError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            kind: CandidateErrorKind::Unavailable,
        }
    }

    pub fn byte_limit_exceeded(path: &RepoPath, max_bytes: usize) -> Self {
        Self {
            reason: format!(
                "candidate path {} exceeds the {max_bytes}-byte read limit",
                path.as_str()
            ),
            kind: CandidateErrorKind::ByteLimitExceeded,
        }
    }

    pub fn is_byte_limit_exceeded(&self) -> bool {
        self.kind == CandidateErrorKind::ByteLimitExceeded
    }

    pub fn budget_limitation_code(&self) -> Option<&'static str> {
        match self.kind {
            CandidateErrorKind::ByteLimitExceeded => Some("file-byte-budget-exhausted"),
            CandidateErrorKind::TotalByteLimitExceeded => Some("total-byte-budget-exhausted"),
            CandidateErrorKind::ChangedFileLimitExceeded => Some("changed-file-budget-exhausted"),
            CandidateErrorKind::DeadlineExceeded => Some("deadline-exhausted"),
            CandidateErrorKind::Unavailable => None,
        }
    }

    fn budget(path: &RepoPath, kind: CandidateErrorKind, limit: usize) -> Self {
        let resource = match kind {
            CandidateErrorKind::ByteLimitExceeded => "file-byte",
            CandidateErrorKind::TotalByteLimitExceeded => "total-byte",
            CandidateErrorKind::ChangedFileLimitExceeded => "changed-file",
            CandidateErrorKind::DeadlineExceeded => "deadline",
            CandidateErrorKind::Unavailable => "candidate",
        };
        Self {
            reason: format!(
                "candidate path {} exceeded the {resource} budget ({limit})",
                path.as_str()
            ),
            kind,
        }
    }

    fn deadline(limit: Duration) -> Self {
        Self {
            reason: format!(
                "candidate preparation exceeded the {}ms deadline",
                limit.as_millis()
            ),
            kind: CandidateErrorKind::DeadlineExceeded,
        }
    }
}

impl std::fmt::Display for CandidateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for CandidateError {}

#[derive(Debug, Clone)]
pub struct GitCandidateContent {
    repository: PathBuf,
    source: ReviewSource,
    started: Instant,
    deadline: Duration,
    scope_fingerprint: String,
    candidate_digest: String,
    files: Vec<CandidateFile>,
    content_sizes: BTreeMap<RepoPath, u64>,
    preparation_errors: BTreeMap<RepoPath, CandidateError>,
}

impl GitCandidateContent {
    pub fn open(scope: &AuthoritativeScope) -> Result<Self, CandidateError> {
        Self::open_bounded(scope, CandidateOpenLimits::unbounded())
    }

    pub fn open_bounded(
        scope: &AuthoritativeScope,
        limits: CandidateOpenLimits,
    ) -> Result<Self, CandidateError> {
        let started = Instant::now();
        let mut preparation_errors = BTreeMap::new();
        let mut content_sizes = BTreeMap::new();
        let mut unit_paths = scope
            .units
            .iter()
            .map(|unit| decode_git_quoted_path(&unit.path))
            .collect::<Vec<_>>();
        unit_paths.sort_unstable();
        unit_paths.dedup();
        let mut requested_paths = unit_paths
            .iter()
            .take(limits.max_changed_files)
            .cloned()
            .chain([
                ".pre-commit-review/context-queries".to_string(),
                ".pre-commit-review/test-hints".to_string(),
            ])
            .collect::<Vec<_>>();
        for path in unit_paths.iter().skip(limits.max_changed_files) {
            let repo_path = RepoPath::new(path)?;
            preparation_errors.insert(
                repo_path.clone(),
                CandidateError::budget(
                    &repo_path,
                    CandidateErrorKind::ChangedFileLimitExceeded,
                    limits.max_changed_files,
                ),
            );
        }
        requested_paths.sort_unstable();
        requested_paths.dedup();

        let mut command = Command::new("git");
        configure_read_only(&mut command);
        command.current_dir(&scope.repository);
        match scope.source {
            ReviewSource::Staged | ReviewSource::Unstaged => {
                command.args(["ls-files", "--stage", "-z", "--"]);
            }
            ReviewSource::Branch => {
                command.args(["ls-tree", "-z", "HEAD", "--"]);
            }
        }
        for path in &requested_paths {
            command.arg(path);
        }
        let output = output_bounded(&mut command, remaining_deadline(started, limits.deadline)?)
            .map_err(|error| {
                map_git_output_error(error, limits.deadline, "cannot list staged files")
            })?;
        if !output.status.success() {
            return Err(git_error("cannot list staged files", &output.stderr));
        }

        let mut files = Vec::new();
        let mut reserved_bytes = 0_usize;
        for record in output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
        {
            let tab = record
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or_else(|| CandidateError::new("git emitted an invalid staged record"))?;
            let metadata = std::str::from_utf8(&record[..tab])
                .map_err(|_| CandidateError::new("git emitted non-UTF-8 staged metadata"))?;
            let mut fields = metadata.split_whitespace();
            let mode = fields
                .next()
                .ok_or_else(|| CandidateError::new("staged record is missing mode"))?;
            let (object_id, include) = if scope.source == ReviewSource::Branch {
                let _object_type = fields
                    .next()
                    .ok_or_else(|| CandidateError::new("tree record is missing object type"))?;
                let object_id = fields
                    .next()
                    .ok_or_else(|| CandidateError::new("tree record is missing object id"))?;
                (object_id, true)
            } else {
                let object_id = fields
                    .next()
                    .ok_or_else(|| CandidateError::new("staged record is missing object id"))?;
                let stage = fields
                    .next()
                    .ok_or_else(|| CandidateError::new("staged record is missing stage"))?;
                (object_id, stage == "0")
            };
            if !include {
                continue;
            }
            let path = std::str::from_utf8(&record[tab + 1..])
                .map_err(|_| CandidateError::new("git emitted a non-UTF-8 repository path"))?;
            let repo_path = RepoPath::new(path)?;
            let unit = scope
                .units
                .iter()
                .find(|unit| decode_git_quoted_path(&unit.path) == path);
            let repository_path = scope.repository.join(path);
            let candidate_mode = if scope.source == ReviewSource::Unstaged {
                unstaged_mode(&repository_path, mode).map_err(|error| {
                    CandidateError::new(format!(
                        "cannot inspect unstaged candidate {}: {error}",
                        path
                    ))
                })?
            } else {
                mode.to_string()
            };
            let (mut content_identity, presence, content_bytes) = if scope.source
                == ReviewSource::Unstaged
            {
                if candidate_mode == "160000" {
                    (
                        Some(object_id.to_string()),
                        CandidatePresence::Gitlink,
                        None,
                    )
                } else {
                    match unstaged_path_size(&repository_path, &candidate_mode) {
                        Ok(size) => (None, CandidatePresence::Present, Some(size)),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            (None, CandidatePresence::Deleted, None)
                        }
                        Err(error) => {
                            return Err(CandidateError::new(format!(
                                "cannot inspect unstaged candidate {}: {error}",
                                path
                            )))
                        }
                    }
                }
            } else {
                let presence = if mode == "160000" {
                    CandidatePresence::Gitlink
                } else {
                    CandidatePresence::Present
                };
                let size = if presence == CandidatePresence::Present {
                    match git_blob_size(&scope.repository, object_id, started, limits.deadline) {
                        Ok(size) => Some(size),
                        Err(error) => {
                            preparation_errors.insert(repo_path.clone(), error);
                            None
                        }
                    }
                } else {
                    None
                };
                (Some(object_id.to_string()), presence, size)
            };

            if presence == CandidatePresence::Present
                && !preparation_errors.contains_key(&repo_path)
            {
                if started.elapsed() >= limits.deadline {
                    preparation_errors.insert(
                        repo_path.clone(),
                        CandidateError::budget(
                            &repo_path,
                            CandidateErrorKind::DeadlineExceeded,
                            limits.deadline.as_millis().try_into().unwrap_or(usize::MAX),
                        ),
                    );
                } else if let Some(size) = content_bytes {
                    let size = usize::try_from(size).unwrap_or(usize::MAX);
                    if size > limits.max_file_bytes {
                        preparation_errors.insert(
                            repo_path.clone(),
                            CandidateError::budget(
                                &repo_path,
                                CandidateErrorKind::ByteLimitExceeded,
                                limits.max_file_bytes,
                            ),
                        );
                    } else if reserved_bytes.saturating_add(size) > limits.max_total_bytes {
                        preparation_errors.insert(
                            repo_path.clone(),
                            CandidateError::budget(
                                &repo_path,
                                CandidateErrorKind::TotalByteLimitExceeded,
                                limits.max_total_bytes,
                            ),
                        );
                    } else if scope.source != ReviewSource::Unstaged {
                        reserved_bytes = reserved_bytes.saturating_add(size);
                    }
                }
            }

            if scope.source == ReviewSource::Unstaged
                && presence == CandidatePresence::Present
                && !preparation_errors.contains_key(&repo_path)
            {
                match hash_unstaged_path_bounded(
                    &repository_path,
                    &candidate_mode,
                    &repo_path,
                    started,
                    limits.deadline,
                    limits.max_file_bytes,
                    limits.max_total_bytes.saturating_sub(reserved_bytes),
                ) {
                    Ok((sha256, bytes)) => {
                        content_identity = Some(format!("sha256:{sha256}"));
                        reserved_bytes = reserved_bytes.saturating_add(bytes);
                    }
                    Err(error) => {
                        preparation_errors.insert(repo_path.clone(), error);
                    }
                }
            }

            let changed_ranges = if unit.is_some() && !preparation_errors.contains_key(&repo_path) {
                match crate::review_scope::changed_ranges_bounded(
                    &scope.repository,
                    scope.source,
                    &scope.selected_ref,
                    path,
                    remaining_deadline(started, limits.deadline)?,
                ) {
                    Ok(ranges) if started.elapsed() < limits.deadline => ranges,
                    Ok(_) => {
                        preparation_errors
                            .insert(repo_path.clone(), CandidateError::deadline(limits.deadline));
                        Vec::new()
                    }
                    Err(error) if error.is_deadline_exceeded() => {
                        preparation_errors
                            .insert(repo_path.clone(), CandidateError::deadline(limits.deadline));
                        Vec::new()
                    }
                    Err(error) => {
                        return Err(CandidateError::new(format!(
                            "cannot map changed ranges for {path}: {error}"
                        )))
                    }
                }
            } else {
                Vec::new()
            };
            if let Some(size) = content_bytes {
                content_sizes.insert(repo_path.clone(), size);
            }
            files.push(CandidateFile {
                path: repo_path,
                mode: candidate_mode,
                content_identity,
                presence,
                manifest_unit_id: unit.map(|unit| unit.unit_id.clone()),
                change_status: unit.map(|unit| unit.status.clone()),
                changed_ranges,
            });
        }
        for unit in &scope.units {
            let path = decode_git_quoted_path(&unit.path);
            if !files.iter().any(|file| file.path.as_str() == path) {
                let repo_path = RepoPath::new(&path)?;
                let deleted = unit.status.starts_with('D');
                if !deleted && !preparation_errors.contains_key(&repo_path) {
                    preparation_errors.insert(
                        repo_path.clone(),
                        CandidateError::new(format!(
                            "candidate path is unavailable during manifest collection: {path}"
                        )),
                    );
                }
                let changed_ranges = if deleted
                    && !preparation_errors.contains_key(&repo_path)
                    && started.elapsed() < limits.deadline
                {
                    match crate::review_scope::changed_ranges_bounded(
                        &scope.repository,
                        scope.source,
                        &scope.selected_ref,
                        &path,
                        remaining_deadline(started, limits.deadline)?,
                    ) {
                        Ok(ranges) if started.elapsed() < limits.deadline => ranges,
                        Ok(_) => {
                            preparation_errors.insert(
                                repo_path.clone(),
                                CandidateError::deadline(limits.deadline),
                            );
                            Vec::new()
                        }
                        Err(error) if error.is_deadline_exceeded() => {
                            preparation_errors.insert(
                                repo_path.clone(),
                                CandidateError::deadline(limits.deadline),
                            );
                            Vec::new()
                        }
                        Err(error) => {
                            return Err(CandidateError::new(format!(
                                "cannot map changed ranges for {path}: {error}"
                            )))
                        }
                    }
                } else {
                    Vec::new()
                };
                files.push(CandidateFile {
                    path: repo_path,
                    mode: "000000".to_string(),
                    content_identity: None,
                    presence: if deleted {
                        CandidatePresence::Deleted
                    } else {
                        CandidatePresence::Present
                    },
                    manifest_unit_id: Some(unit.unit_id.clone()),
                    change_status: Some(unit.status.clone()),
                    changed_ranges,
                });
            }
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let candidate_digest =
            digest_candidate_manifest(&scope.fingerprint, &files, &preparation_errors);

        Ok(Self {
            repository: scope.repository.clone(),
            source: scope.source,
            started,
            deadline: limits.deadline,
            scope_fingerprint: scope.fingerprint.clone(),
            candidate_digest,
            files,
            content_sizes,
            preparation_errors,
        })
    }
}

fn digest_candidate_manifest(
    scope_fingerprint: &str,
    files: &[CandidateFile],
    preparation_errors: &BTreeMap<RepoPath, CandidateError>,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"pre-commit-review-candidate-input-manifest/v1\0");
    digest_field(&mut digest, scope_fingerprint.as_bytes());
    for file in files {
        digest_field(&mut digest, file.path.as_str().as_bytes());
        digest_field(&mut digest, file.mode.as_bytes());
        digest_field(
            &mut digest,
            match file.presence {
                CandidatePresence::Present => b"present",
                CandidatePresence::Deleted => b"deleted",
                CandidatePresence::Gitlink => b"gitlink",
            },
        );
        digest_optional_field(&mut digest, file.manifest_unit_id.as_deref());
        digest_optional_field(&mut digest, file.content_identity.as_deref());
        digest_optional_field(
            &mut digest,
            preparation_errors
                .get(&file.path)
                .and_then(CandidateError::budget_limitation_code),
        );
    }
    format!("{:x}", digest.finalize())
}

fn digest_optional_field(digest: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            digest.update([1]);
            digest_field(digest, value.as_bytes());
        }
        None => digest.update([0]),
    }
}

fn digest_field(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

impl CandidateContent for GitCandidateContent {
    fn scope_fingerprint(&self) -> &str {
        &self.scope_fingerprint
    }

    fn candidate_digest(&self) -> &str {
        &self.candidate_digest
    }

    fn source(&self) -> ReviewSource {
        self.source
    }

    fn files(&self) -> &[CandidateFile] {
        &self.files
    }

    fn read_bounded(
        &self,
        path: &RepoPath,
        max_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError> {
        if let Some(error) = self.preparation_errors.get(path) {
            return Err(error.clone());
        }
        let file = self
            .files
            .iter()
            .find(|file| &file.path == path)
            .ok_or_else(|| {
                CandidateError::new(format!(
                    "candidate path is not available: {}",
                    path.as_str()
                ))
            })?;
        if file.presence != CandidatePresence::Present {
            return Err(CandidateError::new(format!(
                "candidate path has no readable blob: {}",
                path.as_str()
            )));
        }
        let bytes = match self.source {
            ReviewSource::Unstaged => read_unstaged_path_bounded(
                &self.repository.join(path.as_str()),
                &file.mode,
                path,
                max_bytes,
                self.started,
                self.deadline,
            )?,
            ReviewSource::Staged | ReviewSource::Branch => {
                let object_id = file.content_identity.as_deref().ok_or_else(|| {
                    CandidateError::new("candidate blob is missing object identity")
                })?;
                let content_size = self.content_sizes.get(path).copied().ok_or_else(|| {
                    CandidateError::new("candidate blob is missing its verified size")
                })?;
                read_git_blob_bounded(
                    &self.repository,
                    object_id,
                    path,
                    max_bytes,
                    content_size,
                    self.started,
                    self.deadline,
                )?
            }
        };
        remaining_deadline(self.started, self.deadline)?;
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        if self.source == ReviewSource::Unstaged {
            let expected = file
                .content_identity
                .as_deref()
                .and_then(|identity| identity.strip_prefix("sha256:"))
                .ok_or_else(|| {
                    CandidateError::new("unstaged candidate is missing SHA256 identity")
                })?;
            if expected != sha256 {
                return Err(CandidateError::new(format!(
                    "candidate content changed after manifest collection: {}",
                    path.as_str()
                )));
            }
        }
        let binary = bytes.iter().take(8192).any(|byte| *byte == 0);
        Ok(CandidateBytes {
            bytes,
            sha256,
            binary,
        })
    }
}

fn unstaged_path_size(path: &Path, mode: &str) -> std::io::Result<u64> {
    if mode == "120000" {
        let target = fs::read_link(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            return Ok(target.as_os_str().as_bytes().len() as u64);
        }
        #[cfg(not(unix))]
        return Ok(target.to_string_lossy().len() as u64);
    }
    fs::metadata(path).map(|metadata| metadata.len())
}

fn hash_unstaged_path_bounded(
    path: &Path,
    mode: &str,
    repo_path: &RepoPath,
    started: Instant,
    deadline: Duration,
    max_file_bytes: usize,
    remaining_total_bytes: usize,
) -> Result<(String, usize), CandidateError> {
    let mut digest = Sha256::new();
    if mode == "120000" {
        if started.elapsed() >= deadline {
            return Err(CandidateError::budget(
                repo_path,
                CandidateErrorKind::DeadlineExceeded,
                deadline.as_millis().try_into().unwrap_or(usize::MAX),
            ));
        }
        let target = fs::read_link(path).map_err(|error| {
            CandidateError::new(format!("cannot read unstaged candidate: {error}"))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let bytes = target.as_os_str().as_bytes();
            enforce_unstaged_hash_limits(
                repo_path,
                bytes.len(),
                max_file_bytes,
                remaining_total_bytes,
            )?;
            digest.update(bytes);
            Ok((format!("{:x}", digest.finalize()), bytes.len()))
        }
        #[cfg(not(unix))]
        {
            let target = target.to_string_lossy();
            let bytes = target.as_bytes();
            enforce_unstaged_hash_limits(
                repo_path,
                bytes.len(),
                max_file_bytes,
                remaining_total_bytes,
            )?;
            digest.update(bytes);
            Ok((format!("{:x}", digest.finalize()), bytes.len()))
        }
    } else {
        let input = open_unstaged_regular_file(path).map_err(|error| {
            CandidateError::new(format!("cannot read unstaged candidate: {error}"))
        })?;
        let hard_limit = max_file_bytes.min(remaining_total_bytes);
        let read_limit = u64::try_from(hard_limit)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut input = input.take(read_limit);
        let mut buffer = [0_u8; 64 * 1024];
        let mut total = 0_usize;
        loop {
            if started.elapsed() >= deadline {
                return Err(CandidateError::budget(
                    repo_path,
                    CandidateErrorKind::DeadlineExceeded,
                    deadline.as_millis().try_into().unwrap_or(usize::MAX),
                ));
            }
            let read = input.read(&mut buffer).map_err(|error| {
                CandidateError::new(format!("cannot read unstaged candidate: {error}"))
            })?;
            if read == 0 {
                break;
            }
            total = total.saturating_add(read);
            enforce_unstaged_hash_limits(repo_path, total, max_file_bytes, remaining_total_bytes)?;
            digest.update(&buffer[..read]);
        }
        Ok((format!("{:x}", digest.finalize()), total))
    }
}

fn enforce_unstaged_hash_limits(
    path: &RepoPath,
    observed_bytes: usize,
    max_file_bytes: usize,
    remaining_total_bytes: usize,
) -> Result<(), CandidateError> {
    if observed_bytes > max_file_bytes {
        return Err(CandidateError::budget(
            path,
            CandidateErrorKind::ByteLimitExceeded,
            max_file_bytes,
        ));
    }
    if observed_bytes > remaining_total_bytes {
        return Err(CandidateError::budget(
            path,
            CandidateErrorKind::TotalByteLimitExceeded,
            remaining_total_bytes,
        ));
    }
    Ok(())
}

fn git_blob_size(
    repository: &Path,
    object_id: &str,
    started: Instant,
    deadline: Duration,
) -> Result<u64, CandidateError> {
    let mut command = Command::new("git");
    configure_read_only(&mut command);
    command
        .current_dir(repository)
        .args(["cat-file", "-s", object_id]);
    let output = output_bounded(&mut command, remaining_deadline(started, deadline)?)
        .map_err(|error| map_git_output_error(error, deadline, "cannot inspect candidate blob"))?;
    if !output.status.success() {
        return Err(git_error("cannot inspect candidate blob", &output.stderr));
    }
    std::str::from_utf8(&output.stdout)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .ok_or_else(|| CandidateError::new("cannot inspect candidate blob: invalid object size"))
}

fn remaining_deadline(started: Instant, deadline: Duration) -> Result<Duration, CandidateError> {
    let remaining = deadline.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        Err(CandidateError::deadline(deadline))
    } else {
        Ok(remaining)
    }
}

fn map_git_output_error(
    error: GitOutputError,
    deadline: Duration,
    context: &str,
) -> CandidateError {
    match error {
        GitOutputError::DeadlineExceeded => CandidateError::deadline(deadline),
        GitOutputError::OutputLimitExceeded => CandidateError::new(format!(
            "{context}: Git output exceeded the {}-byte capture limit",
            crate::git_policy::MAX_GIT_OUTPUT_BYTES
        )),
        GitOutputError::Io(error) => CandidateError::new(format!("{context}: {error}")),
    }
}

fn read_unstaged_path_bounded(
    path: &Path,
    mode: &str,
    repo_path: &RepoPath,
    max_bytes: usize,
    started: Instant,
    deadline: Duration,
) -> Result<Vec<u8>, CandidateError> {
    remaining_deadline(started, deadline)?;
    if mode != "120000" {
        let mut input = open_unstaged_regular_file(path).map_err(|error| {
            CandidateError::new(format!(
                "cannot read unstaged candidate {}: {error}",
                repo_path.as_str()
            ))
        })?;
        let metadata = input.metadata().map_err(|error| {
            CandidateError::new(format!(
                "cannot inspect unstaged candidate {}: {error}",
                repo_path.as_str()
            ))
        })?;
        let max_bytes_u64 = u64::try_from(max_bytes).unwrap_or(u64::MAX);
        if metadata.len() > max_bytes_u64 {
            return Err(CandidateError::byte_limit_exceeded(repo_path, max_bytes));
        }
        let capacity = usize::try_from(metadata.len())
            .unwrap_or(max_bytes)
            .min(max_bytes);
        let mut bytes = Vec::with_capacity(capacity);
        input
            .by_ref()
            .take(max_bytes_u64.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| {
                CandidateError::new(format!(
                    "cannot read unstaged candidate {}: {error}",
                    repo_path.as_str()
                ))
            })?;
        if bytes.len() > max_bytes {
            return Err(CandidateError::byte_limit_exceeded(repo_path, max_bytes));
        }
        remaining_deadline(started, deadline)?;
        return Ok(bytes);
    }

    let target = fs::read_link(path).map_err(|error| {
        CandidateError::new(format!(
            "cannot read unstaged candidate {}: {error}",
            repo_path.as_str()
        ))
    })?;
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        target.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = target.to_string_lossy().into_owned().into_bytes();
    if bytes.len() > max_bytes {
        return Err(CandidateError::byte_limit_exceeded(repo_path, max_bytes));
    }
    remaining_deadline(started, deadline)?;
    Ok(bytes)
}

#[cfg(unix)]
fn open_unstaged_regular_file(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "candidate path is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(windows)]
fn open_unstaged_regular_file(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "candidate path is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn open_unstaged_regular_file(path: &Path) -> std::io::Result<File> {
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "candidate path is not a regular file",
        ));
    }
    Ok(file)
}

fn read_git_blob_bounded(
    repository: &Path,
    object_id: &str,
    path: &RepoPath,
    max_bytes: usize,
    content_size: u64,
    started: Instant,
    deadline: Duration,
) -> Result<Vec<u8>, CandidateError> {
    if content_size > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
        return Err(CandidateError::byte_limit_exceeded(path, max_bytes));
    }

    let mut content_command = Command::new("git");
    configure_read_only(&mut content_command);
    content_command
        .current_dir(repository)
        .args(["cat-file", "blob", object_id]);
    let output = output_bounded(&mut content_command, remaining_deadline(started, deadline)?)
        .map_err(|error| map_git_output_error(error, deadline, "cannot read candidate blob"))?;
    if !output.status.success() {
        return Err(git_error("cannot read candidate blob", &output.stderr));
    }
    if output.stdout.len() > max_bytes {
        return Err(CandidateError::byte_limit_exceeded(path, max_bytes));
    }
    remaining_deadline(started, deadline)?;
    Ok(output.stdout)
}

fn unstaged_mode(path: &Path, index_mode: &str) -> std::io::Result<String> {
    if index_mode == "160000" {
        return Ok(index_mode.to_string());
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(index_mode.to_string())
        }
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() {
        return Ok("120000".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = metadata.permissions().mode() & 0o111 != 0;
        Ok(if executable { "100755" } else { "100644" }.to_string())
    }
    #[cfg(not(unix))]
    {
        Ok(index_mode.to_string())
    }
}

fn git_error(context: &str, stderr: &[u8]) -> CandidateError {
    let detail = String::from_utf8_lossy(stderr)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    CandidateError::new(format!(
        "{context}: {}",
        if detail.is_empty() {
            "git failed"
        } else {
            &detail
        }
    ))
}
