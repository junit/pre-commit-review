use collect_diff_context_cli::artifacts::contract::{
    canonical_json, sha256_bytes, ArtifactBaseline, ArtifactFileBinding, ArtifactManifest,
    ArtifactOperation, ArtifactPackRecord, ArtifactReceipt, ArtifactReport, ArtifactReportEntry,
    ArtifactReportStatus, ArtifactRole, ArtifactState, BaselineMeasurement, CorePackFileBinding,
    CorePackManifest, PackFileRecord, PackFileRole, PackFormat, PackManifest, ProbeId, ProbeResult,
    RevocationEntry, RevocationIndex, SourceAssetRecord, SourceLock,
};
use serde_json::Value;
use std::{fs, path::PathBuf};

const ARTIFACT_SCHEMAS: &[(&str, &str)] = &[
    (
        "third-party-artifacts.schema.json",
        include_str!("../schemas/third-party-artifacts.schema.json"),
    ),
    (
        "third-party-artifact-pack.schema.json",
        include_str!("../schemas/third-party-artifact-pack.schema.json"),
    ),
    (
        "third-party-artifact-receipt.schema.json",
        include_str!("../schemas/third-party-artifact-receipt.schema.json"),
    ),
    (
        "third-party-artifact-report.schema.json",
        include_str!("../schemas/third-party-artifact-report.schema.json"),
    ),
    (
        "third-party-artifact-baseline.schema.json",
        include_str!("../schemas/third-party-artifact-baseline.schema.json"),
    ),
    (
        "third-party-artifact-revocations.schema.json",
        include_str!("../schemas/third-party-artifact-revocations.schema.json"),
    ),
    (
        "third-party-source-lock.schema.json",
        include_str!("../schemas/third-party-source-lock.schema.json"),
    ),
    (
        "pre-commit-review-core-pack.schema.json",
        include_str!("../schemas/pre-commit-review-core-pack.schema.json"),
    ),
];

const CANONICAL_MANIFEST_SHA256: &str =
    "62ac5077244a8ed5161dbd9b5a44ea7bcbd91eda7c0ae46cc70a6c61f722b75c";
const CANONICAL_REVOCATIONS_SHA256: &str =
    "e62256210a5f27606e808c36005ae9052aa900a5b890b0976367c05b62cf0457";
const GITLEAKS_SOURCE_LOCK_SHA256: &str =
    "659556055e7366c27886b14b0bd94104b8ab77df2584da729350f43d3ef8e3a0";

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn fixture_record(
    platform_id: &str,
    target_triple: &str,
    executable_name: &str,
    digest_character: char,
) -> ArtifactPackRecord {
    ArtifactPackRecord {
        artifact_id: "gitleaks".to_string(),
        artifact_role: ArtifactRole::Sanitizer,
        tool_version: "8.30.1".to_string(),
        upstream_repository: "gitleaks/gitleaks".to_string(),
        upstream_tag: "v8.30.1".to_string(),
        upstream_commit: digest('1')[..40].to_string(),
        source_lock_sha256: digest('a'),
        platform_id: platform_id.to_string(),
        target_triple: target_triple.to_string(),
        state: ArtifactState::Active,
        pack_version: "8.30.1-pcr.1".to_string(),
        project_release_tag: "artifact-gitleaks-8.30.1-pcr.1".to_string(),
        project_asset_name: format!("gitleaks-8.30.1-pcr.1-{platform_id}.tar.gz"),
        expected_compressed_size: 1_024,
        max_compressed_size: 2_048,
        pack_sha256: digest(digest_character),
        pack_manifest_sha256: digest('b'),
        sbom_sha256: digest('c'),
        pack_format: PackFormat::NormalizedTarGzipV1,
        executable: ArtifactFileBinding {
            path: format!("bin/{executable_name}"),
            size: 512,
            sha256: digest('d'),
        },
        version_probe: ProbeId::GitleaksVersionV1,
        capability_probe: ProbeId::GitleaksStdinJsonV1,
        expected_version: "8.30.1".to_string(),
        license_component: "gitleaks".to_string(),
        license_files: vec![ArtifactFileBinding {
            path: "licenses/GITLEAKS-LICENSE".to_string(),
            size: 128,
            sha256: digest('e'),
        }],
        sbom_component: "pkg:github/gitleaks/gitleaks@8.30.1".to_string(),
        default_configuration_sha256: Some(digest('f')),
        quality_baseline_sha256: None,
        revoked_reason: None,
        replacement_pack_version: None,
    }
}

