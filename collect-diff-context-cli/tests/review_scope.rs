use collect_diff_context_cli::collect_diff_context_main;
use collect_diff_context_cli::review_scope::{
    open_authoritative_scope, revalidate_scope, ReviewSource, ScopeRequest,
};
use serde_json::Value;
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

fn control_plane(repo: &Path, environment: &[(&str, &str)]) -> Result<Value, Box<dyn Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_collect-diff-context-cli"));
    command
        .args(["--control-plane", "--source", "staged"])
        .current_dir(repo);
    for (name, value) in environment {
        command.env(name, value);
    }
    let output = command.output()?;
    assert!(
        output.status.success(),
        "control plane failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    let payload = stdout
        .split("## Review Control Plane JSON\n")
        .nth(1)
        .and_then(|remainder| remainder.lines().next())
        .ok_or("control-plane JSON is missing")?;
    Ok(serde_json::from_str(payload)?)
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

#[test]
fn custom_risk_configuration_changes_scope_fingerprint() -> Result<(), Box<dyn Error>> {
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
    fs::create_dir_all(repo.path().join(".pre-commit-review"))?;
    let risk_paths = repo.path().join(".pre-commit-review/risk-paths");
    fs::write(&risk_paths, "^src/\n")?;

    let matching = open_authoritative_scope(ScopeRequest {
        repository: repo.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_fingerprint: None,
    })?;
    fs::write(&risk_paths, "^never/\n")?;
    let non_matching = open_authoritative_scope(ScopeRequest {
        repository: repo.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_fingerprint: None,
    })?;

    assert_ne!(matching.fingerprint, non_matching.fingerprint);
    Ok(())
}

#[test]
fn revalidation_rejects_custom_risk_configuration_drift() -> Result<(), Box<dyn Error>> {
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
    fs::create_dir_all(repo.path().join(".pre-commit-review"))?;
    let risk_paths = repo.path().join(".pre-commit-review/risk-paths");
    fs::write(&risk_paths, "^src/\n")?;
    let scope = open_authoritative_scope(ScopeRequest {
        repository: repo.path().to_path_buf(),
        source: Some(ReviewSource::Staged),
        expected_fingerprint: None,
    })?;

    fs::write(&risk_paths, "^never/\n")?;
    let error = revalidate_scope(&scope).expect_err("risk configuration drift must invalidate");

    assert!(
        error.to_string().contains("risk configuration changed"),
        "{error}"
    );
    Ok(())
}

#[test]
fn group_budget_configuration_changes_scope_fingerprint() -> Result<(), Box<dyn Error>> {
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

    let split = control_plane(
        repo.path(),
        &[
            ("PRE_COMMIT_REVIEW_GROUP_TARGET_BYTES", "1"),
            ("PRE_COMMIT_REVIEW_GROUP_HARD_BYTES", "1"),
        ],
    )?;
    let default = control_plane(repo.path(), &[])?;

    assert_ne!(split["scope_fingerprint"], default["scope_fingerprint"]);
    Ok(())
}
