#![no_main]

mod support;

use collect_diff_context_cli::impact_context::contracts::EdgeKind;
use collect_diff_context_cli::impact_context::index::traversal::{
    traverse_repository_graph, TraversalDirection, TraversalRequest,
};
use libfuzzer_sys::fuzz_target;
use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use support::{hex_id, open_graph, publish_graph, synthetic_graph, MAX_FUZZ_INPUT_BYTES};

struct Fixture {
    _cache: tempfile::TempDir,
    reader:
        collect_diff_context_cli::impact_context::cache::sqlite_generation::RepositoryGraphReader,
}

fn fixtures() -> &'static [Mutex<Fixture>] {
    static FIXTURES: OnceLock<Vec<Mutex<Fixture>>> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        [(4, 8), (8, 24), (16, 48), (32, 64)]
            .into_iter()
            .map(|(nodes, edges)| {
                let cache = tempfile::tempdir().expect("create bounded fuzz cache");
                let graph = synthetic_graph(nodes, edges);
                let path = publish_graph(cache.path(), &graph);
                let reader = open_graph(&path, &graph);
                Mutex::new(Fixture {
                    _cache: cache,
                    reader,
                })
            })
            .collect()
    })
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_FUZZ_INPUT_BYTES {
        return;
    }
    let nodes = usize::from(data.first().copied().unwrap_or(4) % 31).saturating_add(2);
    let edges = usize::from(data.get(1).copied().unwrap_or(8) % 64);
    let fixture_index = match nodes.max(edges.div_ceil(2)) {
        0..=4 => 0,
        5..=8 => 1,
        9..=16 => 2,
        _ => 3,
    };
    let fixture = fixtures()[fixture_index]
        .lock()
        .expect("lock traversal fuzz fixture");
    let root_count = usize::from(data.get(2).copied().unwrap_or(1) % 4).saturating_add(1);
    let roots = (0..root_count.min(nodes))
        .map(|index| hex_id(1_000 + index))
        .collect();
    let request = TraversalRequest {
        roots,
        directions: BTreeSet::from([TraversalDirection::Incoming, TraversalDirection::Outgoing]),
        edge_kinds: BTreeSet::from([EdgeKind::Calls, EdgeKind::References]),
        maximum_depth: usize::from(data.get(3).copied().unwrap_or(1) % 2).saturating_add(1),
        maximum_rows: usize::from(data.get(4).copied().unwrap_or(64) % 64).saturating_add(1),
        maximum_nodes: 64,
        maximum_edges: 64,
        maximum_bytes: 64 * 1024,
        deadline: Duration::from_millis(100),
    };
    let mut first = traverse_repository_graph(&fixture.reader, None, &request)
        .expect("bounded arbitrary traversal must terminate");
    let mut second = traverse_repository_graph(&fixture.reader, None, &request)
        .expect("bounded arbitrary traversal must be repeatable");
    first.elapsed_ms = 0;
    second.elapsed_ms = 0;
    assert_eq!(first, second);
    assert!(first.rows_read <= request.maximum_rows);
    assert!(first.nodes_visited <= request.maximum_nodes);
    assert!(first.edges.len() <= request.maximum_edges);
    assert!(first.bytes_read <= request.maximum_bytes);
});
