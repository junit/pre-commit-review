use crate::candidate::{CandidateContent, CandidatePresence, ChangedRange, RepoPath};
use crate::impact_context::adapters::text::TextAdapter;
use crate::impact_context::adapters::tree_sitter_rust::TreeSitterRustAdapter;
use crate::impact_context::budget::{BudgetResource, BudgetTracker, ImpactBudget};
use crate::impact_context::contracts::{
    Completeness, ImpactContext, ImpactContractError, ImpactCoverage, ImpactMetrics, ImpactMode,
    ImpactPresence, ImpactScope, ImpactStatus, ImpactUnit, Limitation, ParseQuality,
    ProviderRecord, ProviderStatus, SourceRange, UnitStatus,
};
use crate::impact_context::normalizer::{normalize_unit, stable_id};
use crate::impact_context::summarizer::summarize_unit;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

const SYNTAX_PROVIDER_KIND: &str = "tree-sitter-rust";
const SYNTAX_PROVIDER_VERSION: &str = "0.24.2";
const TEXT_PROVIDER_KIND: &str = "text-adapter";
const TEXT_PROVIDER_VERSION: &str = "1";

#[derive(Debug, Clone)]
pub struct ImpactRequest {
    pub mode: ImpactMode,
    pub budget: ImpactBudget,
    pub enabled_languages: BTreeSet<String>,
    pub cache_read: bool,
    pub cache_write: bool,
    pub semantic_providers: Vec<String>,
    pub max_snippet_chars: usize,
}

