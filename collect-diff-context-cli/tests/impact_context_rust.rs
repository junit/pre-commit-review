use collect_diff_context_cli::candidate::{
    CandidateBytes, CandidateContent, CandidateError, CandidateFile, CandidatePresence,
    ChangedRange, RepoPath,
};
use collect_diff_context_cli::impact_context::adapters::text::{
    TextAdapter, TextFactKind, TextProvenance,
};
use collect_diff_context_cli::impact_context::adapters::tree_sitter_rust::TreeSitterRustAdapter;
use collect_diff_context_cli::impact_context::budget::{
    BudgetResource, BudgetTracker, ImpactBudget,
};
use collect_diff_context_cli::impact_context::contracts::{ParseQuality, Resolution};
use collect_diff_context_cli::review_scope::ReviewSource;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::Duration;

struct MemoryCandidate {
    files: Vec<CandidateFile>,
    contents: BTreeMap<String, Vec<u8>>,
}

impl MemoryCandidate {
    fn new(entries: &[(&str, &[u8], bool)]) -> Self {
        let mut files = Vec::new();
        let mut contents = BTreeMap::new();
        for (path, bytes, changed) in entries {
            contents.insert((*path).to_string(), bytes.to_vec());
            files.push(CandidateFile {
                path: RepoPath::new(*path).unwrap(),
                mode: "100644".to_string(),
                content_identity: Some(format!("sha256:{:x}", Sha256::digest(bytes))),
                presence: CandidatePresence::Present,
                manifest_unit_id: changed.then(|| format!("file:{path}")),
                change_status: changed.then(|| "M".to_string()),
                changed_ranges: Vec::new(),
            });
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Self { files, contents }
    }
}

impl CandidateContent for MemoryCandidate {
    fn scope_fingerprint(&self) -> &str {
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }

    fn candidate_digest(&self) -> &str {
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }

    fn source(&self) -> ReviewSource {
        ReviewSource::Staged
    }

    fn files(&self) -> &[CandidateFile] {
        &self.files
    }

