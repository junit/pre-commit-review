use collect_diff_context_cli::static_analysis::contracts::StaticAnalysisProfile;
#[cfg(unix)]
use collect_diff_context_cli::static_analysis::contracts::{ExecutionStatus, FailureReason};
#[cfg(unix)]
use collect_diff_context_cli::static_analysis::executor::{
    execute_prepared, prepare_profile, run_analysis, ExecutionLimits, RunRequest,
};
#[cfg(unix)]
use collect_diff_context_cli::static_analysis::snapshot::{CandidateSnapshot, SnapshotLimits};
use serde_json::json;
#[cfg(unix)]
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use tempfile::TempDir;

fn valid_profile() -> serde_json::Value {
    json!({
        "schema_version": 1,
        "kind": "static_analysis_profile",
        "name": "fixture profile",
        "tool": {"name": "fixture", "version": "1.0"},
        "executable": {
            "path": "/opt/review/bin/fixture",
            "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        },
        "arguments": ["--format", "json"],
        "output_format": "normalized-json",
        "success_exit_codes": [0],
        "limits": {
            "timeout_seconds": 30,
            "max_output_bytes": 1048576,
            "max_snapshot_bytes": 10485760,
            "max_snapshot_files": 1000
        },
        "repository_configuration": "disabled",
        "network_access": "offline-required"
    })
}

#[test]
fn contracts_accept_valid_profile() {
    let profile: StaticAnalysisProfile = serde_json::from_value(valid_profile()).unwrap();
    profile.validate().unwrap();
}

#[test]
fn contracts_reject_unknown_profile_fields() {
    let mut profile = valid_profile();
    profile["limits"]["unexpected"] = json!(1);
    assert!(serde_json::from_value::<StaticAnalysisProfile>(profile).is_err());
}

#[test]
fn contracts_reject_invalid_profile_hash_and_bounds() {
    let mut profile = valid_profile();
    profile["executable"]["sha256"] = json!("ABCDEF");
    let profile: StaticAnalysisProfile = serde_json::from_value(profile).unwrap();
    assert!(profile.validate().is_err());

    let mut profile = valid_profile();
    profile["limits"]["timeout_seconds"] = json!(0);
    let profile: StaticAnalysisProfile = serde_json::from_value(profile).unwrap();
    assert!(profile.validate().is_err());
}

#[test]
fn contracts_reject_duplicate_exit_codes_and_nul_arguments() {
    let mut profile = valid_profile();
    profile["success_exit_codes"] = json!([0, 0]);
    let profile: StaticAnalysisProfile = serde_json::from_value(profile).unwrap();
    assert!(profile.validate().is_err());

    let mut profile = valid_profile();
    profile["arguments"] = json!(["bad\u{0}argument"]);
    let profile: StaticAnalysisProfile = serde_json::from_value(profile).unwrap();
    assert!(profile.validate().is_err());
}

#[cfg(unix)]
fn git(repository: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn execution_repository() -> TempDir {
    let repository = TempDir::new().unwrap();
    git(repository.path(), &["init", "-q"]);
    git(
        repository.path(),
        &["config", "user.email", "review@example.test"],
    );
    git(repository.path(), &["config", "user.name", "Review Test"]);
    fs::write(repository.path().join("candidate.txt"), "base\n").unwrap();
    git(repository.path(), &["add", "candidate.txt"]);
    git(repository.path(), &["commit", "-qm", "base"]);
    fs::write(repository.path().join("candidate.txt"), "candidate\n").unwrap();
    git(repository.path(), &["add", "candidate.txt"]);
    repository
}

#[cfg(unix)]
fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path).unwrap();
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(unix)]
fn write_executable(directory: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = directory.join(name);
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn write_profile(
    directory: &Path,
    executable: &Path,
    executable_hash: &str,
    arguments: serde_json::Value,
    repository_configuration: &str,
    success_exit_codes: serde_json::Value,
) -> (PathBuf, String) {
    let path = directory.join("profile.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "static_analysis_profile",
            "name": "fixture profile",
            "tool": {"name": "fixture", "version": "1.0"},
            "executable": {
                "path": executable.to_string_lossy(),
                "sha256": executable_hash
            },
            "arguments": arguments,
            "output_format": "normalized-json",
            "success_exit_codes": success_exit_codes,
            "limits": {
                "timeout_seconds": 30,
                "max_output_bytes": 1048576,
                "max_snapshot_bytes": 10485760,
                "max_snapshot_files": 1000
            },
            "repository_configuration": repository_configuration,
            "network_access": "offline-required"
        }))
        .unwrap(),
    )
    .unwrap();
    let hash = sha256_file(&path);
    (path, hash)
}

