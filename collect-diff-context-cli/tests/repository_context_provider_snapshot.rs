use collect_diff_context_cli::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
use collect_diff_context_cli::repository_context_provider::contract::{
    CandidateBinding, PositionEncoding, ProviderRange, RustAnalyzerCrate, RustAnalyzerDependency,
    RustAnalyzerProjectModel,
};
use collect_diff_context_cli::repository_context_provider::snapshot::{
    BoundCandidateSnapshot, LspPosition, LspRange, SnapshotFilePath, SnapshotSourceBudget,
    SnapshotUriMapper, SourceDocument,
};
use collect_diff_context_cli::review_scope::ReviewSource;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use tempfile::TempDir;
use url::Url;

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct ProviderFixture {
    _repository: TempDir,
    snapshot: CandidateSnapshot,
    model: RustAnalyzerProjectModel,
    binding: CandidateBinding,
}

impl ProviderFixture {
    fn new() -> Self {
        Self::with_configuration(None)
    }

    fn with_configuration(configuration: Option<&str>) -> Self {
        let repository = TempDir::new().unwrap();
        git(repository.path(), &["init", "-q"]);
        fs::create_dir_all(repository.path().join("src")).unwrap();
        fs::create_dir_all(repository.path().join("vendor")).unwrap();
        fs::write(
            repository.path().join("src/lib.rs"),
            b"pub fn seed() { dependency(); }\n",
        )
        .unwrap();
        fs::write(
            repository.path().join("vendor/dep.rs"),
            b"pub fn dependency() {}\n",
        )
        .unwrap();
        if let Some(path) = configuration {
            let path = repository.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"[workspace]\n").unwrap();
        }
        git(repository.path(), &["add", "--", "."]);

        let snapshot = CandidateSnapshot::materialize(
            repository.path(),
            ReviewSource::Staged,
            SnapshotLimits {
                max_files: 100,
                max_bytes: 1_000_000,
            },
        )
        .unwrap();
        let mut model = RustAnalyzerProjectModel {
            schema_version: 1,
            algorithm: "rust-analyzer-linked-project-v1".to_string(),
            digest: digest('0'),
            target_triple: "x86_64-unknown-linux-gnu".to_string(),
            crates: vec![
                RustAnalyzerCrate {
                    crate_id: "app".to_string(),
                    root_module: "src/lib.rs".to_string(),
                    edition: "2021".to_string(),
                    dependencies: vec![RustAnalyzerDependency {
                        crate_id: "dependency".to_string(),
                        name: "dependency".to_string(),
                    }],
                },
                RustAnalyzerCrate {
                    crate_id: "dependency".to_string(),
                    root_module: "vendor/dep.rs".to_string(),
                    edition: "2021".to_string(),
                    dependencies: Vec::new(),
                },
            ],
            cfg: vec!["feature=\"provider\"".to_string(), "unix".to_string()],
            env: BTreeMap::from([("CARGO_PKG_NAME".to_string(), "fixture".to_string())]),
            limitations: vec!["build-scripts-disabled".to_string()],
        };
        model.digest = model.canonical_sha256();
        let binding = CandidateBinding {
            source: ReviewSource::Staged,
            scope_fingerprint: digest('1'),
            candidate_digest: digest('2'),
            snapshot_root: fs::canonicalize(snapshot.path()).unwrap(),
            snapshot_sha256: snapshot.sha256.clone(),
            snapshot_files: snapshot.files,
            snapshot_bytes: snapshot.bytes,
            project_model_digest: model.digest.clone(),
        };
        Self {
            _repository: repository,
            snapshot,
            model,
            binding,
        }
    }

    fn bound(&self) -> BoundCandidateSnapshot<'_> {
        BoundCandidateSnapshot::new(&self.snapshot, &self.model, &self.binding).unwrap()
    }
}

