# Persistent Symbol Index Design

## Status

Proposed for full-design approval on 2026-07-27.

The storage-engine decision in this document was approved on 2026-07-27:

- content-addressed immutable FileFacts;
- immutable SQLite Repository Graph generations;
- no RocksDB dependency in the first delivery.

Implementation has not started. This document refines Subproject B from
[Repository Impact Context Design](2026-07-26-repository-impact-context-design.md).
The supporting engine research is in
[Persistent Symbol Index Storage Engine Research](../../persistent-symbol-index-storage-engine-research.md).

## Decision Summary

Subproject B adds a local, persistent, whole-repository symbol index for exact
Git candidates. It provides bounded heuristic impact relationships without
claiming compiler-complete name or call resolution.

The design uses two deep Modules:

1. a content-addressed FileFacts Store containing path-independent full-file
   syntax facts;
2. an immutable SQLite Repository Graph Store containing path-dependent module,
   symbol, import, reference, and call relationships for one exact candidate.

Each graph generation is built in a private staging file, validated, closed,
synced, and atomically published to a digest-addressed final path. Published
generations are never migrated or modified in place.

Fast Mode may read an already compatible generation but performs no persistent
writes and never waits for a writer. Deep and explicit index operations may
create FileFacts and graph generations within declared budgets.

The graph remains an implementation detail. Only bounded changed-symbol and
impact slices enter `impact_context/v1`.

## Context

Subproject A parses complete changed files and emits accurate structural facts
for the changed ranges. It deliberately does not parse unchanged cache misses or
maintain whole-repository relationships.

That boundary leaves several review questions unanswered:

- which unchanged modules import a changed module;
- which unchanged call sites may reference a changed symbol;
- which tests or entry points are connected within one or two graph hops;
- whether a rename or deletion invalidates known reverse relationships;
- whether a staged candidate differs from the indexed base in a way that makes
  repository impact incomplete.

These questions require persistent full-file facts, path-aware resolution,
reverse relationships, and bounded traversal. They do not require pretending
that Tree-sitter provides compiler semantics.

## Decision Drivers

- Bind every accepted relationship to exact candidate bytes and project-model
  bytes.
- Keep Fast Mode read-only, non-blocking, offline, and bounded.
- Reuse unchanged syntax work across branches and staged candidates.
- Publish graph state atomically so readers never observe a draft.
- Detect incompatible or corrupt records and treat them as cache misses.
- Support efficient incoming and outgoing edge lookup.
- Preserve deterministic ids, ordering, coverage, and limitations.
- Keep the four-platform static release matrix supportable.
- Leave a clean Seam for later semantic providers without making them required.

## Goals

- Index all eligible tracked Rust source files for a selected candidate.
- Persist path-independent definitions, scopes, imports, references, and call
  sites by content identity.
- Resolve Rust module and lexical relationships heuristically across files.
- Persist forward and reverse graph relationships for an exact candidate.
- Reuse a compatible base generation with an exact staged or working-tree
  overlay.
- Traverse incoming and outgoing impact within hop, row, node, edge, byte, and
  time budgets.
- Report graph-index completeness separately from graph-query completeness and
  output truncation.
- Provide bounded build, doctor, inspect, and cleanup commands.
- Remain useful when the index is partial, stale, missing, or corrupt.

## Non-Goals

- Compiler-complete Rust name resolution.
- Macro expansion, procedural macro execution, build scripts, or Cargo builds.
- Type inference sufficient to resolve arbitrary method dispatch.
- Runtime call-graph completeness for reflection, function values, dynamic
  dispatch, or generated code.
- Cross-language calls in the first delivery.
- Persistent raw source files or unrestricted source snippets in the cache.
- A mutable graph daemon or IDE server.
- RocksDB, SCIP, rust-analyzer, or Joern integration in Subproject B.
- Migrating old cache generations after a schema or engine change.

## Considered Storage Options

### One mutable SQLite WAL database

Rejected for the first delivery. It provides transactions and concurrent reads,
but Fast Mode would be coupled to a mutable file, WAL/SHM sidecars, checkpoint
behavior, and possible `SQLITE_BUSY` results.

