# pre-commit-review

[English](./README.md) | [简体中文](./README.zh-CN.md)

`pre-commit-review` 是一个可复用的 skill 包，用于在提交、推送或创建 Pull Request 之前审查 Git diff。

它面向 Codex、Claude 等 agent/skill 工作流，目标是提供一个结构化、可重复执行的提交前质量门，而不是临时性的 diff 摘要。

## 可用语言

- English: `README.md`
- 简体中文: `README.zh-CN.md`

不同语言版本应尽量保持功能和信息一致。更新其中一个版本时，最好同时更新其他版本。

## 功能概览

- 按优先级审查最合适的 diff 来源：
  - 用户显式提供的 diff
  - staged 变更
  - unstaged 变更
  - 当前分支相对 base 分支的差异
- 产出统一的审查结构，重点覆盖：
  - 改了什么
  - 代码质量问题
  - 变更意图
  - 逻辑变化
  - 影响范围
  - 回归风险
  - 性能与成本影响（仅在热路径、查询、循环或网络/IO 调用时）
- 返回清晰的结论：
  - `SAFE_TO_COMMIT`
  - `SAFE_TO_COMMIT_WITH_NOTES`
  - `DO_NOT_COMMIT`
- 使用只读辅助脚本收集本地 Git 上下文，不修改仓库内容

## 为什么有这个仓库

这个仓库不是应用或框架，而是一个小而可移植的 skill 包，可以：

- 作为独立开源仓库发布
- 复制到现有 skills 集合中
- 适配到需要“提交前审查”能力的本地 agent 工具链里

## 仓库结构

```text
.
├── install.sh
├── SKILL.md
├── agents/
│   └── openai.yaml
├── collect-diff-context-cli/
│   ├── Cargo.toml
│   └── src/
├── docs/
│   └── superpowers/
├── references/
├── scripts/
│   ├── bin/
│   ├── build_all_binaries.sh
│   ├── build_with_docker.sh
│   ├── collect_diff_context.sh
│   ├── collect_diff_context.legacy.sh
│   └── validate_schemas.py
├── tests/
│   ├── lib/
│   ├── collect_diff_context_test.sh
│   ├── full_review_workflow_test.sh
│   ├── helper_shadow_mode_test.sh
│   ├── install_agent_matrix_test.sh
│   ├── install_smoke_test.sh
│   ├── parity_assets_test.sh
│   ├── parity_golden_test.sh
│   └── skill_contract_test.sh
└── evals/
    ├── output/
    ├── taxonomy/
    ├── eval_contract_test.sh
    ├── readme_surface_test.sh
    ├── readme_host_entrypoints_test.sh
    ├── output-eval.json
    ├── trigger-eval.json
    ├── output_eval_runner.sh
    ├── output_eval_runner_test.sh
    ├── output_eval_codex_runner.sh
    ├── output_eval_claude_runner.sh
    ├── output_eval_codex_case.sh
    ├── output_eval_claude_case.sh
    └── output_eval_host_wrappers_test.sh
```

### `references/`

由 `SKILL.md` 按需加载，references 现在按职责分层：

| 层级 | 文件 | 加载时机 | 用途 |
|------|------|----------|------|
| `decision/` | `verdict-rules.md`、`risk-taxonomy.md`、`finding-verification.md` | 所有常规审查；强结论进入报告前额外执行 finding verification | verdict 选择、阻塞阈值、finding 标记、统计口径、证据约束与高影响结论验证 |
| `rendering/` | `output-en.md`、`output-zh.md`、`visual-output.md`、`review-meta.md` | 生成输出时 | 中英文审查骨架、可选视觉化呈现指导，以及机器可读元数据 |
| `advanced/` | `coverage-led-review.md`、`visual-review-rules.md`、`grading-compat.md` | 仅复杂工作流 | coverage-led 审查流程、UI/视觉审查规则，以及评测兼容精确术语 |
| `examples/` | `default-tiny-en.md`、`default-tiny-zh.md`、`complex-visual-and-coverage.md` | 仅在需要校准结构时 | 用于对齐结构与语气的具体示例，不重新定义规则 |

日常 Default/Tiny 审查刻意不加载 `examples/` 层，除非确实需要结构校准，以避免 token 膨胀并保持常规运行稳定。

### `SKILL.md`

定义 skill 本身，包括：

- 何时触发
- 如何解析 diff 来源
- 如何处理大体积 diff
- 审查必须覆盖哪些维度
- 输出模板与 verdict 规则

### `scripts/collect_diff_context.sh`

这是一个只读辅助脚本，用于为审查流程收集本地仓库上下文。它会：

