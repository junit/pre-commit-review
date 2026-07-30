use collect_diff_context_cli::artifacts::contract::{
    canonical_json, sha256_bytes, ArtifactBaseline, ArtifactFileBinding, ArtifactManifest,
    ArtifactPackRecord, ArtifactRole, ArtifactState, BaselineMeasurement, PackFormat, ProbeId,
    SourceAssetRecord, SourceLock,
};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};

const RUST_ANALYZER_SOURCE_LOCK_SHA256: &str =
    "82ee6473601fba11e01fc37f60ee48f0634bfa1f24f3d01714119cfadf84b742";
const PROVIDER_PACK_VERSION: &str = "2026.07.27-pcr.1";
const EXPECTED_VERSION_OUTPUT: &str = "rust-analyzer 0.3.2989-standalone (12c3381f0b 2026-07-26)";

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn source_asset(
    platform_id: &str,
    target_triple: &str,
    archive: (&str, u64, &str),
    executable: (&str, u64, &str),
) -> SourceAssetRecord {
    let (archive_name, archive_size, archive_sha256) = archive;
    let (executable_name, executable_size, executable_sha256) = executable;
    SourceAssetRecord {
        platform_id: platform_id.to_string(),
        target_triple: target_triple.to_string(),
        url: format!(
            "https://github.com/rust-lang/rust-analyzer/releases/download/2026-07-27/{archive_name}"
        ),
        archive_name: archive_name.to_string(),
        archive_size,
        archive_sha256: archive_sha256.to_string(),
        executable_name: executable_name.to_string(),
        executable_size,
        executable_sha256: executable_sha256.to_string(),
        expected_version_output: EXPECTED_VERSION_OUTPUT.to_string(),
        license_source_paths: vec!["LICENSE-APACHE".to_string(), "LICENSE-MIT".to_string()],
    }
}

fn expected_source_lock() -> SourceLock {
    SourceLock {
        schema_version: 1,
        kind: "third_party_sources".to_string(),
        artifact_id: "rust-analyzer".to_string(),
        tool_version: "2026-07-27".to_string(),
        upstream_repository: "rust-lang/rust-analyzer".to_string(),
        upstream_tag: "2026-07-27".to_string(),
        upstream_commit: "12c3381f0b17b8eec21075d1c72fd010996a9bda".to_string(),
        assets: vec![
            source_asset(
                "darwin-amd64",
                "x86_64-apple-darwin",
                (
                    "rust-analyzer-x86_64-apple-darwin.gz",
                    14_715_786,
                    "9d1a60991ead6c27baa9d265fc8fd03bba9c39cf0ec2aaf389e37e6155af7cbb",
                ),
                (
                    "rust-analyzer",
                    39_729_020,
                    "01ed4388725ef878a8682ab086749b8c9f3dfa76cf9ac9a7b173add6075236b3",
                ),
            ),
            source_asset(
                "darwin-arm64",
                "aarch64-apple-darwin",
                (
                    "rust-analyzer-aarch64-apple-darwin.gz",
                    13_987_778,
                    "102215ae7e7a41c0dda8f24e910a01e757f58091204863e5e3e6696b743f7e97",
                ),
                (
                    "rust-analyzer",
                    38_192_576,
                    "c4e9a82238092144191799a0631d21927ea75b8cbf245f79b51d1e89ca9fd760",
                ),
            ),
            source_asset(
                "linux-amd64",
                "x86_64-unknown-linux-musl",
                (
                    "rust-analyzer-x86_64-unknown-linux-musl.gz",
                    15_070_124,
                    "4793930e0fe32f18ed7e8e689df3ebb03b632f76c16625c44754fb42ce39fc72",
                ),
                (
                    "rust-analyzer",
                    44_889_000,
                    "bf809712906c99b4056e19d05fbd42d51804a045f64bd211df9bc29ad2776eb6",
                ),
            ),
            source_asset(
                "windows-amd64",
                "x86_64-pc-windows-msvc",
                (
                    "rust-analyzer-x86_64-pc-windows-msvc.zip",
                    17_612_036,
                    "7abdf50734026de963b3b25eba7714be8acf43a15ffb7f4f9d8b041e796ce2c9",
                ),
                (
                    "rust-analyzer.exe",
                    38_694_912,
                    "61ad88c3c90a5dece93f590aa31407f69be96023a2536a4f0285bd3def9cb278",
                ),
            ),
        ],
    }
}

