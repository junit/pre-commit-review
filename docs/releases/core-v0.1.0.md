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

Validation for this release is recorded by the tagged GitHub Actions run. The
repository does not claim a line-coverage threshold until the dedicated
coverage gate is added in a follow-up maintenance change.
