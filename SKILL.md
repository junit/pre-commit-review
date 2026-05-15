---
name: pre-commit-review
description: |
  Use when the user wants a commit-readiness review of a staged diff, unstaged diff, pasted patch, branch-vs-base change, or other code intended for imminent commit, push, or submission. Triggers include "review before commit", "ready to commit", "pre-commit review", "check staged changes", "提交前审查", "提交前检查", and "检查 staged 变更". Avoid triggering for general code review, PR design review, debugging, architecture feedback, or single-function review unless the user explicitly frames it as commit readiness or diff review.
---

# Pre-Commit Review

Review a Git diff as a commit-quality gate. Build a clear model of what changed, why it changed, what behavior shifted, what could break, and whether the commit is safe to make. This review does not replace CI or prove correctness; it catches obvious blockers, risky behavior changes, and missing verification before commit, push, or PR.

## Core Rules

- Optimize for developer actionability, not visual completeness. Every output element must help the developer decide: can I commit, what must I fix, what should I test, or what risk should I watch.
- After the title, put the verdict first, then a one-sentence action summary in the selected output language. Example: `Conclusion: Safe to commit after running the updated auth test.` or `结论：先修复空值分支，再提交。`
- Do not pretend to inspect local changes. If no repository or shell access is available, review only the diff or code the user supplied and state that boundary in `Diff source` and `Review scope`.
- Prefer the exact user-provided diff when the user pasted or uploaded one. Do not replace it with local Git output unless the user asks.
- When repository access is available, prefer running the bundled helper script `scripts/collect_diff_context.sh` from this skill package before composing manual Git commands. Do not substitute a similarly named script from the target repository. Use the helper output as the source of truth for `Diff source`, `Review scope`, `Change scale`, changed files, staged/unstaged notes, untracked-file notes, and review boundaries. For very large diffs, use `PRE_COMMIT_REVIEW_MAX_DIFF_BYTES=0` only when printing the full diff is safe.
- Do not run `git fetch`, modify files, stage files, commit, push, or change branches unless the user explicitly asks.
- If the diff is too large to review completely, perform a prioritized partial review and clearly label it. Do not say the commit is fully safe when material changes were not reviewed.
- Remember that untracked files are not included in `git diff`; review them only when they are staged, provided by the user, or otherwise readable.
- When flagging secrets, credentials, tokens, connection strings, or private hosts, never reproduce the full value. Show the file, line if known, secret type, and a redacted preview only.
- Select the output language in this order: first, obey any explicit user request such as `use English` or `用中文输出`; otherwise, use the dominant language of the user's latest request. If the latest request is mainly Chinese, render the review in Chinese. If it is mainly English, render the review in English. Only ask for clarification when the request is genuinely mixed-language and no dominant language is clear. Preserve only the verdict tokens `SAFE_TO_COMMIT`, `SAFE_TO_COMMIT_WITH_NOTES`, and `DO_NOT_COMMIT` in English. For non-English output, translate headings, field labels, and connective prose naturally. Do not leave labels such as `Diff source`, `Review scope`, `Priority Findings`, or `Commit Guidance` in English unless the user asked for English output.

## Input Resolution

Resolve the diff source in this priority order:

1. **User-provided diff**: Use pasted diffs, uploaded patch files, or explicit change hunks directly.
2. **Staged changes**: If repository access exists, run `scripts/collect_diff_context.sh`. If it reports staged changes, review those as the commit candidate.
3. **Unstaged changes**: If no staged diff exists but unstaged changes exist, review the unstaged diff.
4. **Branch vs base**: If the working tree has no changes, review the current branch against the detected base branch, usually `origin/main` or `origin/master`.
5. **Untracked files**: `git diff` does not include untracked files; ask the user to stage them or provide their contents unless the helper output already includes a readable diff.
6. **No diff**: If no diff is available, say: `No diff available. Stage your changes or provide a diff to review.`

If the user provides code but no diff, perform a static pre-commit-style review. Set `Diff source` to `user-provided code`, set `Review scope` to `partial review - no before/after diff available`, and do not infer prior behavior unless the user showed it.

### Manual base branch detection

Use this only when the helper script is unavailable:

