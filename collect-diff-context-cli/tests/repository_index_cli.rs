#[allow(dead_code)]
mod support;

use collect_diff_context_cli::impact_context::contracts::{ImpactContext, ImpactStatus};
use collect_diff_context_cli::impact_context::index::model::{
    IndexAction, IndexReport, IndexReportStatus,
};
use collect_diff_context_cli::review_scope::ReviewSource;
use rusqlite::{Connection, OpenFlags};
use std::error::Error;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::UNIX_EPOCH;
use support::GitRepo;

fn repository_context(
    repo: &GitRepo,
    cache: &Path,
    arguments: &[&str],
) -> Result<Output, Box<dyn Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_repository-context-cli"))
        .args(arguments)
        .current_dir(repo.path())
        .env("PRE_COMMIT_REVIEW_CACHE_DIR", cache)
        .env("PRE_COMMIT_REVIEW_SECRET_SCAN", "off")
        .output()?)
}

fn rust_repository() -> Result<GitRepo, Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.write(
        "Cargo.toml",
        b"[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    repo.write(
        "src/lib.rs",
        b"pub fn validate() -> bool { true }\npub fn caller() -> bool { validate() }\n",
    )?;
    repo.git(["add", "--", "Cargo.toml", "src/lib.rs"])?;
    repo.git(["commit", "-qm", "fixture"])?;
    repo.write(
        "src/lib.rs",
        b"pub fn validate() -> bool { false }\npub fn caller() -> bool { validate() }\n",
    )?;
    repo.git(["add", "--", "src/lib.rs"])?;
    Ok(repo)
}

fn parse_report(output: &Output) -> Result<IndexReport, Box<dyn Error>> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.contains(&b'\n'));
    let report: IndexReport = serde_json::from_slice(&output.stdout)?;
    report.validate()?;
    Ok(report)
}

fn build_index(repo: &GitRepo, cache: &Path) -> Result<IndexReport, Box<dyn Error>> {
    let scope = repo.scope(ReviewSource::Staged)?;
    let output = repository_context(
        repo,
        cache,
        &[
            "index",
            "build",
            "--source",
            "staged",
            "--expect-scope",
            &scope.fingerprint,
        ],
    )?;
    parse_report(&output)
}

fn generation_path(cache: &Path, report: &IndexReport) -> PathBuf {
    cache
        .join("v2")
        .join("repos")
        .join(&report.repository_id)
        .join("graphs")
        .join(format!(
            "{}.sqlite",
            report.generation_key.as_deref().unwrap()
        ))
}

fn snapshot(root: &Path) -> Vec<(String, u64, u128)> {
    fn visit(base: &Path, path: &Path, output: &mut Vec<(String, u64, u128)>) {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            output.push((
                path.strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                metadata.len(),
                metadata
                    .modified()
                    .unwrap()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            if metadata.is_dir() {
                visit(base, &path, output);
            }
        }
    }
    let mut output = Vec::new();
    visit(root, root, &mut output);
    output.sort();
    output
}

#[test]
fn help_lists_collect_fast_deep_and_index_subcommands() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    let cache = tempfile::tempdir()?;

    let help = repository_context(&repo, cache.path(), &["--help"])?;
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout)?;
    assert!(help.contains("repository-context-cli collect"));
    assert!(help.contains("repository-context-cli index"));

    let collect = repository_context(&repo, cache.path(), &["collect", "--help"])?;
    assert!(collect.status.success());
    let collect = String::from_utf8(collect.stdout)?;
    assert!(collect.contains("--mode <fast|deep>"));

    let index = repository_context(&repo, cache.path(), &["index", "--help"])?;
    assert!(index.status.success());
    let index = String::from_utf8(index.stdout)?;
    for command in ["build", "doctor", "inspect", "clean"] {
        assert!(index.contains(command));
    }
    Ok(())
}

#[test]
fn index_build_requires_source_expected_scope_and_lower_only_limits() -> Result<(), Box<dyn Error>>
{
    let repo = GitRepo::new()?;
    let cache = tempfile::tempdir()?;
    let fingerprint = "a".repeat(40);

    for arguments in [
        vec!["index", "build", "--expect-scope", &fingerprint],
        vec!["index", "build", "--source", "staged"],
        vec![
            "index",
            "build",
            "--source",
            "staged",
            "--expect-scope",
            &fingerprint,
            "--max-symbols",
            "1000001",
        ],
        vec![
            "index",
            "build",
            "--source",
            "staged",
            "--expect-scope",
            &fingerprint,
            "--max-graph-depth",
            "3",
        ],
    ] {
        let output = repository_context(&repo, cache.path(), &arguments)?;
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8(output.stderr)?.starts_with("repository-context-cli:"));
    }
    Ok(())
}

