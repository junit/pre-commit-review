# Third-Party Artifact and Provider Distribution Design

## Status

Approved in design discussion on 2026-07-29. This document defines Phase 2
Delivery 5 as two separately planned and delivered changes:

- Delivery 5A establishes generic third-party artifact distribution and
  migrates Gitleaks without changing its user-facing behavior.
- Delivery 5B publishes and provisions a real, pinned rust-analyzer pack and
  adds the quality evidence required to use it with the Delivery 4 provider.

Implementation planning remains gated on review of this written specification.
Delivery 4 remains the authoritative provider execution contract.
The supporting distribution and trust analysis is recorded in
[`docs/gitleaks-distribution-strategy-research.md`](../../gitleaks-distribution-strategy-research.md).

## Product Boundary

pre-commit-review is local developer tooling and static-analysis/code-review
infrastructure. It is not a network-security product. Gitleaks remains an
optional local model-input redaction layer, and rust-analyzer remains an
explicit semantic context provider.

The controls in this design provide reproducible artifact selection,
integrity checks, bounded provisioning, and release evidence. They do not
claim an operating-system network sandbox, proof of upstream build provenance,
or complete prevention of malicious behavior by an authorized executable.

No ordinary review, Fast Mode, repository-index, SQLite, or static-analysis
orchestration path downloads or invokes rust-analyzer. Downloads occur only
during an explicit installation or provisioning command.

## Decision Summary

The current Gitleaks runtime trust boundary is retained: exact bytes, no PATH
discovery, a version and capability probe, an explicit absolute-path override,
and a best-effort failure that leaves review available. Its hard-coded
distribution implementation is replaced before a second third-party tool is
added.

Delivery 5A introduces a strict `third_party_artifacts/v1` manifest, a
Rust-backed artifact manager, per-platform core and Gitleaks packs, immutable
content-addressed caching, external-binary SBOM entries, and project release
attestations. The all-platform `pre-commit-review-runtime.tar.gz` is retired.

Delivery 5B pins rust-analyzer stable tag `2026-07-27` for the four supported
platforms. Project CI downloads fixed upstream assets, verifies them, and
repackages them into project-published provider packs. The installer accepts
`--with-rust-analyzer` as an explicit opt-in, installs only the current
platform, and generates a target-local Delivery 4 profile and provider
registry with absolute paths and exact digests.

Provider pack versions are independent from both the upstream tool version and
the core release version. A core manifest names one exact active pack version
and outer SHA256 for each supported platform. There is no direct-upstream,
package-manager, rustup, PATH, `latest`, or `nightly` fallback.

## Goals

- Make one strict manifest the source of truth for artifact identity, platform
  mapping, project release asset, outer digest, installed executable digest,
  license evidence, SBOM, probes, and lifecycle state.
- Download only the pack needed for the current platform and selected
  capability.
- Keep downloaded cache entries immutable and make installed targets
  independent copies, never references into a cache directory.
- Preserve Gitleaks installation, explicit override, `--no-download`, doctor,
  and fail-open review semantics while changing its distribution internals.
- Make explicit rust-analyzer installation transactional and generate inputs
  accepted unchanged by the Delivery 4 CLI and schemas.
- Produce honest release evidence for project repackaging, external
  executables, exact manifest bytes, and exact SBOM bytes.
- Establish real-server compatibility, determinism, latency, memory, cleanup,
  offline, and sustained-fuzz evidence on all supported platforms.
- Support revoking a canonical pack in a subsequent core manifest without
  pretending that an already installed offline copy can be remotely disabled.

## Non-Goals

- Automatic provider discovery, selection, invocation, or update.
- A global provider registry or mutation of a user-supplied registry.
- Downloading during provider execution or analysis.
- Accepting an unpinned upstream release, moving tag, package-manager result,
  rustup component, PATH executable, or arbitrary mirror bytes.
- Building rust-analyzer from source in this delivery.
- Replacing Gitleaks, adding a second secret scanner, or generalizing the
  secret-finding execution protocol.
- Making the rust-analyzer provider part of a default review or index path.
- Persisting rust-analyzer semantic results or claiming a complete runtime call
  graph.
- Claiming that a project attestation proves how the upstream project built its
  binary.
- A remote revocation lookup or kill switch.

## Delivery Boundaries

Delivery 5A and Delivery 5B receive separate implementation plans and commit
series. Delivery 5A must be accepted before Delivery 5B changes the installer
or release surface for rust-analyzer.

Delivery 5A owns:

- the distribution and pack schemas;
- the generic artifact manager and immutable cache;
- safe pack verification and target provisioning;
- Gitleaks migration;
- platform-specific core and Gitleaks release packs;
- pack receipts, generic doctor behavior, revocation semantics, external
  binary SBOM records, release attestations, and release immutability gates.

Delivery 5B owns:

- the rust-analyzer upstream source lock and project pack build;
- independent provider-pack publication and the generated manifest-update PR;
- `install.sh --with-rust-analyzer`;
- generated Delivery 4 profile and registry files;
- real-server fixtures on four platforms;
- process-tree memory and latency gates;
- PR, scheduled, and release fuzz durations;
- final provider-pack release-readiness evidence.

## Distribution Manifest

### Location And Ownership

The repository stores the canonical manifest at
`third_party_artifacts/manifest.json` and its Draft 2020-12 schema at
`collect-diff-context-cli/schemas/third-party-artifacts.schema.json`.
Its semantic identity is `third_party_artifacts/v1`:

```json
{
  "schema_version": 1,
  "kind": "third_party_artifacts",
  "release_repository": "junit/pre-commit-review",
  "revocation_index_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
  "packs": []
}
```

The all-zero digest in this structural example is illustrative only; a
published manifest must contain the exact digest of its canonical revocation
index.

The packaged manifest is trusted project input, not repository-under-review
configuration. Provider execution never searches a candidate repository for
this file. Platform core packs include the exact manifest used by that core
release, and the core pack inventory binds its digest.

The internal pack, target receipt, artifact-manager report, and benchmark
baseline contracts have sibling strict schemas named
`third-party-artifact-pack.schema.json`,
`third-party-artifact-receipt.schema.json`,
`third-party-artifact-report.schema.json`, and
`third-party-artifact-baseline.schema.json` in the same schema directory. The
revocation index uses `third-party-artifact-revocations.schema.json`, and the
CI-only upstream input uses `third-party-source-lock.schema.json`. Platform core
inventories use `pre-commit-review-core-pack.schema.json`.

All schema objects use `additionalProperties: false`. Rust semantic validation
also enforces records sorted by artifact id, platform id, and pack version;
unique composite keys and pack asset names; lowercase SHA256 values; bounded
strings; and exact enum values. The manifest is at most 1 MiB, contains at most
256 pack records, and contains at most one `active` record for each
artifact/platform pair.

### Pack Records

Each pack record contains:

- `artifact_id`: a stable lowercase identifier such as `gitleaks` or
  `rust-analyzer`;
- `artifact_role`: a closed enum such as `sanitizer` or
  `repository-context-provider`;
- `tool_version`, `upstream_repository`, `upstream_tag`, and, when published by
  upstream, `upstream_commit`;
- canonical source-lock SHA256 used to build the pack;
- `platform_id` and exact target triple;
- `state`, either `active` or `revoked`;
- independent `pack_version`;
- immutable project release tag and exact asset name;
- expected compressed size, hard maximum size, and outer pack SHA256;
- expected `pack-manifest.json` SHA256 and `sbom.cdx.json` SHA256;
- pack format, fixed to the normalized project pack format;
- expected installed executable path, size, and SHA256;
- a closed version-probe id, capability-probe id, and exact expected version;
- license component identity and the expected license files in the pack;
- the SBOM component identity expected in the pack-level CycloneDX document;
- `default_configuration_sha256` for sanitizer roles, binding the project
  default configuration while leaving explicit user configuration under
  explicit-user-trust semantics;
- `quality_baseline_sha256` for provider roles, binding the reviewed
  pack-versioned latency baseline;
- revoked reason and replacement pack version only when state is `revoked`.

Probe ids select code-owned argument and parser implementations. The manifest
cannot provide a shell command, arbitrary argument vector, regular expression,
environment variable, or destination path.

The project release URL is constructed from the manifest's fixed project
repository, immutable release tag, and asset name. It is not an arbitrary URL
template. The downloader permits a bounded HTTPS redirect chain required by
GitHub release assets, rejects protocol downgrade, and has fixed connection,
read, total-byte, and total-time limits.

### Lifecycle State

Only the single `active` record for an artifact/platform pair may be
provisioned. Revoked historical records can coexist with its replacement, so
an updated doctor can recognize an installed receipt and reject that exact
canonical pack with a stable reason. A replacement is another explicitly
versioned record and never an implicit newest version.

An old offline core installation retains its old manifest and therefore cannot
learn a later revocation. Documentation and doctor output must state this
limitation. There is no remote lookup in doctor or provider execution.

The manifest keeps active records and a bounded recent window of full revoked
pack records. Older revoked digests move to the target-local
`runtime/distribution/revocations.json`, whose digest is pinned by
`revocation_index_sha256` and whose entries are sorted by pack digest. The
index is append-only and never silently drops a revoked digest; its initial
hard ceiling is 16,384 entries or 8 MiB. A release that would exceed that
ceiling fails and must publish a reviewed compacted/indexed format before
shipping. Doctor rejects a receipt found in either the full manifest records or
the compact index, so active replacement does not exhaust the main manifest's
pack-record budget.

## Project Pack Contract

### Normalized Format

Third-party packs use reproducible gzip-compressed POSIX ustar on every
platform. The gzip timestamp is zero and its optional filename and comment are
empty. Tar members are path sorted, timestamps and numeric owner/group ids are
zero, owner/group names are empty, directory and executable modes are `0755`,
and other regular-file modes are `0644`. The pack builder uses the pinned
pure-Rust `miniz_oxide` backend through the locked `flate2` dependency at
compression level 9, emits gzip OS byte 255 and XFL 2, and emits the canonical
ustar end-of-archive blocks. JSON members use compact `serde_json::to_vec`
serialization with no trailing newline. A pack contains exactly:

```text
pack-manifest.json
bin/<tool-platform-name>
licenses/*
sbom.cdx.json
```

Windows uses the expected `.exe` name. Directories and regular files are the
only permitted archive members. Symlinks, hardlinks, devices, sparse files,
absolute paths, parent traversal, duplicate normalized paths, case-folded path
collisions, alternate data streams, and unexpected files are rejected.

