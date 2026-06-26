# English Output Templates

Loaded when the review is rendered in English. SKILL.md is authoritative for review logic and the Localization Rule; these templates are the concrete English output shapes.

#### English Default Developer Review

```markdown
# Pre-Commit Review

**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
**Conclusion:** <short, actionable 1-2 sentence decision summary: whether to commit + primary basis/highest risk + required before-commit action or suggested verification>
**Tally:** <N blockers · N non-blocking warnings · N test-gaps · N review-limits; write `None` if all four counts are 0>
**Diff source:** <source + exact scope: command/PR/commit range/user-pasted diff; specify staged/unstaged/base branch; write unknown if unknown>
**Review scope:** <full review if every diff hunk was inspected, partial only if part of the diff itself could not be reviewed (truncation, binary, missing source, etc.); name what was covered. Not having repository/caller access beyond the diff does NOT make a review partial — list such blind spots under Impact Scope or Unreviewed changes instead, and state whether any gap affects the verdict>
**Change scale:** <count of files under text diff> files, +<insertions> / -<deletions>; <individually specify generated files/lockfiles/binary assets/migration files if any, omit if none>
**Risk level:** <🔴 High | 🟡 Medium | 🟢 Low> - <risk basis: change type + path frequency/importance + high-risk domains + testing sufficiency>
**Unreviewed changes:** <none | list items: object + reason unreviewed + potential hidden risk + whether it blocks commit/affects verdict>

## Executive Summary

<2-4 sentences. Describe the change intent, major affected areas, whether blocking risks were found, and the most critical validation action before committing. Do not repeat all details below.>

## Priority Findings

1. [🔒|❌|⚠️|🧪|👁️|📈|🧭] <file:line-line | file:function | file:config_key | file | screenshot/page area | cross-file contract> - <actionable issue title: error mechanism + affected object + trigger conditions; avoid vague titles like "potential risk" or "attention needed">
   - Evidence: <shortest direct evidence: diff hunk, surrounding context, test/config/schema/screenshot location; must support the finding, do not copy large blocks>
   - Impact: <affected object + trigger conditions + failure mode + worst consequence; state whether user visible, data related, or production related>
   - Fix: <code-level modification direction + boundary conditions to handle + preferred solution>
   - Verification: <minimal verification loop: specific command/test name/manual path + scenarios covered + expected result>
   - Confidence: <High | Medium | Low> - <evidence strength explanation: directly proven by diff / needs caller context / needs runtime or product verification>
   - Blocking reason: <why this blocks commit; include only for blockers>

Add `   - Confidence: <medium | low> - <one-line reason>` only when a finding is not high-confidence; omit it for high-confidence findings so the line signals uncertainty rather than routine padding.
For blocking findings only, add `   - Blocking reason: <why this blocks commit; include only for blockers>`. If there are no priority findings, write:

None.

## Commit Guidance

- **Required before commit:** <only list blockers; if none, write `None`>
- **Suggested before commit:** <non-blocking suggestions that should be addressed in this commit; if none, write `None`>
- **Follow-up items:** <maintenance, docs, cleanup items that do not affect this commit; if none, write `None`>
- **Suggested verification:** <list minimal verification loops in priority order: command/test name/manual path + scenarios covered + expected result; explain alternative if unable to run>
- **Suggested documentation:** <PR description, reason for change, rollback plan, migration instructions, screenshots, monitoring notes; if none, write `None`>

## What Changed

- **Modified:** <summarize behavior changes by responsibility domain: business logic/API/UI/config/test/doc/dependency, do not just list file names>
- **New:** <new capabilities, entry points, tests, configs, migrations, dependencies and their purpose; if none, write `None - modifications only.`>
- **Deleted:** <deleted behavior, compatibility layers, configs, tests, or assets, and whether alternative paths exist; if none, write `None.`>
- **Behavioral changes:** <no user/API/data/permissions/performance/deployment behavior changes | clearly explain before, after, affected objects, and trigger conditions>

## Risk Summary

| Dimension | Conclusion | Basis |
|---|---|---|
| Correctness | <Pass / Risky / Under-verified> | <brief basis> |
| Security & Privacy | <No obvious risk / Risky / Under-verified> | <auth, authorization, credentials, PII, injection, supply chain, etc.> |
| Data & Migration | <Not applicable / Risky / Under-verified> | <schema, migration, compatibility, rollback, data integrity, etc.> |
| Performance & Scalability | <No obvious risk / Risky / Under-verified> | <queries, loops, caching, batching, resource usage, etc.> |
| Compatibility | <No breakage / Risky / Under-verified> | <API, config, serialization, frontend/backend, version compatibility, etc.> |
| Observability & Rollback | <Sufficient / Insufficient / Not applicable> | <logs, metrics, feature flags, rollback plan, etc.> |
| Test Coverage | <Sufficient / Gaps / Not run> | <unit tests, integration, E2E, manual verification, snapshots, etc.> |

For concrete post-commit monitoring only, add `- **Watchpoints:** <specific logs, metrics, dashboards, errors, or user behaviors>`.

## Impact Scope

- **Direct impact:** <modules, entry points, user paths, services, or tasks directly reached by changes>
- **Indirect impact:** <potentially affected callers, dependencies, consumers, data pipelines, deployment pipelines; if none, write `No obvious indirect impact.`>
- **Domain confirmation needed:** <none | domain + question to confirm + blocking/non-blocking>

## Regression Risk

**Level:** <🔴 High | 🟡 Medium | 🟢 Low>
**Reason:** <specifically explain why it is classified as this level>
**Minimal verification loop:** <minimal tests or manual steps required to mitigate risk>
```