fn fixture_manifest() -> ArtifactManifest {
    ArtifactManifest {
        schema_version: 1,
        kind: "third_party_artifacts".to_string(),
        release_repository: "junit/pre-commit-review".to_string(),
        revocation_index_sha256: digest('0'),
        packs: vec![
            fixture_record("darwin-amd64", "x86_64-apple-darwin", "gitleaks", '1'),
            fixture_record("darwin-arm64", "aarch64-apple-darwin", "gitleaks", '2'),
            fixture_record("linux-amd64", "x86_64-unknown-linux-musl", "gitleaks", '3'),
            fixture_record(
                "windows-amd64",
                "x86_64-pc-windows-msvc",
                "gitleaks.exe",
                '4',
            ),
        ],
    }
}

fn source_asset(
    platform_id: &str,
    target_triple: &str,
    archive_name: &str,
    executable_name: &str,
    digest_character: char,
) -> SourceAssetRecord {
    SourceAssetRecord {
        platform_id: platform_id.to_string(),
        target_triple: target_triple.to_string(),
        url: format!(
            "https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/{archive_name}"
        ),
        archive_name: archive_name.to_string(),
        archive_size: 1_024,
        archive_sha256: digest(digest_character),
        executable_name: executable_name.to_string(),
        executable_size: 512,
        executable_sha256: digest('3'),
        expected_version_output: "8.30.1".to_string(),
        license_source_paths: vec!["LICENSE".to_string()],
    }
}

fn canonical_metadata_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("third_party_artifacts")
        .join(relative)
}

fn read_canonical_metadata(relative: &str) -> Vec<u8> {
    let path = canonical_metadata_path(relative);
    let bytes = fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "failed to read canonical metadata {}: {error}",
            path.display()
        )
    });
    assert!(
        !bytes.ends_with(b"\n"),
        "{} has a trailing newline",
        path.display()
    );
    bytes
}

#[test]
fn manifest_round_trip_and_canonical_digest_are_stable() {
    let manifest = fixture_manifest();
    manifest.validate().unwrap();
    let bytes = canonical_json(&manifest).unwrap();
    assert!(!bytes.ends_with(b"\n"));
    assert_eq!(sha256_bytes(&bytes).len(), 64);
    assert_eq!(
        serde_json::from_slice::<ArtifactManifest>(&bytes).unwrap(),
        manifest
    );
    assert_eq!(canonical_json(&manifest).unwrap(), bytes);
}

#[test]
fn manifest_selects_one_exact_active_platform_record() {
    let manifest = fixture_manifest();
    let selected = manifest.select_active("gitleaks", "linux-amd64").unwrap();
    assert_eq!(selected.target_triple, "x86_64-unknown-linux-musl");
    assert_eq!(selected.pack_sha256, digest('3'));
    assert_eq!(
        manifest
            .select_active("rust-analyzer", "linux-amd64")
            .unwrap_err()
            .code,
        "artifact-not-active"
    );
}

#[test]
fn manifest_rejects_untrusted_selection_and_budget_overflow() {
    let mut manifest = fixture_manifest();
    manifest.packs[0].project_release_tag = "latest".to_string();
    assert_eq!(manifest.validate().unwrap_err().code, "release-tag-policy");

    let mut duplicate = fixture_manifest();
    duplicate.packs.insert(1, duplicate.packs[0].clone());
    assert_eq!(duplicate.validate().unwrap_err().code, "duplicate-pack-key");

    let mut two_active = fixture_manifest();
    let mut replacement = two_active.packs[0].clone();
    replacement.pack_version = "8.30.1-pcr.2".to_string();
    replacement.project_asset_name = "gitleaks-8.30.1-pcr.2-darwin-amd64.tar.gz".to_string();
    replacement.pack_sha256 = digest('5');
    two_active.packs.insert(1, replacement);
    assert_eq!(
        two_active.validate().unwrap_err().code,
        "multiple-active-packs"
    );
}

