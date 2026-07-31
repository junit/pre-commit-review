use super::contract::{
    CandidateBinding, ProviderRange, ProviderRangeFormat, ReportedCandidateBinding,
    RustAnalyzerProjectModel,
};
use crate::candidate::snapshot::CandidateSnapshot;
use std::fs::{self, File};
use std::io::{Read, Take};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use url::Url;

pub use super::contract::PositionEncoding;

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

    pub fn as_str(&self) -> &str {
        self.0
            .to_str()
            .expect("snapshot paths are validated as UTF-8")
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotUriMapper {
    canonical_root: PathBuf,
}

impl SnapshotUriMapper {
    pub fn new(root: &Path) -> Result<Self, SnapshotBoundaryError> {
        let canonical_root = fs::canonicalize(root).map_err(|_| {
            SnapshotBoundaryError::new("provider-uri-stale", "snapshot URI root is not available")
        })?;
        if !fs::metadata(&canonical_root)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false)
        {
            return Err(SnapshotBoundaryError::new(
                "provider-uri-invalid",
                "snapshot URI root is not a directory",
            ));
        }
        Ok(Self { canonical_root })
    }

    pub fn to_file_path(&self, uri: &Url) -> Result<SnapshotFilePath, SnapshotBoundaryError> {
        validate_file_uri(uri)?;
        let path = uri.to_file_path().map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-uri-invalid",
                "file URI cannot be converted to a local path",
            )
        })?;
        validate_absolute_uri_path(&path)?;
        let parent = path.parent().ok_or_else(|| {
            SnapshotBoundaryError::new(
                "provider-uri-invalid",
                "file URI target has no parent directory",
            )
        })?;
        let canonical_parent = fs::canonicalize(parent).map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-uri-stale",
                "file URI target parent is no longer available",
            )
        })?;
        ensure_uri_parent_contained(&self.canonical_root, &canonical_parent)?;
        let canonical = fs::canonicalize(&path).map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-uri-stale",
                "file URI target is no longer available",
            )
        })?;
        ensure_uri_contained(&self.canonical_root, &canonical)?;
        let metadata = fs::metadata(&canonical).map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-uri-stale",
                "file URI target is no longer available",
            )
        })?;
        if !metadata.is_file() {
            return Err(SnapshotBoundaryError::new(
                "provider-uri-invalid",
                "file URI target is not a regular file",
            ));
        }
        let relative = canonical.strip_prefix(&self.canonical_root).map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-uri-outside-snapshot",
                "file URI target is outside the snapshot",
            )
        })?;
        let relative = relative.to_str().ok_or_else(|| {
            SnapshotBoundaryError::new(
                "provider-uri-non-utf8",
                "snapshot file path is not valid UTF-8",
            )
        })?;
        SnapshotFilePath::new(relative).map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-uri-invalid",
                "file URI target path is not normalized",
            )
        })
    }

    pub fn to_file_uri(&self, path: &SnapshotFilePath) -> Result<Url, SnapshotBoundaryError> {
        let local = self.canonical_root.join(path.as_path());
        let canonical = fs::canonicalize(&local).map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-uri-stale",
                "snapshot file path is no longer available",
            )
        })?;
        ensure_uri_contained(&self.canonical_root, &canonical)?;
        if !fs::metadata(&canonical)
            .map(|metadata| metadata.is_file())
            .unwrap_or(false)
        {
            return Err(SnapshotBoundaryError::new(
                "provider-uri-invalid",
                "snapshot file path is not a regular file",
            ));
        }
        Url::from_file_path(&local).map_err(|_| {
            SnapshotBoundaryError::new(
                "provider-uri-invalid",
                "snapshot file path cannot be represented as a file URI",
            )
        })
    }
}

fn validate_file_uri(uri: &Url) -> Result<(), SnapshotBoundaryError> {
    if uri.scheme() != "file"
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.host_str().is_some()
        || uri.query().is_some()
        || uri.fragment().is_some()
    {
        return Err(SnapshotBoundaryError::new(
            "provider-uri-invalid",
            "file URI contains unsupported metadata",
        ));
    }
    let serialized = uri.as_str().to_ascii_lowercase();
    if serialized.contains("%2e") || serialized.contains("%2f") || serialized.contains("%5c") {
        return Err(SnapshotBoundaryError::new(
            "provider-uri-invalid",
            "file URI contains an encoded path separator or dot segment",
        ));
    }
    Ok(())
}