### RocksDB

Rejected for the first delivery. Its checksums, Column Families, snapshots, and
atomic WriteBatch are capable, but the current workload does not justify the
C++20, bindgen, compression, compaction, multi-file publication, and release
matrix cost.

### Fully custom immutable graph shards

Rejected as the default. It preserves a small dependency closure but would make
this project own adjacency indexes, transaction publication, inspection,
integrity checking, schema evolution, and crash recovery.

### Content-addressed facts plus immutable SQLite generations

Accepted. It preserves immutable publication and reader/writer separation while
using SQLite for graph indexing, constraints, inspection, and bounded lookup.

## Architecture

```text
Authoritative Review Scope
            |
            v
Repository Manifest Source
            |
       +----+------------------+
       |                       |
       v                       v
FileFacts Builder       Project Model Reader
       |                       |
       v                       v
Content-addressed        Rust Module Resolver
FileFacts Store                 |
       |                        v
       +--------------> Repository Graph Builder
                                |
                                v
                     Immutable SQLite Generation
                                |
                 +--------------+--------------+
                 |                             |
                 v                             v
          Staged Overlay              Bounded Graph Traversal
                 |                             |
                 +--------------+--------------+
                                v
                    Repository Index Adapter
                                |
                                v
                       impact_context/v1
```

The design introduces deep Modules with narrow Interfaces:

- Repository Manifest Source owns exact whole-candidate enumeration.
- FileFacts Builder owns language syntax extraction.
- FileFacts Store owns immutable content-addressed persistence.
- Project Model Reader owns passive parsing of tracked metadata.
- Rust Module Resolver owns heuristic path and name relationships.
- Repository Graph Store owns immutable generation persistence and lookup.
- Overlay owns candidate deltas without persistent Fast Mode writes.
- Traversal owns graph budgets and deterministic selection.
- Repository Index Adapter maps internal results into the existing contract.

No consumer outside `impact_context` reads SQLite directly.

## Source Layout

```text
collect-diff-context-cli/src/impact_context/
|-- cache/
|   |-- mod.rs
|   |-- file_facts.rs
|   |-- sqlite_generation.rs
|   |-- locking.rs
|   |-- integrity.rs
|   `-- cleanup.rs
|-- index/
|   |-- mod.rs
|   |-- manifest.rs
|   |-- model.rs
|   |-- overlay.rs
|   |-- resolver.rs
|   `-- traversal.rs
`-- adapters/
    `-- repository_index.rs
```

The existing Tree-sitter Rust Adapter remains the parser owner. It gains a
full-file indexing Interface rather than moving parsing into the cache Module.

## Repository Manifest Source

The existing `CandidateContent` Interface remains optimized for changed units.
Subproject B adds an internal whole-candidate Interface:

```rust
pub trait RepositoryManifestSource {
    fn scope_fingerprint(&self) -> &str;
    fn source(&self) -> ReviewSource;
    fn repository_locator(&self) -> &RepositoryLocator;
    fn manifest_bounded(
        &self,
        budget: &mut IndexBudgetTracker,
    ) -> Result<RepositoryManifest, RepositoryManifestError>;
    fn read_bounded(
        &self,
        path: &RepoPath,
        maximum_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError>;
}
```

Candidate semantics remain exact:

- staged uses stage-zero index entries and blobs;
- unstaged uses tracked working-tree bytes and excludes untracked files;
- branch uses the selected committed tree;
- deletions, gitlinks, modes, renames, and unavailable content remain explicit.

The manifest is path sorted and contains, for every tracked unit:

- normalized repository-relative path;
- Git mode and presence;
- language eligibility;
- candidate content SHA256 when content is available;
- bounded unavailable or resource-limited state.

`candidate_manifest_digest` is computed from canonical versioned records. It
does not contain timestamps, inode numbers, filesystem order, or display-only
path quoting.

Fast lookup may use a cheaper Git-derived `candidate_locator_digest` to locate a
published generation or base generation. The locator is never sufficient proof
by itself: accepted generations must still bind the exact manifest and scope
identities recorded at build time. Locator ambiguity or deadline exhaustion is
a cache miss.

