use crate::candidate::{CandidateContent, CandidatePresence, RepoPath};
use crate::impact_context::adapters::tree_sitter_rust::TreeSitterRustAdapter;
use crate::impact_context::cache::file_facts::{
    CacheLayout, CacheLookup, FileFactsStore, PublishResult,
};
use crate::impact_context::cache::sqlite_generation::{
    GraphPublishOutcome, ReaderLimits, RepositoryGraphReader, RepositoryGraphWriter,
};
use crate::impact_context::contracts::{
    ChangedSymbol, Completeness, DomainSummary, EdgeKind, ImpactEdge, ImpactMode, Limitation,
    ProviderRecord, ProviderStatus,
};
use crate::impact_context::index::budget::{IndexBudget, IndexBudgetTracker};
use crate::impact_context::index::manifest::RepositoryManifestSource;
use crate::impact_context::index::model::{
    FileFactKey, GraphGenerationIdentity, GraphSymbol, IndexLimitation, IndexMetrics,
    RepositoryManifest,
};
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
        validate_scope(&request, &opening_scope)?;
        let provider_id = repository_index_provider_id();
        let mut tracker = IndexBudgetTracker::new(request.index_budget.clone());
        let prepared = prepare_index(request.manifest_source, &mut tracker)?;
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
            validate_scope(&request, &opening_scope)?;
            let path = match writer
                .publish(&graph, &mut tracker)
                .map_err(map_graph_error)?
            {
                GraphPublishOutcome::Published { path } | GraphPublishOutcome::Reused { path } => {
                    path
                }
            };
            validate_scope(&request, &opening_scope)?;
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
            validate_scope(&request, &opening_scope)?;
            return Ok(finalize_unavailable(
                &provider_id,
                &prepared,
                cache,
                index_limitations,
                metrics,
                started,
            ));
        };

        validate_scope(&request, &opening_scope)?;
        let query = query_graph(
            &reader,
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
        validate_scope(&request, &opening_scope)?;

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
            &prepared.manifest,
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
                let content = request
                    .manifest_source
                    .read_bounded(path, request.index_budget.max_file_bytes)
                    .map_err(|error| {
                        RepositoryIndexError::new(
                            "repository-index-file-read-failed",
                            format!("cannot read {}: {error}", path.as_str()),
                        )
                    })?;
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
                    validate_scope(request, opening_scope)?;
                    match store.publish(key, &facts).map_err(map_cache_error)? {
                        PublishResult::Published => metrics.file_fact_writes += 1,
                        PublishResult::Reused => {}
                    }
                    validate_scope(request, opening_scope)?;
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
                parse_without_publish(request, path, tracker, metrics)?
            }
            CacheLookup::Corrupt { code } => {
                cache.corrupt += 1;
                metrics.file_fact_misses += 1;
                limitations.push(simple_index_limitation(
                    "repository-index-file-facts-corrupt",
                    &code,
                ));
                parse_without_publish(request, path, tracker, metrics)?
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

fn parse_without_publish(
    request: &RepositoryIndexRequest<'_>,
    path: &RepoPath,
    tracker: &mut IndexBudgetTracker,
    metrics: &mut IndexMetrics,
) -> Result<crate::impact_context::adapters::tree_sitter_rust::RustFileFacts, RepositoryIndexError>
{
    let content = request
        .manifest_source
        .read_bounded(path, request.index_budget.max_file_bytes)
        .map_err(|error| {
            RepositoryIndexError::new(
                "repository-index-file-read-failed",
                format!("cannot read {}: {error}", path.as_str()),
            )
        })?;
    let facts = TreeSitterRustAdapter::analyze_index(&content.bytes, tracker).map_err(|error| {
        RepositoryIndexError::new("repository-index-rust-parse-failed", error.to_string())
    })?;
    metrics.parsed_files += 1;
    metrics.parsed_bytes = metrics
        .parsed_bytes
        .saturating_add(content.bytes.len() as u64);
    Ok(facts)
}

fn query_graph(
    reader: &RepositoryGraphReader,
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
        let candidates = reader
            .symbols_for_path(&path, path_limit)
            .map_err(map_graph_error)?;
        rows_read = rows_read.saturating_add(candidates.len());
        if candidates.len() == path_limit {
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
    let traversal = traverse_repository_graph(reader, None, &request)
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
            if let Some(symbol) = reader.symbol(symbol_id).map_err(map_graph_error)? {
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
    RepositoryGraphReader::open_immutable(
        path,
        identity,
        ReaderLimits {
            maximum_database_bytes: u64::try_from(budget.max_generation_bytes).unwrap_or(u64::MAX),
            maximum_rows_per_query: budget.max_query_rows.max(1),
            maximum_string_bytes: 4_096,
        },
    )
    .map_err(map_graph_error)
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
            &prepared.manifest,
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

#[allow(clippy::too_many_arguments)]
fn provider_record(
    provider_id: &str,
    identity: &GraphGenerationIdentity,
    status: ProviderStatus,
    manifest: &RepositoryManifest,
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
        input_files: manifest.entries.len(),
        input_bytes: manifest
            .entries
            .iter()
            .map(|entry| entry.content_bytes.unwrap_or(0) as u64)
            .sum(),
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
            let limitation_id = stable_id(
                "impact-limitation/v1",
                &[
                    limitation.code.as_str(),
                    provider_id,
                    limitation.reason.as_str(),
                    limitation.interpretation.as_str(),
                ],
            );
            Limitation {
                limitation_id,
                code: limitation.code.clone(),
                provider_id: Some(provider_id.to_string()),
                path: None,
                symbol_id: None,
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
) -> Result<(), RepositoryIndexError> {
    if request.candidate.scope_fingerprint() != opening_scope
        || request.manifest_source.scope_fingerprint() != opening_scope
    {
        return Err(RepositoryIndexError::new(
            "repository-index-scope-drift",
            "authoritative scope changed during repository index collection",
        ));
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
