use collect_diff_context_cli::candidate::ChangedRange;
use collect_diff_context_cli::impact_context::adapters::tree_sitter_rust::{
    RustFileFacts, TreeSitterRustAdapter,
};
use collect_diff_context_cli::impact_context::budget::{BudgetTracker, ImpactBudget};
use collect_diff_context_cli::impact_context::contracts::ParseQuality;
use collect_diff_context_cli::impact_context::index::budget::{IndexBudget, IndexBudgetTracker};
use std::time::Duration;

const FULL_FILE_FIXTURE: &[u8] = br#"
#![allow(dead_code)]

pub use crate::alpha::{Item as Alias, nested::*, other::Thing};
use super::support;

#[path = "external_impl.rs"]
mod external;

pub mod inline {
    pub fn nested() {}
}

pub struct Service {
    stored: usize,
}

impl Service {
    #[inline]
    pub fn new() -> Self {
        Self
    }

    pub fn execute(&self) {
        helper();
        crate::api::run();
        self.finish();
        tracing::debug!("executed");
    }

    fn finish(&self) {}
}

const CONST_VALUE: usize = 1;

fn helper() {
    let value = CONST_VALUE;
    consume(value);
}

fn drive(service: &Service) {
    service.execute();
}
"#;

fn analyze(source: &[u8]) -> RustFileFacts {
    let mut budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
    TreeSitterRustAdapter::analyze_index(source, &mut budget).unwrap()
}

#[test]
fn index_extracts_all_definitions_not_only_changed_ranges() {
    let facts = analyze(FULL_FILE_FIXTURE);
    let names = facts
        .symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect::<Vec<_>>();

    for expected in [
        "external",
        "inline",
        "nested",
        "Service",
        "new",
        "execute",
        "finish",
        "CONST_VALUE",
        "helper",
        "drive",
    ] {
        assert!(
            names.contains(&expected),
            "missing symbol {expected}: {names:?}"
        );
    }
    assert!(facts
        .symbols
        .iter()
        .any(|symbol| symbol.name == "execute" && symbol.kind == "method"));
    assert!(facts
        .symbols
        .iter()
        .any(|symbol| symbol.name == "new" && symbol.kind == "associated-function"));
    assert!(facts
        .symbols
        .iter()
        .filter(|symbol| symbol.name == "execute" || symbol.name == "new")
        .all(|symbol| symbol.owner_local_id.is_some()));
}

#[test]
fn index_extracts_module_import_alias_group_and_glob_facts() {
    let facts = analyze(FULL_FILE_FIXTURE);

    assert!(facts.imports.iter().any(|import| {
        import.segments == ["crate", "alpha", "Item"]
            && import.alias.as_deref() == Some("Alias")
            && import.public
            && !import.glob
    }));
    assert!(facts.imports.iter().any(|import| {
        import.segments == ["crate", "alpha", "nested"]
            && import.alias.is_none()
            && import.public
            && import.glob
    }));
    assert!(facts
        .imports
        .iter()
        .any(|import| import.segments == ["super", "support"] && !import.public));

    let external = facts
        .module_declarations
        .iter()
        .find(|module| module.name == "external")
        .unwrap();
    assert!(!external.inline);
    assert_eq!(external.path_override.as_deref(), Some("external_impl.rs"));
    assert!(facts
        .module_declarations
        .iter()
        .any(|module| module.name == "inline" && module.inline));
}

#[test]
fn index_extracts_references_and_call_sites_with_local_owners() {
    let facts = analyze(FULL_FILE_FIXTURE);
    let execute = facts
        .symbols
        .iter()
        .find(|symbol| symbol.name == "execute")
        .unwrap();
    let helper = facts
        .symbols
        .iter()
        .find(|symbol| symbol.name == "helper")
        .unwrap();
    let service = facts
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Service")
        .unwrap();

    assert!(facts.calls.iter().any(|call| {
        call.callee == "run"
            && call.qualifier == ["crate", "api"]
            && call.call_kind == "function"
            && call.caller_local_id.as_deref() == Some(execute.local_id.as_str())
    }));
    assert!(facts.calls.iter().any(|call| {
        call.callee == "finish"
            && call.call_kind == "method"
            && call.caller_local_id.as_deref() == Some(execute.local_id.as_str())
    }));
    assert!(facts.calls.iter().any(|call| {
        call.callee == "debug"
            && call.qualifier == ["tracing"]
            && call.call_kind == "macro"
            && call.caller_local_id.as_deref() == Some(execute.local_id.as_str())
    }));
    assert!(facts.references.iter().any(|reference| {
        reference.name == "CONST_VALUE"
            && reference.owner_local_id.as_deref() == Some(helper.local_id.as_str())
    }));
    assert!(!facts
        .references
        .iter()
        .any(|reference| { reference.name == service.name && reference.range == service.range }));
    assert!(!facts
        .references
        .iter()
        .any(|reference| reference.name == "stored"));
}

