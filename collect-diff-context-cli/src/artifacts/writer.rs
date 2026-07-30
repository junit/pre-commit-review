use super::{
    contract::{
        canonical_json, sha256_bytes, ArtifactFileBinding, ArtifactManifest, ArtifactPackRecord,
        ArtifactRole, ArtifactState, CorePackFileBinding, CorePackManifest, PackFileRecord,
        PackFileRole, PackFormat, PackManifest, ProbeId, RevocationIndex, SourceLock,
    },
    pack::{verify_pack, VerifyLimits},
};
use flate2::{write::GzEncoder, Compression, GzBuilder};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

pub type WriterResult<T> = Result<T, String>;

pub struct GitleaksPackOptions<'a> {
    pub platform_id: &'a str,
    pub pack_version: &'a str,
    pub source_root: &'a Path,
    pub manifest_path: &'a Path,
    pub source_lock_path: &'a Path,
    pub binary_path: &'a Path,
    pub output_path: &'a Path,
    pub record_output: Option<&'a Path>,
    pub manifest_output: Option<&'a Path>,
}

pub struct CorePackOptions<'a> {
    pub platform_id: &'a str,
    pub pack_version: &'a str,
    pub source_root: &'a Path,
    pub manifest_path: &'a Path,
    pub revocations_path: &'a Path,
    pub output_path: &'a Path,
    pub record_output: Option<&'a Path>,
}

#[derive(Clone)]
pub(crate) struct ArchiveFile {
    pub(crate) bytes: Vec<u8>,
    pub(crate) mode: u32,
}

