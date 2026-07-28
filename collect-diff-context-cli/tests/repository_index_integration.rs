use collect_diff_context_cli::candidate::{
    CandidateBytes, CandidateContent, CandidateError, CandidateFile, CandidatePresence,
    ChangedRange, RepoPath,
};
use collect_diff_context_cli::impact_context::adapters::repository_index::{
    RepositoryIndexAdapter, RepositoryIndexRequest,
};
use collect_diff_context_cli::impact_context::cache::file_facts::CacheLayout;
use collect_diff_context_cli::impact_context::contracts::{
    ChangedSymbol, Completeness, Confidence, ImpactMode, ImpactStatus, Resolution, SourceRange,
    UnitStatus,
};
use collect_diff_context_cli::impact_context::engine::{
    build_impact_context_with_repository_index, ImpactRequest, RepositoryIndexRuntime,
};
use collect_diff_context_cli::impact_context::index::budget::IndexBudget;
use collect_diff_context_cli::impact_context::index::manifest::RepositoryManifestSource;
use collect_diff_context_cli::impact_context::index::model::{
    GraphGenerationIdentity, IndexLimitation, RepositoryLocator, RepositoryManifest,
    RepositoryManifestEntry,
};
use collect_diff_context_cli::review_scope::ReviewSource;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn repeated(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn repo_path(value: &str) -> RepoPath {
    RepoPath::new(value).unwrap()
}

fn source_range(line: u32) -> SourceRange {
    SourceRange {
        start_line: line,
        start_column: 1,
        end_line: line,
        end_column: 24,
        start_byte: (line as usize - 1) * 24,
        end_byte: line as usize * 24 - 1,
    }
}

fn repository_files() -> BTreeMap<RepoPath, Vec<u8>> {
    BTreeMap::from([
        (
            repo_path("Cargo.toml"),
            b"[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n\n[lib]\npath = \"src/lib.rs\"\n"
                .to_vec(),
        ),
        (
            repo_path("src/api.rs"),
            b"use crate::auth::validate;\npub fn login() { validate(); }\n".to_vec(),
        ),
        (
            repo_path("src/auth.rs"),
            b"pub fn validate() -> bool { true }\n".to_vec(),
        ),
        (
            repo_path("src/lib.rs"),
            b"pub mod api;\npub mod auth;\n".to_vec(),
        ),
    ])
}

struct MemoryCandidate {
    scope: String,
    candidate_digest: String,
    files: Vec<CandidateFile>,
    bytes: BTreeMap<RepoPath, Vec<u8>>,
    reads: RefCell<Vec<String>>,
}

impl MemoryCandidate {
    fn changed_auth() -> Self {
        let bytes = repository_files();
        let auth = repo_path("src/auth.rs");
        Self {
            scope: repeated('a'),
            candidate_digest: repeated('b'),
            files: vec![CandidateFile {
                path: auth.clone(),
                mode: "100644".to_string(),
                content_identity: Some(digest(&bytes[&auth])),
                presence: CandidatePresence::Present,
                manifest_unit_id: Some("changed:src/auth.rs".to_string()),
                change_status: Some("M".to_string()),
                changed_ranges: vec![ChangedRange {
                    start_line: 1,
                    end_line: 1,
                    deletion_anchor: false,
                }],
            }],
            bytes,
            reads: RefCell::new(Vec::new()),
        }
    }
}

impl CandidateContent for MemoryCandidate {
    fn scope_fingerprint(&self) -> &str {
        &self.scope
    }

    fn candidate_digest(&self) -> &str {
        &self.candidate_digest
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
        self.reads.borrow_mut().push(path.as_str().to_string());
        let bytes = self
            .bytes
            .get(path)
            .unwrap_or_else(|| panic!("unexpected candidate read: {}", path.as_str()));
        if bytes.len() > max_bytes {
            return Err(CandidateError::byte_limit_exceeded(path, max_bytes));
        }
        Ok(CandidateBytes {
            bytes: bytes.clone(),
            sha256: digest(bytes),
            binary: false,
        })
    }
}

struct MemoryManifestSource {
    opening_scope: String,
    drifted_scope: String,
    drift_after_scope_reads: Option<usize>,
    scope_reads: Cell<usize>,
    files: BTreeMap<RepoPath, Vec<u8>>,
    manifest: RepositoryManifest,
    reads: RefCell<Vec<String>>,
}

impl MemoryManifestSource {
    fn stable() -> Self {
        Self::new(None, false)
    }

    fn partial() -> Self {
        Self::new(None, true)
    }

    fn drifting() -> Self {
        Self::new(Some(2), false)
    }

    fn drifting_before_first_publish() -> Self {
        Self::new(Some(1), false)
    }

    fn new(drift_after_scope_reads: Option<usize>, partial: bool) -> Self {
        let files = repository_files();
        let mut entries = files
            .iter()
            .map(|(path, bytes)| RepositoryManifestEntry {
                path: path.clone(),
                mode: "100644".to_string(),
                presence: CandidatePresence::Present,
                content_sha256: Some(digest(bytes)),
                content_bytes: Some(bytes.len()),
                language: path
                    .as_str()
                    .ends_with(".rs")
                    .then(|| "rust".to_string())
                    .or_else(|| path.as_str().ends_with(".toml").then(|| "toml".to_string())),
                status: UnitStatus::Completed,
                limitation_codes: Vec::new(),
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest_digest = digest(
            &entries
                .iter()
                .flat_map(|entry| {
                    [
                        entry.path.as_str().as_bytes(),
                        entry.content_sha256.as_deref().unwrap().as_bytes(),
                    ]
                    .concat()
                })
                .collect::<Vec<_>>(),
        );
        let limitations = partial.then(|| IndexLimitation {
            code: "fixture-manifest-partial".to_string(),
            path: Some(repo_path("src/auth.rs")),
            symbol_id: None,
            reason: "fixture omits an external workspace member".to_string(),
            interpretation: "the repository index is intentionally partial".to_string(),
        });
        let manifest = RepositoryManifest {
            locator: RepositoryLocator {
                source: ReviewSource::Staged,
                object_format: "sha1".to_string(),
                base_tree: Some(std::iter::repeat_n('1', 40).collect()),
                index_manifest_digest: Some(repeated('2')),
                overlay_candidate_digest: repeated('3'),
            },
            digest: manifest_digest,
            entries,
            completeness: if partial {
                Completeness::Partial
            } else {
                Completeness::Complete
            },
            limitations: limitations.into_iter().collect(),
        };
        Self {
            opening_scope: repeated('a'),
            drifted_scope: repeated('c'),
            drift_after_scope_reads,
            scope_reads: Cell::new(0),
            files,
            manifest,
            reads: RefCell::new(Vec::new()),
        }
    }
}

impl RepositoryManifestSource for MemoryManifestSource {
    fn scope_fingerprint(&self) -> &str {
        let read = self.scope_reads.get();
        self.scope_reads.set(read + 1);
        if self
            .drift_after_scope_reads
            .is_some_and(|threshold| read >= threshold)
        {
            &self.drifted_scope
        } else {
            &self.opening_scope
        }
    }

    fn source(&self) -> ReviewSource {
        ReviewSource::Staged
    }

    fn repository_locator(&self) -> &RepositoryLocator {
        &self.manifest.locator
    }

    fn manifest_bounded(
        &self,
        _budget: &mut collect_diff_context_cli::impact_context::index::budget::IndexBudgetTracker,
    ) -> Result<
        RepositoryManifest,
        collect_diff_context_cli::impact_context::index::manifest::RepositoryManifestError,
    > {
        Ok(self.manifest.clone())
    }

    fn read_bounded(
        &self,
        path: &RepoPath,
        maximum_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError> {
        self.reads.borrow_mut().push(path.as_str().to_string());
        let bytes = self
            .files
            .get(path)
            .unwrap_or_else(|| panic!("unexpected repository read: {}", path.as_str()));
        if bytes.len() > maximum_bytes {
            return Err(CandidateError::byte_limit_exceeded(path, maximum_bytes));
        }
        Ok(CandidateBytes {
            bytes: bytes.clone(),
            sha256: digest(bytes),
            binary: false,
        })
    }
}

fn changed_symbol() -> ChangedSymbol {
    ChangedSymbol {
        symbol_id: "1111111111111111".to_string(),
        provider_id: "2222222222222222".to_string(),
        path: "src/auth.rs".to_string(),
        language: "rust".to_string(),
        kind: "function".to_string(),
        name: "validate".to_string(),
        owner: None,
        signature: Some("pub fn validate() -> bool".to_string()),
        visibility: Some("pub".to_string()),
        range: source_range(1),
        confidence: Confidence::High,
    }
}

fn cache_layout(root: &Path) -> CacheLayout {
    let repository_id = repeated('d');
    let repository_root = root.join("v2").join("repos").join(&repository_id);
    CacheLayout {
        root: root.to_path_buf(),
        repository_id,
        facts_dir: repository_root.join("facts"),
        graphs_dir: repository_root.join("graphs"),
        staging_dir: repository_root.join("staging"),
        locks_dir: repository_root.join("locks"),
        quarantine_dir: repository_root.join("quarantine"),
    }
}

fn deep_request<'a>(
    candidate: &'a MemoryCandidate,
    source: &'a MemoryManifestSource,
) -> RepositoryIndexRequest<'a> {
    RepositoryIndexRequest {
        candidate,
        manifest_source: source,
        changed_symbols: Box::leak(vec![changed_symbol()].into_boxed_slice()),
        mode: ImpactMode::Deep,
        cache_read: true,
        cache_write: true,
        index_budget: IndexBudget::deep_defaults(),
    }
}

fn fast_request<'a>(
    candidate: &'a MemoryCandidate,
    source: &'a MemoryManifestSource,
    changed_symbols: &'a [ChangedSymbol],
) -> RepositoryIndexRequest<'a> {
    let mut budget = IndexBudget::deep_defaults();
    budget.deadline = Duration::from_secs(2);
    budget.max_graph_depth = 1;
    RepositoryIndexRequest {
        candidate,
        manifest_source: source,
        changed_symbols,
        mode: ImpactMode::Fast,
        cache_read: true,
        cache_write: false,
        index_budget: budget,
    }
}

