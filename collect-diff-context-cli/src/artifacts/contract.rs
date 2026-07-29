use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use url::Url;

pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_PACK_RECORDS: usize = 256;
pub const MAX_REVOCATION_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_REVOCATION_ENTRIES: usize = 16_384;

const RELEASE_REPOSITORY: &str = "junit/pre-commit-review";
const MAX_TEXT_BYTES: usize = 512;
const MAX_URL_BYTES: usize = 2_048;
const MAX_LICENSE_FILES: usize = 32;
const MAX_SOURCE_ASSETS: usize = 4;
const MAX_COMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactError {
    pub code: &'static str,
    message: String,
}

impl ArtifactError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ArtifactError {}

pub fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, ArtifactError> {
    serde_json::to_vec(value)
        .map_err(|_| ArtifactError::new("json-serialization", "artifact JSON serialization failed"))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactRole {
    Sanitizer,
    RepositoryContextProvider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactState {
    Active,
    Revoked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PackFormat {
    #[serde(rename = "normalized-tar-gzip-v1")]
    NormalizedTarGzipV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ProbeId {
    #[serde(rename = "gitleaks-version-v1")]
    GitleaksVersionV1,
    #[serde(rename = "gitleaks-stdin-json-v1")]
    GitleaksStdinJsonV1,
    #[serde(rename = "rust-analyzer-version-v1")]
    RustAnalyzerVersionV1,
    #[serde(rename = "rust-analyzer-stdio-v1")]
    RustAnalyzerStdioV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactFileBinding {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

impl ArtifactFileBinding {
    fn validate(&self, prefix: &str) -> Result<(), ArtifactError> {
        self.validate_any()?;
        if !self.path.starts_with(prefix) {
            return Err(ArtifactError::new(
                "artifact-path-role",
                "artifact file path does not match its role",
            ));
        }
        Ok(())
    }

    fn validate_any(&self) -> Result<(), ArtifactError> {
        validate_relative_path(&self.path)?;
        if self.size == 0 || self.size > MAX_EXPANDED_BYTES {
            return Err(ArtifactError::new(
                "artifact-file-size",
                "artifact file size is outside the authorized range",
            ));
        }
        validate_sha256(&self.sha256)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPackRecord {
    pub artifact_id: String,
    pub artifact_role: ArtifactRole,
    pub tool_version: String,
    pub upstream_repository: String,
    pub upstream_tag: String,
    pub upstream_commit: String,
    pub source_lock_sha256: String,
    pub platform_id: String,
    pub target_triple: String,
    pub state: ArtifactState,
    pub pack_version: String,
    pub project_release_tag: String,
    pub project_asset_name: String,
    pub expected_compressed_size: u64,
    pub max_compressed_size: u64,
    pub pack_sha256: String,
    pub pack_manifest_sha256: String,
    pub sbom_sha256: String,
    pub pack_format: PackFormat,
    pub executable: ArtifactFileBinding,
    pub version_probe: ProbeId,
    pub capability_probe: ProbeId,
    pub expected_version: String,
    pub license_component: String,
    pub license_files: Vec<ArtifactFileBinding>,
    pub sbom_component: String,
    pub default_configuration_sha256: Option<String>,
    pub quality_baseline_sha256: Option<String>,
    pub revoked_reason: Option<String>,
    pub replacement_pack_version: Option<String>,
}

impl ArtifactPackRecord {
    pub(crate) fn validate(&self) -> Result<(), ArtifactError> {
        validate_identifier(&self.artifact_id)?;
        validate_text(&self.tool_version)?;
        validate_repository(&self.upstream_repository)?;
        validate_source_tag(&self.upstream_tag)?;
        validate_commit(&self.upstream_commit)?;
        validate_sha256(&self.source_lock_sha256)?;
        validate_platform(&self.platform_id, &self.target_triple)?;
        validate_text(&self.pack_version)?;
        validate_release_tag(&self.project_release_tag)?;
        validate_filename(&self.project_asset_name)?;
        if self.expected_compressed_size == 0
            || self.max_compressed_size < self.expected_compressed_size
            || self.max_compressed_size > MAX_COMPRESSED_BYTES
        {
            return Err(ArtifactError::new(
                "pack-size-policy",
                "pack compressed size is outside the authorized range",
            ));
        }
        validate_sha256(&self.pack_sha256)?;
        validate_sha256(&self.pack_manifest_sha256)?;
        validate_sha256(&self.sbom_sha256)?;
        self.executable.validate("bin/")?;
        validate_text(&self.expected_version)?;
        validate_text(&self.license_component)?;
        validate_text(&self.sbom_component)?;
        if self.license_files.is_empty() || self.license_files.len() > MAX_LICENSE_FILES {
            return Err(ArtifactError::new(
                "license-file-count",
                "artifact license file count is outside the authorized range",
            ));
        }
        let mut previous_path: Option<&str> = None;
        for license in &self.license_files {
            license.validate("licenses/")?;
            if previous_path.is_some_and(|previous| previous >= license.path.as_str()) {
                return Err(ArtifactError::new(
                    "license-files-not-sorted",
                    "artifact license files must be sorted and unique",
                ));
            }
            previous_path = Some(&license.path);
        }
        self.validate_role_fields()?;
        self.validate_lifecycle_fields()
    }

    fn validate_role_fields(&self) -> Result<(), ArtifactError> {
        match self.artifact_role {
            ArtifactRole::Sanitizer => {
                if self.version_probe != ProbeId::GitleaksVersionV1
                    || self.capability_probe != ProbeId::GitleaksStdinJsonV1
                    || self.default_configuration_sha256.is_none()
                    || self.quality_baseline_sha256.is_some()
                {
                    return Err(ArtifactError::new(
                        "artifact-role-policy",
                        "sanitizer pack fields do not match the sanitizer policy",
                    ));
                }
                validate_sha256(self.default_configuration_sha256.as_deref().unwrap())?;
            }
            ArtifactRole::RepositoryContextProvider => {
                if self.version_probe != ProbeId::RustAnalyzerVersionV1
                    || self.capability_probe != ProbeId::RustAnalyzerStdioV1
                    || self.default_configuration_sha256.is_some()
                    || self.quality_baseline_sha256.is_none()
                {
                    return Err(ArtifactError::new(
                        "artifact-role-policy",
                        "provider pack fields do not match the provider policy",
                    ));
                }
                validate_sha256(self.quality_baseline_sha256.as_deref().unwrap())?;
            }
        }
        Ok(())
    }

    fn validate_lifecycle_fields(&self) -> Result<(), ArtifactError> {
        match self.state {
            ArtifactState::Active => {
                if self.revoked_reason.is_some() || self.replacement_pack_version.is_some() {
                    return Err(ArtifactError::new(
                        "active-pack-lifecycle",
                        "active pack cannot contain revocation fields",
                    ));
                }
            }
            ArtifactState::Revoked => {
                validate_text(self.revoked_reason.as_deref().ok_or_else(|| {
                    ArtifactError::new("revoked-pack-reason", "revoked pack must contain a reason")
                })?)?;
                if let Some(replacement) = self.replacement_pack_version.as_deref() {
                    validate_text(replacement)?;
                    if replacement == self.pack_version {
                        return Err(ArtifactError::new(
                            "revoked-pack-replacement",
                            "revoked pack replacement must name another pack version",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactManifest {
    pub schema_version: u8,
    pub kind: String,
    pub release_repository: String,
    pub revocation_index_sha256: String,
    pub packs: Vec<ArtifactPackRecord>,
}

impl ArtifactManifest {
    pub fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != 1 || self.kind != "third_party_artifacts" {
            return Err(ArtifactError::new(
                "manifest-identity",
                "artifact manifest identity is invalid",
            ));
        }
        if self.release_repository != RELEASE_REPOSITORY {
            return Err(ArtifactError::new(
                "release-repository-policy",
                "artifact release repository is not authorized",
            ));
        }
        validate_sha256(&self.revocation_index_sha256)?;
        if self.packs.len() > MAX_PACK_RECORDS {
            return Err(ArtifactError::new(
                "pack-record-limit",
                "artifact manifest contains too many pack records",
            ));
        }
        let mut active = BTreeSet::new();
        let mut assets = BTreeSet::new();
        let mut digests = BTreeSet::new();
        let mut previous_key: Option<(&str, &str, &str)> = None;
        for pack in &self.packs {
            pack.validate()?;
            let key = (
                pack.artifact_id.as_str(),
                pack.platform_id.as_str(),
                pack.pack_version.as_str(),
            );
            if let Some(previous) = previous_key {
                if previous == key {
                    return Err(ArtifactError::new(
                        "duplicate-pack-key",
                        "artifact manifest contains a duplicate pack key",
                    ));
                }
                if previous > key {
                    return Err(ArtifactError::new(
                        "pack-records-not-sorted",
                        "artifact pack records must be sorted",
                    ));
                }
            }
            previous_key = Some(key);
            if !assets.insert(pack.project_asset_name.as_str()) {
                return Err(ArtifactError::new(
                    "duplicate-pack-asset",
                    "artifact manifest contains a duplicate pack asset name",
                ));
            }
            if !digests.insert(pack.pack_sha256.as_str()) {
                return Err(ArtifactError::new(
                    "duplicate-pack-digest",
                    "artifact manifest contains a duplicate pack digest",
                ));
            }
            if pack.state == ArtifactState::Active
                && !active.insert((pack.artifact_id.as_str(), pack.platform_id.as_str()))
            {
                return Err(ArtifactError::new(
                    "multiple-active-packs",
                    "artifact manifest contains multiple active packs for a platform",
                ));
            }
        }
        if canonical_json(self)?.len() > MAX_MANIFEST_BYTES {
            return Err(ArtifactError::new(
                "manifest-size-limit",
                "artifact manifest exceeds its byte limit",
            ));
        }
        Ok(())
    }

    pub fn select_active(
        &self,
        artifact_id: &str,
        platform_id: &str,
    ) -> Result<&ArtifactPackRecord, ArtifactError> {
        self.validate()?;
        self.packs
            .iter()
            .find(|pack| {
                pack.artifact_id == artifact_id
                    && pack.platform_id == platform_id
                    && pack.state == ArtifactState::Active
            })
            .ok_or_else(|| {
                ArtifactError::new(
                    "artifact-not-active",
                    "no active artifact pack matches the selection",
                )
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackFileRole {
    Executable,
    License,
    Sbom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackFileRecord {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub role: PackFileRole,
}

impl PackFileRecord {
    fn validate(&self) -> Result<(), ArtifactError> {
        let binding = ArtifactFileBinding {
            path: self.path.clone(),
            size: self.size,
            sha256: self.sha256.clone(),
        };
        match self.role {
            PackFileRole::Executable => binding.validate("bin/"),
            PackFileRole::License => binding.validate("licenses/"),
            PackFileRole::Sbom => {
                binding.validate_any()?;
                if self.path != "sbom.cdx.json" {
                    return Err(ArtifactError::new(
                        "pack-file-role-path",
                        "pack SBOM must use its canonical path",
                    ));
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackManifest {
    pub schema_version: u8,
    pub kind: String,
    pub artifact_id: String,
    pub tool_version: String,
    pub pack_version: String,
    pub platform_id: String,
    pub target_triple: String,
    pub upstream_asset_name: String,
    pub upstream_asset_sha256: String,
    pub source_lock_sha256: String,
    pub project_asset_name: String,
    pub files: Vec<PackFileRecord>,
}

impl PackManifest {
    pub fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != 1 || self.kind != "third_party_artifact_pack" {
            return Err(ArtifactError::new(
                "pack-manifest-identity",
                "pack manifest identity is invalid",
            ));
        }
        validate_identifier(&self.artifact_id)?;
        validate_text(&self.tool_version)?;
        validate_text(&self.pack_version)?;
        validate_platform(&self.platform_id, &self.target_triple)?;
        validate_filename(&self.upstream_asset_name)?;
        validate_sha256(&self.upstream_asset_sha256)?;
        validate_sha256(&self.source_lock_sha256)?;
        validate_filename(&self.project_asset_name)?;
        if self.files.len() < 3 || self.files.len() > 127 {
            return Err(ArtifactError::new(
                "pack-file-count",
                "pack manifest file count is outside the authorized range",
            ));
        }
        let executable_count = self
            .files
            .iter()
            .filter(|file| file.role == PackFileRole::Executable)
            .count();
        let license_count = self
            .files
            .iter()
            .filter(|file| file.role == PackFileRole::License)
            .count();
        let sbom_count = self
            .files
            .iter()
            .filter(|file| file.role == PackFileRole::Sbom)
            .count();
        if executable_count != 1 || license_count == 0 || sbom_count != 1 {
            return Err(ArtifactError::new(
                "pack-file-role-count",
                "pack manifest must contain one executable, licenses, and one SBOM",
            ));
        }
        let mut previous: Option<&str> = None;
        for file in &self.files {
            file.validate()?;
            if previous.is_some_and(|value| value >= file.path.as_str()) {
                return Err(ArtifactError::new(
                    "pack-files-not-sorted",
                    "pack manifest files must be sorted and unique",
                ));
            }
            previous = Some(&file.path);
        }
        if canonical_json(self)?.len() > MAX_MANIFEST_BYTES {
            return Err(ArtifactError::new(
                "pack-manifest-size-limit",
                "pack manifest exceeds its byte limit",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeResult {
    pub probe_id: ProbeId,
    pub success: bool,
    pub observed_version: Option<String>,
}

impl ProbeResult {
    pub(crate) fn validate(&self) -> Result<(), ArtifactError> {
        if !self.success {
            return Err(ArtifactError::new(
                "receipt-probe-failed",
                "artifact receipt cannot authorize a failed probe",
            ));
        }
        match self.probe_id {
            ProbeId::GitleaksVersionV1 | ProbeId::RustAnalyzerVersionV1 => {
                validate_text(self.observed_version.as_deref().ok_or_else(|| {
                    ArtifactError::new(
                        "receipt-probe-version",
                        "version probe result must contain an observed version",
                    )
                })?)?;
            }
            ProbeId::GitleaksStdinJsonV1 | ProbeId::RustAnalyzerStdioV1 => {
                if self.observed_version.is_some() {
                    return Err(ArtifactError::new(
                        "receipt-probe-version",
                        "capability probe cannot contain an observed version",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReceipt {
    pub schema_version: u8,
    pub kind: String,
    pub distribution_manifest_sha256: String,
    pub artifact_id: String,
    pub tool_version: String,
    pub pack_version: String,
    pub platform_id: String,
    pub pack_sha256: String,
    pub pack_manifest_sha256: String,
    pub sbom_sha256: String,
    pub installed_files: Vec<ArtifactFileBinding>,
    pub license_files: Vec<ArtifactFileBinding>,
    pub probes: Vec<ProbeResult>,
    pub lifecycle_state: ArtifactState,
}

impl ArtifactReceipt {
    pub fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != 1 || self.kind != "third_party_artifact_receipt" {
            return Err(ArtifactError::new(
                "receipt-identity",
                "artifact receipt identity is invalid",
            ));
        }
        validate_sha256(&self.distribution_manifest_sha256)?;
        validate_identifier(&self.artifact_id)?;
        validate_text(&self.tool_version)?;
        validate_text(&self.pack_version)?;
        platform_target(&self.platform_id)?;
        validate_sha256(&self.pack_sha256)?;
        validate_sha256(&self.pack_manifest_sha256)?;
        validate_sha256(&self.sbom_sha256)?;
        validate_sorted_bindings(&self.installed_files, "receipt-installed-files")?;
        validate_sorted_bindings(&self.license_files, "receipt-license-files")?;
        if self.installed_files.is_empty() || self.license_files.is_empty() {
            return Err(ArtifactError::new(
                "receipt-file-count",
                "artifact receipt must bind installed and license files",
            ));
        }
        if self.probes.len() != 2 {
            return Err(ArtifactError::new(
                "receipt-probe-count",
                "artifact receipt must contain two probe results",
            ));
        }
        let mut previous: Option<ProbeId> = None;
        for probe in &self.probes {
            probe.validate()?;
            if previous.is_some_and(|value| value >= probe.probe_id) {
                return Err(ArtifactError::new(
                    "receipt-probes-not-sorted",
                    "artifact receipt probe results must be sorted and unique",
                ));
            }
            previous = Some(probe.probe_id);
        }
        if canonical_json(self)?.len() > MAX_MANIFEST_BYTES {
            return Err(ArtifactError::new(
                "receipt-size-limit",
                "artifact receipt exceeds its byte limit",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactOperation {
    Verify,
    Provision,
    Doctor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactReportStatus {
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReport {
    pub schema_version: u8,
    pub kind: String,
    pub operation: ArtifactOperation,
    pub status: ArtifactReportStatus,
    pub artifact_id: Option<String>,
    pub platform_id: Option<String>,
    pub pack_version: Option<String>,
    pub pack_sha256: Option<String>,
    pub executable_sha256: Option<String>,
    pub sbom_sha256: Option<String>,
    pub lifecycle_state: Option<ArtifactState>,
    pub code: Option<String>,
}

impl ArtifactReport {
    pub fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != 1 || self.kind != "third_party_artifact_report" {
            return Err(ArtifactError::new(
                "report-identity",
                "artifact report identity is invalid",
            ));
        }
        match self.status {
            ArtifactReportStatus::Completed => {
                validate_identifier(self.artifact_id.as_deref().ok_or_else(|| {
                    ArtifactError::new(
                        "report-completed-identity",
                        "completed report must identify its artifact",
                    )
                })?)?;
                platform_target(self.platform_id.as_deref().ok_or_else(|| {
                    ArtifactError::new(
                        "report-completed-identity",
                        "completed report must identify its platform",
                    )
                })?)?;
                validate_text(self.pack_version.as_deref().ok_or_else(|| {
                    ArtifactError::new(
                        "report-completed-identity",
                        "completed report must identify its pack version",
                    )
                })?)?;
                validate_sha256(self.pack_sha256.as_deref().ok_or_else(|| {
                    ArtifactError::new(
                        "report-completed-identity",
                        "completed report must bind its pack digest",
                    )
                })?)?;
                validate_sha256(self.executable_sha256.as_deref().ok_or_else(|| {
                    ArtifactError::new(
                        "report-completed-identity",
                        "completed report must bind its executable digest",
                    )
                })?)?;
                validate_sha256(self.sbom_sha256.as_deref().ok_or_else(|| {
                    ArtifactError::new(
                        "report-completed-identity",
                        "completed report must bind its SBOM digest",
                    )
                })?)?;
                if self.lifecycle_state.is_none() || self.code.is_some() {
                    return Err(ArtifactError::new(
                        "report-completed-fields",
                        "completed report fields are inconsistent",
                    ));
                }
            }
            ArtifactReportStatus::Failed => {
                validate_error_code(self.code.as_deref().ok_or_else(|| {
                    ArtifactError::new(
                        "report-failure-code",
                        "failed report must contain a bounded code",
                    )
                })?)?;
            }
        }
        if canonical_json(self)?.len() > 64 * 1024 {
            return Err(ArtifactError::new(
                "report-size-limit",
                "artifact report exceeds its byte limit",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineMeasurement {
    pub platform_id: String,
    pub pack_sha256: String,
    pub executable_sha256: String,
    pub profile_sha256: String,
    pub fixture_id: String,
    pub fixture_sha256: String,
    pub request_sha256: String,
    pub runner_class: String,
    pub samples_ms: Vec<u64>,
    pub p95_ms: u64,
    pub peak_process_tree_rss_bytes: u64,
}

impl BaselineMeasurement {
    fn validate(&self) -> Result<(), ArtifactError> {
        platform_target(&self.platform_id)?;
        validate_sha256(&self.pack_sha256)?;
        validate_sha256(&self.executable_sha256)?;
        validate_sha256(&self.profile_sha256)?;
        validate_identifier(&self.fixture_id)?;
        validate_sha256(&self.fixture_sha256)?;
        validate_sha256(&self.request_sha256)?;
        validate_identifier(&self.runner_class)?;
        if !(20..=100).contains(&self.samples_ms.len())
            || self
                .samples_ms
                .iter()
                .any(|sample| *sample == 0 || *sample > 30_000)
        {
            return Err(ArtifactError::new(
                "baseline-samples",
                "baseline samples are outside the authorized range",
            ));
        }
        let mut ordered = self.samples_ms.clone();
        ordered.sort_unstable();
        let rank = (ordered.len() * 95).div_ceil(100);
        if self.p95_ms != ordered[rank - 1] {
            return Err(ArtifactError::new(
                "baseline-p95",
                "baseline p95 does not match nearest-rank calculation",
            ));
        }
        if self.peak_process_tree_rss_bytes == 0
            || self.peak_process_tree_rss_bytes > MAX_EXPANDED_BYTES
        {
            return Err(ArtifactError::new(
                "baseline-rss",
                "baseline process-tree RSS is outside the acceptance range",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactBaseline {
    pub schema_version: u8,
    pub kind: String,
    pub artifact_id: String,
    pub pack_version: String,
    pub source_lock_sha256: String,
    pub measurements: Vec<BaselineMeasurement>,
}

impl ArtifactBaseline {
    pub fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != 1 || self.kind != "third_party_artifact_baseline" {
            return Err(ArtifactError::new(
                "baseline-identity",
                "artifact baseline identity is invalid",
            ));
        }
        validate_identifier(&self.artifact_id)?;
        validate_text(&self.pack_version)?;
        validate_sha256(&self.source_lock_sha256)?;
        if self.measurements.is_empty() || self.measurements.len() > 64 {
            return Err(ArtifactError::new(
                "baseline-measurement-count",
                "artifact baseline measurement count is outside the authorized range",
            ));
        }
        let mut previous: Option<(&str, &str)> = None;
        for measurement in &self.measurements {
            measurement.validate()?;
            let key = (
                measurement.platform_id.as_str(),
                measurement.fixture_id.as_str(),
            );
            if previous.is_some_and(|value| value >= key) {
                return Err(ArtifactError::new(
                    "baseline-measurements-not-sorted",
                    "baseline measurements must be sorted and unique",
                ));
            }
            previous = Some(key);
        }
        if canonical_json(self)?.len() > MAX_MANIFEST_BYTES {
            return Err(ArtifactError::new(
                "baseline-size-limit",
                "artifact baseline exceeds its byte limit",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorePackManifest {
    pub schema_version: u8,
    pub kind: String,
    pub core_version: String,
    pub platform_id: String,
    pub target_triple: String,
    pub distribution_manifest_sha256: String,
    pub revocation_index_sha256: String,
    pub members: Vec<ArtifactFileBinding>,
}

impl CorePackManifest {
    pub fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != 1 || self.kind != "pre_commit_review_core_pack" {
            return Err(ArtifactError::new(
                "core-pack-identity",
                "core pack manifest identity is invalid",
            ));
        }
        validate_text(&self.core_version)?;
        validate_platform(&self.platform_id, &self.target_triple)?;
        validate_sha256(&self.distribution_manifest_sha256)?;
        validate_sha256(&self.revocation_index_sha256)?;
        if self.members.is_empty() || self.members.len() > 512 {
            return Err(ArtifactError::new(
                "core-member-count",
                "core pack member count is outside the authorized range",
            ));
        }
        let manifest = self
            .members
            .iter()
            .find(|member| member.path == "runtime/distribution/manifest.json")
            .ok_or_else(|| {
                ArtifactError::new(
                    "core-manifest-member",
                    "core pack does not contain its distribution manifest",
                )
            })?;
        if manifest.sha256 != self.distribution_manifest_sha256 {
            return Err(ArtifactError::new(
                "core-manifest-member",
                "core distribution manifest digest does not match its inventory",
            ));
        }
        let revocations = self
            .members
            .iter()
            .find(|member| member.path == "runtime/distribution/revocations.json")
            .ok_or_else(|| {
                ArtifactError::new(
                    "core-revocation-member",
                    "core pack does not contain its revocation index",
                )
            })?;
        if revocations.sha256 != self.revocation_index_sha256 {
            return Err(ArtifactError::new(
                "core-revocation-member",
                "core revocation index digest does not match its inventory",
            ));
        }
        let expected_binary = format!("scripts/bin/collect_diff_context-{}", self.platform_id);
        let expected_binary = if self.platform_id == "windows-amd64" {
            format!("{expected_binary}.exe")
        } else {
            expected_binary
        };
        if !self
            .members
            .iter()
            .any(|member| member.path == expected_binary)
            || self.members.iter().any(|member| {
                member.path.starts_with("scripts/bin/collect_diff_context-")
                    && member.path != expected_binary
            })
        {
            return Err(ArtifactError::new(
                "core-platform-member",
                "core pack contains a missing or foreign platform collector",
            ));
        }
        validate_sorted_bindings(&self.members, "core-members")?;
        if canonical_json(self)?.len() > MAX_MANIFEST_BYTES {
            return Err(ArtifactError::new(
                "core-pack-size-limit",
                "core pack manifest exceeds its byte limit",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceAssetRecord {
    pub platform_id: String,
    pub target_triple: String,
    pub url: String,
    pub archive_name: String,
    pub archive_size: u64,
    pub archive_sha256: String,
    pub executable_name: String,
    pub executable_size: u64,
    pub executable_sha256: String,
    pub expected_version_output: String,
    pub license_source_paths: Vec<String>,
}

impl SourceAssetRecord {
    fn validate(&self, lock: &SourceLock) -> Result<(), ArtifactError> {
        validate_platform(&self.platform_id, &self.target_triple)?;
        validate_filename(&self.archive_name)?;
        validate_filename(&self.executable_name)?;
        if self.archive_size == 0 || self.archive_size > MAX_COMPRESSED_BYTES {
            return Err(ArtifactError::new(
                "source-archive-size",
                "source archive size is outside the authorized range",
            ));
        }
        if self.executable_size == 0 || self.executable_size > MAX_EXPANDED_BYTES {
            return Err(ArtifactError::new(
                "source-executable-size",
                "source executable size is outside the authorized range",
            ));
        }
        validate_sha256(&self.archive_sha256)?;
        validate_sha256(&self.executable_sha256)?;
        validate_text(&self.expected_version_output)?;
        if self.license_source_paths.is_empty()
            || self.license_source_paths.len() > MAX_LICENSE_FILES
        {
            return Err(ArtifactError::new(
                "source-license-count",
                "source lock license path count is outside the authorized range",
            ));
        }
        let mut previous: Option<&str> = None;
        for path in &self.license_source_paths {
            validate_relative_path(path)?;
            if previous.is_some_and(|value| value >= path.as_str()) {
                return Err(ArtifactError::new(
                    "source-licenses-not-sorted",
                    "source license paths must be sorted and unique",
                ));
            }
            previous = Some(path);
        }
        validate_source_url(self, lock)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLock {
    pub schema_version: u8,
    pub kind: String,
    pub artifact_id: String,
    pub tool_version: String,
    pub upstream_repository: String,
    pub upstream_tag: String,
    pub upstream_commit: String,
    pub assets: Vec<SourceAssetRecord>,
}

impl SourceLock {
    pub fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != 1 || self.kind != "third_party_sources" {
            return Err(ArtifactError::new(
                "source-lock-identity",
                "source lock identity is invalid",
            ));
        }
        validate_identifier(&self.artifact_id)?;
        validate_text(&self.tool_version)?;
        validate_repository(&self.upstream_repository)?;
        validate_source_tag(&self.upstream_tag)?;
        validate_commit(&self.upstream_commit)?;
        if self.assets.len() != MAX_SOURCE_ASSETS {
            return Err(ArtifactError::new(
                "source-asset-count",
                "source lock must contain exactly four platform assets",
            ));
        }
        let mut previous: Option<&str> = None;
        for asset in &self.assets {
            asset.validate(self)?;
            if previous.is_some_and(|value| value >= asset.platform_id.as_str()) {
                return Err(ArtifactError::new(
                    "source-assets-not-sorted",
                    "source assets must be sorted and unique",
                ));
            }
            previous = Some(&asset.platform_id);
        }
        if canonical_json(self)?.len() > MAX_MANIFEST_BYTES {
            return Err(ArtifactError::new(
                "source-lock-size-limit",
                "source lock exceeds its byte limit",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevocationEntry {
    pub pack_sha256: String,
    pub artifact_id: String,
    pub platform_id: String,
    pub pack_version: String,
    pub reason: String,
    pub replacement_pack_version: Option<String>,
}

impl RevocationEntry {
    fn validate(&self) -> Result<(), ArtifactError> {
        validate_sha256(&self.pack_sha256)?;
        validate_identifier(&self.artifact_id)?;
        platform_target(&self.platform_id)?;
        validate_text(&self.pack_version)?;
        validate_text(&self.reason)?;
        if let Some(replacement) = self.replacement_pack_version.as_deref() {
            validate_text(replacement)?;
            if replacement == self.pack_version {
                return Err(ArtifactError::new(
                    "revocation-replacement",
                    "revocation replacement must name another pack version",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevocationIndex {
    pub schema_version: u8,
    pub kind: String,
    pub entries: Vec<RevocationEntry>,
}

impl RevocationIndex {
    pub fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != 1 || self.kind != "third_party_artifact_revocations" {
            return Err(ArtifactError::new(
                "revocation-index-identity",
                "revocation index identity is invalid",
            ));
        }
        if self.entries.len() > MAX_REVOCATION_ENTRIES {
            return Err(ArtifactError::new(
                "revocation-entry-limit",
                "revocation index contains too many entries",
            ));
        }
        let mut previous: Option<&str> = None;
        for entry in &self.entries {
            entry.validate()?;
            if previous.is_some_and(|value| value >= entry.pack_sha256.as_str()) {
                return Err(ArtifactError::new(
                    "revocations-not-sorted",
                    "revocation entries must be sorted and unique",
                ));
            }
            previous = Some(&entry.pack_sha256);
        }
        if canonical_json(self)?.len() > MAX_REVOCATION_BYTES {
            return Err(ArtifactError::new(
                "revocation-size-limit",
                "revocation index exceeds its byte limit",
            ));
        }
        Ok(())
    }
}

fn validate_identifier(value: &str) -> Result<(), ArtifactError> {
    if value.is_empty()
        || value.len() > 64
        || !value.as_bytes()[0].is_ascii_lowercase()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ArtifactError::new(
            "invalid-identifier",
            "artifact identifier is invalid",
        ));
    }
    Ok(())
}

fn validate_error_code(value: &str) -> Result<(), ArtifactError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ArtifactError::new(
            "invalid-error-code",
            "artifact error code is invalid",
        ));
    }
    Ok(())
}

fn validate_sorted_bindings(
    bindings: &[ArtifactFileBinding],
    code: &'static str,
) -> Result<(), ArtifactError> {
    let mut previous: Option<&str> = None;
    for binding in bindings {
        binding.validate_any()?;
        if previous.is_some_and(|value| value >= binding.path.as_str()) {
            return Err(ArtifactError::new(
                code,
                "artifact file bindings must be sorted and unique",
            ));
        }
        previous = Some(&binding.path);
    }
    Ok(())
}

fn validate_text(value: &str) -> Result<(), ArtifactError> {
    if value.is_empty() || value.len() > MAX_TEXT_BYTES || value.chars().any(char::is_control) {
        return Err(ArtifactError::new(
            "invalid-text",
            "artifact text value is invalid",
        ));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), ArtifactError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ArtifactError::new(
            "invalid-sha256",
            "artifact SHA256 must be lower-case hexadecimal",
        ));
    }
    Ok(())
}

fn validate_commit(value: &str) -> Result<(), ArtifactError> {
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ArtifactError::new(
            "invalid-upstream-commit",
            "upstream commit must be a lower-case Git object ID",
        ));
    }
    Ok(())
}

fn validate_repository(value: &str) -> Result<(), ArtifactError> {
    if !matches!(value, "gitleaks/gitleaks" | "rust-lang/rust-analyzer") {
        return Err(ArtifactError::new(
            "upstream-repository-policy",
            "upstream repository is not allowlisted",
        ));
    }
    Ok(())
}

fn validate_source_tag(value: &str) -> Result<(), ArtifactError> {
    validate_text(value)?;
    let lower = value.to_ascii_lowercase();
    if lower.contains("latest") || lower.contains("nightly") || value.contains('/') {
        return Err(ArtifactError::new(
            "source-tag-policy",
            "source tag must be an immutable named release",
        ));
    }
    Ok(())
}

fn validate_release_tag(value: &str) -> Result<(), ArtifactError> {
    validate_text(value)?;
    let lower = value.to_ascii_lowercase();
    if lower.contains("latest")
        || lower.contains("nightly")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(ArtifactError::new(
            "release-tag-policy",
            "project release tag is invalid",
        ));
    }
    Ok(())
}

fn validate_filename(value: &str) -> Result<(), ArtifactError> {
    if value.is_empty()
        || value.len() > 255
        || matches!(value, "." | "..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(ArtifactError::new(
            "invalid-filename",
            "artifact filename is invalid",
        ));
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<(), ArtifactError> {
    if value.is_empty()
        || value.len() > 512
        || value.starts_with('/')
        || value.contains('\\')
        || value.contains(':')
        || value
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || value.chars().any(char::is_control)
    {
        return Err(ArtifactError::new(
            "invalid-relative-path",
            "artifact path is not a safe relative path",
        ));
    }
    Ok(())
}

fn validate_platform(platform_id: &str, target_triple: &str) -> Result<(), ArtifactError> {
    if platform_target(platform_id)? != target_triple {
        return Err(ArtifactError::new(
            "platform-target-mismatch",
            "artifact platform and target triple do not match",
        ));
    }
    Ok(())
}

fn platform_target(platform_id: &str) -> Result<&'static str, ArtifactError> {
    match platform_id {
        "darwin-amd64" => Ok("x86_64-apple-darwin"),
        "darwin-arm64" => Ok("aarch64-apple-darwin"),
        "linux-amd64" => Ok("x86_64-unknown-linux-musl"),
        "windows-amd64" => Ok("x86_64-pc-windows-msvc"),
        _ => Err(ArtifactError::new(
            "unsupported-platform",
            "artifact platform is not supported",
        )),
    }
}

fn validate_source_url(asset: &SourceAssetRecord, lock: &SourceLock) -> Result<(), ArtifactError> {
    if asset.url.len() > MAX_URL_BYTES {
        return Err(ArtifactError::new(
            "source-url-policy",
            "source URL exceeds its byte limit",
        ));
    }
    let url = Url::parse(&asset.url)
        .map_err(|_| ArtifactError::new("source-url-policy", "source URL is not a valid URL"))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ArtifactError::new(
            "source-url-policy",
            "source URL is outside the fixed HTTPS GitHub policy",
        ));
    }
    let expected_path = format!(
        "/{}/releases/download/{}/{}",
        lock.upstream_repository, lock.upstream_tag, asset.archive_name
    );
    if url.path() != expected_path {
        return Err(ArtifactError::new(
            "source-url-policy",
            "source URL does not match the locked release asset",
        ));
    }
    Ok(())
}
