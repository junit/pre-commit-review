# Controlled Rust-Analyzer Repository Context Provider Design

## Status

Approved for implementation planning on 2026-07-28.

This document defines Phase 2 from
[Whole-Repository Symbols and Call Graph Options](../../call-graph-open-source-options.md):
an opt-in, bounded rust-analyzer Call Hierarchy provider. It follows the
persistent heuristic repository index from Subproject B, but it is an
independent delivery. Subproject B's local implementation and release gates are
not reclassified by this document; its final four-platform remote evidence
remains a separate completion requirement.

## Decision Summary

Add an independent `repository_context_provider` contract and a synchronous,
bounded rust-analyzer LSP client. The provider consumes an already materialized,
read-only candidate snapshot plus exact scope, candidate, project-model, binary,
and configuration bindings. It queries only explicitly supplied changed Rust
functions and their incoming or outgoing callers within one or two hops.

The provider does not:

- materialize a candidate snapshot;
- read the original repository or its `.git` metadata;
- run in the ordinary review or Fast Mode path;
- write FileFacts or SQLite Repository Graph generations;
- replace or silently upgrade Tree-sitter or repository-index edges;
- persist opaque LSP session data;
- claim that best-effort offline process configuration is an OS network sandbox.

The first implementation cycle covers the provider contract, strict snapshot
URI and range mapping, bounded JSON-RPC transport, managed process lifecycle,
and rust-analyzer initialize and Call Hierarchy requests. A standalone CLI,
real-server release profiles, sustained fuzzing, and four-platform release
gates remain later tasks in the same Phase 2 delivery.

## Context

Subproject B now provides deterministic syntax facts and a persistent heuristic
repository graph. Those facts deliberately do not perform compiler-backed type
or method resolution. rust-analyzer can provide higher-confidence semantic call
relationships through the LSP Call Hierarchy requests:

- `textDocument/prepareCallHierarchy`;
- `callHierarchy/incomingCalls`;
- `callHierarchy/outgoingCalls`.

LSP does not provide a bulk call graph, a cross-session stable symbol identity,
or a standard persistent representation for `CallHierarchyItem.data`.
rust-analyzer also expects a filesystem workspace and may normally use Cargo,
build scripts, procedural macros, check-on-save, and dependency discovery.
Those defaults do not satisfy this project's candidate binding and execution
trust model.

The existing static-analysis runner is useful precedent but not the provider
protocol. It closes stdin and treats stdout as one completed report. An LSP
client must maintain a bounded bidirectional session, correlate request ids,
answer a small set of server requests, and terminate the complete process tree
on every failure path.

## Goals

- Bind every accepted symbol and edge to an exact candidate snapshot, review
  scope, passive project model, provider binary, provider version, and hardened
  configuration.
- Query changed Rust functions and at most two incoming or outgoing hops.
- Preserve semantic provider provenance alongside, rather than over, heuristic
  repository-index evidence.
- Reject stale, external, malformed, or lossy URI mappings.
- Convert negotiated LSP positions into validated repository source ranges.
- Bound headers, frames, messages, pending requests, nodes, edges, source bytes,
  stderr, total output, and elapsed time.
- Kill and reap rust-analyzer and its descendants after completion, timeout,
  crash, invalid output, or caller cancellation.
- Return honest `completed`, `partial`, `unavailable`, `timeout`,
  `invalid-output`, and `failed` states.
- Keep deterministic tests independent of an installed rust-analyzer binary.

## Non-Goals

- A long-lived daemon shared across reviews.
- A complete whole-repository call graph export.
- Runtime dispatch completeness.
- Automatic dependency installation, Cargo fetching, project generation,
  builds, tests, build scripts, or procedural macro execution.
- Automatic execution during ordinary review, Fast Mode, or `index build`.
- Persistent semantic graph storage in the first delivery.
- SCIP, clangd, gopls, Joern, or cross-language call support.
- Expanding `ImpactContext.changed_symbols` to include unchanged related
  symbols.
- Reusing the static-analysis finding contract for semantic graph facts.

## Chosen Architecture

