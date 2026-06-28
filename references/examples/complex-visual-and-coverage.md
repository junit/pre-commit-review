# Complex Visual And Coverage-Led Examples

These examples are optional calibration aids for non-routine reviews.

Use them when:

- the review is partial
- the review is visual/UI-heavy
- the review is generated-heavy or coverage-led
- the review contains a clear blocking issue and needs structural anchoring

Do not use this file as a second rules source.

## Example 1: Blocking Correctness Or Security Issue

Scenario: an auth middleware refactor introduces a short-circuit bug that allows unauthenticated access.

```markdown
# 提交前审查

**VERDICT:** DO_NOT_COMMIT
**结论：** 不可提交 — 鉴权中间件重构引入短路求值 bug，当 `req.user` 为 falsy 时跳过鉴权，未认证请求可直接访问受保护路由；必须修复后再提交。
**统计：** 1 个阻塞项 · 0 个非阻塞提醒 · 0 个测试缺口 · 0 个审查限制
**差异来源：** 暂存区差异，经 helper 脚本（`scripts/collect_diff_context.sh --source staged`）提取，匹配 `origin/main`
**审查范围：** 完整审查 - 已审查 `src/middleware/auth.ts` 的全部 hunk；同文件既有 `requireRole` 用法用于确认正确模式
**变更规模：** 1 个文件, +9 行 / -4 行；无迁移、锁文件或二进制资产
**风险等级：** 🔴 高 - 认证绕过位于受保护路由前置中间件，可被任意未认证请求触达
**未审查变更：** 无

## 执行摘要

本次重构将鉴权中间件改为提前返回模式，但在条件判断中引入了短路求值 bug：当 `req.user` 为 falsy（未认证）时，鉴权检查被整体跳过而非拒绝。由于该中间件挂载在所有受保护路由之前，未认证请求可直接访问受保护资源。必须修复后再提交。

## 重点发现

1. 🔒 `src/middleware/auth.ts:24` - 短路求值导致未认证请求绕过鉴权
   - 证据：重构后为 `if (!req.user || !checkPermission(req.user, req.route.requiredPermission)) return next();`；当 `!req.user` 为真（未认证）时，`||` 短路，`checkPermission` 不执行，直接 `return next()` 放行；正确逻辑应为未认证时 `return res.status(401).end()`
   - 影响：任意未认证请求可访问挂载该中间件的全部受保护路由（用户数据、订单、管理接口）；最坏后果为全站未授权数据访问与越权操作；用户可见且生产相关
   - 修复：将条件拆分为两步——先 `if (!req.user) return res.status(401).json({ error: "unauthenticated" });`，再 `if (!checkPermission(...)) return res.status(403).json({ error: "forbidden" });`；禁止用 `||` 合并认证与授权判断
   - 验证：新增测试覆盖：无 token → 401；有 token 但无权限 → 403；有 token 且有权限 → 200；对受保护路由发起无凭证请求确认返回 401
   - 置信度：高
   - 阻塞原因：必须在提交前修复，因为该中间件是受保护路由的唯一前置防线，`||` 短路使认证检查在未认证时被完全跳过。
```

## Example 2: Partial Coverage-Led Review

Scenario: a large frontend refactor includes many generated snapshots and one large bundled artifact that could not be fully inspected.

