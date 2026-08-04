use collect_diff_context_cli::candidate::{CandidateOpenLimits, GitCandidateContent, RepoPath};
use collect_diff_context_cli::impact_context::adapters::repository_index::{
    RepositoryIndexAdapter, RepositoryIndexRequest,
};
use collect_diff_context_cli::impact_context::budget::ImpactBudget;
use collect_diff_context_cli::impact_context::cache::cleanup::{
    clean_repository_cache, doctor_repository_cache, inspect_repository_generation,
    CacheOperationResult, CleanRequest, InspectSelector,
};
use collect_diff_context_cli::impact_context::cache::file_facts::CacheLayout;
use collect_diff_context_cli::impact_context::contracts::{
    Completeness, ImpactContext, ImpactMode, ImpactPresence, ImpactStatus, Limitation,
    ProviderStatus, UnitStatus,
};
use collect_diff_context_cli::impact_context::engine::{
    build_impact_context_with_repository_index, enforce_presentation_budget, ImpactRequest,
    RepositoryIndexRuntime,
};
use collect_diff_context_cli::impact_context::index::budget::IndexBudget;
use collect_diff_context_cli::impact_context::index::manifest::GitRepositoryManifestSource;
use collect_diff_context_cli::impact_context::index::model::{
    IndexAction, IndexLimitation, IndexReport, IndexReportStatus,
};
use collect_diff_context_cli::impact_context::normalizer::stable_id;
use collect_diff_context_cli::review_scope::{
    open_authoritative_scope_bounded, revalidate_scope_bounded, ReviewSource, ScopeRequest,
};
use collect_diff_context_cli::secret_scan;
use std::env;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const HELP: &str = "Usage:\n  repository-context-cli collect --source <staged|unstaged|branch> --expect-scope <fingerprint> --mode <fast|deep> [options]\n  repository-context-cli index <build|doctor|inspect|clean> [options]\n";
const COLLECT_HELP: &str = "Usage: repository-context-cli collect --source <staged|unstaged|branch> --expect-scope <fingerprint> --mode <fast|deep> [options]\n\nOptions:\n  --deadline-ms <positive bounded integer>\n  --max-changed-files <positive bounded integer>\n  --max-file-bytes <positive bounded integer>\n  --max-total-bytes <positive bounded integer>\n  --max-nodes <positive bounded integer>\n  --max-facts <positive bounded integer>\n  --max-edges <positive bounded integer>\n  --max-output-bytes <positive bounded integer>\n  -h, --help\n";
const INDEX_HELP: &str = "Usage:\n  repository-context-cli index build --source <staged|unstaged|branch> --expect-scope <fingerprint> [index limits]\n  repository-context-cli index doctor [--cache-dir <absolute>] [--generation <digest>]\n  repository-context-cli index inspect --generation <digest> (--path <repo-path> | --symbol <id>) [--max-rows <n>]\n  repository-context-cli index clean [--dry-run|--execute] [--max-bytes <n>] [--retain-generations <n>] [--invalid] [--max-scan-generations <n>] [--max-scan-bytes <n>] [--timeout-ms <n>]\n";
const INDEX_BUILD_HELP: &str = "Usage: repository-context-cli index build --source <staged|unstaged|branch> --expect-scope <fingerprint> [index limits]\n\nLimits may only lower the built-in Deep defaults.\n";

#[derive(Debug)]
struct CollectArgs {
    source: ReviewSource,
    expected_scope: String,
    mode: ImpactMode,
    budget: ImpactBudget,
}

#[derive(Debug)]
struct IndexBuildArgs {
    source: ReviewSource,
    expected_scope: String,
    budget: IndexBudget,
}

#[derive(Debug)]
struct IndexDoctorArgs {
    cache_dir: Option<PathBuf>,
    generation: Option<String>,
}

#[derive(Debug)]
struct IndexInspectArgs {
    generation: String,
    selector: InspectSelector,
    maximum_rows: usize,
}

#[derive(Debug)]
struct IndexCleanArgs {
    execute: bool,
    maximum_bytes: usize,
    retain_generations: usize,
    invalid_only: bool,
    maximum_scan_generations: usize,
    maximum_scan_bytes: usize,
    deadline: Duration,
}

