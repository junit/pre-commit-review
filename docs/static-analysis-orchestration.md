# Static Analysis Orchestration

Static-analysis orchestration runs an explicitly authorized ordered set of analyzer profiles against one authoritative candidate snapshot. The product implementation is the Rust `static-analysis-cli orchestrate` subcommand. `scripts/orchestrate_static_analysis.sh` is the public Shell wrapper and applies the same optional output sanitization as the single-profile lane.

This lane is optional. It supplements the normal diff review and never marks review manifest units as reviewed.

## Supported Analyzer Boundary

The MVP supports self-contained, source-only, offline analyzers that can inspect the tracked candidate snapshot without a build, dependency installation, generated resources, a daemon, or repository-owned executable configuration. Each executable and invocation must already be represented by an authorized `static_analysis_profile/v1`.

Build-coupled analyzers such as compiler plugins, project type-checkers, dependency-aware linters, and tools that require generated resources belong in trusted CI or another prepared environment. Supply their completed SARIF 2.1.0 or `static_analysis_input/v1` output through `scripts/collect_static_evidence.sh`; do not add build or dependency preparation to the orchestration manifest.

Hashing the declared profile and executable entrypoint establishes exact entrypoint authorization. It is not a complete execution closure for an arbitrary native analyzer: a trusted binary could load undeclared libraries, resources, or host facilities. Use this lane only for known tools whose fixed invocation independently satisfies the offline and self-contained boundary.

## Authorization

Require all of the following before execution:

1. an absolute manifest path explicitly supplied by the user or trusted CI policy;
2. the exact lowercase SHA256 of those manifest bytes;
3. the opening authoritative scope fingerprint;
4. separate acceptance of repository configuration when any referenced profile uses `repository_configuration: explicitly-trusted`.

The orchestrator never discovers a manifest, profile, executable, analyzer configuration, plugin, package script, build target, or dependency preparation step. It preflights the complete declared profile set before opening the shared snapshot or launching a process.

## Manifest

The manifest is ordered and limited to 1 through 16 profiles. Every profile reference contains a stable `profile_id`, an absolute profile path, and the exact profile SHA256.

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
      "profile_id": "correctness",
      "path": "/opt/review/profiles/correctness.json",
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

The manifest schema is `collect-diff-context-cli/schemas/static-analysis-orchestration-manifest.schema.json`. Referenced profiles continue to use `static-analysis-profile.schema.json`.

## Workflow

```text
authoritative control plane
          |
          v
absolute manifest path + exact SHA256
          |
          v
preflight every profile and executable
          |
          v
one bounded read-only tracked-file snapshot
          |
          v
serial analyzer execution in manifest order
          |
          +------ timeout / output / finding / total budget ledger
          |
          v
independent execution-scoped evidence union
          |
          v
scope + manifest + profile + executable revalidation
          |
          v
orchestration JSON + static_analysis_evidence/v1
```

Open the ordinary control plane, record its selected source and fingerprint, and run:

```bash
scripts/orchestrate_static_analysis.sh \
  --source <staged|unstaged|branch> \
  --expect-scope <scope_fingerprint> \
  --manifest /absolute/trusted/orchestration-manifest.json \
  --expect-manifest-sha256 <64-lowercase-hex> \
  [--allow-repository-configuration]
```

`--allow-repository-configuration` is valid only when at least one authorized profile declares `repository_configuration: explicitly-trusted`. It does not weaken authorization for any other manifest, profile, or executable.

## One Shared Snapshot

All profiles inspect the same materialized tracked-file candidate. Staged mode reads index blobs, unstaged mode uses tracked working-tree files, and branch mode reads `HEAD`; Git metadata, untracked dependencies, checkout filters, and gitlink contents are absent. The effective snapshot limit is the strictest file and byte limit across the manifest and all profiles.

Profiles run serially in manifest order. The source tree remains read-only, while each process receives its own isolated runtime directories. If a profile mutates the shared snapshot, that run becomes `invalidated/snapshot-mutated`; every later profile becomes `not-run/shared-integrity-failure`. The mutated output is not accepted as evidence.

## Cumulative Budgets

The manifest owns cumulative limits across the complete orchestration:

| Budget | Accounting |
|---|---|
| `execution_millis` | Sum of elapsed analyzer process time; a profile's effective timeout is capped by the remaining total |
| `captured_output_bytes` | Sum of bounded stdout and stderr bytes across executed profiles |
| `findings` | Independent combined findings retained after execution-scoped id rewriting |
| `snapshot_files` | Files in the one shared snapshot |
| `snapshot_bytes` | Bytes in the one shared snapshot |

Each budget reports `initial`, `consumed`, and `remaining`. When no execution or output budget remains, the current or subsequent profile becomes `not-run/budget-exhausted`. Per-profile limits still apply and may be stricter than the remaining orchestration budget.

## Terminal States

| Orchestration status | Meaning |
|---|---|
| `completed` | Every manifest profile executed with `result_accepted: true` |
| `partial` | At least one profile produced accepted evidence and at least one profile failed, timed out, hit an output limit, emitted invalid output, was invalidated, or was not run |
| `failed` | No profile produced accepted evidence |

Every manifest profile has exactly one ordered run entry:

| `run_kind` | Payload |
|---|---|
| `executed` | Full `static_analysis_execution/v1`; its internal status can be `completed`, `failed`, `timeout`, `output-limit`, or `invalid-output` |
| `invalidated` | `reason: snapshot-mutated`; no execution evidence is accepted |
| `not-run` | `reason: budget-exhausted` or `shared-integrity-failure`; no execution object is present |

`partial` and `failed` are coverage facts, not automatic commit verdicts. Preserve every unavailable profile as a visible review limitation and do not convert a timeout or clean completed subset into a claim of broad static-analysis coverage.

## Evidence Union And Review Use

The wrapper emits exactly two machine-readable sections:

- `Static Analysis Orchestration JSON` contains authorization identity, shared scope and snapshot, budgets, ordered run entries, and the complete report/finding id sets;
- `Static Analysis Evidence JSON` contains one reducer-compatible `static_analysis_evidence/v1` union.

Report and finding ids are namespaced by execution so different analyzers remain independent even when their rule ids, locations, messages, or source fingerprints match. The union does not merge findings semantically, change analyzer severity/confidence, or create corroboration weighting.

Only reports from an executed profile with `status: completed` and `result_accepted: true` may supply candidates. Every candidate still passes the normal source-location, changed-line, reachability, impact, framework, and blocking verification gates. Failed, timed-out, output-limited, invalid, invalidated, and not-run profiles are unavailable verification.

Before releasing authoritative output, the orchestrator revalidates the repository scope, repository state, manifest bytes, every profile, every executable, and the shared snapshot integrity. Any drift fails closed and releases no authoritative orchestration/evidence pair.

