use collect_diff_context_cli::artifacts::{
    contract::{sha256_bytes, ArtifactManifest, ArtifactState},
    provider::{generate_provider_authorization, select_provider_install_record, VerifiedProvider},
};
use collect_diff_context_cli::repository_context_provider::contract::ProviderLimits;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::TempDir;

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

fn staged_provider(root: &Path, executable: &[u8]) -> VerifiedProvider {
    let relative =
        PathBuf::from("runtime/third-party/rust-analyzer/2026.07.27-pcr.1/bin/rust-analyzer");
    let path = root.join(&relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, executable).unwrap();
    VerifiedProvider {
        staging_target: root.to_path_buf(),
        provider_version: "2026-07-27".to_string(),
        executable_relative_path: relative,
        executable_sha256: sha256_bytes(executable),
        target_triple: "x86_64-unknown-linux-musl".to_string(),
    }
}

#[test]
fn generated_authorization_uses_final_paths_and_delivery_four_bindings() {
    let final_parent = TempDir::new().unwrap();
    let final_target = final_parent.path().join("managed-skill");
    let expected_target = fs::canonicalize(final_parent.path())
        .unwrap()
        .join("managed-skill");
    let first_stage = TempDir::new().unwrap();
    let second_stage = TempDir::new().unwrap();
    let first = staged_provider(first_stage.path(), b"verified provider bytes");
    let second = staged_provider(second_stage.path(), b"verified provider bytes");

    let generated = generate_provider_authorization(&final_target, &first).unwrap();
    generated.profile.validate().unwrap();
    generated.registry.validate().unwrap();
    generated
        .registry
        .validate_profile_binding(&generated.profile)
        .unwrap();

    assert_eq!(generated.profile.provider_kind, "rust-analyzer");
    assert_eq!(generated.profile.provider_version, first.provider_version);
    assert_eq!(generated.profile.executable_sha256, first.executable_sha256);
    assert_eq!(generated.profile.target_triple, first.target_triple);
    assert_eq!(generated.profile.toolchain_mode, "none");
    assert_eq!(generated.profile.arguments, ["--stdio"]);
    assert_eq!(generated.profile.maximum_limits, ProviderLimits::maximum());
    assert_eq!(
        generated.profile.configuration_sha256,
        generated.profile.canonical_configuration_sha256()
    );
    assert!(!generated.profile.hardening.cargo_build_scripts);
    assert!(generated.profile.hardening.cargo_no_deps);
    assert!(!generated.profile.hardening.proc_macro);
    assert!(generated.profile.hardening.empty_path);
    assert!(generated.profile.hardening.server_status_notification);

    let entry = &generated.registry.entries[0];
    assert_eq!(entry.provider_id, "rust-analyzer-project-pack");
    assert_eq!(
        entry.profile_path,
        expected_target.join("runtime/providers/rust-analyzer.profile.json")
    );
    assert_eq!(
        entry.executable_path,
        expected_target.join(&first.executable_relative_path)
    );
    assert_eq!(entry.profile_sha256, generated.profile.sha256());
    assert_eq!(entry.executable_sha256, first.executable_sha256);
    assert_eq!(
        entry.configuration_sha256,
        generated.profile.configuration_sha256
    );

    assert_eq!(
        generated.profile_bytes,
        serde_json::to_vec(&generated.profile).unwrap()
    );
    assert_eq!(
        generated.registry_bytes,
        serde_json::to_vec(&generated.registry).unwrap()
    );
    assert!(!generated.profile_bytes.ends_with(b"\n"));
    assert!(!generated.registry_bytes.ends_with(b"\n"));
    assert_eq!(
        sha256_bytes(&generated.profile_bytes),
        generated.profile.sha256()
    );

    let moved_stage = generate_provider_authorization(&final_target, &second).unwrap();
    assert_eq!(moved_stage.profile_bytes, generated.profile_bytes);
    assert_eq!(moved_stage.registry_bytes, generated.registry_bytes);
}

#[test]
fn generated_authorization_rejects_unresolved_escape_and_digest_drift() {
    let final_parent = TempDir::new().unwrap();
    let final_target = final_parent.path().join("managed-skill");
    let stage = TempDir::new().unwrap();
    let verified = staged_provider(stage.path(), b"verified provider bytes");

    let unresolved = final_parent.path().join("missing-parent/managed-skill");
    assert_eq!(
        generate_provider_authorization(&unresolved, &verified)
            .unwrap_err()
            .code,
        "provider-final-target"
    );

    assert_eq!(
        generate_provider_authorization(Path::new("relative-target"), &verified)
            .unwrap_err()
            .code,
        "provider-final-target"
    );

    let mut escaped = verified.clone();
    escaped.executable_relative_path = PathBuf::from("../rust-analyzer");
    assert_eq!(
        generate_provider_authorization(&final_target, &escaped)
            .unwrap_err()
            .code,
        "provider-staging-path"
    );

    let mut absolute = verified.clone();
    absolute.executable_relative_path = stage.path().join("rust-analyzer");
    assert_eq!(
        generate_provider_authorization(&final_target, &absolute)
            .unwrap_err()
            .code,
        "provider-staging-path"
    );

    let mut missing = verified.clone();
    missing.executable_relative_path = PathBuf::from("missing/rust-analyzer");
    assert_eq!(
        generate_provider_authorization(&final_target, &missing)
            .unwrap_err()
            .code,
        "provider-staging-path"
    );

    let non_regular_path = stage.path().join("non-regular");
    fs::create_dir(&non_regular_path).unwrap();
    let mut non_regular = verified.clone();
    non_regular.executable_relative_path = PathBuf::from("non-regular");
    assert_eq!(
        generate_provider_authorization(&final_target, &non_regular)
            .unwrap_err()
            .code,
        "provider-staging-path"
    );

    let mut drifted = verified;
    drifted.executable_sha256 = "0".repeat(64);
    assert_eq!(
        generate_provider_authorization(&final_target, &drifted)
            .unwrap_err()
            .code,
        "provider-executable-binding"
    );
}

#[test]
fn generated_authorization_resolves_the_existing_final_parent() {
    let final_parent = TempDir::new().unwrap();
    fs::create_dir(final_parent.path().join("nested")).unwrap();
    let final_target = final_parent
        .path()
        .join("nested")
        .join("..")
        .join("managed-skill");
    let expected_target = fs::canonicalize(final_parent.path())
        .unwrap()
        .join("managed-skill");
    let stage = TempDir::new().unwrap();
    let verified = staged_provider(stage.path(), b"verified provider bytes");

    let generated = generate_provider_authorization(&final_target, &verified).unwrap();

    assert_eq!(
        generated.registry.entries[0].profile_path,
        expected_target.join("runtime/providers/rust-analyzer.profile.json")
    );
    assert_eq!(
        generated.registry.entries[0].executable_path,
        expected_target.join(&verified.executable_relative_path)
    );
}
