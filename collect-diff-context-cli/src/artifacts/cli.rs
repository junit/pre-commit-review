use super::{
    cache::{
        installed_executable_path, open_cache, provision_from_cache, publish_cache,
        read_target_receipt, verify_target_receipt, ArtifactCacheBoundaries, ArtifactCacheLayout,
    },
    contract::{
        canonical_json, sha256_bytes, ArtifactError, ArtifactManifest, ArtifactOperation,
        ArtifactPackRecord, ArtifactReport, ArtifactReportEntry, ArtifactReportStatus,
        ArtifactRole, ArtifactState, CorePackFileBinding, CorePackManifest, RevocationIndex,
        MAX_MANIFEST_BYTES, MAX_REVOCATION_BYTES,
    },
    pack::{verify_pack, VerifiedPack, VerifyLimits},
    probes::{run_installed_probes, run_probes},
    transport::Transport,
};
use crate::{
    impact_context::cache::file_facts::open_regular_file_no_follow,
    repository_context_provider::{
        cli::{
            validate_provider_installation, CliError as ProviderCliError,
            ValidatedProviderInstallation, MAX_PROFILE_BYTES, MAX_REGISTRY_BYTES,
        },
        cli_contract::ProviderRegistry,
        contract::AuthorizedProviderProfile,
    },
};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
};

const MAX_RECEIPTS: usize = 256;
const COPY_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug)]
enum ArtifactCommand {
    Verify(Selection),
    Provision {
        selection: Selection,
        target_root: PathBuf,
        cache_only: bool,
    },
    Doctor {
        target_root: PathBuf,
        artifact_id: Option<String>,
    },
}

#[derive(Debug)]
struct Selection {
    manifest_path: PathBuf,
    artifact_id: String,
    platform_id: String,
    pack_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
struct CliError {
    code: &'static str,
}

#[derive(Debug, Clone, Copy)]
struct Progress {
    enabled: bool,
}

struct PreparedArtifact {
    manifest: ArtifactManifest,
    record: ArtifactPackRecord,
    verified: VerifiedPack,
    probes: Vec<super::contract::ProbeResult>,
}

pub fn main_entry(arguments: &[OsString]) -> i32 {
    let operation = operation_hint(arguments.first());
    let command = match parse(arguments) {
        Ok(command) => command,
        Err(error) => return emit_failure(operation, error.code, 2),
    };
    let operation = command.operation();
    let progress = match Progress::for_command(&command) {
        Ok(progress) => progress,
        Err(error) => return emit_failure(operation, error.code, 2),
    };
    match execute(command, progress) {
        Ok(report) => emit(report, 0),
        Err(error) => emit_failure(operation, error.code, 1),
    }
}

impl ArtifactCommand {
    fn operation(&self) -> ArtifactOperation {
        match self {
            Self::Verify(_) => ArtifactOperation::Verify,
            Self::Provision { .. } => ArtifactOperation::Provision,
            Self::Doctor { .. } => ArtifactOperation::Doctor,
        }
    }
}

impl Progress {
    fn for_command(command: &ArtifactCommand) -> Result<Self, CliError> {
        if matches!(command, ArtifactCommand::Doctor { .. }) {
            return Ok(Self { enabled: false });
        }
        let value = std::env::var_os("PRE_COMMIT_REVIEW_FETCH_PROGRESS")
            .unwrap_or_else(|| OsString::from("auto"));
        match value.to_str() {
            Some("auto") => Ok(Self {
                enabled: io::stderr().is_terminal(),
            }),
            Some("always") => Ok(Self { enabled: true }),
            Some("never") => Ok(Self { enabled: false }),
            _ => Err(CliError {
                code: "progress-mode-invalid",
            }),
        }
    }

    fn fetching(self) {
        if self.enabled {
            eprintln!("collect-diff-context: fetching verified artifact");
        }
    }

