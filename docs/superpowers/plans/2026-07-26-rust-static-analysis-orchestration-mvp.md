# Rust Static Analysis Orchestration MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add deterministic multi-analyzer orchestration over the Rust-only static-analysis kernel using one hash-pinned manifest, one shared candidate snapshot, serial bounded execution, honest terminal states, and independent reducer-compatible evidence.

**Architecture:** The `orchestration` module is the deep public module: it preflights every manifest/profile/entrypoint before execution, opens one authoritative scope, materializes one snapshot, runs prepared profiles serially, accounts cumulative budgets, and returns an orchestration artifact plus one combined `static_analysis_evidence/v1`. `evidence_union` namespaces technical ids by execution but never semantically merges findings or changes severity/confidence.

**Tech Stack:** Rust 2021 library from Delivery A, serde/serde_json, sha2, tempfile, Bash compatibility wrapper, Git integration fixtures, JSON Schema draft 2020-12, existing Python development validator.

---

## Prerequisite And Scope

Execute this plan only after `2026-07-26-rust-static-analysis-consolidation.md` is complete and the Python product implementations are absent.

Do not add analyzer discovery, installation, builds, dependency preparation, external resource bundles, parallelism, caching, PR annotations, IDE integration, central policy, cross-tool semantic grouping, corroboration weighting, or `static_analysis_input/v2`.

Remaining profiles stopped by a snapshot-integrity failure are represented explicitly as `not-run/shared-integrity-failure`; budget exhaustion uses `not-run/budget-exhausted`. This closes the approved design's requirement that failed and never-started profiles remain distinguishable.

## File Map

**Create:**

- `collect-diff-context-cli/schemas/static-analysis-orchestration-manifest.schema.json`
- `collect-diff-context-cli/schemas/static-analysis-orchestration.schema.json`
- `collect-diff-context-cli/src/static_analysis/evidence_union.rs`
- `collect-diff-context-cli/src/static_analysis/orchestration.rs`
- `collect-diff-context-cli/tests/static_orchestration.rs`
- `tests/static_analysis_orchestration_test.sh`
- `scripts/orchestrate_static_analysis.sh`
- `docs/static-analysis-orchestration.md`
- `references/decision/static-analysis-orchestration.md`

**Modify:**

- `collect-diff-context-cli/src/static_analysis/mod.rs`
- `collect-diff-context-cli/src/static_analysis/contracts.rs`
- `collect-diff-context-cli/schemas/static-analysis-evidence.schema.json`
- `collect-diff-context-cli/src/static_analysis/executor.rs`
- `collect-diff-context-cli/src/static_analysis/snapshot.rs`
- `collect-diff-context-cli/src/static_analysis/output.rs`
- `collect-diff-context-cli/src/bin/static_analysis.rs`
- `scripts/validate_schemas.py`
- `install.sh`
- `.github/workflows/lint.yml`
- `.github/workflows/release.yml`
- `tests/install_smoke_test.sh`
- `tests/skill_contract_test.sh`
- `SKILL.md`
- `README.md`
- `README.zh-CN.md`
- `docs/helper-capabilities.md`
- `references/decision/finding-verification.md`
- `references/decision/verdict-rules.md`
- `evals/output-eval.json`
- `evals/output/advanced-output-eval.json`
- `evals/output_eval_runner.sh`
- `evals/output_eval_runner_test.sh`
- `evals/eval_contract_test.sh`

### Task 1: Define Manifest And Orchestration Contracts

**Files:**
- Create: `collect-diff-context-cli/schemas/static-analysis-orchestration-manifest.schema.json`
- Create: `collect-diff-context-cli/schemas/static-analysis-orchestration.schema.json`
- Modify: `collect-diff-context-cli/schemas/static-analysis-evidence.schema.json`
- Modify: `collect-diff-context-cli/src/static_analysis/contracts.rs`
- Modify: `scripts/validate_schemas.py`
- Test: `collect-diff-context-cli/tests/static_orchestration.rs`
- Test: `tests/static_analysis_orchestration_test.sh`

- [ ] **Step 1: Write failing strict-contract tests**

Cover valid manifest/artifact examples and reject unknown fields, relative paths, uppercase/short hashes, zero or more than 16 profiles, duplicate `profile_id`, duplicate path/hash pairs, out-of-range budgets, invalid run unions, and inconsistent overall status. Include a valid `failed` artifact where the first analyzer mutates the snapshot, every later profile is not run, and the combined v1 evidence contains zero reports and zero findings.

