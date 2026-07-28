use crate::candidate::RepoPath;
use crate::impact_context::cache::file_facts::{
    sync_directory, CacheLayout, CacheLookup, FileFactsEnvelope, FileFactsStore,
};
use crate::impact_context::cache::locking::acquire_writer_lock;
use crate::impact_context::cache::sqlite_generation::{ReaderLimits, RepositoryGraphReader};
use crate::impact_context::index::model::{IndexLimitation, IndexMetrics, IndexReportStatus};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const MAXIMUM_OBJECT_BYTES: usize = 16 * 1024 * 1024;
const MAXIMUM_DATABASE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAXIMUM_STRING_BYTES: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectSelector {
    Path(RepoPath),
    Symbol(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheOperationResult {
    pub status: IndexReportStatus,
    pub generation_key: Option<String>,
    pub metrics: IndexMetrics,
    pub limitations: Vec<IndexLimitation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanRequest {
    pub execute: bool,
    pub maximum_bytes: usize,
    pub retain_generations: usize,
    pub invalid_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheOperationError {
    pub code: &'static str,
    pub message: String,
}

impl CacheOperationError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CacheOperationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CacheOperationError {}

pub fn doctor_repository_cache(
    layout: &CacheLayout,
    generation: Option<&str>,
    maximum_files: usize,
    maximum_bytes: usize,
) -> Result<CacheOperationResult, CacheOperationError> {
    let started = Instant::now();
    if let Some(generation) = generation {
        let path = layout.graphs_dir.join(format!("{generation}.sqlite"));
        match fs::symlink_metadata(path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(unavailable_operation(
                    started,
                    Some(generation.to_string()),
                    "repository-index-generation-miss",
                    "the requested immutable generation does not exist",
                ));
            }
            Err(error) => {
                return Err(CacheOperationError::new(
                    "repository-index-cache-metadata-failed",
                    format!("cannot inspect graph generation: {error}"),
                ));
            }
        }
    }
    let mut metrics = empty_metrics();
    let mut limitations = Vec::new();
    let mut consumed_files = 0usize;
    let mut consumed_bytes = 0usize;

    let generation_paths = selected_generation_paths(layout, generation)?;
    for path in generation_paths {
        if !consume_path_budget(
            &path,
            maximum_files,
            maximum_bytes,
            &mut consumed_files,
            &mut consumed_bytes,
            &mut limitations,
        )? {
            break;
        }
        metrics.generation_bytes = metrics.generation_bytes.saturating_add(
            fs::symlink_metadata(&path)
                .map(|value| value.len())
                .unwrap_or(0),
        );
        doctor_generation(&path, &mut limitations)?;
    }

    let store = FileFactsStore::new(layout.clone(), MAXIMUM_OBJECT_BYTES)
        .map_err(|error| CacheOperationError::new(error.code, error.message))?;
    for path in regular_files_bounded(&layout.facts_dir, maximum_files)? {
        if !consume_path_budget(
            &path,
            maximum_files,
            maximum_bytes,
            &mut consumed_files,
            &mut consumed_bytes,
            &mut limitations,
        )? {
            break;
        }
        metrics.manifest_files = metrics.manifest_files.saturating_add(1);
        metrics.manifest_bytes = metrics.manifest_bytes.saturating_add(
            fs::symlink_metadata(&path)
                .map(|value| value.len())
                .unwrap_or(0),
        );
        match validate_file_facts_path(&store, &path, MAXIMUM_OBJECT_BYTES) {
            Ok(true) => metrics.file_fact_hits = metrics.file_fact_hits.saturating_add(1),
            Ok(false) => {
                metrics.file_fact_misses = metrics.file_fact_misses.saturating_add(1);
                limitations.push(limitation(
                    "repository-index-file-facts-corrupt",
                    "a FileFacts object failed checksum, key, path, or payload validation",
                    "doctor is read-only; rebuild the index or run explicit cleanup",
                ));
            }
            Err(error) => {
                metrics.file_fact_misses = metrics.file_fact_misses.saturating_add(1);
                limitations.push(limitation(
                    "repository-index-file-facts-unreadable",
                    &error.message,
                    "the unreadable object was not modified",
                ));
            }
        }
    }
    sort_limitations(&mut limitations);
    metrics.elapsed_ms = elapsed_ms(started);
    Ok(CacheOperationResult {
        status: if limitations.is_empty() {
            IndexReportStatus::Completed
        } else {
            IndexReportStatus::Partial
        },
        generation_key: generation.map(ToOwned::to_owned),
        metrics,
        limitations,
    })
}

pub fn inspect_repository_generation(
    layout: &CacheLayout,
    generation: &str,
    selector: &InspectSelector,
    maximum_rows: usize,
) -> Result<CacheOperationResult, CacheOperationError> {
    let started = Instant::now();
    let path = layout.graphs_dir.join(format!("{generation}.sqlite"));
    let limits = ReaderLimits {
        maximum_database_bytes: MAXIMUM_DATABASE_BYTES,
        maximum_rows_per_query: maximum_rows,
        maximum_string_bytes: MAXIMUM_STRING_BYTES,
    };
    let identity =
        match RepositoryGraphReader::read_identity_immutable(&path, limits).map_err(graph_error)? {
            CacheLookup::Hit(identity) => identity,
            CacheLookup::Miss => {
                return Ok(unavailable_operation(
                    started,
                    Some(generation.to_string()),
                    "repository-index-generation-miss",
                    "the requested immutable generation does not exist",
                ))
            }
            CacheLookup::Stale { code } => {
                return Ok(partial_operation(
                    started,
                    Some(generation.to_string()),
                    "repository-index-generation-stale",
                    &code,
                ))
            }
            CacheLookup::Corrupt { code } => {
                return Ok(partial_operation(
                    started,
                    Some(generation.to_string()),
                    "repository-index-generation-corrupt",
                    &code,
                ))
            }
        };
    let reader = match RepositoryGraphReader::open_immutable(&path, &identity, limits)
        .map_err(graph_error)?
    {
        CacheLookup::Hit(reader) => reader,
        CacheLookup::Miss => {
            return Ok(unavailable_operation(
                started,
                Some(generation.to_string()),
                "repository-index-generation-miss",
                "the requested immutable generation disappeared",
            ))
        }
        CacheLookup::Stale { code } => {
            return Ok(partial_operation(
                started,
                Some(generation.to_string()),
                "repository-index-generation-stale",
                &code,
            ))
        }
        CacheLookup::Corrupt { code } => {
            return Ok(partial_operation(
                started,
                Some(generation.to_string()),
                "repository-index-generation-corrupt",
                &code,
            ))
        }
    };

    let mut metrics = empty_metrics();
    metrics.generation_bytes = fs::symlink_metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    match selector {
        InspectSelector::Path(path) => {
            let symbols = reader
                .symbols_for_path(path, maximum_rows)
                .map_err(graph_error)?;
            metrics.symbols = symbols.len();
            metrics.query_rows = symbols.len();
        }
        InspectSelector::Symbol(symbol_id) => {
            let symbol = reader.symbol(symbol_id).map_err(graph_error)?;
            metrics.symbols = usize::from(symbol.is_some());
            metrics.query_rows = usize::from(symbol.is_some());
        }
    }
    metrics.elapsed_ms = elapsed_ms(started);
    Ok(CacheOperationResult {
        status: IndexReportStatus::Completed,
        generation_key: Some(generation.to_string()),
        metrics,
        limitations: Vec::new(),
    })
}

pub fn clean_repository_cache(
    layout: &CacheLayout,
    request: CleanRequest,
) -> Result<CacheOperationResult, CacheOperationError> {
    let started = Instant::now();
    let mut candidates = generation_candidates(layout)?;
    candidates.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.key.cmp(&right.key))
    });
    let total_bytes = candidates.iter().fold(0usize, |total, candidate| {
        total.saturating_add(candidate.bytes)
    });
    let retained_bytes = candidates
        .iter()
        .take(request.retain_generations)
        .fold(0usize, |total, candidate| {
            total.saturating_add(candidate.bytes)
        });
    let mut projected_bytes = total_bytes;
    let mut selected = Vec::new();
    for (index, candidate) in candidates.iter().enumerate().rev() {
        let invalid = generation_is_invalid(&candidate.path)?;
        let retained = index < request.retain_generations;
        let select = if request.invalid_only {
            invalid
        } else {
            !retained && projected_bytes > request.maximum_bytes
        };
        if select {
            projected_bytes = projected_bytes.saturating_sub(candidate.bytes);
            selected.push(candidate.clone());
        }
    }
    selected.sort_by(|left, right| left.key.cmp(&right.key));

    let mut limitations = Vec::new();
    if !request.invalid_only && retained_bytes > request.maximum_bytes {
        limitations.push(limitation(
            "repository-index-clean-retention-prevents-target",
            "retained generations exceed the requested maximum byte target",
            "reduce --retain-generations or increase --max-bytes",
        ));
    }
    if request.execute {
        let mut removed_any = false;
        for candidate in selected {
            let writer_lock = match acquire_writer_lock(layout, &candidate.key, Duration::ZERO) {
                Ok(writer_lock) => writer_lock,
                Err(error) if error.code == "writer-busy" => {
                    limitations.push(limitation(
                        "repository-index-clean-generation-in-use",
                        "an immutable generation is currently in use",
                        "cleanup deferred the generation without modifying it",
                    ));
                    continue;
                }
                Err(error) => {
                    limitations.push(limitation(
                        "repository-index-clean-lock-failed",
                        &error.message,
                        "cleanup deferred the generation without modifying it",
                    ));
                    continue;
                }
            };
            match fs::remove_file(&candidate.path) {
                Ok(()) => removed_any = true,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::PermissionDenied
                            | std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::ResourceBusy
                    ) =>
                {
                    limitations.push(limitation(
                        "repository-index-clean-generation-in-use",
                        "the platform refused removal of an in-use immutable generation",
                        "cleanup deferred the generation without modifying it",
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    limitations.push(limitation(
                        "repository-index-clean-remove-failed",
                        &format!("cannot remove immutable generation: {error}"),
                        "cleanup left the generation unchanged",
                    ));
                }
            }
            drop(writer_lock);
        }
        if removed_any {
            sync_directory(&layout.graphs_dir)
                .map_err(|error| CacheOperationError::new(error.code, error.message))?;
        }
    }
    sort_limitations(&mut limitations);
    let mut metrics = empty_metrics();
    metrics.generation_bytes = u64::try_from(total_bytes).unwrap_or(u64::MAX);
    metrics.elapsed_ms = elapsed_ms(started);
    Ok(CacheOperationResult {
        status: if limitations.is_empty() {
            IndexReportStatus::Completed
        } else {
            IndexReportStatus::Partial
        },
        generation_key: None,
        metrics,
        limitations,
    })
}

#[derive(Debug, Clone)]
struct GenerationCandidate {
    key: String,
    path: PathBuf,
    bytes: usize,
    modified: SystemTime,
}

fn generation_candidates(
    layout: &CacheLayout,
) -> Result<Vec<GenerationCandidate>, CacheOperationError> {
    let entries = match fs::read_dir(&layout.graphs_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CacheOperationError::new(
                "repository-index-clean-read-failed",
                format!("cannot read graph generation directory: {error}"),
            ))
        }
    };
    let mut candidates = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| {
                CacheOperationError::new(
                    "repository-index-clean-read-failed",
                    format!("cannot read graph generation entry: {error}"),
                )
            })?
            .path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            CacheOperationError::new(
                "repository-index-clean-metadata-failed",
                format!("cannot inspect graph generation entry: {error}"),
            )
        })?;
        if !metadata.file_type().is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(key) = name.strip_suffix(".sqlite") else {
            continue;
        };
        if !valid_sha256(key) {
            continue;
        }
        candidates.push(GenerationCandidate {
            key: key.to_string(),
            path,
            bytes: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        });
    }
    Ok(candidates)
}

