#![cfg(feature = "test-fixture")]

use collect_diff_context_cli::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
use collect_diff_context_cli::repository_context_provider::cli_contract::{
    ProviderRegistry, ProviderRunRequest,
};
use collect_diff_context_cli::repository_context_provider::contract::{
    AuthorizedProviderProfile, CallDirection, PositionEncoding, ProviderLimits, ProviderRange,
    ProviderRangeFormat, RepositoryContextProviderReport, RepositoryContextProviderStatus,
    SeedKind, SeedSymbol,
};
use collect_diff_context_cli::repository_context_provider::model::{
    build_linked_project_model, ProviderModelLimits,
};
use collect_diff_context_cli::review_scope::{
    open_authoritative_scope_bounded, ReviewSource, ScopeRequest,
};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;
use tempfile::TempDir;

const FIXTURES: [&str; 5] = [
    "single_crate",
    "multi_crate",
    "partial",
    "unicode_crlf",
    "cycles",
];

fn fixture_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repository_context_provider/real")
        .join(name)
}

fn git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {arguments:?} failed");
}

fn git_repository() -> TempDir {
    let repository = TempDir::new().unwrap();
    git(repository.path(), &["init", "-q"]);
    git(
        repository.path(),
        &["config", "user.email", "provider-real@example.invalid"],
    );
    git(
        repository.path(),
        &["config", "user.name", "Provider Real Fixture"],
    );
    fs::write(repository.path().join("README.md"), b"baseline\n").unwrap();
    git(repository.path(), &["add", "--", "README.md"]);
    git(repository.path(), &["commit", "-q", "-m", "baseline"]);
    repository
}

