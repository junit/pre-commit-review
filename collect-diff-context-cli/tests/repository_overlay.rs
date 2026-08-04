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
use collect_diff_context_cli::impact_context::index::overlay::{
    build_repository_overlay, RepositoryOverlay,
};
use std::path::PathBuf;

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

fn graph_file(path: &str, content: char, module_id: Option<String>) -> GraphFile {
    GraphFile {
        path: repo_path(path),
        mode: "100644".to_string(),
        presence: CandidatePresence::Present,
        content_sha256: Some(repeated(content)),
        file_fact_key: None,
        language: Some("rust".to_string()),
        module_id,
    }
}

fn graph_module(id: char, parent: Option<char>, path: &str, root: bool) -> GraphModule {
    GraphModule {
        module_id: repeated(id),
        parent_module_id: parent.map(repeated),
        crate_name: "fixture".to_string(),
        path: repo_path(path),
        inline: false,
        root_module: root,
        resolution_status: "resolved".to_string(),
    }
}

fn graph_symbol(id: char, module: char, path: &str, name: &str, line: u32) -> GraphSymbol {
    GraphSymbol {
        symbol_id: repeated(id),
        local_id: format!("{name}-local"),
        module_id: repeated(module),
        path: repo_path(path),
        language: "rust".to_string(),
        kind: "function".to_string(),
        name: name.to_string(),
        owner_symbol_id: None,
        signature: Some(format!("pub fn {name}()")),
        visibility: Some("pub".to_string()),
        range: source_range(line),
        confidence: Confidence::Medium,
    }
}

fn graph_edge(
    id: char,
    kind: EdgeKind,
    from: char,
    to: Option<char>,
    unresolved: Option<&str>,
    path: &str,
    line: u32,
) -> GraphEdge {
    GraphEdge {
        edge_id: repeated(id),
        kind,
        from_symbol: repeated(from),
        to_symbol: to.map(repeated),
        unresolved_target: unresolved.map(str::to_string),
        path: repo_path(path),
        range: source_range(line),
        provider_id: "rust-tree-sitter-resolver".to_string(),
        provider_version: "rust-resolver/v1".to_string(),
        resolution: if to.is_some() {
            Resolution::ResolvedReference
        } else {
            Resolution::Unresolved
        },
        confidence: if to.is_some() {
            Confidence::Medium
        } else {
            Confidence::Low
        },
        limitation_code: unresolved.map(|_| "rust-resolver-call-unresolved".to_string()),
    }
}

fn base_graph() -> RepositoryGraph {
    canonical_graph(RepositoryGraph {
        identity: identity('3'),
        files: vec![
            graph_file("src/api.rs", 'a', Some(repeated('b'))),
            graph_file("src/auth.rs", 'b', Some(repeated('c'))),
            graph_file("src/lib.rs", 'c', Some(repeated('a'))),
        ],
        modules: vec![
            graph_module('a', None, "src/lib.rs", true),
            graph_module('b', Some('a'), "src/api.rs", false),
            graph_module('c', Some('a'), "src/auth.rs", false),
        ],
        symbols: vec![
            graph_symbol('b', 'c', "src/auth.rs", "validate", 1),
            graph_symbol('c', 'c', "src/auth.rs", "helper", 2),
            graph_symbol('d', 'b', "src/api.rs", "login", 1),
        ],
        edges: vec![
            graph_edge('1', EdgeKind::Calls, 'd', Some('b'), None, "src/api.rs", 1),
            graph_edge(
                '2',
                EdgeKind::Imports,
                'd',
                Some('b'),
                None,
                "src/api.rs",
                1,
            ),
            graph_edge('3', EdgeKind::Calls, 'b', Some('c'), None, "src/auth.rs", 1),
        ],
        completeness: Completeness::Complete,
        limitations: Vec::new(),
    })
}

fn replacement_graph(content: char) -> RepositoryGraph {
    let mut graph = base_graph();
    graph.identity = identity(content);
    graph.files[1].content_sha256 = Some(repeated(content));
    graph
        .symbols
        .retain(|symbol| symbol.symbol_id != repeated('b'));
    graph
        .symbols
        .push(graph_symbol('e', 'c', "src/auth.rs", "validate", 1));
    graph.edges = vec![
        graph_edge('4', EdgeKind::Calls, 'd', Some('e'), None, "src/api.rs", 1),
        graph_edge(
            '5',
            EdgeKind::Imports,
            'd',
            Some('e'),
            None,
            "src/api.rs",
            1,
        ),
        graph_edge('6', EdgeKind::Calls, 'e', Some('c'), None, "src/auth.rs", 1),
    ];
    canonical_graph(graph)
}

