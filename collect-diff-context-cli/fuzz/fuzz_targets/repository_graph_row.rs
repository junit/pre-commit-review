#![no_main]

mod support;

use collect_diff_context_cli::impact_context::cache::file_facts::CacheLookup;
use collect_diff_context_cli::impact_context::cache::sqlite_generation::{
    ReaderLimits, RepositoryGraphReader,
};
use libfuzzer_sys::fuzz_target;
use rusqlite::Connection;
use std::sync::{Mutex, OnceLock};
use support::{hex_id, publish_graph, synthetic_graph, MAX_FUZZ_INPUT_BYTES};

struct Fixture {
    _cache: tempfile::TempDir,
    graph: collect_diff_context_cli::impact_context::index::model::RepositoryGraph,
    path: std::path::PathBuf,
}

fn fixture() -> &'static Mutex<Fixture> {
    static FIXTURE: OnceLock<Mutex<Fixture>> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let cache = tempfile::tempdir().expect("create bounded fuzz cache");
        let graph = synthetic_graph(4, 8);
        let path = publish_graph(cache.path(), &graph);
        Mutex::new(Fixture {
            _cache: cache,
            graph,
            path,
        })
    })
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_FUZZ_INPUT_BYTES {
        return;
    }
    let fixture = fixture().lock().expect("lock graph row fuzz fixture");
    let replacement = String::from_utf8_lossy(data);
    let connection = Connection::open(&fixture.path).expect("open fuzz generation for mutation");
    connection
        .execute(
            "UPDATE edges SET canonical_json = ?1 WHERE edge_id = ?2",
            (&replacement.as_ref(), hex_id(10_000)),
        )
        .expect("mutate one bounded graph row");
    drop(connection);

    let lookup = RepositoryGraphReader::open_immutable(
        &fixture.path,
        &fixture.graph.identity,
        ReaderLimits {
            maximum_database_bytes: 32 * 1024 * 1024,
            maximum_rows_per_query: 32,
            maximum_string_bytes: 4_096,
        },
    )
    .expect("arbitrary row bytes must open safely");
    if let CacheLookup::Hit(reader) = lookup {
        match reader.outgoing(&hex_id(1_000), 8) {
            Ok(edges) => assert!(edges.len() <= 8),
            Err(error) => assert_eq!(error.code, "generation-row-corrupt"),
        }
    }
});
