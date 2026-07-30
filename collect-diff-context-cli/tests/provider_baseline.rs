use collect_diff_context_cli::artifacts::contract::{
    canonical_json, sha256_bytes, ArtifactBaseline, ArtifactManifest,
};
use collect_diff_context_cli::artifacts::provider::{accept_p95, release_threshold_ms};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const SOURCE_LOCK_SHA256: &str = "298bc6c0339fe2c58fd35bfbd53db285ea7ff34e40734a4f0c36ccb3fe60d862";
const PLATFORMS: [&str; 4] = [
    "darwin-amd64",
    "darwin-arm64",
    "linux-amd64",
    "windows-amd64",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn fixture_root() -> PathBuf {
    repo_root().join("tests/fixtures/provider-release")
}

fn run_generator(fixture: &Path) -> Output {
    Command::new("python3")
        .arg(repo_root().join("scripts/generate_provider_manifest_update.py"))
        .arg("--fixture")
        .arg(fixture)
        .output()
        .unwrap()
}

fn run_generator_in_core_release(fixture: &Path) -> Output {
    Command::new("python3")
        .arg(repo_root().join("scripts/generate_provider_manifest_update.py"))
        .arg("--fixture")
        .arg(fixture)
        .env("PCR_CORE_RELEASE_JOB", "1")
        .output()
        .unwrap()
}

fn run_generator_in_named_core_workflow(fixture: &Path) -> Output {
    Command::new("python3")
        .arg(repo_root().join("scripts/generate_provider_manifest_update.py"))
        .arg("--fixture")
        .arg(fixture)
        .env("GITHUB_ACTIONS", "true")
        .env("GITHUB_WORKFLOW", "Release Multi-Platform Packs")
        .output()
        .unwrap()
}

fn copy_generator_fixture() -> tempfile::TempDir {
    let temporary = tempfile::tempdir().unwrap();
    for name in ["reviewed-baseline.json", "verified-publication.json"] {
        fs::copy(fixture_root().join(name), temporary.path().join(name)).unwrap();
    }
    temporary
}

fn mutate_json(path: &Path, mutate: impl FnOnce(&mut Value)) {
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    mutate(&mut value);
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn assert_rejected(fixture: &Path, expected_code: &str) {
    let output = run_generator(fixture);
    assert!(!output.status.success(), "generator unexpectedly succeeded");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(expected_code),
        "expected {expected_code} rejection, stderr: {stderr}"
    );
}

#[test]
fn release_threshold_uses_checked_integer_ceiling_policy() {
    assert_eq!(release_threshold_ms(1001).unwrap(), 1502);
    assert!(accept_p95(1502, 1001).unwrap());
    assert!(!accept_p95(1503, 1001).unwrap());
}

#[test]
fn release_threshold_rejects_arithmetic_overflow() {
    let error = release_threshold_ms(u64::MAX).unwrap_err();
    assert_eq!(error.code, "baseline-threshold-overflow");

    let error = accept_p95(1, u64::MAX).unwrap_err();
    assert_eq!(error.code, "baseline-threshold-overflow");
}

#[test]
fn release_threshold_rejects_a_zero_baseline() {
    let error = release_threshold_ms(0).unwrap_err();
    assert_eq!(error.code, "baseline-threshold-range");
}

#[test]
fn synthetic_reviewed_baseline_is_canonical_and_policy_valid() {
    let path = fixture_root().join("reviewed-baseline.json");
    let bytes = fs::read(path).unwrap();
    assert!(!bytes.ends_with(b"\n"));
    let baseline: ArtifactBaseline = serde_json::from_slice(&bytes).unwrap();
    baseline.validate().unwrap();
    assert_eq!(canonical_json(&baseline).unwrap(), bytes);
    assert_eq!(baseline.source_lock_sha256, SOURCE_LOCK_SHA256);
    assert_eq!(baseline.measurements.len(), PLATFORMS.len());
    assert_eq!(
        baseline
            .measurements
            .iter()
            .map(|measurement| measurement.platform_id.as_str())
            .collect::<Vec<_>>(),
        PLATFORMS
    );
}

#[test]
fn generator_emits_a_four_platform_review_candidate_without_mutating_manifest() {
    let manifest_path = repo_root().join("third_party_artifacts/manifest.json");
    let manifest_before = fs::read(&manifest_path).unwrap();
    let baseline_bytes = fs::read(fixture_root().join("reviewed-baseline.json")).unwrap();
    let publication: Value = serde_json::from_slice(
        &fs::read(fixture_root().join("verified-publication.json")).unwrap(),
    )
    .unwrap();

    let output = run_generator(&fixture_root());
    assert!(
        output.status.success(),
        "generator failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.ends_with(b"\n"));
    let candidate: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(serde_json::to_vec(&candidate).unwrap(), output.stdout);
    assert_eq!(candidate["kind"], "provider_manifest_update_candidate");
    assert_eq!(candidate["synthetic_fixture_only"], true);
    assert_eq!(
        candidate["quality_baseline_sha256"],
        sha256_bytes(&baseline_bytes)
    );
    assert_eq!(candidate["platforms"].as_array().unwrap().len(), 4);
    assert_eq!(
        candidate["manifest_candidate"]["packs"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let manifest: ArtifactManifest =
        serde_json::from_value(candidate["manifest_candidate"].clone()).unwrap();
    manifest.validate().unwrap();

    for (generated, published) in candidate["platforms"]
        .as_array()
        .unwrap()
        .iter()
        .zip(publication["platforms"].as_array().unwrap())
    {
        assert_eq!(generated["platform_id"], published["platform_id"]);
        assert_eq!(
            generated["pack_asset_name"],
            published["subjects"][0]["name"]
        );
        assert_eq!(generated["pack_sha256"], published["subjects"][0]["sha256"]);
        assert_eq!(
            generated["pack_manifest_asset_name"],
            published["subjects"][1]["name"]
        );
        assert_eq!(
            generated["pack_manifest_sha256"],
            published["subjects"][1]["sha256"]
        );
        assert_eq!(
            generated["sbom_asset_name"],
            published["subjects"][2]["name"]
        );
        assert_eq!(generated["sbom_sha256"], published["subjects"][2]["sha256"]);
        assert_eq!(
            generated["executable_sha256"],
            published["executable"]["sha256"]
        );
        assert_eq!(generated["source_lock_sha256"], SOURCE_LOCK_SHA256);
        assert_eq!(
            generated["quality_baseline_sha256"],
            candidate["quality_baseline_sha256"]
        );
    }

    assert_eq!(fs::read(manifest_path).unwrap(), manifest_before);
}

#[test]
fn generator_refuses_unpublished_or_incompletely_attested_platforms() {
    let unpublished = copy_generator_fixture();
    mutate_json(
        &unpublished.path().join("verified-publication.json"),
        |value| value["platforms"][0]["published"] = json!(false),
    );
    assert_rejected(unpublished.path(), "publication-state");

    let missing_attestation = copy_generator_fixture();
    mutate_json(
        &missing_attestation.path().join("verified-publication.json"),
        |value| {
            value["platforms"][0]["subjects"][0]
                .as_object_mut()
                .unwrap()
                .remove("attestation");
        },
    );
    assert_rejected(missing_attestation.path(), "attestation-contract");
}

#[test]
fn generator_refuses_missing_internal_manifest_or_sbom_digests() {
    for role in ["manifest", "sbom"] {
        let fixture = copy_generator_fixture();
        mutate_json(&fixture.path().join("verified-publication.json"), |value| {
            let subject = value["platforms"][0]["subjects"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|subject| subject["role"] == role)
                .unwrap();
            subject.as_object_mut().unwrap().remove("sha256");
        });
        assert_rejected(fixture.path(), "publication-digest");
    }
}

#[test]
fn generator_refuses_source_lock_and_every_baseline_binding_drift() {
    let source_lock = copy_generator_fixture();
    mutate_json(
        &source_lock.path().join("verified-publication.json"),
        |value| value["source_lock_sha256"] = json!("0".repeat(64)),
    );
    assert_rejected(source_lock.path(), "source-lock-binding");

    let mutations = [
        ("pack_sha256", "0".repeat(64)),
        ("executable_sha256", "1".repeat(64)),
        ("profile_sha256", "2".repeat(64)),
        ("fixture_id", "different-fixture".to_string()),
        ("fixture_sha256", "3".repeat(64)),
        ("request_sha256", "4".repeat(64)),
        ("runner_class", "different-runner".to_string()),
    ];
    for (field, replacement) in mutations {
        let fixture = copy_generator_fixture();
        mutate_json(&fixture.path().join("reviewed-baseline.json"), |value| {
            value["measurements"][0][field] = json!(replacement)
        });
        assert_rejected(fixture.path(), "baseline-binding");
    }

    for (field, replacement) in [
        ("source_lock_sha256", "5".repeat(64)),
        ("pack_version", "2026.07.27-pcr.changed".to_string()),
    ] {
        let fixture = copy_generator_fixture();
        mutate_json(&fixture.path().join("reviewed-baseline.json"), |value| {
            value[field] = json!(replacement)
        });
        assert_rejected(fixture.path(), "baseline-binding");
    }
}

#[test]
fn generator_refuses_noncanonical_publication_or_baseline_bytes() {
    let publication = copy_generator_fixture();
    let path = publication.path().join("verified-publication.json");
    let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    assert_rejected(publication.path(), "canonical-json");

    let baseline = copy_generator_fixture();
    let path = baseline.path().join("reviewed-baseline.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b'\n');
    fs::write(path, bytes).unwrap();
    assert_rejected(baseline.path(), "canonical-json");
}

#[test]
fn generator_rejects_malformed_list_entries_with_stable_contract_errors() {
    let platform = copy_generator_fixture();
    mutate_json(
        &platform.path().join("verified-publication.json"),
        |value| value["platforms"][0] = json!("not-an-object"),
    );
    assert_rejected(platform.path(), "publication-contract");

    let subject = copy_generator_fixture();
    mutate_json(&subject.path().join("verified-publication.json"), |value| {
        value["platforms"][0]["subjects"][0] = json!("not-an-object")
    });
    assert_rejected(subject.path(), "attestation-contract");

    let measurement = copy_generator_fixture();
    mutate_json(
        &measurement.path().join("reviewed-baseline.json"),
        |value| value["measurements"][0] = json!("not-an-object"),
    );
    assert_rejected(measurement.path(), "baseline-binding");
}

#[test]
fn generator_refuses_a_boolean_nearest_rank_p95() {
    let fixture = copy_generator_fixture();
    mutate_json(&fixture.path().join("reviewed-baseline.json"), |value| {
        value["measurements"][0]["samples_ms"] = json!(vec![1; 20]);
        value["measurements"][0]["p95_ms"] = json!(true);
    });
    assert_rejected(fixture.path(), "baseline-binding");
}

#[test]
fn core_release_context_cannot_generate_or_rewrite_the_candidate() {
    let manifest_path = repo_root().join("third_party_artifacts/manifest.json");
    let manifest_before = fs::read(&manifest_path).unwrap();
    let output = run_generator_in_core_release(&fixture_root());
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("core-release-boundary"));
    let named_workflow = run_generator_in_named_core_workflow(&fixture_root());
    assert!(!named_workflow.status.success());
    assert!(String::from_utf8_lossy(&named_workflow.stderr).contains("core-release-boundary"));
    assert_eq!(fs::read(manifest_path).unwrap(), manifest_before);

    let workflow = fs::read_to_string(repo_root().join(".github/workflows/release.yml")).unwrap();
    assert!(!workflow.contains("generate_provider_manifest_update.py"));
}
