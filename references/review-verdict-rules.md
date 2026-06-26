# Review Verdict Rules

This reference file defines the verdict rules, blocker matrix, non-blocking matrix, and output quality gate.

## Verdict 判定规则 / Verdict Rules

### SAFE_TO_COMMIT
- 没有阻塞项；
- 没有高置信度 correctness / security / data / migration / auth / build / runtime 风险；
- 测试缺口不影响提交决策，或已有足够替代验证；
- 未审查范围为无，或未审查内容明显不影响提交风险。
- *English*: No blocking issues. No high-confidence correctness / security / data / migration / auth / build / runtime risks. Test gaps do not affect commit decision, or sufficient alternative validation exists. Unreviewed scope is none or clearly does not affect commit risk.

### SAFE_TO_COMMIT_WITH_NOTES
- 没有阻塞项；
- 存在 non-blocking 提醒、轻微风险、建议验证、文档/可维护性建议、低到中等测试缺口；
- 存在部分审查限制，但限制范围不会合理地改变提交决策；
- 需要提交者知情，但不要求提交前必须修改。
- *English*: No blocking issues. Non-blocking notes, minor risks, suggested verification, documentation/maintainability recommendations, or low-to-medium test gaps exist. Partial review limitations exist but do not reasonably change the commit decision. The committer needs to be aware, but fixes are not required before committing.

### DO_NOT_COMMIT
- 存在任何阻塞项；
- 存在可能导致构建失败、运行时失败、数据损坏、权限绕过、隐私泄露、安全漏洞、不可逆迁移、重大兼容性破坏、发布事故的高置信度问题；
- 关键文件、生成物、迁移、锁文件、安全敏感变更或大范围未审查内容无法审查；
- 测试缺口覆盖高风险逻辑，且没有足够替代验证。
- *English*: Any blocking issue exists. High-confidence issues that could cause build failure, runtime failure, data corruption, privilege escalation/bypass, privacy leak, security vulnerability, irreversible migration, major compatibility breakage, or release incident. Critical files, generated assets, migrations, lock files, security-sensitive changes, or large unreviewed scopes cannot be reviewed. Test gaps cover high-risk logic without sufficient alternative validation.

---

## Blocker Matrix / 阻塞判定矩阵

以下情况默认判定为阻塞项（即 `DO_NOT_COMMIT`），除非有明确证据证明影响极低：
The following conditions are blocked by default unless there is clear evidence of negligible impact:

| 类型 (Type) | 阻塞条件 (Blocking Condition) |
|---|---|
| 构建 (Build) | 代码无法编译、类型检查必然失败、导入缺失、配置破坏构建 |
| 运行时 (Runtime) | 明显空指针、未处理异常、错误调用签名、死循环、资源泄露 |
| 安全 (Security) | 认证/授权绕过、注入、XSS、SSRF、反序列化、凭证泄露、弱随机、路径穿越 |
| 隐私 (Privacy) | 暴露 PII、日志记录敏感信息、错误权限访问用户数据 |
| 数据 (Data) | 破坏数据完整性、不可逆迁移、schema 与代码不兼容、回滚不可行 |
| 依赖 (Dependency) | 引入不可信依赖、许可证风险、锁文件不一致、供应链风险 |
| 性能 (Performance) | 高流量路径引入明显 N+1、无界循环、无界内存、昂贵同步调用 |
| 兼容性 (Compatibility) | 破坏公开 API、事件格式、配置格式、序列化格式或客户端兼容 |
| 发布 (Release) | 缺少必要 feature flag、迁移顺序错误、无法灰度、无法回滚 |
| 测试 (Testing) | 高风险逻辑无测试且无法通过现有覆盖或手动验证降低风险 |
| 审查范围 (Scope) | 关键变更未审查，且可能改变提交决策 |

---

## Non-blocking Matrix / 非阻塞判定矩阵

以下情况通常为非阻塞，除非叠加高风险上下文：
The following conditions are usually non-blocking unless compounded by high-risk contexts:

| 类型 (Type) | 非阻塞条件 (Non-blocking Condition) |
|---|---|
| 可维护性 (Quality) | 命名、注释、局部结构可以更清晰，但不影响运行时行为 |
| 小型重构 (Refactoring) | 行为不变，测试或上下文支持该判断 |
| 文档 (Docs) | 文档可更完整，但不会误导用户或破坏发布 |
| 测试建议 (Tests) | 低风险路径缺少额外测试，但已有足够现有覆盖 |
| 性能提醒 (Performance) | 潜在微小性能影响，不在高频路径 or 大数据路径 |
| 兼容提醒 (Compatibility) | 内部接口变化，调用方已同步更新 |
| 视觉建议 (Visual) | 轻微间距、文案、样式建议，不影响可用性或可访问性 |

---

## Output Quality Gate / 输出质量守门规则

生成审查结果前，检查以下条件 (Check before emitting review results):

1. **Verdict 与重点发现一致 (Verdict consistency)**:
   - 有阻塞项时必须是 `DO_NOT_COMMIT`。
   - 无阻塞项但有提醒、测试缺口或审查限制时，通常是 `SAFE_TO_COMMIT_WITH_NOTES`。
   - `SAFE_TO_COMMIT` 不应包含需要提交前处理的事项。
2. **每个重点发现必须可执行 (Actionable findings)**:
   - 有具体文件或位置；
   - 有证据；
   - 有影响；
   - 有修复；
   - 有验证。
3. **不夸大风险 (Do not exaggerate risks)**:
   - 不把风格建议写成阻塞项；
   - 不把没有证据的猜测写成发现项；
   - 低置信度问题必须标明不确定性。
4. **不隐藏风险 (Do not hide risks)**:
   - 关键未审查范围必须写入“未审查变更”或“审查限制”；
   - 安全、数据、权限、迁移、发布风险不能只写成普通提醒；
   - 测试缺口必须说明是否影响提交决策。
5. **不输出空洞建议 (No empty suggestions)**:
   - 避免“建议加强测试”“注意安全”等泛泛表述；
   - 必须说明测试什么、怎么测、为什么测。
