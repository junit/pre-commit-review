use super::contract::{
    canonical_json, ArtifactError, ArtifactFileBinding, ArtifactPackRecord, PackFileRecord,
    PackFileRole, PackManifest, MAX_MANIFEST_BYTES,
};
use flate2::bufread::GzDecoder;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::Path,
};
use tempfile::{NamedTempFile, TempDir};

const HARD_MAX_ENTRIES: usize = 128;
const HARD_MAX_COMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
const HARD_MAX_EXPANDED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const HARD_MAX_PATH_BYTES: usize = 512;
const HARD_MAX_METADATA_BYTES: u64 = 16 * 1024;
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const TAR_BLOCK_BYTES: u64 = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyLimits {
    pub max_entries: usize,
    pub max_compressed_bytes: u64,
    pub max_expanded_bytes: u64,
    pub max_file_bytes: u64,
    pub max_path_bytes: usize,
    pub max_metadata_bytes: u64,
}

impl Default for VerifyLimits {
    fn default() -> Self {
        Self {
            max_entries: HARD_MAX_ENTRIES,
            max_compressed_bytes: HARD_MAX_COMPRESSED_BYTES,
            max_expanded_bytes: HARD_MAX_EXPANDED_BYTES,
            max_file_bytes: HARD_MAX_EXPANDED_BYTES,
            max_path_bytes: HARD_MAX_PATH_BYTES,
            max_metadata_bytes: HARD_MAX_METADATA_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub role: PackFileRole,
}

#[derive(Debug)]
pub struct VerifiedPack {
    staging: TempDir,
    pub pack_sha256: String,
    pub pack_size: u64,
    pub pack_manifest_sha256: String,
    pub manifest: PackManifest,
    pub files: BTreeMap<String, VerifiedFile>,
}

impl VerifiedPack {
    pub fn root(&self) -> &Path {
        self.staging.path()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectedKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InspectedEntry {
    path: String,
    size: u64,
    kind: InspectedKind,
    data_offset: u64,
}

pub fn verify_pack<R: Read>(
    reader: R,
    record: &ArtifactPackRecord,
    limits: &VerifyLimits,
) -> Result<VerifiedPack, ArtifactError> {
    record.validate()?;
    let (mut compressed, pack_size, pack_sha256) = copy_compressed(reader, record, limits)?;
    validate_gzip_header(compressed.as_file_mut())?;
    let mut expanded = decompress_pack(compressed.as_file_mut(), limits)?;
    let entries = inspect_ustar(expanded.as_file_mut(), limits)?;
    let manifest_bytes =
        read_inspected_file(expanded.as_file_mut(), &entries, "pack-manifest.json")?;
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(error(
            "pack-manifest-size-limit",
            "pack manifest exceeds its byte limit",
        ));
    }
    let pack_manifest_sha256 = digest_bytes(&manifest_bytes);
    if pack_manifest_sha256 != record.pack_manifest_sha256 {
        return Err(error(
            "pack-manifest-digest",
            "pack manifest digest does not match the selected record",
        ));
    }
    let manifest: PackManifest = serde_json::from_slice(&manifest_bytes).map_err(|_| {
        error(
            "pack-manifest-json",
            "pack manifest is not valid strict JSON",
        )
    })?;
    manifest.validate()?;
    if canonical_json(&manifest)? != manifest_bytes {
        return Err(error(
            "pack-manifest-canonical",
            "pack manifest bytes are not canonical",
        ));
    }
    validate_manifest_identity(&manifest, record)?;
    validate_inventory(&entries, &manifest)?;

    let (staging, files) = extract_inventory(expanded.as_file_mut(), &manifest)?;
    validate_record_bindings(&files, &manifest, record)?;
    validate_sbom(staging.path(), &manifest, record)?;

    Ok(VerifiedPack {
        staging,
        pack_sha256,
        pack_size,
        pack_manifest_sha256,
        manifest,
        files,
    })
}

fn copy_compressed<R: Read>(
    mut reader: R,
    record: &ArtifactPackRecord,
    limits: &VerifyLimits,
) -> Result<(NamedTempFile, u64, String), ArtifactError> {
    let effective_limit = limits
        .max_compressed_bytes
        .min(record.max_compressed_size)
        .min(HARD_MAX_COMPRESSED_BYTES);
    let mut temporary = NamedTempFile::new().map_err(|_| {
        error(
            "pack-temporary-file",
            "could not create a private pack file",
        )
    })?;
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|_| error("pack-read", "could not read pack bytes"))?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| error("pack-compressed-limit", "pack compressed size overflowed"))?;
        if total > effective_limit {
            return Err(error(
                "pack-compressed-limit",
                "pack exceeds its compressed byte limit",
            ));
        }
        digest.update(&buffer[..count]);
        temporary
            .write_all(&buffer[..count])
            .map_err(|_| error("pack-temporary-write", "could not stage pack bytes"))?;
    }
    if total != record.expected_compressed_size {
        return Err(error(
            "pack-size-mismatch",
            "pack size does not match the selected record",
        ));
    }
    let observed_sha256 = format!("{:x}", digest.finalize());
    if observed_sha256 != record.pack_sha256 {
        return Err(error(
            "pack-digest-mismatch",
            "pack digest does not match the selected record",
        ));
    }
    temporary
        .as_file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|_| error("pack-temporary-read", "could not reopen staged pack bytes"))?;
    Ok((temporary, total, observed_sha256))
}