impl ImpactRequest {
    pub fn fast_defaults() -> Self {
        Self {
            mode: ImpactMode::Fast,
            budget: ImpactBudget::fast_defaults(),
            enabled_languages: BTreeSet::from(["rust".to_string()]),
            cache_read: false,
            cache_write: false,
            semantic_providers: Vec::new(),
            max_snippet_chars: 1_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpactContextError {
    code: &'static str,
    message: String,
}

impl ImpactContextError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl std::fmt::Display for ImpactContextError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ImpactContextError {}

#[derive(Debug, Default)]
struct ProviderStats {
    input_files: usize,
    input_bytes: u64,
    output_facts: usize,
    completed: usize,
    partial: usize,
    unsupported: usize,
    budget_exhausted: usize,
    unavailable: usize,
    limitation_ids: Vec<String>,
}

pub fn build_impact_context(
    candidate: &dyn CandidateContent,
    request: ImpactRequest,
) -> Result<ImpactContext, ImpactContextError> {
    validate_request(&request)?;
    let started = Instant::now();
    let syntax_provider_id = stable_id(
        "impact-provider/v1",
        &[SYNTAX_PROVIDER_KIND, SYNTAX_PROVIDER_VERSION],
    );
    let text_provider_id = stable_id(
        "impact-provider/v1",
        &[TEXT_PROVIDER_KIND, TEXT_PROVIDER_VERSION],
    );
    let mut tracker = BudgetTracker::new(request.budget.clone());
    let text_configuration =
        TextAdapter::load_configuration(candidate, &mut tracker).map_err(|error| {
            ImpactContextError::new("text-configuration-invalid", error.to_string())
        })?;
    let mut limitations = BTreeMap::new();
    let mut syntax_stats = ProviderStats::default();
    let mut text_stats = ProviderStats::default();
    for code in &text_configuration.limitation_codes {
        if code.ends_with("budget-exhausted") {
            text_stats.budget_exhausted += 1;
        }
        let id = insert_limitation(
            &mut limitations,
            code,
            Some(&text_provider_id),
            None,
            None,
            "Candidate-bound text configuration was limited.",
            "Configured text evidence may be incomplete.",
            true,
        );
        text_stats.limitation_ids.push(id);
    }

    let mut candidate_input_sizes = BTreeMap::new();
    for path in [
        ".pre-commit-review/context-queries",
        ".pre-commit-review/test-hints",
    ] {
        let repo_path = RepoPath::new(path)
            .map_err(|error| ImpactContextError::new("invalid-config-path", error.to_string()))?;
        if candidate
            .files()
            .iter()
            .any(|file| file.path == repo_path && file.presence == CandidatePresence::Present)
        {
            if let Ok(content) = candidate.read(&repo_path) {
                text_stats.input_files += 1;
                text_stats.input_bytes += content.bytes.len() as u64;
                candidate_input_sizes.insert(path.to_string(), content.bytes.len());
            }
        }
    }

    let mut changed_files = candidate
        .files()
        .iter()
        .filter(|file| file.manifest_unit_id.is_some())
        .collect::<Vec<_>>();
    changed_files.sort_by(|left, right| left.path.cmp(&right.path));
    let mut units = Vec::new();
    let mut all_symbols = Vec::new();
    let mut all_edges = Vec::new();
    let mut all_summaries = Vec::new();
    let mut normalized_fact_count = 0;
    let mut nodes_visited = 0;
    let mut max_nesting_depth = 0;

    for file in changed_files {
        let file_budget = tracker.consume(BudgetResource::ChangedFiles, 1);
        let mut unit_limitation_ids = Vec::new();
        let mut syntax_output = None;
        let mut text_output = None;
        let language = detect_language(file.path.as_str()).to_string();
        let mut content_sha256 = None;
        let mut content_bytes = None;
        let mut source_bytes = None;
        let mut binary = false;
        if file.presence == CandidatePresence::Present {
            match candidate.read(&file.path) {
                Ok(content) => {
                    binary = content.binary;
                    content_sha256 = Some(content.sha256.clone());
                    content_bytes = Some(content.bytes.len());
                    candidate_input_sizes
                        .insert(file.path.as_str().to_string(), content.bytes.len());
                    source_bytes = Some(content.bytes);
                }
                Err(error) => {
                    let id = insert_limitation(
                        &mut limitations,
                        "candidate-read-unavailable",
                        None,
                        Some(file.path.as_str()),
                        None,
                        &format!("Candidate bytes could not be read: {error}"),
                        "No structural or text facts were accepted for this unit.",
                        false,
                    );
                    unit_limitation_ids.push(id);
                }
            }
        }

        let changed_ranges = source_bytes
            .as_deref()
            .map(|bytes| map_changed_ranges(bytes, &file.changed_ranges))
            .unwrap_or_else(|| map_deleted_ranges(&file.changed_ranges));
        let mut syntax_eligible = language == "rust"
            && request.enabled_languages.contains("rust")
            && file.presence == CandidatePresence::Present
            && source_bytes.is_some()
            && !binary
            && !is_generated_like(file.path.as_str())
            && !file.changed_ranges.is_empty();
        let mut syntax_status = UnitStatus::Unavailable;
        let mut text_status = UnitStatus::Unavailable;
        let mut parse_quality = None;
        let mut error_node_count = 0;
        let mut missing_node_count = 0;
        let mut parse_affected_ranges = Vec::new();

        if let Err(exhaustion) = file_budget {
            syntax_eligible = false;
            syntax_status = UnitStatus::BudgetExhausted;
            text_status = UnitStatus::BudgetExhausted;
            let id = resource_limitation(
                &mut limitations,
                exhaustion.code(),
                Some(file.path.as_str()),
            );
            unit_limitation_ids.push(id);
        } else if tracker.check_deadline().is_err() {
            syntax_eligible = false;
            syntax_status = UnitStatus::BudgetExhausted;
            text_status = UnitStatus::BudgetExhausted;
            let id = resource_limitation(
                &mut limitations,
                "deadline-exhausted",
                Some(file.path.as_str()),
            );
            unit_limitation_ids.push(id);
        } else {
            match file.presence {
                CandidatePresence::Deleted => {
                    syntax_eligible = false;
                    syntax_status = UnitStatus::Unavailable;
                    text_status = UnitStatus::Unavailable;
                    let id = insert_limitation(
                        &mut limitations,
                        "removed-structure-unavailable-in-fast-mvp",
                        None,
                        Some(file.path.as_str()),
                        None,
                        "Fast mode parses candidate-after bytes and does not guess removed symbols.",
                        "Review the deletion through the authoritative diff context.",
                        true,
                    );
                    unit_limitation_ids.push(id);
                }
                CandidatePresence::Gitlink => {
                    syntax_eligible = false;
                    syntax_status = UnitStatus::Unsupported;
                    text_status = UnitStatus::Unsupported;
                    let id = insert_limitation(
                        &mut limitations,
                        "gitlink-structure-unavailable",
                        None,
                        Some(file.path.as_str()),
                        None,
                        "Gitlink content is not materialized by fast mode.",
                        "Only the gitlink change remains visible.",
                        true,
                    );
                    unit_limitation_ids.push(id);
                }
                CandidatePresence::Present => {
                    if let Some(bytes) = source_bytes.as_deref() {
                        if binary {
                            syntax_eligible = false;
                            syntax_status = UnitStatus::Unsupported;
                            text_status = UnitStatus::Unsupported;
                            let id = insert_limitation(
                                &mut limitations,
                                "binary-structure-unavailable",
                                None,
                                Some(file.path.as_str()),
                                None,
                                "Candidate bytes contain NUL and are treated as binary.",
                                "No source structure is claimed for this unit.",
                                false,
                            );
                            unit_limitation_ids.push(id);
                        } else if is_generated_like(file.path.as_str()) {
                            syntax_eligible = false;
                            syntax_status = UnitStatus::Unsupported;
                            let id = insert_limitation(
                                &mut limitations,
                                generated_limitation_code(file.path.as_str()),
                                None,
                                Some(file.path.as_str()),
                                None,
                                "Generated, vendored, or minified-like source is retained without structural coverage credit.",
                                "Review the changed artifact and its generator or source inputs.",
                                true,
                            );
                            unit_limitation_ids.push(id);
                        } else if file.changed_ranges.is_empty() {
                            syntax_eligible = false;
                            syntax_status = UnitStatus::Unsupported;
                            let id = insert_limitation(
                                &mut limitations,
                                "mode-only-no-structural-range",
                                None,
                                Some(file.path.as_str()),
                                None,
                                "The unit has no candidate-side changed source range.",
                                "Mode-only metadata remains visible without invented symbols.",
                                false,
                            );
                            unit_limitation_ids.push(id);
                        } else if let Err(exhaustion) = tracker
                            .observe(BudgetResource::FileBytes, bytes.len())
                            .and_then(|_| tracker.consume(BudgetResource::TotalBytes, bytes.len()))
                        {
                            syntax_status = UnitStatus::BudgetExhausted;
                            text_status = UnitStatus::BudgetExhausted;
                            let id = resource_limitation(
                                &mut limitations,
                                exhaustion.code(),
                                Some(file.path.as_str()),
                            );
                            unit_limitation_ids.push(id);
                        } else {
                            if syntax_eligible {
                                syntax_stats.input_files += 1;
                                syntax_stats.input_bytes += bytes.len() as u64;
                                match TreeSitterRustAdapter::analyze(
                                    bytes,
                                    &file.changed_ranges,
                                    &mut tracker,
                                ) {
                                    Ok(output) => {
                                        nodes_visited += output.nodes_visited;
                                        max_nesting_depth =
                                            max_nesting_depth.max(output.max_nesting_depth);
                                        error_node_count = output.error_node_count;
                                        missing_node_count = output.missing_node_count;
                                        parse_affected_ranges = output.affected_ranges.clone();
                                        parse_quality = Some(output.parse_quality);
                                        let budget_limited = output
                                            .limitation_codes
                                            .iter()
                                            .any(|code| code.ends_with("budget-exhausted"));
                                        syntax_status = if budget_limited {
                                            UnitStatus::BudgetExhausted
                                        } else if output.parse_quality == ParseQuality::Clean {
                                            UnitStatus::Completed
                                        } else {
                                            UnitStatus::Partial
                                        };
                                        for code in &output.limitation_codes {
                                            let id = insert_limitation(
                                                &mut limitations,
                                                code,
                                                Some(&syntax_provider_id),
                                                Some(file.path.as_str()),
                                                None,
                                                "Rust syntax extraction reported a bounded limitation.",
                                                "Structural confidence or completeness is reduced.",
                                                true,
                                            );
                                            unit_limitation_ids.push(id.clone());
                                            syntax_stats.limitation_ids.push(id);
                                        }
                                        syntax_output = Some(output);
                                    }
                                    Err(error) => {
                                        syntax_status = UnitStatus::Unavailable;
                                        let id = insert_limitation(
                                            &mut limitations,
                                            "tree-sitter-rust-unavailable",
                                            Some(&syntax_provider_id),
                                            Some(file.path.as_str()),
                                            None,
                                            &error.to_string(),
                                            "No Rust structural facts were accepted.",
                                            true,
                                        );
                                        unit_limitation_ids.push(id.clone());
                                        syntax_stats.limitation_ids.push(id);
                                    }
                                }
                            } else if syntax_status == UnitStatus::Unavailable {
                                syntax_status = UnitStatus::Unsupported;
                                let id = insert_limitation(
                                    &mut limitations,
                                    "unsupported-language",
                                    Some(&syntax_provider_id),
                                    Some(file.path.as_str()),
                                    None,
                                    "No built-in syntax grammar is enabled for this changed unit.",
                                    "Text evidence may still be available without structural equivalence.",
                                    true,
                                );
                                unit_limitation_ids.push(id.clone());
                                syntax_stats.limitation_ids.push(id);
                            }

                            text_stats.input_files += 1;
                            text_stats.input_bytes += bytes.len() as u64;
                            let output = TextAdapter::scan(
                                &file.path,
                                bytes,
                                false,
                                &text_configuration,
                                &mut tracker,
                            );
                            text_status = output.status;
                            for code in &output.limitation_codes {
                                let id = insert_limitation(
                                    &mut limitations,
                                    code,
                                    Some(&text_provider_id),
                                    Some(file.path.as_str()),
                                    None,
                                    "Text extraction reported a bounded limitation.",
                                    "Text evidence may be incomplete.",
                                    true,
                                );
                                unit_limitation_ids.push(id.clone());
                                text_stats.limitation_ids.push(id);
                            }
                            text_output = Some(output);
                        }
                    }
                }
            }
        }

        let mut normalized = normalize_unit(
            file.path.as_str(),
            &language,
            &syntax_provider_id,
            &text_provider_id,
            syntax_output.as_ref(),
            text_output.as_ref(),
        );
        for fact in &mut normalized.facts {
            fact.text = bounded(&fact.text, request.max_snippet_chars);
        }
        let source_text = source_bytes
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok());
        let summaries = summarize_unit(&normalized, source_text);
        syntax_stats.output_facts += normalized
            .changed_symbols
            .iter()
            .filter(|symbol| symbol.provider_id == syntax_provider_id)
            .count()
            + normalized
                .facts
                .iter()
                .filter(|fact| fact.provider_id == syntax_provider_id)
                .count();
        text_stats.output_facts += normalized
            .facts
            .iter()
            .filter(|fact| fact.provider_id == text_provider_id)
            .count();
        update_provider_terminal(&mut syntax_stats, syntax_status);
        update_provider_terminal(&mut text_stats, text_status);
        normalized_fact_count += normalized.facts.len();
        all_symbols.extend(normalized.changed_symbols.iter().cloned());
        all_edges.extend(normalized.impact_edges.iter().cloned());
        all_summaries.extend(summaries);

        unit_limitation_ids.sort();
        unit_limitation_ids.dedup();
        let mut provider_ids = Vec::new();
        if syntax_eligible || syntax_status != UnitStatus::Unsupported {
            provider_ids.push(syntax_provider_id.clone());
        }
        if source_bytes.is_some() && !binary {
            provider_ids.push(text_provider_id.clone());
        }
        provider_ids.sort();
        provider_ids.dedup();
        let changed_symbol_ids = normalized
            .changed_symbols
            .iter()
            .map(|symbol| symbol.symbol_id.clone())
            .collect::<Vec<_>>();
        let mut parse_affected_symbol_ids = normalized
            .changed_symbols
            .iter()
            .filter(|symbol| {
                parse_affected_ranges
                    .iter()
                    .any(|range| ranges_overlap(&symbol.range, range))
            })
            .map(|symbol| symbol.symbol_id.clone())
            .collect::<Vec<_>>();
        parse_affected_symbol_ids.sort();
        units.push(ImpactUnit {
            manifest_unit_id: file.manifest_unit_id.clone().unwrap_or_default(),
            path: file.path.as_str().to_string(),
            language,
            content_sha256,
            content_bytes,
            presence: impact_presence(file.presence),
            syntax_eligible,
            syntax_status,
            text_status,
            parse_quality,
            provider_ids,
            changed_ranges,
            error_node_count,
            missing_node_count,
            parse_affected_ranges,
            parse_affected_symbol_ids,
            changed_symbol_ids,
            limitation_ids: unit_limitation_ids,
        });
    }

    all_symbols.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
    all_symbols.dedup_by(|left, right| left.symbol_id == right.symbol_id);
    all_edges.sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
    all_edges.dedup_by(|left, right| left.edge_id == right.edge_id);
    all_summaries.sort_by(|left, right| left.summary_id.cmp(&right.summary_id));
    all_summaries.dedup_by(|left, right| left.summary_id == right.summary_id);
    units.sort_by(|left, right| left.path.cmp(&right.path));

    syntax_stats.limitation_ids.sort();
    syntax_stats.limitation_ids.dedup();
    text_stats.limitation_ids.sort();
    text_stats.limitation_ids.dedup();
    let provider_elapsed_ms = started.elapsed().as_millis() as u64;
    let mut providers = vec![
        provider_record(
            &syntax_provider_id,
            SYNTAX_PROVIDER_KIND,
            SYNTAX_PROVIDER_VERSION,
            &syntax_stats,
            provider_elapsed_ms,
        ),
        provider_record(
            &text_provider_id,
            TEXT_PROVIDER_KIND,
            TEXT_PROVIDER_VERSION,
            &text_stats,
            provider_elapsed_ms,
        ),
    ];
    providers.sort_by(|left, right| left.provider_id.cmp(&right.provider_id));

    let coverage = build_coverage(&units);
    let usable = !all_symbols.is_empty()
        || !all_edges.is_empty()
        || !all_summaries.is_empty()
        || normalized_fact_count > 0;
    let all_complete = units.iter().all(|unit| {
        unit.syntax_status == UnitStatus::Completed && unit.text_status == UnitStatus::Completed
    });
    let providers_complete = providers
        .iter()
        .all(|provider| provider.status == ProviderStatus::Completed);
    let status = if usable && all_complete && providers_complete {
        ImpactStatus::Completed
    } else if usable {
        ImpactStatus::Partial
    } else {
        ImpactStatus::Unavailable
    };
    let candidate_input_bytes = candidate_input_sizes.values().sum::<usize>() as u64;
    let mut context = ImpactContext {
        schema_version: 1,
        kind: "impact_context".to_string(),
        scope: ImpactScope {
            fingerprint: candidate.scope_fingerprint().to_string(),
            source: candidate.source(),
            candidate_digest: candidate.candidate_digest().to_string(),
        },
        mode: request.mode,
        status,
        providers,
        units,
        changed_symbols: all_symbols,
        impact_edges: all_edges,
        domain_summaries: all_summaries,
        coverage,
        limitations: limitations.into_values().collect(),
        metrics: ImpactMetrics {
            elapsed_ms: started.elapsed().as_millis() as u64,
            candidate_input_files: candidate_input_sizes.len(),
            candidate_input_bytes,
            nodes_visited,
            max_nesting_depth,
            facts_emitted: normalized_fact_count,
            edges_emitted: 0,
            summaries_emitted: 0,
            output_bytes: 0,
        },
    };
    context
        .limitations
        .sort_by(|left, right| left.limitation_id.cmp(&right.limitation_id));
    context.metrics.edges_emitted = context.impact_edges.len();
    context.metrics.summaries_emitted = context.domain_summaries.len();
    apply_presentation_budget(&mut context, request.budget.max_output_bytes);
    update_output_bytes(&mut context);
    context
        .validate()
        .map_err(contract_error_to_context_error)?;
    Ok(context)
}

fn validate_request(request: &ImpactRequest) -> Result<(), ImpactContextError> {
    if request.mode != ImpactMode::Fast {
        return Err(ImpactContextError::new(
            "deep-mode-unavailable",
            "Subproject A supports only fast mode",
        ));
    }
    if request.cache_write {
        return Err(ImpactContextError::new(
            "cache-write-forbidden",
            "fast mode cannot write persistent cache state",
        ));
    }
    if !request.semantic_providers.is_empty() {
        return Err(ImpactContextError::new(
            "semantic-provider-unavailable",
            "fast mode cannot execute semantic providers",
        ));
    }
    if request.max_snippet_chars == 0 || request.max_snippet_chars > 1_000 {
        return Err(ImpactContextError::new(
            "invalid-snippet-limit",
            "max_snippet_chars must be between 1 and 1000",
        ));
    }
    Ok(())
}

fn provider_record(
    provider_id: &str,
    kind: &str,
    version: &str,
    stats: &ProviderStats,
    elapsed_ms: u64,
) -> ProviderRecord {
    ProviderRecord {
        provider_id: provider_id.to_string(),
        provider_kind: kind.to_string(),
        provider_version: version.to_string(),
        configuration_digest: sha256_hex(&format!("{kind}\0{version}\0fast-mvp")),
        status: provider_status(stats),
        elapsed_ms,
        input_files: stats.input_files,
        input_bytes: stats.input_bytes,
        output_fact_count: stats.output_facts,
        cache_hits: 0,
        cache_misses: 0,
        cache_stale: 0,
        cache_corrupt: 0,
        limitation_ids: stats.limitation_ids.clone(),
    }
}

fn provider_status(stats: &ProviderStats) -> ProviderStatus {
    if stats.budget_exhausted > 0 {
        ProviderStatus::BudgetExhausted
    } else if stats.partial > 0 || stats.unavailable > 0 {
        ProviderStatus::Partial
    } else if stats.completed > 0 {
        ProviderStatus::Completed
    } else if stats.unsupported > 0 {
        ProviderStatus::Unsupported
    } else {
        ProviderStatus::Unavailable
    }
}

fn update_provider_terminal(stats: &mut ProviderStats, status: UnitStatus) {
    match status {
        UnitStatus::Completed => stats.completed += 1,
        UnitStatus::Partial => stats.partial += 1,
        UnitStatus::Unsupported => stats.unsupported += 1,
        UnitStatus::BudgetExhausted => stats.budget_exhausted += 1,
        UnitStatus::Unavailable => stats.unavailable += 1,
    }
}

fn build_coverage(units: &[ImpactUnit]) -> ImpactCoverage {
    let parsed = units
        .iter()
        .filter(|unit| {
            matches!(
                unit.syntax_status,
                UnitStatus::Completed | UnitStatus::Partial
            )
        })
        .count();
    ImpactCoverage {
        total_candidate_files: units.len(),
        changed_candidate_files: units.len(),
        syntax_eligible_files: units.iter().filter(|unit| unit.syntax_eligible).count(),
        parsed_files: parsed,
        clean_parse_files: units
            .iter()
            .filter(|unit| {
                matches!(
                    unit.syntax_status,
                    UnitStatus::Completed | UnitStatus::Partial
                ) && unit.parse_quality == Some(ParseQuality::Clean)
            })
            .count(),
        recovered_parse_files: units
            .iter()
            .filter(|unit| {
                matches!(
                    unit.syntax_status,
                    UnitStatus::Completed | UnitStatus::Partial
                ) && unit.parse_quality == Some(ParseQuality::Recovered)
            })
            .count(),
        degraded_parse_files: units
            .iter()
            .filter(|unit| {
                matches!(
                    unit.syntax_status,
                    UnitStatus::Completed | UnitStatus::Partial
                ) && unit.parse_quality == Some(ParseQuality::Degraded)
            })
            .count(),
        unsupported_files: units
            .iter()
            .filter(|unit| unit.syntax_status == UnitStatus::Unsupported)
            .count(),
        resource_limited_files: units
            .iter()
            .filter(|unit| unit.syntax_status == UnitStatus::BudgetExhausted)
            .count(),
        unavailable_files: units
            .iter()
            .filter(|unit| unit.syntax_status == UnitStatus::Unavailable)
            .count(),
        cache_hits: 0,
        cache_misses: 0,
        cache_stale: 0,
        cache_corrupt: 0,
        requested_graph_depth: 0,
        reached_graph_depth: 0,
        graph_index_completeness: Completeness::Unavailable,
        graph_query_completeness: Completeness::Unavailable,
        output_truncated: false,
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_limitation(
    limitations: &mut BTreeMap<String, Limitation>,
    code: &str,
    provider_id: Option<&str>,
    path: Option<&str>,
    symbol_id: Option<&str>,
    reason: &str,
    interpretation: &str,
    improvable_in_deep_mode: bool,
) -> String {
    let limitation_id = stable_id(
        "impact-limitation/v1",
        &[
            code,
            provider_id.unwrap_or(""),
            path.unwrap_or(""),
            symbol_id.unwrap_or(""),
        ],
    );
    limitations
        .entry(limitation_id.clone())
        .or_insert_with(|| Limitation {
            limitation_id: limitation_id.clone(),
            code: bounded(reason_code(code), 100),
            provider_id: provider_id.map(str::to_string),
            path: path.map(str::to_string),
            symbol_id: symbol_id.map(str::to_string),
            reason: bounded(reason, 1_000),
            interpretation: bounded(interpretation, 1_000),
            improvable_in_deep_mode,
        });
    limitation_id
}

fn resource_limitation(
    limitations: &mut BTreeMap<String, Limitation>,
    code: &str,
    path: Option<&str>,
) -> String {
    insert_limitation(
        limitations,
        code,
        None,
        path,
        None,
        "A fast-path resource budget was exhausted.",
        "Earlier accepted facts remain valid; this unit or later stages may be incomplete.",
        true,
    )
}

fn apply_presentation_budget(context: &mut ImpactContext, maximum: usize) {
    update_output_bytes(context);
    if context.metrics.output_bytes <= maximum {
        return;
    }
    context.coverage.output_truncated = true;
    context.status = if context.status == ImpactStatus::Unavailable {
        ImpactStatus::Unavailable
    } else {
        ImpactStatus::Partial
    };
    let limitation = Limitation {
        limitation_id: stable_id("impact-limitation/v1", &["output-truncated", "", "", ""]),
        code: "output-truncated".to_string(),
        provider_id: None,
        path: None,
        symbol_id: None,
        reason: "Presentation output exceeded the configured byte budget.".to_string(),
        interpretation: "Lower-ranked context was omitted; unit visibility is retained."
            .to_string(),
        improvable_in_deep_mode: false,
    };
    context.limitations.push(limitation);
    context
        .limitations
        .sort_by(|left, right| left.limitation_id.cmp(&right.limitation_id));
    while serialized_len(context) > maximum && !context.impact_edges.is_empty() {
        context.impact_edges.pop();
    }
    while serialized_len(context) > maximum && !context.domain_summaries.is_empty() {
        let index = context
            .domain_summaries
            .iter()
            .enumerate()
            .max_by_key(|(_, summary)| summary_priority(summary.summary_kind))
            .map(|(index, _)| index)
            .unwrap_or(0);
        context.domain_summaries.remove(index);
    }
    while serialized_len(context) > maximum && !context.changed_symbols.is_empty() {
        let removed = context.changed_symbols.pop().unwrap();
        for unit in &mut context.units {
            unit.changed_symbol_ids
                .retain(|symbol_id| symbol_id != &removed.symbol_id);
            unit.parse_affected_symbol_ids
                .retain(|symbol_id| symbol_id != &removed.symbol_id);
        }
        context
            .impact_edges
            .retain(|edge| edge.to_symbol.as_deref() != Some(&removed.symbol_id));
        context
            .domain_summaries
            .retain(|summary| summary.symbol_id.as_deref() != Some(&removed.symbol_id));
    }
    context.metrics.edges_emitted = context.impact_edges.len();
    context.metrics.summaries_emitted = context.domain_summaries.len();
}

fn update_output_bytes(context: &mut ImpactContext) {
    for _ in 0..3 {
        context.metrics.output_bytes = serialized_len(context);
    }
}

fn serialized_len(context: &ImpactContext) -> usize {
    serde_json::to_vec(context)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

fn summary_priority(kind: crate::impact_context::contracts::SummaryKind) -> u8 {
    use crate::impact_context::contracts::SummaryKind;
    match kind {
        SummaryKind::InterfaceChange
        | SummaryKind::TestSelection
        | SummaryKind::ConfigurationEffect => 0,
        SummaryKind::DependencyChange | SummaryKind::TextQueryMatch => 1,
        _ => 2,
    }
}

fn map_changed_ranges(source: &[u8], ranges: &[ChangedRange]) -> Vec<SourceRange> {
    let mut mapped = ranges
        .iter()
        .map(|range| line_range(source, range))
        .collect::<Vec<_>>();
    mapped.sort_by_key(|range| (range.start_byte, range.end_byte));
    mapped
}

fn map_deleted_ranges(ranges: &[ChangedRange]) -> Vec<SourceRange> {
    ranges
        .iter()
        .map(|range| SourceRange {
            start_line: range.start_line.max(1),
            start_column: 1,
            end_line: range.end_line.max(range.start_line).max(1),
            end_column: 1,
            start_byte: 0,
            end_byte: 0,
        })
        .collect()
}

fn line_range(source: &[u8], range: &ChangedRange) -> SourceRange {
    let line_starts = std::iter::once(0)
        .chain(
            source
                .iter()
                .enumerate()
                .filter(|(_, byte)| **byte == b'\n')
                .map(|(index, _)| index + 1),
        )
        .collect::<Vec<_>>();
    let start_line = range.start_line.max(1) as usize;
    let end_line = range.end_line.max(range.start_line).max(1) as usize;
    let start_byte = line_starts
        .get(start_line.saturating_sub(1))
        .copied()
        .unwrap_or(source.len());
    let end_byte = if range.deletion_anchor {
        start_byte
    } else {
        line_starts.get(end_line).copied().unwrap_or(source.len())
    };
    let end_column = if range.deletion_anchor {
        1
    } else {
        String::from_utf8_lossy(&source[start_byte..end_byte])
            .trim_end_matches('\n')
            .chars()
            .count() as u32
            + 1
    };
    SourceRange {
        start_line: range.start_line.max(1),
        start_column: 1,
        end_line: range.end_line.max(range.start_line).max(1),
        end_column,
        start_byte,
        end_byte,
    }
}

pub fn detect_language(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".rs") {
        "rust"
    } else if lower.ends_with(".toml") {
        "toml"
    } else if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        "yaml"
    } else if lower.ends_with("dockerfile") || lower.contains("dockerfile.") {
        "dockerfile"
    } else if lower.ends_with(".sql") {
        "sql"
    } else {
        "unknown"
    }
}

fn is_generated_like(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("vendor/")
        || lower.contains("/vendor/")
        || lower.starts_with("generated/")
        || lower.contains("/generated/")
        || lower.starts_with("dist/")
        || lower.contains("/dist/")
        || lower.ends_with(".min.js")
        || lower.ends_with(".min.css")
}

fn generated_limitation_code(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.contains("vendor") {
        "vendored-structure-skipped"
    } else if lower.ends_with(".min.js") || lower.ends_with(".min.css") {
        "minified-structure-skipped"
    } else {
        "generated-like-structure-skipped"
    }
}

fn impact_presence(presence: CandidatePresence) -> ImpactPresence {
    match presence {
        CandidatePresence::Present => ImpactPresence::Present,
        CandidatePresence::Deleted => ImpactPresence::Deleted,
        CandidatePresence::Gitlink => ImpactPresence::Gitlink,
    }
}

fn ranges_overlap(left: &SourceRange, right: &SourceRange) -> bool {
    left.start_byte <= right.end_byte && right.start_byte <= left.end_byte
}

fn sha256_hex(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn bounded(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

fn reason_code(code: &str) -> &str {
    if code.is_empty() {
        "impact-context-limited"
    } else {
        code
    }
}

fn contract_error_to_context_error(error: ImpactContractError) -> ImpactContextError {
    ImpactContextError::new("impact-context-invalid", error.to_string())
}