### Candidate Locator and Base Selection

Fast Mode may establish exactness compositionally without hashing every
unchanged file again:

- branch may use a generation bound to the selected committed tree and Git
  object format;
- staged may use a generation bound to the opening HEAD tree plus the complete
  stage-zero changed-path set and exact staged overlay bytes;
- unstaged may use a generation bound to the exact stage-zero index manifest
  plus the complete tracked working-tree changed-path set and exact overlay
  bytes.

The index manifest locator is a digest of path-sorted stage-zero path, mode, and
Git object identity records. Git object identities are lookup coordinates, not
substitutes for the SHA256 identities stored in FileFacts and graph metadata.

A base-plus-overlay result is accepted only when the authoritative scope proves
that the overlay path set is complete relative to that base and every overlay
content identity matches the candidate bytes. If the index manifest, comparison
base, changed-path closure, or remaining deadline cannot be established, the
graph is unavailable or partial. The reader never assumes that a locator match
alone implies a candidate-manifest match.

## FileFacts Model

FileFacts contain only path-independent syntax information and source ranges.
They must be reusable when identical content appears at another path or in
another candidate.

The FileFacts key is:

```text
file-facts/v1
+ language
+ candidate_blob_sha256
+ grammar_version
+ query_digest
+ adapter_version
+ normalization_rules_digest
+ file_facts_schema_version
```

The value includes bounded, deterministically ordered records:

- local definitions and declarations;
- local scopes and ownership;
- symbol kind, name, signature, and visibility;
- imports, aliases, groups, and glob markers;
- exports and re-exports;
- identifier and qualified-name references;
- syntactic call sites and their enclosing local symbol;
- attributes and module declarations needed by the resolver;
- parse quality, recovery ranges, and extraction limitations;
- fact, node, nesting, and byte counts.

FileFacts do not contain repository paths, resolved module ids, repository
symbol ids, or claims of semantic call resolution.

The current Rust Adapter extracts only changed symbols for Fast Mode. Its new
indexing operation extracts all eligible facts from a complete file while
sharing grammar, query, range, budget, and recovery behavior with Fast Mode.

## FileFacts Store

Default layout:

```text
<cache-root>/v2/repos/<repository-id>/facts/sha256/ab/<fact-key>.facts
```

Every object has a bounded envelope containing:

- format magic and schema version;
- complete FileFacts key inputs;
- payload length;
- payload SHA256;
- deterministic payload bytes.

Objects are created in the destination directory, synced, and published without
overwriting an existing final object. Published objects are never modified.

On read:

- the path is derived only from a validated lowercase digest;
- envelope and payload lengths are bounded before allocation;
- the requested key must exactly match the envelope;
- the payload digest is recomputed;
- decode, schema, range, or digest failure becomes a corrupt cache miss.

FileFacts may be committed even if the total graph build later exhausts its
budget. A later build can reuse those valid objects.

## Project Model

The Rust project model is derived only from tracked candidate bytes that the
resolver is authorized to read. Initial inputs are:

- workspace and package `Cargo.toml` files;
- Rust source paths and modes;
- `mod` declarations and supported `#[path]` attributes;
- conventional Cargo target roots.

Manifests are parsed as data. The product does not execute Cargo, metadata
commands, build scripts, package managers, compiler probes, or repository-owned
configuration.

`project_model_digest` binds path-sorted exact bytes of every consumed metadata
file plus the resolver policy and parser versions. Unsupported manifest syntax,
workspace inheritance, generated targets, or ambiguous roots reduce
completeness rather than authorizing discovery commands.

## Rust Resolver Scope

The first resolver handles bounded heuristic relationships for:

- `src/lib.rs`, `src/main.rs`, and conventional `src/bin` roots;
- inline and file-backed modules;
- `crate`, `self`, and `super` prefixes;
- simple, nested, grouped, and aliased `use` declarations;
- explicit re-exports;
- unique lexical and module-qualified definitions;
- direct free-function and associated-function call candidates;
- reverse module imports and resolved-reference candidates.