enum RepositoryContextCommand {
    Collect(CollectArgs),
    IndexBuild(IndexBuildArgs),
    IndexDoctor(IndexDoctorArgs),
    IndexInspect(IndexInspectArgs),
    IndexClean(IndexCleanArgs),
}

enum ParseOutcome {
    Help(&'static str),
    Command(RepositoryContextCommand),
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
            Ok(ParseOutcome::Help(help)) => print_help(help),
            Ok(ParseOutcome::Command(RepositoryContextCommand::Collect(arguments))) => {
                run_collect(arguments)
            }
            Ok(ParseOutcome::Command(_)) => cli_error("invalid collect command", 2),
            Err(error) => cli_error(&error, 2),
        },
        Some("index") => match parse_index(arguments.collect()) {
            Ok(ParseOutcome::Help(help)) => print_help(help),
            Ok(ParseOutcome::Command(RepositoryContextCommand::IndexBuild(arguments))) => {
                run_index_build(arguments)
            }
            Ok(ParseOutcome::Command(RepositoryContextCommand::IndexDoctor(arguments))) => {
                run_index_doctor(arguments)
            }
            Ok(ParseOutcome::Command(RepositoryContextCommand::IndexInspect(arguments))) => {
                run_index_inspect(arguments)
            }
            Ok(ParseOutcome::Command(RepositoryContextCommand::IndexClean(arguments))) => {
                run_index_clean(arguments)
            }
            Ok(ParseOutcome::Command(_)) => cli_error("invalid index command", 2),
            Err(error) => cli_error(&error, 2),
        },
        _ => cli_error("expected collect or index subcommand", 2),
    }
}

fn print_help(help: &str) -> i32 {
    print!("{help}");
    0
}

fn parse_collect(arguments: Vec<String>) -> Result<ParseOutcome, String> {
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        return Ok(ParseOutcome::Help(COLLECT_HELP));
    }

    let defaults = match option_value(&arguments, "--mode").as_deref() {
        Some("deep") => ImpactBudget::deep_defaults(),
        _ => ImpactBudget::fast_defaults(),
    };
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
                mode = Some(match value.as_str() {
                    "fast" => ImpactMode::Fast,
                    "deep" => ImpactMode::Deep,
                    _ => return Err(format!("--mode must be fast or deep; received {value}")),
                });
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
    let mode = mode.ok_or_else(|| "--mode is required".to_string())?;
    if budget.max_file_bytes > budget.max_total_bytes {
        return Err("--max-file-bytes cannot exceed --max-total-bytes".to_string());
    }
    Ok(ParseOutcome::Command(RepositoryContextCommand::Collect(
        CollectArgs {
            source,
            expected_scope,
            mode,
            budget,
        },
    )))
}

fn parse_index(mut arguments: Vec<String>) -> Result<ParseOutcome, String> {
    if arguments.is_empty()
        || arguments
            .iter()
            .any(|argument| argument == "--help" || argument == "-h")
            && arguments.first().is_none_or(|argument| argument != "build")
    {
        return Ok(ParseOutcome::Help(INDEX_HELP));
    }
    let command = arguments.remove(0);
    match command.as_str() {
        "build" => parse_index_build(arguments),
        "doctor" => parse_index_doctor(arguments),
        "inspect" => parse_index_inspect(arguments),
        "clean" => parse_index_clean(arguments),
        observed => Err(format!("unsupported index subcommand: {observed}")),
    }
}

