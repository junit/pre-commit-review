use crate::provider_resources::{
    ResourceAccountingStatus, MAX_RESOURCE_SAMPLE_INTERVAL_MS,
    PRODUCTION_PROCESS_TREE_RSS_LIMIT_BYTES,
};
use crate::review_scope::ReviewSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

pub const MAX_DEADLINE_MS: u64 = 30_000;
pub const MAX_DEPTH: u8 = 2;
pub const MAX_SEEDS: usize = 64;
pub const MAX_REQUESTS: usize = 512;
pub const MAX_PENDING_REQUESTS: usize = 1;
pub const MAX_MESSAGES: usize = 2_048;
pub const MAX_NOTIFICATIONS: usize = 512;
pub const MAX_SERVER_REQUESTS: usize = 128;
pub const MAX_INVALID_MESSAGES: usize = 32;
pub const MAX_CALL_RANGES: usize = 1_000;
pub const MAX_HEADER_BYTES: usize = 16 * 1_024;
pub const MAX_FRAME_BYTES: usize = 4 * 1_024 * 1_024;
pub const MAX_PROTOCOL_BYTES: usize = 64 * 1_024 * 1_024;
pub const MAX_STDERR_BYTES: usize = 1_024 * 1_024;
pub const MAX_TOTAL_OUTPUT_BYTES: usize = 65 * 1_024 * 1_024;
pub const MAX_SOURCE_FILE_BYTES: usize = 4 * 1_024 * 1_024;
pub const MAX_SOURCE_BYTES: usize = 64 * 1_024 * 1_024;
pub const MAX_NODES: usize = 5_000;
pub const MAX_EDGES: usize = 10_000;
pub const MAX_REPORT_BYTES: usize = 16 * 1_024 * 1_024;