```markdown
# 提交前审查

> ⚠️ **部分审查:** diff 输出被截断，已优先审查高风险文件。未审查的部分列于"未审查变更"中。

**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES
**结论：** 已审查高风险路径，未发现阻塞项；提交前建议检查未展开的生成文件。
**统计：** 0 个阻塞项 · 0 个非阻塞提醒 · 0 个测试缺口 · 1 个审查限制
**差异来源：** 暂存区差异，基于 `git diff --cached` 提取后被系统自动截断
**审查范围：** 部分审查 - 已覆盖变动的手动编写逻辑；未审查生成的快照和编译后 bundle 产物，主要是由于 diff 体积超出 LLM 预算
**变更规模：** 18 个文件, +900 行 / -120 行；包含 15 个生成的测试快照文件与 1 个压缩产物
**风险等级：** 🟡 中 - 大量生成的快照文件和压缩 bundle 未经逐行审查
**未审查变更：** `dist/app.bundle.js` 未审查 - 生成文件体积过大且为压缩混淆格式；可能隐藏构建产物与源码不一致风险；不阻塞提交，但建议确认构建命令可复现

## 执行摘要

本次变更包含前端组件重构及自动生成的测试快照文件。由于快照文件体积过大，diff 已被截断。核心重构文件已通过完整审查，未发现阻塞项。

## 重点发现

无。

## 提交建议

- **提交前必须处理：** 无
- **提交前建议处理：** 本地重新执行 `npm run build` 确保生成的 bundle 无哈希冲突
- **可后续跟进：** 无
- **建议验证：** 运行 `pnpm test`，重点覆盖受重构组件影响的 Checkout 交互路径
- **建议补充说明：** PR 描述中补充 Storybook 视觉验证截图
```

## Example 3: Visual Review

Scenario: a shared `<Button>` component is refactored with new variants and sizes, requiring state and accessibility validation.

```markdown
# Pre-Commit Review

> ⚠️ **Partial review:** Visual/interaction states beyond the documented `variant` and `size` props could not be exercised without a running build; uncovered items are listed under Unreviewed changes.

**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES
**Conclusion:** Safe to commit the `<Button>` variant/size refactor, but verify the focus ring contrast on the danger variant and confirm dark-mode hover states in Storybook before shipping.
**Diff source:** staged diff via helper script, matching `origin/main`
**Review scope:** full review - every diff hunk in `Button.tsx` and `button.css` inspected; runtime/styled-states beyond the diff are a blind spot, not a coverage gap
**Change scale:** 2 files, +34 / -12; no generated or binary assets
**Visual scope:** reviewed `<Button>` component on desktop light + dark theme, `variant=primary|danger` and `size=md|sm`, hover/focus/disabled states; uncovered: mobile viewport, RTL, loading state
**Unreviewed changes:** mobile (<375px) and RTL layouts - not exercised in this review; could hide wrapping/overflow. Non-blocking; verify in Storybook before release.

## Visual Review Matrix

| Area | Signal | Evidence | Conclusion |
|---|---|---|---|
| Design consistency | Pass | New `variant`/`size` props reuse existing design tokens (`--color-danger`, `--space-sm`) | Consistent with the token system |
| Layout & responsive | Under-verified | `size="sm"` reduces padding via `clamp()`; desktop looks correct, but mobile wrapping not exercised | Verify at 320px before release |
| Interactive states | Issue | `:hover` on danger variant drops opacity to 0.6 with no `:focus-visible` ring change | Add a distinct `:focus-visible` outline that persists on hover |
| Accessibility | Issue | Danger variant contrast falls below the 4.5:1 AA target for normal text | Darken the danger text token or increase size/weight |
| Text & localization | Pass | Button label reuses existing `children`; no new copy or truncation risk for short labels | No localization impact |
| Regression risk | Medium | `<Button>` is consumed by many pages; focus-ring regression affects all of them | Snapshot tests on high-traffic pages before merge |

## Priority Findings

1. ⚠️ `src/components/Button/button.css:42` - Danger variant fails WCAG AA contrast on focus
   - Evidence: `:hover { opacity: 0.6 }` with no compensating `:focus-visible` rule; computed contrast 4.1:1
   - Impact: Keyboard and low-vision users lose the focus indicator and readable text on the danger action across consuming pages
   - Fix: Add a persistent `:focus-visible` outline and raise the danger text contrast
   - Verification: Run `axe-core` against the Storybook danger button in both themes; expect 0 contrast violations
   - Confidence: High
```