fn parse_index_build(arguments: Vec<String>) -> Result<ParseOutcome, String> {
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        return Ok(ParseOutcome::Help(INDEX_BUILD_HELP));
    }
    let defaults = IndexBudget::deep_defaults();
    let mut budget = defaults.clone();
    let mut source = None;
    let mut expected_scope = None;
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
            "--source" => source = Some(parse_source(&value)?),
            "--expect-scope" => expected_scope = Some(parse_fingerprint(&value)?),
            "--deadline-ms" => {
                let maximum = usize::try_from(defaults.deadline.as_millis()).unwrap_or(usize::MAX);
                budget.deadline = Duration::from_millis(parse_limit(flag, &value, maximum)? as u64);
            }
            "--max-manifest-files" => {
                budget.max_manifest_files = parse_limit(flag, &value, defaults.max_manifest_files)?;
            }
            "--max-manifest-bytes" => {
                budget.max_manifest_bytes = parse_limit(flag, &value, defaults.max_manifest_bytes)?;
            }
            "--max-project-model-files" => {
                budget.max_project_model_files =
                    parse_limit(flag, &value, defaults.max_project_model_files)?;
            }
            "--max-project-model-bytes" => {
                budget.max_project_model_bytes =
                    parse_limit(flag, &value, defaults.max_project_model_bytes)?;
            }
            "--max-file-bytes" => {
                budget.max_file_bytes = parse_limit(flag, &value, defaults.max_file_bytes)?;
            }
            "--max-parse-bytes" => {
                budget.max_parse_bytes = parse_limit(flag, &value, defaults.max_parse_bytes)?;
            }
            "--max-nodes" => {
                budget.max_nodes = parse_limit(flag, &value, defaults.max_nodes)?;
            }
            "--max-facts" => {
                budget.max_facts = parse_limit(flag, &value, defaults.max_facts)?;
            }
            "--max-symbols" => {
                budget.max_symbols = parse_limit(flag, &value, defaults.max_symbols)?;
            }
            "--max-edges" => {
                budget.max_edges = parse_limit(flag, &value, defaults.max_edges)?;
            }
            "--max-generation-bytes" => {
                budget.max_generation_bytes =
                    parse_limit(flag, &value, defaults.max_generation_bytes)?;
            }
            "--max-overlay-paths" => {
                budget.max_overlay_paths = parse_limit(flag, &value, defaults.max_overlay_paths)?;
            }
            "--max-query-rows" => {
                budget.max_query_rows = parse_limit(flag, &value, defaults.max_query_rows)?;
            }
            "--max-graph-depth" => {
                budget.max_graph_depth = parse_limit(flag, &value, defaults.max_graph_depth)?;
            }
            observed => return Err(format!("unsupported argument: {observed}")),
        }
        index += if inline_value.is_some() { 1 } else { 2 };
    }
    if budget.max_file_bytes > budget.max_parse_bytes {
        return Err("--max-file-bytes cannot exceed --max-parse-bytes".to_string());
    }
    Ok(ParseOutcome::Command(RepositoryContextCommand::IndexBuild(
        IndexBuildArgs {
            source: source.ok_or_else(|| "--source is required".to_string())?,
            expected_scope: expected_scope
                .ok_or_else(|| "--expect-scope is required".to_string())?,
            budget,
        },
    )))
}

fn parse_index_doctor(arguments: Vec<String>) -> Result<ParseOutcome, String> {
    let mut cache_dir = None;
    let mut generation = None;
    let mut index = 0;
    while index < arguments.len() {
        let (flag, value, consumed) = argument_value(&arguments, index)?;
        match flag {
            "--cache-dir" => {
                let path = PathBuf::from(value);
                if !path.is_absolute() {
                    return Err("--cache-dir must be absolute".to_string());
                }
                cache_dir = Some(path);
            }
            "--generation" => generation = Some(parse_sha256(value, "--generation")?),
            observed => return Err(format!("unsupported argument: {observed}")),
        }
        index += consumed;
    }
    Ok(ParseOutcome::Command(
        RepositoryContextCommand::IndexDoctor(IndexDoctorArgs {
            cache_dir,
            generation,
        }),
    ))
}

fn parse_index_inspect(arguments: Vec<String>) -> Result<ParseOutcome, String> {
    let mut generation = None;
    let mut path = None;
    let mut symbol = None;
    let mut maximum_rows = 100usize;
    let mut index = 0;
    while index < arguments.len() {
        let (flag, value, consumed) = argument_value(&arguments, index)?;
        match flag {
            "--generation" => generation = Some(parse_sha256(value, "--generation")?),
            "--path" => {
                path = Some(RepoPath::new(value).map_err(|error| error.to_string())?);
            }
            "--symbol" => symbol = Some(parse_sha256(value, "--symbol")?),
            "--max-rows" => maximum_rows = parse_limit(flag, value, 50_000)?,
            observed => return Err(format!("unsupported argument: {observed}")),
        }
        index += consumed;
    }
    let selector = match (path, symbol) {
        (Some(path), None) => InspectSelector::Path(path),
        (None, Some(symbol)) => InspectSelector::Symbol(symbol),
        _ => return Err("index inspect requires exactly one of --path or --symbol".to_string()),
    };
    Ok(ParseOutcome::Command(
        RepositoryContextCommand::IndexInspect(IndexInspectArgs {
            generation: generation.ok_or_else(|| "--generation is required".to_string())?,
            selector,
            maximum_rows,
        }),
    ))
}