fn validate_gzip_header(file: &mut File) -> Result<(), ArtifactError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| error("gzip-format", "could not read the gzip header"))?;
    let mut header = [0_u8; 10];
    file.read_exact(&mut header)
        .map_err(|_| error("gzip-format", "pack has an incomplete gzip header"))?;
    if header[..3] != [0x1f, 0x8b, 8] {
        return Err(error("gzip-format", "pack is not a gzip stream"));
    }
    if header[3] != 0 || header[4..8] != [0, 0, 0, 0] || header[8] != 2 || header[9] != 255 {
        return Err(error(
            "gzip-metadata",
            "pack gzip metadata is not canonical",
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| error("gzip-format", "could not rewind the gzip stream"))?;
    Ok(())
}

fn decompress_pack(
    compressed: &mut File,
    limits: &VerifyLimits,
) -> Result<NamedTempFile, ArtifactError> {
    compressed
        .seek(SeekFrom::Start(0))
        .map_err(|_| error("gzip-format", "could not rewind the gzip stream"))?;
    let cloned = compressed
        .try_clone()
        .map_err(|_| error("gzip-format", "could not open the gzip stream"))?;
    let buffered = BufReader::new(cloned);
    let mut decoder = GzDecoder::new(buffered);
    let mut expanded = NamedTempFile::new().map_err(|_| {
        error(
            "pack-temporary-file",
            "could not create an expanded pack file",
        )
    })?;
    let effective_limit = limits.max_expanded_bytes.min(HARD_MAX_EXPANDED_BYTES);
    let mut total = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let count = decoder
            .read(&mut buffer)
            .map_err(|_| error("gzip-format", "pack gzip payload is invalid"))?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| error("pack-expanded-limit", "pack expanded size overflowed"))?;
        if total > effective_limit {
            return Err(error(
                "pack-expanded-limit",
                "pack exceeds its expanded byte limit",
            ));
        }
        expanded.write_all(&buffer[..count]).map_err(|_| {
            error(
                "pack-temporary-write",
                "could not stage expanded pack bytes",
            )
        })?;
    }
    let mut remaining = decoder.into_inner();
    if !remaining
        .fill_buf()
        .map_err(|_| error("gzip-format", "could not finish the gzip stream"))?
        .is_empty()
    {
        return Err(error(
            "gzip-trailing-data",
            "pack contains trailing or concatenated gzip data",
        ));
    }
    expanded
        .as_file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|_| error("archive-read", "could not reopen the expanded pack"))?;
    Ok(expanded)
}