#[test]
fn bound_view_requires_the_exact_materialized_snapshot_and_model() {
    let fixture = ProviderFixture::new();
    let bound = fixture.bound();
    assert_eq!(
        bound.root(),
        fs::canonicalize(fixture.snapshot.path()).unwrap()
    );
    assert_eq!(bound.model().digest, fixture.model.digest);
    assert_eq!(
        bound.reported_binding().snapshot_sha256,
        fixture.snapshot.sha256
    );

    let mut lexical_root = fixture.binding.clone();
    lexical_root.snapshot_root = fixture.snapshot.path().to_path_buf();
    assert!(BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &lexical_root).is_ok());

    let mut changed = fixture.binding.clone();
    changed.snapshot_sha256 = digest('9');
    assert!(BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &changed).is_err());

    let mut changed = fixture.binding.clone();
    changed.snapshot_files += 1;
    assert!(BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &changed).is_err());

    let mut changed = fixture.binding.clone();
    changed.snapshot_bytes += 1;
    assert!(BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &changed).is_err());

    let mut changed = fixture.binding.clone();
    changed.project_model_digest = digest('9');
    assert!(BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &changed).is_err());

    let mut changed = fixture.binding.clone();
    changed.source = ReviewSource::Branch;
    assert!(BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &changed).is_err());

    let mut changed = fixture.binding.clone();
    changed.scope_fingerprint = "short".to_string();
    assert!(BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &changed).is_err());

    let mut changed = fixture.binding.clone();
    changed.candidate_digest = digest('A');
    assert!(BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &changed).is_err());
}

#[test]
fn bound_view_rejects_missing_model_roots_and_repository_configuration() {
    let fixture = ProviderFixture::new();
    let mut missing = fixture.model.clone();
    missing.crates[0].root_module = "src/missing.rs".to_string();
    missing.digest = missing.canonical_sha256();
    let mut binding = fixture.binding.clone();
    binding.project_model_digest = missing.digest.clone();
    assert!(BoundCandidateSnapshot::new(&fixture.snapshot, &missing, &binding).is_err());

    for configuration in ["rust-analyzer.toml", "nested/rust-analyzer.toml"] {
        let fixture = ProviderFixture::with_configuration(Some(configuration));
        let error =
            BoundCandidateSnapshot::new(&fixture.snapshot, &fixture.model, &fixture.binding)
                .unwrap_err();
        assert_eq!(error.code, "provider-snapshot-configuration-forbidden");
    }
}

#[test]
fn source_path_and_budget_reject_escape_vcs_directory_non_rust_and_oversize() {
    let fixture = ProviderFixture::new();
    let bound = fixture.bound();
    assert!(SnapshotFilePath::new("../escape.rs").is_err());
    assert!(SnapshotFilePath::new("src//lib.rs").is_err());
    assert!(SnapshotFilePath::new(".git/config").is_err());
    assert!(SnapshotFilePath::new(".GIT/config").is_err());
    assert!(SnapshotFilePath::new("src/line\nfeed.rs").is_err());

    let directory = SnapshotFilePath::new("src").unwrap();
    let mut budget = SnapshotSourceBudget::new(1_000, 1_000).unwrap();
    assert!(bound.read_source(&directory, &mut budget).is_err());

    let source = SnapshotFilePath::new("src/lib.rs").unwrap();
    let mut file_budget = SnapshotSourceBudget::new(1, 1_000).unwrap();
    assert!(bound.read_source(&source, &mut file_budget).is_err());
    let mut total_budget = SnapshotSourceBudget::new(1_000, 1).unwrap();
    assert!(bound.read_source(&source, &mut total_budget).is_err());

    assert!(SnapshotSourceBudget::new(0, 1).is_err());
    assert!(SnapshotSourceBudget::new(2, 1).is_ok());
}

#[test]
fn source_reads_valid_utf8_rust_once_with_deterministic_accounting() {
    let fixture = ProviderFixture::new();
    let bound = fixture.bound();
    let source = SnapshotFilePath::new("src/lib.rs").unwrap();
    let expected = fs::read(fixture.snapshot.path().join("src/lib.rs")).unwrap();
    let mut budget = SnapshotSourceBudget::new(expected.len(), expected.len()).unwrap();

    let observed = bound.read_source(&source, &mut budget).unwrap();
    assert_eq!(observed.as_ref(), expected);
    assert_eq!(budget.remaining_bytes(), 0);
    assert!(bound.read_source(&source, &mut budget).is_err());
}

#[test]
fn linked_project_json_is_canonical_and_digest_bound() {
    let fixture = ProviderFixture::new();
    let linked = fixture.model.linked_project_value().unwrap();
    assert_eq!(linked["sysroot_src"], serde_json::Value::Null);
    assert_eq!(linked["crates"][0]["root_module"], "src/lib.rs");
    assert_eq!(linked["crates"][0]["deps"][0]["crate"], 1);
    assert_eq!(linked["crates"][0]["deps"][0]["name"], "dependency");
    assert_eq!(
        linked["crates"][0]["cfg"],
        serde_json::json!(["feature=\"provider\"", "unix"])
    );
    assert_eq!(linked["crates"][0]["env"]["CARGO_PKG_NAME"], "fixture");
    assert_eq!(linked["crates"][0]["target"], "x86_64-unknown-linux-gnu");

    for field in [
        "root",
        "edition",
        "cfg",
        "dependency",
        "target",
        "limitation",
    ] {
        let mut changed = fixture.model.clone();
        match field {
            "root" => changed.crates[0].root_module = "vendor/dep.rs".to_string(),
            "edition" => changed.crates[0].edition = "2024".to_string(),
            "cfg" => changed.cfg.push("windows".to_string()),
            "dependency" => changed.crates[0].dependencies[0].name = "renamed".to_string(),
            "target" => changed.target_triple = "aarch64-apple-darwin".to_string(),
            "limitation" => changed.limitations.push("proc-macros-disabled".to_string()),
            _ => unreachable!(),
        }
        assert!(
            BoundCandidateSnapshot::new(&fixture.snapshot, &changed, &fixture.binding).is_err(),
            "{field}"
        );
    }
}

