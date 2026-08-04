use collect_diff_context_cli::artifacts::{
    contract::{
        canonical_json, sha256_bytes, ArtifactFileBinding, ArtifactManifest, ArtifactPackRecord,
        ArtifactRole, ArtifactState, PackFileRecord, PackFileRole, PackFormat, PackManifest,
        ProbeId, SourceLock,
    },
    pack::{verify_pack, VerifyLimits},
};
use flate2::{write::GzEncoder, Compression, GzBuilder};
use serde_json::json;
use std::{fs, io::Write, path::Path, process::Command};

const ZERO_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Clone, Copy)]
enum ArchiveShape {
    Valid,
    ParentTraversal,
    AbsolutePath,
    AlternateDataStream,
    Symlink,
    Hardlink,
    CharacterDevice,
    Sparse,
    DuplicatePath,
    CaseFoldCollision,
    UnexpectedFile,
    OversizedMetadata,
    TooManyEntries,
    UnsortedPaths,
    NonzeroHeaderMetadata,
    MissingEndBlock,
    NonzeroGzipMtime,
    ManifestIdentityMismatch,
    ManifestDigestMismatch,
    ExecutableDigestMismatch,
    LicenseDigestMismatch,
    SbomDigestMismatch,
    InvalidSbomComponent,
    InvalidSbomSource,
    MissingSbomLicense,
    InvalidSbomEvidence,
    NoncanonicalSbomJson,
    OuterDigestMismatch,
    OuterSizeMismatch,
    TrailingGzipData,
}

struct FixturePack {
    bytes: Vec<u8>,
    record: ArtifactPackRecord,
    executable_sha256: String,
}

#[derive(Clone)]
struct Member {
    path: String,
    data: Vec<u8>,
    entry_type: u8,
    link_name: String,
    mode: u32,
    uid: u64,
    gid: u64,
    mtime: u64,
}

impl Member {
    fn file(path: &str, data: Vec<u8>, mode: u32) -> Self {
        Self {
            path: path.to_string(),
            data,
            entry_type: b'0',
            link_name: String::new(),
            mode,
            uid: 0,
            gid: 0,
            mtime: 0,
        }
    }

    fn special(path: &str, entry_type: u8, link_name: &str) -> Self {
        Self {
            path: path.to_string(),
            data: Vec::new(),
            entry_type,
            link_name: link_name.to_string(),
            mode: 0o644,
            uid: 0,
            gid: 0,
            mtime: 0,
        }
    }
}

fn base_record() -> ArtifactPackRecord {
    ArtifactPackRecord {
        artifact_id: "gitleaks".to_string(),
        artifact_role: ArtifactRole::Sanitizer,
        tool_version: "8.30.1".to_string(),
        upstream_repository: "gitleaks/gitleaks".to_string(),
        upstream_tag: "v8.30.1".to_string(),
        upstream_commit: "83d9cd684c87d95d656c1458ef04895a7f1cbd8e".to_string(),
        source_lock_sha256: "659556055e7366c27886b14b0bd94104b8ab77df2584da729350f43d3ef8e3a0"
            .to_string(),
        platform_id: "linux-amd64".to_string(),
        target_triple: "x86_64-unknown-linux-musl".to_string(),
        state: ArtifactState::Active,
        pack_version: "8.30.1-pcr.1".to_string(),
        project_release_tag: "artifact-gitleaks-8.30.1-pcr.1".to_string(),
        project_asset_name: "gitleaks-8.30.1-pcr.1-linux-amd64.tar.gz".to_string(),
        expected_compressed_size: 1,
        max_compressed_size: 1,
        pack_sha256: ZERO_SHA256.to_string(),
        pack_manifest_sha256: ZERO_SHA256.to_string(),
        sbom_sha256: ZERO_SHA256.to_string(),
        pack_format: PackFormat::NormalizedTarGzipV1,
        executable: ArtifactFileBinding {
            path: "bin/gitleaks".to_string(),
            size: 1,
            sha256: ZERO_SHA256.to_string(),
        },
        version_probe: ProbeId::GitleaksVersionV1,
        capability_probe: ProbeId::GitleaksStdinJsonV1,
        expected_version: "8.30.1".to_string(),
        license_component: "gitleaks".to_string(),
        license_files: vec![ArtifactFileBinding {
            path: "licenses/GITLEAKS-LICENSE".to_string(),
            size: 1,
            sha256: ZERO_SHA256.to_string(),
        }],
        sbom_component: "pkg:github/gitleaks/gitleaks@8.30.1".to_string(),
        default_configuration_sha256: Some(
            "18bd02d1fac81e5642a2302766263d0bf2fcf61152e25ba10a8d6dc22df5142b".to_string(),
        ),
        quality_baseline_sha256: None,
        revoked_reason: None,
        replacement_pack_version: None,
    }
}

