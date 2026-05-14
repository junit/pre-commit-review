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
  - 代码卫生问题
  - 变更意图
  - 逻辑变化
  - 影响范围
  - 回归风险
- 返回清晰的结论：
  - `PASS`
  - `PASS_WITH_NOTES`
  - `NEEDS_WORK`
- 使用只读辅助脚本收集本地 Git 上下文，不修改仓库内容

## 为什么有这个仓库

这个仓库不是应用或框架，而是一个小而可移植的 skill 包，可以：

- 作为独立开源仓库发布
- 复制到现有 skills 集合中
- 适配到需要“提交前审查”能力的本地 agent 工具链里

## 仓库结构

```text
.
├── SKILL.md
├── agents/
│   └── openai.yaml
└── scripts/
    └── collect_diff_context.sh
```

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
- 在 diff 过大时安全截断输出

它不会执行 fetch、stage、reset、install，也不会修改任何文件。

### `agents/openai.yaml`

为通过 agent 注册表暴露 skill 的环境提供轻量级元信息。

## 工作方式

skill 按以下顺序解析审查输入：

1. 用户明确提供的 diff
2. 当前仓库中的 staged 变更
3. 如果没有 staged，则使用 unstaged 变更
4. 当前分支与检测到的 base 分支进行比较
5. 如果没有可用 diff，则提示用户先 stage 变更或直接提供 diff

当本地仓库可访问时，工作流优先使用 `scripts/collect_diff_context.sh` 作为以下信息的事实来源：

- diff 来源
- 审查边界
- 变更文件统计
- staged 与 unstaged 的说明
- untracked 文件警告

## 使用方式

### 方案 1：作为独立仓库使用

将本仓库克隆或复制到你的 agent 运行时读取自定义 skills 的位置。

示例目录结构：

```text
your-skills/
└── pre-commit-review/
    ├── SKILL.md
    ├── agents/
    └── scripts/
```

随后根据你的 agent 平台的 skill 加载机制，注册或暴露该 skill。

### 方案 2：合并到现有 skills 集合

如果你已经维护了一个更大的 skills 仓库，可以把当前目录作为一个独立 skill 包复制进去，并保留相对路径：

- `SKILL.md`
- `scripts/collect_diff_context.sh`
- `agents/openai.yaml`

辅助脚本会在 skill 说明中被引用，因此除非你同步修改引用路径，否则应保持目录结构不变。

## 审查输出

预期输出是结构化的提交前审查结果，包含：

- diff 来源
- 审查边界
- 变更文件统计
- 已审查与未审查范围
- 行为变化分析
- 风险评估

最终 verdict 的含义：

- `PASS`：在已审查范围内，看起来可以安全提交
- `PASS_WITH_NOTES`：可以提交，但存在后续建议或审查边界限制
- `NEEDS_WORK`：发现阻塞问题，不应按当前状态提交

## 安全特性

这个包的设计倾向保守：

- 当本地仓库不可访问时，不会假装看到了本地变更
- 会区分 staged 和 unstaged 的审查范围
- 会提醒 `git diff` 中未包含的 untracked 文件
- 当 diff 过大时，除非明确检查高风险文件，否则会视为部分审查场景

## 限制

- 该仓库不包含加载或执行 skill 的运行时本身
- 具体安装方式取决于你的 agent 平台
- 辅助脚本依赖环境中可用的 `git`
- 当前仓库即使脱离 Git 也能作为内容包存在，但本地 diff 收集只有在 Git 仓库内才有效

## 贡献

更适合的贡献方向包括：

- 改进审查启发式规则
- 收紧安全边界
- 优化输出模板
- 增强脚本在不同仓库状态下的健壮性

如果你修改了脚本路径或仓库结构，请同步更新 `SKILL.md`。
如果你修改了对外文档，请尽量保持各本地化 README 版本同步。

## License

本项目采用 Apache License 2.0。详见 [LICENSE](./LICENSE)。