```bash
base=$(
  git symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null     | sed 's|^origin/||'
)

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

If the detected base ref does not exist or `git diff <base>...HEAD` fails, do not guess. Mark branch-vs-base review as unavailable and ask the user for a base branch or a diff.

## Staged and Unstaged Mixed State

If staged changes exist, treat only the staged diff as the commit candidate. Still check whether unstaged changes exist:

- If unstaged changes touch different files, mention they were detected but not reviewed.
- If unstaged changes touch the same files as the staged commit candidate, mark this as a review limitation because tests and local runtime behavior may include uncommitted code that is not part of the commit.
- Do not merge staged and unstaged diffs into one review unless the user explicitly asks for all uncommitted changes.

## Depth Scaling

Scale detail to the size and risk of the diff:

- **Tiny diffs (<5 changed lines, low risk, and no priority findings)**: MUST use the Tiny Diff format. Keep each dimension to the shortest honest answer, such as `Self-contained - no external impact` or `No logic change`.
- **Normal diffs**: Use the Default Developer Review format. This covers most changes under roughly 300 changed lines or fewer than about 10 changed files after excluding generated, vendored, minified, and lockfile-only files when no high-risk area is involved.
- **Large/high-risk diffs**: Prioritize high-signal areas when the change is roughly 300+ changed lines, spans 10+ changed files excluding generated, vendored, minified, and lockfile-only files, is generated/lockfile-heavy, or touches security, auth, public APIs, migrations, data correctness, dependencies, config/deployment, concurrency, payment/billing, data deletion, or resource lifecycle.
- **Too-large diffs**: Use the Large Diff Handling rules and mark `Review scope` as partial. Do not imply a full safety guarantee.

## Large Diff Handling

If the diff cannot fit in context or the helper reports truncation:

1. Start from diff stat, file list, and changed file types.
2. Prioritize security, auth, permissions, API/public interfaces, database migrations, payment/billing, data deletion, concurrency, async retry logic, configuration, deployment, dependency changes, and resource lifecycle changes.
3. Review generated, vendored, minified, and lock files only for source/config consistency, version changes, and suspicious major upgrades unless they are small and clearly relevant.
4. Use file-specific diffs matching the selected review source to inspect high-risk files deeply: staged reviews use `git diff --cached -- path/to/file`, unstaged reviews use `git diff -- path/to/file`, and branch-vs-base reviews use `git diff <base>...HEAD -- path/to/file`.
5. Set `Review scope` to partial and list any material unreviewed areas.
6. Avoid `SAFE_TO_COMMIT` for a partial review unless all omitted files are clearly generated or non-executable and no risky files were skipped.

## Review Workflow

Work through these dimensions in order. Name files, functions, APIs, migrations, tests, and line numbers when available. Keep findings actionable: evidence, impact, fix.

These checkpoints are internal review steps. Do not render them as literal checkboxes unless the user explicitly asks for an expanded checklist.

### 1. What Changed

Account for every reviewed hunk or every reviewed file group.

- **Modified code**: Summarize each meaningful modified segment. If nothing was modified, write `None - pure additions.`
- **New code**: Summarize each meaningful new segment. If nothing was added, write `None - modifications only.`

For large diffs, group low-risk repeated changes, but do not hide important logic changes inside a broad summary.

### 2. Code Hygiene

Scan changed hunks for issues that are embarrassing, unsafe, or operationally risky to commit:

- **Dead code & imports**: Flag unused imports, variables, unreachable branches, commented-out code, debug prints/logs, stale TODO scaffolding, stale feature flags, or code made unused by this diff.
- **Security scan**: Check hardcoded secrets, tokens, credentials, private hosts, production endpoints, connection strings, auth coverage on new or changed entry points, input validation at trust boundaries, injection/XSS vectors, unsafe deserialization, insecure defaults, and information leakage in logs, errors, telemetry, or API responses. Redact sensitive values.
- **Consistency & conventions**: Compare adjacent code and project conventions. Check naming, return shapes, error handling, authorization checks, validation style, logging style, transaction patterns, framework best practices, and resource lifecycle patterns for clients, sessions, connections, files, locks, background tasks, and other infrastructure resources.
- **New code completeness**: For new functions/classes/files, check edge cases, null/empty/zero handling, validation, idempotency, error paths, domain requirements, timeout/retry behavior, cancellation, cleanup, and graceful degradation.

If clean, write `Clean - no hygiene issues found.` Do not pad this section.

### 3. Why It Changed (Optional)

Infer intent from the diff, filenames, commit messages, tests, surrounding code, and user context. Use this only when intent is strongly inferable or meaningfully affects the review. Do not pad the output with low-confidence guesses.

- **Business intent**: State the likely user-facing or product outcome. If inferred, use cautious language such as `Likely...` or `Appears to...`.
- **Technical intent**: State the engineering goal, such as refactor, API contract change, performance, reliability, security, testability, dependency update, migration, or operational hardening.

If intent is weakly supported and not useful, omit this section entirely instead of writing filler.

### 4. Logic Shifts

Trace the core execution paths affected by the diff.

- **Before**: Describe the previous path, data flow, conditions, API shape, persistence behavior, or error semantics.
- **After**: Describe the new path at the same level of detail.
- **Delta**: State the precise behavior difference.

For formatting, renames, comments, or mechanical refactors, say `No logic change - cosmetic/refactor only` when supported. For pure additions, describe what did not exist before and assess whether the new code is correct and complete enough before callers depend on it.

### 5. Blast Radius

Identify who consumes or is affected by the changed code.

- **Upstream**: Callers, importers, clients, CLI users, APIs, scheduled jobs, tests, or services that call into this code.
- **Downstream**: Databases, queues, files, caches, external APIs, telemetry, network calls, generated artifacts, deployment systems, and data contracts this code touches.
- **Lateral**: Shared config, auth state, caches, feature flags, schemas, shared libraries, global state, or modules that can be affected indirectly.

If self-contained, say so and explain why.

### 6. Regression Risk

Be concrete. Prefer exact scenarios over generic warnings.

- **Risks**: Describe specific ways existing behavior could break, including compatibility, data correctness, runtime errors, race conditions, authorization gaps, and unexpected rejection or acceptance of inputs.
- **Test scope**: Group recommendations into existing automated tests to run, new tests to add, manual verification, and negative/edge cases when useful. Name specific files, commands, or scenarios if visible.
- **Watchpoints**: Name logs, metrics, dashboards, alert conditions, endpoint error rates, queue retries, migration status, or user behaviors to monitor after the commit lands.

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

Produce exactly one verdict token: `SAFE_TO_COMMIT`, `SAFE_TO_COMMIT_WITH_NOTES`, or `DO_NOT_COMMIT`.

### Decision guide

- **SAFE_TO_COMMIT**: No issues found within the reviewed scope. Safe to commit now.
- **SAFE_TO_COMMIT_WITH_NOTES**: Safe to commit now, but there are non-blocking observations, verification notes, or review-scope limits worth reading. Key test: if someone commits or deploys without reading the review, nothing breaks, nothing leaks, and no irreversible bad state is created.
- **DO_NOT_COMMIT**: Do not commit until fixed. Key test: if someone commits or deploys without reading the review, something can break, leak, corrupt data, bypass security, or create an irreversible bad state.

Use `DO_NOT_COMMIT` for blocking issues including:

1. Hardcoded secrets, credentials, private keys, or sensitive internal hosts in source. Production endpoints count when they bypass environment configuration, expose sensitive infrastructure, appear unintentionally in test/dev code, or could send real traffic or data to production.
2. Security vulnerabilities such as missing auth checks, injection/XSS paths, unsafe input handling, unsafe deserialization, or information leakage.
3. Functional bugs in code that will be called, including unhandled runtime exceptions and incorrect core logic.
4. Breaking API, schema, contract, or behavior changes without caller protection, compatibility handling, migration, or rollout safety.
5. Data loss/corruption risk, unsafe migrations, unsafe retries, non-idempotent duplicate processing, or irreversible side effects.

**New code with zero callers:** bugs in core logic, security-sensitive implementation, data handling, migrations, or operational infrastructure still mean `DO_NOT_COMMIT`; style, readability, naming, or non-blocking completeness issues usually mean `SAFE_TO_COMMIT_WITH_NOTES`.

For `DO_NOT_COMMIT`, list each blocking issue with file:line when available and what to fix. For `SAFE_TO_COMMIT_WITH_NOTES`, list non-blocking observations. Review limitations can justify `SAFE_TO_COMMIT_WITH_NOTES`; risky unreviewed areas can justify `DO_NOT_COMMIT`.

## Output Format

Default to the compact developer review. Optimize for fast scanability: verdict, action, findings, then supporting detail. Use tables only for summaries and comparisons. Use bullet findings when evidence, impact, and fix details matter. Do not use numeric health scores, health bars, distribution charts, or ASCII verdict boxes by default.

Use exactly one verdict token in the entire review: the top-level `VERDICT`. Inside findings, use non-verdict labels such as `Decision impact: blocker` or `Decision impact: note`.

#### Non-Translatable Verdict Field

The field label `VERDICT` must remain exactly `VERDICT`. Do not translate it to `结论`, `裁定`, `状态`, `判定`, or any other language.

Always render the verdict line exactly in this shape:

```markdown
**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
```

The localized one-sentence action summary belongs on the next line, such as `**Conclusion:** ...` in English or `**结论：** ...` in Chinese.

#### Finding Icons

Use these icons to prefix Priority Findings for quick visual scanning. Do not use other emoji in the review output.

| Icon | Use for |
|------|---------|
| 🔒 | Security finding (secrets, injection, auth gaps) |
| ❌ | Blocking bug or functional error |
| ⚠️ | Non-blocking observation or convention issue |
| 🧪 | Test gap or verification needed |

#### Risk Severity Markers

In the Risk Summary, prefix the risk level with a severity marker:

| Marker | Use for |
|--------|---------|
| 🔴 | High regression risk |
| 🟡 | Medium regression risk |
| 🟢 | Low regression risk |

Example: `Regression risk: 🔴 High — validation bypass allows empty payload through the new endpoint.`

#### Partial Review Warning

For partial reviews, add this warning block immediately after the header, rendered only in the selected output language:

- English: `> ⚠️ **Partial review:** <reason>. Areas not reviewed are listed under Unreviewed changes.`
- Chinese: `> ⚠️ **部分审查:** <reason>. 未审查的部分列于"未审查变更"中。`

#### Localization Rule

The review must be fully monolingual in the selected output language. This means:

- **Section headings** (`Priority Findings`, `Commit Guidance`, `What Changed`, `Risk Summary`, `Supporting Analysis`, etc.) must be translated.
- **Field labels** (`Diff source`, `Review scope`, `Change scale`, `Unreviewed changes`, `Modified`, `New`, `Before commit`, `Suggested verification`, etc.) must be translated.
- **Finding sub-labels** (`Evidence`, `Impact`, `Fix`, `Decision impact`) must be translated.
- **Connective prose** (conclusion, descriptions, risk explanations) must be in the selected language.
- **Keep in English only:** the field label `VERDICT`, file paths, code identifiers, command lines, and the three verdict strings `SAFE_TO_COMMIT` / `SAFE_TO_COMMIT_WITH_NOTES` / `DO_NOT_COMMIT`.

Do not leave headings or labels in English when the rest of the review is in another language, except for the required `VERDICT` field. Mixed-language headings are a formatting violation.

For expanded localized examples, consult `references/output-examples.md` only when needed. Do not depend on it for the default path.

### Default Developer Review

Use the concrete template matching the selected output language.

#### English Default Developer Review

```markdown
# Pre-Commit Review

