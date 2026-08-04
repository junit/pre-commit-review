# Rust-Analyzer Repository Context Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Phase 2 Deliveries 1-3 as an opt-in, bounded rust-analyzer Call Hierarchy provider over an already materialized candidate snapshot.

**Architecture:** The new `repository_context_provider` module owns contracts, snapshot/range validation, JSON-RPC framing, managed session lifecycle, and rust-analyzer traversal. The provider borrows the existing `CandidateSnapshot`, accepts a separately authorized normalized `linkedProjects` model and profile, uses single-flight requests for deterministic budget accounting, and is unreachable from ordinary review, Fast Mode, repository indexing, SQLite persistence, and static-analysis orchestration.

**Tech Stack:** Rust 1.95, Serde/serde_json, SHA-256, `url = "=2.5.7"`, synchronous std I/O/channels, existing Unix process groups and Windows Job Objects, cargo-fuzz/libFuzzer, JSON Schema Draft 2020-12, GitHub Actions.

---

## File Map And Boundaries

The first implementation cycle ends after Task 10. It does not add a provider CLI, profile registry/distribution, real rust-analyzer artifacts, sustained fuzzing, semantic persistence, or release claims beyond the fake-server and cross-platform fixture gates.

Create:

- `collect-diff-context-cli/src/repository_context_provider/mod.rs`: public invocation and submodule exports.
- `collect-diff-context-cli/src/repository_context_provider/contract.rs`: request, profile, normalized project model, report, limits, ranges, statuses, and validation.
- `collect-diff-context-cli/src/repository_context_provider/snapshot.rs`: borrowed snapshot boundary, provider file paths, source budget, URI mapping, and position conversion.
- `collect-diff-context-cli/src/repository_context_provider/json_rpc.rs`: incremental framing, message types, request encoding, and correlation counters.
- `collect-diff-context-cli/src/repository_context_provider/session.rs`: bounded reader threads, managed interactive child, deadline/cancellation, server-request handling, and lifecycle cleanup.
- `collect-diff-context-cli/src/repository_context_provider/rust_analyzer.rs`: typed profile configuration, linked-project initialization, capability gate, Call Hierarchy wire types, traversal, normalization, and status mapping.
- `collect-diff-context-cli/src/trusted_runtime.rs`: shared private runtime and pinned executable copy.
- `collect-diff-context-cli/src/bin/repository_context_provider_fixture.rs`: independent fake LSP server for deterministic tests.
- `collect-diff-context-cli/tests/repository_context_provider_contracts.rs`
- `collect-diff-context-cli/tests/repository_context_provider_snapshot.rs`
- `collect-diff-context-cli/tests/repository_context_json_rpc.rs`
- `collect-diff-context-cli/tests/repository_context_session.rs`
- `collect-diff-context-cli/tests/repository_context_rust_analyzer.rs`
- `collect-diff-context-cli/tests/repository_context_provider_platform.rs`
- `collect-diff-context-cli/fuzz/fuzz_targets/repository_context_frame.rs`
- `collect-diff-context-cli/fuzz/fuzz_targets/repository_context_messages.rs`
- `collect-diff-context-cli/fuzz/corpus/repository_context_frame/empty`
- `collect-diff-context-cli/fuzz/corpus/repository_context_frame/content-length`
- `collect-diff-context-cli/fuzz/corpus/repository_context_messages/response`
- `collect-diff-context-cli/schemas/repository-context-provider-request.schema.json`
- `collect-diff-context-cli/schemas/repository-context-provider-profile.schema.json`
- `collect-diff-context-cli/schemas/repository-context-project-model.schema.json`
- `collect-diff-context-cli/schemas/repository-context-provider-report.schema.json`
- `THIRD_PARTY_LICENSES/url-LICENSE-APACHE`
- `THIRD_PARTY_LICENSES/url-LICENSE-MIT`
- `docs/rust-analyzer-context-provider.md`

Modify only for the named boundary:

- `collect-diff-context-cli/src/lib.rs:1-13`: export the provider and shared runtime modules.
- `collect-diff-context-cli/src/candidate/snapshot.rs:610-730`: make tree/mode/directory/VCS revalidation match the design.
- `collect-diff-context-cli/src/static_analysis/executor.rs:1-544`: use extracted runtime helpers without changing one-shot output.
- `collect-diff-context-cli/src/process_group.rs:29-169`: preserve platform setup while allowing a Drop-safe owner to terminate and reap.
- `collect-diff-context-cli/Cargo.toml:28-43` and `Cargo.lock`: pin `url` and register the test fixture.
- `collect-diff-context-cli/fuzz/Cargo.toml` and `fuzz/Cargo.lock`: register both provider targets.
- `.github/workflows/lint.yml:30-92,94-143`: Rust 1.95 locked gate, fuzz smoke, and platform-focused provider tests.
- `.github/workflows/release.yml:172-210`: assert the pinned `url` component in the SBOM and packaged license notices.
- `collect-diff-context-cli/fuzz/README.md:1-14`: document smoke and deferred sustained commands.
- `docs/helper-capabilities.md` and `docs/call-graph-open-source-options.md`: document opt-in status and Delivery 4/5 deferral.

### Task 1: Contracts, Profile Authorization, And Linked-Project Model

**Files:**

- Create: `collect-diff-context-cli/src/repository_context_provider/mod.rs`
- Create: `collect-diff-context-cli/src/repository_context_provider/contract.rs`
- Create: `collect-diff-context-cli/tests/repository_context_provider_contracts.rs`
- Create: the four provider/project-model JSON schemas listed in the file map
- Modify: `collect-diff-context-cli/src/lib.rs:1-13`

- [ ] **Step 1: Write failing contract tests**

Build valid typed values and mutate each binding, status, limit, range, ID, and unknown field. The minimum public assertions are:

```rust
#[test]
fn valid_request_profile_model_and_report_round_trip() {
    let request = valid_request();
    request.validate().unwrap();
    let profile = valid_profile();
    profile.validate().unwrap();
    let model = valid_project_model();
    model.validate().unwrap();
    let report = valid_report();
    report.validate().unwrap();
    assert_eq!(serde_json::from_slice::<RepositoryContextProviderRequest>(
        &serde_json::to_vec(&request).unwrap()
    ).unwrap(), request);
    assert_eq!(serde_json::from_slice::<AuthorizedProviderProfile>(
        &serde_json::to_vec(&profile).unwrap()
    ).unwrap(), profile);
}

#[test]
fn request_rejects_empty_seeds_duplicate_directions_and_raised_limits() {
    let mut request = valid_request();
    request.seeds.clear();
    assert!(request.validate().is_err());
    let mut request = valid_request();
    request.directions = vec![CallDirection::Incoming, CallDirection::Incoming];
    assert!(request.validate().is_err());
    let mut request = valid_request();
    request.limits.max_depth = 3;
    assert!(request.validate().is_err());
}

#[test]
fn report_keeps_seed_mapping_and_related_symbols_separate() {
    let mut report = valid_report();
    report.related_symbols.push(report.seed_symbols[0].symbol.clone());
    assert!(report.validate().is_err());
    report.edges[0].from_symbol = "missing".to_string();
    assert!(report.validate().is_err());
}
```

