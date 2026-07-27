use crate::candidate::{CandidatePresence, RepoPath};
use crate::impact_context::contracts::{Completeness, UnitStatus};
use crate::review_scope::ReviewSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_MANIFEST_ENTRIES: usize = 100_000;
const MAX_LIMITATIONS: usize = 1_000;
const MAX_LIMITATION_CODES: usize = 1_000;
const MAX_TEXT_CHARS: usize = 1_000;
const MAX_IDENTIFIER_CHARS: usize = 4_096;
const MAX_LANGUAGE_CHARS: usize = 100;
const MAX_VERSION_CHARS: usize = 200;
const MAX_OUTPUT_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IndexAction {
    Build,
    Doctor,
    Inspect,
    Clean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndexReportStatus {
    Completed,
    Partial,
    Unavailable,
    Invalidated,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryLocator {
    pub source: ReviewSource,
    pub object_format: String,
    pub base_tree: Option<String>,
    pub index_manifest_digest: Option<String>,
    pub overlay_candidate_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryManifestEntry {
    pub path: RepoPath,
    pub mode: String,
    pub presence: CandidatePresence,
    pub content_sha256: Option<String>,
    pub content_bytes: Option<usize>,
    pub language: Option<String>,
    pub status: UnitStatus,
    pub limitation_codes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryManifest {
    pub locator: RepositoryLocator,
    pub digest: String,
    pub entries: Vec<RepositoryManifestEntry>,
    pub completeness: Completeness,
    pub limitations: Vec<IndexLimitation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileFactKey {
    pub language: String,
    pub content_sha256: String,
    pub grammar_version: String,
    pub query_digest: String,
    pub adapter_version: String,
    pub normalization_rules_digest: String,
    pub schema_version: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileFactsManifestEntry {
    pub path: RepoPath,
    pub presence: CandidatePresence,
    pub file_fact_key: Option<FileFactKey>,
    pub status: UnitStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphGenerationIdentity {
    pub graph_schema_version: u16,
    pub candidate_manifest_digest: String,
    pub project_model_digest: String,
    pub resolver_digest: String,
    pub adapter_query_digest: String,
    pub file_facts_manifest_digest: String,
    pub normalization_rules_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexMetrics {
    pub elapsed_ms: u64,
    pub manifest_files: usize,
    pub manifest_bytes: u64,
    pub file_fact_hits: usize,
    pub file_fact_misses: usize,
    pub file_fact_writes: usize,
    pub parsed_files: usize,
    pub parsed_bytes: u64,
    pub symbols: usize,
    pub edges: usize,
    pub query_rows: usize,
    pub generation_bytes: u64,
    pub output_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexLimitation {
    pub code: String,
    pub path: Option<RepoPath>,
    pub symbol_id: Option<String>,
    pub reason: String,
    pub interpretation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexReport {
    pub schema_version: u8,
    pub kind: String,
    pub action: IndexAction,
    pub status: IndexReportStatus,
    pub scope_fingerprint: Option<String>,
    pub repository_id: String,
    pub generation_key: Option<String>,
    pub metrics: IndexMetrics,
    pub limitations: Vec<IndexLimitation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexContractError {
    message: String,
}

impl IndexContractError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for IndexContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for IndexContractError {}

impl RepositoryLocator {
    pub fn validate(&self) -> Result<(), IndexContractError> {
        let object_id_length = match self.object_format.as_str() {
            "sha1" => 40,
            "sha256" => 64,
            _ => return invalid("object_format must be sha1 or sha256"),
        };
        if let Some(base_tree) = &self.base_tree {
            validate_hex(base_tree, object_id_length, "base_tree")?;
        }
        if let Some(index_manifest_digest) = &self.index_manifest_digest {
            validate_hex(index_manifest_digest, 64, "index_manifest_digest")?;
        }
        validate_hex(
            &self.overlay_candidate_digest,
            64,
            "overlay_candidate_digest",
        )
    }
}

impl RepositoryManifestEntry {
    fn validate(&self) -> Result<(), IndexContractError> {
        if self.mode.len() != 6 || !self.mode.bytes().all(|byte| matches!(byte, b'0'..=b'7')) {
            return invalid("manifest mode must be six octal digits");
        }
        match self.presence {
            CandidatePresence::Present => match (&self.content_sha256, self.content_bytes) {
                (Some(digest), Some(_)) => validate_hex(digest, 64, "content_sha256")?,
                (None, None) if self.status != UnitStatus::Completed => {}
                _ => {
                    return invalid(
                        "present manifest entries require paired content identity and bytes",
                    )
                }
            },
            CandidatePresence::Deleted | CandidatePresence::Gitlink => {
                if self.content_sha256.is_some() || self.content_bytes.is_some() {
                    return invalid("non-file manifest entries cannot contain content identity");
                }
            }
        }
        if let Some(language) = &self.language {
            validate_text(language, MAX_LANGUAGE_CHARS, "language")?;
        }
        validate_sorted_unique_text(
            &self.limitation_codes,
            MAX_LIMITATION_CODES,
            100,
            "limitation_codes",
        )
    }
}

impl RepositoryManifest {
    pub fn validate(&self) -> Result<(), IndexContractError> {
        self.locator.validate()?;
        validate_hex(&self.digest, 64, "manifest digest")?;
        if self.entries.len() > MAX_MANIFEST_ENTRIES {
            return invalid("manifest entries exceed 100000 items");
        }
        let mut previous_path: Option<&str> = None;
        for entry in &self.entries {
            if previous_path.is_some_and(|previous| previous >= entry.path.as_str()) {
                return invalid("manifest paths must be sorted and unique");
            }
            previous_path = Some(entry.path.as_str());
            entry.validate()?;
        }
        if self.completeness == Completeness::Complete
            && self
                .entries
                .iter()
                .any(|entry| entry.status != UnitStatus::Completed)
        {
            return invalid("complete manifests require completed entries");
        }
        validate_limitations(&self.limitations)
    }
}

impl FileFactKey {
    pub fn validate(&self) -> Result<(), IndexContractError> {
        validate_text(&self.language, MAX_LANGUAGE_CHARS, "language")?;
        validate_hex(&self.content_sha256, 64, "content_sha256")?;
        validate_text(&self.grammar_version, MAX_VERSION_CHARS, "grammar_version")?;
        validate_hex(&self.query_digest, 64, "query_digest")?;
        validate_text(&self.adapter_version, MAX_VERSION_CHARS, "adapter_version")?;
        validate_hex(
            &self.normalization_rules_digest,
            64,
            "normalization_rules_digest",
        )?;
        if self.schema_version == 0 {
            return invalid("file fact schema_version must be positive");
        }
        Ok(())
    }
}

impl FileFactsManifestEntry {
    pub fn validate(&self) -> Result<(), IndexContractError> {
        match (&self.presence, &self.file_fact_key) {
            (CandidatePresence::Present, Some(key)) => key.validate(),
            (CandidatePresence::Present, None) if self.status != UnitStatus::Completed => Ok(()),
            (CandidatePresence::Deleted | CandidatePresence::Gitlink, None) => Ok(()),
            _ => invalid("file facts manifest entry has inconsistent presence and key"),
        }
    }
}

impl GraphGenerationIdentity {
    pub fn validate(&self) -> Result<(), IndexContractError> {
        if self.graph_schema_version == 0 {
            return invalid("graph_schema_version must be positive");
        }
        for (name, digest) in [
            (
                "candidate_manifest_digest",
                self.candidate_manifest_digest.as_str(),
            ),
            ("project_model_digest", self.project_model_digest.as_str()),
            ("resolver_digest", self.resolver_digest.as_str()),
            ("adapter_query_digest", self.adapter_query_digest.as_str()),
            (
                "file_facts_manifest_digest",
                self.file_facts_manifest_digest.as_str(),
            ),
            (
                "normalization_rules_digest",
                self.normalization_rules_digest.as_str(),
            ),
        ] {
            validate_hex(digest, 64, name)?;
        }
        Ok(())
    }

    pub fn generation_key(&self) -> Result<String, IndexContractError> {
        self.validate()?;
        let mut digest = Sha256::new();
        hash_component(&mut digest, b"repository-graph-generation/v1");
        hash_component(&mut digest, &self.graph_schema_version.to_be_bytes());
        for value in [
            self.candidate_manifest_digest.as_bytes(),
            self.project_model_digest.as_bytes(),
            self.resolver_digest.as_bytes(),
            self.adapter_query_digest.as_bytes(),
            self.file_facts_manifest_digest.as_bytes(),
            self.normalization_rules_digest.as_bytes(),
        ] {
            hash_component(&mut digest, value);
        }
        Ok(format!("{:x}", digest.finalize()))
    }
}

impl IndexMetrics {
    fn validate(&self) -> Result<(), IndexContractError> {
        if self.elapsed_ms > 60_000 {
            return invalid("elapsed_ms exceeds 60000");
        }
        if self.manifest_files > MAX_MANIFEST_ENTRIES {
            return invalid("manifest_files exceeds 100000");
        }
        if self.manifest_bytes > 32 * 1024 * 1024 {
            return invalid("manifest_bytes exceeds 32 MiB");
        }
        let lookups = self
            .file_fact_hits
            .checked_add(self.file_fact_misses)
            .ok_or_else(|| IndexContractError::new("file fact lookup count overflow"))?;
        if lookups > self.manifest_files {
            return invalid("file fact lookups exceed manifest_files");
        }
        if self.file_fact_writes > self.file_fact_misses {
            return invalid("file_fact_writes exceeds file_fact_misses");
        }
        if self.parsed_files > self.file_fact_misses {
            return invalid("parsed_files exceeds file_fact_misses");
        }
        if self.parsed_bytes > 512 * 1024 * 1024 {
            return invalid("parsed_bytes exceeds 512 MiB");
        }
        if self.symbols > 1_000_000 {
            return invalid("symbols exceeds 1000000");
        }
        if self.edges > 5_000_000 {
            return invalid("edges exceeds 5000000");
        }
        if self.query_rows > 50_000 {
            return invalid("query_rows exceeds 50000");
        }
        if self.generation_bytes > 2 * 1024 * 1024 * 1024 {
            return invalid("generation_bytes exceeds 2 GiB");
        }
        if self.output_bytes > MAX_OUTPUT_BYTES {
            return invalid("output_bytes exceeds 1 MiB");
        }
        Ok(())
    }
}

impl IndexLimitation {
    fn validate(&self) -> Result<(), IndexContractError> {
        validate_text(&self.code, 100, "limitation code")?;
        if let Some(symbol_id) = &self.symbol_id {
            validate_text(symbol_id, MAX_IDENTIFIER_CHARS, "limitation symbol_id")?;
        }
        validate_text(&self.reason, MAX_TEXT_CHARS, "limitation reason")?;
        validate_text(
            &self.interpretation,
            MAX_TEXT_CHARS,
            "limitation interpretation",
        )
    }
}

impl IndexReport {
    pub fn validate(&self) -> Result<(), IndexContractError> {
        if self.schema_version != 1 {
            return invalid("schema_version must equal 1");
        }
        if self.kind != "repository_index_report" {
            return invalid("kind must equal repository_index_report");
        }
        match (&self.action, &self.scope_fingerprint) {
            (IndexAction::Build, Some(fingerprint)) => {
                validate_hex_lengths(fingerprint, &[40, 64], "scope_fingerprint")?
            }
            (IndexAction::Build, None) => {
                return invalid("build reports require scope_fingerprint")
            }
            (_, Some(fingerprint)) => {
                validate_hex_lengths(fingerprint, &[40, 64], "scope_fingerprint")?
            }
            (_, None) => {}
        }
        validate_hex(&self.repository_id, 64, "repository_id")?;
        if let Some(generation_key) = &self.generation_key {
            validate_hex(generation_key, 64, "generation_key")?;
        }
        if self.action == IndexAction::Build
            && self.status == IndexReportStatus::Completed
            && self.generation_key.is_none()
        {
            return invalid("completed build reports require generation_key");
        }
        self.metrics.validate()?;
        validate_limitations(&self.limitations)
    }
}

fn validate_limitations(limitations: &[IndexLimitation]) -> Result<(), IndexContractError> {
    if limitations.len() > MAX_LIMITATIONS {
        return invalid("limitations exceed 1000 items");
    }
    let mut previous = None;
    for limitation in limitations {
        limitation.validate()?;
        let key = (
            limitation.code.as_str(),
            limitation.path.as_ref().map(RepoPath::as_str).unwrap_or(""),
            limitation.symbol_id.as_deref().unwrap_or(""),
            limitation.reason.as_str(),
            limitation.interpretation.as_str(),
        );
        if previous.is_some_and(|previous_key| previous_key >= key) {
            return invalid("limitations must be sorted and unique");
        }
        previous = Some(key);
    }
    Ok(())
}

fn validate_sorted_unique_text(
    values: &[String],
    maximum_items: usize,
    maximum_chars: usize,
    name: &str,
) -> Result<(), IndexContractError> {
    if values.len() > maximum_items {
        return invalid(format!("{name} exceeds {maximum_items} items"));
    }
    let mut previous: Option<&str> = None;
    for value in values {
        validate_text(value, maximum_chars, name)?;
        if previous.is_some_and(|previous_value| previous_value >= value.as_str()) {
            return invalid(format!("{name} must be sorted and unique"));
        }
        previous = Some(value);
    }
    Ok(())
}

fn validate_text(value: &str, maximum_chars: usize, name: &str) -> Result<(), IndexContractError> {
    let length = value.chars().count();
    if length == 0 || length > maximum_chars || value.as_bytes().contains(&0) {
        return invalid(format!(
            "{name} must contain 1..={maximum_chars} non-NUL characters"
        ));
    }
    Ok(())
}

fn validate_hex(value: &str, length: usize, name: &str) -> Result<(), IndexContractError> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return invalid(format!(
            "{name} must contain exactly {length} lowercase hex characters"
        ));
    }
    Ok(())
}

fn validate_hex_lengths(
    value: &str,
    lengths: &[usize],
    name: &str,
) -> Result<(), IndexContractError> {
    if !lengths.contains(&value.len())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return invalid(format!(
            "{name} must contain lowercase hex with an approved length"
        ));
    }
    Ok(())
}

fn hash_component(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn invalid<T>(message: impl Into<String>) -> Result<T, IndexContractError> {
    Err(IndexContractError::new(message))
}