pub fn write_gitleaks_pack(options: &GitleaksPackOptions<'_>) -> WriterResult<ArtifactPackRecord> {
    let (distribution, _) = read_canonical::<ArtifactManifest>(options.manifest_path)?;
    distribution.validate().map_err(|error| error.to_string())?;
    let (source_lock, source_lock_bytes) = read_canonical::<SourceLock>(options.source_lock_path)?;
    source_lock.validate().map_err(|error| error.to_string())?;
    if source_lock.artifact_id != "gitleaks" {
        return Err("source lock is not for Gitleaks".to_string());
    }
    if let Some(active) = distribution.packs.iter().find(|record| {
        record.artifact_id == "gitleaks"
            && record.platform_id == options.platform_id
            && record.state == ArtifactState::Active
    }) {
        if active.pack_version != options.pack_version {
            return Err("manifest active pack version does not match --pack-version".to_string());
        }
    }
    let asset = source_lock
        .assets
        .iter()
        .find(|asset| asset.platform_id == options.platform_id)
        .ok_or_else(|| format!("source lock has no asset for {}", options.platform_id))?;

    let executable_bytes = read_regular(options.binary_path)?;
    let executable_sha256 = sha256_bytes(&executable_bytes);
    if executable_bytes.len() as u64 != asset.executable_size
        || executable_sha256 != asset.executable_sha256
    {
        return Err("Gitleaks executable does not match the locked source asset".to_string());
    }
    let license_path = options
        .source_root
        .join("THIRD_PARTY_LICENSES/gitleaks-LICENSE");
    let license_bytes = read_regular(&license_path)?;
    let configuration_bytes = read_regular(
        &options
            .source_root
            .join("references/security/gitleaks.toml"),
    )?;
    let executable_name = if options.platform_id == "windows-amd64" {
        "gitleaks.exe"
    } else {
        "gitleaks"
    };
    let executable_path = format!("bin/{executable_name}");
    let source_lock_sha256 = sha256_bytes(&source_lock_bytes);
    let project_asset_name = format!(
        "pre-commit-review-gitleaks-{}-{}.tar.gz",
        options.pack_version, options.platform_id
    );
    let sbom_component = format!("pkg:github/gitleaks/gitleaks@{}", source_lock.tool_version);
    let pack_ref = format!(
        "urn:pre-commit-review:pack:gitleaks:{}:{}",
        options.pack_version, options.platform_id
    );
    let source_url = format!("https://github.com/{}", source_lock.upstream_repository);
    let sbom = canonical_json(&json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "bom-ref": pack_ref,
                "name": "pre-commit-review-gitleaks-pack",
                "version": options.pack_version
            }
        },
        "components": [{
            "type": "application",
            "bom-ref": sbom_component,
            "name": "gitleaks",
            "version": source_lock.tool_version,
            "supplier": { "name": "Gitleaks" },
            "purl": sbom_component,
            "hashes": [{ "alg": "SHA-256", "content": executable_sha256 }],
            "licenses": [{ "license": { "id": "MIT" } }],
            "externalReferences": [
                { "type": "website", "url": source_url },
                {
                    "type": "distribution",
                    "url": asset.url,
                    "hashes": [{ "alg": "SHA-256", "content": asset.archive_sha256 }]
                }
            ],
            "properties": [
                { "name": "pre-commit-review:artifact-id", "value": "gitleaks" },
                { "name": "pre-commit-review:pack-version", "value": options.pack_version },
                { "name": "pre-commit-review:platform-id", "value": options.platform_id },
                { "name": "pre-commit-review:evidence-scope", "value": "component-evidence" },
                { "name": "pre-commit-review:transitive-closure", "value": "unknown" }
            ]
        }],
        "dependencies": [{ "ref": pack_ref, "dependsOn": [sbom_component] }]
    }))
    .map_err(|error| error.to_string())?;
    let sbom_sha256 = sha256_bytes(&sbom);

    let mut files = BTreeMap::new();
    files.insert(
        executable_path.clone(),
        ArchiveFile {
            bytes: executable_bytes,
            mode: 0o755,
        },
    );
    files.insert(
        "licenses/GITLEAKS-LICENSE".to_string(),
        ArchiveFile {
            bytes: license_bytes,
            mode: 0o644,
        },
    );
    files.insert(
        "sbom.cdx.json".to_string(),
        ArchiveFile {
            bytes: sbom,
            mode: 0o644,
        },
    );
    let manifest_files = files
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
        .collect();
    let pack_manifest = PackManifest {
        schema_version: 1,
        kind: "third_party_artifact_pack".to_string(),
        artifact_id: "gitleaks".to_string(),
        tool_version: source_lock.tool_version.clone(),
        pack_version: options.pack_version.to_string(),
        platform_id: options.platform_id.to_string(),
        target_triple: asset.target_triple.clone(),
        upstream_asset_name: asset.archive_name.clone(),
        upstream_asset_sha256: asset.archive_sha256.clone(),
        source_lock_sha256: source_lock_sha256.clone(),
        project_asset_name: project_asset_name.clone(),
        files: manifest_files,
    };
    pack_manifest
        .validate()
        .map_err(|error| error.to_string())?;
    let pack_manifest_bytes = canonical_json(&pack_manifest).map_err(|error| error.to_string())?;
    let pack_manifest_sha256 = sha256_bytes(&pack_manifest_bytes);
    files.insert(
        "pack-manifest.json".to_string(),
        ArchiveFile {
            bytes: pack_manifest_bytes,
            mode: 0o644,
        },
    );

    let pack = normalized_archive(&files)?;
    let record = ArtifactPackRecord {
        artifact_id: "gitleaks".to_string(),
        artifact_role: ArtifactRole::Sanitizer,
        tool_version: source_lock.tool_version.clone(),
        upstream_repository: source_lock.upstream_repository.clone(),
        upstream_tag: source_lock.upstream_tag.clone(),
        upstream_commit: source_lock.upstream_commit.clone(),
        source_lock_sha256,
        platform_id: options.platform_id.to_string(),
        target_triple: asset.target_triple.clone(),
        state: ArtifactState::Active,
        pack_version: options.pack_version.to_string(),
        project_release_tag: format!("artifact-gitleaks-{}", options.pack_version),
        project_asset_name,
        expected_compressed_size: pack.len() as u64,
        max_compressed_size: pack.len() as u64,
        pack_sha256: sha256_bytes(&pack),
        pack_manifest_sha256,
        sbom_sha256,
        pack_format: PackFormat::NormalizedTarGzipV1,
        executable: binding(&executable_path, &files[&executable_path].bytes),
        version_probe: ProbeId::GitleaksVersionV1,
        capability_probe: ProbeId::GitleaksStdinJsonV1,
        expected_version: asset.expected_version_output.clone(),
        license_component: "gitleaks".to_string(),
        license_files: vec![binding(
            "licenses/GITLEAKS-LICENSE",
            &files["licenses/GITLEAKS-LICENSE"].bytes,
        )],
        sbom_component,
        default_configuration_sha256: Some(sha256_bytes(&configuration_bytes)),
        quality_baseline_sha256: None,
        revoked_reason: None,
        replacement_pack_version: None,
    };
    verify_pack(pack.as_slice(), &record, &VerifyLimits::default())
        .map_err(|error| format!("writer verification failed: {error}"))?;
    write_atomic(options.output_path, &pack)?;
    if let Some(path) = options.record_output {
        write_atomic(
            path,
            &canonical_json(&record).map_err(|error| error.to_string())?,
        )?;
    }
    if let Some(path) = options.manifest_output {
        let mut updated = distribution;
        updated.packs.retain(|existing| {
            existing.artifact_id != record.artifact_id
                || existing.platform_id != record.platform_id
                || existing.state != ArtifactState::Active
        });
        updated.packs.push(record.clone());
        updated.packs.sort_by(|left, right| {
            (&left.artifact_id, &left.platform_id, &left.pack_version).cmp(&(
                &right.artifact_id,
                &right.platform_id,
                &right.pack_version,
            ))
        });
        updated.validate().map_err(|error| error.to_string())?;
        write_atomic(
            path,
            &canonical_json(&updated).map_err(|error| error.to_string())?,
        )?;
    }
    Ok(record)
}

