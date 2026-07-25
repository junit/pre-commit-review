# Rust Multi-Analyzer Orchestration Design

## Status

Approved design for phase three of static-analysis support.

Phase one ingests explicitly supplied SARIF or normalized JSON. Phase two runs one explicitly authorized, hash-pinned analyzer in a bounded candidate snapshot. Phase three adds deterministic orchestration for multiple analyzers and moves the default static-analysis runtime into Rust.

## Goals

- Authorize an ordered analyzer set through one absolute orchestration-manifest path and its exact SHA256.
- Pin every referenced profile and executable by exact SHA256 before any analyzer starts.
- Materialize one authoritative candidate snapshot and reuse its identity for every analyzer.
- Execute analyzers serially in manifest order.
- Apply both orchestration-wide cumulative limits and existing per-profile limits.
- Continue after tool-local failures and preserve accepted evidence from other tools.
- Aggregate corroborating findings conservatively without inflating severity or confidence merely because multiple tools reported them.
- Preserve the existing single-analyzer CLI and JSON contracts.
- Make Rust the default runtime for report collection, single execution, and orchestration.
- Remove Python as a default runtime dependency while retaining one explicit compatibility mode during migration.

## Non-Goals

- Automatic analyzer, profile, plugin, package-script, or build-target discovery.
- Repository-owned orchestration manifests or profiles without an external exact-hash authorization.
- Parallel execution or dependency-graph scheduling.
- Kernel-level hostile-code sandboxing or a guaranteed network namespace.
- Automatic installation or downloading of analyzers.
- Treating tool success as review coverage or a clean result as proof that a change is safe.
- Maintaining a cross-tool rule-alias registry in phase three.

## Why Rust

The authoritative diff control plane, scope fingerprints, reducer structures, and output sanitizer already live in the Rust crate. Python was a reasonable incremental choice for the optional phase-one and phase-two lanes because it allowed rapid SARIF/JSON and process-control work without changing the bundled Rust release chain.

That trade-off stops being attractive once orchestration becomes a core capability. Extending the Python implementation would maintain two security-relevant implementations of Git access, snapshot identity, process execution, and structured contracts. It would also preserve a Python runtime dependency for the most complex execution path.

Phase three therefore uses a Rust-first design. The existing Python implementations remain temporarily as explicit parity references; they receive no new orchestration features.

## Considered Approaches

### Repeatedly invoke the existing Python runner

This has the smallest initial diff, but every analyzer would rebuild its snapshot and reopen the control plane. It cannot guarantee one shared snapshot identity and makes cumulative budget enforcement approximate. Rejected.

### Add a Python orchestration package

This can share a snapshot after refactoring the Python runner, but it deepens the split between the Rust control plane and the Python static-analysis runtime. It also leaves deployment and behavior-parity costs in place. Rejected.

### Extract a Rust library and add a Rust static-analysis binary

This is the selected approach. It concentrates scope, snapshot, execution, evidence, and orchestration behavior in one Rust implementation while preserving Shell entrypoints and JSON contracts.

## Architecture

The current crate becomes a library plus binaries:

```text
Shell compatibility entrypoints
          |
          +-----------------------------+
          |                             |
          v                             v
collect-diff-context binary    static-analysis-cli binary
                                        |
                                        +-- collect
                                        +-- run
                                        +-- orchestrate
                                                  |
                                                  v
┌──────────────────── Rust library ─────────────────────────┐
│ review_scope       authoritative scope and state digest   │
│ static_analysis::contracts                                │
│ static_analysis::snapshot                                 │
│ static_analysis::executor                                 │
│ static_analysis::evidence                                 │
│ static_analysis::aggregation                              │
│ static_analysis::orchestration                            │
└───────────────────────────────────────────────────────────┘
```

Suggested source layout:

```text
collect-diff-context-cli/src/
├── lib.rs
├── main.rs
├── review_scope.rs
├── secret_scan.rs
├── bin/
│   └── static_analysis.rs
└── static_analysis/
    ├── mod.rs
    ├── contracts.rs
    ├── snapshot.rs
    ├── executor.rs
    ├── evidence.rs
    ├── aggregation.rs
    └── orchestration.rs
```

The orchestration module is a deep module. Its primary interface is:

```rust
pub fn execute(
    request: OrchestrationRequest,
) -> Result<OrchestrationArtifact, OrchestrationError>;
```

Callers and orchestration-level tests use this interface. Manifest parsing, preflight authorization, Git plumbing, snapshot construction, process supervision, budget accounting, and aggregation remain implementation details.

The local filesystem, real Git repositories, and fixture executables are local-substitutable dependencies. Integration tests use temporary real repositories and processes instead of exposing public mock ports. A private clock seam may have system and deterministic-test adapters for cumulative-budget tests.

## Compatibility Interfaces

Existing entrypoints remain stable:

```text
scripts/collect_static_evidence.sh
scripts/run_static_analysis.sh
```

The new entrypoint is:

```text
scripts/orchestrate_static_analysis.sh
```

The Shell wrappers select the bundled or locally built Rust static-analysis binary, apply the existing optional output sanitizer, and preserve current exit-code behavior. They do not parse or reinterpret the Rust JSON artifact.

`run_static_analysis.sh` continues to emit one `static_analysis_execution/v1` section and one linked `static_analysis_evidence/v1` section. Internally, the Rust implementation may reuse orchestration primitives, but the section markers, schemas, and semantic fields of the single-run output remain compatible. Parity tests normalize only nondeterministic values such as duration and temporary paths.

## Authorization Manifest

The new contract is `static_analysis_orchestration_manifest/v1`.

Example:

```json
{
  "schema_version": 1,
  "kind": "static_analysis_orchestration_manifest",
  "name": "trusted pre-commit analyzer set",
  "profiles": [
    {
      "profile_id": "security",
      "path": "/opt/review/profiles/security.json",
      "sha256": "<64-lowercase-hex>"
    },
    {
      "profile_id": "types",
      "path": "/opt/review/profiles/types.json",
      "sha256": "<64-lowercase-hex>"
    }
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

Manifest rules:

- The manifest path supplied to the CLI must be absolute.
- The CLI requires the exact lowercase SHA256 of the manifest bytes.
- The manifest contains 1 to 16 ordered profiles.
- `profile_id` values are unique and match `^[a-z0-9][a-z0-9._-]{0,63}$`.
- Every profile path is absolute. Its location, including whether it resides inside the reviewed repository, confers no trust; only the exact pinned SHA256 authorizes its bytes.
- Repeating the same profile path and SHA256 is rejected to prevent accidental duplicate weighting.
- The same executable may appear in different profiles when the fixed arguments or rules differ.
- All manifests, profiles, and executables are loaded and verified before the first analyzer starts.
- If any profile uses `repository_configuration: explicitly-trusted`, the orchestration CLI also requires `--allow-repository-configuration`.
- The manifest does not weaken profile limits or trust declarations.
- Unknown fields fail closed.

Schema bounds:

- `max_execution_seconds`: 1 to 1800.
- `max_captured_output_bytes`: 1024 to 100000000.
- `max_findings`: 1 to 5000.
- `max_snapshot_bytes`: 1048576 to 2147483648.
- `max_snapshot_files`: 1 to 200000.

## Orchestration Request

The CLI constructs `OrchestrationRequest` from:

- repository root;
- source: `staged`, `unstaged`, or `branch`;
- opening authoritative scope fingerprint;
- absolute manifest path;
- expected manifest SHA256;
- repository-configuration authorization flag.

The request does not contain executable arguments or analyzer selection. Those facts come only from the authorized manifest and profiles.

## Data Flow

```text
manifest path + manifest SHA256 + expected scope
                         |
                         v
          verify manifest, profiles, executables
                         |
                         v
               open authoritative scope
                         |
                         v
             materialize candidate snapshot
                         |
                         v
              execute profiles in order
                         |
              +----------+----------+
              |                     |
              v                     v
       normalize evidence     update budget ledger
              |                     |
              +----------+----------+
                         |
                         v
              conservatively aggregate
                         |
                         v
       revalidate scope, repository, hashes
                         |
                         v
      orchestration artifact + aggregate evidence
