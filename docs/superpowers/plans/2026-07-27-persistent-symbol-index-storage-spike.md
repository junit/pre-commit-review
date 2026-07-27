# Persistent Symbol Index SQLite Storage Spike Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove or reject bundled SQLite as the immutable Repository Graph generation engine before it enters the production `repository-context-cli` path.

**Architecture:** Add an optional, isolated `sqlite-storage-spike` binary and feature. The spike builds a fixed graph fixture into a staging SQLite file, validates and publishes it without replacement, opens the result with read-only immutable flags, exercises bounded forward/reverse queries, and records four-platform build, correctness, concurrency, corruption, resource, and latency evidence.

**Tech Stack:** Rust 2021, `rusqlite 0.40.1` with `default-features = false` and `bundled`, SQLite DELETE journal mode, serde/serde_json, sha2, tempfile, existing GitHub Actions release targets, Rust integration tests, and Criterion-style measured command output without adding a second benchmark framework.

---

## Status and Hard Gate

The design is approved at
`docs/superpowers/specs/2026-07-27-persistent-symbol-index-design.md`.

This plan is B0 only. Do not add SQLite to `repository-context-cli`, create the
production cache Modules, or start B1-B7 until every go/no-go criterion in Task
7 passes.

Implementation runs directly in the current `feature/SAST` working tree per the
user's instruction. Do not create another worktree. Preserve unrelated ignored
and user-owned files.

## File Map

**Create:**

- `collect-diff-context-cli/src/bin/sqlite_storage_spike.rs`
- `collect-diff-context-cli/tests/sqlite_storage_spike.rs`
- `collect-diff-context-cli/tests/fixtures/sqlite_storage_spike/README.md`
- `THIRD_PARTY_LICENSES/rusqlite-LICENSE`
- `THIRD_PARTY_LICENSES/sqlite-PUBLIC-DOMAIN.md`
- `docs/persistent-symbol-index-sqlite-spike-results.md`

**Modify:**

- `collect-diff-context-cli/Cargo.toml`
- `collect-diff-context-cli/Cargo.lock`
- `.github/workflows/lint.yml`
- `.github/workflows/release.yml`

The spike binary is temporary. The production plan removes it after the result
is accepted and promotes the same pinned dependency into the cache Module.

### Task 1: Add the Isolated Bundled SQLite Dependency Boundary

**Files:**
- Modify: `collect-diff-context-cli/Cargo.toml`
- Modify: `collect-diff-context-cli/Cargo.lock`
- Create: `collect-diff-context-cli/src/bin/sqlite_storage_spike.rs`
- Create: `THIRD_PARTY_LICENSES/rusqlite-LICENSE`
- Create: `THIRD_PARTY_LICENSES/sqlite-PUBLIC-DOMAIN.md`

- [ ] **Step 1: Record the pre-spike dependency and binary baseline**

Run:

```bash
rtk cargo metadata --manifest-path collect-diff-context-cli/Cargo.toml --format-version 1 --no-deps
rtk cargo build --release --manifest-path collect-diff-context-cli/Cargo.toml --bins
rtk ls -lh collect-diff-context-cli/target/release/repository-context-cli
```

Expected: existing binaries build without SQLite and the repository-context
binary size is recorded in the spike results document under `Before spike`.

- [ ] **Step 2: Add the optional feature and spike binary declaration**

Add to `collect-diff-context-cli/Cargo.toml`:

```toml
[features]
test-fixture = []
sqlite-storage-spike = ["dep:rusqlite"]

[[bin]]
name = "sqlite-storage-spike"
path = "src/bin/sqlite_storage_spike.rs"
required-features = ["sqlite-storage-spike"]

[dependencies]
rusqlite = { version = "=0.40.1", default-features = false, features = ["bundled"], optional = true }
```

Keep every existing package, binary, dependency, target dependency, bench, and
profile declaration unchanged. Merge the snippets into their existing tables;
do not create duplicate `[features]` or `[dependencies]` headers.

