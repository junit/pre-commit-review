# Rust-Analyzer Provider Pack And Release Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish and provision the pinned rust-analyzer `2026-07-27` project packs, generate Delivery 4 authorization inputs transactionally, and add real-server, resource, performance, fuzz, and release evidence on all four supported platforms.

**Architecture:** Delivery 5B consumes the generic `artifacts` manager and strict pack contracts delivered by 5A. A CI-only source lock drives a normalized project repackaging workflow; only already-published, independently attested packs can enter a reviewed core manifest. `install.sh --with-rust-analyzer` provisions the current platform into a staged target and generates typed Delivery 4 profile/registry bytes using final absolute paths. The existing provider runner, fake server, managed runtime, handshake gate, and deterministic BFS remain authoritative and are extended only for process-tree RSS and real-server evidence.

**Tech Stack:** Rust 1.95.0 with committed lockfiles and `--locked`, existing `serde`/`serde_json`/`sha2` provider contracts, the 5A normalized pack manager and CycloneDX/attestation tooling, Bash, Python evidence generators, cargo-fuzz nightly only for fuzz jobs, GitHub Actions pinned to reviewed commit SHAs, and repository-owned fixture projects that never run Cargo or fetch dependencies.

---

## Execution Boundary And File Map

Execute after Delivery 5A is accepted, from `feature/provider-artifact-distribution`; do not modify `feature/SAST` directly. Do not add provider discovery or invocation to ordinary review, Fast Mode, repository indexing, SQLite persistence, or static-analysis orchestration. `--with-rust-analyzer` is explicit copy-mode installation only; `--link --with-rust-analyzer` is rejected before any mutation.

Create:

- `third_party_artifacts/sources/rust-analyzer-2026-07-27.json`: strict `third_party_sources/v1` source lock.
- `third_party_artifacts/baselines/rust-analyzer-2026.07.27-pcr.1.json`: reviewed canonical latency baseline.
- `collect-diff-context-cli/src/artifacts/provider.rs`: provider-pack selection, generated profile/registry values, and manifest-update data.
- `collect-diff-context-cli/src/provider_resources.rs`: platform process-tree RSS accounting and sampled threshold state.
- `collect-diff-context-cli/schemas/third-party-source-lock.schema.json` and `third-party-artifact-baseline.schema.json` if not already created by 5A.
- `collect-diff-context-cli/tests/artifact_provider_pack.rs`
- `collect-diff-context-cli/tests/provider_install.rs`
- `collect-diff-context-cli/tests/repository_context_resources.rs`
- `collect-diff-context-cli/tests/repository_context_provider_real.rs`
- `collect-diff-context-cli/tests/provider_baseline.rs`
- `collect-diff-context-cli/tests/fixtures/repository_context_provider/real/{single_crate,multi_crate,partial,unicode_crlf,cycles}`
- `scripts/generate_provider_manifest_update.py`
- `scripts/measure_provider_baseline.py`
- `scripts/verify_provider_release.sh`
- `tests/install_rust_analyzer_test.sh`
- `tests/provider_real_server_test.sh`
- `.github/workflows/artifact-pack-release.yml`
- `.github/workflows/provider-real-server.yml`
- `.github/workflows/provider-fuzz-scheduled.yml`

Modify:

- `collect-diff-context-cli/src/artifacts/contract.rs`, `src/artifacts/pack.rs`, and `src/artifacts/mod.rs`: provider role/source-lock/baseline fields and provider pack APIs from 5A.
- `collect-diff-context-cli/src/artifacts/cli.rs`: provider selection and transaction-facing report values without adding runtime fallback.
- `install.sh`: explicit `--with-rust-analyzer`, `--no-download` provider behavior, link preflight, staged generated files, and target-aware doctor delegation.
- `collect-diff-context-cli/src/repository_context_provider/contract.rs` and `cli_contract.rs`: expose typed constructors only; do not change Delivery 4 JSON fields or hardening maxima.
- `collect-diff-context-cli/src/trusted_runtime.rs`, `src/process_group.rs`, `src/repository_context_provider/session.rs`, `src/repository_context_provider/mod.rs`, and report schemas: RSS monitor lifecycle and bounded evidence.
- `collect-diff-context-cli/src/bin/repository_context_provider_fixture.rs` and existing provider/session tests: resource and real-server test scenarios while preserving fake-server adversarial coverage.
- `.github/workflows/lint.yml`, `.github/workflows/release.yml`, `collect-diff-context-cli/fuzz/README.md`, `collect-diff-context-cli/fuzz/fuzz_targets/repository_context_frame.rs`, and `repository_context_messages.rs`: exact fuzz tiers and Rust 1.95 locked release gates.
- `docs/rust-analyzer-context-provider.md`, `docs/helper-capabilities.md`, `README.md`, and release evidence docs.

## Task 1: Lock rust-analyzer Inputs And Provider Pack Records

**Files:**

- Create: `third_party_artifacts/sources/rust-analyzer-2026-07-27.json`
- Create or modify: `collect-diff-context-cli/schemas/third-party-source-lock.schema.json`, `third-party-artifact-baseline.schema.json`
- Modify: `collect-diff-context-cli/src/artifacts/contract.rs`
- Test: `collect-diff-context-cli/tests/artifact_provider_pack.rs`

- [ ] **Step 1: Write failing source-lock and selection tests.**

Assert exact tag `2026-07-27`, the four platform/target pairs, one fixed upstream GitHub URL per platform, archive and executable names/sizes/digests, expected version probe, license paths, and a reviewed compact source-lock digest. Reject `latest`, `nightly`, arbitrary hosts, query/template URLs, changed target triples, duplicate platform records, missing executable hashes, and any source-lock field the installer could interpret as a command or environment setting.