fn source_lock_with_asset_mutation(
    index: usize,
    mutate: impl FnOnce(&mut SourceAssetRecord),
) -> SourceLock {
    let mut lock = expected_source_lock();
    mutate(&mut lock.assets[index]);
    lock
}

fn source_lock_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../third_party_artifacts/sources/rust-analyzer-2026-07-27.json")
}

fn distribution_manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../third_party_artifacts/manifest.json")
}

fn provider_record(source_lock_sha256: &str) -> ArtifactPackRecord {
    ArtifactPackRecord {
        artifact_id: "rust-analyzer".to_string(),
        artifact_role: ArtifactRole::RepositoryContextProvider,
        tool_version: "2026-07-27".to_string(),
        upstream_repository: "rust-lang/rust-analyzer".to_string(),
        upstream_tag: "2026-07-27".to_string(),
        upstream_commit: "12c3381f0b17b8eec21075d1c72fd010996a9bda".to_string(),
        source_lock_sha256: source_lock_sha256.to_string(),
        platform_id: "linux-amd64".to_string(),
        target_triple: "x86_64-unknown-linux-musl".to_string(),
        state: ArtifactState::Active,
        pack_version: PROVIDER_PACK_VERSION.to_string(),
        project_release_tag: "artifact-rust-analyzer-2026.07.27-pcr.1".to_string(),
        project_asset_name: "pre-commit-review-rust-analyzer-2026.07.27-pcr.1-linux-amd64.tar.gz"
            .to_string(),
        expected_compressed_size: 16 * 1024 * 1024,
        max_compressed_size: 32 * 1024 * 1024,
        pack_sha256: digest('1'),
        pack_manifest_sha256: digest('2'),
        sbom_sha256: digest('3'),
        pack_format: PackFormat::NormalizedTarGzipV1,
        executable: ArtifactFileBinding {
            path: "bin/rust-analyzer".to_string(),
            size: 44_889_000,
            sha256: "bf809712906c99b4056e19d05fbd42d51804a045f64bd211df9bc29ad2776eb6".to_string(),
        },
        version_probe: ProbeId::RustAnalyzerVersionV1,
        capability_probe: ProbeId::RustAnalyzerStdioV1,
        expected_version: EXPECTED_VERSION_OUTPUT.to_string(),
        license_component: "rust-analyzer".to_string(),
        license_files: vec![
            ArtifactFileBinding {
                path: "licenses/LICENSE-APACHE".to_string(),
                size: 11_358,
                sha256: digest('4'),
            },
            ArtifactFileBinding {
                path: "licenses/LICENSE-MIT".to_string(),
                size: 1_080,
                sha256: digest('5'),
            },
        ],
        sbom_component: "pkg:github/rust-lang/rust-analyzer@2026-07-27".to_string(),
        default_configuration_sha256: None,
        quality_baseline_sha256: Some(digest('6')),
        revoked_reason: None,
        replacement_pack_version: None,
    }
}

fn provider_manifest(record: ArtifactPackRecord) -> ArtifactManifest {
    ArtifactManifest {
        schema_version: 1,
        kind: "third_party_artifacts".to_string(),
        release_repository: "junit/pre-commit-review".to_string(),
        revocation_index_sha256: digest('0'),
        packs: vec![record],
    }
}

fn rejection(record: ArtifactPackRecord) -> &'static str {
    provider_manifest(record).validate().unwrap_err().code
}