const MAX_PATH_BYTES: usize = 4_096;
const MAX_ID_BYTES: usize = 256;
const MAX_KIND_BYTES: usize = 100;
const MAX_VERSION_BYTES: usize = 100;
const MAX_NAME_BYTES: usize = 1_024;
const MAX_TARGET_BYTES: usize = 256;
const MAX_CFG_ITEMS: usize = 4_096;
const MAX_ENV_ITEMS: usize = 1_024;
const MAX_LIMITATIONS: usize = 1_000;
const MAX_LIMITATION_BYTES: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepositoryContextProviderStatus {
    Completed,
    Partial,
    Unavailable,
    Timeout,
    InvalidOutput,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderCompleteness {
    Complete,
    Partial,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallDirection {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SeedKind {
    Function,
    Method,
    AssociatedFunction,
    FunctionDeclaration,
    MethodDeclaration,
    AssociatedFunctionDeclaration,
}

impl SeedKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::AssociatedFunction => "associated-function",
            Self::FunctionDeclaration => "function-declaration",
            Self::MethodDeclaration => "method-declaration",
            Self::AssociatedFunctionDeclaration => "associated-function-declaration",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderRangeFormat {
    #[serde(rename = "provider-source-range-v1/utf8-byte-columns/end-exclusive")]
    Utf8ByteColumnsEndExclusiveV1,
}

impl ProviderRangeFormat {
    fn as_str(self) -> &'static str {
        "provider-source-range-v1/utf8-byte-columns/end-exclusive"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PositionEncoding {
    #[serde(rename = "utf-8")]
    Utf8,
    #[serde(rename = "utf-16")]
    Utf16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderNetworkIsolation {
    BestEffortOffline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRange {
    pub format: ProviderRangeFormat,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub start_byte: usize,
    pub end_byte: usize,
}

impl ProviderRange {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.start_line == 0
            || self.start_column == 0
            || self.end_line == 0
            || self.end_column == 0
        {
            return contract_error(
                "provider-range-invalid",
                "provider range lines and columns must be one-based",
            );
        }
        let coordinate_order =
            (self.start_line, self.start_column) < (self.end_line, self.end_column);
        if !coordinate_order || self.start_byte >= self.end_byte {
            return contract_error(
                "provider-range-invalid",
                "provider ranges must be non-empty and end-exclusive",
            );
        }
        if self.end_byte > MAX_SOURCE_FILE_BYTES {
            return contract_error(
                "provider-range-unbounded",
                "provider range exceeds the source-file byte maximum",
            );
        }
        Ok(())
    }

    fn contains(&self, other: &Self) -> bool {
        self.start_byte <= other.start_byte
            && other.end_byte <= self.end_byte
            && (self.start_line, self.start_column) <= (other.start_line, other.start_column)
            && (other.end_line, other.end_column) <= (self.end_line, self.end_column)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderLimits {
    pub deadline_ms: u64,
    pub max_depth: u8,
    pub max_seeds: usize,
    pub max_requests: usize,
    pub max_pending_requests: usize,
    pub max_messages: usize,
    pub max_notifications: usize,
    pub max_server_requests: usize,
    pub max_invalid_messages: usize,
    pub max_call_ranges: usize,
    pub max_header_bytes: usize,
    pub max_frame_bytes: usize,
    pub max_protocol_bytes: usize,
    pub max_stderr_bytes: usize,
    pub max_total_output_bytes: usize,
    pub max_source_file_bytes: usize,
    pub max_source_bytes: usize,
    pub max_nodes: usize,
    pub max_edges: usize,
    pub max_report_bytes: usize,
}

impl ProviderLimits {
    pub const fn maximum() -> Self {
        Self {
            deadline_ms: MAX_DEADLINE_MS,
            max_depth: MAX_DEPTH,
            max_seeds: MAX_SEEDS,
            max_requests: MAX_REQUESTS,
            max_pending_requests: MAX_PENDING_REQUESTS,
            max_messages: MAX_MESSAGES,
            max_notifications: MAX_NOTIFICATIONS,
            max_server_requests: MAX_SERVER_REQUESTS,
            max_invalid_messages: MAX_INVALID_MESSAGES,
            max_call_ranges: MAX_CALL_RANGES,
            max_header_bytes: MAX_HEADER_BYTES,
            max_frame_bytes: MAX_FRAME_BYTES,
            max_protocol_bytes: MAX_PROTOCOL_BYTES,
            max_stderr_bytes: MAX_STDERR_BYTES,
            max_total_output_bytes: MAX_TOTAL_OUTPUT_BYTES,
            max_source_file_bytes: MAX_SOURCE_FILE_BYTES,
            max_source_bytes: MAX_SOURCE_BYTES,
            max_nodes: MAX_NODES,
            max_edges: MAX_EDGES,
            max_report_bytes: MAX_REPORT_BYTES,
        }
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        let maximum = Self::maximum();
        validate_limit(self.deadline_ms, maximum.deadline_ms, "deadline_ms")?;
        validate_limit(self.max_depth, maximum.max_depth, "max_depth")?;
        validate_limit(self.max_seeds, maximum.max_seeds, "max_seeds")?;
        validate_limit(self.max_requests, maximum.max_requests, "max_requests")?;
        validate_limit(
            self.max_pending_requests,
            maximum.max_pending_requests,
            "max_pending_requests",
        )?;
        validate_limit(self.max_messages, maximum.max_messages, "max_messages")?;
        validate_limit(
            self.max_notifications,
            maximum.max_notifications,
            "max_notifications",
        )?;
        validate_limit(
            self.max_server_requests,
            maximum.max_server_requests,
            "max_server_requests",
        )?;
        validate_limit(
            self.max_invalid_messages,
            maximum.max_invalid_messages,
            "max_invalid_messages",
        )?;
        validate_limit(
            self.max_call_ranges,
            maximum.max_call_ranges,
            "max_call_ranges",
        )?;
        validate_limit(
            self.max_header_bytes,
            maximum.max_header_bytes,
            "max_header_bytes",
        )?;
        validate_limit(
            self.max_frame_bytes,
            maximum.max_frame_bytes,
            "max_frame_bytes",
        )?;
        validate_limit(
            self.max_protocol_bytes,
            maximum.max_protocol_bytes,
            "max_protocol_bytes",
        )?;
        validate_limit(
            self.max_stderr_bytes,
            maximum.max_stderr_bytes,
            "max_stderr_bytes",
        )?;
        validate_limit(
            self.max_total_output_bytes,
            maximum.max_total_output_bytes,
            "max_total_output_bytes",
        )?;
        validate_limit(
            self.max_source_file_bytes,
            maximum.max_source_file_bytes,
            "max_source_file_bytes",
        )?;
        validate_limit(
            self.max_source_bytes,
            maximum.max_source_bytes,
            "max_source_bytes",
        )?;
        validate_limit(self.max_nodes, maximum.max_nodes, "max_nodes")?;
        validate_limit(self.max_edges, maximum.max_edges, "max_edges")?;
        validate_limit(
            self.max_report_bytes,
            maximum.max_report_bytes,
            "max_report_bytes",
        )?;
        if self.max_source_file_bytes > self.max_source_bytes {
            return contract_error(
                "provider-limit-inconsistent",
                "max_source_file_bytes cannot exceed max_source_bytes",
            );
        }
        if self.max_frame_bytes > self.max_protocol_bytes {
            return contract_error(
                "provider-limit-inconsistent",
                "max_frame_bytes cannot exceed max_protocol_bytes",
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedSymbol {
    pub changed_symbol_id: String,
    pub path: String,
    pub kind: SeedKind,
    pub name: String,
    pub symbol_range: ProviderRange,
    pub selection_range: ProviderRange,
    pub query_byte: usize,
}

impl SeedSymbol {
    pub(crate) fn validate(&self) -> Result<(), ContractError> {
        validate_sha256(&self.changed_symbol_id, "changed_symbol_id")?;
        validate_snapshot_relative_path(&self.path, "seed path")?;
        validate_text(&self.name, MAX_NAME_BYTES, "seed name")?;
        self.symbol_range.validate()?;
        self.selection_range.validate()?;
        if !self.symbol_range.contains(&self.selection_range) {
            return contract_error(
                "provider-seed-selection-invalid",
                "seed selection range must be contained by its symbol range",
            );
        }
        if self.query_byte < self.selection_range.start_byte
            || self.query_byte >= self.selection_range.end_byte
        {
            return contract_error(
                "provider-seed-query-invalid",
                "seed query byte must be inside the end-exclusive selection range",
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateBinding {
    pub source: ReviewSource,
    pub scope_fingerprint: String,
    pub candidate_digest: String,
    pub snapshot_root: PathBuf,
    pub snapshot_sha256: String,
    pub snapshot_files: usize,
    pub snapshot_bytes: u64,
    pub project_model_digest: String,
}

impl CandidateBinding {
    pub(crate) fn validate(&self) -> Result<(), ContractError> {
        validate_scope_fingerprint(&self.scope_fingerprint)?;
        validate_sha256(&self.candidate_digest, "candidate digest")?;
        validate_sha256(&self.snapshot_sha256, "snapshot digest")?;
        validate_sha256(&self.project_model_digest, "project-model digest")?;
        validate_absolute_path(&self.snapshot_root, "snapshot root")?;
        if self.snapshot_files == 0 {
            return contract_error(
                "provider-candidate-empty",
                "candidate snapshot must contain at least one file",
            );
        }
        if self.snapshot_bytes > MAX_SOURCE_BYTES as u64 {
            return contract_error(
                "provider-candidate-unbounded",
                "candidate snapshot byte count exceeds the contract maximum",
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderBinding {
    pub kind: String,
    pub version: String,
    pub profile_path: PathBuf,
    pub profile_sha256: String,
    pub executable_path: PathBuf,
    pub executable_sha256: String,
    pub configuration_sha256: String,
    pub target_triple: String,
    pub toolchain_mode: String,
}

impl ProviderBinding {
    fn validate(&self, snapshot_root: &Path) -> Result<(), ContractError> {
        validate_text(&self.kind, MAX_KIND_BYTES, "provider kind")?;
        if self.kind != "rust-analyzer" {
            return contract_error(
                "provider-kind-invalid",
                "provider kind must equal rust-analyzer",
            );
        }
        validate_text(&self.version, MAX_VERSION_BYTES, "provider version")?;
        validate_absolute_path(&self.profile_path, "profile path")?;
        validate_absolute_path(&self.executable_path, "executable path")?;
        if self.profile_path.starts_with(snapshot_root)
            || self.executable_path.starts_with(snapshot_root)
        {
            return contract_error(
                "provider-path-inside-snapshot",
                "profile and executable paths must be outside the candidate snapshot",
            );
        }
        validate_sha256(&self.profile_sha256, "profile digest")?;
        validate_sha256(&self.executable_sha256, "executable digest")?;
        validate_sha256(&self.configuration_sha256, "configuration digest")?;
        validate_target(&self.target_triple)?;
        if self.toolchain_mode != "none" {
            return contract_error(
                "provider-toolchain-forbidden",
                "provider toolchain mode must equal none",
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryContextProviderRequest {
    pub schema_version: u8,
    pub kind: String,
    pub candidate: CandidateBinding,
    pub provider: ProviderBinding,
    pub seeds: Vec<SeedSymbol>,
    pub directions: Vec<CallDirection>,
    pub limits: ProviderLimits,
}

impl RepositoryContextProviderRequest {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema_version != 1 {
            return contract_error(
                "provider-request-schema-invalid",
                "request schema_version must equal 1",
            );
        }
        if self.kind != "repository_context_provider_request" {
            return contract_error(
                "provider-request-kind-invalid",
                "request kind is not recognized",
            );
        }
        self.candidate.validate()?;
        self.provider.validate(&self.candidate.snapshot_root)?;
        self.limits.validate()?;
        if self.seeds.is_empty() || self.seeds.len() > self.limits.max_seeds {
            return contract_error(
                "provider-seeds-invalid",
                "request seeds must be non-empty and within max_seeds",
            );
        }
        validate_sorted_unique_by(
            &self.seeds,
            |left, right| left.changed_symbol_id.cmp(&right.changed_symbol_id),
            "provider-seeds-order-invalid",
            "request seeds must be sorted with unique changed symbol IDs",
        )?;
        for seed in &self.seeds {
            seed.validate()?;
        }
        if self.directions.is_empty() || self.directions.len() > 2 {
            return contract_error(
                "provider-directions-invalid",
                "request directions must be non-empty",
            );
        }
        validate_sorted_unique_by(
            &self.directions,
            |left, right| left.cmp(right),
            "provider-directions-order-invalid",
            "request directions must be sorted and unique",
        )?;
        Ok(())
    }

    pub fn binding_digest(&self, project_model_algorithm: &str) -> Result<String, ContractError> {
        self.validate()?;
        validate_text(
            project_model_algorithm,
            MAX_KIND_BYTES,
            "project-model algorithm",
        )?;
        let mut digest = LengthPrefixedDigest::new("repository-context-binding-v1");
        digest.push(self.candidate.source.as_str().as_bytes());
        digest.push(self.candidate.scope_fingerprint.as_bytes());
        digest.push(self.candidate.candidate_digest.as_bytes());
        digest.push(self.candidate.snapshot_sha256.as_bytes());
        digest.push(project_model_algorithm.as_bytes());
        digest.push(self.candidate.project_model_digest.as_bytes());
        digest.push(self.provider.profile_sha256.as_bytes());
        digest.push(self.provider.kind.as_bytes());
        digest.push(self.provider.version.as_bytes());
        digest.push(self.provider.executable_sha256.as_bytes());
        digest.push(self.provider.configuration_sha256.as_bytes());
        digest.push(self.provider.target_triple.as_bytes());
        digest.push(self.provider.toolchain_mode.as_bytes());
        Ok(digest.finish())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderHardening {
    pub cargo_build_scripts: bool,
    pub cargo_no_deps: bool,
    pub cargo_sysroot: Option<String>,
    pub cargo_sysroot_src: Option<String>,
    pub proc_macro: bool,
    pub check_on_save: bool,
    pub workspace_discovery: bool,
    pub empty_path: bool,
    pub server_status_notification: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizedProviderProfile {
    pub schema_version: u8,
    pub kind: String,
    pub provider_kind: String,
    pub provider_version: String,
    pub executable_sha256: String,
    pub configuration_sha256: String,
    pub target_triple: String,
    pub toolchain_mode: String,
    pub arguments: Vec<String>,
    pub hardening: ProviderHardening,
    pub maximum_limits: ProviderLimits,
}

impl AuthorizedProviderProfile {
    pub fn rust_analyzer(
        provider_version: String,
        executable_sha256: String,
        target_triple: String,
    ) -> Self {
        let profile = Self {
            schema_version: 1,
            kind: "repository_context_provider_profile".to_string(),
            provider_kind: "rust-analyzer".to_string(),
            provider_version,
            executable_sha256,
            configuration_sha256: String::new(),
            target_triple,
            toolchain_mode: "none".to_string(),
            arguments: Vec::new(),
            hardening: ProviderHardening {
                cargo_build_scripts: false,
                cargo_no_deps: true,
                cargo_sysroot: None,
                cargo_sysroot_src: None,
                proc_macro: false,
                check_on_save: false,
                workspace_discovery: false,
                empty_path: true,
                server_status_notification: true,
            },
            maximum_limits: ProviderLimits::maximum(),
        };
        Self {
            configuration_sha256: profile.canonical_configuration_sha256(),
            ..profile
        }
    }

    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.schema_version != 1 {
            return profile_error(
                "provider-profile-schema-invalid",
                "profile schema_version must equal 1",
            );
        }
        if self.kind != "repository_context_provider_profile" {
            return profile_error(
                "provider-profile-kind-invalid",
                "profile kind is not recognized",
            );
        }
        if self.provider_kind != "rust-analyzer" {
            return profile_error(
                "provider-profile-provider-invalid",
                "profile provider kind must equal rust-analyzer",
            );
        }
        validate_text(
            &self.provider_version,
            MAX_VERSION_BYTES,
            "provider version",
        )
        .map_err(ProfileError::from)?;
        validate_sha256(&self.executable_sha256, "executable digest")
            .map_err(ProfileError::from)?;
        validate_sha256(&self.configuration_sha256, "configuration digest")
            .map_err(ProfileError::from)?;
        validate_target(&self.target_triple).map_err(ProfileError::from)?;
        if self.toolchain_mode != "none" {
            return profile_error(
                "provider-profile-toolchain-forbidden",
                "profile toolchain mode must equal none",
            );
        }
        if !self.arguments.is_empty() {
            return profile_error(
                "provider-profile-arguments-invalid",
                "profile arguments must use rust-analyzer's default stdio mode",
            );
        }
        let hardening = &self.hardening;
        if hardening.cargo_build_scripts
            || !hardening.cargo_no_deps
            || hardening.cargo_sysroot.is_some()
            || hardening.cargo_sysroot_src.is_some()
            || hardening.proc_macro
            || hardening.check_on_save
            || hardening.workspace_discovery
            || !hardening.empty_path
            || !hardening.server_status_notification
        {
            return profile_error(
                "provider-profile-hardening-invalid",
                "profile must retain the fixed no-toolchain hardening policy",
            );
        }
        if self.maximum_limits != ProviderLimits::maximum() {
            return profile_error(
                "provider-profile-limits-invalid",
                "profile maximum limits must equal the immutable contract maxima",
            );
        }
        if self.configuration_sha256 != self.canonical_configuration_sha256() {
            return profile_error(
                "provider-profile-configuration-mismatch",
                "profile configuration digest does not match its typed configuration",
            );
        }
        Ok(())
    }

    pub fn validate_request(
        &self,
        request: &RepositoryContextProviderRequest,
    ) -> Result<(), ProfileError> {
        self.validate()?;
        request.validate().map_err(ProfileError::from)?;
        if request.provider.kind != self.provider_kind
            || request.provider.version != self.provider_version
        {
            return profile_error(
                "provider-profile-binding-mismatch",
                "request provider identity is not authorized by the profile",
            );
        }
        if request.provider.profile_sha256 != self.sha256()
            || request.provider.executable_sha256 != self.executable_sha256
            || request.provider.configuration_sha256 != self.configuration_sha256
            || request.provider.target_triple != self.target_triple
            || request.provider.toolchain_mode != self.toolchain_mode
        {
            return profile_error(
                "provider-profile-binding-mismatch",
                "request binding does not match the authorized profile",
            );
        }
        Ok(())
    }

    pub fn canonical_configuration_sha256(&self) -> String {
        #[derive(Serialize)]
        struct Configuration<'a> {
            target_triple: &'a str,
            toolchain_mode: &'a str,
            hardening: &'a ProviderHardening,
        }
        sha256_json(&Configuration {
            target_triple: &self.target_triple,
            toolchain_mode: &self.toolchain_mode,
            hardening: &self.hardening,
        })
    }

    pub fn sha256(&self) -> String {
        sha256_json(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustAnalyzerDependency {
    pub crate_id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustAnalyzerCrate {
    pub crate_id: String,
    pub root_module: String,
    pub edition: String,
    pub dependencies: Vec<RustAnalyzerDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustAnalyzerProjectModel {
    pub schema_version: u8,
    pub algorithm: String,
    pub digest: String,
    pub target_triple: String,
    pub crates: Vec<RustAnalyzerCrate>,
    pub cfg: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub limitations: Vec<String>,
}

impl RustAnalyzerProjectModel {
    pub fn validate(&self) -> Result<(), ProjectModelError> {
        if self.schema_version != 1 {
            return project_model_error(
                "provider-model-schema-invalid",
                "project model schema_version must equal 1",
            );
        }
        if self.algorithm != "rust-analyzer-linked-project-v1" {
            return project_model_error(
                "provider-model-algorithm-invalid",
                "project model algorithm is not recognized",
            );
        }
        validate_sha256(&self.digest, "project-model digest").map_err(ProjectModelError::from)?;
        validate_target(&self.target_triple).map_err(ProjectModelError::from)?;
        if self.crates.is_empty() || self.crates.len() > MAX_NODES {
            return project_model_error(
                "provider-model-crates-invalid",
                "project model crates must be non-empty and bounded",
            );
        }
        validate_sorted_unique_by(
            &self.crates,
            |left, right| left.crate_id.cmp(&right.crate_id),
            "provider-model-crates-order-invalid",
            "project model crates must be sorted by unique crate_id",
        )
        .map_err(ProjectModelError::from)?;
        let crate_ids = self
            .crates
            .iter()
            .map(|item| item.crate_id.as_str())
            .collect::<BTreeSet<_>>();
        for item in &self.crates {
            validate_identifier(&item.crate_id, "crate_id").map_err(ProjectModelError::from)?;
            validate_snapshot_relative_path(&item.root_module, "crate root_module")
                .map_err(ProjectModelError::from)?;
            if !matches!(item.edition.as_str(), "2015" | "2018" | "2021" | "2024") {
                return project_model_error(
                    "provider-model-edition-invalid",
                    "project model crate edition is unsupported",
                );
            }
            if item.dependencies.len() > MAX_NODES {
                return project_model_error(
                    "provider-model-dependencies-unbounded",
                    "project model dependency list exceeds the maximum",
                );
            }
            validate_sorted_unique_by(
                &item.dependencies,
                |left, right| {
                    left.crate_id
                        .cmp(&right.crate_id)
                        .then_with(|| left.name.cmp(&right.name))
                },
                "provider-model-dependencies-order-invalid",
                "project model dependencies must be sorted and unique",
            )
            .map_err(ProjectModelError::from)?;
            let mut dependency_ids = BTreeSet::new();
            let mut dependency_names = BTreeSet::new();
            for dependency in &item.dependencies {
                validate_identifier(&dependency.crate_id, "dependency crate_id")
                    .map_err(ProjectModelError::from)?;
                validate_identifier(&dependency.name, "dependency name")
                    .map_err(ProjectModelError::from)?;
                if dependency.crate_id == item.crate_id
                    || !crate_ids.contains(dependency.crate_id.as_str())
                    || !dependency_ids.insert(dependency.crate_id.as_str())
                    || !dependency_names.insert(dependency.name.as_str())
                {
                    return project_model_error(
                        "provider-model-dependency-invalid",
                        "project model dependency IDs and names must be unique and target another crate",
                    );
                }
            }
        }
        validate_sorted_unique_text(&self.cfg, MAX_CFG_ITEMS, MAX_NAME_BYTES, "model cfg")
            .map_err(ProjectModelError::from)?;
        if self.env.len() > MAX_ENV_ITEMS {
            return project_model_error(
                "provider-model-env-unbounded",
                "project model environment exceeds the maximum",
            );
        }
        for (key, value) in &self.env {
            validate_identifier(key, "environment key").map_err(ProjectModelError::from)?;
            validate_text(value, MAX_LIMITATION_BYTES, "environment value")
                .map_err(ProjectModelError::from)?;
        }
        validate_sorted_unique_text(
            &self.limitations,
            MAX_LIMITATIONS,
            MAX_LIMITATION_BYTES,
            "model limitations",
        )
        .map_err(ProjectModelError::from)?;
        if self.digest != self.canonical_sha256() {
            return project_model_error(
                "provider-model-digest-mismatch",
                "project model digest does not match its canonical fields",
            );
        }
        Ok(())
    }

    pub fn canonical_sha256(&self) -> String {
        #[derive(Serialize)]
        struct CanonicalModel<'a> {
            schema_version: u8,
            algorithm: &'a str,
            target_triple: &'a str,
            crates: &'a [RustAnalyzerCrate],
            cfg: &'a [String],
            env: &'a BTreeMap<String, String>,
            limitations: &'a [String],
        }
        sha256_json(&CanonicalModel {
            schema_version: self.schema_version,
            algorithm: &self.algorithm,
            target_triple: &self.target_triple,
            crates: &self.crates,
            cfg: &self.cfg,
            env: &self.env,
            limitations: &self.limitations,
        })
    }

    pub fn linked_project_value(&self) -> Result<serde_json::Value, ProjectModelError> {
        self.linked_project_value_with_root(None)
    }

    pub fn linked_project_value_at(
        &self,
        snapshot_root: &Path,
    ) -> Result<serde_json::Value, ProjectModelError> {
        if !snapshot_root.is_absolute() {
            return project_model_error(
                "provider-model-root-invalid",
                "linked-project snapshot root must be absolute",
            );
        }
        self.linked_project_value_with_root(Some(snapshot_root))
    }

    fn linked_project_value_with_root(
        &self,
        snapshot_root: Option<&Path>,
    ) -> Result<serde_json::Value, ProjectModelError> {
        self.validate()?;
        let crate_indices = self
            .crates
            .iter()
            .enumerate()
            .map(|(index, item)| (item.crate_id.as_str(), index))
            .collect::<BTreeMap<_, _>>();
        let mut crates = Vec::with_capacity(self.crates.len());
        for item in &self.crates {
            let absolute_root_module = snapshot_root
                .map(|root| root.join(&item.root_module))
                .map(|path| {
                    path.into_os_string().into_string().map_err(|_| {
                        ProjectModelError::new(
                            "provider-model-root-invalid",
                            "linked-project root module is not valid UTF-8",
                        )
                    })
                })
                .transpose()?;
            let mut dependencies = Vec::with_capacity(item.dependencies.len());
            for dependency in &item.dependencies {
                let Some(crate_index) = crate_indices.get(dependency.crate_id.as_str()) else {
                    return project_model_error(
                        "provider-model-dependency-invalid",
                        "project model dependency is missing from the canonical crate order",
                    );
                };
                dependencies.push(serde_json::json!({
                    "crate": crate_index,
                    "name": dependency.name,
                }));
            }
            crates.push(serde_json::json!({
                "root_module": absolute_root_module.as_deref().unwrap_or(&item.root_module),
                "edition": item.edition,
                "deps": dependencies,
                "cfg": self.cfg,
                "env": self.env,
                "target": self.target_triple,
                "is_workspace_member": true,
                "source": null,
            }));
        }
        Ok(serde_json::json!({
            "sysroot_src": null,
            "crates": crates,
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportedCandidateBinding {
    pub source: ReviewSource,
    pub scope_fingerprint: String,
    pub candidate_digest: String,
    pub snapshot_sha256: String,
    pub snapshot_files: usize,
    pub snapshot_bytes: u64,
    pub project_model_digest: String,
}

impl From<&CandidateBinding> for ReportedCandidateBinding {
    fn from(value: &CandidateBinding) -> Self {
        Self {
            source: value.source,
            scope_fingerprint: value.scope_fingerprint.clone(),
            candidate_digest: value.candidate_digest.clone(),
            snapshot_sha256: value.snapshot_sha256.clone(),
            snapshot_files: value.snapshot_files,
            snapshot_bytes: value.snapshot_bytes,
            project_model_digest: value.project_model_digest.clone(),
        }
    }
}

impl ReportedCandidateBinding {
    fn validate(&self) -> Result<(), ContractError> {
        validate_scope_fingerprint(&self.scope_fingerprint)?;
        validate_sha256(&self.candidate_digest, "candidate digest")?;
        validate_sha256(&self.snapshot_sha256, "snapshot digest")?;
        validate_sha256(&self.project_model_digest, "project-model digest")?;
        if self.snapshot_files == 0 || self.snapshot_bytes > MAX_SOURCE_BYTES as u64 {
            return contract_error(
                "provider-report-candidate-invalid",
                "reported candidate counts are outside the contract bounds",
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderExecutionRecord {
    pub kind: String,
    pub version: String,
    pub profile_sha256: String,
    pub executable_sha256: String,
    pub configuration_sha256: String,
    pub target_triple: String,
    pub toolchain_mode: String,
    pub project_model_algorithm: String,
    pub negotiated_encoding: Option<PositionEncoding>,
}

impl ProviderExecutionRecord {
    fn validate(&self) -> Result<(), ContractError> {
        validate_text(&self.kind, MAX_KIND_BYTES, "provider kind")?;
        if self.kind != "rust-analyzer" {
            return contract_error(
                "provider-report-provider-invalid",
                "reported provider kind must equal rust-analyzer",
            );
        }
        validate_text(&self.version, MAX_VERSION_BYTES, "provider version")?;
        validate_sha256(&self.profile_sha256, "profile digest")?;
        validate_sha256(&self.executable_sha256, "executable digest")?;
        validate_sha256(&self.configuration_sha256, "configuration digest")?;
        validate_target(&self.target_triple)?;
        if self.project_model_algorithm != "rust-analyzer-linked-project-v1" {
            return contract_error(
                "provider-report-model-algorithm-invalid",
                "reported project-model algorithm is not recognized",
            );
        }
        if self.toolchain_mode != "none" {
            return contract_error(
                "provider-report-toolchain-invalid",
                "reported provider toolchain mode must equal none",
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextSymbol {
    pub symbol_id: String,
    pub path: String,
    pub kind: SeedKind,
    pub name: String,
    pub symbol_range: ProviderRange,
    pub selection_range: ProviderRange,
}

impl ContextSymbol {
    fn validate(&self) -> Result<(), ContractError> {
        validate_sha256(&self.symbol_id, "provider symbol ID")?;
        validate_snapshot_relative_path(&self.path, "provider symbol path")?;
        validate_text(&self.name, MAX_NAME_BYTES, "provider symbol name")?;
        self.symbol_range.validate()?;
        self.selection_range.validate()?;
        if !self.symbol_range.contains(&self.selection_range) {
            return contract_error(
                "provider-symbol-selection-invalid",
                "provider symbol selection must be contained by its symbol range",
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedContextSymbol {
    pub changed_symbol_id: String,
    pub symbol: ContextSymbol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticCallEdge {
    pub edge_id: String,
    pub from_symbol: String,
    pub to_symbol: String,
    pub call_site_path: String,
    pub call_site_range: ProviderRange,
    pub kind: String,
    pub resolution: String,
    pub confidence: String,
    pub provider_id: String,
    pub provider_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderLimitation {
    pub code: String,
    pub message: String,
    pub changed_symbol_id: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderIsolation {
    pub network: ProviderNetworkIsolation,
    pub shell_enabled: bool,
    pub original_repository_access: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderMetrics {
    pub requests: usize,
    pub messages: usize,
    pub notifications: usize,
    pub server_requests: usize,
    pub invalid_messages: usize,
    pub call_ranges: usize,
    pub protocol_bytes: usize,
    pub stderr_bytes: usize,
    pub source_bytes: usize,
    pub nodes: usize,
    pub edges: usize,
    pub report_bytes: usize,
    pub elapsed_ms: u64,
    pub process_tree_peak_rss_bytes: u64,
    pub process_tree_sample_interval_ms: u64,
    pub process_tree_accounting: ResourceAccountingStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryContextProviderReport {
    pub schema_version: u8,
    pub kind: String,
    pub candidate: ReportedCandidateBinding,
    pub provider: ProviderExecutionRecord,
    pub status: RepositoryContextProviderStatus,
    pub index_completeness: ProviderCompleteness,
    pub query_completeness: ProviderCompleteness,
    pub seed_symbols: Vec<SeedContextSymbol>,
    pub related_symbols: Vec<ContextSymbol>,
    pub edges: Vec<SemanticCallEdge>,
    pub limitations: Vec<ProviderLimitation>,
    pub isolation: ProviderIsolation,
    pub metrics: ProviderMetrics,
}

impl RepositoryContextProviderReport {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema_version != 1 {
            return contract_error(
                "provider-report-schema-invalid",
                "report schema_version must equal 1",
            );
        }
        if self.kind != "repository_context_provider_report" {
            return contract_error(
                "provider-report-kind-invalid",
                "report kind is not recognized",
            );
        }
        self.candidate.validate()?;
        self.provider.validate()?;
        if self.index_completeness != ProviderCompleteness::Unknown {
            return contract_error(
                "provider-report-index-completeness-invalid",
                "provider index completeness must be unknown in this contract version",
            );
        }
        let facts_empty = self.seed_symbols.is_empty()
            && self.related_symbols.is_empty()
            && self.edges.is_empty();
        match self.status {
            RepositoryContextProviderStatus::Completed
                if self.query_completeness == ProviderCompleteness::Complete => {}
            RepositoryContextProviderStatus::Partial
                if self.query_completeness == ProviderCompleteness::Partial => {}
            RepositoryContextProviderStatus::Unavailable
            | RepositoryContextProviderStatus::Timeout
            | RepositoryContextProviderStatus::InvalidOutput
            | RepositoryContextProviderStatus::Failed
                if self.query_completeness == ProviderCompleteness::Unavailable && facts_empty => {}
            _ => {
                return contract_error(
                    "provider-report-status-invalid",
                    "report status, completeness, and retained facts are inconsistent",
                );
            }
        }
        if self.seed_symbols.len() > MAX_SEEDS
            || self.seed_symbols.len() + self.related_symbols.len() > MAX_NODES
            || self.edges.len() > MAX_EDGES
            || self.limitations.len() > MAX_LIMITATIONS
        {
            return contract_error(
                "provider-report-facts-unbounded",
                "report fact arrays exceed the contract maxima",
            );
        }
        validate_sorted_unique_by(
            &self.seed_symbols,
            |left, right| left.symbol.symbol_id.cmp(&right.symbol.symbol_id),
            "provider-report-seeds-order-invalid",
            "report seed symbols must be sorted by unique symbol ID",
        )?;
        validate_sorted_unique_by(
            &self.related_symbols,
            |left, right| left.symbol_id.cmp(&right.symbol_id),
            "provider-report-related-order-invalid",
            "report related symbols must be sorted by unique symbol ID",
        )?;
        validate_sorted_unique_by(
            &self.edges,
            |left, right| left.edge_id.cmp(&right.edge_id),
            "provider-report-edges-order-invalid",
            "report edges must be sorted by unique edge ID",
        )?;
        validate_sorted_unique_by(
            &self.limitations,
            |left, right| left.cmp(right),
            "provider-report-limitations-order-invalid",
            "report limitations must be sorted and unique",
        )?;

        let mut symbol_ids = BTreeSet::new();
        let mut symbols = BTreeMap::new();
        let mut changed_ids = BTreeSet::new();
        for seed in &self.seed_symbols {
            validate_sha256(&seed.changed_symbol_id, "changed symbol ID")?;
            seed.symbol.validate()?;
            if !changed_ids.insert(seed.changed_symbol_id.as_str())
                || !symbol_ids.insert(seed.symbol.symbol_id.as_str())
            {
                return contract_error(
                    "provider-report-seed-duplicate",
                    "report seed mappings must have unique changed and provider symbol IDs",
                );
            }
            symbols.insert(seed.symbol.symbol_id.as_str(), &seed.symbol);
        }
        for symbol in &self.related_symbols {
            symbol.validate()?;
            if !symbol_ids.insert(symbol.symbol_id.as_str()) {
                return contract_error(
                    "provider-report-symbol-overlap",
                    "seed and related provider symbol IDs must be disjoint",
                );
            }
            symbols.insert(symbol.symbol_id.as_str(), symbol);
        }
        for edge in &self.edges {
            validate_sha256(&edge.edge_id, "provider edge ID")?;
            validate_sha256(&edge.from_symbol, "edge source symbol ID")?;
            validate_sha256(&edge.to_symbol, "edge target symbol ID")?;
            validate_snapshot_relative_path(&edge.call_site_path, "call-site path")?;
            edge.call_site_range.validate()?;
            if edge.kind != "calls" || edge.resolution != "semantic" || edge.confidence != "high" {
                return contract_error(
                    "provider-report-edge-semantics-invalid",
                    "provider call edges must use calls/semantic/high semantics",
                );
            }
            if edge.provider_id != self.provider.kind
                || edge.provider_version != self.provider.version
            {
                return contract_error(
                    "provider-report-edge-provider-invalid",
                    "provider call edge provenance does not match the execution record",
                );
            }
            let from = symbols.get(edge.from_symbol.as_str()).ok_or_else(|| {
                ContractError::new(
                    "provider-report-edge-endpoint-missing",
                    "provider call edge source does not exist in the report",
                )
            })?;
            if !symbols.contains_key(edge.to_symbol.as_str()) {
                return contract_error(
                    "provider-report-edge-endpoint-missing",
                    "provider call edge target does not exist in the report",
                );
            }
            if edge.call_site_path != from.path {
                return contract_error(
                    "provider-report-edge-path-invalid",
                    "provider call edge path must match its source symbol path",
                );
            }
        }
        for limitation in &self.limitations {
            validate_identifier(&limitation.code, "limitation code")?;
            validate_text(
                &limitation.message,
                MAX_LIMITATION_BYTES,
                "limitation message",
            )?;
            if let Some(changed_symbol_id) = limitation.changed_symbol_id.as_deref() {
                validate_sha256(changed_symbol_id, "limitation changed symbol ID")?;
                if !changed_ids.contains(changed_symbol_id) {
                    return contract_error(
                        "provider-report-limitation-reference-invalid",
                        "provider limitation references an unknown changed symbol ID",
                    );
                }
            }
            if let Some(path) = limitation.path.as_deref() {
                validate_snapshot_relative_path(path, "limitation path")?;
            }
        }
        if self.isolation.shell_enabled || self.isolation.original_repository_access {
            return contract_error(
                "provider-report-isolation-invalid",
                "provider report cannot claim shell or original repository access",
            );
        }
        self.metrics.validate()?;
        if matches!(
            self.status,
            RepositoryContextProviderStatus::Completed | RepositoryContextProviderStatus::Partial
        ) && (self.metrics.process_tree_accounting != ResourceAccountingStatus::Available
            || self.metrics.process_tree_peak_rss_bytes > PRODUCTION_PROCESS_TREE_RSS_LIMIT_BYTES)
        {
            return contract_error(
                "provider-report-resource-accounting-invalid",
                "completed or partial provider reports require in-limit process-tree RSS accounting",
            );
        }
        if self.metrics.nodes != symbol_ids.len()
            || self.metrics.edges != self.edges.len()
            || self.metrics.call_ranges != self.edges.len()
        {
            return contract_error(
                "provider-report-metrics-invalid",
                "provider report fact metrics do not match retained facts",
            );
        }
        let encoded_bytes = serde_json::to_vec(self)
            .map_err(|_| {
                ContractError::new(
                    "provider-report-serialization-failed",
                    "report serialization failed",
                )
            })?
            .len();
        if encoded_bytes > MAX_REPORT_BYTES {
            return contract_error(
                "provider-report-bytes-exceeded",
                "encoded provider report exceeds the contract byte maximum",
            );
        }
        Ok(())
    }
}

impl ProviderMetrics {
    fn validate(&self) -> Result<(), ContractError> {
        validate_metric(self.requests, MAX_REQUESTS, "requests")?;
        validate_metric(self.messages, MAX_MESSAGES, "messages")?;
        validate_metric(self.notifications, MAX_NOTIFICATIONS, "notifications")?;
        validate_metric(self.server_requests, MAX_SERVER_REQUESTS, "server_requests")?;
        validate_metric(
            self.invalid_messages,
            MAX_INVALID_MESSAGES,
            "invalid_messages",
        )?;
        validate_metric(self.call_ranges, MAX_CALL_RANGES, "call_ranges")?;
        validate_metric(self.protocol_bytes, MAX_PROTOCOL_BYTES, "protocol_bytes")?;
        validate_metric(self.stderr_bytes, MAX_STDERR_BYTES, "stderr_bytes")?;
        validate_metric(self.source_bytes, MAX_SOURCE_BYTES, "source_bytes")?;
        validate_metric(self.nodes, MAX_NODES, "nodes")?;
        validate_metric(self.edges, MAX_EDGES, "edges")?;
        validate_metric(self.report_bytes, MAX_REPORT_BYTES, "report_bytes")?;
        if self.elapsed_ms > MAX_DEADLINE_MS {
            return contract_error(
                "provider-report-metric-unbounded",
                "provider elapsed_ms exceeds the contract maximum",
            );
        }
        if self.process_tree_peak_rss_bytes
            > PRODUCTION_PROCESS_TREE_RSS_LIMIT_BYTES.saturating_add(1)
        {
            return contract_error(
                "provider-report-metric-unbounded",
                "provider process_tree_peak_rss_bytes exceeds the contract maximum",
            );
        }
        if self.process_tree_sample_interval_ms == 0
            || self.process_tree_sample_interval_ms > MAX_RESOURCE_SAMPLE_INTERVAL_MS
        {
            return contract_error(
                "provider-report-metric-unbounded",
                "provider process_tree_sample_interval_ms is outside the contract bounds",
            );
        }
        Ok(())
    }
}

pub fn report_symbol_id(
    binding_digest: &str,
    path: &str,
    kind: SeedKind,
    name: &str,
    symbol_range: &ProviderRange,
    selection_range: &ProviderRange,
) -> Result<String, ContractError> {
    validate_sha256(binding_digest, "binding digest")?;
    validate_snapshot_relative_path(path, "provider symbol path")?;
    validate_text(name, MAX_NAME_BYTES, "provider symbol name")?;
    symbol_range.validate()?;
    selection_range.validate()?;
    if !symbol_range.contains(selection_range) {
        return contract_error(
            "provider-symbol-selection-invalid",
            "provider symbol selection must be contained by its symbol range",
        );
    }
    let mut digest = LengthPrefixedDigest::new("repository-context-symbol-v1");
    digest.push(binding_digest.as_bytes());
    digest.push(path.as_bytes());
    digest.push(kind.as_str().as_bytes());
    digest.push(name.as_bytes());
    push_range(&mut digest, symbol_range);
    push_range(&mut digest, selection_range);
    Ok(digest.finish())
}

pub fn report_edge_id(
    binding_digest: &str,
    from_symbol: &str,
    to_symbol: &str,
    call_site_path: &str,
    call_site_range: &ProviderRange,
) -> Result<String, ContractError> {
    validate_sha256(binding_digest, "binding digest")?;
    validate_sha256(from_symbol, "edge source symbol ID")?;
    validate_sha256(to_symbol, "edge target symbol ID")?;
    validate_snapshot_relative_path(call_site_path, "call-site path")?;
    call_site_range.validate()?;
    let mut digest = LengthPrefixedDigest::new("repository-context-edge-v1");
    digest.push(binding_digest.as_bytes());
    digest.push(from_symbol.as_bytes());
    digest.push(to_symbol.as_bytes());
    digest.push(call_site_path.as_bytes());
    push_range(&mut digest, call_site_range);
    Ok(digest.finish())
}

fn push_range(digest: &mut LengthPrefixedDigest, range: &ProviderRange) {
    digest.push(range.format.as_str().as_bytes());
    digest.push(&range.start_line.to_be_bytes());
    digest.push(&range.start_column.to_be_bytes());
    digest.push(&range.end_line.to_be_bytes());
    digest.push(&range.end_column.to_be_bytes());
    digest.push(&(range.start_byte as u64).to_be_bytes());
    digest.push(&(range.end_byte as u64).to_be_bytes());
}

struct LengthPrefixedDigest(Sha256);

impl LengthPrefixedDigest {
    fn new(domain: &str) -> Self {
        let mut value = Self(Sha256::new());
        value.push(domain.as_bytes());
        value
    }

    fn push(&mut self, bytes: &[u8]) {
        self.0.update((bytes.len() as u64).to_be_bytes());
        self.0.update(bytes);
    }

    fn finish(self) -> String {
        format!("{:x}", self.0.finalize())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError {
    pub code: &'static str,
    message: String,
}

impl ContractError {
    fn new(code: &'static str, message: impl AsRef<str>) -> Self {
        Self {
            code,
            message: bounded_error_message(message.as_ref()),
        }
    }
}

impl std::fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ContractError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileError {
    pub code: &'static str,
    message: String,
}

impl ProfileError {
    fn new(code: &'static str, message: impl AsRef<str>) -> Self {
        Self {
            code,
            message: bounded_error_message(message.as_ref()),
        }
    }
}

impl From<ContractError> for ProfileError {
    fn from(value: ContractError) -> Self {
        Self::new(value.code, value.message)
    }
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProfileError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectModelError {
    pub code: &'static str,
    message: String,
}

impl ProjectModelError {
    fn new(code: &'static str, message: impl AsRef<str>) -> Self {
        Self {
            code,
            message: bounded_error_message(message.as_ref()),
        }
    }
}

impl From<ContractError> for ProjectModelError {
    fn from(value: ContractError) -> Self {
        Self::new(value.code, value.message)
    }
}

impl std::fmt::Display for ProjectModelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProjectModelError {}

fn contract_error<T>(code: &'static str, message: &'static str) -> Result<T, ContractError> {
    Err(ContractError::new(code, message))
}

fn profile_error<T>(code: &'static str, message: &'static str) -> Result<T, ProfileError> {
    Err(ProfileError::new(code, message))
}

fn project_model_error<T>(
    code: &'static str,
    message: &'static str,
) -> Result<T, ProjectModelError> {
    Err(ProjectModelError::new(code, message))
}

fn bounded_error_message(message: &str) -> String {
    message.chars().take(384).collect()
}

fn validate_limit<T>(value: T, maximum: T, name: &'static str) -> Result<(), ContractError>
where
    T: Copy + Ord + From<u8>,
{
    if value < T::from(1) || value > maximum {
        return Err(ContractError::new(
            "provider-limit-invalid",
            format!("{name} must be positive and cannot exceed its immutable maximum"),
        ));
    }
    Ok(())
}

fn validate_metric(value: usize, maximum: usize, name: &'static str) -> Result<(), ContractError> {
    if value > maximum {
        return Err(ContractError::new(
            "provider-report-metric-unbounded",
            format!("provider metric {name} exceeds the contract maximum"),
        ));
    }
    Ok(())
}

pub(crate) fn validate_sha256(value: &str, name: &'static str) -> Result<(), ContractError> {
    if value.len() != 64
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(ContractError::new(
            "provider-digest-invalid",
            format!("{name} must be exactly 64 lower-case hexadecimal characters"),
        ));
    }
    Ok(())
}

fn validate_scope_fingerprint(value: &str) -> Result<(), ContractError> {
    if !matches!(value.len(), 40 | 64)
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(ContractError::new(
            "provider-scope-fingerprint-invalid",
            "scope fingerprint must be 40 or 64 lower-case hexadecimal characters",
        ));
    }
    Ok(())
}

pub(crate) fn validate_text(
    value: &str,
    maximum: usize,
    name: &'static str,
) -> Result<(), ContractError> {
    if value.is_empty() || value.len() > maximum || value.contains(['\0', '\r', '\n']) {
        return Err(ContractError::new(
            "provider-text-invalid",
            format!("{name} must be non-empty, single-line, and bounded"),
        ));
    }
    Ok(())
}

fn validate_identifier(value: &str, name: &'static str) -> Result<(), ContractError> {
    validate_text(value, MAX_ID_BYTES, name)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(ContractError::new(
            "provider-identifier-invalid",
            format!("{name} contains unsupported characters"),
        ));
    }
    Ok(())
}

pub(crate) fn validate_target(value: &str) -> Result<(), ContractError> {
    validate_text(value, MAX_TARGET_BYTES, "target triple")?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return contract_error(
            "provider-target-invalid",
            "target triple contains unsupported characters",
        );
    }
    Ok(())
}

pub(crate) fn validate_absolute_path(path: &Path, name: &'static str) -> Result<(), ContractError> {
    let Some(value) = path.to_str() else {
        return contract_error(
            "provider-path-invalid",
            "provider paths must be valid UTF-8",
        );
    };
    if !path.is_absolute() || value.len() > MAX_PATH_BYTES {
        return Err(ContractError::new(
            "provider-path-invalid",
            format!("{name} must be an absolute bounded path"),
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(ContractError::new(
            "provider-path-invalid",
            format!("{name} must be lexically normalized"),
        ));
    }
    Ok(())
}

fn validate_snapshot_relative_path(value: &str, name: &'static str) -> Result<(), ContractError> {
    validate_text(value, MAX_PATH_BYTES, name)?;
    let path = Path::new(value);
    if path.is_absolute()
        || value.contains('\\')
        || value.contains("//")
        || value.ends_with('/')
        || value.contains(':')
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ContractError::new(
            "provider-relative-path-invalid",
            format!("{name} must be a normalized snapshot-relative path"),
        ));
    }
    Ok(())
}

fn validate_sorted_unique_by<T>(
    values: &[T],
    compare: impl Fn(&T, &T) -> std::cmp::Ordering,
    code: &'static str,
    message: &'static str,
) -> Result<(), ContractError> {
    if values
        .windows(2)
        .any(|window| compare(&window[0], &window[1]) != std::cmp::Ordering::Less)
    {
        return contract_error(code, message);
    }
    Ok(())
}

fn validate_sorted_unique_text(
    values: &[String],
    maximum_items: usize,
    maximum_bytes: usize,
    name: &'static str,
) -> Result<(), ContractError> {
    if values.len() > maximum_items {
        return Err(ContractError::new(
            "provider-array-unbounded",
            format!("{name} exceeds the item maximum"),
        ));
    }
    validate_sorted_unique_by(
        values,
        |left, right| left.cmp(right),
        "provider-array-order-invalid",
        "string arrays must be sorted and unique",
    )?;
    for value in values {
        validate_text(value, maximum_bytes, name)?;
    }
    Ok(())
}

pub(crate) fn sha256_json(value: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(value).expect("typed provider contracts always serialize");
    format!("{:x}", Sha256::digest(bytes))
}