The resolver records partial or unresolved relationships for:

- glob imports whose candidate set cannot be proven complete;
- method calls without sufficient receiver type information;
- trait dispatch and generic bounds;
- macro-generated modules, imports, definitions, or calls;
- conditional compilation whose active configuration is unknown;
- external dependencies not represented in the candidate;
- non-conventional generated targets or unsupported Cargo metadata.

Tree-sitter-derived resolution is never `semantic`. Successful unique
cross-file binding uses `resolved-reference` with at most medium confidence.
Ambiguous method or trait targets use `polymorphic-candidate` or remain
`unresolved`.

## Repository Graph Identity

A graph generation key is the SHA256 of a canonical tuple containing:

```text
repository-graph/v1
+ repository_graph_schema_version
+ candidate_manifest_digest
+ project_model_digest
+ resolver_digest
+ language_adapter_and_query_digests
+ file_facts_manifest_digest
+ normalization_rules_digest
```

`file_facts_manifest_digest` binds the path-sorted mapping from candidate paths
to exact FileFacts keys and presence states.

Changing file content, paths, modes, parser queries, project metadata, resolver
policy, normalization, or schema produces a different generation. Cache data is
derived and disposable, so incompatible generations are not migrated.

## SQLite Generation

Default layout:

```text
<cache-root>/v2/repos/<repository-id>/graphs/<generation-key>.sqlite
```

The first schema contains these logical tables:

- `generation_meta`: exact identities, versions, counts, completeness, and
  application root digest;
- `files`: path, mode, presence, content digest, FileFacts key, language, and
  module identity;
- `modules`: module id, parent, root, path, and resolver status;
- `symbols`: repository symbol id, local fact id, module, path, kind, name,
  owner, visibility, signature, range, and confidence;
- `edges`: defines, imports, exports, references, calls, implements, and
  unresolved candidate edges;
- `limitations`: stable graph-build limitation codes and affected identities.

The edge table has independent indexes for:

- generation-local outgoing lookup by source symbol and kind;
- incoming lookup by target symbol and kind;
- path invalidation and overlay suppression;
- unresolved target and module lookup where bounded queries require them.

SQLite foreign keys and fixed application checks protect internal shape. The
application still validates every decoded enum, digest, path, range, count, and
row bound; a successful SQL query alone is not proof of a valid graph record.

The implementation uses an exact approved `rusqlite` version with
`default-features = false` and only `bundled` plus demonstrated required
features. Extension loading, SQLCipher, session, backup, and runtime SQL from
the repository are forbidden.

## Build and Publication Protocol

Only explicit Deep or index operations may publish cache records.

```text
compute generation key
        |
        v
acquire bounded key-specific writer lock
        |
        v
create same-filesystem staging SQLite file
        |
        v
build fixed schema and rows in transactions
        |
        v
validate foreign keys, counts, root digest, integrity_check
        |
        v
close connection and sync file
        |
        v
publish without replacing an existing final generation
        |
        v
release lock
```

Build-time SQLite uses DELETE journal mode and a durability setting validated by
the storage spike. No WAL or SHM sidecars are part of a published generation.

Publication requirements:

- staging and final files are on the same filesystem;
- final paths are derived only from validated digests;
- an existing valid generation is reused and never overwritten;
- an existing invalid generation is quarantined by an explicit writer before a
  rebuild;
- interruption leaves either no final generation or a fully published one;
- temporary and journal files are never accepted by readers;
- Windows file-in-use cleanup failures are reported and retried later rather
  than forcing deletion.

### Partial Generation Publication

A partial generation may be published only when:

- the complete candidate manifest and generation identity are known;
- every eligible processed and omitted path has a deterministic recorded state;
- all stored rows, reverse indexes, counts, and root digests are internally
  consistent;
- the limitation set explains why index completeness is partial;
- a reader cannot interpret an omitted relationship as proof of absence.

