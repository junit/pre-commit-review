# `collect_diff_context.sh` — Full Capability Reference

This is the deep-integrator reference for the read-only helper at `scripts/collect_diff_context.sh`. Most users only need the summary in [README.md](../README.md); read this when you are building automation on top of the helper's structured output.

The helper is the source of truth for diff source, review boundaries, and snapshot identity. The review entrypoint never fetches, stages, resets, installs, or modifies files, and it never runs, rewrites, or skips tests. Pinned scanner download is limited to an explicit user-initiated install or release-staging operation handled by `install.sh` and `scripts/fetch_gitleaks.sh`, not an Agent-time fallback.

## Control Plane Gateway

The review workflow starts with `scripts/collect_diff_context.sh --control-plane`. This bounded gateway:

- emits a compact `--control-plane` JSON gateway with an authoritative full-scope content fingerprint, per-unit fingerprints, bounded units/groups, work order, and reusable command templates
- supports `--expect-scope <fingerprint>` on follow-up retrieval so stale group/path output fails closed
- disables external diff and textconv drivers for both fingerprints and emitted review bytes, keeping snapshot identity and inspected content semantically aligned
- is authoritative only when its collection-start and collection-end fingerprints match

## Diff Source Resolution

- detects whether the current directory is a Git repository
- prefers staged changes when present
- falls back to unstaged changes or branch-vs-base comparison
- reports diff stats, file lists, and status
- identifies truncation, path/content high-risk candidates, generated-like files, lock files, and top-churn files
- records rename, delete, binary, mode-only, and submodule pointer changes as manifest units

## Coverage-Led and Reducer Automation Output

For large or fragmented diffs, the helper emits structured sections so a reducer or subagent can review every unit without Markdown table parsing:

- Review Manifest and Review Groups for coverage-led commit-readiness workflows
- Review Plan JSON v2 for reducer-friendly automation, including an `impact_context/v1` retrieval reference with `coverage_credit: none`
- Split Suggestions for review groups that exceed the hard budget
- Split Unit Diff Preview blocks for hunk-level review
- Coverage Ledger Template with pending review units
- Group Review Result templates for reducer-ready group findings
- Reducer State Snapshot Template for long multi-step reviews
- Coverage Validation Checklist for reducer preflight
- Full Review Execution Plan with ordered split/review steps
- Group Review Work Packets for serial or delegated group review
- Reducer Finalization Template for final synthesis gates
- a fingerprint-bound `command_templates.impact_context` command for optional `impact_context/v1` retrieval through `scripts/collect_impact_context.sh`
- a suggested review queue for large or truncated diffs

The default report no longer emits `Dependency Summary`, `Semantic Context Queries`, or `Test Selection Hints`. The separate fast impact-context collector parses complete changed Rust files with Tree-sitter, scans changed candidate files with the bounded text adapter, and returns normalized dependency, configured-query, framework, configuration, and test-selection summaries. It does not parse unrelated repository files, run builds, invoke the network, or grant review coverage.

## Safety Semantics

- omits the global raw diff from default output when it exceeds the inline budget, while keeping the structured plan visible
- truncates explicitly requested or inlined diffs safely when needed
- when a trusted scanner is available, scans and redacts the full selected diff before applying the output byte limit, so a detected credential crossing the truncation boundary cannot leak as an unmatched prefix; each replacement uses Gitleaks' 1-based scan-input byte coordinates
- computes scope/content fingerprints from original Git bytes; display redaction never changes snapshot identity
- captures Gitleaks `Match` values only inside the local sanitizer process to validate or recover byte spans; `Match` and `Secret` values are never serialized to helper stdout, stderr, reports, or model input by the helper
- rescans successfully redacted diff views before printing them; scanner failures degrade with `status: unavailable`, while a returned finding that cannot be mapped or verified degrades with `status: redaction-failed`; both continue with the original view instead of blocking review
- applies a bounded per-process deadline to version, capability, and content scans; timeout kills and reaps the scanner before review continues with `reason: scanner-timeout`
- sanitizes each split file view once before deriving its hunk previews, avoiding one scanner process per hunk while preserving line layout
- buffers and best-effort sanitizes the wrapper's complete stdout/stderr for Rust, legacy, fallback, and shadow modes
- emits `status: unavailable`, `status: redaction-failed`, or `status: disabled` with `redaction_applied: no` and `review_continued: yes` for their respective non-redacted paths; callers must not claim that such output was protected from secret exposure
- uses the skill-owned `references/security/gitleaks.toml`; repository `.gitleaks.toml`, `.gitleaksignore`, and `gitleaks:allow` cannot relax the scanner configuration
- accepts the default bundled scanner only when its executable SHA256 and version match the skill-owned manifests, then performs an empty-stdin JSON capability check before scanning content
- never searches `PATH` for a scanner; `PRE_COMMIT_REVIEW_GITLEAKS_BIN` is the only external scanner path, must be absolute, and represents explicit user trust while still requiring the pinned version and capability check
- `test-selection` domain summaries are read-only guidance for choosing focused verification commands and for distinguishing environment failures from code failures. A `no-known-env-heavy-marker` summary is not proof that a test is isolated; it only means the collector did not match a known environment-heavy marker.