fn generation_is_invalid(path: &Path) -> Result<bool, CacheOperationError> {
    let limits = ReaderLimits {
        maximum_database_bytes: MAXIMUM_DATABASE_BYTES,
        maximum_rows_per_query: 1,
        maximum_string_bytes: MAXIMUM_STRING_BYTES,
    };
    let identity =
        match RepositoryGraphReader::read_identity_immutable(path, limits).map_err(graph_error)? {
            CacheLookup::Hit(identity) => identity,
            CacheLookup::Miss | CacheLookup::Stale { .. } | CacheLookup::Corrupt { .. } => {
                return Ok(true)
            }
        };
    match RepositoryGraphReader::open_immutable(path, &identity, limits).map_err(graph_error)? {
        CacheLookup::Hit(reader) => Ok(reader.integrity_check().is_err()),
        CacheLookup::Miss | CacheLookup::Stale { .. } | CacheLookup::Corrupt { .. } => Ok(true),
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn doctor_generation(
    path: &Path,
    limitations: &mut Vec<IndexLimitation>,
) -> Result<(), CacheOperationError> {
    let limits = ReaderLimits {
        maximum_database_bytes: MAXIMUM_DATABASE_BYTES,
        maximum_rows_per_query: 50_000,
        maximum_string_bytes: MAXIMUM_STRING_BYTES,
    };
    let identity =
        match RepositoryGraphReader::read_identity_immutable(path, limits).map_err(graph_error)? {
            CacheLookup::Hit(identity) => identity,
            CacheLookup::Miss => return Ok(()),
            CacheLookup::Stale { code } => {
                limitations.push(limitation(
                    "repository-index-generation-stale",
                    &code,
                    "the stale generation was not modified",
                ));
                return Ok(());
            }
            CacheLookup::Corrupt { code } => {
                limitations.push(limitation(
                    "repository-index-generation-corrupt",
                    &code,
                    "the corrupt generation was not modified",
                ));
                return Ok(());
            }
        };
    match RepositoryGraphReader::open_immutable(path, &identity, limits).map_err(graph_error)? {
        CacheLookup::Hit(reader) => {
            if let Err(error) = reader.integrity_check() {
                limitations.push(limitation(
                    "repository-index-generation-corrupt",
                    &error.message,
                    "the corrupt generation was not modified",
                ));
            }
        }
        CacheLookup::Miss => {}
        CacheLookup::Stale { code } => limitations.push(limitation(
            "repository-index-generation-stale",
            &code,
            "the stale generation was not modified",
        )),
        CacheLookup::Corrupt { code } => limitations.push(limitation(
            "repository-index-generation-corrupt",
            &code,
            "the corrupt generation was not modified",
        )),
    }
    Ok(())
}

fn selected_generation_paths(
    layout: &CacheLayout,
    generation: Option<&str>,
) -> Result<Vec<PathBuf>, CacheOperationError> {
    if let Some(generation) = generation {
        return Ok(vec![layout.graphs_dir.join(format!("{generation}.sqlite"))]);
    }
    regular_files_bounded(&layout.graphs_dir, 100_000)
}

fn regular_files_bounded(
    root: &Path,
    maximum_files: usize,
) -> Result<Vec<PathBuf>, CacheOperationError> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(CacheOperationError::new(
                    "repository-index-cache-read-failed",
                    format!("cannot read cache directory: {error}"),
                ))
            }
        };
        let mut paths = entries
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                CacheOperationError::new(
                    "repository-index-cache-read-failed",
                    format!("cannot read cache entry: {error}"),
                )
            })?;
        paths.sort();
        for path in paths.into_iter().rev() {
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                CacheOperationError::new(
                    "repository-index-cache-metadata-failed",
                    format!("cannot inspect cache entry: {error}"),
                )
            })?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                files.push(path);
                if files.len() >= maximum_files {
                    files.sort();
                    return Ok(files);
                }
            }
        }
    }
    files.sort();
    Ok(files)
}