An interruption before those conditions are met publishes no graph generation.
Valid FileFacts already published remain reusable. A later build may replace a
partial generation only by publishing a different immutable artifact whose key
also binds its FileFacts manifest and declared completeness inputs; it never
updates the old file in place.

## Fast Reader Protocol

Fast Mode:

1. computes a bounded exact candidate or base locator;
2. opens only an already published generation using read-only immutable SQLite
   flags;
3. validates schema and generation metadata;
4. creates an in-memory candidate overlay when required;
5. performs indexed, bounded graph lookups;
6. validates every consumed FileFacts object;
7. closes the generation without writes.

Fast Mode does not:

- acquire a writer lock;
- wait or retry on database lock results;
- create a database, journal, WAL, SHM, temporary file, or access-time record;
- migrate, repair, checkpoint, quarantine, or clean cache data;
- run a full-database integrity scan.

Missing files, lock or I/O results, incompatible metadata, corrupt rows, failed
FileFacts validation, or exceeded lookup budgets become explicit cache misses or
partial graph coverage. Ordinary diff review continues.

## Candidate Overlay

An overlay represents the exact difference between a compatible immutable base
generation and the selected candidate.

Overlay contents are bounded in-memory maps containing:

- changed or added FileFacts;
- deleted and replaced path tombstones;
- module additions, removals, and replacements;
- symbol additions, removals, and replacements;
- forward and reverse edge additions;
- base-edge suppression for changed owner paths;
- overlay limitations and completeness.

Lookup precedence is:

```text
overlay tombstone
    > overlay replacement or addition
    > compatible immutable base generation
```

When imports, exports, module declarations, or public definitions change, the
resolver refreshes known reverse import dependents. If glob imports, macros,
conditional compilation, ambiguous modules, or budget exhaustion prevent proof
of a complete affected closure, the overlay remains usable but
`graph_index_completeness` or `graph_query_completeness` becomes `partial`.

Fast Mode does not persist overlays. An explicit index operation may build and
publish a complete generation for the exact staged or working-tree candidate.

## Symbol and Edge Identity

FileFacts local ids are content-local and path independent. Repository graph ids
bind provider, module, path, kind, name, owner, and definition range.

Stable ids are deterministic within an exact generation. They are not promised
to survive a rename, move, signature edit, resolver change, or schema change.

Repository Index edges retain:

- provider id and version;
- edge kind;
- source and optional resolved target;
- unresolved target text when bounded and safe;
- repository path and range;
- resolution class;
- confidence;
- limitation linkage when resolution is incomplete.

Semantic providers in later subprojects may add higher-confidence edges but do
not delete or silently upgrade Repository Index edges.

## Bounded Traversal

Traversal is implemented in Rust, not as an unbounded recursive SQL query.

The algorithm is deterministic breadth-first traversal from changed symbols and
changed modules. Each hop:

- queries overlay relationships first;
- fetches indexed incoming and outgoing rows from SQLite;
- validates and deduplicates rows;
- applies kind and confidence policy;
- consumes deadline, database-row, node, edge, byte, and hop budgets;
- sorts the next frontier by stable identity.

Default product behavior targets one hop, with an explicitly bounded two-hop
Deep query. Higher depths require a later measured decision.

Cycles are detected by `(generation, direction, symbol_id, edge_kind)` visit
identity. Reaching a depth or resource limit returns valid partial results and a
stable limitation; it does not claim a complete impact set.

Presentation ranking prioritizes:

1. high-confidence semantic edges supplied by later providers;
2. unique resolved-reference callers and callees;
3. direct reverse imports and exported interface relationships;
4. test and entry-point relationships selected by the Domain Summarizer;
5. ambiguous syntactic candidates.

The persistent graph itself is never serialized wholesale.

## Completeness Semantics

`graph_index_completeness` describes stored and overlaid repository knowledge:

- `complete`: all eligible manifest files and required resolver relationships
  were processed within the declared language scope;
- `partial`: usable graph data exists but files, facts, project-model inputs, or
  resolver closures are incomplete;
- `unavailable`: no compatible trustworthy graph is usable.

`graph_query_completeness` describes one traversal:

- `complete`: the requested bounded traversal finished over the available graph;
- `partial`: hop, row, node, edge, byte, time, corruption, or overlay limits
  prevented completion;
- `unavailable`: traversal could not start from a trustworthy graph and changed
  symbol set.

Neither field claims compiler completeness. A heuristic graph can be completely
indexed and completely queried while still recording semantic limitations.

## Failure Semantics

Generation results fail closed for:

- scope or candidate mismatch;
- candidate manifest or project-model mismatch;
- FileFacts key, schema, length, or checksum mismatch;
- SQLite schema, metadata, root, row-shape, or integrity failure;
- path escape or unsafe cache path;
- resolver, adapter, query, or normalization identity drift;
- repository drift before authoritative output release.

The affected graph is ignored. It is never repaired by Fast Mode and never
partially trusted without an explicit completeness record.

Optional capability failures remain visible and fail open for ordinary review:

- cache miss;
- writer busy during explicit indexing;
- unsupported or malformed source;
- missing or unsupported project metadata;
- resource or deadline exhaustion;
- ambiguous module or symbol resolution;
- unavailable base generation;
- cleanup unable to remove an in-use generation.

## Locking and Concurrency

Locks serialize only writers targeting the same repository namespace or
generation operation. Readers never acquire writer locks.

Requirements:

- lock acquisition is bounded by the caller deadline;
- lock records contain no authority to trust a generation;
- process termination releases the operating-system lock;
- stale lock-file bytes are harmless without a held OS lock;
- cleanup uses a separate bounded writer operation;
- two builders of the same generation converge on one validated final file;
- builders of different generations may run concurrently only when memory,
  file-descriptor, and cache-root budgets permit it.

SQLite's internal locks protect only a staging database while it is being
built. Published immutable readers and staging writers never open the same file.

## Cache Location and Permissions

The cache-root policy from the parent design remains authoritative. The
repository namespace is derived from the canonical local Git common-directory
identity and a namespace schema version.

Additional requirements:

- `PRE_COMMIT_REVIEW_CACHE_DIR` must be absolute;
- the cache root may not resolve inside the reviewed worktree or Git common
  directory;
- created directories and files are current-user private;
- symlink or reparse-point traversal cannot escape the validated cache root;
- no cache path is derived from unvalidated repository text;
- cache data is repository-sensitive even though raw source is not stored.

## Security

- No network access, dependency download, build command, or repository code
  execution occurs.
- SQLite uses fixed application-owned schema and prepared statements.
- Extension loading and repository-provided SQL are forbidden.
- `trusted_schema` is disabled when supported by the approved SQLite version.
- Decode allocation, SQL row count, string length, blob length, range count, and
  recursion are bounded.
- Manifest and Cargo metadata are parsed as untrusted data.
- Paths are normalized repository-relative values and never interpolated into
  SQL.
- Inspection output is bounded and passes through the existing output sanitizer
  before Agent consumption.
- Cache failures never authorize a semantic claim.

## CLI

The CLI evolves to:

```text
repository-context-cli collect --mode <fast|deep> ...
repository-context-cli index build ...
repository-context-cli index doctor ...
repository-context-cli index inspect ...
repository-context-cli index clean ...
```

`index build`:

- requires source and expected scope;
- accepts explicit file, byte, time, node, fact, edge, and generation-size
  limits;
- emits a bounded machine-readable build report;
- may publish FileFacts before graph completion;
- publishes a graph generation only after all publication checks pass.

`index doctor` is read-only by default and reports:

- cache-root and repository namespace;
- supported schema and SQLite identities;
- FileFacts and generation counts and bytes;
- missing, incompatible, corrupt, and orphaned objects;
- generation metadata and integrity results;
- cleanup candidates without deleting them.

`index inspect` emits bounded metadata, files, symbols, or neighbor relationships
selected by exact digest, path, or symbol arguments. It never dumps the whole
graph by default.

`index clean` is an explicit cache mutation. It supports dry-run, exact
repository namespace, maximum-byte, invalid-object, and retained-generation
policies. It never follows paths outside the validated cache root and tolerates
in-use Windows files by reporting deferred cleanup.

