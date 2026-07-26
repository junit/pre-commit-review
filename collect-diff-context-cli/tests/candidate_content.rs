mod support;

use collect_diff_context_cli::candidate::{
    CandidateContent, CandidateOpenLimits, CandidatePresence, GitCandidateContent, RepoPath,
};
use collect_diff_context_cli::review_scope::ReviewSource;
use sha2::{Digest, Sha256};
use std::error::Error;
use std::time::Duration;
use support::GitRepo;
use tempfile::TempDir;

#[cfg(unix)]
#[test]
fn candidate_open_does_not_invoke_repository_configured_fsmonitor() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;

    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"base\n")?;
    repo.write("src/lib.rs", b"candidate\n")?;
    repo.git(["add", "--", "src/lib.rs"])?;

    let marker_root = TempDir::new()?;
    let marker = marker_root.path().join("fsmonitor-invoked");
    repo.write(
        "fsmonitor.sh",
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()).as_bytes(),
    )?;
    let fsmonitor = repo.path().join("fsmonitor.sh");
    std::fs::set_permissions(&fsmonitor, std::fs::Permissions::from_mode(0o755))?;
    repo.git([
        "config",
        "core.fsmonitor",
        fsmonitor.to_string_lossy().as_ref(),
    ])?;

    let scope = repo.scope(ReviewSource::Staged)?;
    GitCandidateContent::open(&scope)?;

    assert!(
        !marker.exists(),
        "read-only candidate Git commands invoked core.fsmonitor"
    );
    Ok(())
}

#[test]
fn staged_reads_stage_zero_blob_without_worktree_fallback() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("README.md", b"base\n")?;
    repo.write("src/lib.rs", b"staged\n")?;
    repo.git(["add", "--", "src/lib.rs"])?;
    repo.write("src/lib.rs", b"working\n")?;

    let scope = repo.scope(ReviewSource::Staged)?;
    let candidate = GitCandidateContent::open(&scope)?;
    let content = candidate.read(&RepoPath::new("src/lib.rs")?)?;

    assert_eq!(content.bytes, b"staged\n");
    assert_eq!(content.sha256, format!("{:x}", Sha256::digest(b"staged\n")));
    Ok(())
}

#[test]
fn staged_bounded_read_rejects_a_blob_above_the_byte_limit() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("README.md", b"base\n")?;
    repo.write("src/lib.rs", b"larger-than-limit\n")?;
    repo.git(["add", "--", "src/lib.rs"])?;

    let scope = repo.scope(ReviewSource::Staged)?;
    let candidate = GitCandidateContent::open(&scope)?;
    let error = candidate
        .read_bounded(&RepoPath::new("src/lib.rs")?, 4)
        .expect_err("oversized candidate bytes must not be released");

    assert!(error.is_byte_limit_exceeded());
    Ok(())
}

#[test]
fn unstaged_open_bounded_preserves_an_oversized_unit_without_hashing_it(
) -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"base\n")?;
    repo.write("src/lib.rs", b"larger-than-limit\n")?;

    let scope = repo.scope(ReviewSource::Unstaged)?;
    let candidate = GitCandidateContent::open_bounded(
        &scope,
        CandidateOpenLimits {
            deadline: Duration::from_secs(1),
            max_changed_files: 30,
            max_file_bytes: 4,
            max_total_bytes: 100,
        },
    )?;
    let file = candidate
        .files()
        .iter()
        .find(|file| file.path.as_str() == "src/lib.rs")
        .unwrap();
    assert_eq!(file.presence, CandidatePresence::Present);
    assert!(file.content_identity.is_none());

    let error = candidate
        .read_bounded(&RepoPath::new("src/lib.rs")?, 4)
        .expect_err("oversized unit must remain resource-limited");
    assert_eq!(
        error.budget_limitation_code(),
        Some("file-byte-budget-exhausted")
    );
    Ok(())
}

#[test]
fn unstaged_open_bounded_preserves_units_beyond_the_changed_file_limit(
) -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/a.rs", b"base-a\n")?;
    repo.commit_file("src/b.rs", b"base-b\n")?;
    repo.write("src/a.rs", b"next-a\n")?;
    repo.write("src/b.rs", b"next-b\n")?;

    let scope = repo.scope(ReviewSource::Unstaged)?;
    let candidate = GitCandidateContent::open_bounded(
        &scope,
        CandidateOpenLimits {
            deadline: Duration::from_secs(1),
            max_changed_files: 1,
            max_file_bytes: 100,
            max_total_bytes: 100,
        },
    )?;

    assert_eq!(candidate.files().len(), 2);
    let error = candidate
        .read_bounded(&RepoPath::new("src/b.rs")?, 100)
        .expect_err("units beyond the changed-file budget must remain visible");
    assert_eq!(
        error.budget_limitation_code(),
        Some("changed-file-budget-exhausted")
    );
    Ok(())
}