fn sbom_bytes(
    record: &ArtifactPackRecord,
    executable_sha256: &str,
    upstream_archive_sha256: &str,
    shape: ArchiveShape,
) -> Vec<u8> {
    let component = if matches!(shape, ArchiveShape::InvalidSbomComponent) {
        "pkg:github/example/wrong@1.0.0"
    } else {
        record.sbom_component.as_str()
    };
    let source_url = if matches!(shape, ArchiveShape::InvalidSbomSource) {
        "https://example.invalid/gitleaks.tar.gz".to_string()
    } else {
        format!(
            "https://github.com/{}/releases/download/{}/gitleaks_8.30.1_linux_x64.tar.gz",
            record.upstream_repository, record.upstream_tag
        )
    };
    let evidence_scope = if matches!(shape, ArchiveShape::InvalidSbomEvidence) {
        "complete-transitive-closure"
    } else {
        "component-evidence"
    };
    let licenses = if matches!(shape, ArchiveShape::MissingSbomLicense) {
        Vec::new()
    } else {
        vec![json!({ "license": { "id": "MIT" } })]
    };
    let pack_ref = format!(
        "urn:pre-commit-review:pack:{}:{}:{}",
        record.artifact_id, record.pack_version, record.platform_id
    );

    serde_json::to_vec(&json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "bom-ref": pack_ref,
                "name": format!("pre-commit-review-{}-pack", record.artifact_id),
                "version": record.pack_version
            }
        },
        "components": [{
            "type": "application",
            "bom-ref": record.sbom_component,
            "name": record.license_component,
            "version": record.tool_version,
            "purl": component,
            "hashes": [{ "alg": "SHA-256", "content": executable_sha256 }],
            "licenses": licenses,
            "externalReferences": [{
                "type": "distribution",
                "url": source_url,
                "hashes": [{ "alg": "SHA-256", "content": upstream_archive_sha256 }]
            }],
            "properties": [
                { "name": "pre-commit-review:artifact-id", "value": record.artifact_id },
                { "name": "pre-commit-review:pack-version", "value": record.pack_version },
                { "name": "pre-commit-review:platform-id", "value": record.platform_id },
                { "name": "pre-commit-review:evidence-scope", "value": evidence_scope },
                { "name": "pre-commit-review:transitive-closure", "value": "unknown" }
            ]
        }],
        "dependencies": [{ "ref": pack_ref, "dependsOn": [record.sbom_component] }]
    }))
    .unwrap()
}