The public Shell wrapper remains thin and does not reinterpret JSON.

## Storage Spike Gate

Before the SQLite dependency enters the product path, an isolated storage spike
must prove:

1. `rusqlite` with bundled SQLite builds on Linux musl, macOS arm64, macOS
   x86_64, and Windows MSVC release targets;
2. staging transaction, integrity check, close, sync, no-clobber publication,
   and immutable read-only open work on every target;
3. 10k, 100k, and 1M symbol/edge fixtures meet measured cold-open, one-hop,
   two-hop, and reverse-lookup targets;
4. twenty concurrent Fast readers do not wait for a writer building another
   generation and create no sidecar files;
5. process termination before and after transaction, sync, and publication
   never produces an accepted partial generation;
6. header damage, truncation, index-page damage, row-shape damage, and FileFacts
   payload damage become cache misses or doctor failures;
7. binary size, build time, open file count, RSS, and P50/P95/P99 are recorded;
8. license and SBOM closure contain only the approved SQLite dependency set.

Failure of this spike blocks the SQLite implementation plan. The fallback order
is:

1. immutable FileFacts plus custom adjacency shards;
2. a revised immutable SQLite layout;
3. RocksDB only when measured scale or access patterns specifically justify an
   LSM engine.

## Budgets and Performance Targets

Subproject B adds explicit Deep budgets for:

- total manifest files and bytes;
- project-model files and bytes;
- FileFacts cache reads, writes, and decoded bytes;
- parsed files, bytes, nodes, facts, and nesting depth;
- graph symbols, edges, unresolved candidates, and generation bytes;
- SQLite rows read per query;
- overlay paths, symbols, and edges;
- graph hops and frontier nodes;
- lock wait, total index time, and total query time.

Release targets:

- Fast cache miss returns without retry or persistent write;
- Fast compatible lookup remains inside the existing total 750ms hard deadline;
- warm Deep one-hop and two-hop query P95 is at or below two seconds;
- no repository-size-independent cold-index latency promise is made;
- cold indexing reports throughput and partial progress within hard budgets;
- repeated identical builds and queries produce deterministic accepted facts and
  output ordering.

## Testing Strategy

### Contract and Identity Tests

- deterministic manifest, FileFacts, project-model, resolver, and generation
  digests;
- path reuse with different content and identical content at different paths;
- grammar, query, resolver, project-model, normalization, and schema drift;
- exact staged, unstaged, and branch candidate binding;
- graph and query completeness arithmetic;
- deterministic symbol, edge, limitation, and traversal ordering.

### Rust Resolver Fixtures

- crate, self, super, aliases, grouped imports, and re-exports;
- inline, sibling, nested, and `#[path]` modules;
- free and associated functions;
- methods, traits, generic calls, and ambiguous candidates;
- glob imports and duplicate symbol names;
- workspace/package roots and conventional binaries;
- syntax recovery, macro-generated uncertainty, and cfg uncertainty;
- rename, deletion, module move, and public interface change;
- incoming and outgoing reverse relationships.

### Cache and Publication Tests

- FileFacts object interruption, truncation, digest mismatch, and incompatible
  schema;
- SQLite staging interruption at every publication phase;
- concurrent same-key and different-key writers;
- Fast readers during a build and publication;
- existing valid and invalid final generations;
- full and partial graph builds;
- Windows in-use generation cleanup;
- cache-root symlink, permission, and path-escape attacks.

### Overlay Tests

- staged additions, modifications, deletions, and renames;
- unstaged tracked-file overlays and staged baseline differences;
- import, export, module, and public-symbol invalidation;
- reverse dependent refresh;
- incomplete closure and budget exhaustion;
- base suppression and overlay precedence;
- exact candidate bytes when staged and working-tree content differ.

### Fuzzing

- FileFacts envelope and payload decoding;
- manifest and project-model normalization;
- SQLite row-to-domain mapping;
- symbol and module id normalization;
- graph edge normalization and deduplication;
- overlay merge and tombstone behavior;
- bounded traversal with cycles and adversarial fan-out.