fn consume_path_budget(
    path: &Path,
    maximum_files: usize,
    maximum_bytes: usize,
    consumed_files: &mut usize,
    consumed_bytes: &mut usize,
    limitations: &mut Vec<IndexLimitation>,
) -> Result<bool, CacheOperationError> {
    if *consumed_files >= maximum_files {
        limitations.push(limitation(
            "repository-index-doctor-file-budget-exhausted",
            "doctor reached its cache file limit",
            "remaining cache objects were not inspected",
        ));
        return Ok(false);
    }
    let bytes = fs::symlink_metadata(path)
        .map_err(|error| {
            CacheOperationError::new(
                "repository-index-cache-metadata-failed",
                format!("cannot inspect cache entry: {error}"),
            )
        })?
        .len();
    let bytes = usize::try_from(bytes).unwrap_or(usize::MAX);
    if consumed_bytes.saturating_add(bytes) > maximum_bytes {
        limitations.push(limitation(
            "repository-index-doctor-byte-budget-exhausted",
            "doctor reached its cache byte limit",
            "remaining cache objects were not inspected",
        ));
        return Ok(false);
    }
    *consumed_files = consumed_files.saturating_add(1);
    *consumed_bytes = consumed_bytes.saturating_add(bytes);
    Ok(true)
}

