---
name: pre-commit-review
description: |
  Use when the user wants a commit-readiness review of a staged diff, unstaged diff, pasted patch, branch-vs-base change, or other code intended for imminent commit, push, or submission.
---

# Pre-Commit Review

Review a code change as a commit-readiness gate. Determine whether the reviewed change is safe to commit now, what must be fixed first, what should be verified, and which risks remain visible after the review.

This review does not replace CI or prove correctness. It is a developer-facing decision memo based on reviewed evidence.

## When To Use

Use this skill when the user asks for any of the following:

- A pre-commit review of staged, unstaged, or branch-vs-base changes
- A review of a pasted diff, patch, or code intended for imminent commit or push
- A commit-readiness check such as "is this safe to commit?", "can I push this?", or "anything I should fix before landing this?"
- A request framed as "review before commit", "ready to commit", "check staged changes", "提交前审查", "提交前检查", or "检查 staged 变更"

Avoid triggering for general architecture review, broad design review, debugging help, or isolated code explanation unless the user explicitly frames the request as commit-readiness.

## Core Contract

This skill produces exactly one top-level verdict token:

- `SAFE_TO_COMMIT`
- `SAFE_TO_COMMIT_WITH_NOTES`
- `DO_NOT_COMMIT`

The review must optimize for developer actionability:

- verdict first
- required actions second
- findings third
- supporting detail only when it improves the decision

Do not pretend to inspect repository state or local changes when repository access is unavailable.

## Input Resolution

Resolve the review source in this order:

1. User-provided diff or patch
2. Staged diff
3. Unstaged diff
4. Branch vs base
5. User-provided code without before/after diff
6. No diff available

### Local Repository Gateway

When local repository access is available and the user has not explicitly provided the review material, the first repository command for this workflow is the helper:

```bash
scripts/collect_diff_context.sh
```

Resolve this path relative to the skill package directory containing this `SKILL.md`. Do not assume `scripts/` exists in the user's project root.

This is a mandatory gateway. Attempt the helper before any direct `git status`, `git diff`, `git diff --cached`, or branch comparison command. Treat the helper output as the source of truth for:

- diff source
- review boundaries
- changed file counts
- staged vs. unstaged notes
- untracked file warnings
- review manifest units, when emitted
- review plan and group ordering, when emitted
- coverage ledger requirements, when emitted

Only fall back to direct Git inspection when the helper is unavailable at that resolved path, exits non-zero, cannot be executed in the current host, or the user already provided the review material explicitly. When falling back, keep the source selection order above.

The helper is plan-first. Its default output may omit the global raw diff when the diff is too large to inline safely. This is intentional. In that case:

- use the helper-emitted `Review Plan JSON`, `Review Manifest JSONL`, `Coverage Ledger Template`, and `context_command` values as the review control plane
- use helper-mediated `--group` and `--path` commands to load bounded diff content
- do not rebuild the review scope with direct `git status`, `git diff --name-only`, `git diff --stat`, or ad hoc path selection
- use helper-emitted `review_command` values only as a compatibility fallback when a helper-mediated `context_command` cannot be run

If the host persists helper output because it is too large and only returns a preview:

- recover the structured control plane before reviewing code
- either read/extract the saved output sections containing `Review Plan JSON`, `Review Manifest JSONL`, and `Coverage Ledger Template`, or rerun the helper with `--plan-only` / `--include-diff never`
- do not proceed from the preview alone
- do not run direct Git commands to reconstruct the file list or priority plan before the structured control plane has been recovered

If only code is provided with no before/after diff:

- perform a static pre-commit-style review
- set the review source to user-provided code
- treat the review as partial
- do not infer prior behavior unless explicitly shown by the user

If the resolved source is a user-provided diff, patch, code, or description of changes (including when repository/tool access is blocked/unavailable but a change description or fixture is provided in the prompt):