### Performance and Resource Tests

- cold FileFacts creation and warm reuse;
- SQLite generation build, validation, sync, and open;
- one-hop and two-hop forward and reverse queries;
- overlay construction and merged traversal;
- small repository, medium repository, and large monorepo corpus;
- malformed, generated, minified, huge, and deeply nested files;
- file descriptor, generation byte, RSS, and binary-size gates.

## Delivery Sequence

### B0: SQLite Storage Spike

Prove the Storage Spike Gate without enabling the product path.

### B1: Whole-Candidate Manifest and Full-File Facts

Add bounded repository enumeration and the full-file Tree-sitter Rust indexing
Interface with golden and fuzz coverage.

### B2: Content-Addressed FileFacts Store

Add object identity, integrity, publication, read-only lookup, metrics, and
fault tests.

### B3: Rust Project Model and Resolver

Build path-aware modules, repository symbols, resolved-reference candidates,
reverse imports, and explicit unresolved relationships.

### B4: Immutable SQLite Graph Generations

Add schema, staging build, application root validation, no-clobber publication,
read-only immutable lookup, and corruption handling.

### B5: Overlay and Bounded Traversal

Add staged and working-tree overlays, invalidation closure, incoming/outgoing
queries, completeness, limitations, and `impact_context/v1` integration.

### B6: CLI, Doctor, Inspection, and Cleanup

Add bounded commands, wrapper integration, sanitizer handling, diagnostics, and
safe cache lifecycle operations.

### B7: Release Readiness

Run the full contract, integration, fuzz, fault, performance, license, SBOM, and
four-platform release gates before enabling index reads by default.

## Consequences

### Positive

- Whole-repository relationships become reusable without reparsing unchanged
  files on every review.
- SQLite removes substantial custom graph-index and inspection code.
- Immutable generations keep Fast readers isolated from writers and drafts.
- Exact identities make cache invalidation conservative and explainable.
- Later semantic providers can add evidence through an existing graph Seam.

### Negative

- Bundled SQLite increases binary size, compile time, SBOM, and native build
  surface.
- Full-candidate manifests and cold graph builds can be expensive on large
  repositories.
- Staged overlays require careful reverse-edge invalidation.
- Heuristic Rust resolution remains incomplete despite a complete stored graph.
- Immutable generations duplicate graph data until explicit cleanup.

### Risks and Mitigations

- SQLite release failure: block at B0 and use adjacency shards.
- Silent stale context: bind every generation and output to exact identities and
  revalidate scope before release.
- Large graph fan-out: enforce row, node, edge, hop, byte, and deadline budgets.
- Corrupt cache acceptance: validate keys, schemas, digests, rows, and published
  generation metadata; corruption becomes a miss.
- Cache growth: explicit size reporting and conservative cleanup with dry-run.
- Resolver overclaim: cap heuristic confidence and emit unresolved/partial
  states rather than semantic labels.

## Acceptance Criteria

Subproject B is complete when:

- the storage spike passes every four-platform gate;
- exact staged, unstaged, and branch manifests are deterministic and bounded;
- unchanged content reuses validated FileFacts;
- explicit index operations atomically publish immutable graph generations;
- Fast Mode can consume a compatible generation with zero persistent writes and
  no writer wait;
- staged overlays use exact candidate bytes and correctly suppress replaced base
  relationships;
- Rust module, import, definition, reference, and syntactic-call fixtures produce
  deterministic heuristic relationships with honest limitations;
- incoming and outgoing one-hop and two-hop traversal obey all budgets;
- corrupt, stale, incomplete, or incompatible cache data cannot become accepted
  graph evidence;
- doctor, inspect, and clean operations are bounded and path safe;
- `impact_context/v1` reports cache metrics, graph completeness, query
  completeness, and limitations without exposing the full graph;
- warm Deep traversal meets the P95 target on the documented corpus;
- all tests, fuzz targets, Clippy, formatting, schema validation, licenses, SBOM,
  and release binaries pass;
- no RocksDB, semantic provider, or later-language work is required for the
  Subproject B release.