#[test]
fn index_build_emits_valid_compact_report_and_publishes_generation() -> Result<(), Box<dyn Error>> {
    let repo = rust_repository()?;
    let cache = tempfile::tempdir()?;
    let scope = repo.scope(ReviewSource::Staged)?;

    let output = repository_context(
        &repo,
        cache.path(),
        &[
            "index",
            "build",
            "--source",
            "staged",
            "--expect-scope",
            &scope.fingerprint,
        ],
    )?;
    let report = parse_report(&output)?;

    assert_eq!(report.action, IndexAction::Build);
    assert_eq!(report.status, IndexReportStatus::Completed);
    assert_eq!(
        report.scope_fingerprint.as_deref(),
        Some(scope.fingerprint.as_str())
    );
    assert!(report.metrics.file_fact_writes > 0);
    let path = generation_path(cache.path(), &report);
    assert!(path.is_file());
    Ok(())
}

#[test]
fn index_doctor_is_read_only_and_reports_corrupt_or_orphaned_objects() -> Result<(), Box<dyn Error>>
{
    let repo = rust_repository()?;
    let cache = tempfile::tempdir()?;
    let built = build_index(&repo, cache.path())?;
    let generation = built.generation_key.as_deref().unwrap();
    let before = snapshot(cache.path());

    let healthy = repository_context(
        &repo,
        cache.path(),
        &["index", "doctor", "--generation", generation],
    )?;
    let healthy = parse_report(&healthy)?;
    assert_eq!(healthy.action, IndexAction::Doctor);
    assert_eq!(healthy.status, IndexReportStatus::Completed);
    assert_eq!(snapshot(cache.path()), before);

    let fact = snapshot(cache.path())
        .into_iter()
        .find(|(path, _, _)| path.ends_with(".facts"))
        .map(|(path, _, _)| cache.path().join(path))
        .ok_or("built index did not publish FileFacts")?;
    let orphan = fact.parent().unwrap().join("orphan.facts");
    fs::copy(&fact, &orphan)?;
    let orphan_before = snapshot(cache.path());
    let orphaned = repository_context(
        &repo,
        cache.path(),
        &["index", "doctor", "--generation", generation],
    )?;
    let orphaned = parse_report(&orphaned)?;
    assert_eq!(orphaned.status, IndexReportStatus::Partial);
    assert!(orphaned
        .limitations
        .iter()
        .any(|limitation| limitation.code == "repository-index-file-facts-corrupt"));
    assert_eq!(snapshot(cache.path()), orphan_before);
    fs::remove_file(orphan)?;

    let path = generation_path(cache.path(), &built);
    fs::OpenOptions::new()
        .write(true)
        .open(&path)?
        .set_len(32)?;
    let corrupt_before = snapshot(cache.path());
    let corrupt = repository_context(
        &repo,
        cache.path(),
        &["index", "doctor", "--generation", generation],
    )?;
    let corrupt = parse_report(&corrupt)?;
    assert_eq!(corrupt.status, IndexReportStatus::Partial);
    assert!(corrupt
        .limitations
        .iter()
        .any(|limitation| limitation.code == "repository-index-generation-corrupt"));
    assert_eq!(snapshot(cache.path()), corrupt_before);
    Ok(())
}

#[test]
fn index_doctor_without_generation_ignores_valid_locator_references() -> Result<(), Box<dyn Error>>
{
    let repo = rust_repository()?;
    let cache = tempfile::tempdir()?;
    build_index(&repo, cache.path())?;
    let before = snapshot(cache.path());

    let output = repository_context(&repo, cache.path(), &["index", "doctor"])?;
    let report = parse_report(&output)?;

    assert_eq!(report.status, IndexReportStatus::Completed);
    assert_eq!(snapshot(cache.path()), before);
    Ok(())
}

