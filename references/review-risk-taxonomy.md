# Review Risk Taxonomy

This reference file defines severity levels, statistical rules, findings requirements, and evidence constraints.

## Severity Levels / 严重程度标记

Each prioritized finding must be prefixed with exactly one of the following severity markers for scanning:
每个重点发现必须使用且只使用以下标记之一作为前缀：

- `🔒` **Security & Privacy / 安全与隐私**: Credentials, keys, auth bypass, injection, data leak, supply chain, PII exposure. (Default blocking / 默认阻塞)
- `❌` **Correctness & Runtime / 正确性与运行时**: Logic bug, compile/build error, unhandled exception, data corruption, schema incompatibility, broken dependency. (Default blocking / 默认阻塞)
- `⚠️` **Non-blocking Risk & Maintainability / 非阻塞风险与可维护性**: Maintainability concern, style suggestion, minor edge case, unclear naming, missing doc, deprecation notice. (Non-blocking / 非阻塞)
- `🧪` **Test Gap & Verification / 测试缺口与验证**: Missing test coverage on critical paths, lacking automated/manual regression test suite. (Blocking status depends on code risk / 是否阻塞取决于变更风险)
- `👁️` **Review Limitation & Scope / 审查限制与范围**: Unreviewed files, truncated diff, generated/binary assets, missing screenshot, third-party packages, or missing context. (Only blocks when limits can change the verdict / 仅在限制可能改变提交决策时阻塞)
- `📈` **Performance & Scalability / 性能与扩展性**: N+1 queries, resource leak, unbounded loops, high CPU/memory/token cost, database locks. (Blocking status depends on context / 是否阻塞取决于上下文)
- `🧭` **Release, Migration & Ops / 发布、迁移与运维**: Incorrect migration sequence, missing feature flag, rollback hazard, deployment config issues. (Blocking status depends on context / 是否阻塞取决于上下文)

---

## Statistics Rules / 统计规则

Tally counts are calculated based on the primary type of findings:
统计按主类型计算：
- **Blockers / 阻塞项**: Findings marked with `🔒` or `❌`, and any `🧪`, `👁️`, `📈`, `🧭` that are determined to block the commit.
- **Warnings / 提醒**: Non-blocking findings marked with `⚠️`, `📈`, `🧭`, or `👁️`.
- **Test Gaps / 测试缺口**: Any findings marked with `🧪`, regardless of whether they are blocking.
- **Review Limits / 审查限制**: Any findings marked with `👁️`, indicating unreviewed scope.

*Note*: Each finding item can have only one primary marker. For multi-dimensional risks, explain them in the title or impact description.

---

## Finding Structure / 发现项分级

Each priority finding must include:
每个发现项必须包含：
- **File & Line**: Formatted as `` `file:line` `` (or `` `file` `` if exact line is unavailable).
- **Issue Title**: Concise summary of the problem.
- **Evidence**: Short code snippet or context illustrating the issue. Avoid copying large blocks.
- **Impact**: What it can break, leak, corrupt, degrade, or block.
- **Fix**: Actionable remediation steps or alternative implementation.
- **Verification**: Tests, commands, manual validation, or monitoring steps after fix.
- **Confidence**: High, Medium, or Low with a brief explanation.
- **Blocking Reason**: (Required for blockers only) Why this issue must be fixed before committing.

Do not output findings without concrete evidence. Put low-confidence suspicions into "Review Limitations" or "Suggested Verification" instead.

---

## Evidence Rules / 证据规则

Always prioritize direct evidence from the diff. If necessary, use relevant context including:
优先使用 diff 中的直接证据。必要时允许使用相关上下文，包括：
- Upstream caller or downstream callee contracts;
- Configurations, schemas, migrations, lockfiles, generated assets;
- Test files, snapshots, screenshots, runtime logs, CI output;
- PR description, issue tracker, design specifications;
- Existing codebase conventions or security policies.
