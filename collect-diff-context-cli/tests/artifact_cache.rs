#[allow(dead_code)]
mod support;

use collect_diff_context_cli::artifacts::{
    cache::{
        open_cache, provision_from_cache, publish_cache, verify_target_receipt,
        ArtifactCacheBoundaries, ArtifactCacheLayout, CachePublishStatus,
    },
    contract::{
        canonical_json, sha256_bytes, ArtifactFileBinding, ArtifactManifest, ArtifactPackRecord,
        ArtifactRole, ArtifactState, PackFileRecord, PackFileRole, PackFormat, PackManifest,
        ProbeId, ProbeResult,
    },
    pack::{verify_pack, VerifiedPack, VerifyLimits},
    transport::{
        HttpBackend, HttpBackendError, HttpRequest, HttpResponse, Transport, TransportLimits,
    },
};
use flate2::{write::GzEncoder, Compression, GzBuilder};
use serde_json::json;
use std::{
    collections::VecDeque,
    fs,
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Barrier, Mutex},
};
use support::GitRepo;
use tempfile::TempDir;

const ZERO_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";

struct FixturePack {
    bytes: Vec<u8>,
    record: ArtifactPackRecord,
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

struct ScriptedBackend {
    responses: Mutex<VecDeque<Result<HttpResponse, HttpBackendError>>>,
}

impl ScriptedBackend {
    fn new(responses: Vec<Result<HttpResponse, HttpBackendError>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

impl HttpBackend for ScriptedBackend {
    fn get(&self, _request: HttpRequest) -> Result<HttpResponse, HttpBackendError> {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted transport response exhausted")
    }
}

struct FailingBody;

impl Read for FailingBody {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("SECRET RESPONSE BODY"))
    }
}

fn response(
    status: u16,
    location: Option<&str>,
    content_length: Option<u64>,
    body: impl Read + Send + 'static,
) -> HttpResponse {
    HttpResponse {
        status,
        location: location.map(str::to_string),
        content_length,
        content_encoding: None,
        body: Box::new(body),
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

fn fixture_pack() -> FixturePack {
    fixture_pack_with_version("8.30.1-pcr.1")
}

fn fixture_pack_with_version(pack_version: &str) -> FixturePack {
    let mut record = base_record();
    record.pack_version = pack_version.to_string();
    let executable = b"fixture-gitleaks-binary\n".to_vec();
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

fn probes() -> Vec<ProbeResult> {
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

fn manifest(record: &ArtifactPackRecord) -> ArtifactManifest {
    ArtifactManifest {
        schema_version: 1,
        kind: "third_party_artifacts".to_string(),
        release_repository: "junit/pre-commit-review".to_string(),
        revocation_index_sha256: ZERO_SHA256.to_string(),
        packs: vec![record.clone()],
    }
}

fn verified(fixture: &FixturePack) -> VerifiedPack {
    verify_pack(
        fixture.bytes.as_slice(),
        &fixture.record,
        &VerifyLimits::default(),
    )
    .unwrap()
}

fn layout(root: &Path, boundaries: ArtifactCacheBoundaries) -> ArtifactCacheLayout {
    ArtifactCacheLayout::resolve(Some(root), &boundaries).unwrap()
}

#[test]
fn local_transport_accepts_only_the_exact_pinned_bytes() {
    let fixture = fixture_pack();
    let directory = TempDir::new().unwrap();
    let pack_path = directory.path().join("fixture.tar.gz");
    fs::write(&pack_path, &fixture.bytes).unwrap();

    let fetched = Transport::local(&pack_path, &fixture.record.pack_sha256)
        .unwrap()
        .fetch(&fixture.record)
        .unwrap();
    assert_eq!(fetched.size(), fixture.record.expected_compressed_size);
    assert_eq!(fetched.sha256(), fixture.record.pack_sha256);
    let mut observed = Vec::new();
    fetched.open().unwrap().read_to_end(&mut observed).unwrap();
    assert_eq!(observed, fixture.bytes);

    fs::write(&pack_path, &fixture.bytes[..fixture.bytes.len() - 1]).unwrap();
    let wrong_size = Transport::local(&pack_path, &fixture.record.pack_sha256)
        .unwrap()
        .fetch(&fixture.record)
        .unwrap_err();
    assert_eq!(wrong_size.code, "transport-size-mismatch");

    let mut wrong_digest_bytes = fixture.bytes.clone();
    wrong_digest_bytes[20] ^= 1;
    fs::write(&pack_path, wrong_digest_bytes).unwrap();
    let wrong_digest = Transport::local(&pack_path, &fixture.record.pack_sha256)
        .unwrap()
        .fetch(&fixture.record)
        .unwrap_err();
    assert_eq!(wrong_digest.code, "transport-digest-mismatch");
}

#[test]
fn project_transport_rejects_protocol_downgrade() {
    let fixture = fixture_pack();
    let backend = ScriptedBackend::new(vec![Ok(response(
        302,
        Some("http://release-assets.githubusercontent.com/fixture"),
        Some(0),
        Cursor::new(Vec::new()),
    ))]);
    let error = Transport::project_asset(&fixture.record)
        .unwrap()
        .fetch_with_backend(&fixture.record, &TransportLimits::default(), &backend)
        .unwrap_err();
    assert_eq!(error.code, "transport-protocol-downgrade");
}

#[test]
fn project_transport_bounds_the_redirect_chain() {
    let fixture = fixture_pack();
    let redirects = (0..4)
        .map(|index| {
            Ok(response(
                302,
                Some(&format!(
                    "https://release-assets.githubusercontent.com/fixture?redirect={index}"
                )),
                Some(0),
                Cursor::new(Vec::new()),
            ))
        })
        .collect();
    let backend = ScriptedBackend::new(redirects);
    let error = Transport::project_asset(&fixture.record)
        .unwrap()
        .fetch_with_backend(&fixture.record, &TransportLimits::default(), &backend)
        .unwrap_err();
    assert_eq!(error.code, "transport-redirect-limit");
}

#[test]
fn project_transport_maps_timeouts_to_a_stable_code() {
    let fixture = fixture_pack();
    let backend = ScriptedBackend::new(vec![Err(HttpBackendError::Timeout)]);
    let error = Transport::project_asset(&fixture.record)
        .unwrap()
        .fetch_with_backend(&fixture.record, &TransportLimits::default(), &backend)
        .unwrap_err();
    assert_eq!(error.code, "transport-timeout");
}

#[test]
fn project_transport_enforces_its_body_budget() {
    let fixture = fixture_pack();
    let oversized = usize::try_from(fixture.record.max_compressed_size).unwrap() + 1;
    let backend = ScriptedBackend::new(vec![Ok(response(
        200,
        None,
        None,
        Cursor::new(vec![0_u8; oversized]),
    ))]);
    let error = Transport::project_asset(&fixture.record)
        .unwrap()
        .fetch_with_backend(&fixture.record, &TransportLimits::default(), &backend)
        .unwrap_err();
    assert_eq!(error.code, "transport-byte-limit");
}

#[test]
fn project_transport_never_includes_response_data_in_errors() {
    let fixture = fixture_pack();
    let backend = ScriptedBackend::new(vec![Ok(response(200, None, None, FailingBody))]);
    let error = Transport::project_asset(&fixture.record)
        .unwrap()
        .fetch_with_backend(&fixture.record, &TransportLimits::default(), &backend)
        .unwrap_err();
    assert_eq!(error.code, "transport-read");
    assert!(!error.to_string().contains("SECRET"));
    assert!(!format!("{error:?}").contains("SECRET"));
}

#[test]
fn two_cache_writers_publish_one_atomic_entry() {
    let fixture = fixture_pack();
    let cache_root = TempDir::new().unwrap();
    let cache_layout = layout(cache_root.path(), ArtifactCacheBoundaries::default());
    let barrier = Arc::new(Barrier::new(2));
    let mut writers = Vec::new();

    for _ in 0..2 {
        let fixture = fixture_pack();
        let verified = verified(&fixture);
        let cache_layout = cache_layout.clone();
        let barrier = Arc::clone(&barrier);
        writers.push(std::thread::spawn(move || {
            barrier.wait();
            publish_cache(&cache_layout, &verified, &fixture.record, &probes())
                .map(|publication| publication.status())
        }));
    }

    let mut statuses = writers
        .into_iter()
        .map(|writer| writer.join().unwrap().unwrap())
        .collect::<Vec<_>>();
    statuses.sort();
    assert_eq!(
        statuses,
        vec![CachePublishStatus::Published, CachePublishStatus::Reused]
    );
    assert_eq!(fs::read_dir(cache_layout.sha256_root()).unwrap().count(), 1);
    open_cache(&cache_layout, &fixture.record).unwrap();
}

#[test]
fn corrupt_existing_cache_entry_is_rejected_without_repair() {
    let fixture = fixture_pack();
    let cache_root = TempDir::new().unwrap();
    let cache_layout = layout(cache_root.path(), ArtifactCacheBoundaries::default());
    let publication = publish_cache(
        &cache_layout,
        &verified(&fixture),
        &fixture.record,
        &probes(),
    )
    .unwrap();
    let executable = publication.entry().root().join("bin/gitleaks");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(&executable, b"corrupt").unwrap();

    let error = publish_cache(
        &cache_layout,
        &verified(&fixture),
        &fixture.record,
        &probes(),
    )
    .unwrap_err();
    assert_eq!(error.code, "corrupt-cache");
    assert_eq!(fs::read(executable).unwrap(), b"corrupt");
}

#[test]
fn incomplete_existing_cache_entry_is_rejected_without_repair() {
    let fixture = fixture_pack();
    let cache_root = TempDir::new().unwrap();
    let cache_layout = layout(cache_root.path(), ArtifactCacheBoundaries::default());
    let publication = publish_cache(
        &cache_layout,
        &verified(&fixture),
        &fixture.record,
        &probes(),
    )
    .unwrap();
    let sbom = publication.entry().root().join("sbom.cdx.json");
    fs::remove_file(&sbom).unwrap();

    let error = publish_cache(
        &cache_layout,
        &verified(&fixture),
        &fixture.record,
        &probes(),
    )
    .unwrap_err();
    assert_eq!(error.code, "corrupt-cache");
    assert!(!sbom.exists());
}

#[test]
fn target_copy_has_no_cache_path_dependency() {
    let fixture = fixture_pack();
    let cache_root = TempDir::new().unwrap();
    let target_root = TempDir::new().unwrap();
    let cache_layout = layout(
        cache_root.path(),
        ArtifactCacheBoundaries {
            target_root: Some(target_root.path().to_path_buf()),
            ..ArtifactCacheBoundaries::default()
        },
    );
    let publication = publish_cache(
        &cache_layout,
        &verified(&fixture),
        &fixture.record,
        &probes(),
    )
    .unwrap();
    let target = provision_from_cache(
        publication.entry(),
        target_root.path(),
        &manifest(&fixture.record),
    )
    .unwrap();
    let cache_executable = publication.entry().root().join("bin/gitleaks");

    assert!(!fs::symlink_metadata(target.executable_path())
        .unwrap()
        .file_type()
        .is_symlink());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_ne!(
            fs::metadata(&cache_executable).unwrap().ino(),
            fs::metadata(target.executable_path()).unwrap().ino(),
            "target provisioning must copy rather than hard-link cache files"
        );
    }
    let receipt_bytes = fs::read(target.receipt_path()).unwrap();
    assert!(!String::from_utf8_lossy(&receipt_bytes)
        .contains(cache_layout.namespace_root().to_string_lossy().as_ref()));

    fs::remove_dir_all(cache_layout.namespace_root()).unwrap();
    let receipt = verify_target_receipt(
        target_root.path(),
        &fixture.record.artifact_id,
        &manifest(&fixture.record),
    )
    .unwrap();
    assert_eq!(receipt.pack_sha256, fixture.record.pack_sha256);
    assert_eq!(
        fs::read(target.executable_path()).unwrap(),
        b"fixture-gitleaks-binary\n"
    );
}

#[test]
fn target_copy_rejects_unsafe_pack_version_before_writing() {
    let fixture = fixture_pack_with_version("../../../../escaped-artifact");
    let cache_root = TempDir::new().unwrap();
    let target_parent = TempDir::new().unwrap();
    let target_root = target_parent.path().join("managed-target");
    fs::create_dir(&target_root).unwrap();
    let cache_layout = layout(
        cache_root.path(),
        ArtifactCacheBoundaries {
            target_root: Some(target_root.clone()),
            ..ArtifactCacheBoundaries::default()
        },
    );
    let publication = publish_cache(
        &cache_layout,
        &verified(&fixture),
        &fixture.record,
        &probes(),
    )
    .unwrap();

    let error = provision_from_cache(
        publication.entry(),
        &target_root,
        &manifest(&fixture.record),
    )
    .unwrap_err();
    assert_eq!(error.code, "target-pack-version-invalid");
    assert!(!target_parent.path().join("escaped-artifact").exists());
    assert!(!target_root.join("runtime").exists());
}

#[test]
fn cache_override_rejects_every_protected_location() {
    let repo = GitRepo::new().unwrap();
    repo.commit_file("src/lib.rs", b"pub fn fixture() {}\n")
        .unwrap();
    let snapshot = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    let boundaries = ArtifactCacheBoundaries {
        candidate_repository: Some(repo.path().to_path_buf()),
        snapshot_root: Some(snapshot.path().to_path_buf()),
        target_root: Some(target.path().to_path_buf()),
    };
    let cases: Vec<(PathBuf, &str)> = vec![
        (repo.path().join("cache"), "cache-root-inside-repository"),
        (
            repo.path().join(".git/cache"),
            "cache-root-inside-git-directory",
        ),
        (
            snapshot.path().join("cache"),
            "artifact-cache-inside-snapshot",
        ),
        (target.path().join("cache"), "artifact-cache-inside-target"),
    ];

    for (path, code) in cases {
        let error = ArtifactCacheLayout::resolve(Some(&path), &boundaries).unwrap_err();
        assert_eq!(error.code, code, "unexpected result for {}", path.display());
        assert!(!path.exists());
    }
    let relative =
        ArtifactCacheLayout::resolve(Some(Path::new("relative")), &boundaries).unwrap_err();
    assert_eq!(relative.code, "cache-root-not-absolute");
}
