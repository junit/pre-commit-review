mod support;

use collect_diff_context_cli::candidate::{
    CandidateBytes, CandidateContent, CandidateError, CandidateFile, CandidatePresence,
    ChangedRange, GitCandidateContent, RepoPath,
};
use collect_diff_context_cli::impact_context::adapters::repository_index::{
    RepositoryIndexAdapter, RepositoryIndexRequest,
};
use collect_diff_context_cli::impact_context::cache::file_facts::{
    CacheLayout, CacheLookup, FileFactsStore,
};
use collect_diff_context_cli::impact_context::contracts::{
    ChangedSymbol, Completeness, Confidence, ImpactMode, ImpactStatus, Resolution, SourceRange,
    UnitStatus,
};
use collect_diff_context_cli::impact_context::engine::{
    build_impact_context_with_repository_index, ImpactRequest, RepositoryIndexRuntime,
};
use collect_diff_context_cli::impact_context::index::budget::IndexBudget;
use collect_diff_context_cli::impact_context::index::manifest::{
    GitRepositoryManifestSource, RepositoryManifestSource,
};
use collect_diff_context_cli::impact_context::index::model::{
    FileFactKey, GraphGenerationIdentity, IndexLimitation, RepositoryLocator, RepositoryManifest,
    RepositoryManifestEntry,
};
use collect_diff_context_cli::review_scope::ReviewSource;
use rusqlite::Connection;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};
use support::GitRepo;

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
    source: ReviewSource,
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
            source: ReviewSource::Staged,
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

    fn with_source(mut self, source: ReviewSource) -> Self {
        self.source = source;
        self
    }

    fn renamed_auth() -> Self {
        let mut candidate = Self::changed_auth();
        let path = repo_path("src/auth.rs");
        let bytes = b"pub fn authorize() -> bool { true }\n".to_vec();
        candidate.bytes.insert(path, bytes.clone());
        candidate.files[0].content_identity = Some(digest(&bytes));
        candidate
    }

    fn changed_auth_signature() -> Self {
        let mut candidate = Self::changed_auth();
        let path = repo_path("src/auth.rs");
        let bytes = b"pub fn validate(token: &str) -> bool { !token.is_empty() }\n".to_vec();
        candidate.bytes.insert(path, bytes.clone());
        candidate.files[0].content_identity = Some(digest(&bytes));
        candidate
    }

    fn deleted_auth() -> Self {
        let mut candidate = Self::changed_auth();
        candidate.files[0].mode = "000000".to_string();
        candidate.files[0].content_identity = None;
        candidate.files[0].presence = CandidatePresence::Deleted;
        candidate.files[0].change_status = Some("D".to_string());
        candidate
    }

    fn added_extra() -> Self {
        let mut bytes = repository_files();
        let extra = repo_path("src/extra.rs");
        let content = b"pub fn new_api() -> bool { true }\n".to_vec();
        bytes.insert(extra.clone(), content.clone());
        Self {
            scope: repeated('a'),
            candidate_digest: repeated('b'),
            source: ReviewSource::Staged,
            files: vec![CandidateFile {
                path: extra.clone(),
                mode: "100644".to_string(),
                content_identity: Some(digest(&content)),
                presence: CandidatePresence::Present,
                manifest_unit_id: Some("changed:src/extra.rs".to_string()),
                change_status: Some("A".to_string()),
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

    fn changed_api() -> Self {
        let bytes = repository_files();
        let api = repo_path("src/api.rs");
        Self {
            scope: repeated('a'),
            candidate_digest: repeated('b'),
            source: ReviewSource::Staged,
            files: vec![CandidateFile {
                path: api.clone(),
                mode: "100644".to_string(),
                content_identity: Some(digest(&bytes[&api])),
                presence: CandidatePresence::Present,
                manifest_unit_id: Some("changed:src/api.rs".to_string()),
                change_status: Some("M".to_string()),
                changed_ranges: vec![ChangedRange {
                    start_line: 2,
                    end_line: 2,
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
        self.source
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
    invalidate_authoritative_on_scope_read: Option<usize>,
    authoritative_scope_valid: Cell<bool>,
    scope_reads: Cell<usize>,
    files: BTreeMap<RepoPath, Vec<u8>>,
    manifest: RepositoryManifest,
    manifest_reads: Cell<usize>,
    reads: RefCell<Vec<String>>,
}

impl MemoryManifestSource {
    fn stable() -> Self {
        Self::new(None, false)
    }

    fn partial() -> Self {
        Self::new(None, true)
    }

    fn branch() -> Self {
        let mut source = Self::new(None, false);
        source.manifest.locator.source = ReviewSource::Branch;
        source.manifest.locator.index_manifest_digest = None;
        source.manifest.locator.overlay_candidate_digest = repeated('4');
        source
    }

    fn unstaged() -> Self {
        let mut source = Self::new(None, false);
        source.manifest.locator.source = ReviewSource::Unstaged;
        source.manifest.locator.overlay_candidate_digest = repeated('5');
        source
    }

    fn drifting() -> Self {
        Self::new(Some(2), false)
    }

    fn drifting_before_first_publish() -> Self {
        Self::new(Some(1), false)
    }

    fn invalidating_during_next_publish() -> Self {
        let mut source = Self::new(None, false);
        source.invalidate_authoritative_on_scope_read = Some(2);
        source
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
            invalidate_authoritative_on_scope_read: None,
            authoritative_scope_valid: Cell::new(true),
            scope_reads: Cell::new(0),
            files,
            manifest,
            manifest_reads: Cell::new(0),
            reads: RefCell::new(Vec::new()),
        }
    }
}

impl RepositoryManifestSource for MemoryManifestSource {
    fn scope_fingerprint(&self) -> &str {
        let read = self.scope_reads.get();
        self.scope_reads.set(read + 1);
        if self
            .invalidate_authoritative_on_scope_read
            .is_some_and(|threshold| read + 1 == threshold)
        {
            self.authoritative_scope_valid.set(false);
        }
        if self
            .drift_after_scope_reads
            .is_some_and(|threshold| read >= threshold)
        {
            &self.drifted_scope
        } else {
            &self.opening_scope
        }
    }

    fn revalidate_scope_bounded(
        &self,
        _deadline: Duration,
    ) -> Result<
        (),
        collect_diff_context_cli::impact_context::index::manifest::RepositoryManifestError,
    > {
        if self.authoritative_scope_valid.get() {
            Ok(())
        } else {
            Err(
                collect_diff_context_cli::impact_context::index::manifest::RepositoryManifestError {
                    code: "index-scope-drift",
                    message: "fixture authoritative scope changed".to_string(),
                },
            )
        }
    }

    fn source(&self) -> ReviewSource {
        self.manifest.locator.source
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
        self.manifest_reads
            .set(self.manifest_reads.get().saturating_add(1));
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

fn changed_login_symbol() -> ChangedSymbol {
    let mut symbol = changed_symbol();
    symbol.path = "src/api.rs".to_string();
    symbol.name = "login".to_string();
    symbol.signature = Some("pub fn login()".to_string());
    symbol.range = source_range(2);
    symbol
}

fn changed_extra_symbol() -> ChangedSymbol {
    let mut symbol = changed_symbol();
    symbol.path = "src/extra.rs".to_string();
    symbol.name = "new_api".to_string();
    symbol.signature = Some("pub fn new_api() -> bool".to_string());
    symbol
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

fn locator_reference_path(layout: &CacheLayout, kind: &str) -> PathBuf {
    snapshot(&layout.graphs_dir)
        .into_iter()
        .filter(|(path, _, _)| path.ends_with(".json"))
        .map(|(path, _, _)| layout.graphs_dir.join(path))
        .find(|path| {
            fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|value| {
                    value
                        .pointer("/payload/lookup/kind")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .as_deref()
                == Some(kind)
        })
        .unwrap_or_else(|| panic!("missing {kind} generation locator reference"))
}

#[test]
fn fast_mode_reads_compatible_generation_without_writes() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let source = MemoryManifestSource::stable();
    let adapter = RepositoryIndexAdapter::new(layout.clone());
    adapter.analyze(deep_request(&candidate, &source)).unwrap();
    assert_eq!(source.manifest_reads.get(), 1);
    let before = snapshot(cache.path());
    let changed = vec![changed_symbol()];

    let output = adapter
        .analyze(fast_request(&candidate, &source, &changed))
        .unwrap();

    assert!(output.provider.cache_hits > 0);
    assert_eq!(source.manifest_reads.get(), 1);
    assert_eq!(snapshot(cache.path()), before);
}

#[test]
fn fast_staged_candidate_uses_branch_base_overlay_without_whole_manifest() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let adapter = RepositoryIndexAdapter::new(layout);
    let branch_candidate = MemoryCandidate::changed_auth().with_source(ReviewSource::Branch);
    let branch_source = MemoryManifestSource::branch();
    let branch = adapter
        .analyze(deep_request(&branch_candidate, &branch_source))
        .unwrap();

    let staged_candidate = MemoryCandidate::changed_auth();
    let staged_source = MemoryManifestSource::stable();
    let changed = vec![changed_symbol()];
    let output = adapter
        .analyze(fast_request(&staged_candidate, &staged_source, &changed))
        .unwrap();

    assert_eq!(staged_source.manifest_reads.get(), 0);
    assert!(output.provider.cache_hits > 0);
    assert_eq!(output.metrics.file_fact_misses, 1);
    assert_eq!(output.metrics.parsed_files, 1);
    assert_eq!(
        output.metrics.parsed_bytes,
        repository_files()[&repo_path("src/auth.rs")].len() as u64
    );
    assert_ne!(
        output.provider.configuration_digest, branch.provider.configuration_digest,
        "a base-plus-overlay result must bind the exact candidate identity"
    );
    assert!(output.edges.iter().any(|edge| edge.path == "src/api.rs"));
}

#[test]
fn fast_locator_rejects_reference_whose_filename_does_not_bind_generation() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let adapter = RepositoryIndexAdapter::new(layout.clone());
    let branch_candidate = MemoryCandidate::changed_auth().with_source(ReviewSource::Branch);
    let branch_source = MemoryManifestSource::branch();
    adapter
        .analyze(deep_request(&branch_candidate, &branch_source))
        .unwrap();

    let reference = locator_reference_path(&layout, "base-tree");
    let mismatched = reference
        .parent()
        .unwrap()
        .join(format!("{}.json", repeated('0')));
    fs::rename(reference, mismatched).unwrap();

    let staged_candidate = MemoryCandidate::changed_auth();
    let staged_source = MemoryManifestSource::stable();
    let changed = vec![changed_symbol()];
    let output = adapter
        .analyze(fast_request(&staged_candidate, &staged_source, &changed))
        .unwrap();

    assert_eq!(output.index_completeness, Completeness::Unavailable);
    assert!(output.provider.cache_corrupt > 0);
    assert!(output
        .limitations
        .iter()
        .any(|limitation| limitation.code == "repository-index-base-generation-corrupt"));
}

#[test]
fn fast_locator_faults_are_bounded_and_fail_closed() {
    for fault in ["corrupt", "missing-target", "cardinality"] {
        let cache = tempfile::tempdir().unwrap();
        let layout = cache_layout(cache.path());
        let adapter = RepositoryIndexAdapter::new(layout.clone());
        let branch_candidate = MemoryCandidate::changed_auth().with_source(ReviewSource::Branch);
        let branch_source = MemoryManifestSource::branch();
        adapter
            .analyze(deep_request(&branch_candidate, &branch_source))
            .unwrap();
        let reference = locator_reference_path(&layout, "base-tree");
        match fault {
            "corrupt" => fs::write(&reference, b"{").unwrap(),
            "missing-target" => fs::remove_file(generation_path(&layout)).unwrap(),
            "cardinality" => {
                let bytes = fs::read(&reference).unwrap();
                for index in 0..32 {
                    fs::write(
                        reference
                            .parent()
                            .unwrap()
                            .join(format!("extra-{index}.json")),
                        &bytes,
                    )
                    .unwrap();
                }
            }
            _ => unreachable!(),
        }

        let staged_candidate = MemoryCandidate::changed_auth();
        let staged_source = MemoryManifestSource::stable();
        let changed = vec![changed_symbol()];
        let output = adapter
            .analyze(fast_request(&staged_candidate, &staged_source, &changed))
            .unwrap();

        assert_eq!(
            output.index_completeness,
            Completeness::Unavailable,
            "locator fault {fault} must not release graph evidence"
        );
        match fault {
            "missing-target" => assert!(output.provider.cache_stale > 0),
            _ => assert!(output.provider.cache_corrupt > 0),
        }
    }
}

#[cfg(unix)]
#[test]
fn fast_locator_does_not_follow_reference_symlinks() {
    use std::os::unix::fs::symlink;

    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let adapter = RepositoryIndexAdapter::new(layout.clone());
    let branch_candidate = MemoryCandidate::changed_auth().with_source(ReviewSource::Branch);
    let branch_source = MemoryManifestSource::branch();
    adapter
        .analyze(deep_request(&branch_candidate, &branch_source))
        .unwrap();

    let reference = locator_reference_path(&layout, "base-tree");
    let sentinel = cache.path().join("outside-reference");
    fs::write(&sentinel, b"not a locator").unwrap();
    fs::remove_file(&reference).unwrap();
    symlink(&sentinel, &reference).unwrap();

    let staged_candidate = MemoryCandidate::changed_auth();
    let staged_source = MemoryManifestSource::stable();
    let changed = vec![changed_symbol()];
    let output = adapter
        .analyze(fast_request(&staged_candidate, &staged_source, &changed))
        .unwrap();

    assert_eq!(output.index_completeness, Completeness::Unavailable);
    assert!(output.provider.cache_corrupt > 0);
    assert_eq!(fs::read(sentinel).unwrap(), b"not a locator");
}

#[cfg(unix)]
#[test]
fn fast_locator_does_not_follow_symlinked_locator_directories() {
    use std::os::unix::fs::symlink;

    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let adapter = RepositoryIndexAdapter::new(layout.clone());
    let branch_candidate = MemoryCandidate::changed_auth().with_source(ReviewSource::Branch);
    let branch_source = MemoryManifestSource::branch();
    adapter
        .analyze(deep_request(&branch_candidate, &branch_source))
        .unwrap();

    let locator_root = layout.graphs_dir.join("locators");
    let outside = cache.path().join("outside-locators");
    fs::rename(&locator_root, &outside).unwrap();
    symlink(&outside, &locator_root).unwrap();

    let staged_candidate = MemoryCandidate::changed_auth();
    let staged_source = MemoryManifestSource::stable();
    let changed = vec![changed_symbol()];
    let output = adapter
        .analyze(fast_request(&staged_candidate, &staged_source, &changed))
        .unwrap();

    assert_eq!(output.index_completeness, Completeness::Unavailable);
    assert!(output.provider.cache_corrupt > 0);
    assert!(outside.is_dir());
}

#[test]
fn fast_unstaged_candidate_uses_index_base_overlay_without_whole_manifest() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let adapter = RepositoryIndexAdapter::new(layout);
    let staged_candidate = MemoryCandidate::changed_auth();
    let staged_source = MemoryManifestSource::stable();
    adapter
        .analyze(deep_request(&staged_candidate, &staged_source))
        .unwrap();

    let unstaged_candidate = MemoryCandidate::changed_auth().with_source(ReviewSource::Unstaged);
    let unstaged_source = MemoryManifestSource::unstaged();
    let changed = vec![changed_symbol()];
    let output = adapter
        .analyze(fast_request(
            &unstaged_candidate,
            &unstaged_source,
            &changed,
        ))
        .unwrap();

    assert_eq!(unstaged_source.manifest_reads.get(), 0);
    assert!(output.provider.cache_hits > 0);
    assert!(output.edges.iter().any(|edge| edge.path == "src/api.rs"));
}

#[test]
fn fast_overlay_preserves_replaced_symbol_callers_as_unresolved_impact() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let adapter = RepositoryIndexAdapter::new(layout);
    let branch_candidate = MemoryCandidate::changed_auth().with_source(ReviewSource::Branch);
    let branch_source = MemoryManifestSource::branch();
    adapter
        .analyze(deep_request(&branch_candidate, &branch_source))
        .unwrap();

    let staged_candidate = MemoryCandidate::renamed_auth();
    let staged_source = MemoryManifestSource::stable();
    let mut authorize = changed_symbol();
    authorize.name = "authorize".to_string();
    authorize.signature = Some("pub fn authorize() -> bool".to_string());
    let output = adapter
        .analyze(fast_request(
            &staged_candidate,
            &staged_source,
            &[authorize],
        ))
        .unwrap();

    assert!(output.edges.iter().any(|edge| {
        edge.path == "src/api.rs"
            && edge.resolution == Resolution::Unresolved
            && edge.to_symbol.is_none()
    }));
}

#[test]
fn fast_overlay_reresolves_unchanged_reverse_dependents_to_replaced_symbols() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let adapter = RepositoryIndexAdapter::new(layout);
    let branch_candidate = MemoryCandidate::changed_auth().with_source(ReviewSource::Branch);
    let branch_source = MemoryManifestSource::branch();
    adapter
        .analyze(deep_request(&branch_candidate, &branch_source))
        .unwrap();
    let baseline = adapter
        .analyze(fast_request(
            &branch_candidate,
            &branch_source,
            &[changed_symbol()],
        ))
        .unwrap();
    let base_target = baseline
        .edges
        .iter()
        .find(|edge| edge.path == "src/api.rs" && edge.resolution == Resolution::ResolvedReference)
        .and_then(|edge| edge.to_symbol.clone())
        .expect("base graph should contain the resolved api caller");

    let staged_candidate = MemoryCandidate::changed_auth_signature();
    let staged_source = MemoryManifestSource::stable();
    let mut validate = changed_symbol();
    validate.signature = Some("pub fn validate(token: &str) -> bool".to_string());
    let output = adapter
        .analyze(fast_request(&staged_candidate, &staged_source, &[validate]))
        .unwrap();

    assert!(output.edges.iter().any(|edge| {
        edge.path == "src/api.rs"
            && edge.resolution == Resolution::ResolvedReference
            && edge
                .to_symbol
                .as_deref()
                .is_some_and(|target| target != base_target)
    }));
    assert!(!output.limitations.iter().any(|limitation| {
        limitation.code == "repository-overlay-dependent-refresh-unavailable"
            && limitation.path.as_deref() == Some("src/api.rs")
    }));
}

#[test]
fn fast_overlay_query_row_budget_degrades_to_partial_context() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let adapter = RepositoryIndexAdapter::new(layout);
    let branch_candidate = MemoryCandidate::changed_auth().with_source(ReviewSource::Branch);
    let branch_source = MemoryManifestSource::branch();
    adapter
        .analyze(deep_request(&branch_candidate, &branch_source))
        .unwrap();

    let staged_candidate = MemoryCandidate::changed_api();
    let staged_source = MemoryManifestSource::stable();
    let changed = vec![changed_login_symbol()];
    let mut request = fast_request(&staged_candidate, &staged_source, &changed);
    request.index_budget.max_query_rows = 1;
    let output = adapter.analyze(request).unwrap();

    assert_eq!(output.query_completeness, Completeness::Partial);
    assert!(output
        .limitations
        .iter()
        .any(|limitation| limitation.code == "index-query-row-budget-exhausted"));
}

#[test]
fn fast_deleted_path_accounts_for_base_seed_rows_before_tombstoning() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let adapter = RepositoryIndexAdapter::new(layout);
    let branch_candidate = MemoryCandidate::changed_auth().with_source(ReviewSource::Branch);
    let branch_source = MemoryManifestSource::branch();
    adapter
        .analyze(deep_request(&branch_candidate, &branch_source))
        .unwrap();

    let staged_candidate = MemoryCandidate::deleted_auth();
    let staged_source = MemoryManifestSource::stable();
    let changed = vec![changed_symbol()];
    let mut request = fast_request(&staged_candidate, &staged_source, &changed);
    request.index_budget.max_query_rows = 1;
    request.index_budget.max_graph_depth = 0;
    let output = adapter.analyze(request).unwrap();

    assert_eq!(output.query_completeness, Completeness::Partial);
    assert_eq!(output.metrics.query_rows, 1);
    assert!(
        output.symbols.is_empty(),
        "overlay construction exhausted the shared row budget before traversal"
    );
    assert!(output
        .limitations
        .iter()
        .any(|limitation| limitation.code == "index-query-row-budget-exhausted"));
}

#[test]
fn fast_added_rust_file_infers_module_from_existing_crate_root() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let adapter = RepositoryIndexAdapter::new(layout);
    let branch_candidate = MemoryCandidate::changed_auth().with_source(ReviewSource::Branch);
    let branch_source = MemoryManifestSource::branch();
    adapter
        .analyze(deep_request(&branch_candidate, &branch_source))
        .unwrap();

    let staged_candidate = MemoryCandidate::added_extra();
    let staged_source = MemoryManifestSource::stable();
    let changed = vec![changed_extra_symbol()];
    let output = adapter
        .analyze(fast_request(&staged_candidate, &staged_source, &changed))
        .unwrap();

    assert!(output
        .symbols
        .iter()
        .any(|symbol| { symbol.path == "src/extra.rs" && symbol.name == "new_api" }));
    assert!(!output
        .limitations
        .iter()
        .any(|limitation| limitation.code == "repository-overlay-module-unresolved"));
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
fn authoritative_drift_during_file_facts_publish_leaves_no_reusable_artifacts() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let source = MemoryManifestSource::invalidating_during_next_publish();

    let error = RepositoryIndexAdapter::new(layout.clone())
        .analyze(deep_request(&candidate, &source))
        .unwrap_err();

    assert_eq!(error.code, "repository-index-scope-drift");
    assert!(
        snapshot(&layout.facts_dir)
            .iter()
            .all(|(path, _, _)| !path.ends_with(".facts")),
        "scope-invalid FileFacts must not remain reusable"
    );
    assert!(
        snapshot(&layout.graphs_dir)
            .iter()
            .all(|(path, _, _)| !path.ends_with(".sqlite") && !path.ends_with(".json")),
        "scope-invalid graph artifacts must not remain reusable"
    );
}

#[test]
fn authoritative_drift_during_graph_publish_removes_new_generation() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let stable = MemoryManifestSource::stable();
    RepositoryIndexAdapter::new(layout.clone())
        .analyze(deep_request(&candidate, &stable))
        .unwrap();
    fs::remove_dir_all(&layout.graphs_dir).unwrap();
    let source = MemoryManifestSource::invalidating_during_next_publish();

    let error = RepositoryIndexAdapter::new(layout.clone())
        .analyze(deep_request(&candidate, &source))
        .unwrap_err();

    assert_eq!(error.code, "repository-index-scope-drift");
    assert!(
        snapshot(&layout.graphs_dir)
            .iter()
            .all(|(path, _, _)| !path.ends_with(".sqlite") && !path.ends_with(".json")),
        "scope-invalid graph generation must not remain reusable"
    );
}

#[test]
fn real_git_scope_drift_before_indexing_leaves_no_reusable_cache_artifacts(
) -> Result<(), Box<dyn std::error::Error>> {
    let repository = GitRepo::new()?;
    repository.commit_file("src/lib.rs", b"pub fn original() {}\n")?;
    repository.write("src/lib.rs", b"pub fn first_staged() {}\n")?;
    repository.git(["add", "--", "src/lib.rs"])?;

    let scope = repository.scope(ReviewSource::Staged)?;
    let candidate = GitCandidateContent::open(&scope)?;
    let source = GitRepositoryManifestSource::new(&scope)?;
    let cache = tempfile::tempdir()?;
    let layout = cache_layout(cache.path());

    repository.write("src/lib.rs", b"pub fn second_staged() {}\n")?;
    repository.git(["add", "--", "src/lib.rs"])?;

    let error = RepositoryIndexAdapter::new(layout.clone())
        .analyze(RepositoryIndexRequest {
            candidate: &candidate,
            manifest_source: &source,
            changed_symbols: &[],
            mode: ImpactMode::Deep,
            cache_read: true,
            cache_write: true,
            index_budget: IndexBudget::deep_defaults(),
        })
        .expect_err("a changed staged scope must invalidate the opened manifest source");

    assert_eq!(error.code, "repository-index-scope-drift");
    assert!(
        snapshot(&layout.facts_dir).is_empty(),
        "scope-invalid FileFacts must not be reusable under the stale manifest key"
    );
    assert!(
        snapshot(&layout.graphs_dir).is_empty(),
        "scope-invalid graph generations and locators must not be reusable"
    );
    Ok(())
}

#[test]
fn file_facts_are_never_published_under_a_mismatched_content_digest() {
    let cache = tempfile::tempdir().unwrap();
    let layout = cache_layout(cache.path());
    let candidate = MemoryCandidate::changed_auth();
    let mut source = MemoryManifestSource::stable();
    source.files.insert(
        repo_path("src/auth.rs"),
        b"pub fn validate() -> bool { false }\n".to_vec(),
    );

    let error = RepositoryIndexAdapter::new(layout.clone())
        .analyze(deep_request(&candidate, &source))
        .expect_err("content bytes must agree with the FileFacts manifest key");

    assert_eq!(error.code, "repository-index-file-content-digest-mismatch");
    let expected_digest = source
        .manifest
        .entries
        .iter()
        .find(|entry| entry.path.as_str() == "src/auth.rs")
        .and_then(|entry| entry.content_sha256.as_deref())
        .unwrap();
    let stale_key = FileFactKey {
        language: "rust".to_string(),
        content_sha256: expected_digest.to_string(),
        grammar_version: "tree-sitter-rust@0.24.2".to_string(),
        query_digest: digest(b"tree-sitter-rust-index-query/v1"),
        adapter_version: "tree-sitter-rust-index/v1".to_string(),
        normalization_rules_digest: digest(b"repository-index-normalization/v1"),
        schema_version: 1,
    };
    assert!(
        matches!(
            FileFactsStore::new(layout.clone(), 16 * 1024 * 1024)
                .unwrap()
                .lookup(&stale_key)
                .unwrap(),
            CacheLookup::Miss
        ),
        "H2 bytes must not be published under the H1 FileFacts key"
    );
    assert!(
        snapshot(&layout.graphs_dir).is_empty(),
        "a content-mismatched facts set must not produce a graph or locator"
    );
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
