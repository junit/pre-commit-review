# Rust-Analyzer Provider Explicit CLI Implementation Plan

> For agentic workers: REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox ([ ]) syntax for tracking.

**Goal:** Add an explicit, standalone rust-analyzer provider CLI with strict registry and run-request contracts, snapshot-only model construction, packaging gates, and no default-pipeline integration.

**Architecture:** Keep the existing repository_context_provider library as the execution boundary. Add a CLI contract module for registry and run-request inputs, a snapshot-only linked-project model builder, a thin command binary, and a shell resolver/wrapper. The CLI constructs all candidate/provider bindings itself from an authoritative scope, validated registry entry, exact model file, and bounded request; it never trusts caller-supplied binding fields.

**Tech Stack:** Rust 1.95, existing serde/serde_json/sha2/toml/tempfile APIs, existing CandidateSnapshot and review_scope control plane, existing fake LSP fixture, Bash wrappers, JSON Schema Draft 2020-12, GitHub Actions, and the existing installer/release payload.

---

## Execution Boundary

Execute this plan from a new branch named feature/rust-analyzer-provider-cli cut from feature/SAST in a project-local .worktrees directory. The worktree must be created with superpowers:using-git-worktrees before Task 1. Do not modify feature/SAST with runtime code.

The plan intentionally does not add a real rust-analyzer binary, download URL,
installer, platform artifact, or sustained fuzz campaign. Those remain Delivery
5. The provider remains unreachable from ordinary review, Fast Mode, repository
index, SQLite persistence, and static-analysis orchestration.

## File Map

Create:

- collect-diff-context-cli/src/repository_context_provider/cli_contract.rs:
  strict registry, run-request, and model-limit types.
- collect-diff-context-cli/src/repository_context_provider/model.rs:
  bounded snapshot-only Cargo metadata reader and linked-project converter.
- collect-diff-context-cli/src/repository_context_provider/cli.rs:
  argument parsing, bounded JSON loading, scope/snapshot setup, binding
  construction, report rendering, and stable exit mapping.
- collect-diff-context-cli/src/bin/repository_context_provider.rs:
  thin main entrypoint calling cli::main_entry.
- collect-diff-context-cli/schemas/repository-context-provider-registry.schema.json
- collect-diff-context-cli/schemas/repository-context-provider-run-request.schema.json
- collect-diff-context-cli/tests/repository_context_provider_cli_contracts.rs
- collect-diff-context-cli/tests/repository_context_provider_model.rs
- collect-diff-context-cli/tests/repository_context_provider_cli.rs
- scripts/run_repository_context_provider.sh
- scripts/lib/repository_context_provider_cli.sh
- tests/repository_context_provider_cli_test.sh

Modify:

- collect-diff-context-cli/src/repository_context_provider/mod.rs:
  export cli_contract, model, and cli without changing the library runner
  signature.
- collect-diff-context-cli/Cargo.toml:
  register the standalone binary.
- scripts/build_all_binaries.sh:
  build and copy the provider CLI alongside existing platform binaries.
- install.sh:
  include the wrapper, resolver, provider CLI binary, and two schemas in
  offline copy payloads.
- tests/install_smoke_test.sh:
  assert provider CLI, wrapper, resolver, and schemas are present.
- scripts/validate_schemas.py:
  load the two schemas and validate provider CLI output invariants.
- .github/workflows/lint.yml and .github/workflows/release.yml:
  build, help-smoke, schema, shell, and payload gates.
- docs/rust-analyzer-context-provider.md and docs/helper-capabilities.md:
  document the explicit CLI and the no-artifact Delivery 4 boundary.

## Task 1: Define Registry And Run-Request Contracts

Files:

- Create: collect-diff-context-cli/src/repository_context_provider/cli_contract.rs
- Create: collect-diff-context-cli/schemas/repository-context-provider-registry.schema.json
- Create: collect-diff-context-cli/schemas/repository-context-provider-run-request.schema.json
- Modify: collect-diff-context-cli/src/repository_context_provider/mod.rs
- Test: collect-diff-context-cli/tests/repository_context_provider_cli_contracts.rs

