# Rust Multi-Analyzer Orchestration Design

## Status

Approved revised design for phase three of static-analysis support.

Phase one ingests explicitly supplied SARIF or normalized JSON. Phase two runs one explicitly authorized analyzer entrypoint in a bounded candidate snapshot. Phase three adds deterministic orchestration for multiple analyzers and consolidates the product runtime in Rust.

## Goals

- Authorize an ordered analyzer set through one absolute orchestration-manifest path and its exact SHA256.
- Pin every referenced profile and entrypoint executable by exact SHA256 before any analyzer starts.
- Materialize one authoritative candidate snapshot and reuse its identity for every analyzer.
- Execute analyzers serially in manifest order.
- Apply both orchestration-wide cumulative limits and existing per-profile limits.
- Continue after tool-local failures and preserve accepted evidence from other tools.
- Preserve the existing single-analyzer CLI and JSON contracts.
- Make Rust the only product runtime for report collection, single execution, and orchestration.
- Remove the Python static-analysis implementation after internal parity and platform validation pass.
- Preserve compatibility at the Shell, JSON, and exit-semantics interfaces instead of exposing multiple product runtimes.
- Limit controlled orchestration to self-contained, source-only, offline analyzers that emit SARIF or normalized JSON on stdout.
- Preserve every tool finding independently in one reducer-compatible evidence object.

## Non-Goals

- Automatic analyzer, profile, plugin, package-script, or build-target discovery.
- Repository-owned orchestration manifests or profiles without an external exact-hash authorization.
- Parallel execution or dependency-graph scheduling.
- Kernel-level hostile-code sandboxing or a guaranteed network namespace.
- Automatic installation or downloading of analyzers.
- Treating tool success as review coverage or a clean result as proof that a change is safe.
- A public `rust|python|shadow` implementation selector or automatic Rust-to-Python fallback.
- Long-term maintenance of Python and Rust static-analysis implementations.
- Build-coupled or multi-stage analyzers that require generated files, dependency installation, Git metadata, mutable external rules, query packs, plugins, or additional executables.
- Claiming that one pinned entrypoint executable constitutes a complete execution closure for arbitrary analyzers.
- Cross-tool semantic grouping, corroboration-based severity changes, a rule-alias registry, or `static_analysis_input/v2`.
- Result caching, background execution, parallel scheduling, PR annotations, IDE integration, or central policy management.

## Why Rust

The authoritative diff control plane, scope fingerprints, reducer structures, and output sanitizer already live in the Rust crate. Python was a reasonable incremental choice for the optional phase-one and phase-two lanes because it allowed rapid SARIF/JSON and process-control work without changing the bundled Rust release chain.

That trade-off stops being attractive once orchestration becomes a core capability. Extending the Python implementation would maintain two security-relevant implementations of Git access, snapshot identity, process execution, and structured contracts. It would also preserve a Python runtime dependency for the most complex execution path.

Phase three therefore uses a Rust-only product design. The existing Python implementations remain temporarily in development and CI as parity references while `collect` and `run` are ported. They are not exposed as selectable product runtimes, receive no new behavior, and are deleted when the Rust parity and release-platform gates pass.

## Considered Approaches

### Repeatedly invoke the existing Python runner

This has the smallest initial diff, but every analyzer would rebuild its snapshot and reopen the control plane. It cannot guarantee one shared snapshot identity and makes cumulative budget enforcement approximate. Rejected.

### Add a Python orchestration package

This can share a snapshot after refactoring the Python runner, but it deepens the split between the Rust control plane and the Python static-analysis runtime. It also leaves deployment and behavior-parity costs in place. Rejected.

### Extract a Rust library and add a Rust static-analysis binary

This is the selected approach. It concentrates scope, snapshot, execution, evidence, and orchestration behavior in one Rust implementation while preserving Shell entrypoints and JSON contracts. Migration comparison remains an internal test mechanism rather than a public compatibility interface.

### Switch directly to Rust without parity comparison

This has the smallest migration surface but weakens confidence in compatibility across existing evidence and execution edge cases. Rejected. Internal parity fixtures are retained until the Rust implementation passes the existing behavior gates.

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
│ static_analysis::evidence_union                           │
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
    ├── evidence_union.rs
    └── orchestration.rs
