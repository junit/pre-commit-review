mod support;

use collect_diff_context_cli::impact_context::contracts::{ImpactContext, ImpactStatus};
use collect_diff_context_cli::review_scope::ReviewSource;
use sha2::{Digest, Sha256};
use std::error::Error;
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::PathBuf;
use std::process::{Command, Output};
#[cfg(unix)]
use std::time::{Duration, Instant};
use support::GitRepo;
#[cfg(unix)]
use tempfile::TempDir;

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

#[cfg(unix)]
fn executable_on_path(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let path = std::env::var_os("PATH").ok_or("PATH is unavailable")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| format!("cannot find {name} on PATH").into())
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
fn candidate_preparation_limits_release_valid_bounded_context() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"pub fn base() {}\n")?;
    repo.write("src/lib.rs", b"pub fn larger_than_limit() {}\n")?;
    let scope = repo.scope(ReviewSource::Unstaged)?;

    let output = repository_context(
        &repo,
        &[
            "collect",
            "--source",
            "unstaged",
            "--expect-scope",
            &scope.fingerprint,
            "--mode",
            "fast",
            "--max-file-bytes",
            "4",
        ],
    )?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let context: ImpactContext = serde_json::from_slice(&output.stdout)?;
    context.validate()?;
    assert_eq!(context.status, ImpactStatus::Unavailable);
    assert!(context
        .limitations
        .iter()
        .any(|limitation| limitation.code == "file-byte-budget-exhausted"));
    assert_eq!(context.units.len(), 1);
    assert!(context.units[0].content_sha256.is_none());
    Ok(())
}