    fn fetched(self) {
        if self.enabled {
            eprintln!("collect-diff-context: artifact bytes verified");
        }
    }
}

fn parse(arguments: &[OsString]) -> Result<ArtifactCommand, CliError> {
    let operation = arguments
        .first()
        .and_then(|argument| argument.to_str())
        .ok_or(CliError {
            code: "artifact-operation-invalid",
        })?;
    if !matches!(operation, "verify" | "provision" | "doctor") {
        return Err(CliError {
            code: "artifact-operation-invalid",
        });
    }

    let mut manifest_path = None;
    let mut artifact_id = None;
    let mut platform_id = None;
    let mut pack_path = None;
    let mut target_root = None;
    let mut cache_only = false;
    let mut index = 1;
    while index < arguments.len() {
        let flag = arguments[index].to_str().ok_or(CliError {
            code: "argument-unknown",
        })?;
        if flag == "--no-download" {
            if cache_only {
                return Err(CliError {
                    code: "argument-duplicate",
                });
            }
            cache_only = true;
            index += 1;
            continue;
        }
        if !matches!(
            flag,
            "--manifest" | "--artifact-id" | "--platform-id" | "--pack" | "--target-root"
        ) {
            return Err(CliError {
                code: "argument-unknown",
            });
        }
        let value = arguments.get(index + 1).ok_or(CliError {
            code: "argument-value-missing",
        })?;
        match flag {
            "--manifest" => set_once(&mut manifest_path, PathBuf::from(value))?,
            "--artifact-id" => set_once(&mut artifact_id, text_value(value)?)?,
            "--platform-id" => set_once(&mut platform_id, text_value(value)?)?,
            "--pack" => set_once(&mut pack_path, PathBuf::from(value))?,
            "--target-root" => set_once(&mut target_root, PathBuf::from(value))?,
            _ => unreachable!("artifact flags were exhaustively matched"),
        }
        index += 2;
    }

    match operation {
        "verify" => {
            if cache_only {
                return Err(CliError {
                    code: "argument-unsupported",
                });
            }
            reject_present(&target_root)?;
            Ok(ArtifactCommand::Verify(selection(
                manifest_path,
                artifact_id,
                platform_id,
                pack_path,
            )?))
        }
        "provision" => {
            if cache_only && pack_path.is_some() {
                return Err(CliError {
                    code: "argument-unsupported",
                });
            }
            Ok(ArtifactCommand::Provision {
                selection: selection(manifest_path, artifact_id, platform_id, pack_path)?,
                target_root: required_absolute(
                    target_root,
                    "argument-required",
                    "target-root-not-absolute",
                )?,
                cache_only,
            })
        }
        "doctor" => {
            if cache_only {
                return Err(CliError {
                    code: "argument-unsupported",
                });
            }
            reject_present(&manifest_path)?;
            reject_present(&platform_id)?;
            reject_present(&pack_path)?;
            if let Some(value) = artifact_id.as_deref() {
                validate_identifier(value)?;
            }
            Ok(ArtifactCommand::Doctor {
                target_root: required_absolute(
                    target_root,
                    "argument-required",
                    "target-root-not-absolute",
                )?,
                artifact_id,
            })
        }
        _ => unreachable!("artifact operations were exhaustively matched"),
    }
}

fn selection(
    manifest_path: Option<PathBuf>,
    artifact_id: Option<String>,
    platform_id: Option<String>,
    pack_path: Option<PathBuf>,
) -> Result<Selection, CliError> {
    let artifact_id = artifact_id.ok_or(CliError {
        code: "argument-required",
    })?;
    let platform_id = platform_id.ok_or(CliError {
        code: "argument-required",
    })?;
    validate_identifier(&artifact_id)?;
    validate_identifier(&platform_id)?;
    let manifest_path = required_absolute(
        manifest_path,
        "argument-required",
        "manifest-path-not-absolute",
    )?;
    if pack_path.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err(CliError {
            code: "pack-path-not-absolute",
        });
    }
    Ok(Selection {
        manifest_path,
        artifact_id,
        platform_id,
        pack_path,
    })
}