`pack-manifest.json` is strict, bounded, and identifies the artifact id, tool
version, pack version, platform, target triple, upstream source asset and
digest, canonical source-lock digest, project pack asset, and every payload
file. Each payload entry records its path, byte size, SHA256, and role. The
outer pack digest binds the internal
manifest; the internal inventory independently binds the executable, licenses,
and SBOM after extraction.

The verifier limits archive entries, compressed bytes, expanded bytes, each
file size, path length, and metadata size before allocating or writing. Initial
hard ceilings are 128 entries, 512 MiB compressed, and 2 GiB expanded. Each
manifest record may lower but never raise those compiled ceilings.

### Verification Order

The artifact manager performs these checks in order:

1. Select one exact active pack record from the strict core manifest.
2. Stream the pack into a private temporary file while enforcing size and time
   limits and computing the outer SHA256.
3. Reject an outer digest or exact-size mismatch before extraction.
4. Parse the archive inventory without following links or writing payloads;
   reject unsafe, duplicate, unexpected, or oversized entries.
5. Extract allowlisted regular files into a private same-filesystem staging
   directory.
6. Validate the strict internal manifest and its identity against the core
   manifest selection.
7. Recompute every internal file size and SHA256.
8. Validate that the CycloneDX document contains the expected external binary
   component, hashes, source, license, platform, and evidence-level fields.
9. Run the code-owned version and capability probes in the same bounded private
   runtime model used for trusted child processes.
10. Publish a write-once, digest-pinned cache entry only after every check
    succeeds.

No partial extraction or failed probe becomes a usable cache entry. Errors are
stable bounded codes and do not include response bodies, child stderr, or
temporary paths.

## Artifact Manager

### Module And CLI

Delivery 5A adds a focused Rust library module and an `artifacts` subcommand
family to the existing cross-platform `collect-diff-context` binary. The
module owns manifest validation, platform selection, bounded fetching, archive
inspection, hashing, cache publication, target copying, receipts, probes, and
doctor results. This deliberately avoids a new bootstrap binary: the four
tracked platform collector binaries are refreshed with the subcommand, and
each platform core pack carries the matching binary. Shell scripts remain
compatibility and installation wrappers rather than independent policy
implementations.

The command surface is `collect-diff-context artifacts verify|provision|doctor`.
Every input path is absolute, every selected artifact and platform is named,
and machine output is one bounded JSON document. A local pack file is accepted
only as an offline transport for bytes whose exact outer digest is already
pinned in the manifest; it is not an alternate artifact source.

`doctor` requires `--target-root /absolute/managed-skill` and optionally an
`--artifact-id`; without an artifact id it checks every target receipt and the
distribution/revocation files. It reopens the target-local canonical manifest,
core inventory, retained pack manifests, receipts, profiles, and registry,
then re-hashes the installed files and checks current active/revoked state.
The target root is never inferred from the current working directory.

`install.sh --doctor` retains its existing source/core-payload Gitleaks
diagnostic. `install.sh --doctor-target /absolute/managed-skill` is the new
target-aware entry point and delegates to the artifact doctor; it checks moved
targets and reports stale absolute provider paths without rewriting them. The
installed payload also includes `scripts/check_artifacts.sh`, a thin wrapper
that passes its explicit target root to the same command. Neither doctor mode
downloads, repairs, migrates, or selects a replacement.

Production behavior has no base-URL environment override. Tests exercise a
fixture transport through the Rust test boundary and local digest-pinned pack
files rather than weakening the production source policy.

In a source clone, the installer resolves the host-compatible tracked
`collect-diff-context` binary and never invokes Cargo to obtain the artifact
manager. If that binary lacks the artifact subcommand, optional Gitleaks
provisioning reports its existing unavailable downgrade and required
rust-analyzer provisioning fails before target commit. A release/core-pack
installation has no such fallback: its core inventory must contain the
matching collector binary and `install.sh` rejects a missing or mismatched
core tool before copying a target.

### Content-Addressed Cache

The artifact cache uses the existing platform cache-root policy and this fixed
suffix:

```text
third-party-artifacts/sha256/<pack-digest>/
```

Each entry contains the extracted allowlisted pack and a pack-intrinsic verified
receipt. The cache receipt records the outer pack digest, internal manifest and
payload hashes, probe results, verifier version, and cache format version; it
contains no core-manifest digest, lifecycle state, target path, or installation
receipt fields. The same digest-pinned pack can therefore be referenced by a
later core manifest without a cache-key collision.
Entries are created with private permissions through a sibling staging
directory and atomic rename. The cache is a write-once/content-addressed
policy: read-only permissions are best effort, and every provision/doctor use
revalidates the pinned hashes. A mismatched or incomplete existing entry is a
corrupt-cache error; it is never repaired in place or silently accepted.

The cache resolver uses the platform default user cache root and a dedicated
`third-party-artifacts` namespace. An optional
`PRE_COMMIT_REVIEW_ARTIFACT_CACHE_DIR` override must be absolute; it is
rejected when it is inside the candidate repository, Git common directory,
snapshot root, or managed installation target. When repository context is
available, the same `.git` and ancestor checks used by the existing cache-root
policy are applied. Artifact provisioning has no repository-relative cache
fallback.

Provisioning copies regular files from a verified cache entry into an
installation staging tree, then re-hashes the target copy. It does not symlink,
hardlink, or record cache paths in runtime profiles. Deleting or relocating the
cache after installation cannot change or disable an installed target.