fn parse_index_clean(arguments: Vec<String>) -> Result<ParseOutcome, String> {
    let mut execution = None;
    let mut maximum_bytes = 2 * 1024 * 1024 * 1024usize;
    let mut retain_generations = 2usize;
    let mut invalid_only = false;
    let mut maximum_scan_generations = 100_000usize;
    let mut maximum_scan_bytes = 2 * 1024 * 1024 * 1024usize;
    let mut timeout_millis = 30_000usize;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--dry-run" => {
                if execution.replace(false).is_some() {
                    return Err("--dry-run and --execute are mutually exclusive".to_string());
                }
                index += 1;
            }
            "--execute" => {
                if execution.replace(true).is_some() {
                    return Err("--dry-run and --execute are mutually exclusive".to_string());
                }
                index += 1;
            }
            "--invalid" => {
                invalid_only = true;
                index += 1;
            }
            _ => {
                let (flag, value, consumed) = argument_value(&arguments, index)?;
                match flag {
                    "--max-bytes" => {
                        maximum_bytes = parse_limit(flag, value, 2 * 1024 * 1024 * 1024usize)?;
                    }
                    "--retain-generations" => {
                        retain_generations = value
                            .parse::<usize>()
                            .map_err(|_| "--retain-generations must be an integer".to_string())?;
                        if retain_generations > 100_000 {
                            return Err(
                                "--retain-generations must be between 0 and 100000".to_string()
                            );
                        }
                    }
                    "--max-scan-generations" => {
                        maximum_scan_generations = parse_limit(flag, value, 100_000)?;
                    }
                    "--max-scan-bytes" => {
                        maximum_scan_bytes = parse_limit(flag, value, 2 * 1024 * 1024 * 1024usize)?;
                    }
                    "--timeout-ms" => {
                        timeout_millis = parse_limit(flag, value, 30_000)?;
                    }
                    observed => return Err(format!("unsupported argument: {observed}")),
                }
                index += consumed;
            }
        }
    }
    Ok(ParseOutcome::Command(RepositoryContextCommand::IndexClean(
        IndexCleanArgs {
            execute: execution.unwrap_or(false),
            maximum_bytes,
            retain_generations,
            invalid_only,
            maximum_scan_generations,
            maximum_scan_bytes,
            deadline: Duration::from_millis(timeout_millis as u64),
        },
    )))
}

fn argument_value(arguments: &[String], index: usize) -> Result<(&str, &str, usize), String> {
    let argument = &arguments[index];
    if let Some((flag, value)) = argument.split_once('=') {
        if value.is_empty() {
            return Err(format!("{flag} requires a value"));
        }
        return Ok((flag, value, 1));
    }
    let value = arguments
        .get(index + 1)
        .ok_or_else(|| format!("{argument} requires a value"))?;
    Ok((argument, value, 2))
}

fn parse_sha256(value: &str, flag: &str) -> Result<String, String> {
    if value.len() != 64
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(format!(
            "{flag} must be 64 lowercase hexadecimal characters"
        ));
    }
    Ok(value.to_string())
}

fn parse_source(value: &str) -> Result<ReviewSource, String> {
    match value {
        "staged" => Ok(ReviewSource::Staged),
        "unstaged" => Ok(ReviewSource::Unstaged),
        "branch" => Ok(ReviewSource::Branch),
        observed => Err(format!(
            "--source must be staged, unstaged, or branch; received {observed}"
        )),
    }
}

