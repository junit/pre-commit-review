# Coverage-Led Review Reference

Load this file when a commit-readiness review is large, truncated, group-based, split-required, delegated, or needs reducer-state handling.

## Core Contract

Use coverage-led review for commit-readiness whenever repository/helper access or a complete user-provided diff makes it possible to enumerate review units.

Coverage-led review requires a coverage ledger: every `Review Manifest` unit must appear in exactly one group review result before the final verdict can claim a full review.

Commit-readiness reviews are coverage-led by default: start from `Review Manifest JSONL` or `Review Manifest`, account for every review unit, and treat large or truncated diffs as a reason to split or retrieve context rather than sample or skip.

Risk classification controls review order and split strategy; it never authorizes omitting executable or material units from a commit-readiness review.

Helper candidates are not exhaustive; semantically scan the full file list, diff stat, and changed file types, then promote any ordinary-looking file to high risk when its role, imports, API surface, or changed content affects a trust boundary or irreversible behavior.

## Planning Inputs

- Prefer `Review Manifest JSONL` and `Review Groups JSONL` for reducer or subagent automation; keep TSV tables for human scanning only.
- Use `Review Plan JSON` as the reducer-friendly aggregate plan when present; it captures group order, required units, budget status, context commands, and coverage gates without parsing Markdown tables.
- Treat `Review Manifest`, `Review Groups`, `Split Suggestions`, `Coverage Ledger Template`, and `Full Review Execution Plan` as TSV tables; do not parse their rows by comma because paths and commands may contain commas.
- Treat `Dependency Summary` as TSV as well; file paths and dependency details may contain commas, so do not parse it as CSV.
- Use `Dependency Summary` as reducer input for changed imports, exports, signatures, and schema/config signals, but treat it as best-effort rather than complete static analysis.

## Work Order

1. Treat `Review Manifest` as the authoritative list of review units. If a file is too large for one context window, split it into hunk units before claiming full coverage.
2. Use `Review Groups` as the initial work plan. Review high-risk groups first, consistency groups next, and medium-risk groups last unless cross-file dependencies require a different order.
3. Use `Full Review Execution Plan` as the default work order: split `split-required` groups first, then review high-risk groups, consistency groups, and medium-risk groups unless dependency evidence requires reordering.
4. Review generated, vendored, minified, and lock files for source/config consistency, version changes, and suspicious major upgrades; they still need coverage entries even when summarized as a consistency group.
5. For large diffs, group low-risk repeated changes, but do not hide important logic changes inside a broad summary.

## Splitting

- If a `Review Groups` row has `budget_status` of `split-required`, split that group into smaller file or hunk units before reviewing it; do not mark it covered as a single group.
- Use `Split Suggestions` as the starting point for replacing an over-budget group with smaller file or hunk units in the coverage ledger.
- Start coverage-led review from the `Coverage Ledger Template`; leave units pending until a group result records the exact reviewed unit, and replace `needs-split` rows with `Split Suggestions` units before review.
- Use `Split Unit Diff Preview` for hunk-level review when present; if the preview is insufficient or truncated, fall back to the listed file-specific command and hunk header.
- Do not use `--group` to review a `split-required` group as one unit; replace it with `Split Suggestions` units first.

## Context Retrieval

- Use `Group Review Work Packets` as the handoff context for serial or delegated group review; each packet carries the group id, required units, review commands, and split guidance.
- Use the work packet `context_command` when a group or file needs fresh context after global diff truncation; it must return only the requested group or file diff without widening review scope.
- Prefer group-level `context_command` values with `--group <group_id>` for groups within the hard budget; use file-level `--path <path>` commands from the manifest only when a group needs narrower context or has been split.
- Every `context_command` must include `--source staged`, `--source unstaged`, or `--source branch` so follow-up context retrieval cannot switch diff sources when the working tree changes.
- The helper may read optional project-level risk hints from `.pre-commit-review/risk-paths` and `.pre-commit-review/risk-content`; each non-empty, non-comment line is an extended regular expression used only to promote matching files into high-risk ordering.
- The helper may read optional `.pre-commit-review/context-queries`; each non-empty, non-comment line is an extended regular expression executed only through bounded read-only `git grep` to provide surrounding semantic context, never as a shell command and never as a coverage substitute.
- Treat `Semantic Context Queries` as best-effort surrounding context for dependency and caller checks; it can promote follow-up inspection, but it cannot mark any manifest unit reviewed.

## Group Results

For each group, inspect the complete group diff with file-specific commands and record a compact group result.

File-specific commands must match the selected review source: staged reviews use `git diff --cached -- path/to/file`, unstaged reviews use `git diff -- path/to/file`, and branch-vs-base reviews use `git diff <base>...HEAD -- path/to/file`.

Use the helper-provided `Group Review Result Template` for every group result; keep `required_units` intact and fill `reviewed_units` only with units actually inspected.

```json
{
  "group_id": "high-risk-auth",
  "reviewed_units": ["file:auth/session.py"],
  "coverage": "full",
  "findings": [],
  "contract_changes": [],
  "dependencies_to_check": [],
  "tests_recommended": []
}
```

If subagents are available and the user has asked for delegated or parallel work, group reviews may run in parallel. Otherwise, review groups serially in the current thread and keep the same group result shape.

## Reducer State

Use `Reducer State Snapshot Template` as the compact persistent state for long reviews; carry it forward after every group result and update `reviewed_units`, `pending_units`, `needs_split_units`, `group_results`, `coverage_gaps`, `finding_merge`, `dependency_checks`, and `test_recommendations`.

Do not write reducer state into the repository unless the user explicitly asks for an artifact. In normal operation, persist the compact state in the conversation, handoff packet, or agent scratch state.

Before every reducer pass, reconcile the current reducer state against `Review Plan JSON` and `Coverage Ledger Template`; if the state is missing a manifest unit or contains an unknown unit, treat coverage validation as failed until corrected.

## Final Reduction

Run Coverage Validation before cross-file reduction: compute `manifest_units - reviewed_units`; any high-risk coverage gap makes the verdict `DO_NOT_COMMIT`.

Before merging findings, use `Coverage Validation Checklist` as reducer preflight; full review is forbidden until `manifest_units - reviewed_units` is empty and all `needs-split` units have replacement results.

Perform cross-file reduction only after coverage validation. Merge findings, de-duplicate repeated notes, inspect `contract_changes` and `dependencies_to_check`, and re-check API signatures, imports, shared types, migrations/config rollout order, and callers affected by changed behavior.

Use `Reducer Finalization Template` for the final synthesis; do not produce the top-level verdict until coverage validation, finding merge, dependency checks, and test recommendations are filled.

Set `Review scope` to `full review` only when coverage validation is empty. If any material unit remains unreviewed, use partial review wording and explain the coverage gap.

Unreviewed high-risk candidates make commit-readiness `DO_NOT_COMMIT`; advisory fallback must not present a commit-safe verdict.