**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
**Conclusion:** <one sentence stating whether they can commit and what to do next>
**Diff source:** <how the diff was obtained>
**Review scope:** <full review | partial review with reason>
**Change scale:** <count> files, +<insertions> / -<deletions>
**Unreviewed changes:** <none | unstaged/generated/too-large files or other limits>

## Priority Findings

1. [🔒|❌|⚠️|🧪] `file:line` - <issue title>
   - Evidence: <what in the diff shows this>
   - Impact: <what can break, leak, corrupt, or confuse>
   - Fix: <specific next action>
   - Decision impact: <blocker | note>

If there are no blockers or notes, write `None`.

## Commit Guidance
- **Before commit:** <required fix, review note, or `None`>
- **Suggested verification:** <tests/manual checks to run next>

## What Changed
- **Modified:** <summary or `None - pure additions.`>
- **New:** <summary or `None - modifications only.`>

## Risk Summary
- **Logic shift:** <no logic change | concise delta>
- **Blast radius:** <self-contained | impacted callers/dependencies/systems>
- **Regression risk:** <🔴 High | 🟡 Medium | 🟢 Low> - <concrete reason>
- **Watchpoints:** <logs, metrics, dashboards, errors, or `None needed`>

```

#### Chinese Default Developer Review

```markdown
# 提交前审查

