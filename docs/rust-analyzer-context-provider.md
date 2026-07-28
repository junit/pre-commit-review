# Rust-Analyzer Context Provider

## Status

This is a library-only, opt-in provider for local developer tooling and code
review infrastructure. It is not a network-security product and is not part
of the default review, Fast Mode, repository index, SQLite persistence, or
static-analysis orchestration paths.

The current delivery uses an independent fake LSP server for deterministic
tests. A real rust-analyzer distribution, sustained fuzzing, and release
artifacts remain deferred work.

## Inputs And Binding

The public runner accepts a borrowed, already materialized `CandidateSnapshot`,
a validated `RustAnalyzerProjectModel`, an authorized profile, and a request
whose candidate/provider digests match those values. It never discovers a
repository, invokes Git, reads the original worktree, or accepts an arbitrary
directory as a snapshot.

Profile and executable paths are outside the snapshot and are checked before
spawn and again after the session. Snapshot, model, profile, and executable
changes return a stale-binding error. Reports contain repository-relative paths
and digests, never local roots.

## Linked Project Model

Initialization sends exactly one canonical inline linked-project object. The
model is digest-bound and contains sorted crates, snapshot-relative root
modules, dependencies, cfg values, environment values, and explicit
limitations. Build scripts, proc macros, dependency fetching, sysroot
discovery, check-on-save, and workspace discovery are disabled.

## Bounded Protocol

The session uses incremental CRLF `Content-Length` framing with limits for
headers, frames, protocol bytes, messages, pending requests, notifications,
server requests, invalid messages, and total output. Requests are single-flight
and correlated by bounded IDs. Server requests are answered by an explicit
policy; unknown requests fail with a JSON-RPC method error.

The runner negotiates UTF-8 or UTF-16 positions, waits for the typed
`experimental/serverStatus` quiescence gate, and sends `didOpen` once per
distinct seed file. Call Hierarchy traversal is a deterministic depth-one or
depth-two BFS. Symbols, call ranges, edges, limitations, and report bytes are
bounded and sorted before publication.

## Execution Isolation

The child runs from a private runtime directory with a pinned executable,
private home/temp/target directories, an empty PATH, no shell, fixed locale,
offline Cargo settings, disabled toolchain installation, and invalid proxy
endpoints. Process-group termination and reader joins are Drop-safe. These are
best-effort offline controls, not an operating-system network sandbox.

## Report Semantics

Reports preserve seed mappings separately from related symbols. Call edges use
`calls`/`semantic`/`high` provenance and retain the provider identity. A
complete bounded query is not a claim of a complete runtime call graph;
`index_completeness` remains `unknown`. Readiness warnings, unresolved or
ambiguous seeds, stale URIs, invalid call ranges, and exhausted fact budgets
produce partial results with explicit limitations. Timeout, invalid output,
crash, unsupported capability, and cancellation never publish facts.

## Known Limitations

Call Hierarchy is a server query protocol, not a full graph export contract.
Dynamic dispatch, macro expansion, missing dependencies, unsupported symbol
kinds, and server-specific readiness can reduce precision. `CallHierarchyItem`
opaque data is retained only for same-session follow-up requests and is never
serialized in the report. The provider does not persist semantic facts or
modify existing impact/index/cache contracts.

## Local Verification

Use the Rust 1.95 locked tests and checks from the implementation plan:

```text
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --features test-fixture --test repository_context_rust_analyzer --test repository_context_provider_platform
rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets --all-features -- -D warnings
rtk cargo +nightly fuzz run repository_context_frame --fuzz-dir collect-diff-context-cli/fuzz -- -runs=256 -timeout=5
rtk cargo +nightly fuzz run repository_context_messages --fuzz-dir collect-diff-context-cli/fuzz -- -runs=256 -timeout=5
```

## Deferred Release Work

Delivery 4/5 must still provide pinned real rust-analyzer artifacts on the
supported platforms, artifact-specific SBOM/license closure, a sustained fuzz
campaign, resource/latency benchmarks, and explicit product/CLI surface
decisions. None of those claims are implied by the fake-server gates here.
