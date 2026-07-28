# Controlled Rust-Analyzer Repository Context Provider Design

## Status

Approved for implementation planning on 2026-07-28. Amended after contract and
LSP compatibility review on the same date.

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
read-only candidate snapshot plus exact scope, candidate, project-model,
profile, binary, and configuration bindings. The normalized project model is
the `linkedProjects` input actually supplied to rust-analyzer. The provider
queries only explicitly supplied changed Rust functions and their incoming
callers or outgoing callees within one or two hops.

The provider does not:

- materialize a candidate snapshot;
- read the original repository or its `.git` metadata;
- run in the ordinary review or Fast Mode path;
- write FileFacts or SQLite Repository Graph generations;
- replace or silently upgrade Tree-sitter or repository-index edges;
- persist opaque LSP session data;
- claim that best-effort offline process configuration is an OS network sandbox.

The first implementation cycle covers the provider, project-model, and profile
contracts; strict snapshot URI and range mapping; bounded JSON-RPC transport;
managed process lifecycle; and rust-analyzer initialize and Call Hierarchy
requests. A standalone CLI, profile/artifact distribution, sustained fuzzing,
and four-platform real-server release gates remain later tasks in the same
Phase 2 delivery.

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
- Persistent semantic graph storage in the first implementation cycle.
- SCIP, clangd, gopls, Joern, or cross-language call support.
- Expanding `ImpactContext.changed_symbols` to include unchanged related
  symbols.
- Reusing the static-analysis finding contract for semantic graph facts.

## Chosen Architecture

```text
Trusted Control Plane
  |
  | &CandidateSnapshot + BoundProjectModel + AuthorizedProfile + ProviderRequest
  v
Repository Context Provider
  +-- request and binding validation
  +-- snapshot verifier
  +-- strict SnapshotUriMapper
  +-- pinned profile/executable/runtime preparation
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

1. `contract` validates requests, project models, profiles, and reports.
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

The provider accepts an in-process `BoundCandidateSnapshot<'a>` that borrows an
existing `&'a CandidateSnapshot`; it never reconstructs snapshot authority from
JSON and never accepts a repository path or arbitrary directory string. The
serialized request repeats these values only for exact comparison and report
provenance:

```text
source
scope_fingerprint
candidate_digest
snapshot_root
snapshot_sha256
snapshot_files
snapshot_bytes
project_model_digest
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
- reject every repository-controlled `rust-analyzer.toml`, at any depth, so
  workspace configuration cannot override the authorized client settings;
- verify file count, byte count, observed modes, directory entries, safe
  symlinks, read-only state, and snapshot SHA256;
- reject added, removed, or renamed empty directories and any `.git` entry;
- validate every seed path through a provider-only `SnapshotFilePath` that
  permits only normalized normal components and resolves to a regular file
  strictly inside the snapshot;
- validate the supplied project-model digest against the bound normalized
  model;
- require the provider executable and profile outside the snapshot.

`CandidateSnapshot::verify_unchanged` is hardened before provider work so it
compares observed modes rather than hashing stored modes, includes directory
entries in the digest, and detects `.git` mutations. A future CLI cannot rebuild
`BoundCandidateSnapshot` from the summarized JSON fields; it must materialize a
new authoritative `CandidateSnapshot` or carry a separately designed complete
manifest.

After shutdown or forced termination, the provider repeats snapshot, binary,
and profile verification. A mismatch invalidates the entire report. No edge
collected before a binding failure is accepted.

The adapter never receives the original repository path and never runs Git.
The trusted caller owns materialization and authoritative scope revalidation.

## Project Model And Toolchain Boundary

The provider consumes a versioned `RustAnalyzerProjectModel`, separate from the
existing heuristic `RustProjectModel`. It contains canonical snapshot-relative
crate roots, editions, target triple, cfg values, environment values, and
crate-to-crate dependencies. Its private constructor validates all roots
against `BoundCandidateSnapshot`, validates dependency ids and deterministic
ordering, and recomputes `project_model_digest` from the complete canonical
model. The request's digest is never accepted without the typed model.

The complete canonical model is supplied as the sole inline JSON object in
rust-analyzer `linkedProjects`; no random private-runtime path participates in
the stable configuration digest. Automatic Cargo workspace discovery is
disabled. The first implementation cycle uses a profile with
`toolchain_mode = none`: Cargo, rustc, build scripts, proc macros, sysroot
discovery, check-on-save, and dependency fetching are disabled, and `PATH`
points only at an empty private runtime directory. The target triple is fixed
by the profile and model and is part of their digests. A future profile that
authorizes Cargo, rustc, or a sysroot requires absolute paths, SHA256 bindings,
and a separate design change.

Index readiness is not observable through standard LSP or Call Hierarchy.
The pinned rust-analyzer protocol does expose the operational
`experimental/serverStatus` notification. The client advertises
`experimental.serverStatusNotification = true` and, after `initialized`, waits
within the global deadline for a status with `quiescent = true` before opening
or querying seed files. Missing status, unhealthy status, or a deadline expiry
produces no queries or facts; a missing status that consumes the global
deadline is `timeout`, while an explicit unhealthy status is `unavailable`.
This barrier prevents early `NO_RETRY` Call Hierarchy requests; it is not
evidence that the semantic index is complete.
First-cycle reports therefore always set index completeness to `unknown`.
Query completeness describes only whether all explicitly requested RPCs
completed; it never implies repository-wide semantic completeness.

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
    "project_model_digest": "<64-lowercase-hex>"
  },
  "provider": {
    "kind": "rust-analyzer",
    "version": "<pinned-version>",
    "profile_path": "/absolute/trusted/profile.json",
    "profile_sha256": "<64-lowercase-hex>",
    "executable_path": "/absolute/trusted/rust-analyzer",
    "executable_sha256": "<64-lowercase-hex>",
    "configuration_sha256": "<64-lowercase-hex>",
    "target_triple": "<pinned-target-triple>",
    "toolchain_mode": "none"
  },
  "seeds": [{
    "changed_symbol_id": "<existing-id>",
    "path": "src/lib.rs",
    "kind": "function",
    "name": "entry",
    "symbol_range": {},
    "selection_range": {},
    "query_byte": 123
  }],
  "directions": ["incoming", "outgoing"],
  "limits": {}
}
```

