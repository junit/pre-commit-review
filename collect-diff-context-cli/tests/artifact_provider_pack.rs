use collect_diff_context_cli::artifacts::contract::{
    canonical_json, sha256_bytes, ArtifactBaseline, ArtifactFileBinding, ArtifactManifest,
    ArtifactPackRecord, ArtifactRole, ArtifactState, BaselineMeasurement, PackFormat, ProbeId,
    SourceAssetRecord, SourceLock,
};
use collect_diff_context_cli::artifacts::provider::{
    build_provider_pack, write_rust_analyzer_pack, ProviderLicenseInput, ProviderPackInput,
    RustAnalyzerPackOptions,
};
use flate2::read::GzDecoder;
use serde_json::{json, Value};
use std::{fs, io::Read, path::PathBuf, process::Command};

const RUST_ANALYZER_SOURCE_LOCK_SHA256: &str =
    "298bc6c0339fe2c58fd35bfbd53db285ea7ff34e40734a4f0c36ccb3fe60d862";
const PROVIDER_PACK_VERSION: &str = "2026.07.27-pcr.3";
const EXPECTED_VERSION_OUTPUT: &str = "rust-analyzer 0.3.2989-standalone (12c3381f0b 2026-07-26)";
const LINUX_EXPECTED_VERSION_OUTPUT: &str = "rust-analyzer 0.3.2989-standalone";

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
        expected_version_output: if platform_id == "linux-amd64" {
            LINUX_EXPECTED_VERSION_OUTPUT
        } else {
            EXPECTED_VERSION_OUTPUT
        }
        .to_string(),
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
                "x86_64-unknown-linux-gnu",
                (
                    "rust-analyzer-x86_64-unknown-linux-gnu.gz",
                    15_035_345,
                    "ac4f42ddbbd040d75d847e991894776485783e28beb744b9719a660a99abe115",
                ),
                (
                    "rust-analyzer",
                    42_570_504,
                    "f06d56b784d621794290826d28f30345029122f86fb2223d7dda820de8dc8de6",
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
        target_triple: "x86_64-unknown-linux-gnu".to_string(),
        state: ArtifactState::Active,
        pack_version: PROVIDER_PACK_VERSION.to_string(),
        project_release_tag: "artifact-rust-analyzer-2026.07.27-pcr.3".to_string(),
        project_asset_name: "pre-commit-review-rust-analyzer-2026.07.27-pcr.3-linux-amd64.tar.gz"
            .to_string(),
        expected_compressed_size: 16 * 1024 * 1024,
        max_compressed_size: 32 * 1024 * 1024,
        pack_sha256: digest('1'),
        pack_manifest_sha256: digest('2'),
        sbom_sha256: digest('3'),
        pack_format: PackFormat::NormalizedTarGzipV1,
        executable: ArtifactFileBinding {
            path: "bin/rust-analyzer".to_string(),
            size: 42_570_504,
            sha256: "f06d56b784d621794290826d28f30345029122f86fb2223d7dda820de8dc8de6".to_string(),
        },
        version_probe: ProbeId::RustAnalyzerVersionV1,
        capability_probe: ProbeId::RustAnalyzerStdioV1,
        expected_version: LINUX_EXPECTED_VERSION_OUTPUT.to_string(),
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
    changed_target.assets[2].target_triple = "x86_64-unknown-linux-musl".to_string();
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
    assert_eq!(
        rejection(unreviewed_provider),
        "platform-target-mismatch",
        "GNU/Linux must remain scoped to the reviewed rust-analyzer identity"
    );

    let mut wrong_artifact = provider_record(RUST_ANALYZER_SOURCE_LOCK_SHA256);
    wrong_artifact.artifact_id = "gitleaks".to_string();
    assert_eq!(
        rejection(wrong_artifact),
        "platform-target-mismatch",
        "Gitleaks must keep the generic musl mapping"
    );

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
        pack_version: "2026.07.27-pcr.3".to_string(),
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

fn fixture_pack_input(
    platform_id: &str,
    target_triple: &str,
    executable_name: &str,
) -> ProviderPackInput {
    ProviderPackInput {
        tool_version: "2026-07-27".to_string(),
        pack_version: PROVIDER_PACK_VERSION.to_string(),
        platform_id: platform_id.to_string(),
        target_triple: target_triple.to_string(),
        source_lock_sha256: digest('a'),
        upstream_repository: "rust-lang/rust-analyzer".to_string(),
        upstream_tag: "2026-07-27".to_string(),
        upstream_asset_name: format!("rust-analyzer-{target_triple}.fixture"),
        upstream_archive: format!("fixture archive for {platform_id}\n").into_bytes(),
        executable_name: executable_name.to_string(),
        executable: format!("fixture executable for {platform_id}\n").into_bytes(),
        licenses: vec![
            ProviderLicenseInput {
                source_path: "LICENSE-APACHE".to_string(),
                bytes: b"fixture Apache-2.0 license\n".to_vec(),
            },
            ProviderLicenseInput {
                source_path: "LICENSE-MIT".to_string(),
                bytes: b"fixture MIT license\n".to_vec(),
            },
        ],
    }
}

fn regular_members(archive: &[u8]) -> Vec<(String, Vec<u8>)> {
    let decoder = GzDecoder::new(archive);
    let mut tar = tar::Archive::new(decoder);
    let mut members = Vec::new();
    for entry in tar.entries().unwrap() {
        let mut entry = entry.unwrap();
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().unwrap().to_str().unwrap().to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        members.push((path, bytes));
    }
    members
}

#[test]
fn provider_pack_reproduction_and_sbom_are_byte_stable() {
    for (platform_id, target_triple, executable_name) in [
        ("darwin-amd64", "x86_64-apple-darwin", "rust-analyzer"),
        ("darwin-arm64", "aarch64-apple-darwin", "rust-analyzer"),
        ("linux-amd64", "x86_64-unknown-linux-gnu", "rust-analyzer"),
        (
            "windows-amd64",
            "x86_64-pc-windows-msvc",
            "rust-analyzer.exe",
        ),
    ] {
        let input = fixture_pack_input(platform_id, target_triple, executable_name);
        let first = build_provider_pack(&input).unwrap();
        let second = build_provider_pack(&input).unwrap();
        assert_eq!(first.archive, second.archive);
        assert_eq!(first.archive[..3], [0x1f, 0x8b, 8]);
        assert_eq!(first.archive[3], 0);
        assert_eq!(&first.archive[4..8], &[0, 0, 0, 0]);
        assert_eq!(first.archive[8], 2);
        assert_eq!(first.archive[9], 255);

        let members = regular_members(&first.archive);
        assert_eq!(
            members
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            vec![
                format!("bin/{executable_name}"),
                "licenses/LICENSE-APACHE".to_string(),
                "licenses/LICENSE-MIT".to_string(),
                "pack-manifest.json".to_string(),
                "sbom.cdx.json".to_string(),
            ]
        );
        assert_eq!(
            canonical_json(&first.manifest).unwrap(),
            first.manifest_bytes
        );
        assert_eq!(canonical_json(&first.sbom).unwrap(), first.sbom_bytes);
        assert!(!first.manifest_bytes.ends_with(b"\n"));
        assert!(!first.sbom_bytes.ends_with(b"\n"));

        let component = &first.sbom["components"][0];
        assert_eq!(component["name"], "rust-analyzer");
        assert_eq!(component["version"], "2026-07-27");
        assert_eq!(
            component["purl"],
            "pkg:github/rust-lang/rust-analyzer@2026-07-27"
        );
        assert_eq!(
            component["hashes"][0]["content"],
            sha256_bytes(&input.executable)
        );
        assert_eq!(
            component["externalReferences"][0]["hashes"][0]["content"],
            sha256_bytes(&input.upstream_archive)
        );
        assert_eq!(
            first.sbom["dependencies"][0]["dependsOn"][0],
            component["bom-ref"]
        );
        assert!(component["properties"]
            .as_array()
            .unwrap()
            .iter()
            .any(|property| {
                property["name"] == "pre-commit-review:transitive-closure"
                    && property["value"] == "unknown"
            }));
    }
}

#[test]
fn production_provider_writer_rejects_unreviewed_upstream_bytes_before_output() {
    let temporary = tempfile::tempdir().unwrap();
    let archive = temporary
        .path()
        .join("rust-analyzer-x86_64-unknown-linux-gnu.gz");
    let executable = temporary.path().join("rust-analyzer");
    let version = temporary.path().join("version-output.txt");
    let source_lock = temporary.path().join("rust-analyzer-2026-07-27.json");
    let generator_config = temporary.path().join("generator-config.json");
    let output = temporary.path().join("provider.tar.gz");
    fs::copy(source_lock_path(), &source_lock).unwrap();
    fs::write(&archive, b"not the reviewed archive").unwrap();
    fs::write(&executable, b"not the reviewed executable").unwrap();
    fs::write(&version, LINUX_EXPECTED_VERSION_OUTPUT).unwrap();
    fs::write(temporary.path().join("LICENSE-APACHE"), b"Apache-2.0").unwrap();
    fs::write(temporary.path().join("LICENSE-MIT"), b"MIT").unwrap();
    fs::write(
        &generator_config,
        br#"{"compression":"gzip-level-9","gzip_mtime":0,"gzip_os":255,"pack_version":"2026.07.27-pcr.3","platform_id":"linux-amd64","rust_toolchain":"1.95.0","tar_format":"posix-ustar"}"#,
    )
    .unwrap();

    let error = write_rust_analyzer_pack(&RustAnalyzerPackOptions {
        platform_id: "linux-amd64",
        pack_version: PROVIDER_PACK_VERSION,
        source_lock_path: &source_lock,
        generator_config_path: &generator_config,
        output_path: &output,
        manifest_output: None,
        sbom_output: None,
    })
    .unwrap_err();
    assert!(error.contains("upstream archive does not match"));
    assert!(!output.exists());
}

#[test]
fn provider_writer_cli_rejects_independently_selected_trust_inputs() {
    let temporary = tempfile::tempdir().unwrap();
    let output = temporary.path().join("provider.tar.gz");
    let result = Command::new(env!("CARGO_BIN_EXE_artifact-pack-writer"))
        .arg("rust-analyzer")
        .arg("--platform-id")
        .arg("linux-amd64")
        .arg("--pack-version")
        .arg(PROVIDER_PACK_VERSION)
        .arg("--source-lock")
        .arg(source_lock_path())
        .arg("--upstream-archive")
        .arg(temporary.path().join("arbitrary-upstream.gz"))
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("unknown argument: --upstream-archive"),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!output.exists());
}

