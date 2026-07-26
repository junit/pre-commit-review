use crate::git_policy::configure_read_only;
use crate::review_scope::ReviewSource;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use tempfile::TempDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotLimits {
    pub max_files: usize,
    pub max_bytes: u64,
}

#[derive(Debug)]
pub struct CandidateSnapshot {
    root: TempDir,
    pub snapshot_id: String,
    pub sha256: String,
    pub files: usize,
    pub bytes: u64,
    limits: SnapshotLimits,
    digest_modes: HashMap<Vec<u8>, u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotError {
    message: String,
}

impl SnapshotError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SnapshotError {}

#[derive(Debug)]
struct GitEntry {
    path: PathBuf,
    mode: String,
    object_id: String,
}

#[derive(Debug)]
struct SnapshotInfo {
    sha256: String,
    files: usize,
    bytes: u64,
    modes: HashMap<Vec<u8>, u32>,
}

impl CandidateSnapshot {
    pub fn materialize(
        repository: &Path,
        source: ReviewSource,
        limits: SnapshotLimits,
    ) -> Result<Self, SnapshotError> {
        let repository = fs::canonicalize(repository)
            .map_err(|error| SnapshotError::new(format!("cannot resolve repository: {error}")))?;
        let root = tempfile::tempdir()
            .map_err(|error| SnapshotError::new(format!("cannot create snapshot: {error}")))?;
        match source {
            ReviewSource::Staged => {
                let entries =
                    parse_index_entries(&run_git(&repository, &["ls-files", "--stage", "-z"])?)?;
                materialize_blobs(&repository, root.path(), &entries, limits)?;
            }
            ReviewSource::Branch => {
                let entries = parse_tree_entries(&run_git(
                    &repository,
                    &["ls-tree", "-rz", "--full-tree", "HEAD"],
                )?)?;
                materialize_blobs(&repository, root.path(), &entries, limits)?;
            }
            ReviewSource::Unstaged => {
                let paths = run_git(&repository, &["ls-files", "--cached", "-z"])?;
                materialize_unstaged(&repository, root.path(), &paths, limits)?;
            }
        }
        let info = snapshot_info(root.path(), limits, None)?;
        make_snapshot_read_only(root.path())?;
        let snapshot_id = info.sha256[..16].to_string();
        Ok(Self {
            root,
            snapshot_id,
            sha256: info.sha256,
            files: info.files,
            bytes: info.bytes,
            limits,
            digest_modes: info.modes,
        })
    }

    pub fn path(&self) -> &Path {
        self.root.path()
    }

    pub fn verify_unchanged(&self) -> Result<(), SnapshotError> {
        verify_read_only(self.path())?;
        let observed = snapshot_info(self.path(), self.limits, Some(&self.digest_modes))?;
        if observed.sha256 != self.sha256
            || observed.files != self.files
            || observed.bytes != self.bytes
        {
            return Err(SnapshotError::new(
                "analysis snapshot changed after materialization",
            ));
        }
        Ok(())
    }
}

impl Drop for CandidateSnapshot {
    fn drop(&mut self) {
        make_snapshot_writable(self.root.path());
    }
}

fn run_git(repository: &Path, arguments: &[&str]) -> Result<Vec<u8>, SnapshotError> {
    let mut command = Command::new("git");
    configure_read_only(&mut command);
    let output = command
        .args(arguments)
        .current_dir(repository)
        .output()
        .map_err(|error| SnapshotError::new(format!("Git snapshot command failed: {error}")))?;
    if !output.status.success() {
        return Err(SnapshotError::new(format!(
            "Git snapshot command failed: {}",
            bounded_detail(&output.stderr, "unknown Git error")
        )));
    }
    Ok(output.stdout)
}

fn bounded_detail(value: &[u8], fallback: &str) -> String {
    let detail = String::from_utf8_lossy(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let detail = detail.chars().take(500).collect::<String>();
    if detail.is_empty() {
        fallback.to_string()
    } else {
        detail
    }
}

fn parse_index_entries(raw: &[u8]) -> Result<Vec<GitEntry>, SnapshotError> {
    let mut entries = Vec::new();
    for record in raw
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let (metadata, raw_path) = split_once(record, b'\t')
            .ok_or_else(|| SnapshotError::new("cannot parse staged Git index entry"))?;
        let fields = metadata.split(|byte| *byte == b' ').collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err(SnapshotError::new("cannot parse staged Git index entry"));
        }
        let mode = ascii(fields[0], "cannot parse staged Git index entry")?;
        let object_id = ascii(fields[1], "cannot parse staged Git index entry")?;
        let stage = ascii(fields[2], "cannot parse staged Git index entry")?;
        if stage != "0" {
            return Err(SnapshotError::new(
                "cannot analyze an index with unmerged entries",
            ));
        }
        validate_object_id(&object_id)?;
        entries.push(GitEntry {
            path: safe_relative_path(raw_path)?,
            mode,
            object_id,
        });
    }
    Ok(entries)
}