fn snapshot(root: &Path) -> Vec<(String, u64, u128)> {
    fn visit(base: &Path, path: &Path, output: &mut Vec<(String, u64, u128)>) {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let relative = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let modified = metadata
                .modified()
                .unwrap()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            output.push((relative, metadata.len(), modified));
            if metadata.is_dir() {
                visit(base, &path, output);
            }
        }
    }
    let mut output = Vec::new();
    visit(root, root, &mut output);
    output.sort();
    output
}

fn generation_path(layout: &CacheLayout) -> PathBuf {
    fs::read_dir(&layout.graphs_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "sqlite")
        })
        .unwrap()
}

#[test]
fn fast_mode_reads_compatible_generation_without_writes() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let source = MemoryManifestSource::stable();
    let adapter = RepositoryIndexAdapter::new(layout.clone());
    adapter.analyze(deep_request(&candidate, &source)).unwrap();
    let before = snapshot(cache.path());
    let changed = vec![changed_symbol()];

    let output = adapter
        .analyze(fast_request(&candidate, &source, &changed))
        .unwrap();

    assert!(output.provider.cache_hits > 0);
    assert_eq!(snapshot(cache.path()), before);
}

#[test]
fn warm_one_and_two_hop_repository_queries_meet_release_p95_gate() {
    if cfg!(debug_assertions) {
        return;
    }

    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let source = MemoryManifestSource::stable();
    let adapter = RepositoryIndexAdapter::new(layout);
    adapter.analyze(deep_request(&candidate, &source)).unwrap();
    let before = snapshot(cache.path());
    let changed = vec![changed_symbol()];

    for depth in [1, 2] {
        for _ in 0..5 {
            let mut request = fast_request(&candidate, &source, &changed);
            request.index_budget.max_graph_depth = depth;
            request.index_budget.deadline = Duration::from_secs(2);
            std::hint::black_box(adapter.analyze(request).unwrap());
        }
        let mut samples = Vec::with_capacity(50);
        for _ in 0..50 {
            let mut request = fast_request(&candidate, &source, &changed);
            request.index_budget.max_graph_depth = depth;
            request.index_budget.deadline = Duration::from_secs(2);
            let started = Instant::now();
            std::hint::black_box(adapter.analyze(request).unwrap());
            samples.push(started.elapsed());
        }
        samples.sort_unstable();
        let rank = samples.len().saturating_mul(95).div_ceil(100);
        let p95 = samples[rank.saturating_sub(1).min(samples.len() - 1)];
        eprintln!("warm repository traversal depth={depth} p95={p95:?}");
        assert!(
            p95 <= Duration::from_secs(2),
            "warm {depth}-hop repository query P95 {p95:?} exceeds 2s"
        );
    }

    assert_eq!(snapshot(cache.path()), before);
}

