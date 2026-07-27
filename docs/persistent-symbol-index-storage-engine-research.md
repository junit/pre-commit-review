# 持久化符号索引存储引擎调研

> 调研日期：2026-07-27
> 来源范围：SQLite、RocksDB、rusqlite、rust-rocksdb、Rust 标准库的官方文档与官方源代码。
> 决策范围：Subproject B 的 FileFacts Store、Repository Graph Store、完整性、并发读取、原子发布、诊断和清理。

## 结论

建议引入 **SQLite，但不采用“一个长期可写的 WAL 单库”作为 Fast Mode 的直接数据源**。推荐方案是：

- FileFacts 继续使用内容寻址、不可变对象；
- 每个 Repository Graph generation 构建为一个自包含的 SQLite 文件；
- generation 文件名由 candidate manifest、project model、resolver/schema/toolchain digests 共同决定；
- `deep/index` 在 staging 路径中用 SQLite 事务构建、校验、关闭并同步文件，然后一次性发布到最终内容地址；
- Fast Mode 只打开已经发布且永不再修改的 generation，使用 `mode=ro&immutable=1`，不存在就立即返回 cache miss；
- staged candidate 在内存中叠加 FileFacts/graph overlay，Fast Mode 不创建数据库、journal、WAL 或临时文件。

这是一种 **不可变分代架构 + SQLite 图存储** 的混合方案。它保留原方案的非阻塞读取、候选绑定和损坏隔离，同时把图的索引、事务、查询、检查和 inspection 能力交给成熟引擎。

当前不建议 RocksDB。它的 Column Family、snapshot、WriteBatch 和 checksum 能力都满足通用数据库要求，但本项目不是高吞吐持续写入服务。为得到这些能力，需要承担 C++20、Clang/LLVM bindgen、压缩库、后台 flush/compaction、multi-file DB directory 和 musl/Windows 原生发布复杂度；这些成本没有被当前 1-2 hop 有界图查询负载证明是必要的。

纯自研不可变对象存储仍然可行，但不再是首选。它最小化依赖，却会让项目自行实现 adjacency index、事务一致性、schema migration、doctor/inspection、并发发布和崩溃恢复，长期维护成本高于引入 bundled SQLite。

## 决策约束

本次比较以仓库已有设计为准：

- Rust 单二进制；
- 发布目标为 macOS arm64/x86_64、Linux `x86_64-unknown-linux-musl`、Windows MSVC；
- Fast Mode 不能进行持久化写入，也不能等待 writer；
- 只有显式 `deep/index` 操作可以写缓存；
- FileFacts 以内容 digest 寻址并可跨 candidate 复用；
- generation 必须绑定 exact candidate manifest 与 project model；
- 发布必须是原子的，半成品不得被 reader 观察；
- 读取到不兼容、缺失或损坏的数据必须退化为 cache miss；
- warm 1-2 hop 查询 P95 目标不超过 2 秒，且遍历深度、节点数、边数和输出大小仍由应用层预算控制。

这里的“Fast reader 不等待 writer”必须是应用契约，而不是依赖数据库默认超时。任何锁冲突、`BUSY`、I/O 错误或完整性错误都应立即映射为 miss/unavailable，不能进入重试等待。

## 方案比较

| 维度 | SQLite WAL 单库 | 不可变 SQLite generation | RocksDB | 纯不可变对象/分片文件 |
| --- | --- | --- | --- | --- |
| Fast Mode 零写入 | 有条件；WAL 读取涉及 `-wal/-shm` 条件 | **强；`mode=ro&immutable=1`** | Read-only 不写 primary，但 secondary 需要独立目录 | **强** |
| reader 与 writer 解耦 | 高，但仍可能返回 `SQLITE_BUSY` | **最高；reader/writer 不打开同一可变文件** | Read-only 是静态视图；动态 catch-up 需 secondary | **最高** |
| 原子 generation 发布 | 单库事务内强 | **staging 事务 + 单文件发布** | WriteBatch 强，但 DB 是多文件目录 | 需自研 manifest/pointer 协议 |
| 图的正反向索引与检查 | **强** | **强** | 强，但需自行设计 key encoding | 需自研 |
| 损坏检测 | 错误码 + `integrity_check` | **同左，且 generation 可整体丢弃** | 默认读校验和 + paranoid checks | 需逐对象 digest 和全局 doctor |
| 内容寻址 FileFacts | 可用主键实现 | **对象 CAS + generation 引用** | 可用 key/Column Family 实现 | **天然适配** |
| Rust 单二进制发布 | `rusqlite/bundled` 风险较低 | **同左** | C++/bindgen/压缩库风险高 | **无新增原生依赖** |
| 后台线程与运行时调优 | 无必要后台 compaction | **无必要后台 compaction** | 有 flush/compaction thread pools | 无 |
| 自研代码与长期维护 | 中 | **中** | 高 | 高 |
| 对当前负载的匹配度 | 中 | **高** | 低到中 | 中 |

