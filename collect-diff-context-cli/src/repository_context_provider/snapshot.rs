use super::contract::{CandidateBinding, ReportedCandidateBinding, RustAnalyzerProjectModel};
use crate::candidate::snapshot::CandidateSnapshot;
use std::fs::{self, File};
use std::io::{Read, Take};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotBoundaryError {
    pub code: &'static str,
    message: String,
}

impl SnapshotBoundaryError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl std::fmt::Display for SnapshotBoundaryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SnapshotBoundaryError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFilePath(PathBuf);

impl SnapshotFilePath {
    pub fn new(value: &str) -> Result<Self, SnapshotBoundaryError> {
        if value.is_empty()
            || value.len() > 4_096
            || value.contains(['\\', ':'])
            || value.contains("//")
            || value.ends_with('/')
            || value.chars().any(char::is_control)
        {
            return Err(SnapshotBoundaryError::new(
                "provider-snapshot-path-invalid",
                "snapshot file path is not normalized",
            ));
        }
        let path = Path::new(value);
        if path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            || path.components().any(|component| {
                matches!(component, Component::Normal(name) if name
                        .to_str()
                        .is_some_and(|value| value.eq_ignore_ascii_case(".git")))
            })
        {
            return Err(SnapshotBoundaryError::new(
                "provider-snapshot-path-invalid",
                "snapshot file path is not normalized",
            ));
        }
        Ok(Self(path.to_path_buf()))
    }

    fn as_path(&self) -> &Path {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSourceBudget {
    max_file_bytes: usize,
    remaining_bytes: usize,
}

impl SnapshotSourceBudget {
    pub fn new(max_file_bytes: usize, total_bytes: usize) -> Result<Self, SnapshotBoundaryError> {
        if max_file_bytes == 0 || total_bytes == 0 {
            return Err(SnapshotBoundaryError::new(
                "provider-source-budget-invalid",
                "source budget must be positive and internally consistent",
            ));
        }
        Ok(Self {
            max_file_bytes,
            remaining_bytes: total_bytes,
        })
    }

    pub fn remaining_bytes(&self) -> usize {
        self.remaining_bytes
    }

    fn can_read(&self, bytes: usize) -> Result<(), SnapshotBoundaryError> {
        if bytes > self.max_file_bytes {
            return Err(SnapshotBoundaryError::new(
                "provider-source-file-too-large",
                "source file exceeds the per-file budget",
            ));
        }
        if bytes > self.remaining_bytes {
            return Err(SnapshotBoundaryError::new(
                "provider-source-budget-exhausted",
                "source bytes exceed the total budget",
            ));
        }
        Ok(())
    }

    fn consume(&mut self, bytes: usize) {
        self.remaining_bytes -= bytes;
    }
}

#[derive(Debug)]
pub struct BoundCandidateSnapshot<'a> {
    snapshot: &'a CandidateSnapshot,
    model: &'a RustAnalyzerProjectModel,
    binding: ReportedCandidateBinding,
    canonical_root: PathBuf,
}

impl<'a> BoundCandidateSnapshot<'a> {
    pub fn new(
        snapshot: &'a CandidateSnapshot,
        model: &'a RustAnalyzerProjectModel,
        binding: &CandidateBinding,
    ) -> Result<Self, SnapshotBoundaryError> {
        binding.validate().map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-candidate-binding-invalid",
                "candidate binding does not satisfy the provider contract",
            )
        })?;
        let canonical_root = fs::canonicalize(snapshot.path()).map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-snapshot-root-invalid",
                "candidate snapshot root cannot be canonicalized",
            )
        })?;
        let binding_root = fs::canonicalize(&binding.snapshot_root).map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-snapshot-root-invalid",
                "candidate binding root cannot be canonicalized",
            )
        })?;
        if binding_root != canonical_root || binding.source != snapshot.source() {
            return Err(SnapshotBoundaryError::new(
                "provider-snapshot-binding-mismatch",
                "candidate binding does not match the materialized snapshot",
            ));
        }
        snapshot.verify_unchanged().map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-snapshot-stale",
                "candidate snapshot changed after materialization",
            )
        })?;
        if binding.snapshot_sha256 != snapshot.sha256
            || binding.snapshot_files != snapshot.files
            || binding.snapshot_bytes != snapshot.bytes
        {
            return Err(SnapshotBoundaryError::new(
                "provider-snapshot-binding-mismatch",
                "candidate binding does not match the materialized snapshot",
            ));
        }
        model.validate().map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-model-invalid",
                "linked project model is not valid",
            )
        })?;
        if binding.project_model_digest != model.digest {
            return Err(SnapshotBoundaryError::new(
                "provider-model-binding-mismatch",
                "candidate binding does not match the linked project model",
            ));
        }
        reject_repository_configuration(&canonical_root)?;
        for crate_model in &model.crates {
            let path = SnapshotFilePath::new(&crate_model.root_module)?;
            ensure_snapshot_file(&canonical_root, &path)?;
        }
        Ok(Self {
            snapshot,
            model,
            binding: ReportedCandidateBinding::from(binding),
            canonical_root,
        })
    }

    pub fn root(&self) -> &Path {
        self.snapshot.path()
    }

    pub fn model(&self) -> &RustAnalyzerProjectModel {
        self.model
    }

    pub fn reported_binding(&self) -> &ReportedCandidateBinding {
        &self.binding
    }

    pub fn read_source(
        &self,
        path: &SnapshotFilePath,
        budget: &mut SnapshotSourceBudget,
    ) -> Result<Arc<[u8]>, SnapshotBoundaryError> {
        if path.as_path().extension().and_then(|value| value.to_str()) != Some("rs") {
            return Err(SnapshotBoundaryError::new(
                "provider-source-type-invalid",
                "provider source path must name a Rust file",
            ));
        }
        let source = self.canonical_root.join(path.as_path());
        let canonical = fs::canonicalize(&source).map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-source-missing",
                "provider source file is not available in the snapshot",
            )
        })?;
        ensure_contained(&self.canonical_root, &canonical)?;
        let metadata = fs::metadata(&canonical).map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-source-type-invalid",
                "provider source is not a regular file",
            )
        })?;
        if !metadata.is_file() {
            return Err(SnapshotBoundaryError::new(
                "provider-source-type-invalid",
                "provider source is not a regular file",
            ));
        }
        let expected_len = usize::try_from(metadata.len()).map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-source-file-too-large",
                "provider source length is outside the bounded range",
            )
        })?;
        budget.can_read(expected_len)?;
        let mut input = bounded_reader(
            File::open(&canonical).map_err(|_| {
                SnapshotBoundaryError::new(
                    "provider-source-missing",
                    "provider source file is not available in the snapshot",
                )
            })?,
            budget.max_file_bytes,
        );
        let mut bytes = Vec::with_capacity(expected_len);
        input.read_to_end(&mut bytes).map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-source-read-failed",
                "provider source could not be read",
            )
        })?;
        if bytes.len() != expected_len {
            return Err(SnapshotBoundaryError::new(
                "provider-source-changed",
                "provider source changed while it was read",
            ));
        }
        if std::str::from_utf8(&bytes).is_err() {
            return Err(SnapshotBoundaryError::new(
                "provider-source-encoding-invalid",
                "provider source is not valid UTF-8",
            ));
        }
        budget.consume(bytes.len());
        Ok(Arc::from(bytes.into_boxed_slice()))
    }

    pub fn verify_unchanged(&self) -> Result<(), SnapshotBoundaryError> {
        self.snapshot.verify_unchanged().map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-snapshot-stale",
                "candidate snapshot changed after materialization",
            )
        })
    }
}