fn set_once<T>(slot: &mut Option<T>, value: T) -> Result<(), CliError> {
    if slot.replace(value).is_some() {
        return Err(CliError {
            code: "argument-duplicate",
        });
    }
    Ok(())
}

fn reject_present<T>(value: &Option<T>) -> Result<(), CliError> {
    if value.is_some() {
        return Err(CliError {
            code: "argument-unknown",
        });
    }
    Ok(())
}

fn required_absolute(
    value: Option<PathBuf>,
    missing_code: &'static str,
    relative_code: &'static str,
) -> Result<PathBuf, CliError> {
    let value = value.ok_or(CliError { code: missing_code })?;
    if !value.is_absolute() {
        return Err(CliError {
            code: relative_code,
        });
    }
    Ok(value)
}

fn text_value(value: &OsStr) -> Result<String, CliError> {
    value.to_str().map(str::to_string).ok_or(CliError {
        code: "argument-value-invalid",
    })
}

fn validate_identifier(value: &str) -> Result<(), CliError> {
    if value.is_empty()
        || value.len() > 64
        || !value.as_bytes()[0].is_ascii_lowercase()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(CliError {
            code: "argument-value-invalid",
        });
    }
    Ok(())
}

fn operation_hint(argument: Option<&OsString>) -> ArtifactOperation {
    match argument.and_then(|argument| argument.to_str()) {
        Some("provision") => ArtifactOperation::Provision,
        Some("doctor") => ArtifactOperation::Doctor,
        _ => ArtifactOperation::Verify,
    }
}

fn execute(command: ArtifactCommand, progress: Progress) -> Result<ArtifactReport, ArtifactError> {
    match command {
        ArtifactCommand::Verify(selection) => {
            let prepared = prepare(selection, progress)?;
            Ok(report_from_record(
                ArtifactOperation::Verify,
                &prepared.record,
            ))
        }
        ArtifactCommand::Provision {
            selection,
            target_root,
            cache_only,
        } => {
            let boundaries = ArtifactCacheBoundaries {
                target_root: Some(target_root.clone()),
                ..ArtifactCacheBoundaries::default()
            };
            let layout = ArtifactCacheLayout::resolve(None, &boundaries)?;
            if cache_only {
                let (manifest, _) = read_strict_json::<ArtifactManifest>(
                    &selection.manifest_path,
                    MAX_MANIFEST_BYTES,
                    "manifest-json",
                    "manifest-canonical",
                )?;
                manifest.validate()?;
                let record = manifest
                    .select_active(&selection.artifact_id, &selection.platform_id)?
                    .clone();
                let cached = open_cache(&layout, &record)?;
                provision_from_cache(&cached, &target_root, &manifest)?;
                return Ok(report_from_record(ArtifactOperation::Provision, &record));
            }
            let prepared = prepare(selection, progress)?;
            let publication = publish_cache(
                &layout,
                &prepared.verified,
                &prepared.record,
                &prepared.probes,
            )?;
            provision_from_cache(publication.entry(), &target_root, &prepared.manifest)?;
            Ok(report_from_record(
                ArtifactOperation::Provision,
                &prepared.record,
            ))
        }
        ArtifactCommand::Doctor {
            target_root,
            artifact_id,
        } => doctor(&target_root, artifact_id.as_deref()),
    }
}

fn prepare(selection: Selection, progress: Progress) -> Result<PreparedArtifact, ArtifactError> {
    let (manifest, _) = read_strict_json::<ArtifactManifest>(
        &selection.manifest_path,
        MAX_MANIFEST_BYTES,
        "manifest-json",
        "manifest-canonical",
    )?;
    manifest.validate()?;
    let record = manifest
        .select_active(&selection.artifact_id, &selection.platform_id)?
        .clone();
    let transport = match selection.pack_path {
        Some(path) => Transport::local(&path, &record.pack_sha256)?,
        None => Transport::project_asset(&record)?,
    };
    progress.fetching();
    let fetched = transport.fetch(&record)?;
    progress.fetched();
    let verified = verify_pack(fetched.open()?, &record, &VerifyLimits::default())?;
    let probes = run_probes(&verified, &record)?;
    Ok(PreparedArtifact {
        manifest,
        record,
        verified,
        probes,
    })
}

