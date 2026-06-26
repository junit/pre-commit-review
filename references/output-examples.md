# Output Examples

This file is a high-level reference for Visual Review Mode or complex reviews. Daily Default/Tiny reviews must not read or depend on this file to prevent token bloat.
SKILL.md is authoritative; examples illustrate valid outputs only.

## Principles

- Follow `SKILL.md` if any example appears incomplete or outdated.
- Use examples for tone, localization, and scenario shape, not as a second output specification.
- Never replace the `**VERDICT:**` field label with a translated label.


## English Example

Scenario: a public REST endpoint response shape changes from `{ items: [...] }` to `{ data: [...], meta: { page, total } }` — a backward-compatible addition, but one that touches the API contract and has indirect consumers.

```markdown
# Pre-Commit Review

**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES
**Conclusion:** Safe to commit the paginated list response, but confirm no external client keys off the old `{ items }` shape and add a contract test before exposing it to partners.
**Tally:** 0 blockers · 1 non-blocking warning · 1 test-gap · 0 review-limits
**Diff source:** staged diff via helper script (`scripts/collect_diff_context.sh --source staged`), matching `origin/main`
**Review scope:** full review - every hunk in `src/api/listController.ts` and `src/api/serializer.ts` inspected; no repository access to enumerate external callers, so caller impact is reasoned from the contract change, not a coverage gap
**Change scale:** 2 files, +28 / -9; no generated, lockfile, binary, or migration files
**Risk level:** 🟡 Medium - public API contract change on a high-traffic list endpoint with unknown external consumers; new fields are additive but the serialization path is shared
**Unreviewed changes:** none

## Executive Summary

This change paginates the `GET /orders` response, splitting the previous `{ items: [...] }` envelope into `{ data: [...], meta: { page, total, pageSize } }` while keeping `items` as a deprecated alias for one release. No blocking issues were found. The main residual risk is compatibility for clients that deserialize strictly or assert on the top-level `items` key, which should be confirmed against known consumers before opening the endpoint to partners.

## Priority Findings

1. ⚠️ `src/api/serializer.ts:44` - `items` alias is dropped when `pageSize` exceeds the dataset, breaking strict clients silently
   - Evidence: the alias returns `data.slice(0, pageSize)`, so a request for `pageSize=100` on a 50-row dataset returns `items` of length 50 while `meta.total` is 50 — clients that assert `items.length === meta.total` will pass here but a caller paginating past the end gets `items: []` with no signal that the alias is deprecated
   - Impact: external or partner clients that key off `items` and treat an empty array as "no data" rather than "past last page" will misinterpret paginated tail requests; worst case a dashboard shows a false "empty state" on the last page
   - Fix: keep `items` returning the full `data` (not the sliced page) for the deprecation window, or emit a `Deprecation` response header so clients notice during the transition
   - Verification: hit `GET /orders?page=3&pageSize=100` on a 250-row fixture; assert `items.length === data.length` and the `Deprecation` header is present
   - Confidence: High - the slice logic is directly visible in the diff
   - Blocking reason: none — additive response, no caller is forced to break, but partners should be notified

2. 🧪 `src/api/listController.ts:67` - no contract test pins the new `{ data, meta }` shape or the `items` alias
   - Evidence: the diff modifies the serializer and controller but adds no test asserting the response envelope
   - Impact: a future refactor could silently remove the `items` alias or reshape `meta` with no regression signal
   - Fix: add a serialization test covering `{ data, meta }` shape, the `items` alias equality, and the `Deprecation` header
   - Verification: `npm test -- listController.contract` passes with the three assertions above green
   - Confidence: Medium - assumes the team pins API contracts via tests rather than OpenAPI snapshots

## Commit Guidance

- **Required before commit:** None
- **Suggested before commit:** Keep `items` returning the full `data` set (not the page slice) for the deprecation window, or add a `Deprecation` header
- **Follow-up items:** After one release, remove the `items` alias and update OpenAPI + partner docs
- **Suggested verification:** `npm test -- listController.contract`; manually call `GET /orders?page=N` across first/middle/last/empty pages on the staging fixture
- **Suggested documentation:** PR description must note the additive `meta` field, the `items` deprecation window, and the migration path for partner clients

## What Changed

- **Modified:** API/business logic — `GET /orders` now paginates; the serializer emits `{ data, meta }` and keeps `items` as a deprecated alias
- **New:** `meta` envelope (`page`, `total`, `pageSize`); pagination query params `page`/`pageSize`
- **Deleted:** the implicit "return all rows" behavior on the list endpoint (bounded by `pageSize`, default 50)
- **Behavioral changes:** before, `GET /orders` returned all rows in `items`; after, it returns a page in `data` with `meta`, and `items` mirrors the page only (see finding 1)

## Risk Summary

| Dimension | Conclusion | Basis |
|---|---|---|
| Correctness | Pass | pagination math verified against a fixture; slice bounds correct |
| Security & Privacy | No obvious risk | no auth, PII, or input handling changes |
| Data & Migration | Not applicable | no schema or migration |
| Performance & Scalability | Pass | pagination reduces default payload from unbounded to 50 rows |
| Compatibility | Risky | public response shape changed; `items` alias mitigates but page-slice behavior can mislead strict clients |
| Observability & Rollback | Sufficient | feature-flagged via `PAGINATION_V2`; revert flips the flag without a deploy |
| Test Coverage | Gaps | no contract test for the new envelope or alias |

## Impact Scope

- **Direct impact:** `GET /orders` endpoint and its serializer; any internal service calling it
- **Indirect impact:** external/partner clients and dashboards that deserialize the response, especially those asserting on `items` length; OpenAPI consumers regenerated from the spec
- **Domain confirmation needed:** API platform — confirm whether any contracted partner pins the `{ items }` shape before removing the alias next release; non-blocking for this commit but required before the alias removal

## Regression Risk

**Level:** 🟡 Medium
**Reason:** public API contract change with unknown external consumers; mitigated by an additive `meta` field and a temporary `items` alias behind a feature flag, but strict clients could still misinterpret paginated tail responses
**Minimal verification loop:** run the contract test, then exercise first/middle/last/empty pages against the staging fixture

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

## English Example with Blocking Issue

Scenario: a search feature builds a SQL filter by string-concatenating user input into a raw query instead of using the ORM's parameterized builder.

```markdown
# Pre-Commit Review

