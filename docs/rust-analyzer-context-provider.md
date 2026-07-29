# Rust-Analyzer Context Provider

## Status

This is an opt-in semantic provider for local developer tooling and code review
infrastructure. It is not a network-security product. Delivery 4 exposes both
the bounded library API and an explicit standalone CLI, but neither surface is
part of the default review, Fast Mode, repository index, SQLite persistence, or
static-analysis orchestration paths.

The current delivery uses an independent fake LSP server for deterministic
tests. Delivery 4 packages only the project-owned adapter CLI and its contracts.
Delivery 4 does not bundle or download a real `rust-analyzer` artifact.

## Explicit CLI Workflow

The standalone binary has two commands. Use
`repository-context-provider-cli model` to construct a canonical linked-project
model from the authoritative candidate, then use
`repository-context-provider-cli run` with an explicit registry entry, model,
and bounded request. The compatibility wrapper at
`scripts/run_repository_context_provider.sh` forwards the same arguments to an
explicit override, a local release build, or the packaged adapter CLI. It never
searches `PATH`, discovers a registry, or resolves `rust-analyzer`.

The input contracts are:

- `collect-diff-context-cli/schemas/repository-context-provider-registry.schema.json`
- `collect-diff-context-cli/schemas/repository-context-provider-run-request.schema.json`

Generate a snapshot-bound model with the opening control-plane scope
fingerprint:

```text
repository-context-provider-cli model \
  --source staged \
  --expect-scope <opening-scope-fingerprint> \
  --max-model-files 1000 \
  --max-model-bytes 8388608 \
  > /absolute/trusted/provider-model.json
```

The model builder is snapshot-only: the CLI opens and materializes the
authoritative candidate, while the builder reads only that bounded snapshot.
It never runs Cargo, rustc, build scripts, Git, or another process. Model output
is one compact JSON value with a canonical semantic digest.

Run an explicitly authorized registry entry after computing the exact SHA256
of the registry and model files (`sha256sum`, or `shasum -a 256` on macOS):

```text
repository-context-provider-cli run \
  --source staged \
  --expect-scope <opening-scope-fingerprint> \
  --registry /absolute/trusted/provider-registry.json \
  --expect-registry-sha256 <registry-file-sha256> \
  --provider-id <provider-id> \
  --model /absolute/trusted/provider-model.json \
  --expect-model-sha256 <model-file-sha256> \
  --request /absolute/trusted/provider-request.json
```

The registry digest is checked before its profile or executable is opened. The
selected entry then binds the exact profile, executable, configuration, target,
toolchain mode, and model identity. All input paths must be absolute; drift in
the scope, snapshot, registry, model, profile, or executable fails closed.

Exit code `0` means the command emitted its complete contract output; for
`run`, that includes safe `partial` or `unavailable` reports. Exit code `2`
means arguments, contracts, scope, authorization, or a digest binding were
rejected. Exit code `3` means cancellation or runtime/session failure prevented
a safe report. Successful stdout contains exactly one model or provider-report
JSON value. Errors are bounded stable codes on stderr; child stderr, local
runtime paths, and raw JSON-RPC messages are never forwarded.

## Inputs And Binding

The public library runner accepts a borrowed, already materialized
`CandidateSnapshot`, a validated `RustAnalyzerProjectModel`, an authorized
profile, and a request whose candidate/provider digests match those values. It
never discovers a repository, invokes Git, reads the original worktree, or
accepts an arbitrary directory as a snapshot. The CLI constructs these
bindings itself from the authoritative scope and explicit contract files; it
does not trust caller-supplied candidate or provider bindings.

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
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --features test-fixture --test repository_context_provider_cli_contracts --test repository_context_provider_model --test repository_context_provider_cli
rtk bash tests/repository_context_provider_cli_test.sh
rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets --all-features -- -D warnings
rtk cargo +nightly fuzz run repository_context_frame --fuzz-dir collect-diff-context-cli/fuzz -- -runs=256 -timeout=5
rtk cargo +nightly fuzz run repository_context_messages --fuzz-dir collect-diff-context-cli/fuzz -- -runs=256 -timeout=5
```

## Deferred Release Work

Delivery 5 owns any decision to distribute pinned real `rust-analyzer`
artifacts on supported platforms, plus artifact-specific SBOM/license closure,
real-server fixture evidence, a sustained fuzz campaign, trust-chain evidence,
and resource/latency benchmarks. None of those claims are implied by the
Delivery 4 adapter CLI, packaged contracts, or fake-server gates.