fn deletion_graph() -> RepositoryGraph {
    let mut graph = base_graph();
    graph.identity = identity('d');
    graph.files[1] = GraphFile {
        path: repo_path("src/auth.rs"),
        mode: "100644".to_string(),
        presence: CandidatePresence::Deleted,
        content_sha256: None,
        file_fact_key: None,
        language: Some("rust".to_string()),
        module_id: None,
    };
    graph
        .modules
        .retain(|module| module.path.as_str() != "src/auth.rs");
    graph
        .symbols
        .retain(|symbol| symbol.path.as_str() != "src/auth.rs");
    graph.edges = vec![graph_edge(
        '7',
        EdgeKind::Calls,
        'd',
        None,
        Some("validate"),
        "src/api.rs",
        1,
    )];
    graph.completeness = Completeness::Partial;
    graph.limitations = vec![IndexLimitation {
        code: "rust-resolver-call-unresolved".to_string(),
        path: Some(repo_path("src/api.rs")),
        symbol_id: Some(repeated('d')),
        reason: "deleted target".to_string(),
        interpretation: "incoming impact remains unresolved".to_string(),
    }];
    canonical_graph(graph)
}

fn addition_graph() -> RepositoryGraph {
    let mut graph = base_graph();
    graph.identity = identity('e');
    graph
        .files
        .push(graph_file("src/new.rs", 'e', Some(repeated('e'))));
    graph
        .modules
        .push(graph_module('e', Some('a'), "src/new.rs", false));
    graph
        .symbols
        .push(graph_symbol('e', 'e', "src/new.rs", "added", 1));
    canonical_graph(graph)
}