The profile is loaded from the absolute path with the expected profile SHA256
before parsing. Its schema, canonical digest, executable authorization,
target, toolchain mode, hardened configuration, arguments, and immutable
maximum limits are Delivery 1 requirements. The request's duplicated values
must match it exactly. Profile registry/distribution and a user-facing selector
remain Delivery 4 work; profile authorization itself does not.

Each seed contains an existing changed-symbol id, normalized snapshot file
path, name, versioned end-exclusive symbol and selection ranges, and a query
byte inside the selection range. Version 1 accepts the existing Rust kinds
`function`, `method`, `associated-function`, `function-declaration`,
`method-declaration`, and `associated-function-declaration`. A `#[test]`
attribute does not invent a separate kind.

The selection range must be contained in the symbol range, and `query_byte`
must be a UTF-8 boundary inside the selection range. A prepared item belongs to
the seed only when its URI maps to the same path, its name and LSP kind are
compatible, its selection range is contained in its full range, and its
selection range contains the query position. Zero matches produce a partial
`provider-seed-unresolved` result; multiple matches produce partial
`provider-seed-ambiguous` and no guessed association. The report preserves the
explicit `changed_symbol_id -> provider symbol_id` mapping.

Limits may lower built-in maxima but cannot raise them. They include:

- total session deadline;
- maximum depth, restricted to 1 or 2;
- maximum seeds;
- maximum LSP requests and pending requests, with version 1 fixed to one
  pending client request;
- maximum total messages, notifications, server requests, invalid messages,
  and call ranges;
- maximum header and frame bytes;
- maximum cumulative protocol bytes and stderr bytes;
- maximum source file bytes opened through LSP;
- maximum nodes, edges, and encoded report bytes.

Seeds and directions must be non-empty, sorted, and unique. Every numeric limit
must be positive and cannot exceed these immutable maxima: 30 seconds, depth
2, 64 seeds, 512 client requests, 1 pending request, 2,048 total messages, 512
notifications, 128 server requests, 32 invalid messages, 1,000 call ranges per
response, 16 KiB headers, 4 MiB frames, 64 MiB cumulative protocol bytes, 1 MiB
stderr, 65 MiB combined process output, 4 MiB per source file, 64 MiB source
bytes, 5,000 nodes, 10,000 edges, and 16 MiB encoded report bytes. Counters are
inclusive: consuming the maximum succeeds; the next unit exhausts the budget.

## Provider Report

