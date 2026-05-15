# Optional Visual Output Guidance

Use this file only when `SKILL.md` visual-mode criteria are met: the user asks for a visual/report-style review, the result will be shared with a team, or the diff already meets large/high-risk criteria and structured visual summaries materially improve the commit decision.

## Principles

- Visual elements must improve one of four decisions: can I commit, what must I fix, what should I test, what risk should I watch.
- Prefer compact status tables over health bars or numeric percentages.
- Use bullets for findings that need evidence, impact, and fix details.
- Do not invent precision. If coverage, risk, or completeness is unknown, write `Unknown` or `N/A`.
- Keep the single verdict token from the main template. Do not add a second verdict box with a conflicting label.
- Follow the selected output language from `SKILL.md`.
- Keep the field label `VERDICT` exactly in English, even in localized visual reports.

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

| Area | Status | Note |
|---|---|---|
| Code hygiene | <Clean / Needs attention / Unknown> | <short evidence> |
| Security | <Clean / Needs attention / Unknown> | <short evidence> |
| Tests | <Covered / Needs verification / Unknown> | <short evidence> |
| Regression risk | <Low / Medium / High> | <short evidence> |

## Priority Findings

<findings with evidence, impact, fix, decision impact; write `None` if none>

## Commit Guidance

- **Before commit:** <required fix or `None`>
- **Suggested verification:** <tests/manual checks>

## Risk Detail

<risk matrix, changed-area summary, or flow diagram only when it improves the commit decision>
```

### Chinese Visual Review Skeleton

```markdown
# 提交前审查

> ⚠️ **部分审查:** <原因>. 未审查的部分列于"未审查变更"中。

**VERDICT:** <SAFE_TO_COMMIT | SAFE_TO_COMMIT_WITH_NOTES | DO_NOT_COMMIT>
**结论：** <一句话提交决策>
**差异来源：** <来源>
**审查范围：** <完整 | 部分>
**变更规模：** <文件数和行数>
**未审查变更：** <无或具体限制>

| 领域 | 状态 | 说明 |
|---|---|---|
| 代码卫生 | <干净 / 需注意 / 未知> | <简短证据> |
| 安全 | <干净 / 需注意 / 未知> | <简短证据> |
| 测试 | <已覆盖 / 需验证 / 未知> | <简短证据> |
| 回归风险 | <低 / 中 / 高> | <简短证据> |

## 重点发现

<包含证据、影响、修复、决定影响的问题列表；没有则写 `无`>

## 提交建议

- **提交前：** <必须修复的问题或 `无`>
- **建议验证：** <测试或手动检查>

## 风险细节

<仅在有助于提交决策时使用风险矩阵、变更区域摘要或流程图>
```

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
