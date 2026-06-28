# English Default And Tiny Examples

These examples are optional calibration aids only.

Use them to align tone and structure when needed.
Do not treat them as the authoritative rules source.

## Example 1: Default Review

Scenario: an additive schema change adds a nullable `preferred_locale` column and a small repository read path.

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

## Example 2: Tiny Review

Scenario: a one-line README typo fix.

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
- **Suggested verification:** No tests needed
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