#[test]
fn manifest_rejects_unknown_fields_and_noncanonical_digests() {
    let manifest = fixture_manifest();
    let mut value = serde_json::to_value(&manifest).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("unknown".to_string(), Value::Bool(true));
    assert!(serde_json::from_value::<ArtifactManifest>(value).is_err());

    let mut manifest = fixture_manifest();
    manifest.packs[0].pack_sha256 = "A".repeat(64);
    assert_eq!(manifest.validate().unwrap_err().code, "invalid-sha256");
}

#[test]
fn source_lock_accepts_only_fixed_upstream_release_assets() {
    let lock = SourceLock {
        schema_version: 1,
        kind: "third_party_sources".to_string(),
        artifact_id: "gitleaks".to_string(),
        tool_version: "8.30.1".to_string(),
        upstream_repository: "gitleaks/gitleaks".to_string(),
        upstream_tag: "v8.30.1".to_string(),
        upstream_commit: digest('1')[..40].to_string(),
        assets: vec![
            source_asset(
                "darwin-amd64",
                "x86_64-apple-darwin",
                "gitleaks_8.30.1_darwin_x64.tar.gz",
                "gitleaks",
                '2',
            ),
            source_asset(
                "darwin-arm64",
                "aarch64-apple-darwin",
                "gitleaks_8.30.1_darwin_arm64.tar.gz",
                "gitleaks",
                '3',
            ),
            source_asset(
                "linux-amd64",
                "x86_64-unknown-linux-musl",
                "gitleaks_8.30.1_linux_x64.tar.gz",
                "gitleaks",
                '4',
            ),
            source_asset(
                "windows-amd64",
                "x86_64-pc-windows-msvc",
                "gitleaks_8.30.1_windows_x64.zip",
                "gitleaks.exe",
                '5',
            ),
        ],
    };
    lock.validate().unwrap();

    let mut gnu_target = lock.clone();
    gnu_target.assets[2].target_triple = "x86_64-unknown-linux-gnu".to_string();
    assert_eq!(
        gnu_target.validate().unwrap_err().code,
        "platform-target-mismatch",
        "GNU/Linux must not become a generic Gitleaks target"
    );

    let mut moving = lock.clone();
    moving.upstream_tag = "latest".to_string();
    moving.assets[0].url =
        "https://github.com/gitleaks/gitleaks/releases/latest/download/gitleaks.tar.gz".to_string();
    assert_eq!(moving.validate().unwrap_err().code, "source-tag-policy");

    let mut wrong_host = lock;
    wrong_host.assets[0].url = "https://example.invalid/gitleaks.tar.gz".to_string();
    assert_eq!(wrong_host.validate().unwrap_err().code, "source-url-policy");
}

#[test]
fn canonical_seed_metadata_is_compact_valid_and_digest_bound() {
    let revocation_bytes = read_canonical_metadata("revocations.json");
    let revocations: RevocationIndex = serde_json::from_slice(&revocation_bytes).unwrap();
    revocations.validate().unwrap();
    assert!(revocations.entries.is_empty());
    assert_eq!(canonical_json(&revocations).unwrap(), revocation_bytes);
    assert_eq!(
        sha256_bytes(&revocation_bytes),
        CANONICAL_REVOCATIONS_SHA256
    );

    let manifest_bytes = read_canonical_metadata("manifest.json");
    let manifest: ArtifactManifest = serde_json::from_slice(&manifest_bytes).unwrap();
    manifest.validate().unwrap();
    assert!(manifest.packs.is_empty());
    assert_eq!(canonical_json(&manifest).unwrap(), manifest_bytes);
    assert_eq!(sha256_bytes(&manifest_bytes), CANONICAL_MANIFEST_SHA256);
    assert_eq!(
        manifest.revocation_index_sha256,
        sha256_bytes(&revocation_bytes)
    );
    assert!(!String::from_utf8(manifest_bytes)
        .unwrap()
        .contains("github.com"));

    let source_lock_bytes = read_canonical_metadata("sources/gitleaks-8.30.1.json");
    let source_lock: SourceLock = serde_json::from_slice(&source_lock_bytes).unwrap();
    source_lock.validate().unwrap();
    assert_eq!(canonical_json(&source_lock).unwrap(), source_lock_bytes);
    assert_eq!(
        sha256_bytes(&source_lock_bytes),
        GITLEAKS_SOURCE_LOCK_SHA256
    );
    assert_eq!(source_lock.artifact_id, "gitleaks");
    assert_eq!(source_lock.tool_version, "8.30.1");
    assert_eq!(
        source_lock.upstream_commit,
        "83d9cd684c87d95d656c1458ef04895a7f1cbd8e"
    );
    assert_eq!(
        source_lock
            .assets
            .iter()
            .map(|asset| asset.platform_id.as_str())
            .collect::<Vec<_>>(),
        [
            "darwin-amd64",
            "darwin-arm64",
            "linux-amd64",
            "windows-amd64"
        ]
    );
}

