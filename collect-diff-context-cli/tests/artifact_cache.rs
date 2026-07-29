#[path = "support/artifact_fixture.rs"]
mod artifact_fixture;
#[allow(dead_code)]
mod support;

use artifact_fixture::{fixture_pack, fixture_pack_with_version, manifest, probes, verified};
use collect_diff_context_cli::artifacts::{
    cache::{
        open_cache, provision_from_cache, publish_cache, verify_target_receipt,
        ArtifactCacheBoundaries, ArtifactCacheLayout, CachePublishStatus,
    },
    transport::{
        HttpBackend, HttpBackendError, HttpRequest, HttpResponse, Transport, TransportLimits,
    },
};
use std::{
    collections::VecDeque,
    fs,
    io::{self, Cursor, Read},
    path::{Path, PathBuf},
    sync::{Arc, Barrier, Mutex},
};
use support::GitRepo;
use tempfile::TempDir;

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