- 判断当前目录是否是 Git 仓库
- 在存在 staged 变更时优先使用 staged diff
- 在没有 staged 时回退到 unstaged 或 branch-vs-base 比较
- 输出 diff 统计、文件列表和状态信息
- 标识截断状态、基于路径和内容的高风险候选文件、疑似生成文件、lockfile 和高 churn 文件
- 输出 Review Manifest 和 Review Groups，用于 coverage-led commit-readiness 流程
- 将 rename、delete、binary、mode-only 和 submodule 指针更新记录为 manifest units
- 输出 Review Plan JSON，便于 reducer 自动化消费，避免解析 Markdown 表
- 对超过硬预算的 review group 输出 Split Suggestions
- 输出 Split Unit Diff Preview，用于 hunk 级审查
- 输出 Coverage Ledger Template，列出待审查单元
- 输出 Group Review Result 模板，便于 reducer 合并 group findings
- 输出 Reducer State Snapshot Template，用于长流程多轮审查
- 输出 Coverage Validation Checklist，用于 reducer preflight
- 输出 Full Review Execution Plan，提供有序 split/review 步骤
- 输出 Group Review Work Packets，供串行或委派 group review 使用
- 输出 Reducer Finalization Template，用于最终综合门禁
- 输出 best-effort Dependency Summary，用于跨文件综合
- 根据项目提供的只读 grep 模式输出有界 Semantic Context Queries
- 为大体积或被截断的 diff 输出建议审查队列
- 在 diff 过大时安全截断输出

它不会执行 fetch、stage、reset、install，也不会修改任何文件。

默认 diff 输出预算是 200KB。可通过 `PRE_COMMIT_REVIEW_MAX_DIFF_BYTES` 覆盖；当当前对话上下文已经很大时应调低，只有在确认输出完整 diff 安全时才设为 `0`。

Review group 预算默认目标值为 120KB，硬上限为 160KB。可通过 `PRE_COMMIT_REVIEW_GROUP_TARGET_BYTES` 与 `PRE_COMMIT_REVIEW_GROUP_HARD_BYTES` 覆盖；超过硬上限的 group 会标记为 `split-required`。

### 灰度发布与多实现控制（Rollout & Multi-Implementation Controls）
入口包装脚本 `scripts/collect_diff_context.sh` 支持多种运行模式，以确保版本过渡期的安全：
- `PRE_COMMIT_REVIEW_HELPER_IMPL`: 指定底层调用的辅助脚本实现模式。
  - `rust` (默认值): 优先执行编译后的 Rust CLI 二进制程序。如果执行失败，会在 `stderr` 打印警告，并**自动无缝降级执行**旧版 Shell 脚本 `collect_diff_context.legacy.sh`。
  - `legacy` 或 `shell`: 强制直接运行旧版 Shell 脚本。
  - `shadow`: 双路执行模式。同时运行旧版 Shell 脚本和 Rust 二进制程序，比对它们的标准输出，将差异记录至 `/tmp/collect_diff_context_shadow_diff.log` 中。此模式下返回旧版 Shell 的结果以绝对保障生产安全。
- `PRE_COMMIT_REVIEW_SHADOW_MODE`: 设为 `1` 时会强制开启上述 `shadow` 双路比对模式，即使 `PRE_COMMIT_REVIEW_HELPER_IMPL` 被显式设为 `legacy` 或 `shell` 也一样。
- `PRE_COMMIT_REVIEW_DISABLE_FALLBACK`: 设为 `1` 时禁用 Rust 失败降级机制，直接透传 Rust 程序的异常和退出码（用于测试与 CI）。

当全局 diff 被截断时，可用 `scripts/collect_diff_context.sh --source <staged|unstaged|branch> --group <group_id>` 只输出一个未超硬预算 review group 的 diff。需要更窄上下文或 group 已拆分时，用 `--path <path>` 做文件级补取。helper 输出的 `context_command` 会包含 `--source`，确保后续取上下文时仍固定在原始 diff source；`split-required` group 必须通过 split suggestions 审查，不能作为一个整体 group 审查。

项目级风险提示可以放在 `.pre-commit-review/risk-paths` 和 `.pre-commit-review/risk-content`。每个非空、非注释行都是一个扩展正则表达式；匹配项只会提升到 high-risk 审查顺序，不会改变覆盖要求。

项目级语义上下文提示可以放在 `.pre-commit-review/context-queries`。每个非空、非注释行都是一个扩展正则表达式，只会通过有界、只读的 `git grep` 执行；匹配结果可辅助依赖或调用方检查，但永远不能满足审查覆盖。

