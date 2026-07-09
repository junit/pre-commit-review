# English Output Templates

Loaded when the review is rendered in English. This file defines only the concrete rendering skeletons. Decision rules, evidence rules, coverage-led workflow, visual review rules, grading compatibility, and examples live elsewhere.

## Default Developer Review

```markdown
# Pre-Commit Review

**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
**Conclusion:** <short, actionable 1-2 sentence decision summary: whether to commit + primary basis/highest risk + required before-commit action or suggested verification>
**Tally:** <N blockers · N non-blocking warnings · N test-gaps · N review-limits; write `None` if all four counts are 0>
**Diff source:** <source + exact scope: command/PR/commit range/user-pasted diff; specify staged/unstaged/base branch; write unknown if unknown>
**Review scope:** <full review / partial review + covered content + uncovered content + whether it affects the verdict>
**Change scale:** <count of files under text diff> files, +<insertions> / -<deletions>; <individually specify generated files/lockfiles/binary assets/migration files if any, omit if none>
**Risk level:** <🔴 High | 🟡 Medium | 🟢 Low> - <risk basis: change type + path importance + high-risk domains + testing sufficiency>
**Unreviewed changes:** <none | list items: object + reason unreviewed + potential hidden risk + whether it blocks commit/affects verdict>

## Executive Summary

<Adapt length to change scope: a one-liner is enough for trivial fixes; multi-domain large changes may use a sub-bullet list per area (e.g. **- Area 1:** details). Required content: change intent, major affected areas, whether blocking risks were found, and the most critical validation action before committing. When multiple independent areas are covered, do not run (1)(2)(3) together in a single paragraph; use Markdown list items or bold area labels instead. Do not repeat technical details already present in "Priority Findings" and "What Changed" (Note: this brevity constraint applies only to the Executive Summary and must never be used to reduce the depth or completeness of "Priority Findings" and "What Changed").>

## Priority Findings

1. [🔒|❌|⚠️|🧪|👁️|📈|🧭] <file:line-line | file:function | file:config_key | file | cross-file contract> - <actionable issue title>
   - Evidence: <shortest direct evidence>
   - Impact: <affected object + trigger conditions + failure mode + worst consequence>
   - Fix: <code-level modification direction + boundary conditions + preferred solution>
   - Verification: <minimal verification loop: specific command/test name/manual path + scenarios covered + expected result>
   - Confidence: <High | Medium | Low> - <only explain when not high-confidence>
   - Blocking reason: <include only for blockers>

If there are no priority findings, write:

None.

## Commit Guidance

- **Required before commit:** <only blockers; write `None` if none>
- **Suggested before commit:** <non-blocking suggestions worth addressing in this commit; write `None` if none>
- **Follow-up items:** <maintenance, docs, cleanup items that do not affect this commit; write `None` if none>
- **Suggested verification:** <minimal verification loops in priority order: command/test name/manual path + scenarios covered + expected result; explain alternatives if unable to run>
- **Suggested documentation:** <PR description, reason for change, rollback plan, migration instructions, screenshots, monitoring notes; write `None` if none>

## What Changed

- **Modified:** <summarize behavior changes by responsibility domain: business logic/API/UI/config/test/doc/dependency>
- **New:** <new capabilities, entry points, tests, configs, migrations, dependencies and their purpose; write `None - modifications only.` if none>
- **Deleted:** <deleted behavior, compatibility layers, configs, tests, or assets; write `None.` if none>
- **Behavioral changes:** <no user/API/data/permissions/performance/deployment behavior changes | clearly explain before, after, affected objects, and trigger conditions>

## Risk Summary

| Dimension | Conclusion | Basis |
|---|---|---|
| Correctness | <Pass / Risky / Under-verified> | <brief basis> |
| Security & Privacy | <No obvious risk / Risky / Under-verified> | <brief basis> |
| Data & Migration | <Not applicable / Risky / Under-verified> | <brief basis> |
| Performance & Scalability | <No obvious risk / Risky / Under-verified> | <brief basis> |
| Compatibility | <No breakage / Risky / Under-verified> | <brief basis> |
| Observability & Rollback | <Sufficient / Insufficient / Not applicable> | <brief basis> |
| Test Coverage | <Sufficient / Gaps / Not run> | <brief basis> |

## Impact Scope

- **Direct impact:** <modules, entry points, user paths, services, or tasks directly reached by changes>
- **Indirect impact:** <potentially affected callers, dependencies, consumers, data pipelines, deployment pipelines; write `No obvious indirect impact.` if none>
- **Domain confirmation needed:** <none | domain + question to confirm + blocking/non-blocking>

## Regression Risk

**Level:** <🔴 High | 🟡 Medium | 🟢 Low>
**Reason:** <specifically explain why it is classified at this level>
**Minimal verification loop:** <minimal tests or manual steps required to mitigate risk>
```

