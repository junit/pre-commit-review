use crate::git_policy::{output_bounded, GitOutputError};
use crate::impact_context::adapters::tree_sitter_rust::RustFileFacts;
use crate::impact_context::cache::integrity::{
    canonical_file_facts, file_fact_key_digest, payload_digest, validate_file_facts,
};
use crate::impact_context::index::model::FileFactKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tempfile::NamedTempFile;

const FILE_FACTS_MAGIC: &str = "pre-commit-review-file-facts";
const FILE_FACTS_ENVELOPE_SCHEMA: u16 = 1;
const DEFAULT_MAXIMUM_OBJECT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheLayout {
    pub root: PathBuf,
    pub repository_id: String,
    pub facts_dir: PathBuf,
    pub graphs_dir: PathBuf,
    pub staging_dir: PathBuf,
    pub locks_dir: PathBuf,
    pub quarantine_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileFactsEnvelope {
    pub(crate) magic: String,
    pub(crate) schema_version: u16,
    pub(crate) key: FileFactKey,
    pub(crate) payload_length: usize,
    pub(crate) payload_sha256: String,
    pub(crate) payload: RustFileFacts,
}

#[derive(Debug, Clone)]
pub struct FileFactsStore {
    layout: CacheLayout,
    maximum_object_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishResult {
    Published,
    Reused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheLookup<T> {
    Hit(T),
    Miss,
    Stale { code: String },
    Corrupt { code: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheError {
    pub code: &'static str,
    pub message: String,
}

impl CacheError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CacheError {}

impl CacheLayout {
    pub fn resolve(repository: &Path, override_root: Option<&Path>) -> Result<Self, CacheError> {
        if !repository.is_absolute() {
            return Err(CacheError::new(
                "repository-path-not-absolute",
                "repository path must be absolute",
            ));
        }
        let repository_input = fs::canonicalize(repository).map_err(|error| {
            CacheError::new(
                "repository-path-unavailable",
                format!("cannot canonicalize repository path: {error}"),
            )
        })?;
        let (worktree, git_common_dir) = repository_git_paths(&repository_input)?;
        let selected_root = if let Some(root) = override_root {
            root.to_path_buf()
        } else if let Some(root) = std::env::var_os("PRE_COMMIT_REVIEW_CACHE_DIR") {
            PathBuf::from(root)
        } else {
            platform_default_cache_root()?
        };
        if !selected_root.is_absolute() {
            return Err(CacheError::new(
                "cache-root-not-absolute",
                "cache root must be absolute",
            ));
        }
        let root = resolve_absolute_path(&selected_root)?;
        if root.starts_with(&git_common_dir) {
            return Err(CacheError::new(
                "cache-root-inside-git-directory",
                "cache root cannot be inside the Git common directory",
            ));
        }
        if root.starts_with(&worktree) {
            return Err(CacheError::new(
                "cache-root-inside-repository",
                "cache root cannot be inside the reviewed worktree",
            ));
        }
        if root.exists()
            && !fs::metadata(&root)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false)
        {
            return Err(CacheError::new(
                "cache-root-not-directory",
                "cache root exists but is not a directory",
            ));
        }

        let repository_id = repository_id(&git_common_dir);
        let repository_root = root.join("v2").join("repos").join(&repository_id);
        Ok(Self {
            root,
            repository_id,
            facts_dir: repository_root.join("facts"),
            graphs_dir: repository_root.join("graphs"),
            staging_dir: repository_root.join("staging"),
            locks_dir: repository_root.join("locks"),
            quarantine_dir: repository_root.join("quarantine"),
        })
    }

    pub(crate) fn ensure_private_directories(&self) -> Result<(), CacheError> {
        if !self.root.exists() {
            create_private_path(&self.root)?;
        }
        let v2 = self.root.join("v2");
        let repos = v2.join("repos");
        let repository_root = repos.join(&self.repository_id);
        for path in [
            &v2,
            &repos,
            &repository_root,
            &self.facts_dir,
            &self.graphs_dir,
            &self.staging_dir,
            &self.locks_dir,
            &self.quarantine_dir,
        ] {
            create_private_directory(path)?;
        }
        Ok(())
    }
}

impl FileFactsStore {
    pub fn new(layout: CacheLayout, maximum_object_bytes: usize) -> Result<Self, CacheError> {
        if maximum_object_bytes == 0 {
            return Err(CacheError::new(
                "cache-object-limit-invalid",
                "cache object limit must be positive",
            ));
        }
        Ok(Self {
            layout,
            maximum_object_bytes: maximum_object_bytes.min(DEFAULT_MAXIMUM_OBJECT_BYTES),
        })
    }

    pub fn layout(&self) -> &CacheLayout {
        &self.layout
    }

    pub fn object_path(&self, key: &FileFactKey) -> Result<PathBuf, CacheError> {
        let digest = file_fact_digest(key)?;
        Ok(self
            .layout
            .facts_dir
            .join("sha256")
            .join(&digest[..2])
            .join(format!("{digest}.facts")))
    }

    pub fn lookup(&self, key: &FileFactKey) -> Result<CacheLookup<RustFileFacts>, CacheError> {
        let path = self.object_path(key)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CacheLookup::Miss)
            }
            Err(error) => {
                return Err(CacheError::new(
                    "cache-object-metadata-unavailable",
                    format!("cannot inspect file facts object: {error}"),
                ))
            }
        };
        if !metadata.file_type().is_file() {
            return Ok(corrupt("file-facts-object-not-regular"));
        }
        let maximum_u64 = u64::try_from(self.maximum_object_bytes).unwrap_or(u64::MAX);
        if metadata.len() > maximum_u64 {
            return Ok(corrupt("file-facts-object-too-large"));
        }
        let mut file = open_regular_file_no_follow(&path).map_err(|error| {
            CacheError::new(
                "cache-object-open-failed",
                format!("cannot open file facts object: {error}"),
            )
        })?;
        let mut bytes = Vec::with_capacity(
            usize::try_from(metadata.len())
                .unwrap_or(self.maximum_object_bytes)
                .min(self.maximum_object_bytes),
        );
        Read::by_ref(&mut file)
            .take(maximum_u64.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| {
                CacheError::new(
                    "cache-object-read-failed",
                    format!("cannot read file facts object: {error}"),
                )
            })?;
        if bytes.len() > self.maximum_object_bytes {
            return Ok(corrupt("file-facts-object-too-large"));
        }
        let envelope: FileFactsEnvelope = match serde_json::from_slice(&bytes) {
            Ok(envelope) => envelope,
            Err(_) => return Ok(corrupt("file-facts-envelope-invalid")),
        };
        if envelope.magic != FILE_FACTS_MAGIC {
            return Ok(corrupt("file-facts-magic-mismatch"));
        }
        if envelope.schema_version != FILE_FACTS_ENVELOPE_SCHEMA {
            return Ok(corrupt("file-facts-schema-unsupported"));
        }
        if envelope.key.validate().is_err() {
            return Ok(corrupt("file-facts-key-invalid"));
        }
        if envelope.key != *key {
            return Ok(CacheLookup::Stale {
                code: "file-facts-key-mismatch".to_string(),
            });
        }
        if envelope.payload_length > self.maximum_object_bytes {
            return Ok(corrupt("file-facts-payload-too-large"));
        }
        let canonical_payload = canonical_file_facts(&envelope.payload);
        if canonical_payload != envelope.payload || validate_file_facts(&envelope.payload).is_err()
        {
            return Ok(corrupt("file-facts-payload-invalid"));
        }
        let payload_bytes = match serde_json::to_vec(&envelope.payload) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(corrupt("file-facts-payload-invalid")),
        };
        if payload_bytes.len() != envelope.payload_length {
            return Ok(corrupt("file-facts-payload-length-mismatch"));
        }
        if payload_digest(&payload_bytes) != envelope.payload_sha256 {
            return Ok(corrupt("file-facts-payload-checksum-mismatch"));
        }
        Ok(CacheLookup::Hit(envelope.payload))
    }

    pub fn publish(
        &self,
        key: &FileFactKey,
        facts: &RustFileFacts,
    ) -> Result<PublishResult, CacheError> {
        key.validate().map_err(|error| {
            CacheError::new(
                "file-facts-key-invalid",
                format!("invalid file facts key: {error}"),
            )
        })?;
        let facts = canonical_file_facts(facts);
        validate_file_facts(&facts).map_err(|error| {
            CacheError::new(
                "file-facts-payload-invalid",
                format!("invalid file facts payload: {error}"),
            )
        })?;
        match self.lookup(key)? {
            CacheLookup::Hit(existing) if existing == facts => return Ok(PublishResult::Reused),
            CacheLookup::Hit(_) | CacheLookup::Stale { .. } | CacheLookup::Corrupt { .. } => {
                return Err(CacheError::new(
                    "file-facts-object-conflict",
                    "an incompatible immutable file facts object already exists",
                ))
            }
            CacheLookup::Miss => {}
        }

        let payload_bytes = serde_json::to_vec(&facts).map_err(|error| {
            CacheError::new(
                "file-facts-encode-failed",
                format!("cannot encode file facts payload: {error}"),
            )
        })?;
        let envelope = FileFactsEnvelope {
            magic: FILE_FACTS_MAGIC.to_string(),
            schema_version: FILE_FACTS_ENVELOPE_SCHEMA,
            key: key.clone(),
            payload_length: payload_bytes.len(),
            payload_sha256: payload_digest(&payload_bytes),
            payload: facts.clone(),
        };
        let encoded = serde_json::to_vec(&envelope).map_err(|error| {
            CacheError::new(
                "file-facts-encode-failed",
                format!("cannot encode file facts envelope: {error}"),
            )
        })?;
        if encoded.len() > self.maximum_object_bytes {
            return Err(CacheError::new(
                "file-facts-object-too-large",
                "encoded file facts object exceeds the configured limit",
            ));
        }

        self.layout.ensure_private_directories()?;
        let final_path = self.object_path(key)?;
        let parent = final_path.parent().ok_or_else(|| {
            CacheError::new(
                "cache-object-path-invalid",
                "file facts object path has no parent",
            )
        })?;
        let sha256_dir = self.layout.facts_dir.join("sha256");
        create_private_directory(&sha256_dir)?;
        create_private_directory(parent)?;
        let mut temporary = NamedTempFile::new_in(parent).map_err(|error| {
            CacheError::new(
                "cache-object-temporary-create-failed",
                format!("cannot create file facts staging object: {error}"),
            )
        })?;
        set_private_file_permissions(temporary.as_file())?;
        temporary.write_all(&encoded).map_err(|error| {
            CacheError::new(
                "cache-object-write-failed",
                format!("cannot write file facts staging object: {error}"),
            )
        })?;
        temporary.as_file().sync_all().map_err(|error| {
            CacheError::new(
                "cache-object-sync-failed",
                format!("cannot sync file facts staging object: {error}"),
            )
        })?;

        match temporary.persist_noclobber(&final_path) {
            Ok(_) => {
                sync_directory(parent)?;
                Ok(PublishResult::Published)
            }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                match self.lookup(key)? {
                    CacheLookup::Hit(existing) if existing == facts => Ok(PublishResult::Reused),
                    _ => Err(CacheError::new(
                        "file-facts-object-conflict",
                        "concurrent writer published an incompatible file facts object",
                    )),
                }
            }
            Err(error) => Err(CacheError::new(
                "cache-object-publish-failed",
                format!("cannot publish file facts object: {}", error.error),
            )),
        }
    }
}

