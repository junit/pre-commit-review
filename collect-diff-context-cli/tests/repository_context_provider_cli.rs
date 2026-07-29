#[allow(dead_code)]
mod support;

use collect_diff_context_cli::repository_context_provider::contract::RustAnalyzerProjectModel;
use collect_diff_context_cli::review_scope::ReviewSource;
use std::error::Error;
use std::process::{Command, Output};
use support::GitRepo;

fn provider_cli(repository: &GitRepo, arguments: &[&str]) -> Result<Output, Box<dyn Error>> {
    Ok(
        Command::new(env!("CARGO_BIN_EXE_repository-context-provider-cli"))
            .args(arguments)
            .current_dir(repository.path())
            .output()?,
    )
}

#[test]
fn help_and_parser_failures_are_stable() -> Result<(), Box<dyn Error>> {
    let repository = GitRepo::new()?;
    let help = provider_cli(&repository, &["--help"])?;
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout)?;
    assert!(help.contains("repository-context-provider-cli model"));
    assert!(help.contains("repository-context-provider-cli run"));

    let model_help = provider_cli(&repository, &["model", "--help"])?;
    assert!(model_help.status.success());
    let model_help = String::from_utf8(model_help.stdout)?;
    assert!(model_help.contains("--source <staged|unstaged|branch>"));
    assert!(model_help.contains("--max-model-files"));
    assert!(model_help.contains("--max-model-bytes"));

    let run_help = provider_cli(&repository, &["run", "--help"])?;
    assert!(run_help.status.success());
    assert!(String::from_utf8(run_help.stdout)?.contains("--expect-registry-sha256"));

    let scope = "a".repeat(64);
    let invalid_cases = vec![
        vec!["unknown".to_string()],
        vec!["model".to_string(), "--unknown".to_string()],
        vec![
            "model".to_string(),
            "--source".to_string(),
            "staged".to_string(),
        ],
        vec![
            "model".to_string(),
            "--expect-scope".to_string(),
            scope.clone(),
        ],
        vec![
            "model".to_string(),
            "--source".to_string(),
            "working-tree".to_string(),
            "--expect-scope".to_string(),
            scope.clone(),
        ],
        vec![
            "model".to_string(),
            "--source".to_string(),
            "staged".to_string(),
            "--expect-scope".to_string(),
            "A".repeat(64),
        ],
        vec![
            "model".to_string(),
            "--source".to_string(),
            "staged".to_string(),
            "--source=staged".to_string(),
            "--expect-scope".to_string(),
            scope.clone(),
        ],
        vec![
            "model".to_string(),
            "--source".to_string(),
            "staged".to_string(),
            "--expect-scope".to_string(),
            scope.clone(),
            "--max-model-files=0".to_string(),
        ],
        vec![
            "model".to_string(),
            "--source".to_string(),
            "staged".to_string(),
            "--expect-scope".to_string(),
            scope.clone(),
            "--max-model-bytes".to_string(),
        ],
        vec![
            "run".to_string(),
            "--source".to_string(),
            "staged".to_string(),
            "--expect-scope".to_string(),
            scope,
            "--registry".to_string(),
            "relative/registry.json".to_string(),
            "--expect-registry-sha256".to_string(),
            "b".repeat(64),
            "--provider-id".to_string(),
            "local".to_string(),
            "--model".to_string(),
            "/tmp/model.json".to_string(),
            "--expect-model-sha256".to_string(),
            "c".repeat(64),
            "--request".to_string(),
            "/tmp/request.json".to_string(),
        ],
    ];
    for arguments in invalid_cases {
        let arguments = arguments.iter().map(String::as_str).collect::<Vec<_>>();
        let output = provider_cli(&repository, &arguments)?;
        assert_eq!(output.status.code(), Some(2), "arguments: {arguments:?}");
        assert!(output.stdout.is_empty(), "arguments: {arguments:?}");
        let stderr = String::from_utf8(output.stderr)?;
        assert!(
            stderr.starts_with("repository-context-provider-cli: provider-cli-"),
            "arguments: {arguments:?}, stderr: {stderr}"
        );
        assert!(stderr.len() <= 512);
    }
    Ok(())
}

#[test]
fn model_emits_one_compact_deterministic_digest_bound_value() -> Result<(), Box<dyn Error>> {
    let repository = GitRepo::new()?;
    repository.commit_file("README.md", b"base\n")?;
    repository.write(
        "Cargo.toml",
        b"[package]\nname = \"cli-model\"\nedition = \"2021\"\n",
    )?;
    repository.write("src/lib.rs", b"pub fn cli_model() {}\n")?;
    repository.git(["add", "--", "Cargo.toml", "src/lib.rs"])?;
    let scope = repository.scope(ReviewSource::Staged)?;
    let arguments = [
        "model",
        "--source=staged",
        "--expect-scope",
        &scope.fingerprint,
        "--max-model-files=64",
        "--max-model-bytes",
        "65536",
    ];

    let first = provider_cli(&repository, &arguments)?;
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(first.stderr.is_empty());
    let model: RustAnalyzerProjectModel = serde_json::from_slice(&first.stdout)?;
    model.validate()?;
    assert_eq!(model.digest, model.canonical_sha256());
    assert_eq!(
        model
            .crates
            .iter()
            .map(|item| item.root_module.as_str())
            .collect::<Vec<_>>(),
        vec!["src/lib.rs"]
    );
    assert!(model
        .limitations
        .windows(2)
        .all(|items| items[0] < items[1]));
    assert_eq!(
        first.stdout,
        format!("{}\n", serde_json::to_string(&model)?).as_bytes()
    );
    assert!(
        !String::from_utf8_lossy(&first.stdout).contains(&repository.path().display().to_string())
    );

    let repeated = provider_cli(&repository, &arguments)?;
    assert!(repeated.status.success());
    assert!(repeated.stderr.is_empty());
    assert_eq!(first.stdout, repeated.stdout);
    Ok(())
}

#[test]
fn model_rejects_scope_drift_without_stdout() -> Result<(), Box<dyn Error>> {
    let repository = GitRepo::new()?;
    repository.commit_file("README.md", b"base\n")?;
    repository.write(
        "Cargo.toml",
        b"[package]\nname = \"drift\"\nedition = \"2021\"\n",
    )?;
    repository.write("src/lib.rs", b"pub fn drift() {}\n")?;
    repository.git(["add", "--", "Cargo.toml", "src/lib.rs"])?;
    let scope = repository.scope(ReviewSource::Staged)?;
    repository.write("src/lib.rs", b"pub fn changed_after_scope() {}\n")?;
    repository.git(["add", "--", "src/lib.rs"])?;

    let output = provider_cli(
        &repository,
        &[
            "model",
            "--source",
            "staged",
            "--expect-scope",
            &scope.fingerprint,
        ],
    )?;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr)?.contains("provider-cli-scope-invalid"));
    Ok(())
}