- [ ] **Step 2: Run and verify red**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_orchestration contracts`

Expected: FAIL because orchestration contract types do not exist.

- [ ] **Step 3: Add the two strict JSON schemas**

The manifest requires exactly:

```json
{
  "schema_version": 1,
  "kind": "static_analysis_orchestration_manifest",
  "name": "trusted pre-commit analyzer set",
  "profiles": [
    {"profile_id": "security", "path": "/opt/review/security.json", "sha256": "<64-hex>"}
  ],
  "limits": {
    "max_execution_seconds": 600,
    "max_captured_output_bytes": 30000000,
    "max_findings": 5000,
    "max_snapshot_bytes": 536870912,
    "max_snapshot_files": 100000
  }
}
```

The orchestration artifact contains authoritative scope, manifest identity, snapshot identity, budget ledger, overall status, ordered run entries, report ids, and finding ids. Define `not-run` reasons `budget-exhausted` and `shared-integrity-failure`; define `invalidated` reason `snapshot-mutated`.

Relax only the lower bounds of `static-analysis-evidence.schema.json` so orchestration can emit a reducer-compatible empty evidence object after a first-run snapshot invalidation: `reports.minItems` becomes `0` and `counts.reports.minimum` becomes `0`. Standalone `collect` still requires at least one `--result`, so its behavior does not change. Add semantic tests proving empty evidence is accepted only as the companion to an orchestration with no executed run evidence.

- [ ] **Step 4: Add typed Rust contracts**

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationManifest {
    pub schema_version: u8,
    pub kind: String,
    pub name: String,
    pub profiles: Vec<ManifestProfileRef>,
    pub limits: OrchestrationLimits,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "run_kind", rename_all = "kebab-case")]
pub enum OrchestrationRun {
    Executed { profile_id: String, execution: StaticAnalysisExecution },
    NotRun { profile_id: String, reason: NotRunReason },
    Invalidated { profile_id: String, reason: InvalidationReason },
}

#[derive(Debug, Clone, Serialize)]
pub struct OrchestrationArtifact {
    pub schema_version: u8,
    pub kind: &'static str,
    pub authoritative: bool,
    pub orchestration_id: String,
    pub scope: EvidenceScope,
    pub manifest: ManifestIdentity,
    pub snapshot: OrchestrationSnapshot,
    pub status: OrchestrationStatus,
    pub budgets: BudgetRecord,
    pub runs: Vec<OrchestrationRun>,
    pub report_ids: Vec<String>,
    pub finding_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrchestrationSnapshot {
    pub snapshot_id: String,
    pub kind: &'static str,
    pub sha256: String,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BudgetAmount {
    pub initial: u64,
    pub consumed: u64,
    pub remaining: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BudgetRecord {
    pub execution_millis: BudgetAmount,
    pub captured_output_bytes: BudgetAmount,
    pub findings: BudgetAmount,
    pub snapshot_files: BudgetAmount,
    pub snapshot_bytes: BudgetAmount,
}
```

- [ ] **Step 5: Make schema and contract tests green**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_orchestration contracts`

Run: `rtk python3 scripts/validate_schemas.py`

Expected: both PASS.

- [ ] **Step 6: Commit contracts**

```bash
rtk git add collect-diff-context-cli/schemas/static-analysis-orchestration-manifest.schema.json collect-diff-context-cli/schemas/static-analysis-orchestration.schema.json collect-diff-context-cli/schemas/static-analysis-evidence.schema.json collect-diff-context-cli/src/static_analysis/contracts.rs collect-diff-context-cli/tests/static_orchestration.rs scripts/validate_schemas.py tests/static_analysis_orchestration_test.sh
rtk git commit -m "feat: define static analysis orchestration contracts"
```

### Task 2: Preflight The Complete Declared Entrypoint Set

**Files:**
- Create: `collect-diff-context-cli/src/static_analysis/orchestration.rs`
- Modify: `collect-diff-context-cli/src/static_analysis/mod.rs`
- Modify: `collect-diff-context-cli/src/static_analysis/executor.rs`
- Test: `collect-diff-context-cli/tests/static_orchestration.rs`

- [ ] **Step 1: Add failing preflight tests**

Assert no analyzer marker is created when the manifest hash, any profile hash, profile schema, executable hash, duplicate profile reference, repository-configuration authorization, or manifest limit fails.

- [ ] **Step 2: Run and verify red**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_orchestration preflight`