#[test]
fn canonical_rust_analyzer_source_lock_binds_reviewed_release_inputs() {
    let bytes = fs::read(source_lock_path()).unwrap();
    assert!(!bytes.ends_with(b"\n"));
    let lock: SourceLock = serde_json::from_slice(&bytes).unwrap();
    lock.validate().unwrap();
    assert_eq!(canonical_json(&lock).unwrap(), bytes);
    assert_eq!(sha256_bytes(&bytes), RUST_ANALYZER_SOURCE_LOCK_SHA256);
    assert_eq!(lock, expected_source_lock());

    for expected in &expected_source_lock().assets {
        assert_eq!(
            lock.assets
                .iter()
                .find(|asset| asset.platform_id == expected.platform_id)
                .unwrap(),
            expected,
            "wrong source asset locked for {}",
            expected.platform_id
        );
    }
}

#[test]
fn unpublished_provider_records_are_absent_from_the_distribution_manifest() {
    let bytes = fs::read(distribution_manifest_path()).unwrap();
    let manifest: ArtifactManifest = serde_json::from_slice(&bytes).unwrap();
    assert!(manifest.packs.iter().all(|record| {
        record.artifact_id != "rust-analyzer" || record.state != ArtifactState::Active
    }));
}

#[test]
fn source_lock_rejects_moving_untrusted_or_ambiguous_inputs() {
    for moving_tag in ["latest", "nightly"] {
        let mut lock = expected_source_lock();
        lock.upstream_tag = moving_tag.to_string();
        assert_eq!(lock.validate().unwrap_err().code, "source-tag-policy");
    }

    let mut arbitrary_host = expected_source_lock();
    arbitrary_host.assets[0].url = "https://example.invalid/rust-analyzer.gz".to_string();
    assert_eq!(
        arbitrary_host.validate().unwrap_err().code,
        "source-url-policy"
    );

    for unsafe_url in [
        "https://github.com/rust-lang/rust-analyzer/releases/download/2026-07-27/rust-analyzer-x86_64-apple-darwin.gz?download=1",
        "https://github.com/rust-lang/rust-analyzer/releases/download/{tag}/{asset}",
    ] {
        let mut lock = expected_source_lock();
        lock.assets[0].url = unsafe_url.to_string();
        assert_eq!(lock.validate().unwrap_err().code, "source-url-policy");
    }

    let mut changed_target = expected_source_lock();
    changed_target.assets[2].target_triple = "x86_64-unknown-linux-gnu".to_string();
    assert_eq!(
        changed_target.validate().unwrap_err().code,
        "platform-target-mismatch"
    );

    let mut duplicate_platform = expected_source_lock();
    duplicate_platform.assets[1] = duplicate_platform.assets[0].clone();
    assert_eq!(
        duplicate_platform.validate().unwrap_err().code,
        "source-assets-not-sorted"
    );

    let mut missing_executable_hash = expected_source_lock();
    missing_executable_hash.assets[0].executable_sha256.clear();
    assert_eq!(
        missing_executable_hash.validate().unwrap_err().code,
        "invalid-sha256"
    );
}

#[test]
fn rust_analyzer_source_lock_rejects_reviewed_release_identity_drift() {
    let mut wrong_tool_version = expected_source_lock();
    wrong_tool_version.tool_version = "2026-07-28".to_string();
    assert_eq!(
        wrong_tool_version.validate().unwrap_err().code,
        "rust-analyzer-source-policy"
    );

    let mut wrong_tag = expected_source_lock();
    wrong_tag.upstream_tag = "2026-07-28".to_string();
    for asset in &mut wrong_tag.assets {
        asset.url = asset.url.replace("2026-07-27", "2026-07-28");
    }
    assert_eq!(
        wrong_tag.validate().unwrap_err().code,
        "rust-analyzer-source-policy"
    );

    let mut wrong_commit = expected_source_lock();
    wrong_commit.upstream_commit = "22c3381f0b17b8eec21075d1c72fd010996a9bda".to_string();
    assert_eq!(
        wrong_commit.validate().unwrap_err().code,
        "rust-analyzer-source-policy"
    );
}