Review-planning 表和 `Dependency Summary` 使用 TSV，因为路径、命令和依赖详情中可能包含逗号。

Reducer 和 subagent 自动化应优先使用 `Review Plan JSON`、`Reducer State Snapshot Template` 和 JSONL section；TSV 表主要用于人工快速浏览。

### `tests/`

确定性 shell 测试，不依赖模型。`skill_contract_test.sh` 固化 `SKILL.md` 与 `references/` 之间的跨文档契约（禁止的占位符、必需的标签、不可翻译的 `VERDICT` 字段）。`collect_diff_context_test.sh` 和 `full_review_workflow_test.sh` 针对临时真实 Git 仓库测试辅助脚本。`parity_golden_test.sh` 复用共享 parity 夹具和专用 normalize 脚本，确保 legacy 与 Rust 的比对结果稳定。`install_smoke_test.sh` 和 `install_agent_matrix_test.sh` 在 copy/link/dry-run 模式和受支持的 agent 矩阵上验证安装器。它们全部只需 `bash` 和 `jq`，从不调用模型，可在 CI 中安全运行。

### `evals/`

基于 LLM 的评估 harness 现在按职责分层：

- `trigger-eval.json` 负责 skill 触发行为评估
- `output-eval.json` 保留为核心输出场景的兼容总入口
- `evals/output/routine-output-eval.json`、`advanced-output-eval.json`、`visual-output-eval.json`、`localization-output-eval.json` 将输出评测拆分为 routine、复杂、视觉和本地化四套矩阵
- `evals/taxonomy/marker-eval.json` 独立承载 `🔒`、`❌`、`⚠️`、`🧪`、`👁️`、`📈`、`🧭` 的 finding marker 与统计口径预期

执行入口也做了分层：

- `output_eval_runner.sh` 针对任意单个 eval 文件准备真实本地 fixture，可选调用外部模型 runner，并按期望 verdict 与必含短语对保存响应评分
- `--eval-file` 可让 `output_eval_runner.sh` 指向任意单个分层 output eval JSON，例如 `evals/output/visual-output-eval.json`。
- `run_layered_output_evals.sh` 端到端执行 layered output eval 矩阵，覆盖 routine、advanced、visual 和 localization 四套 eval 文件
- `run_marker_eval_checks.sh` 校验 marker taxonomy 覆盖，并汇总 blocking / non-blocking case 数量
- `output_eval_codex_case.sh` 和 `output_eval_claude_case.sh` 每个宿主执行单个 eval case
- `output_eval_codex_runner.sh` 和 `output_eval_claude_runner.sh` 是宿主专用薄封装，会把当前仓库链接到 fixture 的 project-local skill 目录（Codex 用 `.agents/skills`，Claude Code 用 `.claude/skills`），再用适合各自宿主的非交互命令委托给 `output_eval_runner.sh`
- `output_eval_runner_test.sh` 是 fixture 准备与评分逻辑的确定性自测
- `output_eval_host_wrappers_test.sh` 用 mock Codex/Claude 二进制验证这些 wrapper，确保宿主命令模板回归时不消耗真实模型调用
- `run_helper_gateway_probe.sh` 是一个 real-host stage，会对 bundled helper 与选定的直接 Git 命令做日志探针；如果 host 在尝试 `scripts/collect_diff_context.sh` 之前就检查 Git diff 来源，则判定失败
- `readme_surface_test.sh` 守护 README 面向外部暴露的 public surface，确保文档里的 contract gate 与入口清单保持一致
- `readme_host_entrypoints_test.sh` 固化分层 `Host Entrypoints` 文档，确保 README 持续以 `Primary`、`Analysis`、`Stage` 和 `Internal / Repo-wide` 暴露 host lane surface
- `eval_contract_test.sh` 是 repo 级门禁，统一守护 trigger eval、layered output eval、marker taxonomy 资产以及 host lane contract surface

### Host Entrypoints

对于 host lane 工作流，可按下面的层级使用这些脚本：