fn build_fixture(shape: ArchiveShape) -> FixturePack {
    let mut record = base_record();
    let executable = b"fixture-gitleaks-binary\n".to_vec();
    let license = b"fixture MIT license\n".to_vec();
    let executable_sha256 = sha256_bytes(&executable);
    let license_sha256 = sha256_bytes(&license);
    let upstream_archive_sha256 =
        "551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb";
    let mut sbom = sbom_bytes(&record, &executable_sha256, upstream_archive_sha256, shape);
    if matches!(shape, ArchiveShape::NoncanonicalSbomJson) {
        sbom.push(b'\n');
    }
    let sbom_sha256 = sha256_bytes(&sbom);

    let mut executable_binding_sha256 = executable_sha256.clone();
    let mut license_binding_sha256 = license_sha256.clone();
    let mut sbom_binding_sha256 = sbom_sha256.clone();
    if matches!(shape, ArchiveShape::ExecutableDigestMismatch) {
        executable_binding_sha256 = ZERO_SHA256.to_string();
    }
    if matches!(shape, ArchiveShape::LicenseDigestMismatch) {
        license_binding_sha256 = ZERO_SHA256.to_string();
    }
    if matches!(shape, ArchiveShape::SbomDigestMismatch) {
        sbom_binding_sha256 = ZERO_SHA256.to_string();
    }

    let manifest = PackManifest {
        schema_version: 1,
        kind: "third_party_artifact_pack".to_string(),
        artifact_id: if matches!(shape, ArchiveShape::ManifestIdentityMismatch) {
            "other-artifact".to_string()
        } else {
            record.artifact_id.clone()
        },
        tool_version: record.tool_version.clone(),
        pack_version: record.pack_version.clone(),
        platform_id: record.platform_id.clone(),
        target_triple: record.target_triple.clone(),
        upstream_asset_name: "gitleaks_8.30.1_linux_x64.tar.gz".to_string(),
        upstream_asset_sha256: upstream_archive_sha256.to_string(),
        source_lock_sha256: record.source_lock_sha256.clone(),
        project_asset_name: record.project_asset_name.clone(),
        files: vec![
            PackFileRecord {
                path: "bin/gitleaks".to_string(),
                size: executable.len() as u64,
                sha256: executable_binding_sha256,
                role: PackFileRole::Executable,
            },
            PackFileRecord {
                path: "licenses/GITLEAKS-LICENSE".to_string(),
                size: license.len() as u64,
                sha256: license_binding_sha256,
                role: PackFileRole::License,
            },
            PackFileRecord {
                path: "sbom.cdx.json".to_string(),
                size: sbom.len() as u64,
                sha256: sbom_binding_sha256,
                role: PackFileRole::Sbom,
            },
        ],
    };
    let manifest_bytes = canonical_json(&manifest).unwrap();

    record.executable.size = executable.len() as u64;
    record.executable.sha256 = executable_sha256.clone();
    record.license_files[0].size = license.len() as u64;
    record.license_files[0].sha256 = license_sha256;
    record.pack_manifest_sha256 = sha256_bytes(&manifest_bytes);
    record.sbom_sha256 = sbom_sha256;
    if matches!(shape, ArchiveShape::ManifestDigestMismatch) {
        record.pack_manifest_sha256 = ZERO_SHA256.to_string();
    }

    let mut members = vec![
        Member::file("bin/gitleaks", executable, 0o755),
        Member::file("licenses/GITLEAKS-LICENSE", license, 0o644),
        Member::file("pack-manifest.json", manifest_bytes, 0o644),
        Member::file("sbom.cdx.json", sbom, 0o644),
    ];
    match shape {
        ArchiveShape::ParentTraversal => {
            members.insert(0, Member::file("../escape", b"escape".to_vec(), 0o644));
        }
        ArchiveShape::AbsolutePath => {
            members.insert(0, Member::file("/absolute", b"escape".to_vec(), 0o644));
        }
        ArchiveShape::AlternateDataStream => {
            members.insert(
                1,
                Member::file("bin/gitleaks:evil", b"escape".to_vec(), 0o644),
            );
        }
        ArchiveShape::Symlink => {
            members.insert(0, Member::special("bin/link", b'2', "bin/gitleaks"));
        }
        ArchiveShape::Hardlink => {
            members.insert(0, Member::special("bin/link", b'1', "bin/gitleaks"));
        }
        ArchiveShape::CharacterDevice => {
            members.insert(0, Member::special("bin/device", b'3', ""));
        }
        ArchiveShape::Sparse => {
            members.insert(0, Member::special("bin/sparse", b'S', ""));
        }
        ArchiveShape::DuplicatePath => {
            members.insert(1, members[0].clone());
        }
        ArchiveShape::CaseFoldCollision => {
            members.insert(
                2,
                Member::file("licenses/gitleaks-license", b"collision".to_vec(), 0o644),
            );
        }
        ArchiveShape::UnexpectedFile => {
            members.push(Member::file(
                "unexpected.txt",
                b"unexpected".to_vec(),
                0o644,
            ));
        }
        ArchiveShape::OversizedMetadata => {
            members.insert(
                0,
                Member::file("PaxHeaders.0/long", vec![b'x'; 16_385], 0o644),
            );
            members[0].entry_type = b'x';
        }
        ArchiveShape::TooManyEntries => {
            for index in 0..125 {
                members.push(Member::file(
                    &format!("extra/{index:03}"),
                    vec![index as u8],
                    0o644,
                ));
            }
            members.sort_by(|left, right| left.path.cmp(&right.path));
        }
        ArchiveShape::UnsortedPaths => members.swap(0, 1),
        ArchiveShape::NonzeroHeaderMetadata => members[0].uid = 1,
        _ => {}
    }

    let end_blocks = if matches!(shape, ArchiveShape::MissingEndBlock) {
        1
    } else {
        2
    };
    let tar = build_ustar(&members, end_blocks);
    let mut encoder: GzEncoder<Vec<u8>> = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(Vec::new(), Compression::best());
    encoder.write_all(&tar).unwrap();
    let mut bytes = encoder.finish().unwrap();
    if matches!(shape, ArchiveShape::NonzeroGzipMtime) {
        bytes[4] = 1;
    }
    if matches!(shape, ArchiveShape::TrailingGzipData) {
        bytes.push(0);
    }

    record.expected_compressed_size = bytes.len() as u64;
    record.max_compressed_size = bytes.len() as u64;
    record.pack_sha256 = sha256_bytes(&bytes);
    if matches!(shape, ArchiveShape::OuterDigestMismatch) {
        record.pack_sha256 = ZERO_SHA256.to_string();
    }
    if matches!(shape, ArchiveShape::OuterSizeMismatch) {
        record.expected_compressed_size += 1;
        record.max_compressed_size = record.expected_compressed_size;
    }

    FixturePack {
        bytes,
        record,
        executable_sha256,
    }
}

