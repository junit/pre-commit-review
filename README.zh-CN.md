# pre-commit-review

[![Lint](https://img.shields.io/github/actions/workflow/status/wifibaby4u/pre-commit-review/lint.yml?branch=main&label=lint&logo=github)](https://github.com/wifibaby4u/pre-commit-review/actions/workflows/lint.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](./LICENSE)
[![Shell](https://img.shields.io/badge/shell-bash-4EAA25?logo=gnubash&logoColor=white)](https://www.shellcheck.net/)

[English](./README.md) | [简体中文](./README.zh-CN.md)

`pre-commit-review` 是一个可复用的 skill 包，用于在提交、推送或创建 Pull Request 之前审查 Git diff。通俗地说：**一个可以直接挂到你 AI 编程 agent（Codex、Claude Code、Gemini CLI 或 Kiro）上的提交前审查步骤**。AI "skill" 就是 agent 按需加载的一组指令——装好后，当你让它检查提交前的改动时，它会给出一个结构化的结论，而不是临时性的 diff 摘要。

## 可用语言

- English: `README.md`
- 简体中文: `README.zh-CN.md`

不同语言版本应尽量保持功能和信息一致。更新其中一个版本时，最好同时更新其他版本。

## 目录

**给使用者——装上就能用：**

- [它能抓到什么](#它能抓到什么)
- [输出示例](#输出示例)
- [环境要求](#环境要求)
- [快速安装](#快速安装)
- [如何触发一次审查](#如何触发一次审查)
- [安全特性](#安全特性)
- [限制](#限制)

**给开发者和集成者——改造或扩展：**

- [为什么有这个仓库](#为什么有这个仓库)
- [仓库结构](#仓库结构)
- [内部工作原理](#内部工作原理)
- [其他集成方式](#其他集成方式)
- [审查输出格式](#审查输出格式)
- [贡献](#贡献)
- [License](#license)

## 它能抓到什么

这个审查会检查你的改动，在提交前报告 bug、安全风险和缺失的测试。每发现一个问题，都会给出文件和行号、为什么重要、具体怎么修、以及如何验证。

它会按以下顺序审查最相关的 diff：

1. 你粘贴的 diff 或 patch
2. 你的 staged（已暂存）改动
3. 你的 unstaged（未暂存）改动（当没有 staged 时）
4. 当前分支相对 base 分支的差异
5. 你粘贴的纯代码（无 diff 历史，按部分审查处理）
6. 什么都没有——它会请你暂存改动或粘贴 diff

然后给出三种结论之一：

- `SAFE_TO_COMMIT`——没有阻塞项，可以提交
- `SAFE_TO_COMMIT_WITH_NOTES`——可以提交，但建议处理后续提醒
- `DO_NOT_COMMIT`——发现阻塞项，请先修复

它聚焦于对提交决策真正重要的方面：正确性、安全、数据处理、回归风险，以及——仅在有影响时——热路径、查询、循环或网络/IO 调用上的性能。它绝不修改你的仓库；由一个只读辅助脚本收集 Git 上下文。

## 输出示例

下面是一次附加型 schema 变更的完整默认审查。它展示了 skill 产出的完整结构——含结论头部、执行摘要、重点发现、提交建议、变更概览、风险摘要表、影响范围，以及回归风险等级：

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
```

对于存在阻塞项的改动，结论为 `DO_NOT_COMMIT`，重点发现中会带 `🔒` 标记的阻塞项。对于大 diff，skill 还会补充 coverage-led 段落。视觉与 coverage-led 示例见 [`references/examples/`](./references/examples/)。

## 环境要求

- 一个能加载 skill 的受支持 AI 编程 agent 运行时（Codex、Claude Code、Gemini CLI 或 Kiro）。skill 包本身不附带运行时。
- 本地 diff 收集需要 `PATH` 中存在 `git`。当你直接粘贴 diff 或代码时，无需 git 也能审查。
- 网络访问是可选的。从源码 clone 安装时，`install.sh` 会尝试下载当前平台固定的 Gitleaks `8.30.1`，并同时校验 release archive 与解压后 executable 的 SHA256。自包含 release 包已经附带验证过的二进制。下载被关闭、不可用或失败时，skill 仍会完成安装并继续审查，只是不提供本地密钥打码；不会隐式搜索 `PATH`。
- 运行 `install.sh` 和辅助脚本需要 Unix 兼容 shell。Windows 上请使用 Git Bash、MSYS2 或 WSL。

## 快速安装

在克隆后的仓库目录中，可以为任意受支持 agent 执行全局安装：

```bash
./install.sh --agent codex
./install.sh --agent claude-code
./install.sh --agent gemini-cli
./install.sh --agent kiro-cli
```

以上命令会在安装阶段尝试准备当前平台的固定版本 Gitleaks。这是用户显式触发的安装器行为；Agent 审查流程本身绝不会下载工具。准备失败会产生告警，但不会阻止安装或审查。

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

- `--copy` 只把最小运行时 skill payload 复制到目标目录，默认就是这个模式
- `--link` 把当前仓库以符号链接形式安装过去，适合本地开发
- `--project` 安装到该 agent 的项目级 skills 目录
- `--dir PATH` 手动指定目标 skills 目录
- `--force` 覆盖一个并非由当前安装器管理的同名目标
- `--dry-run` 只打印将执行的动作，不真正修改文件
- `--no-download` 跳过可选的 Gitleaks 下载；没有密钥打码时审查仍可继续
- `--doctor` 在不安装 skill 的情况下诊断 scanner 来源、版本、bundled SHA256、可信配置和 stdin/JSON 能力；打码不可用时会非零退出，但不代表审查被阻塞

示例：

```bash
./install.sh --agent cursor --project
./install.sh --agent windsurf --link --project
./install.sh --agent github-copilot --dry-run
./install.sh kiro --dir .kiro/skills
```

## 如何触发一次审查

根据您的审查场景，您可以在对话中使用以下提示词来触发并引导 AI 执行相应的审查流程。以下是 5 种核心场景、其具体定义与常用提示词：

1. **Staged/Unstaged 变更审查（日常提交前检查）**
   * **场景**：开发者在本地修改了代码，在执行 `git commit` 前希望评估代码变更是否安全。
   * **作用**：检查代码中是否存在语法错误、死锁、敏感凭证泄漏、测试缺失等高危问题。
   * **常用提示词**：
     * *“帮我做个提交前审查。”*
     * *“检查一下我已暂存（staged）的变更，看看是否安全。”*
     * *“审查我本地未提交（unstaged）的代码，检查一下漏洞和隐患。”*
     * *“检查当前修改，有没有敏感凭证泄漏或者缺失单测？”*
2. **Branch vs. Base 分支合并审查（PR 门禁）**
   * **场景**：当前分支开发完毕，准备向主分支（如 `main`、`develop`）提起 Pull Request，需审查该分支与基线分支之间的累计差异。
   * **作用**：对比当前开发分支与目标基线分支（如 `develop`、`main`）之间的差异，模拟 PR 合并前的静态代码门禁。
   * **常用提示词**：
     * *“请帮我审查当前分支相对 develop 分支的变更（PR 审查）”*
     * *“做一次 branch 级别的合并审查，对比 base 分支是 origin/main。”*
     * *“审查当前分支与 master 分支的累计差异，看看适不适合合并。”*
3. **用户提供代码/补丁审查（纯文本 Diff 审查）**
   * **场景**：在远程无 Git 仓库访问权限的沙箱或平台中，用户通过复制粘贴一段 Diff/Patch，让 AI 评估其质量和风险。
   * **作用**：在没有 Git 仓库读取权限，或者需要审查第三方 patch 文件时，手动复制粘贴 diff。
   * **常用提示词**：
     * *“我这里有一段 git diff，请帮我执行 pre-commit 审查：[粘贴 diff 内容]”*
     * *“分析一下这个 patch 补丁是否存在回归风险或安全性问题：\n```diff\n...[在此粘贴 diff 内容]...\n```”*
4. **静态代码审查（无 Diff 的单文件审查）**
   * **场景**：用户直接提供一段写好的代码，要求评估其实现方式。
   * **作用**：在没有 Git 修改历史（没有 before/after 对比）时，仅对新写的单文件或函数做防范性的提交前代码风格和安全策略检查。SKILL 将执行静态审查，但因为没有 Diff 历史，会标记为“部分审查（partial review）”。
   * **常用提示词**：
     * *“我写了一段新代码准备提交，帮我做个提交前静态审查：[粘贴代码内容]”*
     * *“审查以下单文件，当做提交就绪性静态审计（pre-commit-review）：\n```python\n...[在此粘贴代码]...\n```”*
5. **复杂/大型差异审查（Coverage-Led 覆盖引导审查）**
   * **场景**：差异过大或文件过多导致单次模型上下文超限。
   * **作用**：变更文件特别多或差异非常庞大（如重构、版本升级）时，必须防范模型由于上下文截断或注意力分散而遗漏关键文件。系统通过 `collect_diff_context.sh` 自动拆分文件，建立 manifests 列表，逐个 group 审查，最终在 reducer 中汇总并审计覆盖率，确保“没有任何一处修改被漏审”。
   * **常用提示词**：
     * *“当前分支修改非常多，请执行大体积 diff 的 coverage-led 覆盖引导审查。”*
     * *“文件变更很多，请启动 pre-commit 覆盖引导审查，帮我拆分 group 逐步 review，并生成 Review Plan。”*
     * *“分析一下当前分支的大量变更，生成 Review Plan 并按计划逐个模块审计，直到所有单元都覆盖完毕。”*

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
    ├── compare_output_eval_quality.sh
    ├── compare_output_eval_quality_test.sh
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

这是一个只读辅助脚本，用于为审查流程收集本地仓库上下文。它做四件事：

1. **diff 来源解析** —— 判断当前目录是否是 Git 仓库，存在 staged 时优先使用 staged，否则回退到 unstaged 或 branch-vs-base；输出 diff 统计、文件列表、状态、截断状态、基于路径/内容的高风险候选、疑似生成文件、lockfile 和高 churn 文件。rename、delete、binary、mode-only 和 submodule 指针更新都会记录为 manifest units。
2. **有界 control plane** —— 通过 `--control-plane` 输出紧凑 JSON gateway，包含完整 scope 内容指纹、逐单元指纹、有界 units/groups、work order 与可复用命令模板；后续补取支持 `--expect-scope <fingerprint>`，快照过期时 fail closed；指纹和实际审查字节都会禁用外部 diff/textconv driver，确保快照身份与模型检查到的内容保持同一语义。
3. **coverage-led 与测试选择提示** —— 输出 Review Manifest/Groups 以及 reducer 友好的结构化段落（Review Plan JSON、split 建议、ledgers、work packets、finalization 模板）、有界只读 Semantic Context Queries，以及对变更中测试文件的 Test Selection Hints，用于识别常见 JVM/Spring/Quarkus/Micronaut、Maven/Gradle 集成测试命名、JUnit tags、Testcontainers、Docker Compose、WireMock/MockServer、pytest markers、Playwright/Cypress/Node e2e、Go build tags、Rust ignored/integration tests，以及数据库/缓存/消息/搜索服务配置等环境依赖测试。
4. **可选的本地密钥打码** —— 可信 Gitleaks 可用时，先扫描和打码完整的所选 diff，再应用输出字节上限，将命中范围替换为 `[redacted:<rule-id>]` 后复扫，并对 wrapper 捕获的完整 stdout/stderr 做打码。这个顺序能防止已检测到的密钥跨越截断边界时以无法匹配的前缀泄露。scanner 被关闭、不可用、超时或没有返回命中时，审查继续使用原始输出。若 Gitleaks 已返回命中，但本地坐标映射或复核失败，helper 会明确报告 `status: redaction-failed`，而不是把它说成 scanner 不可用；此路径同样继续输出原始内容，不暂扣审查材料。

完整输出段落清单（Coverage Ledger Template、Group Review Work Packets、Reducer State Snapshot 等）见 [`docs/helper-capabilities.md`](./docs/helper-capabilities.md)，供构建 reducer/subagent 自动化的集成者参考。

审查入口不会执行 fetch、stage、reset、install，也不会修改任何文件。用户显式执行安装时，如果当前平台二进制尚未 bundled，`install.sh` 会调用 `scripts/fetch_gitleaks.sh`；该脚本只下载仓库固定的上游 release asset，并同时校验 archive 与解压后 executable 的固定 SHA256。交互式终端默认显示下载进度；输出被宿主捕获时可设置 `PRE_COMMIT_REVIEW_FETCH_PROGRESS=always` 强制显示，或设为 `never` 关闭。`--dry-run` 不会下载，`--no-download` 会跳过这项可选安装行为，Agent 审查期间也绝不会联网安装 Gitleaks。可运行 `./install.sh --doctor` 诊断本地打码是否可用。
它不会运行、改写或跳过测试。Test Selection Hints 只是只读提示，用于选择更聚焦的验证命令，并区分沙箱环境失败和代码失败。`no-known-env-heavy-marker` 并不证明测试是隔离单测，只表示 helper 没匹配到已知的重环境标记。

审查流程首先运行 `scripts/collect_diff_context.sh --control-plane`。这个有界 gateway 不输出 raw diff，且只有 collection-start 与 collection-end 指纹一致时才标记为 authoritative。兼容用的默认输出仍是 plan-first，并可能省略全局 raw diff。`PRE_COMMIT_REVIEW_INLINE_DIFF_BYTES`（默认 `60000`）控制该默认输出何时内联全局 diff。`PRE_COMMIT_REVIEW_MAX_DIFF_BYTES`（默认 `200000`）只控制已经被选择输出的 diff 如何截断；只有在确认完整的已打码 diff 输出安全时才设为 `0`。

即使所选模型标称支持 200K 以上上下文，默认预算仍然有意保持保守。CLI 宿主可能在内容进入模型之前就把大型工具 stdout 持久化或只返回 preview；大段 raw diff 还会增加延迟和多轮 token 成本，并削弱审查焦点。请把默认值视为跨宿主稳定基线，而不是模型上下文上限。

高级 gateway 预算调参：
- `PRE_COMMIT_REVIEW_INLINE_DIFF_BYTES`: 默认 `60000`。私有化部署使用更大上下文模型时可以调高，例如 `150000`；小上下文模型可以调低，例如 `30000`。
- `PRE_COMMIT_REVIEW_MAX_DIFF_BYTES`: 默认 `200000`。限制所有通过 gateway 或后续 context command 显式输出的 diff 大小。
- Prompt caching 与自适应 inline 预算属于部署相关优化。只有在确认宿主不会把大 stdout 隐藏成 preview，且延迟/成本可接受后，才应提高 inline 预算。

Review group 预算默认目标值为 120KB，硬上限为 160KB。可通过 `PRE_COMMIT_REVIEW_GROUP_TARGET_BYTES` 与 `PRE_COMMIT_REVIEW_GROUP_HARD_BYTES` 覆盖；超过硬上限的 group 会标记为 `split-required`。

### 灰度发布与多实现控制（Rollout & Multi-Implementation Controls）
入口包装脚本 `scripts/collect_diff_context.sh` 支持多种运行模式，以确保版本过渡期的安全：
- `PRE_COMMIT_REVIEW_HELPER_IMPL`: 指定底层调用的辅助脚本实现模式。
  - `rust` (默认值): 优先执行编译后的 Rust CLI 二进制程序。收集错误可以降级到旧版 Shell 脚本。密钥扫描错误不会触发特殊 fallback，也不会阻塞输出；所选实现会在报告降级状态后继续输出未打码内容。
  - `legacy` 或 `shell`: 强制直接运行旧版 Shell 脚本。
  - `shadow`: 双路执行模式。同时运行旧版 Shell 脚本和 Rust 二进制程序，比对它们的标准输出，并在不一致时告警。此模式下返回旧版 Shell 的结果以保障生产安全。
- `PRE_COMMIT_REVIEW_SHADOW_MODE`: 设为 `1` 时会强制开启上述 `shadow` 双路比对模式，即使 `PRE_COMMIT_REVIEW_HELPER_IMPL` 被显式设为 `legacy` 或 `shell` 也一样。
- `PRE_COMMIT_REVIEW_SHADOW_DIFF_LOG`: 可选的 shadow mismatch diff 日志路径。默认 shadow mode 不会把 diff 内容写入 `/tmp`。
- `PRE_COMMIT_REVIEW_DISABLE_FALLBACK`: 设为 `1` 时禁用 Rust 失败降级机制，直接透传 Rust 程序的异常和退出码（用于测试与 CI）。
- `PRE_COMMIT_REVIEW_SECRET_SCAN`: 控制可选本地打码：`auto`（默认）在可信 scanner 可用时启用；`off` 跳过扫描并继续输出未打码审查内容。
- `PRE_COMMIT_REVIEW_GITLEAKS_BIN`: 在开发、测试或受控离线环境中显式指定可信 scanner 绝对路径。它必须匹配固定版本并通过 stdin/JSON 能力测试。设置该变量代表用户主动信任此外部程序；否则只接受通过 SHA256 验证的 bundled binary，且绝不搜索 `PATH`。
- `PRE_COMMIT_REVIEW_GITLEAKS_CONFIG`: 开发/测试时显式指定可信 scanner 配置。不要指向被审查仓库提供的配置。
- `PRE_COMMIT_REVIEW_GITLEAKS_TIMEOUT_MS`: 单个 Gitleaks 进程的毫秒级超时。默认值为 `30000`，允许覆盖为 `50` 到 `120000`。超时后 helper 会终止并回收 scanner，报告 `scanner-timeout`，然后在未打码状态下继续审查。
- `PRE_COMMIT_REVIEW_FETCH_PROGRESS`: 控制 Gitleaks 下载进度：`auto`（默认）、`always` 或 `never`。

所有实现模式都使用同一个尽力而为的流打码器。扫描成功时，shadow mismatch 日志基于已打码的 stdout/stderr。`status: unavailable` 表示 scanner 无法运行或未能完成；`status: redaction-failed` 表示 scanner 已返回命中，但 helper 未能应用或复核替换。两种状态都不会暂扣输出，并会明确说明没有完成打码。

当宿主把 helper 输出持久化、只返回 preview 时，可用 `scripts/collect_diff_context.sh --plan-only` 或 `--include-diff never` 重新获取结构化控制面。只有明确需要全局 diff 时才使用 `--include-diff always`；输出仍受 `PRE_COMMIT_REVIEW_MAX_DIFF_BYTES` 限制，在假定内容已打码前必须检查 `## Secret Scan` 状态。

打开控制面后，可用 `scripts/collect_diff_context.sh --source <staged|unstaged|branch> --group <group_id> --expect-scope <fingerprint>` 只输出一个未超硬预算 review group 的 diff。需要更窄上下文或 group 已拆分时，用带同一 fingerprint 的 `--path <path>` 补取。最终 verdict 前必须重跑 `--control-plane`；快照漂移会使旧 ledger 失效，不能把两个版本拼成一次“完整审查”。`split-required` group 必须通过有界 replacement 审查，不能作为一个整体 group 审查。

项目级风险提示可以放在 `.pre-commit-review/risk-paths` 和 `.pre-commit-review/risk-content`。每个非空、非注释行都是一个扩展正则表达式；匹配项只会提升到 high-risk 审查顺序，不会改变覆盖要求。

项目级语义上下文提示可以放在 `.pre-commit-review/context-queries`。每个非空、非注释行都是一个扩展正则表达式，只会通过有界、只读的 `git grep` 执行；匹配结果可辅助依赖或调用方检查，但永远不能满足审查覆盖。

项目级测试选择提示可以放在 `.pre-commit-review/test-hints`。每个非注释行是一条 TSV 规则：

```text
rule_id<TAB>path_regex<TAB>content_regex<TAB>test_kind<TAB>environment_dependency<TAB>confidence<TAB>hint
```

helper 会优先输出第一条路径或内容正则匹配变更测试文件的自定义提示，再回退到内置提示。内置规则覆盖热门跨生态约定，但项目级配置仍应用于本地 profile、命名约定、私有测试框架，以及无法仅从路径/内容标记稳定识别的服务依赖测试套件。

Review-planning 表和 `Dependency Summary` 使用 TSV，因为路径、命令和依赖详情中可能包含逗号。

Reducer 和 subagent 自动化应优先使用 authoritative `Review Control Plane JSON`；旧的 Review Plan/Manifest/Ledger section 继续作为兼容输出。TSV 表主要用于人工快速浏览。helper 已输出 manifest 后，自动化不得再通过直接 `git status` 或 `git diff --name-only` 重建审查范围。

### `tests/`

确定性 shell 测试，不依赖模型。`skill_contract_test.sh` 固化 `SKILL.md` 与 `references/` 之间的跨文档契约（禁止的占位符、必需的标签、不可翻译的 `VERDICT` 字段）。`collect_diff_context_test.sh`、`control_plane_test.sh` 和 `full_review_workflow_test.sh` 针对临时真实 Git 仓库验证普通输出、权威快照 pinning/漂移 fail-closed、schema 与完整 reduction。`parity_golden_test.sh` 复用共享 parity 夹具和专用 normalize 脚本，确保 legacy 与 Rust 的比对结果稳定。`install_smoke_test.sh` 和 `install_agent_matrix_test.sh` 在 copy/link/dry-run 模式和受支持的 agent 矩阵上验证安装器。它们不调用模型，可在 CI 中安全运行。

### `evals/`

基于 LLM 的评估 harness 现在按职责分层：

- `trigger-eval.json` 负责 skill 触发行为评估
- `output-eval.json` 保留为核心输出场景的兼容总入口
- `evals/output/routine-output-eval.json`、`advanced-output-eval.json`、`visual-output-eval.json`、`localization-output-eval.json` 将输出评测拆分为 routine、复杂、视觉和本地化四套矩阵
- `evals/taxonomy/marker-eval.json` 独立承载 `🔒`、`❌`、`⚠️`、`🧪`、`👁️`、`📈`、`🧭` 的 finding marker 与统计口径预期

执行入口也做了分层：

- `output_eval_runner.sh` 针对任意单个 eval 文件准备真实本地 fixture，可选调用外部模型 runner，并按期望 verdict 与必含短语对保存响应评分
- `--eval-file` 可让 `output_eval_runner.sh` 指向任意单个分层 output eval JSON，例如 `evals/output/visual-output-eval.json`。
- `--skill-dir` 用于选择链接到宿主 fixture 的 skill checkout，从而用同一套 eval case 分别生成引入前和引入后的响应，而无需切换 harness checkout
- `run_layered_output_evals.sh` 端到端执行 layered output eval 矩阵，覆盖 routine、advanced、visual 和 localization 四套 eval 文件
- `run_marker_eval_checks.sh` 校验 marker taxonomy 覆盖，并汇总 blocking / non-blocking case 数量
- `output_eval_codex_case.sh` 和 `output_eval_claude_case.sh` 每个宿主执行单个 eval case
- `output_eval_codex_runner.sh` 和 `output_eval_claude_runner.sh` 是宿主专用薄封装，会把当前仓库链接到 fixture 的 project-local skill 目录（Codex 用 `.agents/skills`，Claude Code 用 `.claude/skills`），再用适合各自宿主的非交互命令委托给 `output_eval_runner.sh`
- `output_eval_runner_test.sh` 是 fixture 准备与评分逻辑的确定性自测
- `compare_output_eval_quality.sh` 会用同一套分层 eval case 对已保存的引入前/引入后响应评分，输出 `output-eval-quality-diff/v1` JSON 报告；发现回归或响应集不完整时失败，比较过程不调用模型。密钥注意力 case 会额外统计非密钥 finding 召回；当凭据问题导致授权、迁移或兼容性问题召回下降时，通过 `secret_attention_regressions` 单独失败。
- `compare_output_eval_quality_test.sh` 确定性覆盖回归、改进、无回归与响应不完整四类结果
- `output_eval_host_wrappers_test.sh` 用 mock Codex/Claude 二进制验证这些 wrapper，确保宿主命令模板回归时不消耗真实模型调用
- `run_helper_gateway_probe.sh` 是一个 real-host stage，会对 bundled helper 与选定的直接 Git 命令做日志探针；如果 host 在尝试 `scripts/collect_diff_context.sh` 之前就检查 Git diff 来源，则判定失败
- `check_persisted_output_contract.sh` 会扫描宿主 transcript；如果 helper 大输出被持久化后，模型在声称完整审查前没有恢复保存的 plan/manifest，则判定失败
- `readme_surface_test.sh` 守护 README 面向外部暴露的 public surface，确保文档里的 contract gate 与入口清单保持一致
- `readme_host_entrypoints_test.sh` 固化分层 `Host Entrypoints` 文档，确保 README 持续以 `Primary`、`Analysis`、`Stage` 和 `Internal / Repo-wide` 暴露 host lane surface
- `eval_contract_test.sh` 是 repo 级门禁，统一守护 trigger eval、layered output eval、marker taxonomy 资产以及 host lane contract surface

先使用相同的 eval 文件、宿主、模型与 runner 设置分别生成引入前和引入后的响应目录，再执行不触发额外模型调用的比较：

```bash
./evals/compare_output_eval_quality.sh \
  --baseline-responses /path/to/baseline-responses \
  --current-responses /path/to/current-responses \
  --report-json /path/to/output-quality-diff.json
```

`advanced-independent-findings-enumeration-en` 使用中性的审查请求，fixture 同时包含一个凭据问题和三个相互独立的非密钥问题。为了得到有意义的随机性 A/B 结果，应在相同宿主与模型配置下对每个 checkout 重复运行 5～10 次，并要求当前版本的非密钥问题全部召回且每次均不低于基线。

scanner 关闭/开启的受控 pilot、结果及其样本量限制记录在 [`docs/gitleaks-quality-evaluation.md`](./docs/gitleaks-quality-evaluation.md)。

### Host Entrypoints

对于 host lane 工作流，可按下面的层级使用这些脚本：

- `Primary`: `evals/run_host_readiness_pipeline.sh`、`evals/run_cross_host_readiness.sh`
- 默认入口，用于执行端到端单 host 或跨 host 验证
- `Primary / Real Host Smoke`: `evals/run_real_host_smoke.sh`、`.github/workflows/real-host-smoke.yml`
- 当你需要一个稳定入口去跑真实、已认证 host 的 smoke 验证并收集产物时，使用这两个入口
- `Primary / Output Matrix`: `evals/run_layered_output_evals.sh`、`evals/run_marker_eval_checks.sh`
- 用于执行分层 output eval surface 与 marker taxonomy 检查，无需手工逐个挑选 eval 资产
- `Analysis`: `evals/analyze_host_readiness_diff.sh`、`evals/compare_output_eval_quality.sh`
- 用于对比 cross-host readiness 报告，或比较已保存的引入前/引入后 output-eval 响应；比较阶段无需重跑各 stage，也不会调用模型
- `Stage`: `evals/check_host_availability.sh`、`evals/run_helper_gateway_probe.sh`、`evals/check_persisted_output_contract.sh`、`evals/run_layered_host_evals.sh`、`evals/host_contract_subset.sh`
- 当你只想调试或单独运行某一层 host 边界时使用
- `Internal / Repo-wide`: `evals/eval_contract_test.sh`、host `*_test.sh`、`evals/host_failure_taxonomy.sh`
- 这些是重要的内部或仓库级 surface，不是普通用户入口
- `Stage reports`: `check_host_availability.sh`、`run_helper_gateway_probe.sh`、`run_layered_host_evals.sh` 和 `host_contract_subset.sh` 都可以输出 `host-stage-report/v1`
- `Pipeline report`: `run_host_readiness_pipeline.sh` 输出 `host-readiness-report/v1`
- `Cross-host 与 diff reports`: `run_cross_host_readiness.sh` 输出 `cross-host-readiness-report/v1`，`analyze_host_readiness_diff.sh` 输出 `host-readiness-diff-report/v1`

### `agents/openai.yaml`

为通过 agent 注册表暴露 skill 的环境提供轻量级元信息。

### `install.sh`

把这个 skill 包安装到受支持 AI 编程 agent 的 skills 目录。用法见[快速安装](#快速安装)。

## 内部工作原理

本节面向想理解解析逻辑或对其进行扩展的开发者。使用者可以直接跳到[如何触发一次审查](#如何触发一次审查)。

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

## 审查输出格式

预期输出以提交决策开头，并尽量精简细节，包含：

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

## 贡献

欢迎贡献。好的方向包括：改进审查启发式规则、收紧安全边界、优化输出模板、增强脚本在不同仓库状态下的健壮性。

开发环境搭建（shellcheck、Rust CLI 编译、确定性测试套件）与 PR 检查清单见 **[CONTRIBUTING.md](./CONTRIBUTING.md)**。

> 注意：`README.md` 与 `README.zh-CN.md` 是契约文件——多个测试会断言其中必须出现某些短语。修改时请保持这些精确字符串不变，或同时更新对应的断言。

## License

本项目采用 Apache License 2.0。详见 [LICENSE](./LICENSE)。