fn copy_fixture(source: &Path, destination: &Path) {
    let mut entries = fs::read_dir(source)
        .unwrap()
        .map(|entry| entry.unwrap())
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&target).unwrap();
            copy_fixture(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn materialize_fixture(source: &Path, destination: &Path) {
    copy_fixture(source, destination);
    let attributes = source.join(".gitattributes");
    if attributes.is_file()
        && fs::read_to_string(attributes)
            .unwrap()
            .lines()
            .any(|line| line == "src/lib.rs -text")
    {
        let source_path = destination.join("src/lib.rs");
        let bytes = fs::read(&source_path).unwrap();
        let mut crlf =
            Vec::with_capacity(bytes.len() + bytes.iter().filter(|byte| **byte == b'\n').count());
        for byte in bytes {
            if byte == b'\n' && crlf.last() != Some(&b'\r') {
                crlf.push(b'\r');
            }
            crlf.push(byte);
        }
        fs::write(source_path, crlf).unwrap();
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn byte_position(source: &[u8], offset: usize) -> (u32, u32) {
    assert!(offset <= source.len());
    let prefix = &source[..offset];
    let line_start = prefix
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let line = prefix.iter().filter(|byte| **byte == b'\n').count() + 1;
    (
        u32::try_from(line).unwrap(),
        u32::try_from(offset - line_start + 1).unwrap(),
    )
}

fn seed_symbol(path: &str, source: &[u8]) -> SeedSymbol {
    let declaration = b"pub fn seed";
    let declaration_start = source
        .windows(declaration.len())
        .position(|window| window == declaration)
        .expect("fixture must declare pub fn seed");
    let selection_start = declaration_start + b"pub fn ".len();
    let selection_end = selection_start + b"seed".len();
    let (start_line, start_column) = byte_position(source, selection_start);
    let (end_line, end_column) = byte_position(source, selection_end);
    let selection_range = ProviderRange {
        format: ProviderRangeFormat::Utf8ByteColumnsEndExclusiveV1,
        start_line,
        start_column,
        end_line,
        end_column,
        start_byte: selection_start,
        end_byte: selection_end,
    };
    SeedSymbol {
        changed_symbol_id: sha256(format!("{path}\0seed").as_bytes()),
        path: path.to_string(),
        kind: SeedKind::Function,
        name: "seed".to_string(),
        symbol_range: selection_range.clone(),
        selection_range,
        query_byte: selection_start + 1,
    }
}

struct RealRunHarness {
    repository: TempDir,
    assets: TempDir,
    scope_fingerprint: String,
    registry_path: PathBuf,
    registry_sha256: String,
    provider_id: String,
    model_path: PathBuf,
    model_sha256: String,
    request_path: PathBuf,
    profile: AuthorizedProviderProfile,
    runtime_temp_root: PathBuf,
    authorized_files: Vec<(PathBuf, String)>,
}

impl RealRunHarness {
    fn new(target_root: &Path, fixture: &str, seed_path: &str) -> Self {
        Self::new_with_request(target_root, fixture, seed_path, |_, _| {})
    }

    fn new_with_request(
        target_root: &Path,
        fixture: &str,
        seed_path: &str,
        configure: impl FnOnce(&mut ProviderRunRequest, &[u8]),
    ) -> Self {
        let target_root = fs::canonicalize(target_root).expect("real provider target must exist");
        let registry_path =
            fs::canonicalize(target_root.join("runtime/providers/provider-registry.json"))
                .expect("target-local provider registry must exist");
        assert!(registry_path.starts_with(&target_root));
        let registry_bytes = fs::read(&registry_path).unwrap();
        let registry: ProviderRegistry = serde_json::from_slice(&registry_bytes).unwrap();
        registry.validate().unwrap();
        let provider_id = "rust-analyzer-project-pack".to_string();
        let entry = registry.select(&provider_id).unwrap();
        let profile_path = fs::canonicalize(&entry.profile_path).unwrap();
        let executable_path = fs::canonicalize(&entry.executable_path).unwrap();
        assert!(profile_path.starts_with(&target_root));
        assert!(executable_path.starts_with(&target_root));
        let profile_bytes = fs::read(&profile_path).unwrap();
        let profile: AuthorizedProviderProfile = serde_json::from_slice(&profile_bytes).unwrap();
        profile.validate().unwrap();
        assert_eq!(entry.profile_sha256, sha256(&profile_bytes));
        assert_eq!(
            entry.executable_sha256,
            sha256(&fs::read(executable_path).unwrap())
        );
        assert_eq!(entry.provider_version, profile.provider_version);
        assert_eq!(entry.target_triple, profile.target_triple);
        assert!(profile.arguments.is_empty());

        let repository = git_repository();
        materialize_fixture(&fixture_root(fixture), repository.path());
        git(repository.path(), &["add", "--", "."]);
        let scope = open_authoritative_scope_bounded(
            ScopeRequest {
                repository: repository.path().to_path_buf(),
                source: Some(ReviewSource::Staged),
                expected_fingerprint: None,
            },
            Duration::from_secs(5),
        )
        .unwrap();
        let snapshot = CandidateSnapshot::materialize(
            repository.path(),
            ReviewSource::Staged,
            SnapshotLimits {
                max_files: 64,
                max_bytes: 256 * 1024,
            },
        )
        .unwrap();
        let model = build_linked_project_model(
            &snapshot,
            ProviderModelLimits {
                max_files: 64,
                max_bytes: 256 * 1024,
                max_file_bytes: 64 * 1024,
            },
        )
        .unwrap();
        model.validate().unwrap();
        assert_eq!(model.target_triple, profile.target_triple);

        let assets = TempDir::new().unwrap();
        let model_path = assets.path().join("model.json");
        let model_bytes = serde_json::to_vec(&model).unwrap();
        fs::write(&model_path, &model_bytes).unwrap();
        let model_path = fs::canonicalize(model_path).unwrap();

        let source = fs::read(snapshot.path().join(seed_path)).unwrap();
        let mut request = ProviderRunRequest {
            schema_version: 1,
            kind: "repository_context_provider_run_request".to_string(),
            seeds: vec![seed_symbol(seed_path, &source)],
            directions: vec![CallDirection::Incoming, CallDirection::Outgoing],
            limits: ProviderLimits {
                deadline_ms: 10_000,
                ..ProviderLimits::maximum()
            },
        };
        configure(&mut request, &source);
        request.validate_against(&profile.maximum_limits).unwrap();
        let request_path = assets.path().join("request.json");
        fs::write(&request_path, serde_json::to_vec(&request).unwrap()).unwrap();
        let request_path = fs::canonicalize(request_path).unwrap();
        let runtime_temp_root = assets.path().join("runtime-temp");
        fs::create_dir(&runtime_temp_root).unwrap();
        let authorized_files = [
            &registry_path,
            &profile_path,
            &entry.executable_path,
            &model_path,
            &request_path,
        ]
        .into_iter()
        .map(|path| (path.clone(), sha256(&fs::read(path).unwrap())))
        .collect();

        Self {
            repository,
            assets,
            scope_fingerprint: scope.fingerprint,
            registry_path,
            registry_sha256: sha256(&registry_bytes),
            provider_id,
            model_path,
            model_sha256: sha256(&model_bytes),
            request_path,
            profile,
            runtime_temp_root,
            authorized_files,
        }
    }

    fn run(&self) -> Output {
        self.run_with_additional_arguments(&[])
    }

    fn run_with_position_encoding(&self, encoding: PositionEncoding) -> Output {
        let encoding = match encoding {
            PositionEncoding::Utf8 => "utf-8",
            PositionEncoding::Utf16 => "utf-16",
        };
        self.run_with_additional_arguments(&["--test-position-encoding", encoding])
    }

    fn run_with_additional_arguments(&self, additional_arguments: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_repository-context-provider-cli"));
        command
            .args([
                "run",
                "--source",
                "staged",
                "--expect-scope",
                &self.scope_fingerprint,
                "--registry",
                self.registry_path.to_str().unwrap(),
                "--expect-registry-sha256",
                &self.registry_sha256,
                "--provider-id",
                &self.provider_id,
                "--model",
                self.model_path.to_str().unwrap(),
                "--expect-model-sha256",
                &self.model_sha256,
                "--request",
                self.request_path.to_str().unwrap(),
            ])
            .args(additional_arguments)
            .current_dir(self.repository.path())
            .env("TMPDIR", &self.runtime_temp_root)
            .env("TMP", &self.runtime_temp_root)
            .env("TEMP", &self.runtime_temp_root);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "real provider CLI failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stderr.is_empty(),
            "real provider CLI wrote stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            fs::read_dir(&self.runtime_temp_root)
                .unwrap()
                .next()
                .is_none(),
            "provider CLI left private runtime state behind"
        );
        for (path, expected_sha256) in &self.authorized_files {
            assert_eq!(sha256(&fs::read(path).unwrap()), *expected_sha256);
        }
        output
    }
}

fn normalized_report(report: RepositoryContextProviderReport) -> RepositoryContextProviderReport {
    RepositoryContextProviderReport {
        metrics: collect_diff_context_cli::repository_context_provider::contract::ProviderMetrics {
            elapsed_ms: 0,
            process_tree_peak_rss_bytes: 0,
            report_bytes: 0,
            ..report.metrics
        },
        ..report
    }
}

#[test]
fn repository_owned_real_fixtures_build_linked_projects_without_external_tooling() {
    for name in FIXTURES {
        let source = fixture_root(name);
        assert!(source.join("Cargo.toml").is_file(), "missing {name}");

        let repository = TempDir::new().unwrap();
        materialize_fixture(&source, repository.path());
        git(repository.path(), &["init", "-q"]);
        git(repository.path(), &["add", "--", "."]);

        let snapshot = CandidateSnapshot::materialize(
            repository.path(),
            ReviewSource::Staged,
            SnapshotLimits {
                max_files: 64,
                max_bytes: 256 * 1024,
            },
        )
        .unwrap();
        let model = build_linked_project_model(
            &snapshot,
            ProviderModelLimits {
                max_files: 64,
                max_bytes: 256 * 1024,
                max_file_bytes: 64 * 1024,
            },
        )
        .unwrap();

        model.validate().unwrap();
        assert!(!model.crates.is_empty(), "{name} has no linked crates");
        assert!(model
            .crates
            .iter()
            .all(|item| item.root_module.ends_with(".rs")));
        assert!(!source.join("build.rs").exists());
        assert!(!fs::read_to_string(source.join("Cargo.toml"))
            .unwrap()
            .contains("git ="));
    }
}

#[test]
fn real_fixture_inventory_covers_required_semantic_cases() {
    let single = fs::read_to_string(fixture_root("single_crate").join("src/lib.rs")).unwrap();
    assert!(single.contains("callee(value)"));
    assert!(single.contains("seed(41)"));

    let multi = fs::read_to_string(fixture_root("multi_crate").join("app/src/lib.rs")).unwrap();
    assert!(multi.contains("provider_real_shared::shared(value)"));

    let partial = fs::read_to_string(fixture_root("partial").join("src/lib.rs")).unwrap();
    assert!(partial.contains("dyn DynamicCall"));
    assert!(partial.contains("generated_call!(target.invoke())"));

    let unicode_root = fixture_root("unicode_crlf");
    assert_eq!(
        fs::read_to_string(unicode_root.join(".gitattributes")).unwrap(),
        "src/lib.rs -text\n"
    );
    let materialized = TempDir::new().unwrap();
    materialize_fixture(&unicode_root, materialized.path());
    let unicode = fs::read(materialized.path().join("src/lib.rs")).unwrap();
    assert!(unicode
        .windows("计算".len())
        .any(|bytes| bytes == "计算".as_bytes()));
    for (index, byte) in unicode.iter().enumerate() {
        if *byte == b'\n' {
            assert!(
                index > 0 && unicode[index - 1] == b'\r',
                "Unicode fixture must use CRLF"
            );
        }
    }

    let cycles = fs::read_to_string(fixture_root("cycles").join("src/lib.rs")).unwrap();
    assert!(cycles.contains("second(value - 1)"));
    assert!(cycles.contains("third(value)"));
    assert!(cycles.contains("first(value)"));
}

#[test]
fn normalized_real_single_crate_reports_are_byte_identical() {
    let Some(target_root) = env::var_os("PCR_REAL_PROVIDER_TARGET_ROOT") else {
        eprintln!("PCR_REAL_PROVIDER_TARGET_ROOT is not set; skipping real provider execution");
        return;
    };
    let harness = RealRunHarness::new(Path::new(&target_root), "single_crate", "src/lib.rs");
    let first: RepositoryContextProviderReport =
        serde_json::from_slice(&harness.run().stdout).unwrap();
    let second: RepositoryContextProviderReport =
        serde_json::from_slice(&harness.run().stdout).unwrap();
    for report in [&first, &second] {
        report.validate().unwrap();
        assert_eq!(
            report.status,
            RepositoryContextProviderStatus::Completed,
            "unexpected real-provider limitations: {:?}",
            report.limitations
        );
        assert_eq!(report.provider.kind, "rust-analyzer");
        assert_eq!(report.provider.version, harness.profile.provider_version);
        assert_eq!(report.provider.profile_sha256, harness.profile.sha256());
        assert_eq!(
            report.metrics.stderr_bytes, 0,
            "successful real-provider runs must suppress runtime-dependent diagnostics"
        );
        assert!(report
            .seed_symbols
            .iter()
            .any(|symbol| symbol.symbol.name == "seed"));
        let symbols = report
            .seed_symbols
            .iter()
            .map(|item| &item.symbol)
            .chain(report.related_symbols.iter())
            .map(|item| (item.symbol_id.as_str(), item.name.as_str()))
            .collect::<std::collections::BTreeMap<_, _>>();
        for (from, to) in [("caller", "seed"), ("seed", "callee")] {
            assert!(report.edges.iter().any(|edge| {
                symbols.get(edge.from_symbol.as_str()) == Some(&from)
                    && symbols.get(edge.to_symbol.as_str()) == Some(&to)
            }));
        }
    }
    let first = serde_json::to_vec(&normalized_report(first)).unwrap();
    let second = serde_json::to_vec(&normalized_report(second)).unwrap();
    assert_eq!(
        sha256(&first),
        sha256(&second),
        "normalized real-provider reports differ"
    );
    assert!(harness.assets.path().is_dir());
}

#[test]
fn real_multi_crate_report_contains_the_cross_crate_call_edge() {
    let Some(target_root) = env::var_os("PCR_REAL_PROVIDER_TARGET_ROOT") else {
        eprintln!("PCR_REAL_PROVIDER_TARGET_ROOT is not set; skipping real provider execution");
        return;
    };
    let harness = RealRunHarness::new(Path::new(&target_root), "multi_crate", "app/src/lib.rs");
    let report: RepositoryContextProviderReport =
        serde_json::from_slice(&harness.run().stdout).unwrap();
    report.validate().unwrap();
    assert_eq!(
        report.status,
        RepositoryContextProviderStatus::Completed,
        "unexpected real-provider limitations: {:?}",
        report.limitations
    );
    let symbols = report
        .seed_symbols
        .iter()
        .map(|item| &item.symbol)
        .chain(report.related_symbols.iter())
        .map(|item| {
            (
                item.symbol_id.as_str(),
                (item.name.as_str(), item.path.as_str()),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert!(report.edges.iter().any(|edge| {
        symbols.get(edge.from_symbol.as_str()) == Some(&("seed", "app/src/lib.rs"))
            && symbols.get(edge.to_symbol.as_str()) == Some(&("shared", "shared/src/lib.rs"))
    }));
}

#[test]
fn real_unicode_crlf_report_negotiates_utf16_and_preserves_the_unicode_call_edge() {
    let Some(target_root) = env::var_os("PCR_REAL_PROVIDER_TARGET_ROOT") else {
        eprintln!("PCR_REAL_PROVIDER_TARGET_ROOT is not set; skipping real provider execution");
        return;
    };
    let harness = RealRunHarness::new(Path::new(&target_root), "unicode_crlf", "src/lib.rs");
    let report: RepositoryContextProviderReport = serde_json::from_slice(
        &harness
            .run_with_position_encoding(PositionEncoding::Utf16)
            .stdout,
    )
    .unwrap();
    report.validate().unwrap();
    assert_eq!(
        report.status,
        RepositoryContextProviderStatus::Completed,
        "unexpected real-provider limitations: {:?}",
        report.limitations
    );
    assert_eq!(
        report.provider.negotiated_encoding,
        Some(PositionEncoding::Utf16)
    );
    let symbols = report
        .seed_symbols
        .iter()
        .map(|item| &item.symbol)
        .chain(report.related_symbols.iter())
        .map(|item| (item.symbol_id.as_str(), item.name.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert!(report.edges.iter().any(|edge| {
        symbols.get(edge.from_symbol.as_str()) == Some(&"seed")
            && symbols.get(edge.to_symbol.as_str()) == Some(&"计算")
    }));
}

#[test]
fn real_cycles_report_retains_depth_two_edges_without_duplicates() {
    let Some(target_root) = env::var_os("PCR_REAL_PROVIDER_TARGET_ROOT") else {
        eprintln!("PCR_REAL_PROVIDER_TARGET_ROOT is not set; skipping real provider execution");
        return;
    };
    let harness = RealRunHarness::new(Path::new(&target_root), "cycles", "src/lib.rs");
    let report: RepositoryContextProviderReport =
        serde_json::from_slice(&harness.run().stdout).unwrap();
    report.validate().unwrap();
    assert_eq!(
        report.status,
        RepositoryContextProviderStatus::Completed,
        "unexpected real-provider limitations: {:?}",
        report.limitations
    );
    let symbols = report
        .seed_symbols
        .iter()
        .map(|item| &item.symbol)
        .chain(report.related_symbols.iter())
        .map(|item| (item.symbol_id.as_str(), item.name.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (from, to) in [("seed", "first"), ("first", "second")] {
        assert!(report.edges.iter().any(|edge| {
            symbols.get(edge.from_symbol.as_str()) == Some(&from)
                && symbols.get(edge.to_symbol.as_str()) == Some(&to)
        }));
    }
    assert_eq!(
        report.edges.len(),
        report
            .edges
            .iter()
            .map(|edge| edge.edge_id.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );
}

#[test]
fn real_cycles_respect_depth_one_and_requested_fact_budgets() {
    let Some(target_root) = env::var_os("PCR_REAL_PROVIDER_TARGET_ROOT") else {
        eprintln!("PCR_REAL_PROVIDER_TARGET_ROOT is not set; skipping real provider execution");
        return;
    };
    let harness = RealRunHarness::new_with_request(
        Path::new(&target_root),
        "cycles",
        "src/lib.rs",
        |request, _| {
            request.limits.max_depth = 1;
            request.limits.max_nodes = 2;
            request.limits.max_edges = 1;
            request.limits.max_call_ranges = 1;
            request.limits.max_report_bytes = 16 * 1024;
        },
    );
    let output = harness.run();
    assert!(output.stdout.len() <= 16 * 1024);
    let report: RepositoryContextProviderReport = serde_json::from_slice(&output.stdout).unwrap();
    report.validate().unwrap();
    assert!(report.metrics.nodes <= 2);
    assert!(report.metrics.edges <= 1);
    assert!(report.metrics.call_ranges <= 1);
    assert!(report.metrics.report_bytes <= 16 * 1024);
    let symbols = report
        .seed_symbols
        .iter()
        .map(|item| &item.symbol)
        .chain(report.related_symbols.iter())
        .map(|item| (item.symbol_id.as_str(), item.name.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert!(report.edges.iter().any(|edge| {
        symbols.get(edge.from_symbol.as_str()) == Some(&"seed")
            && symbols.get(edge.to_symbol.as_str()) == Some(&"first")
    }));
    assert!(!report.edges.iter().any(|edge| {
        symbols.get(edge.from_symbol.as_str()) == Some(&"first")
            && symbols.get(edge.to_symbol.as_str()) == Some(&"second")
    }));
}

#[test]
fn real_stale_seed_range_is_honestly_partial_without_dangling_symbol_binding() {
    let Some(target_root) = env::var_os("PCR_REAL_PROVIDER_TARGET_ROOT") else {
        eprintln!("PCR_REAL_PROVIDER_TARGET_ROOT is not set; skipping real provider execution");
        return;
    };
    let harness = RealRunHarness::new_with_request(
        Path::new(&target_root),
        "single_crate",
        "src/lib.rs",
        |request, source| {
            let seed = request.seeds.first_mut().unwrap();
            let (end_line, end_column) = byte_position(source, source.len());
            seed.symbol_range = ProviderRange {
                format: ProviderRangeFormat::Utf8ByteColumnsEndExclusiveV1,
                start_line: 1,
                start_column: 1,
                end_line,
                end_column,
                start_byte: 0,
                end_byte: source.len(),
            };
        },
    );
    let report: RepositoryContextProviderReport =
        serde_json::from_slice(&harness.run().stdout).unwrap();
    report.validate().unwrap();
    assert_eq!(report.status, RepositoryContextProviderStatus::Partial);
    assert!(report.seed_symbols.is_empty());
    assert!(report.related_symbols.is_empty());
    assert!(report.edges.is_empty());
    let unresolved = report
        .limitations
        .iter()
        .find(|item| item.code == "seed-unresolved")
        .expect("stale seed range must be reported as unresolved");
    assert!(unresolved.changed_symbol_id.is_none());
    assert_eq!(unresolved.path.as_deref(), Some("src/lib.rs"));
}

#[test]
fn real_dynamic_macro_report_is_honestly_partial() {
    let Some(target_root) = env::var_os("PCR_REAL_PROVIDER_TARGET_ROOT") else {
        eprintln!("PCR_REAL_PROVIDER_TARGET_ROOT is not set; skipping real provider execution");
        return;
    };
    let harness = RealRunHarness::new(Path::new(&target_root), "partial", "src/lib.rs");
    let report: RepositoryContextProviderReport =
        serde_json::from_slice(&harness.run().stdout).unwrap();
    report.validate().unwrap();
    assert_eq!(
        report.status,
        RepositoryContextProviderStatus::Partial,
        "dynamic/macro fixture must not claim complete traversal: {:?}",
        report
    );
    assert!(!report.limitations.is_empty());
    let limitation_codes = report
        .limitations
        .iter()
        .map(|item| item.code.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(limitation_codes.contains("dynamic-dispatch-partial"));
    assert!(limitation_codes.contains("macro-invocation-partial"));
    let symbols = report
        .seed_symbols
        .iter()
        .map(|item| &item.symbol)
        .chain(report.related_symbols.iter())
        .map(|item| (item.symbol_id.as_str(), item.name.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert!(report.edges.iter().any(|edge| {
        symbols.get(edge.from_symbol.as_str()) == Some(&"caller")
            && symbols.get(edge.to_symbol.as_str()) == Some(&"seed")
    }));
}
