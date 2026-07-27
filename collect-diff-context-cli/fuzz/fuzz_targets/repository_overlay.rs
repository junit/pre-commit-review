#![no_main]

mod support;

use collect_diff_context_cli::candidate::CandidatePresence;
use collect_diff_context_cli::impact_context::contracts::Completeness;
use collect_diff_context_cli::impact_context::index::budget::{IndexBudget, IndexBudgetTracker};
use collect_diff_context_cli::impact_context::index::overlay::build_repository_overlay;
use libfuzzer_sys::fuzz_target;
use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};
use support::{
    identity, open_graph, publish_graph, repo_path, synthetic_graph, MAX_FUZZ_INPUT_BYTES,
};

struct Fixture {
    _cache: tempfile::TempDir,
    base: collect_diff_context_cli::impact_context::index::model::RepositoryGraph,
    reader:
        collect_diff_context_cli::impact_context::cache::sqlite_generation::RepositoryGraphReader,
}

fn fixture() -> &'static Mutex<Fixture> {
    static FIXTURE: OnceLock<Mutex<Fixture>> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let cache = tempfile::tempdir().expect("create bounded fuzz cache");
        let base = synthetic_graph(8, 24);
        let path = publish_graph(cache.path(), &base);
        let reader = open_graph(&path, &base);
        Mutex::new(Fixture {
            _cache: cache,
            base,
            reader,
        })
    })
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_FUZZ_INPUT_BYTES {
        return;
    }
    let fixture = fixture().lock().expect("lock overlay fuzz fixture");
    let changed_path = repo_path("src/file_00.rs");
    let mut changed = BTreeSet::from([changed_path.clone()]);
    let mut candidate = fixture.base.clone();
    candidate.identity = identity(999);
    match data.first().copied().unwrap_or_default() % 4 {
        0 => {
            candidate.files.retain(|file| file.path != changed_path);
            candidate
                .modules
                .retain(|module| module.path != changed_path);
            let removed = candidate
                .symbols
                .iter()
                .filter(|symbol| symbol.path == changed_path)
                .map(|symbol| symbol.symbol_id.clone())
                .collect::<BTreeSet<_>>();
            candidate
                .symbols
                .retain(|symbol| !removed.contains(&symbol.symbol_id));
            candidate.edges.retain(|edge| {
                !removed.contains(&edge.from_symbol)
                    && edge
                        .to_symbol
                        .as_ref()
                        .is_none_or(|target| !removed.contains(target))
            });
        }
        1 => {
            let renamed = repo_path("src/renamed.rs");
            changed.insert(renamed.clone());
            for file in &mut candidate.files {
                if file.path == changed_path {
                    file.path = renamed.clone();
                    file.presence = CandidatePresence::Present;
                }
            }
            for module in &mut candidate.modules {
                if module.path == changed_path {
                    module.path = renamed.clone();
                }
            }
            for symbol in &mut candidate.symbols {
                if symbol.path == changed_path {
                    symbol.path = renamed.clone();
                }
            }
        }
        2 => candidate.completeness = Completeness::Partial,
        _ => {}
    }
    candidate
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    candidate
        .modules
        .sort_by(|left, right| left.module_id.cmp(&right.module_id));
    candidate
        .symbols
        .sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
    candidate
        .edges
        .sort_by(|left, right| left.edge_id.cmp(&right.edge_id));

    let mut budget = IndexBudget::deep_defaults();
    budget.max_overlay_paths = usize::from(data.get(1).copied().unwrap_or(8) % 8).saturating_add(1);
    budget.max_nodes = 128;
    budget.max_edges = 128;
    let mut first_budget = IndexBudgetTracker::new(budget.clone());
    let mut second_budget = IndexBudgetTracker::new(budget);
    let first = build_repository_overlay(&fixture.reader, &candidate, &changed, &mut first_budget);
    let second =
        build_repository_overlay(&fixture.reader, &candidate, &changed, &mut second_budget);
    assert_eq!(first, second);
});
