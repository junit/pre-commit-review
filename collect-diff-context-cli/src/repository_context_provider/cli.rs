use crate::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
use crate::repository_context_provider::cli_contract::{
    ProviderRegistry, ProviderRegistryEntry, ProviderRunRequest,
};
use crate::repository_context_provider::contract::{
    sha256_json, validate_absolute_path, validate_sha256, validate_text, AuthorizedProviderProfile,
    CandidateBinding, ProviderBinding, RepositoryContextProviderRequest, RustAnalyzerProjectModel,
    MAX_REPORT_BYTES,
};
use crate::repository_context_provider::model::{build_linked_project_model, ProviderModelLimits};
use crate::repository_context_provider::snapshot::BoundCandidateSnapshot;
use crate::repository_context_provider::{
    run_repository_context_provider, ProviderError, ProviderInvocation,
};
use crate::review_scope::{
    open_authoritative_scope_bounded, revalidate_scope_bounded, AuthoritativeScope, ReviewSource,
    ScopeRequest,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

const HELP: &str = "Usage:\n  repository-context-provider-cli model --source <staged|unstaged|branch> --expect-scope <fingerprint> [options]\n  repository-context-provider-cli run --source <staged|unstaged|branch> --expect-scope <fingerprint> --registry <absolute-path> --expect-registry-sha256 <sha256> --provider-id <id> --model <absolute-path> --expect-model-sha256 <sha256> --request <absolute-path>\n";
const MODEL_HELP: &str = "Usage: repository-context-provider-cli model --source <staged|unstaged|branch> --expect-scope <fingerprint> [options]\n\nOptions:\n  --max-model-files <positive bounded integer>\n  --max-model-bytes <positive bounded integer>\n  -h, --help\n";
const RUN_HELP: &str = "Usage: repository-context-provider-cli run --source <staged|unstaged|branch> --expect-scope <fingerprint> --registry <absolute-path> --expect-registry-sha256 <sha256> --provider-id <id> --model <absolute-path> --expect-model-sha256 <sha256> --request <absolute-path>\n\nOptions:\n  -h, --help\n";
const SCOPE_DEADLINE: Duration = Duration::from_secs(30);
const MAX_PROVIDER_ID_BYTES: usize = 256;
pub(crate) const MAX_REGISTRY_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_PROFILE_BYTES: usize = 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_EXECUTABLE_BYTES: usize = 512 * 1024 * 1024;

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

struct RunFailure {
    error: CliError,
    exit_code: i32,
}

pub(crate) struct ValidatedProviderInstallation {
    pub(crate) entry: ProviderRegistryEntry,
    pub(crate) profile: AuthorizedProviderProfile,
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
        Ok(ParseOutcome::Command(Command::Run(arguments))) => match run_provider(arguments) {
            Ok(output) => {
                println!("{output}");
                0
            }
            Err(failure) => emit_error(&failure.error, failure.exit_code),
        },
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

fn run_provider(arguments: RunArgs) -> Result<String, RunFailure> {
    let repository = env::current_dir().map_err(|_| {
        authorization_failure(
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
        authorization_failure(
            "provider-cli-scope-invalid",
            "authoritative scope cannot be opened",
        )
    })?;

    let (registry, registry_sha256) =
        read_json_once::<ProviderRegistry>(&arguments.registry_path, MAX_REGISTRY_BYTES).map_err(
            |_| {
                authorization_failure(
                    "provider-cli-registry-invalid",
                    "provider registry cannot be loaded",
                )
            },
        )?;
    if registry_sha256 != arguments.expected_registry_sha256 {
        return Err(authorization_failure(
            "provider-cli-registry-invalid",
            "provider registry digest does not match the authorized input",
        ));
    }
    registry.validate().map_err(|_| {
        authorization_failure(
            "provider-cli-registry-invalid",
            "provider registry contract validation failed",
        )
    })?;
    let mut entry = registry
        .select(&arguments.provider_id)
        .map_err(|_| {
            authorization_failure(
                "provider-cli-registry-invalid",
                "provider registry entry is unavailable",
            )
        })?
        .clone();

    let (model, model_file_sha256) =
        read_json_once::<RustAnalyzerProjectModel>(&arguments.model_path, MAX_REPORT_BYTES)
            .map_err(|_| {
                authorization_failure(
                    "provider-cli-model-invalid",
                    "linked project model cannot be loaded",
                )
            })?;
    if model_file_sha256 != arguments.expected_model_sha256 {
        return Err(authorization_failure(
            "provider-cli-model-invalid",
            "linked project model file digest does not match the authorized input",
        ));
    }
    model.validate().map_err(|_| {
        authorization_failure(
            "provider-cli-model-invalid",
            "linked project model contract validation failed",
        )
    })?;

    let (run_request, _) =
        read_json_once::<ProviderRunRequest>(&arguments.request_path, MAX_REQUEST_BYTES).map_err(
            |_| {
                authorization_failure(
                    "provider-cli-request-invalid",
                    "provider run request cannot be loaded",
                )
            },
        )?;
    run_request.validate().map_err(|_| {
        authorization_failure(
            "provider-cli-request-invalid",
            "provider run request contract validation failed",
        )
    })?;

    let (profile, profile_file_sha256) =
        read_json_once::<AuthorizedProviderProfile>(&entry.profile_path, MAX_PROFILE_BYTES)
            .map_err(|_| {
                authorization_failure(
                    "provider-cli-profile-invalid",
                    "authorized provider profile cannot be loaded",
                )
            })?;
    let validated = validate_provider_installation(&entry, profile, &profile_file_sha256)
        .map_err(provider_installation_failure)?;
    entry = validated.entry;
    let profile = validated.profile;

    validate_entry_bindings(&entry, &profile, &model).map_err(|_| {
        authorization_failure(
            "provider-cli-binding-invalid",
            "registry, profile, executable, and model bindings do not match",
        )
    })?;
    run_request
        .validate_against(&profile.maximum_limits)
        .map_err(|_| {
            authorization_failure(
                "provider-cli-request-invalid",
                "provider run request exceeds the authorized profile",
            )
        })?;
    revalidate_scope_bounded(&scope, SCOPE_DEADLINE).map_err(|_| {
        authorization_failure(
            "provider-cli-scope-invalid",
            "authoritative scope changed during input validation",
        )
    })?;

    let snapshot = CandidateSnapshot::materialize(
        &scope.repository,
        arguments.source,
        SnapshotLimits {
            max_files: ProviderModelLimits::default().max_files,
            max_bytes: run_request.limits.max_source_bytes as u64,
        },
    )
    .map_err(|_| {
        authorization_failure(
            "provider-cli-snapshot-invalid",
            "candidate snapshot cannot be materialized",
        )
    })?;
    revalidate_scope_bounded(&scope, SCOPE_DEADLINE).map_err(|_| {
        authorization_failure(
            "provider-cli-scope-invalid",
            "authoritative scope changed during snapshot materialization",
        )
    })?;
    let request = build_provider_request(
        &scope,
        &registry,
        &entry,
        &model,
        &run_request,
        &snapshot,
        &profile,
    )
    .map_err(|_| {
        authorization_failure(
            "provider-cli-binding-invalid",
            "owned provider request construction failed",
        )
    })?;
    let report = run_repository_context_provider(ProviderInvocation {
        snapshot: &snapshot,
        model: &model,
        request: &request,
        profile: &profile,
        cancellation: Arc::new(AtomicBool::new(false)),
    })
    .map_err(provider_failure)?;
    revalidate_scope_bounded(&scope, SCOPE_DEADLINE).map_err(|_| {
        authorization_failure(
            "provider-cli-scope-invalid",
            "authoritative scope changed during provider execution",
        )
    })?;
    snapshot.verify_unchanged().map_err(|_| {
        authorization_failure(
            "provider-cli-snapshot-invalid",
            "candidate snapshot changed during provider execution",
        )
    })?;
    report.validate().map_err(|_| {
        runtime_failure(
            "provider-cli-report-invalid",
            "provider report contract validation failed",
        )
    })?;
    let output = serde_json::to_string(&report).map_err(|_| {
        runtime_failure(
            "provider-cli-report-invalid",
            "provider report cannot be serialized",
        )
    })?;
    if output.len() > run_request.limits.max_report_bytes {
        return Err(runtime_failure(
            "provider-cli-report-invalid",
            "provider report exceeds the requested byte maximum",
        ));
    }
    Ok(output)
}

pub fn read_json_once<T: DeserializeOwned>(
    path: &Path,
    maximum_bytes: usize,
) -> Result<(T, String), CliError> {
    if maximum_bytes == 0 {
        return Err(CliError::new(
            "provider-cli-json-invalid",
            "JSON byte maximum must be positive",
        ));
    }
    let canonical = canonical_regular_file(path)?;
    let metadata = fs::metadata(&canonical).map_err(|_| {
        CliError::new(
            "provider-cli-json-invalid",
            "JSON input metadata cannot be read",
        )
    })?;
    let expected_bytes = usize::try_from(metadata.len()).map_err(|_| {
        CliError::new(
            "provider-cli-json-invalid",
            "JSON input length exceeds this platform",
        )
    })?;
    if expected_bytes > maximum_bytes {
        return Err(CliError::new(
            "provider-cli-json-invalid",
            "JSON input exceeds its byte maximum",
        ));
    }
    let maximum_read = u64::try_from(maximum_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut input = File::open(&canonical)
        .map_err(|_| CliError::new("provider-cli-json-invalid", "JSON input cannot be opened"))?
        .take(maximum_read);
    let mut bytes = Vec::with_capacity(expected_bytes);
    input
        .read_to_end(&mut bytes)
        .map_err(|_| CliError::new("provider-cli-json-invalid", "JSON input cannot be read"))?;
    if bytes.len() != expected_bytes || bytes.len() > maximum_bytes {
        return Err(CliError::new(
            "provider-cli-json-invalid",
            "JSON input changed while it was read",
        ));
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let value = serde_json::from_slice(&bytes).map_err(|_| {
        CliError::new(
            "provider-cli-json-invalid",
            "JSON input does not match its strict contract",
        )
    })?;
    Ok((value, digest))
}

pub(crate) fn validate_provider_installation(
    entry: &ProviderRegistryEntry,
    profile: AuthorizedProviderProfile,
    profile_file_sha256: &str,
) -> Result<ValidatedProviderInstallation, CliError> {
    profile.validate().map_err(|_| {
        CliError::new(
            "provider-cli-profile-invalid",
            "authorized provider profile contract validation failed",
        )
    })?;
    if profile_file_sha256 != entry.profile_sha256 || profile_file_sha256 != profile.sha256() {
        return Err(CliError::new(
            "provider-cli-profile-invalid",
            "authorized provider profile digest does not match the registry",
        ));
    }
    let profile_path = canonical_regular_file(&entry.profile_path).map_err(|_| {
        CliError::new(
            "provider-cli-profile-invalid",
            "authorized provider profile path is invalid",
        )
    })?;
    let (executable_path, executable_sha256) =
        read_file_sha256(&entry.executable_path, MAX_EXECUTABLE_BYTES).map_err(|_| {
            CliError::new(
                "provider-cli-executable-invalid",
                "authorized provider executable cannot be loaded",
            )
        })?;
    if executable_sha256 != entry.executable_sha256
        || executable_sha256 != profile.executable_sha256
    {
        return Err(CliError::new(
            "provider-cli-executable-invalid",
            "authorized provider executable digest does not match the registry",
        ));
    }
    ensure_executable(&executable_path).map_err(|_| {
        CliError::new(
            "provider-cli-executable-invalid",
            "authorized provider executable is not executable",
        )
    })?;
    validate_entry_profile_bindings(entry, &profile)?;

    let mut entry = entry.clone();
    entry.profile_path = profile_path;
    entry.executable_path = executable_path;
    Ok(ValidatedProviderInstallation { entry, profile })
}

fn build_provider_request(
    scope: &AuthoritativeScope,
    registry: &ProviderRegistry,
    entry: &ProviderRegistryEntry,
    model: &RustAnalyzerProjectModel,
    run_request: &ProviderRunRequest,
    snapshot: &CandidateSnapshot,
    profile: &AuthorizedProviderProfile,
) -> Result<RepositoryContextProviderRequest, CliError> {
    registry.validate().map_err(|_| {
        CliError::new(
            "provider-cli-binding-invalid",
            "provider registry is invalid",
        )
    })?;
    profile.validate().map_err(|_| {
        CliError::new(
            "provider-cli-binding-invalid",
            "provider profile is invalid",
        )
    })?;
    model.validate().map_err(|_| {
        CliError::new(
            "provider-cli-binding-invalid",
            "linked project model is invalid",
        )
    })?;
    run_request
        .validate_against(&profile.maximum_limits)
        .map_err(|_| {
            CliError::new(
                "provider-cli-binding-invalid",
                "provider run request exceeds its profile",
            )
        })?;
    if scope.source != snapshot.source() {
        return Err(CliError::new(
            "provider-cli-binding-invalid",
            "scope source does not match the candidate snapshot",
        ));
    }
    validate_entry_bindings(entry, profile, model)?;
    let snapshot_root = fs::canonicalize(snapshot.path()).map_err(|_| {
        CliError::new(
            "provider-cli-binding-invalid",
            "candidate snapshot root cannot be canonicalized",
        )
    })?;
    let request = RepositoryContextProviderRequest {
        schema_version: 1,
        kind: "repository_context_provider_request".to_string(),
        candidate: CandidateBinding {
            source: scope.source,
            scope_fingerprint: scope.fingerprint.clone(),
            candidate_digest: candidate_digest(scope, snapshot),
            snapshot_root,
            snapshot_sha256: snapshot.sha256.clone(),
            snapshot_files: snapshot.files,
            snapshot_bytes: snapshot.bytes,
            project_model_digest: model.digest.clone(),
        },
        provider: ProviderBinding {
            kind: entry.provider_kind.clone(),
            version: entry.provider_version.clone(),
            profile_path: entry.profile_path.clone(),
            profile_sha256: entry.profile_sha256.clone(),
            executable_path: entry.executable_path.clone(),
            executable_sha256: entry.executable_sha256.clone(),
            configuration_sha256: entry.configuration_sha256.clone(),
            target_triple: entry.target_triple.clone(),
            toolchain_mode: entry.toolchain_mode.clone(),
        },
        seeds: run_request.seeds.clone(),
        directions: run_request.directions.clone(),
        limits: run_request.limits,
    };
    request.validate().map_err(|_| {
        CliError::new(
            "provider-cli-binding-invalid",
            "owned provider request is invalid",
        )
    })?;
    profile.validate_request(&request).map_err(|_| {
        CliError::new(
            "provider-cli-binding-invalid",
            "owned provider request is not authorized by the profile",
        )
    })?;
    BoundCandidateSnapshot::new(snapshot, model, &request.candidate).map_err(|_| {
        CliError::new(
            "provider-cli-binding-invalid",
            "linked project model does not match the candidate snapshot",
        )
    })?;
    Ok(request)
}

fn validate_entry_bindings(
    entry: &ProviderRegistryEntry,
    profile: &AuthorizedProviderProfile,
    model: &RustAnalyzerProjectModel,
) -> Result<(), CliError> {
    validate_entry_profile_bindings(entry, profile)?;
    if model.target_triple != profile.target_triple {
        return Err(CliError::new(
            "provider-cli-binding-invalid",
            "registry entry does not match the profile and project model",
        ));
    }
    Ok(())
}

fn validate_entry_profile_bindings(
    entry: &ProviderRegistryEntry,
    profile: &AuthorizedProviderProfile,
) -> Result<(), CliError> {
    if entry.provider_kind != profile.provider_kind
        || entry.provider_version != profile.provider_version
        || entry.profile_sha256 != profile.sha256()
        || entry.executable_sha256 != profile.executable_sha256
        || entry.configuration_sha256 != profile.configuration_sha256
        || entry.target_triple != profile.target_triple
        || entry.toolchain_mode != profile.toolchain_mode
    {
        return Err(CliError::new(
            "provider-cli-binding-invalid",
            "registry entry does not match the profile and project model",
        ));
    }
    Ok(())
}

fn candidate_digest(scope: &AuthoritativeScope, snapshot: &CandidateSnapshot) -> String {
    #[derive(Serialize)]
    struct CandidateIdentity<'a> {
        algorithm: &'static str,
        source: ReviewSource,
        scope_fingerprint: &'a str,
        snapshot_sha256: &'a str,
        snapshot_files: usize,
        snapshot_bytes: u64,
    }
    sha256_json(&CandidateIdentity {
        algorithm: "repository-context-provider-candidate/v1",
        source: scope.source,
        scope_fingerprint: &scope.fingerprint,
        snapshot_sha256: &snapshot.sha256,
        snapshot_files: snapshot.files,
        snapshot_bytes: snapshot.bytes,
    })
}

fn canonical_regular_file(path: &Path) -> Result<PathBuf, CliError> {
    validate_absolute_path(path, "CLI input path").map_err(|_| {
        CliError::new(
            "provider-cli-path-invalid",
            "CLI input path must be absolute and normalized",
        )
    })?;
    let lexical_metadata = fs::symlink_metadata(path).map_err(|_| {
        CliError::new(
            "provider-cli-path-invalid",
            "CLI input path cannot be inspected",
        )
    })?;
    if lexical_metadata.file_type().is_dir() {
        return Err(CliError::new(
            "provider-cli-path-invalid",
            "CLI input path must name a regular file",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        CliError::new(
            "provider-cli-path-invalid",
            "CLI input path has no trusted parent",
        )
    })?;
    let canonical_parent = fs::canonicalize(parent).map_err(|_| {
        CliError::new(
            "provider-cli-path-invalid",
            "CLI input parent cannot be canonicalized",
        )
    })?;
    let canonical = fs::canonicalize(path).map_err(|_| {
        CliError::new(
            "provider-cli-path-invalid",
            "CLI input path cannot be canonicalized",
        )
    })?;
    if canonical == canonical_parent || !canonical.starts_with(&canonical_parent) {
        return Err(CliError::new(
            "provider-cli-path-invalid",
            "CLI input symlink escapes its trusted parent",
        ));
    }
    let metadata = fs::symlink_metadata(&canonical).map_err(|_| {
        CliError::new(
            "provider-cli-path-invalid",
            "canonical CLI input cannot be inspected",
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(CliError::new(
            "provider-cli-path-invalid",
            "canonical CLI input is not a regular file",
        ));
    }
    Ok(canonical)
}

fn read_file_sha256(path: &Path, maximum_bytes: usize) -> Result<(PathBuf, String), CliError> {
    let canonical = canonical_regular_file(path)?;
    let expected_bytes = fs::metadata(&canonical)
        .map_err(|_| {
            CliError::new(
                "provider-cli-file-invalid",
                "authorized file metadata cannot be read",
            )
        })?
        .len();
    if expected_bytes > maximum_bytes as u64 {
        return Err(CliError::new(
            "provider-cli-file-invalid",
            "authorized file exceeds its byte maximum",
        ));
    }
    let mut input = File::open(&canonical).map_err(|_| {
        CliError::new(
            "provider-cli-file-invalid",
            "authorized file cannot be opened",
        )
    })?;
    let mut digest = Sha256::new();
    let mut observed_bytes = 0_u64;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = input.read(&mut buffer).map_err(|_| {
            CliError::new(
                "provider-cli-file-invalid",
                "authorized file cannot be read",
            )
        })?;
        if read == 0 {
            break;
        }
        observed_bytes = observed_bytes.checked_add(read as u64).ok_or_else(|| {
            CliError::new(
                "provider-cli-file-invalid",
                "authorized file byte count overflowed",
            )
        })?;
        if observed_bytes > maximum_bytes as u64 {
            return Err(CliError::new(
                "provider-cli-file-invalid",
                "authorized file exceeds its byte maximum",
            ));
        }
        digest.update(&buffer[..read]);
    }
    if observed_bytes != expected_bytes {
        return Err(CliError::new(
            "provider-cli-file-invalid",
            "authorized file changed while it was read",
        ));
    }
    Ok((canonical, format!("{:x}", digest.finalize())))
}

fn ensure_executable(path: &Path) -> Result<(), CliError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)
            .map_err(|_| {
                CliError::new(
                    "provider-cli-executable-invalid",
                    "provider executable metadata cannot be read",
                )
            })?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            return Err(CliError::new(
                "provider-cli-executable-invalid",
                "provider executable has no execute permission",
            ));
        }
    }
    Ok(())
}

fn provider_failure(error: ProviderError) -> RunFailure {
    match error {
        ProviderError::InvalidRequest
        | ProviderError::ProfileMismatch
        | ProviderError::StaleBinding => authorization_failure(
            "provider-cli-binding-invalid",
            "provider authorization changed during execution",
        ),
        ProviderError::Cancelled => {
            runtime_failure("provider-cli-cancelled", "provider execution was cancelled")
        }
        ProviderError::Preflight | ProviderError::Session | ProviderError::ReportInvalid => {
            runtime_failure(
                "provider-cli-execution-failed",
                "provider execution failed before a safe report was available",
            )
        }
    }
}

fn provider_installation_failure(error: CliError) -> RunFailure {
    match error.code {
        "provider-cli-profile-invalid" => authorization_failure(
            "provider-cli-profile-invalid",
            "authorized provider profile is invalid",
        ),
        "provider-cli-executable-invalid" => authorization_failure(
            "provider-cli-executable-invalid",
            "authorized provider executable is invalid",
        ),
        _ => authorization_failure(
            "provider-cli-binding-invalid",
            "registry, profile, and executable bindings do not match",
        ),
    }
}

fn authorization_failure(code: &'static str, message: &'static str) -> RunFailure {
    RunFailure {
        error: CliError::new(code, message),
        exit_code: 2,
    }
}

fn runtime_failure(code: &'static str, message: &'static str) -> RunFailure {
    RunFailure {
        error: CliError::new(code, message),
        exit_code: 3,
    }
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