#[test]
fn fast_cache_miss_parses_only_changed_files_and_remains_valid() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let source = MemoryManifestSource::stable();
    let runtime = RepositoryIndexRuntime {
        manifest_source: &source,
        cache_layout: layout,
    };

    let context = build_impact_context_with_repository_index(
        &candidate,
        ImpactRequest::fast_defaults(),
        Some(runtime),
    )
    .unwrap();

    context.validate().unwrap();
    assert_eq!(candidate.reads.borrow().as_slice(), ["src/auth.rs"]);
    assert!(!source
        .reads
        .borrow()
        .iter()
        .any(|path| path == "src/api.rs"));
}

#[test]
fn deep_mode_builds_missing_facts_and_generation_when_write_is_authorized() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let source = MemoryManifestSource::stable();
    let output = RepositoryIndexAdapter::new(layout.clone())
        .analyze(deep_request(&candidate, &source))
        .unwrap();

    assert!(output.metrics.file_fact_misses > 0);
    assert!(output.metrics.file_fact_writes > 0);
    assert!(generation_path(&layout).is_file());
}

#[test]
fn deep_scope_drift_before_first_file_facts_publish_leaves_cache_unchanged() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let source = MemoryManifestSource::drifting_before_first_publish();
    let before = snapshot(cache.path());

    let error = RepositoryIndexAdapter::new(layout)
        .analyze(deep_request(&candidate, &source))
        .unwrap_err();

    assert_eq!(error.code, "repository-index-scope-drift");
    assert_eq!(snapshot(cache.path()), before);
}