fn inspect_ustar(
    file: &mut File,
    limits: &VerifyLimits,
) -> Result<Vec<InspectedEntry>, ArtifactError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| error("archive-read", "could not read the pack archive"))?;
    let archive_size = file
        .metadata()
        .map_err(|_| error("archive-read", "could not inspect the pack archive"))?
        .len();
    let max_entries = limits.max_entries.min(HARD_MAX_ENTRIES);
    let max_file_bytes = limits
        .max_file_bytes
        .min(limits.max_expanded_bytes)
        .min(HARD_MAX_EXPANDED_BYTES);
    let max_path_bytes = limits.max_path_bytes.min(HARD_MAX_PATH_BYTES);
    let max_metadata_bytes = limits.max_metadata_bytes.min(HARD_MAX_METADATA_BYTES);
    let mut entries = Vec::new();
    let mut observed_paths = BTreeSet::new();
    let mut folded_paths = BTreeSet::new();
    let mut previous_path: Option<String> = None;
    let mut offset = 0_u64;

    loop {
        let mut header = [0_u8; TAR_BLOCK_BYTES as usize];
        if file.read_exact(&mut header).is_err() {
            return Err(error(
                "archive-end-blocks",
                "pack archive does not have canonical end blocks",
            ));
        }
        offset = offset
            .checked_add(TAR_BLOCK_BYTES)
            .ok_or_else(|| error("pack-expanded-limit", "pack archive offset overflowed"))?;
        if header.iter().all(|byte| *byte == 0) {
            let mut second = [0_u8; TAR_BLOCK_BYTES as usize];
            if file.read_exact(&mut second).is_err()
                || second.iter().any(|byte| *byte != 0)
                || offset + TAR_BLOCK_BYTES != archive_size
            {
                return Err(error(
                    "archive-end-blocks",
                    "pack archive does not have canonical end blocks",
                ));
            }
            break;
        }

        if entries.len() >= max_entries {
            return Err(error(
                "archive-entry-limit",
                "pack archive contains too many entries",
            ));
        }
        validate_header_checksum(&header)?;
        if &header[257..263] != b"ustar\0" || &header[263..265] != b"00" {
            return Err(error(
                "archive-header-format",
                "pack archive entry is not POSIX ustar",
            ));
        }
        let size = parse_octal(&header[124..136]).ok_or_else(|| {
            error(
                "archive-header-format",
                "pack archive contains an invalid size field",
            )
        })?;
        let entry_type = header[156];
        if matches!(entry_type, b'x' | b'g' | b'L' | b'K') && size > max_metadata_bytes {
            return Err(error(
                "archive-metadata-limit",
                "pack archive metadata exceeds its byte limit",
            ));
        }
        let kind = match entry_type {
            b'0' => InspectedKind::File,
            b'5' => InspectedKind::Directory,
            _ => {
                return Err(error(
                    "archive-entry-type",
                    "pack archive contains a forbidden entry type",
                ));
            }
        };
        let path = parse_ustar_path(&header, kind, max_path_bytes)?;
        validate_header_metadata(&header, &path, kind)?;
        if size > max_file_bytes {
            return Err(error(
                "archive-file-limit",
                "pack archive entry exceeds its file byte limit",
            ));
        }
        if kind == InspectedKind::Directory && size != 0 {
            return Err(error(
                "archive-header-metadata",
                "pack archive directory has nonzero content",
            ));
        }
        if previous_path
            .as_deref()
            .is_some_and(|previous| previous > path.as_str())
        {
            return Err(error(
                "archive-path-order",
                "pack archive entries are not path sorted",
            ));
        }
        if !observed_paths.insert(path.clone()) {
            return Err(error(
                "archive-duplicate-path",
                "pack archive contains a duplicate path",
            ));
        }
        if !folded_paths.insert(path.to_lowercase()) {
            return Err(error(
                "archive-case-collision",
                "pack archive contains a case-folded path collision",
            ));
        }
        previous_path = Some(path.clone());

        let data_offset = offset;
        let padded_size = size
            .checked_add(TAR_BLOCK_BYTES - 1)
            .and_then(|value| value.checked_div(TAR_BLOCK_BYTES))
            .and_then(|blocks| blocks.checked_mul(TAR_BLOCK_BYTES))
            .ok_or_else(|| error("pack-expanded-limit", "pack archive size overflowed"))?;
        let next_offset = offset
            .checked_add(padded_size)
            .ok_or_else(|| error("pack-expanded-limit", "pack archive offset overflowed"))?;
        if next_offset > archive_size {
            return Err(error(
                "archive-truncated",
                "pack archive entry is truncated",
            ));
        }
        if padded_size > size {
            file.seek(SeekFrom::Start(data_offset + size))
                .map_err(|_| error("archive-read", "could not inspect archive padding"))?;
            let mut padding = vec![0_u8; (padded_size - size) as usize];
            file.read_exact(&mut padding)
                .map_err(|_| error("archive-truncated", "pack archive padding is truncated"))?;
            if padding.iter().any(|byte| *byte != 0) {
                return Err(error(
                    "archive-header-metadata",
                    "pack archive padding is not canonical",
                ));
            }
        }
        file.seek(SeekFrom::Start(next_offset))
            .map_err(|_| error("archive-read", "could not advance through the archive"))?;
        offset = next_offset;
        entries.push(InspectedEntry {
            path,
            size,
            kind,
            data_offset,
        });
    }
    Ok(entries)
}