pub fn write_core_pack(options: &CorePackOptions<'_>) -> WriterResult<serde_json::Value> {
    let (distribution, distribution_bytes) =
        read_canonical::<ArtifactManifest>(options.manifest_path)?;
    distribution.validate().map_err(|error| error.to_string())?;
    let (revocations, revocation_bytes) =
        read_canonical::<RevocationIndex>(options.revocations_path)?;
    revocations.validate().map_err(|error| error.to_string())?;
    if distribution.revocation_index_sha256 != sha256_bytes(&revocation_bytes) {
        return Err("distribution manifest does not bind the revocation index".to_string());
    }
    let target_triple = target_triple(options.platform_id)?;
    let mut files = BTreeMap::new();
    for name in ["SKILL.md", "LICENSE", "install.sh"] {
        add_source_file(&mut files, name, &options.source_root.join(name))?;
    }
    for (source, prefix) in [
        ("agents", "agents"),
        ("references", "references"),
        ("docs", "docs"),
        ("THIRD_PARTY_LICENSES", "THIRD_PARTY_LICENSES"),
        (
            "collect-diff-context-cli/schemas",
            "collect-diff-context-cli/schemas",
        ),
    ] {
        add_tree(&mut files, &options.source_root.join(source), prefix, false)?;
    }
    add_tree(
        &mut files,
        &options.source_root.join("scripts"),
        "scripts",
        true,
    )?;
    let suffix = if options.platform_id == "windows-amd64" {
        ".exe"
    } else {
        ""
    };
    for prefix in [
        "collect_diff_context",
        "static_analysis",
        "repository_context",
        "repository_context_provider",
    ] {
        let name = format!("{prefix}-{}{suffix}", options.platform_id);
        add_source_file(
            &mut files,
            &format!("scripts/bin/{name}"),
            &options.source_root.join("scripts/bin").join(name),
        )?;
    }
    files.insert(
        "runtime/distribution/manifest.json".to_string(),
        ArchiveFile {
            bytes: distribution_bytes.clone(),
            mode: 0o644,
        },
    );
    files.insert(
        "runtime/distribution/revocations.json".to_string(),
        ArchiveFile {
            bytes: revocation_bytes.clone(),
            mode: 0o644,
        },
    );
    let binary_components: Vec<_> = files
        .iter()
        .filter(|(path, _)| path.starts_with("scripts/bin/"))
        .map(|(path, file)| {
            json!({
                "type": "application",
                "bom-ref": format!("urn:pre-commit-review:core:{}:{}", options.platform_id, path),
                "name": path.rsplit('/').next().unwrap_or(path),
                "version": options.pack_version,
                "hashes": [{ "alg": "SHA-256", "content": sha256_bytes(&file.bytes) }]
            })
        })
        .collect();
    let component_refs: Vec<_> = binary_components
        .iter()
        .filter_map(|component| component.get("bom-ref").cloned())
        .collect();
    let core_ref = format!(
        "urn:pre-commit-review:core-pack:{}:{}",
        options.pack_version, options.platform_id
    );
    let core_sbom = canonical_json(&json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": { "component": {
            "type": "application",
            "bom-ref": core_ref,
            "name": "pre-commit-review-core",
            "version": options.pack_version,
            "properties": [{ "name": "pre-commit-review:platform-id", "value": options.platform_id }]
        }},
        "components": binary_components,
        "dependencies": [{ "ref": core_ref, "dependsOn": component_refs }]
    }))
    .map_err(|error| error.to_string())?;
    files.insert(
        "runtime/distribution/core-sbom.cdx.json".to_string(),
        ArchiveFile {
            bytes: core_sbom,
            mode: 0o644,
        },
    );

    let inventory = CorePackManifest {
        schema_version: 1,
        kind: "pre_commit_review_core_pack".to_string(),
        core_version: options.pack_version.to_string(),
        platform_id: options.platform_id.to_string(),
        target_triple: target_triple.to_string(),
        distribution_manifest_sha256: sha256_bytes(&distribution_bytes),
        revocation_index_sha256: sha256_bytes(&revocation_bytes),
        members: files
            .iter()
            .map(|(path, file)| CorePackFileBinding {
                path: path.clone(),
                mode: file.mode,
                size: file.bytes.len() as u64,
                sha256: sha256_bytes(&file.bytes),
            })
            .collect(),
    };
    inventory.validate().map_err(|error| error.to_string())?;
    let inventory_bytes = canonical_json(&inventory).map_err(|error| error.to_string())?;
    let inventory_sha256 = sha256_bytes(&inventory_bytes);
    files.insert(
        "runtime/distribution/core-pack-manifest.json".to_string(),
        ArchiveFile {
            bytes: inventory_bytes,
            mode: 0o644,
        },
    );
    let pack = normalized_archive(&files)?;
    let record = json!({
        "kind": "core",
        "core_version": options.pack_version,
        "platform_id": options.platform_id,
        "target_triple": target_triple,
        "project_asset_name": format!("pre-commit-review-core-{}-{}.tar.gz", options.pack_version, options.platform_id),
        "core_manifest_sha256": inventory_sha256,
        "pack_sha256": sha256_bytes(&pack),
        "pack_size": pack.len(),
        "members": inventory.members
    });
    write_atomic(options.output_path, &pack)?;
    if let Some(path) = options.record_output {
        write_atomic(
            path,
            &canonical_json(&record).map_err(|error| error.to_string())?,
        )?;
    }
    Ok(record)
}

