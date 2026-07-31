mod fixture_tree;

use crate::artifacts::cache::verify_target_receipt;
use crate::artifacts::contract::{canonical_json, sha256_bytes, ArtifactManifest, SourceLock};
use crate::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
use crate::git_policy::output_bounded;
use crate::impact_context::cache::file_facts::open_regular_file_no_follow;
use crate::repository_context_provider::cli_contract::{ProviderRegistry, ProviderRunRequest};
use crate::repository_context_provider::contract::{
    AuthorizedProviderProfile, CallDirection, ProviderLimits, ProviderRange, ProviderRangeFormat,
    RepositoryContextProviderReport, RepositoryContextProviderStatus, SeedKind, SeedSymbol,
};
use crate::repository_context_provider::model::{build_linked_project_model, ProviderModelLimits};
use crate::repository_context_provider::{
    run_repository_context_provider_measured, ProviderInvocation,
};
use crate::review_scope::{open_authoritative_scope_bounded, ReviewSource, ScopeRequest};
use serde::Serialize;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tempfile::TempDir;

use self::fixture_tree::FixtureInventory;

const HELP: &str = "Usage:\n  provider-baseline-sample-runner contract --target-root <absolute-path> --source-lock <absolute-path> --fixture-root <absolute-path> --runner-class <id> --output <absolute-path>\n  provider-baseline-sample-runner sample --target-root <absolute-path> --source-lock <absolute-path> --fixture-root <absolute-path> --runner-class <id>\n";
const SOURCE_LOCK_SHA256: &str = "298bc6c0339fe2c58fd35bfbd53db285ea7ff34e40734a4f0c36ccb3fe60d862";
const PACK_VERSION: &str = "2026.07.27-pcr.3";
const TOOLCHAIN: &str = "rust-1.95.0-locked";
const TIMING_SCOPE: &str = "provider-run-only-v1";
const MAX_JSON_BYTES: u64 = 1024 * 1024;
const MAX_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;
const SAMPLE_TOTAL_DEADLINE: Duration = Duration::from_secs(30);
const PREPARATION_GIT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct RunnerError {
    code: &'static str,
    message: String,
}

impl RunnerError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into().chars().take(512).collect(),
        }
    }
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RunnerError {}

type Result<T> = std::result::Result<T, RunnerError>;

#[derive(Debug)]
enum Action {
    Help,
    Contract(Arguments),
    Sample(Arguments),
}

#[derive(Debug)]
struct Arguments {
    target_root: PathBuf,
    source_lock: PathBuf,
    fixture_root: PathBuf,
    runner_class: String,
    output: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Identity {
    platform_id: String,
    pack_version: String,
    pack_sha256: String,
    executable_sha256: String,
    source_lock_sha256: String,
    profile_sha256: String,
    fixture_id: String,
    fixture_sha256: String,
    request_sha256: String,
    runner_class: String,
    toolchain: String,
    timing_scope: String,
    provisioning_included: bool,
}

#[derive(Serialize)]
struct RunnerContract {
    schema_version: u8,
    kind: &'static str,
    command: Vec<String>,
    current_directory: String,
    environment: BTreeMap<String, String>,
    expected: Identity,
}

#[derive(Serialize)]
struct Sample {
    schema_version: u8,
    kind: &'static str,
    #[serde(flatten)]
    identity: Identity,
    elapsed_ms: u64,
    peak_process_tree_rss_bytes: u64,
}

struct PreparedRun {
    _repository: TempDir,
    snapshot: CandidateSnapshot,
    model: crate::repository_context_provider::contract::RustAnalyzerProjectModel,
    request: crate::repository_context_provider::contract::RepositoryContextProviderRequest,
    profile: AuthorizedProviderProfile,
    identity: Identity,
}

struct SampleDeadline {
    cancellation: Arc<AtomicBool>,
    deadline: Instant,
    stop: Option<mpsc::Sender<()>>,
    watchdog: Option<JoinHandle<()>>,
}

impl SampleDeadline {
    fn start(duration: Duration) -> Self {
        let deadline = Instant::now()
            .checked_add(duration)
            .unwrap_or_else(Instant::now);
        let cancellation = Arc::new(AtomicBool::new(duration.is_zero()));
        if duration.is_zero() {
            return Self {
                cancellation,
                deadline,
                stop: None,
                watchdog: None,
            };
        }
        let (stop, receiver) = mpsc::channel();
        let watched_cancellation = Arc::clone(&cancellation);
        let watchdog = std::thread::spawn(move || {
            if receiver.recv_timeout(duration) == Err(mpsc::RecvTimeoutError::Timeout) {
                watched_cancellation.store(true, Ordering::Release);
            }
        });
        Self {
            cancellation,
            deadline,
            stop: Some(stop),
            watchdog: Some(watchdog),
        }
    }

