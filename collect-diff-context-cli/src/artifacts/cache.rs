use super::{
    contract::{
        canonical_json, sha256_bytes, ArtifactError, ArtifactFileBinding, ArtifactManifest,
        ArtifactPackRecord, ArtifactReceipt, PackFileRecord, PackFileRole, PackManifest,
        ProbeResult, MAX_MANIFEST_BYTES,
    },
    pack::VerifiedPack,
};
#[cfg(windows)]
use crate::impact_context::cache::file_facts::set_private_file_permissions;
use crate::impact_context::cache::file_facts::{
    create_private_directory, is_symlink_or_reparse, open_regular_file_no_follow,
    platform_default_cache_root, resolve_absolute_path, sync_directory, CacheLayout,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const CACHE_NAMESPACE: &str = "third-party-artifacts";
const CACHE_RECEIPT_FILE: &str = "cache-receipt.json";
const PACK_MANIFEST_FILE: &str = "pack-manifest.json";
const CACHE_FORMAT_VERSION: u8 = 1;
const VERIFIER_VERSION: &str = "pre-commit-review-artifact-verifier/v1";
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const MAX_CACHE_FILES: usize = 130;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactCacheBoundaries {
    pub candidate_repository: Option<PathBuf>,
    pub snapshot_root: Option<PathBuf>,
    pub target_root: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactCacheLayout {
    root: PathBuf,
    namespace_root: PathBuf,
    sha256_root: PathBuf,
}

impl ArtifactCacheLayout {
    pub fn resolve(
        override_root: Option<&Path>,
        boundaries: &ArtifactCacheBoundaries,
    ) -> Result<Self, ArtifactError> {
        let selected = if let Some(root) = override_root {
            root.to_path_buf()
        } else if let Some(root) = std::env::var_os("PRE_COMMIT_REVIEW_ARTIFACT_CACHE_DIR") {
            PathBuf::from(root)
        } else {
            platform_default_cache_root().map_err(map_cache_root_error)?
        };
        if !selected.is_absolute() {
            return Err(error(
                "cache-root-not-absolute",
                "artifact cache root must be absolute",
            ));
        }
        let mut root = resolve_absolute_path(&selected).map_err(map_cache_root_error)?;
        if let Some(repository) = boundaries.candidate_repository.as_deref() {
            root = CacheLayout::resolve(repository, Some(&root))
                .map_err(map_cache_root_error)?
                .root;
        } else if root.exists()
            && !fs::metadata(&root)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false)
        {
            return Err(error(
                "cache-root-not-directory",
                "artifact cache root exists but is not a directory",
            ));
        }

        reject_protected_root(
            &root,
            boundaries.snapshot_root.as_deref(),
            "artifact-cache-inside-snapshot",
        )?;
        reject_protected_root(
            &root,
            boundaries.target_root.as_deref(),
            "artifact-cache-inside-target",
        )?;

        let namespace_root = root.join(CACHE_NAMESPACE);
        let sha256_root = namespace_root.join("sha256");
        Ok(Self {
            root,
            namespace_root,
            sha256_root,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn namespace_root(&self) -> &Path {
        &self.namespace_root
    }

    pub fn sha256_root(&self) -> &Path {
        &self.sha256_root
    }

    fn entry_path(&self, digest: &str) -> PathBuf {
        self.sha256_root.join(digest)
    }

    fn ensure(&self) -> Result<(), ArtifactError> {
        ensure_private_path(&self.root)?;
        create_private_directory(&self.namespace_root).map_err(map_cache_io_error)?;
        create_private_directory(&self.sha256_root).map_err(map_cache_io_error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CachePublishStatus {
    Published,
    Reused,
}

#[derive(Debug, Clone)]
pub struct CachedArtifact {
    root: PathBuf,
    cache_namespace_root: PathBuf,
    receipt: CacheReceipt,
}

impl CachedArtifact {
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[derive(Debug, Clone)]
pub struct CachePublication {
    entry: CachedArtifact,
    status: CachePublishStatus,
}

impl CachePublication {
    pub fn entry(&self) -> &CachedArtifact {
        &self.entry
    }

    pub fn status(&self) -> CachePublishStatus {
        self.status
    }
}

#[derive(Debug, Clone)]
pub struct ProvisionedArtifact {
    executable_path: PathBuf,
    receipt_path: PathBuf,
}

impl ProvisionedArtifact {
    pub fn executable_path(&self) -> &Path {
        &self.executable_path
    }

    pub fn receipt_path(&self) -> &Path {
        &self.receipt_path
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheReceipt {
    schema_version: u8,
    kind: String,
    cache_format_version: u8,
    verifier_version: String,
    artifact_id: String,
    tool_version: String,
    pack_version: String,
    platform_id: String,
    pack_size: u64,
    pack_sha256: String,
    pack_manifest_sha256: String,
    files: Vec<PackFileRecord>,
    probes: Vec<ProbeResult>,
}

pub fn publish_cache(
    layout: &ArtifactCacheLayout,
    verified: &VerifiedPack,
    record: &ArtifactPackRecord,
    probes: &[ProbeResult],
) -> Result<CachePublication, ArtifactError> {
    record.validate()?;
    validate_verified_pack(verified, record)?;
    validate_probe_evidence(probes, record)?;
    layout.ensure()?;
    let final_path = layout.entry_path(&record.pack_sha256);
    match fs::symlink_metadata(&final_path) {
        Ok(_) => {
            return Ok(CachePublication {
                entry: open_cache(layout, record)?,
                status: CachePublishStatus::Reused,
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(corrupt_cache()),
    }

    let staging = tempfile::Builder::new()
        .prefix(".artifact-staging-")
        .tempdir_in(&layout.sha256_root)
        .map_err(|_| {
            error(
                "cache-staging-create",
                "artifact cache staging could not be created",
            )
        })?;
    create_private_directory(staging.path()).map_err(map_cache_io_error)?;
    for file in &verified.manifest.files {
        let source = verified.root().join(&file.path);
        let destination = staging.path().join(&file.path);
        copy_bound_file(&source, &destination, file, true)?;
    }
    let manifest_bytes = canonical_json(&verified.manifest)?;
    write_new_file(
        &staging.path().join(PACK_MANIFEST_FILE),
        &manifest_bytes,
        false,
        true,
    )?;
    let receipt = CacheReceipt {
        schema_version: 1,
        kind: "third_party_artifact_cache_receipt".to_string(),
        cache_format_version: CACHE_FORMAT_VERSION,
        verifier_version: VERIFIER_VERSION.to_string(),
        artifact_id: record.artifact_id.clone(),
        tool_version: record.tool_version.clone(),
        pack_version: record.pack_version.clone(),
        platform_id: record.platform_id.clone(),
        pack_size: verified.pack_size,
        pack_sha256: verified.pack_sha256.clone(),
        pack_manifest_sha256: verified.pack_manifest_sha256.clone(),
        files: verified.manifest.files.clone(),
        probes: probes.to_vec(),
    };
    validate_cache_receipt(&receipt, &verified.manifest, record)?;
    write_new_file(
        &staging.path().join(CACHE_RECEIPT_FILE),
        &canonical_json(&receipt)?,
        false,
        true,
    )?;
    sync_known_directories(staging.path(), &receipt.files)?;

    match fs::rename(staging.path(), &final_path) {
        Ok(()) => {
            sync_directory(&layout.sha256_root).map_err(map_cache_io_error)?;
            Ok(CachePublication {
                entry: open_cache(layout, record)?,
                status: CachePublishStatus::Published,
            })
        }
        Err(_) if final_path.exists() => Ok(CachePublication {
            entry: open_cache(layout, record)?,
            status: CachePublishStatus::Reused,
        }),
        Err(_) => Err(error(
            "cache-publish",
            "artifact cache entry could not be published",
        )),
    }
}

pub fn open_cache(
    layout: &ArtifactCacheLayout,
    record: &ArtifactPackRecord,
) -> Result<CachedArtifact, ArtifactError> {
    record.validate()?;
    let root = layout.entry_path(&record.pack_sha256);
    validate_cache_entry(&root, &layout.namespace_root, record).map_err(|_| corrupt_cache())
}

pub fn provision_from_cache(
    cached: &CachedArtifact,
    target_root: &Path,
    manifest: &ArtifactManifest,
) -> Result<ProvisionedArtifact, ArtifactError> {
    manifest.validate()?;
    if !target_root.is_absolute() {
        return Err(error(
            "target-root-not-absolute",
            "artifact target root must be absolute",
        ));
    }
    let target_root = resolve_absolute_path(target_root).map_err(|_| {
        error(
            "target-root-unavailable",
            "artifact target root could not be resolved",
        )
    })?;
    ensure_private_path(&target_root)?;
    if target_root.starts_with(&cached.cache_namespace_root)
        || cached.cache_namespace_root.starts_with(&target_root)
    {
        return Err(error(
            "target-cache-overlap",
            "artifact target and cache roots must be independent",
        ));
    }
    let record =
        manifest.select_active(&cached.receipt.artifact_id, &cached.receipt.platform_id)?;
    let refreshed = validate_cache_entry(&cached.root, &cached.cache_namespace_root, record)
        .map_err(|_| corrupt_cache())?;
    let relative_pack_root = target_pack_root(record)?;
    let pack_root = target_root.join(&relative_pack_root);
    if pack_root.exists() {
        return Err(error(
            "target-artifact-exists",
            "artifact target already contains the selected pack",
        ));
    }
    ensure_private_path(&pack_root)?;

    let cached_manifest = cached.root.join(PACK_MANIFEST_FILE);
    let target_manifest = pack_root.join(PACK_MANIFEST_FILE);
    let manifest_size = fs::metadata(&cached_manifest)
        .map_err(|_| corrupt_cache())?
        .len();
    copy_exact_file(
        &cached_manifest,
        &target_manifest,
        manifest_size,
        &record.pack_manifest_sha256,
        false,
        false,
    )?;

    for file in &refreshed.receipt.files {
        copy_bound_file(
            &cached.root.join(&file.path),
            &pack_root.join(&file.path),
            file,
            false,
        )?;
    }

    let mut installed_files = Vec::new();
    let mut license_files = Vec::new();
    installed_files.push(target_binding(
        &relative_pack_root.join(PACK_MANIFEST_FILE),
        manifest_size,
        &record.pack_manifest_sha256,
    )?);
    for file in &refreshed.receipt.files {
        let binding = target_binding(
            &relative_pack_root.join(&file.path),
            file.size,
            &file.sha256,
        )?;
        if file.role == PackFileRole::License {
            license_files.push(binding);
        } else {
            installed_files.push(binding);
        }
    }
    installed_files.sort_by(|left, right| left.path.cmp(&right.path));
    license_files.sort_by(|left, right| left.path.cmp(&right.path));
    let receipt = ArtifactReceipt {
        schema_version: 1,
        kind: "third_party_artifact_receipt".to_string(),
        distribution_manifest_sha256: sha256_bytes(&canonical_json(manifest)?),
        artifact_id: record.artifact_id.clone(),
        tool_version: record.tool_version.clone(),
        pack_version: record.pack_version.clone(),
        platform_id: record.platform_id.clone(),
        pack_sha256: record.pack_sha256.clone(),
        pack_manifest_sha256: record.pack_manifest_sha256.clone(),
        sbom_sha256: record.sbom_sha256.clone(),
        installed_files,
        license_files,
        probes: refreshed.receipt.probes.clone(),
        lifecycle_state: record.state,
    };
    receipt.validate()?;
    let receipts_root = target_root.join("runtime/artifact-receipts");
    ensure_private_path(&receipts_root)?;
    let receipt_path = receipts_root.join(format!("{}.json", record.artifact_id));
    write_new_file(&receipt_path, &canonical_json(&receipt)?, false, false)?;
    sync_directory(&receipts_root).map_err(map_cache_io_error)?;
    sync_known_directories(&pack_root, &refreshed.receipt.files)?;

    verify_target_receipt(&target_root, &record.artifact_id, manifest)?;
    Ok(ProvisionedArtifact {
        executable_path: pack_root.join(&record.executable.path),
        receipt_path,
    })
}

pub fn verify_target_receipt(
    target_root: &Path,
    artifact_id: &str,
    manifest: &ArtifactManifest,
) -> Result<ArtifactReceipt, ArtifactError> {
    manifest.validate()?;
    if !manifest
        .packs
        .iter()
        .any(|record| record.artifact_id == artifact_id)
    {
        return Err(error(
            "target-artifact-unknown",
            "target artifact is not present in the distribution manifest",
        ));
    }
    if !target_root.is_absolute() {
        return Err(error(
            "target-root-not-absolute",
            "artifact target root must be absolute",
        ));
    }
    let target_root = fs::canonicalize(target_root).map_err(|_| {
        error(
            "target-root-unavailable",
            "artifact target root could not be opened",
        )
    })?;
    let receipt_path = target_root
        .join("runtime/artifact-receipts")
        .join(format!("{artifact_id}.json"));
    let receipt_bytes = read_bounded(&receipt_path, MAX_MANIFEST_BYTES)?;
    let receipt: ArtifactReceipt = serde_json::from_slice(&receipt_bytes).map_err(|_| {
        error(
            "target-receipt-json",
            "artifact target receipt is not valid strict JSON",
        )
    })?;
    if canonical_json(&receipt)? != receipt_bytes {
        return Err(error(
            "target-receipt-canonical",
            "artifact target receipt bytes are not canonical",
        ));
    }
    receipt.validate()?;
    let manifest_sha256 = sha256_bytes(&canonical_json(manifest)?);
    let record = manifest.select_active(&receipt.artifact_id, &receipt.platform_id)?;
    if receipt.artifact_id != artifact_id
        || receipt.distribution_manifest_sha256 != manifest_sha256
        || receipt.tool_version != record.tool_version
        || receipt.pack_version != record.pack_version
        || receipt.pack_sha256 != record.pack_sha256
        || receipt.pack_manifest_sha256 != record.pack_manifest_sha256
        || receipt.sbom_sha256 != record.sbom_sha256
        || receipt.lifecycle_state != record.state
    {
        return Err(error(
            "target-receipt-binding",
            "artifact target receipt does not match the distribution manifest",
        ));
    }
    validate_probe_evidence(&receipt.probes, record)?;

    let relative_pack_root = target_pack_root(record)?;
    let pack_root = target_root.join(&relative_pack_root);
    let manifest_bytes = read_bounded(&pack_root.join(PACK_MANIFEST_FILE), MAX_MANIFEST_BYTES)?;
    if sha256_bytes(&manifest_bytes) != record.pack_manifest_sha256 {
        return Err(error(
            "target-pack-manifest-digest",
            "target pack manifest digest does not match the selected record",
        ));
    }
    let pack_manifest: PackManifest = serde_json::from_slice(&manifest_bytes).map_err(|_| {
        error(
            "target-pack-manifest-json",
            "target pack manifest is not valid strict JSON",
        )
    })?;
    pack_manifest.validate()?;
    if canonical_json(&pack_manifest)? != manifest_bytes {
        return Err(error(
            "target-pack-manifest-canonical",
            "target pack manifest bytes are not canonical",
        ));
    }
    validate_pack_manifest(&pack_manifest, record)?;

    let mut expected_installed = vec![target_binding(
        &relative_pack_root.join(PACK_MANIFEST_FILE),
        manifest_bytes.len() as u64,
        &record.pack_manifest_sha256,
    )?];
    let mut expected_licenses = Vec::new();
    for file in &pack_manifest.files {
        let binding = target_binding(
            &relative_pack_root.join(&file.path),
            file.size,
            &file.sha256,
        )?;
        if file.role == PackFileRole::License {
            expected_licenses.push(binding);
        } else {
            expected_installed.push(binding);
        }
    }
    expected_installed.sort_by(|left, right| left.path.cmp(&right.path));
    expected_licenses.sort_by(|left, right| left.path.cmp(&right.path));
    if receipt.installed_files != expected_installed || receipt.license_files != expected_licenses {
        return Err(error(
            "target-receipt-inventory",
            "artifact target receipt inventory is incomplete",
        ));
    }
    for binding in receipt
        .installed_files
        .iter()
        .chain(receipt.license_files.iter())
    {
        verify_binding(&target_root, binding)?;
    }
    let expected_pack_files: BTreeSet<String> = std::iter::once(PACK_MANIFEST_FILE.to_string())
        .chain(pack_manifest.files.iter().map(|file| file.path.clone()))
        .collect();
    if collect_regular_files(&pack_root, &expected_pack_files)? != expected_pack_files {
        return Err(error(
            "target-pack-inventory",
            "artifact target pack inventory is inconsistent",
        ));
    }
    Ok(receipt)
}

fn validate_verified_pack(
    verified: &VerifiedPack,
    record: &ArtifactPackRecord,
) -> Result<(), ArtifactError> {
    if verified.pack_sha256 != record.pack_sha256
        || verified.pack_size != record.expected_compressed_size
        || verified.pack_manifest_sha256 != record.pack_manifest_sha256
    {
        return Err(error(
            "cache-verified-pack-binding",
            "verified pack does not match the selected record",
        ));
    }
    validate_pack_manifest(&verified.manifest, record)?;
    if verified.files.len() != verified.manifest.files.len()
        || verified.manifest.files.iter().any(|file| {
            verified.files.get(&file.path).is_none_or(|observed| {
                observed.size != file.size
                    || observed.sha256 != file.sha256
                    || observed.role != file.role
            })
        })
    {
        return Err(error(
            "cache-verified-pack-inventory",
            "verified pack payload inventory is inconsistent",
        ));
    }
    Ok(())
}

fn validate_cache_entry(
    root: &Path,
    cache_namespace_root: &Path,
    record: &ArtifactPackRecord,
) -> Result<CachedArtifact, ArtifactError> {
    let metadata = fs::symlink_metadata(root).map_err(|_| corrupt_cache())?;
    if !metadata.file_type().is_dir() || is_symlink_or_reparse(root, &metadata) {
        return Err(corrupt_cache());
    }
    let receipt_bytes = read_bounded(&root.join(CACHE_RECEIPT_FILE), MAX_MANIFEST_BYTES)?;
    let receipt: CacheReceipt = serde_json::from_slice(&receipt_bytes).map_err(|_| {
        error(
            "cache-receipt-json",
            "artifact cache receipt is not valid strict JSON",
        )
    })?;
    if canonical_json(&receipt)? != receipt_bytes {
        return Err(error(
            "cache-receipt-canonical",
            "artifact cache receipt bytes are not canonical",
        ));
    }
    let manifest_bytes = read_bounded(&root.join(PACK_MANIFEST_FILE), MAX_MANIFEST_BYTES)?;
    if sha256_bytes(&manifest_bytes) != record.pack_manifest_sha256 {
        return Err(error(
            "cache-pack-manifest-digest",
            "artifact cache pack manifest digest is inconsistent",
        ));
    }
    let manifest: PackManifest = serde_json::from_slice(&manifest_bytes).map_err(|_| {
        error(
            "cache-pack-manifest-json",
            "artifact cache pack manifest is not valid strict JSON",
        )
    })?;
    manifest.validate()?;
    if canonical_json(&manifest)? != manifest_bytes {
        return Err(error(
            "cache-pack-manifest-canonical",
            "artifact cache pack manifest bytes are not canonical",
        ));
    }
    validate_cache_receipt(&receipt, &manifest, record)?;
    validate_pack_manifest(&manifest, record)?;
    for file in &receipt.files {
        verify_pack_file(root, file)?;
    }
    let expected: BTreeSet<String> = [
        CACHE_RECEIPT_FILE.to_string(),
        PACK_MANIFEST_FILE.to_string(),
    ]
    .into_iter()
    .chain(receipt.files.iter().map(|file| file.path.clone()))
    .collect();
    if collect_regular_files(root, &expected)? != expected {
        return Err(error(
            "cache-inventory",
            "artifact cache inventory is inconsistent",
        ));
    }
    Ok(CachedArtifact {
        root: root.to_path_buf(),
        cache_namespace_root: cache_namespace_root.to_path_buf(),
        receipt,
    })
}

fn validate_cache_receipt(
    receipt: &CacheReceipt,
    manifest: &PackManifest,
    record: &ArtifactPackRecord,
) -> Result<(), ArtifactError> {
    if receipt.schema_version != 1
        || receipt.kind != "third_party_artifact_cache_receipt"
        || receipt.cache_format_version != CACHE_FORMAT_VERSION
        || receipt.verifier_version != VERIFIER_VERSION
        || receipt.artifact_id != record.artifact_id
        || receipt.tool_version != record.tool_version
        || receipt.pack_version != record.pack_version
        || receipt.platform_id != record.platform_id
        || receipt.pack_size != record.expected_compressed_size
        || receipt.pack_sha256 != record.pack_sha256
        || receipt.pack_manifest_sha256 != record.pack_manifest_sha256
        || receipt.files != manifest.files
    {
        return Err(error(
            "cache-receipt-binding",
            "artifact cache receipt does not match the selected pack",
        ));
    }
    validate_probe_evidence(&receipt.probes, record)?;
    if canonical_json(receipt)?.len() > MAX_MANIFEST_BYTES {
        return Err(error(
            "cache-receipt-size-limit",
            "artifact cache receipt exceeds its byte limit",
        ));
    }
    Ok(())
}

fn validate_probe_evidence(
    probes: &[ProbeResult],
    record: &ArtifactPackRecord,
) -> Result<(), ArtifactError> {
    if probes.len() != 2
        || probes[0].probe_id != record.version_probe
        || probes[1].probe_id != record.capability_probe
    {
        return Err(error(
            "probe-evidence-binding",
            "artifact probe evidence does not match the selected record",
        ));
    }
    for probe in probes {
        probe.validate()?;
    }
    if probes[0].observed_version.as_deref() != Some(record.expected_version.as_str())
        || probes[1].observed_version.is_some()
    {
        return Err(error(
            "probe-evidence-version",
            "artifact probe evidence does not match the expected version",
        ));
    }
    Ok(())
}

fn validate_pack_manifest(
    manifest: &PackManifest,
    record: &ArtifactPackRecord,
) -> Result<(), ArtifactError> {
    if manifest.artifact_id != record.artifact_id
        || manifest.tool_version != record.tool_version
        || manifest.pack_version != record.pack_version
        || manifest.platform_id != record.platform_id
        || manifest.target_triple != record.target_triple
        || manifest.source_lock_sha256 != record.source_lock_sha256
        || manifest.project_asset_name != record.project_asset_name
    {
        return Err(error(
            "pack-identity-mismatch",
            "pack manifest identity does not match the selected record",
        ));
    }
    let executable = manifest
        .files
        .iter()
        .find(|file| file.role == PackFileRole::Executable)
        .ok_or_else(|| {
            error(
                "pack-executable-binding",
                "pack manifest has no executable binding",
            )
        })?;
    if !binding_matches(&record.executable, executable) {
        return Err(error(
            "pack-executable-binding",
            "pack executable does not match the selected record",
        ));
    }
    let licenses = manifest
        .files
        .iter()
        .filter(|file| file.role == PackFileRole::License)
        .collect::<Vec<_>>();
    if licenses.len() != record.license_files.len()
        || licenses
            .iter()
            .zip(&record.license_files)
            .any(|(file, binding)| !binding_matches(binding, file))
    {
        return Err(error(
            "pack-license-binding",
            "pack licenses do not match the selected record",
        ));
    }
    let sbom = manifest
        .files
        .iter()
        .find(|file| file.role == PackFileRole::Sbom)
        .ok_or_else(|| error("pack-sbom-binding", "pack manifest has no SBOM binding"))?;
    if sbom.path != "sbom.cdx.json" || sbom.sha256 != record.sbom_sha256 {
        return Err(error(
            "pack-sbom-binding",
            "pack SBOM does not match the selected record",
        ));
    }
    Ok(())
}

fn binding_matches(binding: &ArtifactFileBinding, file: &PackFileRecord) -> bool {
    binding.path == file.path && binding.size == file.size && binding.sha256 == file.sha256
}

fn copy_bound_file(
    source: &Path,
    destination: &Path,
    expected: &PackFileRecord,
    cache_permissions: bool,
) -> Result<(), ArtifactError> {
    copy_exact_file(
        source,
        destination,
        expected.size,
        &expected.sha256,
        expected.role == PackFileRole::Executable,
        cache_permissions,
    )
}

fn copy_exact_file(
    source: &Path,
    destination: &Path,
    expected_size: u64,
    expected_sha256: &str,
    executable: bool,
    cache_permissions: bool,
) -> Result<(), ArtifactError> {
    let mut input = open_regular_file_no_follow(source).map_err(|_| {
        error(
            "artifact-copy-source",
            "artifact copy source could not be opened safely",
        )
    })?;
    let parent = destination.parent().ok_or_else(|| {
        error(
            "artifact-copy-path",
            "artifact copy destination has no parent",
        )
    })?;
    ensure_private_path(parent)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| {
            error(
                "artifact-copy-create",
                "artifact copy destination could not be created",
            )
        })?;
    let (size, sha256) = copy_hash(&mut input, &mut output)?;
    if size != expected_size || sha256 != expected_sha256 {
        return Err(error(
            "artifact-copy-binding",
            "artifact copy does not match its verified binding",
        ));
    }
    set_file_mode(destination, executable, cache_permissions)?;
    output.sync_all().map_err(|_| {
        error(
            "artifact-copy-sync",
            "artifact copy could not be synchronized",
        )
    })
}

fn copy_hash(input: &mut File, output: &mut File) -> Result<(u64, String), ArtifactError> {
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let count = input.read(&mut buffer).map_err(|_| {
            error(
                "artifact-copy-read",
                "artifact copy source could not be read",
            )
        })?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .ok_or_else(|| error("artifact-copy-size", "artifact copy byte count overflowed"))?;
        digest.update(&buffer[..count]);
        output.write_all(&buffer[..count]).map_err(|_| {
            error(
                "artifact-copy-write",
                "artifact copy destination could not be written",
            )
        })?;
    }
    Ok((size, format!("{:x}", digest.finalize())))
}

fn write_new_file(
    path: &Path,
    bytes: &[u8],
    executable: bool,
    cache_permissions: bool,
) -> Result<(), ArtifactError> {
    let parent = path.parent().ok_or_else(|| {
        error(
            "artifact-file-path",
            "artifact file destination has no parent",
        )
    })?;
    ensure_private_path(parent)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| {
            error(
                "artifact-file-create",
                "artifact file destination could not be created",
            )
        })?;
    file.write_all(bytes).map_err(|_| {
        error(
            "artifact-file-write",
            "artifact file destination could not be written",
        )
    })?;
    set_file_mode(path, executable, cache_permissions)?;
    file.sync_all().map_err(|_| {
        error(
            "artifact-file-sync",
            "artifact file destination could not be synchronized",
        )
    })
}

fn verify_pack_file(root: &Path, expected: &PackFileRecord) -> Result<(), ArtifactError> {
    let binding = ArtifactFileBinding {
        path: expected.path.clone(),
        size: expected.size,
        sha256: expected.sha256.clone(),
    };
    verify_binding(root, &binding)
}

fn verify_binding(root: &Path, expected: &ArtifactFileBinding) -> Result<(), ArtifactError> {
    let mut file = open_regular_file_no_follow(&root.join(&expected.path)).map_err(|_| {
        error(
            "artifact-binding-open",
            "artifact bound file could not be opened safely",
        )
    })?;
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let count = file.read(&mut buffer).map_err(|_| {
            error(
                "artifact-binding-read",
                "artifact bound file could not be read",
            )
        })?;
        if count == 0 {
            break;
        }
        size = size.checked_add(count as u64).ok_or_else(|| {
            error(
                "artifact-binding-size",
                "artifact bound file size overflowed",
            )
        })?;
        if size > expected.size {
            return Err(error(
                "artifact-binding-mismatch",
                "artifact bound file does not match its receipt",
            ));
        }
        digest.update(&buffer[..count]);
    }
    if size != expected.size || format!("{:x}", digest.finalize()) != expected.sha256 {
        return Err(error(
            "artifact-binding-mismatch",
            "artifact bound file does not match its receipt",
        ));
    }
    Ok(())
}

fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, ArtifactError> {
    let file = open_regular_file_no_follow(path).map_err(|_| {
        error(
            "artifact-file-open",
            "artifact metadata file could not be opened safely",
        )
    })?;
    let maximum_u64 = maximum as u64;
    if file
        .metadata()
        .map_err(|_| {
            error(
                "artifact-file-metadata",
                "artifact metadata file could not be inspected",
            )
        })?
        .len()
        > maximum_u64
    {
        return Err(error(
            "artifact-file-size-limit",
            "artifact metadata file exceeds its byte limit",
        ));
    }
    let mut bytes = Vec::new();
    file.take(maximum_u64.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| {
            error(
                "artifact-file-read",
                "artifact metadata file could not be read",
            )
        })?;
    if bytes.len() > maximum {
        return Err(error(
            "artifact-file-size-limit",
            "artifact metadata file exceeds its byte limit",
        ));
    }
    Ok(bytes)
}

fn collect_regular_files(
    root: &Path,
    expected_files: &BTreeSet<String>,
) -> Result<BTreeSet<String>, ArtifactError> {
    let expected_directories = expected_files
        .iter()
        .flat_map(|path| {
            let mut directories = Vec::new();
            let mut current = Path::new(path).parent();
            while let Some(directory) = current {
                if !directory.as_os_str().is_empty() {
                    directories.push(path_to_slashes(directory).unwrap_or_default());
                }
                current = directory.parent();
            }
            directories
        })
        .collect::<BTreeSet<_>>();
    let mut observed = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|_| {
            error(
                "artifact-inventory-read",
                "artifact inventory directory could not be read",
            )
        })? {
            let entry = entry.map_err(|_| {
                error(
                    "artifact-inventory-read",
                    "artifact inventory entry could not be read",
                )
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|_| {
                error(
                    "artifact-inventory-metadata",
                    "artifact inventory entry could not be inspected",
                )
            })?;
            if is_symlink_or_reparse(&path, &metadata) {
                return Err(error(
                    "artifact-inventory-unsafe",
                    "artifact inventory contains a link or reparse point",
                ));
            }
            let relative = path.strip_prefix(root).map_err(|_| {
                error(
                    "artifact-inventory-path",
                    "artifact inventory path escaped its root",
                )
            })?;
            let relative = path_to_slashes(relative)?;
            if metadata.file_type().is_dir() {
                if !expected_directories.contains(&relative) {
                    return Err(error(
                        "artifact-inventory-unexpected",
                        "artifact inventory contains an unexpected directory",
                    ));
                }
                pending.push(path);
            } else if metadata.file_type().is_file() {
                if !expected_files.contains(&relative) || !observed.insert(relative) {
                    return Err(error(
                        "artifact-inventory-unexpected",
                        "artifact inventory contains an unexpected file",
                    ));
                }
                if observed.len() > MAX_CACHE_FILES {
                    return Err(error(
                        "artifact-inventory-limit",
                        "artifact inventory exceeds its file limit",
                    ));
                }
            } else {
                return Err(error(
                    "artifact-inventory-unsafe",
                    "artifact inventory contains an unsafe entry",
                ));
            }
        }
    }
    Ok(observed)
}

fn target_binding(
    path: &Path,
    size: u64,
    sha256: &str,
) -> Result<ArtifactFileBinding, ArtifactError> {
    Ok(ArtifactFileBinding {
        path: path_to_slashes(path)?,
        size,
        sha256: sha256.to_string(),
    })
}

fn target_pack_root(record: &ArtifactPackRecord) -> Result<PathBuf, ArtifactError> {
    validate_target_component(&record.pack_version)?;
    Ok(PathBuf::from("runtime")
        .join("third-party")
        .join(&record.artifact_id)
        .join(&record.pack_version))
}

fn validate_target_component(value: &str) -> Result<(), ArtifactError> {
    if value.is_empty()
        || value.len() > 255
        || matches!(value, "." | "..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(error(
            "target-pack-version-invalid",
            "artifact pack version is not a safe target path component",
        ));
    }
    Ok(())
}

fn path_to_slashes(path: &Path) -> Result<String, ArtifactError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_str().ok_or_else(|| {
                error("artifact-path-encoding", "artifact path is not valid UTF-8")
            })?),
            _ => {
                return Err(error(
                    "artifact-path-invalid",
                    "artifact path is not normalized and relative",
                ))
            }
        }
    }
    if parts.is_empty() {
        return Err(error("artifact-path-invalid", "artifact path is empty"));
    }
    Ok(parts.join("/"))
}

fn sync_known_directories(root: &Path, files: &[PackFileRecord]) -> Result<(), ArtifactError> {
    let directories = files
        .iter()
        .filter_map(|file| Path::new(&file.path).parent())
        .filter(|path| !path.as_os_str().is_empty())
        .collect::<BTreeSet<_>>();
    for directory in directories {
        sync_directory(&root.join(directory)).map_err(map_cache_io_error)?;
    }
    sync_directory(root).map_err(map_cache_io_error)
}

fn ensure_private_path(path: &Path) -> Result<(), ArtifactError> {
    let mut existing = path;
    let mut suffix = Vec::<OsString>::new();
    while !existing.exists() {
        let name = existing.file_name().ok_or_else(|| {
            error(
                "artifact-directory-create",
                "artifact directory has no existing ancestor",
            )
        })?;
        suffix.push(name.to_os_string());
        existing = existing.parent().ok_or_else(|| {
            error(
                "artifact-directory-create",
                "artifact directory has no existing ancestor",
            )
        })?;
    }
    let existing_metadata = fs::symlink_metadata(existing).map_err(|_| {
        error(
            "artifact-directory-metadata",
            "artifact directory ancestor could not be inspected",
        )
    })?;
    if !existing_metadata.file_type().is_dir()
        || is_symlink_or_reparse(existing, &existing_metadata)
    {
        return Err(error(
            "artifact-directory-unsafe",
            "artifact directory ancestor is unsafe",
        ));
    }
    let mut current = existing.to_path_buf();
    for component in suffix.into_iter().rev() {
        current.push(component);
        create_private_directory(&current).map_err(map_cache_io_error)?;
    }
    create_private_directory(path).map_err(map_cache_io_error)
}

fn reject_protected_root(
    root: &Path,
    protected: Option<&Path>,
    code: &'static str,
) -> Result<(), ArtifactError> {
    let Some(protected) = protected else {
        return Ok(());
    };
    if !protected.is_absolute() {
        return Err(error(
            "cache-boundary-not-absolute",
            "artifact cache protected boundary must be absolute",
        ));
    }
    let protected = resolve_absolute_path(protected).map_err(map_cache_root_error)?;
    if root.starts_with(protected) {
        return Err(error(
            code,
            "artifact cache root is inside a protected location",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn set_file_mode(
    path: &Path,
    executable: bool,
    cache_permissions: bool,
) -> Result<(), ArtifactError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = match (cache_permissions, executable) {
        (true, true) => 0o500,
        (true, false) => 0o400,
        (false, true) => 0o700,
        (false, false) => 0o600,
    };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|_| {
        error(
            "artifact-file-permission",
            "artifact file permissions could not be restricted",
        )
    })
}

#[cfg(windows)]
fn set_file_mode(
    path: &Path,
    _executable: bool,
    _cache_permissions: bool,
) -> Result<(), ArtifactError> {
    let file = OpenOptions::new().read(true).open(path).map_err(|_| {
        error(
            "artifact-file-permission",
            "artifact file permissions could not be inspected",
        )
    })?;
    set_private_file_permissions(&file).map_err(map_cache_io_error)
}

fn map_cache_root_error(
    cache_error: crate::impact_context::cache::file_facts::CacheError,
) -> ArtifactError {
    ArtifactError::new(
        cache_error.code,
        "artifact cache root policy rejected the path",
    )
}

fn map_cache_io_error(
    _cache_error: crate::impact_context::cache::file_facts::CacheError,
) -> ArtifactError {
    error(
        "artifact-cache-io",
        "artifact cache filesystem operation failed",
    )
}

fn corrupt_cache() -> ArtifactError {
    error(
        "corrupt-cache",
        "artifact cache entry is incomplete or inconsistent",
    )
}

fn error(code: &'static str, message: &'static str) -> ArtifactError {
    ArtifactError::new(code, message)
}