Add mutations for wrong schema/kind, upper-case or short digests, profile path inside the snapshot, executable/config/profile digest mismatch, target/toolchain mismatch, malformed model dependencies, duplicate IDs, unbounded text, zero limits, invalid status/completeness pairs, non-end-exclusive ranges, and report bytes above the authorized maximum.

- [ ] **Step 2: Run the contract test and observe the missing module**

Run:

```bash
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test repository_context_provider_contracts
```

Expected: compilation fails because `repository_context_provider` and its contract types are absent.

- [ ] **Step 3: Define the stable contract types and immutable maxima**

Use `#[serde(deny_unknown_fields)]` on every object. Keep the provider status separate from `impact_context::contracts::ProviderStatus`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepositoryContextProviderStatus {
    Completed, Partial, Unavailable, Timeout, InvalidOutput, Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderCompleteness { Complete, Partial, Unavailable, Unknown }

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallDirection { Incoming, Outgoing }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SeedKind {
    Function, Method, AssociatedFunction,
    FunctionDeclaration, MethodDeclaration, AssociatedFunctionDeclaration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderRangeFormat {
    #[serde(rename = "provider-source-range-v1/utf8-byte-columns/end-exclusive")]
    Utf8ByteColumnsEndExclusiveV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRange {
    pub format: ProviderRangeFormat,
    pub start_line: u32, pub start_column: u32,
    pub end_line: u32, pub end_column: u32,
    pub start_byte: usize, pub end_byte: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderLimits {
    pub deadline_ms: u64, pub max_depth: u8, pub max_seeds: usize,
    pub max_requests: usize, pub max_pending_requests: usize,
    pub max_messages: usize, pub max_notifications: usize,
    pub max_server_requests: usize, pub max_invalid_messages: usize,
    pub max_call_ranges: usize, pub max_header_bytes: usize,
    pub max_frame_bytes: usize, pub max_protocol_bytes: usize,
    pub max_stderr_bytes: usize, pub max_total_output_bytes: usize,
    pub max_source_file_bytes: usize, pub max_source_bytes: usize,
    pub max_nodes: usize, pub max_edges: usize, pub max_report_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedSymbol {
    pub changed_symbol_id: String, pub path: String, pub kind: SeedKind,
    pub name: String, pub symbol_range: ProviderRange,
    pub selection_range: ProviderRange, pub query_byte: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateBinding {
    pub source: ReviewSource, pub scope_fingerprint: String,
    pub candidate_digest: String, pub snapshot_root: PathBuf,
    pub snapshot_sha256: String, pub snapshot_files: usize,
    pub snapshot_bytes: u64, pub project_model_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderBinding {
    pub kind: String, pub version: String, pub profile_path: PathBuf,
    pub profile_sha256: String, pub executable_path: PathBuf,
    pub executable_sha256: String, pub configuration_sha256: String,
    pub target_triple: String, pub toolchain_mode: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryContextProviderRequest {
    pub schema_version: u8, pub kind: String, pub candidate: CandidateBinding,
    pub provider: ProviderBinding, pub seeds: Vec<SeedSymbol>,
    pub directions: Vec<CallDirection>, pub limits: ProviderLimits,
}
```

Define `ContractError { code: &'static str, message: String }`, `ProfileError`, and `ProjectModelError` with bounded display text. Define `AuthorizedProviderProfile` with the binding, fixed argument list, target triple, `toolchain_mode = "none"`, typed hardening (`cargo.buildScripts.enable=false`, `cargo.noDeps=true`, `procMacro.enable=false`, `checkOnSave.enable=false`, no workspace discovery), immutable maxima, and a canonical `sha256()` method. Define `RustAnalyzerProjectModel` with `algorithm`, `digest`, `target_triple`, `crates`, `cfg`, `env`, and sorted `limitations`; each crate has a snapshot-relative `root_module`, `edition`, and sorted dependency records. Its `validate()` recomputes the digest from the full canonical model, rejects duplicate crate/dependency IDs and outside roots, and never trusts a digest string alone.

Define report-only `ReportedCandidateBinding` without `snapshot_root`, `ProviderExecutionRecord` (including profile/executable/configuration digests and negotiated encoding), `SeedContextSymbol { changed_symbol_id, symbol }`, `ContextSymbol`, `SemanticCallEdge`, `ProviderLimitation`, `ProviderIsolation`, and `ProviderMetrics`. The report owns status and both completeness fields at top level:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryContextProviderReport {
    pub schema_version: u8,
    pub kind: String,
    pub candidate: ReportedCandidateBinding,
    pub provider: ProviderExecutionRecord,
    pub status: RepositoryContextProviderStatus,
    pub index_completeness: ProviderCompleteness,
    pub query_completeness: ProviderCompleteness,
    pub seed_symbols: Vec<SeedContextSymbol>,
    pub related_symbols: Vec<ContextSymbol>,
    pub edges: Vec<SemanticCallEdge>,
    pub limitations: Vec<ProviderLimitation>,
    pub isolation: ProviderIsolation,
    pub metrics: ProviderMetrics,
}
```

`index_completeness` is always `Unknown` in this cycle. Edge endpoints must exist in `seed_symbols ∪ related_symbols`; each call range is one edge; report arrays are sorted by IDs.

The binding digest used for symbol and edge IDs length-prefixes scope, candidate, snapshot, model algorithm/digest, profile, provider/version, executable, configuration, target, path, kind/name, symbol range, selection range, and call range. IDs are report-local and must not be treated as cross-report identity.

- [ ] **Step 4: Add four strict JSON schemas**

Create request, profile, project-model, and report schemas with Draft 2020-12, `additionalProperties: false` at every object, all required fields, exact enum values, lower-case hex patterns, end-exclusive provider ranges, bounded arrays/text/integers, and no local snapshot path/raw stderr/raw JSON-RPC/opaque LSP data in the report. The request schema must require non-empty seeds/directions and the profile schema must require the no-toolchain hardening values.

Run:

```bash
rtk python3 scripts/validate_schemas.py
```

Expected: all repository schemas, including the four new schemas, validate and the command exits 0.

- [ ] **Step 5: Run, format, and commit**

```bash
rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/Cargo.toml -- --check
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test repository_context_provider_contracts
rtk git diff --check
rtk git add collect-diff-context-cli/src/lib.rs collect-diff-context-cli/src/repository_context_provider collect-diff-context-cli/tests/repository_context_provider_contracts.rs collect-diff-context-cli/schemas/repository-context-provider-*.schema.json collect-diff-context-cli/schemas/repository-context-project-model.schema.json
rtk git commit -m "feat(provider): define bound context contracts"
```

Expected: tests and schema validation pass and the commit contains no existing impact contract changes.

### Task 2: Harden Candidate Snapshot And Bind The Normalized Model

**Files:**

- Modify: `collect-diff-context-cli/src/candidate/snapshot.rs:610-730`
- Create: `collect-diff-context-cli/src/repository_context_provider/snapshot.rs`
- Modify: `collect-diff-context-cli/src/repository_context_provider/contract.rs`
- Create: `collect-diff-context-cli/tests/repository_context_provider_snapshot.rs`
- Modify: `collect-diff-context-cli/tests/static_execution_platform.rs`

- [ ] **Step 1: Write failing verifier and bound-view tests**

Add tests for mode-only mutation, added/removed empty directories, `.git` file/directory mutation, root and nested `rust-analyzer.toml`, writable snapshot, unsafe symlink, changed content, and digest/file/byte mismatch. Add provider boundary tests showing that a bare directory cannot create a bound view and that the view borrows a `CandidateSnapshot`.

```rust
#[test]
fn bound_view_requires_the_exact_materialized_snapshot_and_model() {
    let fixture = ProviderFixture::new();
    let bound = BoundCandidateSnapshot::new(
        &fixture.snapshot,
        &fixture.model,
        &fixture.request.candidate,
    ).unwrap();
    assert_eq!(bound.root(), fixture.snapshot.path());
    assert_eq!(bound.model().digest, fixture.model.digest);
    let mut changed = fixture.request.candidate.clone();
    changed.snapshot_sha256 = "0".repeat(64);
    assert!(BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &changed).is_err());
}

#[test]
fn source_path_and_budget_reject_escape_vcs_directory_and_oversize() {
    let fixture = ProviderFixture::new();
    let bound = fixture.bound();
    assert!(SnapshotFilePath::new("../escape.rs").is_err());
    assert!(SnapshotFilePath::new("src//lib.rs").is_err());
    let mut budget = SnapshotSourceBudget::new(1, 1).unwrap();
    assert!(bound.read_source(&SnapshotFilePath::new("src/lib.rs").unwrap(), &mut budget).is_err());
}
```

Use private snapshot test helpers to create `.git` and empty-directory mutations before `verify_unchanged`; do not weaken production read-only permissions merely to test them.

- [ ] **Step 2: Run the test and observe missing hardening**

```bash
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test repository_context_provider_snapshot
```

Expected: compilation fails for `BoundCandidateSnapshot`, `SnapshotFilePath`, and `SnapshotSourceBudget`, and existing snapshot tests still compile.

- [ ] **Step 3: Harden `CandidateSnapshot` hashing and revalidation**

Change `snapshot_info`/`HashState` so directory entries, including empty directories, are sorted and included in the digest; compare the observed mode map to the stored mode map rather than substituting stored modes into the observed hash; reject any `.git` file or directory at every recursion; preserve safe symlink target bytes and containment checks. Add focused tests in `tests/static_execution_platform.rs` for mode-only and directory mutation, then run all existing snapshot tests before touching the provider view.

- [ ] **Step 4: Implement the borrowed provider boundary**

```rust
pub struct BoundCandidateSnapshot<'a> {
    snapshot: &'a CandidateSnapshot,
    model: &'a RustAnalyzerProjectModel,
    binding: ReportedCandidateBinding,
    canonical_root: PathBuf,
}

pub struct SnapshotSourceBudget { max_file_bytes: usize, remaining_bytes: usize }

impl SnapshotSourceBudget {
    pub fn new(max_file_bytes: usize, total_bytes: usize)
        -> Result<Self, SnapshotBoundaryError>;
}

impl<'a> BoundCandidateSnapshot<'a> {
    pub fn new(
        snapshot: &'a CandidateSnapshot,
        model: &'a RustAnalyzerProjectModel,
        binding: &CandidateBinding,
    ) -> Result<Self, SnapshotBoundaryError>;
    pub fn root(&self) -> &Path;
    pub fn model(&self) -> &RustAnalyzerProjectModel;
    pub fn reported_binding(&self) -> &ReportedCandidateBinding;
    pub fn read_source(
        &self, path: &SnapshotFilePath, budget: &mut SnapshotSourceBudget,
    ) -> Result<Arc<[u8]>, SnapshotBoundaryError>;
    pub fn verify_unchanged(&self) -> Result<(), SnapshotBoundaryError>;
}
```

The constructor requires a canonical absolute root equal to `snapshot.path()`, calls `snapshot.verify_unchanged()`, compares all candidate/snapshot/model fields, validates the model against the snapshot roots and digest, rejects `.git` and every repository-controlled `rust-analyzer.toml` at any depth, and never invokes Git. `SnapshotFilePath::new` accepts only normalized non-empty normal components; `read_source` canonicalizes and checks strict containment, regular-file type, per-file/total source budgets, and valid UTF-8 Rust bytes. Errors contain stable codes and no local paths.

- [ ] **Step 5: Verify the model is the model sent to rust-analyzer**

Implement `RustAnalyzerProjectModel::linked_project_value()` with deterministic crate/dependency order and snapshot-relative roots. The later session supplies that exact canonical JSON object as the sole inline `linkedProjects` element; no random runtime path enters the configuration digest and no Cargo workspace discovery is used. Add tests that alter any crate root, edition, cfg, dependency, target, or limitation and observe a digest mismatch before spawn.

- [ ] **Step 6: Run regressions and commit**

```bash
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test repository_context_provider_snapshot --test static_execution_platform
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test rust_project_model
rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets -- -D warnings
rtk git diff --check
rtk git add collect-diff-context-cli/src/candidate/snapshot.rs collect-diff-context-cli/src/repository_context_provider collect-diff-context-cli/tests/repository_context_provider_snapshot.rs collect-diff-context-cli/tests/static_execution_platform.rs
rtk git commit -m "feat(provider): enforce snapshot and model identity"
```

Expected: all existing snapshot behavior remains green, with new mode/directory/VCS mutations rejected.

### Task 3: Strict File URIs And Versioned End-Exclusive Ranges

**Files:**

- Modify: `collect-diff-context-cli/Cargo.toml:28-38`
- Modify: `collect-diff-context-cli/Cargo.lock`
- Modify: `collect-diff-context-cli/src/repository_context_provider/snapshot.rs`
- Modify: `collect-diff-context-cli/tests/repository_context_provider_snapshot.rs`
- Create: `THIRD_PARTY_LICENSES/url-LICENSE-APACHE`
- Create: `THIRD_PARTY_LICENSES/url-LICENSE-MIT`
- Modify: `.github/workflows/release.yml:186-199`

- [ ] **Step 1: Add the pinned URI dependency and failing mapping tests**

Add `url = "=2.5.7"`, then update only that package in the lockfile:

```bash
rtk cargo +1.95.0 update --manifest-path collect-diff-context-cli/Cargo.toml -p url --precise 2.5.7
```

All subsequent build and test commands use `--locked`. Write tests for valid Unix/Windows file URIs plus credentials, query, fragment, authority, non-file scheme, percent-encoded escape, root URI, missing file, directory, stale symlink, non-UTF-8 path, duplicate separators, `.`/`..`, and trailing slash.

- [ ] **Step 2: Implement the strict mapper and provider file path**

```rust
pub struct SnapshotUriMapper { canonical_root: PathBuf }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspPosition { pub line: u32, pub character: u32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspRange { pub start: LspPosition, pub end: LspPosition }

impl SnapshotUriMapper {
    pub fn new(root: &Path) -> Result<Self, SnapshotBoundaryError>;
    pub fn to_file_path(&self, uri: &Url) -> Result<SnapshotFilePath, SnapshotBoundaryError>;
    pub fn to_file_uri(&self, path: &SnapshotFilePath) -> Result<Url, SnapshotBoundaryError>;
}

pub struct SourceDocument { bytes: Arc<[u8]>, line_starts: Vec<usize> }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionEncoding { Utf8, Utf16 }

impl SourceDocument {
    pub fn new(bytes: Arc<[u8]>) -> Result<Self, SnapshotBoundaryError>;
    pub fn lsp_to_byte(&self, position: LspPosition, encoding: PositionEncoding)
        -> Result<(usize, bool), SnapshotBoundaryError>;
    pub fn byte_to_lsp(&self, byte: usize, encoding: PositionEncoding)
        -> Result<LspPosition, SnapshotBoundaryError>;
    pub fn lsp_range_to_provider(
        &self, range: LspRange, encoding: PositionEncoding,
    ) -> Result<ProviderRange, SnapshotBoundaryError>;
    pub fn provider_range_to_lsp(
        &self, range: &ProviderRange, encoding: PositionEncoding,
    ) -> Result<LspRange, SnapshotBoundaryError>;
}
```

Use `url::Url` and platform file-path conversion. Reject credentials/query/fragment before conversion, canonicalize the existing target, require strict containment below the canonical root and a regular file, reject lossy path conversion, and return only bounded codes (`provider-uri-invalid`, `provider-uri-outside-snapshot`, `provider-uri-stale`, `provider-uri-non-utf8`). `ProviderRange` is `provider-source-range-v1/utf8-byte-columns/end-exclusive`; line/column and byte offsets must agree against the exact UTF-8 bytes.

Follow LSP 3.17: a character beyond a line normalizes to line end but returns `normalized = true` so the caller emits `provider-position-normalized`; LF, CRLF, and bare CR terminators are end-exclusive and crossing ranges end at the next line's character zero; empty lines and final lines with or without a terminator use the same checked line index; UTF-8/UTF-16 mid-code-point/surrogate positions, invalid UTF-8, reversed ranges, overflow, and lines beyond EOF are errors.

- [ ] **Step 3: Add Unicode, line-ending, EOF, and selection/query tests**

```rust
#[test]
fn utf8_and_utf16_map_to_the_same_provider_bytes() {
    let document = SourceDocument::new(Arc::from("a😀z\r\nβ\n".as_bytes())).unwrap();
    let utf8 = document.lsp_range_to_provider(
        LspRange::new(0, 1, 0, 5), PositionEncoding::Utf8,
    ).unwrap();
    let utf16 = document.lsp_range_to_provider(
        LspRange::new(0, 1, 0, 3), PositionEncoding::Utf16,
    ).unwrap();
    assert_eq!(utf8, utf16);
    assert_eq!((utf8.start_byte, utf8.end_byte), (1, 5));
    assert!(document.lsp_to_byte(LspPosition::new(0, 99), PositionEncoding::Utf8).unwrap().1);
}
```

Assert the selection range is contained in the symbol range, `query_byte` is a UTF-8 boundary inside selection, and a line-end normalization creates a limitation rather than silently changing a report.

- [ ] **Step 4: Close dependency and schema license/SBOM gates**

Copy the `url` 2.5.7 crate's upstream `LICENSE-APACHE` and `LICENSE-MIT` files verbatim into `THIRD_PARTY_LICENSES/url-LICENSE-APACHE` and `THIRD_PARTY_LICENSES/url-LICENSE-MIT`. The crate declares `MIT OR Apache-2.0`. Add `url@2.5.7` to the release workflow's required CycloneDX component set, assert both packaged notice files exist, and run the release license path's existing checks. Do not add an unpinned URL parser or a network-capable runtime.

- [ ] **Step 5: Run and commit**

```bash
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test repository_context_provider_snapshot
rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets -- -D warnings
rtk git diff --check
rtk git add collect-diff-context-cli/Cargo.toml collect-diff-context-cli/Cargo.lock collect-diff-context-cli/src/repository_context_provider/snapshot.rs collect-diff-context-cli/tests/repository_context_provider_snapshot.rs THIRD_PARTY_LICENSES/url-LICENSE-APACHE THIRD_PARTY_LICENSES/url-LICENSE-MIT .github/workflows/release.yml
rtk git commit -m "feat(provider): map bounded file URIs and ranges"
```

### Task 4: Bounded JSON-RPC Framing, Correlation, And Fuzzing

**Files:**

- Create: `collect-diff-context-cli/src/repository_context_provider/json_rpc.rs`
- Create: `collect-diff-context-cli/tests/repository_context_json_rpc.rs`
- Create: `collect-diff-context-cli/fuzz/fuzz_targets/repository_context_frame.rs`
- Create: `collect-diff-context-cli/fuzz/fuzz_targets/repository_context_messages.rs`
- Create: the three provider fuzz corpus seeds in the file map
- Modify: `collect-diff-context-cli/fuzz/Cargo.toml` and `fuzz/Cargo.lock`

- [ ] **Step 1: Write failing frame tests**

Test every split point of a valid frame, multiple frames in one read, partial EOF, duplicate/conflicting/missing/negative/overflow Content-Length, LF-only headers, unsupported transfer framing, body/header/cumulative/message limits, malformed JSON, and zero-length body. Use a decoder configured below the production maxima.

- [ ] **Step 2: Implement the bounded incremental decoder**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLimits {
    pub max_header_bytes: usize, pub max_frame_bytes: usize,
    pub max_protocol_bytes: usize, pub max_messages: usize,
}

pub struct FrameDecoder {
    limits: FrameLimits, buffer: Vec<u8>, expected_body: Option<usize>,
    protocol_bytes: usize, messages: usize,
}

impl FrameDecoder {
    pub fn new(limits: FrameLimits) -> Result<Self, ProtocolError>;
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, ProtocolError>;
    pub fn finish(self) -> Result<(), ProtocolError>;
    pub fn buffered_bytes(&self) -> usize;
}
```

Parse ASCII headers terminated by CRLFCRLF, require exactly one decimal Content-Length, validate all lengths before allocation, drain complete bodies, and use checked cumulative arithmetic. Never expose header/body bytes in a public error.

- [ ] **Step 3: Add strict message and correlation types**

Define `ClientResponse { id: u64, outcome: ResponseOutcome }`, `ServerRequestId { Number(u64), String(String) }`, `ServerRequest`, `ServerNotification`, `InboundMessage`, `RpcErrorObject`, `ProtocolError`, and:

```rust
pub struct MessageLimits {
    pub max_requests: usize, pub max_pending_requests: usize,
    pub max_messages: usize, pub max_notifications: usize,
    pub max_server_requests: usize, pub max_invalid_messages: usize,
}

pub struct CorrelationState { /* private counters and pending IDs */ }

impl CorrelationState {
    pub fn new(limits: MessageLimits) -> Result<Self, ProtocolError>;
    pub fn reserve_request(&mut self, method: &str) -> Result<u64, ProtocolError>;
    pub fn accept_client_response(&mut self, response: ClientResponse)
        -> Result<ClientResponse, ProtocolError>;
    pub fn observe_server_request(&mut self) -> Result<(), ProtocolError>;
    pub fn observe_notification(&mut self) -> Result<(), ProtocolError>;
    pub fn observe_invalid(&mut self) -> Result<(), ProtocolError>;
    pub fn pending_len(&self) -> usize;
}
```

`parse_inbound`, `encode_request`, `encode_notification`, `encode_result`, `encode_error`, and `frame_json` must reject malformed JSON-RPC envelopes, require JSON-RPC 2.0, bound method/error strings and params, and emit ASCII Content-Length framing. Unknown/duplicate/completed IDs count as invalid output. The generic state accepts out-of-order responses for transport tests; the provider profile fixes `max_pending_requests = 1` and the adapter is single-flight.

- [ ] **Step 4: Add fuzz targets and run smoke**

The frame target feeds arbitrary bytes in arbitrary chunks and asserts `buffered_bytes` never exceeds `max_header_bytes + max_frame_bytes`. The message target treats input as newline-delimited JSON, calls `parse_inbound`, feeds numeric responses into a state with four preloaded IDs, and asserts no counter exceeds its limit. Register both targets and use plain ASCII seeds.

```bash
rtk cargo +nightly fuzz build --fuzz-dir collect-diff-context-cli/fuzz
rtk cargo +nightly fuzz run repository_context_frame --fuzz-dir collect-diff-context-cli/fuzz -- -runs=256 -timeout=5
rtk cargo +nightly fuzz run repository_context_messages --fuzz-dir collect-diff-context-cli/fuzz -- -runs=256 -timeout=5
```

Expected: no crash, abort, or unbounded allocation.

- [ ] **Step 5: Run and commit**

```bash
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test repository_context_json_rpc
rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/fuzz/Cargo.toml -- --check
rtk git diff --check
rtk git add collect-diff-context-cli/src/repository_context_provider/json_rpc.rs collect-diff-context-cli/tests/repository_context_json_rpc.rs collect-diff-context-cli/fuzz
rtk git commit -m "feat(provider): bound LSP framing and correlation"
```

### Task 5: Extract Shared Private Runtime And Managed Child

**Files:**

- Create: `collect-diff-context-cli/src/trusted_runtime.rs`
- Modify: `collect-diff-context-cli/src/lib.rs:1-13`
- Modify: `collect-diff-context-cli/src/process_group.rs:29-169`
- Modify: `collect-diff-context-cli/src/static_analysis/executor.rs:1-544`

- [ ] **Step 1: Freeze one-shot regression output**

```bash
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --features test-fixture --test static_execution --test static_execution_modes --test static_execution_platform
```

Expected: all existing controlled-execution tests pass before extraction.

- [ ] **Step 2: Write a failing private-copy unit test in the new module**

Use `std::env::current_exe()` and SHA-256 to assert an authorized copy has the same digest and a wrong digest is rejected. Define `TrustedRuntimeError` in the module test fixture only after the first compile failure.

- [ ] **Step 3: Extract `PrivateRuntime` without changing profile authority**

```rust
pub(crate) struct PrivateRuntime { /* TempDir, home, tmp, empty path, executable */ }

impl PrivateRuntime {
    pub(crate) fn create(source: &Path, expected_sha256: &str)
        -> Result<Self, TrustedRuntimeError>;
    pub(crate) fn path(&self) -> &Path;
    pub(crate) fn home(&self) -> &Path;
    pub(crate) fn temporary(&self) -> &Path;
    pub(crate) fn empty_path(&self) -> &Path;
    pub(crate) fn executable_path(&self) -> &Path;
    pub(crate) fn verify(&self) -> Result<(), TrustedRuntimeError>;
}
```

Stream-copy a regular executable into a `create_new` private file, check SHA-256 before and after permission setup, use Unix `0500`/Windows read-only permissions, create private home/tmp/target/empty-path directories, and never copy the snapshot or profile into the runtime.

- [ ] **Step 4: Add the Drop-safe process owner and switch static execution**

```rust
pub(crate) struct ManagedChild { child: Option<Child>, process_group: ProcessGroup }

impl ManagedChild {
    pub(crate) fn spawn(command: Command) -> Result<Self, TrustedRuntimeError>;
    pub(crate) fn child_mut(&mut self) -> &mut Child;
    pub(crate) fn try_wait(&mut self) -> Result<Option<ExitStatus>, TrustedRuntimeError>;
    pub(crate) fn wait(&mut self) -> Result<ExitStatus, TrustedRuntimeError>;
    pub(crate) fn terminate_and_wait(&mut self) -> Result<Option<ExitStatus>, TrustedRuntimeError>;
}

impl Drop for ManagedChild {
    fn drop(&mut self) { let _ = self.terminate_and_wait(); }
}
```

Attach the existing process group immediately after spawn, kill and wait on attach failure, and make termination idempotent. Extract the base environment helper but preserve the static executor's `/bin:/usr/bin` or Windows system PATH; the provider will pass its own empty PATH. Keep stdin-null, output sentinel, status, digest, scope, and snapshot timing unchanged in static execution.

- [ ] **Step 5: Run shared regressions and commit**

```bash
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --features test-fixture --test static_execution --test static_execution_modes --test static_execution_platform
rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets --all-features -- -D warnings
rtk git diff --check
rtk git add collect-diff-context-cli/src/lib.rs collect-diff-context-cli/src/process_group.rs collect-diff-context-cli/src/trusted_runtime.rs collect-diff-context-cli/src/static_analysis/executor.rs
rtk git commit -m "refactor(runtime): share pinned managed child"
```

### Task 6: Interactive Session And Independent Fake LSP Server

**Files:**

- Create: `collect-diff-context-cli/src/repository_context_provider/session.rs`
- Create: `collect-diff-context-cli/src/bin/repository_context_provider_fixture.rs`
- Create: `collect-diff-context-cli/tests/repository_context_session.rs`
- Modify: `collect-diff-context-cli/Cargo.toml:8-27`

- [ ] **Step 1: Register fixture scenarios and write failing session tests**

Register a `repository-context-provider-fixture` binary behind `test-fixture`. Its independent frame implementation supports these scenarios: `lifecycle`, `missing-capability`, `config-requests`, `readiness-ok`, `readiness-warning`, `readiness-error`, `readiness-hang`, `unknown-encoding`, `malformed-frame`, `unknown-id`, `hang`, `stderr-flood`, `crash`, and `spawn-descendant`. It logs methods to a caller-provided file and bounds fixture allocations to 1 MiB. Do not reuse production framing in the fixture.

Test split frames, server request interleaving, deadline, stderr limit-plus-one, crash, malformed EOF, cancellation, and descendant termination using `env!("CARGO_BIN_EXE_repository-context-provider-fixture")`.

- [ ] **Step 2: Implement bounded reader threads**

The stdout reader uses fixed 8 KiB chunks, `FrameDecoder`, and a bounded `sync_channel`; it uses `try_send` and sets overflow rather than blocking when the channel is full. The stderr reader retains only `max_stderr_bytes + 1` bytes and records bytes/digest. Killing the child closes both pipes before joining threads. No raw server/stderr bytes enter errors or reports.

- [ ] **Step 3: Implement `ManagedLspSession` with one pending provider request**

```rust
pub struct SessionLaunch<'a> {
    pub snapshot: &'a BoundCandidateSnapshot<'a>,
    pub executable: &'a Path,
    pub executable_sha256: &'a str,
    pub arguments: &'a [String],
    pub source: ReviewSource,
    pub scope_fingerprint: &'a str,
    pub limits: &'a ProviderLimits,
    pub cancellation: Arc<AtomicBool>,
}

pub struct ManagedLspSession { /* runtime, child, pipes, correlation, deadline, metrics */ }

impl ManagedLspSession {
    pub fn spawn(launch: SessionLaunch<'_>) -> Result<Self, SessionError>;
    pub fn send_request(&mut self, method: &str, params: Value) -> Result<u64, SessionError>;
    pub fn send_notification(&mut self, method: &str, params: Value) -> Result<(), SessionError>;
    pub fn send_server_result(&mut self, id: &ServerRequestId, value: Value) -> Result<(), SessionError>;
    pub fn send_server_error(&mut self, id: &ServerRequestId, code: i64, message: &str) -> Result<(), SessionError>;
    pub fn next_message(&mut self) -> Result<InboundMessage, SessionError>;
    pub fn shutdown_and_reap(&mut self) -> Result<(), SessionError>;
    pub fn terminate(&mut self);
    pub fn metrics(&self) -> &SessionMetrics;
}
```

`SessionError { code: &'static str, message: String }` and `SessionMetrics` are bounded. Every operation checks cancellation/deadline/overflow/child exit. Drop closes stdin, terminates the full process group, waits, and joins readers. The provider calls `next_message` in a loop and handles server requests; session code never silently discards them.

Set `env_clear`, snapshot current directory, no shell, private HOME/tmp/target/empty PATH, fixed locale, `NO_COLOR`, `CARGO_NET_OFFLINE`, `RUSTUP_AUTO_INSTALL=0`, invalid proxy endpoints, empty NO_PROXY, scope/source diagnostics, and Windows SystemRoot/WINDIR. This is best-effort offline, not an OS network sandbox.

- [ ] **Step 4: Run and commit**

```bash
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --features test-fixture --test repository_context_session
rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets --all-features -- -D warnings
rtk git diff --check
rtk git add collect-diff-context-cli/Cargo.toml collect-diff-context-cli/src/bin/repository_context_provider_fixture.rs collect-diff-context-cli/src/repository_context_provider/session.rs collect-diff-context-cli/tests/repository_context_session.rs
rtk git commit -m "feat(provider): manage bounded LSP sessions"
```

### Task 7: Rust-Analyzer Linked-Project Initialization And Capability Gate

**Files:**

- Create: `collect-diff-context-cli/src/repository_context_provider/rust_analyzer.rs`
- Modify: `collect-diff-context-cli/src/repository_context_provider/session.rs`
- Modify: `collect-diff-context-cli/src/bin/repository_context_provider_fixture.rs`
- Create: `collect-diff-context-cli/tests/repository_context_rust_analyzer.rs`

- [ ] **Step 1: Write failing profile/handshake tests**

Assert profile canonical digest and all no-toolchain settings, profile/executable/configuration mismatch rejection, the sole inline linked-project object is the canonical model value, `initialized` is sent before capability gating, missing Call Hierarchy returns `unavailable` followed by shutdown/exit, and a server JSON-RPC initialize error returns `failed`.

Assert the client advertises `experimental.serverStatusNotification = true`; no `didOpen` or hierarchy request is sent before an `experimental/serverStatus` notification with `quiescent = true`; ok proceeds, warning proceeds as partial with a limitation, explicit error is unavailable, missing status consumes the global deadline as timeout, and malformed status is invalid output. Every non-proceeding case returns no facts. Assert an unoffered `positionEncoding` is invalid output while an absent value defaults to UTF-16.

The fixture's configuration scenario must assert `workspace/configuration` returns an `LSPAny[]` exactly equal in length/order to request items, with `null` for unavailable slots. A registration request containing one disallowed registration must receive one error and adopt none.

- [ ] **Step 2: Define typed rust-analyzer wire types and profile configuration**

Use `Url`, `LspPosition`, `LspRange`, and typed Serde structs for initialize params/result, capabilities, configuration items, readiness status, and server requests. Initialization must carry the canonical snapshot URI, the sole inline canonical linked-project object, `general.positionEncodings = ["utf-8", "utf-16"]`, `textDocument.callHierarchy.dynamicRegistration = false`, `workspace.configuration = true`, `experimental.serverStatusNotification = true`, and this nested typed hardening object:

```json
{
  "cargo": {
    "buildScripts": { "enable": false },
    "noDeps": true,
    "sysroot": null,
    "sysrootSrc": null,
    "target": "<bound-target-triple>"
  },
  "procMacro": { "enable": false },
  "checkOnSave": false
}
```

The profile fixes `toolchain_mode = "none"`, target triple, empty PATH policy, Cargo/rustc/sysroot disablement, readiness policy, and arguments. The configuration digest covers canonical typed capability, hardening, server-request, and readiness policy bytes; the separate project-model digest covers the complete inline model. The profile checks the request's duplicated binding. Any returned position encoding other than offered UTF-8/UTF-16 is invalid output; absence defaults UTF-16.

- [ ] **Step 3: Implement server-request policy and lifecycle order**

Handle `workspace/configuration` with same-length ordered arrays, `window/workDoneProgress/create` with null, all-or-error `client/registerCapability` only for bounded non-execution methods, `workspace/applyEdit` with `{ "applied": false }`, unknown requests with `-32601`, and unknown notifications within their budget. Send `initialized` after a successful initialize response before capability inspection. After the capability gate, wait within the shared deadline for typed `experimental/serverStatus`: false quiescence keeps waiting, ok/true proceeds, warning/true records a partial limitation, error is unavailable, no status before the global deadline is timeout, and malformed status is invalid output. Capability/readiness-unavailable sessions send shutdown then exit; timeout and other failures terminate/reap.

- [ ] **Step 4: Run and commit**

```bash
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --features test-fixture --test repository_context_rust_analyzer
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --features test-fixture --test repository_context_session
rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets --all-features -- -D warnings
rtk git diff --check
rtk git add collect-diff-context-cli/src/repository_context_provider collect-diff-context-cli/src/bin/repository_context_provider_fixture.rs collect-diff-context-cli/tests/repository_context_rust_analyzer.rs
rtk git commit -m "feat(provider): gate linked-project rust-analyzer sessions"
```

### Task 8: Single-Flight Call Hierarchy Traversal And Normalization

**Files:**

- Modify: `collect-diff-context-cli/src/repository_context_provider/rust_analyzer.rs`
- Modify: `collect-diff-context-cli/src/bin/repository_context_provider_fixture.rs`
- Modify: `collect-diff-context-cli/tests/repository_context_rust_analyzer.rs`

- [ ] **Step 1: Write failing graph tests**

The fixture graph includes two seeds, shared nodes, an incoming caller, outgoing callee, self-call, cycle, duplicate items, duplicate call ranges, null prepare, empty call lists, and invalid/stale/external URIs. Assert one edge per unique caller-callee-call-range tuple, incoming ranges on the incoming caller, outgoing ranges on the current caller, semantic/high/calls provenance, and deterministic IDs/ordering.

Also assert response item ownership: URI path, name, compatible LSP kind, full/selection containment, and selection containing `query_byte`; zero matches are unresolved partial, multiple matches are ambiguous partial, never guessed.

- [ ] **Step 2: Define bounded Call Hierarchy types**

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallHierarchyItem {
    name: String, kind: u32, detail: Option<String>, uri: Url,
    range: LspRange, selection_range: LspRange, data: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IncomingCall { from: CallHierarchyItem, from_ranges: Vec<LspRange> }

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutgoingCall { to: CallHierarchyItem, from_ranges: Vec<LspRange> }
```

Bound names/details/data/ranges before retention; keep `data` only for a same-session follow-up request. Convert LSP SymbolKind values to the six seed kinds and reject incompatible values.

- [ ] **Step 3: Implement seed prepare and stable single-flight BFS**

Read and `didOpen` each distinct seed file once. Convert `query_byte` to the negotiated LSP position. For each seed and frontier item, send one request, wait for its correlated response while servicing server requests, normalize fully, then commit facts and move to the next stable ID. Track `(direction, symbol_id)` visited keys; depth is 1 or 2; all request/message/source/node/edge/report/deadline budgets are checked before mutation. This ordering makes finite-resource results independent of response arrival order.

Generate provider symbol IDs from the complete binding digest and provider range/selection; preserve `changed_symbol_id` only in `seed_symbols`. Generate one edge per distinct call range and sort every output array by ID. Never write heuristic edges or semantic facts to existing impact/index/cache types.

- [ ] **Step 4: Run traversal gates and commit**

```bash
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --features test-fixture --test repository_context_rust_analyzer
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test repository_context_provider_snapshot --test repository_context_json_rpc
rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets --all-features -- -D warnings
rtk git diff --check
rtk git add collect-diff-context-cli/src/repository_context_provider/rust_analyzer.rs collect-diff-context-cli/src/bin/repository_context_provider_fixture.rs collect-diff-context-cli/tests/repository_context_rust_analyzer.rs
rtk git commit -m "feat(provider): traverse bounded semantic call hierarchy"
```

### Task 9: Public Runner, Status Matrix, Postflight, And Platform Tests

**Files:**

- Modify: `collect-diff-context-cli/src/repository_context_provider/mod.rs`
- Modify: `collect-diff-context-cli/src/repository_context_provider/contract.rs`
- Modify: `collect-diff-context-cli/src/repository_context_provider/session.rs`
- Modify: `collect-diff-context-cli/src/repository_context_provider/rust_analyzer.rs`
- Create/modify: `collect-diff-context-cli/tests/repository_context_provider_platform.rs`

- [ ] **Step 1: Write failing public-runner tests**

Use only the public entry point and assert exact terminal behavior:

```rust
#[test]
fn completed_report_contains_every_binding_and_no_local_path() {
    let fixture = ProviderRunFixture::new("graph");
    let report = fixture.run().unwrap();
    report.validate().unwrap();
    assert_eq!(report.status, RepositoryContextProviderStatus::Completed);
    assert_eq!(report.candidate.snapshot_sha256, fixture.snapshot.sha256);
    assert_eq!(report.candidate.project_model_digest, fixture.model.digest);
    assert_eq!(report.provider.profile_sha256, fixture.profile.sha256());
    assert_eq!(report.provider.executable_sha256, fixture.profile.binding.executable_sha256);
    assert_eq!(report.index_completeness, ProviderCompleteness::Unknown);
    assert!(!serde_json::to_string(&report).unwrap().contains(
        fixture.snapshot.path().to_str().unwrap()
    ));
}

#[test]
fn timeout_invalid_output_crash_and_cancel_return_no_facts() {
    for (scenario, expected) in [
        ("hang", RepositoryContextProviderStatus::Timeout),
        ("malformed-frame", RepositoryContextProviderStatus::InvalidOutput),
        ("unknown-id", RepositoryContextProviderStatus::InvalidOutput),
        ("crash", RepositoryContextProviderStatus::Failed),
    ] {
        let report = ProviderRunFixture::new(scenario).run().unwrap();
        assert_eq!(report.status, expected);
        assert!(report.seed_symbols.is_empty());
        assert!(report.related_symbols.is_empty());
        assert!(report.edges.is_empty());
    }
}
```

Add preflight no-spawn tests for each binding mismatch and repository-controlled `rust-analyzer.toml`, postflight snapshot/profile/executable/model mutation tests returning `ProviderError::StaleBinding`, one test per budget, cancellation returning `ProviderError::Cancelled` after reaping, degraded model and readiness warning partial, unsupported capability and explicitly unhealthy readiness unavailable, missing readiness timeout, and report-byte truncation. Verify no error/report leaks root paths, raw URIs, stderr, environment, JSON-RPC, or opaque data.

- [ ] **Step 2: Implement the public invocation and deterministic status precedence**

```rust
pub struct ProviderInvocation<'a> {
    pub snapshot: &'a CandidateSnapshot,
    pub model: &'a RustAnalyzerProjectModel,
    pub request: &'a RepositoryContextProviderRequest,
    pub profile: &'a AuthorizedProviderProfile,
    pub cancellation: Arc<AtomicBool>,
}

pub fn run_repository_context_provider(
    invocation: ProviderInvocation<'_>,
) -> Result<RepositoryContextProviderReport, ProviderError>;
```

Run in this order: request/profile/model validation; borrowed snapshot binding and seed validation; profile/executable/config preflight; session; initialize/initialized/capability; open/prepare/BFS; graceful shutdown or forced termination; snapshot/model/profile/executable postflight; report size/validation. Use the exact precedence binding error, cancellation, invalid-output, timeout, failed, unavailable, partial, completed. A postflight mismatch is an API error, not a stale report. Keep only fully committed facts for partial; clear all facts for timeout/invalid-output/failed/cancellation.

- [ ] **Step 3: Add fake-server platform lifecycle tests**

Gate the file with `#![cfg(feature = "test-fixture")]`. Test the pinned fake server on the current host for snapshot read-only state, no shell/literal arguments, empty PATH policy, stderr limit-plus-one, process-tree timeout/drop, initialize/capability/prepare/incoming/outgoing/shutdown/exit ordering, and no default-pipeline reachability. The same test target runs on Linux, macOS arm64, and Windows in CI; real rust-analyzer remains Delivery 5.

- [ ] **Step 4: Run all affected tests and commit**

```bash
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --features test-fixture --test repository_context_rust_analyzer --test repository_context_provider_platform
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test impact_context_rust semantic_providers_are_rejected
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test repository_index_integration
rtk git diff --check
rtk git add collect-diff-context-cli/src/repository_context_provider collect-diff-context-cli/tests/repository_context_provider_platform.rs
rtk git commit -m "feat(provider): finalize bound context runner"
```

### Task 10: Rust 1.95, CI/Fuzz Smoke, Documentation, And Completion Sweep

**Files:**

- Modify: `.github/workflows/lint.yml:30-92,94-143`
- Modify: `collect-diff-context-cli/fuzz/README.md:1-14`
- Create: `docs/rust-analyzer-context-provider.md`
- Modify: `docs/helper-capabilities.md`
- Modify: `docs/call-graph-open-source-options.md`

- [ ] **Step 1: Add locked Rust 1.95 and platform gates**

Add a `rust-1-95` CI job using `dtolnay/rust-toolchain@1.95.0` that runs `cargo +1.95.0 check --all-targets --all-features --locked` and the full provider contract/snapshot/JSON-RPC tests with `--locked`. Add the six provider test targets to the existing `test-fixture` platform matrix. Keep the provider absent from release binaries and CLI help smoke tests.

- [ ] **Step 2: Add fuzz smoke and deferred sustained commands**

Add provider frame/message smoke to the existing nightly build job:

```yaml
          cargo +nightly fuzz run repository_context_frame --fuzz-dir collect-diff-context-cli/fuzz -- -runs=256 -timeout=5
          cargo +nightly fuzz run repository_context_messages --fuzz-dir collect-diff-context-cli/fuzz -- -runs=256 -timeout=5
```

Document one-hour commands in `fuzz/README.md` as Delivery 5 work, explicitly separate from first-cycle completion.

- [ ] **Step 3: Document the opt-in capability**

Create `docs/rust-analyzer-context-provider.md` with `Status`, `Inputs And Binding`, `Linked Project Model`, `Bounded Protocol`, `Execution Isolation`, `Report Semantics`, `Known Limitations`, `Local Verification`, and `Deferred Release Work`. State that the provider is library-only and opt-in, accepts a borrowed materialized snapshot and typed model/profile, uses best-effort offline controls rather than an OS network sandbox, never runs default review/Fast Mode/index/static-analysis paths, never persists semantic facts, and never claims a complete runtime call graph. Link it from helper capabilities and mark Delivery 1-3 as locally scoped in call-graph options.

- [ ] **Step 4: Run the complete local gates**

```bash
rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/Cargo.toml -- --check
rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/fuzz/Cargo.toml -- --check
rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets -- -D warnings
rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets --all-features -- -D warnings
rtk python3 scripts/validate_schemas.py
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-features
rtk cargo +1.95.0 build --release --manifest-path collect-diff-context-cli/Cargo.toml --locked
rtk cargo +nightly fuzz build --fuzz-dir collect-diff-context-cli/fuzz
rtk cargo +nightly fuzz run repository_context_frame --fuzz-dir collect-diff-context-cli/fuzz -- -runs=256 -timeout=5
rtk cargo +nightly fuzz run repository_context_messages --fuzz-dir collect-diff-context-cli/fuzz -- -runs=256 -timeout=5
rtk git diff --check
```

Expected: all commands pass without warnings. Rust 1.95 and `--locked` evidence is required; stable-only success is insufficient.

- [ ] **Step 5: Prove no default wiring or persistence**

Run these separately and record the expected exit 1/no-match result:

```bash
rtk rg -n "run_repository_context_provider|ProviderInvocation" collect-diff-context-cli/src/app.rs collect-diff-context-cli/src/main.rs collect-diff-context-cli/src/bin/repository_context.rs collect-diff-context-cli/src/impact_context/engine.rs collect-diff-context-cli/src/static_analysis/orchestration.rs scripts
rtk rg -n "repository_context_provider" collect-diff-context-cli/src/impact_context/cache collect-diff-context-cli/src/impact_context/index 2>/dev/null
```

- [ ] **Step 6: Commit and inspect the final range**

```bash
rtk git add .github/workflows/lint.yml collect-diff-context-cli/fuzz/README.md docs/rust-analyzer-context-provider.md docs/helper-capabilities.md docs/call-graph-open-source-options.md
rtk git commit -m "test(provider): gate bounded context provider"
rtk git status --short --branch
rtk git diff --check 42cfd8e..HEAD
rtk git diff --stat 42cfd8e..HEAD
```

Expected: clean worktree, no whitespace errors, changes limited to the file map, and all provider tests represented in CI. Four-platform real rust-analyzer, sustained fuzz, latency/resource benchmarks, SBOM/license closure for the real artifact, explicit CLI surface, and release documentation remain Delivery 4/5 work.

## Plan Self-Review Checklist

- [ ] Every design requirement maps to a task: exact snapshot/mode/directory/VCS binding, profile/model/toolchain identity, strict URI/range, bounded framing/messages, single-flight traversal, lifecycle/reaping, status precedence, and postflight verification.
- [ ] The existing `ImpactContext`, ordinary review, Fast Mode, repository index, SQLite, and static-analysis public contracts remain untouched.
- [ ] The fake server proves protocol/lifecycle behavior without an installed rust-analyzer; real-server behavior is explicitly deferred.
- [ ] Every code-facing type named in later tasks is defined in an earlier task or is a local test fixture.
- [ ] No placeholder tokens or unbounded generic error-handling instructions remain.