```

No analyzer starts until the complete authorization set validates. The runner records repository state before snapshot construction and rechecks it before release.

## Shared Snapshot

One candidate snapshot is materialized for the orchestration:

- `staged` reads stage-zero index blobs through Git plumbing.
- `unstaged` captures tracked working-tree bytes.
- `branch` reads `HEAD` tree blobs.
- Git metadata, untracked files, ignored files, hooks, checkout filters, and smudge filters are absent.
- Unsafe paths and escaping symlinks fail closed.
- Gitlinks are omitted and remain a separate review obligation.

The effective snapshot file and byte limits are the minimum of the manifest limits and every referenced profile limit. The artifact records one `snapshot_id`, content SHA256, file count, and byte count. Every accepted execution repeats the same snapshot identity.

The snapshot is made read-only before execution. Because this is not a hostile-code sandbox, the orchestrator hashes it before and after every analyzer. If a tool changes it, that tool is invalidated and no later analyzer runs against the compromised directory.

## Scheduling

Profiles run serially and strictly in manifest order. Phase three does not expose concurrency or dependency ordering.

Serial execution provides:

- deterministic resource accounting;
- stable artifact order;
- unambiguous failure attribution;
- predictable host pressure;
- a simple single-snapshot integrity check.

## Budget Accounting

Profiles retain their existing per-tool limits. The orchestration manifest adds cumulative limits.

### Time

`max_execution_seconds` counts cumulative analyzer process duration, not preflight validation or snapshot construction. Before starting a tool, its effective timeout is the smaller of its profile timeout and the remaining orchestration duration. If no positive duration remains, the tool and all remaining tools are marked `not-run/budget-exhausted`.

### Captured output

`max_captured_output_bytes` is a shared allowance across stdout and stderr for all analyzers. A tool still has its existing per-stream profile limit. Capture stops when either a per-stream limit or the remaining combined orchestration allowance is exceeded. Stored bytes, including the one-byte overflow sentinel where applicable, are deducted from the cumulative allowance.

### Findings

`max_findings` limits aggregate emitted findings after cross-tool grouping. All report counts remain recorded. Excess aggregate findings set `truncated: true`; the review cannot claim complete static-analysis disposition until the truncated evidence is expanded or recorded as a limitation.

### Snapshot

Snapshot limits are paid once, not once per analyzer. They are enforced before any tool starts.

## Result Contracts

The new top-level contract is `static_analysis_orchestration/v1`.

It records:

- authoritative scope;
- manifest name, SHA256, and compact manifest id;
- orchestration id;
- shared snapshot identity;
- overall status;
- initial, consumed, and remaining budgets;
- ordered run entries;
- nested authoritative `static_analysis_execution/v1` objects for valid started runs;
- linked report and aggregate finding ids;
- source provenance for grouped findings.

Run entries are an ordered union:

- `executed`: links an authoritative execution object;
- `not-run`: carries `budget-exhausted`;
- `invalidated`: carries `snapshot-mutated` and does not expose an authoritative execution object.

The orchestration id is the first 16 lowercase hexadecimal characters of a SHA256 over the scope fingerprint, manifest SHA256, snapshot SHA256, and the ordered terminal run identities and statuses, separated by NUL bytes.

The combined CLI output contains:

```text
## Static Analysis Orchestration JSON
<static_analysis_orchestration/v1>

