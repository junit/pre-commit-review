use collect_diff_context_cli::artifacts::contract::{canonical_json, ArtifactManifest};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn fixture_root() -> PathBuf {
    repo_root().join("tests/fixtures/provider-release")
}

fn run_generator_with_manifest(fixture: &Path, manifest: &Path) -> Output {
    Command::new("python3")
        .arg(repo_root().join("scripts/generate_provider_manifest_update.py"))
        .arg("--fixture")
        .arg(fixture)
        .arg("--manifest")
        .arg(manifest)
        .output()
        .unwrap()
}

fn run_generator(fixture: &Path) -> Output {
    run_generator_with_manifest(fixture, &fixture_root().join("base-manifest.json"))
}

fn assert_manifest_state_rejected(output: Output) {
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("manifest-state"));
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

#[test]
fn generator_is_idempotent_for_reviewed_provider_records_and_rejects_drift() {
    let initial = run_generator(&fixture_root());
    assert!(
        initial.status.success(),
        "generator failed: {}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let candidate: Value = serde_json::from_slice(&initial.stdout).unwrap();
    let manifest: ArtifactManifest =
        serde_json::from_value(candidate["manifest_candidate"].clone()).unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let manifest_path = fixture.path().join("manifest.json");
    fs::write(&manifest_path, canonical_json(&manifest).unwrap()).unwrap();

    let repeated = run_generator_with_manifest(&fixture_root(), &manifest_path);
    assert!(
        repeated.status.success(),
        "idempotent generation failed: {}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    let repeated_candidate: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(
        repeated_candidate["manifest_candidate"],
        candidate["manifest_candidate"]
    );

    let canonical = String::from_utf8(canonical_json(&manifest).unwrap()).unwrap();
    for (original, replacement) in [
        (
            "\"expected_compressed_size\":16000001",
            "\"expected_compressed_size\":16000002",
        ),
        (
            "\"expected_compressed_size\":16000001",
            "\"expected_compressed_size\":16000001.0",
        ),
        (
            "\"expected_compressed_size\":16000001,\"max_compressed_size\":33554432",
            "\"max_compressed_size\":33554432,\"expected_compressed_size\":16000001",
        ),
        ("\"schema_version\":1", "\"schema_version\":true"),
        (
            "\"packs\":[",
            "\"packs\":[{\"artifact_id\":\"gitleaks\",\"platform_id\":\"linux-amd64\",\"pack_version\":\"8.30.1-pcr.1\"},",
        ),
    ] {
        let drifted = canonical.replacen(original, replacement, 1);
        assert_ne!(drifted, canonical);
        fs::write(&manifest_path, drifted).unwrap();
        assert_manifest_state_rejected(run_generator_with_manifest(
            &fixture_root(),
            &manifest_path,
        ));
    }

    let malformed = json!({
        "schema_version": 1,
        "kind": "third_party_artifacts",
        "release_repository": "junit/pre-commit-review",
        "revocation_index_sha256": "e62256210a5f27606e808c36005ae9052aa900a5b890b0976367c05b62cf0457",
        "packs": ["not-an-object"],
    });
    fs::write(&manifest_path, serde_json::to_vec(&malformed).unwrap()).unwrap();
    assert_manifest_state_rejected(run_generator_with_manifest(&fixture_root(), &manifest_path));
}

#[test]
fn generator_rejects_boolean_schema_versions() {
    let publication = copy_generator_fixture();
    mutate_json(
        &publication.path().join("verified-publication.json"),
        |value| value["schema_version"] = json!(true),
    );
    let output = run_generator(publication.path());
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("publication-contract"));

    let baseline = copy_generator_fixture();
    mutate_json(&baseline.path().join("reviewed-baseline.json"), |value| {
        value["schema_version"] = json!(true)
    });
    let output = run_generator(baseline.path());
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("baseline-binding"));
}