### Target Receipts

Every installed canonical pack has a strict target-local receipt containing
the distribution manifest digest, artifact id, tool version, pack version,
platform, pack SHA256, installed relative paths and SHA256 values, SBOM digest,
license digests, probe results, and lifecycle state observed at installation.
Receipts contain no cache or temporary paths.

Doctor validates the current packaged manifest, receipt, target inventory,
internal hashes, version/capability probe, and active/revoked state. Doctor is
read-only. It does not fetch, repair, migrate, or select a replacement.

## Installer And Packaging

### Platform Core Packs

Delivery 5A replaces the single all-platform runtime archive with four core
archives:

- `pre-commit-review-core-<core-version>-darwin-arm64.tar.gz`;
- `pre-commit-review-core-<core-version>-darwin-amd64.tar.gz`;
- `pre-commit-review-core-<core-version>-linux-amd64.tar.gz`;
- `pre-commit-review-core-<core-version>-windows-amd64.tar.gz`.

Each core pack contains only project-owned binaries for its platform, the
skill payload, `install.sh`, schemas, documentation, project licenses, its
strict `core-pack-manifest.json` inventory, the core SBOM, the
`collect-diff-context` artifact subcommand, and the canonical distribution
manifest.
It does not contain binaries for other platforms or a rust-analyzer binary.

Gitleaks and rust-analyzer are separate platform packs. No release recreates a
convenience archive containing all supported platforms. Their asset grammars
are `pre-commit-review-gitleaks-<pack-version>-<platform>.tar.gz` and
`pre-commit-review-rust-analyzer-<pack-version>-<platform>.tar.gz`.

The core archive has the same reproducible gzip/ustar settings as a
third-party pack. Its strict `core-pack-manifest.json` lists every regular
file, mode, byte size, SHA256, core version, platform, target triple, schema
version, canonical distribution-manifest SHA256, and revocation-index SHA256.
It contains only files present when the core archive is built. The installer
validates this inventory before using a core pack and retains it in the target.
Third-party pack manifests are instead pinned by the distribution records and
copied into the target with their target receipts during provisioning. Release
assets publish an outer archive SHA256 file and project attestation; those are
verified by the clean consumer job and by the documented release bootstrap
procedure.

`install.sh` never bootstraps or downloads a core pack. A release user selects
the core archive matching the host platform, verifies its published archive
digest and attestation under the project signer policy, extracts it, and runs
its included installer. A source clone follows the existing clone-install
workflow and trusts the checked-out files under the repository's normal review
trust model; the refreshed tracked collector binary supplies the artifact
subcommand without a Cargo build.

The release bootstrap trust policy is external to the extracted core files.
The consumer verifies the sidecar SHA256 and the project attestation for the
core archive before extraction, requiring all of the following: subject digest
equal to the archive, GitHub repository `junit/pre-commit-review`, the release
workflow `.github/workflows/release.yml` and immutable version tag as the
source ref, the exact source commit, and the GitHub Actions OIDC/Sigstore
issuer. Provider and Gitleaks packs analogously require
`.github/workflows/artifact-pack-release.yml`. An equivalent offline
verifier may use a previously pinned attestation and digest; an unscoped
subject-only attestation is insufficient. This first-download check is
documented and tested as a release-consumer gate, not delegated to the
package's own inventory after extraction.

### Staged Installation

`install.sh` continues to stage copy-mode installations next to the final
target. It copies the core payload, provisions selected third-party packs into
the staging tree, generates receipts and provider inputs, revalidates the
complete staging tree, and only then reaches the existing target replacement
commit point.

Gitleaks remains best effort. If its default download or validation fails, the
installer logs that redaction is unavailable and may commit the otherwise
valid core target, exactly as today. `--no-download` permits only a valid
existing canonical cache entry or the existing explicitly trusted absolute
Gitleaks path; ordinary review remains available when neither exists.

`--no-download --with-rust-analyzer` is also valid, but the required provider
pack must already exist as a verified canonical cache entry. If it does not,
installation fails before target commit. An air-gapped operator can seed that
entry with `collect-diff-context artifacts provision` and an absolute local
pack whose
bytes match the manifest's exact outer digest.

`--with-rust-analyzer` is an explicit required request. Any missing pack,
download error, digest mismatch, extraction rejection, probe failure, profile
generation error, or registry validation error aborts before target commit and
leaves an existing target unchanged. A newly verified write-once cache entry may
remain because cache population is separate from target mutation.

Delivery 5B supports `--with-rust-analyzer` for copy mode. Combining it with
`--link` fails during argument preflight before any download or mutation. Link
users retain Delivery 4's explicit user/CI-supplied profile and registry path;
this avoids writing generated absolute paths through a source-tree symlink.

### Installed Layout

Canonical third-party files live under a target-owned runtime directory:

```text
runtime/third-party/<artifact-id>/<pack-version>/bin/<installed-name>
runtime/third-party/<artifact-id>/<pack-version>/licenses/*
runtime/third-party/<artifact-id>/<pack-version>/pack-manifest.json
runtime/third-party/<artifact-id>/<pack-version>/sbom.cdx.json
runtime/artifact-receipts/<artifact-id>.json
runtime/distribution/manifest.json
runtime/distribution/core-pack-manifest.json
runtime/distribution/revocations.json
runtime/providers/rust-analyzer.profile.json
runtime/providers/provider-registry.json
```