`RepositoryContextProviderReport` is separate from `ImpactContext`. This avoids
the existing invariant that an `ImpactEdge.to_symbol` must be present in the
`changed_symbols` table. Semantic callers and callees are usually unchanged and
must not be mislabeled as changed.

The report contains:

- the complete candidate binding, excluding the local snapshot path;
- provider id, version, profile digest, executable digest, configuration digest,
  target triple, and negotiated position encoding;
- overall provider status;
- `index_completeness = unknown` for this implementation cycle and query
  completeness for the explicitly requested RPCs;
- `seed_symbols`;
- `related_symbols`;
- semantic call edges;
- sorted limitations;
- session, protocol, traversal, byte, and elapsed-time metrics;
- an isolation record stating that network prevention is best-effort.

Symbols use deterministic ids derived from a length-prefixed full binding digest
and these fields:

```text
scope fingerprint
candidate digest
snapshot SHA256
project-model algorithm and digest
profile SHA256
provider id and version
executable SHA256 and configuration digest
repository-relative path
kind and name
validated source range and selection range
```

The same complete binding and input yields the same id and ordering. IDs are
valid only for a report with that full binding; no consumer may use them as
cross-report identity after any binding component changes.

Edges use the existing call-edge semantics:

- `kind = calls`;
- `resolution = semantic` for accepted Call Hierarchy relationships;
- `confidence = high` for a concrete server-returned item;
- provider id and version remain explicit;
- call-site path and range refer to the caller's snapshot file.

Each returned call-site range produces one edge. Identical caller, callee, path,
and range tuples deduplicate; distinct ranges remain distinct edges and each
consumes one edge budget unit.

Provider edges do not overwrite or deduplicate away Tree-sitter syntactic or
repository-index heuristic edges. A later consumer may correlate facts while
preserving each provider's provenance.

The LSP `CallHierarchyItem.data` field is retained only in bounded memory for
follow-up requests in the same session. It is not logged, returned, hashed into
stable ids, or persisted.

## Strict URI Mapping

`SnapshotUriMapper` is the only path from an LSP URI to a provider
`SnapshotFilePath` and then to a `RepoPath`.

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
- reject `.`, `..`, repeated separators, empty components, and trailing
  separators in the provider file path before converting through `RepoPath::new`;
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

LSP positions are zero-based and use a negotiated encoding. Provider ranges are
versioned, end-exclusive, and use one-based UTF-8 byte columns plus byte
offsets: `provider-source-range-v1/utf8-byte-columns/end-exclusive`. They are
not the untyped legacy `SourceRange` representation.

The client advertises UTF-8 and UTF-16. It uses the server's returned
`positionEncoding`; a returned value not present in that offer is invalid
output, while absent negotiation defaults to UTF-16 as required by LSP.
For every returned item, the adapter reads the corresponding snapshot file
within the source-byte budget, validates the full range and `selectionRange`
containment, and converts positions against the exact bytes. Before
`prepareCallHierarchy`, it validates each seed's symbol/selection range and
`query_byte`, then converts that byte into the negotiated LSP encoding. Both
directions use the same checked line index.

Conversion rejects:

- a line beyond end of file;
- invalid UTF-8 Rust source;
- a UTF-16 offset inside a surrogate pair;
- a UTF-8 offset inside a code point;
- an end position before the start;
- integer or byte-count overflow.

LSP permits a character beyond the line to normalize to the line end. The
adapter performs that normalization only with an explicit
`provider-position-normalized` limitation; it never silently clamps a range.
LF, CRLF, and bare CR line endings are treated as end-exclusive boundaries: a
range crossing a terminator ends at the next line's character zero. Empty
lines, EOF, and a final line without a terminator follow the same checked line
index. Invalid positions omit the affected fact and record
`provider-range-invalid`.

## Bounded JSON-RPC Transport

The client implements only LSP Content-Length framing over stdin and stdout.
It does not add an async runtime.

The reader accepts ASCII headers terminated by `\r\n\r\n`, requires exactly one
valid decimal `Content-Length`, bounds header and body sizes before allocation,
and parses one JSON value per frame. It rejects duplicate lengths, conflicting
lengths, unsupported transfer framing, malformed JSON, and frames beyond the
remaining cumulative byte budget.