#[test]
fn unstaged_open_bounded_stops_hashing_after_the_total_byte_limit() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/a.rs", b"aaaa\n")?;
    repo.commit_file("src/b.rs", b"bbbb\n")?;
    repo.write("src/a.rs", b"one\n")?;
    repo.write("src/b.rs", b"two\n")?;

    let scope = repo.scope(ReviewSource::Unstaged)?;
    let candidate = GitCandidateContent::open_bounded(
        &scope,
        CandidateOpenLimits {
            deadline: Duration::from_secs(1),
            max_changed_files: 30,
            max_file_bytes: 100,
            max_total_bytes: 4,
        },
    )?;

    candidate.read_bounded(&RepoPath::new("src/a.rs")?, 100)?;
    let error = candidate
        .read_bounded(&RepoPath::new("src/b.rs")?, 100)
        .expect_err("total-byte exhaustion must stop later candidate hashing");
    assert_eq!(
        error.budget_limitation_code(),
        Some("total-byte-budget-exhausted")
    );
    Ok(())
}

#[test]
fn staged_read_refuses_to_lazy_fetch_a_missing_promisor_blob() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("README.md", b"base\n")?;
    repo.write("src/lib.rs", b"candidate\n")?;
    repo.git(["add", "--", "src/lib.rs"])?;
    repo.git(["commit", "-qm", "candidate"])?;

    let remote = TempDir::new()?;
    repo.git([
        "clone",
        "--bare",
        ".",
        remote.path().to_string_lossy().as_ref(),
    ])?;
    repo.git(["reset", "--soft", "HEAD~1"])?;
    let scope = repo.scope(ReviewSource::Staged)?;
    let candidate = GitCandidateContent::open(&scope)?;
    let object_id = String::from_utf8(repo.git(["rev-parse", ":src/lib.rs"])?.stdout)?;
    let object_id = object_id.trim();

    repo.git(["config", "core.repositoryformatversion", "1"])?;
    repo.git(["config", "extensions.partialClone", "origin"])?;
    repo.git([
        "config",
        "remote.origin.url",
        remote.path().to_string_lossy().as_ref(),
    ])?;
    repo.git(["config", "remote.origin.promisor", "true"])?;
    repo.git(["config", "remote.origin.partialclonefilter", "blob:none"])?;
    let object_path = repo
        .path()
        .join(".git/objects")
        .join(&object_id[..2])
        .join(&object_id[2..]);
    assert!(
        object_path.exists(),
        "fixture blob must start locally present"
    );
    std::fs::remove_file(&object_path)?;

    let error = candidate
        .read(&RepoPath::new("src/lib.rs")?)
        .expect_err("fast candidate reads must not lazy-fetch missing objects");

    assert!(error.to_string().contains("candidate blob"));
    assert!(
        !object_path.exists(),
        "candidate read must not rewrite .git/objects"
    );
    Ok(())
}

#[test]
fn staged_open_refuses_to_lazy_fetch_a_missing_promisor_blob() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("README.md", b"base\n")?;
    repo.write("src/lib.rs", b"candidate\n")?;
    repo.git(["add", "--", "src/lib.rs"])?;
    repo.git(["commit", "-qm", "candidate"])?;

    let remote = TempDir::new()?;
    repo.git([
        "clone",
        "--bare",
        ".",
        remote.path().to_string_lossy().as_ref(),
    ])?;
    repo.git(["reset", "--soft", "HEAD~1"])?;
    let scope = repo.scope(ReviewSource::Staged)?;
    let object_id = String::from_utf8(repo.git(["rev-parse", ":src/lib.rs"])?.stdout)?;
    let object_id = object_id.trim();

    repo.git(["config", "core.repositoryformatversion", "1"])?;
    repo.git(["config", "extensions.partialClone", "origin"])?;
    repo.git([
        "config",
        "remote.origin.url",
        remote.path().to_string_lossy().as_ref(),
    ])?;
    repo.git(["config", "remote.origin.promisor", "true"])?;
    repo.git(["config", "remote.origin.partialclonefilter", "blob:none"])?;
    let object_path = repo
        .path()
        .join(".git/objects")
        .join(&object_id[..2])
        .join(&object_id[2..]);
    std::fs::remove_file(&object_path)?;

    let candidate = GitCandidateContent::open(&scope)?;
    assert!(
        !object_path.exists(),
        "candidate opening must not rewrite .git/objects"
    );
    let error = candidate
        .read(&RepoPath::new("src/lib.rs")?)
        .expect_err("missing candidate objects must remain unavailable");
    assert!(error.to_string().contains("candidate blob"));
    assert!(
        !object_path.exists(),
        "candidate reading must not rewrite .git/objects"
    );
    Ok(())
}