fn validate_header_checksum(header: &[u8; 512]) -> Result<(), ArtifactError> {
    if !header[148..154].iter().all(u8::is_ascii_digit) || header[154] != 0 || header[155] != b' ' {
        return Err(error(
            "archive-header-format",
            "pack archive checksum field is not canonical",
        ));
    }
    let expected = parse_octal(&header[148..155]).ok_or_else(|| {
        error(
            "archive-header-format",
            "pack archive checksum field is invalid",
        )
    })?;
    let observed: u64 = header
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if (148..156).contains(&index) {
                u64::from(b' ')
            } else {
                u64::from(*byte)
            }
        })
        .sum();
    if expected != observed {
        return Err(error(
            "archive-header-checksum",
            "pack archive header checksum is invalid",
        ));
    }
    Ok(())
}

fn validate_header_metadata(
    header: &[u8; 512],
    path: &str,
    kind: InspectedKind,
) -> Result<(), ArtifactError> {
    let mode = parse_octal(&header[100..108]);
    let uid = parse_octal(&header[108..116]);
    let gid = parse_octal(&header[116..124]);
    let mtime = parse_octal(&header[136..148]);
    let expected_mode = if kind == InspectedKind::Directory || path.starts_with("bin/") {
        0o755
    } else {
        0o644
    };
    if mode != Some(expected_mode)
        || uid != Some(0)
        || gid != Some(0)
        || mtime != Some(0)
        || header[157..257].iter().any(|byte| *byte != 0)
        || header[265..329].iter().any(|byte| *byte != 0)
        || !numeric_zero_or_empty(&header[329..337])
        || !numeric_zero_or_empty(&header[337..345])
        || header[500..512].iter().any(|byte| *byte != 0)
    {
        return Err(error(
            "archive-header-metadata",
            "pack archive header metadata is not canonical",
        ));
    }
    Ok(())
}

fn numeric_zero_or_empty(field: &[u8]) -> bool {
    field.iter().all(|byte| *byte == 0) || parse_octal(field) == Some(0)
}

