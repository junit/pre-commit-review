mod support;

use collect_diff_context_cli::impact_context::contracts::{ImpactContext, ImpactStatus};
use collect_diff_context_cli::review_scope::ReviewSource;
use sha2::{Digest, Sha256};
use std::error::Error;
use std::process::{Command, Output};
use support::GitRepo;

fn repository_context(repo: &GitRepo, arguments: &[&str]) -> Result<Output, Box<dyn Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_repository-context-cli"))
        .args(arguments)
        .current_dir(repo.path())
        .env("PRE_COMMIT_REVIEW_SECRET_SCAN", "off")
        .output()?)
}

fn repository_context_with_required_sanitizer(
    repo: &GitRepo,
    arguments: &[&str],
) -> Result<Output, Box<dyn Error>> {
    let unavailable_scanner = repo.path().join("missing-gitleaks");
    Ok(Command::new(env!("CARGO_BIN_EXE_repository-context-cli"))
        .args(arguments)
        .current_dir(repo.path())
        .env_remove("PRE_COMMIT_REVIEW_SECRET_SCAN")
        .env("PRE_COMMIT_REVIEW_GITLEAKS_BIN", unavailable_scanner)
        .env_remove("PRE_COMMIT_REVIEW_GITLEAKS_CONFIG")
        .output()?)
}

#[test]
fn help_and_unsupported_subcommands_are_stable() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    let help = repository_context(&repo, &["--help"])?;
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout)?.contains("repository-context-cli collect"));

    let collect_help = repository_context(&repo, &["collect", "--help"])?;
    assert!(collect_help.status.success());
    assert!(String::from_utf8(collect_help.stdout)?.contains("--mode fast"));

    for arguments in [&["index"][..], &["collect", "--mode", "deep"][..]] {
        let output = repository_context(&repo, arguments)?;
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8(output.stderr)?.starts_with("repository-context-cli:"));
    }
    Ok(())
}

#[test]
fn collect_requires_source_scope_and_fast_mode() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    for arguments in [
        &[
            "collect",
            "--expect-scope",
            &"a".repeat(40),
            "--mode",
            "fast",
        ][..],
        &["collect", "--source", "staged", "--mode", "fast"][..],
        &[
            "collect",
            "--source",
            "staged",
            "--expect-scope",
            &"a".repeat(40),
        ][..],
    ] {
        let output = repository_context(&repo, arguments)?;
        assert_eq!(output.status.code(), Some(2));
    }
    Ok(())
}

#[test]
fn staged_collect_uses_stage_zero_bytes_and_emits_valid_compact_json() -> Result<(), Box<dyn Error>>
{
    let repo = GitRepo::new()?;
    repo.commit_file("README.md", b"base\n")?;
    repo.write("src/lib.rs", b"pub fn staged() {}\n")?;
    repo.git(["add", "--", "src/lib.rs"])?;
    repo.write("src/lib.rs", b"pub fn working() {}\n")?;
    let scope = repo.scope(ReviewSource::Staged)?;

    let output = repository_context(
        &repo,
        &[
            "collect",
            "--source",
            "staged",
            "--expect-scope",
            &scope.fingerprint,
            "--mode",
            "fast",
        ],
    )?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.contains(&b'\n'));
    let context: ImpactContext = serde_json::from_slice(&output.stdout)?;
    context.validate()?;
    assert_eq!(context.status, ImpactStatus::Completed);
    assert_eq!(context.scope.fingerprint, scope.fingerprint);
    assert_eq!(context.units.len(), 1);
    assert_eq!(
        context.units[0].content_sha256.as_deref(),
        Some(format!("{:x}", Sha256::digest(b"pub fn staged() {}\n")).as_str())
    );
    assert!(context
        .changed_symbols
        .iter()
        .any(|symbol| symbol.name == "staged"));
    assert!(context
        .changed_symbols
        .iter()
        .all(|symbol| symbol.name != "working"));
    Ok(())
}

#[test]
fn wrong_scope_fingerprint_is_rejected() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("README.md", b"base\n")?;
    repo.write("src/lib.rs", b"pub fn changed() {}\n")?;
    repo.git(["add", "--", "src/lib.rs"])?;

    let output = repository_context(
        &repo,
        &[
            "collect",
            "--source",
            "staged",
            "--expect-scope",
            "0000000000000000000000000000000000000000",
            "--mode",
            "fast",
        ],
    )?;

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr)?.starts_with("repository-context-cli:"));
    Ok(())
}

