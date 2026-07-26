use collect_diff_context_cli::candidate::GitCandidateContent;
use collect_diff_context_cli::impact_context::budget::ImpactBudget;
use collect_diff_context_cli::impact_context::contracts::{
    Completeness, ImpactContext, ImpactMode, ImpactStatus, Limitation, ProviderStatus, UnitStatus,
};
use collect_diff_context_cli::impact_context::engine::{build_impact_context, ImpactRequest};
use collect_diff_context_cli::impact_context::normalizer::stable_id;
use collect_diff_context_cli::review_scope::{
    open_authoritative_scope, revalidate_scope, ReviewSource, ScopeRequest,
};
use collect_diff_context_cli::secret_scan;
use std::env;
use std::time::Duration;

const HELP: &str = "Usage: repository-context-cli collect --source <staged|unstaged|branch> --expect-scope <fingerprint> --mode fast [options]\n";
const COLLECT_HELP: &str = "Usage: repository-context-cli collect --source <staged|unstaged|branch> --expect-scope <fingerprint> --mode fast [options]\n\nOptions:\n  --deadline-ms <1..750>\n  --max-changed-files <1..30>\n  --max-file-bytes <1..2097152>\n  --max-total-bytes <1..8388608>\n  --max-nodes <1..250000>\n  --max-facts <1..5000>\n  --max-edges <1..500>\n  --max-output-bytes <1..1048576>\n  -h, --help\n";

#[derive(Debug)]
struct CollectArgs {
    source: ReviewSource,
    expected_scope: String,
    budget: ImpactBudget,
}

enum ParseOutcome {
    Help,
    Collect(CollectArgs),
}

fn main() {
    let exit_code = main_entry();
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn main_entry() -> i32 {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("--help" | "-h") => {
            print!("{HELP}");
            0
        }
        Some("collect") => match parse_collect(arguments.collect()) {
            Ok(ParseOutcome::Help) => {
                print!("{COLLECT_HELP}");
                0
            }
            Ok(ParseOutcome::Collect(arguments)) => run_collect(arguments),
            Err(error) => cli_error(&error, 2),
        },
        _ => cli_error("expected collect subcommand", 2),
    }
}

fn parse_collect(arguments: Vec<String>) -> Result<ParseOutcome, String> {
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        return Ok(ParseOutcome::Help);
    }

    let defaults = ImpactBudget::fast_defaults();
    let mut budget = defaults.clone();
    let mut source = None;
    let mut expected_scope = None;
    let mut mode = None;
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        let (flag, inline_value) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(flag, value)| {
                (flag, Some(value))
            });
        let value = if let Some(value) = inline_value {
            value.to_string()
        } else {
            arguments
                .get(index + 1)
                .cloned()
                .ok_or_else(|| format!("{flag} requires a value"))?
        };
        match flag {
            "--source" => {
                source = Some(match value.as_str() {
                    "staged" => ReviewSource::Staged,
                    "unstaged" => ReviewSource::Unstaged,
                    "branch" => ReviewSource::Branch,
                    observed => {
                        return Err(format!(
                            "--source must be staged, unstaged, or branch; received {observed}"
                        ))
                    }
                });
            }
            "--expect-scope" => expected_scope = Some(parse_fingerprint(&value)?),
            "--mode" => {
                if value != "fast" {
                    return Err(format!("--mode must be fast; received {value}"));
                }
                mode = Some(ImpactMode::Fast);
            }
            "--deadline-ms" => {
                let value = parse_limit(flag, &value, defaults.deadline.as_millis() as usize)?;
                budget.deadline = Duration::from_millis(value as u64);
            }
            "--max-changed-files" => {
                budget.max_changed_files = parse_limit(flag, &value, defaults.max_changed_files)?;
            }
            "--max-file-bytes" => {
                budget.max_file_bytes = parse_limit(flag, &value, defaults.max_file_bytes)?;
            }
            "--max-total-bytes" => {
                budget.max_total_bytes = parse_limit(flag, &value, defaults.max_total_bytes)?;
            }
            "--max-nodes" => {
                budget.max_nodes = parse_limit(flag, &value, defaults.max_nodes)?;
            }
            "--max-facts" => {
                budget.max_facts = parse_limit(flag, &value, defaults.max_facts)?;
            }
            "--max-edges" => {
                budget.max_edges = parse_limit(flag, &value, defaults.max_edges)?;
            }
            "--max-output-bytes" => {
                budget.max_output_bytes = parse_limit(flag, &value, defaults.max_output_bytes)?;
            }
            observed => return Err(format!("unsupported argument: {observed}")),
        }
        index += if inline_value.is_some() { 1 } else { 2 };
    }

    let source = source.ok_or_else(|| "--source is required".to_string())?;
    let expected_scope = expected_scope.ok_or_else(|| "--expect-scope is required".to_string())?;
    mode.ok_or_else(|| "--mode fast is required".to_string())?;
    if budget.max_file_bytes > budget.max_total_bytes {
        return Err("--max-file-bytes cannot exceed --max-total-bytes".to_string());
    }
    Ok(ParseOutcome::Collect(CollectArgs {
        source,
        expected_scope,
        budget,
    }))
}

fn parse_limit(flag: &str, value: &str, maximum: usize) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("{flag} must be an integer"))?;
    if parsed == 0 || parsed > maximum {
        return Err(format!("{flag} must be between 1 and {maximum}"));
    }
    Ok(parsed)
}

fn parse_fingerprint(value: &str) -> Result<String, String> {
    if !matches!(value.len(), 40 | 64)
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err("--expect-scope must be 40 or 64 lowercase hexadecimal characters".to_string());
    }
    Ok(value.to_string())
}

