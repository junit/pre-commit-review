# Third-Party Artifact Distribution And Gitleaks Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the hard-coded Gitleaks archive path with a bounded, digest-pinned third-party artifact manager and four platform core/Gitleaks packs without changing Gitleaks' user-facing behavior.

**Architecture:** A strict `third_party_artifacts/v1` manifest and CI-only `third_party_sources/v1` locks are parsed by a focused Rust `artifacts` module exposed through the existing tracked `collect-diff-context` binary. The manager streams and verifies normalized packs, publishes write-once content-addressed cache entries, copies verified files into an installation staging tree, and emits target receipts and doctor reports. `install.sh` and the existing Gitleaks scripts remain compatibility wrappers; scanner execution continues to use an explicit absolute path and best-effort semantics.

**Tech Stack:** Rust 1.95.0 with `--locked`, `serde`/`serde_json`, `sha2`, pinned `tar` and `flate2` (`miniz_oxide` backend, level 9), the repository's bounded HTTPS client, `tempfile`, Bash/ShellCheck, JSON Schema Draft 2020-12, CycloneDX 1.5, GitHub Actions pinned to reviewed commit SHAs, and the existing four platform collector binaries.

---

## Execution Boundary And File Map

Execute from `feature/provider-artifact-distribution`, created from `feature/SAST`. Delivery 5A must be accepted before Delivery 5B adds rust-analyzer installer or provider-pack release behavior. Do not modify ordinary review, Fast Mode, repository-index, SQLite, or static-analysis orchestration entry points except to prove that no artifact command is reachable from them.

Create:

- `third_party_artifacts/manifest.json`: reviewed canonical active/revoked pack records.
- `third_party_artifacts/revocations.json`: sorted digest-pinned compact revocation index.
- `third_party_artifacts/sources/gitleaks-<version>.json`: CI-only `third_party_sources/v1` source lock.
- `collect-diff-context-cli/src/artifacts/mod.rs`: public artifact-manager API and stable error/report types.
- `collect-diff-context-cli/src/artifacts/contract.rs`: strict manifest, source-lock, pack, receipt, report, baseline, and revocation types plus semantic limits.
- `collect-diff-context-cli/src/artifacts/pack.rs`: normalized tar/gzip inspection, safe extraction, internal manifest and SBOM verification.
- `collect-diff-context-cli/src/artifacts/cache.rs`: platform cache-root policy, content-addressed cache receipts, atomic write-once publication, and target copying.
- `collect-diff-context-cli/src/artifacts/transport.rs`: local digest-pinned transport and bounded HTTPS release-asset transport.
- `collect-diff-context-cli/src/artifacts/probes.rs`: code-owned executable version/capability probes.
- `collect-diff-context-cli/src/artifacts/cli.rs`: `artifacts verify|provision|doctor` parser and bounded JSON output.
- `collect-diff-context-cli/schemas/third-party-artifacts.schema.json`
- `collect-diff-context-cli/schemas/third-party-artifact-pack.schema.json`
- `collect-diff-context-cli/schemas/third-party-artifact-receipt.schema.json`
- `collect-diff-context-cli/schemas/third-party-artifact-report.schema.json`
- `collect-diff-context-cli/schemas/third-party-artifact-baseline.schema.json`
- `collect-diff-context-cli/schemas/third-party-artifact-revocations.schema.json`
- `collect-diff-context-cli/schemas/third-party-source-lock.schema.json`
- `collect-diff-context-cli/schemas/pre-commit-review-core-pack.schema.json`
- `collect-diff-context-cli/tests/artifact_contracts.rs`
- `collect-diff-context-cli/tests/artifact_pack.rs`
- `collect-diff-context-cli/tests/artifact_cache.rs`
- `collect-diff-context-cli/tests/artifact_cli.rs`
- `scripts/build_artifact_pack.sh`
- `.github/workflows/artifact-pack-release.yml`
- `tests/artifact_distribution_test.sh`

Modify:

- `collect-diff-context-cli/src/lib.rs` and `src/app.rs`: export the module and dispatch only the explicit `artifacts` subcommand before the ordinary collector parser.
- `collect-diff-context-cli/Cargo.toml` and `Cargo.lock`: add pinned archive/HTTP dependencies with Rust 1.95 compatibility.
- `collect-diff-context-cli/src/secret_scan.rs`: resolve the target-owned canonical Gitleaks executable through the manager while retaining explicit override and fail-open behavior.
- `install.sh`, `scripts/fetch_gitleaks.sh`, `scripts/check_gitleaks.sh`, and `scripts/lib/gitleaks_integrity.sh`: delegate distribution/doctor work to the Rust manager and preserve compatibility output.
- `.github/workflows/lint.yml`, `.github/workflows/release.yml`, `scripts/validate_schemas.py`, `tests/install_gitleaks_test.sh`, `tests/gitleaks_distribution_test.sh`, and `tests/secret_gate_test.sh`: enforce migration and release gates.
- `README.md`, `docs/helper-capabilities.md`, and `docs/gitleaks-distribution-strategy-research.md`: document target-aware doctor, offline/cache semantics, external-binary SBOM scope, and no remote revocation.

## Task 1: Define Canonical Artifact Contracts

**Files:**

- Create: `collect-diff-context-cli/src/artifacts/contract.rs`
- Create: `third_party_artifacts/manifest.json`
- Create: `third_party_artifacts/revocations.json`
- Create: `third_party_artifacts/sources/gitleaks-8.30.1.json`
- Create: `collect-diff-context-cli/schemas/third-party-artifacts.schema.json`, `third-party-artifact-pack.schema.json`, `third-party-artifact-receipt.schema.json`, `third-party-artifact-report.schema.json`, `third-party-artifact-revocations.schema.json`, `third-party-source-lock.schema.json`, `pre-commit-review-core-pack.schema.json`
- Modify: `collect-diff-context-cli/src/lib.rs`
- Test: `collect-diff-context-cli/tests/artifact_contracts.rs`

- [ ] **Step 1: Write failing typed-contract tests.**

Construct one canonical `gitleaks` record for each supported platform and assert that compact `serde_json::to_vec` bytes hash to the reviewed manifest digest. Mutate one field at a time and assert rejection for unknown keys, uppercase/short digests, unsorted records, duplicate artifact/platform/version keys, two active records for one platform, `latest`/`nightly` tags, arbitrary release URLs, relative paths, empty pack contents, a manifest over 1 MiB, over 256 records, or a revocation index over 16,384 entries/8 MiB. Assert that a source lock records four exact fixed GitHub release URLs but the installer-facing manifest never exposes upstream URLs as a download source.

```rust
#[test]
fn manifest_round_trip_and_canonical_digest_are_stable() {
    let manifest = fixture_manifest();
    manifest.validate().unwrap();
    let bytes = serde_json::to_vec(&manifest).unwrap();
    assert_eq!(sha256_bytes(&bytes), FIXTURE_MANIFEST_SHA256);
    assert_eq!(serde_json::from_slice::<ArtifactManifest>(&bytes).unwrap(), manifest);
}

#[test]
fn manifest_rejects_untrusted_selection_and_budget_overflow() {
    let mut manifest = fixture_manifest();
    manifest.packs[0].project_release_tag = "latest".into();
    assert_eq!(manifest.validate().unwrap_err().code, "release-tag-policy");
    let mut duplicate = fixture_manifest();
    duplicate.packs.push(duplicate.packs[0].clone());
    assert_eq!(duplicate.validate().unwrap_err().code, "duplicate-pack-key");
}
```

- [ ] **Step 2: Run the focused test to verify the missing contracts.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test artifact_contracts`. Expected: compilation fails because `ArtifactManifest`, `ArtifactPackRecord`, and `ArtifactManifest::validate` do not exist.

- [ ] **Step 3: Implement strict Rust values and semantic limits.**

Define `ArtifactManifest { schema_version: u8, kind: String, release_repository: String, revocation_index_sha256: String, packs: Vec<ArtifactPackRecord> }`, `ArtifactPackRecord` with the fields listed in the approved design, `SourceLock`, `RevocationIndex`, `PackManifest`, `ArtifactReceipt`, and `ArtifactReport`. Put `#[serde(deny_unknown_fields)]` on every object. Use exact enums for role, state, pack format, probe ids, and evidence scope. Expose:

```rust
pub fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, ArtifactError>;
pub fn sha256_bytes(bytes: &[u8]) -> String;
impl ArtifactManifest {
    pub fn validate(&self) -> Result<(), ArtifactError>;
    pub fn select_active(&self, artifact_id: &str, platform_id: &str)
        -> Result<&ArtifactPackRecord, ArtifactError>;
}
```

Validation must sort-check records, enforce lower-case SHA256, absolute bounded paths where applicable, the fixed project repository, pack size ceilings, source-lock digest binding, compact canonical JSON with no trailing newline, and the 256-record/1 MiB limits. A revoked record must include a bounded reason and optional replacement version; an active record must not.

- [ ] **Step 4: Add strict Draft 2020-12 schemas and seed fixtures.**

Set `additionalProperties: false` at every object level; require schema version/kind, exact enums, lower-case `^[0-9a-f]{64}$` digests, sorted-key array constraints represented by semantic Rust checks, and the documented array/byte maxima. The core-pack schema must distinguish immutable core inventory from post-install target receipts and include the source-lock and internal pack-manifest digests required by the manifest binding.

- [ ] **Step 5: Run contract, schema, and formatting gates.**

Run `rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/Cargo.toml -- --check`, `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test artifact_contracts`, `rtk python3 scripts/validate_schemas.py`, and `rtk git diff --check`. Expected: all commands exit 0.

- [ ] **Step 6: Commit the contract boundary.**

Run `rtk git add collect-diff-context-cli/src/lib.rs collect-diff-context-cli/src/artifacts/contract.rs collect-diff-context-cli/tests/artifact_contracts.rs collect-diff-context-cli/schemas third_party_artifacts` followed by `rtk git commit -m "feat(artifacts): define third-party pack contracts"`. Expected: one commit containing only contracts, schemas, and canonical seed metadata.

## Task 2: Verify Normalized Packs Before Any Extraction

**Files:**

- Create: `collect-diff-context-cli/src/artifacts/pack.rs`
- Test: `collect-diff-context-cli/tests/artifact_pack.rs`
- Modify: `collect-diff-context-cli/Cargo.toml`, `Cargo.lock`
- Test fixtures: `collect-diff-context-cli/tests/fixtures/artifacts/*`

- [ ] **Step 1: Write failing archive safety tests.**

Generate fixture archives in tests for a valid normalized pack and each rejected shape: `../escape`, absolute path, symlink, hardlink, device, sparse member, duplicate path, case-fold collision, alternate data stream, unexpected file, oversized header, 129th member, compressed bytes over 512 MiB, expanded bytes over 2 GiB, internal-manifest identity mismatch, executable/license/SBOM digest mismatch, invalid CycloneDX component, and non-zero gzip metadata. Assert no destination file exists after every rejected verification.

```rust
#[test]
fn verifier_extracts_only_a_verified_normalized_pack() {
    let pack = fixture_pack(ArchiveShape::Valid);
    let verified = verify_pack(&pack, &fixture_record(), &VerifyLimits::default()).unwrap();
    assert_eq!(verified.files["bin/gitleaks"].sha256, FIXTURE_EXECUTABLE_SHA256);
}
```

- [ ] **Step 2: Run the focused test and observe missing verifier behavior.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test artifact_pack`. Expected: compilation fails because `verify_pack`, `VerifyLimits`, and `VerifiedPack` do not exist.

- [ ] **Step 3: Implement streaming outer digest and archive inspection.**

Implement `verify_pack(reader, record, limits)` so it streams into a private temporary file, rejects outer size/digest mismatch before extraction, parses POSIX ustar and gzip metadata, sorts and allowlists members, rejects links/devices/duplicates/collisions, and enforces 128 entries, 512 MiB compressed, 2 GiB expanded, per-file, path, and metadata ceilings before allocation. Use `flate2` with the pinned pure-Rust `miniz_oxide` backend at compression level 9; require gzip mtime 0, empty filename/comment, OS 255, XFL 2, and canonical ustar end blocks.

- [ ] **Step 4: Extract into same-filesystem staging and validate internal evidence.**

Extract only regular allowlisted members into a private staging directory, parse `pack-manifest.json`, verify identity against the selected outer record, recompute every payload size/digest, and inspect CycloneDX 1.5 for the expected external executable component, source URL, upstream archive hash, executable hash, license, platform, and `component-evidence` scope. Return bounded stable error codes without bodies, stderr, temporary paths, or untrusted text.

- [ ] **Step 5: Prove valid/rejected fixtures and commit.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test artifact_pack`, `rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets -- -D warnings`, and `rtk git diff --check`. Expected: all archive fixtures pass or reject at the named code and Clippy is clean. Then run `rtk git add collect-diff-context-cli/src/artifacts/pack.rs collect-diff-context-cli/tests/artifact_pack.rs collect-diff-context-cli/tests/fixtures/artifacts collect-diff-context-cli/Cargo.toml Cargo.lock` and `rtk git commit -m "feat(artifacts): verify normalized packs safely"`.