#[test]
fn unstaged_reads_tracked_worktree_bytes_and_excludes_untracked() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"base\n")?;
    repo.write("src/lib.rs", b"working\n")?;
    repo.write("src/untracked.rs", b"untracked\n")?;

    let scope = repo.scope(ReviewSource::Unstaged)?;
    let candidate = GitCandidateContent::open(&scope)?;
    let content = candidate.read(&RepoPath::new("src/lib.rs")?)?;

    assert_eq!(content.bytes, b"working\n");
    assert!(candidate
        .files()
        .iter()
        .all(|file| file.path.as_str() != "src/untracked.rs"));
    Ok(())
}

#[test]
fn branch_reads_head_tree_bytes() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"base\n")?;
    repo.git(["checkout", "-qb", "feature"])?;
    repo.write("src/lib.rs", b"committed\n")?;
    repo.git(["add", "--", "src/lib.rs"])?;
    repo.git(["commit", "-qm", "change"])?;
    repo.write("src/lib.rs", b"working\n")?;

    let scope = repo.scope(ReviewSource::Branch)?;
    let candidate = GitCandidateContent::open(&scope)?;
    let content = candidate.read(&RepoPath::new("src/lib.rs")?)?;

    assert_eq!(content.bytes, b"committed\n");
    Ok(())
}

#[test]
fn candidate_input_manifest_is_path_sorted_and_digest_is_stable() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("README.md", b"base\n")?;
    repo.commit_file(".pre-commit-review/context-queries", b"unsafe-query\n")?;
    repo.commit_file(".pre-commit-review/test-hints", b"cargo test\n")?;
    repo.write("z.rs", b"fn z() {}\n")?;
    repo.write("a.rs", b"fn a() {}\n")?;
    repo.git(["add", "--", "z.rs", "a.rs"])?;

    let scope = repo.scope(ReviewSource::Staged)?;
    let first = GitCandidateContent::open(&scope)?;
    let second = GitCandidateContent::open(&scope)?;
    let paths = first
        .files()
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        paths,
        vec![
            ".pre-commit-review/context-queries",
            ".pre-commit-review/test-hints",
            "a.rs",
            "z.rs",
        ]
    );
    assert_eq!(first.candidate_digest(), second.candidate_digest());
    assert_eq!(first.candidate_digest().len(), 64);
    Ok(())
}

#[test]
fn deleted_gitlink_binary_mode_only_and_rename_remain_visible() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("deleted.rs", b"fn deleted() {}\n")?;
    repo.commit_file("mode.sh", b"#!/bin/sh\n")?;
    repo.commit_file("old.rs", b"fn renamed() {}\n")?;
    repo.git(["rm", "-q", "--", "deleted.rs"])?;
    repo.git(["mv", "--", "old.rs", "new.rs"])?;
    repo.git(["update-index", "--chmod=+x", "mode.sh"])?;
    repo.write("binary.bin", b"binary\0payload")?;
    repo.git(["add", "--", "binary.bin"])?;
    let head = String::from_utf8(repo.git(["rev-parse", "HEAD"])?.stdout)?;
    let cache_info = format!("160000,{},vendor/submodule", head.trim());
    repo.git(["update-index", "--add", "--cacheinfo", cache_info.as_str()])?;

    let scope = repo.scope(ReviewSource::Staged)?;
    let candidate = GitCandidateContent::open(&scope)?;
    let file = |path: &str| {
        candidate
            .files()
            .iter()
            .find(|file| file.path.as_str() == path)
            .unwrap_or_else(|| panic!("missing candidate unit {path}"))
    };

    assert_eq!(file("deleted.rs").presence, CandidatePresence::Deleted);
    assert_eq!(
        file("vendor/submodule").presence,
        CandidatePresence::Gitlink
    );
    assert_eq!(file("mode.sh").mode, "100755");
    assert_eq!(file("new.rs").change_status.as_deref(), Some("R100"));
    assert!(candidate.read(&RepoPath::new("binary.bin")?)?.binary);
    Ok(())
}