The Gitleaks runtime resolver and doctor use its new target-owned canonical
path while retaining the existing environment overrides and behavior. Runtime
code never searches the cache, PATH, a package manager, or a global registry.

Moving an installed target invalidates generated absolute provider paths.
Doctor reports that state; it does not rewrite the registry. Re-running the
installer is the supported way to regenerate paths.

## Gitleaks Migration

Gitleaks is the first `third_party_artifacts/v1` entry and proves that the
generic layer preserves an existing product contract. Delivery 5A publishes
one Gitleaks pack per supported platform and changes fetch, installer, doctor,
tests, and release jobs to consume the manifest and artifact manager.

Its upstream asset inputs move to the same strict
`third_party_sources/v1` source-lock contract used by rust-analyzer. The lock
records the four exact archive URLs, sizes, upstream archive digests, extracted
binary digests, version output, and license source. It is CI-only; installer
selection still uses the project-published Gitleaks pack and the core manifest.

The existing hard-coded version file, archive digest table, binary digest
table, platform cases, and release matrix assertions cease to be independent
sources of truth. Compatibility scripts may retain their current names and
arguments, but they delegate selection and verification to the Rust manager.

These user-visible semantics remain unchanged:

- Gitleaks is optional and enabled by the normal installer unless
  `--no-download` is supplied.
- `PRE_COMMIT_REVIEW_GITLEAKS_BIN` remains an explicitly trusted absolute-path
  override and must still pass the pinned version and capability probe.
- `PRE_COMMIT_REVIEW_GITLEAKS_CONFIG` retains its current explicit
  configuration behavior. The project default configuration is digest bound by
  the active manifest/core inventory; an explicit user configuration remains
  `explicit-user-trust` under its current path rules and is reported as such.
- `PRE_COMMIT_REVIEW_FETCH_PROGRESS` retains `auto`, `always`, and `never`
  validation and controls the Rust manager's bounded download progress output;
  `auto` uses interactive stderr detection, `always` forces stderr progress,
  and `never` suppresses it. Progress never contaminates JSON stdout, and
  invalid values fail before any network request.
- No PATH discovery or implicit executable fallback is added.
- Scanner unavailability is reported and review output remains allowed.
- Bundled/canonical bytes are digest checked before use.

Generic doctor output adds pack version, pack digest, executable digest, SBOM
digest, and lifecycle state. The compatibility Gitleaks doctor retains its
existing `redaction_available` and `review_output_allowed` meanings. Gitleaks
packs follow the same publish-first, independently verify, then reviewed
manifest-update sequence as provider packs; a core release does not consume a
Gitleaks pack created by that same release run.

Changing the scanner finding contract, fail-open policy, output budgets, or
redaction algorithm is outside this distribution migration and requires a
separate design.

## Rust-Analyzer Provider Pack

### Initial Upstream Pin

The first provider pack uses rust-analyzer stable tag `2026-07-27`. The source
lock at
`third_party_artifacts/sources/rust-analyzer-2026-07-27.json` records exact
upstream asset names, reported upstream digests, locally
verified archive digests, extracted executable digests, sizes, source
repository, tag, and upstream commit for:

| Platform id | Target triple |
| --- | --- |
| `darwin-arm64` | `aarch64-apple-darwin` |
| `darwin-amd64` | `x86_64-apple-darwin` |
| `linux-amd64` | `x86_64-unknown-linux-gnu` |
| `windows-amd64` | `x86_64-pc-windows-msvc` |

The provider `linux-amd64` pack uses the reviewed GNU/Linux asset and requires
glibc 2.28 or newer. The upstream musl asset is not self-contained: it requires
both `/lib/ld-musl-x86_64.so.1` and a musl-compatible `libgcc_s.so.1`, neither
of which is available on a stock Ubuntu runner or guaranteed by the installer.
Delivery 5B therefore does not claim Alpine or other musl-host support. The
installer must reject an explicit rust-analyzer request before provisioning
when the Linux host cannot prove the required glibc baseline.

The source lock is a strict `third_party_sources/v1` value validated by
`third-party-source-lock.schema.json`. It contains only bounded records for
the named artifact, exact upstream tag and commit, the allowlisted upstream
repository, and one asset per supported platform. Each asset records the
exact upstream URL, archive name, archive size and SHA256, extracted
executable name, executable size and SHA256, expected version-probe output,
and required license source paths. URLs must match the fixed upstream GitHub
repository and HTTPS release shape; `latest`, `nightly`, arbitrary hosts,
redirect templates, shell commands, and environment values are rejected.
Canonical JSON bytes of the source lock have a reviewed SHA256. The pack build
workflow, its attestation materials, and the generated core manifest all bind
that source-lock digest, but the installer never consumes the source lock or
contacts its upstream URLs.

The pack version uses an independent revision namespace such as
`2026.07.27-pcr.1`; equality with the upstream tag is neither required nor
implied. Repacking unchanged upstream bytes requires a new pack version and a
new reviewed digest.

### Pack Build

The provider-pack workflow consumes a reviewed source lock at an exact project
commit. It downloads only the four fixed upstream assets, verifies their
recorded archive digests, extracts only the expected executable, verifies the
executable digest and version, and creates the normalized project packs.