## Task 3: Add Bounded Transport, Cache, Receipts, And Target Copying

**Files:**

- Create: `collect-diff-context-cli/src/artifacts/transport.rs`
- Create: `collect-diff-context-cli/src/artifacts/cache.rs`
- Test: `collect-diff-context-cli/tests/artifact_cache.rs`
- Modify: `collect-diff-context-cli/src/impact_context/cache/file_facts.rs` only to expose the existing safe platform cache-root helper.

- [ ] **Step 1: Write failing transport/cache tests.**

Use a test-only transport boundary to feed a local fixture and assert: exact digest-pinned local bytes are accepted; a wrong digest, wrong size, protocol downgrade, redirect beyond the bounded chain, timeout, or byte budget is rejected; the response body never appears in the error. Race two writers for the same digest and assert one atomic cache entry; corrupt or incomplete existing entries must fail rather than repair in place. Copy a verified cache entry into a target, delete the cache, and assert the target remains usable. Reject cache overrides inside a candidate repository, Git common directory, snapshot root, or target.

```rust
#[test]
fn target_copy_has_no_cache_path_dependency() {
    let cache = publish_fixture_cache().unwrap();
    let target = provision_from_cache(&cache, &target_root(), &fixture_record()).unwrap();
    std::fs::remove_dir_all(cache.root()).unwrap();
    assert!(verify_target_receipt(&target, &fixture_manifest()).is_ok());
}
```

- [ ] **Step 2: Run focused cache tests and observe missing APIs.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test artifact_cache`. Expected: compilation fails because `publish_cache`, `provision_from_cache`, `verify_target_receipt`, and the test transport do not exist.

- [ ] **Step 3: Implement bounded project-release transport.**

Expose `Transport::local(path, expected_digest)` and `Transport::project_asset(record)`. The production URL is constructed only from the fixed project repository, immutable tag, and asset name. Enforce HTTPS, no downgrade, at most three GitHub asset redirects, bounded connection/read/total time, expected compressed size, and streaming SHA256. `PRE_COMMIT_REVIEW_ARTIFACT_CACHE_DIR` must be absolute and pass the existing repository/Git/target containment checks; there is no repository-relative fallback and no base-URL override.

- [ ] **Step 4: Implement write-once cache and target receipts.**

Use `third-party-artifacts/sha256/<pack-digest>/` under the validated platform cache root. Stage extracted files and a pack-intrinsic receipt with private permissions, then atomic-rename once. Reopen and revalidate every cache use; mismatches return `corrupt-cache` and never mutate the old entry. Provision copies regular files into a target staging tree, never links to cache, rehashes the copy, writes a receipt without cache/temporary paths, and records observed lifecycle state, pack, executable, SBOM, license, and internal manifest digests.

- [ ] **Step 5: Run race, corruption, offline, and relocation tests and commit.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test artifact_cache`, `rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/Cargo.toml -- --check`, and `rtk git diff --check`. Expected: cache publication is atomic, target copies survive cache deletion, and all invalid cache/override cases return stable codes. Then run `rtk git add collect-diff-context-cli/src/artifacts/transport.rs collect-diff-context-cli/src/artifacts/cache.rs collect-diff-context-cli/tests/artifact_cache.rs collect-diff-context-cli/src/impact_context/cache/file_facts.rs` and `rtk git commit -m "feat(artifacts): add bounded cache and target receipts"`.