**VERDICT:** DO_NOT_COMMIT
**Conclusion:** Do not commit — the new search builds a SQL filter by concatenating `req.query.q` into a raw query string, enabling SQL injection on a production user-lookup path; switch to the parameterized builder before committing.
**Tally:** 1 blocker · 0 non-blocking warnings · 0 test-gaps · 0 review-limits
**Diff source:** staged diff via helper script (`scripts/collect_diff_context.sh --source staged`), matching `origin/main`
**Review scope:** full review - both changed hunks in `src/repo/userSearch.ts` inspected; the ORM's query builder API was confirmed from the existing `findById` usage in the same file
**Change scale:** 1 file, +14 / -2; no migrations, lockfiles, or generated assets
**Risk level:** 🔴 High - untrusted user input reaches a raw SQL query on the user-lookup path, which is reachable without elevated privileges
**Unreviewed changes:** none

## Executive Summary

This change adds a free-text search to the user repository. The implementation concatenates the query parameter directly into a raw SQL `WHERE` clause instead of binding it as a parameter. Because the endpoint is reachable by any authenticated caller and the table holds PII (email, phone), this is a SQL injection vulnerability that can exfiltrate or modify user data. The commit must not ship until the filter uses the ORM's parameterized builder.

## Priority Findings

1. 🔒 `src/repo/userSearch.ts:38` - SQL injection via string-concatenated `req.query.q`
   - Evidence: `const sql = "SELECT * FROM users WHERE name LIKE '%" + term + "%'";` followed by `db.raw(sql)`, where `term` is `req.query.q` with no escaping or allow-listing
   - Impact: an authenticated caller can submit `q = "' UNION SELECT password, email FROM credentials--"` to read arbitrary columns, or `q = "'; DROP TABLE users;--"` to destroy data; the `users` table contains PII (email, phone), so this is user-visible and data-related, with production-wide blast radius
   - Fix: use the ORM's parameterized builder — `db.users.whereRaw("name LIKE ?", [`%${term}%`])` — so `term` is bound as a value, not parsed as SQL; reject or truncate `term` over a sane max length at the controller boundary
   - Verification: add a test asserting `q = "' OR '1'='1"` returns zero rows (not all users); run `sqlmap` against the staging endpoint and confirm no injection vector
   - Confidence: High - the concatenation and `db.raw` call are directly visible in the diff
   - Blocking reason: must fix before commit because untrusted input reaches a raw query on a reachable entry point, and the ORM's parameterized path (already used by `findById` in this file) is the correct defense — there is no existing guard (WAF/input validation) that reliably neutralizes arbitrary SQL fragments at this layer