fn run_collect(arguments: CollectArgs) -> i32 {
    let repository = match env::current_dir() {
        Ok(repository) => repository,
        Err(error) => return cli_error(&format!("cannot resolve current directory: {error}"), 2),
    };
    let scope = match open_authoritative_scope(ScopeRequest {
        repository,
        source: Some(arguments.source),
        expected_fingerprint: Some(arguments.expected_scope),
    }) {
        Ok(scope) => scope,
        Err(error) => return cli_error(&error.to_string(), 2),
    };
    let candidate = match GitCandidateContent::open(&scope) {
        Ok(candidate) => candidate,
        Err(error) => return cli_error(&error.to_string(), 2),
    };
    let mut request = ImpactRequest::fast_defaults();
    request.budget = arguments.budget;
    let context = match build_impact_context(&candidate, request) {
        Ok(context) => context,
        Err(error) => return cli_error(&error.to_string(), 2),
    };

    if let Err(error) = revalidate_scope(&scope) {
        return match render_context(invalidated_context(context, &error.to_string())) {
            Ok(output) => {
                print!("{output}");
                3
            }
            Err(render_error) => cli_error(&render_error, 3),
        };
    }

    match render_context(context) {
        Ok(output) => {
            print!("{output}");
            0
        }
        Err(error) => cli_error(&error, 2),
    }
}

fn render_context(context: ImpactContext) -> Result<String, String> {
    context.validate().map_err(|error| error.to_string())?;
    let compact = serde_json::to_string(&context).map_err(|error| error.to_string())?;
    if env::var("PRE_COMMIT_REVIEW_SECRET_SCAN").as_deref() == Ok("off") {
        return Ok(compact);
    }
    match secret_scan::sanitize_for_model(&compact) {
        Ok(sanitized) => {
            let sanitized_context: ImpactContext =
                serde_json::from_str(&sanitized.content).map_err(|error| error.to_string())?;
            sanitized_context
                .validate()
                .map_err(|error| error.to_string())?;
            serde_json::to_string(&sanitized_context).map_err(|error| error.to_string())
        }
        Err(_)
            if matches!(
                context.status,
                ImpactStatus::Invalidated | ImpactStatus::Failed
            ) =>
        {
            Ok(compact)
        }
        Err(error) => {
            let failed = failed_sanitization_context(context, error.reason_code());
            failed.validate().map_err(|error| error.to_string())?;
            serde_json::to_string(&failed).map_err(|error| error.to_string())
        }
    }
}

fn invalidated_context(mut context: ImpactContext, reason: &str) -> ImpactContext {
    let limitation = static_limitation(
        "scope-drift",
        "Repository scope changed before context release.",
        reason,
    );
    invalidate_facts(&mut context, ImpactStatus::Invalidated, &limitation);
    context
}

fn failed_sanitization_context(mut context: ImpactContext, reason: &str) -> ImpactContext {
    let limitation = static_limitation(
        "output-sanitization-unavailable",
        "Impact context could not be sanitized without violating its contract.",
        reason,
    );
    invalidate_facts(&mut context, ImpactStatus::Failed, &limitation);
    context
}

fn static_limitation(code: &str, reason: &str, interpretation: &str) -> Limitation {
    Limitation {
        limitation_id: stable_id("impact-limitation/v1", &[code, "", "", ""]),
        code: code.to_string(),
        provider_id: None,
        path: None,
        symbol_id: None,
        reason: reason.to_string(),
        interpretation: interpretation.chars().take(1_000).collect(),
        improvable_in_deep_mode: false,
    }
}

fn invalidate_facts(context: &mut ImpactContext, status: ImpactStatus, limitation: &Limitation) {
    context.status = status;
    context.changed_symbols.clear();
    context.impact_edges.clear();
    context.domain_summaries.clear();
    context.limitations = vec![limitation.clone()];
    for provider in &mut context.providers {
        provider.status = match status {
            ImpactStatus::Invalidated => ProviderStatus::Stale,
            _ => ProviderStatus::InvalidOutput,
        };
        provider.output_fact_count = 0;
        provider.limitation_ids = vec![limitation.limitation_id.clone()];
    }
    for unit in &mut context.units {
        unit.syntax_status = UnitStatus::Unavailable;
        unit.text_status = UnitStatus::Unavailable;
        unit.parse_quality = None;
        unit.error_node_count = 0;
        unit.missing_node_count = 0;
        unit.parse_affected_ranges.clear();
        unit.parse_affected_symbol_ids.clear();
        unit.changed_symbol_ids.clear();
        unit.limitation_ids = vec![limitation.limitation_id.clone()];
    }
    context.coverage.parsed_files = 0;
    context.coverage.clean_parse_files = 0;
    context.coverage.recovered_parse_files = 0;
    context.coverage.degraded_parse_files = 0;
    context.coverage.unsupported_files = 0;
    context.coverage.resource_limited_files = 0;
    context.coverage.unavailable_files = context.units.len();
    context.coverage.cache_hits = 0;
    context.coverage.cache_misses = 0;
    context.coverage.cache_stale = 0;
    context.coverage.cache_corrupt = 0;
    context.coverage.requested_graph_depth = 0;
    context.coverage.reached_graph_depth = 0;
    context.coverage.graph_index_completeness = Completeness::Unavailable;
    context.coverage.graph_query_completeness = Completeness::Unavailable;
    context.coverage.output_truncated = false;
    context.metrics.facts_emitted = 0;
    context.metrics.edges_emitted = 0;
    context.metrics.summaries_emitted = 0;
    for _ in 0..3 {
        context.metrics.output_bytes = serde_json::to_vec(context)
            .map(|bytes| bytes.len())
            .unwrap_or(0);
    }
}

fn cli_error(message: &str, exit_code: i32) -> i32 {
    eprintln!("repository-context-cli: {message}");
    exit_code
}