fn parse_tree_entries(raw: &[u8]) -> Result<Vec<GitEntry>, SnapshotError> {
    let mut entries = Vec::new();
    for record in raw
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let (metadata, raw_path) = split_once(record, b'\t')
            .ok_or_else(|| SnapshotError::new("cannot parse branch Git tree entry"))?;
        let fields = metadata.split(|byte| *byte == b' ').collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err(SnapshotError::new("cannot parse branch Git tree entry"));
        }
        let mode = ascii(fields[0], "cannot parse branch Git tree entry")?;
        let object_type = ascii(fields[1], "cannot parse branch Git tree entry")?;
        let object_id = ascii(fields[2], "cannot parse branch Git tree entry")?;
        if object_type != "blob" {
            continue;
        }
        validate_object_id(&object_id)?;
        entries.push(GitEntry {
            path: safe_relative_path(raw_path)?,
            mode,
            object_id,
        });
    }
    Ok(entries)
}

fn split_once(value: &[u8], delimiter: u8) -> Option<(&[u8], &[u8])> {
    value
        .iter()
        .position(|byte| *byte == delimiter)
        .map(|index| (&value[..index], &value[index + 1..]))
}

fn ascii(value: &[u8], error: &str) -> Result<String, SnapshotError> {
    std::str::from_utf8(value)
        .map(str::to_owned)
        .map_err(|_| SnapshotError::new(error))
}

fn validate_object_id(value: &str) -> Result<(), SnapshotError> {
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SnapshotError::new("Git returned an invalid object id"));
    }
    Ok(())
}

fn safe_relative_path(raw: &[u8]) -> Result<PathBuf, SnapshotError> {
    if raw.is_empty() {
        return Err(SnapshotError::new(
            "Git contains a path that escapes the temporary snapshot",
        ));
    }
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(OsString::from_vec(raw.to_vec()))
    };
    #[cfg(not(unix))]
    let path = PathBuf::from(
        std::str::from_utf8(raw)
            .map_err(|_| SnapshotError::new("Git contains a non-UTF-8 path"))?,
    );
    let mut normal_components = 0;
    for component in path.components() {
        match component {
            Component::Normal(_) => normal_components += 1,
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(SnapshotError::new(
                    "Git contains a path that escapes the temporary snapshot",
                ));
            }
        }
    }
    if normal_components == 0 {
        return Err(SnapshotError::new(
            "Git contains a path that escapes the temporary snapshot",
        ));
    }
    Ok(path)
}