fn parse_octal(field: &[u8]) -> Option<u64> {
    let terminator = field.iter().position(|byte| *byte == 0 || *byte == b' ')?;
    if field[terminator..]
        .iter()
        .any(|byte| *byte != 0 && *byte != b' ')
        || field[..terminator]
            .iter()
            .any(|byte| !(b'0'..=b'7').contains(byte))
        || terminator == 0
    {
        return None;
    }
    field[..terminator].iter().try_fold(0_u64, |value, byte| {
        value
            .checked_mul(8)
            .and_then(|value| value.checked_add(u64::from(*byte - b'0')))
    })
}

fn parse_ustar_path(
    header: &[u8; 512],
    kind: InspectedKind,
    max_path_bytes: usize,
) -> Result<String, ArtifactError> {
    let name = parse_nul_padded(&header[..100])?;
    let prefix = parse_nul_padded(&header[345..500])?;
    let mut bytes = Vec::with_capacity(prefix.len() + usize::from(!prefix.is_empty()) + name.len());
    if !prefix.is_empty() {
        bytes.extend_from_slice(prefix);
        bytes.push(b'/');
    }
    bytes.extend_from_slice(name);
    let path = std::str::from_utf8(&bytes)
        .map_err(|_| error("archive-path", "pack archive path is not valid UTF-8"))?;
    if path.is_empty() || path.len() > max_path_bytes {
        return Err(error(
            "archive-path",
            "pack archive path is outside its byte limit",
        ));
    }
    let normalized = if kind == InspectedKind::Directory {
        path.strip_suffix('/').unwrap_or(path)
    } else {
        if path.ends_with('/') {
            return Err(error(
                "archive-path",
                "pack archive file path is not canonical",
            ));
        }
        path
    };
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.contains('\\')
        || normalized.contains(':')
        || normalized
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || normalized.chars().any(char::is_control)
    {
        return Err(error(
            "archive-path",
            "pack archive contains an unsafe path",
        ));
    }
    if kind == InspectedKind::Directory {
        Ok(format!("{normalized}/"))
    } else {
        Ok(normalized.to_string())
    }
}

fn parse_nul_padded(field: &[u8]) -> Result<&[u8], ArtifactError> {
    match field.iter().position(|byte| *byte == 0) {
        Some(end) => {
            if field[end..].iter().any(|byte| *byte != 0) {
                return Err(error(
                    "archive-header-format",
                    "pack archive path field is not canonical",
                ));
            }
            Ok(&field[..end])
        }
        None => Ok(field),
    }
}

fn read_inspected_file(
    file: &mut File,
    entries: &[InspectedEntry],
    path: &str,
) -> Result<Vec<u8>, ArtifactError> {
    let entry = entries
        .iter()
        .find(|entry| entry.path == path && entry.kind == InspectedKind::File)
        .ok_or_else(|| {
            error(
                "pack-manifest-missing",
                "pack archive has no internal manifest",
            )
        })?;
    if entry.size > MAX_MANIFEST_BYTES as u64 {
        return Err(error(
            "pack-manifest-size-limit",
            "pack manifest exceeds its byte limit",
        ));
    }
    let size = usize::try_from(entry.size)
        .map_err(|_| error("archive-file-limit", "pack archive file is too large"))?;
    let mut bytes = vec![0_u8; size];
    file.seek(SeekFrom::Start(entry.data_offset))
        .and_then(|_| file.read_exact(&mut bytes))
        .map_err(|_| error("archive-truncated", "pack archive file is truncated"))?;
    Ok(bytes)
}

fn validate_manifest_identity(
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
    Ok(())
}