fn option_value(arguments: &[String], requested_flag: &str) -> Option<String> {
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if let Some((flag, value)) = argument.split_once('=') {
            if flag == requested_flag {
                return Some(value.to_string());
            }
            index += 1;
            continue;
        }
        if argument == requested_flag {
            return arguments.get(index + 1).cloned();
        }
        index += 1;
    }
    None
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
    let maximum_output_bytes = arguments.budget.max_output_bytes;
    let total_deadline = arguments.budget.deadline;
    let collection_started = Instant::now();
    let repository = match env::current_dir() {
        Ok(repository) => repository,
        Err(error) => return cli_error(&format!("cannot resolve current directory: {error}"), 2),
    };
    let scope = match open_authoritative_scope_bounded(
        ScopeRequest {
            repository,
            source: Some(arguments.source),
            expected_fingerprint: Some(arguments.expected_scope),
        },
        total_deadline.saturating_sub(collection_started.elapsed()),
    ) {
        Ok(scope) => scope,
        Err(error) => return cli_error(&error.to_string(), 2),
    };
    let candidate = match GitCandidateContent::open_bounded(
        &scope,
        CandidateOpenLimits {
            deadline: total_deadline.saturating_sub(collection_started.elapsed()),
            max_changed_files: arguments.budget.max_changed_files,
            max_file_bytes: arguments.budget.max_file_bytes,
            max_total_bytes: arguments.budget.max_total_bytes,
        },
    ) {
        Ok(candidate) => candidate,
        Err(error) => return cli_error(&error.to_string(), 2),
    };
    let manifest_source = GitRepositoryManifestSource::new_bounded(
        &scope,
        total_deadline.saturating_sub(collection_started.elapsed()),
    )
    .ok();
    let cache_layout = CacheLayout::resolve(&scope.repository, None).ok();
    let repository_runtime =
        manifest_source
            .as_ref()
            .zip(cache_layout)
            .map(|(manifest_source, cache_layout)| RepositoryIndexRuntime {
                manifest_source,
                cache_layout,
            });
    let mut request = match arguments.mode {
        ImpactMode::Fast => ImpactRequest::fast_defaults(),
        ImpactMode::Deep => ImpactRequest::deep_defaults(),
    };
    request.budget = arguments.budget;
    request.budget.deadline = total_deadline.saturating_sub(collection_started.elapsed());
    let context =
        match build_impact_context_with_repository_index(&candidate, request, repository_runtime) {
            Ok(context) => context,
            Err(error) => return cli_error(&error.to_string(), 2),
        };

    if let Err(error) = revalidate_scope_bounded(
        &scope,
        total_deadline.saturating_sub(collection_started.elapsed()),
    ) {
        return match render_context(
            invalidated_context(context, &error.to_string()),
            maximum_output_bytes,
        ) {
            Ok(output) => {
                print!("{output}");
                3
            }
            Err(render_error) => cli_error(&render_error, 3),
        };
    }

    match render_context(context, maximum_output_bytes) {
        Ok(output) => {
            print!("{output}");
            0
        }
        Err(error) => cli_error(&error, 2),
    }
}

