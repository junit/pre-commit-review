use crate::candidate::RepoPath;
use crate::review_scope::ReviewSource;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_PROVIDERS: usize = 16;
const MAX_UNITS: usize = 30;
const MAX_SYMBOLS: usize = 5_000;
const MAX_EDGES: usize = 500;
const MAX_SUMMARIES: usize = 1_000;
const MAX_LIMITATIONS: usize = 1_000;
const MAX_MESSAGE_CHARS: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImpactMode {
    Fast,
    Deep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImpactStatus {
    Completed,
    Partial,
    Unavailable,
    Invalidated,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderStatus {
    Completed,
    Partial,
    Unsupported,
    Timeout,
    BudgetExhausted,
    Stale,
    InvalidOutput,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParseQuality {
    Clean,
    Recovered,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    Defines,
    References,
    Imports,
    Exports,
    Calls,
    Implements,
    Overrides,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resolution {
    Syntactic,
    Lexical,
    ResolvedReference,
    Semantic,
    PolymorphicCandidate,
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnitStatus {
    Completed,
    Partial,
    Unsupported,
    BudgetExhausted,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SummaryKind {
    DependencyChange,
    InterfaceChange,
    TextQueryMatch,
    TestSelection,
    FrameworkEffect,
    ConfigurationEffect,
    AuthorizationEffect,
    StorageEffect,
    NetworkEffect,
    LifecycleEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImpactPresence {
    Present,
    Deleted,
    Gitlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Completeness {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImpactScope {
    pub fingerprint: String,
    pub source: ReviewSource,
    pub candidate_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRange {
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRecord {
    pub provider_id: String,
    pub provider_kind: String,
    pub provider_version: String,
    pub configuration_digest: String,
    pub status: ProviderStatus,
    pub elapsed_ms: u64,
    pub input_files: usize,
    pub input_bytes: u64,
    pub output_fact_count: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub cache_stale: usize,
    pub cache_corrupt: usize,
    pub limitation_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImpactUnit {
    pub manifest_unit_id: String,
    pub path: String,
    pub language: String,
    pub content_sha256: Option<String>,
    pub content_bytes: Option<usize>,
    pub presence: ImpactPresence,
    pub syntax_eligible: bool,
    pub syntax_status: UnitStatus,
    pub text_status: UnitStatus,
    pub parse_quality: Option<ParseQuality>,
    pub provider_ids: Vec<String>,
    pub changed_ranges: Vec<SourceRange>,
    pub error_node_count: usize,
    pub missing_node_count: usize,
    pub parse_affected_ranges: Vec<SourceRange>,
    pub parse_affected_symbol_ids: Vec<String>,
    pub changed_symbol_ids: Vec<String>,
    pub limitation_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangedSymbol {
    pub symbol_id: String,
    pub provider_id: String,
    pub path: String,
    pub language: String,
    pub kind: String,
    pub name: String,
    pub owner: Option<String>,
    pub signature: Option<String>,
    pub visibility: Option<String>,
    pub range: SourceRange,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImpactEdge {
    pub edge_id: String,
    pub kind: EdgeKind,
    pub from_symbol: String,
    pub to_symbol: Option<String>,
    pub unresolved_target: Option<String>,
    pub path: String,
    pub range: SourceRange,
    pub provider_id: String,
    pub resolution: Resolution,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainSummary {
    pub summary_id: String,
    pub summary_kind: SummaryKind,
    pub path: String,
    pub symbol_id: Option<String>,
    pub confidence: Confidence,
    pub message: String,
    pub evidence_fact_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImpactCoverage {
    pub total_candidate_files: usize,
    pub changed_candidate_files: usize,
    pub syntax_eligible_files: usize,
    pub parsed_files: usize,
    pub clean_parse_files: usize,
    pub recovered_parse_files: usize,
    pub degraded_parse_files: usize,
    pub unsupported_files: usize,
    pub resource_limited_files: usize,
    pub unavailable_files: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub cache_stale: usize,
    pub cache_corrupt: usize,
    pub requested_graph_depth: usize,
    pub reached_graph_depth: usize,
    pub graph_index_completeness: Completeness,
    pub graph_query_completeness: Completeness,
    pub output_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limitation {
    pub limitation_id: String,
    pub code: String,
    pub provider_id: Option<String>,
    pub path: Option<String>,
    pub symbol_id: Option<String>,
    pub reason: String,
    pub interpretation: String,
    pub improvable_in_deep_mode: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImpactMetrics {
    pub elapsed_ms: u64,
    pub candidate_input_files: usize,
    pub candidate_input_bytes: u64,
    pub nodes_visited: usize,
    pub max_nesting_depth: usize,
    pub facts_emitted: usize,
    pub edges_emitted: usize,
    pub summaries_emitted: usize,
    pub output_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImpactContext {
    pub schema_version: u8,
    pub kind: String,
    pub scope: ImpactScope,
    pub mode: ImpactMode,
    pub status: ImpactStatus,
    pub providers: Vec<ProviderRecord>,
    pub units: Vec<ImpactUnit>,
    pub changed_symbols: Vec<ChangedSymbol>,
    pub impact_edges: Vec<ImpactEdge>,
    pub domain_summaries: Vec<DomainSummary>,
    pub coverage: ImpactCoverage,
    pub limitations: Vec<Limitation>,
    pub metrics: ImpactMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpactContractError {
    message: String,
}

impl ImpactContractError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ImpactContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ImpactContractError {}

impl ImpactContext {
    pub fn validate(&self) -> Result<(), ImpactContractError> {
        if self.schema_version != 1 {
            return invalid("schema_version must equal 1");
        }
        if self.kind != "impact_context" {
            return invalid("kind must equal impact_context");
        }
        validate_hex(&self.scope.fingerprint, &[40, 64], "scope fingerprint")?;
        validate_hex(&self.scope.candidate_digest, &[64], "candidate digest")?;
        validate_maximum(self.providers.len(), MAX_PROVIDERS, "providers")?;
        validate_maximum(self.units.len(), MAX_UNITS, "units")?;
        validate_maximum(self.changed_symbols.len(), MAX_SYMBOLS, "changed_symbols")?;
        validate_maximum(self.impact_edges.len(), MAX_EDGES, "impact_edges")?;
        validate_maximum(
            self.domain_summaries.len(),
            MAX_SUMMARIES,
            "domain_summaries",
        )?;
        validate_maximum(self.limitations.len(), MAX_LIMITATIONS, "limitations")?;

        validate_sorted_unique(
            self.providers
                .iter()
                .map(|record| record.provider_id.as_str()),
            "provider ids",
        )?;
        validate_sorted_unique(
            self.changed_symbols
                .iter()
                .map(|symbol| symbol.symbol_id.as_str()),
            "symbol ids",
        )?;
        validate_sorted_unique(
            self.impact_edges.iter().map(|edge| edge.edge_id.as_str()),
            "edge ids",
        )?;
        validate_sorted_unique(
            self.domain_summaries
                .iter()
                .map(|summary| summary.summary_id.as_str()),
            "summary ids",
        )?;
        validate_sorted_unique(
            self.limitations
                .iter()
                .map(|limitation| limitation.limitation_id.as_str()),
            "limitation ids",
        )?;

        let providers = self
            .providers
            .iter()
            .map(|record| (record.provider_id.as_str(), record))
            .collect::<BTreeMap<_, _>>();
        let symbols = self
            .changed_symbols
            .iter()
            .map(|symbol| (symbol.symbol_id.as_str(), symbol))
            .collect::<BTreeMap<_, _>>();
        let limitations = self
            .limitations
            .iter()
            .map(|limitation| (limitation.limitation_id.as_str(), limitation))
            .collect::<BTreeMap<_, _>>();
        let units = self
            .units
            .iter()
            .map(|unit| (unit.path.as_str(), unit))
            .collect::<BTreeMap<_, _>>();
        if units.len() != self.units.len() {
            return invalid("unit paths must be unique");
        }
        let manifest_ids = self
            .units
            .iter()
            .map(|unit| unit.manifest_unit_id.as_str())
            .collect::<BTreeSet<_>>();
        if manifest_ids.len() != self.units.len() {
            return invalid("manifest unit ids must be unique");
        }

        for provider in &self.providers {
            validate_id(&provider.provider_id, "provider id")?;
            validate_hex(
                &provider.configuration_digest,
                &[64],
                "provider configuration digest",
            )?;
            validate_bounded_text(&provider.provider_kind, 100, "provider kind")?;
            validate_bounded_text(&provider.provider_version, 100, "provider version")?;
            validate_id_references(
                &provider.limitation_ids,
                &limitations,
                "provider limitation ids",
            )?;
        }

        for unit in &self.units {
            validate_path(&unit.path)?;
            validate_bounded_text(&unit.language, 100, "unit language")?;
            match unit.presence {
                ImpactPresence::Present => {
                    let sha256 = unit.content_sha256.as_deref().ok_or_else(|| {
                        ImpactContractError::new("present unit is missing content_sha256")
                    })?;
                    validate_hex(sha256, &[64], "unit content SHA256")?;
                    if unit.content_bytes.is_none() {
                        return invalid("present unit is missing content_bytes");
                    }
                }
                ImpactPresence::Deleted | ImpactPresence::Gitlink => {
                    if unit.content_sha256.is_some() || unit.content_bytes.is_some() {
                        return invalid("deleted and gitlink units cannot carry content bytes");
                    }
                }
            }
            validate_id_references(&unit.provider_ids, &providers, "unit provider ids")?;
            validate_id_references(
                &unit.changed_symbol_ids,
                &symbols,
                "unit changed symbol ids",
            )?;
            validate_id_references(
                &unit.parse_affected_symbol_ids,
                &symbols,
                "unit parse affected symbol ids",
            )?;
            validate_id_references(&unit.limitation_ids, &limitations, "unit limitation ids")?;
            for range in unit
                .changed_ranges
                .iter()
                .chain(unit.parse_affected_ranges.iter())
            {
                range.validate(unit.content_bytes)?;
            }
            for symbol_id in &unit.changed_symbol_ids {
                if symbols[symbol_id.as_str()].path != unit.path {
                    return invalid("unit references a changed symbol from another path");
                }
            }
        }

        for symbol in &self.changed_symbols {
            validate_id(&symbol.symbol_id, "symbol id")?;
            validate_path(&symbol.path)?;
            validate_bounded_text(&symbol.language, 100, "symbol language")?;
            validate_bounded_text(&symbol.kind, 100, "symbol kind")?;
            validate_bounded_text(&symbol.name, MAX_MESSAGE_CHARS, "symbol name")?;
            validate_optional_text(symbol.owner.as_deref(), MAX_MESSAGE_CHARS, "symbol owner")?;
            validate_optional_text(
                symbol.signature.as_deref(),
                MAX_MESSAGE_CHARS,
                "symbol signature",
            )?;
            validate_optional_text(symbol.visibility.as_deref(), 100, "symbol visibility")?;
            if !providers.contains_key(symbol.provider_id.as_str()) {
                return invalid("symbol references an unknown provider");
            }
            let unit = units
                .get(symbol.path.as_str())
                .ok_or_else(|| ImpactContractError::new("symbol path has no impact unit"))?;
            symbol.range.validate(unit.content_bytes)?;
        }

        for edge in &self.impact_edges {
            validate_id(&edge.edge_id, "edge id")?;
            validate_path(&edge.path)?;
            let provider = providers
                .get(edge.provider_id.as_str())
                .ok_or_else(|| ImpactContractError::new("edge references an unknown provider"))?;
            let unit = units
                .get(edge.path.as_str())
                .ok_or_else(|| ImpactContractError::new("edge path has no impact unit"))?;
            edge.range.validate(unit.content_bytes)?;
            match (&edge.to_symbol, &edge.unresolved_target) {
                (None, None) => return invalid("edge must carry a symbol or unresolved target"),
                (Some(_), Some(_)) => {
                    return invalid("edge cannot carry both symbol and unresolved target")
                }
                (Some(symbol_id), None) => {
                    validate_id(symbol_id, "edge target symbol id")?;
                    if !symbols.contains_key(symbol_id.as_str()) {
                        return invalid("edge references an unknown target symbol");
                    }
                }
                (None, Some(target)) => {
                    validate_bounded_text(target, MAX_MESSAGE_CHARS, "unresolved target")?;
                }
            }
            if provider.provider_kind == "text-adapter" {
                return invalid("text-adapter cannot emit symbol edges");
            }
            if provider.provider_kind == "tree-sitter-rust"
                && matches!(
                    edge.resolution,
                    Resolution::ResolvedReference
                        | Resolution::Semantic
                        | Resolution::PolymorphicCandidate
                )
            {
                return invalid("tree-sitter-rust cannot claim resolved semantics");
            }
        }

        for summary in &self.domain_summaries {
            validate_id(&summary.summary_id, "summary id")?;
            validate_path(&summary.path)?;
            if !units.contains_key(summary.path.as_str()) {
                return invalid("summary path has no impact unit");
            }
            if let Some(symbol_id) = &summary.symbol_id {
                validate_id(symbol_id, "summary symbol id")?;
                if !symbols.contains_key(symbol_id.as_str()) {
                    return invalid("summary references an unknown symbol");
                }
            }
            validate_bounded_text(&summary.message, MAX_MESSAGE_CHARS, "summary message")?;
            validate_ids(&summary.evidence_fact_ids, "summary evidence fact ids")?;
        }

        for limitation in &self.limitations {
            validate_id(&limitation.limitation_id, "limitation id")?;
            validate_bounded_text(&limitation.code, 100, "limitation code")?;
            validate_bounded_text(&limitation.reason, MAX_MESSAGE_CHARS, "limitation reason")?;
            validate_bounded_text(
                &limitation.interpretation,
                MAX_MESSAGE_CHARS,
                "limitation interpretation",
            )?;
            if let Some(provider_id) = &limitation.provider_id {
                if !providers.contains_key(provider_id.as_str()) {
                    return invalid("limitation references an unknown provider");
                }
            }
            if let Some(path) = &limitation.path {
                validate_path(path)?;
                if !units.contains_key(path.as_str()) {
                    return invalid("limitation path has no impact unit");
                }
            }
            if let Some(symbol_id) = &limitation.symbol_id {
                if !symbols.contains_key(symbol_id.as_str()) {
                    return invalid("limitation references an unknown symbol");
                }
            }
        }

        self.coverage.validate(self.units.len())?;
        let output_truncation = self
            .limitations
            .iter()
            .any(|limitation| limitation.code == "output-truncated");
        if self.coverage.output_truncated != output_truncation {
            return invalid("output truncation coverage and limitation disagree");
        }
        if !self.coverage.output_truncated {
            if self.metrics.edges_emitted != self.impact_edges.len() {
                return invalid("metrics.edges_emitted does not match impact_edges");
            }
            if self.metrics.summaries_emitted != self.domain_summaries.len() {
                return invalid("metrics.summaries_emitted does not match domain_summaries");
            }
        }
        Ok(())
    }
}

impl SourceRange {
    fn validate(&self, content_bytes: Option<usize>) -> Result<(), ImpactContractError> {
        if self.start_line == 0
            || self.start_column == 0
            || self.end_line == 0
            || self.end_column == 0
        {
            return invalid("source range lines and columns must be one-based");
        }
        if (self.end_line, self.end_column) < (self.start_line, self.start_column) {
            return invalid("source range end precedes start");
        }
        if self.end_byte < self.start_byte {
            return invalid("source byte range end precedes start");
        }
        if content_bytes.is_some_and(|bytes| self.end_byte > bytes) {
            return invalid("source byte range exceeds candidate content");
        }
        Ok(())
    }
}

impl ImpactCoverage {
    fn validate(&self, emitted_units: usize) -> Result<(), ImpactContractError> {
        if self.changed_candidate_files > self.total_candidate_files {
            return invalid("changed candidate files exceed total candidate files");
        }
        if emitted_units != self.changed_candidate_files {
            return invalid("coverage changed candidate files do not match units");
        }
        if self.syntax_eligible_files > self.changed_candidate_files
            || self.parsed_files > self.syntax_eligible_files
        {
            return invalid("syntax coverage exceeds changed or eligible files");
        }
        let parse_quality_total = self
            .clean_parse_files
            .checked_add(self.recovered_parse_files)
            .and_then(|count| count.checked_add(self.degraded_parse_files))
            .ok_or_else(|| ImpactContractError::new("parse coverage arithmetic overflow"))?;
        if parse_quality_total != self.parsed_files {
            return invalid("parse quality counts must partition parsed files");
        }
        let terminal_total = self
            .parsed_files
            .checked_add(self.unsupported_files)
            .and_then(|count| count.checked_add(self.resource_limited_files))
            .and_then(|count| count.checked_add(self.unavailable_files))
            .ok_or_else(|| ImpactContractError::new("coverage arithmetic overflow"))?;
        if terminal_total != self.changed_candidate_files {
            return invalid("syntax terminal counts must partition changed files");
        }
        if self.reached_graph_depth > self.requested_graph_depth {
            return invalid("reached graph depth exceeds requested graph depth");
        }
        Ok(())
    }
}

fn validate_path(path: &str) -> Result<(), ImpactContractError> {
    if path.contains('\\') {
        return invalid("repository path must use slash separators");
    }
    RepoPath::new(path.to_string())
        .map(|_| ())
        .map_err(|error| ImpactContractError::new(format!("invalid repository path: {error}")))
}

fn validate_id_references<T>(
    ids: &[String],
    available: &BTreeMap<&str, T>,
    label: &str,
) -> Result<(), ImpactContractError> {
    validate_ids(ids, label)?;
    if ids.iter().any(|id| !available.contains_key(id.as_str())) {
        return invalid(format!("{label} contain an unknown id"));
    }
    Ok(())
}

fn validate_ids(ids: &[String], label: &str) -> Result<(), ImpactContractError> {
    validate_sorted_unique(ids.iter().map(String::as_str), label)?;
    for id in ids {
        validate_id(id, label)?;
    }
    Ok(())
}

fn validate_sorted_unique<'a>(
    values: impl IntoIterator<Item = &'a str>,
    label: &str,
) -> Result<(), ImpactContractError> {
    let mut previous = None;
    for value in values {
        if previous.is_some_and(|previous| previous >= value) {
            return invalid(format!("{label} must be unique and sorted"));
        }
        previous = Some(value);
    }
    Ok(())
}

fn validate_id(value: &str, label: &str) -> Result<(), ImpactContractError> {
    validate_hex(value, &[16], label)
}

fn validate_hex(value: &str, lengths: &[usize], label: &str) -> Result<(), ImpactContractError> {
    if !lengths.contains(&value.len())
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return invalid(format!("{label} must be lowercase hexadecimal"));
    }
    Ok(())
}

fn validate_optional_text(
    value: Option<&str>,
    max_chars: usize,
    label: &str,
) -> Result<(), ImpactContractError> {
    if let Some(value) = value {
        validate_bounded_text(value, max_chars, label)?;
    }
    Ok(())
}

fn validate_bounded_text(
    value: &str,
    max_chars: usize,
    label: &str,
) -> Result<(), ImpactContractError> {
    if value.is_empty() || value.chars().count() > max_chars {
        return invalid(format!("{label} must contain 1 to {max_chars} characters"));
    }
    Ok(())
}

fn validate_maximum(
    observed: usize,
    maximum: usize,
    label: &str,
) -> Result<(), ImpactContractError> {
    if observed > maximum {
        return invalid(format!("{label} exceed the contract maximum"));
    }
    Ok(())
}

fn invalid<T>(message: impl Into<String>) -> Result<T, ImpactContractError> {
    Err(ImpactContractError::new(message))
}
