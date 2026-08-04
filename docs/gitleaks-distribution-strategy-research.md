# Gitleaks 分发模式与 rust-analyzer 长期打包研究

## 状态与范围

研究日期：2026-07-29。

本文回答两个问题：

1. 现有 Gitleaks 模式是否是当前项目的最优方式，是否具备长期扩展能力；
2. Delivery 5 如果开始分发真实 `rust-analyzer`，是否应直接复制 Gitleaks 的实现。

这里评估的是第三方可执行文件的获取、验证、安装和发布方式，不重新评估
Gitleaks 的检测规则质量，也不把本项目描述为网络安全产品。本项目仍是本地开发
工具和静态分析/代码审查基础设施；Gitleaks 只是可选的本地模型输入脱敏层，
`rust-analyzer` 只是显式调用的语义上下文 provider。

证据仅来自本仓库实现、Gitleaks 和 rust-analyzer 官方仓库/发布、GitHub 官方
Actions/Release 文档以及 SLSA 规范。本文没有把第三方博客或市场宣传作为依据。

## 结论

现有 Gitleaks 模式是**当前约束下的局部最优默认方案**，不是全局最优方案，也
不是可以原样复制到任意第三方工具的长期分发框架。

它做对了最重要的运行时边界：版本和字节固定、只解析显式或包内路径、不从
`PATH` 猜测、安装失败时保留审查能力、运行前验证版本/能力、运行时再次验证
包内二进制。这比依赖用户机器上的包管理器、`PATH` 或 `latest` 下载更符合本
项目的确定性和离线要求。

但当前**运行时信任/失败边界比发布实现更成熟**，并不表示 scanner 执行层已经
通用化或完全有界。现有模式仍存在五个结构性上限：

- 工具、版本、平台、归档名和摘要分散在多个 Shell、测试和 workflow 分支中；
- SHA-256 能证明字节与本仓库记录一致，却不能独立证明字节由上游发布者签发；
- 单一 runtime 包会聚合所有平台的第三方二进制，增加包体积和以后新增工具的
  放大成本；
- 当前 CycloneDX 由项目 Rust manifest 生成，没有为捆绑的 Gitleaks 二进制建立
  完整、可验证的第三方二进制组件/SBOM 闭包。
- scanner 协议、finding 类型和参数仍与 Gitleaks 直接耦合；输出/finding 没有
  独立总量预算，受信配置也没有摘要绑定。

因此建议：

> 保留 Gitleaks 的用户语义和运行时信任边界；在引入真实 rust-analyzer 前，
> 把分发层重构为声明式第三方 artifact registry、按平台 provider pack、统一
> 获取/校验器、外部二进制 SBOM 和项目 release attestation。不要为
> rust-analyzer 再复制一套 `fetch_*.sh + *.version + 两份 sha256 + 多处 case`。

真实 `rust-analyzer` 的首选交付应是**显式 opt-in、只获取当前平台、固定日期
release tag 和两个摘要、原子安装到内容寻址缓存，再由现有 provider registry
绑定绝对路径和 executable SHA-256**。`rustup`、Homebrew、系统包或用户自备
二进制应保留为显式受信覆盖路径，不应成为内置 provider 的规范来源。

如果“最优”还包括**秘密检测引擎的检出率、误报率和对 review 质量的影响**，
现有证据不足以下结论。本仓库自己的质量验收要求 5–10 组 matched pairs，目前
只完成一组，并明确说明不能视为稳定统计结论。
[Gitleaks review-quality evaluation](gitleaks-quality-evaluation.md)

## 决策标准

“最优”必须相对目标判断。本文使用以下维度：

| 维度 | 本项目需要的性质 |
|---|---|
| 默认可用性 | 用户显式安装后无需预装第三方语言运行时或包管理器 |
| 确定性 | 同一 provider profile 对应相同版本、参数和可执行文件字节 |
| 来源可信度 | 能区分完整性摘要、发布者身份和构建 provenance |
| 离线能力 | 下载完成后可离线运行；`--no-download` 和 air-gap 有清晰路径 |
| 失败语义 | 可选能力不可用时不伪装成成功，也不阻断普通 review |
| 平台扩展 | 新增 OS/arch 不需要在多个脚本中手工复制策略 |
| 发布体积 | 用户不应安装其机器永远不会执行的其他平台二进制 |
| 更新成本 | 版本升级可以生成可审查 PR，并自动验证资产、摘要和能力 |
| 合规闭包 | 第三方许可证、组件、摘要、来源和 SBOM/证明与产物一致 |
| 运行隔离 | 不改变现有显式授权、只读 snapshot、预算和进程生命周期边界 |