#[test]
fn revocation_index_is_sorted_bounded_and_digest_addressed() {
    let index = RevocationIndex {
        schema_version: 1,
        kind: "third_party_artifact_revocations".to_string(),
        entries: vec![RevocationEntry {
            pack_sha256: digest('1'),
            artifact_id: "gitleaks".to_string(),
            platform_id: "linux-amd64".to_string(),
            pack_version: "8.30.1-pcr.1".to_string(),
            reason: "superseded after a verified rebuild".to_string(),
            replacement_pack_version: Some("8.30.1-pcr.2".to_string()),
        }],
    };
    index.validate().unwrap();

    let mut unsorted = index.clone();
    let mut earlier = unsorted.entries[0].clone();
    earlier.pack_sha256 = digest('0');
    unsorted.entries.push(earlier);
    assert_eq!(
        unsorted.validate().unwrap_err().code,
        "revocations-not-sorted"
    );
}

#[test]
fn pack_manifest_binds_every_payload_file_and_role() {
    let manifest = PackManifest {
        schema_version: 1,
        kind: "third_party_artifact_pack".to_string(),
        artifact_id: "gitleaks".to_string(),
        tool_version: "8.30.1".to_string(),
        pack_version: "8.30.1-pcr.1".to_string(),
        platform_id: "linux-amd64".to_string(),
        target_triple: "x86_64-unknown-linux-musl".to_string(),
        upstream_asset_name: "gitleaks_8.30.1_linux_x64.tar.gz".to_string(),
        upstream_asset_sha256: digest('1'),
        source_lock_sha256: digest('2'),
        project_asset_name: "gitleaks-8.30.1-pcr.1-linux-amd64.tar.gz".to_string(),
        files: vec![
            PackFileRecord {
                path: "bin/gitleaks".to_string(),
                size: 512,
                sha256: digest('3'),
                role: PackFileRole::Executable,
            },
            PackFileRecord {
                path: "licenses/GITLEAKS-LICENSE".to_string(),
                size: 128,
                sha256: digest('4'),
                role: PackFileRole::License,
            },
            PackFileRecord {
                path: "sbom.cdx.json".to_string(),
                size: 256,
                sha256: digest('5'),
                role: PackFileRole::Sbom,
            },
        ],
    };
    manifest.validate().unwrap();

    let mut duplicate_role = manifest.clone();
    duplicate_role.files[1].role = PackFileRole::Executable;
    assert_eq!(
        duplicate_role.validate().unwrap_err().code,
        "pack-file-role-count"
    );
}

#[test]
fn target_receipt_contains_no_cache_paths_and_binds_probe_results() {
    let receipt = ArtifactReceipt {
        schema_version: 1,
        kind: "third_party_artifact_receipt".to_string(),
        distribution_manifest_sha256: digest('0'),
        artifact_id: "gitleaks".to_string(),
        tool_version: "8.30.1".to_string(),
        pack_version: "8.30.1-pcr.1".to_string(),
        platform_id: "linux-amd64".to_string(),
        pack_sha256: digest('1'),
        pack_manifest_sha256: digest('2'),
        sbom_sha256: digest('3'),
        installed_files: vec![ArtifactFileBinding {
            path: "runtime/third-party/gitleaks/8.30.1-pcr.1/bin/gitleaks".to_string(),
            size: 512,
            sha256: digest('4'),
        }],
        license_files: vec![ArtifactFileBinding {
            path: "runtime/third-party/gitleaks/8.30.1-pcr.1/licenses/GITLEAKS-LICENSE".to_string(),
            size: 128,
            sha256: digest('5'),
        }],
        probes: vec![
            ProbeResult {
                probe_id: ProbeId::GitleaksVersionV1,
                success: true,
                observed_version: Some("8.30.1".to_string()),
            },
            ProbeResult {
                probe_id: ProbeId::GitleaksStdinJsonV1,
                success: true,
                observed_version: None,
            },
        ],
        lifecycle_state: ArtifactState::Active,
    };
    receipt.validate().unwrap();
    let encoded = String::from_utf8(canonical_json(&receipt).unwrap()).unwrap();
    assert!(!encoded.contains("cache"));

    let mut failed_probe = receipt;
    failed_probe.probes[0].success = false;
    assert_eq!(
        failed_probe.validate().unwrap_err().code,
        "receipt-probe-failed"
    );
}

