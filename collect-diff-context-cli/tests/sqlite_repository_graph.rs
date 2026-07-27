use collect_diff_context_cli::candidate::{CandidatePresence, RepoPath};
use collect_diff_context_cli::impact_context::cache::file_facts::CacheLayout;
use collect_diff_context_cli::impact_context::cache::sqlite_generation::{
    GraphPublishOutcome, RepositoryGraphWriter,
};
use collect_diff_context_cli::impact_context::contracts::{
    Completeness, Confidence, EdgeKind, Resolution, SourceRange,
};
use collect_diff_context_cli::impact_context::index::budget::{IndexBudget, IndexBudgetTracker};
use collect_diff_context_cli::impact_context::index::model::{
    FileFactKey, GraphEdge, GraphFile, GraphGenerationIdentity, GraphModule, GraphSymbol,
    IndexLimitation, RepositoryGraph,
};
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

const APPLICATION_ID: i32 = 0x5052_4349;

fn repeated(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn layout(cache: &Path) -> CacheLayout {
    CacheLayout::resolve(&repository_root(), Some(cache)).unwrap()
}

fn file_key() -> FileFactKey {
    FileFactKey {
        language: "rust".to_string(),
        content_sha256: repeated('1'),
        grammar_version: "tree-sitter-rust@0.24.0".to_string(),
        query_digest: repeated('2'),
        adapter_version: "tree-sitter-rust-index/v1".to_string(),
        normalization_rules_digest: repeated('3'),
        schema_version: 1,
    }
}

fn range(line: u32, start_byte: usize) -> SourceRange {
    SourceRange {
        start_line: line,
        start_column: 1,
        end_line: line,
        end_column: 8,
        start_byte,
        end_byte: start_byte + 7,
    }
}

fn graph() -> RepositoryGraph {
    let module_id = repeated('a');
    let first = repeated('b');
    let second = repeated('c');
    let third = repeated('d');
    let path = RepoPath::new("src/lib.rs").unwrap();
    let mut graph = RepositoryGraph {
        identity: GraphGenerationIdentity {
            graph_schema_version: 1,
            candidate_manifest_digest: repeated('4'),
            project_model_digest: repeated('5'),
            resolver_digest: repeated('6'),
            adapter_query_digest: repeated('7'),
            file_facts_manifest_digest: repeated('8'),
            normalization_rules_digest: repeated('9'),
        },
        files: vec![GraphFile {
            path: path.clone(),
            mode: "100644".to_string(),
            presence: CandidatePresence::Present,
            content_sha256: Some(repeated('1')),
            file_fact_key: Some(file_key()),
            language: Some("rust".to_string()),
            module_id: Some(module_id.clone()),
        }],
        modules: vec![GraphModule {
            module_id: module_id.clone(),
            parent_module_id: None,
            crate_name: "fixture".to_string(),
            path: path.clone(),
            inline: false,
            root_module: true,
            resolution_status: "resolved".to_string(),
        }],
        symbols: vec![
            GraphSymbol {
                symbol_id: first.clone(),
                local_id: "first-local".to_string(),
                module_id: module_id.clone(),
                path: path.clone(),
                language: "rust".to_string(),
                kind: "function".to_string(),
                name: "first".to_string(),
                owner_symbol_id: None,
                signature: Some("pub fn first()".to_string()),
                visibility: Some("pub".to_string()),
                range: range(1, 0),
                confidence: Confidence::Medium,
            },
            GraphSymbol {
                symbol_id: second.clone(),
                local_id: "second-local".to_string(),
                module_id: module_id.clone(),
                path: path.clone(),
                language: "rust".to_string(),
                kind: "function".to_string(),
                name: "second".to_string(),
                owner_symbol_id: None,
                signature: Some("pub fn second()".to_string()),
                visibility: Some("pub".to_string()),
                range: range(2, 8),
                confidence: Confidence::Medium,
            },
            GraphSymbol {
                symbol_id: third.clone(),
                local_id: "third-local".to_string(),
                module_id,
                path: path.clone(),
                language: "rust".to_string(),
                kind: "function".to_string(),
                name: "third".to_string(),
                owner_symbol_id: None,
                signature: Some("pub fn third()".to_string()),
                visibility: Some("pub".to_string()),
                range: range(3, 16),
                confidence: Confidence::Medium,
            },
        ],
        edges: vec![
            GraphEdge {
                edge_id: repeated('0'),
                kind: EdgeKind::Calls,
                from_symbol: first.clone(),
                to_symbol: Some(second.clone()),
                unresolved_target: None,
                path: path.clone(),
                range: range(1, 0),
                provider_id: "rust-tree-sitter-resolver".to_string(),
                provider_version: "rust-resolver/v1".to_string(),
                resolution: Resolution::ResolvedReference,
                confidence: Confidence::Medium,
                limitation_code: None,
            },
            GraphEdge {
                edge_id: repeated('1'),
                kind: EdgeKind::References,
                from_symbol: second.clone(),
                to_symbol: Some(third.clone()),
                unresolved_target: None,
                path: path.clone(),
                range: range(2, 8),
                provider_id: "rust-tree-sitter-resolver".to_string(),
                provider_version: "rust-resolver/v1".to_string(),
                resolution: Resolution::ResolvedReference,
                confidence: Confidence::Medium,
                limitation_code: None,
            },
            GraphEdge {
                edge_id: repeated('2'),
                kind: EdgeKind::Calls,
                from_symbol: third,
                to_symbol: Some(first),
                unresolved_target: None,
                path,
                range: range(3, 16),
                provider_id: "rust-tree-sitter-resolver".to_string(),
                provider_version: "rust-resolver/v1".to_string(),
                resolution: Resolution::ResolvedReference,
                confidence: Confidence::Medium,
                limitation_code: None,
            },
        ],
        completeness: Completeness::Complete,
        limitations: Vec::new(),
    };
    graph
        .symbols
        .sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
    graph
        .edges
        .sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
    graph
}

fn publish(writer: &RepositoryGraphWriter, graph: &RepositoryGraph) -> GraphPublishOutcome {
    let mut budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
    writer.publish(graph, &mut budget).unwrap()
}

fn outcome_path(outcome: &GraphPublishOutcome) -> &Path {
    match outcome {
        GraphPublishOutcome::Published { path } | GraphPublishOutcome::Reused { path } => path,
    }
}

fn open_database(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

#[test]
fn writer_creates_fixed_schema_and_digest_named_generation() {
    let cache = tempfile::tempdir().unwrap();
    let writer = RepositoryGraphWriter::new(layout(cache.path()));
    let graph = graph();
    let outcome = publish(&writer, &graph);
    let path = outcome_path(&outcome);
    let generation_key = graph.identity.generation_key().unwrap();

    assert!(matches!(outcome, GraphPublishOutcome::Published { .. }));
    assert_eq!(
        path.file_name().unwrap().to_string_lossy(),
        format!("{generation_key}.sqlite")
    );
    let connection = open_database(path);
    let application_id: i32 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .unwrap();
    let user_version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(application_id, APPLICATION_ID);
    assert_eq!(user_version, 1);
    let tables = connection
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        tables,
        [
            "edges",
            "files",
            "generation_meta",
            "limitations",
            "modules",
            "symbols"
        ]
    );
}

#[test]
fn writer_persists_outgoing_and_incoming_indexes() {
    let cache = tempfile::tempdir().unwrap();
    let writer = RepositoryGraphWriter::new(layout(cache.path()));
    let path = outcome_path(&publish(&writer, &graph())).to_path_buf();
    let connection = open_database(&path);
    let indexes = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'index' AND tbl_name = 'edges' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<BTreeSet<_>, _>>()
        .unwrap();
    for expected in ["edges_from_kind_id", "edges_path_id", "edges_to_kind_id"] {
        assert!(
            indexes.contains(expected),
            "missing {expected}: {indexes:?}"
        );
    }
}

#[test]
fn writer_validates_foreign_keys_counts_root_and_integrity() {
    let cache = tempfile::tempdir().unwrap();
    let writer = RepositoryGraphWriter::new(layout(cache.path()));
    let graph = graph();
    let path = outcome_path(&publish(&writer, &graph)).to_path_buf();
    let connection = open_database(&path);

    assert!(connection
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query([])
        .unwrap()
        .next()
        .unwrap()
        .is_none());
    let integrity: String = connection
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    let (files, modules, symbols, edges, limitations, root): (
        i64,
        i64,
        i64,
        i64,
        i64,
        String,
    ) = connection
        .query_row(
            "SELECT file_count, module_count, symbol_count, edge_count, limitation_count, application_root FROM generation_meta",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .unwrap();
    assert_eq!(files, graph.files.len() as i64);
    assert_eq!(modules, graph.modules.len() as i64);
    assert_eq!(symbols, graph.symbols.len() as i64);
    assert_eq!(edges, graph.edges.len() as i64);
    assert_eq!(limitations, 0);
    assert_eq!(root.len(), 64);
    assert!(root
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
}

#[test]
fn same_key_writers_converge_on_one_generation() {
    let cache = tempfile::tempdir().unwrap();
    let writer = Arc::new(RepositoryGraphWriter::new(layout(cache.path())));
    let graph = Arc::new(graph());
    let barrier = Arc::new(Barrier::new(8));
    let handles = (0..8)
        .map(|_| {
            let writer = Arc::clone(&writer);
            let graph = Arc::clone(&graph);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                publish(&writer, &graph)
            })
        })
        .collect::<Vec<_>>();
    let outcomes = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, GraphPublishOutcome::Published { .. }))
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_dir(&writer.layout().graphs_dir)
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "sqlite"))
            .count(),
        1
    );
}