**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
**结论：** <一句话说明现在能否提交，以及下一步要做什么>
**差异来源：** <差异获取方式>
**审查范围：** <完整审查 | 部分审查及原因>
**变更规模：** <文件数> 个文件, +<新增行数> 行 / -<删除行数> 行
**未审查变更：** <无 | 未暂存变更、生成文件、过大文件或其他限制>

## 重点发现

1. [🔒|❌|⚠️|🧪] `file:line` - <问题标题>
   - 证据：<diff 中显示该问题的内容>
   - 影响：<可能破坏、泄露、损坏或误导什么>
   - 修复：<具体下一步>
   - 决定影响：<阻塞项 | 备注>

如果没有阻塞项或备注，写 `无`。

## 提交建议
- **提交前：** <必须修复的问题、审查备注，或 `无`>
- **建议验证：** <接下来要运行的测试或手动检查>

## 变更概览
- **修改：** <摘要，或 `无 - 纯新增。`>
- **新增：** <摘要，或 `无 - 仅修改。`>

## 风险摘要
- **逻辑变化：** <无逻辑变化 | 简要说明行为差异>
- **影响范围：** <自包含 | 受影响调用方/依赖/系统>
- **回归风险：** <🔴 高 | 🟡 中 | 🟢 低> - <具体原因>
- **监控点：** <日志、指标、仪表盘、错误，或 `无需额外监控`>

