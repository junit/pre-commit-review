# Optional Visual Output Guidance

Use this file only when `SKILL.md` visual-mode criteria are met: the user asks for a visual/report-style review, the result will be shared with a team, or the diff already meets large/high-risk criteria and structured visual summaries materially improve the commit decision.

## Principles

- Visual elements must improve one of four decisions: can I commit, what must I fix, what should I test, what risk should I watch.
- Prefer compact status tables over health bars or numeric percentages.
- Use bullets for findings that need evidence, impact, and fix details.
- Do not invent precision. If coverage, risk, or completeness is unknown, write `Unknown` or `N/A`.
- In summary tables, include only rows with a concern, verification need, review limit, or meaningful risk signal. Do not add rows whose only message is clean/no issue.
- Keep the single verdict token from the main template. Do not add a second verdict box with a conflicting label.
- Follow the selected output language from `SKILL.md`.
- Keep the field label `VERDICT` exactly in English, even in localized visual reports.

## Visual Review Skeleton

When visual mode is justified, render the per-language visual skeleton: load `references/output-en.md` for English or `references/output-zh.md` for Chinese, per the Localization Rule. Each file holds the Visual Review Skeleton for that language alongside its Default and Tiny templates.

## Useful Visual Elements

### Summary Status Table

```markdown
| Area | Signal | Evidence |
|---|---|---|
| Code quality | Needs attention | New helper duplicates existing validation path |
| Security | Needs attention | New endpoint lacks visible auth check |
| Tests | Unknown | No test diff provided |
| Regression risk | Medium | Validation behavior changed |
```

### Risk Matrix

Use only when there are two or more meaningful risks.

```markdown
| # | Risk scenario | Severity | Likelihood | Mitigation |
|---|---|---|---|---|
| 1 | <specific risk> | High | Medium | <fix or test> |
```

### Change Distribution

Use only for large diffs (10+ files) where the breakdown adds signal about what kind of change this is.
Only calculate distribution from real `name-status`, `numstat`, or reviewed file counts. If exact counts are unavailable, use a status table instead of percentages or bars.

```markdown
Added     ████████████░░░░  65%  (11 files)
Modified  ████░░░░░░░░░░░░  25%  (4 files)
Deleted   ██░░░░░░░░░░░░░░  10%  (2 files)
```

Skip for small diffs where the Change scale line already communicates the shape clearly.

### Flow Diagram

Use only when the diff changes a real control flow, data flow, or public contract. Keep names generic unless the diff shows concrete components.

```text
[Entry point] -> [Domain/service layer] -> [Persistence or external dependency]
       |                         |
       v                         v
[Validation/error path]      [Response/output]
```

Skip diagrams for tiny diffs, renames, formatting, comment-only changes, dependency lockfile changes, and simple tests.

## Avoid by Default

- Numeric health scores without defined evidence.
- 10-character health bars for tiny diffs.
- ASCII verdict boxes.
- Long language label maps.
- Duplicated SAFE_TO_COMMIT/SAFE_TO_COMMIT_WITH_NOTES/DO_NOT_COMMIT legends in every output.
- Bilingual warnings when the user used only one language.