#[test]
fn different_generation_writer_does_not_block_immutable_reader() {
    let cache = tempfile::tempdir().unwrap();
    let writer = RepositoryGraphWriter::new(layout(cache.path()));
    let first = graph();
    let first_path = outcome_path(&publish(&writer, &first)).to_path_buf();
    let reader = open_database(&first_path);
    let mut second = graph();
    second.identity.candidate_manifest_digest = repeated('e');
    let writer_thread = writer.clone();
    let handle = std::thread::spawn(move || publish(&writer_thread, &second));

    let started = Instant::now();
    let count: i64 = reader
        .query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, first.edges.len() as i64);
    assert!(started.elapsed() < Duration::from_secs(1));
    handle.join().unwrap();
}

#[test]
fn interrupted_writer_never_publishes_a_partial_generation() {
    let cache = tempfile::tempdir().unwrap();
    let writer = RepositoryGraphWriter::new(layout(cache.path()));
    let graph = graph();
    let path = writer.generation_path(&graph.identity).unwrap();
    let mut limits = IndexBudget::deep_defaults();
    limits.max_generation_bytes = 1;
    let mut budget = IndexBudgetTracker::new(limits);

    let error = writer.publish(&graph, &mut budget).unwrap_err();
    assert_eq!(error.code, "index-generation-byte-budget-exhausted");
    assert!(!path.exists());
    if writer.layout().staging_dir.exists() {
        assert_eq!(
            std::fs::read_dir(&writer.layout().staging_dir)
                .unwrap()
                .count(),
            0
        );
    }
}