```

Do not force every supporting subsection into every review. The default review should feel like a concise decision memo, not a compliance form.

Append supporting analysis only when it adds decision value beyond the required sections. If needed, use the localized heading `Supporting Analysis` or `补充分析` and include only useful subsections such as code hygiene, intent, before/after detail, or additional test scope. Omit the section entirely for routine reviews.

### Tiny Diff Format

Use this for very small, low-risk diffs. Do not force the full template when it would reduce readability.

Apply the same localization rule from the Localization Rule section above: all headings, labels, and prose must be in the selected output language.

#### English Tiny Diff Review

```markdown
# Pre-Commit Review

**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
**Conclusion:** <one sentence>
**Diff source:** <source>
**Review scope:** <full | partial>
**Change scale:** <files and lines>

- **Change:** <one sentence>
- **Hygiene:** <clean or issue>
- **Logic:** <no logic change | exact delta>
- **Blast radius:** <self-contained | affected callers/dependencies>
- **Risk:** <🔴 High | 🟡 Medium | 🟢 Low> - <reason>
- **Test:** <minimal verification or required fix>
```

#### Chinese Tiny Diff Review

```markdown
# 提交前审查

**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
**结论：** <一句话结论>
**差异来源：** <差异获取方式>
**审查范围：** <完整审查 | 部分审查>
**变更规模：** <文件数> 个文件, +<新增行数> 行 / -<删除行数> 行

- **变更：** <一句话>
- **代码卫生：** <干净或具体问题>
- **逻辑：** <无逻辑变化 | 精确差异>
- **影响范围：** <自包含 | 受影响调用方/依赖>
- **风险：** <🔴 高 | 🟡 中 | 🟢 低> - <原因>
- **测试：** <最小验证或必须修复的问题>
```

### Full Visual Mode

Visual mode is justified only when the user asks for a visual report, the review is being shared with a team, or the diff already meets large/high-risk criteria and a matrix materially improves the commit decision. Large/high-risk means roughly 300+ changed lines, 10+ changed files excluding generated, vendored, minified, and lockfile-only files, generated/lockfile-heavy changes, or changes touching security, auth, public APIs, migrations, data correctness, dependencies, config/deployment, concurrency, payment/billing, data deletion, or resource lifecycle. Otherwise use the Tiny Diff or Default Developer Review format and do not mix visual tables into the default path. When visual mode is justified, consult `references/visual-output.md`.

Rules for visual mode:

- Prefer status labels over numeric scores. Use `N/A` or `Unknown` when evidence is insufficient.
- Never invent percentages, coverage numbers, components, or data-flow paths.
- Use diagrams only for real control-flow, data-flow, or API-contract changes visible in the diff or surrounding context.
- For partial reviews, render the warning only in the selected output language.
