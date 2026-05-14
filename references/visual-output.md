# Optional Visual Output Guidance

Use this file only when the user asks for a visual/report-style review, the result will be shared with a team, or the diff is large/high-risk enough that structured visual summaries improve scanability.

## Principles

- Visual elements must improve one of four decisions: can I commit, what must I fix, what should I test, what risk should I watch.
- Prefer compact status tables over health bars or numeric percentages.
- Use bullets for findings that need evidence, impact, and fix details.
- Do not invent precision. If coverage, risk, or completeness is unknown, write `Unknown` or `N/A`.
- Keep the single verdict token from the main template. Do not add a second verdict box with a conflicting label.

## Useful Visual Elements

### Summary Status Table

```markdown
| Area | Status | Note |
|---|---|---|
| Code hygiene | Clean | Reviewed changed hunks only |
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
- Duplicated PASS/PASS_WITH_NOTES/NEEDS_WORK legends in every output.
- Bilingual warnings when the user used only one language.
