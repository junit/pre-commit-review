#![allow(dead_code)]

use collect_diff_context_cli::artifacts::{
    contract::{
        canonical_json, sha256_bytes, ArtifactFileBinding, ArtifactManifest, ArtifactPackRecord,
        ArtifactRole, ArtifactState, PackFileRecord, PackFileRole, PackFormat, PackManifest,
        ProbeId, ProbeResult,
    },
    pack::{verify_pack, VerifiedPack, VerifyLimits},
};
use flate2::{write::GzEncoder, Compression, GzBuilder};
use serde_json::json;
use std::io::Write;

pub const ZERO_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";

pub struct FixturePack {
    pub bytes: Vec<u8>,
    pub record: ArtifactPackRecord,
}

#[derive(Clone)]
struct Member {
    path: String,
    data: Vec<u8>,
    mode: u32,
}

impl Member {
    fn file(path: &str, data: Vec<u8>, mode: u32) -> Self {
        Self {
            path: path.to_string(),
            data,
            mode,
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
) -> Vec<u8> {
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
                "name": "pre-commit-review-gitleaks-pack",
                "version": record.pack_version
            }
        },
        "components": [{
            "type": "application",
            "bom-ref": record.sbom_component,
            "name": record.license_component,
            "version": record.tool_version,
            "purl": record.sbom_component,
            "hashes": [{ "alg": "SHA-256", "content": executable_sha256 }],
            "licenses": [{ "license": { "id": "MIT" } }],
            "externalReferences": [{
                "type": "distribution",
                "url": "https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_linux_x64.tar.gz",
                "hashes": [{ "alg": "SHA-256", "content": upstream_archive_sha256 }]
            }],
            "properties": [
                { "name": "pre-commit-review:artifact-id", "value": record.artifact_id },
                { "name": "pre-commit-review:pack-version", "value": record.pack_version },
                { "name": "pre-commit-review:platform-id", "value": record.platform_id },
                { "name": "pre-commit-review:evidence-scope", "value": "component-evidence" },
                { "name": "pre-commit-review:transitive-closure", "value": "unknown" }
            ]
        }],
        "dependencies": [{ "ref": pack_ref, "dependsOn": [record.sbom_component] }]
    }))
    .unwrap()
}

pub fn fixture_pack() -> FixturePack {
    fixture_pack_with("8.30.1-pcr.1", b"fixture-gitleaks-binary\n")
}

pub fn fixture_pack_with_version(pack_version: &str) -> FixturePack {
    fixture_pack_with(pack_version, b"fixture-gitleaks-binary\n")
}

pub fn executable_fixture_pack() -> FixturePack {
    executable_fixture_pack_with(
        b"#!/bin/sh\nif [ \"$1\" = \"version\" ]; then\n  printf '8.30.1\\n'\n  exit 0\nfi\nprintf '[]'\n",
    )
}

pub fn executable_fixture_pack_for_artifact(artifact_id: &str) -> FixturePack {
    let mut record = base_record();
    record.artifact_id = artifact_id.to_string();
    record.project_release_tag = format!("artifact-{artifact_id}-8.30.1-pcr.1");
    record.project_asset_name = format!("{artifact_id}-8.30.1-pcr.1-linux-amd64.tar.gz");
    fixture_pack_from_record(
        record,
        b"#!/bin/sh\nif [ \"$1\" = \"version\" ]; then\n  printf '8.30.1\\n'\n  exit 0\nfi\nprintf '[]'\n",
    )
}

pub fn executable_fixture_pack_with(executable: &[u8]) -> FixturePack {
    fixture_pack_with("8.30.1-pcr.1", executable)
}

fn fixture_pack_with(pack_version: &str, executable: &[u8]) -> FixturePack {
    let mut record = base_record();
    record.pack_version = pack_version.to_string();
    if pack_version.starts_with("8.30.1-pcr.") {
        record.project_release_tag = format!("artifact-gitleaks-{pack_version}");
        record.project_asset_name = format!("gitleaks-{pack_version}-linux-amd64.tar.gz");
    }
    fixture_pack_from_record(record, executable)
}