    fn read(&self, path: &RepoPath) -> Result<CandidateBytes, CandidateError> {
        let bytes = self
            .contents
            .get(path.as_str())
            .expect("memory candidate path must exist")
            .clone();
        Ok(CandidateBytes {
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            binary: bytes.iter().take(8192).any(|byte| *byte == 0),
            bytes,
        })
    }
}

#[test]
fn budget_file_bytes_exhaust_independently() {
    let mut budget = ImpactBudget::fast_defaults();
    budget.max_file_bytes = 4;
    budget.max_total_bytes = 100;
    let mut tracker = BudgetTracker::new(budget);

    tracker.observe(BudgetResource::FileBytes, 4).unwrap();
    let error = tracker
        .observe(BudgetResource::FileBytes, 5)
        .expect_err("oversized file must exhaust only the file-byte budget");

    assert_eq!(error.code(), "file-byte-budget-exhausted");
    assert_eq!(tracker.amount(BudgetResource::FileBytes).initial, 4);
    assert_eq!(tracker.amount(BudgetResource::FileBytes).consumed, 4);
    assert_eq!(tracker.amount(BudgetResource::FileBytes).remaining, 0);
    assert_eq!(tracker.amount(BudgetResource::TotalBytes).consumed, 0);
    assert_eq!(tracker.amount(BudgetResource::TotalBytes).remaining, 100);
}

#[test]
fn budget_fast_defaults_match_the_contract() {
    let budget = ImpactBudget::fast_defaults();
    assert_eq!(budget.deadline, Duration::from_millis(750));
    assert_eq!(budget.max_changed_files, 30);
    assert_eq!(budget.max_file_bytes, 2 * 1024 * 1024);
    assert_eq!(budget.max_total_bytes, 8 * 1024 * 1024);
    assert_eq!(budget.max_nodes, 250_000);
    assert_eq!(budget.max_nesting_depth, 512);
    assert_eq!(budget.max_facts, 5_000);
    assert_eq!(budget.max_edges, 500);
    assert_eq!(budget.max_output_bytes, 1_048_576);
    assert_eq!(budget.max_query_patterns, 32);
    assert_eq!(budget.max_matches_per_pattern, 20);
}

#[test]
fn budget_cumulative_resources_never_exceed_their_initial_amount() {
    for resource in [
        BudgetResource::ChangedFiles,
        BudgetResource::TotalBytes,
        BudgetResource::Nodes,
        BudgetResource::Facts,
        BudgetResource::Edges,
        BudgetResource::OutputBytes,
        BudgetResource::QueryPatterns,
    ] {
        let mut budget = ImpactBudget::fast_defaults();
        budget.max_changed_files = 2;
        budget.max_total_bytes = 2;
        budget.max_nodes = 2;
        budget.max_facts = 2;
        budget.max_edges = 2;
        budget.max_output_bytes = 2;
        budget.max_query_patterns = 2;
        let mut tracker = BudgetTracker::new(budget);

        tracker.consume(resource, 2).unwrap();
        let error = tracker.consume(resource, 1).unwrap_err();
        let amount = tracker.amount(resource);

        assert_eq!(error.code(), resource.exhaustion_code());
        assert_eq!(amount.initial, 2);
        assert_eq!(amount.consumed, 2);
        assert_eq!(amount.remaining, 0);
        assert!(amount.exhausted);
    }
}

#[test]
fn budget_observed_resources_use_bounded_high_water_marks() {
    for resource in [
        BudgetResource::FileBytes,
        BudgetResource::NestingDepth,
        BudgetResource::MatchesPerPattern,
    ] {
        let mut budget = ImpactBudget::fast_defaults();
        budget.max_file_bytes = 3;
        budget.max_nesting_depth = 3;
        budget.max_matches_per_pattern = 3;
        let mut tracker = BudgetTracker::new(budget);

        tracker.observe(resource, 2).unwrap();
        let error = tracker.observe(resource, usize::MAX).unwrap_err();
        let amount = tracker.amount(resource);

        assert_eq!(error.code(), resource.exhaustion_code());
        assert_eq!(amount.initial, 3);
        assert_eq!(amount.consumed, 3);
        assert_eq!(amount.remaining, 0);
        assert!(amount.exhausted);
    }
}

#[test]
fn budget_exhausted_unit_does_not_erase_previously_accepted_facts() {
    let mut budget = ImpactBudget::fast_defaults();
    budget.max_file_bytes = 4;
    budget.max_facts = 10;
    let mut tracker = BudgetTracker::new(budget);
    tracker.consume(BudgetResource::Facts, 3).unwrap();

    tracker.observe(BudgetResource::FileBytes, 5).unwrap_err();

    assert_eq!(tracker.amount(BudgetResource::Facts).consumed, 3);
    assert_eq!(tracker.amount(BudgetResource::Facts).remaining, 7);
}

#[test]
fn budget_deadline_exhaustion_is_stable_and_monotonic() {
    let mut budget = ImpactBudget::fast_defaults();
    budget.deadline = Duration::ZERO;
    let mut tracker = BudgetTracker::new(budget);

    let first = tracker.check_deadline().unwrap_err();
    let second = tracker.check_deadline().unwrap_err();

    assert_eq!(first.code(), "deadline-exhausted");
    assert_eq!(second.code(), "deadline-exhausted");
    assert!(tracker.deadline_exhausted());
}

#[test]
fn tree_sitter_clean_fixture_selects_enclosing_changed_function() {
    let source = include_bytes!("fixtures/impact_context/rust-clean.rs");
    let source_text = std::str::from_utf8(source).unwrap();
    let changed_line = source_text
        .lines()
        .position(|line| line.contains("helper(value)"))
        .map(|line| line as u32 + 1)
        .unwrap();
    let changed_ranges = [ChangedRange {
        start_line: changed_line,
        end_line: changed_line,
        deletion_anchor: false,
    }];
    let mut tracker = BudgetTracker::new(ImpactBudget::fast_defaults());

    let output = TreeSitterRustAdapter::analyze(source, &changed_ranges, &mut tracker).unwrap();

    assert_eq!(output.parse_quality, ParseQuality::Clean);
    let process = output
        .changed_symbols
        .iter()
        .find(|symbol| symbol.name == "process")
        .expect("body hunk must select its enclosing function");
    assert!(process.owner.as_deref().unwrap().contains("Service"));
    assert!(process.signature.contains("pub async fn process<U>"));
    assert!(output
        .imports
        .iter()
        .any(|fact| fact.text.contains("HashMap as Map")));
    assert!(output
        .imports
        .iter()
        .any(|fact| fact.text.contains("prelude::*")));
    assert!(output.calls.iter().all(|call| {
        matches!(
            call.resolution,
            Resolution::Syntactic | Resolution::Unresolved
        )
    }));
    assert!(output.calls.iter().any(|call| call.target == "helper"));
    assert!(output
        .macros
        .iter()
        .any(|fact| fact.text == "tracing::debug"));
}

#[test]
fn tree_sitter_clean_fixture_matches_the_structural_golden() {
    let source = include_bytes!("fixtures/impact_context/rust-clean.rs");
    let changed_line = std::str::from_utf8(source)
        .unwrap()
        .lines()
        .position(|line| line.contains("helper(value)"))
        .map(|line| line as u32 + 1)
        .unwrap();
    let mut tracker = BudgetTracker::new(ImpactBudget::fast_defaults());
    let output = TreeSitterRustAdapter::analyze(
        source,
        &[ChangedRange {
            start_line: changed_line,
            end_line: changed_line,
            deletion_anchor: false,
        }],
        &mut tracker,
    )
    .unwrap();
    let projection = json!({
        "parse_quality": output.parse_quality,
        "changed_symbols": output.changed_symbols.iter().map(|symbol| json!({
            "kind": symbol.kind,
            "name": symbol.name,
            "owner": symbol.owner,
        })).collect::<Vec<_>>(),
        "calls": output.calls.iter().map(|call| json!([
            call.target,
            call.resolution,
        ])).collect::<Vec<_>>(),
        "macros": output.macros.iter().map(|fact| fact.text.as_str()).collect::<Vec<_>>(),
        "limitation_codes": output.limitation_codes,
    });
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/impact_context/rust-clean.expected.json"
    ))
    .unwrap();

    assert_eq!(projection, expected);
}