fn materialize_blobs(
    repository: &Path,
    snapshot_root: &Path,
    entries: &[GitEntry],
    limits: SnapshotLimits,
) -> Result<(), SnapshotError> {
    let materialized_files = entries
        .iter()
        .filter(|entry| entry.mode != "160000")
        .count();
    if materialized_files > limits.max_files {
        return Err(SnapshotError::new(format!(
            "analysis snapshot exceeds the {}-file profile limit",
            limits.max_files
        )));
    }
    let mut stderr = tempfile::tempfile()
        .map_err(|error| SnapshotError::new(format!("cannot capture git cat-file: {error}")))?;
    let stderr_child = stderr
        .try_clone()
        .map_err(|error| SnapshotError::new(format!("cannot capture git cat-file: {error}")))?;
    let mut command = Command::new("git");
    configure_read_only(&mut command);
    let mut child = command
        .args(["cat-file", "--batch"])
        .current_dir(repository)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr_child))
        .spawn()
        .map_err(|error| SnapshotError::new(format!("cannot start git cat-file: {error}")))?;
    let child_stdin = child
        .stdin
        .take()
        .ok_or_else(|| SnapshotError::new("cannot open git cat-file batch input"))?;
    let child_stdout = child
        .stdout
        .take()
        .ok_or_else(|| SnapshotError::new("cannot open git cat-file batch output"))?;
    let mut input = BufWriter::new(child_stdin);
    let mut output = BufReader::new(child_stdout);
    let result = materialize_batch_entries(
        snapshot_root,
        entries,
        limits.max_bytes,
        &mut input,
        &mut output,
    );
    drop(input);
    if let Err(error) = result {
        terminate_child(&mut child);
        return Err(error);
    }
    let status = child
        .wait()
        .map_err(|error| SnapshotError::new(format!("cannot wait for git cat-file: {error}")))?;
    if !status.success() {
        stderr.seek(SeekFrom::Start(0)).ok();
        let mut detail = Vec::new();
        stderr.take(500).read_to_end(&mut detail).ok();
        return Err(SnapshotError::new(format!(
            "git cat-file failed while building snapshot: {}",
            bounded_detail(&detail, "unknown Git error")
        )));
    }
    Ok(())
}

fn materialize_batch_entries(
    snapshot_root: &Path,
    entries: &[GitEntry],
    max_bytes: u64,
    input: &mut BufWriter<impl Write>,
    output: &mut BufReader<impl Read>,
) -> Result<(), SnapshotError> {
    let mut total_bytes = 0_u64;
    for entry in entries {
        if entry.mode == "160000" {
            continue;
        }
        if !matches!(entry.mode.as_str(), "100644" | "100755" | "120000") {
            return Err(SnapshotError::new(format!(
                "unsupported tracked file mode in snapshot: {}",
                entry.mode
            )));
        }
        writeln!(input, "{}", entry.object_id)
            .and_then(|_| input.flush())
            .map_err(|error| SnapshotError::new(format!("cannot query git blob: {error}")))?;
        let remaining = max_bytes.saturating_sub(total_bytes);
        let content = read_batch_blob(output, &entry.object_id, remaining)?;
        total_bytes = total_bytes
            .checked_add(content.len() as u64)
            .ok_or_else(|| SnapshotError::new("analysis snapshot byte count overflow"))?;
        let destination = snapshot_root.join(&entry.path);
        create_parent(&destination)?;
        match entry.mode.as_str() {
            "120000" => create_symlink_from_bytes(&content, &destination)?,
            "100755" => write_file(&destination, &content, 0o755)?,
            "100644" => write_file(&destination, &content, 0o644)?,
            _ => unreachable!(),
        }
    }
    Ok(())
}