fn validate_inventory(
    entries: &[InspectedEntry],
    manifest: &PackManifest,
) -> Result<(), ArtifactError> {
    let expected_files: BTreeSet<&str> = std::iter::once("pack-manifest.json")
        .chain(manifest.files.iter().map(|file| file.path.as_str()))
        .collect();
    let observed_files: BTreeSet<&str> = entries
        .iter()
        .filter(|entry| entry.kind == InspectedKind::File)
        .map(|entry| entry.path.as_str())
        .collect();
    if observed_files.difference(&expected_files).next().is_some() {
        return Err(error(
            "archive-unexpected-file",
            "pack archive contains an unexpected file",
        ));
    }
    if expected_files.difference(&observed_files).next().is_some() {
        return Err(error(
            "archive-missing-file",
            "pack archive is missing an expected file",
        ));
    }
    if entries.iter().any(|entry| {
        entry.kind == InspectedKind::Directory
            && !matches!(entry.path.as_str(), "bin/" | "licenses/")
    }) {
        return Err(error(
            "archive-unexpected-file",
            "pack archive contains an unexpected directory",
        ));
    }
    Ok(())
}

fn extract_inventory(
    expanded: &mut File,
    manifest: &PackManifest,
) -> Result<(TempDir, BTreeMap<String, VerifiedFile>), ArtifactError> {
    expanded
        .seek(SeekFrom::Start(0))
        .map_err(|_| error("archive-read", "could not reopen the pack archive"))?;
    let cloned = expanded
        .try_clone()
        .map_err(|_| error("archive-read", "could not open the pack archive"))?;
    let staging = tempfile::Builder::new()
        .prefix("pre-commit-review-pack-")
        .tempdir()
        .map_err(|_| error("pack-temporary-directory", "could not create pack staging"))?;
    let expected: BTreeMap<&str, &PackFileRecord> = manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    let mut files = BTreeMap::new();
    let mut archive = tar::Archive::new(cloned);
    let archive_entries = archive
        .entries()
        .map_err(|_| error("archive-parse", "could not parse the pack archive"))?;
    for archive_entry in archive_entries {
        let mut archive_entry = archive_entry
            .map_err(|_| error("archive-parse", "could not parse a pack archive entry"))?;
        let path_bytes = archive_entry.path_bytes();
        let path = std::str::from_utf8(path_bytes.as_ref())
            .map_err(|_| error("archive-path", "pack archive path is not valid UTF-8"))?
            .to_string();
        if archive_entry.header().entry_type().is_dir() {
            continue;
        }
        let destination = staging.path().join(&path);
        let parent = destination
            .parent()
            .ok_or_else(|| error("archive-path", "pack archive file has no staging parent"))?;
        fs::create_dir_all(parent).map_err(|_| {
            error(
                "archive-extract",
                "could not create pack staging directories",
            )
        })?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|_| error("archive-extract", "could not create a staged pack file"))?;
        let mut digest = Sha256::new();
        let mut size = 0_u64;
        let mut buffer = [0_u8; COPY_BUFFER_BYTES];
        loop {
            let count = archive_entry
                .read(&mut buffer)
                .map_err(|_| error("archive-extract", "could not read a pack archive file"))?;
            if count == 0 {
                break;
            }
            size = size
                .checked_add(count as u64)
                .ok_or_else(|| error("archive-file-limit", "pack file size overflowed"))?;
            digest.update(&buffer[..count]);
            output
                .write_all(&buffer[..count])
                .map_err(|_| error("archive-extract", "could not write a staged pack file"))?;
        }
        let sha256 = format!("{:x}", digest.finalize());
        set_staged_permissions(&destination, path.starts_with("bin/"))?;
        if let Some(expected_file) = expected.get(path.as_str()) {
            if size != expected_file.size {
                return Err(error(
                    "pack-file-size",
                    "pack payload size does not match its internal manifest",
                ));
            }
            if sha256 != expected_file.sha256 {
                return Err(error(
                    "pack-file-digest",
                    "pack payload digest does not match its internal manifest",
                ));
            }
            files.insert(
                path.clone(),
                VerifiedFile {
                    path,
                    size,
                    sha256,
                    role: expected_file.role,
                },
            );
        }
    }
    if files.len() != expected.len() {
        return Err(error(
            "archive-missing-file",
            "not every pack payload file was extracted",
        ));
    }
    Ok((staging, files))
}