    fn check(&self) -> Result<()> {
        if self.cancellation.load(Ordering::Acquire) || Instant::now() >= self.deadline {
            Err(deadline_error())
        } else {
            Ok(())
        }
    }

    fn remaining(&self) -> Result<Duration> {
        self.check()?;
        Ok(self.deadline.saturating_duration_since(Instant::now()))
    }
}

impl Drop for SampleDeadline {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(watchdog) = self.watchdog.take() {
            let _ = watchdog.join();
        }
    }
}

pub fn main_entry() -> i32 {
    match parse_arguments(env::args().skip(1).collect()).and_then(|action| match action {
        Action::Help => {
            print!("{HELP}");
            Ok(())
        }
        Action::Contract(arguments) => write_contract(arguments),
        Action::Sample(arguments) => write_sample(arguments),
    }) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!(
                "provider baseline sample runner failed: {}: {}",
                error.code, error
            );
            1
        }
    }
}

fn parse_arguments(arguments: Vec<String>) -> Result<Action> {
    if matches!(arguments.as_slice(), [value] if value == "--help" || value == "-h") {
        return Ok(Action::Help);
    }
    let action = match arguments.first().map(String::as_str) {
        Some("contract") => "contract",
        Some("sample") => "sample",
        _ => return Err(argument_error("expected contract or sample subcommand")),
    };
    let mut values = BTreeMap::new();
    let mut index = 1;
    while index < arguments.len() {
        let flag = arguments[index].as_str();
        let Some(value) = arguments.get(index + 1) else {
            return Err(argument_error("every option requires one value"));
        };
        if !matches!(
            flag,
            "--target-root" | "--source-lock" | "--fixture-root" | "--runner-class" | "--output"
        ) || values.insert(flag, value.as_str()).is_some()
        {
            return Err(argument_error("runner options are invalid or duplicated"));
        }
        index += 2;
    }
    let target_root = absolute_directory(required(&values, "--target-root")?)?;
    let source_lock = absolute_file(required(&values, "--source-lock")?)?;
    let fixture_root = absolute_directory(required(&values, "--fixture-root")?)?;
    let runner_class = required(&values, "--runner-class")?.to_string();
    if runner_class.is_empty()
        || runner_class.len() > 128
        || !runner_class
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(argument_error("runner class is invalid"));
    }
    validate_runner_class(current_platform()?, &runner_class)?;
    let output = values
        .get("--output")
        .map(|value| absolute_output(value))
        .transpose()?;
    if action == "contract" && output.is_none() {
        return Err(argument_error("contract requires --output"));
    }
    if action == "sample" && output.is_some() {
        return Err(argument_error("sample does not accept --output"));
    }
    let parsed = Arguments {
        target_root,
        source_lock,
        fixture_root,
        runner_class,
        output,
    };
    if action == "contract" {
        Ok(Action::Contract(parsed))
    } else {
        Ok(Action::Sample(parsed))
    }
}

fn required<'a>(values: &'a BTreeMap<&str, &str>, name: &str) -> Result<&'a str> {
    values
        .get(name)
        .copied()
        .ok_or_else(|| argument_error("required runner option is missing"))
}

fn absolute_directory(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if !path.is_absolute() || path.is_symlink() || !path.is_dir() {
        return Err(argument_error(
            "runner directory must be absolute and regular",
        ));
    }
    fs::canonicalize(path).map_err(|_| argument_error("runner directory cannot be resolved"))
}