#[test]
fn provider_writer_cli_rejects_drifted_generator_configuration_before_output() {
    let temporary = tempfile::tempdir().unwrap();
    let source_lock = temporary.path().join("rust-analyzer-2026-07-27.json");
    let generator_config = temporary.path().join("generator-config.json");
    let output = temporary.path().join("provider.tar.gz");
    fs::copy(source_lock_path(), &source_lock).unwrap();
    fs::write(
        &generator_config,
        br#"{"compression":"gzip-level-8","gzip_mtime":0,"gzip_os":255,"pack_version":"2026.07.27-pcr.3","platform_id":"linux-amd64","rust_toolchain":"1.95.0","tar_format":"posix-ustar"}"#,
    )
    .unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_artifact-pack-writer"))
        .arg("rust-analyzer")
        .arg("--platform-id")
        .arg("linux-amd64")
        .arg("--pack-version")
        .arg(PROVIDER_PACK_VERSION)
        .arg("--source-lock")
        .arg(&source_lock)
        .arg("--generator-config")
        .arg(&generator_config)
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("provider generator configuration is not canonical"),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!output.exists());
}

#[test]
fn provider_release_workflow_accepts_only_the_exact_rust_analyzer_tag() {
    const RELEASE_TAG: &str = "artifact-rust-analyzer-2026.07.27-pcr.3";
    const RELEASE_REF: &str = "refs/tags/artifact-rust-analyzer-2026.07.27-pcr.3";
    fn job_condition(job: &str) -> &str {
        job.lines()
            .find_map(|line| line.strip_prefix("    if: "))
            .expect("provider job is missing its selection condition")
    }

    let workflow = include_str!("../../.github/workflows/artifact-pack-release.yml");
    let on_start = workflow.find("on:\n").unwrap();
    let permissions_start = workflow.find("\npermissions:\n").unwrap();
    let triggers = &workflow[on_start..permissions_start];
    let push = triggers
        .split_once("  push:\n")
        .map(|(_, push)| push)
        .expect("provider workflow is missing the exact release push trigger");
    let push_lines = push
        .lines()
        .take_while(|line| line.starts_with("    "))
        .collect::<Vec<_>>();

    assert_eq!(
        push_lines,
        vec![
            "    tags:",
            "      - artifact-rust-analyzer-2026.07.27-pcr.3"
        ]
    );
    assert!(triggers.contains("  workflow_call:\n"));
    assert!(triggers.contains("  workflow_dispatch:\n"));
    assert!(!triggers.contains("branches:"));
    assert!(!triggers.contains("repository_dispatch:"));
    assert!(!workflow.contains("artifact-rust-analyzer-2026.07.27-pcr.1"));
    assert!(!workflow.contains("artifact-rust-analyzer-2026.07.27-pcr.2"));

    let build_start = workflow.find("\n  build:\n").unwrap();
    let rust_build_start = workflow.find("\n  build-rust-analyzer:\n").unwrap();
    let verify_start = workflow.find("\n  verify:\n").unwrap();
    let rust_verify_start = workflow.find("\n  verify-rust-analyzer:\n").unwrap();
    let publish_start = workflow.find("\n  publish:\n").unwrap();
    let rust_publish_start = workflow.find("\n  publish-rust-analyzer:\n").unwrap();
    let gitleaks_build = &workflow[build_start..rust_build_start];
    let rust_build = &workflow[rust_build_start..verify_start];
    let gitleaks_verify = &workflow[verify_start..rust_verify_start];
    let rust_verify = &workflow[rust_verify_start..publish_start];
    let gitleaks_publish = &workflow[publish_start..rust_publish_start];
    let rust_publish = &workflow[rust_publish_start..];
    assert!(gitleaks_build.contains("Install musl-tools"));
    assert!(!rust_build.contains("Install musl-tools"));
    assert!(!rust_build.contains("apt-get install"));

    let tag_selector = format!("github.ref == '{RELEASE_REF}'");
    assert_eq!(
        job_condition(rust_build),
        format!("inputs.artifact == 'rust-analyzer' || {tag_selector}")
    );
    assert_eq!(
        job_condition(rust_verify),
        format!("inputs.artifact == 'rust-analyzer' || {tag_selector}")
    );
    assert_eq!(
        job_condition(rust_publish),
        format!(
            "(inputs.artifact == 'rust-analyzer' && (startsWith(github.ref, 'refs/tags/') || github.event_name == 'workflow_dispatch')) || {tag_selector}"
        )
    );
    assert_eq!(
        job_condition(gitleaks_build),
        "inputs.artifact == 'gitleaks'"
    );
    assert!(gitleaks_verify
        .lines()
        .all(|line| !line.starts_with("    if: ")));
    assert!(gitleaks_verify.contains("    needs: build\n"));
    assert_eq!(
        job_condition(gitleaks_publish),
        "inputs.artifact == 'gitleaks' && (startsWith(github.ref, 'refs/tags/') || github.event_name == 'workflow_dispatch')"
    );

    let ref_name_uses = workflow
        .lines()
        .filter(|line| line.contains("github.ref_name"))
        .collect::<Vec<_>>();
    assert_eq!(ref_name_uses.len(), 2);
    assert!(ref_name_uses
        .iter()
        .all(|line| { line.trim() == "tag_name: ${{ inputs.release_tag || github.ref_name }}" }));
    assert_eq!(triggers.matches(RELEASE_TAG).count(), 1);
}