## Task 4: Expose `artifacts verify|provision|doctor`

**Files:**

- Create: `collect-diff-context-cli/src/artifacts/cli.rs`
- Modify: `collect-diff-context-cli/src/app.rs`, `collect-diff-context-cli/src/artifacts/mod.rs`
- Test: `collect-diff-context-cli/tests/artifact_cli.rs`
- Create: `scripts/check_artifacts.sh`

- [ ] **Step 1: Write failing CLI contract tests.**

Invoke the binary with `artifacts verify --manifest /abs/manifest.json --artifact-id gitleaks --platform-id darwin-arm64`, `artifacts provision --target-root /abs/target --pack /abs/pack`, and `artifacts doctor --target-root /abs/target`. Assert one bounded JSON document on stdout, no progress on stdout, absolute path rejection, missing required flags, unknown artifact/platform rejection, invalid `PRE_COMMIT_REVIEW_FETCH_PROGRESS` rejection before transport, and doctor read-only behavior. Assert doctor detects changed executable, moved target absolute provider paths, active/revoked mismatch, missing receipt, and corrupt compact revocation index without downloading.

- [ ] **Step 2: Run the tests and observe missing dispatch.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test artifact_cli`. Expected: the binary rejects `artifacts` as an unknown ordinary collector argument.

- [ ] **Step 3: Implement the explicit subcommand parser and JSON reports.**

Dispatch `artifacts` before the existing collector parser; do not alter ordinary argument semantics. Require absolute manifest/target paths, named artifact/platform, and either a local pack whose digest is already selected or the fixed project release asset. Return `{ "schema_version": 1, "kind": "third_party_artifact_report", ... }` using compact canonical JSON and stable bounded codes. `doctor` requires `--target-root`, reopens target-local manifest/core inventory/pack manifests/receipts/profiles/registry/revocations, rehashes files, checks lifecycle state, and never fetches or rewrites.

- [ ] **Step 4: Add the installed wrapper and help smoke.**

Implement `scripts/check_artifacts.sh` as a strict shell wrapper that resolves its own directory, requires one absolute target root, and executes the target-owned collector binary with `artifacts doctor --target-root "$target"`. It must not discover PATH tools or infer the current directory.

- [ ] **Step 5: Run CLI and shell gates and commit.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test artifact_cli`, `rtk bash -n scripts/check_artifacts.sh`, `rtk shellcheck scripts/check_artifacts.sh`, `rtk python3 scripts/validate_schemas.py`, and `rtk git diff --check`. Expected: the three subcommands emit bounded JSON and doctor performs only read-only checks. Then run `rtk git add collect-diff-context-cli/src/app.rs collect-diff-context-cli/src/artifacts/mod.rs collect-diff-context-cli/src/artifacts/cli.rs collect-diff-context-cli/tests/artifact_cli.rs scripts/check_artifacts.sh` and `rtk git commit -m "feat(artifacts): expose verify provision and doctor"`.

## Task 5: Migrate Gitleaks Without Changing Its Contract

**Files:**

- Modify: `collect-diff-context-cli/src/secret_scan.rs`
- Modify: `install.sh`, `scripts/fetch_gitleaks.sh`, `scripts/check_gitleaks.sh`, `scripts/lib/gitleaks_integrity.sh`
- Test: `tests/gitleaks_distribution_test.sh`, `tests/install_gitleaks_test.sh`, `tests/secret_gate_test.sh`
- Modify: `README.md`

- [ ] **Step 1: Freeze compatibility tests before changing resolution.**

Add/retain shell tests for optional default provisioning, `--no-download`, explicit absolute `PRE_COMMIT_REVIEW_GITLEAKS_BIN`, invalid/non-absolute overrides, `PRE_COMMIT_REVIEW_GITLEAKS_CONFIG`, `PRE_COMMIT_REVIEW_FETCH_PROGRESS=auto|always|never` plus invalid values, no PATH fallback, stable version/capability probe, fail-open review output, and existing doctor meanings. The tests must assert progress is stderr-only and JSON stdout remains parseable.

- [ ] **Step 2: Run the baseline compatibility tests.**

