mod support;

use collect_diff_context_cli::candidate::{CandidatePresence, RepoPath};
use collect_diff_context_cli::impact_context::contracts::{Completeness, UnitStatus};
use collect_diff_context_cli::impact_context::index::budget::{IndexBudget, IndexBudgetTracker};
use collect_diff_context_cli::impact_context::index::manifest::{
    GitRepositoryManifestSource, RepositoryManifestSource,
};
use collect_diff_context_cli::review_scope::ReviewSource;
use sha2::{Digest, Sha256};
use std::error::Error;
use std::time::Duration;
use support::GitRepo;

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn collect(
    source: &GitRepositoryManifestSource,
) -> Result<
    collect_diff_context_cli::impact_context::index::model::RepositoryManifest,
    Box<dyn Error>,
> {
    let mut budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
    Ok(source.manifest_bounded(&mut budget)?)
}

#[test]
fn staged_manifest_contains_unchanged_and_stage_zero_content() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/base.rs", b"pub fn base() {}\n")?;
    repo.write("src/new.rs", b"pub fn staged() {}\n")?;
    repo.git(["add", "--", "src/new.rs"])?;
    repo.write("src/new.rs", b"pub fn working() {}\n")?;

    let scope = repo.scope(ReviewSource::Staged)?;
    let source = GitRepositoryManifestSource::new(&scope)?;
    let manifest = collect(&source)?;

    assert_eq!(
        manifest
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["src/base.rs", "src/new.rs"]
    );
    let staged = manifest
        .entries
        .iter()
        .find(|entry| entry.path.as_str() == "src/new.rs")
        .unwrap();
    assert_eq!(
        staged.content_sha256.as_deref(),
        Some(sha256(b"pub fn staged() {}\n").as_str())
    );
    assert_ne!(
        staged.content_sha256.as_deref(),
        Some(sha256(b"pub fn working() {}\n").as_str())
    );
    assert_eq!(
        source
            .read_bounded(&RepoPath::new("src/new.rs")?, 1024)?
            .bytes,
        b"pub fn staged() {}\n"
    );
    Ok(())
}

#[test]
fn unstaged_manifest_uses_tracked_worktree_bytes_and_excludes_untracked(
) -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/base.rs", b"pub fn base() {}\n")?;
    repo.write("src/staged.rs", b"pub fn staged() {}\n")?;
    repo.git(["add", "--", "src/staged.rs"])?;
    repo.write("src/base.rs", b"pub fn working() {}\n")?;
    repo.write("src/untracked.rs", b"pub fn untracked() {}\n")?;

    let scope = repo.scope(ReviewSource::Unstaged)?;
    let source = GitRepositoryManifestSource::new(&scope)?;
    let manifest = collect(&source)?;
    let paths = manifest
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>();

    assert_eq!(paths, vec!["src/base.rs", "src/staged.rs"]);
    assert!(source.repository_locator().index_manifest_digest.is_some());
    assert_eq!(
        manifest.entries[0].content_sha256.as_deref(),
        Some(sha256(b"pub fn working() {}\n").as_str())
    );
    assert_eq!(
        manifest.entries[1].content_sha256.as_deref(),
        Some(sha256(b"pub fn staged() {}\n").as_str())
    );
    Ok(())
}

#[test]
fn branch_manifest_uses_committed_tree_despite_worktree_changes() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"pub fn base() {}\n")?;
    repo.git(["checkout", "-qb", "feature"])?;
    repo.commit_file("src/lib.rs", b"pub fn committed() {}\n")?;
    repo.write("src/lib.rs", b"pub fn working() {}\n")?;

    let scope = repo.scope(ReviewSource::Branch)?;
    let source = GitRepositoryManifestSource::new(&scope)?;
    let manifest = collect(&source)?;

    assert_eq!(manifest.entries.len(), 1);
    assert_eq!(
        manifest.entries[0].content_sha256.as_deref(),
        Some(sha256(b"pub fn committed() {}\n").as_str())
    );
    assert_eq!(
        source
            .read_bounded(&RepoPath::new("src/lib.rs")?, 1024)?
            .bytes,
        b"pub fn committed() {}\n"
    );
    Ok(())
}