#[cfg(unix)]
fn set_staged_permissions(path: &Path, executable: bool) -> Result<(), ArtifactError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|_| error("archive-extract", "could not set staged pack permissions"))
}

#[cfg(not(unix))]
fn set_staged_permissions(_path: &Path, _executable: bool) -> Result<(), ArtifactError> {
    Ok(())
}

fn validate_record_bindings(
    files: &BTreeMap<String, VerifiedFile>,
    manifest: &PackManifest,
    record: &ArtifactPackRecord,
) -> Result<(), ArtifactError> {
    let executable = manifest
        .files
        .iter()
        .find(|file| file.role == PackFileRole::Executable)
        .ok_or_else(|| error("pack-executable-binding", "pack has no executable binding"))?;
    if !binding_matches(&record.executable, executable) {
        return Err(error(
            "pack-executable-binding",
            "pack executable does not match the selected record",
        ));
    }
    let licenses: Vec<&PackFileRecord> = manifest
        .files
        .iter()
        .filter(|file| file.role == PackFileRole::License)
        .collect();
    if licenses.len() != record.license_files.len()
        || record
            .license_files
            .iter()
            .zip(licenses)
            .any(|(binding, file)| !binding_matches(binding, file))
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
        .ok_or_else(|| error("pack-sbom-binding", "pack has no SBOM binding"))?;
    if sbom.sha256 != record.sbom_sha256
        || files
            .get(&sbom.path)
            .is_none_or(|file| file.sha256 != record.sbom_sha256)
    {
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

fn validate_sbom(
    staging_root: &Path,
    manifest: &PackManifest,
    record: &ArtifactPackRecord,
) -> Result<(), ArtifactError> {
    let bytes = fs::read(staging_root.join("sbom.cdx.json"))
        .map_err(|_| error("sbom-read", "could not read the pack SBOM"))?;
    let sbom: Value = serde_json::from_slice(&bytes)
        .map_err(|_| error("sbom-json", "pack SBOM is not valid JSON"))?;
    if serde_json::to_vec(&sbom)
        .map_err(|_| error("sbom-json", "pack SBOM could not be normalized"))?
        != bytes
    {
        return Err(error(
            "sbom-canonical",
            "pack SBOM bytes are not compact canonical JSON",
        ));
    }
    if sbom.get("bomFormat").and_then(Value::as_str) != Some("CycloneDX")
        || sbom.get("specVersion").and_then(Value::as_str) != Some("1.5")
        || sbom.get("version").and_then(Value::as_u64) != Some(1)
    {
        return Err(error("sbom-identity", "pack SBOM is not CycloneDX 1.5"));
    }
    let components = sbom
        .get("components")
        .and_then(Value::as_array)
        .ok_or_else(|| error("sbom-component", "pack SBOM has no component inventory"))?;
    if components.len() != 1 {
        return Err(error(
            "sbom-component",
            "pack SBOM must contain one external executable component",
        ));
    }
    let component = &components[0];
    if component.get("type").and_then(Value::as_str) != Some("application")
        || component.get("bom-ref").and_then(Value::as_str) != Some(record.sbom_component.as_str())
        || component.get("purl").and_then(Value::as_str) != Some(record.sbom_component.as_str())
        || component.get("name").and_then(Value::as_str) != Some(record.license_component.as_str())
        || component.get("version").and_then(Value::as_str) != Some(record.tool_version.as_str())
    {
        return Err(error(
            "sbom-component",
            "pack SBOM component does not match the selected record",
        ));
    }
    if !contains_hash(component.get("hashes"), &record.executable.sha256) {
        return Err(error(
            "sbom-executable-hash",
            "pack SBOM does not bind the executable digest",
        ));
    }
    let licenses = component
        .get("licenses")
        .and_then(Value::as_array)
        .ok_or_else(|| error("sbom-license", "pack SBOM has no license evidence"))?;
    if licenses.is_empty()
        || licenses.iter().any(|entry| {
            let license = entry.get("license");
            license
                .and_then(|value| value.get("id").or_else(|| value.get("name")))
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        })
    {
        return Err(error(
            "sbom-license",
            "pack SBOM license evidence is incomplete",
        ));
    }
    let source_url = format!(
        "https://github.com/{}/releases/download/{}/{}",
        record.upstream_repository, record.upstream_tag, manifest.upstream_asset_name
    );
    let references = component
        .get("externalReferences")
        .and_then(Value::as_array)
        .ok_or_else(|| error("sbom-source", "pack SBOM has no distribution source"))?;
    if !references.iter().any(|reference| {
        reference.get("type").and_then(Value::as_str) == Some("distribution")
            && reference.get("url").and_then(Value::as_str) == Some(source_url.as_str())
            && contains_hash(reference.get("hashes"), &manifest.upstream_asset_sha256)
    }) {
        return Err(error(
            "sbom-source",
            "pack SBOM distribution source does not match the internal manifest",
        ));
    }
    let properties = component
        .get("properties")
        .and_then(Value::as_array)
        .ok_or_else(|| error("sbom-evidence", "pack SBOM has no evidence properties"))?;
    let mut property_map = BTreeMap::new();
    for property in properties {
        let name = property.get("name").and_then(Value::as_str);
        let value = property.get("value").and_then(Value::as_str);
        if let (Some(name), Some(value)) = (name, value) {
            if property_map.insert(name, value).is_some() {
                return Err(error(
                    "sbom-evidence",
                    "pack SBOM contains duplicate evidence properties",
                ));
            }
        }
    }
    let expected_properties = [
        ("pre-commit-review:artifact-id", record.artifact_id.as_str()),
        (
            "pre-commit-review:pack-version",
            record.pack_version.as_str(),
        ),
        ("pre-commit-review:platform-id", record.platform_id.as_str()),
        ("pre-commit-review:evidence-scope", "component-evidence"),
        ("pre-commit-review:transitive-closure", "unknown"),
    ];
    if expected_properties
        .iter()
        .any(|(name, value)| property_map.get(name).copied() != Some(*value))
    {
        return Err(error(
            "sbom-evidence",
            "pack SBOM evidence scope is incomplete",
        ));
    }
    validate_sbom_relationship(&sbom, record)?;
    Ok(())
}

fn contains_hash(value: Option<&Value>, expected: &str) -> bool {
    value.and_then(Value::as_array).is_some_and(|hashes| {
        hashes.iter().any(|hash| {
            hash.get("alg").and_then(Value::as_str) == Some("SHA-256")
                && hash.get("content").and_then(Value::as_str) == Some(expected)
        })
    })
}

fn validate_sbom_relationship(
    sbom: &Value,
    record: &ArtifactPackRecord,
) -> Result<(), ArtifactError> {
    let pack_ref = format!(
        "urn:pre-commit-review:pack:{}:{}:{}",
        record.artifact_id, record.pack_version, record.platform_id
    );
    let metadata_ref = sbom
        .pointer("/metadata/component/bom-ref")
        .and_then(Value::as_str);
    let dependencies = sbom.get("dependencies").and_then(Value::as_array);
    let relationship = dependencies.is_some_and(|dependencies| {
        dependencies.iter().any(|dependency| {
            dependency.get("ref").and_then(Value::as_str) == Some(pack_ref.as_str())
                && dependency
                    .get("dependsOn")
                    .and_then(Value::as_array)
                    .is_some_and(|items| {
                        items
                            .iter()
                            .any(|item| item.as_str() == Some(record.sbom_component.as_str()))
                    })
        })
    });
    if metadata_ref != Some(pack_ref.as_str()) || !relationship {
        return Err(error(
            "sbom-relationship",
            "pack SBOM does not contain the pack relationship",
        ));
    }
    Ok(())
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn error(code: &'static str, message: &'static str) -> ArtifactError {
    ArtifactError::new(code, message)
}
