# pre-commit-review core v0.1.0

This is the first immutable core release for the Rust-backed review tooling.

Highlights:

- Rust 1.95 locked multi-platform binaries for Linux, macOS, and Windows.
- Snapshot-bound static-analysis and repository-context CLIs with explicit
  control-plane scope and digest bindings.
- SQLite-backed repository indexing with bounded build, doctor, and inspect
  commands.
- Explicit rust-analyzer provider CLI and reviewed provider-pack boundary;
  provider packs remain separate artifacts.
- Release evidence includes pinned manifests, sidecar SHA-256 files, Cargo
  SBOM data, and GitHub build attestations.

Pre-tag validation for commit `3512594` is recorded by GitHub Actions run
[`30919103549`](https://github.com/junit/pre-commit-review/actions/runs/30919103549),
which passed the Rust, integration, fuzz, release-trust, and multi-platform
gates. The dedicated coverage job enforces at least 80 percent line coverage
and reported 80.02 percent (31,385 of 39,223 production lines). Its denominator
excludes only feature-gated fixture harness sources. The tagged release
workflow remains the final build and publication evidence for `v0.1.0`.
