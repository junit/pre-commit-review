use crate::candidate::{CandidateContent, CandidatePresence, RepoPath};
use crate::impact_context::adapters::tree_sitter_rust::TreeSitterRustAdapter;
use crate::impact_context::cache::file_facts::{
    sync_directory, CacheLayout, CacheLookup, FileFactsStore, PublishResult,
};
use crate::impact_context::cache::generation_locator::{
    GenerationCompatibility, GenerationLocatorStore, LocatedGeneration,
};
use crate::impact_context::cache::sqlite_generation::{
    GraphPublishOutcome, ReaderLimits, RepositoryGraphReader, RepositoryGraphWriter,
};
use crate::impact_context::contracts::{
    ChangedSymbol, Completeness, Confidence, DomainSummary, EdgeKind, ImpactEdge, ImpactMode,
    Limitation, ProviderRecord, ProviderStatus, Resolution, SourceRange,
};
use crate::impact_context::index::budget::{IndexBudget, IndexBudgetTracker, IndexResource};
use crate::impact_context::index::manifest::RepositoryManifestSource;
use crate::impact_context::index::model::{
    FileFactKey, GraphEdge, GraphFile, GraphGenerationIdentity, GraphSymbol, IndexLimitation,
    IndexMetrics, RepositoryGraph, RepositoryManifest,
};
use crate::impact_context::index::overlay::{build_repository_overlay, RepositoryOverlay};
use crate::impact_context::index::project_model::{build_rust_project_model, RustProjectModel};
use crate::impact_context::index::resolver::rust::{
    resolve_rust_repository, RustRepositoryFileFacts,
};
use crate::impact_context::index::traversal::{
    traverse_repository_graph, TraversalDirection, TraversalRequest,
};
use crate::impact_context::normalizer::{normalize_repository_graph, stable_id};
use crate::impact_context::summarizer::summarize_repository_graph;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Instant;

const PROVIDER_KIND: &str = "repository-index";
const PROVIDER_VERSION: &str = "repository-index/g1-r1-q1-n1";
const GRAMMAR_VERSION: &str = "tree-sitter-rust@0.24.2";
const ADAPTER_VERSION: &str = "tree-sitter-rust-index/v1";
const RESOLVER_VERSION: &str = "rust-resolver/v1";
const NORMALIZATION_VERSION: &str = "repository-index-normalization/v1";
const MAXIMUM_FILE_FACT_OBJECT_BYTES: usize = 16 * 1024 * 1024;
const MAXIMUM_TRAVERSAL_OUTPUT_BYTES: usize = 1_048_576;

#[derive(Debug, Clone)]
pub struct RepositoryIndexAdapter {
    layout: CacheLayout,
}