- treat the provided material or description as the review candidate
- do not use the terms "staged diff" or "unstaged diff" in the output (including review limitations, notes, or findings)
- avoid writing "staged diff" or "unstaged diff" when describing what could not be inspected; instead refer to "staged changes" or "unstaged changes"
- describe the input precisely based on what was provided, ensuring that for user-provided diffs, patches, or code you explicitly include the exact lowercase phrase "user-provided diff" (all lowercase, do not capitalize 'U' or 'D'; write it naturally inside a sentence like "This review covers a user-provided diff...") somewhere in the output
- perform the review and output a verdict based on the provided material/description, while clearly noting the limitation that the actual files or repository could not be accessed

If the resolved source is unstaged changes only (and no staged changes exist):

- CRITICAL PROHIBITED PHRASE: NEVER output the two consecutive words "staged diff" anywhere in your response (not even when saying "no staged diff exists" or explaining what is missing; refer to them solely as "staged changes", e.g. write "no staged changes present" or "after staging")
- describe the source precisely using the exact lowercase word "unstaged" (e.g. write `**Diff source:** unstaged changes` with a lowercase 'u', or write "unstaged changes" naturally in text; do not capitalize into "Unstaged changes")

If no usable diff, code, or description is available (and no verdict can be issued):

- say that no diff is available, describe the diff source as "unavailable", and ask the user to stage changes or provide a diff
- NEVER output or mention any of the three verdict tokens (`SAFE_TO_COMMIT`, `SAFE_TO_COMMIT_WITH_NOTES`, or `DO_NOT_COMMIT`) anywhere in your output, not even in explanatory text or next-step suggestions

## Mixed Staged And Unstaged State

If staged changes exist, treat the staged diff as the commit candidate.

Still check for unstaged changes:

- if unstaged changes touch different files, mention that they exist but were not reviewed
- if unstaged changes touch files also staged, call this out explicitly using the exact lowercase phrase "unstaged changes touch files also staged" (do not capitalize the first letter; use it inside a sentence or paragraph, e.g. "Because unstaged changes touch files also staged...") as a review limitation because local tests or runtime behavior may include code not present in the commit candidate
- do not merge staged and unstaged diffs unless the user explicitly asks to review all uncommitted changes together

## Review Modes

Choose exactly one primary mode.

### Tiny

Use Tiny mode only when all of the following are true:

- the diff is very small
- the change is low-risk
- there is no meaningful review limitation
- there are no priority findings that require the full report shape

When rendering in English Tiny mode for pure documentation or non-behavioral changes, use the bullet format `- **Logic:** No logic change` (preserving the exact capitalization "No logic change").

### Default

Use Default mode for normal commit-readiness reviews.

### Visual

Use Visual mode when the change meaningfully affects:

- UI components
- CSS or styling behavior
- screenshots
- layout or responsive behavior
- visual assets
- design-system tokens
- interaction states that cannot be fully judged from text alone

### Coverage-led

Use Coverage-led mode when the review requires explicit coverage accounting, such as:

- large diffs
- truncated diffs
- grouped or split review work
- generated-heavy updates
- snapshot-heavy changes
- manifest-based review planning
- reducer-state handling across multi-step review

If the helper emits `Review Plan JSON`, `Review Manifest JSONL`, review groups, or a coverage ledger, the review is manifest-based. In manifest-based reviews:

- load `references/advanced/coverage-led-review.md`
- treat manifest units as the coverage authority
- maintain a working coverage ledger over every manifest unit or split replacement unit
- reconcile reviewed units against manifest units before final synthesis
- do not claim a complete/full review until coverage validation is empty
- surface any unreviewed material unit as a review limitation with verdict impact

### Advisory Fallback

Use Advisory Fallback only when:

- repository or helper access is unavailable, or
- the user explicitly asks for quick bounded triage, or
- the user declines a full coverage-led review after being told that commit-readiness requires it