Expected: FAIL because `prepare_orchestration` is missing.

- [ ] **Step 3: Implement byte-bound manifest loading**

```rust
pub struct OrchestrationRequest {
    pub repository: PathBuf,
    pub source: ReviewSource,
    pub expected_scope: String,
    pub manifest_path: PathBuf,
    pub expected_manifest_sha256: String,
    pub allow_repository_configuration: bool,
}

pub struct PreparedOrchestration {
    pub manifest: OrchestrationManifest,
    pub manifest_path: PathBuf,
    pub manifest_sha256: String,
    pub manifest_id: String,
    pub profiles: Vec<PreparedManifestProfile>,
}

pub fn prepare_orchestration(
    request: &OrchestrationRequest,
) -> Result<PreparedOrchestration, OrchestrationError>;
```

Read and hash the exact manifest bytes once, validate all profile refs in order, call Delivery A's `prepare_profile` for every profile, and finish all authorization before opening a snapshot or executing any process. Record only entrypoint authorization; do not claim undeclared dependency closure.

- [ ] **Step 4: Add final authorization revalidation**

```rust
impl PreparedOrchestration {
    pub fn revalidate(&self) -> Result<(), OrchestrationError>;
}
```

Rehash manifest, every profile, and every entrypoint executable before artifact release. Any mismatch returns an error and releases no authoritative orchestration/evidence output.

- [ ] **Step 5: Make preflight tests green**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_orchestration preflight`

Expected: PASS and no marker from rejected manifests.

- [ ] **Step 6: Commit preflight**

```bash
rtk git add collect-diff-context-cli/src/static_analysis/mod.rs collect-diff-context-cli/src/static_analysis/orchestration.rs collect-diff-context-cli/src/static_analysis/executor.rs collect-diff-context-cli/tests/static_orchestration.rs
rtk git commit -m "feat: preflight analyzer manifests"
```

### Task 3: Reuse One Snapshot Across Prepared Profiles

**Files:**
- Modify: `collect-diff-context-cli/src/static_analysis/snapshot.rs`
- Modify: `collect-diff-context-cli/src/static_analysis/executor.rs`
- Modify: `collect-diff-context-cli/src/static_analysis/orchestration.rs`
- Test: `collect-diff-context-cli/tests/static_orchestration.rs`

- [ ] **Step 1: Add a failing shared-snapshot identity test**

Use two fixture analyzers that print `PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT` and inspect the same files. Assert both accepted executions record the same snapshot SHA/files/bytes and the snapshot is built only once.

- [ ] **Step 2: Run and verify red**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_orchestration shared_snapshot`

Expected: FAIL because orchestration cannot execute prepared profiles.

- [ ] **Step 3: Calculate effective snapshot limits once**

```rust
fn effective_snapshot_limits(prepared: &PreparedOrchestration) -> SnapshotLimits {
    SnapshotLimits {
        max_files: prepared.profiles.iter()
            .map(|item| item.prepared.profile.limits.max_snapshot_files)
            .chain(std::iter::once(prepared.manifest.limits.max_snapshot_files))
            .min().unwrap(),
        max_bytes: prepared.profiles.iter()
            .map(|item| item.prepared.profile.limits.max_snapshot_bytes)
            .chain(std::iter::once(prepared.manifest.limits.max_snapshot_bytes))
            .min().unwrap(),
    }
}
```

Open the authoritative scope, record repository state, materialize one `CandidateSnapshot`, and pass `&CandidateSnapshot` into every `execute_prepared` call.

- [ ] **Step 4: Verify snapshot integrity around every tool**

Call `verify_unchanged()` before and after each analyzer. A pre-run mismatch invalidates the profile that was about to start; a post-run mismatch invalidates the profile that just ran. In both cases emit no authoritative execution/evidence for that profile, stop scheduling, and mark every later profile `not-run/shared-integrity-failure`.