When updating `scripts/gitleaks.version`, regenerate both `scripts/gitleaks-assets.sha256` from the upstream release archives and `scripts/gitleaks-binaries.sha256` from the corresponding extracted executables. Fetch, doctor, and release checks reject inconsistent artifacts; installer and runtime review degrade without redaction rather than becoming unavailable.

Reducer and subagent automation must use authoritative `Review Control Plane JSON` for scope. Review Plan/Manifest/Ledger sections are report views over that scope, while `impact_context/v1` is optional evidence with no coverage credit. Automation must not reconstruct scope from direct `git status` or `git diff --name-only` after the helper has emitted a manifest.

## Optional Static Analysis Evidence

`scripts/collect_static_evidence.sh` is a separate, opt-in evidence collector layered on top of the authoritative control plane. It accepts only explicitly supplied SARIF 2.1.0 or `static_analysis_input/v1` JSON files. It does not discover reports or run analyzers.

The wrapper resolves the Rust `static-analysis-cli` from an explicit absolute `PRE_COMMIT_REVIEW_STATIC_ANALYSIS_BIN`, a local release build, or the bundled `static_analysis-<platform>` asset, in that order. It invokes `static-analysis-cli collect` directly and never searches `PATH`.

The collector:

- requires the opening `scope_fingerprint` and fails closed on scope drift or report mismatch
- normalizes and deduplicates tool findings
- maps paths to authoritative manifest units and locations to added or unchanged lines
- classifies findings as blocking candidates, priority candidates, notes, or outside-scope evidence
- revalidates fingerprint, units, groups, and work order before emitting `static_analysis_evidence/v1`
- applies optional local secret sanitization to its machine-readable output

Static evidence feeds the existing candidate ledger and reducer finding merge, but never marks a manifest unit reviewed. See [static-analysis-evidence.md](static-analysis-evidence.md) for the protocol and command examples.

## Optional Controlled Static Analysis Execution

`scripts/run_static_analysis.sh` is the opt-in Phase 2 execution lane. It requires an explicitly supplied absolute `static_analysis_profile/v1` path and the exact SHA256 authorizing those profile bytes. It never discovers a profile, analyzer, package command, or result file.

This compatibility wrapper uses the same trusted Rust binary resolver and invokes `static-analysis-cli run` directly.

The runner:

- verifies the profile and absolute external executable hashes before execution and again before release
- rejects executables inside the reviewed repository and never searches `PATH`
- materializes staged index blobs, tracked unstaged files, or branch `HEAD` in a temporary snapshot without Git metadata or checkout filters
- rejects escaping symlinks and enforces snapshot file/byte limits before making the source tree read-only
- invokes the exact argument array without a shell, with an isolated home/temp area and allowlisted environment
- bounds runtime and stdout/stderr size, and emits only digests for raw stderr
- accepts only schema-valid SARIF or normalized JSON whose tool identity matches the profile
- links `static_analysis_execution/v1` to `static_analysis_evidence/v1` through scope, report ids, and `execution_id`
- reuses the Phase 1 mapping, reducer dispositions, final scope refresh, and optional secret sanitization

The network guard is best-effort environment isolation, not an operating-system sandbox. Profiles must require offline execution, and only known hash-pinned tools belong in this lane. See [static-analysis-execution.md](static-analysis-execution.md) for the authorization and threat model.

## Optional Static Analysis Orchestration

`scripts/orchestrate_static_analysis.sh` is the opt-in multi-analyzer lane. It requires an explicitly supplied absolute `static_analysis_orchestration_manifest` path and the exact SHA256 authorizing those manifest bytes. It never discovers manifests, profiles, analyzers, package commands, build targets, dependencies, or result files.

The compatibility wrapper resolves the same trusted Rust binary and invokes `static-analysis-cli orchestrate` directly. The orchestrator:

- preflights the manifest, every absolute profile, and every profile-pinned executable before any process starts
- materializes one bounded read-only tracked candidate snapshot shared by all profiles
- runs profiles serially in manifest order with cumulative execution, output, finding, file, and byte budgets
- records every profile as `executed`, `invalidated`, or `not-run`
- emits honest `completed`, `partial`, or `failed` status without treating timeouts or skipped profiles as clean
- unions accepted reports with execution-scoped ids while keeping cross-analyzer findings independent
- revalidates repository scope and state, manifest bytes, profiles, executables, and snapshot integrity before release
- applies optional local secret sanitization to the two-section orchestration/evidence output

The supported class is self-contained source-only offline analyzers. Build-coupled tools stay in trusted CI or another prepared environment and enter review as explicitly supplied precomputed evidence. Entrypoint hashes are exact authorization, not a complete arbitrary-analyzer dependency closure or operating-system sandbox. See [static-analysis-orchestration.md](static-analysis-orchestration.md) for the manifest, ASCII workflow, state tables, budgets, and review contract.