fn fixture_pack_from_record(mut record: ArtifactPackRecord, executable: &[u8]) -> FixturePack {
    let executable = executable.to_vec();
    let license = b"fixture MIT license\n".to_vec();
    let executable_sha256 = sha256_bytes(&executable);
    let license_sha256 = sha256_bytes(&license);
    let upstream_archive_sha256 =
        "551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb";
    let sbom = sbom_bytes(&record, &executable_sha256, upstream_archive_sha256);
    let sbom_sha256 = sha256_bytes(&sbom);

    let manifest = PackManifest {
        schema_version: 1,
        kind: "third_party_artifact_pack".to_string(),
        artifact_id: record.artifact_id.clone(),
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
                sha256: executable_sha256.clone(),
                role: PackFileRole::Executable,
            },
            PackFileRecord {
                path: "licenses/GITLEAKS-LICENSE".to_string(),
                size: license.len() as u64,
                sha256: license_sha256.clone(),
                role: PackFileRole::License,
            },
            PackFileRecord {
                path: "sbom.cdx.json".to_string(),
                size: sbom.len() as u64,
                sha256: sbom_sha256.clone(),
                role: PackFileRole::Sbom,
            },
        ],
    };
    let manifest_bytes = canonical_json(&manifest).unwrap();

    record.executable.size = executable.len() as u64;
    record.executable.sha256 = executable_sha256;
    record.license_files[0].size = license.len() as u64;
    record.license_files[0].sha256 = license_sha256;
    record.pack_manifest_sha256 = sha256_bytes(&manifest_bytes);
    record.sbom_sha256 = sbom_sha256;

    let members = vec![
        Member::file("bin/gitleaks", executable, 0o755),
        Member::file("licenses/GITLEAKS-LICENSE", license, 0o644),
        Member::file("pack-manifest.json", manifest_bytes, 0o644),
        Member::file("sbom.cdx.json", sbom, 0o644),
    ];
    let tar = build_ustar(&members);
    let mut encoder: GzEncoder<Vec<u8>> = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(Vec::new(), Compression::best());
    encoder.write_all(&tar).unwrap();
    let bytes = encoder.finish().unwrap();
    record.expected_compressed_size = bytes.len() as u64;
    record.max_compressed_size = bytes.len() as u64;
    record.pack_sha256 = sha256_bytes(&bytes);

    FixturePack { bytes, record }
}

fn write_octal(field: &mut [u8], value: u64) {
    let digits = field.len() - 1;
    let encoded = format!("{value:0digits$o}");
    field[..digits].copy_from_slice(encoded.as_bytes());
    field[digits] = 0;
}

fn append_member(output: &mut Vec<u8>, member: &Member) {
    let mut header = [0_u8; 512];
    header[..member.path.len()].copy_from_slice(member.path.as_bytes());
    write_octal(&mut header[100..108], member.mode.into());
    write_octal(&mut header[108..116], 0);
    write_octal(&mut header[116..124], 0);
    write_octal(&mut header[124..136], member.data.len() as u64);
    write_octal(&mut header[136..148], 0);
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
    header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
    output.extend_from_slice(&header);
    output.extend_from_slice(&member.data);
    let padding = (512 - member.data.len() % 512) % 512;
    output.resize(output.len() + padding, 0);
}

fn build_ustar(members: &[Member]) -> Vec<u8> {
    let mut output = Vec::new();
    for member in members {
        append_member(&mut output, member);
    }
    output.resize(output.len() + 1_024, 0);
    output
}

pub fn probes() -> Vec<ProbeResult> {
    vec![
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
    ]
}

pub fn manifest(record: &ArtifactPackRecord) -> ArtifactManifest {
    ArtifactManifest {
        schema_version: 1,
        kind: "third_party_artifacts".to_string(),
        release_repository: "junit/pre-commit-review".to_string(),
        revocation_index_sha256: ZERO_SHA256.to_string(),
        packs: vec![record.clone()],
    }
}

pub fn verified(fixture: &FixturePack) -> VerifiedPack {
    verify_pack(
        fixture.bytes.as_slice(),
        &fixture.record,
        &VerifyLimits::default(),
    )
    .unwrap()
}