- `Primary`: `evals/run_host_readiness_pipeline.sh`、`evals/run_cross_host_readiness.sh`
- 默认入口，用于执行端到端单 host 或跨 host 验证
- `Primary / Real Host Smoke`: `evals/run_real_host_smoke.sh`、`.github/workflows/real-host-smoke.yml`
- 当你需要一个稳定入口去跑真实、已认证 host 的 smoke 验证并收集产物时，使用这两个入口
- `Primary / Output Matrix`: `evals/run_layered_output_evals.sh`、`evals/run_marker_eval_checks.sh`
- 用于执行分层 output eval surface 与 marker taxonomy 检查，无需手工逐个挑选 eval 资产
- `Analysis`: `evals/analyze_host_readiness_diff.sh`
- 用于对比 cross-host readiness 输出，而不必重新跑每个阶段
- `Stage`: `evals/check_host_availability.sh`、`evals/run_helper_gateway_probe.sh`、`evals/run_layered_host_evals.sh`、`evals/host_contract_subset.sh`
- 当你只想调试或单独运行某一层 host 边界时使用
- `Internal / Repo-wide`: `evals/eval_contract_test.sh`、host `*_test.sh`、`evals/host_failure_taxonomy.sh`
- 这些是重要的内部或仓库级 surface，不是普通用户入口
- `Stage reports`: `check_host_availability.sh`、`run_helper_gateway_probe.sh`、`run_layered_host_evals.sh` 和 `host_contract_subset.sh` 都可以输出 `host-stage-report/v1`
- `Pipeline report`: `run_host_readiness_pipeline.sh` 输出 `host-readiness-report/v1`
- `Cross-host 与 diff reports`: `run_cross_host_readiness.sh` 输出 `cross-host-readiness-report/v1`，`analyze_host_readiness_diff.sh` 输出 `host-readiness-diff-report/v1`

### `agents/openai.yaml`

为通过 agent 注册表暴露 skill 的环境提供轻量级元信息。

### `install.sh`

把这个 skill 包安装到受支持 AI 编程 agent 的 skills 目录。

## 快速安装

在克隆后的仓库目录中，可以为任意受支持 agent 执行全局安装：

```bash
./install.sh --agent codex
./install.sh --agent claude-code
./install.sh --agent gemini-cli
./install.sh --agent kiro-cli
```

列出所有受支持的 agent id 及其 project/global 路径：

```bash
./install.sh --list-agents
```

默认目录：

- 全局安装使用 `--list-agents` 中对应 agent 的 global path
- 项目安装使用 `--list-agents` 中对应 agent 的 project path
- `--dir PATH` 会覆盖上述默认路径
- `AGENT_SKILLS_DIR` 会覆盖所有 agent 的全局默认路径
- 也支持已有集成的专用覆盖变量：`CODEX_SKILLS_DIR`、`CLAUDE_SKILLS_DIR`、`GEMINI_SKILLS_DIR`、`KIRO_SKILLS_DIR`、`CODEX_HOME`
- 继续支持兼容别名：`claude`、`gemini`、`kiro`

常用参数：

- `--copy` 把 skill 复制到目标目录，默认就是这个模式
- `--link` 把当前仓库以符号链接形式安装过去，适合本地开发
- `--project` 安装到该 agent 的项目级 skills 目录
- `--dir PATH` 手动指定目标 skills 目录
- `--force` 覆盖一个并非由当前安装器管理的同名目标
- `--dry-run` 只打印将执行的动作，不真正修改文件

示例：

```bash
./install.sh --agent cursor --project
./install.sh --agent windsurf --link --project
./install.sh --agent github-copilot --dry-run
./install.sh kiro --dir .kiro/skills
```

## 工作方式

skill 按以下顺序解析审查输入：

1. 用户明确提供的 diff
2. 当前仓库中的 staged 变更
3. 如果没有 staged，则使用 unstaged 变更
4. 当前分支与检测到的 base 分支进行比较
5. 用户提供代码但没有 before/after diff
6. 如果既没有可用 diff，也没有可审查代码，则提示用户先 stage 变更或直接提供 diff

如果用户只提供代码而没有 before/after diff，skill 会：

- 执行静态的提交前风格审查
- 将审查来源标记为用户提供的代码
- 将本次审查视为部分审查
- 除非用户明确展示了先前行为，否则不推断历史行为

当本地仓库可访问且用户没有显式提供审查材料时，工作流会先尝试运行 `scripts/collect_diff_context.sh`。该路径应相对已安装的 `pre-commit-review` skill 包目录解析，也就是包含 `SKILL.md` 的目录，而不是相对用户项目根目录解析。

helper 是以下信息的事实来源：

- diff 来源
- 审查边界
- 变更文件统计
- staged 与 unstaged 的说明
- untracked 文件警告

只有在 helper 在解析后的路径不可用、返回非零退出码、当前 host 无法执行，或用户已经显式提供了审查材料时，才回退到直接 Git 检查。

## 其他集成方式

### 作为独立仓库使用

将本仓库克隆或复制到你的 agent 运行时读取自定义 skills 的位置。

示例目录结构：

```text
your-skills/
└── pre-commit-review/
    ├── SKILL.md
    ├── agents/
    ├── references/
    └── scripts/
```

随后根据你的 agent 平台的 skill 加载机制，注册或暴露该 skill。