fn read_batch_blob(
    stream: &mut BufReader<impl Read>,
    expected_object: &str,
    remaining_bytes: u64,
) -> Result<Vec<u8>, SnapshotError> {
    let mut header = Vec::new();
    stream
        .by_ref()
        .take(1_025)
        .read_until(b'\n', &mut header)
        .map_err(|error| SnapshotError::new(format!("cannot read git blob header: {error}")))?;
    if header.is_empty() {
        return Err(SnapshotError::new(
            "git cat-file ended before returning a requested blob",
        ));
    }
    if header.len() > 1_024 || header.last() != Some(&b'\n') {
        return Err(SnapshotError::new(
            "git cat-file returned an invalid batch header",
        ));
    }
    header.pop();
    let fields = header.split(|byte| *byte == b' ').collect::<Vec<_>>();
    if fields.len() == 2 && fields[1] == b"missing" {
        return Err(SnapshotError::new(
            "a Git blob needed for the analysis snapshot is missing locally",
        ));
    }
    if fields.len() != 3 {
        return Err(SnapshotError::new(
            "git cat-file returned an invalid batch header",
        ));
    }
    let object_id = ascii(fields[0], "git cat-file returned an invalid object id")?;
    let object_type = ascii(fields[1], "git cat-file returned an invalid object type")?;
    let size = ascii(fields[2], "git cat-file returned an invalid blob size")?
        .parse::<u64>()
        .map_err(|_| SnapshotError::new("git cat-file returned an invalid blob size"))?;
    if object_id != expected_object || object_type != "blob" {
        return Err(SnapshotError::new(
            "git cat-file returned a different object than requested",
        ));
    }
    if size > remaining_bytes {
        return Err(SnapshotError::new(
            "Git blob exceeds the remaining snapshot byte limit",
        ));
    }
    let size = usize::try_from(size)
        .map_err(|_| SnapshotError::new("Git blob size exceeds this platform"))?;
    let mut content = vec![0; size];
    stream
        .read_exact(&mut content)
        .map_err(|_| SnapshotError::new("git cat-file returned a truncated blob"))?;
    let mut terminator = [0_u8; 1];
    stream
        .read_exact(&mut terminator)
        .map_err(|_| SnapshotError::new("git cat-file returned a truncated blob"))?;
    if terminator != [b'\n'] {
        return Err(SnapshotError::new("git cat-file returned a truncated blob"));
    }
    Ok(content)
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn materialize_unstaged(
    repository: &Path,
    snapshot_root: &Path,
    raw_paths: &[u8],
    limits: SnapshotLimits,
) -> Result<(), SnapshotError> {
    let paths = raw_paths
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    let mut total_bytes = 0_u64;
    let mut materialized_files = 0_usize;
    for raw_path in paths {
        let relative = safe_relative_path(raw_path)?;
        let source = repository.join(&relative);
        let destination = snapshot_root.join(&relative);
        let metadata = match fs::symlink_metadata(&source) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(SnapshotError::new(format!(
                    "cannot inspect tracked working-tree path: {error}"
                )));
            }
        };
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            continue;
        }
        if !file_type.is_symlink() && !file_type.is_file() {
            return Err(SnapshotError::new(
                "tracked working-tree path is not a regular file or symlink",
            ));
        }
        materialized_files = materialized_files.saturating_add(1);
        if materialized_files > limits.max_files {
            return Err(SnapshotError::new(format!(
                "analysis snapshot exceeds the {}-file profile limit",
                limits.max_files
            )));
        }
        create_parent(&destination)?;
        if file_type.is_symlink() {
            let target = fs::read_link(&source).map_err(|error| {
                SnapshotError::new(format!("cannot read tracked symlink: {error}"))
            })?;
            let target_bytes = os_path_bytes(&target);
            total_bytes = checked_snapshot_bytes(total_bytes, target_bytes.len() as u64, limits)?;
            create_symlink(&target, &destination)?;
        } else if file_type.is_file() {
            if metadata.len() > limits.max_bytes.saturating_sub(total_bytes) {
                return Err(SnapshotError::new(format!(
                    "analysis snapshot exceeds the {}-byte profile limit",
                    limits.max_bytes
                )));
            }
            let copied = copy_bounded(
                &source,
                &destination,
                limits.max_bytes.saturating_sub(total_bytes),
            )?;
            total_bytes = checked_snapshot_bytes(total_bytes, copied, limits)?;
            set_mode(&destination, metadata_mode(&metadata))?;
        }
    }
    Ok(())
}

fn checked_snapshot_bytes(
    current: u64,
    additional: u64,
    limits: SnapshotLimits,
) -> Result<u64, SnapshotError> {
    let total = current
        .checked_add(additional)
        .ok_or_else(|| SnapshotError::new("analysis snapshot byte count overflow"))?;
    if total > limits.max_bytes {
        return Err(SnapshotError::new(format!(
            "analysis snapshot exceeds the {}-byte profile limit",
            limits.max_bytes
        )));
    }
    Ok(total)
}