#[test]
fn file_uri_mapper_accepts_only_contained_regular_snapshot_files() {
    let fixture = ProviderFixture::new();
    let mapper = SnapshotUriMapper::new(fixture.snapshot.path()).unwrap();
    let path = SnapshotFilePath::new("src/lib.rs").unwrap();
    let uri = mapper.to_file_uri(&path).unwrap();
    assert_eq!(mapper.to_file_path(&uri).unwrap(), path);

    let root_uri = Url::from_file_path(fixture.snapshot.path()).unwrap();
    let error = mapper.to_file_path(&root_uri).unwrap_err();
    assert_eq!(error.code, "provider-uri-outside-snapshot");

    let directory_uri = Url::from_file_path(fixture.snapshot.path().join("src")).unwrap();
    assert_eq!(
        mapper.to_file_path(&directory_uri).unwrap_err().code,
        "provider-uri-invalid"
    );

    let missing_uri = Url::from_file_path(fixture.snapshot.path().join("src/missing.rs")).unwrap();
    assert_eq!(
        mapper.to_file_path(&missing_uri).unwrap_err().code,
        "provider-uri-stale"
    );

    let mut query = uri.clone();
    query.set_query(Some("query"));
    assert_eq!(
        mapper.to_file_path(&query).unwrap_err().code,
        "provider-uri-invalid"
    );
    let mut fragment = uri.clone();
    fragment.set_fragment(Some("fragment"));
    assert_eq!(
        mapper.to_file_path(&fragment).unwrap_err().code,
        "provider-uri-invalid"
    );

    let credentials = Url::parse("https://user:pass@example.test/src/lib.rs").unwrap();
    assert_eq!(
        mapper.to_file_path(&credentials).unwrap_err().code,
        "provider-uri-invalid"
    );
    let authority = Url::parse("file://example.test/src/lib.rs").unwrap();
    assert_eq!(authority.host_str(), Some("example.test"));
    assert_eq!(
        mapper.to_file_path(&authority).unwrap_err().code,
        "provider-uri-invalid"
    );
    let non_file = Url::parse("https://example.test/src/lib.rs").unwrap();
    assert_eq!(
        mapper.to_file_path(&non_file).unwrap_err().code,
        "provider-uri-invalid"
    );

    let outside =
        Url::from_file_path(fixture.snapshot.path().parent().unwrap().join("escape.rs")).unwrap();
    assert_eq!(
        mapper.to_file_path(&outside).unwrap_err().code,
        "provider-uri-outside-snapshot"
    );

    let duplicate = Url::parse(&format!(
        "file://{}//src/lib.rs",
        uri.path().trim_end_matches("/src/lib.rs")
    ))
    .unwrap();
    assert_eq!(
        mapper.to_file_path(&duplicate).unwrap_err().code,
        "provider-uri-invalid"
    );

    let dot = Url::parse(&format!(
        "file://{}/%2e%2e/escape.rs",
        uri.path().trim_end_matches("/src/lib.rs")
    ))
    .unwrap();
    assert_eq!(
        mapper.to_file_path(&dot).unwrap_err().code,
        "provider-uri-outside-snapshot"
    );

    let trailing = Url::parse(&format!("{}/", uri.as_str())).unwrap();
    assert_eq!(
        mapper.to_file_path(&trailing).unwrap_err().code,
        "provider-uri-invalid"
    );
}

