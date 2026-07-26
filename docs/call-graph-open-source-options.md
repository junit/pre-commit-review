# 全仓符号与调用图：开源组件选型

> 调研日期：2026-07-26
> 来源范围：仅使用项目官方仓库、官方文档和协议规范。

## 结论

可以引入开源组件，而且这比从零实现多语言名称解析和调用关系分析更合理。但不存在一个组件能同时提供：

- 多语言；
- 精确的跨文件符号解析；
- 完整调用图；
- 本地、离线、快速、增量；
- 不需要构建、依赖安装或项目准备；
- 可直接绑定 staged/branch candidate snapshot。

建议采用分层组合，而不是选择单一“调用图引擎”：

1. **默认底座：Tree-sitter。** 从候选快照字节直接提取定义、导入和语法调用点，形成快速、高召回但明确标记为 heuristic 的上下文。
2. **精确语义：LSP Call Hierarchy 适配器。** 对已有可用项目模型的语言按需查询 changed symbols 的 incoming/outgoing calls，不在每次 pre-commit 中导出完整全图。
3. **持久化交换：SCIP。** 消费与候选快照完全匹配的 definition/reference index；结合 Tree-sitter 的 call-site 分类后可构造更精确的调用边。SCIP 本身不能直接宣称为调用图格式。
4. **深度分析：Joern。** 作为显式启用的重型 Profile 或 CI evidence provider，不能进入默认快速路径。
5. **不采用 GitHub Stack Graphs 作为核心依赖。** 它的模型适合无构建名称解析，但官方仓库已经归档并明确停止支持。

第一阶段最值得验证的是 **Tree-sitter core + 一个 Rust 的 rust-analyzer Call Hierarchy adapter**。这能验证统一数据模型、候选快照绑定、降级语义和性能预算，而不需要立即承担完整多语言平台成本。

## 先区分四层能力

“AST、符号索引、引用图、调用图”不能混用。它们解决的问题不同：

| 层级 | 回答的问题 | 代表能力 | 不能自动推出 |
| --- | --- | --- | --- |
| 语法解析 | 这里是不是函数定义、导入、调用表达式？ | Tree-sitter CST/queries | `foo()` 究竟绑定到哪个 `foo` |
| 名称/引用解析 | 这个标识符引用哪个定义？ | 语言服务器、Stack Graphs、compiler-backed indexer | 该引用一定发生了调用 |
| 符号索引 | 全仓有哪些定义、引用、实现关系？ | SCIP、LSP workspace index | 完整 caller/callee 图 |
| 调用图 | 某函数调用谁、被谁调用？ | LSP Call Hierarchy、Joern CPG | 运行时动态派发的完整真实集合 |

即使是“实际调用图”也仍是静态近似。反射、动态函数值、运行时注入、条件编译、宏生成代码和虚调用会让结果出现缺边或候选边。因此输出模型必须记录 `provider`、`resolution` 和 `confidence`，不能只有无来源的 `caller -> callee`。

## 方案比较

| 方案 | 语法解析 | 名称/引用解析 | 持久化符号索引 | caller/callee | 增量能力 | 默认路径适配度 |
| --- | --- | --- | --- | --- | --- | --- |
| Tree-sitter | 强 | 无 | 需自行实现 | 仅语法 call-site | 强，文件级 | **高** |
| LSP Call Hierarchy | 由服务端负责 | 强，依语言而定 | 服务端内部，协议不提供统一导出 | **直接支持** | 依服务端 | **中，适合可选适配器** |
| SCIP + indexers | 由 indexer 负责 | 强，依 indexer 而定 | **强** | 可派生，但 schema 无独立 Call role | 协议本身不是 delta 协议 | **中，适合预计算索引** |
| GitHub Stack Graphs | 基于 Tree-sitter | **强项** | 有本地数据库能力 | 不提供调用图 | 设计上支持增量 | **低，项目已归档** |
| Joern | 强 | 前端/type recovery 相关 | CPG 图数据库 | **直接支持** | 未发现官方文件级增量契约 | **低，适合深度 Profile** |