## Commit Guidance

- **Required before commit:**
  - Replace the string-concatenated `db.raw(sql)` with the parameterized builder (`whereRaw(..., [bindings])`)
  - Add an injection regression test (`' OR '1'='1` → zero rows) and confirm `sqlmap` finds no vector
- **Suggested before commit:** None (the fix is the blocker)
- **Follow-up items:** add a lint/CI rule that flags `db.raw(` with string concatenation; review other repositories for the same pattern
- **Suggested verification:** unit test with injection payloads; `sqlmap -u <staging-url> --data "q="` returns no injectable parameter
- **Suggested documentation:** PR must describe the parameterized fix and note that no data migration or rotation is needed (assuming this has not yet shipped)

## What Changed

- **Modified:** data access — `userSearch` now builds a `LIKE` filter; previously the repository had no free-text search
- **New:** `searchUsers(term)` method and its controller wiring
- **Deleted:** none
- **Behavioral changes:** callers can now pass a free-text `q`; the implementation (pre-fix) interprets it as raw SQL rather than a bound value

## Risk Summary

| Dimension | Conclusion | Basis |
|---|---|---|
| Correctness | Risky | returns correct results for benign input, but accepts arbitrary SQL as input |
| Security & Privacy | Risky | SQL injection on a PII table reachable by any authenticated caller |
| Data & Migration | Risky | injection can read/modify/drop the `users` table at runtime |
| Performance & Scalability | Under-verified | a `LIKE '%...%'` leading wildcard defeats indexes; unverified on a large table |
| Compatibility | No breakage | additive search method; no existing API changes |
| Observability & Rollback | Insufficient | no query logging or rate limit on the new endpoint to detect abuse |
| Test Coverage | Gaps | no tests for the search path, let alone injection payloads |

## Impact Scope

- **Direct impact:** `searchUsers` and the `GET /users?q=` endpoint that calls it
- **Indirect impact:** every authenticated caller gains a path to the `users` table; any downstream service or analytics job that reads `users` is affected if data is corrupted
- **Domain confirmation needed:** security — confirm whether a WAF or prepared-statement enforcement layer exists in front of this repository; non-blocking for the fix (parameterize regardless), but informs follow-up hardening

## Regression Risk

**Level:** 🔴 High
**Reason:** an exploitable injection on a production PII table that is reachable without elevated privileges; even a single deployment window exposes user data
**Minimal verification loop:** parameterize the query, add the injection test, and run `sqlmap` against staging before merging

<!-- review-meta
verdict: DO_NOT_COMMIT
blockers: 1
warnings: 0
test_gaps: 0
review_limits: 0
risk: high
scope: full
template: default
-->
```

## Chinese Example

场景：为 `users` 表新增一个可空列 `preferred_locale`，并附带 Prisma 迁移；属于向后兼容的附加性 schema 变更，但存在一个非阻塞的默认值/回填问题。

```markdown
# 提交前审查

**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES
**结论：** 可以提交 `preferred_locale` 列与迁移，但建议在本次提交中为存量行回填默认值，并补一个迁移可逆性测试。
**统计：** 0 个阻塞项 · 1 个非阻塞提醒 · 1 个测试缺口 · 0 个审查限制
**差异来源：** 暂存区差异，经 helper 脚本（`scripts/collect_diff_context.sh --source staged`）提取，匹配 `origin/main`
**审查范围：** 完整审查 - 已审查 `schema.prisma`、迁移文件与 `userRepo.ts` 的全部 hunk；diff 之外的生产库迁移执行情况属于盲点，不构成部分审查
**变更规模：** 3 个文件, +46 行 / -3 行；包含 1 个 Prisma 迁移文件（`20260625_add_preferred_locale/migration.sql`）
**风险等级：** 🟡 中 - 数据库 schema 变更触及数据完整性维度，但为附加列、有默认值、可回滚
**未审查变更：** 无