所有候选都必须通过本仓库真实 corpus 的 cold build、warm lookup、并发 publish、崩溃注入和四平台 release gate。任何官方资料都不能替代本项目的 P95 验证，因此本文不把“SQLite 或 RocksDB 一定低于 2 秒”作为事实；它们都应以基准测试证明。

## SQLite

### WAL 能解决什么

SQLite 官方说明 WAL 模式允许 reader 与 writer 并发，reader 不阻塞 writer，writer 也不阻塞 reader；但同一时刻仍只有一个 writer。WAL 还要求所有进程位于同一主机，并引入 `-wal`、`-shm` 文件和 checkpoint 管理。[SQLite WAL](https://sqlite.org/wal.html)

这意味着一个常驻 WAL 单库可以实现：

- `file_facts`、`symbols`、`forward_edges`、`reverse_edges`、`generations` 等表的统一事务；
- writer 先写不可见 draft generation，再用短事务切换 `ready/current` 状态；
- reader 在一个 read transaction 中得到一致快照；
- 多个 Fast Mode 进程并发读取，一个 `index` 进程写入。

但 WAL 并不能无条件兑现本项目的 Fast Mode 契约：

- 官方明确说明 WAL 查询在少数情况下仍可能返回 `SQLITE_BUSY`，应用必须准备处理；[SQLite WAL: Sometimes Queries Return SQLITE_BUSY](https://sqlite.org/wal.html#sometimes_queries_return_sqlite_busy_in_wal_mode)
- read-only WAL 只有在 `-shm/-wal` 已存在、可以创建，或数据库被声明为 immutable 时才可打开；这使“打开只读连接绝不产生 sidecar 写入”需要额外约束；[SQLite WAL: Read-Only Databases](https://sqlite.org/wal.html#read_only_databases)
- `immutable=1` 会关闭锁和变更检测，SQLite 官方警告：如果底层文件实际发生变化，可能返回错误结果或 `SQLITE_CORRUPT`；因此不能对一个并发更新的 WAL 主库使用 immutable；[SQLite URI filenames](https://sqlite.org/uri.html)
- 长 read transaction 会阻止 checkpoint 完成，持续重叠的 readers 可能导致 WAL 持续增长；某些强制 checkpoint 模式也可能阻塞 reader。[SQLite WAL: Avoiding Excessively Large WAL Files](https://sqlite.org/wal.html#avoiding_excessively_large_wal_files)

因此，WAL 单库可以作为未来 daemon/IDE 场景的候选，但不是 Subproject B 默认 Fast Mode 的最佳第一步。

### 事务、原子性与并发边界

SQLite 支持来自不同连接、线程或进程的多个并发 read transactions，但只支持一个并发 write transaction。[SQLite Transactions](https://sqlite.org/lang_transaction.html)

SQLite 将 atomic commit 定义为一个事务内的全部修改同时发生或全部不发生，并说明在操作系统崩溃或断电时事务仍表现为原子；具体耐久性仍依赖 journal/synchronous 配置和底层文件系统假设。[Atomic Commit in SQLite](https://sqlite.org/atomiccommit.html)

对于本项目，这些能力最有价值的使用位置是 **staging generation 的内部构建**：

1. 在 staging 文件中建立 schema、metadata、path-to-fact mapping、symbols 和双向 edges；
2. 用一个或多个受控事务写入，但只有完整生成后才发布；
3. 即使构建进程崩溃，也只留下不可见 staging 文件；
4. 已发布 generation 从不进行原地 migration 或 update。

这比依赖一个长期可写数据库中的 `current_generation` 指针更容易证明 Fast Mode 不等待、不写入，也缩小了单库损坏的 blast radius。

### 只读不可变 generation

SQLite URI 的 `mode=ro` 会以 read-only 模式打开已有数据库；`immutable=1` 进一步声明文件不会被任何进程修改，使 SQLite 以只读方式打开并跳过文件锁与变更检测。[SQLite URI filenames](https://sqlite.org/uri.html) [SQLite open flags](https://sqlite.org/c3ref/open.html)

这与内容寻址 generation 精确匹配：

- generation key 决定最终文件名；
- reader 只打开精确 key，不需要读取一个可变 `CURRENT` 指针；
- writer 在另一个 staging 路径构建，完成前最终路径不存在；
- 发布后文件永不修改，所以 `immutable=1` 的前提成立；
- writer 构建下一代时不会与当前 reader 共享可变数据库文件。

建议发布文件使用默认 DELETE journal mode，而不是携带 WAL sidecars。SQLite 官方说明 DELETE 是默认 journal mode，事务结束时删除 rollback journal；若需要更强的断电耐久性，`synchronous=EXTRA` 会在 DELETE 模式提交时额外同步 journal 所在目录。[SQLite PRAGMA journal_mode and synchronous](https://sqlite.org/pragma.html#pragma_journal_mode)

数据库事务只保证 staging 文件内部一致性，不自动保证“staging 路径到最终缓存路径”的跨平台发布协议。Rust `std::fs::rename` 当前在 Unix 对应 `rename`，在 Windows 对应 `MoveFileExW` 或 `SetFileInformationByHandle`，且不能跨 mount point；`File::sync_all` 尝试把文件内容与 metadata 同步到磁盘。[Rust `fs::rename`](https://doc.rust-lang.org/std/fs/fn.rename.html) [Rust `File::sync_all`](https://doc.rust-lang.org/std/fs/struct.File.html#method.sync_all)

因此实现仍必须：

- 保证 staging 与最终路径位于同一 filesystem；
- 先关闭 SQLite connection，再 `sync_all` generation 文件；
- 只把文件 rename 到一个尚不存在的内容地址，避免依赖各平台的 replace-existing 差异；
- 对并发构建相同 generation key 使用有界 writer lock；若最终文件已存在则验证并复用，不覆盖；
- 用跨平台 crash-injection tests 验证“旧 generation 可用或新 generation 可用，绝不读取半文件”。

### 完整性与“损坏即 miss”

SQLite 用 `SQLITE_CORRUPT` 表示数据库文件已损坏，用 `SQLITE_NOTADB` 表示输入不像 SQLite 数据库；并发活动还可能返回 `SQLITE_BUSY`。[SQLite result codes](https://sqlite.org/rescode.html)

`PRAGMA integrity_check` 会执行底层格式和一致性检查；`quick_check` 跳过 UNIQUE 约束与 index/table 内容一致性检查，因此是 O(N)，而完整 `integrity_check` 为 O(NlogN)。[SQLite PRAGMA integrity_check and quick_check](https://sqlite.org/pragma.html#pragma_integrity_check)

“损坏即 miss”需要分层定义，不能宣称每次有界查询都能发现数据库任意未读取页上的损坏：

- Fast Mode 校验 schema/version、generation metadata、candidate/project digests，以及本次实际读取的 FileFacts payload digest；任一错误立即使该 generation 或对象 miss；
- SQLite 返回 `CORRUPT`、`NOTADB`、I/O error、unexpected row shape 时，不修复、不重试写入，直接 miss；
- `index` 发布前运行完整 `integrity_check` 和应用级 root/count/digest 校验；
- `doctor` 对所有 generation 运行完整检查，并把失败文件移入 quarantine；
- Fast Mode 不在 2 秒预算内扫描整个数据库做全局 integrity check。

这一定义同样适用于 RocksDB：其 checksum 只保证实际读取数据的验证，全库一致性仍需要显式 doctor/consistency 操作。

### Rust 构建与发布

rusqlite 官方 README 推荐对自主管理 SQLite 数据库的应用使用 `bundled`：该 feature 会从 crate 内嵌源码编译并链接 SQLite，避免依赖用户系统的 SQLite 版本；文档特别指出它适用于 Windows 等链接较复杂的场景。[rusqlite README](https://github.com/rusqlite/rusqlite/blob/master/README.md)

`libsqlite3-sys` 的官方构建说明还表明：

- bundled 使用 `cc` crate 编译内嵌 SQLite C 源码并链接；
- bundled 使用预生成 bindings，不要求普通构建在 build time 运行 bindgen/Clang；
- SQLite 源码版本随固定的 rusqlite/libsqlite3-sys crate 版本确定。

这与当前 Rust 单二进制和 lockfile/SBOM 模型相容度较高。不过官方文档没有替本项目保证 macOS 双架构、musl 和 Windows MSVC 的完整 release matrix；引入前仍需要一个最小 PoC 在现有 `.github/workflows/release.yml` 四个 target 上构建、运行 read/write/integrity smoke tests，并记录二进制体积变化。

建议使用精确锁定的 `rusqlite` 版本与最小 feature 集：

```toml
rusqlite = { version = "=<approved-version>", default-features = false, features = ["bundled"] }
```

不要默认启用 loadable extension、SQLCipher、session、backup 或 buildtime bindgen；这些能力不属于 Subproject B 的必要 closure。

## RocksDB

### 数据模型能力

RocksDB Column Families 可以逻辑分区数据库，支持跨 Column Family 的 atomic writes 和一致视图；`WriteBatch` 可以原子应用多个更新。[RocksDB Column Families](https://github.com/facebook/rocksdb/wiki/Column-Families) [RocksDB Basic Operations](https://github.com/facebook/rocksdb/wiki/Basic-Operations#atomic-updates)

对本项目可以映射为：

- `facts` Column Family；
- `symbols` Column Family；
- `forward_edges` / `reverse_edges` Column Families；
- `generation_meta` Column Family；
- 用一个 WriteBatch 发布 generation metadata 和可见性标记。

RocksDB snapshots 提供 point-in-time consistent read-only view，但普通 snapshot 是进程内对象，不跨数据库重启持久化。[RocksDB Snapshot](https://github.com/facebook/rocksdb/wiki/Snapshot)

RocksDB 的 `TransactionDB` 和 `OptimisticTransactionDB` 提供冲突检测；不过官方文档明确说明多 key atomicity 已由 WriteBatch 提供，transactions 的额外价值是“只有无冲突时才提交”。[RocksDB Transactions](https://github.com/facebook/rocksdb/wiki/Transactions) 本项目明确限制为单个显式 index writer，因此初版即使选择 RocksDB，也应使用 writer lock + WriteBatch，而不是引入 TransactionDB 的锁表、内存历史和调优面。

### 并发 reader/writer 不是零成本替代

RocksDB 官方说明：

- Primary 是普通 read-write instance，同一数据库只允许一个 Primary；
- 多个 read-only/secondary instances 可以并发存在，并且不会在 primary DB directory 创建文件；
- read-only instance 得到创建时的静态视图，不能 catch up；
- secondary 必须由调用方显式 `TryCatchUpWithPrimary()`，并需要自己的目录存放日志；
- secondary 当前不支持 snapshot reads，并要求 `max_open_files=-1`，官方指出这在部分非 POSIX 文件系统上不可工作。[RocksDB Read-only and Secondary instances](https://github.com/facebook/rocksdb/wiki/Read-only-and-Secondary-instances)

这比 SQLite WAL 或不可变 generation 更难直接表达“每次短生命周期 Fast Mode 都读取最新的完整 generation”：普通 read-only 可能是静态旧视图，secondary 引入自己的可写目录和手工 catch-up，而项目又不需要 daemon 式持续跟随。

也可以把 RocksDB 做成不可变 generation directory。官方 Checkpoint API 能创建一致的独立目录，同 filesystem 时 hard-link SST，跨 filesystem 时复制，并复制 MANIFEST/CURRENT/WAL 以形成完整快照。[RocksDB Checkpoints](https://github.com/facebook/rocksdb/wiki/Checkpoints)

但这样会让发布单元从 SQLite 的一个文件变为包含 SST、MANIFEST、CURRENT 和可能 WAL 的目录；应用仍需设计目录级 staging、原子 pointer、引用生命周期和 Windows 清理。与此同时，RocksDB 的持续写入/compaction 优势在发布后的不可变 generation 中几乎不再发挥作用。

### 校验和与损坏处理

RocksDB 为存储数据关联 checksums，`ReadOptions::verify_checksums` 默认开启；`Options::paranoid_checks` 默认开启，在 open 或后续操作检测到内部损坏时返回错误。[RocksDB Basic Operations: Checksums](https://github.com/facebook/rocksdb/wiki/Basic-Operations#checksums)

这是 RocksDB 相对 SQLite 的真实优势：block-level checksums 更直接覆盖实际读取的数据。但它仍不能让一次只读取少量 keys 的查询证明整个 DB directory 完整；应用仍要把任何非 OK status 映射为 miss，并由 `doctor` 做全库验证。

### 后台工作与资源模型

RocksDB 使用 background thread pools 执行 compaction 和 memtable flush，并建议为 HIGH/LOW priority 工作分别配置资源；`max_background_jobs` 控制并发后台任务。[RocksDB Thread Pool](https://github.com/facebook/rocksdb/wiki/Thread-Pool)

Compaction 是 LSM 的核心，存在 read/write/space amplification 权衡；不同 compaction style 和后台 job 数量都影响性能与存储。[RocksDB Compaction](https://github.com/facebook/rocksdb/wiki/Compaction)

这些能力适合高吞吐持续写入、海量 key-value 或服务进程，但也意味着：

- CLI 的一次短查询仍需打开 multi-file engine 和相关 cache；
- index 命令需要显式限制 block cache、write buffers、background jobs、open files 和 compression；
- 性能测试不能只测单次 `Get`，还要覆盖 compaction/flush、冷启动、并发 reader、磁盘增长和 cleanup；
- `panic=abort` 的单二进制需要额外验证 C++ exception/abort、OOM 和 corruption status 的边界。

### Rust 与四平台发布成本

RocksDB 官方 INSTALL 当前要求支持 C++20 的编译器，并列出 Snappy、zlib、bzip2、LZ4、Zstandard 等可选压缩依赖；Windows 使用 Visual Studio/CMake 或 vcpkg。[RocksDB INSTALL](https://github.com/facebook/rocksdb/blob/main/INSTALL.md)

rust-rocksdb 官方 README 和 Cargo manifest 表明：

- binding 静态链接一个具体 RocksDB 版本；
- 默认启用 Snappy、LZ4、Zstd、zlib、bzip2 和 `bindgen-runtime`；
- `bindgen-runtime` 动态链接 libclang；musllinux/Alpine 建议改用 `bindgen-static`；
- Windows 若要静态 MSVC runtime 需要 `mt_static`；
- `librocksdb-sys` build script 会生成 bindings、编译 C++ RocksDB，并为 MSVC/非 Windows 设置不同 C++20 flags。[rust-rocksdb README](https://github.com/rust-rocksdb/rust-rocksdb/blob/master/README.md) [rust-rocksdb Cargo.toml](https://github.com/rust-rocksdb/rust-rocksdb/blob/master/Cargo.toml) [librocksdb-sys build.rs](https://github.com/rust-rocksdb/rust-rocksdb/blob/master/librocksdb-sys/build.rs)

因此 RocksDB 不是“不支持”当前 release targets，而是不能在没有 PoC 的情况下假设它与现有简单 `cargo build --target ...` 流程等价。至少需要增加：

- Linux musl 的 Clang/libclang/C++ runtime 方案；
- Windows MSVC runtime 和 C++ build smoke；
- macOS 双架构的 native archive validation；
- compression features 的最小化和许可证/SBOM closure；
- 二进制体积、构建时长、cold open memory、后台线程数的硬门槛。

当前负载没有证明这些成本值得承担。

## 纯不可变对象与分代图文件

原方案的强项仍然成立：

- FileFacts 以 digest 命名，天然跨 candidate 复用；
- generation manifest 只引用不可变对象，不发生 reader/writer 原地竞争；
- 单对象 digest 失败只影响对应 fact，容易映射为局部 miss；
- 不新增 native dependency，完全沿用当前 Rust release matrix；
- staging + publish + conservative GC 可以保持 Fast Mode 零写入。

主要问题不是“做不到”，而是项目需要自行拥有以下深模块：

- forward/reverse adjacency 的文件布局和随机读取索引；
- 多文件 generation 的一致性与 root digest；
- schema evolution 和旧 generation compatibility；
- crash-safe manifest publication；
- reader leases、Windows 删除失败、GC 与 quarantine；
- doctor 的全图结构检查和 inspection/query tooling；
- path、symbol、module 和 edge 的二级索引。

如果最终只需要读取极少数 precomputed adjacency shards，纯文件方案可能仍是最小实现；但 Subproject B 已明确要求 inspection、doctor、反向关系、bounded graph traversal 和后续多 provider 扩展。SQLite 能在不改变不可变 generation 语义的前提下，显著减少这些自研表面积。

## 推荐架构

### 1. FileFacts Store

保持独立内容寻址对象：

```text
<cache-root>/v2/repos/<repo-id>/facts/sha256/ab/<fact-digest>.facts
```

每个对象 envelope 至少绑定：

```text
schema_version
language
parser/query/adapter digests
candidate_blob_sha256
payload_sha256
payload_length
```

对象发布后不可修改。Fast Mode 对实际读取对象重算 envelope/payload digest，失败即局部 miss。SQLite generation 只存 fact digest、必要的 hot columns 和 graph indexes，不把 mutable DB 变成 FileFacts 的唯一真相来源。

这样保留跨 generation 物理去重和局部损坏隔离；如果基准显示大量小对象打开成为瓶颈，再考虑把 FileFacts payload 内联到每个 generation SQLite，而不是直接升级到 RocksDB。

### 2. Repository Graph Store

每个 generation 是一个 SQLite 文件：

```text
<cache-root>/v2/repos/<repo-id>/graphs/<generation-key>.sqlite
```

`generation-key` 至少绑定：

```text
graph_schema_version
candidate_manifest_digest
project_model_digest
resolver_digest
language_adapter/query digests
file-facts manifest digest
normalization rules digest
```

最低表面建议包括：

- `generation_meta`：上述 digests、counts、build identity、root digest；
- `files`：repository path、candidate blob、fact digest、module identity；
- `symbols`：stable local symbol id、kind、definition range、fact digest；
- `forward_edges`：source symbol/path 到 target symbol/path；
- `reverse_edges`：target 到 source；
- `unresolved_edges`：未解析原因、候选和 confidence；
- `modules` / `module_relations`：resolver 产出的模块边界和依赖关系。

所有 traversal 由 Rust 应用层逐跳查询并执行 deadline、node/edge/hop budgets。不要依赖无界 recursive CTE，也不要把全图反序列化进内存。

### 3. 构建与发布协议

```text
acquire bounded writer lock for generation-key
  -> build <staging>/<uuid>.sqlite in DELETE + synchronous=EXTRA
  -> write graph in transactions
  -> validate foreign keys, application root/counts, integrity_check
  -> close connection
  -> sync_all database file
  -> rename to graphs/<generation-key>.sqlite only if target is absent
  -> release lock
```

不维护可变 `CURRENT`。consumer 根据 exact candidate/project/model digests 计算 generation key；找不到精确文件就 miss。这使 candidate binding 同时成为 cache lookup key 和 publication boundary。

staging 文件、失败 generation 和损坏 generation 不原地修复：分别由 cleanup/quarantine 管理。writer 发现相同最终 key 已存在时先验证，验证通过即复用，失败则 quarantine 后重新构建。

### 4. Fast reader 协议

```text
compute exact generation-key
  -> open file:...?mode=ro&immutable=1
  -> validate schema and generation_meta
  -> attach in-memory staged overlay
  -> perform bounded 1-2 hop indexed lookups
  -> validate consumed FileFacts digests
  -> close
```

Fast reader：

- 不获取 writer lock；
- 不设置 busy timeout 等待；
- 不创建数据库、WAL、SHM、journal 或 temp 文件；
- 不做 migration、repair、checkpoint 或 full integrity scan；
- 任一 open/query/validation failure 都返回 explicit partial/unavailable，不阻断普通 diff review。

### 5. Staged overlay

staged overlay 不需要新的持久化数据库：

- 基线 generation 精确绑定 HEAD/base candidate；
- changed files 的 FileFacts 从当前 candidate bytes 构建；
- 删除/重命名、symbol replacement、forward/reverse edge deltas 保存在内存 overlay；
- traversal 先查 overlay tombstone/addition，再查 immutable base；
- 只有显式 `deep/index` 才把完整 candidate 发布成新 generation。

这满足 Fast Mode 零持久化写入，也避免为一次未提交 staged state 累积大量 generation。

## 不采用方案

### 不采用：单个长期可写 SQLite WAL 数据库

不是因为 WAL 不可靠，而是它把 Fast Mode 重新耦合到 sidecars、checkpoint、可变文件和少数 `BUSY` 情况。对未来长生命周期 daemon 或 IDE server，可以在独立 RFC 中重新评估。

### 不采用：RocksDB 作为 Subproject B 默认引擎

当前拒绝的是默认生产依赖，不是永久禁止。只有出现以下证据之一时才应重开决策：

- corpus 证明 SQLite generation 的 graph lookup 无法稳定满足 P95；
- FileFacts/edges 规模使 SQLite build 或 generation copy 成为不可接受瓶颈；
- 需要持续高频增量写入和长生命周期 daemon；
- 需要 RocksDB 特有的 block checksum、prefix iterator、LSM ingestion 或 Column Family 独立 tuning，且收益超过 native release 成本。

若重开，必须先做隔离 PoC，不直接进入主 implementation plan。

### 不采用：完全自研 graph database

继续保留不可变对象格式用于 FileFacts，但 Repository Graph 的索引、事务和 inspection 交给 SQLite。除非 PoC 证明 bundled SQLite 无法通过四平台门槛，否则没有足够理由自行维护数据库级能力。

## 实施前验收门槛

在正式实施 Subproject B 前，先完成一个不进入产品路径的 SQLite storage spike：

1. 在四个 release targets 上构建 `rusqlite + bundled`；
2. 验证 staging build、`integrity_check`、close/sync/rename、`mode=ro&immutable=1`；
3. 在 10k、100k、1M symbols/edges fixtures 上测 cold open、1-hop、2-hop、reverse lookup；
4. 并发运行 writer 构建下一代与 20 个 Fast readers，证明 reader 不等待且不创建 sidecars；
5. 在 commit 前、commit 后、sync 前后、rename 前后注入进程终止，证明最终路径不存在或是完整 generation；
6. 修改 header、截断文件、破坏 index page、篡改 FileFacts payload，验证 cache miss/quarantine；
7. 记录四平台二进制体积、build time、RSS 和 P50/P95/P99；
8. 对比纯对象 adjacency shard prototype，确认 SQLite 的实现与运行成本确实更优。

通过条件：

- warm 1-2 hop P95 <= 2s，并保留足够预算给上层 context rendering；
- Fast Mode filesystem trace 中没有 create/write/delete；
- writer 活跃时 Fast reader 不等待锁；
- 任意 crash/corruption fixture 不产生半可用 generation；
- 四平台 release、SBOM、license、Clippy/test gates 全部通过；
- 依赖和 feature closure 只有审定版本的 `rusqlite/bundled` 及其必要传递依赖。

如果 spike 失败，回退顺序应为：

1. 纯不可变 FileFacts + adjacency shards；
2. 调整 SQLite generation schema/layout；
3. 只有在规模和负载证据明确指向 LSM 时才评估 RocksDB。

## 最终选择

**选择：内容寻址 FileFacts + 不可变 SQLite Repository Graph generations。**

这不是在原设计与数据库之间二选一，而是保留原设计最关键的 Interface 和 Seam：

- candidate-addressed generation；
- immutable publication；
- explicit writer；
- zero-write/non-waiting fast reader；
- corruption-as-miss；
- bounded traversal；
- provider-agnostic facts and edges。

SQLite 只替换 Repository Graph Store 内部的自研文件/索引实现，不改变上层 `impact_context/v1`、FileFacts 契约、resolver 语义或 Fast/Deep mode 边界。RocksDB 保留为有规模证据后的候选，不进入 Subproject B 首版依赖闭包。