## Tiny Diff Review

```markdown
# Pre-Commit Review

**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
**Conclusion:** <short, actionable 1-2 sentence decision summary>
**Diff source:** <source + exact scope: command/PR/commit range/user-pasted diff; specify staged/unstaged/base branch>
**Review scope:** <full review / partial review + covered content + uncovered content + whether it affects the verdict>
**Change scale:** <count of files under text diff> files, +<insertions> / -<deletions>; <specify generated files/lockfiles/binary assets/migration files if relevant>
**Tiny Diff applicability:** <applicable | not applicable but downgraded to summary | not applicable, should use default template>

- **Change:** <what changed and why the risk stays low or not>
- **Logic:** <No logic change and why | changes: before/after/trigger/affected objects>
- **Impact scope:** <direct impact + indirect impact + confirmed unaffected boundaries>
- **Risk:** <🔴 High | 🟡 Medium | 🟢 Low> - <risk basis>
- **Suggested verification:** <covered/uncovered + minimal suggested verification + whether it affects commit decision>
- **Before commit:** <None / specify required action>
```

## Visual Review

```markdown
# Pre-Commit Review

> ⚠️ **Partial review:** <only when visual/interaction/assets cannot be fully reviewed; explain reason>. Areas not reviewed are listed under Unreviewed changes.

**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
**Conclusion:** <short, actionable 1-2 sentence decision summary>
**Diff source:** <source + exact scope: command/PR/commit range/user-pasted diff; specify staged/unstaged/base branch>
**Review scope:** <full review / partial review + covered content + uncovered content + whether it affects the verdict>
**Change scale:** <count of files under text diff> files, +<insertions> / -<deletions>; <specify generated files/lockfiles/binary assets/migration files if relevant>
**Visual scope:** <reviewed pages/components + states + viewport + theme + language + browser/platform>
**Unreviewed changes:** <none | list items: object + reason unreviewed + potential hidden risk + whether it blocks commit/affects verdict>

## Visual Review Matrix

| Area | Signal | Evidence | Conclusion |
|---|---|---|---|
| Design consistency | <Pass/Issue/Under-verified> | <design tokens, spacing, colors, component rules> | <impact and actions> |
| Layout & responsive | <Pass/Issue/Under-verified> | <component, viewport, diff evidence> | <impact and actions> |
| Interactive states | <Pass/Issue/Under-verified> | <hover/focus/disabled/loading/error/empty state, etc.> | <impact and actions> |
| Accessibility | <Pass/Issue/Under-verified> | <semantics, keyboard, focus, ARIA, contrast> | <impact and actions> |
| Text & localization | <Pass/Issue/Under-verified> | <copy, line wrap, length, language support> | <impact and actions> |
| Regression risk | <Low/Medium/High> | <affected pages, shared components, snapshot changes> | <verification suggestion> |

## Priority Findings

1. [🔒|❌|⚠️|🧪|👁️|📈|🧭] <file:line-line | file:function | file:config_key | file | screenshot/page area | cross-file contract> - <actionable issue title>
   - Evidence: <shortest direct evidence: diff/screenshot/context/component state>
   - Impact: <user experience, accessibility, misoperation, layout breakage, or brand consistency impact>
   - Fix: <specific UI, style, component, copy, or state handling suggestion>
   - Verification: <minimal verification loop: command/test name/manual path + covered scenarios + expected result>
   - Confidence: <High | Medium | Low> - <only explain when not high-confidence>
   - Blocking reason: <include only for blockers>

If no priority findings, write:

None.

## Commit Guidance

- **Required before commit:** <blockers or `None`>
- **Suggested verification:** <screenshot paths, viewports, themes, browsers, interaction states, accessibility checks>
- **Suggested documentation:** <PR screenshots, recordings, design links, Storybook links, test notes; write `None` if none>

## Risk Detail

- **User visible changes:** <none | brief description>
- **Affected paths:** <pages, components, entry points, roles, devices>
- **Uncovered states:** <none | loading/error/empty/disabled/focus/mobile/dark mode/i18n, etc.>
- **Regression risk:** <🔴 High | 🟡 Medium | 🟢 Low> - <reason>
```