#[test]
fn tree_sitter_recovery_quality_tracks_changed_structure_overlap() {
    let source = include_bytes!("fixtures/impact_context/rust-recovered.rs");
    let source_text = std::str::from_utf8(source).unwrap();
    let line = |needle: &str| {
        source_text
            .lines()
            .position(|line| line.contains(needle))
            .map(|line| line as u32 + 1)
            .unwrap()
    };
    let analyze = |changed_line| {
        let mut tracker = BudgetTracker::new(ImpactBudget::fast_defaults());
        TreeSitterRustAdapter::analyze(
            source,
            &[ChangedRange {
                start_line: changed_line,
                end_line: changed_line,
                deletion_anchor: false,
            }],
            &mut tracker,
        )
        .unwrap()
    };
    let stable = analyze(line("pub fn stable"));
    let degraded = analyze(line("let next = @"));
    let projection = json!({
        "stable": {
            "parse_quality": stable.parse_quality,
            "limitation_codes": stable.limitation_codes,
        },
        "degraded": {
            "parse_quality": degraded.parse_quality,
            "limitation_codes": degraded.limitation_codes,
        }
    });
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/impact_context/rust-recovered.expected.json"
    ))
    .unwrap();

    assert!(stable.error_node_count + stable.missing_node_count > 0);
    assert!(degraded.error_node_count + degraded.missing_node_count > 0);
    assert_eq!(projection, expected);
}

#[test]
fn tree_sitter_malformed_and_deeply_nested_input_never_panics() {
    let mut malformed_budget = ImpactBudget::fast_defaults();
    malformed_budget.max_nesting_depth = 16;
    let mut tracker = BudgetTracker::new(malformed_budget);
    let mut source = b"fn hostile() {".to_vec();
    source.extend(std::iter::repeat_n(b'{', 600));
    source.push(0xff);
    source.extend(std::iter::repeat_n(b'}', 600));

    let output = TreeSitterRustAdapter::analyze(
        &source,
        &[ChangedRange {
            start_line: 1,
            end_line: 1,
            deletion_anchor: false,
        }],
        &mut tracker,
    )
    .unwrap();

    assert!(output
        .limitation_codes
        .iter()
        .any(|code| code == "nesting-depth-budget-exhausted"));
    assert!(output.nodes_visited <= tracker.amount(BudgetResource::Nodes).initial);
}