```text
Trusted Control Plane
  |
  | BoundCandidateSnapshot + ProviderRequest
  v
Repository Context Provider
  +-- request and binding validation
  +-- snapshot verifier
  +-- strict SnapshotUriMapper
  +-- pinned executable/runtime preparation
  +-- bounded LSP transport
  +-- rust-analyzer session state machine
  +-- deterministic 1-2 hop traversal
  +-- symbol/range/edge normalizer
  |
  v
RepositoryContextProviderReport
  +-- seed symbols
  +-- related symbols
  +-- semantic call edges
  +-- completeness and limitations
  +-- execution and budget metrics
```

The implementation lives in a new
`collect_diff_context_cli::repository_context_provider` module. It is not
called from `impact_context::engine`, `static_analysis::orchestration`, or the
current `repository-context-cli collect` path.

The module has five narrow components:

1. `contract` validates requests and reports.
2. `snapshot` owns binding verification, URI mapping, and source range
   conversion.
3. `json_rpc` owns Content-Length framing and request correlation.
4. `session` owns the child process and LSP lifecycle.
5. `rust_analyzer` owns provider-specific configuration, request types,
   traversal, and normalization.

The managed child process reuses or extracts the existing process-group,
private runtime, environment allowlist, pinned executable copy, timeout, and
integrity-checking policies. It does not reuse the one-shot stdout capture API.

## Candidate Binding

The provider accepts a `BoundCandidateSnapshot`, not a repository path and not
an arbitrary directory string. The binding contains:

```text
source
scope_fingerprint
candidate_digest
snapshot_root
snapshot_sha256
snapshot_files
snapshot_bytes
project_model_fingerprint
```

`scope_fingerprint`, `candidate_digest`, and `snapshot_sha256` are distinct
identities and must not substitute for one another:

- scope fingerprint binds the selected Git review state and control-plane
  configuration;
- candidate digest binds the candidate manifest and preparation outcomes;
- snapshot SHA256 binds the exact materialized filesystem tree supplied to the
  language server.

Before starting a process, the provider must:

- require a canonical absolute snapshot root;
- require the root to be a directory;
- reject a `.git` file or directory anywhere in the snapshot root;
- verify file count, byte count, modes, safe symlinks, read-only state, and
  snapshot SHA256 with the existing snapshot rules;
- validate every seed path with `RepoPath` and require it to exist inside the
  snapshot;
- validate the supplied project-model fingerprint;
- require the provider executable and profile outside the snapshot.

The project-model fingerprint is recomputed with a versioned provider
algorithm over the snapshot-local Rust project-model inputs. A caller-supplied
digest is never accepted on assertion alone. The algorithm id and resulting
digest are recorded in the report so a later implementation change cannot
silently reuse the old identity.

After shutdown or forced termination, the provider repeats snapshot, binary,
and profile verification. A mismatch invalidates the entire report. No edge
collected before a binding failure is accepted.

The adapter never receives the original repository path and never runs Git.
The trusted caller owns materialization and authoritative scope revalidation.

## Provider Request

The version 1 request contains:

```json
{
  "schema_version": 1,
  "kind": "repository_context_provider_request",
  "candidate": {
    "source": "staged",
    "scope_fingerprint": "<40-or-64-lowercase-hex>",
    "candidate_digest": "<64-lowercase-hex>",
    "snapshot_root": "/absolute/read-only/snapshot",
    "snapshot_sha256": "<64-lowercase-hex>",
    "snapshot_files": 123,
    "snapshot_bytes": 456789,
    "project_model_fingerprint": "<64-lowercase-hex>"
  },
  "provider": {
    "kind": "rust-analyzer",
    "version": "<pinned-version>",
    "executable_path": "/absolute/trusted/rust-analyzer",
    "executable_sha256": "<64-lowercase-hex>",
    "configuration_sha256": "<64-lowercase-hex>"
  },
  "seeds": [],
  "directions": ["incoming", "outgoing"],
  "limits": {}
}
```

The request is evaluated together with an `AuthorizedProviderProfile` supplied
by the trusted control plane. The profile is not selected from snapshot
content. It contains the canonical hardened configuration and executable
authorization; the request's version and digests must match it exactly.

