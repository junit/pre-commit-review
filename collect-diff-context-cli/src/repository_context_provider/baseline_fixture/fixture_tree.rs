use super::{read_open_regular_bounded, Result, RunnerError};
use crate::artifacts::contract::sha256_bytes;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_FIXTURE_FILE_BYTES: u64 = 64 * 1024;
const MAX_FIXTURE_BYTES: u64 = 256 * 1024;
const MAX_FIXTURE_FILES: usize = 64;
const MAX_FIXTURE_DIRECTORIES: usize = 64;
const MAX_FIXTURE_DEPTH: usize = 16;
const MAX_FIXTURE_PATH_BYTES: usize = 192;

pub(super) struct FixtureInventory {
    directories: Vec<PathBuf>,
    files: Vec<FixtureFile>,
}

struct FixtureFile {
    relative_path: PathBuf,
    normalized_path: String,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct FixtureBudget {
    directories: usize,
    files: usize,
    bytes: u64,
}

impl FixtureInventory {
    pub(super) fn validate(root: &Path) -> Result<Self> {
        let mut inventory = Self {
            directories: Vec::new(),
            files: Vec::new(),
        };
        let mut budget = FixtureBudget::default();
        budget.add_directory()?;
        collect(root, root, 0, &mut inventory, &mut budget)?;
        Ok(inventory)
    }

    pub(super) fn copy_to(&self, destination: &Path) -> Result<()> {
        for relative_path in &self.directories {
            fs::create_dir(destination.join(relative_path))
                .map_err(|_| execution_error("fixture directory cannot be created"))?;
        }
        for file in &self.files {
            fs::write(destination.join(&file.relative_path), &file.bytes)
                .map_err(|_| execution_error("fixture file cannot be copied"))?;
        }
        Ok(())
    }

    pub(super) fn sha256(&self) -> Result<String> {
        let files = self
            .files
            .iter()
            .map(|file| {
                json!({
                    "path": file.normalized_path,
                    "size": file.bytes.len(),
                    "sha256": sha256_bytes(&file.bytes),
                })
            })
            .collect::<Vec<_>>();
        serde_json::to_vec(&json!({ "files": files }))
            .map(|bytes| sha256_bytes(&bytes))
            .map_err(|_| execution_error("fixture inventory cannot be serialized"))
    }
}

impl FixtureBudget {
    fn add_directory(&mut self) -> Result<()> {
        self.directories = self
            .directories
            .checked_add(1)
            .ok_or_else(|| structure_error("fixture directory count overflowed"))?;
        if self.directories > MAX_FIXTURE_DIRECTORIES {
            return Err(structure_error("fixture exceeds its directory limit"));
        }
        Ok(())
    }

    fn add_file(&mut self, bytes: u64) -> Result<()> {
        self.files = self
            .files
            .checked_add(1)
            .ok_or_else(|| structure_error("fixture file count overflowed"))?;
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| structure_error("fixture byte count overflowed"))?;
        if self.files > MAX_FIXTURE_FILES
            || bytes > MAX_FIXTURE_FILE_BYTES
            || self.bytes > MAX_FIXTURE_BYTES
        {
            return Err(structure_error("fixture exceeds its byte or file limit"));
        }
        Ok(())
    }
}

fn collect(
    root: &Path,
    directory: &Path,
    depth: usize,
    inventory: &mut FixtureInventory,
    budget: &mut FixtureBudget,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)
        .map_err(|_| structure_error("fixture directory cannot be read"))?
        .map(|entry| entry.map_err(|_| structure_error("fixture entry cannot be read")))
        .collect::<Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|_| structure_error("fixture entry type cannot be read"))?;
        if file_type.is_symlink() {
            return Err(structure_error("fixture cannot contain symbolic links"));
        }
        let relative_path = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| structure_error("fixture path cannot be normalized"))?
            .to_path_buf();
        let normalized_path = normalize_path(&relative_path)?;
        if file_type.is_dir() {
            let child_depth = depth
                .checked_add(1)
                .ok_or_else(|| structure_error("fixture depth overflowed"))?;
            if child_depth > MAX_FIXTURE_DEPTH {
                return Err(structure_error("fixture exceeds its depth limit"));
            }
            budget.add_directory()?;
            inventory.directories.push(relative_path);
            collect(root, &entry.path(), child_depth, inventory, budget)?;
        } else if file_type.is_file() {
            let bytes = read_open_regular_bounded(&entry.path(), MAX_FIXTURE_FILE_BYTES)
                .map_err(|_| structure_error("fixture file cannot be read safely"))?;
            budget.add_file(
                u64::try_from(bytes.len())
                    .map_err(|_| structure_error("fixture file size cannot be represented"))?,
            )?;
            inventory.files.push(FixtureFile {
                relative_path,
                normalized_path,
                bytes,
            });
        } else {
            return Err(structure_error("fixture entry is not a regular file"));
        }
    }
    Ok(())
}

fn normalize_path(path: &Path) -> Result<String> {
    let normalized = path
        .to_str()
        .ok_or_else(|| structure_error("fixture path is not UTF-8"))?
        .replace('\\', "/");
    if normalized.is_empty() || normalized.len() > MAX_FIXTURE_PATH_BYTES {
        return Err(structure_error("fixture path exceeds its byte limit"));
    }
    Ok(normalized)
}

fn structure_error(message: &'static str) -> RunnerError {
    RunnerError::new("fixture-structure-policy", message)
}

fn execution_error(message: &'static str) -> RunnerError {
    RunnerError::new("runner-execution", message)
}
