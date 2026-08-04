use collect_diff_context_cli::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
use collect_diff_context_cli::review_scope::ReviewSource;
#[cfg(unix)]
use collect_diff_context_cli::review_scope::{open_authoritative_scope, ScopeRequest};
#[cfg(unix)]
use collect_diff_context_cli::static_analysis::contracts::ExecutionStatus;
#[cfg(unix)]
use collect_diff_context_cli::static_analysis::executor::{run_analysis, RunRequest};
#[cfg(unix)]
use serde_json::json;
#[cfg(unix)]
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

fn git(repository: &Path, arguments: &[&str]) -> String {
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
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn candidate_repository() -> TempDir {
    let repository = TempDir::new().unwrap();
    git(repository.path(), &["init", "-q"]);
    git(
        repository.path(),
        &["config", "user.email", "review@example.test"],
    );
    git(repository.path(), &["config", "user.name", "Review Test"]);
    fs::write(repository.path().join("tracked.txt"), "head\n").unwrap();
    git(repository.path(), &["add", "tracked.txt"]);
    git(repository.path(), &["commit", "-qm", "base"]);
    fs::write(repository.path().join("tracked.txt"), "staged\n").unwrap();
    git(repository.path(), &["add", "tracked.txt"]);
    fs::write(repository.path().join("tracked.txt"), "working\n").unwrap();
    fs::write(repository.path().join("untracked.txt"), "untracked\n").unwrap();
    repository
}

fn generous_limits() -> SnapshotLimits {
    SnapshotLimits {
        max_files: 100,
        max_bytes: 1_000_000,
    }
}

#[test]
fn snapshot_staged_uses_index_blobs_and_is_immutable() {
    let repository = candidate_repository();
    let snapshot =
        CandidateSnapshot::materialize(repository.path(), ReviewSource::Staged, generous_limits())
            .unwrap();

    assert_eq!(
        fs::read_to_string(snapshot.path().join("tracked.txt")).unwrap(),
        "staged\n"
    );
    assert!(!snapshot.path().join(".git").exists());
    assert!(!snapshot.path().join("untracked.txt").exists());
    assert_eq!(snapshot.files, 1);
    assert_eq!(snapshot.bytes, 7);
    assert_eq!(snapshot.snapshot_id, snapshot.sha256[..16]);
    snapshot.verify_unchanged().unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = snapshot.path().join("tracked.txt");
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o222, 0);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(path, "mutated\n").unwrap();
        assert!(snapshot.verify_unchanged().is_err());
    }
}

#[test]
fn snapshot_unstaged_uses_tracked_working_tree_bytes() {
    let repository = candidate_repository();
    let snapshot = CandidateSnapshot::materialize(
        repository.path(),
        ReviewSource::Unstaged,
        generous_limits(),
    )
    .unwrap();

    assert_eq!(
        fs::read_to_string(snapshot.path().join("tracked.txt")).unwrap(),
        "working\n"
    );
    assert!(!snapshot.path().join("untracked.txt").exists());
}

#[test]
fn snapshot_branch_uses_head_tree_bytes() {
    let repository = candidate_repository();
    let snapshot =
        CandidateSnapshot::materialize(repository.path(), ReviewSource::Branch, generous_limits())
            .unwrap();

    assert_eq!(
        fs::read_to_string(snapshot.path().join("tracked.txt")).unwrap(),
        "head\n"
    );
}

#[test]
fn snapshot_enforces_file_and_byte_limits() {
    let repository = candidate_repository();
    let file_error = CandidateSnapshot::materialize(
        repository.path(),
        ReviewSource::Staged,
        SnapshotLimits {
            max_files: 0,
            max_bytes: 1_000_000,
        },
    )
    .unwrap_err();
    assert!(file_error.to_string().contains("0-file profile limit"));

    let byte_error = CandidateSnapshot::materialize(
        repository.path(),
        ReviewSource::Staged,
        SnapshotLimits {
            max_files: 100,
            max_bytes: 1,
        },
    )
    .unwrap_err();
    assert!(byte_error
        .to_string()
        .contains("remaining snapshot byte limit"));
}

#[cfg(unix)]
#[test]
fn snapshot_rejects_symlinks_that_escape_the_root() {
    use std::os::unix::fs::symlink;

    let repository = TempDir::new().unwrap();
    git(repository.path(), &["init", "-q"]);
    git(
        repository.path(),
        &["config", "user.email", "review@example.test"],
    );
    git(repository.path(), &["config", "user.name", "Review Test"]);
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("secret.txt"), "secret\n").unwrap();
    symlink(
        outside.path().join("secret.txt"),
        repository.path().join("escape"),
    )
    .unwrap();
    git(repository.path(), &["add", "escape"]);

    let error =
        CandidateSnapshot::materialize(repository.path(), ReviewSource::Staged, generous_limits())
            .unwrap_err();
    assert!(error.to_string().contains("absolute symlink"));
}