fn absolute_output(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if !path.is_absolute() || path.is_symlink() {
        return Err(argument_error(
            "runner output must be an absolute non-symlink path",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| argument_error("runner output has no parent"))?;
    if !parent.is_dir() || parent.is_symlink() {
        return Err(argument_error("runner output parent is invalid"));
    }
    Ok(path)
}

fn absolute_file(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if !path.is_absolute() || path.is_symlink() || !path.is_file() {
        return Err(argument_error("runner file must be absolute and regular"));
    }
    fs::canonicalize(path).map_err(|_| argument_error("runner file cannot be resolved"))
}

fn write_contract(arguments: Arguments) -> Result<()> {
    let prepared = prepare(&arguments, None)?;
    let executable =
        env::current_exe().map_err(|_| runner_error("runner executable cannot be resolved"))?;
    if executable.is_symlink() || !executable.is_file() {
        return Err(runner_error("runner executable is not a regular file"));
    }
    let current_directory = env::current_dir()
        .ok()
        .and_then(|path| fs::canonicalize(path).ok())
        .ok_or_else(|| runner_error("runner current directory cannot be resolved"))?;
    let git = resolve_git()?;
    let git_directory = git
        .parent()
        .ok_or_else(|| runner_error("git executable has no parent directory"))?;
    let mut environment = BTreeMap::new();
    environment.insert(
        "PATH".to_string(),
        env::join_paths([git_directory])
            .map_err(|_| runner_error("git directory cannot form PATH"))?
            .to_string_lossy()
            .into_owned(),
    );
    for key in [
        "SystemRoot",
        "TMPDIR",
        "TMP",
        "TEMP",
        "GITHUB_ACTIONS",
        "GITHUB_REPOSITORY",
        "RUNNER_OS",
        "RUNNER_ARCH",
        "ImageOS",
    ] {
        if let Ok(value) = env::var(key) {
            environment.insert(key.to_string(), value);
        }
    }
    environment.insert(
        "GIT_CONFIG_GLOBAL".to_string(),
        if cfg!(windows) { "NUL" } else { "/dev/null" }.to_string(),
    );
    environment.insert("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string());
    environment.insert("GIT_TERMINAL_PROMPT".to_string(), "0".to_string());
    environment.insert("LC_ALL".to_string(), "C".to_string());
    let command = vec![
        executable.to_string_lossy().into_owned(),
        "sample".to_string(),
        "--target-root".to_string(),
        arguments.target_root.to_string_lossy().into_owned(),
        "--source-lock".to_string(),
        arguments.source_lock.to_string_lossy().into_owned(),
        "--fixture-root".to_string(),
        arguments.fixture_root.to_string_lossy().into_owned(),
        "--runner-class".to_string(),
        arguments.runner_class,
    ];
    let contract = RunnerContract {
        schema_version: 1,
        kind: "provider_baseline_runner",
        command,
        current_directory: current_directory.to_string_lossy().into_owned(),
        environment,
        expected: prepared.identity,
    };
    let output = arguments.output.expect("validated contract output");
    write_new_file(
        &output,
        &serde_json::to_vec(&contract)
            .map_err(|_| runner_error("runner contract cannot be serialized"))?,
    )
}

fn write_sample(arguments: Arguments) -> Result<()> {
    let output = env::var_os("PCR_PROVIDER_BASELINE_SAMPLE_OUTPUT")
        .ok_or_else(|| argument_error("sample output environment is missing"))?;
    let output = absolute_output(&output.to_string_lossy())?;
    let deadline = SampleDeadline::start(sample_deadline(&arguments)?);
    deadline.check()?;
    let prepared = prepare(&arguments, Some(&deadline))?;
    deadline.check()?;
    let measured = run_repository_context_provider_measured(ProviderInvocation {
        snapshot: &prepared.snapshot,
        model: &prepared.model,
        request: &prepared.request,
        profile: &prepared.profile,
        cancellation: Arc::clone(&deadline.cancellation),
    })
    .map_err(|error| {
        if deadline.check().is_err() {
            deadline_error()
        } else {
            RunnerError::new(error.code(), "real provider execution failed")
        }
    })?;
    deadline.check()?;
    validate_report(&measured.report)?;
    deadline.check()?;
    let sample = Sample {
        schema_version: 1,
        kind: "provider_baseline_sample",
        identity: prepared.identity,
        elapsed_ms: measured.elapsed_ms,
        peak_process_tree_rss_bytes: measured.report.metrics.process_tree_peak_rss_bytes,
    };
    let sample_bytes = serde_json::to_vec(&sample)
        .map_err(|_| runner_error("provider baseline sample cannot be serialized"))?;
    deadline.check()?;
    write_new_file(&output, &sample_bytes)?;
    deadline.check()
}

fn prepare(arguments: &Arguments, deadline: Option<&SampleDeadline>) -> Result<PreparedRun> {
    check_preparation_deadline(deadline)?;
    let fixture_inventory = FixtureInventory::validate(&arguments.fixture_root)?;
    check_preparation_deadline(deadline)?;
    let source_lock_bytes = read_regular(&arguments.source_lock)?;
    let source_lock_sha256 = sha256_bytes(&source_lock_bytes);
    if source_lock_sha256 != SOURCE_LOCK_SHA256 {
        return Err(binding_error("provider source lock digest differs"));
    }
    let source_lock: SourceLock = serde_json::from_slice(&source_lock_bytes)
        .map_err(|_| binding_error("provider source lock is invalid"))?;
    source_lock
        .validate()
        .map_err(|_| binding_error("provider source lock contract differs"))?;
    if source_lock.artifact_id != "rust-analyzer" || source_lock.tool_version != "2026-07-27" {
        return Err(binding_error("provider source lock identity differs"));
    }
    let distribution_manifest_path = arguments
        .target_root
        .join("runtime/distribution/manifest.json");
    let distribution_manifest_bytes = read_regular(&distribution_manifest_path)
        .map_err(|_| binding_error("target distribution manifest is unavailable"))?;
    let distribution_manifest: ArtifactManifest =
        serde_json::from_slice(&distribution_manifest_bytes)
            .map_err(|_| binding_error("target distribution manifest is invalid"))?;
    distribution_manifest
        .validate()
        .map_err(|_| binding_error("target distribution manifest contract differs"))?;
    if canonical_json(&distribution_manifest)
        .map_err(|_| binding_error("target distribution manifest cannot be serialized"))?
        != distribution_manifest_bytes
    {
        return Err(binding_error(
            "target distribution manifest is not canonical",
        ));
    }
    let receipt = verify_target_receipt(
        &arguments.target_root,
        "rust-analyzer",
        &distribution_manifest,
    )
    .map_err(|_| binding_error("target receipt does not match the distribution manifest"))?;
    let record = distribution_manifest
        .select_active("rust-analyzer", &receipt.platform_id)
        .map_err(|_| binding_error("target provider record is not active"))?;
    if record.pack_version != PACK_VERSION
        || record.platform_id != current_platform()?
        || record.source_lock_sha256 != source_lock_sha256
    {
        return Err(binding_error("target provider record identity differs"));
    }
    let registry_path = arguments
        .target_root
        .join("runtime/providers/provider-registry.json");
    let registry_bytes = read_regular(&registry_path)?;
    let registry: ProviderRegistry = serde_json::from_slice(&registry_bytes)
        .map_err(|_| binding_error("provider registry is invalid"))?;
    registry
        .validate()
        .map_err(|_| binding_error("provider registry contract differs"))?;
    let entry = registry
        .select("rust-analyzer-project-pack")
        .map_err(|_| binding_error("provider registry entry is missing"))?
        .clone();
    let profile_bytes = read_regular(&entry.profile_path)?;
    let profile: AuthorizedProviderProfile = serde_json::from_slice(&profile_bytes)
        .map_err(|_| binding_error("provider profile is invalid"))?;
    profile
        .validate()
        .map_err(|_| binding_error("provider profile contract differs"))?;
    registry
        .validate_profile_binding(&profile)
        .map_err(|_| binding_error("provider profile binding differs"))?;
    let executable_bytes = read_regular_bounded(&entry.executable_path, MAX_EXECUTABLE_BYTES)?;
    let expected_executable_path = arguments
        .target_root
        .join(format!("runtime/third-party/rust-analyzer/{PACK_VERSION}"))
        .join(&record.executable.path);
    if entry.executable_path != expected_executable_path
        || u64::try_from(executable_bytes.len()).ok() != Some(record.executable.size)
        || sha256_bytes(&executable_bytes) != entry.executable_sha256
        || entry.executable_sha256 != profile.executable_sha256
        || entry.executable_sha256 != record.executable.sha256
        || !source_lock.assets.iter().any(|asset| {
            asset.platform_id == receipt.platform_id
                && asset.executable_size == record.executable.size
                && asset.executable_sha256 == entry.executable_sha256
        })
    {
        return Err(binding_error("provider executable binding differs"));
    }

    check_preparation_deadline(deadline)?;
    let git = resolve_git()?;
    let repository = git_repository(&git, deadline)?;
    fixture_inventory.copy_to(repository.path())?;
    run_git(&git, repository.path(), &["add", "--", "."], deadline)?;
    let scope = open_authoritative_scope_bounded(
        ScopeRequest {
            repository: repository.path().to_path_buf(),
            source: Some(ReviewSource::Staged),
            expected_fingerprint: None,
        },
        preparation_timeout(deadline, Duration::from_secs(5))?,
    )
    .map_err(|_| runner_error("fixture scope cannot be opened"))?;
    check_preparation_deadline(deadline)?;
    let snapshot = CandidateSnapshot::materialize_staged_bounded(
        repository.path(),
        &git,
        SnapshotLimits {
            max_files: 64,
            max_bytes: 256 * 1024,
        },
        preparation_timeout(deadline, Duration::from_secs(5))?,
    )
    .map_err(|_| runner_error("fixture snapshot cannot be materialized"))?;
    check_preparation_deadline(deadline)?;
    let model = build_linked_project_model(
        &snapshot,
        ProviderModelLimits {
            max_files: 64,
            max_bytes: 256 * 1024,
            max_file_bytes: 64 * 1024,
        },
    )
    .map_err(|_| runner_error("fixture project model cannot be built"))?;
    check_preparation_deadline(deadline)?;
    if model.target_triple != profile.target_triple {
        return Err(binding_error("fixture model target differs from provider"));
    }
    let source = fs::read(snapshot.path().join("src/lib.rs"))
        .map_err(|_| runner_error("fixture seed source cannot be read"))?;
    let run_request = ProviderRunRequest {
        schema_version: 1,
        kind: "repository_context_provider_run_request".to_string(),
        seeds: vec![seed_symbol("src/lib.rs", &source)?],
        directions: vec![CallDirection::Incoming, CallDirection::Outgoing],
        limits: ProviderLimits {
            deadline_ms: 10_000,
            ..ProviderLimits::maximum()
        },
    };
    run_request
        .validate_against(&profile.maximum_limits)
        .map_err(|_| runner_error("fixture run request is invalid"))?;
    let request = crate::repository_context_provider::cli::build_provider_request(
        &scope,
        &registry,
        &entry,
        &model,
        &run_request,
        &snapshot,
        &profile,
    )
    .map_err(|_| binding_error("provider request binding differs"))?;
    let identity = Identity {
        platform_id: receipt.platform_id,
        pack_version: receipt.pack_version,
        pack_sha256: receipt.pack_sha256,
        executable_sha256: entry.executable_sha256,
        source_lock_sha256,
        profile_sha256: sha256_bytes(&profile_bytes),
        fixture_id: "single-crate".to_string(),
        fixture_sha256: fixture_inventory.sha256()?,
        request_sha256: sha256_bytes(
            &serde_json::to_vec(&run_request)
                .map_err(|_| runner_error("fixture request cannot be serialized"))?,
        ),
        runner_class: arguments.runner_class.clone(),
        toolchain: TOOLCHAIN.to_string(),
        timing_scope: TIMING_SCOPE.to_string(),
        provisioning_included: false,
    };
    Ok(PreparedRun {
        _repository: repository,
        snapshot,
        model,
        request,
        profile,
        identity,
    })
}

fn validate_report(report: &RepositoryContextProviderReport) -> Result<()> {
    report
        .validate()
        .map_err(|_| runner_error("provider report contract differs"))?;
    if report.status != RepositoryContextProviderStatus::Completed
        || report.metrics.elapsed_ms == 0
        || report.metrics.process_tree_peak_rss_bytes == 0
        || report.metrics.stderr_bytes != 0
    {
        return Err(runner_error("provider report is not baseline-eligible"));
    }
    let symbols = report
        .seed_symbols
        .iter()
        .map(|item| &item.symbol)
        .chain(report.related_symbols.iter())
        .map(|item| (item.symbol_id.as_str(), item.name.as_str()))
        .collect::<BTreeMap<_, _>>();
    for (from, to) in [("caller", "seed"), ("seed", "callee")] {
        if !report.edges.iter().any(|edge| {
            symbols.get(edge.from_symbol.as_str()) == Some(&from)
                && symbols.get(edge.to_symbol.as_str()) == Some(&to)
        }) {
            return Err(runner_error("provider report lacks a required call edge"));
        }
    }
    Ok(())
}

fn git_repository(git: &Path, deadline: Option<&SampleDeadline>) -> Result<TempDir> {
    let repository =
        tempfile::tempdir().map_err(|_| runner_error("fixture repository cannot be created"))?;
    run_git(git, repository.path(), &["init", "-q"], deadline)?;
    run_git(
        git,
        repository.path(),
        &["config", "user.email", "provider-baseline@example.invalid"],
        deadline,
    )?;
    run_git(
        git,
        repository.path(),
        &["config", "user.name", "Provider Baseline Fixture"],
        deadline,
    )?;
    fs::write(repository.path().join("README.md"), b"baseline\n")
        .map_err(|_| runner_error("fixture baseline cannot be written"))?;
    run_git(
        git,
        repository.path(),
        &["add", "--", "README.md"],
        deadline,
    )?;
    run_git(
        git,
        repository.path(),
        &["commit", "-q", "-m", "baseline"],
        deadline,
    )?;
    Ok(repository)
}

fn run_git(
    git: &Path,
    repository: &Path,
    arguments: &[&str],
    deadline: Option<&SampleDeadline>,
) -> Result<()> {
    check_preparation_deadline(deadline)?;
    let output = output_bounded(
        Command::new(git).args(arguments).current_dir(repository),
        preparation_timeout(deadline, PREPARATION_GIT_TIMEOUT)?,
    )
    .map_err(|_| runner_error("fixture git command cannot complete safely"))?;
    check_preparation_deadline(deadline)?;
    if !output.status.success() {
        return Err(runner_error("fixture git command failed"));
    }
    Ok(())
}

fn sample_deadline(arguments: &Arguments) -> Result<Duration> {
    let Some(value) = env::var_os("PCR_PROVIDER_BASELINE_TEST_DEADLINE_MS") else {
        return Ok(SAMPLE_TOTAL_DEADLINE);
    };
    if !arguments.runner_class.starts_with("local-") {
        return Err(argument_error(
            "test deadline override is permitted only for local runners",
        ));
    }
    let milliseconds = value
        .to_string_lossy()
        .parse::<u64>()
        .ok()
        .filter(|value| *value <= SAMPLE_TOTAL_DEADLINE.as_millis() as u64)
        .ok_or_else(|| argument_error("test deadline override is invalid"))?;
    Ok(Duration::from_millis(milliseconds))
}

fn check_preparation_deadline(deadline: Option<&SampleDeadline>) -> Result<()> {
    deadline.map_or(Ok(()), SampleDeadline::check)
}

fn preparation_timeout(deadline: Option<&SampleDeadline>, maximum: Duration) -> Result<Duration> {
    deadline
        .map(SampleDeadline::remaining)
        .transpose()
        .map(|remaining| remaining.unwrap_or(maximum).min(maximum))
}

fn deadline_error() -> RunnerError {
    RunnerError::new(
        "runner-deadline",
        "provider baseline sample exceeded its total deadline",
    )
}

fn resolve_git() -> Result<PathBuf> {
    let path = env::var_os("PATH").ok_or_else(|| runner_error("PATH is unavailable"))?;
    let executable = if cfg!(windows) { "git.exe" } else { "git" };
    for directory in env::split_paths(&path) {
        let candidate = directory.join(executable);
        if candidate.is_file() {
            return fs::canonicalize(candidate)
                .map_err(|_| runner_error("git executable cannot be resolved"));
        }
    }
    Err(runner_error("git executable cannot be found"))
}

fn seed_symbol(path: &str, source: &[u8]) -> Result<SeedSymbol> {
    let declaration = b"pub fn seed";
    let declaration_start = source
        .windows(declaration.len())
        .position(|window| window == declaration)
        .ok_or_else(|| runner_error("fixture does not declare the seed function"))?;
    let selection_start = declaration_start + b"pub fn ".len();
    let selection_end = selection_start + b"seed".len();
    let (start_line, start_column) = byte_position(source, selection_start)?;
    let (end_line, end_column) = byte_position(source, selection_end)?;
    let range = ProviderRange {
        format: ProviderRangeFormat::Utf8ByteColumnsEndExclusiveV1,
        start_line,
        start_column,
        end_line,
        end_column,
        start_byte: selection_start,
        end_byte: selection_end,
    };
    Ok(SeedSymbol {
        changed_symbol_id: sha256_bytes(format!("{path}\0seed").as_bytes()),
        path: path.to_string(),
        kind: SeedKind::Function,
        name: "seed".to_string(),
        symbol_range: range.clone(),
        selection_range: range,
        query_byte: selection_start + 1,
    })
}

fn byte_position(source: &[u8], offset: usize) -> Result<(u32, u32)> {
    let prefix = source
        .get(..offset)
        .ok_or_else(|| runner_error("fixture seed offset is invalid"))?;
    let line_start = prefix
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let line = prefix.iter().filter(|byte| **byte == b'\n').count() + 1;
    Ok((
        u32::try_from(line).map_err(|_| runner_error("fixture line exceeds u32"))?,
        u32::try_from(offset - line_start + 1)
            .map_err(|_| runner_error("fixture column exceeds u32"))?,
    ))
}

fn current_platform() -> Result<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok("darwin-arm64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Ok("darwin-amd64")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok("linux-amd64")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Ok("windows-amd64")
    } else {
        Err(binding_error("current provider platform is unsupported"))
    }
}

fn validate_runner_class(platform: &str, runner_class: &str) -> Result<()> {
    if runner_class == format!("local-{platform}") {
        return Ok(());
    }
    let (expected_class, expected_os, expected_arch, expected_image) = match platform {
        "darwin-amd64" => ("github-hosted-macos-15-intel", "macOS", "X64", "macos15"),
        "darwin-arm64" => ("github-hosted-macos-14-arm64", "macOS", "ARM64", "macos14"),
        "linux-amd64" => ("github-hosted-ubuntu-24-x64", "Linux", "X64", "ubuntu24"),
        "windows-amd64" => ("github-hosted-windows-2025-x64", "Windows", "X64", "win25"),
        _ => return Err(binding_error("current provider platform is unsupported")),
    };
    let metadata_matches = runner_class == expected_class
        && env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
        && env::var("GITHUB_REPOSITORY").as_deref() == Ok("junit/pre-commit-review")
        && env::var("RUNNER_OS").as_deref() == Ok(expected_os)
        && env::var("RUNNER_ARCH").as_deref() == Ok(expected_arch)
        && env::var("ImageOS").as_deref() == Ok(expected_image);
    if !metadata_matches {
        return Err(binding_error("hosted runner metadata differs"));
    }
    Ok(())
}

fn read_regular(path: &Path) -> Result<Vec<u8>> {
    read_regular_bounded(path, MAX_JSON_BYTES)
}

fn read_regular_bounded(path: &Path, maximum_bytes: u64) -> Result<Vec<u8>> {
    read_open_regular_bounded(path, maximum_bytes).map_err(|error| match error.kind() {
        std::io::ErrorKind::FileTooLarge | std::io::ErrorKind::UnexpectedEof => {
            binding_error("provider binding file exceeds its byte limit")
        }
        std::io::ErrorKind::InvalidData => {
            binding_error("provider binding file changed while it was read")
        }
        _ => binding_error("provider binding file cannot be read safely"),
    })
}

pub(super) fn read_open_regular_bounded(
    path: &Path,
    maximum_bytes: u64,
) -> std::io::Result<Vec<u8>> {
    let mut file = open_regular_file_no_follow(path)?;
    let before = file_fingerprint(&file)?;
    if before.size == 0 || before.size > maximum_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            "regular file is outside its byte limit",
        ));
    }
    let read_limit = maximum_bytes.saturating_add(1);
    let initial_capacity = usize::try_from(before.size.min(64 * 1024)).unwrap_or(64 * 1024);
    let mut bytes = Vec::with_capacity(initial_capacity);
    (&mut file).take(read_limit).read_to_end(&mut bytes)?;
    let after = file_fingerprint(&file)?;
    if before != after || u64::try_from(bytes.len()).ok() != Some(before.size) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "regular file changed while it was read",
        ));
    }
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            "regular file is outside its byte limit",
        ));
    }
    Ok(bytes)
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
struct FileFingerprint {
    device: u64,
    inode: u64,
    size: u64,
    mode: u32,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[cfg(unix)]
fn file_fingerprint(file: &fs::File) -> std::io::Result<FileFingerprint> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "opened path is not a regular file",
        ));
    }
    Ok(FileFingerprint {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.len(),
        mode: metadata.mode(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    })
}

