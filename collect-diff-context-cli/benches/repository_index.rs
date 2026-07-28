use collect_diff_context_cli::candidate::{
    CandidateBytes, CandidateError, CandidatePresence, RepoPath,
};
use collect_diff_context_cli::impact_context::adapters::tree_sitter_rust::TreeSitterRustAdapter;
use collect_diff_context_cli::impact_context::cache::file_facts::{
    CacheLayout, CacheLookup, FileFactsStore,
};
use collect_diff_context_cli::impact_context::cache::sqlite_generation::{
    GraphPublishOutcome, ReaderLimits, RepositoryGraphReader, RepositoryGraphWriter,
};
use collect_diff_context_cli::impact_context::contracts::{
    Completeness, Confidence, EdgeKind, Resolution, SourceRange, UnitStatus,
};
use collect_diff_context_cli::impact_context::index::budget::{IndexBudget, IndexBudgetTracker};
use collect_diff_context_cli::impact_context::index::model::{
    FileFactKey, GraphEdge, GraphFile, GraphGenerationIdentity, GraphModule, GraphSymbol,
    RepositoryGraph, RepositoryLocator, RepositoryManifest, RepositoryManifestEntry,
};
use collect_diff_context_cli::impact_context::index::overlay::build_repository_overlay;
use collect_diff_context_cli::impact_context::index::project_model::{
    build_rust_project_model, ProjectModelSource, RustProjectModel,
};
use collect_diff_context_cli::impact_context::index::resolver::rust::{
    resolve_rust_repository, RustRepositoryFileFacts,
};
use collect_diff_context_cli::impact_context::index::traversal::{
    traverse_repository_graph, TraversalDirection, TraversalRequest,
};
use collect_diff_context_cli::review_scope::ReviewSource;
use collect_diff_context_cli::secret_scan::sanitize_for_model_optional;
use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

const SOURCE_FILES: usize = 16;

#[derive(Clone)]
struct BenchSource {
    bytes: BTreeMap<RepoPath, Vec<u8>>,
}