pub struct RepositoryIndexRequest<'a> {
    pub candidate: &'a dyn CandidateContent,
    pub manifest_source: &'a dyn RepositoryManifestSource,
    pub changed_symbols: &'a [ChangedSymbol],
    pub mode: ImpactMode,
    pub cache_read: bool,
    pub cache_write: bool,
    pub index_budget: IndexBudget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryIndexOutput {
    pub generation_key: String,
    pub provider: ProviderRecord,
    pub symbols: Vec<ChangedSymbol>,
    pub edges: Vec<ImpactEdge>,
    pub domain_summaries: Vec<DomainSummary>,
    pub index_completeness: Completeness,
    pub query_completeness: Completeness,
    pub reached_depth: usize,
    pub output_truncated: bool,
    pub limitations: Vec<Limitation>,
    pub metrics: IndexMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryIndexError {
    pub code: &'static str,
    pub message: String,
}

impl RepositoryIndexError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RepositoryIndexError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RepositoryIndexError {}

#[derive(Debug, Default)]
struct CacheStats {
    hits: usize,
    misses: usize,
    stale: usize,
    corrupt: usize,
}

struct PreparedIndex {
    manifest: RepositoryManifest,
    project_model: RustProjectModel,
    file_keys: Vec<(RepoPath, FileFactKey)>,
    identity: GraphGenerationIdentity,
}

impl RepositoryIndexAdapter {
    pub fn new(layout: CacheLayout) -> Self {
        Self { layout }
    }

    pub fn analyze(
        &self,
        request: RepositoryIndexRequest<'_>,
    ) -> Result<RepositoryIndexOutput, RepositoryIndexError> {
        validate_request(&request)?;
        let started = Instant::now();
        let opening_scope = request.candidate.scope_fingerprint().to_string();
        let mut published_artifacts = Vec::new();
        validate_scope(&request, &opening_scope, started, &published_artifacts)?;
        let provider_id = repository_index_provider_id();
        if request.mode == ImpactMode::Fast {
            return self.analyze_fast_exact(request, &opening_scope, &provider_id, started);
        }
        let mut tracker = IndexBudgetTracker::new(request.index_budget.clone());
        let prepared = prepare_index(request.manifest_source, &mut tracker)?;
        validate_scope(&request, &opening_scope, started, &published_artifacts)?;
        let mut cache = CacheStats::default();
        let mut metrics = IndexMetrics {
            elapsed_ms: 0,
            manifest_files: prepared.manifest.entries.len(),
            manifest_bytes: prepared
                .manifest
                .entries
                .iter()
                .map(|entry| entry.content_bytes.unwrap_or(0) as u64)
                .sum(),
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
        };
        let writer = RepositoryGraphWriter::new(self.layout.clone());
        let generation_path = writer
            .generation_path(&prepared.identity)
            .map_err(map_graph_error)?;
        let mut index_limitations = prepared.manifest.limitations.clone();
        index_limitations.extend(prepared.project_model.limitations.iter().map(|code| {
            simple_index_limitation(code, "the passive Rust project model is partial")
        }));

        let mut reader = None;
        if request.cache_read {
            match open_reader(&generation_path, &prepared.identity, &request.index_budget)? {
                CacheLookup::Hit(hit) => {
                    cache.hits += 1;
                    reader = Some(hit);
                }
                CacheLookup::Miss => cache.misses += 1,
                CacheLookup::Stale { code } => {
                    cache.stale += 1;
                    index_limitations.push(simple_index_limitation(
                        "repository-index-generation-stale",
                        &code,
                    ));
                }
                CacheLookup::Corrupt { code } => {
                    cache.corrupt += 1;
                    index_limitations.push(simple_index_limitation(
                        "repository-index-generation-corrupt",
                        &code,
                    ));
                }
            }
        } else {
            cache.misses += 1;
        }

        if reader.is_none() && request.mode == ImpactMode::Deep && request.cache_write {
            let facts_store =
                FileFactsStore::new(self.layout.clone(), MAXIMUM_FILE_FACT_OBJECT_BYTES)
                    .map_err(map_cache_error)?;
            let file_facts = build_file_facts(
                &request,
                &opening_scope,
                &prepared,
                &facts_store,
                &mut tracker,
                &mut cache,
                &mut metrics,
                &mut index_limitations,
                started,
                &mut published_artifacts,
            )?;
            let mut graph = resolve_rust_repository(
                &prepared.manifest,
                &prepared.project_model,
                &file_facts,
                prepared.identity.clone(),
                &mut tracker,
            )
            .map_err(|error| RepositoryIndexError::new(error.code, error.message))?;
            for limitation in &prepared.manifest.limitations {
                if (limitation.path.is_some() || limitation.symbol_id.is_some())
                    && !graph.limitations.contains(limitation)
                {
                    graph.limitations.push(limitation.clone());
                }
            }
            if graph.completeness == Completeness::Partial
                && !graph
                    .limitations
                    .iter()
                    .any(|limitation| limitation.path.is_some() || limitation.symbol_id.is_some())
            {
                let path = prepared
                    .project_model
                    .consumed_files
                    .first()
                    .map(|file| file.path.clone())
                    .or_else(|| {
                        prepared
                            .manifest
                            .entries
                            .first()
                            .map(|entry| entry.path.clone())
                    });
                graph.limitations.push(IndexLimitation {
                    code: "repository-index-partial-omission".to_string(),
                    path,
                    symbol_id: None,
                    reason: "the passive project model or resolver reported a partial graph"
                        .to_string(),
                    interpretation: "relationships under the scoped manifest may be incomplete"
                        .to_string(),
                });
            }
            graph.limitations.sort_by(|left, right| {
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
            metrics.symbols = graph.symbols.len();
            metrics.edges = graph.edges.len();
            index_limitations.extend(graph.limitations.clone());
            validate_scope(&request, &opening_scope, started, &published_artifacts)?;
            let outcome = writer
                .publish(&graph, &mut tracker)
                .map_err(map_graph_error)?;
            let path = match outcome {
                GraphPublishOutcome::Published { path } => {
                    published_artifacts.push(path.clone());
                    path
                }
                GraphPublishOutcome::Reused { path } => path,
            };
            validate_scope(&request, &opening_scope, started, &published_artifacts)?;
            metrics.generation_bytes = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            reader = match open_reader(&path, &prepared.identity, &request.index_budget)? {
                CacheLookup::Hit(reader) => Some(reader),
                _ => {
                    return Err(RepositoryIndexError::new(
                        "repository-index-published-generation-unavailable",
                        "published repository graph could not be opened immutably",
                    ))
                }
            };
        }

        let Some(reader) = reader else {
            index_limitations.push(simple_index_limitation(
                "repository-index-generation-miss",
                "no compatible immutable repository graph generation is available",
            ));
            validate_scope(&request, &opening_scope, started, &published_artifacts)?;
            return Ok(finalize_unavailable(
                &provider_id,
                &prepared,
                cache,
                index_limitations,
                metrics,
                started,
            ));
        };

        if request.cache_write {
            validate_scope(&request, &opening_scope, started, &published_artifacts)?;
            let locator_outcome = GenerationLocatorStore::new(self.layout.clone())
                .publish_exact_tracked(
                    &prepared.manifest.locator,
                    &generation_compatibility(),
                    &prepared.identity,
                    reader.completeness(),
                    prepared.manifest.entries.len(),
                    manifest_input_bytes(&prepared.manifest),
                )
                .map_err(map_cache_error)?;
            published_artifacts.extend(locator_outcome.published_paths);
            validate_scope(&request, &opening_scope, started, &published_artifacts)?;
        }

        validate_scope(&request, &opening_scope, started, &published_artifacts)?;
        let query = query_graph(
            &reader,
            None,
            request.changed_symbols,
            &provider_id,
            &request.index_budget,
            &mut index_limitations,
        )?;
        metrics.query_rows = query.rows_read;
        metrics.symbols = query.symbols.len();
        metrics.edges = query.edges.len();
        metrics.output_bytes = serde_json::to_vec(&query.edges)
            .map(|bytes| bytes.len())
            .unwrap_or(0);
        validate_scope(&request, &opening_scope, started, &published_artifacts)?;

        let limitations = impact_limitations(&provider_id, &index_limitations);
        let status = provider_status(
            query.index_completeness,
            query.query_completeness,
            query.output_truncated,
            &index_limitations,
        );
        metrics.elapsed_ms = elapsed_ms(started);
        let provider = provider_record(
            &provider_id,
            &prepared.identity,
            status,
            prepared.manifest.entries.len(),
            manifest_input_bytes(&prepared.manifest),
            &query,
            &cache,
            &limitations,
            elapsed_ms(started),
        );
        Ok(RepositoryIndexOutput {
            generation_key: prepared.identity.generation_key().map_err(|error| {
                RepositoryIndexError::new("repository-index-identity-invalid", error.to_string())
            })?,
            provider,
            symbols: query.symbols,
            edges: query.edges,
            domain_summaries: query.summaries,
            index_completeness: query.index_completeness,
            query_completeness: query.query_completeness,
            reached_depth: query.reached_depth,
            output_truncated: query.output_truncated,
            limitations,
            metrics,
        })
    }

    fn analyze_fast_exact(
        &self,
        request: RepositoryIndexRequest<'_>,
        opening_scope: &str,
        provider_id: &str,
        started: Instant,
    ) -> Result<RepositoryIndexOutput, RepositoryIndexError> {
        let compatibility = generation_compatibility();
        let locator_store = GenerationLocatorStore::new(self.layout.clone());
        let mut cache = CacheStats::default();
        let mut index_limitations = Vec::new();
        let mut located = if request.cache_read {
            match locator_store
                .lookup_exact(
                    request.manifest_source.repository_locator(),
                    &compatibility,
                    reader_limits(&request.index_budget),
                )
                .map_err(map_cache_error)?
            {
                CacheLookup::Hit(located) => {
                    cache.hits += 1;
                    Some(located)
                }
                CacheLookup::Miss => {
                    cache.misses += 1;
                    None
                }
                CacheLookup::Stale { code } => {
                    cache.stale += 1;
                    index_limitations.push(simple_index_limitation(
                        "repository-index-generation-stale",
                        &code,
                    ));
                    None
                }
                CacheLookup::Corrupt { code } => {
                    cache.corrupt += 1;
                    index_limitations.push(simple_index_limitation(
                        "repository-index-generation-corrupt",
                        &code,
                    ));
                    None
                }
            }
        } else {
            cache.misses += 1;
            None
        };

        if located.is_none() && request.cache_read {
            match locator_store
                .lookup_base(
                    request.manifest_source.repository_locator(),
                    &compatibility,
                    reader_limits(&request.index_budget),
                )
                .map_err(map_cache_error)?
            {
                CacheLookup::Hit(base) => {
                    cache.hits += 1;
                    located = Some(base);
                }
                CacheLookup::Miss => {}
                CacheLookup::Stale { code } => {
                    cache.stale += 1;
                    index_limitations.push(simple_index_limitation(
                        "repository-index-base-generation-stale",
                        &code,
                    ));
                }
                CacheLookup::Corrupt { code } => {
                    cache.corrupt += 1;
                    index_limitations.push(simple_index_limitation(
                        "repository-index-base-generation-corrupt",
                        &code,
                    ));
                }
            }
        }

        let Some(LocatedGeneration { reference, reader }) = located else {
            index_limitations.push(simple_index_limitation(
                "repository-index-generation-miss",
                "no exact compatible immutable repository graph generation is available",
            ));
            validate_scope(&request, opening_scope, started, &[])?;
            let lookup_key = locator_store
                .exact_lookup_digest(request.manifest_source.repository_locator(), &compatibility)
                .map_err(map_cache_error)?;
            return Ok(finalize_fast_unavailable(
                provider_id,
                &lookup_key,
                &compatibility,
                cache,
                index_limitations,
                started,
            ));
        };

        let mut metrics = IndexMetrics {
            elapsed_ms: elapsed_ms(started),
            manifest_files: reference.manifest_files,
            manifest_bytes: reference.manifest_bytes,
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
        };
        let mut overlay = None;
        let mut overlay_query_rows = 0usize;
        if reference.locator != *request.manifest_source.repository_locator() {
            let mut tracker = IndexBudgetTracker::new(request.index_budget.clone());
            let candidate_graph = build_fast_candidate_graph(
                &request,
                opening_scope,
                &reader,
                &reference.identity,
                &mut tracker,
                &mut index_limitations,
                &mut metrics,
                started,
            )?;
            let changed_paths = request
                .candidate
                .files()
                .iter()
                .map(|file| file.path.clone())
                .collect::<BTreeSet<_>>();
            let built =
                build_repository_overlay(&reader, &candidate_graph, &changed_paths, &mut tracker)
                    .map_err(|error| RepositoryIndexError::new(error.code, error.message))?;
            index_limitations.extend(built.limitations.clone());
            overlay = Some(built);
            overlay_query_rows = tracker.amount(IndexResource::QueryRows).consumed;
        }

        validate_scope(&request, opening_scope, started, &[])?;
        let mut query_budget = request.index_budget.clone();
        query_budget.max_query_rows = query_budget
            .max_query_rows
            .saturating_sub(overlay_query_rows);
        query_budget.deadline = query_budget.deadline.saturating_sub(started.elapsed());
        let query = query_graph(
            &reader,
            overlay.as_ref(),
            request.changed_symbols,
            provider_id,
            &query_budget,
            &mut index_limitations,
        )?;
        validate_scope(&request, opening_scope, started, &[])?;
        let limitations = impact_limitations(provider_id, &index_limitations);
        let status = provider_status(
            query.index_completeness,
            query.query_completeness,
            query.output_truncated,
            &index_limitations,
        );
        metrics.elapsed_ms = elapsed_ms(started);
        metrics.symbols = query.symbols.len();
        metrics.edges = query.edges.len();
        metrics.query_rows = overlay_query_rows.saturating_add(query.rows_read);
        metrics.output_bytes = serde_json::to_vec(&query.edges)
            .map(|bytes| bytes.len())
            .unwrap_or(0);
        let generation_path = self
            .layout
            .graphs_dir
            .join(format!("{}.sqlite", reference.generation_key));
        metrics.generation_bytes = std::fs::metadata(generation_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let mut provider = provider_record(
            provider_id,
            &reference.identity,
            status,
            reference.manifest_files,
            reference.manifest_bytes,
            &query,
            &cache,
            &limitations,
            elapsed_ms(started),
        );
        if overlay.is_some() {
            provider.configuration_digest = sha256_hex(
                &serde_json::to_vec(&(
                    &reference.identity,
                    request.manifest_source.repository_locator(),
                ))
                .unwrap_or_else(|_| b"invalid".to_vec()),
            );
        }
        Ok(RepositoryIndexOutput {
            generation_key: reference.generation_key,
            provider,
            symbols: query.symbols,
            edges: query.edges,
            domain_summaries: query.summaries,
            index_completeness: query.index_completeness,
            query_completeness: query.query_completeness,
            reached_depth: query.reached_depth,
            output_truncated: query.output_truncated,
            limitations,
            metrics,
        })
    }
}

struct QueryOutput {
    symbols: Vec<ChangedSymbol>,
    edges: Vec<ImpactEdge>,
    summaries: Vec<DomainSummary>,
    index_completeness: Completeness,
    query_completeness: Completeness,
    reached_depth: usize,
    rows_read: usize,
    output_truncated: bool,
}

struct OverlayPathDelta {
    files: Vec<GraphFile>,
    symbols: Vec<GraphSymbol>,
    edges: Vec<GraphEdge>,
    limitations: Vec<IndexLimitation>,
}

#[allow(clippy::too_many_arguments)]
fn build_fast_candidate_graph(
    request: &RepositoryIndexRequest<'_>,
    opening_scope: &str,
    base: &RepositoryGraphReader,
    base_identity: &GraphGenerationIdentity,
    tracker: &mut IndexBudgetTracker,
    limitations: &mut Vec<IndexLimitation>,
    metrics: &mut IndexMetrics,
    started: Instant,
) -> Result<RepositoryGraph, RepositoryIndexError> {
    let mut identity = base_identity.clone();
    identity.candidate_manifest_digest = request
        .manifest_source
        .repository_locator()
        .overlay_candidate_digest
        .clone();
    identity.validate().map_err(|error| {
        RepositoryIndexError::new("repository-overlay-identity-invalid", error.to_string())
    })?;

    let mut files = Vec::new();
    let mut symbols = Vec::new();
    let mut edges = Vec::new();
    let mut graph_limitations = Vec::new();
    for changed in request
        .candidate
        .files()
        .iter()
        .filter(|file| file.manifest_unit_id.is_some())
    {
        tracker
            .check_deadline()
            .map_err(|error| RepositoryIndexError::new(error.code(), error.to_string()))?;
        if changed.presence != CandidatePresence::Present {
            graph_limitations.push(overlay_limitation(
                "repository-overlay-path-removed",
                changed.path.clone(),
                "the candidate path is absent and its base symbols are tombstoned",
            ));
            continue;
        }
        if !changed.path.as_str().ends_with(".rs") {
            graph_limitations.push(overlay_limitation(
                "repository-overlay-language-unsupported",
                changed.path.clone(),
                "the changed path has no incremental repository resolver",
            ));
            continue;
        }

        let content = read_candidate_bytes(request, opening_scope, &changed.path, started, &[])?;
        let key = FileFactKey {
            language: "rust".to_string(),
            content_sha256: content.sha256.clone(),
            grammar_version: GRAMMAR_VERSION.to_string(),
            query_digest: sha256_hex(b"tree-sitter-rust-index-query/v1"),
            adapter_version: ADAPTER_VERSION.to_string(),
            normalization_rules_digest: sha256_hex(NORMALIZATION_VERSION.as_bytes()),
            schema_version: 1,
        };
        let facts =
            TreeSitterRustAdapter::analyze_index(&content.bytes, tracker).map_err(|error| {
                RepositoryIndexError::new("repository-overlay-rust-parse-failed", error.to_string())
            })?;
        metrics.file_fact_misses = metrics.file_fact_misses.saturating_add(1);
        metrics.parsed_files = metrics.parsed_files.saturating_add(1);
        metrics.parsed_bytes = metrics
            .parsed_bytes
            .saturating_add(content.bytes.len() as u64);
        let base_file = base.file_for_path(&changed.path).map_err(map_graph_error)?;
        let symbol_limit = base
            .maximum_rows_per_query()
            .min(tracker.amount(IndexResource::QueryRows).remaining);
        let base_symbols = if symbol_limit == 0 {
            graph_limitations.push(overlay_limitation(
                "index-query-row-budget-exhausted",
                changed.path.clone(),
                "the overlay base-symbol query budget was exhausted",
            ));
            Vec::new()
        } else {
            let rows = base
                .symbols_for_path(&changed.path, symbol_limit)
                .map_err(map_graph_error)?;
            tracker
                .consume(IndexResource::QueryRows, rows.len())
                .map_err(|error| RepositoryIndexError::new(error.code(), error.to_string()))?;
            if rows.len() == symbol_limit {
                graph_limitations.push(overlay_limitation(
                    "index-query-row-budget-exhausted",
                    changed.path.clone(),
                    "the overlay base-symbol query reached its exact row limit",
                ));
            }
            rows
        };
        let edge_limit = base
            .maximum_rows_per_query()
            .min(tracker.amount(IndexResource::QueryRows).remaining);
        let base_edges = if edge_limit == 0 {
            graph_limitations.push(overlay_limitation(
                "index-query-row-budget-exhausted",
                changed.path.clone(),
                "the overlay base-edge query budget was exhausted",
            ));
            Vec::new()
        } else {
            let rows = base
                .edges_for_path(&changed.path, edge_limit)
                .map_err(map_graph_error)?;
            tracker
                .consume(IndexResource::QueryRows, rows.len())
                .map_err(|error| RepositoryIndexError::new(error.code(), error.to_string()))?;
            if rows.len() == edge_limit {
                graph_limitations.push(overlay_limitation(
                    "index-query-row-budget-exhausted",
                    changed.path.clone(),
                    "the overlay base-edge query reached its exact row limit",
                ));
            }
            rows
        };
        let delta = resolve_overlay_path(
            changed,
            &content.sha256,
            key,
            &facts,
            base_file,
            &base_symbols,
            &base_edges,
            base,
            tracker,
        )?;
        files.extend(delta.files);
        symbols.extend(delta.symbols);
        edges.extend(delta.edges);
        graph_limitations.extend(delta.limitations);
    }
    graph_limitations.push(IndexLimitation {
        code: "repository-overlay-incremental-resolution".to_string(),
        path: request.candidate.files().first().map(|file| file.path.clone()),
        symbol_id: None,
        reason: "Fast Mode refreshed only the authoritative changed-path closure".to_string(),
        interpretation:
            "relationships requiring compiler expansion or an unindexed reverse closure may be incomplete"
                .to_string(),
    });
    graph_limitations.sort_by(|left, right| {
        (
            left.code.as_str(),
            left.path.as_ref().map(RepoPath::as_str).unwrap_or(""),
            left.symbol_id.as_deref().unwrap_or(""),
        )
            .cmp(&(
                right.code.as_str(),
                right.path.as_ref().map(RepoPath::as_str).unwrap_or(""),
                right.symbol_id.as_deref().unwrap_or(""),
            ))
    });
    graph_limitations.dedup();
    limitations.extend(graph_limitations.clone());
    files.sort_by(|left, right| left.path.cmp(&right.path));
    symbols.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
    edges.sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
    Ok(RepositoryGraph {
        identity,
        files,
        modules: Vec::new(),
        symbols,
        edges,
        completeness: Completeness::Partial,
        limitations: graph_limitations,
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve_overlay_path(
    changed: &crate::candidate::CandidateFile,
    content_sha256: &str,
    key: FileFactKey,
    facts: &crate::impact_context::adapters::tree_sitter_rust::RustFileFacts,
    base_file: Option<GraphFile>,
    base_symbols: &[GraphSymbol],
    base_edges: &[GraphEdge],
    base: &RepositoryGraphReader,
    tracker: &mut IndexBudgetTracker,
) -> Result<OverlayPathDelta, RepositoryIndexError> {
    let mut limitations = Vec::new();
    let base_by_local = base_symbols
        .iter()
        .map(|symbol| (symbol.local_id.as_str(), symbol))
        .collect::<BTreeMap<_, _>>();
    let mut primary_module = base_file
        .as_ref()
        .and_then(|file| file.module_id.clone())
        .or_else(|| base_symbols.first().map(|symbol| symbol.module_id.clone()));
    if primary_module.is_none() {
        primary_module = infer_added_file_module(base, &changed.path, tracker)?;
        if primary_module.is_some() {
            limitations.push(overlay_limitation(
                "repository-overlay-module-inferred",
                changed.path.clone(),
                "the added Rust file module was inferred from an indexed parent module",
            ));
        }
    }
    let mut symbols = Vec::new();
    let mut ids_by_local = BTreeMap::new();
    for fact in &facts.symbols {
        let module_id = base_by_local
            .get(fact.local_id.as_str())
            .map(|symbol| symbol.module_id.clone())
            .or_else(|| primary_module.clone());
        let Some(module_id) = module_id else {
            limitations.push(overlay_limitation(
                "repository-overlay-module-unresolved",
                changed.path.clone(),
                "the changed symbol could not be assigned to a known base module",
            ));
            continue;
        };
        if let Err(error) = tracker.consume(IndexResource::Symbols, 1) {
            limitations.push(overlay_limitation(
                error.code(),
                changed.path.clone(),
                "the overlay symbol budget was exhausted",
            ));
            break;
        }
        let symbol_id = repository_symbol_id(&module_id, &changed.path, &fact.local_id);
        ids_by_local.insert(fact.local_id.clone(), symbol_id.clone());
        symbols.push(GraphSymbol {
            symbol_id,
            local_id: fact.local_id.clone(),
            module_id,
            path: changed.path.clone(),
            language: "rust".to_string(),
            kind: fact.kind.clone(),
            name: fact.name.clone(),
            owner_symbol_id: None,
            signature: (!fact.signature.is_empty()).then(|| fact.signature.clone()),
            visibility: fact.visibility.clone(),
            range: fact.range.clone(),
            confidence: Confidence::Medium,
        });
    }
    for symbol in &mut symbols {
        symbol.owner_symbol_id = facts
            .symbols
            .iter()
            .find(|fact| fact.local_id == symbol.local_id)
            .and_then(|fact| fact.owner_local_id.as_ref())
            .and_then(|owner| ids_by_local.get(owner))
            .cloned();
    }

    let candidate_by_base_id = base_symbols
        .iter()
        .filter_map(|base_symbol| {
            ids_by_local
                .get(&base_symbol.local_id)
                .map(|candidate| (base_symbol.symbol_id.as_str(), candidate.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let mut edges = Vec::new();
    let mut retained_calls = BTreeSet::new();
    for base_edge in base_edges {
        let Some(from_symbol) = candidate_by_base_id.get(base_edge.from_symbol.as_str()) else {
            continue;
        };
        let target_name = match base_edge.to_symbol.as_deref() {
            Some(target) => base
                .symbol(target)
                .map_err(map_graph_error)?
                .map(|symbol| symbol.name),
            None => base_edge.unresolved_target.clone(),
        };
        if !overlay_edge_still_present(base_edge, target_name.as_deref(), facts) {
            continue;
        }
        let to_symbol = base_edge.to_symbol.as_ref().map(|target| {
            candidate_by_base_id
                .get(target.as_str())
                .cloned()
                .unwrap_or_else(|| target.clone())
        });
        let edge = make_overlay_edge(
            base_edge.kind,
            from_symbol,
            to_symbol,
            base_edge.unresolved_target.clone(),
            &changed.path,
            &base_edge.range,
            base_edge.resolution,
            base_edge.confidence,
            base_edge.limitation_code.clone(),
        );
        if edge.kind == EdgeKind::Calls {
            retained_calls.insert((
                edge.range.start_byte,
                edge.range.end_byte,
                target_name.unwrap_or_default(),
            ));
        }
        edges.push(edge);
    }
    for call in &facts.calls {
        if retained_calls.contains(&(
            call.range.start_byte,
            call.range.end_byte,
            call.callee.clone(),
        )) {
            continue;
        }
        let Some(from_symbol) = call
            .caller_local_id
            .as_ref()
            .and_then(|local| ids_by_local.get(local))
            .or_else(|| symbols.first().map(|symbol| &symbol.symbol_id))
        else {
            continue;
        };
        edges.push(make_overlay_edge(
            EdgeKind::Calls,
            from_symbol,
            None,
            Some(call.callee.clone()),
            &changed.path,
            &call.range,
            Resolution::Unresolved,
            Confidence::Low,
            Some("repository-overlay-call-unresolved".to_string()),
        ));
    }
    edges.sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
    edges.dedup_by(|left, right| left.edge_id == right.edge_id);
    let file = base_file.unwrap_or(GraphFile {
        path: changed.path.clone(),
        mode: changed.mode.clone(),
        presence: CandidatePresence::Present,
        content_sha256: None,
        file_fact_key: None,
        language: Some("rust".to_string()),
        module_id: primary_module,
    });
    let mut file = file;
    file.mode = changed.mode.clone();
    file.presence = CandidatePresence::Present;
    file.content_sha256 = Some(content_sha256.to_string());
    file.file_fact_key = Some(key);
    file.language = Some("rust".to_string());
    Ok(OverlayPathDelta {
        files: vec![file],
        symbols,
        edges,
        limitations,
    })
}

fn overlay_edge_still_present(
    edge: &GraphEdge,
    target_name: Option<&str>,
    facts: &crate::impact_context::adapters::tree_sitter_rust::RustFileFacts,
) -> bool {
    match edge.kind {
        EdgeKind::Calls => facts.calls.iter().any(|call| {
            call.range == edge.range
                && target_name.is_none_or(|target| call.callee.as_str() == target)
        }),
        EdgeKind::References => facts.references.iter().any(|reference| {
            reference.range == edge.range
                && target_name.is_none_or(|target| reference.name.as_str() == target)
        }),
        EdgeKind::Imports | EdgeKind::Exports => facts
            .imports
            .iter()
            .any(|import| import.range == edge.range),
        EdgeKind::Defines | EdgeKind::Implements | EdgeKind::Overrides => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn make_overlay_edge(
    kind: EdgeKind,
    from_symbol: &str,
    to_symbol: Option<String>,
    unresolved_target: Option<String>,
    path: &RepoPath,
    range: &SourceRange,
    resolution: Resolution,
    confidence: Confidence,
    limitation_code: Option<String>,
) -> GraphEdge {
    let mut digest = Sha256::new();
    hash_component(&mut digest, b"rust-repository-edge/v1");
    hash_component(&mut digest, edge_kind_name(kind).as_bytes());
    hash_component(&mut digest, from_symbol.as_bytes());
    hash_component(&mut digest, to_symbol.as_deref().unwrap_or("").as_bytes());
    hash_component(
        &mut digest,
        unresolved_target.as_deref().unwrap_or("").as_bytes(),
    );
    hash_component(&mut digest, path.as_str().as_bytes());
    hash_component(&mut digest, &range.start_byte.to_be_bytes());
    hash_component(&mut digest, &range.end_byte.to_be_bytes());
    GraphEdge {
        edge_id: format!("{:x}", digest.finalize()),
        kind,
        from_symbol: from_symbol.to_string(),
        to_symbol,
        unresolved_target,
        path: path.clone(),
        range: range.clone(),
        provider_id: "rust-tree-sitter-resolver".to_string(),
        provider_version: RESOLVER_VERSION.to_string(),
        resolution,
        confidence,
        limitation_code,
    }
}

fn repository_symbol_id(module_id: &str, path: &RepoPath, local_id: &str) -> String {
    let mut digest = Sha256::new();
    hash_component(&mut digest, b"rust-repository-symbol/v1");
    hash_component(&mut digest, module_id.as_bytes());
    hash_component(&mut digest, path.as_str().as_bytes());
    hash_component(&mut digest, local_id.as_bytes());
    format!("{:x}", digest.finalize())
}

fn infer_added_file_module(
    base: &RepositoryGraphReader,
    path: &RepoPath,
    tracker: &mut IndexBudgetTracker,
) -> Result<Option<String>, RepositoryIndexError> {
    let components = path.as_str().split('/').collect::<Vec<_>>();
    let Some(filename) = components.last().copied() else {
        return Ok(None);
    };
    let (module_name, parent_directory) = if filename == "mod.rs" {
        if components.len() < 2 {
            return Ok(None);
        }
        (
            components[components.len() - 2],
            &components[..components.len() - 2],
        )
    } else {
        let Some(module_name) = filename.strip_suffix(".rs") else {
            return Ok(None);
        };
        (module_name, &components[..components.len() - 1])
    };
    if module_name.is_empty() || parent_directory.is_empty() {
        return Ok(None);
    }

    let directory = parent_directory.join("/");
    let mut candidates = Vec::new();
    if filename != "mod.rs" {
        candidates.push(format!("{directory}/mod.rs"));
        if parent_directory.len() > 1 {
            candidates.push(format!("{directory}.rs"));
        }
    }
    candidates.push(format!("{directory}/lib.rs"));
    candidates.push(format!("{directory}/main.rs"));
    candidates.sort();
    candidates.dedup();

    for candidate in candidates {
        if tracker.amount(IndexResource::QueryRows).remaining == 0 {
            return Ok(None);
        }
        let candidate = RepoPath::new(candidate).map_err(|error| {
            RepositoryIndexError::new("repository-overlay-module-path-invalid", error.to_string())
        })?;
        let Some(parent) = base.file_for_path(&candidate).map_err(map_graph_error)? else {
            continue;
        };
        tracker
            .consume(IndexResource::QueryRows, 1)
            .map_err(|error| RepositoryIndexError::new(error.code(), error.to_string()))?;
        if let Some(parent_module_id) = parent.module_id {
            return Ok(Some(repository_module_id(
                &parent_module_id,
                module_name,
                path,
            )));
        }
    }
    Ok(None)
}

fn repository_module_id(parent_module_id: &str, name: &str, path: &RepoPath) -> String {
    let mut digest = Sha256::new();
    hash_component(&mut digest, b"rust-repository-module/v1");
    hash_component(&mut digest, parent_module_id.as_bytes());
    hash_component(&mut digest, name.as_bytes());
    hash_component(&mut digest, path.as_str().as_bytes());
    hash_component(&mut digest, &[0]);
    format!("{:x}", digest.finalize())
}

fn edge_kind_name(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Defines => "defines",
        EdgeKind::References => "references",
        EdgeKind::Imports => "imports",
        EdgeKind::Exports => "exports",
        EdgeKind::Calls => "calls",
        EdgeKind::Implements => "implements",
        EdgeKind::Overrides => "overrides",
    }
}

fn overlay_limitation(code: &str, path: RepoPath, reason: &str) -> IndexLimitation {
    IndexLimitation {
        code: code.to_string(),
        path: Some(path),
        symbol_id: None,
        reason: reason.to_string(),
        interpretation: "the Fast candidate overlay is partial for this path".to_string(),
    }
}

fn hash_component(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn prepare_index(
    source: &dyn RepositoryManifestSource,
    tracker: &mut IndexBudgetTracker,
) -> Result<PreparedIndex, RepositoryIndexError> {
    let manifest = source
        .manifest_bounded(tracker)
        .map_err(|error| RepositoryIndexError::new(error.code, error.message))?;
    let project_model = build_rust_project_model(source, &manifest, tracker)
        .map_err(|error| RepositoryIndexError::new(error.code, error.message))?;
    let file_keys = manifest
        .entries
        .iter()
        .filter(|entry| {
            entry.presence == CandidatePresence::Present
                && entry.language.as_deref() == Some("rust")
                && entry.content_sha256.is_some()
        })
        .map(|entry| {
            let content_sha256 = entry.content_sha256.clone().unwrap_or_default();
            (
                entry.path.clone(),
                FileFactKey {
                    language: "rust".to_string(),
                    content_sha256,
                    grammar_version: GRAMMAR_VERSION.to_string(),
                    query_digest: sha256_hex(b"tree-sitter-rust-index-query/v1"),
                    adapter_version: ADAPTER_VERSION.to_string(),
                    normalization_rules_digest: sha256_hex(NORMALIZATION_VERSION.as_bytes()),
                    schema_version: 1,
                },
            )
        })
        .collect::<Vec<_>>();
    let file_facts_manifest_digest =
        sha256_hex(&serde_json::to_vec(&file_keys).map_err(|error| {
            RepositoryIndexError::new("repository-index-key-encode", error.to_string())
        })?);
    let identity = GraphGenerationIdentity {
        graph_schema_version: 1,
        candidate_manifest_digest: manifest.digest.clone(),
        project_model_digest: project_model.digest.clone(),
        resolver_digest: sha256_hex(RESOLVER_VERSION.as_bytes()),
        adapter_query_digest: sha256_hex(b"tree-sitter-rust-index-query/v1"),
        file_facts_manifest_digest,
        normalization_rules_digest: sha256_hex(NORMALIZATION_VERSION.as_bytes()),
    };
    identity.validate().map_err(|error| {
        RepositoryIndexError::new("repository-index-identity-invalid", error.to_string())
    })?;
    Ok(PreparedIndex {
        manifest,
        project_model,
        file_keys,
        identity,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_file_facts(
    request: &RepositoryIndexRequest<'_>,
    opening_scope: &str,
    prepared: &PreparedIndex,
    store: &FileFactsStore,
    tracker: &mut IndexBudgetTracker,
    cache: &mut CacheStats,
    metrics: &mut IndexMetrics,
    limitations: &mut Vec<IndexLimitation>,
    started: Instant,
    published_artifacts: &mut Vec<PathBuf>,
) -> Result<Vec<RustRepositoryFileFacts>, RepositoryIndexError> {
    let mut output = Vec::new();
    for (path, key) in &prepared.file_keys {
        tracker
            .check_deadline()
            .map_err(|error| RepositoryIndexError::new(error.code(), error.to_string()))?;
        let lookup = if request.cache_read {
            store.lookup(key).map_err(map_cache_error)?
        } else {
            CacheLookup::Miss
        };
        let facts = match lookup {
            CacheLookup::Hit(facts) => {
                cache.hits += 1;
                metrics.file_fact_hits += 1;
                facts
            }
            CacheLookup::Miss => {
                cache.misses += 1;
                metrics.file_fact_misses += 1;
                let content = read_manifest_bytes(
                    request,
                    opening_scope,
                    path,
                    &key.content_sha256,
                    started,
                    published_artifacts,
                )?;
                let facts = TreeSitterRustAdapter::analyze_index(&content.bytes, tracker).map_err(
                    |error| {
                        RepositoryIndexError::new(
                            "repository-index-rust-parse-failed",
                            error.to_string(),
                        )
                    },
                )?;
                metrics.parsed_files += 1;
                metrics.parsed_bytes = metrics
                    .parsed_bytes
                    .saturating_add(content.bytes.len() as u64);
                if request.cache_write {
                    validate_scope(request, opening_scope, started, published_artifacts)?;
                    match store.publish(key, &facts).map_err(map_cache_error)? {
                        PublishResult::Published => {
                            published_artifacts
                                .push(store.object_path(key).map_err(map_cache_error)?);
                            validate_scope(request, opening_scope, started, published_artifacts)?;
                            metrics.file_fact_writes += 1;
                        }
                        PublishResult::Reused => {}
                    }
                    validate_scope(request, opening_scope, started, published_artifacts)?;
                }
                facts
            }
            CacheLookup::Stale { code } => {
                cache.stale += 1;
                metrics.file_fact_misses += 1;
                limitations.push(simple_index_limitation(
                    "repository-index-file-facts-stale",
                    &code,
                ));
                parse_without_publish(
                    request,
                    opening_scope,
                    path,
                    &key.content_sha256,
                    tracker,
                    metrics,
                    started,
                    published_artifacts,
                )?
            }
            CacheLookup::Corrupt { code } => {
                cache.corrupt += 1;
                metrics.file_fact_misses += 1;
                limitations.push(simple_index_limitation(
                    "repository-index-file-facts-corrupt",
                    &code,
                ));
                parse_without_publish(
                    request,
                    opening_scope,
                    path,
                    &key.content_sha256,
                    tracker,
                    metrics,
                    started,
                    published_artifacts,
                )?
            }
        };
        for code in &facts.limitations {
            limitations.push(simple_index_limitation(code, "Rust FileFacts are partial"));
        }
        output.push(RustRepositoryFileFacts {
            path: path.clone(),
            key: key.clone(),
            facts,
        });
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn parse_without_publish(
    request: &RepositoryIndexRequest<'_>,
    opening_scope: &str,
    path: &RepoPath,
    content_sha256: &str,
    tracker: &mut IndexBudgetTracker,
    metrics: &mut IndexMetrics,
    started: Instant,
    published_artifacts: &[PathBuf],
) -> Result<crate::impact_context::adapters::tree_sitter_rust::RustFileFacts, RepositoryIndexError>
{
    let content = read_manifest_bytes(
        request,
        opening_scope,
        path,
        content_sha256,
        started,
        published_artifacts,
    )?;
    let facts = TreeSitterRustAdapter::analyze_index(&content.bytes, tracker).map_err(|error| {
        RepositoryIndexError::new("repository-index-rust-parse-failed", error.to_string())
    })?;
    metrics.parsed_files += 1;
    metrics.parsed_bytes = metrics
        .parsed_bytes
        .saturating_add(content.bytes.len() as u64);
    Ok(facts)
}

fn read_manifest_bytes(
    request: &RepositoryIndexRequest<'_>,
    opening_scope: &str,
    path: &RepoPath,
    expected_sha256: &str,
    started: Instant,
    published_artifacts: &[PathBuf],
) -> Result<crate::candidate::CandidateBytes, RepositoryIndexError> {
    validate_scope(request, opening_scope, started, published_artifacts)?;
    let content = request
        .manifest_source
        .read_bounded(path, request.index_budget.max_file_bytes)
        .map_err(|error| {
            RepositoryIndexError::new(
                "repository-index-file-read-failed",
                format!("cannot read {}: {error}", path.as_str()),
            )
        })?;
    validate_content_digest(path, &content, Some(expected_sha256))?;
    validate_scope(request, opening_scope, started, published_artifacts)?;
    Ok(content)
}

fn read_candidate_bytes(
    request: &RepositoryIndexRequest<'_>,
    opening_scope: &str,
    path: &RepoPath,
    started: Instant,
    published_artifacts: &[PathBuf],
) -> Result<crate::candidate::CandidateBytes, RepositoryIndexError> {
    validate_scope(request, opening_scope, started, published_artifacts)?;
    let content = request
        .candidate
        .read_bounded(path, request.index_budget.max_file_bytes)
        .map_err(|error| {
            RepositoryIndexError::new(
                "repository-overlay-candidate-read-failed",
                format!("cannot read {}: {error}", path.as_str()),
            )
        })?;
    validate_content_digest(path, &content, None)?;
    validate_scope(request, opening_scope, started, published_artifacts)?;
    Ok(content)
}

fn validate_content_digest(
    path: &RepoPath,
    content: &crate::candidate::CandidateBytes,
    expected_sha256: Option<&str>,
) -> Result<(), RepositoryIndexError> {
    let actual_sha256 = sha256_hex(&content.bytes);
    if content.sha256 != actual_sha256
        || expected_sha256.is_some_and(|expected| expected != actual_sha256)
    {
        return Err(RepositoryIndexError::new(
            "repository-index-file-content-digest-mismatch",
            format!(
                "content bytes for {} do not match the authoritative FileFacts digest",
                path.as_str()
            ),
        ));
    }
    Ok(())
}

fn query_graph(
    reader: &RepositoryGraphReader,
    overlay: Option<&RepositoryOverlay>,
    changed_symbols: &[ChangedSymbol],
    provider_id: &str,
    budget: &IndexBudget,
    limitations: &mut Vec<IndexLimitation>,
) -> Result<QueryOutput, RepositoryIndexError> {
    let mut roots = BTreeSet::new();
    let mut graph_symbols = BTreeMap::<String, GraphSymbol>::new();
    let mut rows_read = 0usize;
    let mut query_completeness = Completeness::Complete;
    for changed in changed_symbols {
        let remaining = budget.max_query_rows.saturating_sub(rows_read);
        if remaining == 0 {
            limitations.push(simple_index_limitation(
                "index-query-row-budget-exhausted",
                "changed-symbol seed lookup exhausted the query row budget",
            ));
            query_completeness = Completeness::Partial;
            break;
        }
        let path = RepoPath::new(changed.path.clone()).map_err(|error| {
            RepositoryIndexError::new("repository-index-changed-path-invalid", error.to_string())
        })?;
        let path_limit = reader.maximum_rows_per_query().min(remaining);
        let mut candidates = reader
            .symbols_for_path(&path, path_limit)
            .map_err(map_graph_error)?;
        let base_rows_read = candidates.len();
        let mut removed_overlay_symbols = Vec::new();
        if let Some(overlay) = overlay {
            if overlay.path_tombstones.contains(&path) {
                removed_overlay_symbols.extend(
                    candidates
                        .iter()
                        .filter(|symbol| !overlay.symbols.contains_key(&symbol.symbol_id))
                        .cloned(),
                );
                candidates.retain(|symbol| symbol.path != path);
            }
            candidates.extend(
                overlay
                    .symbols
                    .values()
                    .filter(|symbol| symbol.path == path)
                    .cloned(),
            );
            candidates.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
            candidates.dedup_by(|left, right| left.symbol_id == right.symbol_id);
        }
        rows_read = rows_read.saturating_add(base_rows_read);
        if base_rows_read == path_limit {
            limitations.push(simple_index_limitation(
                "index-query-row-budget-exhausted",
                "changed-symbol seed lookup reached its exact row limit",
            ));
            query_completeness = Completeness::Partial;
        }
        let mut matched = candidates
            .into_iter()
            .filter(|symbol| {
                symbol.name == changed.name
                    && symbol.language == changed.language
                    && ranges_overlap(&symbol.range, &changed.range)
            })
            .collect::<Vec<_>>();
        if matched.is_empty() {
            limitations.push(IndexLimitation {
                code: "repository-index-changed-symbol-unmatched".to_string(),
                path: Some(path),
                symbol_id: None,
                reason: format!(
                    "changed symbol {} was not found in the repository graph",
                    changed.name
                ),
                interpretation: "graph traversal could not be seeded for this changed symbol"
                    .to_string(),
            });
        }
        matched.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
        for symbol in matched {
            roots.insert(symbol.symbol_id.clone());
            graph_symbols.insert(symbol.symbol_id.clone(), symbol);
        }
        for symbol in removed_overlay_symbols {
            roots.insert(symbol.symbol_id.clone());
            graph_symbols.insert(symbol.symbol_id.clone(), symbol);
        }
    }
    let request = TraversalRequest {
        roots: roots.iter().cloned().collect(),
        directions: BTreeSet::from([TraversalDirection::Incoming, TraversalDirection::Outgoing]),
        edge_kinds: BTreeSet::from([
            EdgeKind::References,
            EdgeKind::Imports,
            EdgeKind::Exports,
            EdgeKind::Calls,
            EdgeKind::Implements,
            EdgeKind::Overrides,
        ]),
        maximum_depth: budget.max_graph_depth,
        maximum_rows: budget.max_query_rows.saturating_sub(rows_read),
        maximum_nodes: budget.max_nodes,
        maximum_edges: budget.max_edges,
        maximum_bytes: MAXIMUM_TRAVERSAL_OUTPUT_BYTES.min(budget.max_generation_bytes),
        deadline: budget.deadline,
    };
    let traversal = traverse_repository_graph(reader, overlay, &request)
        .map_err(|error| RepositoryIndexError::new(error.code, error.message))?;
    rows_read = rows_read.saturating_add(traversal.rows_read);
    limitations.extend(traversal.limitations.clone());
    let mut retained_graph_edges = traversal.edges.clone();
    for edge in &traversal.edges {
        let mut ids = vec![edge.from_symbol.as_str()];
        if let Some(target) = edge.to_symbol.as_deref() {
            ids.push(target);
        }
        for symbol_id in ids {
            if graph_symbols.contains_key(symbol_id) {
                continue;
            }
            if rows_read >= budget.max_query_rows {
                limitations.push(simple_index_limitation(
                    "index-query-row-budget-exhausted",
                    "relationship symbol lookup exhausted the query row budget",
                ));
                query_completeness = Completeness::Partial;
                continue;
            }
            let symbol = if let Some(symbol) =
                overlay.and_then(|overlay| overlay.symbols.get(symbol_id).cloned())
            {
                Some(symbol)
            } else {
                reader.symbol(symbol_id).map_err(map_graph_error)?
            };
            if let Some(symbol) = symbol {
                rows_read = rows_read.saturating_add(1);
                graph_symbols.insert(symbol_id.to_string(), symbol);
            }
        }
    }
    retained_graph_edges.retain(|edge| {
        graph_symbols.contains_key(&edge.from_symbol)
            && edge
                .to_symbol
                .as_ref()
                .is_none_or(|target| graph_symbols.contains_key(target))
    });
    if retained_graph_edges.len() != traversal.edges.len() {
        query_completeness = Completeness::Partial;
    }
    let graph_symbols = graph_symbols.into_values().collect::<Vec<_>>();
    let (symbols, edges) =
        normalize_repository_graph(provider_id, &graph_symbols, &retained_graph_edges);
    let summaries = summarize_repository_graph(&roots, &symbols, &edges);
    query_completeness = merge_completeness(query_completeness, traversal.query_completeness);
    if roots.is_empty() && !changed_symbols.is_empty() {
        query_completeness = Completeness::Partial;
    }
    Ok(QueryOutput {
        symbols,
        edges,
        summaries,
        index_completeness: traversal.index_completeness,
        query_completeness,
        reached_depth: traversal.reached_depth,
        rows_read,
        output_truncated: traversal.output_truncated,
    })
}

fn open_reader(
    path: &std::path::Path,
    identity: &GraphGenerationIdentity,
    budget: &IndexBudget,
) -> Result<CacheLookup<RepositoryGraphReader>, RepositoryIndexError> {
    RepositoryGraphReader::open_immutable(path, identity, reader_limits(budget))
        .map_err(map_graph_error)
}

fn reader_limits(budget: &IndexBudget) -> ReaderLimits {
    ReaderLimits {
        maximum_database_bytes: u64::try_from(budget.max_generation_bytes).unwrap_or(u64::MAX),
        maximum_rows_per_query: budget.max_query_rows.max(1),
        maximum_string_bytes: 4_096,
    }
}

fn generation_compatibility() -> GenerationCompatibility {
    GenerationCompatibility {
        graph_schema_version: 1,
        resolver_digest: sha256_hex(RESOLVER_VERSION.as_bytes()),
        adapter_query_digest: sha256_hex(b"tree-sitter-rust-index-query/v1"),
        normalization_rules_digest: sha256_hex(NORMALIZATION_VERSION.as_bytes()),
    }
}

fn manifest_input_bytes(manifest: &RepositoryManifest) -> u64 {
    manifest
        .entries
        .iter()
        .map(|entry| entry.content_bytes.unwrap_or(0) as u64)
        .sum()
}

fn finalize_unavailable(
    provider_id: &str,
    prepared: &PreparedIndex,
    cache: CacheStats,
    limitations: Vec<IndexLimitation>,
    mut metrics: IndexMetrics,
    started: Instant,
) -> RepositoryIndexOutput {
    let limitations = impact_limitations(provider_id, &limitations);
    metrics.elapsed_ms = elapsed_ms(started);
    let query = QueryOutput {
        symbols: Vec::new(),
        edges: Vec::new(),
        summaries: Vec::new(),
        index_completeness: Completeness::Unavailable,
        query_completeness: Completeness::Unavailable,
        reached_depth: 0,
        rows_read: 0,
        output_truncated: false,
    };
    RepositoryIndexOutput {
        generation_key: prepared
            .identity
            .generation_key()
            .expect("prepared repository index identity is valid"),
        provider: provider_record(
            provider_id,
            &prepared.identity,
            if cache.stale > 0 {
                ProviderStatus::Stale
            } else if cache.corrupt > 0 {
                ProviderStatus::InvalidOutput
            } else {
                ProviderStatus::Unavailable
            },
            prepared.manifest.entries.len(),
            manifest_input_bytes(&prepared.manifest),
            &query,
            &cache,
            &limitations,
            elapsed_ms(started),
        ),
        symbols: Vec::new(),
        edges: Vec::new(),
        domain_summaries: Vec::new(),
        index_completeness: Completeness::Unavailable,
        query_completeness: Completeness::Unavailable,
        reached_depth: 0,
        output_truncated: false,
        limitations,
        metrics,
    }
}

fn finalize_fast_unavailable(
    provider_id: &str,
    lookup_key: &str,
    compatibility: &GenerationCompatibility,
    cache: CacheStats,
    limitations: Vec<IndexLimitation>,
    started: Instant,
) -> RepositoryIndexOutput {
    let limitations = impact_limitations(provider_id, &limitations);
    let elapsed = elapsed_ms(started);
    let provider = ProviderRecord {
        provider_id: provider_id.to_string(),
        provider_kind: PROVIDER_KIND.to_string(),
        provider_version: PROVIDER_VERSION.to_string(),
        configuration_digest: sha256_hex(
            &serde_json::to_vec(compatibility).unwrap_or_else(|_| b"invalid".to_vec()),
        ),
        status: if cache.stale > 0 {
            ProviderStatus::Stale
        } else if cache.corrupt > 0 {
            ProviderStatus::InvalidOutput
        } else {
            ProviderStatus::Unavailable
        },
        elapsed_ms: elapsed,
        input_files: 0,
        input_bytes: 0,
        output_fact_count: 0,
        cache_hits: cache.hits,
        cache_misses: cache.misses,
        cache_stale: cache.stale,
        cache_corrupt: cache.corrupt,
        limitation_ids: limitations
            .iter()
            .map(|limitation| limitation.limitation_id.clone())
            .collect(),
    };
    RepositoryIndexOutput {
        generation_key: lookup_key.to_string(),
        provider,
        symbols: Vec::new(),
        edges: Vec::new(),
        domain_summaries: Vec::new(),
        index_completeness: Completeness::Unavailable,
        query_completeness: Completeness::Unavailable,
        reached_depth: 0,
        output_truncated: false,
        limitations,
        metrics: IndexMetrics {
            elapsed_ms: elapsed,
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
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn provider_record(
    provider_id: &str,
    identity: &GraphGenerationIdentity,
    status: ProviderStatus,
    input_files: usize,
    input_bytes: u64,
    query: &QueryOutput,
    cache: &CacheStats,
    limitations: &[Limitation],
    elapsed_ms: u64,
) -> ProviderRecord {
    ProviderRecord {
        provider_id: provider_id.to_string(),
        provider_kind: PROVIDER_KIND.to_string(),
        provider_version: PROVIDER_VERSION.to_string(),
        configuration_digest: sha256_hex(
            &serde_json::to_vec(identity).unwrap_or_else(|_| b"invalid".to_vec()),
        ),
        status,
        elapsed_ms,
        input_files,
        input_bytes,
        output_fact_count: query
            .symbols
            .len()
            .saturating_add(query.edges.len())
            .saturating_add(query.summaries.len()),
        cache_hits: cache.hits,
        cache_misses: cache.misses,
        cache_stale: cache.stale,
        cache_corrupt: cache.corrupt,
        limitation_ids: limitations
            .iter()
            .map(|limitation| limitation.limitation_id.clone())
            .collect(),
    }
}

fn provider_status(
    index: Completeness,
    query: Completeness,
    output_truncated: bool,
    limitations: &[IndexLimitation],
) -> ProviderStatus {
    if limitations
        .iter()
        .any(|limitation| limitation.code.ends_with("budget-exhausted"))
    {
        ProviderStatus::BudgetExhausted
    } else if index == Completeness::Complete
        && query == Completeness::Complete
        && !output_truncated
    {
        ProviderStatus::Completed
    } else {
        ProviderStatus::Partial
    }
}

fn impact_limitations(provider_id: &str, limitations: &[IndexLimitation]) -> Vec<Limitation> {
    let mut output = limitations
        .iter()
        .map(|limitation| {
            let path = limitation.path.as_ref().map(RepoPath::as_str).unwrap_or("");
            let symbol_id = limitation.symbol_id.as_deref().unwrap_or("");
            let limitation_id = stable_id(
                "impact-limitation/v1",
                &[
                    limitation.code.as_str(),
                    provider_id,
                    path,
                    symbol_id,
                    limitation.reason.as_str(),
                    limitation.interpretation.as_str(),
                ],
            );
            Limitation {
                limitation_id,
                code: limitation.code.clone(),
                provider_id: Some(provider_id.to_string()),
                path: limitation
                    .path
                    .as_ref()
                    .map(|path| path.as_str().to_string()),
                symbol_id: limitation.symbol_id.clone(),
                reason: limitation.reason.clone(),
                interpretation: limitation.interpretation.clone(),
                improvable_in_deep_mode: true,
            }
        })
        .collect::<Vec<_>>();
    output.sort_by(|left, right| left.limitation_id.cmp(&right.limitation_id));
    output.dedup_by(|left, right| left.limitation_id == right.limitation_id);
    output
}

fn validate_request(request: &RepositoryIndexRequest<'_>) -> Result<(), RepositoryIndexError> {
    if request.mode == ImpactMode::Fast && request.cache_write {
        return Err(RepositoryIndexError::new(
            "repository-index-fast-write-forbidden",
            "Fast repository index collection cannot write cache state",
        ));
    }
    if request.candidate.source() != request.manifest_source.source() {
        return Err(RepositoryIndexError::new(
            "repository-index-source-mismatch",
            "candidate and repository manifest sources differ",
        ));
    }
    Ok(())
}

fn validate_scope(
    request: &RepositoryIndexRequest<'_>,
    opening_scope: &str,
    started: Instant,
    published_artifacts: &[PathBuf],
) -> Result<(), RepositoryIndexError> {
    let authoritative = request.manifest_source.revalidate_scope_bounded(
        request
            .index_budget
            .deadline
            .saturating_sub(started.elapsed()),
    );
    if authoritative.is_err()
        || request.candidate.scope_fingerprint() != opening_scope
        || request.manifest_source.scope_fingerprint() != opening_scope
    {
        let mut error = RepositoryIndexError::new(
            "repository-index-scope-drift",
            "authoritative scope changed during repository index collection",
        );
        if let Err(cleanup_error) = remove_published_artifacts(published_artifacts) {
            error.message = format!("{}; {cleanup_error}", error.message);
        }
        return Err(error);
    }
    Ok(())
}

fn remove_published_artifacts(paths: &[PathBuf]) -> Result<(), String> {
    let mut parents = BTreeSet::new();
    for path in paths.iter().rev() {
        match std::fs::remove_file(path) {
            Ok(()) => {
                if let Some(parent) = path.parent() {
                    parents.insert(parent.to_path_buf());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "cannot remove scope-invalid cache artifact {}: {error}",
                    path.display()
                ))
            }
        }
    }
    for parent in parents {
        sync_directory(&parent).map_err(|error| {
            format!(
                "cannot synchronize scope-invalid cache cleanup {}: {error}",
                parent.display()
            )
        })?;
    }
    Ok(())
}

pub fn repository_index_provider_id() -> String {
    stable_id("impact-provider/v1", &[PROVIDER_KIND, PROVIDER_VERSION])
}

fn simple_index_limitation(code: &str, detail: &str) -> IndexLimitation {
    IndexLimitation {
        code: code.to_string(),
        path: None,
        symbol_id: None,
        reason: detail.to_string(),
        interpretation: "repository graph evidence is incomplete or unavailable".to_string(),
    }
}

fn ranges_overlap(
    left: &crate::impact_context::contracts::SourceRange,
    right: &crate::impact_context::contracts::SourceRange,
) -> bool {
    left.start_byte <= right.end_byte && right.start_byte <= left.end_byte
}

fn merge_completeness(left: Completeness, right: Completeness) -> Completeness {
    match (left, right) {
        (Completeness::Unavailable, _) | (_, Completeness::Unavailable) => {
            Completeness::Unavailable
        }
        (Completeness::Partial, _) | (_, Completeness::Partial) => Completeness::Partial,
        (Completeness::Complete, Completeness::Complete) => Completeness::Complete,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn map_cache_error(
    error: crate::impact_context::cache::file_facts::CacheError,
) -> RepositoryIndexError {
    RepositoryIndexError::new(error.code, error.message)
}

fn map_graph_error(
    error: crate::impact_context::cache::sqlite_generation::RepositoryGraphError,
) -> RepositoryIndexError {
    RepositoryIndexError::new(error.code, error.message)
}