fn copy_bounded(source: &Path, destination: &Path, remaining: u64) -> Result<u64, SnapshotError> {
    let input = File::open(source)
        .map_err(|error| SnapshotError::new(format!("cannot read tracked file: {error}")))?;
    let mut input = input.take(remaining.saturating_add(1));
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|error| SnapshotError::new(format!("cannot create snapshot file: {error}")))?;
    let copied = std::io::copy(&mut input, &mut output)
        .map_err(|error| SnapshotError::new(format!("cannot copy tracked file: {error}")))?;
    if copied > remaining {
        return Err(SnapshotError::new(
            "tracked file exceeds the remaining snapshot byte limit",
        ));
    }
    Ok(copied)
}

fn create_parent(path: &Path) -> Result<(), SnapshotError> {
    let parent = path
        .parent()
        .ok_or_else(|| SnapshotError::new("snapshot path has no parent"))?;
    fs::create_dir_all(parent)
        .map_err(|error| SnapshotError::new(format!("cannot create snapshot directory: {error}")))
}

fn write_file(path: &Path, content: &[u8], mode: u32) -> Result<(), SnapshotError> {
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| SnapshotError::new(format!("cannot create snapshot file: {error}")))?;
    output
        .write_all(content)
        .map_err(|error| SnapshotError::new(format!("cannot write snapshot file: {error}")))?;
    set_mode(path, mode)
}

fn create_symlink_from_bytes(target: &[u8], destination: &Path) -> Result<(), SnapshotError> {
    #[cfg(unix)]
    let target = {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(OsString::from_vec(target.to_vec()))
    };
    #[cfg(not(unix))]
    let target = PathBuf::from(
        std::str::from_utf8(target)
            .map_err(|_| SnapshotError::new("Git symlink target is not valid UTF-8"))?,
    );
    create_symlink(&target, destination)
}

#[cfg(unix)]
fn create_symlink(target: &Path, destination: &Path) -> Result<(), SnapshotError> {
    std::os::unix::fs::symlink(target, destination)
        .map_err(|error| SnapshotError::new(format!("cannot create snapshot symlink: {error}")))
}

#[cfg(windows)]
fn create_symlink(target: &Path, destination: &Path) -> Result<(), SnapshotError> {
    std::os::windows::fs::symlink_file(target, destination)
        .map_err(|error| SnapshotError::new(format!("cannot create snapshot symlink: {error}")))
}

#[cfg(not(any(unix, windows)))]
fn create_symlink(_target: &Path, _destination: &Path) -> Result<(), SnapshotError> {
    Err(SnapshotError::new(
        "snapshot symlinks are unsupported on this platform",
    ))
}

fn snapshot_info(
    root: &Path,
    limits: SnapshotLimits,
    expected_modes: Option<&HashMap<Vec<u8>, u32>>,
) -> Result<SnapshotInfo, SnapshotError> {
    let mut state = HashState {
        digest: Sha256::new(),
        files: 0,
        bytes: 0,
        modes: HashMap::new(),
        limits,
        expected_modes,
    };
    hash_directory(root, root, &mut state)?;
    Ok(SnapshotInfo {
        sha256: format!("{:x}", state.digest.finalize()),
        files: state.files,
        bytes: state.bytes,
        modes: state.modes,
    })
}

struct HashState<'a> {
    digest: Sha256,
    files: usize,
    bytes: u64,
    modes: HashMap<Vec<u8>, u32>,
    limits: SnapshotLimits,
    expected_modes: Option<&'a HashMap<Vec<u8>, u32>>,
}

fn hash_directory(
    root: &Path,
    directory: &Path,
    state: &mut HashState<'_>,
) -> Result<(), SnapshotError> {
    let mut directories = Vec::new();
    let mut symlink_directories = Vec::new();
    let mut files = Vec::new();
    let entries = fs::read_dir(directory)
        .map_err(|error| SnapshotError::new(format!("cannot inspect snapshot: {error}")))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| SnapshotError::new(format!("cannot inspect snapshot: {error}")))?;
        let file_type = entry
            .file_type()
            .map_err(|error| SnapshotError::new(format!("cannot inspect snapshot: {error}")))?;
        if file_type.is_symlink() && entry.path().is_dir() {
            symlink_directories.push(entry);
        } else if file_type.is_dir() {
            directories.push(entry);
        } else {
            files.push(entry);
        }
    }
    sort_entries(&mut directories);
    sort_entries(&mut symlink_directories);
    sort_entries(&mut files);
    for entry in symlink_directories.into_iter().chain(files) {
        hash_entry(root, &entry.path(), state)?;
    }
    for entry in directories {
        hash_directory(root, &entry.path(), state)?;
    }
    Ok(())
}

