use collect_diff_context_cli::candidate::{CandidatePresence, RepoPath};
use collect_diff_context_cli::impact_context::contracts::{Completeness, UnitStatus};
use collect_diff_context_cli::impact_context::index::budget::{
    IndexBudget, IndexBudgetTracker, IndexResource,
};
use collect_diff_context_cli::impact_context::index::model::{
    FileFactKey, GraphGenerationIdentity, IndexAction, IndexContractError, IndexLimitation,
    IndexMetrics, IndexReport, IndexReportStatus, RepositoryLocator, RepositoryManifest,
    RepositoryManifestEntry,
};
use collect_diff_context_cli::review_scope::ReviewSource;
use serde_json::json;

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn fingerprint(character: char) -> String {
    std::iter::repeat_n(character, 40).collect()
}

fn valid_locator() -> RepositoryLocator {
    RepositoryLocator {
        source: ReviewSource::Staged,
        object_format: "sha1".to_string(),
        base_tree: Some(fingerprint('1')),
        index_manifest_digest: Some(digest('2')),
        overlay_candidate_digest: digest('3'),
    }
}

fn manifest_entry(path: &str) -> RepositoryManifestEntry {
    RepositoryManifestEntry {
        path: RepoPath::new(path).unwrap(),
        mode: "100644".to_string(),
        presence: CandidatePresence::Present,
        content_sha256: Some(digest('4')),
        content_bytes: Some(10),
        language: Some("rust".to_string()),
        status: UnitStatus::Completed,
        limitation_codes: Vec::new(),
    }
}

fn valid_manifest(paths: &[&str]) -> RepositoryManifest {
    RepositoryManifest {
        locator: valid_locator(),
        digest: digest('5'),
        entries: paths.iter().map(|path| manifest_entry(path)).collect(),
        completeness: Completeness::Complete,
        limitations: Vec::new(),
    }
}

fn valid_file_fact_key() -> FileFactKey {
    FileFactKey {
        language: "rust".to_string(),
        content_sha256: digest('a'),
        grammar_version: "tree-sitter-rust@0.24.2".to_string(),
        query_digest: digest('b'),
        adapter_version: "rust-index-adapter/v1".to_string(),
        normalization_rules_digest: digest('c'),
        schema_version: 1,
    }
}

fn valid_generation_identity() -> GraphGenerationIdentity {
    GraphGenerationIdentity {
        graph_schema_version: 1,
        candidate_manifest_digest: digest('1'),
        project_model_digest: digest('2'),
        resolver_digest: digest('3'),
        adapter_query_digest: digest('4'),
        file_facts_manifest_digest: digest('5'),
        normalization_rules_digest: digest('6'),
    }
}

fn valid_metrics() -> IndexMetrics {
    IndexMetrics {
        elapsed_ms: 1,
        manifest_files: 2,
        manifest_bytes: 20,
        file_fact_hits: 1,
        file_fact_misses: 1,
        file_fact_writes: 1,
        parsed_files: 1,
        parsed_bytes: 10,
        symbols: 2,
        edges: 1,
        query_rows: 1,
        generation_bytes: 4096,
        output_bytes: 512,
    }
}

fn valid_report() -> IndexReport {
    IndexReport {
        schema_version: 1,
        kind: "repository_index_report".to_string(),
        action: IndexAction::Build,
        status: IndexReportStatus::Completed,
        scope_fingerprint: Some(fingerprint('a')),
        repository_id: digest('b'),
        generation_key: Some(digest('c')),
        metrics: valid_metrics(),
        limitations: Vec::new(),
    }
}

#[test]
fn index_budget_defaults_are_bounded() {
    let budget = IndexBudget::deep_defaults();
    assert_eq!(budget.deadline.as_secs(), 30);
    assert_eq!(budget.max_manifest_files, 100_000);
    assert_eq!(budget.max_manifest_bytes, 32 * 1024 * 1024);
    assert_eq!(budget.max_project_model_files, 1_000);
    assert_eq!(budget.max_project_model_bytes, 8 * 1024 * 1024);
    assert_eq!(budget.max_file_bytes, 2 * 1024 * 1024);
    assert_eq!(budget.max_parse_bytes, 512 * 1024 * 1024);
    assert_eq!(budget.max_nodes, 10_000_000);
    assert_eq!(budget.max_facts, 2_000_000);
    assert_eq!(budget.max_symbols, 1_000_000);
    assert_eq!(budget.max_edges, 5_000_000);
    assert_eq!(budget.max_generation_bytes, 2 * 1024 * 1024 * 1024);
    assert_eq!(budget.max_overlay_paths, 10_000);
    assert_eq!(budget.max_query_rows, 50_000);
    assert_eq!(budget.max_graph_depth, 2);

    let mut tracker = IndexBudgetTracker::new(budget);
    tracker
        .consume(IndexResource::ManifestFiles, 100_000)
        .unwrap();
    let error = tracker
        .consume(IndexResource::ManifestFiles, 1)
        .unwrap_err();
    assert_eq!(error.code(), "index-manifest-file-budget-exhausted");
    assert_eq!(error.resource(), Some(IndexResource::ManifestFiles));
    assert!(tracker.amount(IndexResource::ManifestFiles).exhausted);
}