Every outbound request uses a monotonically increasing integer id. The generic
transport tracks a bounded pending-id set and accepts responses in any order;
the version-1 provider adapter deliberately uses single-flight dispatch (one
pending client request) so shared node/edge/byte/deadline budgets cannot depend
on response arrival order. Unknown, duplicate, or already-completed ids count
as invalid output. Notifications and server requests share separate count
limits.

The server-request policy is fixed:

- `workspace/configuration`: return an array exactly as long and in the same
  order as `items`; each unavailable slot is `null` and each available slot is
  the hardened configuration;
- `window/workDoneProgress/create`: acknowledge without granting new behavior;
- `client/registerCapability`: accept the entire request only when every
  registration is a bounded non-execution registration; otherwise return one
  error for the whole request and do not adopt any registration;
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
  -> Initialized
  -> CapabilityGate
  -> ReadinessGate
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
cleared environment, private `HOME`, temporary, target, and empty `PATH`
directories, and the existing cross-platform process-group abstraction.

The environment sets:

- fixed locale and `NO_COLOR`;
- `CARGO_NET_OFFLINE=true`;
- `RUSTUP_AUTO_INSTALL=0`;
- invalid loopback HTTP, HTTPS, and all-proxy endpoints;
- an empty `NO_PROXY`;
- `PATH` set only to the empty private runtime directory; Windows retains
  `SystemRoot` and `WINDIR` for process startup;
- no Cargo, rustc, or sysroot executable path;
- the bound scope and source for diagnostics.

This is recorded as best-effort offline. It is not described as an OS network
sandbox.

### Initialize And Capability Gate

Initialization uses only the snapshot `file:` URI, the canonical inline
`linkedProjects` model, and a fixed client capability set. Workspace discovery
is disabled. The relevant payload shape is exact and nested, not a map of
dotted setting names:

```json
{
  "capabilities": {
    "general": { "positionEncodings": ["utf-8", "utf-16"] },
    "textDocument": { "callHierarchy": { "dynamicRegistration": false } },
    "workspace": { "configuration": true },
    "experimental": { "serverStatusNotification": true }
  },
  "initializationOptions": {
    "linkedProjects": [{ "sysroot_src": null, "crates": [] }],
    "cargo": {
      "buildScripts": { "enable": false },
      "noDeps": true,
      "sysroot": null,
      "sysrootSrc": null,
      "target": "<bound-target-triple>"
    },
    "procMacro": { "enable": false },
    "checkOnSave": false
  }
}
```

The shown linked project is a shape placeholder for the request's complete
canonical `RustAnalyzerProjectModel`, not an empty production model. The
configuration digest covers the typed capability, hardening, server-request,
and readiness policy; the separate project-model digest covers the complete
inline `linkedProjects` object. Hardened rust-analyzer settings include:

- `cargo.buildScripts.enable = false`;
- `cargo.noDeps = true`;
- `procMacro.enable = false`;
- check-on-save disabled;
- no automatic workspace edits;
- no dependency fetching, Cargo/rustc invocation, sysroot discovery, or project
  preparation by this provider.

The configuration JSON, linked-project model digest, target triple, and profile
are canonicalized and bound by SHA256. The client always sends `initialized`
after a successful `initialize` response, even when the capability gate then
returns `unavailable`; no hierarchy requests are sent in that case. A returned
position encoding must be one of the two offered encodings. A server JSON-RPC
error during initialize is `failed`.

After a successful capability gate, the client waits for the pinned
rust-analyzer `experimental/serverStatus` notification. `quiescent = false`
continues waiting. `health = error` is unavailable, no notification before the
remaining global deadline is timeout, and malformed status is invalid output;
all three produce no facts. `health = warning` with `quiescent = true` permits
the query but records a stable limitation and makes the result partial. Only
`health = ok` with `quiescent = true` opens seed files without that limitation.

### Open, Prepare, And Traverse

The adapter reads each seed file from the snapshot within budget and sends one
bounded `textDocument/didOpen`. It sends
`textDocument/prepareCallHierarchy` at the seed position and retains only items
whose URI, range, selection range, kind, name, and query ownership validate.

Traversal is deterministic breadth-first search. It sorts prepared and returned
items by full-binding stable id, deduplicates nodes and edges, and queries each
item at most once per requested direction and depth. Version 1 sends requests
single-flight in stable frontier order. Cycles terminate through the visited
set. All one-hop and two-hop work shares the same request, message, node, edge,
source-byte, protocol-byte, report-byte, and deadline budgets.