#[test]
fn unstaged_and_branch_collect_use_their_exact_candidate_sources() -> Result<(), Box<dyn Error>> {
    let unstaged = GitRepo::new()?;
    unstaged.commit_file("src/lib.rs", b"pub fn base() {}\n")?;
    unstaged.write("src/lib.rs", b"pub fn working() {}\n")?;
    unstaged.write("src/untracked.rs", b"pub fn untracked() {}\n")?;
    let unstaged_scope = unstaged.scope(ReviewSource::Unstaged)?;
    let unstaged_output = repository_context(
        &unstaged,
        &[
            "collect",
            "--source",
            "unstaged",
            "--expect-scope",
            &unstaged_scope.fingerprint,
            "--mode",
            "fast",
        ],
    )?;
    assert!(unstaged_output.status.success());
    let unstaged_context: ImpactContext = serde_json::from_slice(&unstaged_output.stdout)?;
    assert_eq!(unstaged_context.units.len(), 1);
    assert_eq!(unstaged_context.units[0].path, "src/lib.rs");
    assert!(unstaged_context
        .changed_symbols
        .iter()
        .any(|symbol| symbol.name == "working"));

    let branch = GitRepo::new()?;
    branch.commit_file("src/lib.rs", b"pub fn base() {}\n")?;
    branch.git(["checkout", "-qb", "feature"])?;
    branch.write("src/lib.rs", b"pub fn committed() {}\n")?;
    branch.git(["add", "--", "src/lib.rs"])?;
    branch.git(["commit", "-qm", "change"])?;
    branch.write("src/lib.rs", b"pub fn working() {}\n")?;
    let branch_scope = branch.scope(ReviewSource::Branch)?;
    let branch_output = repository_context(
        &branch,
        &[
            "collect",
            "--source",
            "branch",
            "--expect-scope",
            &branch_scope.fingerprint,
            "--mode",
            "fast",
        ],
    )?;
    assert!(branch_output.status.success());
    let branch_context: ImpactContext = serde_json::from_slice(&branch_output.stdout)?;
    assert!(branch_context
        .changed_symbols
        .iter()
        .any(|symbol| symbol.name == "committed"));
    assert!(branch_context
        .changed_symbols
        .iter()
        .all(|symbol| symbol.name != "working"));
    Ok(())
}

#[test]
fn limit_overrides_can_only_lower_fast_defaults() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    for arguments in [
        vec![
            "collect",
            "--source",
            "staged",
            "--expect-scope",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--mode",
            "fast",
            "--max-nodes",
            "0",
        ],
        vec![
            "collect",
            "--source",
            "staged",
            "--expect-scope",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--mode",
            "fast",
            "--deadline-ms",
            "751",
        ],
        vec![
            "collect",
            "--source",
            "staged",
            "--expect-scope",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--mode",
            "fast",
            "--max-file-bytes",
            "10",
            "--max-total-bytes",
            "5",
        ],
    ] {
        let output = repository_context(&repo, &arguments)?;
        assert_eq!(output.status.code(), Some(2));
    }
    Ok(())
}

#[test]
fn unavailable_required_sanitizer_releases_failed_context_without_source_facts(
) -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("README.md", b"base\n")?;
    repo.write("src/lib.rs", b"pub fn sensitive_name() {}\n")?;
    repo.git(["add", "--", "src/lib.rs"])?;
    let scope = repo.scope(ReviewSource::Staged)?;

    let output = repository_context_with_required_sanitizer(
        &repo,
        &[
            "collect",
            "--source",
            "staged",
            "--expect-scope",
            &scope.fingerprint,
            "--mode",
            "fast",
        ],
    )?;

    assert!(output.status.success());
    let context: ImpactContext = serde_json::from_slice(&output.stdout)?;
    context.validate()?;
    assert_eq!(context.status, ImpactStatus::Failed);
    assert!(context.changed_symbols.is_empty());
    assert!(context.impact_edges.is_empty());
    assert!(context.domain_summaries.is_empty());
    assert!(context
        .limitations
        .iter()
        .any(|limitation| limitation.code == "output-sanitization-unavailable"));
    Ok(())
}
