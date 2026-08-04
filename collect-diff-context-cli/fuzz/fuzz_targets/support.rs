#![allow(dead_code)]

use collect_diff_context_cli::candidate::{CandidatePresence, RepoPath};
use collect_diff_context_cli::impact_context::cache::file_facts::{CacheLayout, CacheLookup};
use collect_diff_context_cli::impact_context::cache::sqlite_generation::{
    GraphPublishOutcome, ReaderLimits, RepositoryGraphReader, RepositoryGraphWriter,
};
use collect_diff_context_cli::impact_context::contracts::{
    Completeness, Confidence, EdgeKind, Resolution, SourceRange,
};
use collect_diff_context_cli::impact_context::index::budget::{IndexBudget, IndexBudgetTracker};
use collect_diff_context_cli::impact_context::index::model::{
    GraphEdge, GraphFile, GraphGenerationIdentity, GraphModule, GraphSymbol, RepositoryGraph,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const MAX_FUZZ_INPUT_BYTES: usize = 1024 * 1024;
pub const MAX_GRAPH_INPUT_BYTES: usize = 1 + 4 * 64;

pub fn hex_id(value: usize) -> String {
    format!("{value:064x}")
}

pub fn repo_path(value: &str) -> RepoPath {
    RepoPath::new(value).expect("static fuzz path must be valid")
}

pub fn cache_layout(root: &Path) -> CacheLayout {
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

pub fn identity(candidate: usize) -> GraphGenerationIdentity {
    GraphGenerationIdentity {
        graph_schema_version: 1,
        candidate_manifest_digest: hex_id(10_000usize.wrapping_add(candidate)),
        project_model_digest: hex_id(20_001),
        resolver_digest: hex_id(20_002),
        adapter_query_digest: hex_id(20_003),
        file_facts_manifest_digest: hex_id(20_004),
        normalization_rules_digest: hex_id(20_005),
    }
}

pub fn synthetic_graph(node_count: usize, edge_count: usize) -> RepositoryGraph {
    let node_count = node_count.clamp(2, 32);
    let edge_count = edge_count.min(64);
    let mut files = Vec::with_capacity(node_count);
    let mut modules = Vec::with_capacity(node_count);
    let mut symbols = Vec::with_capacity(node_count);
    for index in 0..node_count {
        let path = repo_path(&format!("src/file_{index:02}.rs"));
        let module_id = hex_id(100 + index);
        let symbol_id = hex_id(1_000 + index);
        files.push(GraphFile {
            path: path.clone(),
            mode: "100644".to_string(),
            presence: CandidatePresence::Present,
            content_sha256: Some(hex_id(2_000 + index)),
            file_fact_key: None,
            language: Some("rust".to_string()),
            module_id: Some(module_id.clone()),
        });
        modules.push(GraphModule {
            module_id: module_id.clone(),
            parent_module_id: None,
            crate_name: "fuzz_fixture".to_string(),
            path: path.clone(),
            inline: false,
            root_module: index == 0,
            resolution_status: "resolved".to_string(),
        });
        symbols.push(GraphSymbol {
            symbol_id,
            local_id: format!("symbol-{index}"),
            module_id,
            path,
            language: "rust".to_string(),
            kind: "function".to_string(),
            name: format!("function_{index}"),
            owner_symbol_id: None,
            signature: Some(format!("pub fn function_{index}()")),
            visibility: Some("pub".to_string()),
            range: source_range(index),
            confidence: Confidence::Medium,
        });
    }
    let mut edges = Vec::with_capacity(edge_count);
    for index in 0..edge_count {
        let from = index % node_count;
        let to = (from + 1 + index / node_count) % node_count;
        edges.push(GraphEdge {
            edge_id: hex_id(10_000 + index),
            kind: if index % 2 == 0 {
                EdgeKind::Calls
            } else {
                EdgeKind::References
            },
            from_symbol: hex_id(1_000 + from),
            to_symbol: Some(hex_id(1_000 + to)),
            unresolved_target: None,
            path: repo_path(&format!("src/file_{from:02}.rs")),
            range: source_range(index),
            provider_id: "rust-tree-sitter-resolver".to_string(),
            provider_version: "rust-resolver/v1".to_string(),
            resolution: Resolution::ResolvedReference,
            confidence: Confidence::Medium,
            limitation_code: None,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    modules.sort_by(|left, right| left.module_id.cmp(&right.module_id));
    symbols.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
    edges.sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
    RepositoryGraph {
        identity: identity(node_count + edge_count),
        files,
        modules,
        symbols,
        edges,
        completeness: Completeness::Complete,
        limitations: Vec::new(),
    }
}

pub fn arbitrary_graph(data: &[u8]) -> RepositoryGraph {
    let data = &data[..data.len().min(MAX_GRAPH_INPUT_BYTES)];
    let node_count = usize::from(data.first().copied().unwrap_or(0) % 15).saturating_add(2);
    let edge_bytes = data.get(1..).unwrap_or_default();
    let edge_count = edge_bytes.len().div_ceil(4).min(64);
    let mut graph = synthetic_graph(node_count, 0);
    graph.identity = identity(data.iter().fold(node_count, |value, byte| {
        value.wrapping_mul(257) ^ usize::from(*byte)
    }));
    graph.edges = edge_bytes
        .chunks(4)
        .take(edge_count)
        .enumerate()
        .map(|(index, chunk)| {
            let from = usize::from(chunk.first().copied().unwrap_or(0)) % node_count;
            let to = usize::from(chunk.get(1).copied().unwrap_or(0)) % node_count;
            let kind = match chunk.get(2).copied().unwrap_or(0) % 7 {
                0 => EdgeKind::Calls,
                1 => EdgeKind::References,
                2 => EdgeKind::Imports,
                3 => EdgeKind::Exports,
                4 => EdgeKind::Defines,
                5 => EdgeKind::Implements,
                _ => EdgeKind::Overrides,
            };
            let resolved = chunk.get(3).copied().unwrap_or(0) % 3 != 0;
            GraphEdge {
                edge_id: hex_id(10_000 + index),
                kind,
                from_symbol: hex_id(1_000 + from),
                to_symbol: resolved.then(|| hex_id(1_000 + to)),
                unresolved_target: (!resolved).then(|| format!("target_{to}")),
                path: repo_path(&format!("src/file_{from:02}.rs")),
                range: source_range(index),
                provider_id: "rust-tree-sitter-resolver".to_string(),
                provider_version: "rust-resolver/v1".to_string(),
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
                limitation_code: (!resolved)
                    .then(|| "rust-resolver-reference-unresolved".to_string()),
            }
        })
        .collect();
    graph
        .edges
        .sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
    graph
}

pub fn split_graph_inputs(data: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut base = Vec::with_capacity(MAX_GRAPH_INPUT_BYTES);
    let mut candidate = Vec::with_capacity(MAX_GRAPH_INPUT_BYTES);
    for (index, byte) in data
        .iter()
        .copied()
        .take(MAX_GRAPH_INPUT_BYTES * 2)
        .enumerate()
    {
        if index % 2 == 0 {
            base.push(byte);
        } else {
            candidate.push(byte);
        }
    }
    (base, candidate)
}

pub fn input_fingerprint(data: &[u8]) -> usize {
    data.iter()
        .take(MAX_GRAPH_INPUT_BYTES)
        .fold(0usize, |value, byte| {
            value.wrapping_mul(257) ^ usize::from(*byte)
        })
}

pub fn mutate_candidate_graph(graph: &mut RepositoryGraph, data: &[u8]) -> BTreeSet<RepoPath> {
    let selected = usize::from(data.first().copied().unwrap_or(0)) % graph.files.len();
    let path = graph.files[selected].path.clone();
    let mut changed = BTreeSet::from([path.clone()]);
    match data.get(1).copied().unwrap_or(0) % 3 {
        0 => delete_path(graph, &path),
        1 => {
            let renamed = repo_path(&format!(
                "src/renamed_{:02}.rs",
                data.get(2).copied().unwrap_or(0)
            ));
            rename_path(graph, &path, &renamed);
            changed.insert(renamed);
        }
        _ => {
            graph.files[selected].content_sha256 =
                Some(hex_id(50_000usize.wrapping_add(input_fingerprint(data))));
        }
    }
    canonicalize_graph(graph);
    changed
}

pub fn select_changed_paths(
    base: &RepositoryGraph,
    candidate: &RepositoryGraph,
    data: &[u8],
    changed: &mut BTreeSet<RepoPath>,
) {
    let universe = base
        .files
        .iter()
        .chain(&candidate.files)
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let count = usize::from(data.get(3).copied().unwrap_or(0) % 4).saturating_add(1);
    let start = usize::from(data.get(4).copied().unwrap_or(0)) % universe.len();
    for offset in 0..count.min(universe.len()) {
        changed.insert(universe[(start + offset) % universe.len()].clone());
    }
}

fn delete_path(graph: &mut RepositoryGraph, path: &RepoPath) {
    let removed_symbols = graph
        .symbols
        .iter()
        .filter(|symbol| symbol.path == *path)
        .map(|symbol| symbol.symbol_id.clone())
        .collect::<BTreeSet<_>>();
    if let Some(file) = graph.files.iter_mut().find(|file| file.path == *path) {
        file.presence = CandidatePresence::Deleted;
        file.content_sha256 = None;
        file.file_fact_key = None;
        file.module_id = None;
    }
    graph.modules.retain(|module| module.path != *path);
    graph.symbols.retain(|symbol| symbol.path != *path);
    graph
        .edges
        .retain(|edge| !removed_symbols.contains(&edge.from_symbol));
    for edge in &mut graph.edges {
        if edge
            .to_symbol
            .as_ref()
            .is_some_and(|target| removed_symbols.contains(target))
        {
            let target = edge.to_symbol.take().expect("resolved target must exist");
            edge.unresolved_target = Some(target);
            edge.resolution = Resolution::Unresolved;
            edge.confidence = Confidence::Low;
            edge.limitation_code = Some("repository-fuzz-target-deleted".to_string());
        }
    }
}

fn rename_path(graph: &mut RepositoryGraph, old: &RepoPath, new: &RepoPath) {
    for file in &mut graph.files {
        if file.path == *old {
            file.path = new.clone();
        }
    }
    for module in &mut graph.modules {
        if module.path == *old {
            module.path = new.clone();
        }
    }
    for symbol in &mut graph.symbols {
        if symbol.path == *old {
            symbol.path = new.clone();
        }
    }
    for edge in &mut graph.edges {
        if edge.path == *old {
            edge.path = new.clone();
        }
    }
}

fn canonicalize_graph(graph: &mut RepositoryGraph) {
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
}

pub fn publish_graph(root: &Path, graph: &RepositoryGraph) -> PathBuf {
    let writer = RepositoryGraphWriter::new(cache_layout(root));
    let mut tracker = IndexBudgetTracker::new(IndexBudget::deep_defaults());
    match writer
        .publish(graph, &mut tracker)
        .expect("bounded fuzz graph must publish")
    {
        GraphPublishOutcome::Published { path } | GraphPublishOutcome::Reused { path } => path,
    }
}

pub fn open_graph(path: &Path, graph: &RepositoryGraph) -> RepositoryGraphReader {
    match RepositoryGraphReader::open_immutable(
        path,
        &graph.identity,
        ReaderLimits {
            maximum_database_bytes: 32 * 1024 * 1024,
            maximum_rows_per_query: 256,
            maximum_string_bytes: 4_096,
        },
    )
    .expect("bounded fuzz graph open must not fail")
    {
        CacheLookup::Hit(reader) => reader,
        CacheLookup::Miss => panic!("published fuzz graph missed"),
        CacheLookup::Stale { code } => panic!("published fuzz graph stale: {code}"),
        CacheLookup::Corrupt { code } => panic!("published fuzz graph corrupt: {code}"),
    }
}

fn source_range(index: usize) -> SourceRange {
    let line = u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX);
    let start_byte = index.saturating_mul(8);
    SourceRange {
        start_line: line,
        start_column: 1,
        end_line: line,
        end_column: 8,
        start_byte,
        end_byte: start_byte.saturating_add(7),
    }
}