#[test]
fn report_status_controls_identity_and_error_fields() {
    let report = ArtifactReport {
        schema_version: 1,
        kind: "third_party_artifact_report".to_string(),
        operation: ArtifactOperation::Verify,
        status: ArtifactReportStatus::Completed,
        artifact_id: Some("gitleaks".to_string()),
        platform_id: Some("linux-amd64".to_string()),
        pack_version: Some("8.30.1-pcr.1".to_string()),
        pack_sha256: Some(digest('1')),
        executable_sha256: Some(digest('2')),
        sbom_sha256: Some(digest('3')),
        lifecycle_state: Some(ArtifactState::Active),
        artifacts: Vec::new(),
        code: None,
    };
    report.validate().unwrap();

    let mut invalid = report;
    invalid.status = ArtifactReportStatus::Failed;
    assert_eq!(invalid.validate().unwrap_err().code, "report-failure-code");
}

#[test]
fn doctor_report_aggregates_sorted_artifact_results() {
    let entry = ArtifactReportEntry {
        artifact_id: "gitleaks".to_string(),
        platform_id: "linux-amd64".to_string(),
        pack_version: "8.30.1-pcr.1".to_string(),
        pack_sha256: digest('1'),
        executable_sha256: digest('2'),
        sbom_sha256: digest('3'),
        lifecycle_state: ArtifactState::Active,
    };
    let report = ArtifactReport {
        schema_version: 1,
        kind: "third_party_artifact_report".to_string(),
        operation: ArtifactOperation::Doctor,
        status: ArtifactReportStatus::Completed,
        artifact_id: None,
        platform_id: None,
        pack_version: None,
        pack_sha256: None,
        executable_sha256: None,
        sbom_sha256: None,
        lifecycle_state: None,
        artifacts: vec![entry],
        code: None,
    };
    report.validate().unwrap();
}

#[test]
fn baseline_recomputes_nearest_rank_p95_and_binds_measurements() {
    let samples_ms: Vec<u64> = (1..=20).map(|value| value * 10).collect();
    let baseline = ArtifactBaseline {
        schema_version: 1,
        kind: "third_party_artifact_baseline".to_string(),
        artifact_id: "rust-analyzer".to_string(),
        pack_version: "2026.07.27-pcr.3".to_string(),
        source_lock_sha256: "298bc6c0339fe2c58fd35bfbd53db285ea7ff34e40734a4f0c36ccb3fe60d862"
            .to_string(),
        measurements: vec![BaselineMeasurement {
            platform_id: "linux-amd64".to_string(),
            pack_sha256: digest('2'),
            executable_sha256: digest('3'),
            runner_sha256: digest('7'),
            profile_sha256: digest('4'),
            fixture_id: "single-crate".to_string(),
            fixture_sha256: digest('5'),
            request_sha256: digest('6'),
            runner_class: "github-hosted-ubuntu-24-x64".to_string(),
            toolchain: "rust-1.95.0-locked".to_string(),
            timing_scope: "provider-run-only-v1".to_string(),
            provisioning_included: false,
            samples_ms,
            p95_ms: 190,
            peak_process_tree_rss_bytes: 256 * 1024 * 1024,
        }],
    };
    baseline.validate().unwrap();

    let mut wrong_p95 = baseline;
    wrong_p95.measurements[0].p95_ms = 180;
    assert_eq!(wrong_p95.validate().unwrap_err().code, "baseline-p95");
}