#[test]
fn tree_sitter_extracts_declared_rust_structure_without_expansion() {
    let source = include_bytes!("fixtures/impact_context/rust-clean.rs");
    let line_count = std::str::from_utf8(source).unwrap().lines().count() as u32;
    let mut tracker = BudgetTracker::new(ImpactBudget::fast_defaults());

    let output = TreeSitterRustAdapter::analyze(
        source,
        &[ChangedRange {
            start_line: 1,
            end_line: line_count,
            deletion_anchor: false,
        }],
        &mut tracker,
    )
    .unwrap();

    assert!(output
        .changed_symbols
        .iter()
        .any(|symbol| symbol.kind == "struct" && symbol.name == "Service"));
    assert!(output
        .changed_symbols
        .iter()
        .any(|symbol| symbol.kind == "enum" && symbol.name == "Mode"));
    assert!(output
        .changed_symbols
        .iter()
        .any(|symbol| symbol.kind == "trait" && symbol.name == "Runner"));
    assert!(output.changed_symbols.iter().any(|symbol| {
        symbol.kind == "function-declaration"
            && symbol.name == "run"
            && symbol.owner.as_deref() == Some("Runner")
    }));
    assert!(output.changed_symbols.iter().any(|symbol| {
        symbol.kind == "method"
            && symbol.name == "new"
            && symbol
                .owner
                .as_deref()
                .is_some_and(|owner| owner.contains("Service"))
    }));
    assert!(output
        .changed_symbols
        .iter()
        .any(|symbol| symbol.kind == "closure" && symbol.name.starts_with("<closure@")));
    for attribute in ["#[derive(Debug)]", "#[inline]", "#[test]", "#[ignore]"] {
        assert!(output.attributes.iter().any(|fact| fact.text == attribute));
    }
    assert_eq!(
        output
            .macros
            .iter()
            .filter(|fact| fact.text == "tracing::debug")
            .count(),
        1
    );
    assert!(output
        .calls
        .iter()
        .all(|call| call.resolution == Resolution::Unresolved));
}

#[test]
fn text_adapter_loads_candidate_configuration_and_emits_textual_facts() {
    let config = include_bytes!("fixtures/impact_context/config.toml");
    let candidate = MemoryCandidate::new(&[
        ("config.toml", config, true),
        (
            ".pre-commit-review/context-queries",
            b"postgres://[^\\s]+\n# ignored\n",
            false,
        ),
        (
            ".pre-commit-review/test-hints",
            b"service-config\tconfig\\.toml$\t\tconfiguration\tpostgres\thigh\tReview service configuration\n",
            false,
        ),
    ]);
    let mut tracker = BudgetTracker::new(ImpactBudget::fast_defaults());
    let configuration = TextAdapter::load_configuration(&candidate, &mut tracker).unwrap();

    let output = TextAdapter::scan(
        &RepoPath::new("config.toml").unwrap(),
        config,
        false,
        &configuration,
        &mut tracker,
    );

    assert!(configuration
        .limitation_codes
        .iter()
        .any(|code| code == "text-query-scope-changed-files"));
    assert!(output
        .facts
        .iter()
        .any(|fact| fact.kind == TextFactKind::ConfiguredQuery));
    assert!(output
        .facts
        .iter()
        .any(|fact| fact.kind == TextFactKind::Configuration));
    assert!(output
        .facts
        .iter()
        .any(|fact| fact.kind == TextFactKind::Storage));
    assert!(output
        .facts
        .iter()
        .any(|fact| fact.kind == TextFactKind::Network));
    assert!(output
        .facts
        .iter()
        .any(|fact| fact.kind == TextFactKind::TestHint));
    assert!(output.facts.iter().all(|fact| {
        fact.provenance == TextProvenance::Textual && fact.resolved_target.is_none()
    }));
}

