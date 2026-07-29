use crate::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
use crate::repository_context_provider::contract::{
    validate_absolute_path, validate_sha256, validate_text,
};
use crate::repository_context_provider::model::{build_linked_project_model, ProviderModelLimits};
use crate::review_scope::{
    open_authoritative_scope_bounded, revalidate_scope_bounded, ReviewSource, ScopeRequest,
};
use std::collections::BTreeSet;
use std::env;
use std::path::PathBuf;
use std::time::Duration;

const HELP: &str = "Usage:\n  repository-context-provider-cli model --source <staged|unstaged|branch> --expect-scope <fingerprint> [options]\n  repository-context-provider-cli run --source <staged|unstaged|branch> --expect-scope <fingerprint> --registry <absolute-path> --expect-registry-sha256 <sha256> --provider-id <id> --model <absolute-path> --expect-model-sha256 <sha256> --request <absolute-path>\n";
const MODEL_HELP: &str = "Usage: repository-context-provider-cli model --source <staged|unstaged|branch> --expect-scope <fingerprint> [options]\n\nOptions:\n  --max-model-files <positive bounded integer>\n  --max-model-bytes <positive bounded integer>\n  -h, --help\n";
const RUN_HELP: &str = "Usage: repository-context-provider-cli run --source <staged|unstaged|branch> --expect-scope <fingerprint> --registry <absolute-path> --expect-registry-sha256 <sha256> --provider-id <id> --model <absolute-path> --expect-model-sha256 <sha256> --request <absolute-path>\n\nOptions:\n  -h, --help\n";
const SCOPE_DEADLINE: Duration = Duration::from_secs(30);
const MAX_PROVIDER_ID_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Model(ModelArgs),
    Run(RunArgs),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelArgs {
    pub source: ReviewSource,
    pub expected_scope: String,
    pub maximum_model_files: usize,
    pub maximum_model_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArgs {
    pub source: ReviewSource,
    pub expected_scope: String,
    pub registry_path: PathBuf,
    pub expected_registry_sha256: String,
    pub provider_id: String,
    pub model_path: PathBuf,
    pub expected_model_sha256: String,
    pub request_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub code: &'static str,
    message: String,
}

impl CliError {
    fn new(code: &'static str, message: impl AsRef<str>) -> Self {
        Self {
            code,
            message: bounded_detail(message.as_ref()),
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

enum ParseOutcome {
    Help(&'static str),
    Command(Command),
}

pub fn main_entry() -> i32 {
    match parse_arguments(env::args().skip(1).collect()) {
        Ok(ParseOutcome::Help(help)) => {
            print!("{help}");
            0
        }
        Ok(ParseOutcome::Command(Command::Model(arguments))) => match run_model(arguments) {
            Ok(output) => {
                println!("{output}");
                0
            }
            Err(error) => emit_error(&error, 2),
        },
        Ok(ParseOutcome::Command(Command::Run(_))) => emit_error(
            &CliError::new(
                "provider-cli-run-unavailable",
                "run execution is not available in this delivery step",
            ),
            2,
        ),
        Err(error) => emit_error(&error, 2),
    }
}

fn parse_arguments(arguments: Vec<String>) -> Result<ParseOutcome, CliError> {
    let Some(command) = arguments.first() else {
        return Err(argument_error("expected model or run subcommand"));
    };
    match command.as_str() {
        "--help" | "-h" if arguments.len() == 1 => Ok(ParseOutcome::Help(HELP)),
        "model" => parse_model(&arguments[1..]),
        "run" => parse_run(&arguments[1..]),
        _ => Err(argument_error("expected model or run subcommand")),
    }
}

fn parse_model(arguments: &[String]) -> Result<ParseOutcome, CliError> {
    if help_requested(arguments) {
        return Ok(ParseOutcome::Help(MODEL_HELP));
    }
    let defaults = ProviderModelLimits::default();
    let mut source = None;
    let mut expected_scope = None;
    let mut maximum_model_files = defaults.max_files;
    let mut maximum_model_bytes = defaults.max_bytes;
    let mut seen = BTreeSet::new();
    let mut index = 0;
    while index < arguments.len() {
        let (flag, value, consumed) = option_value(arguments, index, &mut seen)?;
        match flag {
            "--source" => source = Some(parse_source(value)?),
            "--expect-scope" => expected_scope = Some(parse_fingerprint(value)?),
            "--max-model-files" => {
                maximum_model_files = parse_limit(flag, value, defaults.max_files)?;
            }
            "--max-model-bytes" => {
                maximum_model_bytes = parse_limit(flag, value, defaults.max_bytes)?;
            }
            _ => return Err(argument_error("unsupported model argument")),
        }
        index += consumed;
    }
    Ok(ParseOutcome::Command(Command::Model(ModelArgs {
        source: source.ok_or_else(|| argument_error("--source is required"))?,
        expected_scope: expected_scope
            .ok_or_else(|| argument_error("--expect-scope is required"))?,
        maximum_model_files,
        maximum_model_bytes,
    })))
}

fn parse_run(arguments: &[String]) -> Result<ParseOutcome, CliError> {
    if help_requested(arguments) {
        return Ok(ParseOutcome::Help(RUN_HELP));
    }
    let mut source = None;
    let mut expected_scope = None;
    let mut registry_path = None;
    let mut expected_registry_sha256 = None;
    let mut provider_id = None;
    let mut model_path = None;
    let mut expected_model_sha256 = None;
    let mut request_path = None;
    let mut seen = BTreeSet::new();
    let mut index = 0;
    while index < arguments.len() {
        let (flag, value, consumed) = option_value(arguments, index, &mut seen)?;
        match flag {
            "--source" => source = Some(parse_source(value)?),
            "--expect-scope" => expected_scope = Some(parse_fingerprint(value)?),
            "--registry" => registry_path = Some(parse_absolute_path(value)?),
            "--expect-registry-sha256" => {
                expected_registry_sha256 = Some(parse_sha256(value, flag)?);
            }
            "--provider-id" => {
                validate_text(value, MAX_PROVIDER_ID_BYTES, "provider id")
                    .map_err(|_| argument_error("--provider-id is invalid"))?;
                provider_id = Some(value.to_string());
            }
            "--model" => model_path = Some(parse_absolute_path(value)?),
            "--expect-model-sha256" => {
                expected_model_sha256 = Some(parse_sha256(value, flag)?);
            }
            "--request" => request_path = Some(parse_absolute_path(value)?),
            _ => return Err(argument_error("unsupported run argument")),
        }
        index += consumed;
    }
    Ok(ParseOutcome::Command(Command::Run(RunArgs {
        source: source.ok_or_else(|| argument_error("--source is required"))?,
        expected_scope: expected_scope
            .ok_or_else(|| argument_error("--expect-scope is required"))?,
        registry_path: registry_path.ok_or_else(|| argument_error("--registry is required"))?,
        expected_registry_sha256: expected_registry_sha256
            .ok_or_else(|| argument_error("--expect-registry-sha256 is required"))?,
        provider_id: provider_id.ok_or_else(|| argument_error("--provider-id is required"))?,
        model_path: model_path.ok_or_else(|| argument_error("--model is required"))?,
        expected_model_sha256: expected_model_sha256
            .ok_or_else(|| argument_error("--expect-model-sha256 is required"))?,
        request_path: request_path.ok_or_else(|| argument_error("--request is required"))?,
    })))
}

fn help_requested(arguments: &[String]) -> bool {
    arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
}

fn option_value<'a>(
    arguments: &'a [String],
    index: usize,
    seen: &mut BTreeSet<String>,
) -> Result<(&'a str, &'a str, usize), CliError> {
    let argument = &arguments[index];
    let (flag, value, consumed) = if let Some((flag, value)) = argument.split_once('=') {
        if value.is_empty() {
            return Err(argument_error("option requires a value"));
        }
        (flag, value, 1)
    } else {
        let value = arguments
            .get(index + 1)
            .filter(|value| !value.starts_with('-'))
            .ok_or_else(|| argument_error("option requires a value"))?;
        (argument.as_str(), value.as_str(), 2)
    };
    if !flag.starts_with("--") {
        return Err(argument_error("positional arguments are unsupported"));
    }
    if !seen.insert(flag.to_string()) {
        return Err(argument_error("duplicate option"));
    }
    Ok((flag, value, consumed))
}

fn parse_source(value: &str) -> Result<ReviewSource, CliError> {
    match value {
        "staged" => Ok(ReviewSource::Staged),
        "unstaged" => Ok(ReviewSource::Unstaged),
        "branch" => Ok(ReviewSource::Branch),
        _ => Err(argument_error("--source is invalid")),
    }
}

fn parse_fingerprint(value: &str) -> Result<String, CliError> {
    if !matches!(value.len(), 40 | 64)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(argument_error("--expect-scope is invalid"));
    }
    Ok(value.to_string())
}

fn parse_sha256(value: &str, flag: &str) -> Result<String, CliError> {
    validate_sha256(value, "CLI digest")
        .map_err(|_| argument_error(format!("{flag} is invalid")))?;
    Ok(value.to_string())
}

fn parse_absolute_path(value: &str) -> Result<PathBuf, CliError> {
    let path = PathBuf::from(value);
    validate_absolute_path(&path, "CLI path")
        .map_err(|_| argument_error("CLI paths must be absolute and normalized"))?;
    Ok(path)
}

fn parse_limit(flag: &str, value: &str, maximum: usize) -> Result<usize, CliError> {
    let value = value
        .parse::<usize>()
        .map_err(|_| argument_error(format!("{flag} must be an integer")))?;
    if value == 0 || value > maximum {
        return Err(argument_error(format!("{flag} is outside its maximum")));
    }
    Ok(value)
}

fn run_model(arguments: ModelArgs) -> Result<String, CliError> {
    let repository = env::current_dir().map_err(|_| {
        CliError::new(
            "provider-cli-scope-invalid",
            "current directory cannot be resolved",
        )
    })?;
    let scope = open_authoritative_scope_bounded(
        ScopeRequest {
            repository,
            source: Some(arguments.source),
            expected_fingerprint: Some(arguments.expected_scope),
        },
        SCOPE_DEADLINE,
    )
    .map_err(|_| {
        CliError::new(
            "provider-cli-scope-invalid",
            "authoritative scope cannot be opened",
        )
    })?;
    revalidate_scope_bounded(&scope, SCOPE_DEADLINE).map_err(|_| {
        CliError::new(
            "provider-cli-scope-invalid",
            "authoritative scope changed before snapshot materialization",
        )
    })?;
    let snapshot = CandidateSnapshot::materialize(
        &scope.repository,
        arguments.source,
        SnapshotLimits {
            max_files: arguments.maximum_model_files,
            max_bytes: arguments.maximum_model_bytes as u64,
        },
    )
    .map_err(|_| {
        CliError::new(
            "provider-cli-snapshot-invalid",
            "candidate snapshot cannot be materialized",
        )
    })?;
    revalidate_scope_bounded(&scope, SCOPE_DEADLINE).map_err(|_| {
        CliError::new(
            "provider-cli-scope-invalid",
            "authoritative scope changed during snapshot materialization",
        )
    })?;
    let defaults = ProviderModelLimits::default();
    let model = build_linked_project_model(
        &snapshot,
        ProviderModelLimits {
            max_files: arguments.maximum_model_files,
            max_bytes: arguments.maximum_model_bytes,
            max_file_bytes: defaults.max_file_bytes.min(arguments.maximum_model_bytes),
        },
    )
    .map_err(|_| {
        CliError::new(
            "provider-cli-model-invalid",
            "linked project model cannot be constructed",
        )
    })?;
    revalidate_scope_bounded(&scope, SCOPE_DEADLINE).map_err(|_| {
        CliError::new(
            "provider-cli-scope-invalid",
            "authoritative scope changed during model construction",
        )
    })?;
    snapshot.verify_unchanged().map_err(|_| {
        CliError::new(
            "provider-cli-snapshot-invalid",
            "candidate snapshot changed during model construction",
        )
    })?;
    serde_json::to_string(&model).map_err(|_| {
        CliError::new(
            "provider-cli-output-invalid",
            "linked project model cannot be serialized",
        )
    })
}

fn argument_error(message: impl AsRef<str>) -> CliError {
    CliError::new("provider-cli-argument-invalid", message)
}

fn emit_error(error: &CliError, exit_code: i32) -> i32 {
    eprintln!(
        "repository-context-provider-cli: {}: {}",
        error.code, error.message
    );
    exit_code
}

fn bounded_detail(value: &str) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let value = value.chars().take(400).collect::<String>();
    if value.is_empty() {
        "operation failed".to_string()
    } else {
        value
    }
}