## Static Analysis Evidence JSON
<aggregate static_analysis_evidence/v1>
```

The aggregate evidence object remains reducer-compatible. Each aggregate finding selects a deterministic primary source for its existing singular tool, rule, message, severity, and confidence fields, while `report_ids` links every corroborating report. The orchestration artifact contains `finding_sources`, keyed by aggregate finding id, to preserve every source tool, rule, execution id, report id, message, severity, and confidence without changing the evidence-v1 consumer interface.

## Overall Status

- `completed`: every manifest profile produced an accepted completed result.
- `partial`: at least one result was accepted and at least one profile failed, was invalidated, or was not run.
- `failed`: authorization and scope remained valid, but no analyzer result was accepted.

Authorization, manifest integrity, profile integrity, executable integrity, original-repository mutation, or final scope drift fails closed and emits no authoritative orchestration artifact.

## Tool-Local Failures

The orchestrator continues after:

- non-success exit;
- timeout;
- per-tool or cumulative output overflow;
- malformed SARIF or normalized JSON;
- tool-name or tool-version mismatch.

These runs emit linked failed or timeout evidence with no blocking candidates, as in phase two. They consume the resources already used and do not mark manifest review units as reviewed.

## Shared-Integrity Failures

- A temporary snapshot digest mismatch invalidates the current run and stops remaining runs. Earlier executions whose post-run snapshot checks passed may remain in a `partial` artifact if the original repository, authorization files, and final scope still validate.
- Original-repository state drift, manifest changes, profile changes, executable changes, or final scope drift invalidates the authorization basis for the complete artifact. No authoritative orchestration output is released.

## Conservative Finding Aggregation

Findings are grouped only when all of the following hold:

1. normalized repository paths are equal;
2. source ranges overlap or resolve to the same added line;
3. a reliable semantic identity matches.

Semantic identity is selected in this order:

1. explicit normalized `problem_key` plus `remediation_key` from `static_analysis_input/v2`;
2. a shared CWE or SARIF taxonomy identifier;
3. no match.

Category or message similarity alone never merges findings. When no reliable semantic identity exists, findings remain separate.

The deterministic primary source is selected by:

1. higher normalized severity;
2. higher normalized confidence;
3. lexicographically smaller tool name, rule id, and report id.

The aggregate severity and confidence fields come from that primary source. Corroboration is recorded separately and does not automatically raise severity, confidence, or verdict impact. Every blocking or priority candidate still requires the existing independent finding-verification process.

### Semantic input extension

The existing `static_analysis_input/v1` schema remains accepted without modification. Phase three adds `static_analysis_input/v2` as an optional additive input contract with these finding fields:

- `problem_key`: a producer-defined stable identifier for the underlying problem class;
- `remediation_key`: a producer-defined stable identifier for the required corrective action;
- `taxonomy_ids`: normalized taxonomy identifiers such as `CWE-79`.

The three fields are optional. Unknown or untrusted values do not affect severity or blocking rules; they are used only as conservative aggregation keys. SARIF producers derive `taxonomy_ids` from standard SARIF taxa and rule relationships. A v1 finding without a shared taxonomy remains independent, preserving backward compatibility and avoiding message-similarity heuristics.

## Migration Strategy

### Stage 1: Extract shared Rust library code

Move only the control-plane and state functions required by both binaries out of `main.rs`. Preserve current `collect-diff-context` behavior and golden parity.

### Stage 2: Implement Rust `collect` and `run`

Port phase-one report normalization and phase-two single execution into the Rust library. Existing Shell entrypoints select implementations through:

```text
PRE_COMMIT_REVIEW_STATIC_IMPL=rust|python|shadow
```

During parity development, `shadow` runs Rust and Python, compares normalized structured output, and returns the Python output. It is a diagnostic mode, not an automatic production fallback.

### Stage 3: Implement Rust `orchestrate`

Add the manifest, shared snapshot, serial scheduler, cumulative budget ledger, aggregate evidence, and orchestration artifact. No Python orchestration implementation is created.

### Stage 4: Switch the default

After deterministic parity gates pass, `rust` becomes the default for `collect` and `run`. Python remains explicitly selectable for one compatibility release and receives no new features. Rust failures do not automatically fall back to Python because doing so could bypass fail-closed behavior.

## Testing Strategy

The orchestration module interface is the primary test surface.

### Rust tests

- Manifest strictness, hash validation, duplicate rejection, ordering, and bounds.
- Stable profile, execution, snapshot, finding, and orchestration identifiers.
- Budget consumption and remaining-budget calculations with a deterministic test clock.
- Conservative aggregation, primary-source selection, and provenance retention.
- Serialization against all JSON schemas, including the additive normalized-input v2 schema.

### Real local integration tests

Use temporary Git repositories and fixture executables to test:

- staged, unstaged, and branch snapshot identity;
- one shared snapshot across all accepted executions;
- absence of Git metadata and untracked files;
- serial execution order;
- repository-configuration authorization;
- timeout, non-success exit, output overflow, invalid output, and tool mismatch;
- budget exhaustion and `not-run` entries;
- snapshot mutation and stopping later tools;
- repository, manifest, profile, executable, and scope drift;
- `completed`, `partial`, and `failed` artifacts;
- failed tools producing no blocking candidates;
- conservative cross-tool aggregation.

### Compatibility tests

- Existing Shell contract tests run against the Rust default.
- Python/Rust shadow fixtures compare normalized JSON while ignoring durations, process ids, and temporary paths.
- Existing single-tool execution and evidence schemas remain valid.
- Old Python implementation-specific tests are removed after equivalent behavior is exercised through the Rust module interface or retained only as one explicit legacy smoke test.

### Platform tests

CI runs Rust static-analysis smoke tests on Linux, macOS, and Windows. Process termination, temporary-directory permissions, executable resolution, and path normalization receive platform-specific assertions.

## Packaging and Release

Release assets add one static-analysis binary per supported platform:

```text
static_analysis-darwin-amd64
static_analysis-darwin-arm64
static_analysis-linux-amd64
static_analysis-windows-amd64.exe
```

Build and installation logic pins and validates these assets in the same manner as the current context binary. Shell wrappers prefer an explicitly configured trusted binary, then a local release build, then the bundled platform binary. They do not search ambient `PATH` for the static-analysis executable.

## Security Model

The Rust migration consolidates behavior but does not turn the runner into a hostile-code sandbox.

- Analyzers remain explicitly authorized and hash-pinned.
- Commands run without a shell.
- Child environments remain allowlisted.
- Original repository paths are not intentionally exposed.
- Network proxy poisoning remains a best-effort offline aid, not a kernel network restriction.
- Read-only permissions and digest checks detect ordinary snapshot mutation but do not stop a malicious same-user process from probing the host.
- Raw stderr remains excluded from review evidence; only bounded byte counts and SHA256 values are recorded.

## Completion Criteria

Phase three is complete when:

1. Rust is the default implementation for `collect`, `run`, and `orchestrate`.
2. Normal static-analysis runtime paths no longer require Python.
3. Existing single-report and single-run Shell and JSON contracts remain compatible; normalized input v2 is additive and v1 remains accepted.
4. One manifest authorizes all profiles and executables before execution.
5. Every accepted execution repeats the same snapshot identity.
6. Serial order and cumulative budgets are deterministic.
7. Tool-local failures continue; authorization and original-repository integrity failures fail closed.
8. Aggregate findings preserve source provenance and never merge without a reliable semantic identity.
9. Existing repository tests, Rust formatting, Clippy, schemas, parity, installer, release, and model-evaluation gates pass.
10. Linux, macOS, and Windows static-analysis smoke tests pass.

## Approved Decisions

- Multi-analyzer orchestration is the phase-three priority.
- Tool-local failures continue and produce `partial` results when other evidence succeeds.
- One absolute hash-pinned manifest authorizes an ordered set of hash-pinned profiles.
- Profiles execute serially against one shared snapshot identity.
- Manifest cumulative limits and per-profile limits both apply.
- Cross-tool findings use conservative semantic aggregation with full source provenance.
- The default implementation is Rust-first; Python is a temporary explicit compatibility implementation only.