#[test]
fn text_adapter_covers_configuration_and_marker_file_types() {
    let candidate = MemoryCandidate::new(&[]);
    let mut tracker = BudgetTracker::new(ImpactBudget::fast_defaults());
    let configuration = TextAdapter::load_configuration(&candidate, &mut tracker).unwrap();
    let cases: &[(&str, &[u8], TextFactKind)] = &[
        (
            "service.yaml",
            b"database: postgres\nauthorization: bearer\n",
            TextFactKind::Configuration,
        ),
        (
            "Dockerfile",
            include_bytes!("fixtures/impact_context/Dockerfile"),
            TextFactKind::Lifecycle,
        ),
        (
            "schema.sql",
            b"CREATE TABLE sessions (token TEXT);\n",
            TextFactKind::Configuration,
        ),
        (
            "notes.custom",
            b"authorization token sent over grpc network\n",
            TextFactKind::Authorization,
        ),
        (
            "src/lib.rs",
            b"#[test]\nfn api_test() { let endpoint = \"/api/login\"; let cache = \"redis\"; }\n",
            TextFactKind::TestMarker,
        ),
    ];

    for (path, source, expected_kind) in cases {
        let output = TextAdapter::scan(
            &RepoPath::new(*path).unwrap(),
            source,
            false,
            &configuration,
            &mut tracker,
        );
        assert!(
            output.facts.iter().any(|fact| fact.kind == *expected_kind),
            "missing {expected_kind:?} fact for {path}"
        );
        assert!(output.facts.iter().all(|fact| {
            fact.provenance == TextProvenance::Textual && fact.resolved_target.is_none()
        }));
    }
}

#[test]
fn text_adapter_bounds_invalid_queries_query_count_and_matches() {
    let candidate = MemoryCandidate::new(&[(
        ".pre-commit-review/context-queries",
        b"[\nneedle\nthird\n",
        false,
    )]);
    let mut budget = ImpactBudget::fast_defaults();
    budget.max_query_patterns = 2;
    budget.max_matches_per_pattern = 2;
    let mut tracker = BudgetTracker::new(budget);

    let configuration = TextAdapter::load_configuration(&candidate, &mut tracker).unwrap();
    let output = TextAdapter::scan(
        &RepoPath::new("notes.txt").unwrap(),
        b"needle needle needle",
        false,
        &configuration,
        &mut tracker,
    );

    assert!(configuration
        .limitation_codes
        .iter()
        .any(|code| code == "invalid-text-query"));
    assert!(configuration
        .limitation_codes
        .iter()
        .any(|code| code == "query-pattern-budget-exhausted"));
    assert!(output
        .limitation_codes
        .iter()
        .any(|code| code == "query-match-budget-exhausted"));
    assert_eq!(
        output
            .facts
            .iter()
            .filter(|fact| fact.kind == TextFactKind::ConfiguredQuery)
            .count(),
        2
    );
}

#[test]
fn text_adapter_binary_and_syntax_budget_states_remain_independent() {
    let candidate = MemoryCandidate::new(&[]);
    let mut budget = ImpactBudget::fast_defaults();
    budget.max_nodes = 1;
    let mut tracker = BudgetTracker::new(budget);
    let configuration = TextAdapter::load_configuration(&candidate, &mut tracker).unwrap();

    tracker.consume(BudgetResource::Nodes, 1).unwrap();
    tracker.consume(BudgetResource::Nodes, 1).unwrap_err();
    let text = TextAdapter::scan(
        &RepoPath::new("src/lib.rs").unwrap(),
        b"fn value() { let token = \"jwt\"; }",
        false,
        &configuration,
        &mut tracker,
    );
    let binary = TextAdapter::scan(
        &RepoPath::new("binary.bin").unwrap(),
        b"binary\0payload",
        true,
        &configuration,
        &mut tracker,
    );

    assert!(text
        .facts
        .iter()
        .any(|fact| fact.kind == TextFactKind::Authorization));
    assert_eq!(
        binary.status,
        collect_diff_context_cli::impact_context::contracts::UnitStatus::Unsupported
    );
    assert!(binary.facts.is_empty());
    assert_eq!(binary.limitation_codes, vec!["binary-text-unavailable"]);
}
