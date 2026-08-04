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
use collect_diff_context_cli::impact_context::contracts::{
    ImpactContext, ImpactMode, ImpactPresence, ImpactStatus, ParseQuality, ProviderStatus,
    Resolution, SourceRange, UnitStatus,
};
use collect_diff_context_cli::impact_context::engine::{
    build_impact_context, enforce_presentation_budget, ImpactRequest,
};
use collect_diff_context_cli::impact_context::normalizer::{
    merge_normalized_units, normalize_unit,
};
use collect_diff_context_cli::impact_context::summarizer::summarize_unit;
use collect_diff_context_cli::review_scope::ReviewSource;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
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

    fn read_bounded(
        &self,
        path: &RepoPath,
        max_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError> {
        let source = self
            .contents
            .get(path.as_str())
            .expect("memory candidate path must exist");
        if source.len() > max_bytes {
            return Err(CandidateError::byte_limit_exceeded(path, max_bytes));
        }
        let bytes = source.clone();
        Ok(CandidateBytes {
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            binary: bytes.iter().take(8192).any(|byte| *byte == 0),
            bytes,
        })
    }
}

struct TrackingCandidate {
    inner: MemoryCandidate,
    reads: RefCell<Vec<String>>,
}

struct UnreadableConfigCandidate {
    inner: MemoryCandidate,
}

struct UnreadableCandidate {
    files: Vec<CandidateFile>,
}

impl CandidateContent for UnreadableCandidate {
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

    fn read_bounded(
        &self,
        _path: &RepoPath,
        _max_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError> {
        Err(RepoPath::new("").unwrap_err())
    }
}

impl TrackingCandidate {
    fn new(inner: MemoryCandidate) -> Self {
        Self {
            inner,
            reads: RefCell::new(Vec::new()),
        }
    }
}

impl CandidateContent for TrackingCandidate {
    fn scope_fingerprint(&self) -> &str {
        self.inner.scope_fingerprint()
    }

    fn candidate_digest(&self) -> &str {
        self.inner.candidate_digest()
    }

    fn source(&self) -> ReviewSource {
        self.inner.source()
    }

    fn files(&self) -> &[CandidateFile] {
        self.inner.files()
    }

    fn read_bounded(
        &self,
        path: &RepoPath,
        max_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError> {
        self.reads.borrow_mut().push(path.as_str().to_string());
        self.inner.read_bounded(path, max_bytes)
    }
}

impl CandidateContent for UnreadableConfigCandidate {
    fn scope_fingerprint(&self) -> &str {
        self.inner.scope_fingerprint()
    }

    fn candidate_digest(&self) -> &str {
        self.inner.candidate_digest()
    }

    fn source(&self) -> ReviewSource {
        self.inner.source()
    }

    fn files(&self) -> &[CandidateFile] {
        self.inner.files()
    }

