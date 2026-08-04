# Persistent Symbol Index SQLite Spike Results

## Decision

Go

## Versions

- rusqlite: 0.40.1
- libsqlite3-sys: 0.38.1
- SQLite runtime: 3.53.2

## Platform Matrix

GitHub Actions run
[30233625849](https://github.com/junit/pre-commit-review/actions/runs/30233625849)
completed successfully for commit
`019c58c68da540e6c20c18c34eeeade698f4a8f8`. The run used the explicit
`spike_only=true` manual mode, and the release publication job was skipped.

| Target | Build | Build/Doctor Smoke | Immutable Read | Sidecars |
| --- | --- | --- | --- | --- |
| `x86_64-unknown-linux-musl` | Pass | Pass | Pass | 0 |
| `aarch64-apple-darwin` | Pass | Pass | Pass | 0 |
| `x86_64-apple-darwin` | Pass | Pass | Pass | 0 |
| `x86_64-pc-windows-msvc` | Pass | Pass | Pass | 0 |

Each target built the ordinary release binaries and the isolated bundled
SQLite spike. The native smoke built a generation, opened it through doctor,
and rejected any published `-wal`, `-shm`, or `-journal` sidecar.

## Scale Results

The accepted local release measurements used Rust 1.95.0 and 1,000 bounded
queries per fixture on macOS. These are measured workstation results, not
universal cold-build latency limits.

| Symbols | Edges | DB bytes | Build ms | Cold open ms | Query P50/P95/P99 us | Peak RSS |
| ---: | ---: | ---: | ---: | ---: | --- | ---: |
| 10,000 | 10,000 | 2,359,296 | 70 | 18 | 5/6/7 | 12,304,384 |
| 100,000 | 100,000 | 23,932,928 | 663 | 200 | 6/7/9 | 13,238,272 |
| 1,000,000 | 1,000,000 | 241,291,264 | 7,017 | 2,214 | 6/9/14 | 13,434,880 |

All three reports completed with zero published sidecars. The 1M warm query
P95 was 9 microseconds, below the two-second acceptance limit.

## Crash and Corruption Results

- All four injected exits (`before-commit`, `after-commit`, `after-sync`, and
  `before-publish`) exited with code 99 and left either no generation or a
  doctor-valid immutable generation. No graph sidecars remained.
- Doctor accepted a complete generation and rejected truncation, generation
  metadata mismatch, foreign-key failure, and application-root mismatch.
- No-clobber publication reused an existing valid generation and preserved an
  invalid digest-named file byte-for-byte instead of replacing it.
- Twenty readers of generation A completed within the 750 ms test bound while
  generation B was built concurrently. Readers created no files beside A.
- The complete 13-test SQLite spike integration suite passed locally.

## Binary and Build Cost

- Default `repository-context-cli`: 3,433,536 bytes.
- Temporary `sqlite-storage-spike`: 2,165,328 bytes.
- The default dependency graph excludes `rusqlite`; the dependency is activated
  only by `sqlite-storage-spike` during B0.
- The temporary spike binary is built and smoke-tested on release targets but is
  not copied into release artifacts.
- In the accepted four-platform run, the incremental spike build step took
  approximately 39 seconds on macOS arm64, 62 seconds on Linux musl, 65 seconds
  on Windows MSVC, and 66 seconds on macOS Intel after each ordinary release
  build.
- A CycloneDX 1.5 comparison contained 43 components and 53,149 bytes for the
  default feature set versus 50 components and 60,716 bytes with the spike
  feature. The delta was seven components and 7,567 bytes.

## Dependency and License Closure

The pinned feature closure contains `rusqlite 0.40.1` and
`libsqlite3-sys 0.38.1` with bundled SQLite. `rusqlite` default features are
disabled; cache, wasm, load-extension, SQLCipher, bindgen, backup, and session
features are not enabled.

The seven feature-only SBOM components are:

- `fallible-iterator 0.3.0`
- `fallible-streaming-iterator 0.1.9`
- `libsqlite3-sys 0.38.1`
- `pkg-config 0.3.33`
- `rusqlite 0.40.1`
- `smallvec 1.15.2`
- `vcpkg 0.2.15`

The exact rusqlite MIT license and SQLite public-domain dedication are stored in
`THIRD_PARTY_LICENSES/`. The default product SBOM remains unchanged until B1
promotes the dependency into the production repository index.

## Deviations from the Approved Design

- The production implementation plan assumed a minimum Rust version of 1.89.
  Actual compilation with `rusqlite 0.40.1` failed on Rust 1.91.1 through 1.94.0
  because its dependency closure uses `cfg_select!`, which is unavailable on
  those toolchains. Rust 1.95.0 compiled and passed the complete gate. The
  production plan is therefore corrected to minimum Rust 1.95. This changes the
  toolchain prerequisite but does not require a superseding storage decision.
- No storage-architecture deviation was required. The accepted implementation
  retains immutable digest-named generations, DELETE journal staging,
  no-clobber publication, immutable readers, bounded traversal, integrity
  validation, and no RocksDB dependency.