#[cfg(unix)]
#[test]
fn unstaged_symlink_reads_link_target_without_following_it() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::symlink;

    let repo = GitRepo::new()?;
    repo.write("first-target", b"first contents\n")?;
    repo.write("second-target", b"second contents\n")?;
    symlink("first-target", repo.path().join("current"))?;
    repo.git(["add", "--", "first-target", "second-target", "current"])?;
    repo.git(["commit", "-qm", "links"])?;
    std::fs::remove_file(repo.path().join("current"))?;
    symlink("second-target", repo.path().join("current"))?;

    let scope = repo.scope(ReviewSource::Unstaged)?;
    let candidate = GitCandidateContent::open(&scope)?;
    let content = candidate.read(&RepoPath::new("current")?)?;

    assert_eq!(content.bytes, b"second-target");
    assert_eq!(
        content.sha256,
        format!("{:x}", Sha256::digest(b"second-target"))
    );
    Ok(())
}

#[test]
fn space_tab_and_unicode_paths_remain_distinct() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("README.md", b"base\n")?;
    for path in ["space name.rs", "tab\tname.rs", "snow-雪.rs"] {
        repo.write(path, format!("// {path}\n"))?;
        repo.git(["add", "--", path])?;
    }

    let scope = repo.scope(ReviewSource::Staged)?;
    let candidate = GitCandidateContent::open(&scope)?;
    let paths = candidate
        .files()
        .iter()
        .filter(|file| file.manifest_unit_id.is_some())
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();

    assert_eq!(paths, vec!["snow-雪.rs", "space name.rs", "tab\tname.rs"]);
    Ok(())
}

#[test]
fn scope_path_rejects_absolute_parent_and_nul_paths() {
    for path in [
        "",
        "/absolute/path",
        "../escape",
        "safe/../escape",
        "nul\0path",
        "C:\\absolute\\path",
    ] {
        assert!(
            RepoPath::new(path).is_err(),
            "accepted invalid path {path:?}"
        );
    }
    assert!(RepoPath::new("a".repeat(4097)).is_err());
    assert!(RepoPath::new("safe/child.rs").is_ok());
}

#[test]
fn changed_ranges_preserve_deletion_only_hunk_anchors() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"first\nremoved\nlast\n")?;
    repo.write("src/lib.rs", b"first\nlast\n")?;
    repo.git(["add", "--", "src/lib.rs"])?;

    let scope = repo.scope(ReviewSource::Staged)?;
    let candidate = GitCandidateContent::open(&scope)?;
    let ranges = &candidate
        .files()
        .iter()
        .find(|file| file.path.as_str() == "src/lib.rs")
        .expect("changed file must remain visible")
        .changed_ranges;

    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].start_line, 1);
    assert_eq!(ranges[0].end_line, 1);
    assert!(ranges[0].deletion_anchor);
    Ok(())
}

#[test]
fn unstaged_read_rejects_candidate_identity_drift() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"base\n")?;
    repo.write("src/lib.rs", b"first candidate\n")?;

    let scope = repo.scope(ReviewSource::Unstaged)?;
    let candidate = GitCandidateContent::open(&scope)?;
    repo.write("src/lib.rs", b"second candidate\n")?;

    let error = candidate
        .read(&RepoPath::new("src/lib.rs")?)
        .expect_err("drifted bytes must not be released");
    assert!(error.to_string().contains("candidate content changed"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn unstaged_mode_change_uses_worktree_mode() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;

    let repo = GitRepo::new()?;
    repo.git(["config", "core.filemode", "true"])?;
    repo.commit_file("mode.sh", b"#!/bin/sh\n")?;
    let mut permissions = std::fs::metadata(repo.path().join("mode.sh"))?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(repo.path().join("mode.sh"), permissions)?;

    let scope = repo.scope(ReviewSource::Unstaged)?;
    let candidate = GitCandidateContent::open(&scope)?;
    let file = candidate
        .files()
        .iter()
        .find(|file| file.path.as_str() == "mode.sh")
        .expect("mode-only change must remain visible");

    assert_eq!(file.mode, "100755");
    Ok(())
}