## 执行摘要

本次变更为 `users` 表新增可空列 `preferred_locale`，默认 `'en-US'`，并附带 Prisma 迁移。仓库层新增读取逻辑。未发现阻塞项。主要残留风险是存量行的默认值回填策略与迁移可逆性，建议在本次提交中一并处理。

## 重点发现

1. ⚠️ `prisma/migrations/20260625_add_preferred_locale/migration.sql:8` - 附加列在 PostgreSQL 下用 `DEFAULT` 但未回填存量行
   - 证据：迁移为 `ALTER TABLE users ADD COLUMN preferred_locale TEXT DEFAULT 'en-US'`；PostgreSQL 11+ 会惰性回填（不重写表），但旧版本或大表上仍可能锁表
   - 影响：大表（百万级行）上执行该迁移可能持有 `AccessExclusiveLock` 较久，阻塞读写；存量行读取到的 `preferred_locale` 取决于是否回填
   - 修复：显式分批回填（`UPDATE users SET preferred_locale='en-US' WHERE preferred_locale IS NULL` 分批），或在迁移前确认生产 PG 版本 ≥11
   - 验证：在等价规模 staging 库执行迁移并测量锁时长；确认存量行 `preferred_locale` 均为 `'en-US'`
   - 置信度：中 - 需确认生产 PG 版本与表规模才能确定是否实际锁表
   - 阻塞原因：无 - 附加列、有默认值、可回滚，不阻塞本次提交

2. 🧪 `src/repo/userRepo.ts:22` - 未为 `preferred_locale` 的读取与默认值补充测试
   - 证据：diff 新增 `getLocale(userId)` 读取逻辑，但无对应单测
   - 影响：未来调整默认值或回填策略时缺少回归网
   - 修复：补充单测，覆盖已设置值、NULL 回退到默认值两种情况
   - 验证：`pnpm test userRepo` 全绿，覆盖率包含 `getLocale` 分支
   - 置信度：高 - diff 中确实无测试

## 提交建议

- **提交前必须处理：** 无
- **提交前建议处理：** 确认生产 PG 版本 ≥11；若 <11 或为大表，迁移改为分批回填
- **可后续跟进：** 为 `getLocale` 补充单元测试
- **建议验证：** 在 staging 等价库执行迁移并回滚各一次；`pnpm test userRepo` 全绿
- **建议补充说明：** PR 描述附迁移 SQL、锁时长测量结果与回滚方案

## 变更概览

- **修改：** 数据访问 - `userRepo` 新增 `getLocale(userId)` 读取逻辑
- **新增：** `users.preferred_locale TEXT DEFAULT 'en-US'` 列；Prisma 迁移 `20260625_add_preferred_locale`
- **删除：** 无
- **行为变化：** 读取用户时若未显式设置语言，回退到 `'en-US'`

## 风险摘要

| 维度 | 结论 | 依据 |
|---|---|---|
| 正确性 | 通过 | 附加列、有默认值、读取逻辑正确 |
| 安全与隐私 | 无明显风险 | 不涉及鉴权或敏感字段 |
| 数据与迁移 | 有风险 | 附加列但存量行回填/锁表行为取决于 PG 版本与表规模 |
| 性能与扩展性 | 未充分验证 | 迁移锁时长未在等价规模库测量 |
| 兼容性 | 无破坏 | 附加列，旧代码不读该列不受影响 |
| 可观测性与回滚 | 充足 | 迁移可逆（附 `DROP COLUMN` down 脚本）；有 feature flag 控制 `getLocale` 使用 |
| 测试覆盖 | 有缺口 | `getLocale` 与迁移可逆性缺少测试 |

## 影响范围

- **直接影响：** `userRepo.getLocale` 与 `users` 表 schema
- **间接影响：** 任何依赖 `users` 表的报表/任务；未来消费 `preferred_locale` 的国际化模块
- **需要领域确认：** DBA/SRE - 确认生产 PG 版本与 `users` 表规模，决定是否分批回填；非阻塞本次提交，但迁移部署前需确认