impl ProjectModelSource for BenchSource {
    fn read_bounded(
        &self,
        path: &RepoPath,
        maximum_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError> {
        let bytes = self
            .bytes
            .get(path)
            .unwrap_or_else(|| panic!("missing benchmark path: {}", path.as_str()));
        if bytes.len() > maximum_bytes {
            return Err(CandidateError::byte_limit_exceeded(path, maximum_bytes));
        }
        Ok(CandidateBytes {
            bytes: bytes.clone(),
            sha256: digest(bytes),
            binary: false,
        })
    }
}

struct RepositoryFixture {
    source: BenchSource,
    manifest: RepositoryManifest,
    project_model: RustProjectModel,
    file_facts: Vec<RustRepositoryFileFacts>,
    graph: collect_diff_context_cli::impact_context::index::model::RepositoryGraph,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hex_id(value: usize) -> String {
    format!("{value:064x}")
}

fn repo_path(value: &str) -> RepoPath {
    RepoPath::new(value).expect("static benchmark path must be valid")
}

fn cache_layout(root: &Path) -> CacheLayout {
    let repository_id = hex_id(1);
    let repository_root = root.join("v2").join("repos").join(&repository_id);
    CacheLayout {
        root: root.to_path_buf(),
        repository_id,
        facts_dir: repository_root.join("facts"),
        graphs_dir: repository_root.join("graphs"),
        staging_dir: repository_root.join("staging"),
        locks_dir: repository_root.join("locks"),
        quarantine_dir: repository_root.join("quarantine"),
    }
}

fn file_fact_key(content_sha256: String) -> FileFactKey {
    FileFactKey {
        language: "rust".to_string(),
        content_sha256,
        grammar_version: "tree-sitter-rust@0.24.2".to_string(),
        query_digest: hex_id(301),
        adapter_version: "tree-sitter-rust-index/v1".to_string(),
        normalization_rules_digest: hex_id(302),
        schema_version: 1,
    }
}

fn repository_fixture() -> RepositoryFixture {
    let mut bytes = BTreeMap::new();
    let cargo = b"[package]\nname=\"bench_fixture\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[lib]\npath=\"src/file_00.rs\"\n".to_vec();
    bytes.insert(repo_path("Cargo.toml"), cargo);
    for index in 0..SOURCE_FILES {
        let next = (index + 1) % SOURCE_FILES;
        let source = format!(
            "pub mod nested_{index} {{ pub fn helper() {{}} }}\npub fn function_{index}() {{ crate::function_{next}(); }}\n"
        )
        .into_bytes();
        bytes.insert(repo_path(&format!("src/file_{index:02}.rs")), source);
    }
    let source = BenchSource { bytes };
    let mut entries = source
        .bytes
        .iter()
        .map(|(path, bytes)| RepositoryManifestEntry {
            path: path.clone(),
            mode: "100644".to_string(),
            presence: CandidatePresence::Present,
            content_sha256: Some(digest(bytes)),
            content_bytes: Some(bytes.len()),
            language: path
                .as_str()
                .ends_with(".rs")
                .then(|| "rust".to_string())
                .or_else(|| path.as_str().ends_with(".toml").then(|| "toml".to_string())),
            status: UnitStatus::Completed,
            limitation_codes: Vec::new(),
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest = RepositoryManifest {
        locator: RepositoryLocator {
            source: ReviewSource::Staged,
            object_format: "sha1".to_string(),
            base_tree: Some("1".repeat(40)),
            index_manifest_digest: Some(hex_id(201)),
            overlay_candidate_digest: hex_id(202),
        },
        digest: hex_id(203),
        entries,
        completeness: Completeness::Complete,
        limitations: Vec::new(),
    };
    let mut model_budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
    let project_model = build_rust_project_model(&source, &manifest, &mut model_budget)
        .expect("benchmark project model must build");
    let mut file_facts = Vec::new();
    for (path, bytes) in source
        .bytes
        .iter()
        .filter(|(path, _)| path.as_str().ends_with(".rs"))
    {
        let mut parse_budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
        let facts = TreeSitterRustAdapter::analyze_index(bytes, &mut parse_budget)
            .expect("benchmark Rust source must parse");
        file_facts.push(RustRepositoryFileFacts {
            path: path.clone(),
            key: file_fact_key(digest(bytes)),
            facts,
        });
    }
    file_facts.sort_by(|left, right| left.path.cmp(&right.path));
    let identity = GraphGenerationIdentity {
        graph_schema_version: 1,
        candidate_manifest_digest: manifest.digest.clone(),
        project_model_digest: project_model.digest.clone(),
        resolver_digest: hex_id(401),
        adapter_query_digest: hex_id(402),
        file_facts_manifest_digest: hex_id(403),
        normalization_rules_digest: hex_id(404),
    };
    let mut resolver_budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
    let graph = resolve_rust_repository(
        &manifest,
        &project_model,
        &file_facts,
        identity,
        &mut resolver_budget,
    )
    .expect("benchmark graph must resolve");
    RepositoryFixture {
        source,
        manifest,
        project_model,
        file_facts,
        graph,
    }
}

fn publish_graph(
    layout: CacheLayout,
    graph: &collect_diff_context_cli::impact_context::index::model::RepositoryGraph,
) -> std::path::PathBuf {
    let writer = RepositoryGraphWriter::new(layout);
    let mut budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
    match writer
        .publish(graph, &mut budget)
        .expect("benchmark graph must publish")
    {
        GraphPublishOutcome::Published { path } | GraphPublishOutcome::Reused { path } => path,
    }
}

fn open_graph(
    path: &Path,
    graph: &collect_diff_context_cli::impact_context::index::model::RepositoryGraph,
) -> RepositoryGraphReader {
    match RepositoryGraphReader::open_immutable(
        path,
        &graph.identity,
        ReaderLimits {
            maximum_database_bytes: 256 * 1024 * 1024,
            maximum_rows_per_query: 10_000,
            maximum_string_bytes: 4_096,
        },
    )
    .expect("benchmark graph must open")
    {
        CacheLookup::Hit(reader) => reader,
        other => panic!("benchmark graph unavailable: {other:?}"),
    }
}

struct ScaleGeneration {
    _cache: tempfile::TempDir,
    path: PathBuf,
    identity: GraphGenerationIdentity,
    root_symbol: String,
    symbol_count: usize,
    edge_count: usize,
    items: usize,
}

struct ScaleGraphRowsRoot {
    digest: Sha256,
}

impl ScaleGraphRowsRoot {
    fn new(identity: &str, completeness: &str) -> Self {
        let mut digest = Sha256::new();
        hash_component(&mut digest, b"repository-graph-application-root/v1");
        hash_component(&mut digest, identity.as_bytes());
        hash_component(&mut digest, completeness.as_bytes());
        Self { digest }
    }

    fn start_group(&mut self, row_count: usize) {
        hash_component(&mut self.digest, &(row_count as u64).to_be_bytes());
    }

    fn push_row(&mut self, canonical: &str) {
        hash_component(&mut self.digest, canonical.as_bytes());
    }

    fn finish(self) -> String {
        format!("{:x}", self.digest.finalize())
    }
}

fn hash_component(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn scale_identity(items: usize) -> GraphGenerationIdentity {
    GraphGenerationIdentity {
        graph_schema_version: 1,
        candidate_manifest_digest: hex_id(70_000usize.wrapping_add(items)),
        project_model_digest: hex_id(70_001),
        resolver_digest: hex_id(70_002),
        adapter_query_digest: hex_id(70_003),
        file_facts_manifest_digest: hex_id(70_004),
        normalization_rules_digest: hex_id(70_005),
    }
}

fn scale_file(items: usize) -> GraphFile {
    GraphFile {
        path: repo_path("src/scale.rs"),
        mode: "100644".to_string(),
        presence: CandidatePresence::Present,
        content_sha256: Some(hex_id(80_000usize.wrapping_add(items))),
        file_fact_key: None,
        language: Some("rust".to_string()),
        module_id: Some(hex_id(90_000)),
    }
}

fn scale_module() -> GraphModule {
    GraphModule {
        module_id: hex_id(90_000),
        parent_module_id: None,
        crate_name: "scale".to_string(),
        path: repo_path("src/scale.rs"),
        inline: false,
        root_module: true,
        resolution_status: "resolved".to_string(),
    }
}

fn scale_symbol(index: usize) -> GraphSymbol {
    GraphSymbol {
        symbol_id: hex_id(100_000usize.wrapping_add(index)),
        local_id: format!("s{index}"),
        module_id: hex_id(90_000),
        path: repo_path("src/scale.rs"),
        language: "r".to_string(),
        kind: "f".to_string(),
        name: "f".to_string(),
        owner_symbol_id: None,
        signature: None,
        visibility: None,
        range: SourceRange {
            start_line: 1,
            start_column: 1,
            end_line: 1,
            end_column: 1,
            start_byte: index,
            end_byte: index,
        },
        confidence: Confidence::Medium,
    }
}

fn scale_edge(index: usize, root_symbol: &str) -> GraphEdge {
    let resolved = index.is_multiple_of(2);
    GraphEdge {
        edge_id: hex_id(1_000_000usize.wrapping_add(index)),
        kind: match index % 7 {
            0 => EdgeKind::Calls,
            1 => EdgeKind::References,
            2 => EdgeKind::Imports,
            3 => EdgeKind::Exports,
            4 => EdgeKind::Defines,
            5 => EdgeKind::Implements,
            _ => EdgeKind::Overrides,
        },
        from_symbol: root_symbol.to_string(),
        to_symbol: resolved.then(|| root_symbol.to_string()),
        unresolved_target: (!resolved).then(|| "x".to_string()),
        path: repo_path("src/scale.rs"),
        range: SourceRange {
            start_line: 1,
            start_column: 1,
            end_line: 1,
            end_column: 1,
            start_byte: index,
            end_byte: index,
        },
        provider_id: "s".to_string(),
        provider_version: "v".to_string(),
        resolution: if resolved {
            Resolution::ResolvedReference
        } else {
            Resolution::Unresolved
        },
        confidence: if resolved {
            Confidence::Medium
        } else {
            Confidence::Low
        },
        limitation_code: None,
    }
}

fn scale_counts(items: usize) -> (usize, usize) {
    let symbols = (items / 10).max(1);
    (symbols, items.saturating_sub(symbols))
}

fn scale_graph(items: usize) -> RepositoryGraph {
    let (symbol_count, edge_count) = scale_counts(items);
    let root_symbol = hex_id(100_000);
    RepositoryGraph {
        identity: scale_identity(items),
        files: vec![scale_file(items)],
        modules: vec![scale_module()],
        symbols: (0..symbol_count).map(scale_symbol).collect(),
        edges: (0..edge_count)
            .map(|index| scale_edge(index, &root_symbol))
            .collect(),
        completeness: Completeness::Complete,
        limitations: Vec::new(),
    }
}

fn publish_scale_graph(layout: CacheLayout, graph: &RepositoryGraph) -> PathBuf {
    let writer = RepositoryGraphWriter::new(layout);
    let mut budget = IndexBudget::deep_defaults();
    budget.deadline = Duration::from_secs(10 * 60);
    let mut tracker = IndexBudgetTracker::new(budget);
    match writer
        .publish(graph, &mut tracker)
        .expect("scale graph must publish")
    {
        GraphPublishOutcome::Published { path } | GraphPublishOutcome::Reused { path } => path,
    }
}

fn production_scale_generation(items: usize) -> ScaleGeneration {
    let cache = tempfile::tempdir().expect("create production scale cache");
    let graph = scale_graph(items);
    let (symbol_count, edge_count) = scale_counts(items);
    let identity = graph.identity.clone();
    let root_symbol = graph.symbols[0].symbol_id.clone();
    let path = publish_scale_graph(cache_layout(cache.path()), &graph);
    ScaleGeneration {
        _cache: cache,
        path,
        identity,
        root_symbol,
        symbol_count,
        edge_count,
        items,
    }
}

fn streaming_scale_generation(items: usize) -> ScaleGeneration {
    let cache = tempfile::tempdir().expect("create streaming scale cache");
    let (symbol_count, edge_count) = scale_counts(items);
    let identity = scale_identity(items);
    let root_symbol = hex_id(100_000);
    let seed = RepositoryGraph {
        identity: identity.clone(),
        files: vec![scale_file(items)],
        modules: vec![scale_module()],
        symbols: Vec::new(),
        edges: Vec::new(),
        completeness: Completeness::Complete,
        limitations: Vec::new(),
    };
    let path = publish_scale_graph(cache_layout(cache.path()), &seed);
    append_streaming_scale_rows(&path, items, &root_symbol);
    ScaleGeneration {
        _cache: cache,
        path,
        identity,
        root_symbol,
        symbol_count,
        edge_count,
        items,
    }
}

fn append_streaming_scale_rows(path: &Path, items: usize, root_symbol: &str) {
    let (symbol_count, edge_count) = scale_counts(items);
    let mut connection = Connection::open(path).expect("open streaming scale generation");
    connection
        .pragma_update(None, "journal_mode", "DELETE")
        .expect("configure streaming scale journal");
    connection
        .pragma_update(None, "synchronous", "EXTRA")
        .expect("configure streaming scale sync");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable streaming scale foreign keys");
    connection
        .pragma_update(None, "trusted_schema", false)
        .expect("disable trusted streaming scale schema");
    let (identity_json, completeness): (String, String) = connection
        .query_row(
            "SELECT identity_json, completeness FROM generation_meta",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read streaming scale metadata");
    let file_json: String = connection
        .query_row(
            "SELECT canonical_json FROM files ORDER BY path",
            [],
            |row| row.get(0),
        )
        .expect("read streaming scale file row");
    let module_json: String = connection
        .query_row(
            "SELECT canonical_json FROM modules ORDER BY module_id",
            [],
            |row| row.get(0),
        )
        .expect("read streaming scale module row");
    let mut root = ScaleGraphRowsRoot::new(&identity_json, &completeness);
    root.start_group(1);
    root.push_row(&file_json);
    root.start_group(1);
    root.push_row(&module_json);

    let transaction = connection
        .transaction()
        .expect("start streaming scale transaction");
    root.start_group(symbol_count);
    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO symbols(
                    symbol_id, local_id, module_id, path, language, kind, name,
                    owner_symbol_id, signature, visibility, start_line, start_column,
                    end_line, end_column, start_byte, end_byte, confidence, canonical_json
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                    ?13, ?14, ?15, ?16, ?17, ?18
                )",
            )
            .expect("prepare streaming scale symbols");
        for index in 0..symbol_count {
            let symbol = scale_symbol(index);
            let canonical = serde_json::to_string(&symbol).expect("encode scale symbol");
            statement
                .execute(params![
                    symbol.symbol_id,
                    symbol.local_id,
                    symbol.module_id,
                    symbol.path.as_str(),
                    symbol.language,
                    symbol.kind,
                    symbol.name,
                    symbol.owner_symbol_id,
                    symbol.signature,
                    symbol.visibility,
                    i64::from(symbol.range.start_line),
                    i64::from(symbol.range.start_column),
                    i64::from(symbol.range.end_line),
                    i64::from(symbol.range.end_column),
                    i64::try_from(symbol.range.start_byte).expect("scale symbol byte fits SQLite"),
                    i64::try_from(symbol.range.end_byte).expect("scale symbol byte fits SQLite"),
                    "medium",
                    canonical,
                ])
                .expect("insert streaming scale symbol");
            root.push_row(&canonical);
        }
    }

    root.start_group(edge_count);
    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO edges(
                    edge_id, kind, from_symbol, to_symbol, unresolved_target, path,
                    start_line, start_column, end_line, end_column, start_byte, end_byte,
                    provider_id, provider_version, resolution, confidence, limitation_code,
                    canonical_json
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                    ?13, ?14, ?15, ?16, ?17, ?18
                )",
            )
            .expect("prepare streaming scale edges");
        for index in 0..edge_count {
            let edge = scale_edge(index, root_symbol);
            let canonical = serde_json::to_string(&edge).expect("encode scale edge");
            statement
                .execute(params![
                    edge.edge_id,
                    serde_json::to_value(edge.kind)
                        .expect("encode scale edge kind")
                        .as_str()
                        .expect("edge kind is text"),
                    edge.from_symbol,
                    edge.to_symbol,
                    edge.unresolved_target,
                    edge.path.as_str(),
                    i64::from(edge.range.start_line),
                    i64::from(edge.range.start_column),
                    i64::from(edge.range.end_line),
                    i64::from(edge.range.end_column),
                    i64::try_from(edge.range.start_byte).expect("scale edge byte fits SQLite"),
                    i64::try_from(edge.range.end_byte).expect("scale edge byte fits SQLite"),
                    edge.provider_id,
                    edge.provider_version,
                    serde_json::to_value(edge.resolution)
                        .expect("encode scale resolution")
                        .as_str()
                        .expect("resolution is text"),
                    serde_json::to_value(edge.confidence)
                        .expect("encode scale confidence")
                        .as_str()
                        .expect("confidence is text"),
                    edge.limitation_code,
                    canonical,
                ])
                .expect("insert streaming scale edge");
            root.push_row(&canonical);
        }
    }
    root.start_group(0);
    let application_root = root.finish();
    transaction
        .execute(
            "UPDATE generation_meta
             SET symbol_count = ?1, edge_count = ?2, application_root = ?3",
            params![
                i64::try_from(symbol_count).expect("scale symbol count fits SQLite"),
                i64::try_from(edge_count).expect("scale edge count fits SQLite"),
                application_root,
            ],
        )
        .expect("update streaming scale metadata");
    transaction
        .commit()
        .expect("commit streaming scale generation");
    connection
        .close()
        .expect("close streaming scale generation");
}

fn open_scale_generation(generation: &ScaleGeneration) -> RepositoryGraphReader {
    match RepositoryGraphReader::open_immutable(
        &generation.path,
        &generation.identity,
        ReaderLimits {
            maximum_database_bytes: 2 * 1024 * 1024 * 1024,
            maximum_rows_per_query: 256,
            maximum_string_bytes: 4_096,
        },
    )
    .expect("scale generation must open")
    {
        CacheLookup::Hit(reader) => reader,
        other => panic!("scale generation unavailable: {other:?}"),
    }
}

fn verify_scale_generation(generation: &ScaleGeneration) {
    let connection = Connection::open(&generation.path).expect("open scale counts");
    let symbol_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
        .expect("count scale symbols");
    let edge_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))
        .expect("count scale edges");
    assert_eq!(
        usize::try_from(symbol_count).expect("scale symbol count is non-negative"),
        generation.symbol_count
    );
    assert_eq!(
        usize::try_from(edge_count).expect("scale edge count is non-negative"),
        generation.edge_count
    );
    assert_eq!(
        generation.symbol_count + generation.edge_count,
        generation.items
    );
    drop(connection);
    let reader = open_scale_generation(generation);
    reader
        .integrity_check()
        .expect("scale generation must pass production integrity checks");
    assert!(reader
        .symbol(&generation.root_symbol)
        .expect("query scale root symbol")
        .is_some());
    assert_eq!(
        reader
            .outgoing(&generation.root_symbol, 256)
            .expect("query scale forward edges")
            .len(),
        256
    );
    assert_eq!(
        reader
            .incoming(&generation.root_symbol, 256)
            .expect("query scale reverse edges")
            .len(),
        256
    );
}

