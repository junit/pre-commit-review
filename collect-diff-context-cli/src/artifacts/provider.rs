use super::{
    contract::{
        canonical_json, sha256_bytes, ArtifactError, ArtifactManifest, ArtifactPackRecord,
        ArtifactRole, PackFileRecord, PackFileRole, PackManifest, SourceLock,
    },
    writer::{normalized_archive, read_canonical, read_regular, write_atomic, ArchiveFile},
};
use crate::repository_context_provider::{
    cli_contract::ProviderRegistry, contract::AuthorizedProviderProfile,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

const PROVIDER_PACK_VERSION: &str = "2026.07.27-pcr.2";
const PROVIDER_TOOL_VERSION: &str = "2026-07-27";
const PROVIDER_REPOSITORY: &str = "rust-lang/rust-analyzer";
const PROVIDER_SOURCE_LOCK_FILENAME: &str = "rust-analyzer-2026-07-27.json";
const PROVIDER_GENERATOR_CONFIG_FILENAME: &str = "generator-config.json";
const PROVIDER_SOURCE_LOCK_SHA256: &str =
    "38f5f8ea4f9cbec56d8dabb0ac4b992234ae069f76e7cfdeb46388017b3b22c5";
const MAX_ARCHIVE_BYTES: usize = 512 * 1024 * 1024;
const MAX_EXECUTABLE_BYTES: usize = 128 * 1024 * 1024;
const MAX_LICENSE_BYTES: usize = 1024 * 1024;

pub fn select_provider_install_record<'a>(
    manifest: &'a ArtifactManifest,
    platform_id: &str,
) -> Result<&'a ArtifactPackRecord, ArtifactError> {
    let record = manifest.select_active("rust-analyzer", platform_id)?;
    if record.artifact_role != ArtifactRole::RepositoryContextProvider {
        return Err(ArtifactError::new(
            "provider-install-record",
            "rust-analyzer installation requires a provider pack",
        ));
    }
    Ok(record)
}

pub fn release_threshold_ms(p95_ms: u64) -> Result<u64, ArtifactError> {
    if p95_ms == 0 {
        return Err(ArtifactError::new(
            "baseline-threshold-range",
            "provider baseline p95 must be positive",
        ));
    }
    p95_ms
        .checked_mul(5)
        .map(|scaled| scaled.div_ceil(4))
        .and_then(|scaled| scaled.checked_add(250))
        .ok_or_else(|| {
            ArtifactError::new(
                "baseline-threshold-overflow",
                "provider baseline threshold arithmetic overflowed",
            )
        })
}

