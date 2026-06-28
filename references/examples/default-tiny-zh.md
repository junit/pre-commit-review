# 中文 Default 与 Tiny 示例

这些示例只是可选校准材料。

它们用于在需要时对齐语气与结构。
不要把它们当成权威规则来源。

## 示例 1：Default Review

场景：一次附加型 schema 变更，为 `users` 表增加可空的 `preferred_locale` 列，并新增一个轻量仓库读取逻辑。

```markdown
# 提交前审查

**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES
**结论：** 可以提交添加语言列的迁移，但建议在本次提交中一并编写 `getLocale` 方法的单元测试。
**统计：** 0 个阻塞项 · 1 个非阻塞提醒 · 1 个测试缺口 · 0 个审查限制
**差异来源：** 暂存区差异，经 helper 脚本（`scripts/collect_diff_context.sh`）提取
**审查范围：** 完整审查 - 已审查 `schema.prisma` 与 `userRepo.ts` 的全部 hunk；未访问数据库不构成部分审查
**变更规模：** 2 个文件, +24 行 / -3 行
**风险等级：** 🟡 中 - schema 变更触及数据完整性维度，但为附加列且有默认值
**未审查变更：** 无

## 执行摘要

本次变更为 `users` 表新增 `preferred_locale` 可空列（默认 'en-US'）。未发现阻塞性风险。存量数据行将采用默认值填充，主风险是未来查询时的默认值回退一致性，建议补充测试后提交。

## 重点发现

1. ⚠️ `src/repo/userRepo.ts:22` - 未为新增的 `getLocale` 方法编写单元测试
   - 证据：diff 修改了数据库读取，但测试目录下无对应用例更改
   - 影响：未来如果默认值回退逻辑变更，缺少自动化回归防护网
   - 修复：在 `userRepo.test.ts` 中补充覆盖 `getLocale` 返回 NULL 与正确语言的测试
   - 验证：运行 `pnpm test userRepo` 全绿且覆盖率不降低
   - 置信度：高

## 提交建议

- **提交前必须处理：** 无
- **提交前建议处理：** 补充 `getLocale` 单元测试
- **可后续跟进：** 无
- **建议验证：** `pnpm test userRepo` 验证查询逻辑
- **建议补充说明：** PR 描述中附带迁移回滚的 down 脚本

## 变更概览

- **修改：** 数据访问 - `userRepo` 增加 `getLocale` 获取逻辑
- **新增：** Prisma 字段 `preferred_locale`
- **删除：** 无
- **行为变化：** 查询用户语言时若无设置，回退返回默认值 'en-US'

## 风险摘要

| 维度 | 结论 | 依据 |
|---|---|---|
| 正确性 | 通过 | 附加字段无逻辑冲突 |
| 安全与隐私 | 无明显风险 | 无敏感数据暴露 |
| 数据与迁移 | 有风险 | 大表上执行 DDL 迁移可能产生短暂锁，需确保 PG 版本 ≥11 |
| 性能与扩展性 | 通过 | 单行主键查询，无性能风险 |
| 兼容性 | 无破坏 | 附加列，向后兼容 |
| 可观测性与回滚 | 充足 | 迁移自带 down 脚本，可快速回滚 |
| 测试覆盖 | 有缺口 | 读取逻辑缺乏单测覆盖 |

## 影响范围

- **直接影响：** `userRepo` 读取和 `users` 数据库表
- **间接影响：** 无明显间接影响
- **需要领域确认：** 无

## 回归风险

**等级：** 🟡 中
**原因：** 涉及数据库 schema 变更，但属添加列且有回滚 down 脚本
**最小验证闭环：** staging 库跑一次 migration 并 rollback 成功

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

## 示例 2：Tiny Review

场景：一次 README 文案拼写修正。

```markdown
# 提交前审查

**VERDICT:** SAFE_TO_COMMIT
**结论：** 可以提交；这是 README 文案修正，无运行时风险。
**差异来源：** 暂存区差异，基于 `git diff --cached` 提取
**审查范围：** 完整审查 - 包含 README.md 的单一 hunk
**变更规模：** 1 个文件, +2 行 / -2 行
**Tiny Diff 适用性：** 适用

- **变更：** 修正 README 中的命令拼写错误
- **逻辑：** 无运行时行为变化
- **影响范围：** 仅影响文档阅读者
- **风险：** 🟢 低 - 文档变更不改变构建链与代码
- **建议验证：** 无需额外测试
- **提交前：** 无

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