#[test]
fn changed_symbols_seed_bounded_incoming_and_outgoing_traversal() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let source = MemoryManifestSource::stable();
    let adapter = RepositoryIndexAdapter::new(layout.clone());
    adapter.analyze(deep_request(&candidate, &source)).unwrap();
    let runtime = RepositoryIndexRuntime {
        manifest_source: &source,
        cache_layout: layout,
    };

    let context = build_impact_context_with_repository_index(
        &candidate,
        ImpactRequest::fast_defaults(),
        Some(runtime),
    )
    .unwrap();

    assert!(context.impact_edges.iter().any(|edge| {
        edge.resolution == Resolution::ResolvedReference && edge.path == "src/api.rs"
    }));
    assert!(context
        .domain_summaries
        .iter()
        .any(|summary| summary.message.contains("incoming caller")));
}

#[test]
fn repository_index_provider_reports_hits_misses_stale_corrupt_and_limitations() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let source = MemoryManifestSource::stable();
    let adapter = RepositoryIndexAdapter::new(layout.clone());
    let built = adapter.analyze(deep_request(&candidate, &source)).unwrap();
    assert!(built.provider.cache_misses > 0);

    let changed = vec![changed_symbol()];
    let hit = adapter
        .analyze(fast_request(&candidate, &source, &changed))
        .unwrap();
    assert!(hit.provider.cache_hits > 0);

    let path = generation_path(&layout);
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(32)
        .unwrap();
    let corrupt = adapter
        .analyze(fast_request(&candidate, &source, &changed))
        .unwrap();
    assert!(corrupt.provider.cache_corrupt > 0);
    assert!(!corrupt.limitations.is_empty());

    let stale_cache = tempfile::tempdir().unwrap();
    let stale_layout = cache_layout(stale_cache.path());
    let stale_adapter = RepositoryIndexAdapter::new(stale_layout.clone());
    stale_adapter
        .analyze(deep_request(&candidate, &source))
        .unwrap();
    let stale_path = generation_path(&stale_layout);
    let connection = Connection::open(stale_path).unwrap();
    let identity_json: String = connection
        .query_row("SELECT identity_json FROM generation_meta", [], |row| {
            row.get(0)
        })
        .unwrap();
    let mut identity: GraphGenerationIdentity = serde_json::from_str(&identity_json).unwrap();
    identity.project_model_digest = repeated('e');
    connection
        .execute(
            "UPDATE generation_meta SET identity_json = ?1",
            [serde_json::to_string(&identity).unwrap()],
        )
        .unwrap();
    drop(connection);
    let stale = stale_adapter
        .analyze(fast_request(&candidate, &source, &changed))
        .unwrap();
    assert!(stale.provider.cache_stale > 0);
}