#[test]
fn index_doctor_reports_corrupt_locator_references_without_writes() -> Result<(), Box<dyn Error>> {
    let repo = rust_repository()?;
    let cache = tempfile::tempdir()?;
    build_index(&repo, cache.path())?;
    let reference = snapshot(cache.path())
        .into_iter()
        .map(|(path, _, _)| cache.path().join(path))
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .ok_or("missing generation locator reference")?;
    fs::write(reference, b"{")?;
    let before = snapshot(cache.path());

    let output = repository_context(&repo, cache.path(), &["index", "doctor"])?;
    let report = parse_report(&output)?;

    assert_eq!(report.status, IndexReportStatus::Partial);
    assert!(report
        .limitations
        .iter()
        .any(|limitation| { limitation.code == "repository-index-generation-reference-corrupt" }));
    assert_eq!(snapshot(cache.path()), before);
    Ok(())
}

#[test]
fn index_inspect_requires_exact_digest_path_or_symbol_and_bounds_rows() -> Result<(), Box<dyn Error>>
{
    let repo = rust_repository()?;
    let cache = tempfile::tempdir()?;
    let built = build_index(&repo, cache.path())?;
    let generation = built.generation_key.as_deref().unwrap();

    for arguments in [
        vec!["index", "inspect", "--generation", generation],
        vec![
            "index",
            "inspect",
            "--generation",
            generation,
            "--path",
            "src/lib.rs",
            "--symbol",
            generation,
        ],
        vec![
            "index",
            "inspect",
            "--generation",
            "not-a-digest",
            "--path",
            "src/lib.rs",
        ],
    ] {
        let output = repository_context(&repo, cache.path(), &arguments)?;
        assert_eq!(output.status.code(), Some(2));
    }

    let before = snapshot(cache.path());
    let path_output = repository_context(
        &repo,
        cache.path(),
        &[
            "index",
            "inspect",
            "--generation",
            generation,
            "--path",
            "src/lib.rs",
            "--max-rows",
            "1",
        ],
    )?;
    let path_report = parse_report(&path_output)?;
    assert_eq!(path_report.action, IndexAction::Inspect);
    assert_eq!(path_report.status, IndexReportStatus::Completed);
    assert!(path_report.metrics.query_rows <= 1);
    assert!(path_report.metrics.symbols <= 1);

    let connection = Connection::open_with_flags(
        generation_path(cache.path(), &built),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let symbol: String = connection.query_row(
        "SELECT symbol_id FROM symbols ORDER BY symbol_id LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    drop(connection);
    let symbol_output = repository_context(
        &repo,
        cache.path(),
        &[
            "index",
            "inspect",
            "--generation",
            generation,
            "--symbol",
            &symbol,
            "--max-rows",
            "1",
        ],
    )?;
    let symbol_report = parse_report(&symbol_output)?;
    assert_eq!(symbol_report.metrics.symbols, 1);
    assert_eq!(snapshot(cache.path()), before);
    Ok(())
}

#[test]
fn index_clean_defaults_to_dry_run_and_stays_inside_repository_namespace(
) -> Result<(), Box<dyn Error>> {
    let repo = rust_repository()?;
    let cache = tempfile::tempdir()?;
    let built = build_index(&repo, cache.path())?;
    let generation = built.generation_key.as_deref().unwrap();
    let generation_path = generation_path(cache.path(), &built);
    let sentinel = cache.path().join("outside-repository-namespace");
    fs::write(&sentinel, b"keep")?;

    let dry_run = repository_context(&repo, cache.path(), &["index", "clean"])?;
    let dry_run = parse_report(&dry_run)?;
    assert_eq!(dry_run.action, IndexAction::Clean);
    assert_eq!(dry_run.status, IndexReportStatus::Completed);
    assert!(generation_path.is_file());

    let execute = repository_context(
        &repo,
        cache.path(),
        &[
            "index",
            "clean",
            "--execute",
            "--max-bytes",
            "1",
            "--retain-generations",
            "0",
        ],
    )?;
    let execute = parse_report(&execute)?;
    assert_eq!(execute.status, IndexReportStatus::Completed);
    assert!(!generation_path.exists());
    assert!(!snapshot(cache.path())
        .iter()
        .any(|(path, _, _)| { path.ends_with(&format!("{generation}.json")) }));
    assert_eq!(fs::read(&sentinel)?, b"keep");

    for arguments in [
        &["index", "clean", "--dry-run", "--execute"][..],
        &["index", "clean", "--max-bytes", "0"][..],
    ] {
        let output = repository_context(&repo, cache.path(), arguments)?;
        assert_eq!(output.status.code(), Some(2));
    }
    Ok(())
}

#[test]
fn index_clean_bounds_generation_enumeration_and_integrity_scan() -> Result<(), Box<dyn Error>> {
    let repo = rust_repository()?;
    let cache = tempfile::tempdir()?;
    let built = build_index(&repo, cache.path())?;
    let generation_path = generation_path(cache.path(), &built);
    let graphs = generation_path.parent().ok_or("missing graph directory")?;
    let extra_generation = graphs.join(format!("{}.sqlite", "f".repeat(64)));
    fs::copy(&generation_path, &extra_generation)?;

    let generation_limited = repository_context(
        &repo,
        cache.path(),
        &["index", "clean", "--invalid", "--max-scan-generations", "1"],
    )?;
    let generation_limited = parse_report(&generation_limited)?;
    assert_eq!(generation_limited.status, IndexReportStatus::Partial);
    assert!(generation_limited.limitations.iter().any(|limitation| {
        limitation.code == "repository-index-clean-generation-budget-exhausted"
    }));
    assert!(generation_path.is_file());
    assert!(extra_generation.is_file());

    fs::remove_file(&extra_generation)?;
    let byte_limited = repository_context(
        &repo,
        cache.path(),
        &["index", "clean", "--invalid", "--max-scan-bytes", "1"],
    )?;
    let byte_limited = parse_report(&byte_limited)?;
    assert_eq!(byte_limited.status, IndexReportStatus::Partial);
    assert!(byte_limited.limitations.iter().any(|limitation| {
        limitation.code == "repository-index-clean-scan-byte-budget-exhausted"
    }));
    assert!(generation_path.is_file());

    for arguments in [
        &["index", "clean", "--max-scan-generations", "0"][..],
        &["index", "clean", "--max-scan-bytes", "0"][..],
        &["index", "clean", "--timeout-ms", "0"][..],
    ] {
        let output = repository_context(&repo, cache.path(), arguments)?;
        assert_eq!(output.status.code(), Some(2));
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn index_clean_does_not_follow_symlinked_locator_directory() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::symlink;

    let repo = rust_repository()?;
    let cache = tempfile::tempdir()?;
    let built = build_index(&repo, cache.path())?;
    let generation = built.generation_key.as_deref().unwrap();
    let generation_path = generation_path(cache.path(), &built);
    let graphs = generation_path.parent().unwrap();
    let locator_root = graphs.join("locators");
    let outside = cache.path().join("outside-locators");
    fs::rename(&locator_root, &outside)?;
    symlink(&outside, &locator_root)?;
    let outside_reference = snapshot(&outside)
        .into_iter()
        .map(|(path, _, _)| outside.join(path))
        .find(|path| path.ends_with(format!("{generation}.json")))
        .ok_or("missing locator reference")?;

    let output = repository_context(
        &repo,
        cache.path(),
        &[
            "index",
            "clean",
            "--execute",
            "--max-bytes",
            "1",
            "--retain-generations",
            "0",
        ],
    )?;
    let report = parse_report(&output)?;

    assert_eq!(report.status, IndexReportStatus::Partial);
    assert!(!generation_path.exists());
    assert!(outside_reference.is_file());
    assert!(report
        .limitations
        .iter()
        .any(|limitation| { limitation.code == "repository-index-clean-reference-remove-failed" }));
    Ok(())
}

#[test]
fn index_clean_invalid_removes_generation_with_corrupt_graph_rows() -> Result<(), Box<dyn Error>> {
    let repo = rust_repository()?;
    let cache = tempfile::tempdir()?;
    let built = build_index(&repo, cache.path())?;
    let generation_path = generation_path(cache.path(), &built);
    let connection = Connection::open(&generation_path)?;
    let updated = connection.execute(
        "UPDATE edges SET kind = 'invalid-kind' WHERE edge_id = (SELECT edge_id FROM edges ORDER BY edge_id LIMIT 1)",
        [],
    )?;
    assert_eq!(updated, 1);
    drop(connection);

    let output = repository_context(
        &repo,
        cache.path(),
        &["index", "clean", "--invalid", "--execute"],
    )?;
    let report = parse_report(&output)?;

    assert_eq!(report.status, IndexReportStatus::Completed);
    assert!(!generation_path.exists());
    Ok(())
}

#[test]
fn index_clean_defers_in_use_windows_generations() -> Result<(), Box<dyn Error>> {
    let repo = rust_repository()?;
    let cache = tempfile::tempdir()?;
    let built = build_index(&repo, cache.path())?;
    let generation = built.generation_key.as_deref().unwrap();
    let generation_path = generation_path(cache.path(), &built);
    let lock_path = cache
        .path()
        .join("v2")
        .join("repos")
        .join(&built.repository_id)
        .join("locks")
        .join(format!("{generation}.lock"));
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.lock()?;

    let output = repository_context(
        &repo,
        cache.path(),
        &[
            "index",
            "clean",
            "--execute",
            "--max-bytes",
            "1",
            "--retain-generations",
            "0",
        ],
    )?;
    let report = parse_report(&output)?;
    assert_eq!(report.status, IndexReportStatus::Partial);
    assert!(report
        .limitations
        .iter()
        .any(|limitation| limitation.code == "repository-index-clean-generation-in-use"));
    assert!(generation_path.is_file());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        drop(lock);
        let lock_path = cache
            .path()
            .join("v2")
            .join("repos")
            .join(&built.repository_id)
            .join("locks")
            .join(format!("{generation}.lock"));
        fs::remove_file(&lock_path)?;
        let sentinel = cache.path().join("lock-symlink-target");
        fs::write(&sentinel, b"keep")?;
        symlink(&sentinel, &lock_path)?;
        let output = repository_context(
            &repo,
            cache.path(),
            &[
                "index",
                "clean",
                "--execute",
                "--max-bytes",
                "1",
                "--retain-generations",
                "0",
            ],
        )?;
        let report = parse_report(&output)?;
        assert_eq!(report.status, IndexReportStatus::Partial);
        assert!(report
            .limitations
            .iter()
            .any(|limitation| limitation.code == "repository-index-clean-lock-failed"));
        assert!(generation_path.is_file());
        assert_eq!(fs::read(sentinel)?, b"keep");
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn collect_deep_revalidates_scope_after_cache_writes_and_queries() -> Result<(), Box<dyn Error>> {
    let repo = rust_repository()?;
    let cache = tempfile::tempdir()?;
    let scope = repo.scope(ReviewSource::Staged)?;
    let wrapper_root = tempfile::tempdir()?;
    let wrapper = wrapper_root.path().join("git");
    fs::write(
        &wrapper,
        b"#!/bin/sh\ncase \" $* \" in\n  *\" rev-parse HEAD \"*)\n    count=0\n    if [ -f \"$SCOPE_DRIFT_STATE\" ]; then count=$(cat \"$SCOPE_DRIFT_STATE\"); fi\n    count=$((count + 1))\n    printf '%s\\n' \"$count\" > \"$SCOPE_DRIFT_STATE\"\n    if [ \"$count\" -ge 2 ]; then printf '%040d\\n' 0; exit 0; fi\n    ;;\nesac\nexec \"$REAL_GIT\" \"$@\"\n",
    )?;
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))?;
    let original_path = std::env::var_os("PATH").ok_or("PATH is unavailable")?;
    let real_git = std::env::split_paths(&original_path)
        .map(|directory| directory.join("git"))
        .find(|candidate| candidate.is_file())
        .ok_or("git is unavailable")?;
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
            "deep",
        ])
        .current_dir(repo.path())
        .env("PATH", injected_path)
        .env("REAL_GIT", real_git)
        .env("SCOPE_DRIFT_STATE", wrapper_root.path().join("state"))
        .env("PRE_COMMIT_REVIEW_CACHE_DIR", cache.path())
        .env("PRE_COMMIT_REVIEW_SECRET_SCAN", "off")
        .output()?;

    assert_eq!(output.status.code(), Some(3));
    let context: ImpactContext = serde_json::from_slice(&output.stdout)?;
    assert_eq!(context.status, ImpactStatus::Invalidated);
    assert!(context.changed_symbols.is_empty());
    assert!(context.impact_edges.is_empty());
    assert!(
        !snapshot(cache.path()).iter().any(|(path, _, _)| {
            path.ends_with(".facts") || path.ends_with(".sqlite") || path.ends_with(".json")
        }),
        "scope-invalid collection must not leave reusable FileFacts, graph, or locator artifacts"
    );
    Ok(())
}
