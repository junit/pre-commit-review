#![no_main]

mod support;

use collect_diff_context_cli::impact_context::index::budget::{IndexBudget, IndexBudgetTracker};
use collect_diff_context_cli::impact_context::index::overlay::build_repository_overlay;
use libfuzzer_sys::fuzz_target;
use std::time::Duration;
use support::{
    arbitrary_graph, input_fingerprint, mutate_candidate_graph, open_graph, publish_graph,
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

    let mut budget = IndexBudget::deep_defaults();
    budget.deadline = Duration::from_secs(2);
    budget.max_overlay_paths = usize::from(data.get(6).copied().unwrap_or(0) % 16) + 1;
    budget.max_nodes = usize::from(data.get(7).copied().unwrap_or(0) % 64) + 1;
    budget.max_symbols = usize::from(data.get(8).copied().unwrap_or(0) % 32) + 1;
    budget.max_edges = usize::from(data.get(9).copied().unwrap_or(0) % 64) + 1;
    budget.max_generation_bytes = usize::from(data.get(10).copied().unwrap_or(0) % 64 + 1) * 1024;
    budget.max_query_rows = usize::from(data.get(11).copied().unwrap_or(0) % 64) + 1;
    let limits = budget.clone();
    let mut first_budget = IndexBudgetTracker::new(budget.clone());
    let mut second_budget = IndexBudgetTracker::new(budget);
    let first = build_repository_overlay(&reader, &candidate, &changed, &mut first_budget)
        .expect("bounded arbitrary overlay must build");
    let second = build_repository_overlay(&reader, &candidate, &changed, &mut second_budget)
        .expect("bounded arbitrary overlay must be repeatable");
    assert_eq!(first, second);
    assert!(first.path_tombstones.len() <= limits.max_overlay_paths);
    assert!(first.files.len() + first.modules.len() + first.symbols.len() <= limits.max_nodes);
    assert!(first.symbols.len() <= limits.max_symbols);
    assert!(first.outgoing_edges.values().map(Vec::len).sum::<usize>() <= limits.max_edges);
    for edges in first
        .outgoing_edges
        .values()
        .chain(first.incoming_edges.values())
    {
        assert!(edges
            .windows(2)
            .all(|pair| pair[0].edge_id < pair[1].edge_id));
    }
});
