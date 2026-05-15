# Output Examples

Use this file only when the model needs a concrete localized example to avoid mixed-language output or when the user asked for a more polished report example.
SKILL.md is authoritative; examples illustrate valid outputs only.

## Principles

- Follow `SKILL.md` if any example appears incomplete or outdated.
- Use examples for tone, localization, and scenario shape, not as a second output specification.
- Never replace the `**VERDICT:**` field label with a translated label.

## Chinese Example

```markdown
# 提交前审查

**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES
**结论：** 可以提交，但建议先运行受影响的测试。
**差异来源：** 暂存区差异
**审查范围：** 完整审查
**变更规模：** 2 个文件, +20 行 / -4 行
**未审查变更：** 无

## 重点发现

无

## 提交建议
- **提交前：** 无
- **建议验证：** 运行受影响模块的测试

## 变更概览
- **修改：** `src/auth/validator.js` — 增加了对 session token 的空值检查
- **新增：** 无 - 仅修改。

## 风险摘要
- **逻辑变化：** session token 校验现在会显式拒绝 null/undefined
- **影响范围：** 自包含 — 仅影响 `validateSession` 的调用方
- **回归风险：** 🟢 低 — 更严格的校验，不会拒绝之前能通过的 token
- **监控点：** 无需额外监控
```

## Chinese Example with Blocking Issue

```markdown
# 提交前审查

**VERDICT:** DO_NOT_COMMIT
**结论：** 发现硬编码 API key — 移除后再提交。
**差异来源：** 暂存区差异
**审查范围：** 完整审查
**变更规模：** 1 个文件, +8 行 / -2 行
**未审查变更：** 无

## 重点发现

1. 🔒 `src/config.js:14` - 源码中硬编码 API key
   - 证据：`const API_KEY = "sk-prod-..."`
   - 影响：凭据暴露在版本控制中
   - 修复：改用环境变量，轮换已泄露的 key
   - 决定影响：阻塞项

## 提交建议
- **提交前：** 移除硬编码 key，改用 `process.env.API_KEY`
- **建议验证：** 确认已在服务商后台轮换该 key
```

## Chinese Tiny Diff Example

```markdown
# 提交前审查

**VERDICT:** SAFE_TO_COMMIT
**结论：** 可以提交；这是文档措辞更新，无行为风险。
**差异来源：** 暂存区差异
**审查范围：** 完整审查
**变更规模：** 1 个文件, +2 行 / -2 行

- **变更：** 更新 README 中的安装说明措辞。
- **代码卫生：** 干净 - 未发现代码卫生问题。
- **逻辑：** 无逻辑变化 - 文档变更。
- **影响范围：** 自包含 - 只影响读者说明。
- **风险：** 🟢 低 - 不影响运行时代码。
- **测试：** 无需额外测试。
```

## Chinese Partial Review Example

```markdown
# 提交前审查

> ⚠️ **部分审查:** diff 输出被截断，已优先审查高风险文件。未审查的部分列于"未审查变更"中。

**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES
**结论：** 已审查高风险路径，未发现阻塞项；提交前建议检查未展开的生成文件。
**差异来源：** 通过辅助脚本获取的暂存区差异
**审查范围：** 部分审查 - 辅助脚本截断差异输出
**变更规模：** 18 个文件, +900 行 / -120 行
**未审查变更：** 生成快照文件仅按文件列表和统计检查，未逐行审查

## 重点发现

无

## 提交建议
- **提交前：** 确认生成快照来自预期命令
- **建议验证：** 运行相关测试并检查生成文件是否可复现

## 风险摘要
- **逻辑变化：** 核心逻辑路径未见行为变化；主要是测试快照更新
- **影响范围：** 测试和生成产物
- **回归风险：** 🟡 中 - 大量快照未逐行审查
- **监控点：** 无需额外监控
```

## English Example

```markdown
# Pre-Commit Review

**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES
**Conclusion:** Safe to commit, but run the affected tests first.
**Diff source:** staged diff
**Review scope:** full review
**Change scale:** 2 files, +20 / -4
**Unreviewed changes:** none

## Priority Findings

None

## Commit Guidance
- **Before commit:** None
- **Suggested verification:** Run the affected module tests

## What Changed
- **Modified:** `src/auth/validator.js` — added null check on session token
- **New:** None - modifications only.

## Risk Summary
- **Logic shift:** session token validation now rejects null/undefined explicitly
- **Blast radius:** self-contained — only affects `validateSession` callers
- **Regression risk:** 🟢 Low — stricter validation, no previously-passing tokens rejected
- **Watchpoints:** None needed
```

## English Example with Blocking Issue

```markdown
# Pre-Commit Review

**VERDICT:** DO_NOT_COMMIT
**Conclusion:** Hardcoded API key found — remove before committing.
**Diff source:** staged diff
**Review scope:** full review
**Change scale:** 1 file, +8 / -2
**Unreviewed changes:** none

## Priority Findings

1. 🔒 `src/config.js:14` - Hardcoded API key in source
   - Evidence: `const API_KEY = "sk-prod-..."`
   - Impact: Credential exposure in version control
   - Fix: Move to environment variable, rotate the leaked key
   - Decision impact: blocker

## Commit Guidance
- **Before commit:** Remove hardcoded key, use `process.env.API_KEY`
- **Suggested verification:** Confirm key is rotated in the provider dashboard
```