#[test]
fn index_facts_are_path_independent_and_deterministic() {
    let first = analyze(FULL_FILE_FIXTURE);
    let second = analyze(FULL_FILE_FIXTURE);

    assert_eq!(first, second);
    let encoded = serde_json::to_string(&first).unwrap();
    assert!(!encoded.contains("src/first.rs"));
    let decoded: RustFileFacts = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, first);

    let mut value = serde_json::to_value(&first).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("repository_path".to_string(), "src/first.rs".into());
    assert!(serde_json::from_value::<RustFileFacts>(value).is_err());
}

#[test]
fn index_parse_recovery_records_affected_ranges_without_panicking() {
    let source = br#"
pub fn valid() -> usize { 1 }
fn broken( {
pub fn still_valid() -> usize { 2 }
"#;
    let facts = analyze(source);

    assert_ne!(facts.parse_quality, ParseQuality::Clean);
    assert!(!facts.recovery_ranges.is_empty());
    assert!(facts.symbols.iter().any(|symbol| symbol.name == "valid"));
    assert!(facts
        .symbols
        .iter()
        .any(|symbol| symbol.name == "still_valid"));
}

#[test]
fn index_fact_node_and_deadline_limits_return_partial_output() {
    let mut node_limits = IndexBudget::deep_defaults();
    node_limits.max_nodes = 1;
    let mut node_budget = IndexBudgetTracker::new(node_limits);
    let node_limited =
        TreeSitterRustAdapter::analyze_index(FULL_FILE_FIXTURE, &mut node_budget).unwrap();
    assert_eq!(node_limited.parse_quality, ParseQuality::Degraded);
    assert!(node_limited
        .limitations
        .contains(&"index-node-budget-exhausted".to_string()));
    assert_eq!(node_limited.metrics.nodes_visited, 1);

    let mut fact_limits = IndexBudget::deep_defaults();
    fact_limits.max_facts = 2;
    let mut fact_budget = IndexBudgetTracker::new(fact_limits);
    let fact_limited =
        TreeSitterRustAdapter::analyze_index(FULL_FILE_FIXTURE, &mut fact_budget).unwrap();
    assert_eq!(fact_limited.parse_quality, ParseQuality::Degraded);
    assert!(fact_limited
        .limitations
        .contains(&"index-fact-budget-exhausted".to_string()));
    assert!(fact_limited.metrics.facts_emitted <= 2);

    let mut deadline_limits = IndexBudget::deep_defaults();
    deadline_limits.deadline = Duration::ZERO;
    let mut deadline_budget = IndexBudgetTracker::new(deadline_limits);
    let deadline_limited =
        TreeSitterRustAdapter::analyze_index(FULL_FILE_FIXTURE, &mut deadline_budget).unwrap();
    assert_eq!(deadline_limited.parse_quality, ParseQuality::Degraded);
    assert!(deadline_limited
        .limitations
        .contains(&"index-deadline-exhausted".to_string()));
}

#[test]
fn fast_changed_range_output_remains_unchanged() {
    let source = b"fn unchanged() { outside(); }\nfn changed() { inside(); }\n";
    let changed_ranges = [ChangedRange {
        start_line: 2,
        end_line: 2,
        deletion_anchor: false,
    }];
    let mut first_budget = BudgetTracker::new(ImpactBudget::fast_defaults());
    let before =
        TreeSitterRustAdapter::analyze(source, &changed_ranges, &mut first_budget).unwrap();

    let _ = analyze(source);

    let mut second_budget = BudgetTracker::new(ImpactBudget::fast_defaults());
    let after =
        TreeSitterRustAdapter::analyze(source, &changed_ranges, &mut second_budget).unwrap();
    assert_eq!(before, after);
    assert_eq!(
        after
            .changed_symbols
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect::<Vec<_>>(),
        vec!["changed"]
    );
    assert!(after.calls.iter().any(|call| call.target == "inside"));
    assert!(!after.calls.iter().any(|call| call.target == "outside"));
}