Each seed contains an existing changed-symbol id, a `RepoPath`, symbol kind and
name, and a validated one-based `SourceRange`. Version 1 accepts Rust function,
method, and test-function seeds only.

Limits may lower built-in maxima but cannot raise them. They include:

- total session deadline;
- maximum depth, restricted to 1 or 2;
- maximum seeds;
- maximum LSP requests and pending requests;
- maximum notifications;
- maximum header and frame bytes;
- maximum cumulative protocol bytes and stderr bytes;
- maximum source file bytes opened through LSP;
- maximum nodes, edges, and encoded report bytes.

## Provider Report

`RepositoryContextProviderReport` is separate from `ImpactContext`. This avoids
the existing invariant that an `ImpactEdge.to_symbol` must be present in the
`changed_symbols` table. Semantic callers and callees are usually unchanged and
must not be mislabeled as changed.

The report contains:

- the complete candidate binding, excluding the local snapshot path;
- provider id, version, executable digest, configuration digest, and negotiated
  position encoding;
- overall provider status;
- index and query completeness;
- `seed_symbols`;
- `related_symbols`;
- semantic call edges;
- sorted limitations;
- session, protocol, traversal, byte, and elapsed-time metrics;
- an isolation record stating that network prevention is best-effort.

Symbols use snapshot-local deterministic ids derived from:

```text
provider id and version
provider configuration digest
project-model fingerprint
candidate digest
repository-relative path
kind and name
validated source range
```

The same binding and input yields the same id and ordering. No stability is
promised after a provider version, configuration, project model, candidate, or
range change.

Edges use the existing call-edge semantics:

- `kind = calls`;
- `resolution = semantic` for accepted Call Hierarchy relationships;
- `confidence = high` for a concrete server-returned item;
- provider id and version remain explicit;
- call-site path and range refer to the caller's snapshot file.

Provider edges do not overwrite or deduplicate away Tree-sitter syntactic or
repository-index heuristic edges. A later consumer may correlate facts while
preserving each provider's provenance.

The LSP `CallHierarchyItem.data` field is retained only in bounded memory for
follow-up requests in the same session. It is not logged, returned, hashed into
stable ids, or persisted.

## Strict URI Mapping

`SnapshotUriMapper` is the only path from an LSP URI to a `RepoPath`.

It must:

- accept only `file:` URIs;
- reject credentials, query strings, and fragments;
- percent-decode without lossy conversion;
- handle the platform's file-URI authority and drive rules explicitly;
- normalize and canonicalize the referenced file;
- require the canonical path to be strictly below the canonical snapshot root;
- reject the snapshot root itself;
- reject missing files, directories, unsupported file types, and symlink
  escapes;
- convert the relative path through `RepoPath::new`;
- return an explicit limitation for non-UTF-8 paths that LSP cannot represent
  losslessly.

The existing static-analysis path normalizer is intentionally not reused
because it may retain absolute paths outside its repository root.

URI failures do not expose the local path in the report. They use bounded codes
such as:

- `provider-uri-invalid`;
- `provider-uri-outside-snapshot`;
- `provider-uri-stale`;
- `provider-uri-non-utf8`.

An invalid URI omits only the affected symbol or edge when the remainder of the
session is trustworthy. Repeated invalid URIs may exhaust the invalid-output
budget and invalidate the session.

## Position And Range Mapping

LSP positions are zero-based and use a negotiated encoding. Repository
`SourceRange` is one-based and also records byte offsets.

The client advertises UTF-8 and UTF-16. It uses the server's returned
`positionEncoding`; absent negotiation defaults to UTF-16 as required by LSP.
For every returned range, the adapter reads the corresponding snapshot file
within the source-byte budget and converts positions against the exact bytes.
Before `prepareCallHierarchy`, it also validates each seed's one-based range
and byte offsets against those bytes and converts the selected seed position
into the negotiated LSP encoding. Both directions use the same checked line
index and reject non-boundary offsets.

Conversion rejects:

- a line beyond end of file;
- a character beyond the line;
- a UTF-16 offset inside a surrogate pair;
- a UTF-8 offset inside a code point;
- an end position before the start;
- invalid UTF-8 Rust source;
- integer or byte-count overflow.

Invalid positions omit the affected fact and record `provider-range-invalid`.
The provider never guesses byte offsets or silently clamps a range.