fn bounded_reader(file: File, maximum: usize) -> Take<File> {
    file.take(maximum as u64 + 1)
}

fn ensure_snapshot_file(root: &Path, path: &SnapshotFilePath) -> Result<(), SnapshotBoundaryError> {
    let candidate = root.join(path.as_path());
    let canonical = fs::canonicalize(&candidate).map_err(|_| {
        SnapshotBoundaryError::new(
            "provider-model-root-missing",
            "linked project root is not available in the snapshot",
        )
    })?;
    ensure_contained(root, &canonical)?;
    let metadata = fs::metadata(&canonical).map_err(|_| {
        SnapshotBoundaryError::new(
            "provider-model-root-invalid",
            "linked project root is not a regular file",
        )
    })?;
    if !metadata.is_file() {
        return Err(SnapshotBoundaryError::new(
            "provider-model-root-invalid",
            "linked project root is not a regular file",
        ));
    }
    Ok(())
}

fn ensure_contained(root: &Path, path: &Path) -> Result<(), SnapshotBoundaryError> {
    if path == root || !path.starts_with(root) {
        return Err(SnapshotBoundaryError::new(
            "provider-snapshot-containment-invalid",
            "provider path escapes the candidate snapshot",
        ));
    }
    Ok(())
}

fn reject_repository_configuration(root: &Path) -> Result<(), SnapshotBoundaryError> {
    let entries = fs::read_dir(root).map_err(|_| {
        SnapshotBoundaryError::new(
            "provider-snapshot-inspection-failed",
            "candidate snapshot cannot be inspected",
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-snapshot-inspection-failed",
                "candidate snapshot cannot be inspected",
            )
        })?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case("rust-analyzer.toml"))
        {
            return Err(SnapshotBoundaryError::new(
                "provider-snapshot-configuration-forbidden",
                "repository-controlled rust-analyzer configuration is forbidden",
            ));
        }
        let file_type = entry.file_type().map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-snapshot-inspection-failed",
                "candidate snapshot cannot be inspected",
            )
        })?;
        if file_type.is_dir() {
            reject_repository_configuration(&entry.path())?;
        }
    }
    Ok(())
}