pub fn file_fact_digest(key: &FileFactKey) -> Result<String, CacheError> {
    file_fact_key_digest(key).map_err(|error| {
        CacheError::new(
            "file-facts-key-invalid",
            format!("invalid file facts key: {error}"),
        )
    })
}

fn corrupt<T>(code: &str) -> CacheLookup<T> {
    CacheLookup::Corrupt {
        code: code.to_string(),
    }
}

fn repository_git_paths(repository: &Path) -> Result<(PathBuf, PathBuf), CacheError> {
    let mut command = Command::new("git");
    command
        .current_dir(repository)
        .args(["rev-parse", "--show-toplevel", "--git-common-dir"]);
    let output =
        output_bounded(&mut command, Duration::from_secs(5)).map_err(|error| match error {
            GitOutputError::DeadlineExceeded => CacheError::new(
                "repository-identity-deadline-exhausted",
                "Git repository identity lookup timed out",
            ),
            GitOutputError::OutputLimitExceeded => CacheError::new(
                "repository-identity-output-limit-exhausted",
                "Git repository identity output exceeded the capture limit",
            ),
            GitOutputError::Io(error) => CacheError::new(
                "repository-identity-unavailable",
                format!("cannot inspect Git repository identity: {error}"),
            ),
        })?;
    if !output.status.success() {
        return Err(CacheError::new(
            "repository-identity-unavailable",
            "cannot inspect Git repository identity",
        ));
    }
    let text = std::str::from_utf8(&output.stdout).map_err(|_| {
        CacheError::new(
            "repository-identity-invalid",
            "Git repository identity is not UTF-8",
        )
    })?;
    let lines = text.lines().collect::<Vec<_>>();
    if lines.len() != 2 {
        return Err(CacheError::new(
            "repository-identity-invalid",
            "Git repository identity output has an unexpected shape",
        ));
    }
    let worktree = fs::canonicalize(lines[0]).map_err(|error| {
        CacheError::new(
            "repository-identity-invalid",
            format!("cannot canonicalize Git worktree: {error}"),
        )
    })?;
    let common = PathBuf::from(lines[1]);
    let common = if common.is_absolute() {
        common
    } else {
        repository.join(common)
    };
    let common = fs::canonicalize(common).map_err(|error| {
        CacheError::new(
            "repository-identity-invalid",
            format!("cannot canonicalize Git common directory: {error}"),
        )
    })?;
    Ok((worktree, common))
}