- [ ] Step 1: Write failing contract tests.

Add tests that construct valid registry and run-request values and assert:

    registry.validate().unwrap();
    request.validate().unwrap();
    assert_eq!(
        serde_json::from_slice::<ProviderRegistry>(
            &serde_json::to_vec(&registry).unwrap()
        ).unwrap(),
        registry
    );

Reject an unknown field, a relative profile/executable path, an empty
provider_id, duplicate provider ids, an uppercase or short digest, a target
other than the current target grammar, a toolchain mode other than none, more
than 16 registry entries, an empty seed list, duplicate seed ids, duplicate
directions, zero limits, and a request limit above the provider maxima.

- [ ] Step 2: Run the focused test and observe the missing types.

Run:

    rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_context_provider_cli_contracts

Expected result: compilation fails because ProviderRegistry,
ProviderRegistryEntry, ProviderRunRequest, and their validation methods do not
exist.

- [ ] Step 3: Implement the typed contracts.

Add the following public types and methods:

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ProviderRegistry {
        pub schema_version: u8,
        pub kind: String,
        pub entries: Vec<ProviderRegistryEntry>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ProviderRegistryEntry {
        pub provider_id: String,
        pub provider_kind: String,
        pub provider_version: String,
        pub target_triple: String,
        pub profile_path: PathBuf,
        pub profile_sha256: String,
        pub executable_path: PathBuf,
        pub executable_sha256: String,
        pub configuration_sha256: String,
        pub toolchain_mode: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ProviderRunRequest {
        pub schema_version: u8,
        pub kind: String,
        pub seeds: Vec<SeedSymbol>,
        pub directions: Vec<CallDirection>,
        pub limits: ProviderLimits,
    }

    impl ProviderRegistry {
        pub fn validate(&self) -> Result<(), CliContractError>;
        pub fn sha256(&self) -> String;
        pub fn select(&self, provider_id: &str)
            -> Result<&ProviderRegistryEntry, CliContractError>;
    }

    impl ProviderRunRequest {
        pub fn validate(&self) -> Result<(), CliContractError>;
        pub fn validate_against(&self, maxima: &ProviderLimits)
            -> Result<(), CliContractError>;
    }

Use the existing absolute-path, lower-case SHA256, target, text, range, seed,
direction, and limit validators from repository_context_provider::contract.
Registry validation must not open profile or executable files; byte and profile
validation occurs in the CLI preflight task.

- [ ] Step 4: Add the strict JSON schemas.

The registry schema must require schema_version 1, kind
repository_context_provider_registry, one through sixteen entries, unique
provider_id values, absolute path patterns, lower-case 64-hex digests, and
toolchain_mode none. Set additionalProperties false on every object.

The run-request schema must require schema_version 1, kind
repository_context_provider_run_request, non-empty sorted seeds, non-empty
directions, and a complete bounded limits object. Set additionalProperties
false on every object and reuse the exact provider range and seed enums.

- [ ] Step 5: Make contract and schema checks green.

Run:

    rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_context_provider_cli_contracts
    rtk python3 scripts/validate_schemas.py
    rtk git diff --check

Expected result: all focused tests and every schema validation pass.

- [ ] Step 6: Commit the contracts.

    rtk git add collect-diff-context-cli/src/repository_context_provider/cli_contract.rs collect-diff-context-cli/src/repository_context_provider/mod.rs collect-diff-context-cli/schemas/repository-context-provider-registry.schema.json collect-diff-context-cli/schemas/repository-context-provider-run-request.schema.json collect-diff-context-cli/tests/repository_context_provider_cli_contracts.rs
    rtk git commit -m "feat(provider): define explicit CLI contracts"

## Task 2: Build The Snapshot-Only Linked Project Model

Files:

- Create: collect-diff-context-cli/src/repository_context_provider/model.rs
- Modify: collect-diff-context-cli/src/repository_context_provider/mod.rs
- Test: collect-diff-context-cli/tests/repository_context_provider_model.rs
- Test support: collect-diff-context-cli/tests/support/mod.rs only if a
  reusable fixture helper is required.

- [ ] Step 1: Write failing model-builder tests.

Create temporary Git fixtures containing:

1. one package with lib, bin, and integration-test roots;
2. a literal workspace with two package manifests;
3. workspace globs and inherited package fields;
4. malformed and oversized Cargo.toml files;
5. a repository-owned build.rs and rust-analyzer.toml marker.

Call the desired API:

    let snapshot = CandidateSnapshot::materialize(
        repo.path(),
        ReviewSource::Branch,
        SnapshotLimits { max_files: 64, max_bytes: 64 * 1024 },
    )?;
    let model = build_linked_project_model(
        &snapshot,
        ProviderModelLimits::default(),
    )?;

Assert that roots and crate ids are path sorted, editions are preserved,
unsupported workspace fields become deterministic limitations, malformed
manifests never panic, the model digest changes when consumed bytes or policy
changes, and no Cargo/rustc/build-script marker is created.

- [ ] Step 2: Run the model tests and observe the missing builder.

    rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_context_provider_model

Expected result: compilation fails because build_linked_project_model and
ProviderModelLimits do not exist.

- [ ] Step 3: Implement bounded snapshot enumeration and reading.

Define:

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ProviderModelLimits {
        pub max_files: usize,
        pub max_bytes: usize,
        pub max_file_bytes: usize,
    }

    impl Default for ProviderModelLimits {
        fn default() -> Self {
            Self {
                max_files: 1_000,
                max_bytes: 8 * 1024 * 1024,
                max_file_bytes: 1 * 1024 * 1024,
            }
        }
    }

    pub fn build_linked_project_model(
        snapshot: &CandidateSnapshot,
        limits: ProviderModelLimits,
    ) -> Result<RustAnalyzerProjectModel, ModelBuildError>;

Walk only the canonical snapshot root with a sorted queue. Count every
regular file and consumed byte before reading it. Reject .git files and
directories, symlinks, paths escaping the root, and files beyond the limits.
Read Cargo.toml files and declared source roots from the snapshot; never call
Git or a process. Reuse the existing passive TOML field parsing rules where
they are compatible, but keep the provider model independent of repository
index persistence.

- [ ] Step 4: Convert passive Cargo facts into the provider model.

For each accepted package, emit a stable crate_id derived from the
snapshot-relative manifest path, a snapshot-relative root_module, the
declared or default edition, and sorted dependency records. Emit sorted cfg
and env maps and sorted limitations. Map build script, proc macro, workspace
inheritance, glob, missing root, invalid UTF-8, malformed TOML, and budget
exhaustion into explicit limitation codes.

Canonicalize the complete provider model with the existing serde JSON helper,
hash it with SHA256, store the resulting digest in the model, and call
RustAnalyzerProjectModel::validate before returning. A partial model is valid
with limitations; an invalid binding, unsafe path, or digest mismatch is an
error.

- [ ] Step 5: Make model tests green and commit.

    rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/Cargo.toml
    rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_context_provider_model
    rtk git diff --check
    rtk git add collect-diff-context-cli/src/repository_context_provider/model.rs collect-diff-context-cli/src/repository_context_provider/mod.rs collect-diff-context-cli/tests/repository_context_provider_model.rs collect-diff-context-cli/tests/support/mod.rs
    rtk git commit -m "feat(provider): build linked models from snapshots"

## Task 3: Add The Standalone CLI And Model Command

Files:

- Create: collect-diff-context-cli/src/repository_context_provider/cli.rs
- Create: collect-diff-context-cli/src/bin/repository_context_provider.rs
- Modify: collect-diff-context-cli/src/repository_context_provider/mod.rs
- Modify: collect-diff-context-cli/Cargo.toml
- Test: collect-diff-context-cli/tests/repository_context_provider_cli.rs

- [ ] Step 1: Write failing parser and model-command tests.

Test the binary with --help, model help, unknown flags, duplicate flags,
relative paths, missing source/scope, malformed scope, and unsupported source.
Use a temporary Git fixture to invoke model with the exact scope fingerprint,
parse stdout as repository-context-project-model JSON, and assert that the
model digest and limitation ordering are stable across two runs.

- [ ] Step 2: Run the tests and observe the missing binary.

    rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_context_provider_cli

Expected result: Cargo cannot find the repository-context-provider-cli
binary and the integration test fails.

- [ ] Step 3: Implement bounded CLI parsing.

Define:

    pub fn main_entry() -> i32;

    pub enum Command {
        Model(ModelArgs),
        Run(RunArgs),
    }

    pub struct ModelArgs {
        pub source: ReviewSource,
        pub expected_scope: String,
        pub maximum_model_files: usize,
        pub maximum_model_bytes: usize,
    }

    pub struct RunArgs {
        pub source: ReviewSource,
        pub expected_scope: String,
        pub registry_path: PathBuf,
        pub expected_registry_sha256: String,
        pub provider_id: String,
        pub model_path: PathBuf,
        pub expected_model_sha256: String,
        pub request_path: PathBuf,
    }

Reuse the static-analysis CLI parser conventions: support --flag value and
--flag=value, reject duplicate values and unknown flags, and return stable
bounded errors without panicking. Require every path to be absolute and every
digest to be lower-case 64-hex.

- [ ] Step 4: Implement the model command.

Open the authoritative scope with open_authoritative_scope_bounded using a
bounded deadline, compare its fingerprint to expected_scope, materialize a
CandidateSnapshot with ProviderModelLimits-derived file/byte limits, call
build_linked_project_model, and serialize exactly one compact JSON value to
stdout. Revalidate the scope and snapshot before serialization. Do not print
the temporary snapshot root or any process output.

- [ ] Step 5: Register the binary and make model tests green.

Add:

    [[bin]]
    name = "repository-context-provider-cli"
    path = "src/bin/repository_context_provider.rs"

The binary main function must call
collect_diff_context_cli::repository_context_provider::cli::main_entry and
exit with its result. Run:

    rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/Cargo.toml
    rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --test repository_context_provider_cli
    rtk cargo +1.95.0 run --manifest-path collect-diff-context-cli/Cargo.toml --bin repository-context-provider-cli -- --help

Expected result: help is stable, model integration passes, and the command
does not appear in any default review or repository-context command parser.

- [ ] Step 6: Commit the CLI shell.

    rtk git add collect-diff-context-cli/src/repository_context_provider/cli.rs collect-diff-context-cli/src/bin/repository_context_provider.rs collect-diff-context-cli/src/repository_context_provider/mod.rs collect-diff-context-cli/Cargo.toml collect-diff-context-cli/tests/repository_context_provider_cli.rs
    rtk git commit -m "feat(provider): expose explicit model CLI"

## Task 4: Implement Registry-Backed Provider Run

Files:

- Modify: collect-diff-context-cli/src/repository_context_provider/cli.rs
- Modify: collect-diff-context-cli/src/repository_context_provider/cli_contract.rs
- Test: collect-diff-context-cli/tests/repository_context_provider_cli.rs
- Test: collect-diff-context-cli/tests/repository_context_provider_cli_contracts.rs

- [ ] Step 1: Add failing registry and run integration tests.

Use the existing repository-context-provider-fixture executable as a fake
server and create a temporary registry, profile, model, and run-request file.
Assert that a valid run returns a report with the expected candidate/model and
provider digests. Mutate each registry, profile, executable, model, request,
scope, and snapshot input and assert a nonzero exit with no report on stdout.
Assert that a fake server can produce completed, partial, unavailable,
timeout, invalid-output, and failed report statuses without leaking stderr or
opaque LSP data.

- [ ] Step 2: Run the integration test and observe missing run behavior.

    rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --features test-fixture --test repository_context_provider_cli

Expected result: run arguments are rejected because the run command has not
yet been wired to the provider runner.

- [ ] Step 3: Implement exact bounded JSON loading.

Add:

    pub fn read_json_once<T: DeserializeOwned>(
        path: &Path,
        maximum_bytes: usize,
    ) -> Result<(T, String), CliError>;

Canonicalize every path, reject directories and symlinks that escape the
trusted boundary, read at most the contract maximum plus one byte, compute
the exact file SHA256, deserialize with deny_unknown_fields, and return the
raw file digest with the typed value. The registry digest must match the
expected command-line digest before profile/executable files are opened.

- [ ] Step 4: Construct and validate the owned provider request.

Implement:

    fn build_provider_request(
        scope: &AuthoritativeScope,
        registry: &ProviderRegistry,
        entry: &ProviderRegistryEntry,
        model: &RustAnalyzerProjectModel,
        run_request: &ProviderRunRequest,
        snapshot: &CandidateSnapshot,
        profile: &AuthorizedProviderProfile,
    ) -> Result<RepositoryContextProviderRequest, CliError>;

Materialize the candidate snapshot using the provider limits, construct
CandidateBinding from the snapshot and scope, construct ProviderBinding from
the registry/profile identity, and validate the request against profile
maxima. Check the raw model file digest and RustAnalyzerProjectModel::digest.
Reject profile/executable paths inside the snapshot, profile/executable digest
mismatches, target mismatch, configuration mismatch, and model root paths not
present in the snapshot. Return an owned RepositoryContextProviderRequest. The
run command must retain the snapshot, model, authorized profile, and returned
request in the same scope, set cancellation to a fresh false AtomicBool, create
ProviderInvocation with references to those owned values, and call
run_repository_context_provider only after every check succeeds.

- [ ] Step 5: Render report and map stable exits.

Serialize only RepositoryContextProviderReport on stdout. Map successful
report construction, including partial/unavailable status, to exit 0.
Map argument/schema/scope/registry/profile/model/executable/snapshot
authorization errors to exit 2. Map cancellation and unrecoverable provider
preflight/session errors without a safe report to exit 3. Bound all stderr
messages to 512 bytes and include only stable error codes.

- [ ] Step 6: Run focused tests and commit.

    rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/Cargo.toml
    rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --features test-fixture --test repository_context_provider_cli --test repository_context_provider_cli_contracts
    rtk git diff --check
    rtk git add collect-diff-context-cli/src/repository_context_provider/cli.rs collect-diff-context-cli/src/repository_context_provider/cli_contract.rs collect-diff-context-cli/tests/repository_context_provider_cli.rs collect-diff-context-cli/tests/repository_context_provider_cli_contracts.rs
    rtk git commit -m "feat(provider): run explicit registry entries"

## Task 5: Add Public Shell Wrapper And Resolver

Files:

- Create: scripts/run_repository_context_provider.sh
- Create: scripts/lib/repository_context_provider_cli.sh
- Test: tests/repository_context_provider_cli_test.sh

- [ ] Step 1: Write the failing shell integration test.

The test must set PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN to an
absolute fake CLI path, invoke the wrapper with --help and a valid model/run
fixture, assert stdout is valid JSON, and assert relative overrides,
missing binaries, child stderr, and invalid exit codes are rejected without
printing raw child stderr.

- [ ] Step 2: Run the shell test and observe the missing wrapper.

    rtk bash tests/repository_context_provider_cli_test.sh

Expected result: the wrapper file does not exist and the test fails before
starting a provider.

- [ ] Step 3: Implement the resolver.

Define resolve_repository_context_provider_cli in
scripts/lib/repository_context_provider_cli.sh. Resolve in this order:

1. PRE_COMMIT_REVIEW_REPOSITORY_CONTEXT_PROVIDER_BIN when it is absolute and
   executable;
2. collect-diff-context-cli/target/release/repository-context-provider-cli;
3. scripts/bin/repository_context_provider-<os>-<arch> with .exe on Windows.

Reject unknown OS/architecture and never search ambient PATH.

- [ ] Step 4: Implement the wrapper.

The wrapper must resolve its own directory, source the resolver, require a
resolved binary, execute it with the exact argument vector, preserve the
binary exit code, and bound stderr to the stable wrapper error format. It
must not add a shell around provider arguments, discover a registry, or
resolve rust-analyzer itself. Add shellcheck-safe quoting and an executable
mode.

- [ ] Step 5: Run shell and static checks, then commit.

    rtk bash tests/repository_context_provider_cli_test.sh
    rtk shellcheck -S warning -s bash scripts/run_repository_context_provider.sh scripts/lib/repository_context_provider_cli.sh tests/repository_context_provider_cli_test.sh
    rtk git diff --check
    rtk git add scripts/run_repository_context_provider.sh scripts/lib/repository_context_provider_cli.sh tests/repository_context_provider_cli_test.sh
    rtk git update-index --chmod=+x scripts/run_repository_context_provider.sh tests/repository_context_provider_cli_test.sh
    rtk git commit -m "feat(provider): add explicit CLI wrapper"

## Task 6: Package The CLI And Add CI/Schema Gates

Files:

- Modify: scripts/build_all_binaries.sh
- Modify: install.sh
- Modify: tests/install_smoke_test.sh
- Modify: scripts/validate_schemas.py
- Modify: .github/workflows/lint.yml
- Modify: .github/workflows/release.yml
- Test: tests/repository_context_provider_cli_test.sh

- [ ] Step 1: Add failing payload and schema assertions.

Extend install smoke to require the provider CLI wrapper, resolver, both
provider schemas, and the platform provider CLI binary. Extend schema
validation to load both schemas and reject a report whose candidate scope
does not match the opening scope or whose provider/model digest does not
match the authorized inputs.

- [ ] Step 2: Package the binary and support files.

Build the new Cargo binary for the existing Linux amd64, macOS arm64,
macOS amd64, and Windows amd64 matrix. Copy it as
repository_context_provider-<os>-<arch> with the existing executable mode
conventions. Add the wrapper, resolver, schemas, and provider documentation
to the offline installer and release distribution. Do not add any
rust-analyzer binary or download URL.

- [ ] Step 3: Add semantic schema invariants.

In scripts/validate_schemas.py, add
validate_provider_report_invariants(payload). It must reject local snapshot
roots, raw stderr fields, raw JSON-RPC fields, unknown top-level report keys,
empty candidate/provider identity fields, and a provider execution record
whose profile, executable, configuration, or model digest fields are absent.
The CLI integration test, which owns the expected input files, separately
asserts that the report scope, model digest, profile digest, executable
digest, and configuration digest equal the authorized files. Keep existing
schema validators unchanged for all other contracts.

- [ ] Step 4: Add workflow gates.

The lint workflow must build repository-context-provider-cli, run its --help,
run provider contract/model/CLI tests with the test-fixture feature, run
tests/repository_context_provider_cli_test.sh, and run schema validation.
The release workflow must smoke the packaged provider CLI --help and assert
that no rust-analyzer artifact is present in the release payload.

- [ ] Step 5: Run packaging checks and commit.

    rtk cargo +1.95.0 build --release --manifest-path collect-diff-context-cli/Cargo.toml --bin repository-context-provider-cli
    rtk bash tests/install_smoke_test.sh
    rtk python3 scripts/validate_schemas.py
    rtk bash tests/repository_context_provider_cli_test.sh
    rtk git diff --check
    rtk git add scripts/build_all_binaries.sh install.sh tests/install_smoke_test.sh scripts/validate_schemas.py .github/workflows/lint.yml .github/workflows/release.yml tests/repository_context_provider_cli_test.sh
    rtk git commit -m "build(provider): package explicit CLI"

## Task 7: Update User Documentation And Capability Boundaries

Files:

- Modify: docs/rust-analyzer-context-provider.md
- Modify: docs/helper-capabilities.md
- Modify: docs/call-graph-open-source-options.md
- Test: tests/repository_context_provider_cli_test.sh

- [ ] Step 1: Add a documentation assertion that currently fails.

Extend the CLI shell test to require the documented command names
repository-context-provider-cli model and repository-context-provider-cli run,
the registry/request schema paths, and the statement that no real
rust-analyzer artifact is bundled or downloaded.

- [ ] Step 2: Document the explicit workflow.

Add the model and run command examples, the registry digest requirement, the
snapshot-only model boundary, exit codes, report-output constraints, and
the fact that the wrapper never resolves rust-analyzer. Keep the current
library-only and no-default-pipeline statements intact. State that Delivery 5
owns real-server fixtures, artifacts, sustained fuzzing, and trust-chain
evidence.

- [ ] Step 3: Make documentation and shell checks green, then commit.

    rtk bash tests/repository_context_provider_cli_test.sh
    rtk git diff --check
    rtk git add docs/rust-analyzer-context-provider.md docs/helper-capabilities.md docs/call-graph-open-source-options.md tests/repository_context_provider_cli_test.sh
    rtk git commit -m "docs(provider): document explicit CLI boundary"

## Task 8: Delivery 4 Verification And Audit

Files:

- Verify all files from Tasks 1-7.
- Modify only a task-owned file when a verification failure requires a fix.

- [ ] Step 1: Run Rust format, tests, and Clippy.

    rtk cargo +1.95.0 fmt --all --manifest-path collect-diff-context-cli/Cargo.toml -- --check
    rtk cargo +1.95.0 test --manifest-path collect-diff-context-cli/Cargo.toml --all-features
    rtk cargo +1.95.0 clippy --manifest-path collect-diff-context-cli/Cargo.toml --locked --all-targets --all-features -- -D warnings

Expected result: every existing test plus all provider CLI tests pass with no
warnings.

- [ ] Step 2: Run schema, shell, installer, and release-shape gates.

    rtk python3 scripts/validate_schemas.py
    rtk bash tests/repository_context_provider_cli_test.sh
    rtk bash tests/install_smoke_test.sh
    rtk git diff --check

Expected result: all commands exit 0; the packaged payload contains the
provider CLI and schemas but no rust-analyzer executable or URL.

- [ ] Step 3: Run provider fuzz smoke and reachability checks.

    rtk cargo +nightly fuzz run repository_context_frame --fuzz-dir collect-diff-context-cli/fuzz -- -runs=256 -timeout=5
    rtk cargo +nightly fuzz run repository_context_messages --fuzz-dir collect-diff-context-cli/fuzz -- -runs=256 -timeout=5
    rtk rg -n 'repository_context_provider|repository-context-provider-cli' collect-diff-context-cli/src/main.rs collect-diff-context-cli/src/bin/static_analysis.rs collect-diff-context-cli/src/bin/repository_context.rs

Expected result: fuzz smoke passes, and the reachability search finds no
provider invocation in the default review, static-analysis, or repository
index command paths.

- [ ] Step 4: Audit the approved design invariants.

Confirm that registry and request paths are explicit, file and model digests
are checked before execution, the model builder reads only the snapshot,
provider reports remain unchanged, all drift cases fail closed, and no real
rust-analyzer artifact or release claim was introduced.

- [ ] Step 5: Commit only audit fixes and record completion.

If the audit changed an owning file, run its focused test again and commit the
smallest change with the owning task's commit convention. If no fix is
required, create no empty audit commit. Finish with:

    rtk git status --short --branch
    rtk git log --oneline --decorate -12

Expected result: clean feature/rust-analyzer-provider-cli worktree with all
Delivery 4 commits present and no unrelated staged files.