fn doctor(
    target_root: &Path,
    requested_artifact: Option<&str>,
) -> Result<ArtifactReport, ArtifactError> {
    let target_root = fs::canonicalize(target_root).map_err(|_| {
        error(
            "target-root-unavailable",
            "artifact doctor could not open the target root",
        )
    })?;
    let distribution = target_root.join("runtime/distribution");
    let (manifest, manifest_bytes) = read_strict_json::<ArtifactManifest>(
        &distribution.join("manifest.json"),
        MAX_MANIFEST_BYTES,
        "manifest-json",
        "manifest-canonical",
    )?;
    manifest.validate()?;
    let (core, _) = read_strict_json::<CorePackManifest>(
        &distribution.join("core-pack-manifest.json"),
        MAX_MANIFEST_BYTES,
        "core-pack-json",
        "core-pack-canonical",
    )?;
    core.validate()?;
    let (revocations, revocation_bytes) = read_strict_json::<RevocationIndex>(
        &distribution.join("revocations.json"),
        MAX_REVOCATION_BYTES,
        "revocation-index-json",
        "revocation-index-canonical",
    )?;
    revocations.validate()?;

    let manifest_sha256 = sha256_bytes(&manifest_bytes);
    let revocation_sha256 = sha256_bytes(&revocation_bytes);
    if core.distribution_manifest_sha256 != manifest_sha256 {
        return Err(error(
            "core-manifest-binding",
            "core inventory does not bind the target distribution manifest",
        ));
    }
    if manifest.revocation_index_sha256 != revocation_sha256
        || core.revocation_index_sha256 != revocation_sha256
    {
        return Err(error(
            "revocation-index-binding",
            "target revocation index does not match its reviewed bindings",
        ));
    }
    for binding in &core.members {
        verify_binding(&target_root, binding)?;
    }
    let provider_installations = load_provider_registry(&target_root)?;

    let artifact_ids = receipt_artifact_ids(&target_root, requested_artifact)?;
    if requested_artifact.is_some() {
        let record = doctor_artifact(
            &target_root,
            &manifest,
            &core,
            &revocations,
            provider_installations.as_deref(),
            &artifact_ids[0],
        )?;
        return Ok(report_from_record(ArtifactOperation::Doctor, record));
    }
    let mut artifacts = Vec::with_capacity(artifact_ids.len());
    for artifact_id in artifact_ids {
        let record = doctor_artifact(
            &target_root,
            &manifest,
            &core,
            &revocations,
            provider_installations.as_deref(),
            &artifact_id,
        )?;
        artifacts.push(ArtifactReportEntry::from_record(record));
    }
    Ok(aggregate_doctor_report(artifacts))
}

fn doctor_artifact<'a>(
    target_root: &Path,
    manifest: &'a ArtifactManifest,
    core: &CorePackManifest,
    revocations: &RevocationIndex,
    provider_installations: Option<&[ValidatedProviderInstallation]>,
    artifact_id: &str,
) -> Result<&'a ArtifactPackRecord, ArtifactError> {
    let observed_receipt = read_target_receipt(target_root, artifact_id)?;
    if revocations
        .entries
        .iter()
        .any(|entry| entry.pack_sha256 == observed_receipt.pack_sha256)
    {
        return Err(error(
            "artifact-revoked",
            "installed artifact digest is present in the revocation index",
        ));
    }
    let record = manifest
        .packs
        .iter()
        .find(|record| {
            record.artifact_id == observed_receipt.artifact_id
                && record.platform_id == observed_receipt.platform_id
                && record.pack_version == observed_receipt.pack_version
                && record.pack_sha256 == observed_receipt.pack_sha256
        })
        .ok_or_else(|| {
            error(
                "target-receipt-record-missing",
                "installed artifact receipt has no exact manifest record",
            )
        })?;
    if record.state == ArtifactState::Revoked {
        return Err(error(
            "artifact-revoked",
            "installed artifact record is revoked",
        ));
    }
    if record.artifact_role == ArtifactRole::RepositoryContextProvider {
        verify_provider_receipt_binding(target_root, record, provider_installations)?;
    }
    let receipt = verify_target_receipt(target_root, artifact_id, manifest)?;
    if receipt.platform_id != core.platform_id || record.target_triple != core.target_triple {
        return Err(error(
            "target-platform-mismatch",
            "installed artifact platform does not match the target core inventory",
        ));
    }
    let executable = installed_executable_path(target_root, record)?;
    let live_probes = run_installed_probes(&executable, record)?;
    if live_probes != receipt.probes {
        return Err(error(
            "target-probe-binding",
            "live artifact probes do not match the target receipt",
        ));
    }
    Ok(record)
}

