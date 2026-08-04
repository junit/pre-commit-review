# Rust Static Analysis Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Python `collect` and `run` product implementations with one Rust library and `static-analysis-cli` binary while preserving the existing Shell, JSON, schema, and exit-semantics contracts.

**Architecture:** Keep one Cargo package and expose the existing diff control plane through a typed `review_scope` module. Add focused static-analysis modules for strict contracts, evidence normalization, candidate snapshots, process execution, and output rendering; the Shell wrappers call the Rust binary directly and retain the existing sanitizer behavior. Python remains only long enough for internal parity checks, then the two Python product files and their runtime-selection tests are deleted at cutover.

**Tech Stack:** Rust 2021, serde/serde_json, regex, sha2, tempfile, platform process APIs, Bash wrappers, Git plumbing, JSON Schema draft 2020-12, existing Python `jsonschema` development validator.

---

## Scope And File Map

This plan implements Delivery A only. Do not add manifests, multi-tool scheduling, orchestration schemas, semantic cross-tool grouping, `static_analysis_input/v2`, caching, or parallel execution.

**Create:**

- `collect-diff-context-cli/src/lib.rs` - library entrypoint and module exports.
- `collect-diff-context-cli/src/app.rs` - existing `collect-diff-context` CLI adapter moved out of the binary target.
- `collect-diff-context-cli/src/review_scope.rs` - authoritative scope, Git candidate identity, units, groups, and final revalidation.
- `collect-diff-context-cli/src/bin/static_analysis.rs` - `collect` and `run` CLI dispatch.
- `collect-diff-context-cli/src/static_analysis/mod.rs` - public static-analysis module interface.
- `collect-diff-context-cli/src/static_analysis/contracts.rs` - strict v1 input/profile/evidence/execution types and semantic validation.
- `collect-diff-context-cli/src/static_analysis/evidence.rs` - SARIF/normalized parsing, deduplication, changed-line mapping, and evidence construction.
- `collect-diff-context-cli/src/static_analysis/snapshot.rs` - staged/unstaged/branch tracked-file snapshots and integrity digest.
- `collect-diff-context-cli/src/static_analysis/executor.rs` - profile preflight, direct bounded process execution, and single-run composition.
- `collect-diff-context-cli/src/static_analysis/output.rs` - stable section-marker rendering.
- `collect-diff-context-cli/tests/review_scope.rs` - typed scope and control-plane parity tests.
- `collect-diff-context-cli/tests/static_evidence.rs` - collector contract tests.
- `collect-diff-context-cli/tests/static_execution.rs` - profile, execution, failure, and drift tests.
- `collect-diff-context-cli/tests/static_execution_modes.rs` - staged/unstaged/branch/gitlink tests.
- `scripts/lib/static_analysis_cli.sh` - trusted Rust binary resolution shared by both wrappers.
- `tests/static_analysis_rust_parity_test.sh` - temporary internal Python/Rust comparison, deleted at cutover.

**Modify:**

- `collect-diff-context-cli/src/main.rs`
- `collect-diff-context-cli/Cargo.toml`
- `collect-diff-context-cli/Cargo.lock`
- `scripts/collect_static_evidence.sh`
- `scripts/run_static_analysis.sh`
- `scripts/build_all_binaries.sh`
- `install.sh`
- `.github/workflows/lint.yml`
- `.github/workflows/release.yml`
- `tests/static_analysis_evidence_test.sh`
- `tests/static_analysis_execution_test.sh`
- `tests/static_analysis_execution_modes_test.sh`
- `tests/install_smoke_test.sh`
- `tests/install_agent_matrix_test.sh`
- `tests/skill_contract_test.sh`
- `scripts/validate_schemas.py`
- `README.md`
- `README.zh-CN.md`
- `docs/helper-capabilities.md`
- `docs/static-analysis-evidence.md`
- `docs/static-analysis-execution.md`

**Delete at cutover:**

- `scripts/collect_static_evidence.py`
- `scripts/run_static_analysis.py`
- `tests/static_analysis_rust_parity_test.sh`

### Task 1: Establish The Library And Second Binary

**Files:**
- Create: `collect-diff-context-cli/src/lib.rs`
- Create: `collect-diff-context-cli/src/app.rs`
- Create: `collect-diff-context-cli/src/bin/static_analysis.rs`
- Modify: `collect-diff-context-cli/src/main.rs`
- Modify: `collect-diff-context-cli/Cargo.toml`
- Test: `collect-diff-context-cli/tests/review_scope.rs`

- [ ] **Step 1: Write the failing binary-boundary test**

Add a test that imports the library crate and verifies the existing CLI entrypoint is linkable through the library boundary:

```rust
use collect_diff_context_cli::collect_diff_context_main;

#[test]
fn library_exports_collect_diff_context_entrypoint() {
    let _: fn() -> i32 = collect_diff_context_main;
}
```

- [ ] **Step 2: Run the test and verify red**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test review_scope`

Expected: FAIL because the package has no library target or exported CLI entrypoint.

- [ ] **Step 3: Convert the package to library plus binaries**

Add explicit targets and migration dependencies:

```toml
[[bin]]
name = "collect-diff-context-cli"
path = "src/main.rs"

[[bin]]
name = "static-analysis-cli"
path = "src/bin/static_analysis.rs"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
regex = "1.10"
sha2 = "0.10"
tempfile = "3"

[target.'cfg(unix)'.dependencies]
libc = "0.2"

[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = [
  "Win32_Foundation",
  "Win32_System_JobObjects",
  "Win32_System_Threading",
] }
```

Create the initial library surface:

```rust
pub mod secret_scan;
mod app;

pub fn collect_diff_context_main() -> i32 {
    app::main_entry()
}
```

Move the existing `main.rs` implementation into `app.rs`, remove its nested `mod secret_scan`, and import `crate::secret_scan`. Move the complete existing `main` match, including exact stderr wording and exit-code mapping, into `pub(crate) fn main_entry() -> i32`; successful execution returns `0` and every existing error branch returns its current code. Do not route errors through the current generic `Display` implementation because that would change output. Make `main.rs` a thin exit adapter:

```rust
fn main() {
    let exit_code = collect_diff_context_cli::collect_diff_context_main();
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}
```

Create a deliberately minimal second binary that returns a usage error until Task 4:

```rust
fn main() {
    eprintln!("static-analysis-cli: expected collect or run subcommand");
    std::process::exit(2);
}
```

- [ ] **Step 4: Verify both binaries build and existing behavior is unchanged**

Run: `rtk cargo build --manifest-path collect-diff-context-cli/Cargo.toml --bins`

Expected: PASS and produce `collect-diff-context-cli` plus `static-analysis-cli`.

Run: `rtk bash tests/control_plane_test.sh`

Expected: `control plane tests passed`.

- [ ] **Step 5: Commit the package boundary**

```bash
rtk git add collect-diff-context-cli/Cargo.toml collect-diff-context-cli/Cargo.lock collect-diff-context-cli/src/lib.rs collect-diff-context-cli/src/app.rs collect-diff-context-cli/src/main.rs collect-diff-context-cli/src/bin/static_analysis.rs collect-diff-context-cli/tests/review_scope.rs
rtk git commit -m "refactor: expose Rust review library"
```

### Task 2: Extract The Authoritative Review Scope Module

**Files:**
- Create: `collect-diff-context-cli/src/review_scope.rs`
- Modify: `collect-diff-context-cli/src/lib.rs`
- Modify: `collect-diff-context-cli/src/main.rs`
- Test: `collect-diff-context-cli/tests/review_scope.rs`
- Test: `tests/control_plane_test.sh`
- Test: `tests/parity_golden_test.sh`

- [ ] **Step 1: Add a failing typed-scope integration test**

Build a temporary staged repository and assert the typed result exposes the same identity fields used by static analysis:

```rust
use collect_diff_context_cli::review_scope::{
    open_authoritative_scope, ReviewSource, ScopeRequest,
};
use std::{error::Error, fs, path::Path, process::Command};
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .expect("git must start");
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn typed_scope_matches_control_plane() -> Result<(), Box<dyn Error>> {
    let repo = TempDir::new()?;
    git(repo.path(), &["init", "-q"]);
    git(repo.path(), &["config", "user.email", "review@example.test"]);
    git(repo.path(), &["config", "user.name", "Review Test"]);
    fs::write(repo.path().join("README.md"), "base\n")?;
    git(repo.path(), &["add", "README.md"]);
    git(repo.path(), &["commit", "-qm", "base"]);
    fs::create_dir_all(repo.path().join("src"))?;
    fs::write(repo.path().join("src/app.rs"), "pub fn value() -> u8 { 1 }\n")?;
    git(repo.path(), &["add", "src/app.rs"]);

    let scope = open_authoritative_scope(ScopeRequest {
        repository: repo.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_fingerprint: None,
    })?;
    assert!(scope.authoritative);
    assert_eq!(scope.source, ReviewSource::Staged);
    assert_eq!(scope.units[0].path, "src/app.rs");
    assert_eq!(scope.collection_start, scope.collection_end);
    Ok(())
}
```

- [ ] **Step 2: Run the test and verify red**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test review_scope typed_scope_matches_control_plane`

Expected: FAIL because `ScopeRequest` and `open_authoritative_scope` do not exist.

- [ ] **Step 3: Define the narrow interface**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewSource { Staged, Unstaged, Branch }