## 回归风险

**等级：** 🟡 中  
**原因：** 数据库 schema 变更触及数据完整性，但属附加列、有默认值、可回滚，主要风险是迁移锁时长  
**最小验证闭环：** staging 等价库执行迁移+回滚，测量锁时长，确认存量行默认值正确

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

## Chinese Example with Blocking Issue

场景：一个鉴权中间件重构引入了短路求值 bug，导致当 `req.user` 为 falsy 时跳过鉴权检查，构成可被未认证请求触达的认证绕过。

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
   - 触发条件：发送不带有效会话/token 的请求到任意受保护路由，鉴权被跳过，返回 200 而非 401
   - 修复：将条件拆分为两步——先 `if (!req.user) return res.status(401).json({ error: "unauthenticated" });`，再 `if (!checkPermission(...)) return res.status(403).json({ error: "forbidden" });`；禁止用 `||` 合并认证与授权判断
   - 验证：新增测试覆盖：无 token → 401；有 token 但无权限 → 403；有 token 且有权限 → 200；对受保护路由发起无凭证请求确认返回 401
   - 置信度：高 - 短路逻辑直接可见于 diff
   - 阻塞原因：必须在提交前修复，因为该中间件是受保护路由的唯一前置防线，`||` 短路使认证检查在未认证时被完全跳过，现有的其他防护（如路由层校验）不能替代全局鉴权中间件

## 提交建议

- **提交前必须处理：**
  - 将认证与授权判断拆分为两步，未认证返回 401，无权限返回 403
  - 补充覆盖 401/403/200 三种情况的中间件测试
- **提交前建议处理：** 无（修复即阻塞项）
- **可后续跟进：** 增加 ESLint 规则禁止在鉴权上下文中用 `||` 合并 truthy 判断；审查其他中间件是否有类似短路
- **建议验证：** 中间件单测三种场景全绿；对受保护路由发起无凭证请求返回 401
- **建议补充说明：** PR 必须说明认证/授权拆分逻辑，并确认无其他中间件存在相同短路模式

## 变更概览

- **修改：** 安全/鉴权 - `auth` 中间件改为提前返回模式，但引入短路求值 bug
- **新增：** 无
- **删除：** 无
- **行为变化：** 变化前，未认证请求被拒绝（401）；变化后，未认证请求被放行（200）——这是回归性安全漏洞

## 风险摘要

| 维度 | 结论 | 依据 |
|---|---|---|
| 正确性 | 有风险 | 对已认证用户行为正确，但未认证路径行为错误 |
| 安全与隐私 | 有风险 | 认证绕过，未认证请求可访问受保护路由与 PII |
| 数据与迁移 | 不涉及 | 不涉及 schema 或迁移 |
| 性能与扩展性 | 无明显风险 | 纯逻辑判断 |
| 兼容性 | 有风险 | 行为从 401 退化为 200，破坏 API 契约 |
| 可观测性与回滚 | 不足 | 无鉴权失败日志/告警，无法及时发现绕过 |
| 测试覆盖 | 有缺口 | 无 401/403 场景测试，否则应能捕获该回归 |

## 影响范围

- **直接影响：** `auth` 中间件及全部挂载它的受保护路由
- **间接影响：** 依赖该中间件的所有业务接口、用户数据与管理功能；任何下游读取受保护资源的服务
- **需要领域确认：** 安全 - 确认是否有 WAF 或网关层鉴权可作为补充防线；非阻塞修复本身（必须拆分判断），但影响后续加固范围

## 回归风险

**等级：** 🔴 高  
**原因：** 认证绕过位于全局前置中间件，可被任意未认证请求触达，单次部署即暴露全部受保护资源  
**最小验证闭环：** 拆分认证/授权判断，补 401/403/200 三场景测试，对受保护路由发起无凭证请求确认返回 401