#[test]
fn repository_index_limitations_preserve_affected_paths() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let mut source = MemoryManifestSource::partial();
    source.manifest.limitations.push(IndexLimitation {
        code: "fixture-manifest-partial".to_string(),
        path: Some(repo_path("src/api.rs")),
        symbol_id: None,
        reason: "fixture omits an external workspace member".to_string(),
        interpretation: "the repository index is intentionally partial".to_string(),
    });
    source.manifest.limitations.sort_by(|left, right| {
        left.path
            .as_ref()
            .map(RepoPath::as_str)
            .cmp(&right.path.as_ref().map(RepoPath::as_str))
    });

    let runtime = RepositoryIndexRuntime {
        manifest_source: &source,
        cache_layout: layout,
    };
    let context = build_impact_context_with_repository_index(
        &candidate,
        ImpactRequest::deep_defaults(),
        Some(runtime),
    )
    .unwrap();
    let mut affected_paths = context
        .limitations
        .iter()
        .filter(|limitation| limitation.code == "fixture-manifest-partial")
        .filter_map(|limitation| limitation.path.as_deref())
        .collect::<Vec<_>>();
    affected_paths.sort_unstable();

    assert_eq!(affected_paths, ["src/api.rs", "src/auth.rs"]);
}

#[test]
fn heuristic_edges_never_become_semantic_or_high_confidence() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let source = MemoryManifestSource::stable();
    let output = RepositoryIndexAdapter::new(layout)
        .analyze(deep_request(&candidate, &source))
        .unwrap();

    assert!(!output.edges.is_empty());
    assert!(output.edges.iter().all(|edge| {
        edge.resolution != Resolution::Semantic && edge.confidence != Confidence::High
    }));
}

#[test]
fn graph_index_query_and_output_completeness_remain_independent() {
    let partial_cache = tempfile::tempdir().unwrap();
    let partial_layout = cache_layout(partial_cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let partial_source = MemoryManifestSource::partial();
    let partial = RepositoryIndexAdapter::new(partial_layout)
        .analyze(deep_request(&candidate, &partial_source))
        .unwrap();
    assert_eq!(partial.index_completeness, Completeness::Partial);

    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let source = MemoryManifestSource::stable();
    let adapter = RepositoryIndexAdapter::new(layout.clone());
    adapter.analyze(deep_request(&candidate, &source)).unwrap();
    let changed = vec![changed_symbol()];

    let mut query_request = fast_request(&candidate, &source, &changed);
    query_request.index_budget.max_query_rows = 0;
    let query = adapter.analyze(query_request).unwrap();
    assert_eq!(
        query.index_completeness,
        Completeness::Complete,
        "limitations: {:?}",
        query
            .limitations
            .iter()
            .map(|limitation| limitation.code.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(query.query_completeness, Completeness::Partial);
    assert!(!query.output_truncated);

    let mut output_request = fast_request(&candidate, &source, &changed);
    output_request.index_budget.max_edges = 0;
    output_request.index_budget.max_graph_depth = 3;
    let output = adapter.analyze(output_request).unwrap();
    assert_eq!(output.index_completeness, Completeness::Complete);
    assert_eq!(output.query_completeness, Completeness::Complete);
    assert!(output.output_truncated);
}

#[test]
fn scope_drift_after_index_query_invalidates_all_graph_evidence() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let stable = MemoryManifestSource::stable();
    RepositoryIndexAdapter::new(layout.clone())
        .analyze(deep_request(&candidate, &stable))
        .unwrap();
    let drifting = MemoryManifestSource::drifting();
    let runtime = RepositoryIndexRuntime {
        manifest_source: &drifting,
        cache_layout: layout,
    };

    let context = build_impact_context_with_repository_index(
        &candidate,
        ImpactRequest::fast_defaults(),
        Some(runtime),
    )
    .unwrap();

    assert_eq!(context.status, ImpactStatus::Invalidated);
    let repository_provider_ids = context
        .providers
        .iter()
        .filter(|provider| provider.provider_kind == "repository-index")
        .map(|provider| provider.provider_id.as_str())
        .collect::<Vec<_>>();
    assert!(context
        .impact_edges
        .iter()
        .all(|edge| !repository_provider_ids.contains(&edge.provider_id.as_str())));
    assert!(context
        .limitations
        .iter()
        .any(|limitation| limitation.code == "repository-index-scope-drift"));
}