fn rename_graph() -> RepositoryGraph {
    let mut graph = deletion_graph();
    graph.identity = identity('f');
    graph
        .files
        .push(graph_file("src/security.rs", 'f', Some(repeated('f'))));
    graph
        .modules
        .push(graph_module('f', Some('a'), "src/security.rs", false));
    graph
        .symbols
        .push(graph_symbol('f', 'f', "src/security.rs", "validate", 1));
    graph.edges = vec![
        graph_edge('8', EdgeKind::Calls, 'd', Some('f'), None, "src/api.rs", 1),
        graph_edge(
            '9',
            EdgeKind::Imports,
            'd',
            Some('f'),
            None,
            "src/api.rs",
            1,
        ),
    ];
    graph.completeness = Completeness::Complete;
    graph.limitations.clear();
    canonical_graph(graph)
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

fn build_overlay(
    candidate: &RepositoryGraph,
    changed: &[&str],
    budget: IndexBudget,
) -> RepositoryOverlay {
    let cache = tempfile::tempdir().unwrap();
    let layout = CacheLayout::resolve(&repository_root(), Some(cache.path())).unwrap();
    let writer = RepositoryGraphWriter::new(layout);
    let base = base_graph();
    let mut writer_budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
    let path = match writer.publish(&base, &mut writer_budget).unwrap() {
        collect_diff_context_cli::impact_context::cache::sqlite_generation::GraphPublishOutcome::Published { path }
        | collect_diff_context_cli::impact_context::cache::sqlite_generation::GraphPublishOutcome::Reused { path } => path,
    };
    let reader = match RepositoryGraphReader::open_immutable(
        &path,
        &base.identity,
        ReaderLimits {
            maximum_database_bytes: 16 * 1024 * 1024,
            maximum_rows_per_query: 1_000,
            maximum_string_bytes: 4_096,
        },
    )
    .unwrap()
    {
        CacheLookup::Hit(reader) => reader,
        _ => panic!("base graph unavailable"),
    };
    let changed = changed.iter().map(|path| repo_path(path)).collect();
    let mut tracker = IndexBudgetTracker::new(budget);
    build_repository_overlay(&reader, candidate, &changed, &mut tracker).unwrap()
}

#[test]
fn changed_path_tombstones_all_base_symbols_and_source_edges() {
    let overlay = build_overlay(
        &replacement_graph('9'),
        &["src/auth.rs"],
        IndexBudget::deep_defaults(),
    );
    assert!(overlay.path_tombstones.contains(&repo_path("src/auth.rs")));
    assert!(!overlay.symbols.contains_key(&repeated('b')));
    assert!(overlay.symbols.contains_key(&repeated('e')));
    assert!(overlay.suppressed_base_edge_ids.contains(&repeated('3')));
}

#[test]
fn addition_replacement_delete_and_rename_use_exact_candidate_facts() {
    let addition = build_overlay(
        &addition_graph(),
        &["src/new.rs"],
        IndexBudget::deep_defaults(),
    );
    assert_eq!(
        addition.files[&repo_path("src/new.rs")].content_sha256,
        Some(repeated('e'))
    );

    let replacement = build_overlay(
        &replacement_graph('9'),
        &["src/auth.rs"],
        IndexBudget::deep_defaults(),
    );
    assert_eq!(
        replacement.files[&repo_path("src/auth.rs")].content_sha256,
        Some(repeated('9'))
    );

    let deletion = build_overlay(
        &deletion_graph(),
        &["src/auth.rs"],
        IndexBudget::deep_defaults(),
    );
    assert!(!deletion.files.contains_key(&repo_path("src/auth.rs")));

    let rename = build_overlay(
        &rename_graph(),
        &["src/auth.rs", "src/security.rs"],
        IndexBudget::deep_defaults(),
    );
    assert!(rename.path_tombstones.contains(&repo_path("src/auth.rs")));
    assert!(rename.files.contains_key(&repo_path("src/security.rs")));
}

#[test]
fn overlay_precedence_is_tombstone_then_replacement_then_base() {
    let overlay = build_overlay(
        &replacement_graph('9'),
        &["src/auth.rs"],
        IndexBudget::deep_defaults(),
    );
    assert!(overlay.suppressed_base_edge_ids.contains(&repeated('2')));
    assert!(overlay
        .outgoing_edges
        .get(&repeated('d'))
        .unwrap()
        .iter()
        .any(|edge| edge.to_symbol.as_deref() == Some(repeated('e').as_str())));
}

#[test]
fn public_symbol_and_import_change_refresh_known_reverse_dependents() {
    let overlay = build_overlay(
        &replacement_graph('9'),
        &["src/auth.rs"],
        IndexBudget::deep_defaults(),
    );
    assert!(overlay.path_tombstones.contains(&repo_path("src/api.rs")));
    assert!(overlay.suppressed_base_edge_ids.contains(&repeated('1')));
    assert!(overlay.suppressed_base_edge_ids.contains(&repeated('2')));
}

#[test]
fn glob_macro_cfg_and_budget_limits_mark_closure_partial() {
    let mut candidate = replacement_graph('9');
    candidate.completeness = Completeness::Partial;
    candidate.limitations = vec![IndexLimitation {
        code: "rust-resolver-glob-import-ambiguous".to_string(),
        path: Some(repo_path("src/api.rs")),
        symbol_id: Some(repeated('d')),
        reason: "glob import".to_string(),
        interpretation: "closure is partial".to_string(),
    }];
    let mut budget = IndexBudget::deep_defaults();
    budget.max_overlay_paths = 1;
    let overlay = build_overlay(&candidate, &["src/auth.rs"], budget);
    assert_eq!(overlay.completeness, Completeness::Partial);
    assert!(overlay.limitations.iter().any(|limitation| {
        limitation.code == "rust-resolver-glob-import-ambiguous"
            || limitation.code == "index-overlay-path-budget-exhausted"
    }));

    let mut byte_budget = IndexBudget::deep_defaults();
    byte_budget.max_generation_bytes = 1;
    let byte_limited = build_overlay(&replacement_graph('9'), &["src/auth.rs"], byte_budget);
    assert_eq!(byte_limited.completeness, Completeness::Partial);
    assert!(byte_limited
        .limitations
        .iter()
        .any(|limitation| limitation.code == "index-generation-byte-budget-exhausted"));
}

#[test]
fn incoming_edges_to_deleted_symbols_remain_visible_as_unresolved_impact() {
    let overlay = build_overlay(
        &deletion_graph(),
        &["src/auth.rs"],
        IndexBudget::deep_defaults(),
    );
    assert!(overlay
        .incoming_edges
        .get(&repeated('b'))
        .unwrap()
        .iter()
        .any(|edge| {
            edge.to_symbol.is_none()
                && edge.unresolved_target.as_deref() == Some(repeated('b').as_str())
        }));
}

#[test]
fn staged_overlay_uses_stage_zero_bytes_not_worktree_bytes() {
    let overlay = build_overlay(
        &replacement_graph('9'),
        &["src/auth.rs"],
        IndexBudget::deep_defaults(),
    );
    assert_eq!(
        overlay.files[&repo_path("src/auth.rs")].content_sha256,
        Some(repeated('9'))
    );
    assert_ne!(
        overlay.files[&repo_path("src/auth.rs")].content_sha256,
        Some(repeated('a'))
    );
}

#[test]
fn unstaged_overlay_binds_exact_index_base_and_tracked_worktree_delta() {
    let candidate = replacement_graph('9');
    let overlay = build_overlay(&candidate, &["src/auth.rs"], IndexBudget::deep_defaults());
    assert_eq!(
        overlay.base_generation_key,
        base_graph().identity.generation_key().unwrap()
    );
    assert_eq!(
        overlay.candidate_manifest_digest,
        candidate.identity.candidate_manifest_digest
    );
}

#[test]
fn overlay_output_is_deterministic() {
    let candidate = replacement_graph('9');
    let first = build_overlay(&candidate, &["src/auth.rs"], IndexBudget::deep_defaults());
    let second = build_overlay(&candidate, &["src/auth.rs"], IndexBudget::deep_defaults());
    assert_eq!(first, second);
}
