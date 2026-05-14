---
name: pre-commit-review
description: |
  structured pre-commit review for git diffs before committing, pushing, or submitting code. use when the user asks to review staged changes, unstaged changes, a pasted diff, branch-vs-base changes, or commit readiness. triggers include "review before commit", "ready to commit", "pre-commit review", "check staged changes", "提交前审查", "提交前检查", "检查 staged 变更", or when the user is preparing to commit, push, or submit code and wants a quality gate.
---

# Pre-Commit Review

Review a Git diff as a commit-quality gate. Build a clear model of what changed, why it changed, what behavior shifted, what could break, and whether the commit is safe to make. This review does not replace CI or prove correctness; it catches obvious blockers, risky behavior changes, and missing verification before commit, push, or PR.

## Core Rules

- Do not pretend to inspect local changes. If no repository or shell access is available, review only the diff or code the user supplied and state that boundary in `Diff source` and `Review limits`.
- Prefer the exact user-provided diff when the user pasted or uploaded one. Do not replace it with local Git output unless the user asks.
- When repository access is available, prefer running `scripts/collect_diff_context.sh` from this skill before composing manual Git commands. Use its output as the source of truth for `Diff source`, `Review limits`, `Files changed`, changed files, staged/unstaged notes, untracked-file notes, and review boundaries. For very large diffs, use `PRE_COMMIT_REVIEW_MAX_DIFF_BYTES=0` only when printing the full diff is safe.
- Do not run `git fetch`, modify files, stage files, commit, push, or change branches unless the user explicitly asks.
- If the diff is too large to review completely, perform a prioritized partial review and clearly label it. Do not say the commit is fully safe when material changes were not reviewed.
- Remember that untracked files are not included in `git diff`; review them only when they are staged, provided by the user, or otherwise readable.
- When flagging secrets, credentials, tokens, connection strings, or private hosts, never reproduce the full value. Show the file, line if known, secret type, and a redacted preview only.
- Respond in the same language the user used. Keep the output template headings stable; translate only the analysis text.

## Input Resolution

Resolve the diff source in this priority order:

1. **User-provided diff**: Use pasted diffs, uploaded patch files, or code blocks directly.
2. **Staged changes**: If repository access exists, run `scripts/collect_diff_context.sh`. If it reports staged changes, review those as the commit candidate.
3. **Unstaged changes**: If no staged diff exists but unstaged changes exist, review the unstaged diff.
4. **Branch vs base**: If the working tree has no changes, review the current branch against the detected base branch, usually `origin/main` or `origin/master`.
5. **Untracked files**: `git diff` does not include untracked files; ask the user to stage them or provide their contents unless the helper output already includes a readable diff.
6. **No diff**: If no diff is available, say: `No diff available. Stage your changes or provide a diff to review.`

### Manual base branch detection

Use this only when the helper script is unavailable:

```bash
base=$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null | sed 's|^origin/||')

if [ -z "$base" ]; then
  if git rev-parse --verify --quiet origin/main >/dev/null; then
    base="main"
  elif git rev-parse --verify --quiet origin/master >/dev/null; then
    base="master"
  elif git rev-parse --verify --quiet main >/dev/null; then
    base="main"
  elif git rev-parse --verify --quiet master >/dev/null; then
    base="master"
  else
    base="main"
  fi
fi

echo "$base"
```

## Staged and Unstaged Mixed State

If staged changes exist, treat only the staged diff as the commit candidate. Still check whether unstaged changes exist:

- If unstaged changes touch different files, mention they were detected but not reviewed.
- If unstaged changes touch the same files as the staged commit candidate, mark this as a review limitation because tests and local runtime behavior may include uncommitted code that is not part of the commit.
- Do not merge staged and unstaged diffs into one review unless the user explicitly asks for all uncommitted changes.

## Depth Scaling

Scale detail to the size and risk of the diff:

- **Tiny diffs (<5 changed lines)**: Keep each dimension to 1-2 sentences. Use the shortest honest answer for empty sections, such as `Self-contained - no external impact` or `No logic change`.
- **Normal diffs**: Work through all six dimensions with concise but specific analysis.
- **Large diffs (50+ changed lines or 3+ files)**: Prioritize high-signal areas: security, auth, public APIs, migrations, data correctness, dependencies, config/deployment, concurrency, resource lifecycle, and changed tests. Still fill every output section, but group low-risk mechanical changes.
- **Too-large diffs**: Use the Large Diff Handling rules and mark `Review limits` as partial. Do not imply a full safety guarantee.

## Large Diff Handling

If the diff cannot fit in context or the helper reports truncation:

1. Start from diff stat, file list, and changed file types.
2. Prioritize security, auth, permissions, API/public interfaces, database migrations, payment/billing, data deletion, concurrency, async retry logic, configuration, deployment, dependency changes, and resource lifecycle changes.
3. Review generated, vendored, minified, and lock files only for source/config consistency, version changes, and suspicious major upgrades unless they are small and clearly relevant.
4. Use file-specific diffs when possible, such as `git diff -- path/to/file`, to inspect high-risk files deeply.
5. Set `Review limits` to partial and list any material unreviewed areas.
6. Avoid `PASS` for a partial review unless all omitted files are clearly generated or non-executable and no risky files were skipped.

## Review Workflow

Work through these dimensions in order. For every checkbox, replace `[ ]` with `[x]` and add concise, specific analysis. Name files, functions, APIs, migrations, tests, and line numbers when available.

### 1. What Changed

Account for every reviewed hunk or every reviewed file group.

- [ ] **Modified code**: Summarize each meaningful modified segment. If nothing was modified, write `None - pure additions.`
- [ ] **New code**: Summarize each meaningful new segment. If nothing was added, write `None - modifications only.`

For large diffs, group low-risk repeated changes, but do not hide important logic changes inside a broad summary.

### 2. Code Hygiene

Scan changed hunks for issues that are embarrassing, unsafe, or operationally risky to commit:

- [ ] **Dead code & imports**: Flag unused imports, variables, unreachable branches, commented-out code, debug prints/logs, stale TODO scaffolding, feature flags, or code made unused by this diff.
- [ ] **Security scan**: Check hardcoded secrets, tokens, credentials, private hosts, production endpoints, connection strings, auth coverage on new or changed entry points, input validation at trust boundaries, injection/XSS vectors, unsafe deserialization, insecure defaults, and information leakage in logs, errors, telemetry, or API responses. Redact sensitive values.
- [ ] **Consistency & conventions**: Compare adjacent code and project conventions. Check naming, return shapes, error handling, authorization checks, validation style, logging style, transaction patterns, framework best practices, and resource lifecycle patterns for clients, sessions, connections, files, locks, background tasks, and other infrastructure resources.
- [ ] **New code completeness**: For new functions/classes/files, check edge cases, null/empty/zero handling, validation, idempotency, error paths, domain requirements, timeout/retry behavior, cancellation, cleanup, and graceful degradation.

If clean, write `Clean - no hygiene issues found.` Do not pad this section.

### 3. Why It Changed

Infer intent from the diff, filenames, commit messages, tests, surrounding code, and user context. Try to infer before falling back to unclear, but do not invent unsupported intent.

- [ ] **Business intent**: State the likely user-facing or product outcome. If inferred, use cautious language such as `Likely...` or `Appears to...`.
- [ ] **Technical intent**: State the engineering goal, such as refactor, API contract change, performance, reliability, security, testability, dependency update, migration, or operational hardening.

Use `Intent unclear` only after checking available evidence.

### 4. Logic Shifts

Trace the core execution paths affected by the diff.

- [ ] **Before**: Describe the previous path, data flow, conditions, API shape, persistence behavior, or error semantics.
- [ ] **After**: Describe the new path at the same level of detail.
- [ ] **Delta**: State the precise behavior difference.

For formatting, renames, comments, or mechanical refactors, say `No logic change - cosmetic/refactor only` when supported. For pure additions, describe what did not exist before and assess whether the new code is correct and complete enough before callers depend on it.

### 5. Blast Radius

Identify who consumes or is affected by the changed code.

- [ ] **Upstream**: Callers, importers, clients, CLI users, APIs, scheduled jobs, tests, or services that call into this code.
- [ ] **Downstream**: Databases, queues, files, caches, external APIs, telemetry, network calls, generated artifacts, deployment systems, and data contracts this code touches.
- [ ] **Lateral**: Shared config, auth state, caches, feature flags, schemas, shared libraries, global state, or modules that can be affected indirectly.

If self-contained, say so and explain why.

### 6. Regression Risk

Be concrete. Prefer exact scenarios over generic warnings.

- [ ] **Risks**: Describe specific ways existing behavior could break, including compatibility, data correctness, runtime errors, race conditions, authorization gaps, and unexpected rejection or acceptance of inputs.
- [ ] **Test scope**: Group recommendations into existing automated tests to run, new tests to add, manual verification, and negative/edge cases when useful. Name specific files, commands, or scenarios if visible.
- [ ] **Watchpoints**: Name logs, metrics, dashboards, alert conditions, endpoint error rates, queue retries, migration status, or user behaviors to monitor after the commit lands.

## Conditional Risk Scans

Apply these only when relevant to the diff:

- **API/public interface changes**: Check backward compatibility, versioning, serialization, status codes, error messages, and downstream clients.
- **Database migrations/schema changes**: Check reversibility, locking, backfills, nullability, defaults, index creation, data loss, and rollout order.
- **Auth/security changes**: Check authorization boundaries, privilege escalation, authentication bypass, secret handling, audit logs, and permission defaults.
- **Async/concurrency changes**: Check idempotency, retries, ordering, cancellation, timeouts, race conditions, and duplicate processing.
- **Dependency or lockfile changes**: Check major versions, transitive risk, package manager consistency, runtime compatibility, and whether source changes match lockfile changes.
- **Config/env/deployment changes**: Check defaults, missing environment variables, local vs production differences, feature flag behavior, and rollback safety.
- **Infrastructure/resource lifecycle changes**: Check client/session/connection creation and reuse, timeouts, retries, cancellation, cleanup, context managers, file handles, transaction scope, locks, and graceful degradation against nearby code conventions.
- **Generated/vendor/minified files**: Check whether the generating source/config also changed. Flag generated output without a matching source change unless the reason is clear.
- **Observability changes**: Check whether failures remain diagnosable through logs, metrics, traces, alerts, and useful error messages.

## Verdict

Produce exactly one verdict:

```text
VERDICT: PASS | PASS_WITH_NOTES | NEEDS_WORK
```

### Decision guide

- **PASS**: No issues found within the reviewed scope. Safe to commit.
- **PASS_WITH_NOTES**: Safe to commit, but there are observations worth reading. Key test: if someone commits or deploys without reading the review, nothing breaks, nothing leaks, and no irreversible bad state is created.
- **NEEDS_WORK**: Do not commit until fixed. Key test: if someone commits or deploys without reading the review, something can break, leak, corrupt data, bypass security, or create an irreversible bad state.

Use `NEEDS_WORK` for blocking issues including:

1. Hardcoded secrets, credentials, private keys, production endpoints, or sensitive internal hosts in source.
2. Security vulnerabilities such as missing auth checks, injection/XSS paths, unsafe input handling, unsafe deserialization, or information leakage.
3. Functional bugs in code that will be called, including unhandled runtime exceptions and incorrect core logic.
4. Breaking API, schema, contract, or behavior changes without caller protection, compatibility handling, migration, or rollout safety.
5. Data loss/corruption risk, unsafe migrations, unsafe retries, non-idempotent duplicate processing, or irreversible side effects.

**New code with zero callers:** bugs in core logic, security-sensitive implementation, data handling, migrations, or operational infrastructure still mean `NEEDS_WORK`; style, readability, naming, or non-blocking completeness issues usually mean `PASS_WITH_NOTES`.

For `NEEDS_WORK`, list each blocking issue with file:line when available and what to fix. For `PASS_WITH_NOTES`, list non-blocking observations. Review limitations can justify `PASS_WITH_NOTES`; risky unreviewed areas can justify `NEEDS_WORK`.

## Output Format

Use this structure exactly. Keep each checkbox concise; one or two sentences is usually enough.

```markdown
# Pre-Commit Review

**Diff source:** <how the diff was obtained>
**Review limits:** <full diff reviewed | partial review with reason>
**Files changed:** <count> files, <insertions> insertions(+), <deletions> deletions(-)
**Unreviewed changes:** <none | unstaged/generated/too-large files or other limits>

## 1. What Changed
- [x] **Modified:** <summary or "None - pure additions.">
- [x] **New:** <summary or "None - modifications only.">

## 2. Code Hygiene
- [x] **Dead code & imports:** <issues or "Clean">
- [x] **Security scan:** <issues with redacted values or "Clean">
- [x] **Consistency & conventions:** <issues or "Clean">
- [x] **New code completeness:** <issues or "N/A - modifications only">

## 3. Why It Changed
- [x] **Business:** <intent, cautious inference, or "Intent unclear.">
- [x] **Technical:** <intent, cautious inference, or "Intent unclear.">

## 4. Logic Shifts
- [x] **Before:** <path description>
- [x] **After:** <path description>
- [x] **Delta:** <precise difference>

## 5. Blast Radius
- [x] **Upstream:** <impact analysis>
- [x] **Downstream:** <impact analysis>
- [x] **Lateral:** <impact analysis>

## 6. Regression Risk
- [x] **Risks:** <specific scenarios>
- [x] **Test scope:** <automated tests, new tests, manual checks, edge cases>
- [x] **Watchpoints:** <logs, metrics, dashboards, errors, or "None needed">

---
VERDICT: <PASS | PASS_WITH_NOTES | NEEDS_WORK>
[If NEEDS_WORK: numbered list of blocking issues]
[If PASS_WITH_NOTES: numbered list of observations]
```