- [ ] **Step 5: Make the shared-snapshot test green**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_orchestration shared_snapshot`

Expected: PASS.

- [ ] **Step 6: Commit shared snapshot reuse**

```bash
rtk git add collect-diff-context-cli/src/static_analysis/snapshot.rs collect-diff-context-cli/src/static_analysis/executor.rs collect-diff-context-cli/src/static_analysis/orchestration.rs collect-diff-context-cli/tests/static_orchestration.rs
rtk git commit -m "feat: share one analyzer snapshot"
```

### Task 4: Add Deterministic Cumulative Budget Accounting

**Files:**
- Modify: `collect-diff-context-cli/src/static_analysis/orchestration.rs`
- Modify: `collect-diff-context-cli/src/static_analysis/executor.rs`
- Test: `collect-diff-context-cli/tests/static_orchestration.rs`

- [ ] **Step 1: Add failing time and output budget tests**

Use a deterministic test clock and fixture analyzers with known output sizes. Cover effective per-tool timeout, cumulative consumption, exact remaining values for time/output/findings/snapshot files/snapshot bytes, output overflow, and remaining tools marked `not-run/budget-exhausted`.

- [ ] **Step 2: Run and verify red**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_orchestration budgets`

Expected: FAIL because no budget ledger exists.

- [ ] **Step 3: Implement the private ledger and clock seam**

```rust
struct BudgetLedger {
    initial_millis: u64,
    remaining_millis: u64,
    initial_output_bytes: usize,
    remaining_output_bytes: usize,
    finding_limit: usize,
    snapshot_file_limit: usize,
    snapshot_byte_limit: u64,
}

impl BudgetLedger {
    fn effective_limits(&self, profile: &ProfileLimits) -> Option<ExecutionLimits>;
    fn consume(&mut self, outcome: &ProcessOutcome);
    fn record_findings(&mut self, total_independent: usize);
    fn record_snapshot(&mut self, snapshot: &CandidateSnapshot);
    fn record(&self) -> BudgetRecord;
}
```

Extend Delivery A's private execution limits for orchestration:

```rust
pub struct ExecutionLimits {
    pub timeout: Duration,
    pub max_stream_output_bytes: usize,
    pub max_combined_output_bytes: usize,
}
```

The single-run adapter sets `max_combined_output_bytes` to twice the profile per-stream limit, preserving v1 behavior. Orchestration sets it to the remaining cumulative allowance and shares one counter across stdout/stderr capture. Time counts analyzer process duration only. Output consumption is stored stdout plus stored stderr bytes including overflow sentinel bytes. Findings are recorded after independent union; snapshot files/bytes are paid once. When time or captured-output allowance has no positive remainder, do not start another tool.

Keep the system clock behind Delivery A's public `execute_prepared` wrapper and expose a crate-private deterministic seam for orchestration tests:

```rust
pub(crate) trait Clock {
    fn now(&self) -> Duration;
}

pub(crate) fn execute_prepared_with_clock(
    prepared: &PreparedProfile,
    snapshot: &CandidateSnapshot,
    source: ReviewSource,
    scope_fingerprint: &str,
    limits: ExecutionLimits,
    clock: &dyn Clock,
) -> Result<ProcessOutcome, RunError>;
```

`execute_prepared` delegates to this function with `SystemClock`; tests pass a sequence clock so timeout and consumed-duration assertions contain no wall-clock tolerance.

- [ ] **Step 4: Make budget tests green**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_orchestration budgets`

Expected: PASS.

- [ ] **Step 5: Commit budget accounting**

```bash
rtk git add collect-diff-context-cli/src/static_analysis/orchestration.rs collect-diff-context-cli/src/static_analysis/executor.rs collect-diff-context-cli/tests/static_orchestration.rs
rtk git commit -m "feat: enforce orchestration budgets"
```

### Task 5: Implement Serial Scheduling And Terminal Statuses

**Files:**
- Modify: `collect-diff-context-cli/src/static_analysis/orchestration.rs`
- Test: `collect-diff-context-cli/tests/static_orchestration.rs`

- [ ] **Step 1: Add failing scheduler tests**

Cover strict manifest order, continue-after-non-success/timeout/output-limit/invalid-output, stop-after-snapshot-mutation, all accepted=`completed`, mixed accepted/unavailable=`partial`, none accepted=`failed`, and no artifact on final manifest/profile/executable/repository/scope drift.

- [ ] **Step 2: Run and verify red**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_orchestration scheduler`

Expected: FAIL because `execute` is incomplete.

- [ ] **Step 3: Implement the deep module interface**