## 现有 Gitleaks 模式的事实基线

### 获取与固定

本仓库将版本固定为 `8.30.1`，并分别记录四个上游归档摘要和四个解压后
可执行文件摘要。获取器只支持 `darwin-arm64`、`darwin-amd64`、
`linux-amd64` 和 `windows-amd64`，默认从官方 GitHub Release URL 下载，
先校验归档 SHA-256，再只提取预期文件，随后校验最终可执行文件 SHA-256。
[版本](../scripts/gitleaks.version)、[归档摘要](../scripts/gitleaks-assets.sha256)、
[二进制摘要](../scripts/gitleaks-binaries.sha256)、
[获取实现](../scripts/fetch_gitleaks.sh)

本仓库的四个归档摘要与 Gitleaks v8.30.1 官方
[`gitleaks_8.30.1_checksums.txt`](https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_checksums.txt)
一致。上游官方 README 同时列出 Release 二进制、Homebrew、Docker、源码构建、
pre-commit 和 GitHub Action；因此当前选择不是上游唯一安装方式，而是本项目
针对本地可重复执行选择的方式。
[Gitleaks v8.30.1 README](https://github.com/gitleaks/gitleaks/blob/v8.30.1/README.md#getting-started)

### 安装与降级

安装器优先接受已经存在且通过摘要、版本和能力检查的包内二进制。用户可用
`--no-download` 禁止网络；此时只接受绝对路径的
`PRE_COMMIT_REVIEW_GITLEAKS_BIN` 作为显式信任来源，否则明确报告“脱敏不可用，
review 继续”。下载失败也不会破坏普通 review。
[安装实现](../install.sh)、[产品边界](../SKILL.md)

这点比“发现 `PATH` 中任意同名程序”更稳健：Rust 运行时只选择显式覆盖路径或
包内平台名，不隐式回退到 `PATH`。包内二进制必须匹配摘要；显式覆盖路径被视为
用户信任，但仍要通过固定版本和 stdin/JSON 能力检查。
[Rust 运行时](../collect-diff-context-cli/src/secret_scan.rs)、
[doctor](../scripts/check_gitleaks.sh)

### 发布与测试

CI 在 Linux 集成测试中重新获取固定 Gitleaks 并运行 doctor、分发契约测试和
安装测试。Release matrix 为四个平台分别获取一个 Gitleaks，生成四个独立的
sanitizer pack 和四个平台隔离的 core pack；外部 `.sha256` sidecar 与项目
attestation 在任何归档检查或提取前由独立 verifier 校验。
[lint workflow](../.github/workflows/lint.yml)、
[release workflow](../.github/workflows/release.yml)、
[分发测试](../tests/gitleaks_distribution_test.sh)、
[安装测试](../tests/install_gitleaks_test.sh)

现有实现因此具备三层校验：

1. 获取时归档摘要；
2. 解压后和包内运行时的 executable 摘要；
3. 固定版本输出和真实 stdin/JSON 空输入烟测。

摘要解决字节完整性，版本/能力烟测解决“正确字节却不满足调用协议”的一部分
兼容性问题。两类检查不能互相替代。

## 当前方案为什么适合 Gitleaks

### 它直接满足模型输入脱敏的运行位置

本项目需要在本地将 repository-sourced helper output 送入模型前进行 stdin
扫描和重写。Gitleaks 官方二进制原生提供 `stdin`、JSON report 和完整 redaction
选项；当前 doctor 实际执行该协议。
[Gitleaks CLI](https://github.com/gitleaks/gitleaks/blob/v8.30.1/README.md#usage)、
[本仓库能力烟测](../scripts/lib/gitleaks_integrity.sh)

GitHub Secret Scanning 扫描 GitHub 仓库、历史和协作内容并生成告警；它不是一
个本地 stdin 过滤器，不能在发送模型输入前替换敏感值。因此它可以补充仓库治理，
不能替代这里的 Gitleaks 进程。
[GitHub Secret Scanning](https://docs.github.com/en/code-security/secret-scanning/introduction/about-secret-scanning)

### 单一静态二进制比语言级安装更适合默认安装器

上游 Release 已提供本项目四个平台所需的预构建二进制；不要求用户预装 Go、
Homebrew、Docker 或 pre-commit。当前只下载用户当前平台，下载后可离线使用，
与本项目的本地工具属性一致。
[Gitleaks GoReleaser 平台配置](https://github.com/gitleaks/gitleaks/blob/v8.30.1/.goreleaser.yml)

### 可选失败语义是正确的

Gitleaks 在这里用于降低把 repository output 中凭据送入模型的概率，但它不是
review 完整性的裁决器。当前失败会保留 `redaction unavailable` 事实并继续
review，而不是把“扫描器缺失”错误提升成“候选代码不可审查”。这与本仓库
公开边界一致。[SKILL.md](../SKILL.md)

## 不能称为全局最优的证据

### 1. 摘要不是发布者认证

SHA-256 回答的是“当前字节是否等于预先记录的字节”。如果攻击者在维护者更新
版本时同时影响下载资产和写入本仓库的新摘要，后续摘要比较仍会通过。当前两层
摘要提高了解压和后续存储的完整性，但它们最终由同一个本仓库变更建立信任锚，
不是独立的上游签名。

截至研究日，Gitleaks v8.30.1 的 GitHub Release API 为每个资产返回 SHA-256，
且官方 checksums 文件与本仓库一致；但该 release 的 `immutable` 为 `false`，
资产列表中没有单独的签名或 provenance/attestation 文件。
[Gitleaks v8.30.1 Release API](https://api.github.com/repos/gitleaks/gitleaks/releases/tags/v8.30.1)

这不表示当前二进制不可信；它表示可证明的结论应限定为“与审核并提交到本仓库
的摘要相符”，不能表述为“已用 Gitleaks 发布者签名验证”。

### 2. 分发策略没有数据化

平台映射同时存在于获取器、安装器、测试和 release matrix。新增例如
`linux-arm64` 时，维护者需要同步修改多处 case、预期列表、摘要文件和 workflow。
测试能捕获一部分漂移，但代码结构仍让平台数和工具数近似相乘。
[获取器](../scripts/fetch_gitleaks.sh)、[安装器](../install.sh)、
[分发测试](../tests/gitleaks_distribution_test.sh)、
[release matrix](../.github/workflows/release.yml)

这对一个第三方工具和四个平台尚可，对 Gitleaks、rust-analyzer 以及以后更多
provider 会迅速产生重复策略。

### 3. 全平台单包会放大体积

当前 release workflow 为每个平台发布一个独立的 Gitleaks sanitizer pack，core
pack 也只包含对应平台的项目二进制。官方 v8.30.1 四个选定归档合计约
31.4 MiB；本地解压后的四个生成二进制合计约 84 MiB，但用户只会获取其当前
平台的 pack，不再安装一个聚合所有平台的 runtime 包。
[Gitleaks Release API](https://api.github.com/repos/gitleaks/gitleaks/releases/tags/v8.30.1)、
[release 汇总步骤](../.github/workflows/release.yml)

同样复制 rust-analyzer 会更明显。2026-07-27 官方 release 中，与本项目四平台
对应的压缩 server 资产合计约 58.5 MiB，尚未计算解压后的体积。继续制作一个
包含所有 Gitleaks、所有 rust-analyzer 和所有项目二进制的通用 runtime，会让
新增 provider 的成本直接落到所有用户。
[rust-analyzer 2026-07-27 Release API](https://api.github.com/repos/rust-lang/rust-analyzer/releases/tags/2026-07-27)

### 4. 外部二进制没有进入当前 Rust SBOM 闭包

Release workflow 用 `cargo cyclonedx --manifest-path
collect-diff-context-cli/Cargo.toml` 生成项目 Rust 组件 SBOM，随后另行复制
Gitleaks 二进制和许可证。许可证存在是必要条件，但只从 Cargo manifest 生成的
SBOM 不会自然描述 Gitleaks Go 二进制、其确切资产摘要和嵌入依赖。
[release SBOM 和打包步骤](../.github/workflows/release.yml)、
[Gitleaks 上游 go.mod](https://github.com/gitleaks/gitleaks/blob/v8.30.1/go.mod)、
[本仓库 Gitleaks 许可证](../THIRD_PARTY_LICENSES/gitleaks-LICENSE)

因此目前可以声称“包含上游 MIT 许可证并固定二进制”，不应声称“runtime SBOM
完整覆盖全部第三方可执行文件依赖”。

### 5. Actions artifact 不是长期公共分发层

GitHub `upload-artifact` v4+ 的单个 artifact 是 immutable，并输出 SHA-256
digest；但默认/可配置 retention 有期限，官方 action 文档列出的常规上限是
90 天，而且下载 URL 需要登录并随 artifact、run 或 repository 生命周期失效。
它适合 matrix job 到 release job 的传递，不适合作为用户长期安装源。
[actions/upload-artifact](https://github.com/actions/upload-artifact#usage)

本仓库当前用 Actions artifact 做 job 间传递、用 GitHub Release 做公开交付，
方向正确；长期增强应落在 Release immutability 和 attestation，而不是让安装器
直接依赖 workflow artifact。

### 6. scanner 执行层还不是长期通用抽象

当前 Rust 实现直接反序列化 `GitleaksFinding`，固定 Gitleaks 参数、版本命令、
错误码和包内文件名。这个设计对唯一默认 scanner 很清晰，但增加第二个 secret
scanner 时需要复制或改写发现、执行、解析、位置验证、二次扫描和状态映射。
[Rust scanner 实现](../collect-diff-context-cli/src/secret_scan.rs)

同一实现已经有 30 秒默认 timeout，并在替换后进行第二次扫描以拒绝残留 finding；
但 stdout/stderr 使用 `read_to_end`，findings 直接反序列化到 `Vec`，没有独立的
输出字节或 finding 数量上限。`PRE_COMMIT_REVIEW_GITLEAKS_CONFIG` 也只要求文件
存在，没有像 executable 一样绑定摘要。默认包内配置很小且二进制内置规则随
二进制摘要固定，这降低了当前风险，但不能替代显式的配置和输出预算。
[进程读取与 redaction](../collect-diff-context-cli/src/secret_scan.rs)、
[受信配置](../references/security/gitleaks.toml)

扫描失败时返回原文，同时将状态标为 `unavailable`/`redaction-failed`；tampered
scanner 测试明确要求 review 继续。这是符合“可选 best-effort 脱敏层”的可用性
策略，不是隐私保证。若以后存在必须阻止未脱敏输出的部署，应增加显式
`required` policy，而不是悄悄改变现有默认。
[fail-open 实现](../collect-diff-context-cli/src/secret_scan.rs)、
[tamper/fail-open 测试](../tests/secret_gate_test.sh)

## 备选方式比较

| 方式 | 确定性/来源 | 用户体验与离线 | 平台/维护成本 | 对本项目的判断 |
|---|---|---|---|---|
| 固定上游 standalone binary（当前） | 摘要固定强；没有上游签名时来源认证中等 | 首次显式下载后最好；无需语言运行时 | 资产清单数据化后可控 | Gitleaks 当前最佳默认；rust-analyzer 首选起点 |
| 用户自备绝对路径 | profile 可固定最终摘要；来源由用户/组织承担 | air-gap 和企业镜像好；默认安装差 | 项目维护最低，用户运维最高 | 必须保留的覆盖路径，不应是唯一默认 |
| `rustup` / OS 包管理器 | 包管理器管理来源，但用户间版本和字节不统一 | 已安装用户方便；需要外部工具和在线仓库 | 每个平台策略不同 | 可作为显式来源；不适合内置规范 artifact |
| 固定源码、项目 CI 自建 | 可给自己的构建添加 provenance；不保证与上游 release 字节相同 | 安装可预构建；发布 CI 很重 | toolchain、native linker、目标矩阵和更新成本最高 | 只有在上游 artifact 信任/兼容性不足时升级采用 |
| digest-pinned OCI image | image digest 和环境闭包强，可挂只读 snapshot | 需要 Docker/Podman；macOS/Windows 启动和挂载复杂 | CI 统一，本地集成成本高 | 适合可信 CI lane，不是默认本地 LSP provider |
| 直接嵌入 `ra_ap_rust_analyzer` | Cargo lock 可固定 Rust 依赖；失去独立进程摘要边界 | 无额外下载，但显著增大主程序和内存/故障耦合 | API/编译升级成本转入主仓库 | 与当前 LSP 隔离架构不同，不是分发层替代品 |
| 云端/CI secret scanning | 服务端治理和持续重扫强 | 本地预发送脱敏不可用，离线不可用 | 服务方维护 | 补充控制，不能替代 Gitleaks stdin sanitizer |

### `rustup` 为什么不是规范来源

rust-analyzer 官方文档明确支持 `rustup component add rust-analyzer`，也支持
GitHub Release 二进制、源码构建、Homebrew 和部分 Linux 包管理器。
[rust-analyzer binary installation](https://github.com/rust-lang/rust-analyzer/blob/2026-07-27/docs/book/src/rust_analyzer_binary.md)

但官方安装文档同时说明 rust-analyzer 通常需要 Rust 标准库源，并且只正式支持
最新 stable 标准库源；旧 toolchain 或 project override 可能需要匹配的旧
rust-analyzer。[rust-analyzer installation](https://github.com/rust-lang/rust-analyzer/blob/2026-07-27/docs/book/src/installation.md)

当前 provider contract 刻意要求 `toolchain_mode: none`、禁用 sysroot/sysroot
source discovery、清空 `PATH`，并将 executable SHA-256 绑定到 profile。
[provider profile schema](../collect-diff-context-cli/schemas/repository-context-provider-profile.schema.json)、
[provider 边界](rust-analyzer-context-provider.md)

因此，把 `rustup` 当前活动 toolchain 中的组件当作隐式 provider 会重新引入
toolchain override、自动安装和每机差异。正确兼容方式是：用户或可信 CI 先解析
出真实绝对二进制，显式计算摘要并写入 registry/profile；provider 本身仍不调用
`rustup`。

### 项目 CI 自建为什么不是当前首选

rust-analyzer 官方 release workflow 已经分别处理 macOS 双架构、Windows
x86_64/i686/arm64、Linux glibc 多架构和 x86_64 musl，并包含 PGO、allocator、
glibc baseline、Zig 和 Windows CRT 等目标差异。官方 `xtask dist` 再按平台生成
gzip 或 zip。[官方 release workflow](https://github.com/rust-lang/rust-analyzer/blob/2026-07-27/.github/workflows/release.yaml)、
[官方 dist 实现](https://github.com/rust-lang/rust-analyzer/blob/2026-07-27/xtask/src/dist.rs)

项目 CI 自建可以为“本项目构建的 rust-analyzer”生成更清晰的项目 provenance，
但也意味着接管上述 build matrix、toolchain 和性能配置，且不能仅因源码 commit
相同就假定字节与官方 release 相同。当前没有证据证明这项额外维护成本会带来
provider 质量收益，所以应先使用精确固定的官方 standalone artifact。

### OCI 为什么更适合 CI 而非默认本地 provider

OCI digest 可以绑定完整镜像，并可只读挂载 snapshot；这对 prepared CI
environment 很有价值。但当前 provider 是 stdio LSP 子进程，依赖低启动延迟、
跨 macOS/Linux/Windows 一致的进程树终止和私有 runtime 目录。容器会增加 daemon、
volume path、平台虚拟化和镜像缓存依赖。它可以作为以后受信 CI profile 的另一
种执行后端，不应替换当前本地原生二进制默认。

## rust-analyzer 的上游分发事实

rust-analyzer 官方说明：VS Code extension 自带 server；其他编辑器可下载 GitHub
Release 预构建二进制、使用 `rustup`、从源码构建或使用平台包管理器。
[官方安装总览](https://github.com/rust-lang/rust-analyzer/blob/2026-07-27/docs/book/src/installation.md)、
[官方 binary 安装](https://github.com/rust-lang/rust-analyzer/blob/2026-07-27/docs/book/src/rust_analyzer_binary.md)

官方 stable release 使用日期 tag；workflow 还发布可变的 `nightly`。截至
2026-07-27，官方资产覆盖本项目现有四个目标所需的：

- `aarch64-apple-darwin`；
- `x86_64-apple-darwin`；
- `x86_64-unknown-linux-musl`；
- `x86_64-pc-windows-msvc`。

GitHub API 为每个资产暴露 SHA-256 digest，但该 release 的 `isImmutable` 为
`false`，观察到的资产列表没有单独 checksums、signature 或 provenance 文件。
[2026-07-27 Release API](https://api.github.com/repos/rust-lang/rust-analyzer/releases/tags/2026-07-27)

这使“精确日期 tag + 本仓库审查过的归档摘要 + 解压后二进制摘要”成为可行的
第一步，但信任强度仍与当前 Gitleaks 类似：固定字节，不等于独立验证上游构建者。

## 推荐的长期目标架构

### 1. 一个声明式 artifact registry

新增单一、版本化、机器校验的第三方 artifact manifest，至少记录：

```text
tool_id
tool_version / upstream_tag / upstream_commit
platform(os, arch, abi)
source_repository / source_url
archive_kind / archive_sha256 / archive_size
executable_member / installed_name / executable_sha256
version_probe / capability_probe
license_paths
sbom_component_identity
upstream_provenance_kind
project_attestation_policy
```

安装器、release matrix、doctor 和测试都从该 manifest 派生，不再分别维护平台
case。manifest 更新必须是普通 code review 中可见的版本升级 PR，禁止运行时读取
`latest` 或自动接受未知新资产。

manifest 本身应被 release provenance 覆盖；registry/profile 仍绑定最终 executable
SHA-256。这样“分发时验证”和“执行时授权”保持为两道独立关口。

这个通用 registry 只负责第三方 artifact 生命周期。若以后确实增加第二个 secret
scanner，再单独抽取有界的 scanner provider contract，统一输入字节、输出字节、
finding 数、timeout、坐标和 residual-scan 语义。rust-analyzer 继续使用已经存在的
repository-context provider contract，不应被塞进 secret-scanner 接口。

### 2. 按平台、按能力分包

建议发布：

- `core-<os>-<arch>`：项目自有 CLI、contracts 和 docs；
- `gitleaks-<version>-<os>-<arch>`：当前平台可选 sanitizer pack；
- `rust-analyzer-<date>-<os>-<arch>`：显式 opt-in provider pack；
- 可选 convenience bundle 只组合**一个平台**的 core 和所选 provider。

不要让一个用户安装四个平台的 Gitleaks 和四个平台的 rust-analyzer。安装器可以
把下载内容放入以 executable SHA-256 命名的共享缓存，再原子链接/复制到 skill
runtime；离线包和企业镜像仍能预置同一 pack。

### 3. 区分三类信任结论

报告和 doctor 应精确区分：

| 状态 | 能证明什么 |
|---|---|
| `pinned-digest` | 字节等于本仓库审核过的摘要 |
| `project-attested` | 项目 GitHub workflow 对指定 subject digest 生成了可验证构建/打包 provenance |
| `explicit-user-trust` | 用户显式提供绝对路径；provider 仍验证 profile 中的最终摘要和能力 |

如果上游以后提供签名或 attestation，再增加 `upstream-attested`；在此之前不要把
upstream checksum 或 GitHub API digest 命名为“签名验证”。

GitHub 官方 artifact attestation 将 artifact 名称和 digest 绑定到 SLSA build
provenance，用短期 Sigstore 证书签名，并支持 `gh attestation verify`；还可对
SPDX 或 CycloneDX SBOM 生成 attestation。
[GitHub artifact attestation](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations)、
[actions/attest-build-provenance](https://github.com/actions/attest-build-provenance)、
[SLSA provenance 定义](https://slsa.dev/spec/v1.2/provenance)

注意：如果项目 workflow 只是下载并重新打包上游二进制，项目 attestation 证明
的是“本项目 workflow 打包了这些 digest”，不是“上游从某 commit 构建了这些
字节”。只有上游 provenance 或本项目从固定源码自建，才能加强后一个结论。

### 4. Release immutability 和可验证发布

为本项目未来 release 启用 GitHub release immutability；官方文档说明该设置只
对未来 release 生效。每个平台 pack、manifest、SBOM 和 convenience bundle 都
生成 attestation，并在 release job 内和独立安装 smoke 中验证。
[GitHub immutable releases](https://docs.github.com/en/code-security/how-tos/secure-your-supply-chain/establish-provenance-and-integrity/prevent-release-changes)

workflow action 也应固定到审核过的 commit SHA，而不是只使用移动 major tag；
这属于本项目构建链固定，不能替代第三方二进制摘要。

### 5. 外部二进制 SBOM 闭包

最终 runtime SBOM 至少应把 Gitleaks 和 rust-analyzer 作为顶层 third-party
components，记录版本、supplier/source URL、license、archive 和 executable
hash、所归属的平台 pack，并建立它们与 runtime 的 dependency/contains 关系。

对于项目自建二进制，SBOM 应从固定源码依赖图生成并 attested；对于上游预构建
二进制，若上游没有 SBOM，应明确标记 component-level evidence 和未知的完整
transitive closure，而不是把 Cargo-only SBOM 当作整个 runtime SBOM。

### 6. 保持现有 provider 运行时边界

分发增强不应改变现有 rust-analyzer provider 的安全和产品边界：

- 真实 server 仍不可从普通 review、Fast Mode、index 或 static-analysis 默认路径
  触发；
- 下载只发生在显式安装/provider provisioning，不发生在分析运行中；
- provider registry、profile、executable、configuration、model 和 snapshot 继续
  以摘要绑定；
- 运行时继续空 `PATH`、无 shell、禁用 toolchain 自动安装和 repository command；
- 下载/验证失败返回 provider unavailable，不发布伪语义事实，也不影响普通 review。

[现有 provider 文档](rust-analyzer-context-provider.md)、
[provider 设计](superpowers/specs/2026-07-28-rust-analyzer-provider-design.md)

## 建议的实施顺序

### P0：在真实 rust-analyzer 分发前

1. 定义 `third_party_artifacts/v1` manifest 和 schema；用它生成或验证平台矩阵、
   URL、归档摘要、executable 摘要和许可证闭包。
2. 把 Gitleaks 迁移成第一个 registry entry，保持现有 CLI/环境变量/失败语义不变，
   以迁移证明通用层没有回归。
3. 为 Gitleaks config 增加摘要绑定，为 scanner stdout/stderr 和 finding 数增加
   独立预算；保持现有 best-effort 默认并准确报告 fail-open。
4. 将 release 从全平台单包改成平台包；保留 thin/core 和 `--no-download` 路径。
5. 让 runtime SBOM 显式包含外部二进制 components，并为发布产物生成/验证
   GitHub artifact attestations。
6. 启用未来 release immutability，记录版本升级 runbook 和回滚/撤销策略。

### P1：真实 rust-analyzer opt-in pilot

1. 固定一个 stable 日期 tag，禁止 `latest` 和 `nightly`；记录上游 commit、GitHub
   asset digest、本仓库 archive digest 和解压后二进制 digest。
2. 只为四个已支持平台生成 provider pack，安装时只获取当前平台。
3. 在 exact artifact 上运行 Delivery 5 的真实 Call Hierarchy、离线、超时、进程树、
   capability/readiness、路径/URI、资源和 latency gates。
4. 将准确 binary version 输出和 executable digest 固定进 profile/registry；继续
   保留用户自备绝对路径模式。
5. 发布前记录压缩/解压体积、冷启动、峰值 RSS、空项目与代表性项目 latency；
   没有这些证据前不把 provider 加入默认安装。

### P2：只有证据触发时才升级来源策略

只有出现以下任一事实，才考虑从“固定官方 artifact”升级到项目 CI 自建或
digest-pinned OCI：

- 上游不再提供所需平台或 ABI；
- 上游 artifact 无法满足许可证/SBOM/组织 provenance 政策；
- 官方 build 配置与 provider 所需 hardening/兼容性冲突；
- 性能或崩溃问题只能通过受控 patch 解决；
- 目标部署本来就全部在可信容器 CI，而不是本地开发机。

在这些事实出现前，自建会增加维护面，却不会自动提高语义质量。

## 最终判断

| 问题 | 判断 |
|---|---|
| 现有 Gitleaks 模式现在是否应替换 | 否。保留 pinned optional standalone binary 和显式覆盖路径 |
| 它是否比 `PATH`/包管理器默认发现更好 | 是。对本项目的确定性、离线和失败语义明显更合适 |
| 它是否已经是完整供应链最优 | 否。缺上游发布者证明、外部 binary SBOM 和 immutable/attested release |
| 它是否具备长期扩展能力 | 信任/失败边界具备；当前分发、配置绑定、预算和 scanner 抽象只具备有限扩展能力 |
| rust-analyzer 是否应照抄当前文件布局 | 否。应先建立通用 artifact registry 和平台 provider pack |
| rustup/Homebrew 是否应成为内置默认 | 否。只保留为显式、摘要绑定的用户/组织来源 |
| 是否应立刻改成项目 CI 自建 | 否。先用精确固定的官方 stable artifact，通过真实服务器和发布证据验证后再决定 |

最稳妥的长期方案不是在“捆绑、系统安装、源码构建、容器”中只选一个，而是分层：

> 默认交付使用当前平台的固定官方 artifact；执行授权始终绑定最终字节；企业和
> air-gap 可显式提供同摘要二进制；项目 release 为自己的 pack 提供 immutable、
> SBOM 和 attestation；只有证据表明上游 artifact 不再满足要求时，才接管源码构建。