fn write_octal(field: &mut [u8], value: u64) {
    let digits = field.len() - 1;
    let encoded = format!("{value:0digits$o}");
    assert_eq!(encoded.len(), digits);
    field[..digits].copy_from_slice(encoded.as_bytes());
    field[digits] = 0;
}

fn append_member(output: &mut Vec<u8>, member: &Member) {
    assert!(member.path.len() <= 100);
    assert!(member.link_name.len() <= 100);
    let mut header = [0_u8; 512];
    header[..member.path.len()].copy_from_slice(member.path.as_bytes());
    write_octal(&mut header[100..108], member.mode.into());
    write_octal(&mut header[108..116], member.uid);
    write_octal(&mut header[116..124], member.gid);
    write_octal(&mut header[124..136], member.data.len() as u64);
    write_octal(&mut header[136..148], member.mtime);
    header[148..156].fill(b' ');
    header[156] = member.entry_type;
    header[157..157 + member.link_name.len()].copy_from_slice(member.link_name.as_bytes());
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
    let checksum = format!("{checksum:06o}\0 ");
    header[148..156].copy_from_slice(checksum.as_bytes());

    output.extend_from_slice(&header);
    output.extend_from_slice(&member.data);
    let padding = (512 - member.data.len() % 512) % 512;
    output.resize(output.len() + padding, 0);
}

fn build_ustar(members: &[Member], end_blocks: usize) -> Vec<u8> {
    let mut output = Vec::new();
    for member in members {
        append_member(&mut output, member);
    }
    output.resize(output.len() + end_blocks * 512, 0);
    output
}

fn rejection(shape: ArchiveShape) -> &'static str {
    let fixture = build_fixture(shape);
    verify_pack(
        fixture.bytes.as_slice(),
        &fixture.record,
        &VerifyLimits::default(),
    )
    .unwrap_err()
    .code
}

fn repository_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate must have a repository parent")
}

