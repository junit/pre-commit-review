#[path = "support/artifact_fixture.rs"]
mod artifact_fixture;

use artifact_fixture::{
    executable_fixture_pack, executable_fixture_pack_for_artifact, executable_fixture_pack_with,
    fixture_pack_with_version, manifest, FixturePack,
};
use collect_diff_context_cli::{
    artifacts::contract::{
        canonical_json, sha256_bytes, ArtifactManifest, ArtifactReceipt, ArtifactReport,
        ArtifactReportStatus, ArtifactRole, ArtifactState, CorePackFileBinding, CorePackManifest,
        ProbeId, RevocationEntry, RevocationIndex,
    },
    repository_context_provider::cli_contract::{ProviderRegistry, ProviderRegistryEntry},
    repository_context_provider::contract::{
        AuthorizedProviderProfile, ProviderHardening, ProviderLimits,
    },
};
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

const BINARY: &str = env!("CARGO_BIN_EXE_collect-diff-context-cli");

struct CliFixture {
    _root: TempDir,
    cache_root: PathBuf,
    manifest_path: PathBuf,
    pack_path: PathBuf,
    target_root: PathBuf,
    manifest: ArtifactManifest,
    pack: FixturePack,
}

impl CliFixture {
    fn new() -> Result<Self, Box<dyn Error>> {
        let root = TempDir::new()?;
        let pack = executable_fixture_pack();
        let revocations = RevocationIndex {
            schema_version: 1,
            kind: "third_party_artifact_revocations".to_string(),
            entries: Vec::new(),
        };
        let revocation_bytes = canonical_json(&revocations)?;
        let mut manifest = manifest(&pack.record);
        manifest.revocation_index_sha256 = sha256_bytes(&revocation_bytes);

        let manifest_path = root.path().join("manifest.json");
        let pack_path = root.path().join("gitleaks.tar.gz");
        fs::write(&manifest_path, canonical_json(&manifest)?)?;
        fs::write(&pack_path, &pack.bytes)?;

        Ok(Self {
            cache_root: root.path().join("cache"),
            target_root: root.path().join("target"),
            _root: root,
            manifest_path,
            pack_path,
            manifest,
            pack,
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(BINARY);
        command
            .env("PRE_COMMIT_REVIEW_ARTIFACT_CACHE_DIR", &self.cache_root)
            .env("PRE_COMMIT_REVIEW_FETCH_PROGRESS", "never");
        command
    }

    fn verify(&self) -> Result<Output, Box<dyn Error>> {
        Ok(self
            .command()
            .args([
                "artifacts",
                "verify",
                "--manifest",
                path_text(&self.manifest_path)?,
                "--artifact-id",
                "gitleaks",
                "--platform-id",
                "linux-amd64",
                "--pack",
                path_text(&self.pack_path)?,
            ])
            .output()?)
    }

    fn provision(&self) -> Result<Output, Box<dyn Error>> {
        self.provision_artifact("gitleaks", &self.pack_path)
    }

    fn provision_artifact(
        &self,
        artifact_id: &str,
        pack_path: &Path,
    ) -> Result<Output, Box<dyn Error>> {
        Ok(self
            .command()
            .args([
                "artifacts",
                "provision",
                "--manifest",
                path_text(&self.manifest_path)?,
                "--artifact-id",
                artifact_id,
                "--platform-id",
                "linux-amd64",
                "--target-root",
                path_text(&self.target_root)?,
                "--pack",
                path_text(pack_path)?,
            ])
            .output()?)
    }

    fn doctor(&self) -> Result<Output, Box<dyn Error>> {
        self.doctor_artifact(None)
    }

    fn doctor_artifact(&self, artifact_id: Option<&str>) -> Result<Output, Box<dyn Error>> {
        let mut command = self.command();
        command.args([
            "artifacts",
            "doctor",
            "--target-root",
            path_text(&self.target_root)?,
        ]);
        if let Some(artifact_id) = artifact_id {
            command.args(["--artifact-id", artifact_id]);
        }
        Ok(command.output()?)
    }

    fn seed_target_distribution(&self) -> Result<(), Box<dyn Error>> {
        let distribution = self.target_root.join("runtime/distribution");
        let collector = self
            .target_root
            .join("scripts/bin/collect_diff_context-linux-amd64");
        fs::create_dir_all(&distribution)?;
        fs::create_dir_all(collector.parent().ok_or("collector parent is missing")?)?;

        let manifest_bytes = canonical_json(&self.manifest)?;
        let revocations = RevocationIndex {
            schema_version: 1,
            kind: "third_party_artifact_revocations".to_string(),
            entries: Vec::new(),
        };
        let revocation_bytes = canonical_json(&revocations)?;
        let collector_bytes = b"fixture collector\n";
        fs::write(distribution.join("manifest.json"), &manifest_bytes)?;
        fs::write(distribution.join("revocations.json"), &revocation_bytes)?;
        fs::write(&collector, collector_bytes)?;
        set_mode(&distribution.join("manifest.json"), 0o644)?;
        set_mode(&distribution.join("revocations.json"), 0o644)?;
        set_mode(&collector, 0o755)?;

        let core = CorePackManifest {
            schema_version: 1,
            kind: "pre_commit_review_core_pack".to_string(),
            core_version: "0.1.0".to_string(),
            platform_id: "linux-amd64".to_string(),
            target_triple: "x86_64-unknown-linux-musl".to_string(),
            distribution_manifest_sha256: sha256_bytes(&manifest_bytes),
            revocation_index_sha256: sha256_bytes(&revocation_bytes),
            members: vec![
                core_binding("runtime/distribution/manifest.json", &manifest_bytes),
                core_binding("runtime/distribution/revocations.json", &revocation_bytes),
                core_binding(
                    "scripts/bin/collect_diff_context-linux-amd64",
                    collector_bytes,
                ),
            ],
        };
        core.validate()?;
        fs::write(
            distribution.join("core-pack-manifest.json"),
            canonical_json(&core)?,
        )?;
        Ok(())
    }

    fn install(&self) -> Result<(), Box<dyn Error>> {
        self.seed_target_distribution()?;
        let output = self.provision()?;
        let report = completed_report(&output)?;
        assert_eq!(report.status, ArtifactReportStatus::Completed);
        Ok(())
    }
}

fn core_binding(path: &str, bytes: &[u8]) -> CorePackFileBinding {
    CorePackFileBinding {
        path: path.to_string(),
        mode: if path.starts_with("scripts/bin/") {
            0o755
        } else {
            0o644
        },
        size: bytes.len() as u64,
        sha256: sha256_bytes(bytes),
    }
}

fn set_mode(path: &Path, mode: u32) -> Result<(), Box<dyn Error>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(mode);
        fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

fn path_text(path: &Path) -> Result<&str, Box<dyn Error>> {
    path.to_str().ok_or_else(|| "test path is not UTF-8".into())
}

fn report(output: &Output) -> Result<ArtifactReport, Box<dyn Error>> {
    assert!(
        output.stdout.len() <= 64 * 1024,
        "report exceeded its budget"
    );
    let report: ArtifactReport = serde_json::from_slice(&output.stdout)?;
    report.validate()?;
    assert_eq!(output.stdout, canonical_json(&report)?);
    Ok(report)
}

fn completed_report(output: &Output) -> Result<ArtifactReport, Box<dyn Error>> {
    assert!(
        output.status.success(),
        "command failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = report(output)?;
    assert_eq!(report.status, ArtifactReportStatus::Completed);
    assert!(report.code.is_none());
    Ok(report)
}

fn failed_report(output: &Output, exit_code: i32, code: &str) -> Result<(), Box<dyn Error>> {
    assert_eq!(output.status.code(), Some(exit_code));
    let report = report(output)?;
    assert_eq!(report.status, ArtifactReportStatus::Failed);
    assert_eq!(report.code.as_deref(), Some(code));
    Ok(())
}

fn tree_snapshot(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, Box<dyn Error>> {
    fn visit(
        root: &Path,
        path: &Path,
        snapshot: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) -> Result<(), Box<dyn Error>> {
        let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root)?.to_path_buf();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                snapshot.insert(relative.clone(), b"directory".to_vec());
                visit(root, &path, snapshot)?;
            } else if file_type.is_file() {
                snapshot.insert(relative, fs::read(path)?);
            } else {
                snapshot.insert(relative, b"other".to_vec());
            }
        }
        Ok(())
    }

    let mut snapshot = BTreeMap::new();
    visit(root, root, &mut snapshot)?;
    Ok(snapshot)
}

#[test]
fn parser_failures_are_bounded_json_reports() -> Result<(), Box<dyn Error>> {
    let fixture = CliFixture::new()?;
    let cases = [
        (
            vec![
                "artifacts",
                "verify",
                "--manifest",
                "relative.json",
                "--artifact-id",
                "gitleaks",
                "--platform-id",
                "linux-amd64",
            ],
            "manifest-path-not-absolute",
        ),
        (
            vec!["artifacts", "provision", "--manifest"],
            "argument-value-missing",
        ),
        (vec!["artifacts", "doctor", "--unknown"], "argument-unknown"),
    ];

    for (arguments, code) in cases {
        let output = fixture.command().args(arguments).output()?;
        failed_report(&output, 2, code)?;
    }
    Ok(())
}

#[test]
fn invalid_progress_is_rejected_before_transport() -> Result<(), Box<dyn Error>> {
    let fixture = CliFixture::new()?;
    let missing_pack = fixture._root.path().join("missing.tar.gz");
    let output = fixture
        .command()
        .env("PRE_COMMIT_REVIEW_FETCH_PROGRESS", "sometimes")
        .args([
            "artifacts",
            "verify",
            "--manifest",
            path_text(&fixture.manifest_path)?,
            "--artifact-id",
            "gitleaks",
            "--platform-id",
            "linux-amd64",
            "--pack",
            path_text(&missing_pack)?,
        ])
        .output()?;

    failed_report(&output, 2, "progress-mode-invalid")
}

#[test]
fn invalid_cache_root_is_rejected_before_transport() -> Result<(), Box<dyn Error>> {
    let fixture = CliFixture::new()?;
    let missing_pack = fixture._root.path().join("missing.tar.gz");
    let output = fixture
        .command()
        .env("PRE_COMMIT_REVIEW_ARTIFACT_CACHE_DIR", "relative-cache")
        .args([
            "artifacts",
            "provision",
            "--manifest",
            path_text(&fixture.manifest_path)?,
            "--artifact-id",
            "gitleaks",
            "--platform-id",
            "linux-amd64",
            "--target-root",
            path_text(&fixture.target_root)?,
            "--pack",
            path_text(&missing_pack)?,
        ])
        .output()?;

    failed_report(&output, 1, "cache-root-not-absolute")
}

#[test]
fn unknown_selection_is_rejected_before_transport() -> Result<(), Box<dyn Error>> {
    let fixture = CliFixture::new()?;
    let missing_pack = fixture._root.path().join("missing.tar.gz");
    for (artifact_id, platform_id) in [("unknown", "linux-amd64"), ("gitleaks", "darwin-arm64")] {
        let output = fixture
            .command()
            .args([
                "artifacts",
                "verify",
                "--manifest",
                path_text(&fixture.manifest_path)?,
                "--artifact-id",
                artifact_id,
                "--platform-id",
                platform_id,
                "--pack",
                path_text(&missing_pack)?,
            ])
            .output()?;
        failed_report(&output, 1, "artifact-not-active")?;
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn local_pack_verify_and_provision_emit_compact_reports() -> Result<(), Box<dyn Error>> {
    let fixture = CliFixture::new()?;
    let verify = completed_report(&fixture.verify()?)?;
    assert_eq!(verify.artifact_id.as_deref(), Some("gitleaks"));
    assert_eq!(
        verify.pack_sha256.as_deref(),
        Some(fixture.pack.record.pack_sha256.as_str())
    );

    let provision = completed_report(&fixture.provision()?)?;
    assert_eq!(provision.pack_version.as_deref(), Some("8.30.1-pcr.1"));
    assert!(fixture
        .target_root
        .join("runtime/third-party/gitleaks/8.30.1-pcr.1/bin/gitleaks")
        .is_file());
    assert!(fixture
        .target_root
        .join("runtime/artifact-receipts/gitleaks.json")
        .is_file());

    let progress = fixture
        .command()
        .env("PRE_COMMIT_REVIEW_FETCH_PROGRESS", "always")
        .args([
            "artifacts",
            "verify",
            "--manifest",
            path_text(&fixture.manifest_path)?,
            "--artifact-id",
            "gitleaks",
            "--platform-id",
            "linux-amd64",
            "--pack",
            path_text(&fixture.pack_path)?,
        ])
        .output()?;
    completed_report(&progress)?;
    assert!(!progress.stderr.is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn probe_failure_does_not_expose_child_output() -> Result<(), Box<dyn Error>> {
    let root = TempDir::new()?;
    let pack = executable_fixture_pack_with(
        b"#!/bin/sh\nprintf 'untrusted-child-stderr' >&2\nprintf 'wrong-version\\n'\nexit 9\n",
    );
    let manifest = manifest(&pack.record);
    let manifest_path = root.path().join("manifest.json");
    let pack_path = root.path().join("pack.tar.gz");
    fs::write(&manifest_path, canonical_json(&manifest)?)?;
    fs::write(&pack_path, &pack.bytes)?;
    let output = Command::new(BINARY)
        .env(
            "PRE_COMMIT_REVIEW_ARTIFACT_CACHE_DIR",
            root.path().join("cache"),
        )
        .env("PRE_COMMIT_REVIEW_FETCH_PROGRESS", "never")
        .args([
            "artifacts",
            "verify",
            "--manifest",
            path_text(&manifest_path)?,
            "--artifact-id",
            "gitleaks",
            "--platform-id",
            "linux-amd64",
            "--pack",
            path_text(&pack_path)?,
        ])
        .output()?;

    failed_report(&output, 1, "probe-version-output")?;
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("untrusted-child"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn doctor_is_read_only_and_detects_changed_executable() -> Result<(), Box<dyn Error>> {
    let fixture = CliFixture::new()?;
    fixture.install()?;

    completed_report(&fixture.doctor()?)?;
    let executable = fixture
        .target_root
        .join("runtime/third-party/gitleaks/8.30.1-pcr.1/bin/gitleaks");
    fs::write(&executable, b"changed executable\n")?;
    let before = tree_snapshot(&fixture.target_root)?;
    let output = fixture.doctor()?;
    failed_report(&output, 1, "artifact-binding-mismatch")?;
    assert_eq!(tree_snapshot(&fixture.target_root)?, before);
    Ok(())
}

#[cfg(unix)]
#[test]
fn doctor_checks_every_receipt_and_reports_a_sorted_aggregate() -> Result<(), Box<dyn Error>> {
    let mut fixture = CliFixture::new()?;
    let second = executable_fixture_pack_for_artifact("secondary-sanitizer");
    let second_pack_path = fixture._root.path().join("secondary-sanitizer.tar.gz");
    fs::write(&second_pack_path, &second.bytes)?;
    fixture.manifest.packs.push(second.record.clone());
    fixture.manifest.packs.sort_by(|left, right| {
        (&left.artifact_id, &left.platform_id, &left.pack_version).cmp(&(
            &right.artifact_id,
            &right.platform_id,
            &right.pack_version,
        ))
    });
    fs::write(&fixture.manifest_path, canonical_json(&fixture.manifest)?)?;

    fixture.seed_target_distribution()?;
    completed_report(&fixture.provision()?)?;
    completed_report(&fixture.provision_artifact("secondary-sanitizer", &second_pack_path)?)?;

    let aggregate = completed_report(&fixture.doctor()?)?;
    assert!(aggregate.artifact_id.is_none());
    assert_eq!(
        aggregate
            .artifacts
            .iter()
            .map(|artifact| artifact.artifact_id.as_str())
            .collect::<Vec<_>>(),
        ["gitleaks", "secondary-sanitizer"]
    );

    let single = completed_report(&fixture.doctor_artifact(Some("gitleaks"))?)?;
    assert_eq!(single.artifact_id.as_deref(), Some("gitleaks"));
    assert!(single.artifacts.is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn doctor_reruns_live_probes_on_the_installed_executable() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;

    let fixture = CliFixture::new()?;
    fixture.install()?;
    let executable = fixture
        .target_root
        .join("runtime/third-party/gitleaks/8.30.1-pcr.1/bin/gitleaks");
    let mut permissions = fs::metadata(&executable)?.permissions();
    permissions.set_mode(0o644);
    fs::set_permissions(executable, permissions)?;

    failed_report(&fixture.doctor()?, 1, "trusted-runtime-executable-invalid")?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn doctor_rejects_a_revoked_receipt_before_an_active_replacement() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;

    let mut fixture = CliFixture::new()?;
    fixture.install()?;
    let installed = fixture.manifest.packs[0].clone();
    let mut revoked = installed.clone();
    revoked.state = ArtifactState::Revoked;
    revoked.revoked_reason = Some("fixture revocation".to_string());
    revoked.replacement_pack_version = Some("8.30.1-pcr.2".to_string());
    let replacement = fixture_pack_with_version("8.30.1-pcr.2");
    let revocations = RevocationIndex {
        schema_version: 1,
        kind: "third_party_artifact_revocations".to_string(),
        entries: vec![RevocationEntry {
            pack_sha256: installed.pack_sha256,
            artifact_id: "gitleaks".to_string(),
            platform_id: "linux-amd64".to_string(),
            pack_version: "8.30.1-pcr.1".to_string(),
            reason: "fixture revocation".to_string(),
            replacement_pack_version: Some("8.30.1-pcr.2".to_string()),
        }],
    };
    let revocation_bytes = canonical_json(&revocations)?;
    fixture.manifest.revocation_index_sha256 = sha256_bytes(&revocation_bytes);
    fixture.manifest.packs = vec![revoked, replacement.record];
    fixture.manifest.packs.sort_by(|left, right| {
        (&left.artifact_id, &left.platform_id, &left.pack_version).cmp(&(
            &right.artifact_id,
            &right.platform_id,
            &right.pack_version,
        ))
    });
    let manifest_bytes = canonical_json(&fixture.manifest)?;
    let distribution = fixture.target_root.join("runtime/distribution");
    fs::write(distribution.join("manifest.json"), &manifest_bytes)?;
    fs::write(distribution.join("revocations.json"), &revocation_bytes)?;
    let core_path = distribution.join("core-pack-manifest.json");
    let mut core: CorePackManifest = serde_json::from_slice(&fs::read(&core_path)?)?;
    core.distribution_manifest_sha256 = sha256_bytes(&manifest_bytes);
    core.revocation_index_sha256 = sha256_bytes(&revocation_bytes);
    core.members[0] = core_binding("runtime/distribution/manifest.json", &manifest_bytes);
    core.members[1] = core_binding("runtime/distribution/revocations.json", &revocation_bytes);
    set_mode(&distribution.join("manifest.json"), 0o644)?;
    set_mode(&distribution.join("revocations.json"), 0o644)?;
    fs::write(&core_path, canonical_json(&core)?)?;

    let executable = fixture
        .target_root
        .join("runtime/third-party/gitleaks/8.30.1-pcr.1/bin/gitleaks");
    let mut permissions = fs::metadata(&executable)?.permissions();
    permissions.set_mode(0o644);
    fs::set_permissions(executable, permissions)?;

    failed_report(&fixture.doctor()?, 1, "artifact-revoked")?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn doctor_requires_a_registry_for_a_provider_receipt() -> Result<(), Box<dyn Error>> {
    let mut fixture = CliFixture::new()?;
    fixture.install()?;
    let record = &mut fixture.manifest.packs[0];
    record.artifact_role = ArtifactRole::RepositoryContextProvider;
    record.version_probe = ProbeId::RustAnalyzerVersionV1;
    record.capability_probe = ProbeId::RustAnalyzerStdioV1;
    record.default_configuration_sha256 = None;
    record.quality_baseline_sha256 = Some("7".repeat(64));
    let manifest_bytes = canonical_json(&fixture.manifest)?;
    let distribution = fixture.target_root.join("runtime/distribution");
    fs::write(distribution.join("manifest.json"), &manifest_bytes)?;
    let core_path = distribution.join("core-pack-manifest.json");
    let mut core: CorePackManifest = serde_json::from_slice(&fs::read(&core_path)?)?;
    core.distribution_manifest_sha256 = sha256_bytes(&manifest_bytes);
    core.members[0] = core_binding("runtime/distribution/manifest.json", &manifest_bytes);
    set_mode(&distribution.join("manifest.json"), 0o644)?;
    fs::write(&core_path, canonical_json(&core)?)?;

    let receipt_path = fixture
        .target_root
        .join("runtime/artifact-receipts/gitleaks.json");
    let mut receipt: ArtifactReceipt = serde_json::from_slice(&fs::read(&receipt_path)?)?;
    receipt.distribution_manifest_sha256 = sha256_bytes(&manifest_bytes);
    receipt.probes[0].probe_id = ProbeId::RustAnalyzerVersionV1;
    receipt.probes[1].probe_id = ProbeId::RustAnalyzerStdioV1;
    fs::write(&receipt_path, canonical_json(&receipt)?)?;

    failed_report(&fixture.doctor()?, 1, "provider-registry-required")?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn doctor_requires_provider_registry_to_bind_the_installed_executable() -> Result<(), Box<dyn Error>>
{
    use std::os::unix::fs::PermissionsExt;

    let mut fixture = CliFixture::new()?;
    fixture.install()?;
    let record = &mut fixture.manifest.packs[0];
    record.artifact_role = ArtifactRole::RepositoryContextProvider;
    record.version_probe = ProbeId::RustAnalyzerVersionV1;
    record.capability_probe = ProbeId::RustAnalyzerStdioV1;
    record.default_configuration_sha256 = None;
    record.quality_baseline_sha256 = Some("7".repeat(64));
    let manifest_bytes = canonical_json(&fixture.manifest)?;
    let distribution = fixture.target_root.join("runtime/distribution");
    fs::write(distribution.join("manifest.json"), &manifest_bytes)?;
    let core_path = distribution.join("core-pack-manifest.json");
    let mut core: CorePackManifest = serde_json::from_slice(&fs::read(&core_path)?)?;
    core.distribution_manifest_sha256 = sha256_bytes(&manifest_bytes);
    core.members[0] = core_binding("runtime/distribution/manifest.json", &manifest_bytes);
    set_mode(&distribution.join("manifest.json"), 0o644)?;
    fs::write(&core_path, canonical_json(&core)?)?;

    let receipt_path = fixture
        .target_root
        .join("runtime/artifact-receipts/gitleaks.json");
    let mut receipt: ArtifactReceipt = serde_json::from_slice(&fs::read(&receipt_path)?)?;
    receipt.distribution_manifest_sha256 = sha256_bytes(&manifest_bytes);
    receipt.probes[0].probe_id = ProbeId::RustAnalyzerVersionV1;
    receipt.probes[1].probe_id = ProbeId::RustAnalyzerStdioV1;
    fs::write(&receipt_path, canonical_json(&receipt)?)?;

    let providers = fixture.target_root.join("runtime/providers");
    fs::create_dir_all(&providers)?;
    let installed = fixture
        .target_root
        .join("runtime/third-party/gitleaks/8.30.1-pcr.1/bin/gitleaks");
    let alternate = providers.join("unbound-rust-analyzer");
    fs::copy(&installed, &alternate)?;
    let mut permissions = fs::metadata(&alternate)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&alternate, permissions)?;
    let profile_path = providers.join("rust-analyzer.profile.json");
    let mut profile = AuthorizedProviderProfile {
        schema_version: 1,
        kind: "repository_context_provider_profile".to_string(),
        provider_kind: "rust-analyzer".to_string(),
        provider_version: "8.30.1".to_string(),
        executable_sha256: fixture.pack.record.executable.sha256.clone(),
        configuration_sha256: "0".repeat(64),
        target_triple: "x86_64-unknown-linux-musl".to_string(),
        toolchain_mode: "none".to_string(),
        arguments: vec!["--stdio".to_string()],
        hardening: ProviderHardening {
            cargo_build_scripts: false,
            cargo_no_deps: true,
            cargo_sysroot: None,
            cargo_sysroot_src: None,
            proc_macro: false,
            check_on_save: false,
            workspace_discovery: false,
            empty_path: true,
            server_status_notification: true,
        },
        maximum_limits: ProviderLimits::maximum(),
    };
    profile.configuration_sha256 = profile.canonical_configuration_sha256();
    profile.validate()?;
    let profile_bytes = canonical_json(&profile)?;
    fs::write(&profile_path, &profile_bytes)?;
    let registry = ProviderRegistry {
        schema_version: 1,
        kind: "repository_context_provider_registry".to_string(),
        entries: vec![ProviderRegistryEntry {
            provider_id: "rust-analyzer-project-pack".to_string(),
            provider_kind: profile.provider_kind.clone(),
            provider_version: profile.provider_version.clone(),
            target_triple: profile.target_triple.clone(),
            profile_path,
            profile_sha256: sha256_bytes(&profile_bytes),
            executable_path: alternate,
            executable_sha256: profile.executable_sha256.clone(),
            configuration_sha256: profile.configuration_sha256.clone(),
            toolchain_mode: profile.toolchain_mode.clone(),
        }],
    };
    registry.validate()?;
    fs::write(
        providers.join("provider-registry.json"),
        canonical_json(&registry)?,
    )?;

    failed_report(&fixture.doctor()?, 1, "provider-registry-entry-missing")?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn doctor_detects_missing_receipt_and_corrupt_revocations() -> Result<(), Box<dyn Error>> {
    let missing = CliFixture::new()?;
    missing.install()?;
    fs::remove_file(
        missing
            .target_root
            .join("runtime/artifact-receipts/gitleaks.json"),
    )?;
    failed_report(&missing.doctor()?, 1, "artifact-file-open")?;

    let corrupt = CliFixture::new()?;
    corrupt.install()?;
    fs::write(
        corrupt
            .target_root
            .join("runtime/distribution/revocations.json"),
        b"not-json",
    )?;
    failed_report(&corrupt.doctor()?, 1, "revocation-index-json")?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn doctor_detects_revoked_state_and_stale_provider_paths() -> Result<(), Box<dyn Error>> {
    let revoked = CliFixture::new()?;
    revoked.install()?;
    let manifest_path = revoked
        .target_root
        .join("runtime/distribution/manifest.json");
    let mut manifest = revoked.manifest.clone();
    manifest.packs[0].state = ArtifactState::Revoked;
    manifest.packs[0].revoked_reason = Some("fixture revocation".to_string());
    let manifest_bytes = canonical_json(&manifest)?;
    fs::write(&manifest_path, &manifest_bytes)?;
    let core_path = revoked
        .target_root
        .join("runtime/distribution/core-pack-manifest.json");
    let mut core: CorePackManifest = serde_json::from_slice(&fs::read(&core_path)?)?;
    core.distribution_manifest_sha256 = sha256_bytes(&manifest_bytes);
    core.members[0] = core_binding("runtime/distribution/manifest.json", &manifest_bytes);
    set_mode(&manifest_path, 0o644)?;
    fs::write(&core_path, canonical_json(&core)?)?;
    failed_report(&revoked.doctor()?, 1, "artifact-revoked")?;

    let stale = CliFixture::new()?;
    stale.install()?;
    let stale_root = stale._root.path().join("old-target");
    let providers = stale.target_root.join("runtime/providers");
    fs::create_dir_all(&providers)?;
    let registry = ProviderRegistry {
        schema_version: 1,
        kind: "repository_context_provider_registry".to_string(),
        entries: vec![ProviderRegistryEntry {
            provider_id: "rust-analyzer-project-pack".to_string(),
            provider_kind: "rust-analyzer".to_string(),
            provider_version: "2026-07-27".to_string(),
            target_triple: "x86_64-unknown-linux-gnu".to_string(),
            profile_path: stale_root.join("runtime/providers/rust-analyzer.profile.json"),
            profile_sha256: "0".repeat(64),
            executable_path: stale_root.join("runtime/third-party/rust-analyzer/bin/rust-analyzer"),
            executable_sha256: "1".repeat(64),
            configuration_sha256: "2".repeat(64),
            toolchain_mode: "none".to_string(),
        }],
    };
    registry.validate()?;
    fs::write(
        providers.join("provider-registry.json"),
        serde_json::to_vec(&registry)?,
    )?;
    failed_report(&stale.doctor()?, 1, "provider-path-stale")?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn doctor_binds_the_receipt_to_the_core_platform() -> Result<(), Box<dyn Error>> {
    let fixture = CliFixture::new()?;
    fixture.install()?;
    let core_path = fixture
        .target_root
        .join("runtime/distribution/core-pack-manifest.json");
    let mut core: CorePackManifest = serde_json::from_slice(&fs::read(&core_path)?)?;
    let linux_collector = fixture
        .target_root
        .join("scripts/bin/collect_diff_context-linux-amd64");
    let collector_bytes = fs::read(&linux_collector)?;
    fs::remove_file(linux_collector)?;
    let darwin_collector = fixture
        .target_root
        .join("scripts/bin/collect_diff_context-darwin-arm64");
    fs::write(&darwin_collector, &collector_bytes)?;
    core.platform_id = "darwin-arm64".to_string();
    core.target_triple = "aarch64-apple-darwin".to_string();
    set_mode(&darwin_collector, 0o755)?;
    core.members[2] = core_binding(
        "scripts/bin/collect_diff_context-darwin-arm64",
        &collector_bytes,
    );
    core.validate()?;
    fs::write(core_path, canonical_json(&core)?)?;

    failed_report(&fixture.doctor()?, 1, "target-platform-mismatch")?;
    Ok(())
}