```rust
pub struct OrchestrationOutput {
    pub orchestration: OrchestrationArtifact,
    pub evidence: StaticAnalysisEvidence,
}

pub fn execute(
    request: OrchestrationRequest,
) -> Result<OrchestrationOutput, OrchestrationError>;
```

For tool-local failures, keep linked failed/timeout evidence and continue. For snapshot mutation, discard the current execution/evidence, emit `invalidated/snapshot-mutated`, stop, and mark later profiles not run. Before returning, revalidate scope, repository state, manifest, profiles, and entrypoints.

- [ ] **Step 4: Compute deterministic ids**

Use NUL-separated SHA256 material. `manifest_id` is the first 16 hex chars of the manifest SHA256; `orchestration_id` hashes scope fingerprint, manifest SHA256, snapshot SHA256, and ordered terminal tuples of manifest `profile_id`, terminal run kind/reason, and execution id or the empty string when no execution exists.

- [ ] **Step 5: Make scheduler tests green**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_orchestration scheduler`

Expected: PASS.

- [ ] **Step 6: Commit scheduling**

```bash
rtk git add collect-diff-context-cli/src/static_analysis/orchestration.rs collect-diff-context-cli/tests/static_orchestration.rs
rtk git commit -m "feat: schedule analyzers serially"
```

### Task 6: Union Evidence Without Semantic Merging

**Files:**
- Create: `collect-diff-context-cli/src/static_analysis/evidence_union.rs`
- Modify: `collect-diff-context-cli/src/static_analysis/mod.rs`
- Modify: `collect-diff-context-cli/src/static_analysis/orchestration.rs`
- Test: `collect-diff-context-cli/tests/static_orchestration.rs`

- [ ] **Step 1: Add failing provenance and duplicate tests**

Use two tools that report the same path, line, message, and severity. Assert two findings remain, manifest order is stable, ids are unique even when raw report ids collide, counts sum correctly, and truncation occurs only after union.

- [ ] **Step 2: Run and verify red**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_orchestration evidence_union`

Expected: FAIL because `union_evidence` is missing.

- [ ] **Step 3: Implement technical id namespacing**

```rust
pub fn union_evidence(
    scope: &EvidenceScope,
    runs: &mut [EvidenceRun],
    max_findings: usize,
) -> Result<StaticAnalysisEvidence, OrchestrationError>;

pub struct EvidenceRun {
    pub execution: StaticAnalysisExecution,
    pub evidence: StaticAnalysisEvidence,
}
```

Pass every authoritative `executed` run, including failed, timeout, output-limit, and invalid-output executions; only snapshot-invalidated and not-run entries have no `EvidenceRun`. For each run, derive `combined_report_id = compact_hash("orchestration-report-v1", execution_id, source_report_id)` and `combined_finding_id = compact_hash("orchestration-finding-v1", execution_id, source_finding_id)`. Rewrite the orchestration copy of `execution.evidence.report_ids`, report ids, finding ids, and finding report-id links consistently. Do not compare message, path, line, rule, CWE, category, severity, or confidence for grouping.

- [ ] **Step 4: Aggregate counts and truncation honestly**

Sum report/input/deduplicated/mapped/disposition counts from every source evidence. Preserve `truncated: true` if any source was truncated or the combined independent finding list exceeds the manifest limit. Order reports and findings by manifest profile order, then their source deterministic order. Record findings budget consumption as `min(total_independent_findings, max_findings)` and remaining as the saturating difference; truncation does not erase the full counts.

