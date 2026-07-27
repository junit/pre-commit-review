# Persistent Symbol Index Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a persistent, exact-candidate, Rust-first whole-repository FileFacts and heuristic symbol/call graph index with immutable SQLite generations, in-memory candidate overlays, bounded traversal, and operational CLI tooling.

**Architecture:** A whole-candidate manifest source and full-file Tree-sitter adapter produce path-independent content-addressed FileFacts. A passive Cargo project model and Rust resolver assemble path-dependent repository symbols and relationships into immutable SQLite generation files; Fast Mode reads compatible generations without writes, while explicit Deep/index operations build them. Candidate overlays and application-owned breadth-first traversal map bounded incoming/outgoing evidence into the existing `impact_context/v1` contract.

**Tech Stack:** Rust 2021 with minimum Rust 1.95, required by the accepted `rusqlite 0.40.1` bundled dependency closure and sufficient for standard-library file locking, `toml 1.1.3+spec-1.1.0`, serde/serde_json, sha2, Tree-sitter Rust, tempfile, existing process supervision and sanitizer code, JSON Schema draft 2020-12, Bash wrappers, Criterion, cargo-fuzz, and the existing four-platform release matrix.

---

## Status and Prerequisite

Execute this plan only after
`docs/superpowers/plans/2026-07-27-persistent-symbol-index-storage-spike.md`
records a four-platform `Go` decision in
`docs/persistent-symbol-index-sqlite-spike-results.md`.

If the spike records `No-Go`, this plan is invalid until a superseding design
and implementation plan are approved.

Implementation runs directly in the current `feature/SAST` working tree. The
Subproject B fixed base is commit `8b1e7e33e564ed84a2a073ece91ad040b4d9a31e`.
Do not merge or push to `main` as part of this plan.

This plan implements B1-B7 only. It does not add rust-analyzer, SCIP, Joern,
other language grammars, Built-in Profile Registry entries, IDE diagnostics, or
GitHub PR comments.

## Spec Coverage Map

| Approved design area | Implemented by |
| --- | --- |
| SQLite storage gate | Separate B0 spike plan |
| Whole-candidate manifest and locator | Task 2 |
| Full-file path-independent facts | Task 3 |
| Content-addressed FileFacts Store | Task 4 |
| Passive Cargo project model | Task 5 |
| Rust module and relationship resolver | Task 6 |
| Immutable SQLite generation and locking | Tasks 7-8 |
| Exact staged/working-tree overlay | Task 9 |
| Bounded graph traversal | Task 10 |
| `impact_context/v1` integration | Task 11 |
| Build, doctor, inspect, and cleanup CLI | Tasks 12-13 |
| Security, fuzz, performance, release, and SBOM | Tasks 14 and 16 |
| User-facing capability and limitation docs | Task 15 |

## File Map

**Create:**

- `collect-diff-context-cli/src/impact_context/cache/mod.rs`
- `collect-diff-context-cli/src/impact_context/cache/file_facts.rs`
- `collect-diff-context-cli/src/impact_context/cache/sqlite_generation.rs`
- `collect-diff-context-cli/src/impact_context/cache/locking.rs`
- `collect-diff-context-cli/src/impact_context/cache/integrity.rs`
- `collect-diff-context-cli/src/impact_context/cache/cleanup.rs`
- `collect-diff-context-cli/src/impact_context/index/mod.rs`
- `collect-diff-context-cli/src/impact_context/index/budget.rs`
- `collect-diff-context-cli/src/impact_context/index/manifest.rs`
- `collect-diff-context-cli/src/impact_context/index/model.rs`
- `collect-diff-context-cli/src/impact_context/index/project_model.rs`
- `collect-diff-context-cli/src/impact_context/index/overlay.rs`
- `collect-diff-context-cli/src/impact_context/index/resolver/mod.rs`
- `collect-diff-context-cli/src/impact_context/index/resolver/rust.rs`
- `collect-diff-context-cli/src/impact_context/index/traversal.rs`
- `collect-diff-context-cli/src/impact_context/adapters/repository_index.rs`
- `collect-diff-context-cli/schemas/repository-index-report.schema.json`
- `collect-diff-context-cli/tests/repository_index_contracts.rs`
- `collect-diff-context-cli/tests/repository_manifest.rs`
- `collect-diff-context-cli/tests/rust_file_facts.rs`
- `collect-diff-context-cli/tests/file_facts_store.rs`
- `collect-diff-context-cli/tests/rust_project_model.rs`
- `collect-diff-context-cli/tests/rust_repository_resolver.rs`
- `collect-diff-context-cli/tests/sqlite_repository_graph.rs`
- `collect-diff-context-cli/tests/repository_overlay.rs`
- `collect-diff-context-cli/tests/repository_traversal.rs`
- `collect-diff-context-cli/tests/repository_index_integration.rs`
- `collect-diff-context-cli/tests/repository_index_cli.rs`
- `collect-diff-context-cli/tests/fixtures/repository_index/basic/Cargo.toml`
- `collect-diff-context-cli/tests/fixtures/repository_index/basic/src/lib.rs`
- `collect-diff-context-cli/tests/fixtures/repository_index/basic/src/api.rs`
- `collect-diff-context-cli/tests/fixtures/repository_index/basic/src/auth.rs`
- `collect-diff-context-cli/tests/fixtures/repository_index/basic/tests/auth_flow.rs`
- `collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/Cargo.toml`
- `collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/src/lib.rs`
- `collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/src/a.rs`
- `collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/src/b.rs`
- `collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/src/caller.rs`
- `collect-diff-context-cli/benches/repository_index.rs`
- `collect-diff-context-cli/fuzz/fuzz_targets/file_facts_decode.rs`
- `collect-diff-context-cli/fuzz/fuzz_targets/repository_graph_row.rs`
- `collect-diff-context-cli/fuzz/fuzz_targets/repository_overlay.rs`
- `collect-diff-context-cli/fuzz/fuzz_targets/repository_traversal.rs`
- `scripts/index_repository_context.sh`
- `tests/repository_index_test.sh`

**Modify:**

- `collect-diff-context-cli/Cargo.toml`
- `collect-diff-context-cli/Cargo.lock`
- `collect-diff-context-cli/fuzz/Cargo.toml`
- `collect-diff-context-cli/src/lib.rs`
- `collect-diff-context-cli/src/candidate/mod.rs`
- `collect-diff-context-cli/src/candidate/content.rs`
- `collect-diff-context-cli/src/impact_context/mod.rs`
- `collect-diff-context-cli/src/impact_context/budget.rs`
- `collect-diff-context-cli/src/impact_context/contracts.rs`
- `collect-diff-context-cli/src/impact_context/engine.rs`
- `collect-diff-context-cli/src/impact_context/normalizer.rs`
- `collect-diff-context-cli/src/impact_context/summarizer.rs`
- `collect-diff-context-cli/src/impact_context/adapters/mod.rs`
- `collect-diff-context-cli/src/impact_context/adapters/tree_sitter_rust.rs`
- `collect-diff-context-cli/src/bin/repository_context.rs`
- `collect-diff-context-cli/schemas/impact-context.schema.json`
- `scripts/validate_schemas.py`
- `scripts/collect_impact_context.sh`
- `scripts/build_all_binaries.sh`
- `install.sh`
- `tests/repository_context_test.sh`
- `tests/install_smoke_test.sh`
- `tests/install_agent_matrix_test.sh`
- `.github/workflows/lint.yml`
- `.github/workflows/release.yml`
- `README.md`
- `README.zh-CN.md`
- `SKILL.md`
- `docs/helper-capabilities.md`
- `CONTRIBUTING.md`

**Delete after the accepted spike evidence is preserved:**

- `collect-diff-context-cli/src/bin/sqlite_storage_spike.rs`
- `collect-diff-context-cli/tests/sqlite_storage_spike.rs`
- `collect-diff-context-cli/tests/fixtures/sqlite_storage_spike/README.md`

### Task 1: Promote the Approved Dependencies and Define Index Contracts

**Files:**
- Modify: `collect-diff-context-cli/Cargo.toml`
- Modify: `collect-diff-context-cli/Cargo.lock`
- Modify: `collect-diff-context-cli/src/impact_context/mod.rs`
- Create: `collect-diff-context-cli/src/impact_context/cache/mod.rs`
- Create: `collect-diff-context-cli/src/impact_context/index/mod.rs`
- Create: `collect-diff-context-cli/src/impact_context/index/budget.rs`
- Create: `collect-diff-context-cli/src/impact_context/index/model.rs`
- Create: `collect-diff-context-cli/tests/repository_index_contracts.rs`
- Create: `collect-diff-context-cli/schemas/repository-index-report.schema.json`
- Modify: `scripts/validate_schemas.py`

- [ ] **Step 1: Verify the spike decision before changing product dependencies**

Run:

```bash
rtk rg -n '^Go$' docs/persistent-symbol-index-sqlite-spike-results.md
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --test sqlite_storage_spike
```

Expected: the results document contains the exact `Go` decision and all spike
tests PASS. Stop immediately if either command fails.

- [ ] **Step 2: Write failing contract tests**

Create `repository_index_contracts.rs` with tests named:

```rust
index_budget_defaults_are_bounded
repository_manifest_rejects_unsorted_duplicate_and_unsafe_paths
file_fact_key_requires_exact_lowercase_digests
graph_generation_key_changes_for_every_identity_input
index_report_rejects_unknown_fields_and_invalid_counts
```

Use these required types and imports:

```rust
use collect_diff_context_cli::impact_context::index::budget::IndexBudget;
use collect_diff_context_cli::impact_context::index::model::{
    FileFactKey, GraphGenerationIdentity, IndexAction, IndexReport,
    IndexReportStatus, RepositoryManifest,
};
```

The graph-key test must construct one baseline identity and independently mutate
candidate manifest, project model, resolver, adapter/query, FileFacts manifest,
normalization, and schema values. Every mutation must produce a different key.

- [ ] **Step 3: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_index_contracts
```

Expected: FAIL because the cache/index Modules and types do not exist.

- [ ] **Step 4: Promote and pin production dependencies**

Update the package and dependencies:

```toml
[package]
rust-version = "1.95"

[features]
test-fixture = []
sqlite-storage-spike = []