fn sort_entries(entries: &mut [fs::DirEntry]) {
    entries.sort_by_key(fs::DirEntry::file_name);
}

fn hash_entry(root: &Path, path: &Path, state: &mut HashState<'_>) -> Result<(), SnapshotError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| SnapshotError::new("snapshot path escaped its root"))?;
    let relative_bytes = digest_path_bytes(relative);
    state.files += 1;
    if state.files > state.limits.max_files {
        return Err(SnapshotError::new(format!(
            "analysis snapshot exceeds the {}-file profile limit",
            state.limits.max_files
        )));
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| SnapshotError::new(format!("cannot inspect snapshot entry: {error}")))?;
    let observed_mode = metadata_mode(&metadata);
    let digest_mode = state
        .expected_modes
        .and_then(|modes| modes.get(&relative_bytes))
        .copied()
        .unwrap_or(observed_mode);
    state.modes.insert(relative_bytes.clone(), observed_mode);
    state.digest.update(&relative_bytes);
    state.digest.update([0]);
    state.digest.update(digest_mode.to_string().as_bytes());
    state.digest.update([0]);
    if metadata.file_type().is_symlink() {
        let target_bytes = validate_symlink(path, root)?;
        state.bytes = checked_snapshot_bytes(state.bytes, target_bytes.len() as u64, state.limits)?;
        state.digest.update(b"symlink\0");
        state.digest.update(&target_bytes);
    } else if metadata.file_type().is_file() {
        state.digest.update(b"file\0");
        let mut input = File::open(path)
            .map_err(|error| SnapshotError::new(format!("cannot hash snapshot file: {error}")))?;
        let mut buffer = [0_u8; 1024 * 1024];
        loop {
            let read = input.read(&mut buffer).map_err(|error| {
                SnapshotError::new(format!("cannot hash snapshot file: {error}"))
            })?;
            if read == 0 {
                break;
            }
            state.bytes = checked_snapshot_bytes(state.bytes, read as u64, state.limits)?;
            state.digest.update(&buffer[..read]);
        }
    } else {
        return Err(SnapshotError::new(
            "analysis snapshot contains an unsupported file type",
        ));
    }
    state.digest.update([0]);
    Ok(())
}

fn validate_symlink(path: &Path, root: &Path) -> Result<Vec<u8>, SnapshotError> {
    let target = fs::read_link(path)
        .map_err(|error| SnapshotError::new(format!("cannot read snapshot symlink: {error}")))?;
    if target.is_absolute() {
        return Err(SnapshotError::new(
            "analysis snapshot contains an absolute symlink",
        ));
    }
    let parent = path
        .parent()
        .and_then(|parent| parent.strip_prefix(root).ok())
        .ok_or_else(|| SnapshotError::new("snapshot symlink escaped its root"))?;
    let mut pending = VecDeque::new();
    append_components(&mut pending, parent)?;
    append_components(&mut pending, &target)?;
    let mut resolved = PathBuf::new();
    let mut followed = 0_u8;
    while let Some(part) = pending.pop_front() {
        match part {
            OwnedComponent::Current => {}
            OwnedComponent::Parent => {
                if !resolved.pop() {
                    return Err(SnapshotError::new(
                        "analysis snapshot contains a symlink that escapes the snapshot",
                    ));
                }
            }
            OwnedComponent::Normal(value) => {
                resolved.push(value);
                let candidate = root.join(&resolved);
                if fs::symlink_metadata(&candidate)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
                {
                    followed = followed.saturating_add(1);
                    if followed > 40 {
                        return Err(SnapshotError::new(
                            "analysis snapshot contains a symlink loop",
                        ));
                    }
                    let nested = fs::read_link(&candidate).map_err(|error| {
                        SnapshotError::new(format!("cannot read snapshot symlink: {error}"))
                    })?;
                    if nested.is_absolute() {
                        return Err(SnapshotError::new(
                            "analysis snapshot contains an absolute symlink",
                        ));
                    }
                    resolved.pop();
                    prepend_components(&mut pending, &nested)?;
                }
            }
        }
    }
    Ok(os_path_bytes(&target))
}

