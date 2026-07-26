#![cfg(feature = "test-fixture")]

use collect_diff_context_cli::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
use collect_diff_context_cli::review_scope::{
    open_authoritative_scope, ReviewSource, ScopeRequest,
};
use collect_diff_context_cli::static_analysis::contracts::ExecutionStatus;
use collect_diff_context_cli::static_analysis::executor::{
    execute_prepared, prepare_profile, run_analysis, ExecutionLimits, RunRequest,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

fn fixture_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_static-analysis-fixture"))
}

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

fn repository() -> TempDir {
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

fn sha256_file(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn write_profile(
    directory: &Path,
    arguments: &[String],
    timeout_seconds: u64,
) -> (PathBuf, String) {
    let executable = fixture_binary();
    let path = directory.join("profile.json");
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "kind": "static_analysis_profile",
            "name": "cross-platform execution fixture",
            "tool": {"name": "platform-fixture", "version": "1.0"},
            "executable": {
                "path": executable.to_string_lossy(),
                "sha256": sha256_file(&executable)
            },
            "arguments": arguments,
            "output_format": "normalized-json",
            "success_exit_codes": [0],
            "limits": {
                "timeout_seconds": timeout_seconds,
                "max_output_bytes": 1048576,
                "max_snapshot_bytes": 10485760,
                "max_snapshot_files": 1000
            },
            "repository_configuration": "disabled",
            "network_access": "offline-required"
        }))
        .unwrap(),
    )
    .unwrap();
    let hash = sha256_file(&path);
    (path, hash)
}

#[test]
fn pinned_fixture_executes_with_controlled_evidence_on_this_platform() {
    let repository = repository();
    let fixtures = TempDir::new().unwrap();
    let (profile, profile_hash) = write_profile(fixtures.path(), &["normalized".to_string()], 10);
    let scope = open_authoritative_scope(ScopeRequest {
        repository: repository.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_fingerprint: None,
    })
    .unwrap();

    let artifact = run_analysis(RunRequest {
        repository: repository.path().to_path_buf(),
        source: ReviewSource::Staged,
        expected_scope: scope.fingerprint,
        profile_path: profile,
        expected_profile_sha256: profile_hash,
        allow_repository_configuration: false,
        max_findings: 100,
    })
    .unwrap();

    assert_eq!(
        artifact.execution.execution.status,
        ExecutionStatus::Completed
    );
    assert!(artifact.execution.execution.result_accepted);
    assert_eq!(artifact.evidence.reports.len(), 1);
}

#[test]
fn timeout_terminates_fixture_descendants_on_this_platform() {
    let repository = repository();
    let fixtures = TempDir::new().unwrap();
    let marker = fixtures.path().join("descendant.marker");
    let arguments = vec![
        "spawn-descendant".to_string(),
        marker.to_string_lossy().into_owned(),
        "1500".to_string(),
    ];
    let (profile, profile_hash) = write_profile(fixtures.path(), &arguments, 10);
    let prepared = prepare_profile(repository.path(), &profile, &profile_hash, false).unwrap();
    let snapshot = CandidateSnapshot::materialize(
        repository.path(),
        ReviewSource::Staged,
        SnapshotLimits {
            max_files: 1000,
            max_bytes: 10_485_760,
        },
    )
    .unwrap();

    let outcome = execute_prepared(
        &prepared,
        &snapshot,
        ReviewSource::Staged,
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ExecutionLimits {
            timeout: Duration::from_millis(100),
            max_stream_output_bytes: 4096,
            max_combined_output_bytes: 8192,
        },
    )
    .unwrap();

    assert_eq!(outcome.status, ExecutionStatus::Timeout);
    std::thread::sleep(Duration::from_millis(1800));
    assert!(!marker.exists());
}

#[test]
fn candidate_snapshot_rejects_mutation_on_this_platform() {
    let repository = repository();
    let snapshot = CandidateSnapshot::materialize(
        repository.path(),
        ReviewSource::Staged,
        SnapshotLimits {
            max_files: 1000,
            max_bytes: 10_485_760,
        },
    )
    .unwrap();
    let candidate = snapshot.path().join("candidate.txt");

    assert!(fs::write(&candidate, b"mutated\n").is_err());
    assert!(fs::remove_file(&candidate).is_err());
    assert!(OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(snapshot.path().join("created.txt"))
        .is_err());
}