fn run_index_build(arguments: IndexBuildArgs) -> i32 {
    let started = Instant::now();
    let repository = match env::current_dir() {
        Ok(repository) => repository,
        Err(error) => return cli_error(&format!("cannot resolve current directory: {error}"), 2),
    };
    let scope = match open_authoritative_scope_bounded(
        ScopeRequest {
            repository,
            source: Some(arguments.source),
            expected_fingerprint: Some(arguments.expected_scope),
        },
        arguments.budget.deadline,
    ) {
        Ok(scope) => scope,
        Err(error) => return cli_error(&error.to_string(), 2),
    };
    let candidate = match GitCandidateContent::open_bounded(
        &scope,
        CandidateOpenLimits {
            deadline: arguments.budget.deadline.saturating_sub(started.elapsed()),
            max_changed_files: arguments.budget.max_overlay_paths,
            max_file_bytes: arguments.budget.max_file_bytes,
            max_total_bytes: arguments.budget.max_parse_bytes,
        },
    ) {
        Ok(candidate) => candidate,
        Err(error) => return cli_error(&error.to_string(), 2),
    };
    let manifest_source = match GitRepositoryManifestSource::new_bounded(
        &scope,
        arguments.budget.deadline.saturating_sub(started.elapsed()),
    ) {
        Ok(source) => source,
        Err(error) => return cli_error(&error.to_string(), 2),
    };
    let layout = match CacheLayout::resolve(&scope.repository, None) {
        Ok(layout) => layout,
        Err(error) => return cli_error(&error.to_string(), 2),
    };
    let repository_id = layout.repository_id.clone();
    let output = match RepositoryIndexAdapter::new(layout).analyze(RepositoryIndexRequest {
        candidate: &candidate,
        manifest_source: &manifest_source,
        changed_symbols: &[],
        mode: ImpactMode::Deep,
        cache_read: true,
        cache_write: true,
        index_budget: arguments.budget.clone(),
    }) {
        Ok(output) => output,
        Err(error) => return cli_error(&error.to_string(), 2),
    };
    let mut report = IndexReport {
        schema_version: 1,
        kind: "repository_index_report".to_string(),
        action: IndexAction::Build,
        status: if output.provider.status == ProviderStatus::Completed {
            IndexReportStatus::Completed
        } else {
            IndexReportStatus::Partial
        },
        scope_fingerprint: Some(scope.fingerprint.clone()),
        repository_id,
        generation_key: Some(output.generation_key),
        metrics: output.metrics,
        limitations: index_limitations(output.limitations),
    };
    report.metrics.elapsed_ms = elapsed_ms(started);
    if let Err(error) = revalidate_scope_bounded(
        &scope,
        arguments.budget.deadline.saturating_sub(started.elapsed()),
    ) {
        report.status = IndexReportStatus::Invalidated;
        report.limitations.push(IndexLimitation {
            code: "repository-index-scope-drift".to_string(),
            path: None,
            symbol_id: None,
            reason: "repository scope changed before index report release".to_string(),
            interpretation: error.to_string().chars().take(1_000).collect(),
        });
        sort_index_limitations(&mut report.limitations);
    }
    let exit_code = if report.status == IndexReportStatus::Invalidated {
        3
    } else {
        0
    };
    match render_index_report(report) {
        Ok(output) => {
            print!("{output}");
            exit_code
        }
        Err(error) => cli_error(&error, 2),
    }
}

fn run_index_doctor(arguments: IndexDoctorArgs) -> i32 {
    let layout = match resolve_cache_layout(arguments.cache_dir.as_deref()) {
        Ok(layout) => layout,
        Err(error) => return cli_error(&error, 2),
    };
    let repository_id = layout.repository_id.clone();
    let operation = match doctor_repository_cache(
        &layout,
        arguments.generation.as_deref(),
        100_000,
        32 * 1024 * 1024,
    ) {
        Ok(operation) => operation,
        Err(error) => return cli_error(&error.to_string(), 2),
    };
    render_index_operation(IndexAction::Doctor, repository_id, operation)
}

fn run_index_inspect(arguments: IndexInspectArgs) -> i32 {
    let layout = match resolve_cache_layout(None) {
        Ok(layout) => layout,
        Err(error) => return cli_error(&error, 2),
    };
    let repository_id = layout.repository_id.clone();
    let operation = match inspect_repository_generation(
        &layout,
        &arguments.generation,
        &arguments.selector,
        arguments.maximum_rows,
    ) {
        Ok(operation) => operation,
        Err(error) => return cli_error(&error.to_string(), 2),
    };
    render_index_operation(IndexAction::Inspect, repository_id, operation)
}

fn run_index_clean(arguments: IndexCleanArgs) -> i32 {
    let layout = match resolve_cache_layout(None) {
        Ok(layout) => layout,
        Err(error) => return cli_error(&error, 2),
    };
    let repository_id = layout.repository_id.clone();
    let operation = match clean_repository_cache(
        &layout,
        CleanRequest {
            execute: arguments.execute,
            maximum_bytes: arguments.maximum_bytes,
            retain_generations: arguments.retain_generations,
            invalid_only: arguments.invalid_only,
            maximum_scan_generations: arguments.maximum_scan_generations,
            maximum_scan_bytes: arguments.maximum_scan_bytes,
            deadline: arguments.deadline,
        },
    ) {
        Ok(operation) => operation,
        Err(error) => return cli_error(&error.to_string(), 2),
    };
    render_index_operation(IndexAction::Clean, repository_id, operation)
}

fn resolve_cache_layout(override_root: Option<&std::path::Path>) -> Result<CacheLayout, String> {
    let repository =
        env::current_dir().map_err(|error| format!("cannot resolve current directory: {error}"))?;
    CacheLayout::resolve(&repository, override_root).map_err(|error| error.to_string())
}