#[test]
fn rust_writer_emits_a_complete_verifiable_gitleaks_record() {
    let temporary = tempfile::tempdir().unwrap();
    let source_root = temporary.path().join("payload");
    fs::create_dir_all(source_root.join("THIRD_PARTY_LICENSES")).unwrap();
    fs::create_dir_all(source_root.join("references/security")).unwrap();
    fs::write(
        source_root.join("THIRD_PARTY_LICENSES/gitleaks-LICENSE"),
        b"fixture MIT license\n",
    )
    .unwrap();
    fs::write(
        source_root.join("references/security/gitleaks.toml"),
        b"title = \"fixture\"\n",
    )
    .unwrap();
    let executable = temporary.path().join("gitleaks");
    fs::write(&executable, b"fixture-gitleaks-binary\n").unwrap();
    let output = temporary
        .path()
        .join("pre-commit-review-gitleaks-8.30.1-pcr.1-linux-amd64.tar.gz");
    let record_output = temporary.path().join("record.json");
    let manifest_output = temporary.path().join("manifest.json");
    let rebuilt = temporary.path().join("rebuilt.tar.gz");
    let reviewed_source_lock =
        repository_root().join("third_party_artifacts/sources/gitleaks-8.30.1.json");
    let source_lock = temporary.path().join("fixture-source-lock.json");
    let executable_bytes = b"fixture-gitleaks-binary\n";
    let mut source_lock_value: SourceLock =
        serde_json::from_slice(&fs::read(&reviewed_source_lock).unwrap()).unwrap();
    let asset = source_lock_value
        .assets
        .iter_mut()
        .find(|asset| asset.platform_id == "linux-amd64")
        .unwrap();
    asset.executable_size = executable_bytes.len() as u64;
    asset.executable_sha256 = sha256_bytes(executable_bytes);
    fs::write(&source_lock, canonical_json(&source_lock_value).unwrap()).unwrap();
    let distribution_manifest = repository_root().join("third_party_artifacts/manifest.json");

    let invoke = |destination: &Path,
                  sidecar: Option<&Path>,
                  updated_manifest: Option<&Path>,
                  source_lock_path: &Path| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_artifact-pack-writer"));
        command
            .arg("gitleaks")
            .arg("--platform-id")
            .arg("linux-amd64")
            .arg("--pack-version")
            .arg("8.30.1-pcr.1")
            .arg("--source-root")
            .arg(&source_root)
            .arg("--manifest")
            .arg(&distribution_manifest)
            .arg("--source-lock")
            .arg(source_lock_path)
            .arg("--binary")
            .arg(&executable)
            .arg("--output")
            .arg(destination);
        if let Some(sidecar) = sidecar {
            command.arg("--record-output").arg(sidecar);
        }
        if let Some(updated_manifest) = updated_manifest {
            command.arg("--manifest-output").arg(updated_manifest);
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "writer failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    invoke(
        &output,
        Some(&record_output),
        Some(&manifest_output),
        &source_lock,
    );
    invoke(&rebuilt, None, None, &source_lock);

    let mismatched_output = temporary.path().join("mismatched.tar.gz");
    let mut mismatched = Command::new(env!("CARGO_BIN_EXE_artifact-pack-writer"));
    mismatched
        .arg("gitleaks")
        .arg("--platform-id")
        .arg("linux-amd64")
        .arg("--pack-version")
        .arg("8.30.1-pcr.1")
        .arg("--source-root")
        .arg(&source_root)
        .arg("--manifest")
        .arg(&distribution_manifest)
        .arg("--source-lock")
        .arg(&reviewed_source_lock)
        .arg("--binary")
        .arg(&executable)
        .arg("--output")
        .arg(&mismatched_output);
    assert!(!mismatched.output().unwrap().status.success());

    let bytes = fs::read(&output).unwrap();
    assert_eq!(bytes, fs::read(&rebuilt).unwrap());
    let record_bytes = fs::read(&record_output).unwrap();
    let record: ArtifactPackRecord = serde_json::from_slice(&record_bytes).unwrap();
    assert_eq!(canonical_json(&record).unwrap(), record_bytes);
    assert_eq!(record.artifact_id, "gitleaks");
    assert_eq!(record.artifact_role, ArtifactRole::Sanitizer);
    assert_eq!(record.upstream_repository, "gitleaks/gitleaks");
    assert_eq!(record.upstream_tag, "v8.30.1");
    assert_eq!(record.platform_id, "linux-amd64");
    assert_eq!(record.pack_version, "8.30.1-pcr.1");
    assert_eq!(
        record.project_asset_name,
        "pre-commit-review-gitleaks-8.30.1-pcr.1-linux-amd64.tar.gz"
    );
    assert_eq!(record.expected_compressed_size, bytes.len() as u64);
    assert_eq!(record.pack_sha256, sha256_bytes(&bytes));
    assert_eq!(record.executable.path, "bin/gitleaks");
    assert_eq!(record.license_files.len(), 1);
    assert_eq!(record.license_files[0].path, "licenses/GITLEAKS-LICENSE");
    assert_eq!(record.pack_format, PackFormat::NormalizedTarGzipV1);
    assert_eq!(record.state, ArtifactState::Active);
    let updated_manifest_bytes = fs::read(&manifest_output).unwrap();
    let updated_manifest: ArtifactManifest =
        serde_json::from_slice(&updated_manifest_bytes).unwrap();
    assert_eq!(
        canonical_json(&updated_manifest).unwrap(),
        updated_manifest_bytes
    );
    let reviewed_manifest: ArtifactManifest =
        serde_json::from_slice(&fs::read(&distribution_manifest).unwrap()).unwrap();
    let generated_gitleaks_records = updated_manifest
        .packs
        .iter()
        .filter(|candidate| candidate.artifact_id == "gitleaks")
        .collect::<Vec<_>>();
    assert_eq!(generated_gitleaks_records, vec![&record]);
    let retained_provider_records = updated_manifest
        .packs
        .iter()
        .filter(|candidate| candidate.artifact_id == "rust-analyzer")
        .collect::<Vec<_>>();
    let reviewed_provider_records = reviewed_manifest
        .packs
        .iter()
        .filter(|candidate| candidate.artifact_id == "rust-analyzer")
        .collect::<Vec<_>>();
    assert_eq!(retained_provider_records, reviewed_provider_records);
    assert_eq!(
        updated_manifest.packs.len(),
        reviewed_manifest.packs.len() + 1
    );
    updated_manifest.validate().unwrap();

    let verified = verify_pack(bytes.as_slice(), &record, &VerifyLimits::default()).unwrap();
    assert_eq!(verified.files.len(), 3);
    let sbom_bytes = fs::read(verified.root().join("sbom.cdx.json")).unwrap();
    assert_eq!(record.sbom_sha256, sha256_bytes(&sbom_bytes));
    let sbom: serde_json::Value = serde_json::from_slice(&sbom_bytes).unwrap();
    assert_eq!(
        sbom.pointer("/components/0/supplier/name")
            .and_then(serde_json::Value::as_str),
        Some("Gitleaks")
    );
    assert_eq!(
        sbom.pointer("/components/0/properties/3/value")
            .and_then(serde_json::Value::as_str),
        Some("component-evidence")
    );
    assert_eq!(
        sbom.pointer("/components/0/properties/4/value")
            .and_then(serde_json::Value::as_str),
        Some("unknown")
    );
}

