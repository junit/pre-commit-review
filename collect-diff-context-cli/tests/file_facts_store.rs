#[allow(dead_code)]
mod support;

use collect_diff_context_cli::impact_context::adapters::tree_sitter_rust::{
    RustFileFacts, TreeSitterRustAdapter,
};
use collect_diff_context_cli::impact_context::cache::file_facts::{
    file_fact_digest, CacheLayout, CacheLookup, FileFactsStore, PublishResult,
};
use collect_diff_context_cli::impact_context::index::budget::{IndexBudget, IndexBudgetTracker};
use collect_diff_context_cli::impact_context::index::model::FileFactKey;
use serde_json::Value;
use std::error::Error;
use std::sync::{Arc, Barrier};
use support::GitRepo;
use tempfile::TempDir;

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn key() -> FileFactKey {
    FileFactKey {
        language: "rust".to_string(),
        content_sha256: digest('a'),
        grammar_version: "tree-sitter-rust@0.24.2".to_string(),
        query_digest: digest('b'),
        adapter_version: "rust-index-adapter/v1".to_string(),
        normalization_rules_digest: digest('c'),
        schema_version: 1,
    }
}

fn facts() -> RustFileFacts {
    let mut budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
    TreeSitterRustAdapter::analyze_index(
        b"pub fn value() -> usize { helper() }\nfn helper() -> usize { 1 }\n",
        &mut budget,
    )
    .unwrap()
}

fn store(
    repo: &GitRepo,
    cache: &TempDir,
    maximum_object_bytes: usize,
) -> Result<FileFactsStore, Box<dyn Error>> {
    let layout = CacheLayout::resolve(repo.path(), Some(cache.path()))?;
    Ok(FileFactsStore::new(layout, maximum_object_bytes)?)
}

#[test]
fn cache_root_uses_platform_default_or_absolute_override() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"pub fn value() {}\n")?;
    let default = CacheLayout::resolve(repo.path(), None)?;
    assert!(default.root.is_absolute());
    assert_eq!(default.repository_id.len(), 64);
    assert!(default
        .facts_dir
        .ends_with(format!("v2/repos/{}/facts", default.repository_id)));

    let cache = TempDir::new()?;
    let overridden = CacheLayout::resolve(repo.path(), Some(cache.path()))?;
    assert_eq!(overridden.root, std::fs::canonicalize(cache.path())?);
    assert_eq!(default.repository_id, overridden.repository_id);
    assert!(overridden.graphs_dir.ends_with("graphs"));
    assert!(overridden.staging_dir.ends_with("staging"));
    assert!(overridden.locks_dir.ends_with("locks"));
    assert!(overridden.quarantine_dir.ends_with("quarantine"));
    Ok(())
}

#[test]
fn cache_root_rejects_relative_repository_and_git_internal_paths() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"pub fn value() {}\n")?;

    let relative = CacheLayout::resolve(repo.path(), Some(std::path::Path::new("cache")))
        .expect_err("relative cache override must be rejected");
    assert_eq!(relative.code, "cache-root-not-absolute");

    let repository_cache = repo.path().join("cache");
    let error = CacheLayout::resolve(repo.path(), Some(&repository_cache))
        .expect_err("repository-contained cache must be rejected");
    assert_eq!(error.code, "cache-root-inside-repository");
    assert!(!repository_cache.exists());

    let git_cache = repo.path().join(".git/cache");
    let error = CacheLayout::resolve(repo.path(), Some(&git_cache))
        .expect_err("Git-internal cache must be rejected");
    assert_eq!(error.code, "cache-root-inside-git-directory");
    assert!(!git_cache.exists());
    Ok(())
}

#[test]
fn file_facts_key_changes_for_content_grammar_query_adapter_and_schema() {
    let baseline = key();
    let baseline_digest = file_fact_digest(&baseline).unwrap();
    let mut mutations = Vec::new();

    let mut changed = baseline.clone();
    changed.content_sha256 = digest('d');
    mutations.push(changed);
    let mut changed = baseline.clone();
    changed.grammar_version.push_str("-next");
    mutations.push(changed);
    let mut changed = baseline.clone();
    changed.query_digest = digest('e');
    mutations.push(changed);
    let mut changed = baseline.clone();
    changed.adapter_version.push_str("-next");
    mutations.push(changed);
    let mut changed = baseline.clone();
    changed.normalization_rules_digest = digest('f');
    mutations.push(changed);
    let mut changed = baseline.clone();
    changed.schema_version = 2;
    mutations.push(changed);

    for mutation in mutations {
        assert_ne!(file_fact_digest(&mutation).unwrap(), baseline_digest);
    }
}