fn receipt_artifact_ids(
    target_root: &Path,
    requested_artifact: Option<&str>,
) -> Result<Vec<String>, ArtifactError> {
    if let Some(artifact_id) = requested_artifact {
        return Ok(vec![artifact_id.to_string()]);
    }
    let root = target_root.join("runtime/artifact-receipts");
    let entries = fs::read_dir(root).map_err(|_| {
        error(
            "artifact-file-open",
            "artifact target receipts could not be opened",
        )
    })?;
    let mut artifacts = Vec::new();
    for entry in entries.take(MAX_RECEIPTS + 1) {
        let entry = entry.map_err(|_| {
            error(
                "artifact-file-open",
                "artifact target receipts could not be read",
            )
        })?;
        let file_type = entry.file_type().map_err(|_| {
            error(
                "artifact-file-open",
                "artifact target receipt type could not be read",
            )
        })?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            error(
                "target-receipt-inventory",
                "artifact target receipt name is invalid",
            )
        })?;
        let artifact_id = name.strip_suffix(".json").ok_or_else(|| {
            error(
                "target-receipt-inventory",
                "artifact target receipt inventory is invalid",
            )
        })?;
        if !file_type.is_file() {
            return Err(error(
                "target-receipt-inventory",
                "artifact target receipt inventory is invalid",
            ));
        }
        validate_identifier(artifact_id).map_err(|_| {
            error(
                "target-receipt-inventory",
                "artifact target receipt inventory is invalid",
            )
        })?;
        artifacts.push(artifact_id.to_string());
    }
    artifacts.sort();
    artifacts.dedup();
    if artifacts.len() > MAX_RECEIPTS {
        return Err(error(
            "target-receipt-limit",
            "artifact target receipt inventory exceeds its limit",
        ));
    }
    if artifacts.is_empty() {
        return Err(error(
            "artifact-file-open",
            "artifact target receipt is missing",
        ));
    }
    Ok(artifacts)
}

fn load_provider_registry(
    target_root: &Path,
) -> Result<Option<Vec<ValidatedProviderInstallation>>, ArtifactError> {
    let registry_path = target_root.join("runtime/providers/provider-registry.json");
    match fs::symlink_metadata(&registry_path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(error(
                "provider-registry-open",
                "target provider registry could not be inspected",
            ))
        }
        Ok(_) => {}
    }
    let (registry, _) = read_strict_json::<ProviderRegistry>(
        &registry_path,
        MAX_REGISTRY_BYTES,
        "provider-registry-json",
        "provider-registry-canonical",
    )?;
    registry.validate().map_err(|_| {
        error(
            "provider-registry-invalid",
            "target provider registry is invalid",
        )
    })?;
    let mut installations = Vec::with_capacity(registry.entries.len());
    for entry in &registry.entries {
        let profile_path = canonical_target_path(target_root, &entry.profile_path)?;
        canonical_target_path(target_root, &entry.executable_path)?;
        let (profile, profile_bytes) = read_strict_json::<AuthorizedProviderProfile>(
            &profile_path,
            MAX_PROFILE_BYTES,
            "provider-profile-json",
            "provider-profile-canonical",
        )?;
        installations.push(
            validate_provider_installation(entry, profile, &sha256_bytes(&profile_bytes))
                .map_err(map_provider_validation_error)?,
        );
    }
    Ok(Some(installations))
}

