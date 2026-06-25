# English Output Templates

Loaded when the review is rendered in English. SKILL.md is authoritative for review logic and the Localization Rule; these templates are the concrete English output shapes.

#### English Default Developer Review

```markdown
# Pre-Commit Review

**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
**Conclusion:** <one sentence stating whether they can commit and what to do next>
**Tally:** <count of findings by type: "N blocker · N warning · N test-gap" using the 🔒/❌ blockers, ⚠️ warnings, 🧪 test-gaps; write `None` if no priority findings>
**Diff source:** <how the diff was obtained>
**Review scope:** <full review | partial review with reason>
**Change scale:** <count> files, +<insertions> / -<deletions>
**Unreviewed changes:** <none | unstaged/generated/too-large files or other limits>

## Priority Findings

1. [🔒|❌|⚠️|🧪] `file:line` - <issue title>
   - Evidence: <what in the diff shows this>
   - Impact: <what can break, leak, corrupt, or confuse>
   - Fix: <specific next action>

For blocking findings only, add `   - Blocking reason: <why this blocks commit; include only for blockers>`. If there are no priority findings, write `None`.

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

For concrete post-commit monitoring only, add `- **Watchpoints:** <specific logs, metrics, dashboards, errors, or user behaviors>`.

```

#### English Tiny Diff Review

```markdown
# Pre-Commit Review

**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
**Conclusion:** <one sentence>
**Diff source:** <source>
**Review scope:** <full | partial>
**Change scale:** <files and lines>

- **Change:** <one sentence>
- **Logic:** <no logic change | exact delta>
- **Blast radius:** <self-contained | affected callers/dependencies>
- **Risk:** <🔴 High | 🟡 Medium | 🟢 Low> - <reason>
- **Test:** <minimal verification or required fix>
```

---
## Visual Review Skeleton

Use the concrete skeleton matching the selected output language when visual mode is justified.

### English Visual Review Skeleton

```markdown
# Pre-Commit Review

> ⚠️ **Partial review:** <reason>. Areas not reviewed are listed under Unreviewed changes.

**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
**Conclusion:** <one-sentence commit decision>
**Diff source:** <source>
**Review scope:** <full | partial>
**Change scale:** <files and lines>
**Unreviewed changes:** <none or limitations>

| Area | Signal | Evidence |
|---|---|---|
| <code quality/security/tests/regression risk/etc.> | <concern / verification need / review limit / meaningful risk> | <short evidence> |

## Priority Findings

<findings with evidence, impact, fix, and blocking reason only for blockers; write `None` if none>

## Commit Guidance

- **Before commit:** <required fix or `None`>
- **Suggested verification:** <tests/manual checks>

## Risk Detail

<risk matrix, changed-area summary, or flow diagram only when it improves the commit decision>
```