#[test]
fn provider_release_workflow_prepares_bound_inputs_before_invoking_writer() {
    let workflow = include_str!("../../.github/workflows/artifact-pack-release.yml");
    let prepare_start = workflow
        .find("- name: Fetch, verify, and extract the reviewed upstream asset")
        .unwrap();
    let writer_start = workflow
        .find("- name: Build normalized rust-analyzer pack")
        .unwrap();
    let evidence_start = workflow
        .find("- name: Generate composition evidence")
        .unwrap();
    let attest_start = workflow
        .find("- name: Attest provider pack subject")
        .unwrap();
    let upload_start = workflow
        .find("- name: Upload provider pack and trust material")
        .unwrap();
    let clean_verify_start = workflow.find("verify-rust-analyzer:").unwrap();
    let publish_start = workflow.find("publish:").unwrap();
    let prepare = &workflow[prepare_start..writer_start];
    let writer = &workflow[writer_start..evidence_start];
    let evidence = &workflow[evidence_start..attest_start];
    let attest = &workflow[attest_start..upload_start];
    let clean_verify = &workflow[clean_verify_start..publish_start];

    assert!(prepare.contains("import shutil"));
    assert!(prepare.contains("output / 'generator-config.json'"));
    assert!(prepare.contains("shutil.copy2(lock_path, output / lock_path.name)"));
    assert!(prepare.contains("shutil.copy2(source, output / license_name)"));
    assert!(writer.contains("--generator-config"));
    assert!(evidence.contains("(verify_root / f'{subject.name}.attestation.json').write_text"));
    assert!(!evidence.contains("(root / f'{subject.name}.attestation.json').write_text"));
    assert!(evidence.contains("composition-predicate.json"));
    assert!(!evidence.contains("published_upstream"));
    assert!(!evidence.contains("published_lock"));
    assert_eq!(
        attest
            .matches("actions/attest@daf44fb950173508f38bd2406030372c1d1162b1")
            .count(),
        3
    );
    assert_eq!(
        attest
            .matches("predicate-type: pre-commit-review.artifact-pack/v1")
            .count(),
        3
    );
    assert_eq!(attest.matches("predicate-path:").count(), 3);
    assert!(attest.contains("steps.attest-pack.outputs.bundle-path"));
    assert!(attest.contains("steps.attest-manifest.outputs.bundle-path"));
    assert!(attest.contains("steps.attest-sbom.outputs.bundle-path"));
    assert!(!attest.contains("attest-build-provenance"));
    assert!(clean_verify.contains("--signed-release-root dist"));
    assert!(clean_verify.contains("GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}"));
    for legacy in [
        "--upstream-archive",
        "--binary",
        "--version-output",
        "--license-root",
    ] {
        assert!(
            !writer.contains(legacy),
            "legacy writer input remains: {legacy}"
        );
    }
}