<!-- review-meta
verdict: DO_NOT_COMMIT
blockers: 1
warnings: 0
test_gaps: 0
review_limits: 0
risk: high
scope: full
template: default
-->
```

## English Tiny Diff Example

Scenario: a one-line typo fix in the README install command (`npm intsall` → `npm install`). A trivial, docs-only change — the canonical Tiny Diff case.

```markdown
# Pre-Commit Review

**VERDICT:** SAFE_TO_COMMIT
**Conclusion:** Safe to commit — a docs-only typo fix in the README install command, no runtime risk.
**Diff source:** staged diff via helper script (`scripts/collect_diff_context.sh --source staged`), matching `origin/main`
**Review scope:** full review - the single changed hunk in `README.md` was inspected
**Change scale:** 1 file, +1 / -1
**Tiny Diff applicability:** applicable - under 5 changed lines, no runtime code, no priority findings

- **Change:** Corrected `npm intsall` to `npm install` in the README quickstart section.
- **Logic:** No runtime behavior change — the edit touches only prose in a Markdown file; no conditionals, handlers, API calls, or style classes affected.
- **Impact scope:** Direct impact: readers following the install instructions. No indirect impact — confirmed the README is not consumed by the build or any codegen step.
- **Risk:** 🟢 Low - documentation-only change; affects no runtime code or build chain.
- **Test:** No tests apply — static prose only; non-blocking. Optionally verify the install command renders correctly on the docs site.
- **Before commit:** None.
```

## Chinese Tiny Diff Example

```markdown
# 提交前审查

**VERDICT:** SAFE_TO_COMMIT
**结论：** 可以提交；这是文档更新，无行为风险。
**差异来源：** 暂存区差异，基于 `git diff --cached` 提取
**审查范围：** 完整审查 - 已审查所有可访问文本 diff；未运行测试
**变更规模：** 1 个文件, +2 行 / -2 行
**Tiny Diff 适用性：** 适用

- **变更：** 更新 README 中的安装说明措辞。
- **逻辑：** 无运行时行为变化；仅修正文案拼写，未改变条件判断、事件处理、API 调用或样式类名。
- **影响范围：** 直接影响只涉及文档阅读者；无间接影响，已确认不对应用打包产生影响
- **风险：** 🟢 低 - 文档变更不影响任何运行时代码或构建链
- **测试：** 未看到新增测试；由于仅修改静态文案，不影响提交决策，建议手动确认页面文案渲染正常
- **提交前：** 无
```

## Chinese Partial Review Example

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

## 变更概览

- **修改：** 调整结账页优惠券校验逻辑，从前端本地校验改为依赖服务端返回的折扣状态
- **新增：** 新增优惠券过期场景的单元测试用例
- **删除：** 删除旧的本地正则校验逻辑，用户输入格式现在完全由接口响应决定
- **行为变化：** 变化前，过期优惠券在前端输入时立即报错；变化后，提交到服务端后根据接口响应展示错误

## 风险摘要

| 维度 | 结论 | 依据 |
|---|---|---|
| 正确性 | 通过 | 核心组件重构逻辑通过了单测验证 |
| 安全与隐私 | 无明显风险 | 纯 UI 与视觉逻辑 |
| 数据与迁移 | 不涉及 | 不涉及数据库 Schema 变更 |
| 性能与扩展性 | 无明显风险 | 纯 UI 渲染，无昂贵计算或渲染瓶颈 |
| 兼容性 | 无破坏 | 组件 API 保持一致 |
| 可观测性与回滚 | 不足 | 未确认是否接入错误监控与灰度发布，视觉回归发生时缺乏可观测信号与回滚手段 |
| 测试覆盖 | 有缺口 | 快照变更未逐行审查 |

## 影响范围

- **直接影响：** 前端主布局和样式组件
- **间接影响：** 消费该公共组件的其他页面
- **需要领域确认：** 无

## 回归风险

**等级：** 🟡 中  
**原因：** 大量生成的测试快照未能逐行审查，可能存在意外视觉变化  
**最小验证闭环：** 本地运行 Storybook 检查受影响的 UI 组件

<!-- review-meta
verdict: SAFE_TO_COMMIT_WITH_NOTES
blockers: 0
warnings: 0
test_gaps: 0
review_limits: 1
risk: medium
scope: partial
template: default
-->
```