Run `rtk bash tests/gitleaks_distribution_test.sh`, `rtk bash tests/install_gitleaks_test.sh`, and `rtk bash tests/secret_gate_test.sh`. Expected: the pre-migration suite passes.

- [ ] **Step 3: Route discovery and fetch through the manager.**

Change `Scanner::discover` to check the explicit absolute override, then the target-owned canonical path, and otherwise report unavailable; never call `which` or search PATH. Make `fetch_gitleaks.sh` call `collect-diff-context artifacts provision` using the active manifest and preserve `--no-download`. Preserve the exact progress parser: `auto` follows interactive stderr detection, `always` emits bounded stderr progress, `never` suppresses it, and an invalid value fails before any network request. Keep scanner errors as optional downgrade so review remains allowed.

- [ ] **Step 4: Preserve config and doctor semantics.**

Bind the project default config digest from the active record/core inventory; keep `PRE_COMMIT_REVIEW_GITLEAKS_CONFIG` under explicit-user-trust path rules and report that scope. Keep `install.sh --doctor` as the source/core Gitleaks diagnostic and route `--doctor-target /absolute/managed-skill` to the generic artifact doctor. Do not add output/finding budget changes in this migration.

- [ ] **Step 5: Run compatibility, no-PATH, and failure-mode tests and commit.**

Run the three focused shell suites again plus `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked`, `rtk bash -n install.sh scripts/fetch_gitleaks.sh scripts/check_gitleaks.sh`, and `rtk git diff --check`. Expected: all existing meanings remain unchanged and no PATH/upstream fallback is reachable. Then run `rtk git add collect-diff-context-cli/src/secret_scan.rs install.sh scripts/fetch_gitleaks.sh scripts/check_gitleaks.sh scripts/lib/gitleaks_integrity.sh tests/gitleaks_distribution_test.sh tests/install_gitleaks_test.sh tests/secret_gate_test.sh README.md` and `rtk git commit -m "feat(gitleaks): use the artifact manager"`.

## Task 6: Build Four Core Packs And Four Gitleaks Packs

**Files:**

- Create: `scripts/build_artifact_pack.sh`
- Create: `third_party_artifacts/packs/.gitkeep` (directory marker only; generated archives are release outputs)
- Modify: `scripts/build_all_binaries.sh`, `install.sh`, `.github/workflows/release.yml`
- Test: `tests/artifact_distribution_test.sh`

- [ ] **Step 1: Write pack-content and platform-matrix tests.**

Given fixture binaries and the project payload, assert each core archive contains only its platform collector binary, skill payload, installer, schemas, documentation, licenses, immutable core inventory, and core SBOM. Assert each Gitleaks pack contains exactly `pack-manifest.json`, `bin/<name>`, `licenses/*`, and `sbom.cdx.json`; Windows uses `.exe`; no archive contains another platform binary, a symlink, a cache path, an upstream URL override, or a generated target receipt.

- [ ] **Step 2: Run the tests and observe absent pack builder output.**

Run `rtk bash tests/artifact_distribution_test.sh`. Expected: the test fails because the four platform core and Gitleaks assets are not yet produced.

- [ ] **Step 3: Implement the normalized pack builder and core inventory separation.**

Make `scripts/build_artifact_pack.sh` accept only a checked-in manifest/source lock, platform id, pack version, and output path. Use the Rust pack writer with fixed sorted POSIX ustar metadata, gzip mtime 0, empty filename/comment, OS 255, XFL 2, level-9 `miniz_oxide`; emit compact JSON with no newline. Generate a core inventory that is complete before any provider/Gitleaks provisioning and a target receipt that is generated later; bind every core inventory member, source-lock digest, active pack outer digest, and internal pack-manifest digest in the manifest update.

- [ ] **Step 4: Add SBOM and license evidence.**

Generate CycloneDX 1.5 for each Gitleaks external executable as a top-level component with tool version, supplier/source URL, upstream archive hash, executable hash, license, pack id/version, platform, and `contains` relationship. Record component-level evidence and unknown transitive closure; do not label Cargo-only SBOM coverage as binary coverage. Copy exact license files and bind their digests in the internal manifest.

- [ ] **Step 5: Run pack reproducibility and platform tests and commit.**