#[test]
fn write_then_read_validates_envelope_and_payload_digest() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"pub fn value() {}\n")?;
    let cache = TempDir::new()?;
    let nested_root = cache.path().join("nested/cache");
    let layout = CacheLayout::resolve(repo.path(), Some(&nested_root))?;
    let store = FileFactsStore::new(layout, 16 * 1024 * 1024)?;
    let key = key();
    let facts = facts();

    assert_eq!(store.publish(&key, &facts)?, PublishResult::Published);
    assert_eq!(store.lookup(&key)?, CacheLookup::Hit(facts.clone()));

    let bytes = std::fs::read(store.object_path(&key)?)?;
    let envelope: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(envelope["magic"], "pre-commit-review-file-facts");
    assert_eq!(envelope["schema_version"], 1);
    assert_eq!(
        envelope["payload_length"],
        serde_json::to_vec(&facts)?.len()
    );
    assert_eq!(envelope["payload_sha256"].as_str().unwrap().len(), 64);
    Ok(())
}

#[test]
fn identical_content_reuses_one_object_across_paths() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"pub fn value() {}\n")?;
    let cache = TempDir::new()?;
    let store = store(&repo, &cache, 16 * 1024 * 1024)?;
    let key = key();
    let facts = facts();

    assert_eq!(store.publish(&key, &facts)?, PublishResult::Published);
    assert_eq!(store.publish(&key, &facts)?, PublishResult::Reused);
    let object = store.object_path(&key)?;
    assert!(object.exists());
    assert_eq!(
        std::fs::read_dir(object.parent().unwrap())?.count(),
        1,
        "the content key must publish exactly one immutable object"
    );
    Ok(())
}

#[test]
fn truncated_oversized_unknown_schema_and_checksum_mismatch_are_corrupt_misses(
) -> Result<(), Box<dyn Error>> {
    for corruption in ["truncated", "oversized", "schema", "checksum"] {
        let repo = GitRepo::new()?;
        repo.commit_file("src/lib.rs", b"pub fn value() {}\n")?;
        let cache = TempDir::new()?;
        let writer = store(&repo, &cache, 16 * 1024 * 1024)?;
        let key = key();
        let facts = facts();
        writer.publish(&key, &facts)?;
        let path = writer.object_path(&key)?;

        let reader = if corruption == "oversized" {
            store(&repo, &cache, 64)?
        } else {
            let mut bytes = std::fs::read(&path)?;
            if corruption == "truncated" {
                bytes.truncate(bytes.len() / 2);
            } else {
                let mut envelope: Value = serde_json::from_slice(&bytes)?;
                if corruption == "schema" {
                    envelope["schema_version"] = 99.into();
                } else {
                    envelope["payload_sha256"] = digest('0').into();
                }
                bytes = serde_json::to_vec(&envelope)?;
            }
            std::fs::write(&path, bytes)?;
            store(&repo, &cache, 16 * 1024 * 1024)?
        };

        assert!(matches!(reader.lookup(&key)?, CacheLookup::Corrupt { .. }));
    }
    Ok(())
}

#[test]
fn concurrent_same_key_writers_converge_without_overwrite() -> Result<(), Box<dyn Error>> {
    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"pub fn value() {}\n")?;
    let cache = TempDir::new()?;
    let store = Arc::new(store(&repo, &cache, 16 * 1024 * 1024)?);
    let key = Arc::new(key());
    let facts = Arc::new(facts());
    let barrier = Arc::new(Barrier::new(8));
    let mut writers = Vec::new();
    for _ in 0..8 {
        let store = Arc::clone(&store);
        let key = Arc::clone(&key);
        let facts = Arc::clone(&facts);
        let barrier = Arc::clone(&barrier);
        writers.push(std::thread::spawn(move || {
            barrier.wait();
            store.publish(&key, &facts)
        }));
    }

    let mut published = 0;
    let mut reused = 0;
    for writer in writers {
        match writer.join().unwrap()? {
            PublishResult::Published => published += 1,
            PublishResult::Reused => reused += 1,
        }
    }
    assert_eq!(published, 1);
    assert_eq!(reused, 7);
    assert!(matches!(store.lookup(&key)?, CacheLookup::Hit(_)));
    Ok(())
}

#[cfg(unix)]
#[test]
fn unix_cache_permissions_are_private() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;

    let repo = GitRepo::new()?;
    repo.commit_file("src/lib.rs", b"pub fn value() {}\n")?;
    let cache = TempDir::new()?;
    let store = store(&repo, &cache, 16 * 1024 * 1024)?;
    let key = key();
    store.publish(&key, &facts())?;

    let object = store.object_path(&key)?;
    assert_eq!(
        std::fs::metadata(store.layout().facts_dir.clone())?
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(object)?.permissions().mode() & 0o777,
        0o600
    );
    Ok(())
}