fn canonical_target_path(target_root: &Path, path: &Path) -> Result<PathBuf, ArtifactError> {
    let canonical = fs::canonicalize(path).map_err(|_| {
        error(
            "provider-path-stale",
            "target provider registry contains a stale absolute path",
        )
    })?;
    if !canonical.starts_with(target_root) {
        return Err(error(
            "provider-path-stale",
            "target provider registry contains a stale absolute path",
        ));
    }
    Ok(canonical)
}

fn map_provider_validation_error(source: ProviderCliError) -> ArtifactError {
    match source.code {
        "provider-cli-profile-invalid" => error(
            "provider-profile-invalid",
            "target provider profile is invalid",
        ),
        "provider-cli-binding-invalid" => error(
            "provider-registry-binding",
            "target provider registry does not bind its profile",
        ),
        _ => error(
            "provider-executable-invalid",
            "target provider executable is invalid",
        ),
    }
}

fn verify_provider_receipt_binding(
    target_root: &Path,
    record: &ArtifactPackRecord,
    installations: Option<&[ValidatedProviderInstallation]>,
) -> Result<(), ArtifactError> {
    let installations = installations.ok_or_else(|| {
        error(
            "provider-registry-required",
            "provider artifact receipt requires a target provider registry",
        )
    })?;
    let installed_executable = canonical_target_path(
        target_root,
        &installed_executable_path(target_root, record)?,
    )?;
    let matching: Vec<&ValidatedProviderInstallation> = installations
        .iter()
        .filter(|installation| installation.entry.executable_path == installed_executable)
        .collect();
    let installation = match matching.as_slice() {
        [] => {
            return Err(error(
                "provider-registry-entry-missing",
                "provider registry does not name the installed provider executable",
            ))
        }
        [installation] => *installation,
        _ => {
            return Err(error(
                "provider-registry-entry-ambiguous",
                "provider registry names the installed provider executable more than once",
            ))
        }
    };
    if installation.entry.provider_kind != "rust-analyzer"
        || installation.entry.provider_version != record.tool_version
        || installation.entry.target_triple != record.target_triple
        || installation.entry.executable_sha256 != record.executable.sha256
        || installation.profile.provider_version != record.tool_version
        || installation.profile.target_triple != record.target_triple
        || installation.profile.executable_sha256 != record.executable.sha256
    {
        return Err(error(
            "provider-receipt-binding",
            "provider registry and profile do not bind the installed artifact receipt",
        ));
    }
    Ok(())
}