Run `rtk bash tests/artifact_distribution_test.sh`, `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test artifact_pack`, and `rtk git diff --check`. Expected: rebuilding a pack from identical inputs produces identical bytes and every platform archive passes content inspection. Then run `rtk git add scripts/build_artifact_pack.sh scripts/build_all_binaries.sh install.sh .github/workflows/release.yml tests/artifact_distribution_test.sh` and `rtk git commit -m "build(release): publish platform core and Gitleaks packs"`.

## Task 7: Make Installation Transactional And Target-Aware

**Files:**

- Modify: `install.sh`
- Modify: `scripts/check_artifacts.sh`, `tests/install_smoke_test.sh`, `tests/install_agent_matrix_test.sh`
- Test: `tests/artifact_distribution_test.sh`

- [ ] **Step 1: Write installer transaction tests.**

Test copy-mode installation with an existing target and assert a Gitleaks digest/probe failure leaves the old target unchanged while a required artifact failure leaves it unchanged and returns non-zero. Test `--no-download` with a verified cache hit and cache miss, absolute target doctor after relocation, and `--link --with-rust-analyzer` preflight rejection (the latter remains a Delivery 5B flag but the shared parser must reject before mutation).

- [ ] **Step 2: Implement staged provisioning and commit point.**

Stage the core payload beside the final target, verify the external core sidecar digest and scoped project attestation before extraction, provision optional Gitleaks through the manager, generate receipts, revalidate the entire staging tree, and only then replace the existing target. A Gitleaks failure logs the existing downgrade and commits the valid core; a required artifact failure aborts before target replacement. Never symlink or hardlink installed files into the cache.

- [ ] **Step 3: Add target-aware doctor entry points.**

Parse `--doctor-target /absolute/managed-skill` in `install.sh`, pass the explicit target to `scripts/check_artifacts.sh`, and leave `--doctor`'s source/core Gitleaks behavior unchanged. Doctor reports stale absolute paths after a target move but does not rewrite them, fetch, repair, or choose a replacement.

- [ ] **Step 4: Run installer and relocation tests and commit.**

Run `rtk bash tests/install_smoke_test.sh`, `rtk bash tests/install_agent_matrix_test.sh`, `rtk bash tests/artifact_distribution_test.sh`, `rtk bash -n install.sh scripts/check_artifacts.sh`, and `rtk git diff --check`. Expected: all transaction, downgrade, no-download, and target-aware doctor assertions pass. Then run `rtk git add install.sh scripts/check_artifacts.sh tests/install_smoke_test.sh tests/install_agent_matrix_test.sh tests/artifact_distribution_test.sh` and `rtk git commit -m "feat(install): provision verified artifacts transactionally"`.

## Task 8: Add Release Trust, Attestations, And Revocation Gates

**Files:**

- Create: `.github/workflows/artifact-pack-release.yml`
- Modify: `.github/workflows/release.yml`, `.github/workflows/lint.yml`
- Create: `scripts/verify_release_artifacts.sh`
- Modify: `tests/artifact_distribution_test.sh`, `docs/helper-capabilities.md`, `README.md`

- [ ] **Step 1: Write build-only trust-gate fixtures.**

Test that verification rejects a core or third-party pack when the sidecar digest differs, the subject digest differs, the signer repository/workflow/ref/commit/issuer is wrong, the predicate type is not the expected artifact-pack type, a composition predicate omits any upstream archive/source-lock/manifest/SBOM/generator digest, or an immutable release check is unavailable while documentation claims immutability.

- [ ] **Step 2: Implement pinned release workflow and independent verifier.**

Pin critical Actions to reviewed commit SHAs; install Rust `1.95.0`; use committed lockfiles and `--locked`; build packs, SBOMs, sidecar checksums, and attestations. Verify core archives with an external sidecar/attestation before extraction; package-internal inventory is only a post-trust integrity check. Validate subject name/digest, predicate type, repository, workflow, immutable source ref/commit, OIDC/Sigstore issuer, and every composition input digest. Use a protected reusable pack-builder workflow so caller-controlled predicate text cannot establish composition claims.

- [ ] **Step 3: Implement bounded revocation lifecycle.**