#[test]
fn rust_analyzer_source_lock_rejects_reviewed_asset_metadata_drift() {
    for index in 0..4 {
        let changed_size = source_lock_with_asset_mutation(index, |asset| {
            asset.archive_size += 1;
        });
        assert_eq!(
            changed_size.validate().unwrap_err().code,
            "rust-analyzer-source-policy",
            "archive size drift for asset {index} was accepted"
        );
    }

    let changed_archive = source_lock_with_asset_mutation(0, |asset| {
        asset.archive_name = "rust-analyzer-x86_64-apple-darwin-v2.gz".to_string();
        asset.url = format!(
            "https://github.com/rust-lang/rust-analyzer/releases/download/2026-07-27/{}",
            asset.archive_name
        );
    });
    let changed_archive_hash = source_lock_with_asset_mutation(0, |asset| {
        asset.archive_sha256 = digest('a');
    });
    let changed_executable_name = source_lock_with_asset_mutation(0, |asset| {
        asset.executable_name = "rust-analyzer-v2".to_string();
    });
    let changed_executable_size = source_lock_with_asset_mutation(0, |asset| {
        asset.executable_size += 1;
    });
    let changed_executable_hash = source_lock_with_asset_mutation(0, |asset| {
        asset.executable_sha256 = digest('b');
    });
    let changed_version_probe = source_lock_with_asset_mutation(0, |asset| {
        asset.expected_version_output = "rust-analyzer 0.3.2990-standalone".to_string();
    });
    let changed_license_paths = source_lock_with_asset_mutation(0, |asset| {
        asset.license_source_paths = vec!["LICENSE-MIT".to_string()];
    });

    for (field, lock) in [
        ("release URL and archive name", changed_archive),
        ("archive SHA256", changed_archive_hash),
        ("executable name", changed_executable_name),
        ("executable size", changed_executable_size),
        ("executable SHA256", changed_executable_hash),
        ("version probe output", changed_version_probe),
        ("license paths", changed_license_paths),
    ] {
        assert_eq!(
            lock.validate().unwrap_err().code,
            "rust-analyzer-source-policy",
            "{field} drift was accepted"
        );
    }
}

#[test]
fn source_lock_deserialization_rejects_command_and_environment_fields() {
    let command_fields = [
        "command",
        "arguments",
        "shell",
        "environment",
        "env",
        "working_directory",
    ];
    for field in command_fields {
        let mut root = serde_json::to_value(expected_source_lock()).unwrap();
        root.as_object_mut()
            .unwrap()
            .insert(field.to_string(), json!("unreviewed"));
        assert!(
            serde_json::from_value::<SourceLock>(root).is_err(),
            "root field {field} must be rejected"
        );

        let mut asset = serde_json::to_value(expected_source_lock()).unwrap();
        asset["assets"][0]
            .as_object_mut()
            .unwrap()
            .insert(field.to_string(), json!("unreviewed"));
        assert!(
            serde_json::from_value::<SourceLock>(asset).is_err(),
            "asset field {field} must be rejected"
        );
    }
}

#[test]
fn provider_selection_binds_source_baseline_manifest_and_sbom_digests() {
    let record = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
    let expected = record.clone();
    let manifest = provider_manifest(record);
    manifest.validate().unwrap();

    let selected = manifest
        .select_active("rust-analyzer", "linux-amd64")
        .unwrap();
    assert_eq!(
        selected.source_lock_sha256,
        RUST_ANALYZER_SOURCE_LOCK_SHA256
    );
    assert_eq!(selected.quality_baseline_sha256, Some(digest('6')));
    assert_eq!(selected.default_configuration_sha256, None);
    assert_eq!(selected.pack_manifest_sha256, digest('2'));
    assert_eq!(selected.sbom_sha256, digest('3'));
    assert_eq!(selected.pack_version, PROVIDER_PACK_VERSION);
    assert_ne!(selected.pack_version, selected.upstream_tag);

    let compact = canonical_json(selected).unwrap();
    assert!(!compact.ends_with(b"\n"));
    assert_eq!(
        serde_json::from_slice::<ArtifactPackRecord>(&compact).unwrap(),
        expected
    );
}