## Tree-sitter

Tree-sitter 官方将其定义为 parser generator 和 incremental parsing library；它可以构建 concrete syntax tree，并在文本编辑后高效更新。运行时可嵌入，官方目标包括足够快以支持每次按键解析，以及无运行时依赖。[官方介绍](https://tree-sitter.github.io/tree-sitter/)和 [Rust Parser API](https://docs.rs/tree-sitter/latest/tree_sitter/struct.Parser.html) 还表明解析器可以直接接收文本字节及旧语法树，不要求从工作区路径读取文件。

官方 code-navigation 文档定义了 `@definition.function`、`@definition.method`、`@reference.call` 等 query capture；这足以提取函数、方法和语法调用点。[Tree-sitter Code Navigation Systems](https://tree-sitter.github.io/tree-sitter/4-code-navigation.html)

关键边界：这些 capture 是语法标签，不执行类型推断或跨文件名称绑定。两个模块中同名 `foo` 的调用、方法重载、trait/interface dispatch、别名导入和动态属性调用，都不能仅凭 AST 稳定解析。

对本项目的适配性：

- **本地/离线：高。** Rust binding 和 grammar 可以作为锁定版本的依赖编入 CLI。
- **快速/增量：高。** 缓存可按 `language + grammar_version + blob_sha256` 建立；未变化 blob 无需重解析。
- **候选快照绑定：高。** 直接解析 helper 已确定的 candidate bytes，不读取原工作区，也不需要 URI overlay。
- **固定版本：高。** 核心 crate、grammar crate 和 query 文件均可锁版本；但每个 grammar 的许可证和来源需要单独纳入 SBOM/NOTICE。
- **多语言：中到高。** grammar 生态广，但每种语言仍需要维护 definition/import/call queries 和模块解析规则。
- **无需构建/依赖准备：高。** 不运行仓库代码、包管理器、build script 或插件。

建议定位：**默认 symbol/import/syntactic-call index**，而不是“精确调用图”。

## LSP Call Hierarchy 与语言服务器

LSP 3.16 起标准化了三类请求：

- `textDocument/prepareCallHierarchy`
- `callHierarchy/incomingCalls`
- `callHierarchy/outgoingCalls`

协议返回调用者/被调用者项目和具体 call-site ranges，但 `callHierarchyProvider` 是可选 capability。[LSP 3.17 Call Hierarchy specification](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.17/language/callHierarchy.md)

LSP 是查询协议，不是全仓调用图导出格式：

- 没有标准化的 bulk graph dump；
- 没有跨 server session 的标准稳定 symbol ID；
- `CallHierarchyItem.data` 是服务端 opaque data，只保证在 prepare 与后续 incoming/outgoing 请求间保留；
- 要构造完整全图，客户端必须枚举符号、逐个查询、递归、去重并自行持久化。

因此它更适合从 changed symbols 开始做 1-2 跳影响查询，而不是在 pre-commit 临界路径中遍历全仓所有函数。

### 代表性开源服务端

**rust-analyzer** 实现了 prepare、incoming 和 outgoing handlers，其内部基于 Rust HIR，而不是文本名称匹配。[request handlers](https://github.com/rust-lang/rust-analyzer/blob/master/crates/rust-analyzer/src/handlers/request.rs)

但默认配置不满足当前受控执行约束：`cargo.buildScripts.enable` 和 `procMacro.enable` 默认均为 `true`，会运行 build scripts/构建 procedural macros；`cargo.noDeps=true` 才明确表示完全离线并跳过依赖获取。[rust-analyzer configuration](https://github.com/rust-lang/rust-analyzer/blob/master/docs/book/src/configuration_generated.md) 安全 Profile 至少需要关闭 build scripts、proc macros、check-on-save 和 dependency fetching，并接受由此造成的宏展开和类型精度下降。

**clangd** 的服务端注册 `callHierarchyProvider` 并实现 prepare、incoming、outgoing 请求；outgoing calls 依赖额外索引结构，当前实现默认启用。[ClangdLSPServer.cpp](https://github.com/llvm/llvm-project/blob/main/clang-tools-extra/clangd/ClangdLSPServer.cpp) [ClangdServer.h](https://github.com/llvm/llvm-project/blob/main/clang-tools-extra/clangd/ClangdServer.h)

clangd 会自动建立项目索引，但准确理解 C/C++ 通常需要 `compile_commands.json`。没有 compilation database 时，clangd 会使用类似 `clang foo.cc` 的简化 fallback command，精度会下降。[clangd compilation commands](https://clangd.llvm.org/design/compile-commands) 默认路径只能消费已经存在且被信任的 compilation database，不能为了索引而运行 CMake、Bazel、Bear 或项目构建。

**gopls** 官方明确把 Call Hierarchy 描述为“静态调用图的一部分”，并实现三类 LSP 查询。官方也明确说明 dynamic calls 不会包含，结果可能不完整。[gopls navigation: Call Hierarchy](https://go.dev/gopls/features/navigation#call-hierarchy) gopls 需要从 workspace 推断相应的 `go build` 配置和 module/workspace 边界。[gopls workspace](https://go.dev/gopls/workspace)

对本项目的适配性：

- **本地/离线：中。** 服务端可以本地运行，但必须用受控离线环境阻止依赖获取，并处理依赖缺失后的降级。
- **快速/增量：中到高。** 长生命周期 IDE daemon 通常表现好；当前 orchestration 明确排除 daemon，因此需要独立 provider lane，或使用有界生命周期的一次性进程并接受冷启动成本。
- **候选快照绑定：中。** LSP 使用 file URI/workspace，需要将候选快照物化到隔离目录，不能让服务端读取原仓库工作区。
- **固定版本：高。** 每个平台的服务端二进制可纳入 Built-in Profile Registry，以版本、SHA256、能力握手和固定参数锁定。
- **多语言：中。** 协议统一，但每种语言的项目模型、精度、启动参数和安全设置都不同。
- **无需构建/依赖准备：低到中。** 只有当必要的项目元数据和依赖已经存在时才能获得高精度；默认不得自动准备。

建议定位：**可选的 semantic-call provider**。若 capability 缺失、项目模型不完整或安全配置会执行仓库代码，应返回 `unavailable/degraded`，不能静默冒充精确结果。

## SCIP 与可用 indexers

SCIP 是语言无关的 source-code indexing protocol，目标能力是 Go to definition、Find references 和 Find implementations。[SCIP README](https://github.com/scip-code/scip)

其 schema 提供：

- workspace-level `Index` 和 per-file `Document`；
- 标准化 symbol identity；
- definitions/references/implementations/type-definition relationships；
- occurrence source ranges、symbol roles、syntax kinds；
- 可选 `enclosing_range`，官方注释将 call hierarchy 列为用途之一。

但 [SCIP schema](https://github.com/scip-code/scip/blob/main/scip.proto) 没有独立 `Call` symbol role。`IdentifierFunction` 的定义是“function references, including calls”，因此函数值引用与函数调用不能仅靠该 kind 完全区分。SCIP 可以支撑调用图派生，但不能把任意 function reference 直接当成调用边。

更可靠的组合方式是：

1. Tree-sitter 确认某 source range 是 call expression 的 callee；
2. SCIP occurrence 将该 callee range 解析到标准 symbol；
3. SCIP `enclosing_range` 或 Tree-sitter definition range 确定 caller；
4. 生成带 `provider=scip+tree-sitter` 的 resolved call edge。

SCIP 的 `Index` 表示完整 workspace index，协议没有标准化增量 delta。indexer 可以自己缓存，例如 scip-typescript 默认缓存跨 TypeScript project 的 symbol indexing，但增量行为不是 SCIP consumer 可以统一依赖的契约。

代表性官方 indexer 的准备成本不同：

- [scip-typescript](https://github.com/sourcegraph/scip-typescript) 支持 TypeScript/JavaScript；官方流程要求项目根包含 `tsconfig.json` 或 `package.json`，并明确先执行 `npm install`/`yarn install`。它不满足默认“无依赖准备”。
- [scip-clang](https://github.com/sourcegraph/scip-clang) 支持 C/C++，需要 compilation database；官方文档说明大型项目通常还需要代码生成或构建产物。它不满足默认“无构建准备”。
- [scip-java](https://github.com/sourcegraph/scip-java) 提供 Java/Kotlin indexer；精度和准备要求需要按具体 build tool Profile 验证。
- [scip-python](https://github.com/sourcegraph/scip-python) 提供 Python indexer；Python 环境、venv 和导入路径仍是项目模型的一部分。

本次官方资料核查未确认一个可直接承担本项目 Rust 默认路径的官方 Rust SCIP indexer，因此不能把 SCIP 当作当前 Rust 覆盖的前置假设。SCIP indexer 列表和维护状态变化较快，Registry 应逐个 pin，而不是只 pin `scip` protocol/CLI。

对本项目的适配性：

- **本地/离线：中到高。** 已生成的 `.scip` 文件可完全本地消费；生成阶段取决于 indexer。
- **快速/增量：中。** 消费快；首次生成可能很重，协议本身无增量 delta。
- **候选快照绑定：高，但必须显式实现。** 只接受记录了相同 candidate fingerprint、indexer identity 和 project-model fingerprint 的 index；不匹配就拒绝。
- **固定版本：高。** indexer 可按平台/版本/SHA256 注册；生成结果还必须记录 indexer arguments 和 schema version。
- **多语言：中到高。** 格式统一，实际覆盖由多个独立 indexer 决定。
- **无需构建/依赖准备：低到中。** 多数精确 indexer 依赖已有项目模型。

建议定位：**预计算的全仓 definition/reference baseline 和跨工具交换格式**，不是默认实时调用图引擎。

## GitHub Stack Graphs

Stack Graphs 的技术目标与本项目部分约束高度契合：官方 README 将其描述为可为任意语言定义 name-resolution rules，并强调 efficient、incremental，且无需接入现有 build 或 program-analysis tools。[GitHub Stack Graphs README](https://github.com/github/stack-graphs)

它解决的是跨文件名称解析和 definition/reference navigation，不是 caller/callee 调用图。仓库附带的 language rules 只有 Java、JavaScript、Python 和 TypeScript；其他语言仍需自行开发和验证规则。[official language packages](https://github.com/github/stack-graphs/tree/main/languages)

更重要的是，官方 README 已明确写明该仓库“不再由 GitHub 支持或更新，建议自行 fork”，GitHub repository metadata 也标记为 archived。[repository metadata](https://api.github.com/repos/github/stack-graphs)

建议定位：**不作为生产核心依赖**。其数据模型和无构建名称解析方法可作为设计参考；若 fork，就等于主动承担解析规则、漏洞修复、grammar 升级和长期维护成本。

## Joern

Joern 将源码、bytecode 和 binary 转换为 Code Property Graph。CPG 统一承载程序语法、控制流和数据流等关系，并提供 Scala-based DSL 查询。[Joern README](https://github.com/joernio/joern) [Code Property Graph](https://docs.joern.io/code-property-graph/)

Joern 的 call traversals 是这里最接近“实际调用图”的开箱能力：

- `.call`：全部 call-sites；
- `.callOut`：给定方法的 outgoing calls；
- `.callIn`：给定方法的 incoming call-sites。

官方示例还支持结合 AST/control structure/data flow 查询。[Joern Calls](https://docs.joern.io/cpgql/calls/)

当前官方 frontend 列表包括 C、C#、Go、Java、JavaScript、Kotlin、PHP、Python、Ruby 和 Swift，以及 Ghidra/Jimple 输入，但未列出 Rust。`joern-parse` 面向目录生成完整 CPG；未指定语言时会按文件数量最多的受支持类型选择一个 frontend，因此 polyglot repository 需要显式拆分运行。[Joern Frontends](https://docs.joern.io/frontends/)

工程成本明显高于其他方案：当前 README 要求 JDK 21，官方分发包是平台相关的 CLI zip，并支持 Docker；官方文档展示的是 source-directory parse 和完整 graph export 流程，本次未找到稳定的文件级增量更新协议。[Joern export](https://docs.joern.io/export/)

对本项目的适配性：

- **本地/离线：高。** 固定分发包后可以本地运行。
- **快速/增量：低。** JVM/CPG import 的冷启动、CPU、内存和存储不适合默认 pre-commit 路径；官方未文档化通用文件级 delta 契约。
- **候选快照绑定：中到高。** 可以对隔离物化目录运行，但必须记录 frontend、overlay/pass、版本和输入 fingerprint。
- **固定版本：高。** 平台 zip 可按 SHA256 pin；README 说明 release workflow 高频运行，更需要固定具体 release，不能跟随 `latest`。
- **多语言：中。** frontend 较多但非全覆盖，当前缺 Rust。
- **无需构建/依赖准备：中。** 多个 source frontend 可直接解析，但类型恢复和 dependency context 仍影响精度；JDK 21 是额外运行时前提。

建议定位：**显式 `deep-callgraph` / `security-cpg` Profile 或 CI evidence provider**。超时、资源限制或 frontend 缺失应表现为 unavailable verification，不能拖慢或阻断默认审查。

## 与候选快照和信任模型的集成要求

无论选择哪个组件，都必须先满足当前 authoritative scope 设计，而不是直接对工作区运行工具。

### Snapshot binding

每份索引至少记录：

```text
candidate_fingerprint
provider_id
provider_version
provider_binary_sha256_or_library_lock
adapter_schema_version
language_configuration_fingerprint
project_model_fingerprint
indexed_file_blob_fingerprints
```

消费时必须精确匹配 `candidate_fingerprint`。不能把工作区 daemon 的旧索引、base branch 索引或另一次 staged state 的索引混入本次 evidence。

Tree-sitter 应直接接收候选快照字节。LSP、SCIP indexer 和 Joern 需要 file paths 时，应使用 helper 物化的隔离、只读、无 `.git` snapshot，并将临时 URI 重映射回 repository-relative path。

### Execution trust

- linked library：通过 lockfile、vendor/SBOM 和 grammar query hashes 固定；
- external binary：通过 Built-in Profile Registry 固定平台、版本、SHA256、参数和 capability probe；
- repository configuration：只读取明确 allowlist 的 declarative files；
- dependency/build preparation：默认禁止，不因为检测到 `package.json`、`Cargo.toml` 或 CMake 文件就自动执行命令；
- network：默认 offline；缺少依赖时降级或标记 unavailable；
- output：统一经 bounded adapter 归一化，不直接信任工具生成的路径、严重级别或完整性声明。

### 统一边模型

建议最小调用边包含：

```json
{
  "caller": "stable-or-snapshot-local-symbol-id",
  "callee": "stable-symbol-id-or-null",
  "unresolved_callee": "text-or-null",
  "callsite": { "path": "src/a.rs", "start_line": 10, "start_column": 5 },
  "provider": "tree-sitter|lsp:rust-analyzer|scip+tree-sitter|joern",
  "resolution": "syntax-only|resolved|possible-dispatch|unknown",
  "confidence": "heuristic|high|provider-defined",
  "candidate_fingerprint": "..."
}
```

不要把不同 provider 的边无条件去重成一条“真边”。同一位置的语法边、LSP resolved edge 和 Joern possible-dispatch edge可以关联，但必须保留 provenance。

## 推荐实施顺序

### Phase 1：轻量默认索引

- 在 Rust CLI 内嵌 Tree-sitter；
- 首批只做仓库主要语言的 definitions/imports/call-sites；
- 按 blob hash 增量缓存；
- 输出 changed symbol、direct references candidates 和 syntactic callees；
- 所有调用边标记 `syntax-only/heuristic`。

验收重点不是“覆盖多少语言”，而是 cold/warm latency、缓存正确性、删除/重命名处理、snapshot mismatch rejection 和 malformed-source robustness。

### Phase 2：一个精确 LSP adapter

- 从 rust-analyzer 开始；
- 仅查询 changed functions 及 1-2 跳 incoming/outgoing；
- 使用隔离候选快照和 hardened offline configuration；
- 禁止 build scripts、proc macros、check-on-save 和 dependency fetching；
- 明确记录因禁用这些能力造成的 degraded precision；
- 对 capability 缺失、超时、server crash 和 stale URI 做失败测试。

LSP adapter 不应直接塞入当前“无 daemon” static-analysis orchestration contract；应建立独立的 `repository_context_provider` 契约，或明确修改该契约后再接入。

### Phase 3：SCIP consumer

- 接受用户/可信 CI 显式提供的 `.scip`；
- 要求 exact candidate fingerprint 和 pinned indexer metadata；
- 提供 definition/reference/implementation 上下文；
- 用 Tree-sitter call-site range 与 SCIP occurrence 相交，生成 resolved call edges；
- 不在默认路径自动执行 `npm install`、构建或 compilation database generation。

### Phase 4：Joern deep profile

- 独立资源预算、超时和输出上限；
- 按 frontend 拆分 polyglot input；
- 只围绕 changed symbols 导出 bounded slice；
- 在结果中记录 CPG overlays、frontend 和 Joern exact version；
- 定位为安全/数据流深度证据，而不是每次提交的基础设施。

## 最终决策

**引入开源组件是正确方向，但应引入“能力层”，不是引入一个被称为调用图的黑盒。**

- 现在可以批准：Tree-sitter default index、LSP adapter SPI、SCIP consumer SPI、统一 provenance/confidence model。
- 需要 PoC 后批准：rust-analyzer/clangd/gopls 的内置受信任 Profile、SCIP indexer 的逐语言支持。
- 不应作为核心依赖：已归档的 GitHub Stack Graphs。
- 不应默认启用：Joern、任何需要 dependency install/build prep 的 SCIP/LSP path。

这一分层既能获得类似 Greptile/CodeRabbit 的跨文件上下文，又不会破坏本项目现有的本地、离线、候选快照绑定和可审计执行边界。

## 仍需验证的事实

- 各 LSP 服务端对重载、trait/interface/virtual dispatch、宏展开和动态调用的语义没有被 LSP 规范统一，需要按固定版本建立 fixture corpus。
- SCIP indexer 列表和维护状态会变化；尤其 Rust indexer 的官方支持状态，本次未形成足以承诺产品能力的证据。
- Joern 各 frontend 的 call-linking/type-recovery 精度和资源成本差异较大，需要对目标语言实测；官方文档没有给出适用于本项目的统一增量保证。
- Tree-sitter grammar 与 tags queries 的质量由各语言仓库决定，核心项目活跃不代表每个 grammar/query 同等维护。
- GitHub 当前官方 code-navigation 文档描述的是基于 Tree-sitter 的 search-based navigation，而不是继续承诺 Stack Graphs 产品路径。[GitHub code navigation](https://docs.github.com/en/repositories/working-with-files/using-files/navigating-code-on-github)
