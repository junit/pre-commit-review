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
use collect_diff_context_cli::impact_context::contracts::{Completeness, EdgeKind, UnitStatus};
use collect_diff_context_cli::impact_context::index::budget::{IndexBudget, IndexBudgetTracker};
use collect_diff_context_cli::impact_context::index::model::{
    FileFactKey, GraphGenerationIdentity, RepositoryLocator, RepositoryManifest,
    RepositoryManifestEntry,
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
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
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

fn scale_row_stream(items: usize) -> [u8; 32] {
    let mut digest = Sha256::new();
    for index in 0..items {
        digest.update((index as u64).to_be_bytes());
        digest.update(((index + 1) % items.max(1)).to_be_bytes());
    }
    digest.finalize().into()
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

    let mut scale = criterion.benchmark_group("scale/symbol_edge_row_stream");
    for items in [10_000, 100_000, 1_000_000] {
        scale.bench_with_input(
            BenchmarkId::from_parameter(items),
            &items,
            |bencher, items| bencher.iter(|| black_box(scale_row_stream(black_box(*items)))),
        );
    }
    scale.finish();
}

criterion_group!(benches, repository_index_benchmarks);
criterion_main!(benches);