#### English Tiny Diff Review

```markdown
# Pre-Commit Review

**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
**Conclusion:** <short, actionable 1-2 sentence decision summary: whether to commit + primary basis/highest risk + required before-commit action or suggested verification>
**Diff source:** <source + exact scope: command/PR/commit range/user-pasted diff; specify staged/unstaged/base branch>
**Review scope:** <full review if every diff hunk was inspected, partial only if part of the diff itself could not be reviewed (truncation, binary, missing source, etc.); name what was covered. No repository/caller access beyond the diff does NOT make a review partial — list such blind spots under Impact Scope or Unreviewed changes instead, and state whether any gap affects the verdict>
**Change scale:** <count of files under text diff> files, +<insertions> / -<deletions>; <individually specify generated files/lockfiles/binary assets/migration files if any, omit if none>
**Tiny Diff applicability:** <applicable / not applicable but downgraded to summary / not applicable, should use default template>

- **Change:** <risk basis: change type + path frequency/importance + high-risk domains + testing sufficiency>
- **Logic:** <no logic changes (use exact phrase "No logic change" if no runtime behavior changes) and why | changes: before/after/trigger/affected objects>
- **Impact scope:** <direct impact + indirect impact + confirmed unaffected boundaries; avoid writing "self-contained" without basis>
- **Risk:** <🔴 High | 🟡 Medium | 🟢 Low> - <risk basis: change type + path frequency + high-risk domains + testing sufficiency>
- **Test:** <covered/uncovered + minimal suggested verification + whether it impacts commit decision>
- **Before commit:** <None / specify one required action>
```

---

## Visual Review Skeleton

### English Visual Review Skeleton