- [ ] **Step 5: Make evidence-union tests green**

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --test static_orchestration evidence_union`

Expected: PASS with two independent duplicate findings.

- [ ] **Step 6: Commit evidence union**

```bash
rtk git add collect-diff-context-cli/src/static_analysis/evidence_union.rs collect-diff-context-cli/src/static_analysis/mod.rs collect-diff-context-cli/src/static_analysis/orchestration.rs collect-diff-context-cli/tests/static_orchestration.rs
rtk git commit -m "feat: union analyzer evidence independently"
```

### Task 7: Add The `orchestrate` CLI And Shell Entrypoint

**Files:**
- Modify: `collect-diff-context-cli/src/static_analysis/output.rs`
- Modify: `collect-diff-context-cli/src/bin/static_analysis.rs`
- Create: `scripts/orchestrate_static_analysis.sh`
- Modify: `scripts/lib/static_analysis_cli.sh`
- Test: `tests/static_analysis_orchestration_test.sh`

- [ ] **Step 1: Add a failing public CLI integration test**

Invoke the Shell wrapper with `--source`, `--expect-scope`, `--manifest`, `--expect-manifest-sha256`, and optional `--allow-repository-configuration`; validate both JSON sections and sanitizer behavior.

- [ ] **Step 2: Run and verify red**

Run: `rtk bash tests/static_analysis_orchestration_test.sh`

Expected: FAIL because the wrapper and CLI subcommand do not exist.

- [ ] **Step 3: Render the two-section output**

```rust
pub fn render_orchestration(output: &OrchestrationOutput) -> Result<String, serde_json::Error> {
    Ok(format!(
        "# Pre-Commit Review Static Analysis Orchestration\n\n## Static Analysis Orchestration JSON\n{}\n\n## Static Analysis Evidence JSON\n{}\n",
        serde_json::to_string(&output.orchestration)?,
        serde_json::to_string(&output.evidence)?
    ))
}
```

Wire `static-analysis-cli orchestrate`. Use error prefix `orchestrate_static_analysis:` and exit `2` for authorization, contract, scope, or integrity failures that release no artifact.

- [ ] **Step 4: Reuse wrapper binary resolution and sanitizer**

The new wrapper calls `"$static_bin" orchestrate "$@"`, uses stream name `controlled-static-analysis-orchestration-stdout`, and preserves the same disabled/unavailable/redacted sanitizer states as the existing wrappers.

- [ ] **Step 5: Make the public integration test green**

Run: `rtk bash tests/static_analysis_orchestration_test.sh`

Expected: `static analysis orchestration tests passed`.

- [ ] **Step 6: Commit the entrypoint**

```bash
rtk git add collect-diff-context-cli/src/bin/static_analysis.rs collect-diff-context-cli/src/static_analysis/output.rs scripts/orchestrate_static_analysis.sh scripts/lib/static_analysis_cli.sh tests/static_analysis_orchestration_test.sh
rtk git commit -m "feat: expose static analysis orchestration"
```

### Task 8: Integrate Review Policy, Documentation, And Evaluations

**Files:**
- Create: `docs/static-analysis-orchestration.md`
- Create: `references/decision/static-analysis-orchestration.md`
- Modify: `SKILL.md`
- Modify: `README.md`
- Modify: `README.zh-CN.md`
- Modify: `docs/helper-capabilities.md`
- Modify: `references/decision/finding-verification.md`
- Modify: `references/decision/verdict-rules.md`
- Modify: `tests/skill_contract_test.sh`
- Modify: `evals/output-eval.json`
- Modify: `evals/output/advanced-output-eval.json`
- Modify: `evals/output_eval_runner.sh`
- Modify: `evals/output_eval_runner_test.sh`
- Modify: `evals/eval_contract_test.sh`

- [ ] **Step 1: Add failing skill-contract assertions**

Require exact manifest path/hash authorization, no discovery, supported self-contained analyzer class, build-coupled tools routed to precomputed evidence, `partial` honesty, independent findings, no `input/v2`, and final scope/authorization revalidation.

- [ ] **Step 2: Document the operator workflow and support boundary**

Include the manifest example, ASCII flow, completed/partial/failed table, run-entry union, cumulative budgets, snapshot mutation behavior, source-only analyzer requirements, and the statement that entrypoint hashing is not a complete arbitrary-analyzer execution closure.

- [ ] **Step 3: Update review reduction rules**

Static orchestration evidence never marks manifest units reviewed. Failed, invalidated, and not-run profiles are unavailable verification; only completed accepted reports may support findings, and every blocking/priority candidate still passes independent verification.

- [ ] **Step 4: Add model behavior evaluation**

Add a case with one completed security analyzer and one timeout. The expected response must call the orchestration `partial`, use only the completed evidence as a candidate, preserve the timeout as a limitation, and avoid claiming broad static coverage.

- [ ] **Step 5: Run focused contracts and evals**

Run: `rtk bash tests/skill_contract_test.sh`

Run: `rtk bash evals/eval_contract_test.sh`

Run: `rtk bash evals/output_eval_runner_test.sh`

Expected: all PASS.

- [ ] **Step 6: Commit policy and docs**

```bash
rtk git add docs/static-analysis-orchestration.md references/decision/static-analysis-orchestration.md SKILL.md README.md README.zh-CN.md docs/helper-capabilities.md references/decision/finding-verification.md references/decision/verdict-rules.md tests/skill_contract_test.sh evals/output-eval.json evals/output/advanced-output-eval.json evals/output_eval_runner.sh evals/output_eval_runner_test.sh evals/eval_contract_test.sh
rtk git commit -m "docs: integrate static analysis orchestration"
```

### Task 9: Package And Validate The Orchestration Surface

**Files:**
- Modify: `install.sh`
- Modify: `.github/workflows/lint.yml`
- Modify: `.github/workflows/release.yml`
- Modify: `tests/install_smoke_test.sh`
- Modify: `scripts/validate_schemas.py`

- [ ] **Step 1: Add failing installation and schema assertions**

Require the orchestration wrapper, reference, docs-linked schemas, manifest validation option, and orchestration output validation option in installed/release payloads.

- [ ] **Step 2: Package the new wrapper and schemas**

The existing `static_analysis-<platform>` binary already contains the subcommand. Add the wrapper, both schemas, executable bit, CI integration test, and release smoke validation; do not add another platform binary.

- [ ] **Step 3: Extend semantic schema validation**

Validate that orchestration scope equals combined evidence scope, report/finding id sets match, completed/partial/failed status matches run states, executed report ids exist, invalidated/not-run entries expose no execution object, and failed/timeout reports have no blocking candidates. Permit zero reports only when there are no `executed` entries; otherwise every executed entry's rewritten report ids must exist in combined evidence.

- [ ] **Step 4: Run packaging checks**

Run: `rtk bash tests/install_smoke_test.sh`

Run: `rtk python3 scripts/validate_schemas.py`

Run: `rtk bash tests/static_analysis_orchestration_test.sh`

Expected: all PASS.

- [ ] **Step 5: Commit packaging**

```bash
rtk git add install.sh .github/workflows/lint.yml .github/workflows/release.yml tests/install_smoke_test.sh scripts/validate_schemas.py collect-diff-context-cli/schemas/static-analysis-orchestration-manifest.schema.json collect-diff-context-cli/schemas/static-analysis-orchestration.schema.json
rtk git commit -m "build: package static analysis orchestration"
```

### Task 10: Delivery B Completion Audit

**Files:**
- Verify all files touched in Tasks 1-9.

- [ ] **Step 1: Run Rust gates**

Run: `rtk cargo fmt --all --manifest-path collect-diff-context-cli/Cargo.toml -- --check`

Run: `rtk cargo clippy --manifest-path collect-diff-context-cli/Cargo.toml --all-targets -- -D warnings`

Run: `rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml`

Expected: all PASS.

- [ ] **Step 2: Run all static-analysis public integrations**

Run: `rtk bash tests/static_analysis_evidence_test.sh`

Run: `rtk bash tests/static_analysis_execution_test.sh`

Run: `rtk bash tests/static_analysis_execution_modes_test.sh`

Run: `rtk bash tests/static_analysis_orchestration_test.sh`

Expected: all PASS.

- [ ] **Step 3: Run all deterministic tests and eval self-tests**

Run: `rtk zsh -c 'for test_file in tests/*_test.sh; do bash "$test_file" || exit 1; done'`

Run: `rtk zsh -c 'for test_file in evals/*_test.sh; do bash "$test_file" || exit 1; done'`

Expected: every script exits 0.

- [ ] **Step 4: Run static quality gates**

Run: `rtk shellcheck -S warning -s bash scripts/*.sh scripts/lib/*.sh install.sh tests/*.sh tests/lib/*.sh evals/*.sh`

Run: `rtk python3 scripts/validate_schemas.py`

Run: `rtk git diff --check`

Expected: all PASS.

- [ ] **Step 5: Audit approved design invariants**

Confirm tests prove: preflight before execution, one snapshot identity, strict serial order, per-profile plus cumulative budgets, honest completed/partial/failed states, explicit invalidated/not-run entries, independent findings, no Python runtime, no public implementation selector, and no claim of complete execution closure.

- [ ] **Step 6: Commit audit-only fixes**

Run `rtk git status --short` and commit only files changed to fix a failed audit gate, using the owning task's explicit file list. Skip this commit when the audit produces no changes; never stage unrelated work with a repository-wide add.
