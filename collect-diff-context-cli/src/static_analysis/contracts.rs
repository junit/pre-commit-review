use crate::review_scope::ReviewSource;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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