#[test]
fn manifest_digest_is_path_sorted_and_repeatable() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("README.md", b"base\n")?;
    repo.git(["checkout", "-qb", "feature"])?;
    repo.commit_file("src/z.rs", b"z\n")?;
    repo.commit_file("src/a.rs", b"a\n")?;

    let scope = repo.scope(ReviewSource::Branch)?;
    let source = GitRepositoryManifestSource::new(&scope)?;
    let first = collect(&source)?;
    let second = collect(&source)?;

    assert_eq!(first, second);
    assert_eq!(first.digest, second.digest);
    assert!(first
        .entries
        .windows(2)
        .all(|pair| pair[0].path < pair[1].path));
    assert!(first.validate().is_ok());
    Ok(())
}

#[cfg(unix)]
#[test]
fn manifest_preserves_delete_mode_symlink_and_gitlink_states() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let repo = GitRepo::new()?;
    repo.commit_file("src/deleted.rs", b"delete me\n")?;
    repo.commit_file("scripts/run.sh", b"#!/bin/sh\n")?;
    repo.git(["rm", "-q", "--", "src/deleted.rs"])?;

    let executable = repo.path().join("scripts/run.sh");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))?;
    repo.git(["add", "--", "scripts/run.sh"])?;

    std::fs::create_dir_all(repo.path().join("src"))?;
    symlink("../scripts/run.sh", repo.path().join("src/run-link"))?;
    repo.git(["add", "--", "src/run-link"])?;

    let head = String::from_utf8(repo.git(["rev-parse", "HEAD"])?.stdout)?;
    repo.git([
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{},vendor/sub", head.trim()),
    ])?;

    let scope = repo.scope(ReviewSource::Staged)?;
    let source = GitRepositoryManifestSource::new(&scope)?;
    let manifest = collect(&source)?;

    let deleted = manifest
        .entries
        .iter()
        .find(|entry| entry.path.as_str() == "src/deleted.rs")
        .unwrap();
    assert_eq!(deleted.presence, CandidatePresence::Deleted);
    assert_eq!(deleted.mode, "000000");

    let executable = manifest
        .entries
        .iter()
        .find(|entry| entry.path.as_str() == "scripts/run.sh")
        .unwrap();
    assert_eq!(executable.mode, "100755");

    let symlink = manifest
        .entries
        .iter()
        .find(|entry| entry.path.as_str() == "src/run-link")
        .unwrap();
    assert_eq!(symlink.mode, "120000");
    assert_eq!(
        symlink.content_sha256.as_deref(),
        Some(sha256(b"../scripts/run.sh").as_str())
    );

    let gitlink = manifest
        .entries
        .iter()
        .find(|entry| entry.path.as_str() == "vendor/sub")
        .unwrap();
    assert_eq!(gitlink.mode, "160000");
    assert_eq!(gitlink.presence, CandidatePresence::Gitlink);
    Ok(())
}

#[test]
fn manifest_limits_return_explicit_partial_entries() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"base\n")?;
    repo.git(["checkout", "-qb", "feature"])?;
    repo.commit_file("src/lib.rs", b"larger than four bytes\n")?;

    let scope = repo.scope(ReviewSource::Branch)?;
    let source = GitRepositoryManifestSource::new(&scope)?;
    let mut limits = IndexBudget::deep_defaults();
    limits.max_file_bytes = 4;
    let mut budget = IndexBudgetTracker::new(limits);
    let manifest = source.manifest_bounded(&mut budget)?;

    assert_eq!(manifest.completeness, Completeness::Partial);
    assert_eq!(manifest.entries.len(), 1);
    assert_eq!(manifest.entries[0].status, UnitStatus::BudgetExhausted);
    assert_eq!(
        manifest.entries[0].limitation_codes,
        vec!["index-file-byte-budget-exhausted"]
    );
    assert!(manifest.entries[0].content_sha256.is_none());
    assert!(manifest.validate().is_ok());
    Ok(())
}

