use super::contracts::{OrchestrationManifest, RepositoryConfiguration, StaticAnalysisProfile};
use super::executor::{prepare_profile, sha256_file, verify_prepared_integrity, PreparedProfile};
use crate::review_scope::ReviewSource;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_MANIFEST_BYTES: u64 = 1_000_000;
const MAX_PROFILE_BYTES: u64 = 1_000_000;

#[derive(Debug, Clone)]
pub struct OrchestrationRequest {
    pub repository: PathBuf,
    pub source: ReviewSource,
    pub expected_scope: String,
    pub manifest_path: PathBuf,
    pub expected_manifest_sha256: String,
    pub allow_repository_configuration: bool,
}

#[derive(Debug, Clone)]
pub struct PreparedManifestProfile {
    pub profile_id: String,
    pub prepared: PreparedProfile,
}

#[derive(Debug, Clone)]
pub struct PreparedOrchestration {
    pub manifest: OrchestrationManifest,
    pub manifest_path: PathBuf,
    pub manifest_sha256: String,
    pub manifest_id: String,
    pub profiles: Vec<PreparedManifestProfile>,
}

impl PreparedOrchestration {
    pub fn revalidate(&self) -> Result<(), OrchestrationError> {
        let (manifest_sha256, _) = sha256_file(&self.manifest_path, Some(MAX_MANIFEST_BYTES))
            .map_err(|error| OrchestrationError::new(error.to_string()))?;
        if manifest_sha256 != self.manifest_sha256 {
            return Err(OrchestrationError::new(
                "static-analysis orchestration manifest changed after preflight",
            ));
        }
        for profile in &self.profiles {
            verify_prepared_integrity(&profile.prepared, "after orchestration preflight")
                .map_err(|error| OrchestrationError::new(error.to_string()))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestrationError {
    message: String,
}

impl OrchestrationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for OrchestrationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OrchestrationError {}

pub fn prepare_orchestration(
    request: &OrchestrationRequest,
) -> Result<PreparedOrchestration, OrchestrationError> {
    if !is_scope_fingerprint(&request.expected_scope) {
        return Err(OrchestrationError::new(
            "--expect-scope is missing or invalid",
        ));
    }
    if !is_sha256(&request.expected_manifest_sha256) {
        return Err(OrchestrationError::new(
            "--expect-manifest-sha256 must be 64 lowercase hexadecimal characters",
        ));
    }
    if !request.manifest_path.is_absolute() {
        return Err(OrchestrationError::new(
            "--manifest must be an absolute path",
        ));
    }
    let repository = fs::canonicalize(&request.repository)
        .map_err(|error| OrchestrationError::new(format!("cannot resolve repository: {error}")))?;
    let manifest_bytes = read_bounded(
        &request.manifest_path,
        MAX_MANIFEST_BYTES,
        "static-analysis orchestration manifest",
    )?;
    let manifest_sha256 = sha256_bytes(&manifest_bytes);
    if manifest_sha256 != request.expected_manifest_sha256 {
        return Err(OrchestrationError::new(
            "manifest SHA256 does not match --expect-manifest-sha256",
        ));
    }
    let manifest: OrchestrationManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|error| {
            OrchestrationError::new(format!(
                "static-analysis orchestration manifest is not valid UTF-8 JSON: {error}"
            ))
        })?;
    manifest
        .validate()
        .map_err(|error| OrchestrationError::new(error.to_string()))?;

    let mut profiles = Vec::with_capacity(manifest.profiles.len());
    for profile_ref in &manifest.profiles {
        let profile_path = Path::new(&profile_ref.path);
        let repository_configuration =
            profile_repository_configuration(profile_path, &profile_ref.sha256)?;
        let allow_profile_configuration = request.allow_repository_configuration
            && repository_configuration == RepositoryConfiguration::ExplicitlyTrusted;
        let prepared = prepare_profile(
            &repository,
            profile_path,
            &profile_ref.sha256,
            allow_profile_configuration,
        )
        .map_err(|error| OrchestrationError::new(error.to_string()))?;
        profiles.push(PreparedManifestProfile {
            profile_id: profile_ref.profile_id.clone(),
            prepared,
        });
    }

    let manifest_path = fs::canonicalize(&request.manifest_path).map_err(|error| {
        OrchestrationError::new(format!(
            "cannot resolve static-analysis orchestration manifest: {error}"
        ))
    })?;
    let prepared = PreparedOrchestration {
        manifest,
        manifest_path,
        manifest_id: manifest_sha256[..16].to_string(),
        manifest_sha256,
        profiles,
    };
    prepared.revalidate()?;
    Ok(prepared)
}

fn profile_repository_configuration(
    path: &Path,
    expected_sha256: &str,
) -> Result<RepositoryConfiguration, OrchestrationError> {
    let bytes = read_bounded(path, MAX_PROFILE_BYTES, "static-analysis profile")?;
    if sha256_bytes(&bytes) != expected_sha256 {
        return Err(OrchestrationError::new(
            "profile SHA256 does not match the orchestration manifest",
        ));
    }
    let profile: StaticAnalysisProfile = serde_json::from_slice(&bytes).map_err(|error| {
        OrchestrationError::new(format!(
            "static-analysis profile is not valid UTF-8 JSON: {error}"
        ))
    })?;
    profile
        .validate()
        .map_err(|error| OrchestrationError::new(error.to_string()))?;
    Ok(profile.repository_configuration)
}

fn read_bounded(path: &Path, limit: u64, label: &str) -> Result<Vec<u8>, OrchestrationError> {
    let metadata = fs::metadata(path)
        .map_err(|error| OrchestrationError::new(format!("cannot read {label}: {error}")))?;
    if !metadata.is_file() {
        return Err(OrchestrationError::new(format!(
            "{label} must be a regular file"
        )));
    }
    let mut input = File::open(path)
        .map_err(|error| OrchestrationError::new(format!("cannot read {label}: {error}")))?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut input)
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| OrchestrationError::new(format!("cannot read {label}: {error}")))?;
    if bytes.len() as u64 > limit {
        return Err(OrchestrationError::new(format!(
            "{label} exceeds {limit} bytes"
        )));
    }
    Ok(bytes)
}

fn sha256_bytes(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_scope_fingerprint(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
