use collect_diff_context_cli::artifacts::{
    contract::{ArtifactManifest, ArtifactState},
    provider::select_provider_install_record,
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn reviewed_candidate_manifest() -> ArtifactManifest {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let output = Command::new("python3")
        .arg(repository.join("scripts/generate_provider_manifest_update.py"))
        .arg("--fixture")
        .arg(repository.join("tests/fixtures/provider-release"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "candidate generation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let candidate: Value = serde_json::from_slice(&output.stdout).unwrap();
    serde_json::from_value(candidate["manifest_candidate"].clone()).unwrap()
}

#[test]
fn provider_install_selects_one_active_current_platform_record() {
    let manifest = reviewed_candidate_manifest();
    let record = select_provider_install_record(&manifest, "linux-amd64").unwrap();

    assert_eq!(record.artifact_id, "rust-analyzer");
    assert_eq!(record.platform_id, "linux-amd64");
    assert_eq!(record.pack_version, "2026.07.27-pcr.1");
}

#[test]
fn provider_install_rejects_wrong_missing_and_revoked_platform_records() {
    let manifest = reviewed_candidate_manifest();
    let wrong = select_provider_install_record(&manifest, "linux-arm64").unwrap_err();
    assert_eq!(wrong.code, "artifact-not-active");

    let mut missing = manifest.clone();
    missing
        .packs
        .retain(|record| record.platform_id != "linux-amd64");
    let missing = select_provider_install_record(&missing, "linux-amd64").unwrap_err();
    assert_eq!(missing.code, "artifact-not-active");

    let mut revoked = manifest;
    let record = revoked
        .packs
        .iter_mut()
        .find(|record| record.platform_id == "linux-amd64")
        .unwrap();
    record.state = ArtifactState::Revoked;
    record.revoked_reason = Some("fixture revocation".to_string());
    let revoked = select_provider_install_record(&revoked, "linux-amd64").unwrap_err();
    assert_eq!(revoked.code, "artifact-not-active");
}