fn repository_index_benchmarks(criterion: &mut Criterion) {
    std::env::set_var("PRE_COMMIT_REVIEW_SECRET_SCAN", "off");
    let fixture = repository_fixture();

    criterion.bench_function("manifest/validate", |bencher| {
        bencher.iter(|| {
            fixture.manifest.validate().unwrap();
            black_box(())
        })
    });

    let facts_cache = tempfile::tempdir().unwrap();
    let facts_store =
        FileFactsStore::new(cache_layout(facts_cache.path()), 16 * 1024 * 1024).unwrap();
    let first_fact: &RustRepositoryFileFacts = &fixture.file_facts[0];
    criterion.bench_function("file_facts/miss", |bencher| {
        bencher.iter(|| black_box(facts_store.lookup(black_box(&first_fact.key)).unwrap()))
    });
    facts_store
        .publish(&first_fact.key, &first_fact.facts)
        .expect("publish benchmark file facts");
    criterion.bench_function("file_facts/hit", |bencher| {
        bencher.iter(|| black_box(facts_store.lookup(black_box(&first_fact.key)).unwrap()))
    });

    criterion.bench_function("project_model/build", |bencher| {
        bencher.iter(|| {
            let mut budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
            black_box(
                build_rust_project_model(&fixture.source, &fixture.manifest, &mut budget).unwrap(),
            )
        })
    });

    criterion.bench_function("resolver/resolve", |bencher| {
        bencher.iter(|| {
            let mut budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
            black_box(
                resolve_rust_repository(
                    &fixture.manifest,
                    &fixture.project_model,
                    &fixture.file_facts,
                    fixture.graph.identity.clone(),
                    &mut budget,
                )
                .unwrap(),
            )
        })
    });

    criterion.bench_function("sqlite/cold_build", |bencher| {
        bencher.iter_batched(
            || tempfile::tempdir().unwrap(),
            |cache| {
                black_box(publish_graph(cache_layout(cache.path()), &fixture.graph));
            },
            BatchSize::PerIteration,
        )
    });

    let graph_cache = tempfile::tempdir().unwrap();
    let graph_path = publish_graph(cache_layout(graph_cache.path()), &fixture.graph);
    criterion.bench_function("sqlite/immutable_open", |bencher| {
        bencher.iter(|| black_box(open_graph(&graph_path, &fixture.graph)))
    });
    let reader = open_graph(&graph_path, &fixture.graph);
    let root = fixture.graph.symbols[0].symbol_id.clone();
    criterion.bench_function("query/forward", |bencher| {
        bencher.iter(|| black_box(reader.outgoing(black_box(&root), 10_000).unwrap()))
    });
    criterion.bench_function("query/reverse", |bencher| {
        bencher.iter(|| black_box(reader.incoming(black_box(&root), 10_000).unwrap()))
    });

    let changed_path = fixture.graph.files[0].path.clone();
    let changed_paths = BTreeSet::from([changed_path]);
    criterion.bench_function("overlay/build", |bencher| {
        bencher.iter(|| {
            let mut budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
            black_box(
                build_repository_overlay(&reader, &fixture.graph, &changed_paths, &mut budget)
                    .unwrap(),
            )
        })
    });

    for depth in [1, 2] {
        let request = TraversalRequest {
            roots: vec![root.clone()],
            directions: BTreeSet::from([
                TraversalDirection::Incoming,
                TraversalDirection::Outgoing,
            ]),
            edge_kinds: BTreeSet::from([EdgeKind::Calls, EdgeKind::References]),
            maximum_depth: depth,
            maximum_rows: 10_000,
            maximum_nodes: 10_000,
            maximum_edges: 10_000,
            maximum_bytes: 4 * 1024 * 1024,
            deadline: Duration::from_secs(2),
        };
        criterion.bench_with_input(
            BenchmarkId::new("traversal", format!("{depth}_hop")),
            &request,
            |bencher, request| {
                bencher
                    .iter(|| black_box(traverse_repository_graph(&reader, None, request).unwrap()))
            },
        );
    }

    criterion.bench_function("normalization/canonical_sort", |bencher| {
        bencher.iter(|| {
            let mut graph = fixture.graph.clone();
            graph
                .files
                .sort_by(|left, right| left.path.cmp(&right.path));
            graph
                .modules
                .sort_by(|left, right| left.module_id.cmp(&right.module_id));
            graph
                .symbols
                .sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
            graph
                .edges
                .sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
            black_box(graph)
        })
    });
    let encoded = serde_json::to_string(&fixture.graph).unwrap();
    criterion.bench_function("serialization/repository_graph", |bencher| {
        bencher.iter(|| black_box(serde_json::to_vec(black_box(&fixture.graph)).unwrap()))
    });
    criterion.bench_function("sanitization/repository_graph", |bencher| {
        bencher.iter(|| black_box(sanitize_for_model_optional(black_box(&encoded))))
    });

    let full_scale_gate = std::env::var_os("PRE_COMMIT_REVIEW_SQLITE_SCALE_GATE")
        .as_deref()
        .is_some_and(|value| value == "1");
    let scale_sizes: &[usize] = if full_scale_gate {
        &[10_000, 100_000, 1_000_000]
    } else {
        &[10_000]
    };
    let mut scale = criterion.benchmark_group("scale/sqlite_generation");
    scale.sample_size(10);
    for &items in scale_sizes {
        let generation = if items < 1_000_000 {
            production_scale_generation(items)
        } else {
            streaming_scale_generation(items)
        };
        verify_scale_generation(&generation);
        scale.bench_with_input(
            BenchmarkId::from_parameter(items),
            &generation,
            |bencher, generation| {
                bencher.iter(|| {
                    let reader = open_scale_generation(black_box(generation));
                    let symbol = reader
                        .symbol(black_box(&generation.root_symbol))
                        .expect("query benchmark scale symbol");
                    let outgoing = reader
                        .outgoing(black_box(&generation.root_symbol), 256)
                        .expect("query benchmark scale forward edges");
                    let incoming = reader
                        .incoming(black_box(&generation.root_symbol), 256)
                        .expect("query benchmark scale reverse edges");
                    black_box((symbol, outgoing, incoming))
                })
            },
        );
    }
    scale.finish();
}

criterion_group!(benches, repository_index_benchmarks);
criterion_main!(benches);