This workflow repackages upstream release binaries; it does not compile
rust-analyzer. It emits a standard build-provenance attestation plus a
project-specific `pre-commit-review.artifact-pack/v1` composition predicate.
The predicate lists the source-lock digest, every upstream archive digest, the
pack-builder source commit, normalized pack-manifest digest, SBOM digest, and
generator configuration digest. A verifier that checks that predicate can
conclude that the named project workflow produced the output from those named
inputs. It still cannot conclude that upstream built the executable bytes from
the named upstream commit.

### Independent Publication Sequence

Provider packs are published before a core manifest references them:

1. Review and commit the upstream source lock and pack-build workflow changes.
2. Build, verify, SBOM, attest, and publish the four independently versioned
   provider packs in an immutable project release.
3. Verify the published assets and attestations from a clean workflow.
4. Generate a normal manifest-update pull request containing the final pack
   version, release tag, asset names, sizes, outer SHA256 values, executable
   SHA256 values, source-lock digest, quality-baseline digest, and per-platform
   benchmark baselines.
5. Run all Delivery 5B PR gates against those already-published exact packs.
6. Require human review and merge of the generated manifest update.
7. Permit a core release to consume only the merged manifest bytes.

The core release never consumes an unpublished artifact from the same run and
never rewrites a manifest digest during release.

The first rust-analyzer project pack may be bootstrapped from its reviewed
provider-distribution branch without merging that incomplete branch into the
default branch. The initial exact immutable tag
`artifact-rust-analyzer-2026.07.27-pcr.1` failed before publication because its
Linux source record selected the dynamically linked upstream musl asset. That
public tag and its failed run remain immutable historical evidence; they are
never moved, deleted, or reused.

The second exact immutable tag
`artifact-rust-analyzer-2026.07.27-pcr.2` corrected the Linux ABI selection,
but it also failed before publication. The reviewed GNU/Linux executable emits
the exact version output `rust-analyzer 0.3.2989-standalone`, while the other
three assets emit
`rust-analyzer 0.3.2989-standalone (12c3381f0b 2026-07-26)`. The `pcr.2`
source record incorrectly reused the longer output for Linux. Its public tag
and failed run also remain immutable historical evidence.

The corrected bootstrap uses pack version `2026.07.27-pcr.3` and accepts only
the exact immutable tag `artifact-rust-analyzer-2026.07.27-pcr.3` as a `push`
trigger. The corrected source lock selects the upstream
`rust-analyzer-x86_64-unknown-linux-gnu.gz` asset for `linux-amd64`, binds its
reviewed archive and executable digests, records the Linux-specific short
version output, and leaves the other three upstream assets and version outputs
unchanged. The exact `pcr.3` tag selects the rust-analyzer build, clean
verification, and publication jobs without ambient inputs. No wildcard
provider tag, moving tag, branch push, unrelated tag, or historical `pcr.1` or
`pcr.2` tag starts the corrected publication. The resulting release still
precedes and is independently verified before any core manifest update.

## Generated Provider Authorization

After copying the verified rust-analyzer executable into the target staging
tree, the installer generates
`runtime/providers/rust-analyzer.profile.json` using the existing strict
`repository_context_provider_profile/v1` contract. It records:

- provider kind `rust-analyzer` and exact upstream tool version;
- final installed executable SHA256;
- the existing canonical hardened configuration SHA256;
- exact target triple and `toolchain_mode: none`;
- arguments `--stdio`;
- the existing fixed hardening values and authorized maximum limits.

The installer then generates
`runtime/providers/provider-registry.json` using the existing
`repository_context_provider_registry/v1` contract. Its single generated entry
uses provider id `rust-analyzer-project-pack`, the final target's absolute
profile and executable paths, exact profile and executable SHA256 values, and
the same provider/configuration/target/toolchain identities.

Generation uses final target paths even though files are still in staging.
The installer first resolves the final target from a canonical absolute parent
and rejects a target whose parent cannot be resolved safely. Before commit, it
validates both JSON values with the Rust contract types, hashes the exact
bytes, verifies that every non-path binding matches, and confirms that replacing
the staging prefix with the final target resolves to the staged files. Profile
and registry JSON are serialized with the existing Rust
`serde_json::to_vec`/`sha256_json` canonical compact representation: struct
field order is fixed, whitespace is not emitted, and there is no trailing
newline. The raw profile bytes must hash to both the registry's
`profile_sha256` and the existing `AuthorizedProviderProfile::sha256()` value;
the raw canonical registry bytes are the digest passed to the Delivery 4 CLI.

The generated registry is target-local installation output. It is not the
distribution manifest, a global registry, an automatically discovered default,
or permission to invoke the provider. The Delivery 4 CLI still requires the
caller to pass the registry path, expected registry SHA256, provider id, model,
request, source, and expected scope explicitly.

## Provider Runtime Boundaries

Distribution does not weaken Delivery 4. A real server still runs from a
Drop-safe private runtime with an empty PATH, fixed locale, private home/temp
and target directories, offline Cargo variables, invalid proxy endpoints, no
shell, no toolchain installation, and no repository command execution.

The exact profile and executable are verified before spawn and after the
session. The snapshot, project model, registry, profile, executable, and scope
bindings remain mandatory. Build scripts, proc macros, sysroot discovery,
workspace discovery, check-on-save, and dependency fetching remain disabled.

Pack provisioning never adds a runtime download, rustup, package-manager,
direct-upstream, `latest`, `nightly`, PATH, or user-home registry fallback.

