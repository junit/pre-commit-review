#![no_main]

mod support;

use collect_diff_context_cli::impact_context::cache::file_facts::{CacheLookup, FileFactsStore};
use collect_diff_context_cli::impact_context::index::model::FileFactKey;
use libfuzzer_sys::fuzz_target;
use std::fs;
use std::sync::{Mutex, OnceLock};
use support::{cache_layout, hex_id, MAX_FUZZ_INPUT_BYTES};

struct Fixture {
    _cache: tempfile::TempDir,
    store: FileFactsStore,
    key: FileFactKey,
    path: std::path::PathBuf,
}

fn fixture() -> &'static Mutex<Fixture> {
    static FIXTURE: OnceLock<Mutex<Fixture>> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let cache = tempfile::tempdir().expect("create bounded fuzz cache");
        let store = FileFactsStore::new(cache_layout(cache.path()), MAX_FUZZ_INPUT_BYTES)
            .expect("create bounded file facts store");
        let key = FileFactKey {
            language: "rust".to_string(),
            content_sha256: hex_id(101),
            grammar_version: "tree-sitter-rust@0.24.2".to_string(),
            query_digest: hex_id(102),
            adapter_version: "tree-sitter-rust-index/v1".to_string(),
            normalization_rules_digest: hex_id(103),
            schema_version: 1,
        };
        let path = store.object_path(&key).expect("derive bounded object path");
        fs::create_dir_all(path.parent().expect("object parent")).expect("create object parent");
        Mutex::new(Fixture {
            _cache: cache,
            store,
            key,
            path,
        })
    })
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_FUZZ_INPUT_BYTES {
        return;
    }
    let fixture = fixture().lock().expect("lock file facts fuzz fixture");
    fs::write(&fixture.path, data).expect("write fuzz object");

    let lookup = fixture
        .store
        .lookup(&fixture.key)
        .expect("arbitrary object bytes must decode safely");
    match lookup {
        CacheLookup::Hit(facts) => {
            let encoded = serde_json::to_vec(&facts).expect("facts must serialize");
            assert!(encoded.len() <= MAX_FUZZ_INPUT_BYTES);
        }
        CacheLookup::Miss | CacheLookup::Stale { .. } | CacheLookup::Corrupt { .. } => {}
    }
});