## English Visual Review Example

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
| Code Quality & Consistency | Pass | New `variant`/`size` props reuse existing design tokens (`--color-danger`, `--space-sm`); no hardcoded values introduced | Consistent with the token system |
| Layout & Responsive | Under-verified | `size="sm"` reduces padding via `clamp()`; desktop looks correct, but mobile wrapping not exercised | Verify at 320px before release |
| Interactive States | Issue | `:hover` on danger variant drops opacity to 0.6 with no `:focus-visible` ring change; keyboard users lose the focus indicator on hover | Add a distinct `:focus-visible` outline that persists on hover |
| Accessibility | Issue | Danger variant uses `oklch(58% 0.22 25)` text on `oklch(95% 0.02 25)` bg = 4.1:1, below the 4.5:1 AA target for normal text | Darken the danger text token or increase size/weight to meet AA |
| Text & Localization | Pass | Button label uses existing `children`, no new copy; no truncation risk for short labels | No localization impact |
| Regression Risk | Medium | `<Button>` is consumed by 12+ pages; the focus-ring regression affects all of them | Snapshot tests on 3 high-traffic pages before merge |

## Priority Findings

1. ⚠️ `src/components/Button/button.css:42` - Danger variant fails WCAG AA contrast on focus
   - Evidence: `:hover { opacity: 0.6 }` with no compensating `:focus-visible` rule; computed contrast 4.1:1
   - Impact: Keyboard and low-vision users lose the focus indicator and readable text on the danger action across all 12 consuming pages
   - Fix: Add `:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px }` and raise danger text to `oklch(52% 0.22 25)` for ≥4.5:1
   - Verification: Run `axe-core` against the Storybook danger button in both themes; expect 0 contrast violations
   - Confidence: High - contrast ratio computed directly from the token values
   - Blocking reason: none (non-blocking accessibility note, but recommend fixing before shipping to production)

2. 🧪 `src/components/Button/Button.tsx` - No visual regression snapshot added for the new variants
   - Evidence: Diff adds `variant`/`size` props but no snapshot or Storybook story for `danger`/`sm`
   - Impact: Future restyles can silently shift the danger or small variants without a regression net
   - Fix: Add Storybook stories for `danger` × `{md,sm}` × `{default,hover,focus,disabled}` and a snapshot baseline
   - Verification: `npm run test-storybook -- --include Button` passes; chromatic diff is reviewed
   - Confidence: Medium - depends on whether snapshots are the team's regression convention

## Commit Guidance

- **Required before commit:** None (the contrast issue is recommended but not blocking per project a11y policy)
- **Suggested verification:** `axe-core` scan on the danger variant in light + dark; visual diff at 320px and 1440px
- **Suggested documentation:** PR screenshots of all `variant × size` combos in both themes; Storybook link in the PR body

## Risk Detail

- **User visible changes:** Danger button has a new focus/hover treatment; `size="sm"` tightens padding
- **Affected paths:** All 12 pages importing `<Button>`
- **Uncovered states:** mobile (<375px), RTL, loading
- **Regression risk:** 🟡 Medium - shared component, a11y regression affects many surfaces

<!-- review-meta
verdict: SAFE_TO_COMMIT_WITH_NOTES
blockers: 0
warnings: 1
test_gaps: 1
review_limits: 1
risk: medium
scope: full
template: visual
-->
```

## Chinese Visual Review Example

```markdown
# 提交前审查

> ⚠️ **部分审查:** 受限于未运行构建，无法验证 `variant` 与 `size` 之外更完整的视觉/交互状态。未覆盖项列于"未审查变更"中。

**VERDICT:** SAFE_TO_COMMIT_WITH_NOTES
**结论：** 可以提交 `<Button>` 组件的 variant/size 重构，但提交前请在 Storybook 验证 danger 变体的焦点环对比度与暗色模式 hover 态。
**差异来源：** 暂存区差异，经 helper 脚本提取，匹配 `origin/main`
**审查范围：** 完整审查 - 已审查 `Button.tsx` 与 `button.css` 的全部 hunk；diff 之外的运行时/样式态属于盲点，不构成部分审查
**变更规模：** 2 个文件, +34 行 / -12 行；无生成或二进制资产
**视觉范围：** 已审查 `<Button>` 组件的桌面端浅色 + 暗色主题、`variant=primary|danger` 与 `size=md|sm`、hover/focus/disabled 态；未覆盖：移动端 viewport、RTL、loading 态
**未审查变更：** 移动端（<375px）与 RTL 布局 - 本次未验证；可能隐藏换行/溢出。不阻塞提交，发布前在 Storybook 验证。

