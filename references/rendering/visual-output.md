# Optional Visual Output Guidance

Use this file only when visual mode is justified. This file defines visual presentation options, not general review logic.

Use it when:

- the user explicitly asks for a visual or report-style review
- the change meaningfully affects UI, layout, styling, screenshots, or interaction states
- a structured visual summary materially improves the commit decision

Do not use it for ordinary small text-diff reviews.

## Principles

- Visual elements must improve one of four decisions: can I commit, what must I fix, what should I test, or what risk should I watch.
- Prefer compact status tables over numeric scores or decorative visuals.
- Use bullets for findings that need evidence, impact, and fix details.
- Do not invent precision. If risk, completeness, or coverage is unknown, write `Unknown` or `N/A`.
- Keep the single top-level `VERDICT` field from the main template.
- Follow the selected output language, except that `VERDICT` remains in English.

## Visual Skeleton Source

When visual mode is active, use the per-language Visual Review template from:

- `references/rendering/output-en.md`
- `references/rendering/output-zh.md`

This file supplements those templates with optional visual structures.

## Useful Visual Elements

### Summary Status Table

Use when a compact area-by-area scan improves decision speed.

```markdown
| Area | Signal | Evidence |
|---|---|---|
| Code quality | Needs attention | New helper duplicates existing validation path |
| Security | Needs attention | New endpoint lacks visible auth check |
| Tests | Unknown | No test diff provided |
| Regression risk | Medium | Validation behavior changed |
```

### Risk Matrix

Use only when there are two or more meaningful risks worth comparing.

```markdown
| # | Risk scenario | Severity | Likelihood | Mitigation |
|---|---|---|---|---|
| 1 | <specific risk> | High | Medium | <fix or test> |
```

### Change Distribution

Use only for large diffs where the breakdown itself helps classify the change shape.

Only calculate distribution from real reviewed counts. If exact counts are unavailable, use a status table instead of percentages or bars.

```markdown
Added     ████████████░░░░  65%  (11 files)
Modified  ████░░░░░░░░░░░░  25%  (4 files)
Deleted   ██░░░░░░░░░░░░░░  10%  (2 files)
```

Skip for small diffs where the `Change scale` line already communicates enough.

### Flow Diagram

Use only when the diff changes real control flow, data flow, or a public contract.

```text
[Entry point] -> [Domain/service layer] -> [Persistence or external dependency]
      |                         |
      v                         v
[Validation/error path]      [Response/output]
```

Skip diagrams for tiny diffs, comment-only changes, renames, lockfile churn, or simple tests.

## Avoid By Default

- Numeric health scores without defined evidence
- Decorative health bars for tiny diffs
- ASCII verdict boxes
- Repeated legends for verdict tokens
- Bilingual warnings when the review itself is monolingual
- Dense visual structures when a short text summary is clearer