```markdown
# Pre-Commit Review

> ⚠️ **Partial review:** <only when visual/interaction/assets cannot be fully reviewed; explain reason>. Areas not reviewed are listed under Unreviewed changes.

**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
**Conclusion:** <short, actionable 1-2 sentence decision summary: whether to commit + primary basis/highest risk + required before-commit action or suggested verification>
**Diff source:** <source + exact scope: command/PR/commit range/user-pasted diff; specify staged/unstaged/base branch>
**Review scope:** <full review if every diff hunk was inspected, partial only if part of the diff itself could not be reviewed (truncation, binary, missing source, etc.); name what was covered. No repository/caller access beyond the diff does NOT make a review partial — list such blind spots under Impact Scope or Unreviewed changes instead, and state whether any gap affects the verdict>
**Change scale:** <count of files under text diff> files, +<insertions> / -<deletions>; <individually specify generated files/lockfiles/binary assets/migration files if any, omit if none>
**Visual scope:** <reviewed pages/components + states + viewport + theme + language + browser/platform; uncovered items must be listed in Unreviewed changes>
**Unreviewed changes:** <none | list items: object + reason unreviewed + potential hidden risk + whether it blocks commit/affects verdict>

## Visual Review Matrix

| Area | Signal | Evidence | Conclusion |
|---|---|---|---|
| Code Quality & Consistency | <Pass/Issue/Under-verified> | <design system tokens, spacing, colors, component specifications> | <impact and actions> |
| Layout & Responsive | <Pass/Issue/Under-verified> | <screenshot area, component, viewport, diff evidence> | <impact and actions> |
| Interactive States | <Pass/Issue/Under-verified> | <hover/focus/disabled/loading/error/empty state, etc.> | <impact and actions> |
| Accessibility | <Pass/Issue/Under-verified> | <semantics, keyboard, focus, ARIA, contrast, screen reader impact> | <impact and actions> |
| Text & Localization | <Pass/Issue/Under-verified> | <copy, line wrap, length, Chinese/English/RTL, etc.> | <impact and actions> |
| Regression Risk | <Low/Medium/High> | <affected pages, shared components, snapshot changes> | <suggested verification> |

## Priority Findings

1. [🔒|❌|⚠️|🧪|👁️|📈|🧭] <file:line-line | file:function | file:config_key | file | screenshot/page area | cross-file contract> - <actionable issue title: error mechanism + affected object + trigger conditions; avoid vague titles like "potential risk" or "attention needed">
   - Evidence: <evidence in diff, screenshot area, component state, or visual comparison>
   - Impact: <user experience, accessibility, conversion, misoperation, layout breakage, or brand consistency impact>
   - Fix: <specific UI, style, component, copy, or state handling suggestion>
   - Verification: <minimal verification loop: specific command/test name/manual path + scenarios covered + expected result>
   - Confidence: <High | Medium | Low> - <evidence strength explanation: directly proven by diff / needs screenshot/runtime validation>
   - Blocking reason: <only for blockers>

If no priority findings, write:

None.

## Commit Guidance

- **Required before commit:** <blocker items checklist or `None`>
- **Suggested verification:** <screenshot paths, viewports, themes, browsers, interactive states, accessibility checks>
- **Suggested documentation:** <PR screenshots, video recordings, design links, Storybook links, test notes; if none, write `None`>

## Risk Detail

- **User visible changes:** <none | brief description>
- **Affected paths:** <pages, components, entry points, roles, devices>
- **Uncovered states:** <none | loading/error/empty/disabled/focus/mobile/dark mode/i18n, etc.>
- **Regression risk:** <🔴 High | 🟡 Medium | 🟢 Low> - <reason>
```

---

## Review Limitations / Review Limitations

```markdown
- [👁️] <limitation title>
  - Scope: <which files, assets, states, or contexts were not reviewed>
  - Reason: <too large, binary, generated files, missing screenshots, missing dependencies, cannot run, insufficient context, etc.>
  - Risk: <what issues this limitation might hide>
  - Suggestion: <supplementary diff, screenshots, test results, CI logs, design docs, schema, caller context, etc.>
  - Is blocking: <Yes / No> - <reason>
```

## Machine-Readable Metadata / review-meta block

```markdown
<!-- review-meta
verdict: <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
blockers: <number>
warnings: <number>
test_gaps: <number>
review_limits: <number>
risk: <high | medium | low>
scope: <full | partial>
template: <default | tiny | visual>
-->
```

---

## Minimal Few-Shot Examples