## 视觉审查矩阵

| 领域 | 信号 | 依据 | 结论 |
|---|---|---|---|
| 代码质量与设计一致性 | 通过 | 新增 `variant`/`size` 复用既有 design token（`--color-danger`、`--space-sm`），未引入硬编码值 | 与 token 体系一致 |
| 布局与响应式 | 未验证 | `size="sm"` 经 `clamp()` 收紧内边距；桌面端正确，但移动端换行未验证 | 发布前在 320px 验证 |
| 交互状态 | 问题 | danger 变体 `:hover` 将透明度降至 0.6 且未改 `:focus-visible` 焦点环；键盘用户在 hover 时丢失焦点指示 | 增加在 hover 下仍可见的 `:focus-visible` 描边 |
| 可访问性 | 问题 | danger 变体文字 `oklch(58% 0.22 25)` 配 `oklch(95% 0.02 25)` 背景 = 4.1:1，低于正文 4.5:1 的 AA 目标 | 加深 danger 文字 token 或增大字号/字重以达 AA |
| 文案与本地化 | 通过 | 按钮文案复用既有 `children`，无新增文案；短文案无截断风险 | 无本地化影响 |
| 回归风险 | 中 | `<Button>` 被 12+ 页面消费；焦点环回归会波及全部页面 | 合并前对 3 个高频页面做快照测试 |

## 重点发现

1. ⚠️ `src/components/Button/button.css:42` - danger 变体在 focus 态未达 WCAG AA 对比度
   - 证据：`:hover { opacity: 0.6 }` 且无补偿性的 `:focus-visible` 规则；计算对比度 4.1:1
   - 影响：键盘用户与低视力用户在全部 12 个消费页面的 danger 操作上丢失焦点指示与可读文字
   - 修复：新增 `:focus-visible { outline: 2px solid var(--color-focus); outline-offset: 2px }`，并将 danger 文字提升至 `oklch(52% 0.22 25)` 以达 ≥4.5:1
   - 验证：在浅色与暗色两种主题下对 Storybook 的 danger 按钮跑 `axe-core`；预期对比度违规为 0
   - 置信度：高 - 对比度由 token 值直接计算得出
   - 阻塞原因：无（非阻塞的可访问性提醒，但建议上线生产前修复）

2. 🧪 `src/components/Button/Button.tsx` - 新增变体未补充视觉回归快照
   - 证据：diff 新增 `variant`/`size`，但未为 `danger`/`sm` 补充快照或 Storybook story
   - 影响：未来调样可能静默偏移 danger 或 small 变体而无回归网
   - 修复：为 `danger` × `{md,sm}` × `{default,hover,focus,disabled}` 补充 Storybook story 与快照基线
   - 验证：`npm run test-storybook -- --include Button` 通过；chromatic 差异已评审
   - 置信度：中 - 取决于团队是否以快照作为回归约定

## 提交建议

- **提交前必须处理：** 无（对比度问题建议修复，但按项目 a11y 策略不阻塞）
- **建议验证：** 在浅色 + 暗色下对 danger 变体跑 `axe-core`；在 320px 与 1440px 做视觉差异检查
- **建议补充材料：** PR 附上两种主题下全部 `variant × size` 组合的截图；PR 正文附 Storybook 链接

## 风险细节

- **用户可见变化：** danger 按钮新增 focus/hover 处理；`size="sm"` 收紧内边距
- **受影响路径：** 全部 12 个导入 `<Button>` 的页面
- **未覆盖状态：** 移动端（<375px）、RTL、loading
- **回归风险：** 🟡 中 - 共享组件，a11y 回归波及面广

<!-- review-meta
verdict: SAFE_TO_COMMIT_WITH_NOTES
blockers: 0
warnings: 1
test_gaps: 1
review_limits: 1
risk: medium
scope: full
template: visual
-->
```