fn verify_binding(root: &Path, binding: &CorePackFileBinding) -> Result<(), ArtifactError> {
    let path = root.join(&binding.path);
    let mut file = open_regular_file_no_follow(&path).map_err(|_| {
        error(
            "artifact-binding-open",
            "artifact-bound target file could not be opened safely",
        )
    })?;
    let metadata = file.metadata().map_err(|_| {
        error(
            "artifact-binding-open",
            "artifact-bound target file could not be inspected",
        )
    })?;
    if !metadata.is_file() || metadata.len() != binding.size {
        return Err(error(
            "artifact-binding-size",
            "artifact-bound target file size is inconsistent",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != binding.mode {
            return Err(error(
                "artifact-binding-mode",
                "artifact-bound target file mode is inconsistent",
            ));
        }
    }
    let digest = hash_reader(&mut file, binding.size)?;
    if digest != binding.sha256 {
        return Err(error(
            "artifact-binding-digest",
            "artifact-bound target file digest is inconsistent",
        ));
    }
    Ok(())
}

fn hash_reader(reader: &mut impl Read, expected_size: u64) -> Result<String, ArtifactError> {
    let mut digest = Sha256::new();
    let mut observed = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer).map_err(|_| {
            error(
                "artifact-binding-read",
                "artifact-bound target file could not be read",
            )
        })?;
        if read == 0 {
            break;
        }
        observed = observed.saturating_add(read as u64);
        if observed > expected_size {
            return Err(error(
                "artifact-binding-size",
                "artifact-bound target file exceeded its expected size",
            ));
        }
        digest.update(&buffer[..read]);
    }
    if observed != expected_size {
        return Err(error(
            "artifact-binding-size",
            "artifact-bound target file size is inconsistent",
        ));
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn read_strict_json<T>(
    path: &Path,
    maximum: usize,
    json_code: &'static str,
    canonical_code: &'static str,
) -> Result<(T, Vec<u8>), ArtifactError>
where
    T: DeserializeOwned + Serialize,
{
    let mut file = open_regular_file_no_follow(path).map_err(|_| {
        error(
            "artifact-file-open",
            "artifact contract file could not be opened safely",
        )
    })?;
    let metadata = file.metadata().map_err(|_| {
        error(
            "artifact-file-open",
            "artifact contract file could not be inspected",
        )
    })?;
    if !metadata.is_file() || metadata.len() > maximum as u64 {
        return Err(error(
            "artifact-file-size-limit",
            "artifact contract file exceeds its byte limit",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes).map_err(|_| {
        error(
            "artifact-file-read",
            "artifact contract file could not be read",
        )
    })?;
    let value: T = serde_json::from_slice(&bytes)
        .map_err(|_| ArtifactError::new(json_code, "artifact contract JSON is invalid"))?;
    if canonical_json(&value)? != bytes {
        return Err(ArtifactError::new(
            canonical_code,
            "artifact contract JSON is not canonical",
        ));
    }
    Ok((value, bytes))
}

fn report_from_record(operation: ArtifactOperation, record: &ArtifactPackRecord) -> ArtifactReport {
    ArtifactReport {
        schema_version: 1,
        kind: "third_party_artifact_report".to_string(),
        operation,
        status: ArtifactReportStatus::Completed,
        artifact_id: Some(record.artifact_id.clone()),
        platform_id: Some(record.platform_id.clone()),
        pack_version: Some(record.pack_version.clone()),
        pack_sha256: Some(record.pack_sha256.clone()),
        executable_sha256: Some(record.executable.sha256.clone()),
        sbom_sha256: Some(record.sbom_sha256.clone()),
        lifecycle_state: Some(record.state),
        artifacts: Vec::new(),
        code: None,
    }
}

fn aggregate_doctor_report(artifacts: Vec<ArtifactReportEntry>) -> ArtifactReport {
    ArtifactReport {
        schema_version: 1,
        kind: "third_party_artifact_report".to_string(),
        operation: ArtifactOperation::Doctor,
        status: ArtifactReportStatus::Completed,
        artifact_id: None,
        platform_id: None,
        pack_version: None,
        pack_sha256: None,
        executable_sha256: None,
        sbom_sha256: None,
        lifecycle_state: None,
        artifacts,
        code: None,
    }
}

fn failed_report(operation: ArtifactOperation, code: &'static str) -> ArtifactReport {
    ArtifactReport {
        schema_version: 1,
        kind: "third_party_artifact_report".to_string(),
        operation,
        status: ArtifactReportStatus::Failed,
        artifact_id: None,
        platform_id: None,
        pack_version: None,
        pack_sha256: None,
        executable_sha256: None,
        sbom_sha256: None,
        lifecycle_state: None,
        artifacts: Vec::new(),
        code: Some(code.to_string()),
    }
}

fn emit_failure(operation: ArtifactOperation, code: &'static str, exit_code: i32) -> i32 {
    emit(failed_report(operation, code), exit_code)
}

fn emit(report: ArtifactReport, exit_code: i32) -> i32 {
    if report.validate().is_err() {
        return 1;
    }
    let Ok(bytes) = canonical_json(&report) else {
        return 1;
    };
    if io::stdout().lock().write_all(&bytes).is_err() {
        return 1;
    }
    exit_code
}

fn error(code: &'static str, message: &'static str) -> ArtifactError {
    ArtifactError::new(code, message)
}