#[test]
fn repository_manifest_rejects_unsorted_duplicate_and_unsafe_paths() {
    assert!(valid_manifest(&["src/a.rs", "src/b.rs"]).validate().is_ok());
    assert!(valid_manifest(&["src/b.rs", "src/a.rs"])
        .validate()
        .is_err());
    assert!(valid_manifest(&["src/a.rs", "src/a.rs"])
        .validate()
        .is_err());

    for unsafe_path in ["../escape.rs", "src\\escape.rs"] {
        let unsafe_manifest = json!({
            "locator": {
                "source": "staged",
                "object_format": "sha1",
                "base_tree": fingerprint('1'),
                "index_manifest_digest": digest('2'),
                "overlay_candidate_digest": digest('3')
            },
            "digest": digest('5'),
            "entries": [{
                "path": unsafe_path,
                "mode": "100644",
                "presence": "present",
                "content_sha256": digest('4'),
                "content_bytes": 10,
                "language": "rust",
                "status": "completed",
                "limitation_codes": []
            }],
            "completeness": "complete",
            "limitations": []
        });
        assert!(
            serde_json::from_value::<RepositoryManifest>(unsafe_manifest).is_err(),
            "accepted unsafe path {unsafe_path:?}"
        );
    }
}

#[test]
fn file_fact_key_requires_exact_lowercase_digests() {
    assert!(valid_file_fact_key().validate().is_ok());

    let mut uppercase = valid_file_fact_key();
    uppercase.content_sha256 = "A".repeat(64);
    assert!(uppercase.validate().is_err());

    let mut short = valid_file_fact_key();
    short.query_digest = "a".repeat(63);
    assert!(short.validate().is_err());

    let mut empty_version = valid_file_fact_key();
    empty_version.adapter_version.clear();
    assert!(empty_version.validate().is_err());
}

#[test]
fn graph_generation_key_changes_for_every_identity_input() {
    let baseline = valid_generation_identity();
    let baseline_key = baseline.generation_key().unwrap();
    let mut mutations = Vec::new();

    let mut identity = baseline.clone();
    identity.graph_schema_version += 1;
    mutations.push(identity);

    let mut identity = baseline.clone();
    identity.candidate_manifest_digest = digest('7');
    mutations.push(identity);

    let mut identity = baseline.clone();
    identity.project_model_digest = digest('7');
    mutations.push(identity);

    let mut identity = baseline.clone();
    identity.resolver_digest = digest('7');
    mutations.push(identity);

    let mut identity = baseline.clone();
    identity.adapter_query_digest = digest('7');
    mutations.push(identity);

    let mut identity = baseline.clone();
    identity.file_facts_manifest_digest = digest('7');
    mutations.push(identity);

    let mut identity = baseline;
    identity.normalization_rules_digest = digest('7');
    mutations.push(identity);

    for mutation in mutations {
        assert_ne!(mutation.generation_key().unwrap(), baseline_key);
    }
}

#[test]
fn index_report_rejects_unknown_fields_and_invalid_counts() {
    let report = valid_report();
    assert!(report.validate().is_ok());

    let mut unknown = serde_json::to_value(&report).unwrap();
    unknown["unexpected"] = json!(true);
    assert!(serde_json::from_value::<IndexReport>(unknown).is_err());

    let mut too_many_results = report.clone();
    too_many_results.metrics.manifest_files = 1;
    assert!(too_many_results.validate().is_err());

    let mut too_many_writes = report;
    too_many_writes.metrics.file_fact_writes = 2;
    assert!(too_many_writes.validate().is_err());

    let mut over_deadline = valid_report();
    over_deadline.metrics.elapsed_ms = 60_001;
    assert!(over_deadline.validate().is_err());
}

#[test]
fn index_contract_error_is_an_error_type() {
    fn assert_error<T: std::error::Error>() {}
    assert_error::<IndexContractError>();
}

#[test]
fn index_limitation_paths_are_validated() {
    let mut report = valid_report();
    report.status = IndexReportStatus::Partial;
    report.limitations.push(IndexLimitation {
        code: "index-partial".to_string(),
        path: Some(RepoPath::new("src/lib.rs").unwrap()),
        symbol_id: None,
        reason: "bounded test".to_string(),
        interpretation: "the index is partial".to_string(),
    });
    assert!(report.validate().is_ok());
}