#[test]
fn verifier_extracts_only_a_verified_normalized_pack() {
    let fixture = build_fixture(ArchiveShape::Valid);
    let verified = verify_pack(
        fixture.bytes.as_slice(),
        &fixture.record,
        &VerifyLimits::default(),
    )
    .unwrap();

    assert_eq!(verified.pack_sha256, fixture.record.pack_sha256);
    assert_eq!(
        verified.files["bin/gitleaks"].sha256,
        fixture.executable_sha256
    );
    assert_eq!(
        std::fs::read(verified.root().join("bin/gitleaks")).unwrap(),
        b"fixture-gitleaks-binary\n"
    );
    assert_eq!(verified.files.len(), 3);
}

#[test]
fn verifier_rejects_outer_size_and_digest_before_archive_parsing() {
    assert_eq!(
        rejection(ArchiveShape::OuterSizeMismatch),
        "pack-size-mismatch"
    );
    assert_eq!(
        rejection(ArchiveShape::OuterDigestMismatch),
        "pack-digest-mismatch"
    );
}

#[test]
fn verifier_rejects_unsafe_paths_before_publication() {
    assert_eq!(rejection(ArchiveShape::ParentTraversal), "archive-path");
    assert_eq!(rejection(ArchiveShape::AbsolutePath), "archive-path");
    assert_eq!(rejection(ArchiveShape::AlternateDataStream), "archive-path");
}