fn validate_file_facts_path(
    store: &FileFactsStore,
    path: &Path,
    remaining_bytes: usize,
) -> Result<bool, CacheOperationError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        CacheOperationError::new(
            "repository-index-file-facts-metadata-failed",
            format!("cannot inspect FileFacts object: {error}"),
        )
    })?;
    if !metadata.file_type().is_file()
        || metadata.len() > MAXIMUM_OBJECT_BYTES as u64
        || metadata.len() > remaining_bytes as u64
    {
        return Ok(false);
    }
    let bytes = fs::read(path).map_err(|error| {
        CacheOperationError::new(
            "repository-index-file-facts-read-failed",
            format!("cannot read FileFacts object: {error}"),
        )
    })?;
    let envelope: FileFactsEnvelope = match serde_json::from_slice(&bytes) {
        Ok(envelope) => envelope,
        Err(_) => return Ok(false),
    };
    let expected = store
        .object_path(&envelope.key)
        .map_err(|error| CacheOperationError::new(error.code, error.message))?;
    if expected != path {
        return Ok(false);
    }
    Ok(matches!(
        store
            .lookup(&envelope.key)
            .map_err(|error| CacheOperationError::new(error.code, error.message))?,
        CacheLookup::Hit(_)
    ))
}

fn unavailable_operation(
    started: Instant,
    generation_key: Option<String>,
    code: &str,
    reason: &str,
) -> CacheOperationResult {
    let mut result = partial_operation(started, generation_key, code, reason);
    result.status = IndexReportStatus::Unavailable;
    result
}