## Resource And Performance Gates

### Existing Hard Limits

All Delivery 4 protocol, framing, request, message, source, graph, output,
deadline, and process-cleanup limits remain hard. A real server does not get a
larger profile merely to pass compatibility tests.

### Process-Tree Memory

Every real-server run has a non-negotiable 2 GiB sampled process-tree resident
memory acceptance threshold. The provider monitors the managed child and
descendants with platform-specific process accounting at intervals no longer
than 100 ms. When the observed sum exceeds 2 GiB, it terminates and reaps the
process tree, publishes no semantic facts, and uses the existing bounded
failure form with a stable `process-tree-rss-limit` code.

Where an operating system offers a stronger inherited/job limit, the provider
sets it in addition to monitoring. The documented cross-platform claim remains
an observed and enforced sampled process-tree RSS policy, not a universal
kernel peak or containment boundary; a sub-100-ms transient may not be
observed. Failure to obtain required process accounting is a failed real-server
gate, not an informational warning.

### Latency Baselines

Each platform has a reviewed, pack-versioned baseline at
`third_party_artifacts/baselines/rust-analyzer-<pack-version>.json` for every
release performance fixture. Provisioning and pack extraction are excluded.
The strict canonical baseline records the pack, executable, source-lock,
profile, fixture, request, and runner-class digests; raw sample milliseconds;
sample count; and computed p95. Its canonical file SHA256 must equal the
`quality_baseline_sha256` in the active pack record. Timing starts immediately
before the Delivery 4 run command spawns the server and
ends after report validation and postflight revalidation, so startup,
readiness, Call Hierarchy traversal, normalization, and cleanup are included.

Baseline and release measurements use the same hosted-runner class, exact pack,
fixture bytes, request, profile, and sample procedure. Each metric uses at
least 20 isolated runs after one unmeasured warm-up, and p95 is the nearest-rank
95th percentile. Every sample still obeys the existing 30-second deadline.

For each platform and fixture, release acceptance is:

```text
measured_p95 <= baseline_p95 * 1.25 + 250 ms
```

The implementation computes the threshold in integer milliseconds as
`ceil(baseline_p95_ms * 5 / 4) + 250`.

The first provider-pack publication establishes its reviewed baseline before
the core manifest update. Later baseline changes are ordinary reviewed data
changes and cannot be generated or accepted inside the core release job.

## Real-Server Test Strategy

Fake-server tests remain the deterministic source for adversarial framing,
invalid JSON-RPC, reordered responses, timeouts, crashes, oversized output,
and exact failure-state coverage. Real-server tests add compatibility evidence;
they do not replace fake-server tests.

The real fixtures are repository-owned, network-independent, and require no
Cargo execution, dependency fetch, sysroot, or installed Rust toolchain. They
cover:

- one crate with exact incoming and outgoing direct calls;
- multiple linked crates with deterministic cross-crate calls;
- unresolved, dynamic-dispatch, macro, and unsupported cases that must remain
  honestly partial;
- UTF-8/UTF-16 positions, Unicode identifiers, CRLF, file URIs, and stale
  range rejection;
- depth-one and depth-two BFS ordering, cycles, deduplication, and output
  bounds;
- readiness/capability rejection, cancellation, timeout, process cleanup, and
  postflight executable/profile/snapshot drift;
- two identical runs producing byte-identical normalized reports after
  excluding documented elapsed metrics.

PR CI runs a short real-server smoke on all four platforms using the exact
published pack selected by the candidate manifest. It verifies version,
capabilities, quiescent readiness, one known call edge, deterministic rerun,
offline environment, cleanup, and the 2 GiB limit machinery.

Scheduled and release CI run the complete fixture set on all four platforms.
Release CI additionally enforces every per-platform p95 threshold and records
peak process-tree RSS, compressed/expanded pack size, server version, pack
digest, executable digest, fixture digest, and runner identity as evidence.

## Fuzz Gates

The existing `repository_context_frame` and `repository_context_messages`
targets remain the fuzz surface. The hardened harness invariants and named
corpus seeds remain source controlled; generated hash-named corpus files are
not committed.

- Every PR runs 256 iterations per target with the existing per-input timeout.
- Scheduled CI runs 15 minutes per target.
- Provider-pack and core release CI run 30 minutes per target.

Any crash, timeout, sanitizer finding, counter overflow, bound violation, or
non-deterministic invariant blocks the relevant gate. Release evidence records
toolchain, target, corpus digest, duration, and exit status.

## SBOM, Attestation, And Release Trust

Every third-party pack includes a CycloneDX 1.5 SBOM that names the external
executable as a top-level component and records tool version, supplier/source
URL, license, upstream archive hash, executable hash, pack id and version,
platform, and the pack's contains/dependency relationship. The outer pack hash
is recorded by the core manifest, target receipt, release metadata, and
attestation rather than inside the pack, which avoids a circular digest.

For an upstream prebuilt executable without an upstream SBOM, the project SBOM
states that evidence is component-level and the complete transitive dependency
closure is unknown. A Cargo-only SBOM must never be presented as covering a
Gitleaks or rust-analyzer binary.

Release workflows generate attestations for:

- each platform pack;
- each pack's `pack-manifest.json`;
- each pack's `sbom.cdx.json`;
- the core distribution manifest;
- each platform core pack and core SBOM.