#[test]
fn snapshot_omits_gitlinks() {
    let repository = TempDir::new().unwrap();
    git(repository.path(), &["init", "-q"]);
    git(
        repository.path(),
        &["config", "user.email", "review@example.test"],
    );
    git(repository.path(), &["config", "user.name", "Review Test"]);
    fs::write(repository.path().join("tracked.txt"), "tracked\n").unwrap();
    git(repository.path(), &["add", "tracked.txt"]);
    git(repository.path(), &["commit", "-qm", "base"]);
    let head = git(repository.path(), &["rev-parse", "HEAD"]);
    git(
        repository.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{head},vendor/sub"),
        ],
    );

    let snapshot =
        CandidateSnapshot::materialize(repository.path(), ReviewSource::Staged, generous_limits())
            .unwrap();
    assert!(snapshot.path().join("tracked.txt").exists());
    assert!(!snapshot.path().join("vendor/sub").exists());
    assert_eq!(snapshot.files, 1);
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
fn sha256_file(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

#[cfg(unix)]
#[test]
fn run_analysis_uses_source_specific_candidate_bytes() {
    let repository = TempDir::new().unwrap();
    git(repository.path(), &["init", "-q"]);
    git(
        repository.path(),
        &["config", "user.email", "review@example.test"],
    );
    git(repository.path(), &["config", "user.name", "Review Test"]);
    fs::write(repository.path().join("tracked.txt"), "main\n").unwrap();
    git(repository.path(), &["add", "tracked.txt"]);
    git(repository.path(), &["commit", "-qm", "main"]);
    git(repository.path(), &["switch", "-qc", "feature"]);
    fs::write(repository.path().join("tracked.txt"), "branch\n").unwrap();
    git(repository.path(), &["add", "tracked.txt"]);
    git(repository.path(), &["commit", "-qm", "branch"]);
    fs::write(repository.path().join("tracked.txt"), "staged\n").unwrap();
    git(repository.path(), &["add", "tracked.txt"]);
    fs::write(repository.path().join("tracked.txt"), "unstaged\n").unwrap();

    let tools = TempDir::new().unwrap();
    let executable = write_executable(
        tools.path(),
        "mode-analyzer.sh",
        r#"#!/bin/sh
expected="$1"
observed=$(cat tracked.txt)
if [ "$observed" != "$expected" ]; then
  printf 'expected %s, observed %s\n' "$expected" "$observed" >&2
  exit 9
fi
printf '{"schema_version":1,"kind":"static_analysis_input","scope_fingerprint":"%s","tool":{"name":"fixture","version":"1.0"},"status":"completed","findings":[]}' "$PRE_COMMIT_REVIEW_SCOPE_FINGERPRINT"
"#,
    );
    let executable_hash = sha256_file(&executable);

    for (source, expected) in [
        (ReviewSource::Staged, "staged"),
        (ReviewSource::Unstaged, "unstaged"),
        (ReviewSource::Branch, "branch"),
    ] {
        let profile = tools.path().join(format!("profile-{expected}.json"));
        fs::write(
            &profile,
            serde_json::to_vec(&json!({
                "schema_version": 1,
                "kind": "static_analysis_profile",
                "name": "source mode profile",
                "tool": {"name": "fixture", "version": "1.0"},
                "executable": {
                    "path": executable.to_string_lossy(),
                    "sha256": executable_hash
                },
                "arguments": [expected],
                "output_format": "normalized-json",
                "success_exit_codes": [0],
                "limits": {
                    "timeout_seconds": 10,
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
        let profile_hash = sha256_file(&profile);
        let scope = open_authoritative_scope(ScopeRequest {
            repository: repository.path().to_path_buf(),
            source: Some(source),
            expected_fingerprint: None,
        })
        .unwrap();

        let artifact = run_analysis(RunRequest {
            repository: repository.path().to_path_buf(),
            source,
            expected_scope: scope.fingerprint,
            profile_path: profile,
            expected_profile_sha256: profile_hash,
            allow_repository_configuration: false,
            max_findings: 500,
        })
        .unwrap();
        assert_eq!(
            artifact.execution.execution.status,
            ExecutionStatus::Completed
        );
    }
}