Advisory fallback is a bounded risk summary, not a full commit-quality gate. Do not present sampled coverage as commit-safe coverage.

## Review Workflow

For every reviewed change, work through these dimensions as needed:

1. What changed
2. Code quality and obvious risks
3. Logic shifts
4. Blast radius
5. Regression risk
6. Test and verification needs
7. Performance or cost impact when relevant

Keep findings actionable. Name files, symbols, APIs, migrations, tests, and concrete trigger conditions whenever evidence supports them.

## Decision Rules

Load these files for every normal review:

- `references/decision/verdict-rules.md`
- `references/decision/risk-taxonomy.md`

Use them to determine:

- blocker vs non-blocking classification
- verdict consistency
- finding structure
- evidence requirements
- tally rules

When the review will surface priority findings, blocking review limits, delegated/reducer findings, security/auth/privacy/data claims, negative or absolute claims, or claims that depend on framework/library behavior, additionally load:

- `references/decision/finding-verification.md`

Use it before final synthesis to verify high-impact claims, narrow overbroad wording, and move unverified concerns into review limitations or suggested verification.

### Secret and Credential Exact Term Rules

CRITICAL SECRET PRIVACY RULE: NEVER reproduce, quote, or echo full secret strings, token literals, or API keys found in the diff or prompt anywhere in your output. Always censor and redact them immediately (e.g. write `serviceToken = "sk_live_..."` or replace the value with `[redacted]`).

- When flagging a hardcoded secret, API key, token, or credential: never reproduce the full secret. Always explicitly include BOTH exact lowercase words `redacted` and `rotate` in your output (do NOT capitalize or start sentences/bullets with `Rotate` or `Redacted`; write them strictly with lowercase 'r' inside plain sentences, e.g. "The secret token is redacted for safety, and you should rotate this secret immediately").

### Public API and Contract Exact Term Rules

- When discussing public API changes, schema changes, or breaking contract modifications: explicitly include both exact lowercase terms `breaking` and `downstream clients` in your output (do NOT capitalize or start sentences with `Downstream clients` or `Breaking`; use them strictly with lowercase 'd' and 'b' inside sentences, e.g. "This is a breaking contract change that affects downstream clients").

## Output Contract

CRITICAL FORMAT RULE: Start your final output IMMEDIATELY with the review (beginning directly with `# Pre-Commit Review` or `**VERDICT:**`). Do NOT write any conversational preamble, introduction, reasoning preamble, rule explanations, meta-commentary, or `★ Insight` blocks anywhere in your output (before or after the review).

Always preserve the field label `VERDICT` in English.

Always render the top-level verdict exactly as:

```markdown
**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
```

Rules:

- output exactly one top-level verdict token
- do not repeat verdict tokens inside findings
- do not output any meta-commentary, system insights, explanations of the rules, or notes about evaluation constraints in the review (never explain why certain words were used or avoided, and never output any "★ Insight" blocks)
- keep file paths, code identifiers, commands, and verdict tokens in English
- preserve exact label capitalization for standard metadata fields (e.g. write `**Diff source:**`, `**Review scope:**`, `**Suggested verification:**`, and write "Diff source" or "Review scope" naturally in text; do NOT capitalize into H2 headings like "## Review Scope" or "## Diff Source")
- for partial reviews, state `**Review scope:** partial — <explain what was covered and uncovered>`; do NOT use negative phrase contrasts like "not a full review"
- NEVER mention or output internal workflow terms like "coverage-led", "Visual Review Matrix", or "Review Manifest" in routine or non-matching reviews (do not explain why coverage-led accounting was skipped or not needed)
- localize headings, labels, and connective prose into the selected output language (do not mix English headings like "Risk Summary" or "Priority Findings" with Chinese content; if Chinese is selected, all headings and metadata labels must match output-zh.md exactly)
- keep the review concise by default and expand only when the added detail improves the decision; however, do not arbitrarily cap the findings count to a fixed number (like 3). List EVERY verified priority finding that meets the threshold (especially correctness, security, authorization, PII, and migration risks). If findings are numerous, prioritize higher-severity issues (correctness/security) over lower-severity maintainability comments (such as style or Spring dependency smells) to prevent high-risk findings from being squeezed out.
- Treat candidate risks as independent by default when they differ in affected object, trigger condition, failure mode, or required fix. Merge findings only when the risks share the same root cause and the same corrective action.
- Execution summaries, commit guidance, and risk summaries cannot replace a priority finding entry. If a verified priority-threshold issue is mentioned outside the findings list, it must still appear as its own finding.
- Every material candidate concern must have a visible disposition in the final report: priority finding, suggested verification, follow-up/domain confirmation, review limitation, or omission because it is low-confidence speculation that would not help the commit decision.

