use collect_diff_context_cli::review_scope::ReviewSource;
use collect_diff_context_cli::static_analysis::snapshot::{CandidateSnapshot, SnapshotLimits};
use std::fs;
use std::path::Path;
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