fn repository_id(git_common_dir: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(b"pre-commit-review-repository-cache/v2");
    let identity = path_identity_bytes(git_common_dir);
    digest.update((identity.len() as u64).to_be_bytes());
    digest.update(identity);
    format!("{:x}", digest.finalize())
}

#[cfg(unix)]
fn path_identity_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn path_identity_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

pub(crate) fn platform_default_cache_root() -> Result<PathBuf, CacheError> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").ok_or_else(|| {
            CacheError::new(
                "cache-root-unavailable",
                "HOME is unavailable for the platform cache default",
            )
        })?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Caches")
            .join("pre-commit-review"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(root) = std::env::var_os("XDG_CACHE_HOME") {
            return Ok(PathBuf::from(root).join("pre-commit-review"));
        }
        let home = std::env::var_os("HOME").ok_or_else(|| {
            CacheError::new(
                "cache-root-unavailable",
                "HOME is unavailable for the platform cache default",
            )
        })?;
        Ok(PathBuf::from(home).join(".cache").join("pre-commit-review"))
    }
    #[cfg(windows)]
    {
        let root = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
            CacheError::new(
                "cache-root-unavailable",
                "LOCALAPPDATA is unavailable for the platform cache default",
            )
        })?;
        Ok(PathBuf::from(root).join("pre-commit-review"))
    }
}