pub fn accept_p95(observed_p95_ms: u64, baseline_p95_ms: u64) -> Result<bool, ArtifactError> {
    Ok(observed_p95_ms <= release_threshold_ms(baseline_p95_ms)?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedProvider {
    pub staging_target: PathBuf,
    pub provider_version: String,
    pub executable_relative_path: PathBuf,
    pub executable_sha256: String,
    pub target_triple: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedProviderAuthorization {
    pub profile: AuthorizedProviderProfile,
    pub registry: ProviderRegistry,
    pub profile_bytes: Vec<u8>,
    pub registry_bytes: Vec<u8>,
}

pub fn generate_provider_authorization(
    final_target: &Path,
    verified: &VerifiedProvider,
) -> Result<GeneratedProviderAuthorization, ArtifactError> {
    let final_target = resolve_final_target(final_target)?;
    verify_staged_executable(verified)?;

    let profile = AuthorizedProviderProfile::rust_analyzer(
        verified.provider_version.clone(),
        verified.executable_sha256.clone(),
        verified.target_triple.clone(),
    );
    profile.validate().map_err(|error| {
        ArtifactError::new(
            "provider-profile-binding",
            format!("generated provider profile is invalid: {error}"),
        )
    })?;
    let registry = ProviderRegistry::rust_analyzer(
        final_target.join("runtime/providers/rust-analyzer.profile.json"),
        final_target.join(&verified.executable_relative_path),
        &profile,
    );
    registry.validate().map_err(|error| {
        ArtifactError::new(
            "provider-registry-binding",
            format!("generated provider registry is invalid: {error}"),
        )
    })?;
    registry
        .validate_profile_binding(&profile)
        .map_err(|error| {
            ArtifactError::new(
                "provider-registry-binding",
                format!("generated provider registry is unbound: {error}"),
            )
        })?;

    let profile_bytes = canonical_json(&profile)?;
    let registry_bytes = canonical_json(&registry)?;
    if sha256_bytes(&profile_bytes) != profile.sha256()
        || sha256_bytes(&registry_bytes) != registry.sha256()
    {
        return Err(ArtifactError::new(
            "provider-authorization-digest",
            "generated provider authorization digest drifted",
        ));
    }

    Ok(GeneratedProviderAuthorization {
        profile,
        registry,
        profile_bytes,
        registry_bytes,
    })
}

fn resolve_final_target(final_target: &Path) -> Result<PathBuf, ArtifactError> {
    if !final_target.is_absolute() {
        return Err(ArtifactError::new(
            "provider-final-target",
            "provider final target must be absolute",
        ));
    }
    let parent = final_target.parent().ok_or_else(|| {
        ArtifactError::new(
            "provider-final-target",
            "provider final target must have an existing parent",
        )
    })?;
    let name = final_target.file_name().ok_or_else(|| {
        ArtifactError::new(
            "provider-final-target",
            "provider final target must name a target directory",
        )
    })?;
    let canonical_parent = fs::canonicalize(parent).map_err(|_| {
        ArtifactError::new(
            "provider-final-target",
            "provider final target parent could not be resolved",
        )
    })?;
    if !fs::metadata(&canonical_parent)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
    {
        return Err(ArtifactError::new(
            "provider-final-target",
            "provider final target parent must be a directory",
        ));
    }
    Ok(canonical_parent.join(name))
}

fn verify_staged_executable(verified: &VerifiedProvider) -> Result<(), ArtifactError> {
    if verified.executable_relative_path.as_os_str().is_empty()
        || verified.executable_relative_path.is_absolute()
        || verified
            .executable_relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ArtifactError::new(
            "provider-staging-path",
            "provider executable path must be normalized and staging-relative",
        ));
    }

    let canonical_staging = fs::canonicalize(&verified.staging_target).map_err(|_| {
        ArtifactError::new(
            "provider-staging-path",
            "provider staging target could not be resolved",
        )
    })?;
    if !fs::metadata(&canonical_staging)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
    {
        return Err(ArtifactError::new(
            "provider-staging-path",
            "provider staging target must be a directory",
        ));
    }

    let staged_executable = verified
        .staging_target
        .join(&verified.executable_relative_path);
    let metadata = fs::symlink_metadata(&staged_executable).map_err(|_| {
        ArtifactError::new(
            "provider-staging-path",
            "provider staging executable is missing",
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(ArtifactError::new(
            "provider-staging-path",
            "provider staging executable must be a regular file",
        ));
    }
    let canonical_executable = fs::canonicalize(&staged_executable).map_err(|_| {
        ArtifactError::new(
            "provider-staging-path",
            "provider staging executable could not be resolved",
        )
    })?;
    if !canonical_executable.starts_with(&canonical_staging) {
        return Err(ArtifactError::new(
            "provider-staging-path",
            "provider staging executable escapes its target",
        ));
    }
    let executable = fs::read(&canonical_executable).map_err(|_| {
        ArtifactError::new(
            "provider-staging-path",
            "provider staging executable could not be read",
        )
    })?;
    if sha256_bytes(&executable) != verified.executable_sha256 {
        return Err(ArtifactError::new(
            "provider-executable-binding",
            "provider staging executable digest does not match its verified binding",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderLicenseInput {
    pub source_path: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPackInput {
    pub tool_version: String,
    pub pack_version: String,
    pub platform_id: String,
    pub target_triple: String,
    pub source_lock_sha256: String,
    pub upstream_repository: String,
    pub upstream_tag: String,
    pub upstream_asset_name: String,
    pub upstream_archive: Vec<u8>,
    pub executable_name: String,
    pub executable: Vec<u8>,
    pub licenses: Vec<ProviderLicenseInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltProviderPack {
    pub archive: Vec<u8>,
    pub manifest: PackManifest,
    pub manifest_bytes: Vec<u8>,
    pub sbom: Value,
    pub sbom_bytes: Vec<u8>,
}

pub struct RustAnalyzerPackOptions<'a> {
    pub platform_id: &'a str,
    pub pack_version: &'a str,
    pub source_lock_path: &'a Path,
    pub generator_config_path: &'a Path,
    pub output_path: &'a Path,
    pub manifest_output: Option<&'a Path>,
    pub sbom_output: Option<&'a Path>,
}

pub fn write_rust_analyzer_pack(
    options: &RustAnalyzerPackOptions<'_>,
) -> Result<BuiltProviderPack, String> {
    let prepared_input_root = options
        .source_lock_path
        .parent()
        .ok_or_else(|| "provider source lock has no prepared input root".to_string())?;
    if options
        .source_lock_path
        .file_name()
        .and_then(|name| name.to_str())
        != Some(PROVIDER_SOURCE_LOCK_FILENAME)
        || options.generator_config_path.parent() != Some(prepared_input_root)
        || options
            .generator_config_path
            .file_name()
            .and_then(|name| name.to_str())
            != Some(PROVIDER_GENERATOR_CONFIG_FILENAME)
    {
        return Err("provider inputs are not contained by one prepared input root".to_string());
    }
    let (source_lock, source_lock_bytes) = read_canonical::<SourceLock>(options.source_lock_path)?;
    source_lock.validate().map_err(|error| error.to_string())?;
    let source_lock_sha256 = sha256_bytes(&source_lock_bytes);
    if source_lock.artifact_id != "rust-analyzer"
        || source_lock_sha256 != PROVIDER_SOURCE_LOCK_SHA256
        || options.pack_version != PROVIDER_PACK_VERSION
    {
        return Err("provider pack inputs do not bind the reviewed source lock".to_string());
    }
    let asset = source_lock
        .assets
        .iter()
        .find(|asset| asset.platform_id == options.platform_id)
        .ok_or_else(|| "reviewed source lock has no asset for the platform".to_string())?;
    let expected_generator_config = json!({
        "compression": "gzip-level-9",
        "gzip_mtime": 0,
        "gzip_os": 255,
        "pack_version": options.pack_version,
        "platform_id": options.platform_id,
        "rust_toolchain": "1.95.0",
        "tar_format": "posix-ustar"
    });
    let (generator_config, _) = read_canonical::<Value>(options.generator_config_path)
        .map_err(|_| "provider generator configuration is not canonical".to_string())?;
    if generator_config != expected_generator_config {
        return Err("provider generator configuration is not canonical".to_string());
    }

    let upstream_archive = read_regular(&prepared_input_root.join(&asset.archive_name))?;
    if upstream_archive.len() as u64 != asset.archive_size
        || sha256_bytes(&upstream_archive) != asset.archive_sha256
    {
        return Err("provider upstream archive does not match the source lock".to_string());
    }
    let executable = read_regular(&prepared_input_root.join(&asset.executable_name))?;
    if executable.len() as u64 != asset.executable_size
        || sha256_bytes(&executable) != asset.executable_sha256
    {
        return Err("provider executable does not match the source lock".to_string());
    }
    let version_output = read_regular(&prepared_input_root.join("version-output.txt"))?;
    let observed_version = std::str::from_utf8(&version_output)
        .map_err(|_| "provider version output is not UTF-8".to_string())?
        .trim_end_matches(['\r', '\n']);
    if observed_version != asset.expected_version_output {
        return Err("provider version output does not match the source lock".to_string());
    }
    let licenses = asset
        .license_source_paths
        .iter()
        .map(|source_path| {
            Ok(ProviderLicenseInput {
                source_path: source_path.clone(),
                bytes: read_regular(&prepared_input_root.join(source_path))?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let built = build_provider_pack(&ProviderPackInput {
        tool_version: source_lock.tool_version,
        pack_version: options.pack_version.to_string(),
        platform_id: asset.platform_id.clone(),
        target_triple: asset.target_triple.clone(),
        source_lock_sha256,
        upstream_repository: source_lock.upstream_repository,
        upstream_tag: source_lock.upstream_tag,
        upstream_asset_name: asset.archive_name.clone(),
        upstream_archive,
        executable_name: asset.executable_name.clone(),
        executable,
        licenses,
    })?;
    write_atomic(options.output_path, &built.archive)?;
    if let Some(path) = options.manifest_output {
        write_atomic(path, &built.manifest_bytes)?;
    }
    if let Some(path) = options.sbom_output {
        write_atomic(path, &built.sbom_bytes)?;
    }
    Ok(built)
}

impl BuiltProviderPack {
    pub fn release_metadata(&self) -> Value {
        let executable = self
            .manifest
            .files
            .iter()
            .find(|file| file.role == PackFileRole::Executable)
            .expect("provider pack construction always emits one executable");
        json!({
            "artifact_id": self.manifest.artifact_id,
            "pack_version": self.manifest.pack_version,
            "platform_id": self.manifest.platform_id,
            "project_asset_name": self.manifest.project_asset_name,
            "pack_sha256": sha256_bytes(&self.archive),
            "pack_manifest_sha256": sha256_bytes(&self.manifest_bytes),
            "sbom_sha256": sha256_bytes(&self.sbom_bytes),
            "executable_sha256": executable.sha256,
            "source_lock_sha256": self.manifest.source_lock_sha256,
            "upstream_archive_sha256": self.manifest.upstream_asset_sha256
        })
    }
}

pub fn build_provider_pack(input: &ProviderPackInput) -> Result<BuiltProviderPack, String> {
    validate_input(input)?;

    let archive_sha256 = sha256_bytes(&input.upstream_archive);
    let executable_sha256 = sha256_bytes(&input.executable);
    let executable_path = format!("bin/{}", input.executable_name);
    let project_asset_name = format!(
        "pre-commit-review-rust-analyzer-{}-{}.tar.gz",
        input.pack_version, input.platform_id
    );
    let component_ref = format!("pkg:github/rust-lang/rust-analyzer@{}", input.tool_version);
    let pack_ref = format!(
        "urn:pre-commit-review:pack:rust-analyzer:{}:{}",
        input.pack_version, input.platform_id
    );
    let source_url = format!(
        "https://github.com/{}/releases/download/{}/{}",
        input.upstream_repository, input.upstream_tag, input.upstream_asset_name
    );
    let sbom = json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": { "component": {
            "type": "application",
            "bom-ref": pack_ref,
            "name": "pre-commit-review-rust-analyzer-pack",
            "version": input.pack_version
        }},
        "components": [{
            "type": "application",
            "bom-ref": component_ref,
            "name": "rust-analyzer",
            "version": input.tool_version,
            "supplier": { "name": "The rust-analyzer developers" },
            "purl": component_ref,
            "hashes": [{ "alg": "SHA-256", "content": executable_sha256 }],
            "licenses": [
                { "license": { "id": "Apache-2.0" } },
                { "license": { "id": "MIT" } }
            ],
            "externalReferences": [{
                "type": "distribution",
                "url": source_url,
                "hashes": [{ "alg": "SHA-256", "content": archive_sha256 }]
            }],
            "properties": [
                { "name": "pre-commit-review:artifact-id", "value": "rust-analyzer" },
                { "name": "pre-commit-review:pack-version", "value": input.pack_version },
                { "name": "pre-commit-review:platform-id", "value": input.platform_id },
                { "name": "pre-commit-review:evidence-scope", "value": "component-evidence" },
                { "name": "pre-commit-review:relationship", "value": "contains" },
                { "name": "pre-commit-review:transitive-closure", "value": "unknown" }
            ]
        }],
        "dependencies": [{ "ref": pack_ref, "dependsOn": [component_ref] }]
    });
    let sbom_bytes = canonical_json(&sbom).map_err(|error| error.to_string())?;

    let mut files = BTreeMap::new();
    files.insert(
        executable_path.clone(),
        ArchiveFile {
            bytes: input.executable.clone(),
            mode: 0o755,
        },
    );
    for license in &input.licenses {
        files.insert(
            format!("licenses/{}", license.source_path),
            ArchiveFile {
                bytes: license.bytes.clone(),
                mode: 0o644,
            },
        );
    }
    files.insert(
        "sbom.cdx.json".to_string(),
        ArchiveFile {
            bytes: sbom_bytes.clone(),
            mode: 0o644,
        },
    );

    let manifest = PackManifest {
        schema_version: 1,
        kind: "third_party_artifact_pack".to_string(),
        artifact_id: "rust-analyzer".to_string(),
        tool_version: input.tool_version.clone(),
        pack_version: input.pack_version.clone(),
        platform_id: input.platform_id.clone(),
        target_triple: input.target_triple.clone(),
        upstream_asset_name: input.upstream_asset_name.clone(),
        upstream_asset_sha256: archive_sha256,
        source_lock_sha256: input.source_lock_sha256.clone(),
        project_asset_name,
        files: files
            .iter()
            .map(|(path, file)| PackFileRecord {
                path: path.clone(),
                size: file.bytes.len() as u64,
                sha256: sha256_bytes(&file.bytes),
                role: if path.starts_with("bin/") {
                    PackFileRole::Executable
                } else if path.starts_with("licenses/") {
                    PackFileRole::License
                } else {
                    PackFileRole::Sbom
                },
            })
            .collect(),
    };
    manifest.validate().map_err(|error| error.to_string())?;
    let manifest_bytes = canonical_json(&manifest).map_err(|error| error.to_string())?;
    files.insert(
        "pack-manifest.json".to_string(),
        ArchiveFile {
            bytes: manifest_bytes.clone(),
            mode: 0o644,
        },
    );
    let archive = normalized_archive(&files)?;

    Ok(BuiltProviderPack {
        archive,
        manifest,
        manifest_bytes,
        sbom,
        sbom_bytes,
    })
}

fn validate_input(input: &ProviderPackInput) -> Result<(), String> {
    if input.tool_version != PROVIDER_TOOL_VERSION
        || input.pack_version != PROVIDER_PACK_VERSION
        || input.upstream_repository != PROVIDER_REPOSITORY
        || input.upstream_tag != PROVIDER_TOOL_VERSION
    {
        return Err("provider pack input does not match the reviewed release identity".to_string());
    }
    let expected = match input.platform_id.as_str() {
        "darwin-amd64" => ("x86_64-apple-darwin", "rust-analyzer"),
        "darwin-arm64" => ("aarch64-apple-darwin", "rust-analyzer"),
        "linux-amd64" => ("x86_64-unknown-linux-gnu", "rust-analyzer"),
        "windows-amd64" => ("x86_64-pc-windows-msvc", "rust-analyzer.exe"),
        _ => return Err("provider pack platform is not supported".to_string()),
    };
    if input.target_triple != expected.0 || input.executable_name != expected.1 {
        return Err("provider pack target does not match its platform".to_string());
    }
    if !is_sha256(&input.source_lock_sha256) {
        return Err("provider pack source lock digest is invalid".to_string());
    }
    if !plain_filename(&input.upstream_asset_name) {
        return Err("provider pack upstream asset name is invalid".to_string());
    }
    if input.upstream_archive.is_empty() || input.upstream_archive.len() > MAX_ARCHIVE_BYTES {
        return Err("provider pack upstream archive is outside its byte limit".to_string());
    }
    if input.executable.is_empty() || input.executable.len() > MAX_EXECUTABLE_BYTES {
        return Err("provider executable is outside its byte limit".to_string());
    }
    if input.licenses.len() != 2
        || input.licenses[0].source_path != "LICENSE-APACHE"
        || input.licenses[1].source_path != "LICENSE-MIT"
        || input
            .licenses
            .iter()
            .any(|license| license.bytes.is_empty() || license.bytes.len() > MAX_LICENSE_BYTES)
    {
        return Err("provider license inputs do not match the reviewed paths".to_string());
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn plain_filename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.contains(['/', '\\'])
        && !matches!(value, "." | "..")
        && !value.chars().any(char::is_control)
}