#[test]
fn provider_records_reject_wrong_identity_or_missing_digest_bindings() {
    let mut unreviewed_provider = provider_record(&digest('a'));
    unreviewed_provider.artifact_id = "unreviewed-provider".to_string();
    unreviewed_provider.upstream_repository = "gitleaks/gitleaks".to_string();
    assert_eq!(rejection(unreviewed_provider), "artifact-role-policy");

    let mut wrong_artifact = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
    wrong_artifact.artifact_id = "gitleaks".to_string();
    assert_eq!(rejection(wrong_artifact), "artifact-role-policy");

    let mut wrong_repository = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
    wrong_repository.upstream_repository = "gitleaks/gitleaks".to_string();
    assert_eq!(rejection(wrong_repository), "artifact-role-policy");

    for record in [
        {
            let mut record = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
            record.tool_version = "2026-07-28".to_string();
            record
        },
        {
            let mut record = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
            record.upstream_tag = "2026-07-28".to_string();
            record
        },
        {
            let mut record = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
            record.upstream_commit = "22c3381f0b17b8eec21075d1c72fd010996a9bda".to_string();
            record
        },
        {
            let mut record = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
            record.pack_version = record.upstream_tag.clone();
            record
        },
        {
            let mut record = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
            record.expected_version = "rust-analyzer 0.3.2990-standalone".to_string();
            record
        },
        {
            let mut record = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
            record.executable.sha256 = digest('a');
            record
        },
        {
            let mut record = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
            record.project_release_tag = "artifact-rust-analyzer-unreviewed".to_string();
            record
        },
        {
            let mut record = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
            record.project_asset_name = "rust-analyzer-unreviewed-linux-amd64.tar.gz".to_string();
            record
        },
        {
            let mut record = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
            record.license_component = "unreviewed-component".to_string();
            record
        },
        {
            let mut record = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
            record.license_files[0].path = "licenses/LICENSE-APACHE-v2".to_string();
            record
        },
        {
            let mut record = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
            record.license_files.truncate(1);
            record
        },
        {
            let mut record = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
            record.sbom_component = "pkg:generic/rust-analyzer@2026-07-27".to_string();
            record
        },
    ] {
        assert_eq!(rejection(record), "artifact-role-policy");
    }

    let mut no_source = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
    no_source.source_lock_sha256.clear();
    assert_eq!(rejection(no_source), "invalid-sha256");

    let wrong_source = provider_record(&digest('a'));
    assert_eq!(rejection(wrong_source), "artifact-source-lock-policy");

    let mut no_baseline = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
    no_baseline.quality_baseline_sha256 = None;
    assert_eq!(rejection(no_baseline), "artifact-role-policy");

    let mut configuration = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
    configuration.default_configuration_sha256 = Some(digest('7'));
    assert_eq!(rejection(configuration), "artifact-role-policy");

    let mut no_manifest = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
    no_manifest.pack_manifest_sha256.clear();
    assert_eq!(rejection(no_manifest), "invalid-sha256");

    let mut no_sbom = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
    no_sbom.sbom_sha256 = "A".repeat(64);
    assert_eq!(rejection(no_sbom), "invalid-sha256");

    let mut wrong_version_probe = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
    wrong_version_probe.version_probe = ProbeId::GitleaksVersionV1;
    assert_eq!(rejection(wrong_version_probe), "artifact-role-policy");

    let mut wrong_capability_probe = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
    wrong_capability_probe.capability_probe = ProbeId::GitleaksStdinJsonV1;
    assert_eq!(rejection(wrong_capability_probe), "artifact-role-policy");

    let mut masquerading_sanitizer = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
    masquerading_sanitizer.artifact_role = ArtifactRole::Sanitizer;
    masquerading_sanitizer.version_probe = ProbeId::GitleaksVersionV1;
    masquerading_sanitizer.capability_probe = ProbeId::GitleaksStdinJsonV1;
    masquerading_sanitizer.default_configuration_sha256 = Some(digest('7'));
    masquerading_sanitizer.quality_baseline_sha256 = None;
    assert_eq!(rejection(masquerading_sanitizer), "artifact-role-policy");
}