fn binding(path: &str, bytes: &[u8]) -> ArtifactFileBinding {
    ArtifactFileBinding {
        path: path.to_string(),
        size: bytes.len() as u64,
        sha256: sha256_bytes(bytes),
    }
}

pub(crate) fn read_canonical<T: DeserializeOwned + Serialize>(
    path: &Path,
) -> WriterResult<(T, Vec<u8>)> {
    let bytes = read_regular(path)?;
    let value: T = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid JSON input {}: {error}", path.display()))?;
    let canonical = canonical_json(&value).map_err(|error| error.to_string())?;
    if canonical != bytes {
        return Err(format!("non-canonical JSON input: {}", path.display()));
    }
    Ok((value, bytes))
}

pub(crate) fn read_regular(path: &Path) -> WriterResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("missing pack input {}: {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "pack input is not a regular file: {}",
            path.display()
        ));
    }
    fs::read(path).map_err(|error| format!("could not read pack input {}: {error}", path.display()))
}

fn add_source_file(
    files: &mut BTreeMap<String, ArchiveFile>,
    archive_path: &str,
    source: &Path,
) -> WriterResult<()> {
    let mode = source_mode(source, archive_path)?;
    files.insert(
        archive_path.to_string(),
        ArchiveFile {
            bytes: read_regular(source)?,
            mode,
        },
    );
    Ok(())
}

