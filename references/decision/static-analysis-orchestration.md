# Static Analysis Orchestration

Load this reference only when the user or trusted CI policy explicitly authorizes a multi-analyzer orchestration manifest.

## Authorization Gate

Require an explicitly supplied absolute `static_analysis_orchestration_manifest` schema-version-1 path, the exact lowercase SHA256 of those manifest bytes, and the opening authoritative scope fingerprint. Repository configuration still requires a separate `--allow-repository-configuration` decision when any authorized profile declares `repository_configuration: explicitly-trusted`.

Do not infer authority from a repository manifest, analyzer configuration, profile, executable, package script, build target, or prior run. Never discover or choose any of them on the user's behalf.

## Supported Analyzer Class

Supported analyzers are self-contained source-only offline tools that need no build, dependency installation, generated resources, daemon, or repository-owned executable configuration.

Route build-coupled tools through explicitly supplied precomputed evidence instead of orchestration.

The manifest pins every profile, and every profile pins one absolute external executable plus its fixed arguments. Entrypoint hashing is not a complete dependency or execution closure for an arbitrary native analyzer. Authorize only known tools whose undeclared runtime loading behavior remains inside the accepted trust boundary.

## Execution Workflow

1. Open the authoritative control plane and record source plus scope fingerprint.
2. Confirm the explicit absolute manifest path, its exact SHA256, and any separate repository-configuration trust decision.
3. Load and hash the exact manifest bytes once; preflight every profile and executable in manifest order before opening a snapshot or launching a process.
4. Resolve `scripts/orchestrate_static_analysis.sh` relative to the skill package containing `SKILL.md`.
5. Run:

   ```bash
   scripts/orchestrate_static_analysis.sh \
     --source <staged|unstaged|branch> \
     --expect-scope <scope_fingerprint> \
     --manifest <absolute-manifest-path> \
     --expect-manifest-sha256 <exact-manifest-sha256> \
     [--allow-repository-configuration]
   ```

6. Accept output only when the authoritative `static_analysis_orchestration/v1` and combined `static_analysis_evidence/v1` validate, share the opening scope, and expose matching report/finding id sets.
7. Revalidate the authoritative scope, manifest bytes, every profile, and every executable before accepting the final orchestration and combined evidence.

## Shared Snapshot And Budgets

All profiles run serially against one bounded read-only tracked-file snapshot. The snapshot excludes Git metadata, untracked dependencies, checkout filters, and gitlink contents. The effective snapshot limit is the strictest limit declared by the manifest or any profile.

Treat manifest time, captured-output, finding, snapshot-file, and snapshot-byte limits as cumulative. A profile may receive a lower effective runtime/output allowance as earlier profiles consume the shared budget. Preserve `not-run/budget-exhausted` instead of pretending the skipped profile completed.

If any profile mutates the shared snapshot, preserve that profile as `invalidated/snapshot-mutated` and every later profile as `not-run/shared-integrity-failure`. Do not accept evidence from the invalidated run.

## Status And Coverage Honesty

An orchestration with any failed, timed-out, invalidated, or not-run profile is `partial` unless no profile produced accepted evidence, in which case it is `failed`.

`completed` means every declared profile has `status: completed` and `result_accepted: true`. `partial` and `failed` are orchestration coverage states, not automatic review verdicts. Preserve every unavailable profile and the rules/scope it would have covered as a review limitation. Never describe a successful subset as complete or broad static-analysis coverage.

Each declared profile has one ordered terminal run entry:

- `executed` contains a full execution object, including unsuccessful execution states;
- `invalidated` contains only `snapshot-mutated`;
- `not-run` contains only `budget-exhausted` or `shared-integrity-failure`.

## Evidence Reduction

Only an `executed` profile whose execution is `completed` with `result_accepted: true` contributes usable reports. Failed, timed-out, output-limited, invalid-output, invalidated, and not-run profiles are unavailable verification; they are never clean results.

Findings from different executions remain independent candidates even when rule ids, locations, messages, or fingerprints match.

Execution-scoped ids prevent technical collisions but do not establish corroboration, raise confidence, change severity, or bypass independent finding verification. Every blocking or priority candidate must still be verified against the changed source, execution point, reachability, and impact. Static orchestration evidence never marks a review manifest unit as reviewed.

Analyzer normalized input remains `static_analysis_input/v1`. The combined output remains one orchestration artifact plus reducer-compatible `static_analysis_evidence/v1`.

## Final Checklist

Before citing orchestration evidence:

1. manifest path and exact SHA256 were explicitly authorized;
2. all profiles and executables passed complete preflight before execution;
3. no discovery, build, installation, or dependency preparation occurred;
4. only completed accepted reports supplied candidates;
5. all other terminal states remain visible limitations;
6. independent findings were not semantically collapsed;
7. final scope and authorization bytes were revalidated;
8. the review makes no coverage claim beyond the completed profiles' actual rules and source scope.