[dependencies]
rusqlite = { version = "=0.40.1", default-features = false, features = ["bundled"] }
toml = { version = "=1.1.3+spec-1.1.0", default-features = false, features = ["std", "serde", "parse"] }
```

Remove the optional marker from `rusqlite`, but keep the temporary
`sqlite-storage-spike` feature, bin declaration, source, tests, and workflow
gates until Task 14 replaces them with production repository-index gates. Keep
the accepted spike results and third-party licenses.

- [ ] **Step 5: Define exact budgets and contract enums**

In `index/budget.rs`, define:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexBudget {
    pub deadline: std::time::Duration,
    pub max_manifest_files: usize,
    pub max_manifest_bytes: usize,
    pub max_project_model_files: usize,
    pub max_project_model_bytes: usize,
    pub max_file_bytes: usize,
    pub max_parse_bytes: usize,
    pub max_nodes: usize,
    pub max_facts: usize,
    pub max_symbols: usize,
    pub max_edges: usize,
    pub max_generation_bytes: usize,
    pub max_overlay_paths: usize,
    pub max_query_rows: usize,
    pub max_graph_depth: usize,
}
```

`IndexBudget::deep_defaults()` must use:

```rust
deadline: Duration::from_secs(30),
max_manifest_files: 100_000,
max_manifest_bytes: 32 * 1024 * 1024,
max_project_model_files: 1_000,
max_project_model_bytes: 8 * 1024 * 1024,
max_file_bytes: 2 * 1024 * 1024,
max_parse_bytes: 512 * 1024 * 1024,
max_nodes: 10_000_000,
max_facts: 2_000_000,
max_symbols: 1_000_000,
max_edges: 5_000_000,
max_generation_bytes: 2 * 1024 * 1024 * 1024,
max_overlay_paths: 10_000,
max_query_rows: 50_000,
max_graph_depth: 2,
```

Add a tracker using checked arithmetic and the existing `BudgetExhaustion`
pattern:

```rust
pub struct IndexBudgetTracker {
    budget: IndexBudget,
    started: std::time::Instant,
    consumed: BTreeMap<IndexResource, usize>,
    exhausted: BTreeSet<IndexResource>,
    deadline_exhausted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IndexResource {
    ManifestFiles,
    ManifestBytes,
    ProjectModelFiles,
    ProjectModelBytes,
    FileBytes,
    ParseBytes,
    Nodes,
    Facts,
    Symbols,
    Edges,
    GenerationBytes,
    OverlayPaths,
    QueryRows,
    GraphDepth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexBudgetAmount {
    pub initial: usize,
    pub consumed: usize,
    pub remaining: usize,
    pub exhausted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexBudgetExhaustion {
    resource: Option<IndexResource>,
    code: &'static str,
}

impl IndexBudgetTracker {
    pub fn new(budget: IndexBudget) -> Self {
        Self {
            budget,
            started: std::time::Instant::now(),
            consumed: BTreeMap::new(),
            exhausted: BTreeSet::new(),
            deadline_exhausted: false,
        }
    }

    pub fn budget(&self) -> &IndexBudget {
        &self.budget
    }

    pub fn consume(
        &mut self,
        resource: IndexResource,
        amount: usize,
    ) -> Result<(), IndexBudgetExhaustion> {
        let limit = self.limit(resource);
        let consumed = self.consumed.get(&resource).copied().unwrap_or(0);
        let Some(next) = consumed.checked_add(amount) else {
            self.exhausted.insert(resource);
            return Err(index_resource_exhaustion(resource));
        };
        if next > limit {
            self.exhausted.insert(resource);
            return Err(index_resource_exhaustion(resource));
        }
        self.consumed.insert(resource, next);
        Ok(())
    }

    pub fn observe(
        &mut self,
        resource: IndexResource,
        observed: usize,
    ) -> Result<(), IndexBudgetExhaustion> {
        let limit = self.limit(resource);
        let previous = self.consumed.get(&resource).copied().unwrap_or(0);
        self.consumed
            .insert(resource, previous.max(observed.min(limit)));
        if observed > limit {
            self.exhausted.insert(resource);
            return Err(index_resource_exhaustion(resource));
        }
        Ok(())
    }

    pub fn amount(&self, resource: IndexResource) -> IndexBudgetAmount {
        let initial = self.limit(resource);
        let consumed = self
            .consumed
            .get(&resource)
            .copied()
            .unwrap_or(0)
            .min(initial);
        IndexBudgetAmount {
            initial,
            consumed,
            remaining: initial.saturating_sub(consumed),
            exhausted: self.exhausted.contains(&resource),
        }
    }

    pub fn check_deadline(&mut self) -> Result<(), IndexBudgetExhaustion> {
        if self.deadline_exhausted || self.started.elapsed() >= self.budget.deadline {
            self.deadline_exhausted = true;
            return Err(IndexBudgetExhaustion {
                resource: None,
                code: "index-deadline-exhausted",
            });
        }
        Ok(())
    }

    pub fn remaining_deadline(&self) -> std::time::Duration {
        self.budget.deadline.saturating_sub(self.started.elapsed())
    }

    fn limit(&self, resource: IndexResource) -> usize {
        match resource {
            IndexResource::ManifestFiles => self.budget.max_manifest_files,
            IndexResource::ManifestBytes => self.budget.max_manifest_bytes,
            IndexResource::ProjectModelFiles => self.budget.max_project_model_files,
            IndexResource::ProjectModelBytes => self.budget.max_project_model_bytes,
            IndexResource::FileBytes => self.budget.max_file_bytes,
            IndexResource::ParseBytes => self.budget.max_parse_bytes,
            IndexResource::Nodes => self.budget.max_nodes,
            IndexResource::Facts => self.budget.max_facts,
            IndexResource::Symbols => self.budget.max_symbols,
            IndexResource::Edges => self.budget.max_edges,
            IndexResource::GenerationBytes => self.budget.max_generation_bytes,
            IndexResource::OverlayPaths => self.budget.max_overlay_paths,
            IndexResource::QueryRows => self.budget.max_query_rows,
            IndexResource::GraphDepth => self.budget.max_graph_depth,
        }
    }
}

impl IndexResource {
    pub fn exhaustion_code(self) -> &'static str {
        match self {
            Self::ManifestFiles => "index-manifest-file-budget-exhausted",
            Self::ManifestBytes => "index-manifest-byte-budget-exhausted",
            Self::ProjectModelFiles => "index-project-model-file-budget-exhausted",
            Self::ProjectModelBytes => "index-project-model-byte-budget-exhausted",
            Self::FileBytes => "index-file-byte-budget-exhausted",
            Self::ParseBytes => "index-parse-byte-budget-exhausted",
            Self::Nodes => "index-node-budget-exhausted",
            Self::Facts => "index-fact-budget-exhausted",
            Self::Symbols => "index-symbol-budget-exhausted",
            Self::Edges => "index-edge-budget-exhausted",
            Self::GenerationBytes => "index-generation-byte-budget-exhausted",
            Self::OverlayPaths => "index-overlay-path-budget-exhausted",
            Self::QueryRows => "index-query-row-budget-exhausted",
            Self::GraphDepth => "index-graph-depth-budget-exhausted",
        }
    }
}

impl IndexBudgetExhaustion {
    pub fn code(self) -> &'static str {
        self.code
    }

    pub fn resource(self) -> Option<IndexResource> {
        self.resource
    }
}

fn index_resource_exhaustion(resource: IndexResource) -> IndexBudgetExhaustion {
    IndexBudgetExhaustion {
        resource: Some(resource),
        code: resource.exhaustion_code(),
    }
}
```

Follow the existing `BudgetTracker` behavior but keep the index exhaustion type
separate because `BudgetExhaustion` is bound to `BudgetResource`, not
`IndexResource`. `new` initializes every counter to zero. `consume` uses checked
addition, `observe` records the bounded high-water mark, and deadline exhaustion
uses resource `None`. Every `IndexResource` has a stable kebab-case exhaustion
code. Limits are hard maxima; CLI overrides may only lower them.

In `index/model.rs`, define strict serde contracts for:

```rust
RepositoryLocator
RepositoryManifestEntry
RepositoryManifest
FileFactKey
FileFactsManifestEntry
GraphGenerationIdentity
IndexAction::{Build, Doctor, Inspect, Clean}
IndexReportStatus::{Completed, Partial, Unavailable, Invalidated, Failed}
IndexMetrics
IndexLimitation
IndexReport
```