## Bounded JSON-RPC Transport

The client implements only LSP Content-Length framing over stdin and stdout.
It does not add an async runtime.

The reader accepts ASCII headers terminated by `\r\n\r\n`, requires exactly one
valid decimal `Content-Length`, bounds header and body sizes before allocation,
and parses one JSON value per frame. It rejects duplicate lengths, conflicting
lengths, unsupported transfer framing, malformed JSON, and frames beyond the
remaining cumulative byte budget.

Every outbound request uses a monotonically increasing integer id. The session
tracks a bounded pending-id set and accepts responses in any order. Unknown,
duplicate, or already-completed ids count as invalid output. Notifications and
server requests share separate count limits.

The server-request policy is fixed:

- `workspace/configuration`: return only the hardened configuration;
- `window/workDoneProgress/create`: acknowledge without granting new behavior;
- `client/registerCapability`: acknowledge only bounded non-execution
  registrations and never use them to bypass the initial capability gate;
- `workspace/applyEdit`: return `applied: false`;
- unknown requests: return JSON-RPC MethodNotFound;
- unknown notifications: ignore within the notification budget.

No protocol message is written to the model-facing report. stderr is captured
only for bounded diagnostics and hashes; raw paths and server output do not
become semantic facts.

## Rust-Analyzer Session State Machine

The state machine is linear except for bounded query correlation:

```text
Preflight
  -> Spawn
  -> Initialize
  -> CapabilityGate
  -> Initialized
  -> OpenSeeds
  -> PrepareHierarchy
  -> TraverseIncomingOutgoing
  -> Shutdown
  -> Exit
  -> FinalVerification
  -> Report
```

### Spawn

The provider copies the authorized binary into a private runtime and verifies
the copy. The process uses the snapshot as its working directory, no shell,
cleared environment, private `HOME` and temporary directories, a minimal system
`PATH`, and the existing cross-platform process-group abstraction.

The environment sets:

- fixed locale and `NO_COLOR`;
- `CARGO_NET_OFFLINE=true`;
- `RUSTUP_AUTO_INSTALL=0`;
- invalid loopback HTTP, HTTPS, and all-proxy endpoints;
- an empty `NO_PROXY`;
- the bound scope and source for diagnostics.

This is recorded as best-effort offline. It is not described as an OS network
sandbox.

### Initialize And Capability Gate

Initialization uses only the snapshot `file:` URI and a fixed client
capability set. Hardened rust-analyzer settings include:

- `cargo.buildScripts.enable = false`;
- `cargo.noDeps = true`;
- `procMacro.enable = false`;
- check-on-save disabled;
- no automatic workspace edits;
- no dependency fetching or project preparation by this provider.

The configuration JSON is canonicalized and bound by SHA256. If the server does
not advertise Call Hierarchy support, the result is `unavailable`; no hierarchy
requests are sent.

### Open, Prepare, And Traverse

The adapter reads each seed file from the snapshot within budget and sends one
bounded `textDocument/didOpen`. It sends
`textDocument/prepareCallHierarchy` at the seed position and retains only items
whose URI and range validate.

Traversal is deterministic breadth-first search. It sorts prepared and returned
items by snapshot-local stable id, deduplicates nodes and edges, and queries each
item at most once per requested direction and depth. Cycles terminate through
the visited set. All one-hop and two-hop work shares the same request, message,
node, edge, source-byte, protocol-byte, report-byte, and deadline budgets.

Incoming call ranges are interpreted in the caller item. Outgoing call ranges
are interpreted in the current caller item. Empty or null results are valid and
do not imply repository-wide completeness.

### Shutdown

On successful or unavailable capability completion, the client sends
`shutdown`, waits within the remaining deadline, sends `exit`, and reaps the
process. On timeout, crash, invalid framing, invalid JSON, output overflow,
snapshot mutation, or shutdown failure, it terminates and reaps the complete
process group.

The managed child has Drop-based termination as a final guard so an early Rust
error cannot leave a language server or descendant running.

## Status And Failure Semantics

The report uses these provider states:

- `completed`: every accepted seed and requested hop completed within budget;
- `partial`: at least one trustworthy fact exists, but a seed returned null, a
  URI or range was omitted, the project model was degraded, or a budget cut off
  remaining work;
- `unavailable`: Call Hierarchy is unsupported or hardened configuration cannot
  establish a usable project model;
- `timeout`: the global deadline expired;
- `invalid-output`: framing, JSON-RPC correlation, URI/range error volume, or
  response structure made the session untrustworthy;
- `failed`: rust-analyzer crashed or could not complete a required lifecycle
  transition.

Timeout, invalid output, crash, and post-execution binding mismatch accept no
partial response still in flight. Previously completed facts may be returned
only when their messages were fully validated and the final snapshot, binary,
and profile checks succeed; otherwise the entire fact set is discarded.

Invalid request JSON, untrusted paths, digest mismatches, writable or mutated
snapshots, and invalid authorization are caller errors. The provider rejects
them before producing a semantic report rather than presenting them as a
language-server limitation.

Limitations have stable codes, bounded human text, optional seed or RepoPath,
and an interpretation that distinguishes precision loss from total
unavailability.

## Security Model

The trusted inputs are:

- the caller that produced the authoritative binding;
- the read-only candidate snapshot after verification;
- a provider profile outside the snapshot whose digest is explicitly
  authorized;
- the rust-analyzer binary whose version and SHA256 match that profile.

All LSP output is untrusted until framing, JSON structure, request correlation,
URI, range, size, and binding checks pass.

The snapshot may contain repository-controlled Cargo metadata and Rust source.
The provider permits rust-analyzer to read those files but does not authorize
repository code execution. Disabled build scripts, procedural macros, and
check-on-save plus offline Cargo settings are mandatory. A profile that cannot
meet those settings is rejected.

The first delivery does not claim to prevent every possible direct network
system call by a compromised authorized rust-analyzer binary. Binary pinning,
process isolation, offline configuration, proxy denial, and process-tree
termination are the enforced cross-platform controls. A future stronger
sandbox may add OS network denial without changing the provider contract.

## Testing Strategy

### Contract And Snapshot Tests

- reject unknown fields and unsupported schema versions;
- reject scope, candidate, source, snapshot, project-model, binary, and config
  binding mismatches;
- reject writable, changed, oversized, or VCS-bearing snapshots;
- preserve safe relative symlinks and reject escaping or looping symlinks;
- reject invalid seeds and unsupported seed kinds;
- verify deterministic report ordering and ids.

### URI And Range Tests

- accept valid Unix and Windows file URIs inside the snapshot;
- reject absolute escapes, percent-encoded escapes, authority misuse, query and
  fragment data, non-file schemes, snapshot-root URIs, directories, missing
  files, and stale symlinks;
- report non-UTF-8 paths without lossy conversion;
- convert ASCII, multi-byte UTF-8, and surrogate-pair UTF-16 positions;
- reject mid-code-point, mid-surrogate, reversed, and out-of-file ranges.

### JSON-RPC Transport Tests

A deterministic fake LSP server covers:

- headers and bodies split across arbitrary reads;
- multiple frames in one read;
- responses arriving out of request order;
- duplicate, missing, conflicting, negative, and oversized Content-Length;
- malformed and oversized JSON;
- unknown, duplicate, and completed response ids;
- notification floods and stderr floods;
- bounded server requests and rejected workspace edits.

The framing parser receives fuzz targets for arbitrary byte streams, bounded
message sequences, and request-id correlation.

### Session And Traversal Tests

- missing Call Hierarchy capability;
- initialize, prepare, incoming, outgoing, shutdown, and exit ordering;
- null or empty prepare results;
- one-hop and two-hop incoming and outgoing traversal;
- cycles, self-calls, fan-out, duplicate items, and duplicate call ranges;
- every request, message, byte, node, edge, source, report, and deadline budget;
- server crash before and after initialization;
- total timeout and shutdown timeout;
- snapshot, profile, or binary mutation before and after execution;
- child and descendant process termination on every early return;
- deterministic output despite response reordering.

### Rust-Analyzer Integration Tests

An opt-in, pinned rust-analyzer fixture verifies:

- capability negotiation and position encoding;
- a known direct function call in a local Rust project;
- incoming and outgoing call-site ranges;
- build-script and procedural-macro marker files are never created;
- check-on-save is never invoked;
- missing dependencies yield explicit `partial` or `unavailable` status;
- offline execution does not fetch dependencies;
- stale and external URIs are rejected by the adapter.

The fake server remains the required deterministic CI gate. Real-server tests
become required release gates only after a pinned four-platform profile and
artifact trust chain are committed.

## Delivery Sequence

### Delivery 1: Contract And Snapshot Boundary

- add request/report contracts and JSON schemas;
- add `BoundCandidateSnapshot` verification;
- add strict URI mapping and position conversion;
- add contract, path, snapshot, and range tests.

### Delivery 2: Bounded Transport And Managed Process

- add Content-Length framing and request correlation;
- extract reusable pinned-runtime and process-tree controls without changing
  existing static-analysis behavior;
- add a Drop-safe managed interactive child;
- add the fake LSP server and transport/lifecycle tests;
- add framing and message-sequence fuzz targets.

### Delivery 3: Rust-Analyzer Adapter

- add hardened initialization configuration;
- add capability gating and server-request responses;
- add prepare, incoming, and outgoing request models;
- add deterministic one-hop and two-hop traversal;
- normalize symbols, semantic edges, limitations, and metrics;
- verify all bindings after shutdown.

The first implementation cycle ends after Delivery 3 is locally verified.

### Delivery 4: Explicit User Surface

- add a standalone provider CLI or an equivalently isolated explicit command;
- add pinned profile authorization and report rendering;
- update capability documentation without enabling ordinary review execution;
- add schema, shell, installer, and workflow gates;
- add release binaries only after profile and artifact policy approval.

### Delivery 5: Release Readiness

- add pinned real rust-analyzer fixtures and platform artifacts;
- run sustained protocol and adapter fuzzing;
- add latency and resource benchmarks;
- verify Linux, macOS arm64/x86_64, and Windows process behavior;
- record SBOM and license closure;
- complete code review and release-readiness documentation.

## Acceptance Criteria

The first implementation cycle is complete when:

- requests cannot be accepted without exact scope, candidate, snapshot,
  project-model, binary, and configuration bindings;
- URI and range mapping never emits an unvalidated or lossy repository path;
- the fake server proves bounded initialize, capability, prepare, incoming,
  outgoing, shutdown, and exit behavior;
- one-hop and two-hop traversal is deterministic and respects every budget;
- timeout, crash, malformed output, stale URI, and snapshot mutation have
  explicit tested outcomes;
- build scripts, proc macros, check-on-save, and dependency fetching remain
  disabled in the fixed configuration;
- no provider code is reachable from ordinary review or Fast Mode;
- no semantic result is persisted or used to overwrite heuristic evidence;
- all affected formatting, Clippy, unit, integration, fuzz smoke, schema, and
  cross-platform process tests pass.

Full Phase 2 release readiness additionally requires Delivery 4 and Delivery 5,
including a pinned real rust-analyzer trust chain and four-platform evidence.

## Rejected Alternatives

### Static-Analysis Shim

Rejected. A separate shim would reduce Rust protocol code but add another
trusted executable and split snapshot, URI, opaque-data, and process-lifecycle
validation across two trust boundaries.

### General Async LSP Client Stack

Rejected for the first delivery. A generic async JSON-RPC client and runtime
would enlarge the dependency and concurrency surface beyond the small set of
requests required by this adapter.

### Direct ImpactContext Integration

Rejected. Existing contracts treat related targets as changed symbols and
currently reject semantic providers. Changing those invariants before the
provider contract is proven would couple Phase 2 to the default review path.

### Persistent Semantic Graph

Rejected for the first delivery. LSP does not define cross-session stable ids,
and opaque provider data is session-local. Persistence requires a separate
identity and invalidation design after the adapter is validated.

## Documentation And Compatibility

The implementation plan must preserve Rust 1.95 support, four-platform process
behavior, ASCII protocol framing, deterministic JSON ordering, and the existing
Subproject B public contracts. Documentation must describe the provider as
opt-in and best-effort offline, and must not call LSP results a complete runtime
call graph.