#[cfg(unix)]
#[test]
fn file_uri_mapper_rejects_stale_symlinks_and_non_utf8_paths() {
    use std::os::unix::fs::symlink;

    let directory = TempDir::new().unwrap();
    fs::create_dir(directory.path().join("root")).unwrap();
    fs::write(directory.path().join("root/target.rs"), b"fn target() {}\n").unwrap();
    symlink("target.rs", directory.path().join("root/link.rs")).unwrap();
    let mapper = SnapshotUriMapper::new(&directory.path().join("root")).unwrap();
    fs::remove_file(directory.path().join("root/target.rs")).unwrap();
    let stale = Url::from_file_path(directory.path().join("root/link.rs")).unwrap();
    assert_eq!(
        mapper.to_file_path(&stale).unwrap_err().code,
        "provider-uri-stale"
    );

    let root_uri = Url::from_file_path(directory.path().join("root")).unwrap();
    let invalid_uri = Url::parse(&format!(
        "file://{}/invalid%FF.rs",
        root_uri.path().trim_end_matches('/')
    ))
    .unwrap();
    assert_eq!(
        mapper.to_file_path(&invalid_uri).unwrap_err().code,
        "provider-uri-non-utf8"
    );
}

#[cfg(windows)]
#[test]
fn file_uri_mapper_accepts_windows_file_uri_round_trip() {
    let fixture = ProviderFixture::new();
    let mapper = SnapshotUriMapper::new(fixture.snapshot.path()).unwrap();
    let path = SnapshotFilePath::new("src/lib.rs").unwrap();
    let uri = mapper.to_file_uri(&path).unwrap();
    assert_eq!(uri.scheme(), "file");
    assert_eq!(mapper.to_file_path(&uri).unwrap(), path);
}

#[test]
fn utf8_and_utf16_map_to_the_same_provider_bytes() {
    let document = SourceDocument::new(Arc::from("a😀z\r\nβ\n".as_bytes())).unwrap();
    let utf8 = document
        .lsp_range_to_provider(LspRange::new(0, 1, 0, 5), PositionEncoding::Utf8)
        .unwrap();
    let utf16 = document
        .lsp_range_to_provider(LspRange::new(0, 1, 0, 3), PositionEncoding::Utf16)
        .unwrap();
    assert_eq!(utf8, utf16);
    assert_eq!((utf8.start_byte, utf8.end_byte), (1, 5));
    assert!(
        document
            .lsp_to_byte(LspPosition::new(0, 99), PositionEncoding::Utf8)
            .unwrap()
            .1
    );
}

#[test]
fn source_document_handles_line_endings_eof_and_round_trip_ranges() {
    let document = SourceDocument::new(Arc::from("a\r\nb\rc\n".as_bytes())).unwrap();
    let range = document
        .lsp_range_to_provider(LspRange::new(0, 0, 1, 0), PositionEncoding::Utf8)
        .unwrap();
    assert_eq!((range.start_byte, range.end_byte), (0, 3));
    assert_eq!(range.start_line, 1);
    assert_eq!(range.end_line, 2);
    assert_eq!(range.end_column, 1);
    let round_trip = document
        .provider_range_to_lsp(&range, PositionEncoding::Utf16)
        .unwrap();
    assert_eq!(round_trip, LspRange::new(0, 0, 1, 0));

    let final_line = document
        .lsp_to_byte(LspPosition::new(3, 0), PositionEncoding::Utf8)
        .unwrap();
    assert_eq!(final_line, (7, false));
}

#[test]
fn source_document_rejects_mid_codepoint_reversed_invalid_and_normalized_ranges() {
    let document = SourceDocument::new(Arc::from("a😀z\n".as_bytes())).unwrap();
    assert!(document
        .lsp_to_byte(LspPosition::new(0, 2), PositionEncoding::Utf8)
        .is_err());
    assert!(document
        .lsp_to_byte(LspPosition::new(0, 2), PositionEncoding::Utf16)
        .is_err());
    assert!(document
        .lsp_to_byte(LspPosition::new(4, 0), PositionEncoding::Utf8)
        .is_err());
    assert!(document
        .lsp_range_to_provider(LspRange::new(0, 3, 0, 1), PositionEncoding::Utf8,)
        .is_err());
    let normalized = document
        .lsp_range_to_provider(LspRange::new(0, 0, 0, 99), PositionEncoding::Utf8)
        .unwrap_err();
    assert_eq!(normalized.code, "provider-position-normalized");
    assert!(SourceDocument::new(Arc::from([0xff_u8].as_slice())).is_err());

    let invalid_provider = ProviderRange {
        format: collect_diff_context_cli::repository_context_provider::contract::ProviderRangeFormat::Utf8ByteColumnsEndExclusiveV1,
        start_line: 1,
        start_column: 1,
        end_line: 1,
        end_column: 99,
        start_byte: 0,
        end_byte: 1,
    };
    assert!(document
        .provider_range_to_lsp(&invalid_provider, PositionEncoding::Utf8)
        .is_err());
}