enum OwnedComponent {
    Current,
    Parent,
    Normal(OsString),
}

fn append_components(
    queue: &mut VecDeque<OwnedComponent>,
    path: &Path,
) -> Result<(), SnapshotError> {
    for component in path.components() {
        queue.push_back(owned_component(component)?);
    }
    Ok(())
}

fn prepend_components(
    queue: &mut VecDeque<OwnedComponent>,
    path: &Path,
) -> Result<(), SnapshotError> {
    let mut values = path
        .components()
        .map(owned_component)
        .collect::<Result<Vec<_>, _>>()?;
    while let Some(value) = values.pop() {
        queue.push_front(value);
    }
    Ok(())
}

fn owned_component(component: Component<'_>) -> Result<OwnedComponent, SnapshotError> {
    match component {
        Component::CurDir => Ok(OwnedComponent::Current),
        Component::ParentDir => Ok(OwnedComponent::Parent),
        Component::Normal(value) => Ok(OwnedComponent::Normal(value.to_os_string())),
        Component::RootDir | Component::Prefix(_) => Err(SnapshotError::new(
            "analysis snapshot contains an absolute symlink",
        )),
    }
}

fn digest_path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().replace('\\', "/").into_bytes()
    }
}

fn os_path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().into_owned().into_bytes()
    }
}

#[cfg(unix)]
fn metadata_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn metadata_mode(metadata: &fs::Metadata) -> u32 {
    if metadata.permissions().readonly() {
        0o444
    } else {
        0o644
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), SnapshotError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| SnapshotError::new(format!("cannot set snapshot permissions: {error}")))
}

#[cfg(not(unix))]
fn set_mode(path: &Path, _mode: u32) -> Result<(), SnapshotError> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| {
            SnapshotError::new(format!("cannot inspect snapshot permissions: {error}"))
        })?
        .permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)
        .map_err(|error| SnapshotError::new(format!("cannot set snapshot permissions: {error}")))
}

fn make_snapshot_read_only(root: &Path) -> Result<(), SnapshotError> {
    let mut directories = Vec::new();
    update_permissions(root, &mut directories, true)?;
    for directory in directories.into_iter().rev() {
        set_directory_read_only(&directory)?;
    }
    #[cfg(windows)]
    crate::windows_acl::restrict_tree_read_execute(root).map_err(|error| {
        SnapshotError::new(format!("cannot secure Windows analysis snapshot: {error}"))
    })?;
    Ok(())
}

fn update_permissions(
    directory: &Path,
    directories: &mut Vec<PathBuf>,
    read_only: bool,
) -> Result<(), SnapshotError> {
    directories.push(directory.to_path_buf());
    for entry in fs::read_dir(directory)
        .map_err(|error| SnapshotError::new(format!("cannot inspect snapshot: {error}")))?
    {
        let entry = entry
            .map_err(|error| SnapshotError::new(format!("cannot inspect snapshot: {error}")))?;
        let file_type = entry
            .file_type()
            .map_err(|error| SnapshotError::new(format!("cannot inspect snapshot: {error}")))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            update_permissions(&entry.path(), directories, read_only)?;
        } else if file_type.is_file() {
            set_file_read_only(&entry.path(), read_only)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_file_read_only(path: &Path, read_only: bool) -> Result<(), SnapshotError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(|error| {
            SnapshotError::new(format!("cannot inspect snapshot permissions: {error}"))
        })?
        .permissions()
        .mode();
    let mode = if read_only {
        mode & !0o222
    } else {
        mode | 0o200
    };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| SnapshotError::new(format!("cannot set snapshot permissions: {error}")))
}