#[cfg(windows)]
#[derive(Debug, PartialEq, Eq)]
struct FileFingerprint {
    volume: u32,
    index: u64,
    size: u64,
    attributes: u32,
    modified: i64,
    created: i64,
    changed: i64,
}

#[cfg(windows)]
fn file_fingerprint(file: &fs::File) -> std::io::Result<FileFingerprint> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileBasicInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_BASIC_INFO,
    };

    let handle = file.as_raw_handle() as _;
    let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    // SAFETY: `file` owns a valid handle for this call and `information` points to writable,
    // correctly sized storage that is initialized only after the API reports success.
    let succeeded = unsafe { GetFileInformationByHandle(handle, information.as_mut_ptr()) };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the successful API call initialized the complete output structure.
    let information = unsafe { information.assume_init() };
    let mut basic_information = std::mem::MaybeUninit::<FILE_BASIC_INFO>::zeroed();
    let basic_information_size = u32::try_from(std::mem::size_of::<FILE_BASIC_INFO>())
        .expect("FILE_BASIC_INFO size fits in a Windows DWORD");
    // SAFETY: `handle` remains owned by `file`; the class and buffer size match
    // `FILE_BASIC_INFO`, and the buffer is only assumed initialized after success.
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            basic_information.as_mut_ptr().cast(),
            basic_information_size,
        )
    };
    if succeeded == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the successful API call initialized the complete output structure.
    let basic_information = unsafe { basic_information.assume_init() };
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || basic_information.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "opened path is a reparse point",
        ));
    }
    let combine = |high: u32, low: u32| (u64::from(high) << 32) | u64::from(low);
    Ok(FileFingerprint {
        volume: information.dwVolumeSerialNumber,
        index: combine(information.nFileIndexHigh, information.nFileIndexLow),
        size: combine(information.nFileSizeHigh, information.nFileSizeLow),
        attributes: basic_information.FileAttributes,
        modified: basic_information.LastWriteTime,
        created: basic_information.CreationTime,
        changed: basic_information.ChangeTime,
    })
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| runner_error("runner output cannot be created"))?;
    output
        .write_all(bytes)
        .map_err(|_| runner_error("runner output cannot be written"))
}

fn argument_error(message: &'static str) -> RunnerError {
    RunnerError::new("runner-arguments", message)
}

fn binding_error(message: &'static str) -> RunnerError {
    RunnerError::new("runner-binding", message)
}

fn runner_error(message: &'static str) -> RunnerError {
    RunnerError::new("runner-execution", message)
}