### 合并到现有 skills 集合

如果你已经维护了一个更大的 skills 仓库，可以把当前目录作为一个独立 skill 包复制进去，并保留相对路径：

- `SKILL.md`
- `scripts/collect_diff_context.sh`
- `references/`
- `agents/openai.yaml`

辅助脚本会在 skill 说明中被引用，因此除非你同步修改引用路径，否则应保持目录结构不变。

## 审查输出

预期输出是结论优先、可快速扫描的提交前审查结果，包含：

- verdict 与一句话结论
- diff 来源
- 审查边界
- 变更文件统计
- 优先问题与修复建议
- 必要的风险与测试建议

默认输出会优先回答三个问题：

- 现在能不能提交
- 提交前必须先改什么
- 接下来最该测什么

只有在确实增加信息量时，才会附加更详细的意图、前后逻辑对比或补充分析。

最终 verdict 的含义：

- `SAFE_TO_COMMIT`：在已审查范围内，看起来现在可以安全提交
- `SAFE_TO_COMMIT_WITH_NOTES`：现在可以提交，但存在后续建议或审查边界限制
- `DO_NOT_COMMIT`：发现阻塞问题，不应按当前状态提交

## 安全特性

这个包的设计倾向保守：

- 当本地仓库不可访问时，不会假装看到了本地变更
- 会区分 staged 和 unstaged 的审查范围，并在 unstaged 变更也触及已 staged 的文件时标记
- 会提醒 `git diff` 中未包含的 untracked 文件
- 永不复现密钥值；标记出的凭据以 redacted 预览形式展示并建议 rotate
- 会把大 diff 或截断视为拆分工作、按需取更小上下文的信号，而不是跳过实质性单元的理由
- 只把部分 triage 用作 advisory fallback；高风险单元未审查时会阻塞提交就绪性结论
- 支持 coverage-led commit-readiness；只有每个 manifest unit 都被记录覆盖后，才能声称完整审查
- 会把长流程 reducer state 保持为紧凑、显式的状态对象，而不是依赖隐式对话记忆
- 会把语义上下文查询当成有界只读提示，而不是任意 shell command 或覆盖替代品

## 限制

- 该仓库不包含加载或执行 skill 的运行时本身
- 仓库自带安装脚本，覆盖 Codex、Claude Code、Gemini CLI 的常见目录；如果你的本地布局不同，可能仍需要通过 `--dir` 指定目标位置
- 辅助脚本依赖环境中可用的 `git`
- 在 Windows 环境下，辅助脚本与安装器需要类 Unix 环境（如 Git Bash、MSYS2 或 WSL）支持才能正常运行。
- 当前仓库即使脱离 Git 也能作为内容包存在，但本地 diff 收集只有在 Git 仓库内才有效

## 贡献

更适合的贡献方向包括：

- 改进审查启发式规则
- 收紧安全边界
- 优化输出模板
- 增强脚本在不同仓库状态下的健壮性

如果你修改了脚本路径或仓库结构，请同步更新 `SKILL.md`。
如果你修改了对外文档，请尽量保持各本地化 README 版本同步。

### 开发

Shell 脚本（`scripts/*.sh`、`install.sh`、`tests/*.sh`、`evals/*.sh`）在 CI（`.github/workflows/lint.yml`）中由 [shellcheck](https://www.shellcheck.net/) 检查。提交前请在本地安装（macOS 可用 `brew install shellcheck`）并运行 `shellcheck -s bash scripts/*.sh install.sh tests/*.sh evals/*.sh`。

要在本地编译 Rust CLI 二进制，运行 `cargo build --release --manifest-path collect-diff-context-cli/Cargo.toml` 或执行 `scripts/build_all_binaries.sh`。

确定性单元测试套件为 `bash tests/*_test.sh`。eval harness 也附带不调用模型的确定性自测：`bash evals/eval_contract_test.sh`、`bash evals/output_eval_runner_test.sh` 和 `bash evals/output_eval_host_wrappers_test.sh`（也可通过 `for f in evals/*_test.sh; do bash "$f"; done` 执行所有 eval 自测）。基于模型的 runner（`evals/output_eval_codex_runner.sh`、`evals/output_eval_claude_runner.sh`）需要真实的 Codex 或 Claude CLI，不属于 CI。

手动触发的 real-host smoke workflow 位于 `.github/workflows/real-host-smoke.yml`。它面向已经安装并完成认证的 `claude` 与 `codex` CLI 的 self-hosted runner，并委托 `evals/run_real_host_smoke.sh` 执行。

## License

本项目采用 Apache License 2.0。详见 [LICENSE](./LICENSE)。