#[cfg(unix)]
#[test]
fn executor_preflight_accepts_hash_pinned_external_profile() {
    let repository = execution_repository();
    let tools = TempDir::new().unwrap();
    let executable = write_executable(tools.path(), "analyzer.sh", "#!/bin/sh\nexit 0\n");
    let executable_hash = sha256_file(&executable);
    let (profile, profile_hash) = write_profile(
        tools.path(),
        &executable,
        &executable_hash,
        json!([]),
        "disabled",
        json!([0]),
    );

    let prepared = prepare_profile(repository.path(), &profile, &profile_hash, false).unwrap();

    assert_eq!(prepared.profile_id, &profile_hash[..16]);
    assert_eq!(prepared.profile_sha256, profile_hash);
    assert_eq!(prepared.executable_sha256, executable_hash);
    assert_eq!(
        prepared.executable_path,
        fs::canonicalize(executable).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn executor_preflight_rejects_profile_and_executable_tampering() {
    let repository = execution_repository();
    let tools = TempDir::new().unwrap();
    let executable = write_executable(tools.path(), "analyzer.sh", "#!/bin/sh\nexit 0\n");
    let executable_hash = sha256_file(&executable);
    let (profile, profile_hash) = write_profile(
        tools.path(),
        &executable,
        &executable_hash,
        json!([]),
        "disabled",
        json!([0]),
    );
    fs::OpenOptions::new()
        .append(true)
        .open(&profile)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let profile_error =
        prepare_profile(repository.path(), &profile, &profile_hash, false).unwrap_err();
    assert!(profile_error
        .to_string()
        .contains("profile SHA256 does not match"));

    let (profile, profile_hash) = write_profile(
        tools.path(),
        &executable,
        &executable_hash,
        json!([]),
        "disabled",
        json!([0]),
    );
    fs::write(&executable, "#!/bin/sh\nexit 9\n").unwrap();
    let executable_error =
        prepare_profile(repository.path(), &profile, &profile_hash, false).unwrap_err();
    assert!(executable_error
        .to_string()
        .contains("executable SHA256 does not match"));
}

#[cfg(unix)]
#[test]
fn executor_preflight_rejects_unsafe_paths_and_configuration_authority() {
    let repository = execution_repository();
    let inside = write_executable(
        repository.path(),
        "inside-analyzer.sh",
        "#!/bin/sh\nexit 0\n",
    );
    let inside_hash = sha256_file(&inside);
    let profiles = TempDir::new().unwrap();
    let (inside_profile, inside_profile_hash) = write_profile(
        profiles.path(),
        &inside,
        &inside_hash,
        json!([]),
        "disabled",
        json!([0]),
    );
    let inside_error = prepare_profile(
        repository.path(),
        &inside_profile,
        &inside_profile_hash,
        false,
    )
    .unwrap_err();
    assert!(inside_error
        .to_string()
        .contains("outside the reviewed repository"));

    let outside = write_executable(profiles.path(), "outside.sh", "#!/bin/sh\nexit 0\n");
    let outside_hash = sha256_file(&outside);
    let (relative_profile, relative_hash) = write_profile(
        profiles.path(),
        Path::new("relative-analyzer.sh"),
        &outside_hash,
        json!([]),
        "disabled",
        json!([0]),
    );
    let relative_error =
        prepare_profile(repository.path(), &relative_profile, &relative_hash, false).unwrap_err();
    assert!(relative_error
        .to_string()
        .contains("executable.path must be absolute"));

    let (argument_profile, argument_hash) = write_profile(
        profiles.path(),
        &outside,
        &outside_hash,
        json!([repository.path().join("candidate.txt")]),
        "disabled",
        json!([0]),
    );
    let argument_error =
        prepare_profile(repository.path(), &argument_profile, &argument_hash, false).unwrap_err();
    assert!(argument_error
        .to_string()
        .contains("must not reference paths inside the reviewed repository"));

    let (trusted_profile, trusted_hash) = write_profile(
        profiles.path(),
        &outside,
        &outside_hash,
        json!([]),
        "explicitly-trusted",
        json!([0]),
    );
    let trust_error =
        prepare_profile(repository.path(), &trusted_profile, &trusted_hash, false).unwrap_err();
    assert!(trust_error
        .to_string()
        .contains("requires separate --allow-repository-configuration"));
    prepare_profile(repository.path(), &trusted_profile, &trusted_hash, true).unwrap();

    let (disabled_profile, disabled_hash) = write_profile(
        profiles.path(),
        &outside,
        &outside_hash,
        json!([]),
        "disabled",
        json!([0]),
    );
    let excess_trust =
        prepare_profile(repository.path(), &disabled_profile, &disabled_hash, true).unwrap_err();
    assert!(excess_trust
        .to_string()
        .contains("valid only for an explicitly-trusted profile"));
}

#[cfg(unix)]
fn prepared_fixture(
    script: &str,
    arguments: serde_json::Value,
    success_exit_codes: serde_json::Value,
) -> (
    TempDir,
    TempDir,
    CandidateSnapshot,
    collect_diff_context_cli::static_analysis::executor::PreparedProfile,
) {
    let repository = execution_repository();
    let tools = TempDir::new().unwrap();
    let executable = write_executable(tools.path(), "analyzer.sh", script);
    let executable_hash = sha256_file(&executable);
    let (profile, profile_hash) = write_profile(
        tools.path(),
        &executable,
        &executable_hash,
        arguments,
        "disabled",
        success_exit_codes,
    );
    let prepared = prepare_profile(repository.path(), &profile, &profile_hash, false).unwrap();
    let snapshot = CandidateSnapshot::materialize(
        repository.path(),
        collect_diff_context_cli::review_scope::ReviewSource::Staged,
        SnapshotLimits {
            max_files: 1000,
            max_bytes: 10_485_760,
        },
    )
    .unwrap();
    (repository, tools, snapshot, prepared)
}

#[cfg(unix)]
#[test]
fn executor_runs_without_shell_and_with_allowlisted_environment() {
    let marker_root = TempDir::new().unwrap();
    let marker = marker_root.path().join("must-not-exist");
    let literal = format!("$(touch {})", marker.display());
    let script = r#"#!/bin/sh
set -eu
test "${PRE_COMMIT_REVIEW_TEST_SECRET-unset}" = unset
test "$PRE_COMMIT_REVIEW_SOURCE" = staged
test "$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT" = 0123456789abcdef0123456789abcdef01234567
test ! -e .git
test "$1" = '$EXPECTED_LITERAL'
printf '%s' '{"schema_version":1,"kind":"static_analysis_input","scope_fingerprint":"0123456789abcdef0123456789abcdef01234567","tool":{"name":"fixture","version":"1.0"},"status":"completed","findings":[]}'
"#;
    let script = script.replace("$EXPECTED_LITERAL", &literal);
    let (_repository, _tools, snapshot, prepared) =
        prepared_fixture(&script, json!([literal]), json!([0]));
    struct EnvironmentGuard(&'static str);
    impl Drop for EnvironmentGuard {
        fn drop(&mut self) {
            std::env::remove_var(self.0);
        }
    }
    std::env::set_var("PRE_COMMIT_REVIEW_TEST_SECRET", "must-not-leak");
    let _guard = EnvironmentGuard("PRE_COMMIT_REVIEW_TEST_SECRET");
    let outcome = execute_prepared(
        &prepared,
        &snapshot,
        collect_diff_context_cli::review_scope::ReviewSource::Staged,
        "0123456789abcdef0123456789abcdef01234567",
        ExecutionLimits {
            timeout: Duration::from_secs(2),
            max_output_bytes: 4096,
        },
    )
    .unwrap();
    assert_eq!(outcome.status, ExecutionStatus::Completed);
    assert_eq!(outcome.exit_code, Some(0));
    assert_eq!(outcome.failure_reason, None);
    assert!(String::from_utf8(outcome.read_stdout().unwrap())
        .unwrap()
        .contains("static_analysis_input"));
    assert!(!marker.exists());
    snapshot.verify_unchanged().unwrap();
}

#[cfg(unix)]
#[test]
fn executor_classifies_non_success_exit() {
    let (_repository, _tools, snapshot, prepared) = prepared_fixture(
        "#!/bin/sh\nprintf failure >&2\nexit 7\n",
        json!([]),
        json!([0]),
    );
    let outcome = execute_prepared(
        &prepared,
        &snapshot,
        collect_diff_context_cli::review_scope::ReviewSource::Staged,
        "0123456789abcdef0123456789abcdef01234567",
        ExecutionLimits {
            timeout: Duration::from_secs(2),
            max_output_bytes: 4096,
        },
    )
    .unwrap();

    assert_eq!(outcome.status, ExecutionStatus::Failed);
    assert_eq!(outcome.exit_code, Some(7));
    assert_eq!(outcome.failure_reason, Some(FailureReason::NonSuccessExit));
    assert_eq!(outcome.stderr_bytes, 7);
}

#[cfg(unix)]
#[test]
fn executor_enforces_output_limit_with_bounded_prefix() {
    let (_repository, _tools, snapshot, prepared) =
        prepared_fixture("#!/bin/sh\nhead -c 4096 /dev/zero\n", json!([]), json!([0]));
    let outcome = execute_prepared(
        &prepared,
        &snapshot,
        collect_diff_context_cli::review_scope::ReviewSource::Staged,
        "0123456789abcdef0123456789abcdef01234567",
        ExecutionLimits {
            timeout: Duration::from_secs(2),
            max_output_bytes: 1024,
        },
    )
    .unwrap();

    assert_eq!(outcome.status, ExecutionStatus::OutputLimit);
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.failure_reason, Some(FailureReason::OutputLimit));
    assert_eq!(outcome.stdout_bytes, 1025);
    assert_eq!(outcome.read_stdout().unwrap().len(), 1025);
}

#[cfg(unix)]
#[test]
fn executor_enforces_stderr_output_limit() {
    let (_repository, _tools, snapshot, prepared) = prepared_fixture(
        "#!/bin/sh\nhead -c 4096 /dev/zero >&2\n",
        json!([]),
        json!([0]),
    );
    let outcome = execute_prepared(
        &prepared,
        &snapshot,
        collect_diff_context_cli::review_scope::ReviewSource::Staged,
        "0123456789abcdef0123456789abcdef01234567",
        ExecutionLimits {
            timeout: Duration::from_secs(2),
            max_output_bytes: 1024,
        },
    )
    .unwrap();

    assert_eq!(outcome.status, ExecutionStatus::OutputLimit);
    assert_eq!(outcome.failure_reason, Some(FailureReason::OutputLimit));
    assert_eq!(outcome.stderr_bytes, 1025);
}

#[cfg(unix)]
#[test]
fn executor_timeout_terminates_descendants() {
    let marker_root = TempDir::new().unwrap();
    let marker = marker_root.path().join("descendant-marker");
    let script = "#!/bin/sh\n(sleep 1; touch \"$1\") &\nsleep 10\n";
    let (_repository, _tools, snapshot, prepared) =
        prepared_fixture(script, json!([marker]), json!([0]));
    let outcome = execute_prepared(
        &prepared,
        &snapshot,
        collect_diff_context_cli::review_scope::ReviewSource::Staged,
        "0123456789abcdef0123456789abcdef01234567",
        ExecutionLimits {
            timeout: Duration::from_millis(100),
            max_output_bytes: 4096,
        },
    )
    .unwrap();

    assert_eq!(outcome.status, ExecutionStatus::Timeout);
    assert_eq!(outcome.exit_code, None);
    assert_eq!(outcome.failure_reason, Some(FailureReason::Timeout));
    std::thread::sleep(Duration::from_millis(1200));
    assert!(!marker.exists());
}

#[cfg(unix)]
#[test]
fn executor_rejects_prepared_artifact_replacement_before_spawn() {
    let marker_root = TempDir::new().unwrap();
    let profile_marker = marker_root.path().join("profile-replacement-ran");
    let script = "#!/bin/sh\ntouch \"$1\"\nexit 0\n";
    let (_repository, _tools, snapshot, prepared) =
        prepared_fixture(script, json!([profile_marker]), json!([0]));
    fs::OpenOptions::new()
        .append(true)
        .open(&prepared.profile_path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let profile_error = execute_prepared(
        &prepared,
        &snapshot,
        collect_diff_context_cli::review_scope::ReviewSource::Staged,
        "0123456789abcdef0123456789abcdef01234567",
        ExecutionLimits {
            timeout: Duration::from_secs(2),
            max_output_bytes: 4096,
        },
    )
    .unwrap_err();
    assert!(profile_error
        .to_string()
        .contains("profile changed before execution"));
    assert!(!profile_marker.exists());

    let executable_marker = marker_root.path().join("executable-replacement-ran");
    let (_repository, _tools, snapshot, prepared) =
        prepared_fixture("#!/bin/sh\nexit 0\n", json!([]), json!([0]));
    fs::write(
        &prepared.executable_path,
        format!(
            "#!/bin/sh\ntouch \"{}\"\nexit 0\n",
            executable_marker.display()
        ),
    )
    .unwrap();
    let executable_error = execute_prepared(
        &prepared,
        &snapshot,
        collect_diff_context_cli::review_scope::ReviewSource::Staged,
        "0123456789abcdef0123456789abcdef01234567",
        ExecutionLimits {
            timeout: Duration::from_secs(2),
            max_output_bytes: 4096,
        },
    )
    .unwrap_err();
    assert!(executable_error
        .to_string()
        .contains("executable changed before execution"));
    assert!(!executable_marker.exists());
}

#[cfg(unix)]
fn run_fixture(
    script: &str,
    success_exit_codes: serde_json::Value,
) -> (TempDir, TempDir, PathBuf, String, String) {
    use collect_diff_context_cli::review_scope::{
        open_authoritative_scope, ReviewSource, ScopeRequest,
    };

    let repository = execution_repository();
    let tools = TempDir::new().unwrap();
    let executable = write_executable(tools.path(), "run-analyzer.sh", script);
    let executable_hash = sha256_file(&executable);
    let (profile, profile_hash) = write_profile(
        tools.path(),
        &executable,
        &executable_hash,
        json!([]),
        "disabled",
        success_exit_codes,
    );
    let scope = open_authoritative_scope(ScopeRequest {
        repository: repository.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_fingerprint: None,
    })
    .unwrap();
    (repository, tools, profile, profile_hash, scope.fingerprint)
}

#[cfg(unix)]
fn run_request(
    repository: &Path,
    profile: PathBuf,
    profile_hash: String,
    fingerprint: String,
) -> RunRequest {
    RunRequest {
        repository: repository.to_path_buf(),
        source: collect_diff_context_cli::review_scope::ReviewSource::Staged,
        expected_scope: fingerprint,
        profile_path: profile,
        expected_profile_sha256: profile_hash,
        allow_repository_configuration: false,
        max_findings: 500,
    }
}

#[cfg(unix)]
fn rewrite_profile(profile: &Path, update: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(profile).unwrap()).unwrap();
    update(&mut value);
    fs::write(profile, serde_json::to_vec(&value).unwrap()).unwrap();
    sha256_file(profile)
}

#[cfg(unix)]
#[test]
fn run_artifact_links_completed_execution_and_evidence() {
    use collect_diff_context_cli::static_analysis::contracts::{
        EvidenceScopeBinding, EvidenceTrust,
    };

    let script = r#"#!/bin/sh
printf '{"schema_version":1,"kind":"static_analysis_input","scope_fingerprint":"%s","tool":{"name":"fixture","version":"1.0"},"status":"completed","findings":[]}' "$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT"
"#;
    let (repository, _tools, profile, profile_hash, fingerprint) = run_fixture(script, json!([0]));
    let artifact = run_analysis(run_request(
        repository.path(),
        profile,
        profile_hash,
        fingerprint.clone(),
    ))
    .unwrap();

    assert_eq!(
        artifact.execution.execution.status,
        ExecutionStatus::Completed
    );
    assert!(artifact.execution.execution.result_accepted);
    assert_eq!(artifact.execution.execution_id.len(), 16);
    assert_eq!(artifact.execution.scope.fingerprint, fingerprint);
    assert_eq!(artifact.evidence.scope, artifact.execution.scope);
    assert_eq!(
        artifact.evidence.reports[0].trust,
        EvidenceTrust::ControlledExecution
    );
    assert_eq!(
        artifact.evidence.reports[0].scope_binding,
        EvidenceScopeBinding::ControlledExecution
    );
    assert_eq!(
        artifact.evidence.reports[0].execution_id.as_deref(),
        Some(artifact.execution.execution_id.as_str())
    );
    assert_eq!(
        artifact.execution.evidence.report_ids,
        vec![artifact.evidence.reports[0].report_id.clone()]
    );
}

#[cfg(unix)]
#[test]
fn run_artifact_synthesizes_nonblocking_failure_evidence() {
    let (repository, _tools, profile, profile_hash, fingerprint) =
        run_fixture("#!/bin/sh\nprintf failure >&2\nexit 7\n", json!([0]));
    let artifact = run_analysis(run_request(
        repository.path(),
        profile,
        profile_hash,
        fingerprint,
    ))
    .unwrap();

    assert_eq!(artifact.execution.execution.status, ExecutionStatus::Failed);
    assert_eq!(artifact.execution.execution.exit_code, Some(7));
    assert_eq!(
        artifact.execution.execution.failure_reason,
        Some(FailureReason::NonSuccessExit)
    );
    assert!(!artifact.execution.execution.result_accepted);
    assert!(artifact.evidence.findings.is_empty());
    assert_eq!(artifact.evidence.counts.blocking_candidates, 0);
    assert_eq!(
        artifact.evidence.reports[0].status,
        collect_diff_context_cli::static_analysis::contracts::ReportStatus::Failed
    );
}

#[cfg(unix)]
#[test]
fn run_artifact_rejects_malformed_or_mismatched_success_output() {
    let scripts = [
        "#!/bin/sh\nprintf '{'\n",
        r#"#!/bin/sh
printf '{"schema_version":1,"kind":"static_analysis_input","scope_fingerprint":"%s","tool":{"name":"other-tool","version":"1.0"},"status":"completed","findings":[]}' "$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT"
"#,
    ];
    for script in scripts {
        let (repository, _tools, profile, profile_hash, fingerprint) =
            run_fixture(script, json!([0]));
        let artifact = run_analysis(run_request(
            repository.path(),
            profile,
            profile_hash,
            fingerprint,
        ))
        .unwrap();

        assert_eq!(
            artifact.execution.execution.status,
            ExecutionStatus::InvalidOutput
        );
        assert_eq!(
            artifact.execution.execution.failure_reason,
            Some(FailureReason::InvalidOutput)
        );
        assert!(!artifact.execution.execution.result_accepted);
        assert!(artifact.evidence.findings.is_empty());
        assert_eq!(artifact.evidence.counts.blocking_candidates, 0);
    }
}

#[cfg(unix)]
#[test]
fn run_artifact_synthesizes_bounded_timeout_evidence() {
    let (repository, _tools, profile, _profile_hash, fingerprint) =
        run_fixture("#!/bin/sh\nsleep 2\n", json!([0]));
    let profile_hash = rewrite_profile(&profile, |value| {
        value["limits"]["timeout_seconds"] = json!(1);
    });

    let artifact = run_analysis(run_request(
        repository.path(),
        profile,
        profile_hash,
        fingerprint,
    ))
    .unwrap();

    assert_eq!(
        artifact.execution.execution.status,
        ExecutionStatus::Timeout
    );
    assert_eq!(
        artifact.execution.execution.failure_reason,
        Some(FailureReason::Timeout)
    );
    assert!(!artifact.execution.execution.result_accepted);
    assert_eq!(
        artifact.evidence.reports[0].status,
        collect_diff_context_cli::static_analysis::contracts::ReportStatus::Timeout
    );
    assert!(artifact.evidence.findings.is_empty());
    assert_eq!(artifact.evidence.counts.blocking_candidates, 0);
}

#[cfg(unix)]
#[test]
fn run_artifact_synthesizes_bounded_output_limit_evidence() {
    let script = "#!/bin/sh\ni=0\nwhile [ \"$i\" -lt 4096 ]; do printf x; i=$((i + 1)); done\n";
    let (repository, _tools, profile, _profile_hash, fingerprint) = run_fixture(script, json!([0]));
    let profile_hash = rewrite_profile(&profile, |value| {
        value["limits"]["max_output_bytes"] = json!(1024);
    });

    let artifact = run_analysis(run_request(
        repository.path(),
        profile,
        profile_hash,
        fingerprint,
    ))
    .unwrap();

    assert_eq!(
        artifact.execution.execution.status,
        ExecutionStatus::OutputLimit
    );
    assert_eq!(
        artifact.execution.execution.failure_reason,
        Some(FailureReason::OutputLimit)
    );
    assert!(artifact.execution.execution.stdout_bytes <= 1025);
    assert!(!artifact.execution.execution.result_accepted);
    assert_eq!(
        artifact.evidence.reports[0].status,
        collect_diff_context_cli::static_analysis::contracts::ReportStatus::Failed
    );
    assert!(artifact.evidence.findings.is_empty());
    assert_eq!(artifact.evidence.counts.blocking_candidates, 0);
}

#[cfg(unix)]
#[test]
fn run_artifact_rejects_profile_and_executable_drift() {
    let profile_script = r#"#!/bin/sh
printf '\n' >> "$1"
printf '{"schema_version":1,"kind":"static_analysis_input","scope_fingerprint":"%s","tool":{"name":"fixture","version":"1.0"},"status":"completed","findings":[]}' "$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT"
"#;
    let repository = execution_repository();
    let tools = TempDir::new().unwrap();
    let executable = write_executable(tools.path(), "profile-drift.sh", profile_script);
    let executable_hash = sha256_file(&executable);
    let profile_path = tools.path().join("profile.json");
    let (profile, profile_hash) = write_profile(
        tools.path(),
        &executable,
        &executable_hash,
        json!([profile_path.to_string_lossy()]),
        "disabled",
        json!([0]),
    );
    let scope = collect_diff_context_cli::review_scope::open_authoritative_scope(
        collect_diff_context_cli::review_scope::ScopeRequest {
            repository: repository.path().to_path_buf(),
            source: Some(collect_diff_context_cli::review_scope::ReviewSource::Staged),
            expected_fingerprint: None,
        },
    )
    .unwrap();
    let error = run_analysis(run_request(
        repository.path(),
        profile,
        profile_hash,
        scope.fingerprint,
    ))
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("static-analysis profile changed during execution"),
        "{error}"
    );

    let script = r#"#!/bin/sh
chmod u+w "$0"
printf '\n# changed' >> "$0"
printf '{"schema_version":1,"kind":"static_analysis_input","scope_fingerprint":"%s","tool":{"name":"fixture","version":"1.0"},"status":"completed","findings":[]}' "$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT"
"#;
    let (repository, _tools, profile, profile_hash, fingerprint) = run_fixture(script, json!([0]));
    let error = run_analysis(run_request(
        repository.path(),
        profile,
        profile_hash,
        fingerprint,
    ))
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("trusted analyzer executable changed during execution"),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn run_artifact_rejects_repository_and_scope_drift() {
    let repository = execution_repository();
    let tools = TempDir::new().unwrap();
    let script = format!(
        "#!/bin/sh\nprintf drift >> '{}/candidate.txt'\nprintf '{{\"schema_version\":1,\"kind\":\"static_analysis_input\",\"scope_fingerprint\":\"%s\",\"tool\":{{\"name\":\"fixture\",\"version\":\"1.0\"}},\"status\":\"completed\",\"findings\":[]}}' \"$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT\"\n",
        repository.path().display()
    );
    let executable = write_executable(tools.path(), "repository-drift.sh", &script);
    let executable_hash = sha256_file(&executable);
    let (profile, profile_hash) = write_profile(
        tools.path(),
        &executable,
        &executable_hash,
        json!([]),
        "disabled",
        json!([0]),
    );
    let scope = collect_diff_context_cli::review_scope::open_authoritative_scope(
        collect_diff_context_cli::review_scope::ScopeRequest {
            repository: repository.path().to_path_buf(),
            source: Some(collect_diff_context_cli::review_scope::ReviewSource::Staged),
            expected_fingerprint: None,
        },
    )
    .unwrap();
    let error = run_analysis(run_request(
        repository.path(),
        profile,
        profile_hash,
        scope.fingerprint,
    ))
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("reviewed repository state changed during controlled execution"),
        "{error}"
    );

    let repository = execution_repository();
    git(repository.path(), &["commit", "-qm", "candidate"]);
    git(
        repository.path(),
        &["commit", "--allow-empty", "-qm", "same tree"],
    );
    fs::write(repository.path().join("candidate.txt"), "next candidate\n").unwrap();
    git(repository.path(), &["add", "candidate.txt"]);
    let tools = TempDir::new().unwrap();
    let script = format!(
        "#!/bin/sh\ngit -C '{}' update-ref HEAD HEAD^\nprintf '{{\"schema_version\":1,\"kind\":\"static_analysis_input\",\"scope_fingerprint\":\"%s\",\"tool\":{{\"name\":\"fixture\",\"version\":\"1.0\"}},\"status\":\"completed\",\"findings\":[]}}' \"$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT\"\n",
        repository.path().display()
    );
    let executable = write_executable(tools.path(), "scope-drift.sh", &script);
    let executable_hash = sha256_file(&executable);
    let (profile, profile_hash) = write_profile(
        tools.path(),
        &executable,
        &executable_hash,
        json!([]),
        "disabled",
        json!([0]),
    );
    let scope = collect_diff_context_cli::review_scope::open_authoritative_scope(
        collect_diff_context_cli::review_scope::ScopeRequest {
            repository: repository.path().to_path_buf(),
            source: Some(collect_diff_context_cli::review_scope::ReviewSource::Staged),
            expected_fingerprint: None,
        },
    )
    .unwrap();
    let error = run_analysis(run_request(
        repository.path(),
        profile,
        profile_hash,
        scope.fingerprint,
    ))
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("expected scope fingerprint does not match opening scope"),
        "{error}"
    );
}

#[test]
fn run_artifact_cli_help_and_usage_errors_are_stable() {
    let binary = env!("CARGO_BIN_EXE_static-analysis-cli");
    let help = Command::new(binary)
        .args(["run", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--expect-profile-sha256"));

    let usage = Command::new(binary).arg("run").output().unwrap();
    assert_eq!(usage.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&usage.stderr).starts_with("run_static_analysis:"));
}