- [ ] **Step 3: Add a deliberately failing spike entrypoint**

Create `collect-diff-context-cli/src/bin/sqlite_storage_spike.rs`:

```rust
fn main() {
    eprintln!("sqlite-storage-spike: not implemented");
    std::process::exit(2);
}
```

- [ ] **Step 4: Add the upstream license evidence**

`THIRD_PARTY_LICENSES/rusqlite-LICENSE` must contain the exact MIT license from
the pinned `rusqlite 0.40.1` crate source with a one-line crate/version header.

`THIRD_PARTY_LICENSES/sqlite-PUBLIC-DOMAIN.md` must contain SQLite's official
public-domain dedication text and its official source URL. Do not paraphrase
either license.

- [ ] **Step 5: Resolve the exact dependency closure**

Run:

```bash
rtk cargo check --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --bin sqlite-storage-spike
rtk cargo tree --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike -i rusqlite
rtk cargo tree --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike -i libsqlite3-sys
```

Expected: `rusqlite 0.40.1` and `libsqlite3-sys 0.38.1` are locked; no default
`rusqlite` cache, wasm, load-extension, SQLCipher, bindgen, backup, or session
feature is enabled.

- [ ] **Step 6: Prove production binaries still exclude the optional dependency**

Run:

```bash
rtk cargo clean --manifest-path collect-diff-context-cli/Cargo.toml
rtk cargo build --release --manifest-path collect-diff-context-cli/Cargo.toml --bins
rtk cargo tree --manifest-path collect-diff-context-cli/Cargo.toml -e features -i rusqlite
```

Expected: product binaries build; the final command reports no active `rusqlite`
package for the default feature set.

- [ ] **Step 7: Commit the spike dependency boundary**

```bash
rtk git add collect-diff-context-cli/Cargo.toml collect-diff-context-cli/Cargo.lock collect-diff-context-cli/src/bin/sqlite_storage_spike.rs THIRD_PARTY_LICENSES/rusqlite-LICENSE THIRD_PARTY_LICENSES/sqlite-PUBLIC-DOMAIN.md
rtk git commit -m "build: add isolated sqlite storage spike"
```

### Task 2: Define the Spike CLI and Immutable Generation Fixture

**Files:**
- Modify: `collect-diff-context-cli/src/bin/sqlite_storage_spike.rs`
- Create: `collect-diff-context-cli/tests/sqlite_storage_spike.rs`
- Create: `collect-diff-context-cli/tests/fixtures/sqlite_storage_spike/README.md`

- [ ] **Step 1: Write failing CLI contract tests**

Create `collect-diff-context-cli/tests/sqlite_storage_spike.rs` with helpers that
invoke `env!("CARGO_BIN_EXE_sqlite-storage-spike")`. Add these tests:

```rust
#[test]
fn help_lists_build_query_doctor_and_benchmark() {
    let output = spike(&["--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    for command in ["build", "query", "doctor", "benchmark"] {
        assert!(stdout.contains(command), "missing {command}");
    }
}

#[test]
fn build_publishes_one_digest_named_generation() {
    let cache = tempfile::tempdir().unwrap();
    let output = spike(&[
        "build",
        "--cache-dir",
        cache.path().to_str().unwrap(),
        "--symbols",
        "4",
        "--edges",
        "6",
    ]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: SpikeReport = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report.action, "build");
    assert_eq!(report.status, "completed");
    assert_eq!(report.symbols, 4);
    assert_eq!(report.edges, 6);
    assert_eq!(generation_files(cache.path()).len(), 1);
}
```

Define test-only `SpikeReport` with `serde::Deserialize` and exactly these fields:

```rust
struct SpikeReport {
    schema_version: u8,
    kind: String,
    action: String,
    status: String,
    generation_key: Option<String>,
    symbols: usize,
    edges: usize,
    elapsed_ms: u64,
    output_bytes: usize,
    limitations: Vec<String>,
}
```

