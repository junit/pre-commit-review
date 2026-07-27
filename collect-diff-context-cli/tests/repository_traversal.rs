use collect_diff_context_cli::candidate::{CandidatePresence, RepoPath};
use collect_diff_context_cli::impact_context::cache::file_facts::{CacheLayout, CacheLookup};
use collect_diff_context_cli::impact_context::cache::sqlite_generation::{
    ReaderLimits, RepositoryGraphReader, RepositoryGraphWriter,
};
use collect_diff_context_cli::impact_context::contracts::{
    Completeness, Confidence, EdgeKind, Resolution, SourceRange,
};
use collect_diff_context_cli::impact_context::index::budget::{IndexBudget, IndexBudgetTracker};
use collect_diff_context_cli::impact_context::index::model::{
    GraphEdge, GraphFile, GraphGenerationIdentity, GraphModule, GraphSymbol, IndexLimitation,
    RepositoryGraph,
};
use collect_diff_context_cli::impact_context::index::overlay::RepositoryOverlay;
use collect_diff_context_cli::impact_context::index::traversal::{
    traverse_repository_graph, TraversalDirection, TraversalRequest,
};
use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn repeated(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn repo_path(value: &str) -> RepoPath {
    RepoPath::new(value).unwrap()
}

fn source_range(line: u32) -> SourceRange {
    SourceRange {
        start_line: line,
        start_column: 1,
        end_line: line,
        end_column: 8,
        start_byte: (line as usize - 1) * 8,
        end_byte: line as usize * 8 - 1,
    }
}

fn identity(candidate: char) -> GraphGenerationIdentity {
    GraphGenerationIdentity {
        graph_schema_version: 1,
        candidate_manifest_digest: repeated(candidate),
        project_model_digest: repeated('4'),
        resolver_digest: repeated('5'),
        adapter_query_digest: repeated('6'),
        file_facts_manifest_digest: repeated('7'),
        normalization_rules_digest: repeated('8'),
    }
}

fn graph_file(id: char, path: &str) -> GraphFile {
    GraphFile {
        path: repo_path(path),
        mode: "100644".to_string(),
        presence: CandidatePresence::Present,
        content_sha256: Some(repeated(id)),
        file_fact_key: None,
        language: Some("rust".to_string()),
        module_id: Some(repeated(id)),
    }
}

fn graph_module(id: char, path: &str) -> GraphModule {
    GraphModule {
        module_id: repeated(id),
        parent_module_id: None,
        crate_name: "fixture".to_string(),
        path: repo_path(path),
        inline: false,
        root_module: true,
        resolution_status: "resolved".to_string(),
    }
}

fn graph_symbol(id: char, path: &str, name: &str) -> GraphSymbol {
    GraphSymbol {
        symbol_id: repeated(id),
        local_id: format!("{name}-local"),
        module_id: repeated(id),
        path: repo_path(path),
        language: "rust".to_string(),
        kind: "function".to_string(),
        name: name.to_string(),
        owner_symbol_id: None,
        signature: Some(format!("pub fn {name}()")),
        visibility: Some("pub".to_string()),
        range: source_range(1),
        confidence: Confidence::Medium,
    }
}

fn graph_edge(id: char, kind: EdgeKind, from: char, to: char, path: &str) -> GraphEdge {
    GraphEdge {
        edge_id: repeated(id),
        kind,
        from_symbol: repeated(from),
        to_symbol: Some(repeated(to)),
        unresolved_target: None,
        path: repo_path(path),
        range: source_range(1),
        provider_id: "rust-tree-sitter-resolver".to_string(),
        provider_version: "rust-resolver/v1".to_string(),
        resolution: Resolution::ResolvedReference,
        confidence: Confidence::Medium,
        limitation_code: None,
    }
}

fn graph() -> RepositoryGraph {
    canonical_graph(RepositoryGraph {
        identity: identity('3'),
        files: vec![
            graph_file('a', "src/a.rs"),
            graph_file('b', "src/b.rs"),
            graph_file('c', "src/c.rs"),
            graph_file('d', "src/d.rs"),
        ],
        modules: vec![
            graph_module('a', "src/a.rs"),
            graph_module('b', "src/b.rs"),
            graph_module('c', "src/c.rs"),
            graph_module('d', "src/d.rs"),
        ],
        symbols: vec![
            graph_symbol('a', "src/a.rs", "alpha"),
            graph_symbol('b', "src/b.rs", "beta"),
            graph_symbol('c', "src/c.rs", "gamma"),
            graph_symbol('d', "src/d.rs", "delta"),
        ],
        edges: vec![
            graph_edge('1', EdgeKind::Calls, 'a', 'b', "src/a.rs"),
            graph_edge('2', EdgeKind::Calls, 'b', 'c', "src/b.rs"),
            graph_edge('3', EdgeKind::Calls, 'c', 'a', "src/c.rs"),
            graph_edge('4', EdgeKind::References, 'd', 'b', "src/d.rs"),
            graph_edge('5', EdgeKind::References, 'b', 'a', "src/b.rs"),
        ],
        completeness: Completeness::Complete,
        limitations: Vec::new(),
    })
}

fn canonical_graph(mut graph: RepositoryGraph) -> RepositoryGraph {
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
    graph
        .limitations
        .sort_by(|left, right| left.code.cmp(&right.code));
    graph
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

struct StoredGraph {
    _cache: tempfile::TempDir,
    graph: RepositoryGraph,
    reader: RepositoryGraphReader,
}

fn store_graph(graph: RepositoryGraph) -> StoredGraph {
    let cache = tempfile::tempdir().unwrap();
    let layout = CacheLayout::resolve(&repository_root(), Some(cache.path())).unwrap();
    let writer = RepositoryGraphWriter::new(layout);
    let mut budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
    let path = match writer.publish(&graph, &mut budget).unwrap() {
        collect_diff_context_cli::impact_context::cache::sqlite_generation::GraphPublishOutcome::Published { path }
        | collect_diff_context_cli::impact_context::cache::sqlite_generation::GraphPublishOutcome::Reused { path } => path,
    };
    let reader = open_reader(&path, &graph.identity);
    StoredGraph {
        _cache: cache,
        graph,
        reader,
    }
}

fn open_reader(path: &Path, identity: &GraphGenerationIdentity) -> RepositoryGraphReader {
    match RepositoryGraphReader::open_immutable(
        path,
        identity,
        ReaderLimits {
            maximum_database_bytes: 16 * 1024 * 1024,
            maximum_rows_per_query: 100,
            maximum_string_bytes: 4_096,
        },
    )
    .unwrap()
    {
        CacheLookup::Hit(reader) => reader,
        other => panic!("graph reader unavailable: {other:?}"),
    }
}

fn request(root: char) -> TraversalRequest {
    TraversalRequest {
        roots: vec![repeated(root)],
        directions: BTreeSet::from([TraversalDirection::Incoming, TraversalDirection::Outgoing]),
        edge_kinds: BTreeSet::from([EdgeKind::Calls, EdgeKind::References]),
        maximum_depth: 1,
        maximum_rows: 100,
        maximum_nodes: 100,
        maximum_edges: 100,
        maximum_bytes: 1024 * 1024,
        deadline: Duration::from_secs(1),
    }
}

fn outgoing_request(root: char, depth: usize) -> TraversalRequest {
    let mut request = request(root);
    request.directions = BTreeSet::from([TraversalDirection::Outgoing]);
    request.maximum_depth = depth;
    request
}

fn edge_ids(edges: &[GraphEdge]) -> Vec<String> {
    edges.iter().map(|edge| edge.edge_id.clone()).collect()
}

fn limitation_codes(limitations: &[IndexLimitation]) -> BTreeSet<String> {
    limitations
        .iter()
        .map(|limitation| limitation.code.clone())
        .collect()
}

fn replacement_overlay(stored: &StoredGraph) -> RepositoryOverlay {
    let replacement = graph_edge('6', EdgeKind::Calls, 'a', 'd', "src/a.rs");
    RepositoryOverlay {
        base_generation_key: stored.graph.identity.generation_key().unwrap(),
        candidate_manifest_digest: repeated('f'),
        path_tombstones: BTreeSet::from([repo_path("src/a.rs")]),
        files: BTreeMap::from([(repo_path("src/a.rs"), graph_file('a', "src/a.rs"))]),
        modules: BTreeMap::from([(repeated('a'), graph_module('a', "src/a.rs"))]),
        symbols: BTreeMap::from([(repeated('a'), graph_symbol('a', "src/a.rs", "alpha"))]),
        outgoing_edges: BTreeMap::from([(repeated('a'), vec![replacement.clone()])]),
        incoming_edges: BTreeMap::from([(repeated('d'), vec![replacement])]),
        suppressed_base_edge_ids: BTreeSet::from([repeated('1')]),
        completeness: Completeness::Complete,
        limitations: Vec::new(),
    }
}

#[test]
fn one_hop_returns_sorted_incoming_and_outgoing_edges() {
    let stored = store_graph(graph());
    let result = traverse_repository_graph(&stored.reader, None, &request('b')).unwrap();

    assert_eq!(
        edge_ids(&result.edges),
        vec![repeated('1'), repeated('2'), repeated('4'), repeated('5')]
    );
    assert_eq!(result.reached_depth, 1);
    assert_eq!(result.rows_read, 4);
}

#[test]
fn two_hop_breadth_first_traversal_deduplicates_cycles() {
    let stored = store_graph(graph());
    let result =
        traverse_repository_graph(&stored.reader, None, &outgoing_request('a', 2)).unwrap();

    assert_eq!(
        edge_ids(&result.edges),
        vec![repeated('1'), repeated('2'), repeated('5')]
    );
    assert_eq!(result.reached_depth, 2);
    assert_eq!(result.nodes_visited, 3);
}

#[test]
fn overlay_tombstones_and_replacements_override_base_rows() {
    let stored = store_graph(graph());
    let mut overlay = replacement_overlay(&stored);
    let mut low_confidence = overlay.outgoing_edges[&repeated('a')][0].clone();
    low_confidence.confidence = Confidence::Low;
    let mut high_confidence = low_confidence.clone();
    high_confidence.confidence = Confidence::High;
    overlay
        .outgoing_edges
        .insert(repeated('a'), vec![low_confidence, high_confidence]);
    let result =
        traverse_repository_graph(&stored.reader, Some(&overlay), &outgoing_request('a', 1))
            .unwrap();

    assert_eq!(edge_ids(&result.edges), vec![repeated('6')]);
    assert_eq!(result.edges[0].confidence, Confidence::High);
    assert!(!result
        .edges
        .iter()
        .any(|edge| edge.edge_id == repeated('1')));
}

#[test]
fn row_node_edge_byte_depth_and_deadline_budgets_return_partial() {
    let stored = store_graph(graph());

    let mut row_limited = request('b');
    row_limited.maximum_rows = 1;
    let rows = traverse_repository_graph(&stored.reader, None, &row_limited).unwrap();
    assert_eq!(rows.query_completeness, Completeness::Partial);
    assert!(limitation_codes(&rows.limitations).contains("index-query-row-budget-exhausted"));

    let mut node_limited = outgoing_request('a', 3);
    node_limited.maximum_nodes = 1;
    let nodes = traverse_repository_graph(&stored.reader, None, &node_limited).unwrap();
    assert_eq!(nodes.query_completeness, Completeness::Partial);
    assert!(limitation_codes(&nodes.limitations).contains("index-node-budget-exhausted"));

    let mut edge_limited = outgoing_request('a', 3);
    edge_limited.maximum_edges = 0;
    let edges = traverse_repository_graph(&stored.reader, None, &edge_limited).unwrap();
    assert!(edges.output_truncated);
    assert!(limitation_codes(&edges.limitations).contains("index-edge-budget-exhausted"));

    let mut byte_limited = outgoing_request('a', 3);
    byte_limited.maximum_bytes = 1;
    let bytes = traverse_repository_graph(&stored.reader, None, &byte_limited).unwrap();
    assert!(bytes.output_truncated);
    assert!(limitation_codes(&bytes.limitations).contains("index-output-byte-budget-exhausted"));

    let depth = traverse_repository_graph(&stored.reader, None, &outgoing_request('a', 1)).unwrap();
    assert_eq!(depth.query_completeness, Completeness::Partial);
    assert!(limitation_codes(&depth.limitations).contains("index-graph-depth-budget-exhausted"));

    let mut deadline_limited = outgoing_request('a', 3);
    deadline_limited.deadline = Duration::ZERO;
    let deadline = traverse_repository_graph(&stored.reader, None, &deadline_limited).unwrap();
    assert_eq!(deadline.query_completeness, Completeness::Partial);
    assert!(limitation_codes(&deadline.limitations).contains("index-deadline-exhausted"));
}

#[test]
fn corrupt_row_invalidates_query_without_accepting_other_edges() {
    let cache = tempfile::tempdir().unwrap();
    let layout = CacheLayout::resolve(&repository_root(), Some(cache.path())).unwrap();
    let writer = RepositoryGraphWriter::new(layout);
    let graph = graph();
    let mut budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
    let path = match writer.publish(&graph, &mut budget).unwrap() {
        collect_diff_context_cli::impact_context::cache::sqlite_generation::GraphPublishOutcome::Published { path }
        | collect_diff_context_cli::impact_context::cache::sqlite_generation::GraphPublishOutcome::Reused { path } => path,
    };
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE edges SET kind = 'invalid-kind' WHERE edge_id = ?1",
            [repeated('1')],
        )
        .unwrap();
    drop(connection);
    let reader = open_reader(&path, &graph.identity);

    let error = traverse_repository_graph(&reader, None, &outgoing_request('a', 1)).unwrap_err();
    assert_eq!(error.code, "generation-row-corrupt");
}

#[test]
fn index_completeness_query_completeness_and_output_truncation_are_independent() {
    let mut partial = graph();
    partial.identity = identity('9');
    partial.completeness = Completeness::Partial;
    partial.limitations = vec![IndexLimitation {
        code: "fixture-index-partial".to_string(),
        path: Some(repo_path("src/a.rs")),
        symbol_id: Some(repeated('a')),
        reason: "fixture omits an external relationship".to_string(),
        interpretation: "the stored index is partial".to_string(),
    }];
    let partial = store_graph(partial);
    let no_edges = traverse_repository_graph(&partial.reader, None, &request('f')).unwrap();
    assert_eq!(no_edges.index_completeness, Completeness::Partial);
    assert_eq!(no_edges.query_completeness, Completeness::Complete);
    assert!(!no_edges.output_truncated);

    let complete = store_graph(graph());
    let mut deadline_request = outgoing_request('a', 3);
    deadline_request.deadline = Duration::ZERO;
    let deadline = traverse_repository_graph(&complete.reader, None, &deadline_request).unwrap();
    assert_eq!(deadline.index_completeness, Completeness::Complete);
    assert_eq!(deadline.query_completeness, Completeness::Partial);
    assert!(!deadline.output_truncated);

    let mut output_request = outgoing_request('a', 3);
    output_request.maximum_edges = 0;
    let output = traverse_repository_graph(&complete.reader, None, &output_request).unwrap();
    assert_eq!(output.index_completeness, Completeness::Complete);
    assert_eq!(output.query_completeness, Completeness::Complete);
    assert!(output.output_truncated);
}

#[test]
fn repeated_queries_are_deterministic_except_elapsed_metrics() {
    let stored = store_graph(graph());
    let request = request('b');
    let mut first = traverse_repository_graph(&stored.reader, None, &request).unwrap();
    let mut second = traverse_repository_graph(&stored.reader, None, &request).unwrap();
    first.elapsed_ms = 0;
    second.elapsed_ms = 0;
    assert_eq!(first, second);
}