#[test]
fn provider_policy_does_not_invent_unreviewed_license_byte_bindings() {
    let mut record = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
    record.license_files[0].size += 1;
    record.license_files[0].sha256 = digest('a');
    record.license_files[1].size += 1;
    record.license_files[1].sha256 = digest('b');

    provider_manifest(record).validate().unwrap();
}

#[test]
fn quality_baselines_are_provider_specific_and_source_lock_bound() {
    let baseline = ArtifactBaseline {
        schema_version: 1,
        kind: "third_party_artifact_baseline".to_string(),
        artifact_id: "rust-analyzer".to_string(),
        pack_version: "2026.07.27-pcr.1".to_string(),
        source_lock_sha256: RUST_ANALYZER_SOURCE_LOCK_SHA256.to_string(),
        measurements: vec![BaselineMeasurement {
            platform_id: "linux-amd64".to_string(),
            pack_sha256: digest('1'),
            executable_sha256: digest('2'),
            profile_sha256: digest('3'),
            fixture_id: "single-crate".to_string(),
            fixture_sha256: digest('4'),
            request_sha256: digest('5'),
            runner_class: "github-hosted-linux-x64".to_string(),
            samples_ms: (1..=20).map(|value| value * 10).collect(),
            p95_ms: 190,
            peak_process_tree_rss_bytes: 256 * 1024 * 1024,
        }],
    };
    baseline.validate().unwrap();

    let mut wrong_artifact = baseline.clone();
    wrong_artifact.artifact_id = "gitleaks".to_string();
    assert_eq!(
        wrong_artifact.validate().unwrap_err().code,
        "baseline-artifact-policy"
    );

    let mut wrong_pack_version = baseline.clone();
    wrong_pack_version.pack_version = "2026-07-27".to_string();
    assert_eq!(
        wrong_pack_version.validate().unwrap_err().code,
        "baseline-pack-policy"
    );

    let mut wrong_source_lock = baseline;
    wrong_source_lock.source_lock_sha256 = digest('a');
    assert_eq!(
        wrong_source_lock.validate().unwrap_err().code,
        "baseline-source-lock-policy"
    );
}

#[test]
fn source_lock_schema_is_strict_and_provider_specific() {
    let schema: Value = serde_json::from_str(include_str!(
        "../schemas/third-party-source-lock.schema.json"
    ))
    .unwrap();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(
        schema["$defs"]["sourceAsset"]["additionalProperties"],
        false
    );
    assert_eq!(schema["properties"]["schema_version"]["const"], 1);
    assert_eq!(schema["properties"]["kind"]["const"], "third_party_sources");
    assert_eq!(schema["properties"]["assets"]["minItems"], 4);
    assert_eq!(schema["properties"]["assets"]["maxItems"], 4);
    assert_eq!(
        schema["$defs"]["sourceAsset"]["properties"]["url"]["maxLength"],
        2048
    );
    let rust_analyzer_schema = &schema["$defs"]["rustAnalyzerSourceLock"];
    assert_eq!(rust_analyzer_schema["additionalProperties"], false);
    assert_eq!(
        rust_analyzer_schema["properties"]["tool_version"]["const"],
        "2026-07-27"
    );
    assert_eq!(
        rust_analyzer_schema["properties"]["upstream_commit"]["const"],
        "12c3381f0b17b8eec21075d1c72fd010996a9bda"
    );
    assert_eq!(
        rust_analyzer_schema["properties"]["assets"]["const"],
        serde_json::to_value(expected_source_lock()).unwrap()["assets"]
    );

    let baseline_schema: Value = serde_json::from_str(include_str!(
        "../schemas/third-party-artifact-baseline.schema.json"
    ))
    .unwrap();
    assert_eq!(
        baseline_schema["properties"]["artifact_id"]["const"],
        "rust-analyzer"
    );
    assert_eq!(
        baseline_schema["properties"]["pack_version"]["const"],
        PROVIDER_PACK_VERSION
    );
    assert_eq!(
        baseline_schema["properties"]["source_lock_sha256"]["const"],
        RUST_ANALYZER_SOURCE_LOCK_SHA256
    );
}