```

The orchestration module is a deep module. Its primary interface is:

```rust
pub fn execute(
    request: OrchestrationRequest,
) -> Result<OrchestrationArtifact, OrchestrationError>;
```

Callers and orchestration-level tests use this interface. Manifest parsing, entrypoint preflight authorization, Git plumbing, snapshot construction, process supervision, budget accounting, and evidence union remain implementation details.

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

There is no public static-analysis implementation selector. During Delivery A, parity tests invoke the Python files and Rust binary directly. Before the Rust cutover, the existing Shell wrappers continue to invoke Python; at cutover they switch directly to Rust and the Python static-analysis files are removed. Rust failures never fall back to Python.

## Supported Analyzer Class

Authoritative controlled orchestration supports analyzers with all of these properties:

- one explicitly authorized entrypoint executable;
- source-only analysis against the provided tracked-file snapshot;
- no build, dependency installation, generated-file preparation, or Git metadata requirement;
- no mutable external rule files, plugins, query packs, interpreters, or additional executables required for the authorized behavior;
- no network requirement;
- SARIF 2.1.0 or normalized JSON emitted on stdout;
- bounded operation under the existing process, output, snapshot, and environment controls.

This is a support contract and operator trust requirement, not a claim that arbitrary process dependencies can be discovered and hashed generically. The artifact records entrypoint authorization, not a complete execution closure. Complex analyzers such as build-coupled CodeQL workflows remain supported through the explicit precomputed SARIF/JSON evidence lane.

Profiles that depend on repository configuration may still use the existing `repository_configuration: explicitly-trusted` gate, but orchestration does not make mutable repository configuration part of a cryptographically closed analyzer bundle. Such profiles are outside the self-contained class unless their effective configuration is already contained in the tracked candidate and explicitly accepted by policy.

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
- All manifests, profiles, and entrypoint executables are loaded and verified before the first analyzer starts.
- If any profile uses `repository_configuration: explicitly-trusted`, the orchestration CLI also requires `--allow-repository-configuration`.
- The manifest does not weaken profile limits or trust declarations.
- Unknown fields fail closed.
- Preflight proves the authorized manifest, profile, and entrypoint bytes. It does not claim to discover or pin an analyzer's undeclared process, runtime, plugin, rule, or build dependencies.

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
      verify manifest, profiles, entrypoint executables
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
             union evidence without merging
                         |
                         v
       revalidate scope, repository, hashes
                         |
                         v
       orchestration artifact + combined evidence
```

No analyzer starts until the declared manifest, profile, and entrypoint authorization set validates. The runner records repository state before snapshot construction and rechecks it before release.

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