#[test]
fn manifest_file_limit_prevents_out_of_budget_blob_reads() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.write("src/a.rs", b"base a\n")?;
    repo.write("src/z.rs", b"base z\n")?;
    repo.git(["add", "--", "src/a.rs", "src/z.rs"])?;
    repo.git(["commit", "-qm", "fixture"])?;
    repo.write("src/a.rs", b"staged a\n")?;
    repo.git(["add", "--", "src/a.rs"])?;

    let scope = repo.scope(ReviewSource::Staged)?;
    let source = GitRepositoryManifestSource::new(&scope)?;
    let object_id = String::from_utf8(repo.git(["rev-parse", ":src/z.rs"])?.stdout)?;
    let object_id = object_id.trim();
    let object_path = repo
        .path()
        .join(".git/objects")
        .join(&object_id[..2])
        .join(&object_id[2..]);
    std::fs::remove_file(object_path)?;

    let mut limits = IndexBudget::deep_defaults();
    limits.max_manifest_files = 1;
    let mut budget = IndexBudgetTracker::new(limits);
    let manifest = source.manifest_bounded(&mut budget)?;

    assert_eq!(manifest.completeness, Completeness::Partial);
    assert_eq!(manifest.entries.len(), 1);
    assert_eq!(manifest.entries[0].path.as_str(), "src/a.rs");
    assert!(manifest
        .limitations
        .iter()
        .any(|limitation| { limitation.code == "index-manifest-file-budget-exhausted" }));
    Ok(())
}

#[test]
fn candidate_locator_changes_when_index_or_overlay_changes() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/base.rs", b"base\n")?;
    repo.write("src/staged.rs", b"staged\n")?;
    repo.git(["add", "--", "src/staged.rs"])?;
    repo.write("src/base.rs", b"working-one\n")?;

    let first_scope = repo.scope(ReviewSource::Unstaged)?;
    let first = GitRepositoryManifestSource::new(&first_scope)?;
    repo.write("src/base.rs", b"working-two\n")?;
    let second_scope = repo.scope(ReviewSource::Unstaged)?;
    let second = GitRepositoryManifestSource::new(&second_scope)?;

    assert_eq!(
        first.repository_locator().index_manifest_digest,
        second.repository_locator().index_manifest_digest
    );
    assert_ne!(
        first.repository_locator().overlay_candidate_digest,
        second.repository_locator().overlay_candidate_digest
    );

    repo.write("src/second-staged.rs", b"second staged\n")?;
    repo.git(["add", "--", "src/second-staged.rs"])?;
    let third_scope = repo.scope(ReviewSource::Unstaged)?;
    let third = GitRepositoryManifestSource::new(&third_scope)?;
    assert_ne!(
        second.repository_locator().index_manifest_digest,
        third.repository_locator().index_manifest_digest
    );
    Ok(())
}

#[test]
fn manifest_git_process_obeys_shared_deadline_and_output_limit() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"pub fn base() {}\n")?;
    repo.git(["checkout", "-qb", "feature"])?;
    repo.commit_file("src/lib.rs", b"pub fn value() {}\n")?;
    let scope = repo.scope(ReviewSource::Branch)?;
    let source = GitRepositoryManifestSource::new(&scope)?;

    let mut limits = IndexBudget::deep_defaults();
    limits.deadline = Duration::ZERO;
    let mut budget = IndexBudgetTracker::new(limits);
    let error = source
        .manifest_bounded(&mut budget)
        .expect_err("zero deadline must stop repository Git work");
    assert_eq!(error.code, "index-deadline-exhausted");
    Ok(())
}