pub(crate) fn resolve_absolute_path(path: &Path) -> Result<PathBuf, CacheError> {
    let normalized = normalize_absolute_path(path)?;
    if normalized.exists() {
        return fs::canonicalize(&normalized).map_err(|error| {
            CacheError::new(
                "cache-root-unavailable",
                format!("cannot canonicalize cache root: {error}"),
            )
        });
    }
    let mut existing = normalized.as_path();
    let mut suffix = Vec::<OsString>::new();
    while !existing.exists() {
        let name = existing.file_name().ok_or_else(|| {
            CacheError::new(
                "cache-root-unavailable",
                "cache root has no existing ancestor",
            )
        })?;
        suffix.push(name.to_os_string());
        existing = existing.parent().ok_or_else(|| {
            CacheError::new(
                "cache-root-unavailable",
                "cache root has no existing ancestor",
            )
        })?;
    }
    let mut resolved = fs::canonicalize(existing).map_err(|error| {
        CacheError::new(
            "cache-root-unavailable",
            format!("cannot canonicalize cache root ancestor: {error}"),
        )
    })?;
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf, CacheError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(CacheError::new(
                        "cache-root-invalid",
                        "cache root escapes its filesystem root",
                    ));
                }
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    Ok(normalized)
}

pub(crate) fn create_private_directory(path: &Path) -> Result<(), CacheError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() || is_symlink_or_reparse(path, &metadata) {
                return Err(CacheError::new(
                    "cache-directory-unsafe",
                    format!(
                        "cache directory is not a regular directory: {}",
                        path.display()
                    ),
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Err(error) = fs::create_dir(path) {
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(CacheError::new(
                        "cache-directory-create-failed",
                        format!("cannot create cache directory {}: {error}", path.display()),
                    ));
                }
            }
            let metadata = fs::symlink_metadata(path).map_err(|error| {
                CacheError::new(
                    "cache-directory-unavailable",
                    format!("cannot inspect cache directory {}: {error}", path.display()),
                )
            })?;
            if !metadata.file_type().is_dir() || is_symlink_or_reparse(path, &metadata) {
                return Err(CacheError::new(
                    "cache-directory-unsafe",
                    format!(
                        "cache directory is not a regular directory: {}",
                        path.display()
                    ),
                ));
            }
        }
        Err(error) => {
            return Err(CacheError::new(
                "cache-directory-unavailable",
                format!("cannot inspect cache directory {}: {error}", path.display()),
            ))
        }
    }
    set_private_directory_permissions(path)
}