`max_findings` limits the total independently emitted findings in the combined evidence object. Findings are ordered by manifest profile order and the existing deterministic per-report finding order; no cross-tool grouping occurs. All report and input counts remain recorded. Excess findings set `truncated: true`; the review cannot claim complete static-analysis disposition until the truncated evidence is expanded or recorded as a limitation.

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
- linked report ids and independently emitted finding ids.

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
<combined static_analysis_evidence/v1>
```

The combined evidence object remains reducer-compatible. It contains every accepted or failed report in manifest order. Findings remain independent across tools and retain their existing tool, rule, message, severity, confidence, report id, and execution id provenance. Identical paths, lines, messages, rules, CWE values, or categories do not cause cross-tool merging in phase three.

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

## Evidence Union

Phase three unions report and finding records without semantic cross-tool aggregation:

1. reports are ordered by manifest profile order;
2. findings retain the deterministic order already produced for each report;
3. report ids and execution ids preserve exact source provenance;
4. no duplicate weighting or corroboration rule changes severity, confidence, disposition, or verdict impact;
5. every blocking or priority candidate still passes the existing independent finding-verification process.

The existing `static_analysis_input/v1` and `static_analysis_evidence/v1` contracts remain sufficient. A future semantic grouping contract requires representative duplicate datasets, producer support, and a separate approved design.

## Delivery Strategy

Phase three is split into two consecutive deliveries so migration risk and new orchestration behavior are not debugged simultaneously.

### Delivery A: Rust consolidation

1. Extract only the control-plane and state functions required by both binaries from `main.rs`, preserving current `collect-diff-context` behavior and golden parity.
2. Port phase-one report normalization and phase-two single execution into the Rust library and `static-analysis-cli`.
3. Keep existing Shell wrappers and JSON contracts unchanged while development and CI invoke Python and Rust directly against normalized parity fixtures.
4. Run schema, behavior, installer, release, and Linux/macOS/Windows gates against Rust.
5. At cutover, switch the Shell wrappers directly to Rust and delete `collect_static_evidence.py`, `run_static_analysis.py`, and product tests that exist only to select the Python implementation. Retain language-neutral golden fixtures where useful.

Delivery A contains no orchestration feature and exposes no public runtime selector or fallback.

### Delivery B: Rust orchestration MVP

After Delivery A is accepted, add the supported analyzer class, manifest, shared snapshot, serial scheduler, cumulative budget ledger, independent evidence union, and orchestration artifact. No Python orchestration implementation or semantic cross-tool grouping is created.

## Testing Strategy

The orchestration module interface is the primary test surface.

### Rust tests

- Manifest strictness, hash validation, duplicate rejection, ordering, and bounds.
- Stable profile, execution, snapshot, finding, and orchestration identifiers.
- Budget consumption and remaining-budget calculations with a deterministic test clock.
- Independent evidence union, deterministic ordering, truncation, and provenance retention.
- Serialization against the existing normalized-input and evidence schemas plus the new orchestration schema.

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
- duplicate findings from different tools remaining independent.

### Compatibility tests

- Existing Shell contract tests run against the Rust implementation.
- Internal Python/Rust parity fixtures compare normalized JSON while ignoring durations, process ids, and temporary paths before cutover.
- Existing single-tool execution and evidence schemas remain valid.
- Python implementation files and Python-runtime selection tests are removed at cutover after equivalent behavior is exercised through the Rust module interface.

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

The Rust migration consolidates behavior but does not turn the runner into a hostile-code sandbox or a generic hermetic package manager.

- Manifests, profiles, and analyzer entrypoint executables remain explicitly authorized and hash-pinned.
- External rules, plugins, interpreters, query packs, dynamic runtime assets, and build dependencies are not automatically discovered or pinned; analyzers that require them are outside the authoritative orchestration support class.
- Commands run without a shell.
- Child environments remain allowlisted.
- Original repository paths are not intentionally exposed.
- Network proxy poisoning remains a best-effort offline aid, not a kernel network restriction.
- Read-only permissions and digest checks detect ordinary snapshot mutation but do not stop a malicious same-user process from probing the host.
- Raw stderr remains excluded from review evidence; only bounded byte counts and SHA256 values are recorded.

## Completion Criteria

Phase three is complete when:

1. Rust is the only product implementation for `collect`, `run`, and `orchestrate`.
2. Normal static-analysis runtime paths no longer require the Python implementations, and no public static-analysis runtime selector or Python fallback exists.
3. Existing single-report and single-run Shell, JSON, and exit-semantics contracts remain compatible; `static_analysis_input/v1` remains the normalized input contract.
4. One manifest authorizes all profiles and entrypoint executables before execution, with artifact wording limited to entrypoint authorization rather than a complete execution closure.
5. Every accepted execution repeats the same snapshot identity.
6. Serial order and cumulative budgets are deterministic.
7. Tool-local failures continue; authorization and original-repository integrity failures fail closed.
8. Combined evidence preserves each tool finding independently with report and execution provenance.
9. Existing repository tests, Rust formatting, Clippy, schemas, parity, installer, release, and model-evaluation gates pass.
10. Linux, macOS, and Windows static-analysis smoke tests pass.
11. Documentation explicitly limits authoritative orchestration to the supported analyzer class and routes build-coupled or multi-stage analyzers through precomputed evidence.

## Approved Decisions

- Multi-analyzer orchestration is the phase-three priority.
- Tool-local failures continue and produce `partial` results when other evidence succeeds.
- One absolute hash-pinned manifest authorizes an ordered set of hash-pinned profiles.
- Profiles execute serially against one shared snapshot identity.
- Manifest cumulative limits and per-profile limits both apply.
- Cross-tool findings remain independent in phase three; semantic grouping and `static_analysis_input/v2` are deferred.
- The product runtime is Rust-only after Delivery A; Python exists only as an internal migration oracle and is deleted at cutover.
- Controlled orchestration supports self-contained, source-only, offline analyzers; complex build-coupled analyzers remain on the precomputed evidence path.
- Manifest, profile, and executable hashes authorize entrypoints and declared command bytes but do not claim a complete arbitrary-analyzer execution closure.
