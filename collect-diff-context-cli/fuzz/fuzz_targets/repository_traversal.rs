#![no_main]

mod support;

use collect_diff_context_cli::impact_context::contracts::EdgeKind;
use collect_diff_context_cli::impact_context::index::budget::{IndexBudget, IndexBudgetTracker};
use collect_diff_context_cli::impact_context::index::overlay::build_repository_overlay;
use collect_diff_context_cli::impact_context::index::traversal::{
    traverse_repository_graph, TraversalDirection, TraversalRequest,
};
use libfuzzer_sys::fuzz_target;
use std::collections::BTreeSet;
use std::time::Duration;
use support::{
    arbitrary_graph, hex_id, input_fingerprint, mutate_candidate_graph, open_graph, publish_graph,
    select_changed_paths, split_graph_inputs, MAX_FUZZ_INPUT_BYTES,
};

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_FUZZ_INPUT_BYTES {
        return;
    }
    let (base_input, candidate_input) = split_graph_inputs(data);
    let base = arbitrary_graph(&base_input);
    let mut candidate = arbitrary_graph(&candidate_input);
    candidate.identity = support::identity(
        input_fingerprint(&candidate_input)
            .wrapping_add(data.len())
            .wrapping_add(1),
    );
    let mut changed = mutate_candidate_graph(&mut candidate, data.get(5..).unwrap_or_default());
    select_changed_paths(&base, &candidate, data, &mut changed);

    let cache = tempfile::tempdir().expect("create bounded fuzz cache");
    let path = publish_graph(cache.path(), &base);
    let reader = open_graph(&path, &base);
    let mut overlay_budget = IndexBudget::deep_defaults();
    overlay_budget.deadline = Duration::from_secs(2);
    overlay_budget.max_overlay_paths = 64;
    overlay_budget.max_nodes = 1_024;
    overlay_budget.max_symbols = 256;
    overlay_budget.max_edges = 256;
    overlay_budget.max_generation_bytes = 1024 * 1024;
    overlay_budget.max_query_rows = 512;
    let overlay = build_repository_overlay(
        &reader,
        &candidate,
        &changed,
        &mut IndexBudgetTracker::new(overlay_budget),
    )
    .expect("bounded arbitrary traversal overlay must build");

    let mut symbol_pool = base
        .symbols
        .iter()
        .chain(&candidate.symbols)
        .map(|symbol| symbol.symbol_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if data.get(12).copied().unwrap_or(0) % 4 == 0 {
        symbol_pool.push(hex_id(99_999));
    }
    let root_count = usize::from(data.get(13).copied().unwrap_or(0) % 4) + 1;
    let root_start = usize::from(data.get(14).copied().unwrap_or(0)) % symbol_pool.len();
    let roots = (0..root_count)
        .map(|offset| symbol_pool[(root_start + offset) % symbol_pool.len()].clone())
        .collect();

    let direction_mask = data.get(15).copied().unwrap_or(3) % 4;
    let mut directions = BTreeSet::new();
    if direction_mask & 1 != 0 {
        directions.insert(TraversalDirection::Incoming);
    }
    if direction_mask & 2 != 0 {
        directions.insert(TraversalDirection::Outgoing);
    }
    if directions.is_empty() {
        directions.extend([TraversalDirection::Incoming, TraversalDirection::Outgoing]);
    }
    let all_edge_kinds = [
        EdgeKind::Calls,
        EdgeKind::References,
        EdgeKind::Imports,
        EdgeKind::Exports,
        EdgeKind::Defines,
        EdgeKind::Implements,
        EdgeKind::Overrides,
    ];
    let edge_kind_mask = data.get(16).copied().unwrap_or(u8::MAX);
    let mut edge_kinds = all_edge_kinds
        .into_iter()
        .enumerate()
        .filter_map(|(index, kind)| (edge_kind_mask & (1 << index) != 0).then_some(kind))
        .collect::<BTreeSet<_>>();
    if edge_kinds.is_empty() {
        edge_kinds.extend(all_edge_kinds);
    }
    let request = TraversalRequest {
        roots,
        directions,
        edge_kinds,
        maximum_depth: usize::from(data.get(17).copied().unwrap_or(0) % 3),
        maximum_rows: usize::from(data.get(18).copied().unwrap_or(0) % 64) + 1,
        maximum_nodes: usize::from(data.get(19).copied().unwrap_or(0) % 32) + 1,
        maximum_edges: usize::from(data.get(20).copied().unwrap_or(0) % 65),
        maximum_bytes: usize::from(data.get(21).copied().unwrap_or(0) % 33) * 1024,
        deadline: Duration::from_secs(2),
    };
    let selected_overlay = (data.get(22).copied().unwrap_or(0) % 2 == 0).then_some(&overlay);
    let mut first = traverse_repository_graph(&reader, selected_overlay, &request)
        .expect("bounded arbitrary traversal must terminate");
    let mut second = traverse_repository_graph(&reader, selected_overlay, &request)
        .expect("bounded arbitrary traversal must be repeatable");
    first.elapsed_ms = 0;
    second.elapsed_ms = 0;
    assert_eq!(first, second);
    assert!(first.rows_read <= request.maximum_rows);
    assert!(first.nodes_visited <= request.maximum_nodes);
    assert!(first.edges.len() <= request.maximum_edges);
    assert!(first.bytes_read <= request.maximum_bytes);
    assert!(first.reached_depth <= request.maximum_depth);
    assert!(first
        .edges
        .windows(2)
        .all(|pair| pair[0].edge_id < pair[1].edge_id));
    assert!(first
        .edges
        .iter()
        .all(|edge| request.edge_kinds.contains(&edge.kind)));
});
