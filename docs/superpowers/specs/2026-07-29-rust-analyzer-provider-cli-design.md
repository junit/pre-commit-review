# Rust-Analyzer Provider Explicit CLI Design

## Status

Approved for implementation planning on 2026-07-29. This document defines
Phase 2 Delivery 4 for the existing library-only rust-analyzer repository
context provider. Delivery 5 remains responsible for real rust-analyzer
artifacts, sustained fuzzing, and release trust-chain evidence.

## Decision Summary

Add an explicit, standalone repository-context-provider-cli binary and a
matching shell wrapper. The command is opt-in and constructs the provider
inputs around the existing CandidateSnapshot and provider library. It never
becomes reachable from ordinary review, Fast Mode, repository indexing,
SQLite persistence, or static-analysis orchestration.

The CLI has two commands:

- model builds a normalized, digest-bound RustAnalyzerProjectModel from
  the authoritative candidate snapshot and passive Cargo metadata.
- run loads an explicitly authorized registry entry, model, seed request,
  and limits, then runs the existing bounded provider and renders its report.

The registry is an authorization document, not a downloader or package
manager. It records absolute profile/executable paths, exact SHA256 values,
provider identity, target, and configuration identity. Delivery 4 does not
bundle, download, or install a real rust-analyzer executable.

## Goals

- Provide a usable explicit command without widening the provider's library
  boundary.
- Build project models from exact candidate bytes without invoking Cargo,
  rustc, Git hooks, build scripts, proc macros, dependency preparation, or
  network access.
- Bind the model, profile, executable, configuration, registry authorization,
  scope, and candidate snapshot to every invocation. The existing report
  retains the provider, profile, executable, configuration, model, and
  candidate identities; the registry file digest is an authorization input
  and is not added as a new report field.
- Make registry selection explicit and fail closed on path, digest, target, or
  profile mismatch.
- Keep CLI output bounded, schema-valid, deterministic, and free of local
  snapshot roots, raw stderr, opaque LSP data, and untrusted tool text.
- Package the new command, schemas, wrapper, and CI gates without adding a
  real rust-analyzer artifact.

## Non-Goals

- Automatic provider discovery or profile selection.
- Downloading, updating, extracting, or installing rust-analyzer.
- A built-in platform artifact registry or release binary for rust-analyzer.
- A long-lived daemon, parallel sessions, semantic persistence, or a complete
  whole-repository call graph.
- Integration with the default review, Fast Mode, repository index, SQLite,
  or static-analysis orchestration paths.
- Changing the existing RepositoryContextProviderRequest or report
  contracts to expose CLI-only paths.

## Explicit Command Surface

The binary name is repository-context-provider-cli. Its help text and
argument parser are stable and reject unknown flags, duplicate flags, relative
paths, missing values, and values outside the contract maxima.

    repository-context-provider-cli model
      --source <staged|unstaged|branch>
      --expect-scope <64-lowercase-hex>
      [--max-model-files <positive integer>]
      [--max-model-bytes <positive integer>]

    repository-context-provider-cli run
      --source <staged|unstaged|branch>
      --expect-scope <64-lowercase-hex>
      --registry </absolute/registry.json>
      --expect-registry-sha256 <64-lowercase-hex>
      --provider-id <stable id>
      --model </absolute/project-model.json>
      --expect-model-sha256 <64-lowercase-hex>
      --request </absolute/provider-run-request.json>

All paths are required to be absolute. The command reads JSON inputs once,
checks their exact bytes and canonical paths, and writes one bounded JSON
document to stdout. It writes only a stable bounded error code and detail to
stderr. It never accepts a repository-relative profile, executable, model,
registry, or request path.

model opens and verifies the authoritative scope before materializing a
read-only candidate snapshot. It emits the existing
repository-context-project-model contract, including sorted limitations.
run repeats the scope and snapshot checks, derives the candidate and provider
bindings from the registry/model/request, and calls
run_repository_context_provider.