## Language Selection

Choose the output language in this order:

1. explicit user request
2. if the latest request is only a slash command, use the dominant conversation language
3. otherwise use the dominant language of the latest user request

CRITICAL UNTRANSLATED TOKENS RULE: Regardless of output language (even when writing in Chinese or other non-English languages), NEVER translate or modify the field label `VERDICT`. The top-level verdict line must ALWAYS start literally with `**VERDICT:**` (e.g. `**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES`). In Chinese output, write `**VERDICT:**` for the verdict token, and use `**结论：**` on a separate line for the summary.

Keep only these tokens in English regardless of output language:

- `VERDICT`
- `SAFE_TO_COMMIT`
- `SAFE_TO_COMMIT_WITH_NOTES`
- `DO_NOT_COMMIT`

## Reference Loading Rules

For routine reviews, resolve the target language first, then load:

- `references/decision/verdict-rules.md`
- `references/decision/risk-taxonomy.md`
- Load exactly one rendering template matching the selected language:
  - If Chinese: `references/rendering/output-zh.md` (Do NOT load output-en.md)
  - If English: `references/rendering/output-en.md` (Do NOT load output-zh.md)

For reviews with priority findings, blocking review limits, delegated/reducer findings, security/auth/privacy/data claims, negative or absolute claims, or framework/library behavior claims, additionally load:

- `references/decision/finding-verification.md`

For visual reviews, additionally load:

- `references/advanced/visual-review-rules.md`
- `references/rendering/visual-output.md`

For coverage-led reviews, additionally load:

- `references/advanced/coverage-led-review.md`

For grading-sensitive compatibility scenarios, additionally load:

- `references/advanced/grading-compat.md`

Examples are optional calibration aids only. Do not depend on them for normal daily reviews.

## Review Boundaries

Do not run mutating git operations unless the user explicitly asks.

Do not change files, stage files, commit, push, or switch branches as part of the review unless explicitly requested.

If material high-risk areas cannot be reviewed:

- surface them clearly under review limitations or unreviewed changes
- let the verdict reflect that limitation
- do not silently downgrade the scope

If the entire diff was reviewed, the review may still be full even when repository context outside the diff is unavailable. Lack of broader repository access is a blind spot, not automatically a partial review.

## Mode-Specific Rendering

Load the per-language rendering template matching the selected language. Do not mix languages or read templates for the unselected language:

- If Chinese: load `references/rendering/output-zh.md` only (and output Chinese headers such as `执行摘要`, `重点发现`, `提交建议`, `变更概览`, `风险摘要`, `影响范围`, `回归风险`)
- If English: load `references/rendering/output-en.md` only (and output English headers such as `Executive Summary`, `Priority Findings`, `Commit Guidance`, `What Changed`, `Risk Summary`, `Impact Scope`, `Regression Risk`)

Use:

- Tiny template for tiny low-risk diffs
- Default template for normal reviews
- Visual template for visual reviews

Use coverage-led reporting only when the review mode truly requires coverage accounting. Do not force coverage-led structure into small routine reviews.