#[cfg(not(windows))]
pub(crate) fn is_symlink_or_reparse(_path: &Path, metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
pub(crate) fn is_symlink_or_reparse(path: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileAttributesW, FILE_ATTRIBUTE_REPARSE_POINT, INVALID_FILE_ATTRIBUTES,
    };

    if metadata.file_type().is_symlink() {
        return true;
    }
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    wide.push(0);
    // SAFETY: `wide` is a NUL-terminated path buffer valid for the duration of the call.
    let attributes = unsafe { GetFileAttributesW(wide.as_ptr()) };
    attributes == INVALID_FILE_ATTRIBUTES || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn create_private_path(path: &Path) -> Result<(), CacheError> {
    let mut existing = path;
    let mut suffix = Vec::<OsString>::new();
    while !existing.exists() {
        let name = existing.file_name().ok_or_else(|| {
            CacheError::new(
                "cache-directory-create-failed",
                "cache directory has no existing ancestor",
            )
        })?;
        suffix.push(name.to_os_string());
        existing = existing.parent().ok_or_else(|| {
            CacheError::new(
                "cache-directory-create-failed",
                "cache directory has no existing ancestor",
            )
        })?;
    }
    let mut current = existing.to_path_buf();
    for component in suffix.into_iter().rev() {
        current.push(component);
        create_private_directory(&current)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), CacheError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
        CacheError::new(
            "cache-directory-permission-failed",
            format!("cannot make cache directory private: {error}"),
        )
    })
}

#[cfg(windows)]
fn set_private_directory_permissions(_path: &Path) -> Result<(), CacheError> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn set_private_file_permissions(file: &File) -> Result<(), CacheError> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|error| {
            CacheError::new(
                "cache-object-permission-failed",
                format!("cannot make cache object private: {error}"),
            )
        })
}

#[cfg(windows)]
pub(crate) fn set_private_file_permissions(_file: &File) -> Result<(), CacheError> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn open_regular_file_no_follow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "cache object is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RegularFileFingerprint {
    device: u64,
    inode: u64,
    size: u64,
    mode: u32,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[cfg(unix)]
impl RegularFileFingerprint {
    pub(crate) fn size(&self) -> u64 {
        self.size
    }
}

#[cfg(unix)]
pub(crate) fn opened_regular_file_fingerprint(
    file: &File,
) -> std::io::Result<RegularFileFingerprint> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "opened path is not a regular file",
        ));
    }
    Ok(RegularFileFingerprint {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.len(),
        mode: metadata.mode(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    })
}

#[cfg(windows)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RegularFileFingerprint {
    volume: u32,
    index: u64,
    size: u64,
    attributes: u32,
    modified: i64,
    created: i64,
    changed: i64,
}

#[cfg(windows)]
impl RegularFileFingerprint {
    pub(crate) fn size(&self) -> u64 {
        self.size
    }
}

#[cfg(windows)]
pub(crate) fn opened_regular_file_fingerprint(
    file: &File,
) -> std::io::Result<RegularFileFingerprint> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileBasicInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_BASIC_INFO,
    };

    let handle = file.as_raw_handle() as _;
    let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    // SAFETY: `file` owns a valid handle for this call and `information` points to writable,
    // correctly sized storage that is initialized only after the API reports success.
    let succeeded = unsafe { GetFileInformationByHandle(handle, information.as_mut_ptr()) };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the successful API call initialized the complete output structure.
    let information = unsafe { information.assume_init() };
    let mut basic_information = std::mem::MaybeUninit::<FILE_BASIC_INFO>::zeroed();
    let basic_information_size = u32::try_from(std::mem::size_of::<FILE_BASIC_INFO>())
        .expect("FILE_BASIC_INFO size fits in a Windows DWORD");
    // SAFETY: `handle` remains owned by `file`; the class and buffer size match
    // `FILE_BASIC_INFO`, and the buffer is only assumed initialized after success.
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            basic_information.as_mut_ptr().cast(),
            basic_information_size,
        )
    };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the successful API call initialized the complete output structure.
    let basic_information = unsafe { basic_information.assume_init() };
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || basic_information.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "opened path is a reparse point",
        ));
    }
    let combine = |high: u32, low: u32| (u64::from(high) << 32) | u64::from(low);
    Ok(RegularFileFingerprint {
        volume: information.dwVolumeSerialNumber,
        index: combine(information.nFileIndexHigh, information.nFileIndexLow),
        size: combine(information.nFileSizeHigh, information.nFileSizeLow),
        attributes: basic_information.FileAttributes,
        modified: basic_information.LastWriteTime,
        created: basic_information.CreationTime,
        changed: basic_information.ChangeTime,
    })
}

#[cfg(windows)]
pub(crate) fn open_regular_file_no_follow(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "cache object is not a regular file",
        ));
    }
    Ok(file)
}

#[cfg(unix)]
pub(crate) fn sync_directory(path: &Path) -> Result<(), CacheError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            CacheError::new(
                "cache-directory-sync-failed",
                format!("cannot sync cache object directory: {error}"),
            )
        })
}

#[cfg(windows)]
pub(crate) fn sync_directory(_path: &Path) -> Result<(), CacheError> {
    Ok(())
}