fn add_tree(
    files: &mut BTreeMap<String, ArchiveFile>,
    source_root: &Path,
    archive_root: &str,
    exclude_bin: bool,
) -> WriterResult<()> {
    let metadata = fs::symlink_metadata(source_root)
        .map_err(|error| format!("missing pack input {}: {error}", source_root.display()))?;
    if !metadata.is_dir() {
        return Err(format!(
            "pack input is not a directory: {}",
            source_root.display()
        ));
    }
    let mut pending = vec![PathBuf::new()];
    while let Some(relative_dir) = pending.pop() {
        let directory = source_root.join(&relative_dir);
        let mut entries: Vec<_> = fs::read_dir(&directory)
            .map_err(|error| format!("could not read pack input {}: {error}", directory.display()))?
            .collect::<Result<_, _>>()
            .map_err(|error| {
                format!("could not read pack input {}: {error}", directory.display())
            })?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries.into_iter().rev() {
            let relative = relative_dir.join(entry.file_name());
            if exclude_bin
                && relative
                    .components()
                    .next()
                    .is_some_and(|part| part.as_os_str() == "bin")
            {
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                format!(
                    "could not inspect pack input {}: {error}",
                    entry.path().display()
                )
            })?;
            if metadata.file_type().is_symlink() {
                return Err(format!(
                    "pack input contains a symlink: {}",
                    entry.path().display()
                ));
            }
            if metadata.is_dir() {
                pending.push(relative);
            } else if metadata.is_file() {
                let relative_text = relative
                    .to_str()
                    .ok_or_else(|| {
                        format!("pack input path is not UTF-8: {}", entry.path().display())
                    })?
                    .replace('\\', "/");
                add_source_file(
                    files,
                    &format!("{archive_root}/{relative_text}"),
                    &entry.path(),
                )?;
            } else {
                return Err(format!(
                    "pack input is not regular: {}",
                    entry.path().display()
                ));
            }
        }
    }
    Ok(())
}

fn source_mode(source: &Path, archive_path: &str) -> WriterResult<u32> {
    if archive_path.starts_with("bin/")
        || archive_path.starts_with("scripts/bin/")
        || archive_path == "install.sh"
        || (archive_path.starts_with("scripts/") && archive_path.ends_with(".sh"))
    {
        return Ok(0o755);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::symlink_metadata(source)
            .map_err(|error| format!("could not inspect pack input {}: {error}", source.display()))?
            .permissions()
            .mode();
        if mode & 0o111 != 0 {
            return Ok(0o755);
        }
    }
    Ok(0o644)
}

