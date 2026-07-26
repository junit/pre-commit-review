use crate::review_scope::{AuthoritativeScope, ReviewSource};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RepoPath(String);

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
            || path.starts_with('\\')
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

pub trait CandidateContent {
    fn scope_fingerprint(&self) -> &str;
    fn candidate_digest(&self) -> &str;
    fn source(&self) -> ReviewSource;
    fn files(&self) -> &[CandidateFile];
    fn read(&self, path: &RepoPath) -> Result<CandidateBytes, CandidateError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateError {
    reason: String,
}

impl CandidateError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
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
    scope_fingerprint: String,
    candidate_digest: String,
    files: Vec<CandidateFile>,
}

impl GitCandidateContent {
    pub fn open(scope: &AuthoritativeScope) -> Result<Self, CandidateError> {
        let mut requested_paths = scope
            .units
            .iter()
            .map(|unit| decode_git_quoted_path(&unit.path))
            .chain([
                ".pre-commit-review/context-queries".to_string(),
                ".pre-commit-review/test-hints".to_string(),
            ])
            .collect::<Vec<_>>();
        requested_paths.sort_unstable();
        requested_paths.dedup();

        let mut command = Command::new("git");
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
        let output = command
            .output()
            .map_err(|error| CandidateError::new(format!("cannot list staged files: {error}")))?;
        if !output.status.success() {
            return Err(git_error("cannot list staged files", &output.stderr));
        }

        let mut files = Vec::new();
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
            let unit = scope
                .units
                .iter()
                .find(|unit| decode_git_quoted_path(&unit.path) == path);
            let changed_ranges = unit
                .map(|_| {
                    crate::review_scope::changed_ranges(
                        &scope.repository,
                        scope.source,
                        &scope.selected_ref,
                        path,
                    )
                })
                .transpose()
                .map_err(|error| {
                    CandidateError::new(format!("cannot map changed ranges for {path}: {error}"))
                })?
                .unwrap_or_default();
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
            let (content_identity, presence) = if scope.source == ReviewSource::Unstaged {
                if candidate_mode == "160000" {
                    (Some(object_id.to_string()), CandidatePresence::Gitlink)
                } else {
                    match read_unstaged_path(&repository_path, &candidate_mode) {
                        Ok(bytes) => (
                            Some(format!("sha256:{:x}", Sha256::digest(bytes))),
                            CandidatePresence::Present,
                        ),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            (None, CandidatePresence::Deleted)
                        }
                        Err(error) => {
                            return Err(CandidateError::new(format!(
                                "cannot read unstaged candidate {}: {error}",
                                path
                            )))
                        }
                    }
                }
            } else {
                (
                    Some(object_id.to_string()),
                    if mode == "160000" {
                        CandidatePresence::Gitlink
                    } else {
                        CandidatePresence::Present
                    },
                )
            };
            files.push(CandidateFile {
                path: RepoPath::new(path)?,
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
            if unit.status.starts_with('D') && !files.iter().any(|file| file.path.as_str() == path)
            {
                files.push(CandidateFile {
                    path: RepoPath::new(&path)?,
                    mode: "000000".to_string(),
                    content_identity: None,
                    presence: CandidatePresence::Deleted,
                    manifest_unit_id: Some(unit.unit_id.clone()),
                    change_status: Some(unit.status.clone()),
                    changed_ranges: crate::review_scope::changed_ranges(
                        &scope.repository,
                        scope.source,
                        &scope.selected_ref,
                        &path,
                    )
                    .map_err(|error| {
                        CandidateError::new(format!(
                            "cannot map changed ranges for {path}: {error}"
                        ))
                    })?,
                });
            }
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let candidate_digest = digest_candidate_manifest(&scope.fingerprint, &files);

        Ok(Self {
            repository: scope.repository.clone(),
            source: scope.source,
            scope_fingerprint: scope.fingerprint.clone(),
            candidate_digest,
            files,
        })
    }
}

fn digest_candidate_manifest(scope_fingerprint: &str, files: &[CandidateFile]) -> String {
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

    fn read(&self, path: &RepoPath) -> Result<CandidateBytes, CandidateError> {
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
            ReviewSource::Unstaged => {
                read_unstaged_path(&self.repository.join(path.as_str()), &file.mode).map_err(
                    |error| {
                        CandidateError::new(format!(
                            "cannot read unstaged candidate {}: {error}",
                            path.as_str()
                        ))
                    },
                )?
            }
            ReviewSource::Staged | ReviewSource::Branch => {
                let object_id = file.content_identity.as_deref().ok_or_else(|| {
                    CandidateError::new("candidate blob is missing object identity")
                })?;
                let output = Command::new("git")
                    .current_dir(&self.repository)
                    .args(["cat-file", "blob", object_id])
                    .output()
                    .map_err(|error| {
                        CandidateError::new(format!("cannot read candidate blob: {error}"))
                    })?;
                if !output.status.success() {
                    return Err(git_error("cannot read candidate blob", &output.stderr));
                }
                output.stdout
            }
        };
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

fn read_unstaged_path(path: &std::path::Path, mode: &str) -> std::io::Result<Vec<u8>> {
    if mode != "120000" {
        return fs::read(path);
    }

    let target = fs::read_link(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(target.as_os_str().as_bytes().to_vec())
    }
    #[cfg(not(unix))]
    {
        Ok(target.to_string_lossy().into_owned().into_bytes())
    }
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