The CLI must not accept an arbitrary snapshot directory. The only snapshot is
the one materialized from the explicit source and expected-scope pair.

## Run Request Contract

Add a strict JSON Schema and Rust type named
repository_context_provider_run_request/v1. It contains only caller-owned
query inputs:

    {
      "schema_version": 1,
      "kind": "repository_context_provider_run_request",
      "seeds": [],
      "directions": ["incoming", "outgoing"],
      "limits": {}
    }

seeds uses the existing bounded SeedSymbol shape. The request requires
sorted, unique seed ids and non-empty directions. limits uses the existing
ProviderLimits shape and may only lower the authorized profile maxima. The
CLI, not the input file, supplies candidate root, snapshot digest, model
digest, provider paths, profile digest, executable digest, target, and
configuration digest.

The CLI constructs the in-memory RepositoryContextProviderRequest only after
all bindings have been validated. It verifies both the exact model file SHA256
and the model's canonical digest before using the model digest in the
candidate binding. It never trusts a caller-provided candidate or provider
binding.

## Registry Contract

Add a strict Draft 2020-12 schema and Rust type named
repository_context_provider_registry/v1:

    {
      "schema_version": 1,
      "kind": "repository_context_provider_registry",
      "entries": [
        {
          "provider_id": "rust-analyzer-local",
          "provider_kind": "rust-analyzer",
          "provider_version": "pinned-version",
          "target_triple": "aarch64-apple-darwin",
          "profile_path": "/absolute/profiles/rust-analyzer.json",
          "profile_sha256": "<64-lowercase-hex>",
          "executable_path": "/absolute/bin/rust-analyzer",
          "executable_sha256": "<64-lowercase-hex>",
          "configuration_sha256": "<64-lowercase-hex>",
          "toolchain_mode": "none"
        }
      ]
    }

Every object uses additionalProperties: false. Entries have unique
provider_id values, absolute paths, lower-case SHA256 values, and the fixed
toolchain_mode: none. The registry has a bounded number of entries and
bounded text fields. An entry's profile is loaded and validated as the
existing AuthorizedProviderProfile; its executable and configuration
digests must match both the profile and the request.

provider-id selects exactly one entry from the explicitly supplied registry.
There is no default registry path, ambient PATH lookup, latest version
selection, URL, download command, or platform fallback. The registry's
SHA256 is checked against expect-registry-sha256 before any profile or
executable is opened.

## Passive Project-Model Construction

Create a focused provider model-builder module that adapts the existing
passive Rust Cargo project-model parser to the provider's linked-project
contract. The builder reads only files present in the materialized snapshot:
Cargo manifests, declared target roots, and bounded project metadata. It
converts package/target results into sorted provider crates, root modules,
editions, cfg values, environment values, and limitations.

The builder must:

- use the candidate snapshot as its only byte source;
- reject or report manifests outside the snapshot;
- preserve a deterministic limitation when workspace inheritance, globs,
  build scripts, proc macros, or unsupported target fields are encountered;
- never run Cargo, rustc, rustup, a package manager, Git, a build script, or
  a repository-owned executable;
- account every consumed file and byte under explicit model limits;
- compute the provider model digest from canonical model bytes and policy;
- verify the resulting model against the exact snapshot before returning.

The existing persistent repository-index model remains an implementation
precedent only. The new builder does not write FileFacts, SQLite generations,
or repository index artifacts.

## Run Data Flow

    authoritative source + expected scope
                  |
                  v
    read-only CandidateSnapshot
                  |
                  +--> passive model builder --> digest-bound linked project model
                  |
    explicit registry + expected registry digest
    explicit model + expected model digest
    explicit run request
                  |
                  v
    registry/profile/executable/model/request validation
                  |
                  v
    existing bounded provider library
                  |
                  v
    validated RepositoryContextProviderReport