fn render_index_operation(
    action: IndexAction,
    repository_id: String,
    operation: CacheOperationResult,
) -> i32 {
    let report = IndexReport {
        schema_version: 1,
        kind: "repository_index_report".to_string(),
        action,
        status: operation.status,
        scope_fingerprint: None,
        repository_id,
        generation_key: operation.generation_key,
        metrics: operation.metrics,
        limitations: operation.limitations,
    };
    match render_index_report(report) {
        Ok(output) => {
            print!("{output}");
            0
        }
        Err(error) => cli_error(&error, 2),
    }
}

fn index_limitations(limitations: Vec<Limitation>) -> Vec<IndexLimitation> {
    let mut output = limitations
        .into_iter()
        .map(|limitation| IndexLimitation {
            code: limitation.code,
            path: limitation.path.and_then(|path| RepoPath::new(path).ok()),
            symbol_id: limitation.symbol_id,
            reason: limitation.reason,
            interpretation: limitation.interpretation,
        })
        .collect::<Vec<_>>();
    sort_index_limitations(&mut output);
    output
}

fn sort_index_limitations(limitations: &mut Vec<IndexLimitation>) {
    limitations.sort_by(|left, right| {
        (
            left.code.as_str(),
            left.path.as_ref().map(RepoPath::as_str).unwrap_or(""),
            left.symbol_id.as_deref().unwrap_or(""),
            left.reason.as_str(),
            left.interpretation.as_str(),
        )
            .cmp(&(
                right.code.as_str(),
                right.path.as_ref().map(RepoPath::as_str).unwrap_or(""),
                right.symbol_id.as_deref().unwrap_or(""),
                right.reason.as_str(),
                right.interpretation.as_str(),
            ))
    });
    limitations.dedup();
}

fn render_index_report(mut report: IndexReport) -> Result<String, String> {
    for _ in 0..3 {
        report.metrics.output_bytes = serde_json::to_vec(&report)
            .map_err(|error| error.to_string())?
            .len();
    }
    report.validate().map_err(|error| error.to_string())?;
    let compact = serde_json::to_string(&report).map_err(|error| error.to_string())?;
    if env::var("PRE_COMMIT_REVIEW_SECRET_SCAN").as_deref() == Ok("off") {
        return Ok(compact);
    }
    if let Err(error) =
        sanitize_index_report_text_fields(&mut report, secret_scan::sanitize_for_model)
    {
        let mut failed = report;
        failed.status = IndexReportStatus::Failed;
        failed.limitations = vec![IndexLimitation {
            code: "output-sanitization-unavailable".to_string(),
            path: None,
            symbol_id: None,
            reason: "index report could not be sanitized".to_string(),
            interpretation: error.reason_code().to_string(),
        }];
        for _ in 0..3 {
            failed.metrics.output_bytes = serde_json::to_vec(&failed)
                .map_err(|error| error.to_string())?
                .len();
        }
        failed.validate().map_err(|error| error.to_string())?;
        return serde_json::to_string(&failed).map_err(|error| error.to_string());
    }
    for _ in 0..3 {
        report.metrics.output_bytes = serde_json::to_vec(&report)
            .map_err(|error| error.to_string())?
            .len();
    }
    report.validate().map_err(|error| error.to_string())?;
    serde_json::to_string(&report).map_err(|error| error.to_string())
}