#[test]
fn partial_generation_requires_complete_manifest_and_explicit_omissions() {
    let cache = tempfile::tempdir().unwrap();
    let writer = RepositoryGraphWriter::new(layout(cache.path()));
    let mut graph = graph();
    graph.completeness = Completeness::Partial;
    let mut budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
    let error = writer.publish(&graph, &mut budget).unwrap_err();
    assert_eq!(error.code, "partial-generation-omissions-required");

    graph.limitations.push(IndexLimitation {
        code: "rust-resolver-call-unresolved".to_string(),
        path: Some(RepoPath::new("src/lib.rs").unwrap()),
        symbol_id: None,
        reason: "call target is unavailable".to_string(),
        interpretation: "the generation is intentionally partial".to_string(),
    });
    graph.limitations.push(IndexLimitation {
        code: "rust-resolver-method-call-unresolved".to_string(),
        path: Some(RepoPath::new("src/lib.rs").unwrap()),
        symbol_id: Some(repeated('b')),
        reason: "method target is unavailable".to_string(),
        interpretation: "the generation is intentionally partial".to_string(),
    });
    publish(&writer, &graph);
}

#[test]
fn invalid_existing_generation_is_not_overwritten() {
    let cache = tempfile::tempdir().unwrap();
    let writer = RepositoryGraphWriter::new(layout(cache.path()));
    let graph = graph();
    let path = writer.generation_path(&graph.identity).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"not sqlite").unwrap();
    let mut budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());

    let error = writer.publish(&graph, &mut budget).unwrap_err();
    assert_eq!(error.code, "invalid-existing-generation");
    assert_eq!(std::fs::read(path).unwrap(), b"not sqlite");
}