This section contains minimal, realistic examples for English Default and Tiny reviews to serve as structural anchors for daily reviews.

### Example 1: English Default Developer Review

```markdown
# Pre-Commit Review

**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES
**Conclusion:** Safe to commit the language column migration, but suggest writing unit tests for the new `getLocale` method in this commit.
**Tally:** 0 blockers · 1 non-blocking warning · 1 test-gap · 0 review-limits
**Diff source:** staged diff via helper script (`scripts/collect_diff_context.sh`)
**Review scope:** full review - all hunks in `schema.prisma` and `userRepo.ts` inspected
**Change scale:** 2 files, +24 / -3; no lockfiles or large generated files
**Risk level:** 🟡 Medium - database schema change touches data integrity, but it is an additive column with a default value
**Unreviewed changes:** none

## Executive Summary

This change adds an optional `preferred_locale` column (defaulting to 'en-US') to the `users` table. No blocking issues were found. The main residual risk is the consistency of default fallback logic during retrieval; suggest adding unit tests before committing.

## Priority Findings

1. ⚠️ `src/repo/userRepo.ts:22` - missing unit tests for the new `getLocale` method
   - Evidence: diff adds database retrieval logic, but no test changes are present under test directory
   - Impact: future changes to fallback logic could bypass regression testing
   - Fix: add tests in `userRepo.test.ts` covering both NULL and populated language retrieval
   - Verification: run `pnpm test userRepo` to verify success
   - Confidence: High

## Commit Guidance

- **Required before commit:** None
- **Suggested before commit:** Add unit tests for `getLocale`
- **Follow-up items:** None
- **Suggested verification:** `pnpm test userRepo` to check query logic
- **Suggested documentation:** Include migration down SQL script in the PR description

## What Changed

- **Modified:** data access - added retrieval logic in `userRepo.getLocale`
- **New:** Prisma schema column `preferred_locale`
- **Deleted:** none
- **Behavioral changes:** query defaults to 'en-US' if preferred_locale is not set

## Risk Summary

| Dimension | Conclusion | Basis |
|---|---|---|
| Correctness | Pass | simple query logic without exceptions |
| Security & Privacy | No obvious risk | no sensitive data exposed |
| Data & Migration | Risky | large tables might encounter migration locks; confirm production PG version ≥11 |
| Performance & Scalability | Pass | single-row index query; no hot path impact |
| Compatibility | No breakage | additive column; backward compatible |
| Observability & Rollback | Sufficient | migration includes automated rollback script |
| Test Coverage | Gaps | database query logic lacks unit tests |

## Impact Scope

- **Direct impact:** `userRepo` and database schema
- **Indirect impact:** none
- **Domain confirmation needed:** none

## Regression Risk

**Level:** 🟡 Medium
**Reason:** database schema migration, mitigated by automated rollback scripts
**Minimal verification loop:** run migration and rollback on staging

<!-- review-meta
verdict: SAFE_TO_COMMIT_WITH_NOTES
blockers: 0
warnings: 1
test_gaps: 1
review_limits: 0
risk: medium
scope: full
template: default
-->
```

### Example 2: English Tiny Diff Review

```markdown
# Pre-Commit Review

**VERDICT:** SAFE_TO_COMMIT
**Conclusion:** Safe to commit; documentation update in README has no runtime risk.
**Diff source:** staged diff via `git diff --cached`
**Review scope:** full review - inspected the single modified hunk in `README.md`
**Change scale:** 1 file, +2 / -2
**Tiny Diff applicability:** applicable

- **Change:** Corrected installation commands in the README quickstart
- **Logic:** No runtime behavior change
- **Impact scope:** Affects document readers only
- **Risk:** 🟢 Low - prose update only; no impact on code or build
- **Test:** No tests needed
- **Before commit:** None

<!-- review-meta
verdict: SAFE_TO_COMMIT
blockers: 0
warnings: 0
test_gaps: 0
review_limits: 0
risk: low
scope: full
template: tiny
-->
```