fn sanitize_index_report_text_fields<F>(
    report: &mut IndexReport,
    mut sanitize: F,
) -> Result<(), secret_scan::SecretScanError>
where
    F: FnMut(&str) -> Result<secret_scan::SanitizedOutput, secret_scan::SecretScanError>,
{
    for limitation in &mut report.limitations {
        limitation.reason = sanitize(&limitation.reason)?.content;
        limitation.interpretation = sanitize(&limitation.interpretation)?.content;
    }
    Ok(())
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn render_context(
    mut context: ImpactContext,
    maximum_output_bytes: usize,
) -> Result<String, String> {
    enforce_presentation_budget(&mut context, maximum_output_bytes)
        .map_err(|error| error.to_string())?;
    context.validate().map_err(|error| error.to_string())?;
    let compact = serde_json::to_string(&context).map_err(|error| error.to_string())?;
    if env::var("PRE_COMMIT_REVIEW_SECRET_SCAN").as_deref() == Ok("off") {
        return Ok(compact);
    }
    match secret_scan::sanitize_for_model(&compact) {
        Ok(sanitized) => {
            let mut sanitized_context: ImpactContext =
                serde_json::from_str(&sanitized.content).map_err(|error| error.to_string())?;
            enforce_presentation_budget(&mut sanitized_context, maximum_output_bytes)
                .map_err(|error| error.to_string())?;
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
            let mut failed = failed_sanitization_context(context, error.reason_code());
            enforce_presentation_budget(&mut failed, maximum_output_bytes)
                .map_err(|error| error.to_string())?;
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
    let candidate_unavailable = context
        .units
        .iter()
        .any(|unit| {
            unit.presence == ImpactPresence::Present
                && unit.content_sha256.is_none()
                && unit.content_bytes.is_none()
        })
        .then(|| {
            static_limitation(
                "candidate-read-unavailable",
                "Candidate bytes were unavailable before context invalidation.",
                "No structural or text facts were retained for affected units.",
            )
        });
    context.status = status;
    context.changed_symbols.clear();
    context.impact_edges.clear();
    context.domain_summaries.clear();
    context.limitations = vec![limitation.clone()];
    if let Some(candidate_unavailable) = candidate_unavailable.as_ref() {
        context.limitations.push(candidate_unavailable.clone());
    }
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
        if unit.presence == ImpactPresence::Present
            && unit.content_sha256.is_none()
            && unit.content_bytes.is_none()
        {
            if let Some(candidate_unavailable) = candidate_unavailable.as_ref() {
                unit.limitation_ids
                    .push(candidate_unavailable.limitation_id.clone());
                unit.limitation_ids.sort();
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use collect_diff_context_cli::impact_context::index::model::IndexMetrics;
    use collect_diff_context_cli::secret_scan::{SanitizedOutput, SecretScanStatus};

    #[test]
    fn index_report_sanitization_scans_only_free_text_fields() {
        let mut report = IndexReport {
            schema_version: 1,
            kind: "repository_index_report".to_string(),
            action: IndexAction::Build,
            status: IndexReportStatus::Completed,
            scope_fingerprint: Some("a".repeat(40)),
            repository_id: "b".repeat(64),
            generation_key: Some("c".repeat(64)),
            metrics: IndexMetrics {
                elapsed_ms: 0,
                manifest_files: 0,
                manifest_bytes: 0,
                file_fact_hits: 0,
                file_fact_misses: 0,
                file_fact_writes: 0,
                parsed_files: 0,
                parsed_bytes: 0,
                symbols: 0,
                edges: 0,
                query_rows: 0,
                generation_bytes: 0,
                output_bytes: 0,
            },
            limitations: vec![IndexLimitation {
                code: "example-limitation".to_string(),
                path: None,
                symbol_id: Some("d".repeat(64)),
                reason: "token=secret-value".to_string(),
                interpretation: "review secret-value before continuing".to_string(),
            }],
        };
        let mut scanned = Vec::new();

        sanitize_index_report_text_fields(&mut report, |value| {
            scanned.push(value.to_string());
            Ok(SanitizedOutput {
                content: value.replace("secret-value", "[redacted:test]"),
                redactions: Vec::new(),
                status: SecretScanStatus::Redacted,
            })
        })
        .unwrap();

        assert_eq!(
            scanned,
            vec![
                "token=secret-value".to_string(),
                "review secret-value before continuing".to_string(),
            ]
        );
        assert_eq!(
            report.scope_fingerprint.as_deref(),
            Some("a".repeat(40).as_str())
        );
        assert_eq!(report.repository_id, "b".repeat(64));
        assert_eq!(
            report.generation_key.as_deref(),
            Some("c".repeat(64).as_str())
        );
        assert_eq!(
            report.limitations[0].symbol_id.as_deref(),
            Some("d".repeat(64).as_str())
        );
        assert_eq!(report.limitations[0].reason, "token=[redacted:test]");
        assert_eq!(
            report.limitations[0].interpretation,
            "review [redacted:test] before continuing"
        );
        report.validate().unwrap();
    }
}