Incoming call ranges are interpreted in the incoming caller item. Outgoing call
ranges are interpreted in the current caller item. Empty call lists are valid
completed responses. Null or empty prepare results are valid protocol responses
but make the affected seed unresolved and the report partial. Neither outcome
implies repository-wide completeness.

### Shutdown

On successful or unavailable capability completion, the client sends
`shutdown`, waits within the remaining deadline, sends `exit`, and reaps the
process. On timeout, crash, invalid framing, invalid JSON, output overflow,
snapshot mutation, or shutdown failure, it terminates and reaps the complete
process group.

The managed child has Drop-based termination as a final guard so an early Rust
error cannot leave a language server or descendant running.

## Status And Failure Semantics

The report uses a new `RepositoryContextProviderStatus` enum; the existing
`impact_context::contracts::ProviderStatus` is not reused:

| Status | Trigger | Facts retained | Completeness |
| --- | --- | --- | --- |
| `completed` | Every accepted seed and requested hop completed within budget | All fully validated facts | Query complete; index unknown |
| `partial` | A seed is unresolved/ambiguous, a valid URI/range is omitted, the bound model is degraded, or a finite budget stops later work | Fully validated facts committed before the stop | Query partial; index unknown |
| `unavailable` | Capability absent, linked project model unusable, readiness is explicitly unhealthy, or profile cannot establish the fixed no-toolchain configuration | None | Query unavailable; index unknown |
| `timeout` | Global deadline expires before graceful completion | None | Query unavailable; index unknown |
| `invalid-output` | Framing/JSON-RPC correlation, message structure, or invalid URI/range count exceeds the fixed threshold | None | Query unavailable; index unknown |
| `failed` | Server crash, cancellation, stdin failure, initialize/shutdown error, or required lifecycle transition failure | None | Query unavailable; index unknown |

The precedence for simultaneous terminal observations is binding error,
cancellation, invalid-output, timeout, failed, unavailable, partial, then
completed. A post-execution scope/snapshot/profile/model mismatch is a caller
error and rejects the entire report rather than returning a stale status.
Cancellation always kills/reaps, performs final verification, discards facts,
and returns `ProviderError::Cancelled`; it is not a seventh report status.
Only facts already fully normalized and committed before a `partial` transition
are retained. No in-flight response, timeout, invalid output, crash, or failed
binding fact is ever retained.

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
check-on-save plus offline Cargo settings are mandatory. Every
`rust-analyzer.toml` is rejected before spawn because workspace configuration
could override those client settings. A profile that cannot meet the settings
is rejected.

The first implementation cycle does not claim to prevent every possible direct
network system call by a compromised authorized rust-analyzer binary. Binary
pinning, process isolation, offline configuration, proxy denial, an empty child
`PATH`, and process-tree termination are the enforced cross-platform controls.
A future stronger sandbox may add OS network denial without changing the
provider contract.

## Testing Strategy

### Contract And Snapshot Tests

- reject unknown fields and unsupported schema versions;
- validate the profile and normalized linked-project model schemas, canonical
  digests, target/toolchain policy, and absolute external executable binding;
- reject scope, candidate, source, snapshot, project-model, binary, and config
  binding mismatches;
- reject writable, changed, oversized, mode-only, empty-directory, or VCS-bearing
  snapshots;
- reject a root or nested repository-controlled `rust-analyzer.toml` before
  spawn;
- preserve safe relative symlinks and reject escaping or looping symlinks;
- reject invalid seeds, unsupported seed kinds, missing query points, and
  ambiguous prepare ownership;
- verify deterministic report ordering and ids.

### URI And Range Tests

- accept valid Unix and Windows file URIs inside the snapshot;
- reject absolute escapes, percent-encoded escapes, authority misuse, query and
  fragment data, non-file schemes, snapshot-root URIs, directories, missing
  files, and stale symlinks;
- report non-UTF-8 paths without lossy conversion;
- convert ASCII, multi-byte UTF-8, and surrogate-pair UTF-16 positions;
- normalize an overlong LSP character only with an explicit limitation;
- cover LF, CRLF, bare CR, empty lines, EOF with and without a final terminator,
  and end-exclusive line transitions;
- reject mid-code-point, mid-surrogate, reversed, and out-of-file ranges;
- require full/selection range containment and a seed query byte inside the
  selection range.

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
- exact nested initialization payload, unoffered position encodings, and
  initialize, readiness, prepare, incoming, outgoing, shutdown, and exit
  ordering;