- [ ] **Step 2: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --test sqlite_storage_spike
```

Expected: FAIL because the spike still exits 2 and emits no contract JSON.

- [ ] **Step 3: Define the fixed spike schema and report types**

Implement these production types in `sqlite_storage_spike.rs`:

```rust
#[derive(serde::Serialize)]
struct SpikeReport {
    schema_version: u8,
    kind: &'static str,
    action: &'static str,
    status: &'static str,
    generation_key: Option<String>,
    symbols: usize,
    edges: usize,
    elapsed_ms: u64,
    output_bytes: usize,
    limitations: Vec<String>,
}

#[derive(Debug, Clone)]
struct BuildArgs {
    cache_dir: std::path::PathBuf,
    symbols: usize,
    edges: usize,
    crash_at: Option<CrashPoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrashPoint {
    BeforeCommit,
    AfterCommit,
    AfterSync,
    BeforePublish,
}

#[derive(Debug)]
struct GenerationStats {
    generation_key: String,
    symbols: usize,
    edges: usize,
    application_root: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublishOutcome {
    Published,
    Reused,
}

#[derive(Debug)]
enum SpikeError {
    InvalidInput(String),
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    InvalidGeneration(String),
    InvalidExistingGeneration(String),
}
```

The generated fixture is deterministic:

- symbol `symbol-{index:08}` belongs to `src/module-{index % 128:03}.rs`;
- edge `edge-{index:08}` points from symbol `index % symbols` to symbol
  `(index * 17 + 1) % symbols`;
- every range is one-based and bounded;
- generation key is SHA256 over schema id, symbol count, edge count, and the
  deterministic row stream.

Use a fixed schema with `generation_meta`, `symbols`, and `edges`. Add outgoing
and incoming indexes. Store only integers and bounded text needed by the fixture.

- [ ] **Step 4: Implement strict argument parsing**

Support exactly:

```text
sqlite-storage-spike build --cache-dir <absolute> --symbols <1..2000000> --edges <0..5000000> [--crash-at <point>]
sqlite-storage-spike query --generation <absolute-file> --symbol <symbol-id> --direction <incoming|outgoing> --depth <1|2> --max-edges <1..10000>
sqlite-storage-spike doctor --generation <absolute-file>
sqlite-storage-spike benchmark --cache-dir <absolute> --symbols <count> --edges <count> --queries <count>
```

Reject relative paths, unknown flags, zero symbol counts, `edges` with zero
symbols, depth above two, and limits above the declared maxima with exit 2 and a
single `sqlite-storage-spike:` diagnostic.

- [ ] **Step 5: Implement minimal build and JSON rendering**

Use these SQLite settings for the staging file:

```sql
PRAGMA journal_mode = DELETE;
PRAGMA synchronous = EXTRA;
PRAGMA foreign_keys = ON;
PRAGMA trusted_schema = OFF;
```

Build rows in one explicit transaction. Before commit, write one
`generation_meta` row containing schema version, generation key, symbol count,
edge count, and application root digest. Serialize reports with
`serde_json::to_vec`, set `output_bytes` using the same bounded fixpoint pattern
as `ImpactContext`, then emit one compact JSON object without a trailing log
line.

- [ ] **Step 6: Run and verify green**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --test sqlite_storage_spike help_lists_build_query_doctor_and_benchmark
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --test sqlite_storage_spike build_publishes_one_digest_named_generation
```

Expected: both tests PASS and the final generation filename is 64 lowercase hex
characters plus `.sqlite`.

- [ ] **Step 7: Document the generated fixture contract**

In `tests/fixtures/sqlite_storage_spike/README.md`, record the deterministic
symbol/edge formulas, schema version, maximum inputs, and the rule that fixture
generation never reads repository source.

- [ ] **Step 8: Commit the spike CLI contract**

```bash
rtk git add collect-diff-context-cli/src/bin/sqlite_storage_spike.rs collect-diff-context-cli/tests/sqlite_storage_spike.rs collect-diff-context-cli/tests/fixtures/sqlite_storage_spike/README.md
rtk git commit -m "test: define sqlite generation spike"
```

### Task 3: Prove Transaction, Integrity, and No-Clobber Publication

**Files:**
- Modify: `collect-diff-context-cli/src/bin/sqlite_storage_spike.rs`
- Modify: `collect-diff-context-cli/tests/sqlite_storage_spike.rs`

- [ ] **Step 1: Write failing publication and corruption tests**

Add tests named:

```rust
build_reuses_an_existing_valid_generation
build_never_replaces_an_existing_invalid_generation
doctor_accepts_a_complete_generation
doctor_rejects_truncated_database
doctor_rejects_generation_metadata_mismatch
doctor_rejects_foreign_key_and_root_digest_mismatch
```

The invalid-final test must pre-create the exact digest path with bytes
`b"not sqlite"`, run `build`, and assert non-zero exit plus unchanged file bytes.
The truncated test must build a valid database, truncate it to half its length,
and require `doctor` status `corrupt`.

- [ ] **Step 2: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --test sqlite_storage_spike doctor_
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --test sqlite_storage_spike build_reuses_
```

Expected: new tests FAIL because doctor, application-root validation, and
no-clobber behavior are incomplete.

- [ ] **Step 3: Implement the validation sequence**

Add functions with these signatures:

```rust
fn validate_generation(connection: &rusqlite::Connection, expected_key: &str) -> Result<GenerationStats, SpikeError>;
fn application_root(connection: &rusqlite::Connection) -> Result<String, SpikeError>;
fn integrity_check(connection: &rusqlite::Connection) -> Result<(), SpikeError>;
fn publish_noclobber(staging: tempfile::NamedTempFile, final_path: &std::path::Path) -> Result<PublishOutcome, SpikeError>;
```

`validate_generation` must verify:

- `PRAGMA application_id` and `user_version` exact values;
- one metadata row and exact generation key;
- declared versus queried symbol/edge counts;
- no foreign-key failures;
- `PRAGMA integrity_check` returns exactly `ok`;
- recomputed path-sorted application root matches metadata.

`publish_noclobber` must sync the staging file and use
`NamedTempFile::persist_noclobber`. `AlreadyExists` triggers validation and reuse
of the final generation; any invalid existing final file is left untouched and
reported as `invalid-existing-generation`.

- [ ] **Step 4: Open published generations immutably**

Add:

```rust
fn open_immutable(path: &std::path::Path) -> Result<rusqlite::Connection, SpikeError>;
```

Build a percent-encoded `file:` URI ending in `?mode=ro&immutable=1` and open it
with `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_URI | SQLITE_OPEN_NO_MUTEX`. Immediately
set `query_only = ON` and `trusted_schema = OFF`. Do not set a busy timeout.

- [ ] **Step 5: Run and verify green**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --test sqlite_storage_spike
```

Expected: all spike tests PASS; an invalid exact final file is never overwritten.

- [ ] **Step 6: Commit publication integrity**

```bash
rtk git add collect-diff-context-cli/src/bin/sqlite_storage_spike.rs collect-diff-context-cli/tests/sqlite_storage_spike.rs
rtk git commit -m "feat: prove immutable sqlite publication"
```

### Task 4: Prove Crash and Concurrent Reader Semantics

**Files:**
- Modify: `collect-diff-context-cli/src/bin/sqlite_storage_spike.rs`
- Modify: `collect-diff-context-cli/tests/sqlite_storage_spike.rs`

- [ ] **Step 1: Write failing crash-injection tests**

For every `CrashPoint`, start `build --crash-at <point>` and require process exit
99. Assert:

```rust
assert!(published_files(&cache).is_empty() || all_published_files_pass_doctor(&cache));
assert!(cache.join("graphs").read_dir().unwrap().all(|entry| {
    let name = entry.unwrap().file_name();
    !name.to_string_lossy().ends_with("-journal")
        && !name.to_string_lossy().ends_with("-wal")
        && !name.to_string_lossy().ends_with("-shm")
}));
```

Add test `reader_of_generation_a_does_not_wait_for_writer_of_generation_b`:

- build generation A;
- start 20 query processes against A;
- concurrently build a larger generation B;
- require every reader to finish under 750ms and report completed;
- require no new files next to A.

- [ ] **Step 2: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --test sqlite_storage_spike crash_
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --test sqlite_storage_spike reader_of_generation_a_
```

Expected: FAIL until explicit crash points and query traversal exist.

- [ ] **Step 3: Implement explicit crash points**

At the four named locations call only:

```rust
if arguments.crash_at == Some(point) {
    std::process::exit(99);
}
```

Do not add signal handlers or cleanup that would make this test less representative
of abrupt process termination.

- [ ] **Step 4: Implement bounded one-hop and two-hop query**

Use iterative Rust breadth-first traversal. Query outgoing rows with:

```sql
SELECT edge_id, from_symbol, to_symbol
FROM edges
WHERE from_symbol = ?1
ORDER BY edge_id
LIMIT ?2
```

and incoming rows with the equivalent `to_symbol = ?1` indexed query. Track
visited `(direction, symbol)` pairs, stop at the requested depth or maximum edge
count, and report `partial` with `edge-budget-exhausted` when truncated.

- [ ] **Step 5: Verify crash and concurrency behavior**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --test sqlite_storage_spike crash_ -- --nocapture
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --test sqlite_storage_spike reader_of_generation_a_ -- --nocapture
```

Expected: PASS; every final database is absent or doctor-valid, readers do not
wait for the unrelated writer, and no WAL/SHM/journal sidecar remains published.

- [ ] **Step 6: Commit crash and concurrency evidence**

```bash
rtk git add collect-diff-context-cli/src/bin/sqlite_storage_spike.rs collect-diff-context-cli/tests/sqlite_storage_spike.rs
rtk git commit -m "test: harden sqlite crash and concurrency behavior"
```

### Task 5: Add Deterministic Scale and Resource Measurements

**Files:**
- Modify: `collect-diff-context-cli/src/bin/sqlite_storage_spike.rs`
- Modify: `collect-diff-context-cli/tests/sqlite_storage_spike.rs`

- [ ] **Step 1: Write failing benchmark-report tests**

Add a test that runs:

```text
benchmark --symbols 10000 --edges 10000 --queries 100
```

Deserialize and require these additional report fields:

```rust
database_bytes: u64,
peak_rss_bytes: Option<u64>,
build_ms: u64,
cold_open_ms: u64,
query_p50_us: u64,
query_p95_us: u64,
query_p99_us: u64,
sidecar_files: usize,
```

Require `sidecar_files == 0`, `query_p50_us <= query_p95_us`, and
`query_p95_us <= query_p99_us`.

- [ ] **Step 2: Run and verify red**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --test sqlite_storage_spike benchmark_report_
```

Expected: FAIL because percentile and resource fields do not exist.

- [ ] **Step 3: Implement deterministic benchmark sampling**

Use `std::time::Instant`, precomputed query symbols, and sorted microsecond
samples. Define percentile selection as:

```rust
fn percentile(sorted: &[u64], numerator: usize, denominator: usize) -> u64 {
    let index = sorted
        .len()
        .saturating_mul(numerator)
        .saturating_add(denominator - 1)
        / denominator;
    sorted[index.saturating_sub(1).min(sorted.len() - 1)]
}
```

Use `(50, 100)`, `(95, 100)`, and `(99, 100)`. Exclude fixture build time from
query percentiles. Run one cold open, then reopen once and measure queries on the
warm connection.

Peak RSS is best-effort and may be `null`; database bytes, timings, and sidecar
count are mandatory.

- [ ] **Step 4: Run the three required scale classes**

Run release builds:

```bash
rtk cargo run --release --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --bin sqlite-storage-spike -- benchmark --cache-dir /tmp/pcr-sqlite-spike-10k --symbols 10000 --edges 10000 --queries 1000
rtk cargo run --release --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --bin sqlite-storage-spike -- benchmark --cache-dir /tmp/pcr-sqlite-spike-100k --symbols 100000 --edges 100000 --queries 1000
rtk cargo run --release --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --bin sqlite-storage-spike -- benchmark --cache-dir /tmp/pcr-sqlite-spike-1m --symbols 1000000 --edges 1000000 --queries 1000
```

Expected: each command emits one valid report, creates no published sidecars,
and warm one-hop/two-hop query P95 remains below two seconds. Record actual
measurements; do not invent a stricter universal threshold from one workstation.

- [ ] **Step 5: Verify normal tests remain fast**

Run:

```bash
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --test sqlite_storage_spike
```

Expected: regular integration tests use only small fixtures and remain suitable
for every CI run; the 1M measurement stays in the explicit spike gate.

- [ ] **Step 6: Commit benchmark instrumentation**

```bash
rtk git add collect-diff-context-cli/src/bin/sqlite_storage_spike.rs collect-diff-context-cli/tests/sqlite_storage_spike.rs
rtk git commit -m "perf: measure sqlite graph generation"
```

### Task 6: Add Four-Platform Build and Smoke Gates

**Files:**
- Modify: `.github/workflows/lint.yml`
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Add a local workflow-shape regression test**

In the existing workflow contract tests, or a new shell assertion inside the
appropriate existing test file, require both workflows to contain:

```text
--features sqlite-storage-spike
--bin sqlite-storage-spike
sqlite-storage-spike --help
```

Run the focused shell test and verify it fails before editing the workflows.

- [ ] **Step 2: Build the spike on every release target**

In the existing release matrix, add:

```yaml
- name: Build SQLite storage spike
  run: cargo build --release --target ${{ matrix.target }} --features sqlite-storage-spike --bin sqlite-storage-spike
  working-directory: collect-diff-context-cli
```

Do not add the spike binary to release artifacts.

- [ ] **Step 3: Run a native smoke on every matrix runner**

Add a shell step that selects `.exe` on Windows, then runs:

```text
sqlite-storage-spike --help
sqlite-storage-spike build --cache-dir "$RUNNER_TEMP/pcr-sqlite-smoke" --symbols 100 --edges 200
sqlite-storage-spike doctor --generation "$PUBLISHED_GENERATION"
```

The step must verify no `-wal`, `-shm`, or `-journal` file remains in the graph
directory.

- [ ] **Step 4: Add the Linux CI scale gate**

In `.github/workflows/lint.yml`, after the release build, run the 100k fixture
on every ordinary lint build and the 1M fixture in one Linux release-mode job.
Parse JSON with the existing Python runtime and fail when:

- status is not completed;
- sidecar count is non-zero;
- query P95 exceeds two seconds;
- database bytes or output fields are missing.

- [ ] **Step 5: Validate workflow syntax and focused tests**

Run:

```bash
rtk actionlint -oneline .github/workflows/lint.yml .github/workflows/release.yml
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --test sqlite_storage_spike
```

Expected: actionlint reports no diagnostics and all spike tests PASS.

- [ ] **Step 6: Commit the cross-platform gate**

```bash
rtk git add .github/workflows/lint.yml .github/workflows/release.yml
rtk git commit -m "ci: gate bundled sqlite storage spike"
```

### Task 7: Record the Go/No-Go Decision

**Files:**
- Create: `docs/persistent-symbol-index-sqlite-spike-results.md`
- Modify: `docs/superpowers/specs/2026-07-27-persistent-symbol-index-design.md` only when the result rejects or materially changes the approved design

- [ ] **Step 1: Run the complete local spike gate**

Run:

```bash
rtk cargo fmt --manifest-path collect-diff-context-cli/Cargo.toml --all -- --check
rtk cargo clippy --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --all-targets -- -D warnings
rtk cargo test --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike
rtk cargo build --release --manifest-path collect-diff-context-cli/Cargo.toml --features sqlite-storage-spike --bin sqlite-storage-spike
rtk git diff --check
```

Expected: all commands PASS.

- [ ] **Step 2: Capture platform and scale evidence**

Create `docs/persistent-symbol-index-sqlite-spike-results.md` with these exact
sections and populated values:

```markdown
# Persistent Symbol Index SQLite Spike Results

## Decision

## Versions

- rusqlite: 0.40.1
- libsqlite3-sys: 0.38.1

## Platform Matrix

| Target | Build | Build/Doctor Smoke | Immutable Read | Sidecars |
| --- | --- | --- | --- | --- |

## Scale Results

| Symbols | Edges | DB bytes | Build ms | Cold open ms | Query P50/P95/P99 us | Peak RSS |
| ---: | ---: | ---: | ---: | ---: | --- | ---: |

## Crash and Corruption Results

## Binary and Build Cost

## Dependency and License Closure

## Deviations from the Approved Design
```

Write `None` when there are no deviations. Otherwise list every actual
deviation, its evidence, and whether it requires a superseding design decision.

Under `Decision`, write exactly `Go` or `No-Go`. Under `Versions`, add a third
bullet containing the exact SQLite runtime value emitted by
`rusqlite::version()` during the accepted run.

Do not write `Go` unless all four CI targets have completed successfully. Local
macOS evidence alone is insufficient.

- [ ] **Step 3: Apply the decision rule**

Choose `Go` only when:

- four-platform build and smoke are green;
- immutable readers create no sidecars and do not wait for another generation's
  writer;
- every crash point yields no accepted partial database;
- corruption becomes a doctor failure or cache miss;
- the 1M fixture meets the two-second warm query P95 target;
- binary size, build time, RSS, and dependency closure are acceptable and
  recorded.

Choose `No-Go` when any condition fails. On `No-Go`, stop before the production
plan, document the evidence, and revise the design toward adjacency shards. Do
not silently switch to RocksDB.

- [ ] **Step 4: Commit the spike evidence**

Because `docs/superpowers/` is ignored, force-add only the approved design and
plan files that belong to this work. Do not force-add other ignored content.

```bash
rtk git add docs/persistent-symbol-index-sqlite-spike-results.md
rtk git add -f docs/superpowers/specs/2026-07-27-persistent-symbol-index-design.md docs/superpowers/plans/2026-07-27-persistent-symbol-index-storage-spike.md docs/superpowers/plans/2026-07-27-persistent-symbol-index.md
rtk git commit -m "docs: record sqlite storage spike decision"
```

- [ ] **Step 5: Stop at the checkpoint**

Expected on `Go`: the next executable document is
`docs/superpowers/plans/2026-07-27-persistent-symbol-index.md`.

Expected on `No-Go`: no B1-B7 production task starts until a superseding storage
decision and plan are approved.

## B0 Acceptance Checklist

- [ ] Optional SQLite is absent from default product binaries.
- [ ] Pinned bundled SQLite builds on all four release targets.
- [ ] Fixed staging schema, transaction, integrity, and root checks pass.
- [ ] Published generations are immutable and no-clobber.
- [ ] Read-only immutable queries create no sidecars.
- [ ] Readers of generation A do not wait for a writer of generation B.
- [ ] Crash and corruption fixtures never produce trusted partial data.
- [ ] 10k, 100k, and 1M measurements are recorded.
- [ ] Warm one-hop/two-hop P95 is at or below two seconds.
- [ ] Binary, build, RSS, dependency, license, and SBOM cost is recorded.
- [ ] A four-platform `Go` or evidence-backed `No-Go` decision is committed.