#[test]
fn baseline_rejects_a_non_hosted_runner_class_for_its_platform() {
    let samples_ms: Vec<u64> = (1..=20).map(|value| value * 10).collect();
    let baseline = ArtifactBaseline {
        schema_version: 1,
        kind: "third_party_artifact_baseline".to_string(),
        artifact_id: "rust-analyzer".to_string(),
        pack_version: "2026.07.27-pcr.3".to_string(),
        source_lock_sha256: "298bc6c0339fe2c58fd35bfbd53db285ea7ff34e40734a4f0c36ccb3fe60d862"
            .to_string(),
        measurements: vec![BaselineMeasurement {
            platform_id: "linux-amd64".to_string(),
            pack_sha256: digest('2'),
            executable_sha256: digest('3'),
            runner_sha256: digest('7'),
            profile_sha256: digest('4'),
            fixture_id: "single-crate".to_string(),
            fixture_sha256: digest('5'),
            request_sha256: digest('6'),
            runner_class: "local-linux-amd64".to_string(),
            toolchain: "rust-1.95.0-locked".to_string(),
            timing_scope: "provider-run-only-v1".to_string(),
            provisioning_included: false,
            samples_ms,
            p95_ms: 190,
            peak_process_tree_rss_bytes: 256 * 1024 * 1024,
        }],
    };

    assert_eq!(
        baseline.validate().unwrap_err().code,
        "baseline-runner-class-policy"
    );
}

#[test]
fn core_inventory_is_platform_specific_and_manifest_bound() {
    let core = CorePackManifest {
        schema_version: 1,
        kind: "pre_commit_review_core_pack".to_string(),
        core_version: "0.1.0".to_string(),
        platform_id: "linux-amd64".to_string(),
        target_triple: "x86_64-unknown-linux-musl".to_string(),
        distribution_manifest_sha256: digest('1'),
        revocation_index_sha256: digest('2'),
        members: vec![
            CorePackFileBinding {
                path: "runtime/distribution/manifest.json".to_string(),
                mode: 0o644,
                size: 512,
                sha256: digest('1'),
            },
            CorePackFileBinding {
                path: "runtime/distribution/revocations.json".to_string(),
                mode: 0o644,
                size: 128,
                sha256: digest('2'),
            },
            CorePackFileBinding {
                path: "scripts/bin/collect_diff_context-linux-amd64".to_string(),
                mode: 0o755,
                size: 1_024,
                sha256: digest('3'),
            },
        ],
    };
    core.validate().unwrap();

    let mut gnu_target = core.clone();
    gnu_target.target_triple = "x86_64-unknown-linux-gnu".to_string();
    assert_eq!(
        gnu_target.validate().unwrap_err().code,
        "platform-target-mismatch",
        "GNU/Linux must not become a generic core target"
    );

    let mut other_platform = core;
    other_platform.members.push(CorePackFileBinding {
        path: "scripts/bin/collect_diff_context-darwin-arm64".to_string(),
        mode: 0o755,
        size: 1_024,
        sha256: digest('4'),
    });
    assert_eq!(
        other_platform.validate().unwrap_err().code,
        "core-platform-member"
    );
}

#[test]
fn artifact_schemas_are_draft_2020_12_and_strict_at_every_object() {
    fn assert_strict_objects(value: &Value, path: &str) {
        if value.get("type").and_then(Value::as_str) == Some("object") {
            assert_eq!(
                value.get("additionalProperties"),
                Some(&Value::Bool(false)),
                "object schema is not strict at {path}"
            );
        }
        match value {
            Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    assert_strict_objects(value, &format!("{path}/{index}"));
                }
            }
            Value::Object(values) => {
                for (key, value) in values {
                    assert_strict_objects(value, &format!("{path}/{key}"));
                }
            }
            _ => {}
        }
    }

    for (name, source) in ARTIFACT_SCHEMAS {
        let schema: Value = serde_json::from_str(source).unwrap();
        assert_eq!(
            schema.get("$schema").and_then(Value::as_str),
            Some("https://json-schema.org/draft/2020-12/schema"),
            "wrong draft for {name}"
        );
        assert_strict_objects(&schema, name);
    }
}