fn partial_operation(
    started: Instant,
    generation_key: Option<String>,
    code: &str,
    reason: &str,
) -> CacheOperationResult {
    let mut metrics = empty_metrics();
    metrics.elapsed_ms = elapsed_ms(started);
    CacheOperationResult {
        status: IndexReportStatus::Partial,
        generation_key,
        metrics,
        limitations: vec![limitation(
            code,
            reason,
            "the immutable generation was not modified",
        )],
    }
}

fn limitation(code: &str, reason: &str, interpretation: &str) -> IndexLimitation {
    IndexLimitation {
        code: code.to_string(),
        path: None,
        symbol_id: None,
        reason: reason.chars().take(1_000).collect(),
        interpretation: interpretation.chars().take(1_000).collect(),
    }
}

fn sort_limitations(limitations: &mut Vec<IndexLimitation>) {
    limitations.sort_by(|left, right| {
        (
            left.code.as_str(),
            left.path.as_ref().map(RepoPath::as_str).unwrap_or(""),
            left.symbol_id.as_deref().unwrap_or(""),
            left.reason.as_str(),
            left.interpretation.as_str(),
        )
            .cmp(&(
                right.code.as_str(),
                right.path.as_ref().map(RepoPath::as_str).unwrap_or(""),
                right.symbol_id.as_deref().unwrap_or(""),
                right.reason.as_str(),
                right.interpretation.as_str(),
            ))
    });
    limitations.dedup();
}

fn empty_metrics() -> IndexMetrics {
    IndexMetrics {
        elapsed_ms: 0,
        manifest_files: 0,
        manifest_bytes: 0,
        file_fact_hits: 0,
        file_fact_misses: 0,
        file_fact_writes: 0,
        parsed_files: 0,
        parsed_bytes: 0,
        symbols: 0,
        edges: 0,
        query_rows: 0,
        generation_bytes: 0,
        output_bytes: 0,
    }
}

fn graph_error(
    error: crate::impact_context::cache::sqlite_generation::RepositoryGraphError,
) -> CacheOperationError {
    CacheOperationError::new(error.code, error.message)
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