- quiescent ok/warning/error, missing status, and early-query prevention;
- null or empty prepare results;
- one-hop and two-hop incoming and outgoing traversal;
- cycles, self-calls, fan-out, duplicate items, one-edge-per-call-range, and
  duplicate call ranges;
- every request, message, byte, node, edge, source, report, and deadline budget;
- server crash before and after initialization;
- total timeout and shutdown timeout;
- snapshot, profile, or binary mutation before and after execution;
- child and descendant process termination on every early return;
- deterministic output from single-flight frontier order.

### Later Real Rust-Analyzer Integration Tests

Delivery 5's opt-in, pinned rust-analyzer fixture verifies:

- capability negotiation and position encoding;
- a known direct function call in a local Rust project;
- incoming and outgoing call-site ranges;
- build-script and procedural-macro marker files are never created;
- check-on-save is never invoked;
- missing dependencies yield explicit `partial` or `unavailable` status;
- offline execution does not fetch dependencies;
- stale and external URIs are rejected by the adapter.

The fake server remains the required deterministic Delivery 1-3 CI gate. These
real-server tests are not claimed by the first implementation cycle.

## Delivery Sequence

### Delivery 1: Contract And Snapshot Boundary

- add request/report contracts and JSON schemas;
- add profile, project-model, request/report schemas and exact authorization;
- harden `CandidateSnapshot` verification and add `BoundCandidateSnapshot`;
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
- add the pinned rust-analyzer quiescent readiness gate;
- add prepare, incoming, and outgoing request models;
- add linked-project initialization and deterministic single-flight one-hop and
  two-hop traversal;
- normalize symbols, semantic edges, limitations, and metrics;
- verify all bindings after shutdown.

The first implementation cycle ends after Delivery 3 is locally verified.

### Delivery 4: Explicit User Surface

- add a standalone provider CLI or an equivalently isolated explicit command;
- add profile registry/distribution, normalized project-model construction,
  and report rendering;
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
  project-model, profile, binary, target, and configuration bindings;
- URI and range mapping never emits an unvalidated or lossy repository path;
- the fake server proves bounded initialize, capability, prepare, incoming,
  outgoing, shutdown, and exit behavior;
- one-hop and two-hop single-flight traversal is deterministic and respects
  every budget;
- timeout, crash, malformed output, stale URI, and snapshot mutation have
  explicit tested outcomes;
- the typed linked-project configuration serializes build scripts, proc macros,
  check-on-save, Cargo/rustc/sysroot discovery, and dependency fetching as
  disabled;
- repository-controlled `rust-analyzer.toml` files are rejected and no
  hierarchy query is sent before the bounded quiescent readiness gate;
- no provider code is reachable from ordinary review or Fast Mode;
- no semantic result is persisted or used to overwrite heuristic evidence;
- all affected Rust 1.95 `--locked` formatting/Clippy/unit/integration,
  fuzz-smoke, schema, and fake-process platform tests pass.

Full Phase 2 release readiness additionally requires Delivery 4 and Delivery 5,
including a pinned real rust-analyzer trust chain and four-platform evidence.

## Rejected Alternatives

### Static-Analysis Shim

Rejected. A separate shim would reduce Rust protocol code but add another
trusted executable and split snapshot, URI, opaque-data, and process-lifecycle
validation across two trust boundaries.

### General Async LSP Client Stack

Rejected for the first implementation cycle. A generic async JSON-RPC client and runtime
would enlarge the dependency and concurrency surface beyond the small set of
requests required by this adapter.

### Direct ImpactContext Integration

Rejected. Existing contracts treat related targets as changed symbols and
currently reject semantic providers. Changing those invariants before the
provider contract is proven would couple Phase 2 to the default review path.

### Persistent Semantic Graph

Rejected for the first implementation cycle. LSP does not define cross-session stable ids,
and opaque provider data is session-local. Persistence requires a separate
identity and invalidation design after the adapter is validated.

## Documentation And Compatibility

The implementation plan must preserve Rust 1.95 support, four-platform process
behavior, ASCII protocol framing, deterministic JSON ordering, and the existing
Subproject B public contracts. New dependencies must be checked with
`cargo +1.95.0 ... --locked`, included in the release SBOM component assertion,
and have their license notices closed when introduced. Documentation must
describe the provider as opt-in and best-effort offline, and must not call LSP
results a complete runtime call graph.
