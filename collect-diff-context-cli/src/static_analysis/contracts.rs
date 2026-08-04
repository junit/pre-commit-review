use crate::review_scope::ReviewSource;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError {
    message: String,
}

impl ContractError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ContractError {}

fn fingerprint_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^[0-9a-f]{40}([0-9a-f]{24})?$").unwrap())
}

fn sha256_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^[0-9a-f]{64}$").unwrap())
}

fn compact_id_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^[0-9a-f]{16}$").unwrap())
}

fn profile_id_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^[a-z0-9][a-z0-9._-]{0,63}$").unwrap())
}

fn require_string(value: &str, label: &str, maximum: usize) -> Result<(), ContractError> {
    if value.is_empty() || value.contains('\0') || value.chars().count() > maximum {
        return Err(ContractError::new(format!(
            "{label} must be a non-empty string of at most {maximum} characters"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolIdentity {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
}

impl ToolIdentity {
    fn validate_input(&self) -> Result<(), ContractError> {
        if self.name.is_empty() {
            return Err(ContractError::new("tool.name must be a non-empty string"));
        }
        Ok(())
    }

    fn validate_profile(&self) -> Result<(), ContractError> {
        require_string(&self.name, "tool.name", 200)?;
        let version = self
            .version
            .as_deref()
            .ok_or_else(|| ContractError::new("tool.version is required"))?;
        require_string(version, "tool.version", 100)
    }

    fn validate_evidence(&self) -> Result<(), ContractError> {
        if self.name.is_empty() {
            return Err(ContractError::new("tool.name must be a non-empty string"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportStatus {
    Completed,
    Failed,
    Timeout,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    Error,
    Warning,
    Note,
    None,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingCategory {
    Security,
    Privacy,
    Build,
    Correctness,
    Data,
    Compatibility,
    Reliability,
    Performance,
    Maintainability,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    VeryHigh,
    High,
    Medium,
    Low,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BaselineState {
    New,
    Existing,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputFinding {
    pub rule_id: String,
    pub message: String,
    pub path: String,
    #[serde(default)]
    pub start_line: Option<u32>,
    #[serde(default)]
    pub end_line: Option<u32>,
    pub severity: Severity,
    pub category: FindingCategory,
    pub confidence: Confidence,
    #[serde(default = "default_baseline_state")]
    pub baseline_state: BaselineState,
}

fn default_baseline_state() -> BaselineState {
    BaselineState::Unknown
}

impl InputFinding {
    fn validate(&self) -> Result<(), ContractError> {
        require_string(&self.rule_id, "finding.rule_id", usize::MAX)?;
        require_string(&self.message, "finding.message", usize::MAX)?;
        require_string(&self.path, "finding.path", usize::MAX)?;
        if self.start_line == Some(0) {
            return Err(ContractError::new(
                "finding.start_line must be a positive integer or null",
            ));
        }
        if self.end_line == Some(0) {
            return Err(ContractError::new(
                "finding.end_line must be a positive integer or null",
            ));
        }
        if let (Some(start), Some(end)) = (self.start_line, self.end_line) {
            if end < start {
                return Err(ContractError::new(
                    "finding.end_line cannot precede finding.start_line",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticAnalysisInput {
    pub schema_version: u8,
    pub kind: String,
    pub scope_fingerprint: String,
    pub tool: ToolIdentity,
    pub status: ReportStatus,
    pub findings: Vec<InputFinding>,
}

impl StaticAnalysisInput {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema_version != 1 {
            return Err(ContractError::new("schema_version must be 1"));
        }
        if self.kind != "static_analysis_input" {
            return Err(ContractError::new("kind must be static_analysis_input"));
        }
        if !fingerprint_regex().is_match(&self.scope_fingerprint) {
            return Err(ContractError::new(
                "scope_fingerprint must be 40 or 64 lowercase hexadecimal characters",
            ));
        }
        self.tool.validate_input()?;
        for finding in &self.findings {
            finding.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutableAuthorization {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputFormat {
    Sarif,
    NormalizedJson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileLimits {
    pub timeout_seconds: u64,
    pub max_output_bytes: usize,
    pub max_snapshot_bytes: u64,
    pub max_snapshot_files: usize,
}

impl ProfileLimits {
    fn validate(&self) -> Result<(), ContractError> {
        if !(1..=600).contains(&self.timeout_seconds) {
            return Err(ContractError::new(
                "limits.timeout_seconds must be between 1 and 600",
            ));
        }
        if !(1_024..=10_000_000).contains(&self.max_output_bytes) {
            return Err(ContractError::new(
                "limits.max_output_bytes must be between 1024 and 10000000",
            ));
        }
        if !(1_048_576..=2_147_483_648).contains(&self.max_snapshot_bytes) {
            return Err(ContractError::new(
                "limits.max_snapshot_bytes must be between 1048576 and 2147483648",
            ));
        }
        if !(1..=200_000).contains(&self.max_snapshot_files) {
            return Err(ContractError::new(
                "limits.max_snapshot_files must be between 1 and 200000",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepositoryConfiguration {
    Disabled,
    ExplicitlyTrusted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkAccess {
    #[serde(rename = "offline-required")]
    OfflineRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticAnalysisProfile {
    pub schema_version: u8,
    pub kind: String,
    pub name: String,
    pub tool: ToolIdentity,
    pub executable: ExecutableAuthorization,
    pub arguments: Vec<String>,
    pub output_format: OutputFormat,
    pub success_exit_codes: Vec<i32>,
    pub limits: ProfileLimits,
    pub repository_configuration: RepositoryConfiguration,
    pub network_access: NetworkAccess,
}

impl StaticAnalysisProfile {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema_version != 1 {
            return Err(ContractError::new("profile schema_version must be 1"));
        }
        if self.kind != "static_analysis_profile" {
            return Err(ContractError::new(
                "profile kind must be static_analysis_profile",
            ));
        }
        require_string(&self.name, "profile.name", 200)?;
        self.tool.validate_profile()?;
        require_string(&self.executable.path, "profile.executable.path", 4096)?;
        if !sha256_regex().is_match(&self.executable.sha256) {
            return Err(ContractError::new(
                "profile.executable.sha256 must be 64 lowercase hexadecimal characters",
            ));
        }
        if self.arguments.len() > 128 {
            return Err(ContractError::new(
                "profile.arguments must contain at most 128 strings",
            ));
        }
        for (index, argument) in self.arguments.iter().enumerate() {
            if argument.contains('\0') || argument.chars().count() > 4096 {
                return Err(ContractError::new(format!(
                    "profile.arguments[{index}] must contain at most 4096 characters and no NUL"
                )));
            }
        }
        if self.success_exit_codes.is_empty() || self.success_exit_codes.len() > 16 {
            return Err(ContractError::new(
                "profile.success_exit_codes must contain 1 to 16 values",
            ));
        }
        let mut unique = HashSet::new();
        for exit_code in &self.success_exit_codes {
            if !(0..=255).contains(exit_code) || !unique.insert(*exit_code) {
                return Err(ContractError::new(
                    "profile.success_exit_codes must be unique values between 0 and 255",
                ));
            }
        }
        self.limits.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceTrust {
    ExplicitInput,
    ControlledExecution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceScopeBinding {
    Embedded,
    ExplicitAssertion,
    ControlledExecution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReport {
    pub report_id: String,
    pub format: OutputFormat,
    pub tool: ToolIdentity,
    pub status: ReportStatus,
    pub trust: EvidenceTrust,
    pub scope_binding: EvidenceScopeBinding,
    pub execution_id: Option<String>,
    pub finding_count: usize,
}

impl EvidenceReport {
    pub fn validate(&self) -> Result<(), ContractError> {
        if !compact_id_regex().is_match(&self.report_id) {
            return Err(ContractError::new(
                "report_id must be 16 lowercase hexadecimal characters",
            ));
        }
        self.tool.validate_evidence()?;
        match self.trust {
            EvidenceTrust::ControlledExecution => {
                if self.scope_binding != EvidenceScopeBinding::ControlledExecution {
                    return Err(ContractError::new(
                        "controlled execution report must use controlled scope binding",
                    ));
                }
                let execution_id = self.execution_id.as_deref().ok_or_else(|| {
                    ContractError::new("controlled execution report requires execution_id")
                })?;
                if !compact_id_regex().is_match(execution_id) {
                    return Err(ContractError::new(
                        "execution_id must be 16 lowercase hexadecimal characters",
                    ));
                }
            }
            EvidenceTrust::ExplicitInput => {
                if self.execution_id.is_some()
                    || self.scope_binding == EvidenceScopeBinding::ControlledExecution
                {
                    return Err(ContractError::new(
                        "explicit input report cannot claim controlled execution provenance",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceScope {
    pub source: ReviewSource,
    pub head: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceCounts {
    pub reports: usize,
    pub input_findings: usize,
    pub deduplicated_findings: usize,
    pub mapped_to_units: usize,
    pub added_line: usize,
    pub blocking_candidates: usize,
    pub priority_candidates: usize,
    pub notes: usize,
    pub outside_scope: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LineScope {
    Added,
    Unchanged,
    OutsideScope,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingDisposition {
    BlockingCandidate,
    PriorityCandidate,
    Note,
    OutsideScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceFinding {
    pub finding_id: String,
    pub report_ids: Vec<String>,
    pub tool: ToolIdentity,
    pub rule_id: String,
    pub message: String,
    pub path: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub severity: Severity,
    pub category: FindingCategory,
    pub confidence: Confidence,
    pub baseline_state: BaselineState,
    pub manifest_unit_id: Option<String>,
    pub line_scope: LineScope,
    pub disposition: FindingDisposition,
    pub blocking_candidate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionContract {
    pub blocking: String,
    pub non_blocking: String,
    pub verification: String,
    pub finalization: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticAnalysisEvidence {
    pub schema_version: u8,
    pub kind: String,
    pub authoritative: bool,
    pub scope: EvidenceScope,
    pub reports: Vec<EvidenceReport>,
    pub counts: EvidenceCounts,
    pub findings: Vec<EvidenceFinding>,
    pub truncated: bool,
    pub decision_contract: DecisionContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionProfileRecord {
    pub profile_id: String,
    pub sha256: String,
    pub name: String,
    pub output_format: OutputFormat,
    pub success_exit_codes: Vec<i32>,
    pub limits: ProfileLimits,
    pub repository_configuration: RepositoryConfiguration,
    pub network_access: NetworkAccess,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutableRecord {
    pub name: String,
    pub sha256: String,
    pub path_policy: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRecord {
    pub kind: String,
    pub sha256: String,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IsolationRecord {
    pub shell: bool,
    pub vcs_metadata: bool,
    pub environment: String,
    pub source_tree: String,
    pub original_repository_path: String,
    pub network: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionStatus {
    Completed,
    Failed,
    Timeout,
    OutputLimit,
    InvalidOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureReason {
    NonSuccessExit,
    Timeout,
    OutputLimit,
    InvalidOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRecord {
    pub status: ExecutionStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout_bytes: usize,
    pub stdout_sha256: String,
    pub stderr_bytes: usize,
    pub stderr_sha256: String,
    pub result_accepted: bool,
    pub failure_reason: Option<FailureReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionEvidenceLinks {
    pub report_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticAnalysisExecution {
    pub schema_version: u8,
    pub kind: String,
    pub authoritative: bool,
    pub execution_id: String,
    pub scope: EvidenceScope,
    pub profile: ExecutionProfileRecord,
    pub tool: ToolIdentity,
    pub executable: ExecutableRecord,
    pub snapshot: SnapshotRecord,
    pub isolation: IsolationRecord,
    pub execution: ExecutionRecord,
    pub evidence: ExecutionEvidenceLinks,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestProfileRef {
    pub profile_id: String,
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationLimits {
    pub max_execution_seconds: u64,
    pub max_captured_output_bytes: u64,
    pub max_findings: usize,
    pub max_snapshot_bytes: u64,
    pub max_snapshot_files: usize,
}

impl OrchestrationLimits {
    fn validate(&self) -> Result<(), ContractError> {
        if !(1..=1_800).contains(&self.max_execution_seconds) {
            return Err(ContractError::new(
                "limits.max_execution_seconds must be between 1 and 1800",
            ));
        }
        if !(1_024..=100_000_000).contains(&self.max_captured_output_bytes) {
            return Err(ContractError::new(
                "limits.max_captured_output_bytes must be between 1024 and 100000000",
            ));
        }
        if !(1..=5_000).contains(&self.max_findings) {
            return Err(ContractError::new(
                "limits.max_findings must be between 1 and 5000",
            ));
        }
        if !(1_048_576..=2_147_483_648).contains(&self.max_snapshot_bytes) {
            return Err(ContractError::new(
                "limits.max_snapshot_bytes must be between 1048576 and 2147483648",
            ));
        }
        if !(1..=200_000).contains(&self.max_snapshot_files) {
            return Err(ContractError::new(
                "limits.max_snapshot_files must be between 1 and 200000",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationManifest {
    pub schema_version: u8,
    pub kind: String,
    pub name: String,
    pub profiles: Vec<ManifestProfileRef>,
    pub limits: OrchestrationLimits,
}

impl OrchestrationManifest {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema_version != 1 {
            return Err(ContractError::new("manifest schema_version must be 1"));
        }
        if self.kind != "static_analysis_orchestration_manifest" {
            return Err(ContractError::new(
                "manifest kind must be static_analysis_orchestration_manifest",
            ));
        }
        require_string(&self.name, "manifest.name", 200)?;
        if !(1..=16).contains(&self.profiles.len()) {
            return Err(ContractError::new(
                "manifest.profiles must contain 1 to 16 profiles",
            ));
        }
        let mut profile_ids = HashSet::new();
        let mut path_hash_pairs = HashSet::new();
        for (index, profile) in self.profiles.iter().enumerate() {
            if !profile_id_regex().is_match(&profile.profile_id) {
                return Err(ContractError::new(format!(
                    "manifest.profiles[{index}].profile_id is invalid"
                )));
            }
            if !profile_ids.insert(profile.profile_id.as_str()) {
                return Err(ContractError::new(
                    "manifest profile_id values must be unique",
                ));
            }
            require_string(
                &profile.path,
                &format!("manifest.profiles[{index}].path"),
                4096,
            )?;
            if !Path::new(&profile.path).is_absolute() {
                return Err(ContractError::new(format!(
                    "manifest.profiles[{index}].path must be absolute"
                )));
            }
            if !sha256_regex().is_match(&profile.sha256) {
                return Err(ContractError::new(format!(
                    "manifest.profiles[{index}].sha256 must be 64 lowercase hexadecimal characters"
                )));
            }
            if !path_hash_pairs.insert((profile.path.as_str(), profile.sha256.as_str())) {
                return Err(ContractError::new(
                    "manifest profile path and SHA256 pairs must be unique",
                ));
            }
        }
        self.limits.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestIdentity {
    pub manifest_id: String,
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationSnapshot {
    pub snapshot_id: String,
    pub kind: String,
    pub sha256: String,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrchestrationStatus {
    Completed,
    Partial,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetAmount {
    pub initial: u64,
    pub consumed: u64,
    pub remaining: u64,
}

impl BudgetAmount {
    fn validate(&self, label: &str) -> Result<(), ContractError> {
        if self.consumed > self.initial
            || self.remaining > self.initial
            || self.consumed.checked_add(self.remaining) != Some(self.initial)
        {
            return Err(ContractError::new(format!(
                "budgets.{label} must satisfy initial = consumed + remaining"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetRecord {
    pub execution_millis: BudgetAmount,
    pub captured_output_bytes: BudgetAmount,
    pub findings: BudgetAmount,
    pub snapshot_files: BudgetAmount,
    pub snapshot_bytes: BudgetAmount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotRunReason {
    BudgetExhausted,
    SharedIntegrityFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InvalidationReason {
    SnapshotMutated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "run_kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum OrchestrationRun {
    Executed {
        profile_id: String,
        execution: Box<StaticAnalysisExecution>,
    },
    NotRun {
        profile_id: String,
        reason: NotRunReason,
    },
    Invalidated {
        profile_id: String,
        reason: InvalidationReason,
    },
}

impl OrchestrationRun {
    pub fn profile_id(&self) -> &str {
        match self {
            Self::Executed { profile_id, .. }
            | Self::NotRun { profile_id, .. }
            | Self::Invalidated { profile_id, .. } => profile_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationArtifact {
    pub schema_version: u8,
    pub kind: String,
    pub authoritative: bool,
    pub orchestration_id: String,
    pub scope: EvidenceScope,
    pub manifest: ManifestIdentity,
    pub snapshot: OrchestrationSnapshot,
    pub status: OrchestrationStatus,
    pub budgets: BudgetRecord,
    pub runs: Vec<OrchestrationRun>,
    pub report_ids: Vec<String>,
    pub finding_ids: Vec<String>,
}

impl OrchestrationArtifact {
    pub fn validate(&self, evidence: &StaticAnalysisEvidence) -> Result<(), ContractError> {
        if self.schema_version != 1 {
            return Err(ContractError::new("orchestration schema_version must be 1"));
        }
        if self.kind != "static_analysis_orchestration" {
            return Err(ContractError::new(
                "orchestration kind must be static_analysis_orchestration",
            ));
        }
        if !self.authoritative || !evidence.authoritative {
            return Err(ContractError::new(
                "orchestration and evidence must be authoritative",
            ));
        }
        if !compact_id_regex().is_match(&self.orchestration_id) {
            return Err(ContractError::new(
                "orchestration_id must be 16 lowercase hexadecimal characters",
            ));
        }
        validate_scope(&self.scope)?;
        if self.scope != evidence.scope {
            return Err(ContractError::new(
                "orchestration and evidence scopes must match",
            ));
        }
        self.validate_manifest_identity()?;
        self.validate_snapshot()?;
        self.validate_budgets()?;
        self.validate_runs(evidence)?;
        self.validate_evidence_links(evidence)
    }

    fn validate_manifest_identity(&self) -> Result<(), ContractError> {
        require_string(&self.manifest.name, "manifest.name", 200)?;
        if !sha256_regex().is_match(&self.manifest.sha256) {
            return Err(ContractError::new(
                "manifest.sha256 must be 64 lowercase hexadecimal characters",
            ));
        }
        if self.manifest.manifest_id != self.manifest.sha256[..16] {
            return Err(ContractError::new(
                "manifest_id must be derived from manifest.sha256",
            ));
        }
        Ok(())
    }

    fn validate_snapshot(&self) -> Result<(), ContractError> {
        if self.snapshot.kind != "temporary-tracked-files" {
            return Err(ContractError::new(
                "snapshot.kind must be temporary-tracked-files",
            ));
        }
        if !sha256_regex().is_match(&self.snapshot.sha256) {
            return Err(ContractError::new(
                "snapshot.sha256 must be 64 lowercase hexadecimal characters",
            ));
        }
        if self.snapshot.snapshot_id != self.snapshot.sha256[..16] {
            return Err(ContractError::new(
                "snapshot_id must be derived from snapshot.sha256",
            ));
        }
        Ok(())
    }

    fn validate_budgets(&self) -> Result<(), ContractError> {
        self.budgets.execution_millis.validate("execution_millis")?;
        self.budgets
            .captured_output_bytes
            .validate("captured_output_bytes")?;
        self.budgets.findings.validate("findings")?;
        self.budgets.snapshot_files.validate("snapshot_files")?;
        self.budgets.snapshot_bytes.validate("snapshot_bytes")?;
        if self.budgets.snapshot_files.consumed != self.snapshot.files as u64
            || self.budgets.snapshot_bytes.consumed != self.snapshot.bytes
        {
            return Err(ContractError::new(
                "snapshot budgets must record the shared snapshot exactly once",
            ));
        }
        Ok(())
    }

    fn validate_runs(&self, evidence: &StaticAnalysisEvidence) -> Result<(), ContractError> {
        if !(1..=16).contains(&self.runs.len()) {
            return Err(ContractError::new(
                "orchestration.runs must contain 1 to 16 entries",
            ));
        }
        let mut profile_ids = HashSet::new();
        let mut accepted = 0usize;
        let mut executed = 0usize;
        for run in &self.runs {
            let profile_id = run.profile_id();
            if !profile_id_regex().is_match(profile_id) {
                return Err(ContractError::new(
                    "run profile_id must match the manifest profile-id contract",
                ));
            }
            if !profile_ids.insert(profile_id) {
                return Err(ContractError::new(
                    "orchestration run profile_id values must be unique",
                ));
            }
            if let OrchestrationRun::Executed { execution, .. } = run {
                executed += 1;
                if execution.scope != self.scope {
                    return Err(ContractError::new(
                        "executed run scope must match the orchestration scope",
                    ));
                }
                if execution.snapshot.sha256 != self.snapshot.sha256
                    || execution.snapshot.files != self.snapshot.files
                    || execution.snapshot.bytes != self.snapshot.bytes
                {
                    return Err(ContractError::new(
                        "executed run snapshot must match the shared orchestration snapshot",
                    ));
                }
                if execution.execution.status == ExecutionStatus::Completed
                    && execution.execution.result_accepted
                {
                    accepted += 1;
                }
            }
        }
        let expected_status = if accepted == self.runs.len() {
            OrchestrationStatus::Completed
        } else if accepted > 0 {
            OrchestrationStatus::Partial
        } else {
            OrchestrationStatus::Failed
        };
        if self.status != expected_status {
            return Err(ContractError::new(
                "orchestration status is inconsistent with terminal run states",
            ));
        }
        if executed == 0 && !evidence.reports.is_empty() {
            return Err(ContractError::new(
                "orchestration without executed runs cannot emit evidence reports",
            ));
        }
        if executed > 0 && evidence.reports.is_empty() {
            return Err(ContractError::new(
                "every executed run must emit linked evidence",
            ));
        }
        Ok(())
    }

    fn validate_evidence_links(
        &self,
        evidence: &StaticAnalysisEvidence,
    ) -> Result<(), ContractError> {
        if evidence.schema_version != 1 || evidence.kind != "static_analysis_evidence" {
            return Err(ContractError::new(
                "companion evidence must use static_analysis_evidence/v1",
            ));
        }
        if evidence.counts.reports != evidence.reports.len() {
            return Err(ContractError::new(
                "evidence counts.reports must match reports length",
            ));
        }
        let evidence_report_ids = evidence
            .reports
            .iter()
            .map(|report| report.report_id.clone())
            .collect::<Vec<_>>();
        if self.report_ids != evidence_report_ids {
            return Err(ContractError::new(
                "orchestration report_ids must match companion evidence order",
            ));
        }
        let evidence_finding_ids = evidence
            .findings
            .iter()
            .map(|finding| finding.finding_id.clone())
            .collect::<Vec<_>>();
        if self.finding_ids != evidence_finding_ids {
            return Err(ContractError::new(
                "orchestration finding_ids must match companion evidence order",
            ));
        }
        let report_ids = self.report_ids.iter().collect::<HashSet<_>>();
        if report_ids.len() != self.report_ids.len()
            || self
                .report_ids
                .iter()
                .any(|report_id| !compact_id_regex().is_match(report_id))
        {
            return Err(ContractError::new(
                "orchestration report_ids must be unique compact identifiers",
            ));
        }
        let finding_ids = self.finding_ids.iter().collect::<HashSet<_>>();
        if finding_ids.len() != self.finding_ids.len()
            || self
                .finding_ids
                .iter()
                .any(|finding_id| !compact_id_regex().is_match(finding_id))
        {
            return Err(ContractError::new(
                "orchestration finding_ids must be unique compact identifiers",
            ));
        }
        let linked_report_ids = self
            .runs
            .iter()
            .filter_map(|run| match run {
                OrchestrationRun::Executed { execution, .. } => {
                    Some(&execution.evidence.report_ids)
                }
                _ => None,
            })
            .flatten()
            .collect::<HashSet<_>>();
        if linked_report_ids != report_ids {
            return Err(ContractError::new(
                "executed run report links must match orchestration report_ids",
            ));
        }
        Ok(())
    }
}

fn validate_scope(scope: &EvidenceScope) -> Result<(), ContractError> {
    require_string(&scope.head, "scope.head", 200)?;
    if !fingerprint_regex().is_match(&scope.fingerprint) {
        return Err(ContractError::new(
            "scope.fingerprint must be 40 or 64 lowercase hexadecimal characters",
        ));
    }
    Ok(())
}