#[cfg(unix)]
#[test]
fn candidate_preparation_deadline_terminates_slow_git() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"pub fn base() {}\n")?;
    repo.write("src/lib.rs", b"pub fn changed() {}\n")?;
    repo.git(["add", "--", "src/lib.rs"])?;
    let scope = repo.scope(ReviewSource::Staged)?;

    let wrapper_root = TempDir::new()?;
    let wrapper = wrapper_root.path().join("git");
    fs::write(
        &wrapper,
        b"#!/bin/sh\ncase \"$SLOW_GIT_PHASE: $* \" in\n  scope:*\" rev-parse --show-toplevel \"*) sleep 2 ;;\n  output:*\" rev-parse --show-toplevel \"*) dd if=/dev/zero bs=1048576 count=17 2>/dev/null; exit 0 ;;\n  revalidate:*\" rev-parse HEAD \"*)\n    count=0\n    if [ -f \"$SLOW_GIT_STATE\" ]; then count=$(cat \"$SLOW_GIT_STATE\"); fi\n    count=$((count + 1))\n    printf '%s\\n' \"$count\" > \"$SLOW_GIT_STATE\"\n    if [ \"$count\" -ge 2 ]; then sleep 2; fi\n    ;;\n  list:*\" ls-files --stage \"*) sleep 2 ;;\n  size:*\" cat-file -s \"*) sleep 2 ;;\n  ranges:*\" --unified=0 \"*) sleep 2 ;;\n  blob:*\" cat-file blob \"*) sleep 2 ;;\nesac\nexec \"$REAL_GIT\" \"$@\"\n",
    )?;
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))?;
    let real_git = executable_on_path("git")?;
    let original_path = std::env::var_os("PATH").ok_or("PATH is unavailable")?;
    let injected_path = std::env::join_paths(
        std::iter::once(wrapper_root.path().to_path_buf())
            .chain(std::env::split_paths(&original_path)),
    )?;

    let slow_git_state = wrapper_root.path().join("slow-git-state");
    for phase in [
        "scope",
        "output",
        "list",
        "size",
        "ranges",
        "blob",
        "revalidate",
    ] {
        let _ = fs::remove_file(&slow_git_state);
        let started = Instant::now();
        let output = Command::new(env!("CARGO_BIN_EXE_repository-context-cli"))
            .args([
                "collect",
                "--source",
                "staged",
                "--expect-scope",
                &scope.fingerprint,
                "--mode",
                "fast",
                "--deadline-ms",
                "750",
            ])
            .current_dir(repo.path())
            .env("PATH", &injected_path)
            .env("REAL_GIT", &real_git)
            .env("SLOW_GIT_PHASE", phase)
            .env("SLOW_GIT_STATE", &slow_git_state)
            .env("PRE_COMMIT_REVIEW_SECRET_SCAN", "off")
            .output()?;

        if phase == "revalidate" {
            assert_eq!(
                output.status.code(),
                Some(3),
                "revalidation timeout must invalidate the context"
            );
        } else {
            assert!(
                matches!(output.status.code(), Some(2 | 3)),
                "unexpected slow Git phase status for {phase}: {:?}",
                output.status.code()
            );
        }
        assert!(
            started.elapsed() < Duration::from_millis(1_500),
            "slow Git phase {phase} escaped the fast-path deadline: {:?}",
            started.elapsed()
        );
        let diagnostic = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let expected_diagnostic = if phase == "output" {
            "output"
        } else {
            "deadline"
        };
        assert!(
            diagnostic.contains(expected_diagnostic),
            "fast-path failure must report {expected_diagnostic} for {phase}: {diagnostic}"
        );
        if output.status.code() == Some(3) {
            let context: ImpactContext = serde_json::from_slice(&output.stdout)?;
            context.validate()?;
            assert_eq!(context.status, ImpactStatus::Invalidated);
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn fast_path_deadline_terminates_slow_git_descendants() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"pub fn base() {}\n")?;
    repo.write("src/lib.rs", b"pub fn changed() {}\n")?;
    repo.git(["add", "--", "src/lib.rs"])?;
    let scope = repo.scope(ReviewSource::Staged)?;

    let wrapper_root = TempDir::new()?;
    let wrapper = wrapper_root.path().join("git");
    let child_pid_path = wrapper_root.path().join("child-pid");
    fs::write(
        &wrapper,
        b"#!/bin/sh\ncase \"$* \" in\n  *\"rev-parse --show-toplevel \"*)\n    sleep 10 &\n    child=$!\n    printf '%s\\n' \"$child\" > \"$SLOW_GIT_CHILD_PID\"\n    wait \"$child\"\n    ;;\nesac\nexec \"$REAL_GIT\" \"$@\"\n",
    )?;
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))?;
    let real_git = executable_on_path("git")?;
    let original_path = std::env::var_os("PATH").ok_or("PATH is unavailable")?;
    let injected_path = std::env::join_paths(
        std::iter::once(wrapper_root.path().to_path_buf())
            .chain(std::env::split_paths(&original_path)),
    )?;

    let output = Command::new(env!("CARGO_BIN_EXE_repository-context-cli"))
        .args([
            "collect",
            "--source",
            "staged",
            "--expect-scope",
            &scope.fingerprint,
            "--mode",
            "fast",
            "--deadline-ms",
            "750",
        ])
        .current_dir(repo.path())
        .env("PATH", &injected_path)
        .env("REAL_GIT", &real_git)
        .env("SLOW_GIT_CHILD_PID", &child_pid_path)
        .env("PRE_COMMIT_REVIEW_SECRET_SCAN", "off")
        .output()?;
    assert_eq!(output.status.code(), Some(2));

    let child_pid = fs::read_to_string(&child_pid_path)?.trim().parse::<i32>()?;
    let descendant_stopped = (0..50).any(|_| {
        // SAFETY: signal 0 only probes the recorded child process id.
        let result = unsafe { libc::kill(child_pid, 0) };
        if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            true
        } else {
            std::thread::sleep(Duration::from_millis(10));
            false
        }
    });
    assert!(descendant_stopped, "slow Git descendant survived timeout");
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