The CLI performs the same preflight and postflight checks as the library.
Every failure before report publication releases no report. A provider report
with unavailable, timeout, invalid-output, or failed status is still a valid
bounded report when the library can construct one; authorization, scope,
snapshot, registry, profile, model, or executable drift is a CLI failure with
no authoritative report.

## Exit And Output Semantics

Exit code 0 means a schema-valid report was rendered, including a report whose
provider status is partial or unavailable. Exit code 2 means argument, schema,
authorization, scope, or binding validation failed. Exit code 3 means the
provider session returned a cancellation or unrecoverable preflight failure
before a report could be safely rendered. No exit path prints raw child
stderr, raw JSON-RPC frames, local snapshot roots, or opaque LSP data.

The rendered report is the existing repository-context-provider-report
contract. Report paths are snapshot relative and all ids remain report-local.
The CLI does not add a second finding/evidence contract and does not mark
review units as reviewed.

## Packaging And Workflow Gates

Add:

- collect-diff-context-cli/src/bin/repository_context_provider.rs;
- the run-request and registry schemas;
- a public scripts/run_repository_context_provider.sh wrapper;
- a resolver helper that accepts only an explicit absolute CLI override, a
  local release build, or the packaged provider CLI binary;
- installer and release payload entries for the provider CLI, wrapper, and
  schemas;
- help, parser, model-builder, registry, fake-server, and report tests;
- CI schema validation, Rust 1.95 format/test/Clippy, shell smoke, and
  no-default-pipeline reachability gates.

The wrapper resolves the CLI binary but never resolves rust-analyzer. The
provider registry remains a user/CI supplied input. Delivery 5 may later add
platform-specific artifact manifests and trust-chain checks without changing
the CLI contract.

## Security And Trust Boundary

This feature is for local developer tooling and code-review infrastructure,
not a network-security product. Its controls are authorization and
reproducibility controls:

- registry, profile, model, executable, and request bytes are explicitly
  supplied and digest checked;
- the snapshot is materialized from authoritative Git state and revalidated;
- the provider runs with the existing private runtime and best-effort offline
  environment controls;
- no network download or dependency preparation occurs;
- report output is normalized and bounded before publication.

The registry and entrypoint digests authorize the declared execution inputs;
they do not claim a complete native dependency closure or an operating-system
network sandbox.

## Testing Strategy

Unit and contract tests cover:

- strict registry and run-request schemas;
- duplicate provider ids, relative paths, digest mismatches, unknown fields,
  wrong target, wrong profile, and configuration drift;
- model construction from single-package and workspace fixtures;
- model limits, unsupported workspace fields, invalid manifests, and
  deterministic digest/limitation ordering;
- no Cargo/Git/build-script/process invocation from the model builder.

Integration tests use the existing fake provider server and cover:

- model and run help and argument rejection;
- exact source/scope/model/registry binding;
- completed, partial, unavailable, timeout, invalid-output, and failed
  reports;
- process cleanup, output bounds, and no-default-pipeline reachability;
- wrapper binary resolution, installer payloads, schema validation, and
  release smoke checks.

All tests remain independent of an installed real rust-analyzer. Real-server,
artifact, sustained-fuzz, and four-platform trust-chain tests remain Delivery
5.

## Completion Criteria

Delivery 4 is complete when:

1. The standalone CLI exposes only the explicit model and run commands.
2. Registry and run-request schemas are strict, bounded, and digest-bound.
3. Model construction uses only the candidate snapshot and passive metadata.
4. The CLI constructs provider requests instead of trusting caller bindings.
5. Existing provider reports pass unchanged through the CLI with no local
   roots, raw stderr, or opaque LSP data.
6. All authorization, scope, model, registry, profile, executable, and
   snapshot drift cases fail closed.
7. Installer, release, schema, Rust, shell, fake-server, and reachability
   gates pass.
8. No default review/index/static-analysis path invokes the provider.
9. No real rust-analyzer artifact or release claim is introduced.