fn validate_absolute_uri_path(path: &Path) -> Result<(), SnapshotBoundaryError> {
    let value = path.to_str().ok_or_else(|| {
        SnapshotBoundaryError::new("provider-uri-non-utf8", "file URI path is not valid UTF-8")
    })?;
    if value.is_empty()
        || value.chars().any(char::is_control)
        || value.ends_with('/')
        || value.ends_with('\\')
        || value.contains("//")
        || value.contains("\\\\")
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(SnapshotBoundaryError::new(
            "provider-uri-invalid",
            "file URI path is not normalized",
        ));
    }
    Ok(())
}

fn ensure_uri_contained(root: &Path, path: &Path) -> Result<(), SnapshotBoundaryError> {
    if path == root || !path.starts_with(root) {
        return Err(SnapshotBoundaryError::new(
            "provider-uri-outside-snapshot",
            "file URI target is outside the snapshot",
        ));
    }
    Ok(())
}

fn ensure_uri_parent_contained(root: &Path, path: &Path) -> Result<(), SnapshotBoundaryError> {
    if !path.starts_with(root) {
        return Err(SnapshotBoundaryError::new(
            "provider-uri-outside-snapshot",
            "file URI target is outside the snapshot",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}

impl LspPosition {
    pub const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

impl LspRange {
    pub const fn new(
        start_line: u32,
        start_character: u32,
        end_line: u32,
        end_character: u32,
    ) -> Self {
        Self {
            start: LspPosition::new(start_line, start_character),
            end: LspPosition::new(end_line, end_character),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SourceDocument {
    bytes: Arc<[u8]>,
    line_starts: Vec<usize>,
    line_ends: Vec<usize>,
}

impl SourceDocument {
    pub fn new(bytes: Arc<[u8]>) -> Result<Self, SnapshotBoundaryError> {
        if bytes.len() > 4 * 1024 * 1024 || std::str::from_utf8(&bytes).is_err() {
            return Err(SnapshotBoundaryError::new(
                "provider-source-invalid",
                "source document is invalid or exceeds the source-file limit",
            ));
        }
        let mut line_starts = vec![0];
        let mut line_ends = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'\r' => {
                    line_ends.push(index);
                    index += usize::from(index + 1 < bytes.len() && bytes[index + 1] == b'\n') + 1;
                    line_starts.push(index);
                }
                b'\n' => {
                    line_ends.push(index);
                    index += 1;
                    line_starts.push(index);
                }
                _ => index += 1,
            }
        }
        if line_ends.len() < line_starts.len() {
            line_ends.push(bytes.len());
        }
        Ok(Self {
            bytes,
            line_starts,
            line_ends,
        })
    }

    pub fn lsp_to_byte(
        &self,
        position: LspPosition,
        encoding: PositionEncoding,
    ) -> Result<(usize, bool), SnapshotBoundaryError> {
        let line = usize::try_from(position.line).map_err(|_| position_error())?;
        let character = usize::try_from(position.character).map_err(|_| position_error())?;
        let Some(&start) = self.line_starts.get(line) else {
            return Err(position_error());
        };
        let end = self.line_ends[line];
        let text = std::str::from_utf8(&self.bytes[start..end]).map_err(|_| source_error())?;
        let units = text_units(text, encoding);
        if character > units {
            return Ok((end, true));
        }
        let offset = units_to_byte(text, character, encoding)?;
        Ok((start + offset, false))
    }

    pub fn byte_to_lsp(
        &self,
        byte: usize,
        encoding: PositionEncoding,
    ) -> Result<LspPosition, SnapshotBoundaryError> {
        let (line, offset) = self.byte_to_line_offset(byte)?;
        let prefix = &self.bytes[self.line_starts[line]..self.line_starts[line] + offset];
        let prefix = std::str::from_utf8(prefix).map_err(|_| position_error())?;
        Ok(LspPosition::new(
            u32::try_from(line).map_err(|_| position_error())?,
            u32::try_from(text_units(prefix, encoding)).map_err(|_| position_error())?,
        ))
    }

    pub fn lsp_range_to_provider(
        &self,
        range: LspRange,
        encoding: PositionEncoding,
    ) -> Result<ProviderRange, SnapshotBoundaryError> {
        let (start_byte, start_normalized) = self.lsp_to_byte(range.start, encoding)?;
        let (end_byte, end_normalized) = self.lsp_to_byte(range.end, encoding)?;
        if start_normalized || end_normalized {
            return Err(SnapshotBoundaryError::new(
                "provider-position-normalized",
                "LSP position exceeded the source line",
            ));
        }
        if start_byte >= end_byte {
            return Err(range_error());
        }
        self.provider_range_from_bytes(start_byte, end_byte)
    }

    pub fn provider_range_to_lsp(
        &self,
        range: &ProviderRange,
        encoding: PositionEncoding,
    ) -> Result<LspRange, SnapshotBoundaryError> {
        range.validate().map_err(|_| range_error())?;
        let expected = self.provider_range_from_bytes(range.start_byte, range.end_byte)?;
        if expected.start_line != range.start_line
            || expected.start_column != range.start_column
            || expected.end_line != range.end_line
            || expected.end_column != range.end_column
        {
            return Err(SnapshotBoundaryError::new(
                "provider-range-mismatch",
                "provider range coordinates do not match source bytes",
            ));
        }
        Ok(LspRange {
            start: self.byte_to_lsp(range.start_byte, encoding)?,
            end: self.byte_to_lsp(range.end_byte, encoding)?,
        })
    }

    fn provider_range_from_bytes(
        &self,
        start_byte: usize,
        end_byte: usize,
    ) -> Result<ProviderRange, SnapshotBoundaryError> {
        if start_byte >= end_byte || end_byte > self.bytes.len() {
            return Err(range_error());
        }
        let start = self.byte_to_provider_position(start_byte)?;
        let end = self.byte_to_provider_position(end_byte)?;
        let range = ProviderRange {
            format: ProviderRangeFormat::Utf8ByteColumnsEndExclusiveV1,
            start_line: start.0,
            start_column: start.1,
            end_line: end.0,
            end_column: end.1,
            start_byte,
            end_byte,
        };
        range.validate().map_err(|_| range_error())?;
        Ok(range)
    }

    fn byte_to_provider_position(&self, byte: usize) -> Result<(u32, u32), SnapshotBoundaryError> {
        let (line, offset) = self.byte_to_line_offset(byte)?;
        Ok((
            u32::try_from(line + 1).map_err(|_| position_error())?,
            u32::try_from(offset + 1).map_err(|_| position_error())?,
        ))
    }

    fn byte_to_line_offset(&self, byte: usize) -> Result<(usize, usize), SnapshotBoundaryError> {
        if byte > self.bytes.len() {
            return Err(position_error());
        }
        for line in 0..self.line_starts.len() {
            let start = self.line_starts[line];
            let end = self.line_ends[line];
            let next = self
                .line_starts
                .get(line + 1)
                .copied()
                .unwrap_or(self.bytes.len());
            if byte >= start && byte <= end {
                let offset = byte - start;
                if std::str::from_utf8(&self.bytes[..byte]).is_err() {
                    return Err(position_error());
                }
                return Ok((line, offset));
            }
            if byte > end && byte < next {
                return Err(position_error());
            }
        }
        Err(position_error())
    }
}

fn text_units(text: &str, encoding: PositionEncoding) -> usize {
    match encoding {
        PositionEncoding::Utf8 => text.len(),
        PositionEncoding::Utf16 => text.encode_utf16().count(),
    }
}

fn units_to_byte(
    text: &str,
    units: usize,
    encoding: PositionEncoding,
) -> Result<usize, SnapshotBoundaryError> {
    match encoding {
        PositionEncoding::Utf8 => {
            if units > text.len() || !text.is_char_boundary(units) {
                return Err(position_error());
            }
            Ok(units)
        }
        PositionEncoding::Utf16 => {
            let mut consumed = 0;
            for (byte, character) in text.char_indices() {
                if consumed == units {
                    return Ok(byte);
                }
                let width = character.len_utf16();
                if units < consumed + width {
                    return Err(position_error());
                }
                consumed += width;
            }
            if consumed == units {
                Ok(text.len())
            } else {
                Err(position_error())
            }
        }
    }
}

fn position_error() -> SnapshotBoundaryError {
    SnapshotBoundaryError::new(
        "provider-position-invalid",
        "LSP position is outside a valid source boundary",
    )
}

fn source_error() -> SnapshotBoundaryError {
    SnapshotBoundaryError::new(
        "provider-source-invalid",
        "source document is not valid UTF-8",
    )
}

fn range_error() -> SnapshotBoundaryError {
    SnapshotBoundaryError::new(
        "provider-range-invalid",
        "provider range is reversed, empty, or outside the source",
    )
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
        &self.canonical_root
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