#[cfg(not(unix))]
fn set_file_read_only(path: &Path, read_only: bool) -> Result<(), SnapshotError> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| {
            SnapshotError::new(format!("cannot inspect snapshot permissions: {error}"))
        })?
        .permissions();
    permissions.set_readonly(read_only);
    fs::set_permissions(path, permissions)
        .map_err(|error| SnapshotError::new(format!("cannot set snapshot permissions: {error}")))
}

#[cfg(unix)]
fn set_directory_read_only(path: &Path) -> Result<(), SnapshotError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o555))
        .map_err(|error| SnapshotError::new(format!("cannot set snapshot permissions: {error}")))
}

#[cfg(not(unix))]
fn set_directory_read_only(_path: &Path) -> Result<(), SnapshotError> {
    Ok(())
}

fn verify_read_only(root: &Path) -> Result<(), SnapshotError> {
    verify_read_only_directory(root)
}

fn verify_read_only_directory(directory: &Path) -> Result<(), SnapshotError> {
    if directory_is_writable(directory)? {
        return Err(SnapshotError::new(
            "analysis snapshot directory is writable",
        ));
    }
    for entry in fs::read_dir(directory)
        .map_err(|error| SnapshotError::new(format!("cannot inspect snapshot: {error}")))?
    {
        let entry = entry
            .map_err(|error| SnapshotError::new(format!("cannot inspect snapshot: {error}")))?;
        let file_type = entry
            .file_type()
            .map_err(|error| SnapshotError::new(format!("cannot inspect snapshot: {error}")))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            verify_read_only_directory(&entry.path())?;
        } else if is_writable(&entry.path())? {
            return Err(SnapshotError::new("analysis snapshot file is writable"));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn directory_is_writable(path: &Path) -> Result<bool, SnapshotError> {
    is_writable(path)
}

#[cfg(windows)]
fn directory_is_writable(path: &Path) -> Result<bool, SnapshotError> {
    for attempt in 0..10 {
        let probe = path.join(format!(
            ".pre-commit-review-write-probe-{}-{attempt}",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(_) => {
                let _ = fs::remove_file(probe);
                return Ok(true);
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(SnapshotError::new(format!(
                    "cannot verify snapshot directory permissions: {error}"
                )))
            }
        }
    }
    Err(SnapshotError::new(
        "cannot allocate a snapshot directory permission probe",
    ))
}

#[cfg(unix)]
fn is_writable(path: &Path) -> Result<bool, SnapshotError> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::metadata(path)
        .map_err(|error| {
            SnapshotError::new(format!("cannot inspect snapshot permissions: {error}"))
        })?
        .permissions()
        .mode()
        & 0o222
        != 0)
}

#[cfg(windows)]
fn is_writable(path: &Path) -> Result<bool, SnapshotError> {
    match OpenOptions::new().write(true).open(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Ok(false),
        Err(error) => Err(SnapshotError::new(format!(
            "cannot verify snapshot file permissions: {error}"
        ))),
    }
}

fn make_snapshot_writable(root: &Path) {
    #[cfg(windows)]
    let _ = crate::windows_acl::grant_tree_full_control(root);
    let _ = make_directory_writable(root);
}

fn make_directory_writable(directory: &Path) -> Result<(), SnapshotError> {
    set_directory_writable(directory)?;
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            let _ = make_directory_writable(&entry.path());
        } else {
            let _ = set_file_read_only(&entry.path(), false);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_directory_writable(path: &Path) -> Result<(), SnapshotError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|error| {
        SnapshotError::new(format!("cannot restore snapshot permissions: {error}"))
    })
}

#[cfg(not(unix))]
fn set_directory_writable(_path: &Path) -> Result<(), SnapshotError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_rejects_unsafe_relative_paths() {
        assert_eq!(
            safe_relative_path(b"src/file.txt").unwrap(),
            PathBuf::from("src/file.txt")
        );
        assert!(safe_relative_path(b"../escape").is_err());
        assert!(safe_relative_path(b"/absolute").is_err());
    }
}