pub struct ScopeRequest {
    pub repository: PathBuf,
    pub source: Option<ReviewSource>,
    pub expected_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthoritativeScope {
    pub authoritative: bool,
    pub source: ReviewSource,
    pub head: String,
    pub base: String,
    pub selected_ref: String,
    pub fingerprint: String,
    pub collection_start: String,
    pub collection_end: String,
    pub units: Vec<ScopeUnit>,
    pub groups: Vec<ScopeGroup>,
    pub work_order: Vec<WorkOrderEntry>,
}

pub fn open_authoritative_scope(request: ScopeRequest) -> Result<AuthoritativeScope, ScopeError>;
pub fn revalidate_scope(scope: &AuthoritativeScope) -> Result<(), ScopeError>;
```

- [ ] **Step 4: Move the existing scope implementation behind that interface**

Move the existing `NameStatusEntry`, `NumstatEntry`, `ManifestUnit`, `ReviewGroup`, `ScopeIdentity`, Git diff selection, binary-safe fingerprint, tuple construction, grouping, and work-order logic into `review_scope.rs`. Keep `emit_control_plane` as serialization over `AuthoritativeScope`; do not maintain a second static-analysis parser for helper stdout.

- [ ] **Step 5: Prove byte-compatible control-plane output**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test review_scope`

Expected: PASS.

Run: `rtk bash tests/control_plane_test.sh`

Expected: `control plane tests passed`.

Run: `rtk bash tests/parity_golden_test.sh`

Expected: `parity golden tests passed`.

- [ ] **Step 6: Commit the scope seam**

```bash
rtk git add collect-diff-context-cli/src/lib.rs collect-diff-context-cli/src/main.rs collect-diff-context-cli/src/review_scope.rs collect-diff-context-cli/tests/review_scope.rs
rtk git commit -m "refactor: extract authoritative review scope"
```

### Task 3: Add Strict Static-Analysis Contracts

**Files:**
- Create: `collect-diff-context-cli/src/static_analysis/mod.rs`
- Create: `collect-diff-context-cli/src/static_analysis/contracts.rs`
- Test: `collect-diff-context-cli/tests/static_evidence.rs`
- Test: `collect-diff-context-cli/tests/static_execution.rs`

- [ ] **Step 1: Write failing strict-deserialization tests**

Cover valid v1 input/profile payloads and rejection of unknown fields, invalid bounds, invalid hashes, wrong `kind`, and controlled trust without an execution id.

- [ ] **Step 2: Run the tests and verify red**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_evidence contracts`

Expected: FAIL because the contract types do not exist.

- [ ] **Step 3: Define the contract types and validators**

Use `#[serde(deny_unknown_fields)]` on every externally supplied object and explicit semantic validation:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticAnalysisInput {
    pub schema_version: u8,
    pub kind: String,
    pub scope_fingerprint: String,
    pub tool: ToolIdentity,
    pub status: ReportStatus,
    pub findings: Vec<InputFinding>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticAnalysisProfile {
    pub schema_version: u8,
    pub kind: String,
    pub name: String,
    pub tool: ToolIdentity,
    pub executable: ExecutableAuthorization,
    pub arguments: Vec<String>,
    pub output_format: OutputFormat,
    pub success_exit_codes: Vec<i32>,
    pub limits: ProfileLimits,
    pub repository_configuration: RepositoryConfiguration,
    pub network_access: NetworkAccess,
}

impl StaticAnalysisProfile {
    pub fn validate(&self) -> Result<(), ContractError>;
}
```

Define typed `StaticAnalysisEvidence`, `StaticAnalysisExecution`, counts, report provenance, finding disposition, snapshot identity, and isolation records so serialization matches the four existing schemas exactly.

- [ ] **Step 4: Make contract tests green**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_evidence --test static_execution contracts`

Expected: PASS.

- [ ] **Step 5: Commit the contract layer**

```bash
rtk git add collect-diff-context-cli/src/static_analysis collect-diff-context-cli/tests/static_evidence.rs collect-diff-context-cli/tests/static_execution.rs
rtk git commit -m "feat: add Rust static analysis contracts"
```

### Task 4: Port Report Parsing And Normalization

**Files:**
- Create: `collect-diff-context-cli/src/static_analysis/evidence.rs`
- Create: `collect-diff-context-cli/src/static_analysis/output.rs`
- Modify: `collect-diff-context-cli/src/bin/static_analysis.rs`
- Test: `collect-diff-context-cli/tests/static_evidence.rs`

- [ ] **Step 1: Add failing normalized JSON and SARIF fixtures**

Port the existing duplicate finding, unbound SARIF, multi-run SARIF, malformed JSON, path normalization, severity, confidence, category, `collect --help`, and actionable usage-error expectations into Rust integration tests.

- [ ] **Step 2: Run and verify red**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_evidence parsing`

Expected: FAIL because `collect_evidence` is missing.

- [ ] **Step 3: Implement the collector interface**

```rust
pub struct CollectRequest {
    pub repository: PathBuf,
    pub source: Option<ReviewSource>,
    pub expected_scope: String,
    pub result_paths: Vec<PathBuf>,
    pub asserted_result_scope: Option<String>,
    pub max_findings: usize,
    pub trust: EvidenceTrust,
    pub execution_id: Option<String>,
}

pub fn collect_evidence(request: CollectRequest) -> Result<StaticAnalysisEvidence, EvidenceError>;
```

Port `compact_hash`, bounded UTF-8 JSON loading, normalized input parsing, SARIF rule/location extraction, severity/confidence/category normalization, report collision checks, and per-report finding deduplication. Keep the existing 10 MB input and 10,000 input-finding limits. Preserve the documented collector flags; `--source` remains optional and is resolved by `open_authoritative_scope`, while `--trust controlled-execution` and `--execution-id` remain reserved for the in-process runner path.

- [ ] **Step 4: Render the stable output marker**

```rust
pub fn render_collect(evidence: &StaticAnalysisEvidence) -> Result<String, serde_json::Error> {
    Ok(format!(
        "# Pre-Commit Review Static Analysis Evidence\n\n## Static Analysis Evidence JSON\n{}\n",
        serde_json::to_string(evidence)?
    ))
}
```

Wire `static-analysis-cli collect` with the same flags and exit code `2` for actionable input errors.

- [ ] **Step 5: Make parser tests green**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_evidence parsing`

Expected: PASS.

- [ ] **Step 6: Commit parsing**

```bash
rtk git add collect-diff-context-cli/src/static_analysis/evidence.rs collect-diff-context-cli/src/static_analysis/output.rs collect-diff-context-cli/src/bin/static_analysis.rs collect-diff-context-cli/tests/static_evidence.rs
rtk git commit -m "feat: port static evidence parsing to Rust"
```

### Task 5: Port Scope Mapping And Evidence Classification

**Files:**
- Modify: `collect-diff-context-cli/src/static_analysis/evidence.rs`
- Modify: `collect-diff-context-cli/src/review_scope.rs`
- Test: `collect-diff-context-cli/tests/static_evidence.rs`
- Test: `tests/static_analysis_evidence_test.sh`

- [ ] **Step 1: Add failing changed-line classification tests**

Cover `blocking-candidate`, `priority-candidate`, `note`, `outside-scope`, added-line promotion to `baseline_state: new`, report deduplication, truncation, and final scope drift.

- [ ] **Step 2: Run and verify red**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_evidence classification`

Expected: FAIL because findings are parsed but not mapped.

- [ ] **Step 3: Add the scope mapping implementation**

Expose a binary-safe added-line query from `review_scope`:

```rust
pub fn added_lines(
    repository: &Path,
    source: ReviewSource,
    selected_ref: &str,
    path: &str,
) -> Result<BTreeSet<u32>, ScopeError>;
```

Port the existing manifest-unit lookup, line-scope calculation, disposition rules, counts, deterministic finding ids, and decision contract. Call `revalidate_scope` immediately before returning evidence and compare fingerprint, units, groups, and work order.

- [ ] **Step 4: Make Rust and Shell evidence tests green**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_evidence`

Expected: PASS.

The Shell wrapper still uses Python at this point, so run: `rtk bash tests/static_analysis_evidence_test.sh`

Expected: existing Python-backed test remains PASS.

- [ ] **Step 5: Commit evidence classification**

```bash
rtk git add collect-diff-context-cli/src/review_scope.rs collect-diff-context-cli/src/static_analysis/evidence.rs collect-diff-context-cli/tests/static_evidence.rs
rtk git commit -m "feat: map Rust static evidence to review scope"
```

### Task 6: Port Candidate Snapshot Construction

**Files:**
- Create: `collect-diff-context-cli/src/static_analysis/snapshot.rs`
- Test: `collect-diff-context-cli/tests/static_execution_modes.rs`

- [ ] **Step 1: Write failing staged, unstaged, branch, symlink, and gitlink tests**

Assert staged snapshots use index blobs, unstaged snapshots use tracked working-tree bytes, branch snapshots use `HEAD`, `.git` and untracked files are absent, escaping symlinks fail, gitlinks are omitted, and file/byte bounds apply before execution.

- [ ] **Step 2: Run and verify red**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_execution_modes snapshot`

Expected: FAIL because `CandidateSnapshot` does not exist.

- [ ] **Step 3: Implement the owning snapshot interface**

```rust
pub struct SnapshotLimits {
    pub max_files: usize,
    pub max_bytes: u64,
}

pub struct CandidateSnapshot {
    root: tempfile::TempDir,
    pub snapshot_id: String,
    pub sha256: String,
    pub files: usize,
    pub bytes: u64,
}

impl CandidateSnapshot {
    pub fn materialize(
        repository: &Path,
        source: ReviewSource,
        limits: SnapshotLimits,
    ) -> Result<Self, SnapshotError>;
    pub fn path(&self) -> &Path;
    pub fn verify_unchanged(&self) -> Result<(), SnapshotError>;
}
```

Port `git cat-file --batch`, strict declared-size checks before allocation, safe relative path handling, unstaged copy, deterministic hashing, read-only permissions, and writable cleanup in `Drop`.

- [ ] **Step 4: Make snapshot tests green**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_execution_modes snapshot`

Expected: PASS.

- [ ] **Step 5: Commit snapshots**

```bash
rtk git add collect-diff-context-cli/src/static_analysis/snapshot.rs collect-diff-context-cli/tests/static_execution_modes.rs
rtk git commit -m "feat: build tracked candidate snapshots in Rust"
```

### Task 7: Port Profile Preflight And Bounded Execution

**Files:**
- Create: `collect-diff-context-cli/src/static_analysis/executor.rs`
- Modify: `collect-diff-context-cli/src/static_analysis/mod.rs`
- Test: `collect-diff-context-cli/tests/static_execution.rs`

- [ ] **Step 1: Write failing authorization and process tests**

Cover profile byte replacement after hashing, relative/inside-repository executable rejection, executable hash mismatch, repository-configuration authorization, no shell, environment allowlist, timeout, stdout/stderr overflow, non-success exits, invalid output, and process cleanup.

- [ ] **Step 2: Run and verify red**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_execution executor`

Expected: FAIL because preflight and executor interfaces are missing.

- [ ] **Step 3: Implement reusable preflight and execution interfaces**

```rust
pub struct PreparedProfile {
    pub profile_id: String,
    pub profile: StaticAnalysisProfile,
    pub profile_path: PathBuf,
    pub profile_sha256: String,
    pub executable_path: PathBuf,
    pub executable_sha256: String,
}

pub fn prepare_profile(
    repository: &Path,
    profile_path: &Path,
    expected_sha256: &str,
    allow_repository_configuration: bool,
) -> Result<PreparedProfile, RunError>;

pub struct ExecutionLimits {
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

pub fn execute_prepared(
    prepared: &PreparedProfile,
    snapshot: &CandidateSnapshot,
    source: ReviewSource,
    scope_fingerprint: &str,
    limits: ExecutionLimits,
) -> Result<ProcessOutcome, RunError>;
```

Use direct `Command` arguments, a fresh runtime home/temp directory, the current allowlisted variables, bounded capture threads, monotonic timeout checks, Unix process groups, and Windows Job Objects. Record only bounded stderr digest/length; never include raw stderr in artifacts.

- [ ] **Step 4: Make executor tests green on the host platform**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_execution executor`

Expected: PASS.

- [ ] **Step 5: Commit execution kernel**

```bash
rtk git add collect-diff-context-cli/src/static_analysis/executor.rs collect-diff-context-cli/src/static_analysis/mod.rs collect-diff-context-cli/tests/static_execution.rs collect-diff-context-cli/Cargo.toml collect-diff-context-cli/Cargo.lock
rtk git commit -m "feat: execute authorized analyzers in Rust"
```

### Task 8: Compose The Rust Single-Run Artifact

**Files:**
- Modify: `collect-diff-context-cli/src/static_analysis/executor.rs`
- Modify: `collect-diff-context-cli/src/static_analysis/output.rs`
- Modify: `collect-diff-context-cli/src/bin/static_analysis.rs`
- Test: `collect-diff-context-cli/tests/static_execution.rs`
- Test: `collect-diff-context-cli/tests/static_execution_modes.rs`

- [ ] **Step 1: Add failing end-to-end `run` tests**

Assert completed output links execution/evidence ids and scope; failed, timeout, output-limit, malformed payload, and tool-name/tool-version mismatch runs produce bounded failed/timeout or invalid-output evidence with no blocking candidates; repository/profile/executable/scope drift emits no authoritative artifact; `run --help` exits successfully and usage errors retain exit code `2`.

- [ ] **Step 2: Run and verify red**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_execution run_artifact`

Expected: FAIL because `run_analysis` is missing.

- [ ] **Step 3: Implement single-run composition**

```rust
pub struct RunRequest {
    pub repository: PathBuf,
    pub source: ReviewSource,
    pub expected_scope: String,
    pub profile_path: PathBuf,
    pub expected_profile_sha256: String,
    pub allow_repository_configuration: bool,
    pub max_findings: usize,
}

pub struct RunArtifact {
    pub execution: StaticAnalysisExecution,
    pub evidence: StaticAnalysisEvidence,
}

pub fn run_analysis(request: RunRequest) -> Result<RunArtifact, RunError>;
```

Open the typed scope, record repository state, prepare the profile, materialize one snapshot, execute, normalize stdout in-process, synthesize failure evidence when necessary, verify snapshot/profile/executable/repository/scope integrity, and only then return the artifact.

- [ ] **Step 4: Render the existing two-section contract**

```rust
pub fn render_run(artifact: &RunArtifact) -> Result<String, serde_json::Error> {
    Ok(format!(
        "# Pre-Commit Review Controlled Static Analysis\n\n## Static Analysis Execution JSON\n{}\n\n## Static Analysis Evidence JSON\n{}\n",
        serde_json::to_string(&artifact.execution)?,
        serde_json::to_string(&artifact.evidence)?
    ))
}
```

Wire `static-analysis-cli run` with the existing flags and error prefix `run_static_analysis:`.

- [ ] **Step 5: Run the Rust end-to-end suite**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_execution --test static_execution_modes`

Expected: PASS.

- [ ] **Step 6: Commit single-run composition**

```bash
rtk git add collect-diff-context-cli/src/bin/static_analysis.rs collect-diff-context-cli/src/static_analysis/executor.rs collect-diff-context-cli/src/static_analysis/output.rs collect-diff-context-cli/tests/static_execution.rs collect-diff-context-cli/tests/static_execution_modes.rs
rtk git commit -m "feat: emit Rust controlled analysis artifacts"
```

### Task 9: Prove Python/Rust Parity Internally

**Files:**
- Create: `tests/static_analysis_rust_parity_test.sh`
- Modify: `tests/lib/normalize_parity_output.py`
- Modify: `.github/workflows/lint.yml`
- Test: `tests/static_analysis_rust_parity_test.sh`

- [ ] **Step 1: Add direct implementation comparison fixtures**

Invoke `collect_static_evidence.py` and `static-analysis-cli collect` directly for normalized/SARIF success, truncation, failure, and scope errors. Invoke `run_static_analysis.py` and `static-analysis-cli run` directly for completed, failed, timeout, invalid-output, staged, unstaged, and branch cases.

- [ ] **Step 2: Normalize only allowed nondeterminism**

Extend the normalizer with one recursive function, call it before JSON serialization, and replace only `duration_ms` with `0`. Process ids and temporary paths must not be serialized by either implementation; make the parity test fail if keys such as `pid`, `process_id`, `snapshot_path`, or `runtime_path` appear. Leave hashes, ids, counts, statuses, snapshot digests, report order, findings, and exit codes untouched:

```python
def normalize_static_value(value):
    if isinstance(value, dict):
        if "duration_ms" in value:
            value["duration_ms"] = 0
        forbidden = {"pid", "process_id", "snapshot_path", "runtime_path"}
        unexpected = forbidden.intersection(value)
        if unexpected:
            raise ValueError(f"serialized runtime-only fields: {sorted(unexpected)}")
        for child in value.values():
            normalize_static_value(child)
    elif isinstance(value, list):
        for child in value:
            normalize_static_value(child)

normalize_static_value(data)
```

- [ ] **Step 3: Run parity and fix Rust behavior, not the expected output**

Run: `rtk bash tests/static_analysis_rust_parity_test.sh`

Expected: `static analysis Rust parity tests passed`.

- [ ] **Step 4: Add the temporary CI gate**

Run the parity script after building `static-analysis-cli`, without adding a public wrapper mode or environment selector.

- [ ] **Step 5: Commit the migration gate**

```bash
rtk git add tests/static_analysis_rust_parity_test.sh tests/lib/normalize_parity_output.py .github/workflows/lint.yml
rtk git commit -m "test: gate Rust static analysis parity"
```

### Task 10: Cut Shell Wrappers Over To Rust

**Files:**
- Create: `scripts/lib/static_analysis_cli.sh`
- Modify: `scripts/collect_static_evidence.sh`
- Modify: `scripts/run_static_analysis.sh`
- Modify: `tests/static_analysis_evidence_test.sh`
- Modify: `tests/static_analysis_execution_test.sh`
- Modify: `tests/static_analysis_execution_modes_test.sh`

- [ ] **Step 1: Add failing wrapper-resolution tests**

Assert wrappers accept an explicit absolute `PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN`, then a local `target/release/static-analysis-cli`, then the bundled platform binary; reject relative/non-executable overrides and never search `PATH`.

- [ ] **Step 2: Implement the shared resolver**

```bash
resolve_static_analysis_cli() {
  local script_dir="$1"
  local os_name arch_name static_binary_name
  if [ -n "${PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN:-}" ]; then
    case "$PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN" in /*) ;; *) return 2 ;; esac
    [ -x "$PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN" ] || return 2
    printf '%s\n' "$PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN"
    return 0
  fi
  os_name="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch_name="$(uname -m)"
  case "$os_name" in
    darwin) os_name='darwin' ;;
    msys*|mingw*|cygwin*) os_name='windows' ;;
    *) os_name='linux' ;;
  esac
  case "$arch_name" in
    x86_64|amd64) arch_name='amd64' ;;
    arm64|aarch64) arch_name='arm64' ;;
    *) return 2 ;;
  esac
  static_binary_name="static_analysis-${os_name}-${arch_name}"
  [ "$os_name" = 'windows' ] && static_binary_name="${static_binary_name}.exe"
  if [ -x "$script_dir/../collect-diff-context-cli/target/release/static-analysis-cli" ]; then
    printf '%s\n' "$script_dir/../collect-diff-context-cli/target/release/static-analysis-cli"
    return 0
  fi
  [ -x "$script_dir/bin/$static_binary_name" ] || return 2
  printf '%s\n' "$script_dir/bin/$static_binary_name"
}
```

- [ ] **Step 3: Replace Python invocation with Rust subcommands**

`collect_static_evidence.sh` runs `"$static_bin" collect "$@"`; `run_static_analysis.sh` runs `"$static_bin" run "$@"`. Preserve the existing temp-file capture, sanitizer, stderr, and exit behavior byte-for-byte.

- [ ] **Step 4: Run existing public integration tests against Rust**

Run: `rtk cargo build --release --manifest-path collect-diff-context-cli/Cargo.toml --bin static-analysis-cli`

Run: `rtk bash tests/static_analysis_evidence_test.sh`

Expected: `static analysis evidence tests passed`.

Run: `rtk bash tests/static_analysis_execution_test.sh`

Expected: `static analysis execution tests passed`.

Run: `rtk bash tests/static_analysis_execution_modes_test.sh`

Expected: `static analysis execution source-mode tests passed`.

- [ ] **Step 5: Commit the cutover**

```bash
rtk git add scripts/lib/static_analysis_cli.sh scripts/collect_static_evidence.sh scripts/run_static_analysis.sh tests/static_analysis_evidence_test.sh tests/static_analysis_execution_test.sh tests/static_analysis_execution_modes_test.sh
rtk git commit -m "feat: switch static analysis wrappers to Rust"
```

### Task 11: Package Both Rust Binaries

**Files:**
- Modify: `scripts/build_all_binaries.sh`
- Modify: `.github/workflows/lint.yml`
- Modify: `.github/workflows/release.yml`
- Modify: `install.sh`
- Modify: `tests/install_smoke_test.sh`
- Modify: `tests/install_agent_matrix_test.sh`

- [ ] **Step 1: Add failing package assertions**

Require `static_analysis-<platform>` in release staging and installed runtime payloads while preserving `collect_diff_context-<platform>` and Gitleaks behavior. Add a Linux/macOS/Windows CI matrix that builds both bins and runs platform-safe `static-analysis-cli collect --help` and `static-analysis-cli run --help` commands plus focused Rust contract tests before Python deletion is allowed.

- [ ] **Step 2: Build and copy both Cargo binaries per target**

For every target, copy:

```text
target/<triple>/release/collect-diff-context-cli -> scripts/bin/collect_diff_context-<platform>
target/<triple>/release/static-analysis-cli      -> scripts/bin/static_analysis-<platform>
```

On Windows append `.exe`. Upload both artifacts from the release matrix and include both in `pre-commit-review-runtime.tar.gz`. The release matrix must copy the exact static-analysis binary built for its target and smoke-run it on the native runner before upload.

- [ ] **Step 3: Update installer assertions**

The copy payload includes the bundled static binary when present and the wrappers remain installed even when the optional binary is unavailable in a source checkout.

- [ ] **Step 4: Run package tests**

Run: `rtk bash tests/install_smoke_test.sh`

Expected: `install.sh smoke tests passed`.

Run: `rtk bash tests/install_agent_matrix_test.sh`

Expected: `install agent matrix tests passed`.

The Task 12 cutover may start only after the Linux, macOS, and Windows static-analysis matrix is green on the branch.

- [ ] **Step 5: Commit packaging**

```bash
rtk git add scripts/build_all_binaries.sh .github/workflows/lint.yml .github/workflows/release.yml install.sh tests/install_smoke_test.sh tests/install_agent_matrix_test.sh
rtk git commit -m "build: package Rust static analysis binary"
```

### Task 12: Remove The Python Product Implementations

**Files:**
- Delete: `scripts/collect_static_evidence.py`
- Delete: `scripts/run_static_analysis.py`
- Delete: `tests/static_analysis_rust_parity_test.sh`
- Modify: `.github/workflows/lint.yml`
- Modify: `.github/workflows/release.yml`
- Modify: `tests/install_smoke_test.sh`
- Modify: `tests/skill_contract_test.sh`
- Modify: `README.md`
- Modify: `README.zh-CN.md`
- Modify: `docs/helper-capabilities.md`
- Modify: `docs/static-analysis-evidence.md`
- Modify: `docs/static-analysis-execution.md`

- [ ] **Step 1: Add failing absence and documentation assertions**

Assert the runtime package contains neither Python implementation, documentation says Rust is the only product runtime, and Python is mentioned only for the optional development schema validator.

- [ ] **Step 2: Delete the migration oracle and temporary CI gate**

Remove both Python product files, their executable-bit release steps, direct-import tests, and the temporary parity script. Keep `scripts/validate_schemas.py` and Python fixture-generation snippets used only by tests.

- [ ] **Step 3: Update user and operator documentation**

Document `static-analysis-cli collect|run`, wrapper compatibility, the explicit binary override, supported release assets, and that normal static-analysis runtime paths do not require Python.

- [ ] **Step 4: Run focused cutover checks**

Run: `rtk bash tests/skill_contract_test.sh`

Expected: `skill contract tests passed`.

Run: `rtk bash tests/install_smoke_test.sh`

Expected: `install.sh smoke tests passed`.

Run: `rtk rg -n --glob '!docs/superpowers/**' --glob '!docs/static-analysis-competitive-research.md' 'collect_static_evidence\.py|run_static_analysis\.py|PRE_COMMIT_REVIEW_STATIC_IMPL' README.md README.zh-CN.md docs references scripts tests install.sh .github`

Expected: no matches in active runtime, user, operator, test, installer, or workflow surfaces. Historical design/research documents are excluded explicitly.

- [ ] **Step 5: Commit Python removal**

```bash
rtk git add -A scripts/collect_static_evidence.py scripts/run_static_analysis.py tests/static_analysis_rust_parity_test.sh .github/workflows/lint.yml .github/workflows/release.yml tests/install_smoke_test.sh tests/skill_contract_test.sh README.md README.zh-CN.md docs/helper-capabilities.md docs/static-analysis-evidence.md docs/static-analysis-execution.md
rtk git commit -m "refactor: remove Python static analysis runtime"
```

### Task 13: Delivery A Completion Audit

**Files:**
- Verify every file touched in Tasks 1-12.

- [ ] **Step 1: Run Rust quality gates**

Run: `rtk cargo fmt --all --manifest-path collect-diff-context-cli/Cargo.toml -- --check`

Run: `rtk cargo clippy --manifest-path collect-diff-context-cli/Cargo.toml --all-targets -- -D warnings`

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml`

Expected: all PASS.

- [ ] **Step 2: Run focused static-analysis tests**

Run: `rtk bash tests/static_analysis_evidence_test.sh`

Run: `rtk bash tests/static_analysis_execution_test.sh`

Run: `rtk bash tests/static_analysis_execution_modes_test.sh`

Expected: all PASS against Rust wrappers.

- [ ] **Step 3: Run all repository deterministic tests and eval self-tests**

Run: `rtk zsh -c 'for test_file in tests/*_test.sh; do bash "$test_file" || exit 1; done'`

Run: `rtk zsh -c 'for test_file in evals/*_test.sh; do bash "$test_file" || exit 1; done'`

Expected: every script exits 0.

- [ ] **Step 4: Run static and packaging checks**

Run: `rtk shellcheck -S warning -s bash scripts/*.sh scripts/lib/*.sh install.sh tests/*.sh tests/lib/*.sh evals/*.sh`

Run: `rtk python3 scripts/validate_schemas.py`

Run: `rtk git diff --check`

Expected: all PASS.

- [ ] **Step 5: Confirm the final runtime surface**

Run: `rtk rg -n --glob '!docs/superpowers/**' --glob '!docs/static-analysis-competitive-research.md' 'collect_static_evidence\.py|run_static_analysis\.py|PRE_COMMIT_REVIEW_STATIC_IMPL' scripts install.sh .github/workflows tests README.md README.zh-CN.md docs`

Expected: no active runtime selector or Python product implementation reference.

- [ ] **Step 6: Commit any audit-only fixes**

Run `rtk git status --short` and commit only files changed to fix a failed audit gate, using the owning task's explicit file list. Skip this commit when the audit produces no changes; never stage unrelated work with a repository-wide add.