Use these fields as the stable v1 core:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IndexAction {
    Build,
    Doctor,
    Inspect,
    Clean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndexReportStatus {
    Completed,
    Partial,
    Unavailable,
    Invalidated,
    Failed,
}

pub struct RepositoryLocator {
    pub source: ReviewSource,
    pub object_format: String,
    pub base_tree: Option<String>,
    pub index_manifest_digest: Option<String>,
    pub overlay_candidate_digest: String,
}

pub struct RepositoryManifestEntry {
    pub path: RepoPath,
    pub mode: String,
    pub presence: CandidatePresence,
    pub content_sha256: Option<String>,
    pub content_bytes: Option<usize>,
    pub language: Option<String>,
    pub status: UnitStatus,
    pub limitation_codes: Vec<String>,
}

pub struct RepositoryManifest {
    pub locator: RepositoryLocator,
    pub digest: String,
    pub entries: Vec<RepositoryManifestEntry>,
    pub completeness: Completeness,
    pub limitations: Vec<IndexLimitation>,
}

pub struct FileFactKey {
    pub language: String,
    pub content_sha256: String,
    pub grammar_version: String,
    pub query_digest: String,
    pub adapter_version: String,
    pub normalization_rules_digest: String,
    pub schema_version: u16,
}

pub struct FileFactsManifestEntry {
    pub path: RepoPath,
    pub presence: CandidatePresence,
    pub file_fact_key: Option<FileFactKey>,
    pub status: UnitStatus,
}

pub struct GraphGenerationIdentity {
    pub graph_schema_version: u16,
    pub candidate_manifest_digest: String,
    pub project_model_digest: String,
    pub resolver_digest: String,
    pub adapter_query_digest: String,
    pub file_facts_manifest_digest: String,
    pub normalization_rules_digest: String,
}

pub struct IndexMetrics {
    pub elapsed_ms: u64,
    pub manifest_files: usize,
    pub manifest_bytes: u64,
    pub file_fact_hits: usize,
    pub file_fact_misses: usize,
    pub file_fact_writes: usize,
    pub parsed_files: usize,
    pub parsed_bytes: u64,
    pub symbols: usize,
    pub edges: usize,
    pub query_rows: usize,
    pub generation_bytes: u64,
    pub output_bytes: usize,
}

pub struct IndexLimitation {
    pub code: String,
    pub path: Option<RepoPath>,
    pub symbol_id: Option<String>,
    pub reason: String,
    pub interpretation: String,
}

pub struct IndexReport {
    pub schema_version: u8,
    pub kind: String,
    pub action: IndexAction,
    pub status: IndexReportStatus,
    pub scope_fingerprint: Option<String>,
    pub repository_id: String,
    pub generation_key: Option<String>,
    pub metrics: IndexMetrics,
    pub limitations: Vec<IndexLimitation>,
}
```

All serialized structs use `#[serde(deny_unknown_fields)]`. Digest fields require
64 lowercase hex characters. Paths use `RepoPath`. Collections are path or id
sorted before validation.

- [ ] **Step 6: Define `repository_index_report/v1` schema**

The JSON schema must require:

```json
{
  "schema_version": 1,
  "kind": "repository_index_report",
  "action": "build",
  "status": "completed",
  "scope_fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "repository_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  "generation_key": null,
  "metrics": {},
  "limitations": []
}
```

Use schema patterns rather than accepting the angle-bracket example strings.
The `scope_fingerprint` property is always present: build requires a valid
fingerprint, while doctor/inspect/clean may use JSON `null`. Bound all arrays and
strings consistently with Rust validation. Add the schema to
`scripts/validate_schemas.py`.

- [ ] **Step 7: Run and verify green**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_index_contracts
rtk python3 scripts/validate_schemas.py
```

Expected: contract tests PASS and the validator reports all schemas valid.

- [ ] **Step 8: Commit the production contract boundary**

```bash
rtk git add collect-diff-context-cli/Cargo.toml collect-diff-context-cli/Cargo.lock collect-diff-context-cli/src/impact_context/mod.rs collect-diff-context-cli/src/impact_context/cache/mod.rs collect-diff-context-cli/src/impact_context/index/mod.rs collect-diff-context-cli/src/impact_context/index/budget.rs collect-diff-context-cli/src/impact_context/index/model.rs collect-diff-context-cli/tests/repository_index_contracts.rs collect-diff-context-cli/schemas/repository-index-report.schema.json scripts/validate_schemas.py
rtk git commit -m "feat: define persistent index contracts"
```

### Task 2: Add Exact Whole-Candidate Manifest Enumeration

**Files:**
- Create: `collect-diff-context-cli/src/impact_context/index/manifest.rs`
- Create: `collect-diff-context-cli/tests/repository_manifest.rs`
- Modify: `collect-diff-context-cli/src/impact_context/index/mod.rs`
- Modify: `collect-diff-context-cli/src/candidate/mod.rs`
- Modify: `collect-diff-context-cli/src/candidate/content.rs`

- [ ] **Step 1: Write failing staged, unstaged, and branch manifest tests**

Add tests named:

```rust
staged_manifest_contains_unchanged_and_stage_zero_content
unstaged_manifest_uses_tracked_worktree_bytes_and_excludes_untracked
branch_manifest_uses_committed_tree_despite_worktree_changes
manifest_digest_is_path_sorted_and_repeatable
manifest_preserves_delete_mode_symlink_and_gitlink_states
manifest_limits_return_explicit_partial_entries
candidate_locator_changes_when_index_or_overlay_changes
manifest_git_process_obeys_shared_deadline_and_output_limit
```

The staged test must commit `src/base.rs`, stage `src/new.rs` with `staged` bytes,
then replace its worktree content with `working` bytes. Require the manifest to
contain both paths and the SHA256 of `staged`, never `working`.

The unstaged test must include a separately staged path and prove the index
manifest locator plus working-tree overlay describe the exact tracked candidate.

- [ ] **Step 2: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_manifest
```

Expected: FAIL because `GitRepositoryManifestSource` does not exist.

- [ ] **Step 3: Define the whole-candidate Interface**

Add to `manifest.rs`:

```rust
pub trait RepositoryManifestSource {
    fn scope_fingerprint(&self) -> &str;
    fn source(&self) -> ReviewSource;
    fn repository_locator(&self) -> &RepositoryLocator;
    fn manifest_bounded(
        &self,
        budget: &mut IndexBudgetTracker,
    ) -> Result<RepositoryManifest, RepositoryManifestError>;
    fn read_bounded(
        &self,
        path: &RepoPath,
        maximum_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError>;
}

pub struct GitRepositoryManifestSource {
    scope: AuthoritativeScope,
    repository_locator: RepositoryLocator,
}

#[derive(Debug)]
pub struct RepositoryManifestError {
    pub code: &'static str,
    pub message: String,
}
```

Construction requires an already opened authoritative scope. The Module never
selects a different source or widens the review candidate.

- [ ] **Step 4: Implement bounded source-specific enumeration**

Use read-only Git commands with the existing process-group supervision:

- staged and unstaged index base: `git ls-files --stage -z`;
- branch: `git ls-tree -rz HEAD` for the currently selected committed branch
  candidate;
- staged content: stage-zero blob ids through a streaming `git cat-file --batch`;
- branch content: committed blob ids through the same bounded batch reader;
- unstaged content: tracked filesystem bytes using existing no-follow path and
  symlink handling.

Add a reusable internal streaming batch reader to `candidate/content.rs`. It
must enforce remaining deadline, per-file bytes, total bytes, output bytes, and
process-group termination without buffering the complete repository in one
`Output`.

Manifest records are sorted by raw normalized `RepoPath`. Digest canonicalization
uses length-prefixed bytes for path, mode, presence, content SHA256, and status;
never concatenate ambiguous text with separators.

- [ ] **Step 5: Implement locator composition**

Use these locator inputs:

```rust
pub struct RepositoryLocator {
    pub source: ReviewSource,
    pub object_format: String,
    pub base_tree: Option<String>,
    pub index_manifest_digest: Option<String>,
    pub overlay_candidate_digest: String,
}
```

Branch binds the selected tree. Staged binds opening HEAD tree plus the complete
stage-zero changed set. Unstaged binds the full stage-zero index manifest plus
the complete tracked working overlay. A locator is lookup-only; manifest
validation remains authoritative.

- [ ] **Step 6: Run focused and regression tests**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_manifest
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test candidate_content
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test review_scope
```

Expected: all tests PASS; candidate-content fast behavior remains unchanged.

- [ ] **Step 7: Commit exact repository manifests**

```bash
rtk git add collect-diff-context-cli/src/candidate/mod.rs collect-diff-context-cli/src/candidate/content.rs collect-diff-context-cli/src/impact_context/index/mod.rs collect-diff-context-cli/src/impact_context/index/manifest.rs collect-diff-context-cli/tests/repository_manifest.rs
rtk git commit -m "feat: add exact repository manifests"
```

### Task 3: Extract Path-Independent Full-File Rust Facts

**Files:**
- Modify: `collect-diff-context-cli/src/impact_context/adapters/tree_sitter_rust.rs`
- Create: `collect-diff-context-cli/tests/rust_file_facts.rs`
- Modify: `collect-diff-context-cli/src/impact_context/normalizer.rs`

- [ ] **Step 1: Write failing full-file extraction tests**

Add tests named:

```rust
index_extracts_all_definitions_not_only_changed_ranges
index_extracts_module_import_alias_group_and_glob_facts
index_extracts_references_and_call_sites_with_local_owners
index_facts_are_path_independent_and_deterministic
index_parse_recovery_records_affected_ranges_without_panicking
index_fact_node_and_deadline_limits_return_partial_output
fast_changed_range_output_remains_unchanged
```

Use a fixture containing an inline module, file module declaration, free
function, impl method, associated function, alias import, grouped import, glob
import, qualified call, method call, macro call, and syntax error outside one
valid symbol.

- [ ] **Step 2: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test rust_file_facts
```

Expected: FAIL because only `RustSyntaxOutput` for changed ranges exists.

- [ ] **Step 3: Define path-independent facts**

Add these types:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustLocalSymbolFact {
    pub local_id: String,
    pub kind: String,
    pub name: String,
    pub owner_local_id: Option<String>,
    pub signature: String,
    pub visibility: Option<String>,
    pub range: SourceRange,
}

pub struct RustImportFact {
    pub segments: Vec<String>,
    pub alias: Option<String>,
    pub glob: bool,
    pub public: bool,
    pub range: SourceRange,
}

pub struct RustReferenceFact {
    pub name: String,
    pub qualifier: Vec<String>,
    pub role: String,
    pub owner_local_id: Option<String>,
    pub range: SourceRange,
}

pub struct RustCallSiteFact {
    pub callee: String,
    pub qualifier: Vec<String>,
    pub call_kind: String,
    pub caller_local_id: Option<String>,
    pub range: SourceRange,
}

pub struct RustAttributeFact {
    pub name: String,
    pub arguments: Vec<String>,
    pub range: SourceRange,
}

pub struct RustModuleDeclarationFact {
    pub name: String,
    pub inline: bool,
    pub path_override: Option<String>,
    pub owner_local_id: Option<String>,
    pub range: SourceRange,
}

pub struct RustFileFactMetrics {
    pub nodes_visited: usize,
    pub max_nesting_depth: usize,
    pub facts_emitted: usize,
    pub source_bytes: usize,
}

pub struct RustFileFacts {
    pub parse_quality: ParseQuality,
    pub symbols: Vec<RustLocalSymbolFact>,
    pub imports: Vec<RustImportFact>,
    pub references: Vec<RustReferenceFact>,
    pub calls: Vec<RustCallSiteFact>,
    pub module_declarations: Vec<RustModuleDeclarationFact>,
    pub attributes: Vec<RustAttributeFact>,
    pub recovery_ranges: Vec<SourceRange>,
    pub limitations: Vec<String>,
    pub metrics: RustFileFactMetrics,
}
```

Local ids use only content-local kind, owner, name, and source range. No path or
repository identity enters these ids.

Apply the same `Debug + Clone + Eq + Serialize + Deserialize +
deny_unknown_fields` contract to every path-independent fact and metrics type in
the block. Attribute facts store parsed names and bounded normalized arguments,
not unrestricted source snippets.

- [ ] **Step 4: Add a separate full-file operation**

Implement:

```rust
pub fn analyze_index(
    source: &[u8],
    budget: &mut IndexBudgetTracker,
) -> Result<RustFileFacts, RustAdapterError>;
```

Share parser setup, query compilation, recovery traversal, range normalization,
and capture helpers with `analyze`. Do not implement `analyze_index` by passing a
fake all-file changed range into the Fast operation; the output contracts and
fact selection are different.

Definition-name captures must not also become references. Call-function
identifiers become call-site facts and only become reference facts when their
role is independently valid. Method calls remain syntactic and unresolved.

- [ ] **Step 5: Run focused, performance, and fuzz regressions**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test rust_file_facts
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test impact_context_rust
rtk cargo test --release --manifest-path collect-diff-context-cli/Cargo.toml --test impact_context_performance -- --nocapture
```

Expected: all tests PASS and Fast Mode release gates remain within their existing
thresholds.

- [ ] **Step 6: Commit full-file syntax facts**

```bash
rtk git add collect-diff-context-cli/src/impact_context/adapters/tree_sitter_rust.rs collect-diff-context-cli/src/impact_context/normalizer.rs collect-diff-context-cli/tests/rust_file_facts.rs
rtk git commit -m "feat: extract full-file rust facts"
```

### Task 4: Add the Content-Addressed FileFacts Store

**Files:**
- Create: `collect-diff-context-cli/src/impact_context/cache/file_facts.rs`
- Create: `collect-diff-context-cli/src/impact_context/cache/integrity.rs`
- Create: `collect-diff-context-cli/tests/file_facts_store.rs`
- Modify: `collect-diff-context-cli/src/impact_context/cache/mod.rs`

- [ ] **Step 1: Write failing cache layout and integrity tests**

Add tests named:

```rust
cache_root_uses_platform_default_or_absolute_override
cache_root_rejects_relative_repository_and_git_internal_paths
file_facts_key_changes_for_content_grammar_query_adapter_and_schema
write_then_read_validates_envelope_and_payload_digest
identical_content_reuses_one_object_across_paths
truncated_oversized_unknown_schema_and_checksum_mismatch_are_corrupt_misses
concurrent_same_key_writers_converge_without_overwrite
unix_cache_permissions_are_private
```

The repository-contained override test must try both `<repo>/cache` and
`<repo>/.git/cache` and require rejection before directory creation.

- [ ] **Step 2: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test file_facts_store
```

Expected: FAIL because `CacheLayout` and `FileFactsStore` do not exist.

- [ ] **Step 3: Define cache layout and immutable envelope**

Add:

```rust
pub struct CacheLayout {
    pub root: PathBuf,
    pub repository_id: String,
    pub facts_dir: PathBuf,
    pub graphs_dir: PathBuf,
    pub staging_dir: PathBuf,
    pub locks_dir: PathBuf,
    pub quarantine_dir: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileFactsEnvelope {
    magic: String,
    schema_version: u16,
    key: FileFactKey,
    payload_length: usize,
    payload_sha256: String,
    payload: RustFileFacts,
}

pub struct FileFactsStore {
    layout: CacheLayout,
    maximum_object_bytes: usize,
}

impl FileFactsStore {
    pub fn lookup(&self, key: &FileFactKey) -> Result<CacheLookup<RustFileFacts>, CacheError>;
    pub fn publish(&self, key: &FileFactKey, facts: &RustFileFacts) -> Result<PublishResult, CacheError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishResult {
    Published,
    Reused,
}

#[derive(Debug)]
pub struct CacheError {
    pub code: &'static str,
    pub message: String,
}
```

Use magic `pre-commit-review-file-facts` and schema version 1. Serialize compact
JSON with every map represented by `BTreeMap` and every vector sorted before
writing. Enforce a 16MiB default encoded-object hard limit and the caller's lower
budget.

- [ ] **Step 4: Implement no-clobber object publication**

The final path is:

```text
facts/sha256/<first-two-hex>/<64-hex>.facts
```

Create a `NamedTempFile` in the final parent directory, write the complete
envelope, `sync_all`, then `persist_noclobber`. When the final path exists, read
and validate it; reuse only when key and payload are exact. Never replace an
invalid existing object in Fast Mode.

- [ ] **Step 5: Implement bounded reads and cache result classification**

Return:

```rust
pub enum CacheLookup<T> {
    Hit(T),
    Miss,
    Stale { code: String },
    Corrupt { code: String },
}
```

Before deserialization, check file type, metadata length, maximum bytes, magic,
schema, key, declared payload length, and payload SHA256. Decode errors and
invalid ranges are `Corrupt`, not process failures.

- [ ] **Step 6: Run focused tests**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test file_facts_store
```

Expected: all tests PASS; concurrent writers produce one final valid object.

- [ ] **Step 7: Commit the FileFacts Store**

```bash
rtk git add collect-diff-context-cli/src/impact_context/cache/mod.rs collect-diff-context-cli/src/impact_context/cache/file_facts.rs collect-diff-context-cli/src/impact_context/cache/integrity.rs collect-diff-context-cli/tests/file_facts_store.rs
rtk git commit -m "feat: persist content-addressed file facts"
```

### Task 5: Parse the Passive Rust Project Model

**Files:**
- Create: `collect-diff-context-cli/src/impact_context/index/project_model.rs`
- Create: `collect-diff-context-cli/tests/rust_project_model.rs`
- Modify: `collect-diff-context-cli/src/impact_context/index/mod.rs`

- [ ] **Step 1: Write failing project-model tests**

Add tests named:

```rust
single_package_discovers_conventional_lib_main_bin_and_test_roots
explicit_lib_and_bin_paths_override_conventional_roots
literal_workspace_members_are_path_sorted
workspace_globs_and_inherited_fields_are_partial_not_executed
malformed_and_oversized_manifests_are_bounded_limitations
project_model_digest_binds_exact_consumed_manifest_bytes_and_policy
project_model_never_invokes_cargo_or_repository_commands
```

The command-safety test must put an executable named `cargo` first on `PATH` that
writes a marker and exits 99. Build the project model and require the marker to
remain absent.

- [ ] **Step 2: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test rust_project_model
```

Expected: FAIL because no project-model reader exists.

- [ ] **Step 3: Define the passive model types**

Add:

```rust
pub struct RustProjectModel {
    pub digest: String,
    pub packages: Vec<RustPackageModel>,
    pub roots: Vec<RustTargetRoot>,
    pub consumed_files: Vec<ProjectModelFile>,
    pub completeness: Completeness,
    pub limitations: Vec<String>,
}

pub struct RustPackageModel {
    pub package_name: String,
    pub manifest_path: RepoPath,
    pub package_root: RepoPath,
}

pub struct RustTargetRoot {
    pub package_name: String,
    pub kind: String,
    pub source_path: RepoPath,
    pub crate_name: String,
}
```

- [ ] **Step 4: Parse only approved TOML fields**

Export the focused parser from `index/mod.rs`:

```rust
pub mod project_model;
```

Deserialize exact candidate `Cargo.toml` bytes into private structs covering:

- `[package].name`;
- `[lib].path` and `[lib].name`;
- `[[bin]].name` and `[[bin]].path`;
- `[workspace].members` literal strings.

Support conventional `src/lib.rs`, `src/main.rs`, `src/bin/*.rs`, and tracked
`tests/*.rs` roots. Record partial limitations for workspace globs, inherited
workspace package fields, generated targets, unsupported paths, and manifests
outside budgets. Do not inspect the ambient filesystem for files absent from the
candidate manifest.

- [ ] **Step 5: Run and verify green**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test rust_project_model
```

Expected: all tests PASS and the fake Cargo marker is absent.

- [ ] **Step 6: Commit the project model**

```bash
rtk git add collect-diff-context-cli/src/impact_context/index/mod.rs collect-diff-context-cli/src/impact_context/index/project_model.rs collect-diff-context-cli/tests/rust_project_model.rs
rtk git commit -m "feat: add passive rust project model"
```

### Task 6: Resolve Rust Modules, Symbols, and Heuristic Relationships

**Files:**
- Create: `collect-diff-context-cli/src/impact_context/index/resolver/mod.rs`
- Create: `collect-diff-context-cli/src/impact_context/index/resolver/rust.rs`
- Create: `collect-diff-context-cli/tests/rust_repository_resolver.rs`
- Modify: `collect-diff-context-cli/src/impact_context/index/mod.rs`
- Modify: `collect-diff-context-cli/src/impact_context/index/model.rs`
- Create: `collect-diff-context-cli/tests/fixtures/repository_index/basic/Cargo.toml`
- Create: `collect-diff-context-cli/tests/fixtures/repository_index/basic/src/lib.rs`
- Create: `collect-diff-context-cli/tests/fixtures/repository_index/basic/src/api.rs`
- Create: `collect-diff-context-cli/tests/fixtures/repository_index/basic/src/auth.rs`
- Create: `collect-diff-context-cli/tests/fixtures/repository_index/basic/tests/auth_flow.rs`
- Create: `collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/Cargo.toml`
- Create: `collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/src/lib.rs`
- Create: `collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/src/a.rs`
- Create: `collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/src/b.rs`
- Create: `collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/src/caller.rs`

- [ ] **Step 1: Create concrete repository fixtures**

The `basic` fixture must define:

```rust
// src/lib.rs
pub mod api;
pub mod auth;

// src/auth.rs
pub fn validate_token(token: &str) -> bool { !token.is_empty() }

// src/api.rs
use crate::auth::validate_token as validate;
pub fn login(token: &str) -> bool { validate(token) }

// tests/auth_flow.rs
use fixture::api::login;
#[test]
fn accepts_token() { assert!(login("token")); }
```

The `ambiguous` fixture must export two functions named `parse`, use a glob
import, contain one method call, and contain a macro-generated call. Expected
results remain polymorphic or unresolved.

- [ ] **Step 2: Write failing resolver tests**

Add tests named:

```rust
resolves_crate_self_super_alias_group_and_reexport_paths
builds_parent_child_modules_for_inline_and_file_modules
resolves_unique_free_and_associated_function_calls
records_reverse_imports_and_references
glob_duplicate_method_trait_macro_and_cfg_cases_remain_honestly_partial
rename_delete_and_module_move_change_generation_relationships
resolver_output_is_deterministic_under_manifest_order_changes
resolver_budget_exhaustion_preserves_partial_graph_and_limitations
```

- [ ] **Step 3: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test rust_repository_resolver
```

Expected: FAIL because the graph and resolver do not exist.

- [ ] **Step 4: Define repository graph domain types**

Add to `index/model.rs`:

```rust
use crate::impact_context::contracts::{
    Confidence, EdgeKind, Resolution, SourceRange,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryGraph {
    pub identity: GraphGenerationIdentity,
    pub files: Vec<GraphFile>,
    pub modules: Vec<GraphModule>,
    pub symbols: Vec<GraphSymbol>,
    pub edges: Vec<GraphEdge>,
    pub completeness: Completeness,
    pub limitations: Vec<IndexLimitation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphFile {
    pub path: RepoPath,
    pub mode: String,
    pub presence: CandidatePresence,
    pub content_sha256: Option<String>,
    pub file_fact_key: Option<FileFactKey>,
    pub language: Option<String>,
    pub module_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphModule {
    pub module_id: String,
    pub parent_module_id: Option<String>,
    pub crate_name: String,
    pub path: RepoPath,
    pub inline: bool,
    pub root_module: bool,
    pub resolution_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphSymbol {
    pub symbol_id: String,
    pub local_id: String,
    pub module_id: String,
    pub path: RepoPath,
    pub language: String,
    pub kind: String,
    pub name: String,
    pub owner_symbol_id: Option<String>,
    pub signature: Option<String>,
    pub visibility: Option<String>,
    pub range: SourceRange,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdge {
    pub edge_id: String,
    pub kind: EdgeKind,
    pub from_symbol: String,
    pub to_symbol: Option<String>,
    pub unresolved_target: Option<String>,
    pub path: RepoPath,
    pub range: SourceRange,
    pub provider_id: String,
    pub provider_version: String,
    pub resolution: Resolution,
    pub confidence: Confidence,
    pub limitation_code: Option<String>,
}
```

All graph vectors are sorted and deduplicated by stable ids. Unique
Tree-sitter-based cross-file binding is `ResolvedReference` with medium
confidence. Method, trait, glob, macro, cfg, and external-dependency uncertainty
never becomes semantic or high confidence.

- [ ] **Step 5: Implement the Rust resolver**

Export the resolver from `index/mod.rs`:

```rust
pub mod resolver;
```

Use explicit passes:

1. create target roots from `RustProjectModel`;
2. attach inline and file-backed modules;
3. create repository symbol ids from module/path/local facts;
4. build explicit import and re-export bindings;
5. bind unique lexical and qualified references;
6. classify call sites using resolved references where unique;
7. add reverse import/reference relationships;
8. record unresolved and polymorphic candidates with stable limitation codes.

No pass may read source bytes directly; it consumes manifests, project model,
and validated FileFacts only.

- [ ] **Step 6: Run resolver and upstream tests**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test rust_repository_resolver
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test rust_file_facts
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test rust_project_model
```

Expected: all tests PASS; ambiguous calls remain explicitly unresolved or
polymorphic.

- [ ] **Step 7: Commit heuristic repository resolution**

```bash
rtk git add collect-diff-context-cli/src/impact_context/index/mod.rs collect-diff-context-cli/src/impact_context/index/model.rs collect-diff-context-cli/src/impact_context/index/resolver/mod.rs collect-diff-context-cli/src/impact_context/index/resolver/rust.rs collect-diff-context-cli/tests/rust_repository_resolver.rs collect-diff-context-cli/tests/fixtures/repository_index/basic/Cargo.toml collect-diff-context-cli/tests/fixtures/repository_index/basic/src/lib.rs collect-diff-context-cli/tests/fixtures/repository_index/basic/src/api.rs collect-diff-context-cli/tests/fixtures/repository_index/basic/src/auth.rs collect-diff-context-cli/tests/fixtures/repository_index/basic/tests/auth_flow.rs collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/Cargo.toml collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/src/lib.rs collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/src/a.rs collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/src/b.rs collect-diff-context-cli/tests/fixtures/repository_index/ambiguous/src/caller.rs
rtk git commit -m "feat: resolve rust repository relationships"
```

### Task 7: Persist Immutable SQLite Repository Graph Generations

**Files:**
- Create: `collect-diff-context-cli/src/impact_context/cache/sqlite_generation.rs`
- Create: `collect-diff-context-cli/src/impact_context/cache/locking.rs`
- Create: `collect-diff-context-cli/tests/sqlite_repository_graph.rs`
- Modify: `collect-diff-context-cli/src/impact_context/cache/integrity.rs`
- Modify: `collect-diff-context-cli/src/impact_context/cache/mod.rs`

- [ ] **Step 1: Write failing generation writer tests**

Add tests named:

```rust
writer_creates_fixed_schema_and_digest_named_generation
writer_persists_outgoing_and_incoming_indexes
writer_validates_foreign_keys_counts_root_and_integrity
same_key_writers_converge_on_one_generation
different_generation_writer_does_not_block_immutable_reader
interrupted_writer_never_publishes_a_partial_generation
partial_generation_requires_complete_manifest_and_explicit_omissions
invalid_existing_generation_is_not_overwritten
```

- [ ] **Step 2: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test sqlite_repository_graph writer_
```

Expected: FAIL because the production generation writer does not exist.

- [ ] **Step 3: Implement bounded writer locks**

In `locking.rs`, create a private lock file inside `locks_dir` with
`OpenOptions::create(true).read(true).write(true)`. Use
`std::fs::File::try_lock()` in a loop capped by the caller deadline. Map
`TryLockError::WouldBlock` to `writer-busy` at deadline and every other error to
`writer-lock-failed`. Closing the file releases the lock; lock-file bytes never
authorize a generation.

- [ ] **Step 4: Implement the fixed SQLite schema**

Use schema version 1 with tables:

```sql
generation_meta
files
modules
symbols
edges
limitations
```

Required indexes:

```sql
CREATE INDEX edges_from_kind_id ON edges(from_symbol, kind, edge_id);
CREATE INDEX edges_to_kind_id ON edges(to_symbol, kind, edge_id) WHERE to_symbol IS NOT NULL;
CREATE INDEX edges_path_id ON edges(path, edge_id);
CREATE INDEX symbols_path_id ON symbols(path, symbol_id);
CREATE INDEX symbols_module_name ON symbols(module_id, name, symbol_id);
```

Use fixed prepared statements and transactions. Set `application_id`,
`user_version`, DELETE journal mode, synchronous EXTRA, foreign keys ON, and
trusted schema OFF. Enforce path, string, row, and database-page limits before
commit.

Expose only this cache Interface to the graph builder:

```rust
pub struct RepositoryGraphWriter {
    layout: CacheLayout,
}

impl RepositoryGraphWriter {
    pub fn publish(
        &self,
        graph: &RepositoryGraph,
        budget: &mut IndexBudgetTracker,
    ) -> Result<GraphPublishOutcome, RepositoryGraphError>;
}

pub enum GraphPublishOutcome {
    Published { path: PathBuf },
    Reused { path: PathBuf },
}

#[derive(Debug)]
pub struct RepositoryGraphError {
    pub code: &'static str,
    pub message: String,
}
```

- [ ] **Step 5: Implement application integrity and publication**

`integrity.rs` must compute a canonical root over path/id-sorted files, modules,
symbols, edges, limitations, counts, and graph identity. Before publication:

- `PRAGMA foreign_key_check` returns no rows;
- `PRAGMA integrity_check` returns exactly `ok`;
- stored counts equal SQL counts;
- recomputed application root equals metadata;
- the database file is no larger than `max_generation_bytes`.

Close SQLite, sync the staging `NamedTempFile`, and publish with
`persist_noclobber`. Validate and reuse a valid existing file. Leave an invalid
existing file untouched and report it for explicit quarantine.

- [ ] **Step 6: Run and verify green**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test sqlite_repository_graph writer_
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test sqlite_repository_graph interrupted_
```

Expected: all writer, concurrency, and interruption tests PASS.

- [ ] **Step 7: Commit immutable graph generations**

```bash
rtk git add collect-diff-context-cli/src/impact_context/cache/mod.rs collect-diff-context-cli/src/impact_context/cache/sqlite_generation.rs collect-diff-context-cli/src/impact_context/cache/locking.rs collect-diff-context-cli/src/impact_context/cache/integrity.rs collect-diff-context-cli/tests/sqlite_repository_graph.rs
rtk git commit -m "feat: persist immutable repository graphs"
```

### Task 8: Add Immutable Readers, Inspection, and Corruption Classification

**Files:**
- Modify: `collect-diff-context-cli/src/impact_context/cache/sqlite_generation.rs`
- Modify: `collect-diff-context-cli/src/impact_context/cache/integrity.rs`
- Modify: `collect-diff-context-cli/tests/sqlite_repository_graph.rs`

- [ ] **Step 1: Write failing read and corruption tests**

Add tests named:

```rust
immutable_reader_opens_with_query_only_and_creates_no_sidecars
reader_validates_identity_schema_counts_and_consumed_rows
reader_returns_sorted_bounded_outgoing_and_incoming_edges
missing_generation_is_miss
header_truncation_index_damage_bad_enum_bad_digest_and_bad_range_are_corrupt
reader_never_runs_migration_repair_checkpoint_or_full_integrity_scan
reader_returns_immediately_while_another_generation_is_built
```

The sidecar test snapshots every filename in the graphs directory before and
after 100 reads and requires exact equality.

- [ ] **Step 2: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test sqlite_repository_graph immutable_reader_
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test sqlite_repository_graph reader_
```

Expected: FAIL until production immutable reads and row validation exist.

- [ ] **Step 3: Implement immutable open and metadata validation**

Create:

```rust
pub struct RepositoryGraphReader {
    connection: rusqlite::Connection,
    identity: GraphGenerationIdentity,
    completeness: Completeness,
    limits: ReaderLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderLimits {
    pub maximum_database_bytes: u64,
    pub maximum_rows_per_query: usize,
    pub maximum_string_bytes: usize,
}

impl RepositoryGraphReader {
    pub fn open_immutable(
        path: &Path,
        expected: &GraphGenerationIdentity,
        limits: ReaderLimits,
    ) -> Result<CacheLookup<Self>, RepositoryGraphError>;
    pub fn outgoing(&self, symbol: &str, maximum_rows: usize) -> Result<Vec<GraphEdge>, RepositoryGraphError>;
    pub fn incoming(&self, symbol: &str, maximum_rows: usize) -> Result<Vec<GraphEdge>, RepositoryGraphError>;
}
```

Use a percent-encoded `file:` URI with `mode=ro&immutable=1` and flags
`READ_ONLY | URI | NO_MUTEX`. Set `query_only` and `trusted_schema` defensively.
Do not set a busy timeout. Reject a per-call `maximum_rows` of zero or greater
than `limits.maximum_rows_per_query`; use that exact accepted value as the SQL
limit.

- [ ] **Step 4: Validate every consumed row**

Map SQL text to existing `EdgeKind`, `Resolution`, and `Confidence` using strict
match functions. Validate lowercase ids, safe `RepoPath`, one-based ordered
ranges, optional target invariants, provider identity, and row count. Unexpected
values are corruption and invalidate the generation for that query.

- [ ] **Step 5: Run and verify green**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test sqlite_repository_graph
```

Expected: all tests PASS and immutable reads create no sidecars.

- [ ] **Step 6: Commit immutable graph reads**

```bash
rtk git add collect-diff-context-cli/src/impact_context/cache/sqlite_generation.rs collect-diff-context-cli/src/impact_context/cache/integrity.rs collect-diff-context-cli/tests/sqlite_repository_graph.rs
rtk git commit -m "feat: read immutable repository graphs"
```

### Task 9: Build Exact Candidate Overlays

**Files:**
- Create: `collect-diff-context-cli/src/impact_context/index/overlay.rs`
- Create: `collect-diff-context-cli/tests/repository_overlay.rs`
- Modify: `collect-diff-context-cli/src/impact_context/index/mod.rs`

- [ ] **Step 1: Write failing overlay tests**

Add tests named:

```rust
changed_path_tombstones_all_base_symbols_and_source_edges
addition_replacement_delete_and_rename_use_exact_candidate_facts
overlay_precedence_is_tombstone_then_replacement_then_base
public_symbol_and_import_change_refresh_known_reverse_dependents
glob_macro_cfg_and_budget_limits_mark_closure_partial
incoming_edges_to_deleted_symbols_remain_visible_as_unresolved_impact
staged_overlay_uses_stage_zero_bytes_not_worktree_bytes
unstaged_overlay_binds_exact_index_base_and_tracked_worktree_delta
overlay_output_is_deterministic
```

- [ ] **Step 2: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_overlay
```

Expected: FAIL because no overlay model exists.

- [ ] **Step 3: Define overlay structures**

Export the overlay from `index/mod.rs`:

```rust
pub mod overlay;
```

Add:

```rust
pub struct RepositoryOverlay {
    pub base_generation_key: String,
    pub candidate_manifest_digest: String,
    pub path_tombstones: BTreeSet<RepoPath>,
    pub files: BTreeMap<RepoPath, GraphFile>,
    pub modules: BTreeMap<String, GraphModule>,
    pub symbols: BTreeMap<String, GraphSymbol>,
    pub outgoing_edges: BTreeMap<String, Vec<GraphEdge>>,
    pub incoming_edges: BTreeMap<String, Vec<GraphEdge>>,
    pub suppressed_base_edge_ids: BTreeSet<String>,
    pub completeness: Completeness,
    pub limitations: Vec<IndexLimitation>,
}
```

Build overlays only from authoritative changed paths and exact candidate
FileFacts. Enforce `max_overlay_paths`, symbol, edge, byte, node, and deadline
budgets.

- [ ] **Step 4: Implement reverse-dependent invalidation**

For changes to module declarations, imports, re-exports, public symbols, or
visibility:

- query known reverse import/reference dependents from the base reader;
- re-resolve each dependent using unchanged FileFacts plus overlay facts;
- suppress base source edges for every refreshed dependent path;
- stop with partial completeness when closure, row, or deadline budgets exhaust.

Do not claim that glob, macro, cfg, trait, or external dependency closures are
complete.

- [ ] **Step 5: Run and verify green**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_overlay
```

Expected: all overlay and candidate-byte tests PASS.

- [ ] **Step 6: Commit exact candidate overlays**

```bash
rtk git add collect-diff-context-cli/src/impact_context/index/mod.rs collect-diff-context-cli/src/impact_context/index/overlay.rs collect-diff-context-cli/tests/repository_overlay.rs
rtk git commit -m "feat: overlay exact candidate graph changes"
```

### Task 10: Add Bounded Deterministic Graph Traversal

**Files:**
- Create: `collect-diff-context-cli/src/impact_context/index/traversal.rs`
- Create: `collect-diff-context-cli/tests/repository_traversal.rs`
- Modify: `collect-diff-context-cli/src/impact_context/index/mod.rs`

- [ ] **Step 1: Write failing traversal tests**

Add tests named:

```rust
one_hop_returns_sorted_incoming_and_outgoing_edges
two_hop_breadth_first_traversal_deduplicates_cycles
overlay_tombstones_and_replacements_override_base_rows
row_node_edge_byte_depth_and_deadline_budgets_return_partial
corrupt_row_invalidates_query_without_accepting_other_edges
index_completeness_query_completeness_and_output_truncation_are_independent
repeated_queries_are_deterministic_except_elapsed_metrics
```

- [ ] **Step 2: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_traversal
```

Expected: FAIL because the traversal engine does not exist.

- [ ] **Step 3: Define traversal request and result**

Export traversal from `index/mod.rs`:

```rust
pub mod traversal;
```

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TraversalDirection {
    Incoming,
    Outgoing,
}

pub struct TraversalRequest {
    pub roots: Vec<String>,
    pub directions: BTreeSet<TraversalDirection>,
    pub edge_kinds: BTreeSet<EdgeKind>,
    pub maximum_depth: usize,
    pub maximum_rows: usize,
    pub maximum_nodes: usize,
    pub maximum_edges: usize,
    pub maximum_bytes: usize,
    pub deadline: Duration,
}

pub struct TraversalResult {
    pub edges: Vec<GraphEdge>,
    pub reached_depth: usize,
    pub rows_read: usize,
    pub nodes_visited: usize,
    pub bytes_read: usize,
    pub completeness: Completeness,
    pub limitations: Vec<IndexLimitation>,
}
```

- [ ] **Step 4: Implement application-owned breadth-first traversal**

Use a `VecDeque` frontier sorted by stable symbol id at each depth. Visit identity
is `(direction, symbol_id, edge_kind)`. At each lookup:

- apply overlay tombstone/replacement/addition first;
- fetch remaining base rows with indexed SQL and an exact limit;
- validate before merging;
- deduplicate by edge id, keeping higher confidence only when ids match;
- consume every budget before adding the next frontier.

Do not use recursive CTEs and do not load all graph rows into memory.

- [ ] **Step 5: Run and verify green**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_traversal
```

Expected: all tests PASS, including cyclic and adversarial fan-out fixtures.

- [ ] **Step 6: Commit bounded traversal**

```bash
rtk git add collect-diff-context-cli/src/impact_context/index/mod.rs collect-diff-context-cli/src/impact_context/index/traversal.rs collect-diff-context-cli/tests/repository_traversal.rs
rtk git commit -m "feat: traverse repository impact graph"
```

### Task 11: Integrate the Repository Index Adapter with Impact Context

**Files:**
- Create: `collect-diff-context-cli/src/impact_context/adapters/repository_index.rs`
- Create: `collect-diff-context-cli/tests/repository_index_integration.rs`
- Modify: `collect-diff-context-cli/src/impact_context/adapters/mod.rs`
- Modify: `collect-diff-context-cli/src/impact_context/engine.rs`
- Modify: `collect-diff-context-cli/src/impact_context/budget.rs`
- Modify: `collect-diff-context-cli/src/impact_context/contracts.rs`
- Modify: `collect-diff-context-cli/src/impact_context/normalizer.rs`
- Modify: `collect-diff-context-cli/src/impact_context/summarizer.rs`
- Modify: `collect-diff-context-cli/schemas/impact-context.schema.json`

- [ ] **Step 1: Write failing integration tests**

Add tests named:

```rust
fast_mode_reads_compatible_generation_without_writes
fast_cache_miss_parses_only_changed_files_and_remains_valid
deep_mode_builds_missing_facts_and_generation_when_write_is_authorized
changed_symbols_seed_bounded_incoming_and_outgoing_traversal
repository_index_provider_reports_hits_misses_stale_corrupt_and_limitations
heuristic_edges_never_become_semantic_or_high_confidence
graph_index_query_and_output_completeness_remain_independent
scope_drift_after_index_query_invalidates_all_graph_evidence
```

The zero-write test must snapshot the entire cache directory metadata and names
before and after Fast collection and require no create, remove, length, or
modified-time change.

- [ ] **Step 2: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_index_integration
```

Expected: FAIL because `RepositoryIndexAdapter` and Deep mode do not exist.

- [ ] **Step 3: Define the adapter Interface**

Add:

```rust
pub struct RepositoryIndexRequest<'a> {
    pub candidate: &'a dyn CandidateContent,
    pub manifest_source: &'a dyn RepositoryManifestSource,
    pub changed_symbols: &'a [ChangedSymbol],
    pub mode: ImpactMode,
    pub cache_read: bool,
    pub cache_write: bool,
    pub index_budget: IndexBudget,
}

pub struct RepositoryIndexOutput {
    pub provider: ProviderRecord,
    pub edges: Vec<ImpactEdge>,
    pub domain_summaries: Vec<DomainSummary>,
    pub index_completeness: Completeness,
    pub query_completeness: Completeness,
    pub reached_depth: usize,
    pub limitations: Vec<Limitation>,
    pub metrics: IndexMetrics,
}
```

Provider kind is `repository-index`; provider version binds graph schema,
resolver, adapter/query, and normalization identities.

- [ ] **Step 4: Add `ImpactRequest::deep_defaults` and cache policies**

Fast defaults become `cache_read = true`, `cache_write = false`, maximum graph
depth 1, and retain the total 750ms deadline. Deep defaults use `IndexBudget`,
cache read/write true only for explicit Deep/index invocation, and maximum graph
depth 2.

`build_impact_context` must preserve existing Fast output when cache is absent.
Repository index failures add structured limitations but do not remove changed
file facts or ordinary review context.

- [ ] **Step 5: Merge and summarize graph evidence**

Map `GraphEdge` to `ImpactEdge` without changing provider, resolution, or
confidence. Merge by stable edge id and existing confidence rules. Add bounded
Domain Summaries for:

- direct incoming callers;
- direct outgoing calls;
- reverse import dependents;
- changed exported interfaces;
- connected test symbols.

Summaries cite evidence ids and never state that unresolved or polymorphic
candidates are confirmed calls.

- [ ] **Step 6: Run integration and contract regressions**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_index_integration
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test impact_context_contracts
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test impact_context_rust
rtk python3 scripts/validate_schemas.py
```

Expected: all tests and schemas PASS; Fast cache miss behavior remains compatible
with the accepted `impact_context/v1` contract.

- [ ] **Step 7: Commit Impact Context integration**

```bash
rtk git add collect-diff-context-cli/src/impact_context/adapters/mod.rs collect-diff-context-cli/src/impact_context/adapters/repository_index.rs collect-diff-context-cli/src/impact_context/engine.rs collect-diff-context-cli/src/impact_context/budget.rs collect-diff-context-cli/src/impact_context/contracts.rs collect-diff-context-cli/src/impact_context/normalizer.rs collect-diff-context-cli/src/impact_context/summarizer.rs collect-diff-context-cli/tests/repository_index_integration.rs collect-diff-context-cli/schemas/impact-context.schema.json
rtk git commit -m "feat: add repository graph impact context"
```

### Task 12: Add Index Build, Doctor, Inspect, and Clean CLI Commands

**Files:**
- Modify: `collect-diff-context-cli/src/bin/repository_context.rs`
- Create: `collect-diff-context-cli/src/impact_context/cache/cleanup.rs`
- Create: `collect-diff-context-cli/tests/repository_index_cli.rs`
- Modify: `collect-diff-context-cli/tests/repository_context_cli.rs`

- [ ] **Step 1: Write failing CLI tests**

Add tests named:

```rust
help_lists_collect_fast_deep_and_index_subcommands
index_build_requires_source_expected_scope_and_lower_only_limits
index_build_emits_valid_compact_report_and_publishes_generation
index_doctor_is_read_only_and_reports_corrupt_or_orphaned_objects
index_inspect_requires_exact_digest_path_or_symbol_and_bounds_rows
index_clean_defaults_to_dry_run_and_stays_inside_repository_namespace
index_clean_defers_in_use_windows_generations
collect_deep_revalidates_scope_after_cache_writes_and_queries
```

- [ ] **Step 2: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_index_cli
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_context_cli help_and_unsupported_subcommands_are_stable
```

Expected: FAIL because the new CLI surface is absent and the old help explicitly
rejects Deep/index.

- [ ] **Step 3: Refactor CLI parsing into focused command enums**

Define:

```rust
enum RepositoryContextCommand {
    Collect(CollectArgs),
    IndexBuild(IndexBuildArgs),
    IndexDoctor(IndexDoctorArgs),
    IndexInspect(IndexInspectArgs),
    IndexClean(IndexCleanArgs),
}
```

Support exactly:

```text
repository-context-cli collect --source <staged|unstaged|branch> --expect-scope <fingerprint> --mode <fast|deep> [limits]
repository-context-cli index build --source <...> --expect-scope <fingerprint> [index limits]
repository-context-cli index doctor [--cache-dir <absolute>] [--generation <digest>]
repository-context-cli index inspect --generation <digest> (--path <repo-path> | --symbol <id>) [--max-rows <n>]
repository-context-cli index clean [--dry-run|--execute] [--max-bytes <n>] [--retain-generations <n>] [--invalid]
```

Doctor and inspect are read-only. Clean mutates only after an explicit invocation;
`--dry-run` is the default and `--execute` is required to delete or quarantine.

- [ ] **Step 4: Implement bounded command reports**

All index commands emit `repository_index_report/v1`, one compact JSON object,
through the existing local secret sanitizer. Build and Deep collection open and
revalidate authoritative scope around every cache write and accepted query.

Doctor performs full FileFacts checksum and SQLite integrity checks only within
its explicit limits. Inspect never dumps the whole graph; it requires a selector
and row limit. Clean sorts candidates deterministically, refuses path escapes,
does not follow symlinks, and reports deferred files.

- [ ] **Step 5: Run focused and existing CLI tests**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_index_cli
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_context_cli
```

Expected: all tests PASS; legacy Fast invocation remains valid.

- [ ] **Step 6: Commit the operational CLI**

```bash
rtk git add collect-diff-context-cli/src/bin/repository_context.rs collect-diff-context-cli/src/impact_context/cache/cleanup.rs collect-diff-context-cli/tests/repository_index_cli.rs collect-diff-context-cli/tests/repository_context_cli.rs
rtk git commit -m "feat: add repository index operations"
```

### Task 13: Add Public Wrapper and Workflow Integration

**Files:**
- Create: `scripts/index_repository_context.sh`
- Create: `tests/repository_index_test.sh`
- Modify: `scripts/collect_impact_context.sh`
- Modify: `tests/repository_context_test.sh`
- Modify: `scripts/build_all_binaries.sh`
- Modify: `install.sh`
- Modify: `tests/install_smoke_test.sh`
- Modify: `tests/install_agent_matrix_test.sh`
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Write failing shell integration tests**

The new shell test must cover:

```text
wrapper rejects missing or malformed source/scope
wrapper resolves only an absolute override or trusted bundled binary
index build forwards exact arguments and compact JSON unchanged
doctor and inspect remain bounded and sanitized
clean requires explicit execute
missing binary returns a stable unavailable report without cache writes
staged index reads stage-zero bytes when worktree differs
```

Run:

```bash
rtk bash tests/repository_index_test.sh
```

Expected: FAIL because the wrapper does not exist.

- [ ] **Step 2: Implement a thin index wrapper**

`scripts/index_repository_context.sh` must:

- use `scripts/lib/repository_context_cli.sh` for binary resolution;
- accept `index build|doctor|inspect|clean` and forward all validated arguments;
- require absolute cache and binary overrides;
- capture stdout/stderr in private temporary files;
- apply the existing sanitizer protocol;
- never parse or rewrite valid Rust JSON;
- emit a stable unavailable index report when the binary is absent;
- never convert unavailable index context into a blocked ordinary review.

- [ ] **Step 3: Update the collection wrapper for Deep mode**

Allow `--mode deep` to pass through. The wrapper's unavailable artifact must
preserve the requested mode and set graph completeness unavailable. Fast behavior
and output heading remain unchanged.

- [ ] **Step 4: Update local multi-platform build copying**

The existing `repository-context-cli` binary already contains index commands.
Do not add another shipped binary. Verify `build_all_binaries.sh` copies the same
binary for all targets and its smoke tests run both `collect --help` and
`index --help`.

Update `install.sh` and release packaging so
`scripts/index_repository_context.sh` is installed and executable. Extend the
install smoke and host-matrix tests to require the new wrapper without changing
existing entrypoint behavior.

- [ ] **Step 5: Run shell and Rust integration tests**

Run:

```bash
rtk bash tests/repository_index_test.sh
rtk bash tests/repository_context_test.sh
rtk bash tests/install_smoke_test.sh
rtk bash tests/install_agent_matrix_test.sh
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_index_cli
```

Expected: all tests PASS.

- [ ] **Step 6: Commit public workflow integration**

```bash
rtk git add scripts/index_repository_context.sh scripts/collect_impact_context.sh scripts/build_all_binaries.sh install.sh tests/repository_index_test.sh tests/repository_context_test.sh tests/install_smoke_test.sh tests/install_agent_matrix_test.sh .github/workflows/release.yml
rtk git commit -m "feat: expose repository index workflow"
```

### Task 14: Add Fuzz, Fault, Performance, and Release Gates

**Files:**
- Create: `collect-diff-context-cli/fuzz/fuzz_targets/file_facts_decode.rs`
- Create: `collect-diff-context-cli/fuzz/fuzz_targets/repository_graph_row.rs`
- Create: `collect-diff-context-cli/fuzz/fuzz_targets/repository_overlay.rs`
- Create: `collect-diff-context-cli/fuzz/fuzz_targets/repository_traversal.rs`
- Create: `collect-diff-context-cli/benches/repository_index.rs`
- Delete: `collect-diff-context-cli/src/bin/sqlite_storage_spike.rs`
- Delete: `collect-diff-context-cli/tests/sqlite_storage_spike.rs`
- Delete: `collect-diff-context-cli/tests/fixtures/sqlite_storage_spike/README.md`
- Modify: `collect-diff-context-cli/fuzz/Cargo.toml`
- Modify: `collect-diff-context-cli/Cargo.toml`
- Modify: `.github/workflows/lint.yml`
- Modify: `.github/workflows/release.yml`
- Modify: `CONTRIBUTING.md`

- [ ] **Step 1: Add failing benchmark and adversarial regression tests**

Add release tests or benchmark assertions for:

```text
10k, 100k, and 1M symbol/edge generations
cold FileFacts creation and warm reuse
immutable generation open
one-hop and two-hop forward/reverse traversal
overlay construction and reverse-dependent refresh
corrupt rows and high fan-out
maximum path, string, range, row, and database sizes
```

The warm Deep one/two-hop P95 gate is two seconds on the documented CI corpus.
The existing Fast total 750ms release gate must remain unchanged and pass with a
compatible cache and a cache miss.

- [ ] **Step 2: Add four focused fuzz targets**

Use these entry contracts:

```rust
file_facts_decode: &[u8] -> bounded CacheLookup<RustFileFacts>
repository_graph_row: &[u8] -> strict row decoder result
repository_overlay: arbitrary small base/delta -> deterministic merge result
repository_traversal: arbitrary bounded graph -> terminating traversal result
```

Every target must cap allocations independently of fuzzer input and treat
decode errors as ordinary outcomes. Add permanent seeds for empty, corrupt,
partial, cyclic, high-fanout, rename, delete, and checksum-mismatch cases.

- [ ] **Step 3: Add the repository index benchmark**

Register:

```toml
[[bench]]
name = "repository_index"
harness = false
```

Benchmark stages independently: manifest, FileFacts hit/miss, project model,
resolver, SQLite build/validation, immutable open, forward query, reverse query,
overlay, traversal, normalization, serialization, and sanitization.

- [ ] **Step 4: Update CI and release dependency evidence**

In `.github/workflows/lint.yml` add:

- default and all-feature Clippy with `-D warnings`;
- repository index release performance tests;
- cache/row/overlay/traversal fuzz smoke;
- shell integration;
- schema validation;
- `actionlint`.

In `.github/workflows/release.yml`:

- build the normal product binary with bundled SQLite on all four targets;
- run `repository-context-cli index --help` smoke;
- run a small build/doctor/immutable-query smoke;
- require SBOM components `rusqlite@0.40.1`, `libsqlite3-sys@0.38.1`, and the
  locked TOML parser components;
- package SQLite and rusqlite license evidence.

After the production product-path build, doctor, immutable-read, performance,
and four-platform gates replace every B0 workflow assertion, remove the
temporary spike feature, bin declaration, source, tests, and fixture README in
the same commit. The accepted spike results document remains.

- [ ] **Step 5: Run focused gates locally**

Run:

```bash
rtk cargo fmt --manifest-path collect-diff-context-cli/Cargo.toml --all -- --check
rtk cargo clippy --manifest-path collect-diff-context-cli/Cargo.toml --all-targets --all-features -- -D warnings
rtk cargo test --release --manifest-path collect-diff-context-cli/Cargo.toml --test impact_context_performance -- --nocapture
rtk cargo test --release --manifest-path collect-diff-context-cli/Cargo.toml --test repository_index_integration -- --nocapture
rtk cargo bench --manifest-path collect-diff-context-cli/Cargo.toml --bench repository_index
rtk actionlint -oneline .github/workflows/lint.yml .github/workflows/release.yml
```

Expected: all hard gates PASS. Record benchmark values in test output rather than
hard-coding workstation-specific cold-index latency.

- [ ] **Step 6: Commit release quality gates**

```bash
rtk git add collect-diff-context-cli/Cargo.toml collect-diff-context-cli/fuzz/Cargo.toml collect-diff-context-cli/fuzz/fuzz_targets/file_facts_decode.rs collect-diff-context-cli/fuzz/fuzz_targets/repository_graph_row.rs collect-diff-context-cli/fuzz/fuzz_targets/repository_overlay.rs collect-diff-context-cli/fuzz/fuzz_targets/repository_traversal.rs collect-diff-context-cli/benches/repository_index.rs .github/workflows/lint.yml .github/workflows/release.yml CONTRIBUTING.md
rtk git add -u collect-diff-context-cli/src/bin/sqlite_storage_spike.rs collect-diff-context-cli/tests/sqlite_storage_spike.rs collect-diff-context-cli/tests/fixtures/sqlite_storage_spike/README.md
rtk git commit -m "test: gate persistent repository indexing"
```

### Task 15: Update Product Documentation and Capability Contracts

**Files:**
- Modify: `README.md`
- Modify: `README.zh-CN.md`
- Modify: `SKILL.md`
- Modify: `docs/helper-capabilities.md`
- Modify: `docs/superpowers/specs/2026-07-27-persistent-symbol-index-design.md`
- Modify: `docs/superpowers/plans/2026-07-27-persistent-symbol-index.md`
- Include: `docs/persistent-symbol-index-storage-engine-research.md`
- Include: `docs/persistent-symbol-index-sqlite-spike-results.md`

- [ ] **Step 1: Write failing documentation surface tests**

Extend existing README/skill surface tests to require:

```text
repository-context-cli index build
repository-context-cli index doctor
repository-context-cli index inspect
repository-context-cli index clean
heuristic repository graph
not compiler-complete
Fast Mode zero persistent writes
Deep/index explicit cache writes
```

Run the focused tests and verify they fail before documentation changes.

- [ ] **Step 2: Document public behavior in both languages**

README updates must explain:

- Fast versus Deep behavior;
- cache location and absolute override;
- immutable generation and staged overlay behavior;
- build, doctor, inspect, and clean examples;
- completeness and heuristic-resolution limitations;
- cache removal safety and no raw source storage;
- no automatic Cargo/build/dependency execution.

Chinese and English examples must use the same commands and limits.

- [ ] **Step 3: Update the skill and helper capability contract**

The skill may consume Fast compatible index context when the control plane
provides the fingerprint-bound command. It must not automatically run `index
build`, `collect --mode deep`, doctor, clean, rust-analyzer, or any cache-writing
operation during ordinary review.

`docs/helper-capabilities.md` must distinguish changed-file structural facts,
heuristic repository index facts, and future semantic provider facts.

- [ ] **Step 4: Preserve pre-release document status**

Keep the design status at its approved pre-implementation state and keep both
plans incomplete. Task 16 changes them only after every local and four-platform
gate passes. Do not rewrite the approved decision or remove rejected
alternatives.

- [ ] **Step 5: Run documentation tests and whitespace checks**

Run:

```bash
rtk bash evals/readme_surface_test.sh
rtk bash tests/skill_contract_test.sh
rtk git diff --check
```

Expected: all tests PASS and no whitespace diagnostics are printed.

- [ ] **Step 6: Commit product documentation**

Force-add only this project's approved ignored design and plan files.

```bash
rtk git add README.md README.zh-CN.md SKILL.md docs/helper-capabilities.md docs/persistent-symbol-index-storage-engine-research.md docs/persistent-symbol-index-sqlite-spike-results.md
rtk git add -f docs/superpowers/specs/2026-07-27-persistent-symbol-index-design.md docs/superpowers/plans/2026-07-27-persistent-symbol-index-storage-spike.md docs/superpowers/plans/2026-07-27-persistent-symbol-index.md
rtk git commit -m "docs: document persistent repository indexing"
```

### Task 16: Run the Complete Subproject B Verification Gate

**Files:**
- Modify only files required to fix failures caused by Subproject B

- [ ] **Step 1: Verify the working tree scope**

Run:

```bash
rtk git status --short
rtk git diff --stat 8b1e7e33e564ed84a2a073ece91ad040b4d9a31e...HEAD
rtk git diff --check 8b1e7e33e564ed84a2a073ece91ad040b4d9a31e...HEAD
```

Expected: only Subproject B files and approved documentation are present; no
unrelated ignored content is staged.

- [ ] **Step 2: Run Rust formatting, lint, tests, release, and benchmarks**

Run:

```bash
rtk cargo fmt --manifest-path collect-diff-context-cli/Cargo.toml --all -- --check
rtk cargo clippy --manifest-path collect-diff-context-cli/Cargo.toml --all-targets --all-features -- -D warnings
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --all-features
rtk cargo build --release --manifest-path collect-diff-context-cli/Cargo.toml --bins
rtk cargo test --release --manifest-path collect-diff-context-cli/Cargo.toml --test impact_context_performance -- --nocapture
rtk cargo test --release --manifest-path collect-diff-context-cli/Cargo.toml --test repository_index_integration -- --nocapture
rtk cargo bench --manifest-path collect-diff-context-cli/Cargo.toml --bench impact_context
rtk cargo bench --manifest-path collect-diff-context-cli/Cargo.toml --bench repository_index
```

Expected: every command PASS; Fast deadlines and Deep warm P95 gates pass.

- [ ] **Step 3: Run shell, schema, evaluation, and workflow gates**

Run:

```bash
rtk bash tests/repository_context_test.sh
rtk bash tests/repository_index_test.sh
rtk bash tests/full_review_workflow_test.sh
rtk bash tests/skill_contract_test.sh
rtk bash evals/eval_contract_test.sh
rtk bash evals/readme_surface_test.sh
rtk python3 scripts/validate_schemas.py
rtk shellcheck -S warning scripts/*.sh scripts/lib/*.sh tests/*.sh evals/*.sh
rtk actionlint -oneline .github/workflows/lint.yml .github/workflows/release.yml
```

Expected: all shell, schema, eval, ShellCheck, and actionlint gates PASS.

- [ ] **Step 4: Run sustained fuzz targets**

Run each target long enough to demonstrate continuing coverage growth and no
crash, then record run counts and elapsed time:

```bash
rtk cargo fuzz run file_facts_decode --fuzz-dir collect-diff-context-cli/fuzz -- -max_total_time=60
rtk cargo fuzz run repository_graph_row --fuzz-dir collect-diff-context-cli/fuzz -- -max_total_time=60
rtk cargo fuzz run repository_overlay --fuzz-dir collect-diff-context-cli/fuzz -- -max_total_time=60
rtk cargo fuzz run repository_traversal --fuzz-dir collect-diff-context-cli/fuzz -- -max_total_time=60
rtk cargo fuzz run tree_sitter_rust --fuzz-dir collect-diff-context-cli/fuzz -- -max_total_time=60
rtk cargo fuzz run impact_contract --fuzz-dir collect-diff-context-cli/fuzz -- -max_total_time=60
```

Expected: no crash or timeout caused by an unbounded loop. Keep only intentional
small regression seeds; remove transient generated corpus files from exact fuzz
corpus directories without touching tracked seeds.

- [ ] **Step 5: Verify four-platform CI and release artifacts**

Require green GitHub Actions evidence for:

- Linux `x86_64-unknown-linux-musl`;
- macOS `aarch64-apple-darwin`;
- macOS `x86_64-apple-darwin`;
- Windows `x86_64-pc-windows-msvc`;
- bundled SQLite build/doctor/query smoke;
- release SBOM and license closure.

Do not declare completion from local macOS tests alone.

- [ ] **Step 6: Perform commit-readiness review against the fixed base**

Review:

```bash
rtk git log --oneline 8b1e7e33e564ed84a2a073ece91ad040b4d9a31e..HEAD
rtk git diff --stat 8b1e7e33e564ed84a2a073ece91ad040b4d9a31e...HEAD
rtk git diff --check 8b1e7e33e564ed84a2a073ece91ad040b4d9a31e...HEAD
```

Inspect every changed contract, cache write, path operation, SQLite query,
completeness transition, wrapper, release file, and test. Fix only verified
Subproject B defects and rerun the affected gates. Commit each verified fix with
only its exact files before continuing; do not leave code changes for the final
documentation commit.

- [ ] **Step 7: Create the release-readiness completion commit**

After all gates and CI evidence pass, change the design status to `Implemented`,
the B0 plan status to `Completed`, and this plan status to `Completed`. Record the
date and final Subproject B commit id without changing the accepted rationale.

```bash
rtk git add -f docs/superpowers/specs/2026-07-27-persistent-symbol-index-design.md docs/superpowers/plans/2026-07-27-persistent-symbol-index-storage-spike.md docs/superpowers/plans/2026-07-27-persistent-symbol-index.md
rtk git commit -m "docs: close persistent repository indexing"
```

Expected: the worktree is clean and the complete B stack is ready to remain on
`feature/SAST` or be integrated only when the user explicitly requests it.

## Subproject B Acceptance Checklist

- [ ] B0 records a four-platform SQLite `Go` decision.
- [ ] Whole-candidate staged, unstaged, and branch manifests are exact and bounded.
- [ ] Full-file Rust syntax facts are path independent and deterministic.
- [ ] FileFacts are content addressed, immutable, validated, and reusable.
- [ ] Cargo project metadata is parsed passively without executing repository tools.
- [ ] Rust module/import/reference/call resolution is heuristic and honestly limited.
- [ ] Repository graphs publish as immutable, validated, no-clobber SQLite generations.
- [ ] Fast readers create no persistent files and never wait for writers.
- [ ] Exact candidate overlays suppress and replace base relationships correctly.
- [ ] One-hop and two-hop traversal is deterministic and bounded.
- [ ] `impact_context/v1` reports independent index, query, and output completeness.
- [ ] Build, doctor, inspect, and clean CLI commands are bounded and path safe.
- [ ] Fuzz, fault, performance, schema, shell, and documentation gates pass.
- [ ] Four-platform release binaries, licenses, and SBOM pass.
- [ ] No Subproject C/D or IDE/PR integration work enters the B stack.