```rust
#[test]
fn rust_analyzer_source_lock_is_exact_and_canonical() {
    let lock = load_source_lock("third_party_artifacts/sources/rust-analyzer-2026-07-27.json");
    lock.validate().unwrap();
    assert_eq!(lock.tool_version, "2026-07-27");
    assert_eq!(lock.assets.len(), 4);
    assert_eq!(sha256_bytes(&canonical_json(&lock).unwrap()), REVIEWED_SOURCE_LOCK_SHA256);
}
```

- [ ] **Step 2: Run the focused test and observe absent provider records.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test artifact_provider_pack`. Expected: compilation or fixture loading fails because the provider source-lock type and four records are absent.

- [ ] **Step 3: Implement the strict source-lock and provider fields.**

Use `SourceLock { schema_version: 1, kind: "third_party_sources", artifact_id: "rust-analyzer", tool_version, upstream_repository, upstream_tag, upstream_commit, assets }` and an asset record containing only fixed URL, archive/executable names, sizes, SHA256s, version probe, and license source paths. Add `source_lock_sha256`, `quality_baseline_sha256`, `default_configuration_sha256` (sanitizer only), internal pack-manifest digest, and SBOM digest to `ArtifactPackRecord`. Validate canonical bytes with compact `serde_json::to_vec`, enforce the fixed GitHub release path, and keep the source lock CI-only; the installer consumes project pack records only.

- [ ] **Step 4: Add schema and canonical fixture gates.**

Set `additionalProperties: false` recursively, require exact enum/kind/version values, lower-case digests, four assets, bounded URLs, and no shell/command/environment fields. Add the active `rust-analyzer` records only after provider packs exist; use independent pack version `2026.07.27-pcr.1` and never equate it implicitly with the upstream tag.

- [ ] **Step 5: Run and commit the lock boundary.**

Run `rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/Cargo.toml -- --check`, `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test artifact_provider_pack`, `rtk python3 scripts/validate_schemas.py`, and `rtk git diff --check`. Expected: the lock digest and platform matrix are stable. Commit with `rtk git add third_party_artifacts/sources/rust-analyzer-2026-07-27.json collect-diff-context-cli/src/artifacts/contract.rs collect-diff-context-cli/schemas/third-party-source-lock.schema.json collect-diff-context-cli/schemas/third-party-artifact-baseline.schema.json collect-diff-context-cli/tests/artifact_provider_pack.rs` followed by `rtk git commit -m "feat(provider): lock rust-analyzer release inputs"`.

## Task 2: Build Provider Packs, SBOMs, And Composition Evidence

**Files:**

- Modify: `collect-diff-context-cli/src/artifacts/pack.rs`, `src/artifacts/provider.rs`
- Create: `.github/workflows/artifact-pack-release.yml`, `scripts/verify_provider_release.sh`
- Test: `collect-diff-context-cli/tests/artifact_provider_pack.rs`

- [ ] **Step 1: Write failing normalized-pack and SBOM tests.**

Use four fixed archive fixtures and assert that rebuilding unchanged inputs produces byte-identical normalized packs with sorted POSIX ustar members, gzip mtime 0, empty filename/comment, OS 255, XFL 2, level-9 pure-Rust compression, compact JSON without a trailing newline, and exactly `pack-manifest.json`, `bin/*`, `licenses/*`, `sbom.cdx.json`. Assert the SBOM has a top-level external executable component, upstream archive/executable hashes, source URL, license, platform, pack id/version, `contains` relationship, and component-level evidence when transitive closure is unknown.

```rust
#[test]
fn provider_pack_reproduction_and_sbom_are_byte_stable() {
    let first = build_provider_pack(&fixture_source_lock(), PlatformId::LinuxAmd64).unwrap();
    let second = build_provider_pack(&fixture_source_lock(), PlatformId::LinuxAmd64).unwrap();
    assert_eq!(sha256_bytes(&first.archive), sha256_bytes(&second.archive));
    verify_cyclonedx_external_component(&first.sbom, "rust-analyzer").unwrap();
}
```

- [ ] **Step 2: Run the test and observe the missing provider builder.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test artifact_provider_pack`. Expected: compilation fails for provider pack construction or the SBOM composition verifier.

- [ ] **Step 3: Implement source-lock-driven pack generation.**

The builder accepts only the reviewed lock path, platform id, pack version, output path, and pinned generator configuration. It downloads exactly the four fixed assets in CI, verifies archive and extracted executable hashes/version/license paths, and delegates archive normalization to the 5A pack writer. It never compiles rust-analyzer and never permits direct-upstream or fallback bytes in the installer.

- [ ] **Step 4: Emit composition predicate materials and attestations.**

Generate a project-specific `pre-commit-review.artifact-pack/v1` predicate whose input materials include source-lock digest, every upstream archive digest, pack-builder source commit, normalized pack-manifest digest, SBOM digest, and generator configuration digest. The workflow must also attest the pack, internal manifest, and SBOM. `scripts/verify_provider_release.sh` rejects a subject-only attestation and checks exact subject name/digest, predicate type, signer repository/workflow, source ref/commit, OIDC/Sigstore issuer, and every listed input digest.

- [ ] **Step 5: Run clean verification and commit the builder workflow.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test artifact_provider_pack`, `rtk bash scripts/verify_provider_release.sh --fixture tests/fixtures/provider-release`, and `rtk git diff --check`. Expected: identical bytes, complete composition material, and scoped signer verification pass; a changed archive or omitted material fails. Then run `rtk git add collect-diff-context-cli/src/artifacts/pack.rs collect-diff-context-cli/src/artifacts/provider.rs .github/workflows/artifact-pack-release.yml scripts/verify_provider_release.sh collect-diff-context-cli/tests/artifact_provider_pack.rs` and `rtk git commit -m "build(provider): publish attested rust-analyzer packs"`.

## Task 3: Generate A Reviewed Manifest Update After Publication

**Files:**

- Create: `scripts/generate_provider_manifest_update.py`
- Create: `tests/fixtures/provider-release/reviewed-baseline.json`
- Test: `collect-diff-context-cli/tests/provider_baseline.rs`

- [ ] **Step 1: Write failing sequencing and baseline tests.**

Assert the generator refuses an unpublished pack, missing attestation, missing internal-manifest/SBOM digest, source-lock mismatch, noncanonical bytes, or a synthetic baseline whose pack/executable/source-lock/profile/fixture/request/runner digests differ. Assert that the generated update contains final asset names, outer/internal/SBOM/executable/source-lock/quality-baseline digests and four platform records, and that the core release cannot rewrite it.

```rust
#[test]
fn release_threshold_uses_integer_nearest_rank_policy() {
    assert_eq!(release_threshold_ms(1001), 1502); // ceil(1001 * 5 / 4) + 250
    assert!(accept_p95(1502, 1001));
    assert!(!accept_p95(1503, 1001));
}
```

- [ ] **Step 2: Run the focused tests and observe missing generator/baseline types.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test provider_baseline`. Expected: compilation or generator fixtures fail because the strict baseline and publication-order checks are absent.

- [ ] **Step 3: Implement canonical baseline and reviewed update generation.**

The synthetic baseline fixture records pack/version, executable, source-lock, profile, fixture, request, runner-class digests, samples, nearest-rank p95, and canonical bytes. Implement `release_threshold_ms(p95) -> u64` as `p95.saturating_mul(5).div_ceil(4).saturating_add(250)` with overflow rejection. The generator reads only clean verified release metadata, emits a normal reviewable PR patch, and never mutates manifest bytes inside a core release job. The reviewed real baseline is created only after Task 8 measurements.

- [ ] **Step 4: Run baseline/generator tests and commit the reviewed metadata.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test provider_baseline`, `rtk python3 scripts/generate_provider_manifest_update.py --fixture tests/fixtures/provider-release`, `rtk python3 scripts/validate_schemas.py`, and `rtk git diff --check`. Expected: synthetic publication fixtures prove the exact update/attestation sequencing without creating a real baseline before measurement. Then run `rtk git add scripts/generate_provider_manifest_update.py tests/fixtures/provider-release/reviewed-baseline.json collect-diff-context-cli/tests/provider_baseline.rs` and `rtk git commit -m "build(provider): gate reviewed manifest updates"`.

## Task 4: Add Explicit Transactional rust-analyzer Installation

**Files:**

- Modify: `install.sh`, `scripts/check_artifacts.sh`
- Create: `tests/install_rust_analyzer_test.sh`
- Test: `collect-diff-context-cli/tests/provider_install.rs`
- Modify: `collect-diff-context-cli/src/artifacts/cli.rs`, `src/artifacts/provider.rs`

- [ ] **Step 1: Write failing installer tests.**

Cover default installation without rust-analyzer, successful `--with-rust-analyzer` current-platform-only provisioning, wrong-platform selection, missing/corrupt/revoked pack, version/probe failure, `--no-download --with-rust-analyzer` verified-cache hit/miss, `--link --with-rust-analyzer` preflight rejection, and an existing-target byte hash that must remain unchanged after every provider-specific failure. Assert no network request is made during ordinary review or provider execution.

- [ ] **Step 2: Run the installer test and observe absent flag behavior.**

Run `rtk bash tests/install_rust_analyzer_test.sh`. Expected: `install.sh` rejects `--with-rust-analyzer` as unknown or performs no provider transaction.

- [ ] **Step 3: Implement preflight and current-platform provisioning.**

Parse `--with-rust-analyzer` and reject it with `--link` before staging, fetching, cache access, or mutation. In copy mode, select exactly the host platform's active manifest record, call `collect-diff-context artifacts verify|provision`, copy verified regular files into `runtime/third-party/rust-analyzer/<pack-version>/`, and retain `pack-manifest.json`, licenses, SBOM, receipt, distribution manifest, core inventory, and revocation index. `--no-download` allows only an already verified canonical cache entry; no direct upstream or PATH fallback exists. A provider error aborts before the existing target replacement commit point and leaves the previous target byte-identical.

- [ ] **Step 4: Add target-aware doctor and relocation semantics.**

Keep `install.sh --doctor` as the existing source/core Gitleaks diagnostic. Route `install.sh --doctor-target /absolute/managed-skill` to `collect-diff-context artifacts doctor --target-root /absolute/managed-skill`; doctor rehashes provider receipts and reports stale generated paths after a move without rewriting, downloading, repairing, or selecting a replacement.

- [ ] **Step 5: Run installer and shell gates and commit.**

Run `rtk bash tests/install_rust_analyzer_test.sh`, `rtk bash tests/install_smoke_test.sh`, `rtk bash -n install.sh scripts/check_artifacts.sh`, `rtk shellcheck install.sh scripts/check_artifacts.sh`, and `rtk git diff --check`. Expected: explicit opt-in is transactional, link mode is rejected before mutation, and default installs contain no provider. Commit with `rtk git add install.sh scripts/check_artifacts.sh tests/install_rust_analyzer_test.sh collect-diff-context-cli/src/artifacts/cli.rs collect-diff-context-cli/src/artifacts/provider.rs collect-diff-context-cli/tests/provider_install.rs` followed by `rtk git commit -m "feat(install): add explicit rust-analyzer provisioning"`.

## Task 5: Generate Final-Path Delivery 4 Profile And Registry Bytes

**Files:**

- Modify: `collect-diff-context-cli/src/artifacts/provider.rs`, `src/repository_context_provider/contract.rs`, `src/repository_context_provider/cli_contract.rs`
- Test: `collect-diff-context-cli/tests/provider_install.rs`, `collect-diff-context-cli/tests/repository_context_provider_cli_contracts.rs`

- [ ] **Step 1: Write failing generated-authorization tests.**

Create a staged target and a final absolute target, then assert the generated profile uses provider kind `rust-analyzer`, exact installed version and executable SHA256, canonical configuration SHA256, target triple, `toolchain_mode: none`, fixed hardening, fixed maxima, and an empty argument list because the pinned executable uses stdio by default and rejects `--stdio`. Assert the registry id is `rust-analyzer-project-pack`, contains final absolute profile/executable paths, and binds exact profile/executable/configuration/target values. Assert raw profile and registry bytes have no trailing newline and are equal to compact `serde_json::to_vec`; moving the staging prefix without changing final paths must not change the generated bytes.

```rust
#[test]
fn generated_profile_and_registry_use_delivery_four_hashes() {
    let generated = generate_provider_authorization(&final_target(), &verified_provider()).unwrap();
    generated.profile.validate().unwrap();
    generated.registry.validate().unwrap();
    assert_eq!(sha256_bytes(&generated.profile_bytes), generated.profile.sha256());
    assert_eq!(generated.registry.entries[0].profile_sha256, generated.profile.sha256());
    assert!(!generated.profile_bytes.ends_with(b"\n"));
}
```

- [ ] **Step 2: Run tests and observe absent generation API.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test provider_install`. Expected: compilation fails because `generate_provider_authorization` and its byte-bound result do not exist.

- [ ] **Step 3: Implement typed generation and staged-to-final verification.**

Expose `generate_provider_authorization(final_target: &Path, verified: &VerifiedProvider) -> Result<GeneratedProviderAuthorization, ArtifactError>`. Resolve the final target from a canonical absolute parent; construct `AuthorizedProviderProfile` and `ProviderRegistry` with existing Delivery 4 types; serialize both with exact `serde_json::to_vec` and no newline; validate structs and digests before writing. During staging, replace the final target prefix with the staging prefix only for file existence checks; generated JSON always retains final absolute paths. Reject unresolved parents, path escape, profile/executable/configuration mismatch, altered hardening, altered maxima, unknown fields, and registry hash drift.

- [ ] **Step 4: Run all contract/binding tests and commit.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test provider_install --test repository_context_provider_cli_contracts`, `rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/Cargo.toml -- --check`, and `rtk git diff --check`. Expected: generated bytes validate unchanged Delivery 4 schemas and the registry digest is the exact value passed to the explicit provider CLI. Commit with `rtk git add collect-diff-context-cli/src/artifacts/provider.rs collect-diff-context-cli/src/repository_context_provider/contract.rs collect-diff-context-cli/src/repository_context_provider/cli_contract.rs collect-diff-context-cli/tests/provider_install.rs collect-diff-context-cli/tests/repository_context_provider_cli_contracts.rs` followed by `rtk git commit -m "feat(provider): generate bound profile and registry"`.

## Task 6: Enforce Sampled Process-Tree RSS Without Weakening Runtime Limits

**Files:**

- Create: `collect-diff-context-cli/src/provider_resources.rs`
- Modify: `collect-diff-context-cli/src/lib.rs`, `src/trusted_runtime.rs`, `src/process_group.rs`, `src/repository_context_provider/session.rs`, `src/repository_context_provider/mod.rs`, `src/repository_context_provider/contract.rs`
- Modify: `collect-diff-context-cli/src/bin/repository_context_provider_fixture.rs`
- Test: `collect-diff-context-cli/tests/repository_context_resources.rs`, `tests/repository_context_session.rs`

- [ ] **Step 1: Write failing resource tests.**

Add fake-server scenarios that spawn a descendant, exceed the test threshold, exit normally, or make accounting unavailable. Assert a sampled interval no greater than 100 ms, stable `process-tree-rss-limit` on observed exceedance, no semantic facts, full process-tree termination/reap, and hard failure when required accounting cannot be obtained. Preserve existing framing, output, deadline, cancellation, and descendant-drop tests.

```rust
#[test]
fn rss_limit_terminates_descendants_without_publishing_facts() {
    let result = run_fixture("spawn-descendant-rss", ProviderLimits::test_limits());
    assert_eq!(result.status_code(), "process-tree-rss-limit");
    assert!(result.report().edges.is_empty());
    assert!(result.descendants_reaped());
}
```

- [ ] **Step 2: Run the focused test and observe missing resource accounting.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --features test-fixture --test repository_context_resources`. Expected: compilation fails for the sampler or the fixture scenario, while existing session tests continue to compile.

- [ ] **Step 3: Implement platform accounting and lifecycle ownership.**

Add a sampler owned by the managed session/runtime that accounts for the child and descendants at intervals no greater than 100 ms. On Linux enumerate `/proc/<pid>/task` and `/proc/<pid>/children` RSS; on macOS use the platform process accounting API; on Windows query the existing Job Object/process set and per-process memory counters. Use the platform process-group/Job Object handles already owned by `ManagedChild`; where an inherited stronger job limit exists, configure it in addition to sampling. Enforce `2 * 1024 * 1024 * 1024` bytes in production and inject a smaller threshold only through a test-only constructor. Map sampler failure into `SessionError` and the existing `status_for_session_error` path as `process-tree-rss-limit` before report facts are built. Terminate/reap through the Drop-safe path on exceedance. A missing sampler capability is a gate error, not a warning. Report only bounded peak bytes, sample interval, and accounting status; never expose process roots or raw child output.

- [ ] **Step 4: Run Unix/Windows compile and lifecycle tests and commit.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --features test-fixture --test repository_context_resources --test repository_context_session`, `rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets -- -D warnings`, and `rtk git diff --check`. Expected: over-limit and unavailable-accounting cases fail closed, descendants are reaped, and all prior session tests pass. Commit with `rtk git add collect-diff-context-cli/src/provider_resources.rs collect-diff-context-cli/src/lib.rs collect-diff-context-cli/src/trusted_runtime.rs collect-diff-context-cli/src/process_group.rs collect-diff-context-cli/src/repository_context_provider/session.rs collect-diff-context-cli/src/repository_context_provider/mod.rs collect-diff-context-cli/src/repository_context_provider/contract.rs collect-diff-context-cli/src/bin/repository_context_provider_fixture.rs collect-diff-context-cli/tests/repository_context_resources.rs collect-diff-context-cli/tests/repository_context_session.rs` followed by `rtk git commit -m "feat(provider): enforce sampled process-tree memory"`.

## Task 6A: Enable The Exact Provider Release Tag Trigger

**Files:**

- Modify: `.github/workflows/artifact-pack-release.yml`
- Test: `collect-diff-context-cli/tests/artifact_provider_pack.rs`
- Test: `tests/artifact_distribution_test.sh`

- [ ] **Step 1: Write failing exact-tag workflow tests.**

Assert that the workflow declares only
`artifact-rust-analyzer-2026.07.27-pcr.1` under `push.tags`, that this exact ref
selects every rust-analyzer build/verify/publish job when workflow inputs are
absent, and that Gitleaks jobs remain disabled. Reject wildcard provider tags,
moving aliases, branch pushes, or a tag-derived arbitrary artifact selector.
Keep the existing `workflow_call` and `workflow_dispatch` paths unchanged.

- [ ] **Step 2: Run focused tests and observe the absent tag entrypoint.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml
--locked --test artifact_provider_pack
provider_release_workflow_accepts_only_the_exact_rust_analyzer_tag` and
`rtk bash tests/artifact_distribution_test.sh`. Expected: the new assertion
fails because the workflow has no `push.tags` entry and rust-analyzer jobs
depend only on `inputs.artifact`.

- [ ] **Step 3: Implement exact tag selection.**

Add the one literal tag under `on.push.tags`. For rust-analyzer build, clean
verification, and publication job conditions, accept either the existing
explicit input or the exact full ref
`refs/tags/artifact-rust-analyzer-2026.07.27-pcr.1`. Do not parse the tag into
an artifact name, do not add a wildcard, and continue to use
`inputs.release_tag || github.ref_name` only for the already-gated release
name. Gitleaks conditions remain input-only.

- [ ] **Step 4: Verify and commit the trigger.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml
--locked --test artifact_provider_pack`, `rtk bash
tests/artifact_distribution_test.sh`, `rtk python3
scripts/validate_schemas.py`, and `rtk git diff --check`. Expected: exact-tag,
workflow trust, schema, and existing artifact distribution gates pass. Commit
with `rtk git add .github/workflows/artifact-pack-release.yml
collect-diff-context-cli/tests/artifact_provider_pack.rs
tests/artifact_distribution_test.sh` followed by `rtk git commit -m
"ci(provider): allow exact pack release tag"`.

## Task 6B: Correct The Linux Provider Asset And Retry Immutably

**Files:**

- Modify: `third_party_artifacts/sources/rust-analyzer-2026-07-27.json`
- Modify: `.github/workflows/artifact-pack-release.yml`
- Modify: `install.sh`
- Modify: provider artifact contracts, schemas, release scripts, and active
  provider-release fixtures that bind the pack version or Linux source asset
- Test: `collect-diff-context-cli/tests/artifact_provider_pack.rs`
- Test: `collect-diff-context-cli/tests/artifact_contracts.rs`
- Test: `tests/artifact_distribution_test.sh`
- Test: `tests/install_rust_analyzer_test.sh`
- Test: `tests/provider_release_verifier_test.sh`

- [ ] **Step 1: Preserve the failed `pcr.1` bootstrap as immutable history.**

Assert that the corrected workflow accepts only
`artifact-rust-analyzer-2026.07.27-pcr.2`, never accepts `pcr.1`, and contains
no wildcard, moving tag, or branch selector. Do not move, delete, or reuse the
public `artifact-rust-analyzer-2026.07.27-pcr.1` tag. The failed run and its
three unpublished platform artifacts are historical evidence, not inputs to
the corrected release.

- [ ] **Step 2: Write failing GNU/Linux source and host-compatibility tests.**

Require the `linux-amd64` rust-analyzer source record to select
`rust-analyzer-x86_64-unknown-linux-gnu.gz` with target triple
`x86_64-unknown-linux-gnu`, archive size `15035345`, archive SHA256
`ac4f42ddbbd040d75d847e991894776485783e28beb744b9719a660a99abe115`,
executable size `42570504`, and executable SHA256
`f06d56b784d621794290826d28f30345029122f86fb2223d7dda820de8dc8de6`.
Keep the pinned upstream tag, commit, version output, licenses, and the other
three platform assets unchanged.

Add installer tests proving that an explicit Linux rust-analyzer request:

- accepts glibc 2.28 or newer before provisioning;
- rejects glibc older than 2.28 and musl/unknown libc before provisioning;
- leaves default installs and non-Linux platforms unchanged;
- reports a bounded, actionable prerequisite error without attempting package
  installation or requiring elevated privileges.

Run the focused Rust and shell tests. Expected: they fail against the `pcr.1`
musl source record, old exact tag, and missing installer compatibility gate.

- [ ] **Step 3: Implement the `pcr.2` release identity and Linux contract.**

Update the canonical source lock and all active provider-release policy,
fixtures, schemas, generator/verifier constants, target mappings, and digests
to pack version `2026.07.27-pcr.2` and the reviewed GNU/Linux asset. Preserve
generic core and Gitleaks `x86_64-unknown-linux-musl` mappings. Artifact-aware
validation must permit the GNU target only for the rust-analyzer
`linux-amd64` provider record; it must not silently broaden unrelated artifact
contracts.

Before provisioning an explicitly requested rust-analyzer provider on Linux,
detect the host libc without mutating the host. Accept only glibc 2.28 or
newer, fail closed on missing/unparseable evidence, and never run `apt`,
`apk`, `sudo`, or another package manager. The release workflow must probe the
reviewed GNU executable directly on its Ubuntu runner and must not mask host
requirements by installing musl.

- [ ] **Step 4: Verify locally, review, and commit without publishing.**

Run `rtk bash tests/artifact_distribution_test.sh`, `rtk bash
tests/provider_release_verifier_test.sh`, `rtk bash
tests/install_rust_analyzer_test.sh`, `rtk python3
scripts/validate_schemas.py`, the focused provider artifact Rust tests,
`rtk actionlint .github/workflows/artifact-pack-release.yml`, and `rtk git
diff --check`. Independently review specification compliance and code quality.
Commit the local correction, but do not create or push the new `pcr.2` tag
until the user explicitly authorizes that new remote action.

## Task 6C: Correct The GNU Version Probe And Retry Immutably

**Files:**

- Modify: `third_party_artifacts/sources/rust-analyzer-2026-07-27.json`
- Modify: `.github/workflows/artifact-pack-release.yml`
- Modify: active provider identity constants, schemas, release scripts,
  fixtures, and digest bindings
- Test: `collect-diff-context-cli/tests/artifact_provider_pack.rs`
- Test: `tests/artifact_distribution_test.sh`
- Test: `tests/provider_release_verifier_test.sh`

- [ ] **Step 1: Preserve the failed `pcr.2` bootstrap as immutable history.**

The exact public tag `artifact-rust-analyzer-2026.07.27-pcr.2` remains fixed at
its reviewed commit. Its run built and attested the three non-Linux packs, but
Linux failed before pack creation because its exact GNU version output did not
match the source lock. Clean verification and publication were skipped, and no
GitHub Release was created. Do not move, delete, reuse, or rerun that tag as a
corrected release.

- [ ] **Step 2: Write failing platform-specific version-output tests.**

Assert that the reviewed GNU `linux-amd64` record alone expects exactly
`rust-analyzer 0.3.2989-standalone`. Assert that Darwin arm64, Darwin amd64,
and Windows amd64 continue to expect exactly
`rust-analyzer 0.3.2989-standalone (12c3381f0b 2026-07-26)`. Require the
workflow trigger and all rust-analyzer job guards to accept only
`artifact-rust-analyzer-2026.07.27-pcr.3`, with `pcr.1` and `pcr.2` rejected as
historical tags.

Run the focused provider source-lock/workflow tests and shell distribution
test. Expected: the Linux version-output assertion and `pcr.3` exact-tag
assertions fail against the `pcr.2` contract.

- [ ] **Step 3: Implement the minimal `pcr.3` correction.**

Change only the Linux source record's `expected_version_output` to the observed
short GNU output. Keep its GNU target, URL, archive/executable sizes and
digests, upstream tag/commit, licenses, and the other three asset records
unchanged. Recompute the canonical source-lock digest and update every active
provider-release binding to pack version `2026.07.27-pcr.3`, release tag
`artifact-rust-analyzer-2026.07.27-pcr.3`, and the new source-lock digest.
Preserve the glibc 2.28 installer gate. Do not add a relaxed, prefix, regex, or
cross-platform version comparison.

- [ ] **Step 4: Verify, review, and commit without publishing.**

Run the focused provider Rust tests, artifact distribution shell test,
provider release verifier test, schema validator, `actionlint`, formatting,
and `git diff --check`. Independently review specification compliance and code
quality. Commit locally, but do not create or push the new `pcr.3` tag until
the user explicitly authorizes that new remote action.

## Task 7: Add Repository-Owned Real Fixtures And Deterministic Evidence

**Files:**

- Create: `collect-diff-context-cli/tests/fixtures/repository_context_provider/real/{single_crate,multi_crate,partial,unicode_crlf,cycles}`
- Create: `collect-diff-context-cli/tests/repository_context_provider_real.rs`
- Create: `tests/provider_real_server_test.sh`
- Modify: `collect-diff-context-cli/src/repository_context_provider/rust_analyzer.rs`, `tests/repository_context_rust_analyzer.rs`

- [ ] **Step 1: Write fixture and report-determinism tests.**

Add fixtures with one crate direct incoming/outgoing calls, linked crates, unresolved/dynamic/macro partial cases, Unicode identifiers, UTF-8/UTF-16 positions, CRLF, stale ranges, cycles, depth-one/depth-two BFS, deduplication, and bounded output. Assert two identical explicit CLI runs produce byte-identical normalized reports after zeroing the documented runtime-dependent `elapsed_ms`, sampled `process_tree_peak_rss_bytes`, and derived `report_bytes` fields. Assert real fixtures never invoke Cargo/rustc, fetch dependencies, inspect a sysroot, or use a user-home/global registry.

```rust
#[test]
fn normalized_real_fixture_reports_are_byte_identical() {
    let first = run_real_fixture("single_crate").unwrap().without_elapsed_metrics();
    let second = run_real_fixture("single_crate").unwrap().without_elapsed_metrics();
    assert_eq!(serde_json::to_vec(&first).unwrap(), serde_json::to_vec(&second).unwrap());
}
```

- [ ] **Step 2: Run tests and observe absent real-pack harness.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --features test-fixture --test repository_context_provider_real`. Expected: the real fixture runner reports that no exact published provider pack is selected or installed.

- [ ] **Step 3: Implement the explicit real-server runner.**

Use only the candidate manifest's exact published pack and target-local generated profile/registry. Verify version, capabilities, quiescent readiness, a known call edge, deterministic rerun, offline environment, cleanup, postflight executable/profile/snapshot drift rejection, and all existing status-matrix cases. Keep fake-server tests as the adversarial source for malformed frames, unknown IDs, floods, timeout, crash, cancellation, and cleanup.

- [ ] **Step 4: Run fixture and shell evidence tests and commit.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --features test-fixture --test repository_context_provider_real --test repository_context_rust_analyzer`, `rtk bash tests/provider_real_server_test.sh`, and `rtk git diff --check`. Expected: real-server reports are deterministic, partial cases remain honestly partial, and no default path reaches the provider. Commit with `rtk git add collect-diff-context-cli/tests/fixtures/repository_context_provider/real collect-diff-context-cli/tests/repository_context_provider_real.rs tests/provider_real_server_test.sh collect-diff-context-cli/src/repository_context_provider/rust_analyzer.rs tests/repository_context_rust_analyzer.rs` followed by `rtk git commit -m "test(provider): add real rust-analyzer fixtures"`.

## Task 8: Measure Pack-Versioned Baselines And Release Thresholds

**Files:**

- Create: `scripts/measure_provider_baseline.py`
- Create: `third_party_artifacts/baselines/rust-analyzer-2026.07.27-pcr.1.json`
- Modify: `third_party_artifacts/manifest.json`, `scripts/generate_provider_manifest_update.py`, `collect-diff-context-cli/tests/provider_baseline.rs`, `collect-diff-context-cli/src/artifacts/provider.rs`

- [ ] **Step 1: Write failing baseline acceptance tests.**

Assert fewer than 20 samples, wrong runner class, mismatched pack/executable/source-lock/profile/fixture/request digests, p95 above `ceil(baseline_p95_ms * 5 / 4) + 250`, provisioning included in timing, or a sample over the existing 30-second deadline is rejected. Assert p95 uses nearest-rank selection and generated JSON is compact/no-newline.

- [ ] **Step 2: Implement the isolated measurement harness.**

Run one unmeasured warm-up followed by at least 20 isolated runs on the same hosted-runner class, exact pack, fixture, request, profile, and environment. Start timing immediately before the Delivery 4 run command spawns the server and stop after report validation and postflight; exclude pack download/extraction/provisioning. Record raw milliseconds, nearest-rank p95, observed peak RSS, pack/executable/source-lock/profile/fixture/request/runner digests, and toolchain identity in the strict baseline.

- [ ] **Step 3: Bind baseline digest and acceptance calculation.**

Compute `ceil(p95_ms * 5 / 4) + 250` in checked integer arithmetic and require the canonical baseline file SHA256 to equal `quality_baseline_sha256` in every active provider record. Baselines are reviewed data and cannot be generated or accepted inside the core release job.

- [ ] **Step 4: Run baseline tests and commit reviewed data.**

Run `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --test provider_baseline`, `rtk python3 scripts/measure_provider_baseline.py --fixture single_crate --samples 20`, `rtk python3 scripts/generate_provider_manifest_update.py --fixture tests/fixtures/provider-release --baseline third_party_artifacts/baselines/rust-analyzer-2026.07.27-pcr.1.json`, `rtk python3 scripts/validate_schemas.py`, and `rtk git diff --check`. Expected: the real baseline digest matches the manifest update and threshold tests reject one millisecond above the computed limit. Then run `rtk git add scripts/measure_provider_baseline.py third_party_artifacts/baselines/rust-analyzer-2026.07.27-pcr.1.json third_party_artifacts/manifest.json scripts/generate_provider_manifest_update.py collect-diff-context-cli/tests/provider_baseline.rs collect-diff-context-cli/src/artifacts/provider.rs` and `rtk git commit -m "test(provider): establish pack-versioned latency baselines"`.

## Task 9: Add Four-Platform CI, Fuzz Tiers, And Release Trust Gates

**Files:**

- Create: `.github/workflows/provider-real-server.yml`, `.github/workflows/provider-fuzz-scheduled.yml`
- Modify: `.github/workflows/artifact-pack-release.yml`, `.github/workflows/lint.yml`, `.github/workflows/release.yml`, `collect-diff-context-cli/fuzz/README.md`
- Modify: `collect-diff-context-cli/fuzz/fuzz_targets/repository_context_frame.rs`, `repository_context_messages.rs`
- Test: `tests/provider_real_server_test.sh`, `tests/artifact_distribution_test.sh`

- [ ] **Step 1: Write workflow fixture assertions.**

Assert the PR matrix names `darwin-arm64`, `darwin-amd64`, `linux-amd64`, and `windows-amd64`, consumes an already-published exact pack selected by the candidate manifest, and checks version/capability/readiness/known edge/determinism/offline/cleanup/RSS. Assert scheduled/release jobs run the full fixture suite and p95 gates. Assert fuzz jobs use exactly 256 iterations per existing frame/messages target in PR, 15 minutes per target on schedule, and 30 minutes per target on provider/core release; generated hash-named corpus files are never committed.

- [ ] **Step 2: Implement pinned actions and Rust 1.95 locked jobs.**

Pin checkout, toolchain, cache, upload, attestation, and release actions to reviewed commit SHAs. Replace moving `stable` and unlocked release builds with Rust `1.95.0` and `--locked`; record toolchain and lockfile digests in evidence. Keep `nightly` limited to cargo-fuzz and record its exact toolchain in fuzz evidence. Do not use `real-host-smoke.yml` as the provider matrix; it is a separate self-hosted host-readiness workflow.

- [ ] **Step 3: Implement clean-consumer trust and publication order.**

Provider-pack publication builds and attests packs first. A clean verifier checks external core sidecar/attestation before extraction, pack/manifest/SBOM subject digests, signer repository/workflow/source ref/commit/OIDC issuer, composition predicate material digests, source locks, licenses, receipts, and revocations. The core release consumes only merged reviewed manifest bytes and never references an unpublished same-run provider pack. Verify GitHub release immutability is enabled before claiming immutable releases; otherwise fail the release claim.

- [ ] **Step 4: Run workflow/static gates and commit.**

Run `rtk python3 scripts/validate_schemas.py`, `rtk bash tests/provider_real_server_test.sh`, `rtk bash tests/artifact_distribution_test.sh`, `rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --features test-fixture --test repository_context_provider_platform`, `rtk cargo +nightly fuzz build collect-diff-context-cli/fuzz`, and `rtk git diff --check`. Expected: all workflow assertions, four-platform matrix configuration, fuzz target build, and trust fixtures pass. Commit with `rtk git add .github/workflows/provider-real-server.yml .github/workflows/provider-fuzz-scheduled.yml .github/workflows/artifact-pack-release.yml .github/workflows/lint.yml .github/workflows/release.yml collect-diff-context-cli/fuzz/README.md collect-diff-context-cli/fuzz/fuzz_targets/repository_context_frame.rs collect-diff-context-cli/fuzz/fuzz_targets/repository_context_messages.rs tests/provider_real_server_test.sh tests/artifact_distribution_test.sh` followed by `rtk git commit -m "ci(provider): gate real servers fuzz and release trust"`.

## Task 10: Finish Reachability, Documentation, And Release-Readiness Sweep

**Files:**

- Modify: `collect-diff-context-cli/tests/repository_context_provider_platform.rs`, `tests/static_analysis_orchestration_test.sh`, `tests/static_analysis_execution_test.sh`, `tests/repository_index_workflow_test.sh`
- Modify: `docs/rust-analyzer-context-provider.md`, `docs/helper-capabilities.md`, `README.md`
- Modify: `.github/workflows/lint.yml`, `.github/workflows/release.yml`

- [ ] **Step 1: Add negative reachability tests.**

Assert ordinary review, Fast Mode, repository index, SQLite persistence, and static-analysis orchestration do not mention or invoke provider binaries, do not read target-local provider registries implicitly, and never download artifacts. Assert no production code or shell script contains PATH discovery, rustup, package-manager, direct-upstream, `latest`, `nightly`, or global-registry fallback; nightly remains allowed only in fuzz workflow files.

- [ ] **Step 2: Document explicit install and evidence boundaries.**

Document `install.sh --with-rust-analyzer`, `--no-download` verified-cache behavior, `--link` rejection, generated target-local profile/registry paths, explicit CLI arguments, no runtime download/PATH/rustup/direct upstream, no global registry, sampled RSS policy and its sub-100-ms limitation, p95 threshold, external-binary SBOM evidence scope, immutable-release requirement, and the fact that old offline manifests cannot learn later revocations.

- [ ] **Step 3: Run the complete Rust 1.95/release-readiness gate.**

Run:

```bash
rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/Cargo.toml -- --check
rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets --all-features -- -D warnings
rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets --all-features
rtk cargo +1.95.0 build --manifest-path collect-diff-context-cli/Cargo.toml --locked --bin repository-context-provider-cli
rtk python3 scripts/validate_schemas.py
rtk bash tests/repository_context_provider_cli_test.sh
rtk bash tests/provider_real_server_test.sh
rtk bash tests/install_rust_analyzer_test.sh
rtk bash tests/gitleaks_distribution_test.sh
rtk bash tests/artifact_distribution_test.sh
rtk bash -n install.sh scripts/*.sh scripts/lib/*.sh
rtk shellcheck install.sh scripts/*.sh scripts/lib/*.sh
rtk git diff --check
```

Expected: all existing Delivery 4 fake-server tests and new 5B evidence gates pass; provider remains explicit and unreachable from default paths; no generated pack archives or hash-named fuzz corpus files are staged.

- [ ] **Step 4: Commit the final 5B documentation and gate updates.**

Run `rtk git add collect-diff-context-cli/tests/repository_context_provider_platform.rs tests/static_analysis_orchestration_test.sh tests/static_analysis_execution_test.sh tests/repository_index_workflow_test.sh docs/rust-analyzer-context-provider.md docs/helper-capabilities.md README.md .github/workflows/lint.yml .github/workflows/release.yml` followed by `rtk git commit -m "docs(provider): record release readiness boundaries"`. Expected: the branch contains separate 5A and 5B commit series with no default-pipeline invocation.

## Self-Review Checklist

- [ ] Source lock, pack version, outer/internal/SBOM/executable digests, quality baseline, and target triples are all explicitly bound.
- [ ] Provider packs publish and verify before any core manifest update; core release never consumes same-run unpublished output.
- [ ] Profile and registry are generated from Delivery 4 typed contracts with compact canonical bytes, final absolute paths, exact digests, and no newline.
- [ ] Provider installation is explicit, current-platform-only, copy-mode transactional, offline-cache capable, and leaves an existing target unchanged on every provider failure.
- [ ] RSS is a sampled process-tree acceptance threshold, not a universal kernel containment claim; missing accounting fails the real-server gate.
- [ ] Real fixtures are network-independent and do not run Cargo, fetch dependencies, or discover sysroots; fake server remains the adversarial protocol source.
- [ ] p95 includes spawn/readiness/BFS/normalization/cleanup, excludes provisioning/extraction, uses nearest-rank samples, and applies integer `ceil(p95 * 5 / 4) + 250`.
- [ ] PR/scheduled/release fuzz durations and evidence are exact; hash-named generated corpus files remain untracked.
- [ ] External core trust, signer/workflow/ref/issuer, composition materials, SBOM scope, action SHAs, Rust 1.95 lockfiles, and immutable-release gating are explicit.
- [ ] Every task gives a concrete interface, test, command, expected result, and commit; no task defers an implementation detail to an unnamed step.