Release and an independent verification job validate subject names and exact
digests, the predicate type, the GitHub repository/workflow signer identity,
the immutable source ref and commit, the GitHub OIDC/Sigstore issuer, and all
composition-predicate material digests. A subject-only attestation is rejected.
Critical third-party GitHub Actions are pinned to reviewed commit SHAs. Moving
major tags are not accepted in the release trust path.

The repository must enable GitHub release immutability for future releases
before documentation or release notes claim immutable packs. Build-only CI may
test the workflow earlier, but publication and the immutable-release claim are
gated on the setting. Immutability and project attestations strengthen the
project release; neither is described as upstream build provenance.

## Failure Semantics

Manifest/schema/identity, download, digest, archive, SBOM, license, probe,
cache, receipt, and revocation failures are distinct bounded artifact-manager
codes. They never fall through to another version or source.

For default Gitleaks provisioning, those failures retain the existing optional
redaction downgrade and allow ordinary review. For explicit
`--with-rust-analyzer`, the same failures abort installation before target
commit. During provider execution, authorization or binding failures publish
no report; a schema-valid provider `partial` or `unavailable` report retains
the Delivery 4 exit semantics.

No error includes downloaded body bytes, child stderr, raw LSP frames, cache
roots, private runtime roots, credentials, or untrusted pack text.

## CI And Release Matrix

Delivery 5A CI covers:

- strict schema and Rust semantic validation;
- generated pack fixtures for every unsafe archive shape and limit;
- digest, internal inventory, SBOM, license, probe, cache-race, write-once-cache,
  corrupt-cache, receipt, revocation, and target-copy behavior;
- Gitleaks compatibility, install downgrade, explicit override, no-download,
  doctor, and no-PATH reachability;
- platform core/Gitleaks pack contents and absence of other-platform binaries;
- release SBOM and attestation verification in build-only mode.

Delivery 5B CI adds:

- exact rust-analyzer source-lock and normalized pack reproduction checks;
- transactional installer and generated profile/registry contract tests;
- no provider discovery or default-pipeline reachability;
- four-platform PR real-server smoke;
- four-platform scheduled/release full fixtures and resource evidence;
- per-platform release p95 gates;
- the PR, scheduled, and release fuzz durations defined above;
- published-pack, manifest, SBOM, license, receipt, revocation, and attestation
  verification from a clean consumer job.

All Rust code uses the repository's Rust 1.95 locked test, format, and Clippy
gates. Archive parsing, JSON, URL handling, and hashing use structured Rust
libraries rather than shell parsing. Shell compatibility wrappers remain under
ShellCheck and deterministic shell integration tests.

Core, Gitleaks-pack, provider-pack, and release jobs install exact Rust
`1.95.0`, use the committed Cargo lockfile with `--locked`, and record the
toolchain and lockfile digest in release evidence. The current moving `stable`
release toolchain is replaced; release builds do not silently update Rust or
dependencies.

## Delivery 5A Completion Criteria

Delivery 5A is complete when:

1. `third_party_artifacts/v1` is strict, bounded, schema validated, and the
   sole source of canonical third-party pack policy.
2. The Rust artifact manager safely fetches, verifies, caches, provisions,
   receipts, and doctors fixture packs under all defined limits.
3. Cache entries follow a write-once, digest-pinned policy with revalidation,
   and target installations remain usable after cache removal.
4. Gitleaks uses the generic manifest and manager with no user-facing semantic
   regression and no PATH fallback.
5. The all-platform runtime archive is replaced by four platform core packs
   and four platform Gitleaks packs.
6. External binary components, licenses, exact hashes, and honest evidence
   scope appear in pack SBOMs.
7. Pack, manifest, and SBOM attestations are generated and independently
   verified with SHA-pinned release actions.
8. Active/revoked behavior and the offline no-remote-revocation limitation are
   tested and documented.
9. No rust-analyzer binary is distributed or installed by Delivery 5A.

## Delivery 5B Completion Criteria

Delivery 5B is complete when:

1. The four rust-analyzer `2026-07-27` upstream assets and extracted binaries
   are exact-digest locked and normalized into independently versioned packs.
2. Provider packs are published, SBOMed, attested, independently verified, and
   immutable before a reviewed core manifest references them.
3. `install.sh --with-rust-analyzer` installs only the current platform and is
   transactional for every provider-specific failure.
4. Generated profile and registry bytes validate against the unchanged
   Delivery 4 contracts and contain only final target absolute paths and exact
   digests.
5. No runtime download, PATH, rustup, package-manager, direct-upstream,
   `latest`, `nightly`, automatic discovery, or global registry fallback exists.
6. Four-platform PR smoke and scheduled/release real fixture suites pass.
7. Every real run enforces the 2 GiB process-tree RSS acceptance limit and all
   existing protocol/resource limits.
8. Every release p95 satisfies `baseline * 1.25 + 250 ms` on its platform and
   fixture.
9. Both fuzz targets pass 256 PR iterations, 15-minute scheduled runs, and
   30-minute release runs.
10. Ordinary review, Fast Mode, repository indexing, SQLite, and static-analysis
    orchestration remain unable to invoke the provider.

## Planning Transition

After this specification is reviewed, create two implementation plans in this
order: Delivery 5A artifact distribution and Gitleaks migration, then Delivery
5B rust-analyzer provisioning and quality evidence. Implementation does not
begin until the corresponding plan has been reviewed.