pub(crate) fn normalized_archive(files: &BTreeMap<String, ArchiveFile>) -> WriterResult<Vec<u8>> {
    let mut directories = BTreeSet::new();
    for path in files.keys() {
        let mut prefix = String::new();
        for part in path
            .split('/')
            .take(path.split('/').count().saturating_sub(1))
        {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            directories.insert(format!("{prefix}/"));
        }
    }
    let mut entries: BTreeMap<String, Option<&ArchiveFile>> =
        directories.into_iter().map(|path| (path, None)).collect();
    entries.extend(files.iter().map(|(path, file)| (path.clone(), Some(file))));

    let mut tar_bytes = Vec::new();
    for (path, file) in entries {
        append_ustar_entry(&mut tar_bytes, &path, file)?;
    }
    tar_bytes.resize(tar_bytes.len() + 1024, 0);
    let mut encoder: GzEncoder<Vec<u8>> = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(Vec::new(), Compression::best());
    encoder
        .write_all(&tar_bytes)
        .map_err(|error| format!("could not compress archive: {error}"))?;
    encoder
        .finish()
        .map_err(|error| format!("could not finish compressed archive: {error}"))
}

fn append_ustar_entry(
    output: &mut Vec<u8>,
    path: &str,
    file: Option<&ArchiveFile>,
) -> WriterResult<()> {
    let mut header = [0_u8; 512];
    let (prefix, name) = split_ustar_path(path)?;
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    let (mode, size, entry_type) = match file {
        Some(file) => (file.mode, file.bytes.len() as u64, b'0'),
        None => (0o755, 0, b'5'),
    };
    write_octal(&mut header[100..108], u64::from(mode))?;
    write_octal(&mut header[108..116], 0)?;
    write_octal(&mut header[116..124], 0)?;
    write_octal(&mut header[124..136], size)?;
    write_octal(&mut header[136..148], 0)?;
    header[148..156].fill(b' ');
    header[156] = entry_type;
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
    let checksum = format!("{checksum:06o}\0 ");
    if checksum.len() != 8 {
        return Err("archive checksum exceeds the ustar field".to_string());
    }
    header[148..156].copy_from_slice(checksum.as_bytes());
    output.extend_from_slice(&header);
    if let Some(file) = file {
        output.extend_from_slice(&file.bytes);
        let padding = (512 - file.bytes.len() % 512) % 512;
        output.resize(output.len() + padding, 0);
    }
    Ok(())
}

fn split_ustar_path(path: &str) -> WriterResult<(&str, &str)> {
    if path.len() <= 100 {
        return Ok(("", path));
    }
    path.match_indices('/')
        .filter_map(|(index, _)| {
            let prefix = &path[..index];
            let name = &path[index + 1..];
            (prefix.len() <= 155 && !name.is_empty() && name.len() <= 100).then_some((prefix, name))
        })
        .next_back()
        .ok_or_else(|| format!("archive path does not fit POSIX ustar: {path}"))
}

fn write_octal(field: &mut [u8], value: u64) -> WriterResult<()> {
    let digits = field.len() - 1;
    let encoded = format!("{value:0digits$o}");
    if encoded.len() != digits {
        return Err("archive numeric value exceeds its ustar field".to_string());
    }
    field[..digits].copy_from_slice(encoded.as_bytes());
    field[digits] = 0;
    Ok(())
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> WriterResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("output has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "could not create output directory {}: {error}",
            parent.display()
        )
    })?;
    let mut temporary = NamedTempFile::new_in(parent)
        .map_err(|error| format!("could not create temporary output: {error}"))?;
    temporary
        .write_all(bytes)
        .map_err(|error| format!("could not write temporary output: {error}"))?;
    temporary
        .flush()
        .map_err(|error| format!("could not flush temporary output: {error}"))?;
    temporary.persist(path).map_err(|error| {
        format!(
            "could not publish output {}: {}",
            path.display(),
            error.error
        )
    })?;
    Ok(())
}

fn target_triple(platform: &str) -> WriterResult<&'static str> {
    match platform {
        "darwin-amd64" => Ok("x86_64-apple-darwin"),
        "darwin-arm64" => Ok("aarch64-apple-darwin"),
        "linux-amd64" => Ok("x86_64-unknown-linux-musl"),
        "windows-amd64" => Ok("x86_64-pc-windows-msvc"),
        _ => Err(format!("unsupported platform: {platform}")),
    }
}
