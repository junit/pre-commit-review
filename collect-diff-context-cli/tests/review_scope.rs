use collect_diff_context_cli::collect_diff_context_main;
use collect_diff_context_cli::review_scope::{
    open_authoritative_scope, ReviewSource, ScopeRequest,
};
use std::{error::Error, fs, path::Path, process::Command};
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git must start");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn library_exports_collect_diff_context_entrypoint() {
    let _: fn() -> i32 = collect_diff_context_main;
}

#[test]
fn typed_scope_matches_control_plane() -> Result<(), Box<dyn Error>> {
    let repo = TempDir::new()?;
    git(repo.path(), &["init", "-q"]);
    git(
        repo.path(),
        &["config", "user.email", "review@example.test"],
    );
    git(repo.path(), &["config", "user.name", "Review Test"]);
    fs::write(repo.path().join("README.md"), "base\n")?;
    git(repo.path(), &["add", "README.md"]);
    git(repo.path(), &["commit", "-qm", "base"]);
    fs::create_dir_all(repo.path().join("src"))?;
    fs::write(
        repo.path().join("src/app.rs"),
        "pub fn value() -> u8 { 1 }\n",
    )?;
    git(repo.path(), &["add", "src/app.rs"]);

    let scope = open_authoritative_scope(ScopeRequest {
        repository: repo.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_fingerprint: None,
    })?;

    assert!(scope.authoritative);
    assert_eq!(scope.source, ReviewSource::Staged);
    assert_eq!(scope.units[0].path, "src/app.rs");
    assert_eq!(scope.collection_start, scope.collection_end);
    assert_eq!(scope.fingerprint, scope.collection_end);
    Ok(())
}