Keep one active record per artifact/platform and a bounded recent revoked window in the main manifest. Append older revoked digests to sorted target-local `runtime/distribution/revocations.json`, pin its digest in the manifest, reject receipts found in either location, enforce 16,384-entry/8 MiB ceilings, and document that old offline core installations cannot learn later revocations.

- [ ] **Step 4: Run trust and revocation gates and commit.**

Run `rtk bash scripts/verify_release_artifacts.sh --fixture tests/fixtures/release`, `rtk bash tests/artifact_distribution_test.sh`, `rtk python3 scripts/validate_schemas.py`, and `rtk git diff --check`. Expected: scoped attestations, external core trust, immutable-release gating, and revocation behavior all pass. Then run `rtk git add .github/workflows/artifact-pack-release.yml .github/workflows/release.yml .github/workflows/lint.yml scripts/verify_release_artifacts.sh tests/artifact_distribution_test.sh docs/helper-capabilities.md README.md` and `rtk git commit -m "ci(release): verify artifact trust and revocations"`.

## Task 9: Complete Rust, Shell, Schema, And Delivery Gates

**Files:**

- Modify: `.github/workflows/lint.yml`, `.github/workflows/release.yml`, `scripts/validate_schemas.py`, `collect-diff-context-cli/fuzz/README.md`, `docs/gitleaks-distribution-strategy-research.md`
- Test: all existing Gitleaks/install/secret tests and new artifact tests

- [ ] **Step 1: Add reachability and negative-path assertions.**

Assert ordinary collector, Fast Mode, repository index, SQLite, and static-analysis commands never invoke `artifacts`, download a pack, or execute a third-party binary. Assert no PATH, rustup, package-manager, direct-upstream, `latest`, or `nightly` fallback exists in production scripts or Rust code.

- [ ] **Step 2: Add exact CI matrix and final evidence.**

Require Rust 1.95.0 locked format/test/Clippy, ShellCheck, schema validation, pack fixture tests for all unsafe archive shapes, Gitleaks compatibility tests, platform content tests, build-only SBOM/attestation checks, and release evidence containing toolchain and lockfile digests. Keep generated pack archives and hash-named fuzz corpus files out of source control.

- [ ] **Step 3: Run the complete Delivery 5A gate.**

Run:

```bash
rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/Cargo.toml -- --check
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets
rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets -- -D warnings
rtk python3 scripts/validate_schemas.py
rtk bash tests/gitleaks_distribution_test.sh
rtk bash tests/install_gitleaks_test.sh
rtk bash tests/secret_gate_test.sh
rtk bash tests/artifact_distribution_test.sh
rtk bash -n install.sh scripts/*.sh scripts/lib/*.sh
rtk shellcheck install.sh scripts/*.sh scripts/lib/*.sh
rtk git diff --check
```

Expected: every command exits 0; no rust-analyzer binary or provider install path exists; target copies remain valid after cache removal.

- [ ] **Step 4: Commit the Delivery 5A completion evidence.**

Run `rtk git add .github/workflows/lint.yml .github/workflows/release.yml scripts/validate_schemas.py collect-diff-context-cli/fuzz/README.md docs/gitleaks-distribution-strategy-research.md` and `rtk git commit -m "test(artifacts): close Gitleaks distribution gates"`. Expected: the branch contains a reviewable 5A commit series and no generated release outputs.

## Self-Review Checklist

- [ ] Every manifest, pack, source-lock, receipt, report, baseline, and revocation field has a strict Rust type and schema task.
- [ ] External core trust is verified before extraction; package-internal inventory is never treated as a root of trust.
- [ ] The internal pack-manifest digest is retained in core/receipt bindings so installed payload integrity can be rechecked.
- [ ] Cache is described as write-once/content-addressed, revalidated on every use, and never referenced by installed profiles.
- [ ] Gitleaks progress, override, config, `--no-download`, no-PATH, doctor, and fail-open semantics are tested unchanged.
- [ ] Canonical JSON, tar, gzip level/backend/metadata, SBOM evidence scope, source-lock digest, and release signer/workflow/ref policy are explicit.
- [ ] Every task gives a concrete interface, test, command, expected result, and commit; no task defers an implementation detail to an unnamed step.
