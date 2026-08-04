# Rust-Analyzer Context Provider

## Status

This is an opt-in semantic provider for local developer tooling and code review
infrastructure. It is not a network-security product. Delivery 4 exposes both
the bounded library API and an explicit standalone CLI. Delivery 5B adds
reviewed real-provider packs and explicit transactional installation, but no
provider surface is part of the default review, Fast Mode, repository index,
SQLite persistence, or static-analysis orchestration paths.

Deterministic adversarial tests continue to use an independent fake LSP server.
The core package contains only the project-owned adapter CLI and contracts; a
real `rust-analyzer` enters a managed target only through explicit copy-mode
installation of the reviewed current-platform Delivery 5B pack.

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

## Installed Pack Boundary

A real rust-analyzer executable exists only after an operator explicitly runs
copy-mode installation with `install.sh --with-rust-analyzer`. The installer
selects one reviewed current-platform pack and generates the compact,
digest-bound files below the managed target:

```text
runtime/providers/rust-analyzer.profile.json
runtime/providers/provider-registry.json
```

`--no-download --with-rust-analyzer` permits only a verified canonical-cache
hit. A cache miss, bad pack, probe failure, or authorization-generation failure
leaves an existing target unchanged. `--with-rust-analyzer --link` is rejected
before download or mutation, because generated absolute paths must belong to a
copied target. Moving that target invalidates the generated paths; the
target-aware doctor reports drift without rewriting, downloading, or selecting
a replacement.

The provider is still an explicit CLI lane. Ordinary review, Fast Mode,
repository index, SQLite persistence, and static-analysis orchestration never
read a provider registry implicitly. Runtime execution never downloads a pack,
searches `PATH`, invokes `rustup` or a package manager, resolves a direct
upstream archive, or falls back to a global registry. The only accepted
registry is the caller-supplied target-local absolute path with its expected
SHA256.

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

## Release Evidence

Hosted provider gates sample the complete child process tree at intervals no
greater than 100 ms and fail closed when required accounting is unavailable.
The RSS limit is an observed acceptance threshold, not a claim of universal
kernel containment or hostile-code isolation. The evidence records only the
bounded peak and accounting status.

Release latency uses 20 isolated hosted samples. Each sample includes spawn,
readiness, bounded traversal, report normalization, and cleanup, while pack
provisioning and extraction stay outside the measurement. The nearest-rank p95
must not exceed the integer threshold `ceil(reviewed_p95 * 5 / 4) + 250` ms.

Provider pack SBOMs make component-level claims only: the pinned source lock,
upstream archive, executable, copied license, normalized pack manifest, and
generator are bound to the release evidence. They do not claim a complete
upstream transitive dependency closure. Published packs require immutable
GitHub releases and a sidecar plus scoped project attestation before extraction.
Revocations remain local and offline; an older installed manifest cannot learn
about a later revocation until a newer reviewed core is installed.

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

## Real-Host Smoke Runner Contract

`.github/workflows/real-host-smoke.yml` targets the `self-hosted,node24`
runner label. The label is an operator-managed admission check: the runner
must be Actions Runner `v2.327.1` or newer because the pinned
`actions/checkout` v7 and `actions/upload-artifact` v7 actions execute with
Node 24. Before enabling the label, verify the installed runner package version
on the host and confirm the repository registration exposes both `self-hosted`
and `node24` labels:

```text
gh api repos/junit/pre-commit-review/actions/runners \
  --jq '.runners[] | {name,version,status,labels:[.labels[].name]}'
```

As of the `08be23e` maintenance baseline, this repository has no registered
self-hosted runners (`total_count: 0`), so real-host smoke remains
infrastructure-blocked until an upgraded runner is registered. No CI result
claims that this path has executed.

## Delivery 5 Distribution Boundary

Delivery 5B distributes the pinned real `rust-analyzer` for the four supported
platforms through project-owned normalized packs. The reviewed distribution
includes component-level SBOM/license evidence, real-server fixtures, release
fuzz gates, scoped trust-chain evidence, sampled process-tree RSS, and
pack-versioned latency baselines. These claims apply only to the exact active
manifest records and do not broaden the Delivery 4 adapter or fake-server
contracts.

Manual core workflow dispatches are build-and-verify only. Core attestation and
publication require a pushed immutable `v*` version tag, and the publishing job
fails unless the repository immutable-releases setting is enabled.