    fn read_bounded(
        &self,
        path: &RepoPath,
        max_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError> {
        if path.as_str().starts_with(".pre-commit-review/") {
            return Err(RepoPath::new("").unwrap_err());
        }
        self.inner.read_bounded(path, max_bytes)
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
fn tree_sitter_adapter_honors_an_exhausted_deadline() {
    let source = b"pub fn changed() {}\n";
    let mut budget = ImpactBudget::fast_defaults();
    budget.deadline = Duration::ZERO;
    let mut tracker = BudgetTracker::new(budget);

    let error = TreeSitterRustAdapter::analyze(
        source,
        &[ChangedRange {
            start_line: 1,
            end_line: 1,
            deletion_anchor: false,
        }],
        &mut tracker,
    )
    .unwrap_err();

    assert_eq!(error.to_string(), "deadline-exhausted");
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
fn adversarial_tree_sitter_malformed_and_deeply_nested_input_never_panics() {
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

fn assert_range_within_input(range: &SourceRange, input_bytes: usize) {
    assert!(range.start_line > 0);
    assert!(range.start_column > 0);
    assert!(range.end_line > 0);
    assert!(range.end_column > 0);
    assert!(range.start_byte <= range.end_byte);
    assert!(range.end_byte <= input_bytes);
}

#[test]
fn adversarial_tree_sitter_ranges_and_counts_remain_bounded() {
    let mut source = b"pub fn hostile() { let value = \"".to_vec();
    source.extend(std::iter::repeat_n(b'a', 32_768));
    source.push(0xff);
    source.extend_from_slice(b"\"; value(); }");
    let mut budget = ImpactBudget::fast_defaults();
    budget.max_nodes = 128;
    budget.max_nesting_depth = 16;
    budget.max_facts = 16;
    budget.max_edges = 8;
    let limits = budget.clone();
    let mut tracker = BudgetTracker::new(budget);

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

    for range in &output.affected_ranges {
        assert_range_within_input(range, source.len());
    }
    for range in output
        .changed_symbols
        .iter()
        .map(|fact| &fact.range)
        .chain(output.imports.iter().map(|fact| &fact.range))
        .chain(output.macros.iter().map(|fact| &fact.range))
        .chain(output.attributes.iter().map(|fact| &fact.range))
        .chain(output.calls.iter().map(|fact| &fact.range))
    {
        assert_range_within_input(range, source.len());
    }
    let fact_count = output.changed_symbols.len()
        + output.imports.len()
        + output.calls.len()
        + output.macros.len()
        + output.attributes.len();
    assert!(output.nodes_visited <= limits.max_nodes);
    assert!(output.max_nesting_depth <= limits.max_nesting_depth);
    assert!(fact_count <= limits.max_facts);
    assert!(output.calls.len() <= limits.max_edges);
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
fn text_adapter_rejects_oversized_test_hint_patterns_and_uses_first_match() {
    let oversized = "x".repeat(501);
    let hints = format!(
        "oversized\t{oversized}\t\tunit\tnone\tlow\tignored\nfirst\tnotes\\.txt$\t\tunit\tnone\thigh\tfirst hint\nsecond\tnotes\\.txt$\t\tunit\tnone\thigh\tsecond hint\n"
    );
    let candidate =
        MemoryCandidate::new(&[(".pre-commit-review/test-hints", hints.as_bytes(), false)]);
    let mut tracker = BudgetTracker::new(ImpactBudget::fast_defaults());

    let configuration = TextAdapter::load_configuration(&candidate, &mut tracker).unwrap();
    let output = TextAdapter::scan(
        &RepoPath::new("notes.txt").unwrap(),
        b"plain text",
        false,
        &configuration,
        &mut tracker,
    );

    assert!(configuration
        .limitation_codes
        .iter()
        .any(|code| code == "invalid-test-hint"));
    let hints = output
        .facts
        .iter()
        .filter(|fact| fact.kind == TextFactKind::TestHint)
        .collect::<Vec<_>>();
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].rule_id, "first");
    assert_eq!(hints[0].match_text, "first hint");
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

#[test]
fn normalizer_is_deterministic_and_preserves_unresolved_calls() {
    let source = include_bytes!("fixtures/impact_context/rust-clean.rs");
    let changed_line = std::str::from_utf8(source)
        .unwrap()
        .lines()
        .position(|line| line.contains("helper(value)"))
        .map(|line| line as u32 + 1)
        .unwrap();
    let analyze = || {
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

    let first = normalize_unit(
        "src/lib.rs",
        "rust",
        "1111111111111111",
        "2222222222222222",
        Some(&analyze()),
        None,
    );
    let second = normalize_unit(
        "src/lib.rs",
        "rust",
        "1111111111111111",
        "2222222222222222",
        Some(&analyze()),
        None,
    );

    assert_eq!(first, second);
    assert!(first
        .changed_symbols
        .windows(2)
        .all(|pair| pair[0].symbol_id < pair[1].symbol_id));
    assert!(first
        .impact_edges
        .windows(2)
        .all(|pair| pair[0].edge_id < pair[1].edge_id));
    assert!(first
        .impact_edges
        .iter()
        .filter(|edge| edge.kind
            == collect_diff_context_cli::impact_context::contracts::EdgeKind::Calls)
        .all(|edge| {
            edge.to_symbol.is_none()
                && edge.unresolved_target.is_some()
                && edge.resolution == Resolution::Unresolved
        }));
}

#[test]
fn normalizer_disambiguates_same_named_callers_by_source_range() {
    let source = br#"
mod first {
    fn run() { first_target(); }
}
mod second {
    fn run() { second_target(); }
}
"#;
    let mut tracker = BudgetTracker::new(ImpactBudget::fast_defaults());
    let syntax = TreeSitterRustAdapter::analyze(
        source,
        &[ChangedRange {
            start_line: 1,
            end_line: 8,
            deletion_anchor: false,
        }],
        &mut tracker,
    )
    .unwrap();
    let normalized = normalize_unit(
        "src/lib.rs",
        "rust",
        "1111111111111111",
        "2222222222222222",
        Some(&syntax),
        None,
    );
    let callers = normalized
        .impact_edges
        .iter()
        .filter(|edge| {
            matches!(
                edge.unresolved_target.as_deref(),
                Some("first_target" | "second_target")
            )
        })
        .map(|edge| edge.from_symbol.as_str())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(callers.len(), 2);
    assert!(callers.iter().all(|caller| normalized
        .changed_symbols
        .iter()
        .any(|symbol| symbol.symbol_id == **caller)));
}

#[test]
fn normalizer_dedupes_and_preserves_higher_confidence_claims() {
    let source = include_bytes!("fixtures/impact_context/rust-clean.rs");
    let line_count = std::str::from_utf8(source).unwrap().lines().count() as u32;
    let mut tracker = BudgetTracker::new(ImpactBudget::fast_defaults());
    let syntax = TreeSitterRustAdapter::analyze(
        source,
        &[ChangedRange {
            start_line: 1,
            end_line: line_count,
            deletion_anchor: false,
        }],
        &mut tracker,
    )
    .unwrap();
    let high = normalize_unit(
        "src/lib.rs",
        "rust",
        "1111111111111111",
        "2222222222222222",
        Some(&syntax),
        None,
    );
    let mut low = high.clone();
    low.changed_symbols[0].confidence =
        collect_diff_context_cli::impact_context::contracts::Confidence::Low;
    low.changed_symbols[0].signature = Some("low confidence replacement".to_string());

    let merged = merge_normalized_units("src/lib.rs", [low, high.clone(), high.clone()]);

    assert_eq!(merged.changed_symbols.len(), high.changed_symbols.len());
    assert_eq!(merged.impact_edges.len(), high.impact_edges.len());
    assert_eq!(merged.facts.len(), high.facts.len());
    assert_eq!(
        merged.changed_symbols[0].signature,
        high.changed_symbols[0].signature
    );
}

#[test]
fn normalizer_keeps_text_occurrences_out_of_symbol_edges_and_ids_out_of_snippets() {
    let candidate = MemoryCandidate::new(&[]);
    let mut first_tracker = BudgetTracker::new(ImpactBudget::fast_defaults());
    let configuration = TextAdapter::load_configuration(&candidate, &mut first_tracker).unwrap();
    let mut first_text = TextAdapter::scan(
        &RepoPath::new("settings.txt").unwrap(),
        b"authorization token",
        false,
        &configuration,
        &mut first_tracker,
    );
    let first = normalize_unit(
        "settings.txt",
        "text",
        "1111111111111111",
        "2222222222222222",
        None,
        Some(&first_text),
    );
    first_text.facts[0].match_text = "redacted replacement".to_string();
    let second = normalize_unit(
        "settings.txt",
        "text",
        "1111111111111111",
        "2222222222222222",
        None,
        Some(&first_text),
    );

    assert!(first.impact_edges.is_empty());
    assert!(first.facts.iter().all(|fact| fact.provenance == "textual"));
    assert_eq!(
        first
            .facts
            .iter()
            .map(|fact| &fact.fact_id)
            .collect::<Vec<_>>(),
        second
            .facts
            .iter()
            .map(|fact| &fact.fact_id)
            .collect::<Vec<_>>()
    );
}

#[test]
fn summarizer_emits_bounded_deterministic_domain_summaries() {
    let source = include_bytes!("fixtures/impact_context/rust-clean.rs");
    let line_count = std::str::from_utf8(source).unwrap().lines().count() as u32;
    let mut syntax_tracker = BudgetTracker::new(ImpactBudget::fast_defaults());
    let syntax = TreeSitterRustAdapter::analyze(
        source,
        &[ChangedRange {
            start_line: 1,
            end_line: line_count,
            deletion_anchor: false,
        }],
        &mut syntax_tracker,
    )
    .unwrap();
    let candidate = MemoryCandidate::new(&[(
        ".pre-commit-review/context-queries",
        b"authorization\n",
        false,
    )]);
    let mut text_tracker = BudgetTracker::new(ImpactBudget::fast_defaults());
    let configuration = TextAdapter::load_configuration(&candidate, &mut text_tracker).unwrap();
    let text_source = b"#[test]\n#[ignore]\nfn api_test() { let authorization = \"jwt\"; let database = \"postgres\"; let endpoint = \"https://api.test\"; let lifecycle = \"shutdown\"; }";
    let text = TextAdapter::scan(
        &RepoPath::new("tests/api_test.rs").unwrap(),
        text_source,
        false,
        &configuration,
        &mut text_tracker,
    );
    let normalized = normalize_unit(
        "tests/api_test.rs",
        "rust",
        "1111111111111111",
        "2222222222222222",
        Some(&syntax),
        Some(&text),
    );

    let summaries = summarize_unit(&normalized, Some(std::str::from_utf8(text_source).unwrap()));
    let kinds = summaries
        .iter()
        .map(|summary| summary.summary_kind)
        .collect::<std::collections::BTreeSet<_>>();

    for expected in [
        collect_diff_context_cli::impact_context::contracts::SummaryKind::InterfaceChange,
        collect_diff_context_cli::impact_context::contracts::SummaryKind::DependencyChange,
        collect_diff_context_cli::impact_context::contracts::SummaryKind::TextQueryMatch,
        collect_diff_context_cli::impact_context::contracts::SummaryKind::TestSelection,
        collect_diff_context_cli::impact_context::contracts::SummaryKind::AuthorizationEffect,
        collect_diff_context_cli::impact_context::contracts::SummaryKind::StorageEffect,
        collect_diff_context_cli::impact_context::contracts::SummaryKind::NetworkEffect,
        collect_diff_context_cli::impact_context::contracts::SummaryKind::LifecycleEffect,
    ] {
        assert!(
            kinds.contains(&expected),
            "missing summary kind {expected:?}"
        );
    }
    assert!(summaries
        .windows(2)
        .all(|pair| pair[0].summary_id < pair[1].summary_id));
    assert!(summaries.iter().all(|summary| {
        summary.message.chars().count() <= 1_000
            && summary
                .evidence_fact_ids
                .windows(2)
                .all(|pair| pair[0] < pair[1])
            && !summary.message.to_ascii_lowercase().contains("verdict")
            && !summary.message.to_ascii_lowercase().contains("reviewed")
            && !summary.message.contains("cargo test")
    }));
}

#[test]
fn engine_clean_rust_candidate_produces_completed_valid_context() {
    let source = include_bytes!("fixtures/impact_context/rust-clean.rs");
    let mut candidate = MemoryCandidate::new(&[("src/lib.rs", source, true)]);
    candidate.files[0].changed_ranges = vec![ChangedRange {
        start_line: 1,
        end_line: std::str::from_utf8(source).unwrap().lines().count() as u32,
        deletion_anchor: false,
    }];

    let context = build_impact_context(&candidate, ImpactRequest::fast_defaults()).unwrap();

    context.validate().unwrap();
    assert_eq!(
        context.status,
        collect_diff_context_cli::impact_context::contracts::ImpactStatus::Completed
    );
    assert_eq!(context.units.len(), 1);
    assert_eq!(context.units[0].manifest_unit_id, "file:src/lib.rs");
    assert_eq!(context.coverage.changed_candidate_files, 1);
    assert_eq!(context.coverage.parsed_files, 1);
    assert!(!context.changed_symbols.is_empty());
    assert!(!context.impact_edges.is_empty());
}

#[test]
fn engine_reports_file_byte_budget_exhaustion_independently() {
    let source = b"pub fn changed() { println!(\"changed\"); }\n";
    let mut candidate = MemoryCandidate::new(&[("src/lib.rs", source, true)]);
    candidate.files[0].changed_ranges = vec![ChangedRange {
        start_line: 1,
        end_line: 1,
        deletion_anchor: false,
    }];
    let mut request = ImpactRequest::fast_defaults();
    request.budget.max_file_bytes = source.len() - 1;
    request.budget.max_total_bytes = source.len() * 2;

    let context = build_impact_context(&candidate, request).unwrap();

    context.validate().unwrap();
    assert_eq!(context.status, ImpactStatus::Unavailable);
    assert_eq!(context.units[0].syntax_status, UnitStatus::BudgetExhausted);
    assert!(context
        .limitations
        .iter()
        .any(|limitation| limitation.code == "file-byte-budget-exhausted"));
    assert!(!context
        .limitations
        .iter()
        .any(|limitation| limitation.code == "total-byte-budget-exhausted"));
}

#[test]
fn engine_mixed_rust_and_configuration_context_is_partial() {
    let rust = b"pub fn changed() { println!(\"changed\"); }\n";
    let config = b"database: postgres\nauthorization: bearer\n";
    let mut candidate = MemoryCandidate::new(&[
        ("src/lib.rs", rust, true),
        ("config/service.yaml", config, true),
    ]);
    for file in &mut candidate.files {
        file.changed_ranges = vec![ChangedRange {
            start_line: 1,
            end_line: 2,
            deletion_anchor: false,
        }];
    }

    let context = build_impact_context(&candidate, ImpactRequest::fast_defaults()).unwrap();

    context.validate().unwrap();
    assert_eq!(context.status, ImpactStatus::Partial);
    assert_eq!(context.coverage.changed_candidate_files, 2);
    assert_eq!(context.coverage.syntax_eligible_files, 1);
    assert_eq!(context.coverage.parsed_files, 1);
    assert_eq!(context.coverage.unsupported_files, 1);
    assert!(context
        .domain_summaries
        .iter()
        .any(|summary| summary.path == "config/service.yaml"));
}

#[test]
fn engine_unsupported_only_context_is_unavailable() {
    let mut candidate = MemoryCandidate::new(&[("notes.custom", b"plain prose\n", true)]);
    candidate.files[0].changed_ranges = vec![ChangedRange {
        start_line: 1,
        end_line: 1,
        deletion_anchor: false,
    }];

    let context = build_impact_context(&candidate, ImpactRequest::fast_defaults()).unwrap();

    context.validate().unwrap();
    assert_eq!(context.status, ImpactStatus::Unavailable);
    assert_eq!(context.coverage.unsupported_files, 1);
    assert!(context.changed_symbols.is_empty());
    assert!(context.impact_edges.is_empty());
    assert!(context.domain_summaries.is_empty());
}

#[test]
fn engine_deleted_rust_unit_retains_removal_limitation() {
    let mut candidate = MemoryCandidate::new(&[]);
    candidate.files.push(CandidateFile {
        path: RepoPath::new("src/removed.rs").unwrap(),
        mode: "000000".to_string(),
        content_identity: None,
        presence: CandidatePresence::Deleted,
        manifest_unit_id: Some("file:src/removed.rs".to_string()),
        change_status: Some("D".to_string()),
        changed_ranges: vec![ChangedRange {
            start_line: 8,
            end_line: 8,
            deletion_anchor: true,
        }],
    });

    let context = build_impact_context(&candidate, ImpactRequest::fast_defaults()).unwrap();

    context.validate().unwrap();
    assert_eq!(context.status, ImpactStatus::Unavailable);
    assert_eq!(context.units[0].presence, ImpactPresence::Deleted);
    assert_eq!(context.units[0].changed_ranges[0].start_line, 8);
    assert!(context
        .limitations
        .iter()
        .any(|limitation| limitation.code == "removed-structure-unavailable-in-fast-mvp"));
}

#[test]
fn engine_retains_special_units_without_structural_coverage_credit() {
    let source = b"pub fn generated() {}\n";
    let mut candidate = MemoryCandidate::new(&[
        ("generated/api.rs", source, true),
        ("vendor/dependency.rs", source, true),
        ("dist/app.min.js", b"function bundled(){}\n", true),
        ("assets/data.bin", b"binary\0payload", true),
        ("src/mode_only.rs", source, true),
    ]);
    for file in &mut candidate.files {
        if file.path.as_str() != "src/mode_only.rs" {
            file.changed_ranges = vec![ChangedRange {
                start_line: 1,
                end_line: 1,
                deletion_anchor: false,
            }];
        }
    }
    candidate.files.push(CandidateFile {
        path: RepoPath::new("src/deleted.rs").unwrap(),
        mode: "000000".to_string(),
        content_identity: None,
        presence: CandidatePresence::Deleted,
        manifest_unit_id: Some("file:src/deleted.rs".to_string()),
        change_status: Some("D".to_string()),
        changed_ranges: vec![ChangedRange {
            start_line: 1,
            end_line: 1,
            deletion_anchor: true,
        }],
    });
    candidate.files.push(CandidateFile {
        path: RepoPath::new("third_party/module").unwrap(),
        mode: "160000".to_string(),
        content_identity: Some("0123456789012345678901234567890123456789".to_string()),
        presence: CandidatePresence::Gitlink,
        manifest_unit_id: Some("file:third_party/module".to_string()),
        change_status: Some("M".to_string()),
        changed_ranges: Vec::new(),
    });
    candidate
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));

    let context = build_impact_context(&candidate, ImpactRequest::fast_defaults()).unwrap();

    context.validate().unwrap();
    assert_eq!(context.units.len(), 7);
    assert_eq!(context.coverage.syntax_eligible_files, 0);
    assert_eq!(context.coverage.parsed_files, 0);
    let codes = context
        .limitations
        .iter()
        .map(|limitation| limitation.code.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for code in [
        "generated-like-structure-skipped",
        "vendored-structure-skipped",
        "minified-structure-skipped",
        "binary-structure-unavailable",
        "mode-only-no-structural-range",
        "removed-structure-unavailable-in-fast-mvp",
        "gitlink-structure-unavailable",
    ] {
        assert!(codes.contains(code), "missing limitation {code}");
    }
}

#[test]
fn engine_changed_file_budget_retains_later_units_as_limited() {
    let source = b"pub fn changed() {}\n";
    let mut candidate =
        MemoryCandidate::new(&[("src/a.rs", source, true), ("src/b.rs", source, true)]);
    for file in &mut candidate.files {
        file.changed_ranges = vec![ChangedRange {
            start_line: 1,
            end_line: 1,
            deletion_anchor: false,
        }];
    }
    let mut request = ImpactRequest::fast_defaults();
    request.budget.max_changed_files = 1;

    let context = build_impact_context(&candidate, request).unwrap();

    context.validate().unwrap();
    assert_eq!(context.units.len(), 2);
    assert_eq!(context.units[0].syntax_status, UnitStatus::Completed);
    assert_eq!(context.units[1].syntax_status, UnitStatus::BudgetExhausted);
    assert_eq!(context.coverage.resource_limited_files, 1);
}

#[test]
fn engine_node_and_deadline_budgets_are_visible() {
    let source = include_bytes!("fixtures/impact_context/rust-clean.rs");
    let make_candidate = || {
        let mut candidate = MemoryCandidate::new(&[("src/lib.rs", source, true)]);
        candidate.files[0].changed_ranges = vec![ChangedRange {
            start_line: 1,
            end_line: 20,
            deletion_anchor: false,
        }];
        candidate
    };

    let mut node_request = ImpactRequest::fast_defaults();
    node_request.budget.max_nodes = 1;
    let node_context = build_impact_context(&make_candidate(), node_request).unwrap();
    node_context.validate().unwrap();
    assert!(node_context
        .limitations
        .iter()
        .any(|limitation| limitation.code == "node-budget-exhausted"));

    let mut deadline_request = ImpactRequest::fast_defaults();
    deadline_request.budget.deadline = Duration::ZERO;
    let deadline_context = build_impact_context(&make_candidate(), deadline_request).unwrap();
    deadline_context.validate().unwrap();
    assert_eq!(deadline_context.units.len(), 1);
    assert_eq!(
        deadline_context.units[0].syntax_status,
        UnitStatus::BudgetExhausted
    );
    assert!(deadline_context
        .limitations
        .iter()
        .any(|limitation| limitation.code == "deadline-exhausted"));
}

#[test]
fn engine_output_truncation_is_bounded_and_deterministic() {
    let source = include_bytes!("fixtures/impact_context/rust-clean.rs");
    let mut candidate = MemoryCandidate::new(&[("src/lib.rs", source, true)]);
    candidate.files[0].changed_ranges = vec![ChangedRange {
        start_line: 1,
        end_line: std::str::from_utf8(source).unwrap().lines().count() as u32,
        deletion_anchor: false,
    }];
    let mut request = ImpactRequest::fast_defaults();
    request.budget.max_output_bytes = 5_000;

    let mut first = build_impact_context(&candidate, request.clone()).unwrap();
    let mut second = build_impact_context(&candidate, request).unwrap();

    first.validate().unwrap();
    second.validate().unwrap();
    assert!(first.coverage.output_truncated);
    assert!(first.metrics.output_bytes <= 5_000);
    assert_eq!(first.units.len(), 1);
    first.metrics.elapsed_ms = 0;
    second.metrics.elapsed_ms = 0;
    for provider in &mut first.providers {
        provider.elapsed_ms = 0;
    }
    for provider in &mut second.providers {
        provider.elapsed_ms = 0;
    }
    for _ in 0..3 {
        first.metrics.output_bytes = serde_json::to_vec(&first).unwrap().len();
        second.metrics.output_bytes = serde_json::to_vec(&second).unwrap().len();
    }
    assert_eq!(first, second);
}

#[test]
fn presentation_selection_is_independent_of_runtime_telemetry() {
    let source = include_bytes!("fixtures/impact_context/rust-clean.rs");
    let mut candidate = MemoryCandidate::new(&[("src/lib.rs", source, true)]);
    candidate.files[0].changed_ranges = vec![ChangedRange {
        start_line: 1,
        end_line: std::str::from_utf8(source).unwrap().lines().count() as u32,
        deletion_anchor: false,
    }];
    let mut baseline = build_impact_context(&candidate, ImpactRequest::fast_defaults()).unwrap();
    baseline.metrics.elapsed_ms = 0;
    for provider in &mut baseline.providers {
        provider.elapsed_ms = 0;
    }
    for _ in 0..3 {
        baseline.metrics.output_bytes = serde_json::to_vec(&baseline).unwrap().len();
    }
    let maximum = baseline.metrics.output_bytes;
    let mut long_running = baseline.clone();
    long_running.metrics.elapsed_ms = u64::MAX;
    for provider in &mut long_running.providers {
        provider.elapsed_ms = u64::MAX;
    }

    enforce_presentation_budget(&mut baseline, maximum).unwrap();
    enforce_presentation_budget(&mut long_running, maximum).unwrap();

    assert_eq!(baseline.changed_symbols, long_running.changed_symbols);
    assert_eq!(baseline.impact_edges, long_running.impact_edges);
    assert_eq!(baseline.domain_summaries, long_running.domain_summaries);
    assert_eq!(
        baseline.coverage.output_truncated,
        long_running.coverage.output_truncated
    );
    assert!(long_running.metrics.output_bytes <= maximum);
}

#[test]
fn engine_rejects_an_output_budget_smaller_than_the_irreducible_contract() {
    let source = b"pub fn changed() {}\n";
    let mut candidate = MemoryCandidate::new(&[("src/lib.rs", source, true)]);
    candidate.files[0].changed_ranges = vec![ChangedRange {
        start_line: 1,
        end_line: 1,
        deletion_anchor: false,
    }];
    let mut request = ImpactRequest::fast_defaults();
    request.budget.max_output_bytes = 1;

    let error = build_impact_context(&candidate, request).unwrap_err();

    assert_eq!(error.code(), "output-budget-too-small");
}

#[test]
fn engine_degrades_unreadable_candidate_configuration_instead_of_aborting() {
    let source = b"pub fn changed() {}\n";
    let mut inner = MemoryCandidate::new(&[
        ("src/lib.rs", source, true),
        (".pre-commit-review/context-queries", b"changed", false),
    ]);
    let changed_file = inner
        .files
        .iter_mut()
        .find(|file| file.path.as_str() == "src/lib.rs")
        .unwrap();
    changed_file.changed_ranges = vec![ChangedRange {
        start_line: 1,
        end_line: 1,
        deletion_anchor: false,
    }];
    let candidate = UnreadableConfigCandidate { inner };

    let context = build_impact_context(&candidate, ImpactRequest::fast_defaults()).unwrap();

    context.validate().unwrap();
    assert!(context
        .limitations
        .iter()
        .any(|limitation| limitation.code == "context-query-config-unavailable"));
    assert_eq!(
        context
            .providers
            .iter()
            .find(|provider| provider.provider_kind == "text-adapter")
            .unwrap()
            .status,
        ProviderStatus::Partial
    );
    assert!(context
        .changed_symbols
        .iter()
        .any(|symbol| symbol.name == "changed"));
}

#[test]
fn engine_applies_file_byte_budget_to_candidate_configuration() {
    let source = b"pub fn changed() {}\n";
    let config = vec![b'x'; 101];
    let mut candidate = MemoryCandidate::new(&[
        ("src/lib.rs", source, true),
        (
            ".pre-commit-review/context-queries",
            config.as_slice(),
            false,
        ),
    ]);
    let changed_file = candidate
        .files
        .iter_mut()
        .find(|file| file.path.as_str() == "src/lib.rs")
        .unwrap();
    changed_file.changed_ranges = vec![ChangedRange {
        start_line: 1,
        end_line: 1,
        deletion_anchor: false,
    }];
    let mut request = ImpactRequest::fast_defaults();
    request.budget.max_file_bytes = 100;

    let context = build_impact_context(&candidate, request).unwrap();

    context.validate().unwrap();
    assert!(context
        .limitations
        .iter()
        .any(|limitation| limitation.code == "file-byte-budget-exhausted"));
    assert!(context
        .domain_summaries
        .iter()
        .all(|summary| summary.summary_kind
            != collect_diff_context_cli::impact_context::contracts::SummaryKind::TextQueryMatch));
}

#[test]
fn engine_reads_only_changed_units_and_candidate_configuration() {
    let source = b"pub fn changed() {}\n";
    let mut inner = MemoryCandidate::new(&[
        ("src/changed.rs", source, true),
        ("src/unchanged.rs", source, false),
    ]);
    inner.files[0].changed_ranges = vec![ChangedRange {
        start_line: 1,
        end_line: 1,
        deletion_anchor: false,
    }];
    let candidate = TrackingCandidate::new(inner);

    build_impact_context(&candidate, ImpactRequest::fast_defaults()).unwrap();

    assert_eq!(candidate.reads.borrow().as_slice(), ["src/changed.rs"]);
}

#[test]
fn engine_allows_deep_but_rejects_fast_writes_and_unknown_semantic_providers() {
    let candidate = MemoryCandidate::new(&[]);

    let mut deep = ImpactRequest::fast_defaults();
    deep.mode = ImpactMode::Deep;
    assert_eq!(
        build_impact_context(&candidate, deep).unwrap().mode,
        ImpactMode::Deep
    );

    let mut cache_write = ImpactRequest::fast_defaults();
    cache_write.cache_write = true;
    assert_eq!(
        build_impact_context(&candidate, cache_write)
            .unwrap_err()
            .code(),
        "cache-write-forbidden"
    );

    let mut semantic = ImpactRequest::fast_defaults();
    semantic
        .semantic_providers
        .push("rust-analyzer".to_string());
    assert_eq!(
        build_impact_context(&candidate, semantic)
            .unwrap_err()
            .code(),
        "semantic-provider-unavailable"
    );
}

#[test]
fn adversarial_engine_ignores_repository_owned_parser_assets() {
    let source = b"pub fn changed() {}\n";
    let mut inner = MemoryCandidate::new(&[
        ("src/lib.rs", source, true),
        (
            ".pre-commit-review/tree-sitter-rust.scm",
            b"(function_item) @execute_repository_query\n",
            false,
        ),
        (
            "tree-sitter.json",
            b"{\"grammars\": [\"repository\"]}\n",
            false,
        ),
        (
            "grammars/libtree-sitter-rust.dylib",
            b"plugin\0payload",
            false,
        ),
        ("scripts/repository-context-hook.sh", b"exit 99\n", false),
    ]);
    inner
        .files
        .iter_mut()
        .find(|file| file.path.as_str() == "src/lib.rs")
        .unwrap()
        .changed_ranges = vec![ChangedRange {
        start_line: 1,
        end_line: 1,
        deletion_anchor: false,
    }];
    let candidate = TrackingCandidate::new(inner);

    let context = build_impact_context(&candidate, ImpactRequest::fast_defaults()).unwrap();

    context.validate().unwrap();
    assert_eq!(candidate.reads.borrow().as_slice(), ["src/lib.rs"]);
    assert!(context
        .changed_symbols
        .iter()
        .any(|symbol| symbol.name == "changed"));
}

#[test]
fn adversarial_engine_bounds_binary_invalid_utf8_long_line_and_large_input() {
    let invalid_utf8 = b"pub fn invalid() { let value = \"\xff\"; }\n";
    let long_line = vec![b'a'; 4_096];
    let mut candidate = MemoryCandidate::new(&[
        ("src/binary.rs", b"pub fn binary() {}\0payload", true),
        ("src/invalid.rs", invalid_utf8, true),
        ("src/large.rs", &long_line, true),
    ]);
    for file in &mut candidate.files {
        file.changed_ranges = vec![ChangedRange {
            start_line: 1,
            end_line: 1,
            deletion_anchor: false,
        }];
    }
    let mut request = ImpactRequest::fast_defaults();
    request.budget.max_file_bytes = 128;
    request.budget.max_total_bytes = 256;
    request.budget.max_nodes = 256;
    request.budget.max_facts = 32;
    request.budget.max_edges = 16;
    let limits = request.budget.clone();

    let context = build_impact_context(&candidate, request).unwrap();

    context.validate().unwrap();
    assert_eq!(context.units.len(), 3);
    assert!(context.metrics.nodes_visited <= limits.max_nodes);
    assert!(context.metrics.facts_emitted <= limits.max_facts);
    assert!(context.metrics.edges_emitted <= limits.max_edges);
    assert!(context.metrics.output_bytes <= limits.max_output_bytes);
    let codes = context
        .limitations
        .iter()
        .map(|limitation| limitation.code.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(codes.contains("binary-structure-unavailable"));
    assert!(codes.contains("file-byte-budget-exhausted"));
    for unit in &context.units {
        for range in &unit.changed_ranges {
            assert_range_within_input(range, unit.content_bytes.unwrap_or(0));
        }
    }
}

#[test]
fn adversarial_engine_enforces_independent_edge_budget() {
    let source = b"pub fn changed() { first(); second(); third(); }\n";
    let mut candidate = MemoryCandidate::new(&[("src/lib.rs", source, true)]);
    candidate.files[0].changed_ranges = vec![ChangedRange {
        start_line: 1,
        end_line: 1,
        deletion_anchor: false,
    }];
    let mut request = ImpactRequest::fast_defaults();
    request.budget.max_facts = 32;
    request.budget.max_edges = 1;

    let context = build_impact_context(&candidate, request).unwrap();

    context.validate().unwrap();
    assert!(context.impact_edges.len() <= 1);
    assert!(context
        .limitations
        .iter()
        .any(|limitation| limitation.code == "edge-budget-exhausted"));
}

#[test]
fn adversarial_contract_fuzz_corpus_seeds_are_valid() {
    for (name, bytes) in [
        (
            "completed",
            include_bytes!("../fuzz/corpus/impact_contract/completed.json").as_slice(),
        ),
        (
            "partial",
            include_bytes!("../fuzz/corpus/impact_contract/partial.json").as_slice(),
        ),
        (
            "unavailable",
            include_bytes!("../fuzz/corpus/impact_contract/unavailable.json").as_slice(),
        ),
        (
            "invalidated",
            include_bytes!("../fuzz/corpus/impact_contract/invalidated.json").as_slice(),
        ),
        (
            "failed",
            include_bytes!("../fuzz/corpus/impact_contract/failed.json").as_slice(),
        ),
        (
            "recovered",
            include_bytes!("../fuzz/corpus/impact_contract/recovered.json").as_slice(),
        ),
        (
            "degraded",
            include_bytes!("../fuzz/corpus/impact_contract/degraded.json").as_slice(),
        ),
        (
            "truncated",
            include_bytes!("../fuzz/corpus/impact_contract/truncated.json").as_slice(),
        ),
    ] {
        let context: ImpactContext = serde_json::from_slice(bytes)
            .unwrap_or_else(|error| panic!("{name} corpus seed did not deserialize: {error}"));
        context
            .validate()
            .unwrap_or_else(|error| panic!("{name} corpus seed did not validate: {error}"));
    }
}

#[test]
fn engine_applies_requested_snippet_bound_before_summarization() {
    let source = b"token=ABCDEFGHIJKLMNOPQRSTUVWXYZ\n";
    let mut candidate = MemoryCandidate::new(&[
        ("config/service.custom", source, true),
        (
            ".pre-commit-review/context-queries",
            b"token=[A-Z]+\n",
            false,
        ),
    ]);
    let changed = candidate
        .files
        .iter_mut()
        .find(|file| file.path.as_str() == "config/service.custom")
        .unwrap();
    changed.changed_ranges = vec![ChangedRange {
        start_line: 1,
        end_line: 1,
        deletion_anchor: false,
    }];
    let mut request = ImpactRequest::fast_defaults();
    request.max_snippet_chars = 8;

    let context = build_impact_context(&candidate, request).unwrap();

    context.validate().unwrap();
    let messages = context
        .domain_summaries
        .iter()
        .map(|summary| summary.message.as_str())
        .collect::<Vec<_>>();
    assert!(messages.iter().any(|message| message.contains("token=AB")));
    assert!(messages
        .iter()
        .all(|message| !message.contains("token=ABCDEFGHIJKLMNOPQRSTUVWXYZ")));
}

#[test]
fn engine_provider_budget_exhaustion_prevents_completed_status() {
    let source = b"pub fn changed() {}\n";
    let mut candidate = MemoryCandidate::new(&[
        ("src/lib.rs", source, true),
        (
            ".pre-commit-review/context-queries",
            b"changed\nanother\n",
            false,
        ),
    ]);
    let changed = candidate
        .files
        .iter_mut()
        .find(|file| file.path.as_str() == "src/lib.rs")
        .unwrap();
    changed.changed_ranges = vec![ChangedRange {
        start_line: 1,
        end_line: 1,
        deletion_anchor: false,
    }];
    let mut request = ImpactRequest::fast_defaults();
    request.budget.max_query_patterns = 0;

    let context = build_impact_context(&candidate, request).unwrap();

    context.validate().unwrap();
    assert_eq!(context.status, ImpactStatus::Partial);
    assert!(context
        .limitations
        .iter()
        .any(|limitation| limitation.code == "query-pattern-budget-exhausted"));
    assert!(context.providers.iter().any(|provider| {
        provider.provider_kind == "text-adapter"
            && provider.status
                == collect_diff_context_cli::impact_context::contracts::ProviderStatus::BudgetExhausted
    }));
}

#[test]
fn engine_retains_unreadable_present_unit_with_structured_limitation() {
    let candidate = UnreadableCandidate {
        files: vec![CandidateFile {
            path: RepoPath::new("src/unreadable.rs").unwrap(),
            mode: "100644".to_string(),
            content_identity: Some("0123456789012345678901234567890123456789".to_string()),
            presence: CandidatePresence::Present,
            manifest_unit_id: Some("file:src/unreadable.rs".to_string()),
            change_status: Some("M".to_string()),
            changed_ranges: vec![ChangedRange {
                start_line: 1,
                end_line: 1,
                deletion_anchor: false,
            }],
        }],
    };

    let context = build_impact_context(&candidate, ImpactRequest::fast_defaults()).unwrap();

    context.validate().unwrap();
    assert_eq!(context.status, ImpactStatus::Unavailable);
    assert_eq!(context.units.len(), 1);
    assert_eq!(context.units[0].presence, ImpactPresence::Present);
    assert_eq!(context.units[0].content_sha256, None);
    assert_eq!(context.units[0].content_bytes, None);
    assert_eq!(context.units[0].syntax_status, UnitStatus::Unavailable);
    assert_eq!(context.units[0].text_status, UnitStatus::Unavailable);
    assert!(context
        .limitations
        .iter()
        .any(|limitation| limitation.code == "candidate-read-unavailable"));
}