#[test]
fn verifier_rejects_links_devices_and_sparse_members() {
    assert_eq!(rejection(ArchiveShape::Symlink), "archive-entry-type");
    assert_eq!(rejection(ArchiveShape::Hardlink), "archive-entry-type");
    assert_eq!(
        rejection(ArchiveShape::CharacterDevice),
        "archive-entry-type"
    );
    assert_eq!(rejection(ArchiveShape::Sparse), "archive-entry-type");
}

#[test]
fn verifier_rejects_duplicate_colliding_and_unexpected_members() {
    assert_eq!(
        rejection(ArchiveShape::DuplicatePath),
        "archive-duplicate-path"
    );
    assert_eq!(
        rejection(ArchiveShape::CaseFoldCollision),
        "archive-case-collision"
    );
    assert_eq!(
        rejection(ArchiveShape::UnexpectedFile),
        "archive-unexpected-file"
    );
}

#[test]
fn verifier_enforces_entry_compressed_expanded_and_metadata_budgets() {
    assert_eq!(
        rejection(ArchiveShape::TooManyEntries),
        "archive-entry-limit"
    );
    assert_eq!(
        rejection(ArchiveShape::OversizedMetadata),
        "archive-metadata-limit"
    );

    let fixture = build_fixture(ArchiveShape::Valid);
    let compressed_limits = VerifyLimits {
        max_compressed_bytes: fixture.bytes.len() as u64 - 1,
        ..VerifyLimits::default()
    };
    assert_eq!(
        verify_pack(
            fixture.bytes.as_slice(),
            &fixture.record,
            &compressed_limits
        )
        .unwrap_err()
        .code,
        "pack-compressed-limit"
    );

    let expanded_limits = VerifyLimits {
        max_expanded_bytes: 1_024,
        ..VerifyLimits::default()
    };
    assert_eq!(
        verify_pack(fixture.bytes.as_slice(), &fixture.record, &expanded_limits)
            .unwrap_err()
            .code,
        "pack-expanded-limit"
    );
}

#[test]
fn verifier_requires_canonical_gzip_and_ustar_metadata() {
    assert_eq!(rejection(ArchiveShape::NonzeroGzipMtime), "gzip-metadata");
    assert_eq!(
        rejection(ArchiveShape::TrailingGzipData),
        "gzip-trailing-data"
    );
    assert_eq!(
        rejection(ArchiveShape::NonzeroHeaderMetadata),
        "archive-header-metadata"
    );
    assert_eq!(rejection(ArchiveShape::UnsortedPaths), "archive-path-order");
    assert_eq!(
        rejection(ArchiveShape::MissingEndBlock),
        "archive-end-blocks"
    );
}

#[test]
fn verifier_binds_internal_manifest_and_every_payload_digest() {
    assert_eq!(
        rejection(ArchiveShape::ManifestIdentityMismatch),
        "pack-identity-mismatch"
    );
    assert_eq!(
        rejection(ArchiveShape::ManifestDigestMismatch),
        "pack-manifest-digest"
    );
    assert_eq!(
        rejection(ArchiveShape::ExecutableDigestMismatch),
        "pack-file-digest"
    );
    assert_eq!(
        rejection(ArchiveShape::LicenseDigestMismatch),
        "pack-file-digest"
    );
    assert_eq!(
        rejection(ArchiveShape::SbomDigestMismatch),
        "pack-file-digest"
    );
}

#[test]
fn verifier_requires_component_level_external_binary_sbom_evidence() {
    assert_eq!(
        rejection(ArchiveShape::InvalidSbomComponent),
        "sbom-component"
    );
    assert_eq!(rejection(ArchiveShape::InvalidSbomSource), "sbom-source");
    assert_eq!(rejection(ArchiveShape::MissingSbomLicense), "sbom-license");
    assert_eq!(
        rejection(ArchiveShape::InvalidSbomEvidence),
        "sbom-evidence"
    );
    assert_eq!(
        rejection(ArchiveShape::NoncanonicalSbomJson),
        "sbom-canonical"
    );
}
