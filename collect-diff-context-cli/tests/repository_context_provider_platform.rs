#![cfg(feature = "test-fixture")]

use std::{fs, path::Path};

#[test]
fn provider_is_reachable_only_from_the_opt_in_module() {
    let app = include_str!("../src/app.rs");
    let main = include_str!("../src/main.rs");
    let index = include_str!("../src/impact_context/engine.rs");
    assert!(!app.contains("run_repository_context_provider"));
    assert!(!main.contains("run_repository_context_provider"));
    assert!(!index.contains("run_repository_context_provider"));
}

#[test]
fn ordinary_review_and_analysis_surfaces_cannot_implicitly_reach_provider() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository_root = root.parent().expect("manifest has a repository parent");
    let surfaces = [
        root.join("src/app.rs"),
        root.join("src/main.rs"),
        root.join("src/impact_context"),
        root.join("src/static_analysis"),
        root.join("src/bin/repository_context.rs"),
        repository_root.join("scripts/collect_diff_context.sh"),
        repository_root.join("scripts/collect_impact_context.sh"),
        repository_root.join("scripts/index_repository_context.sh"),
        repository_root.join("scripts/collect_static_evidence.sh"),
        repository_root.join("scripts/run_static_analysis.sh"),
        repository_root.join("scripts/orchestrate_static_analysis.sh"),
    ];
    let forbidden = [
        "run_repository_context_provider",
        "repository-context-provider-cli",
        "rust-analyzer",
        "artifacts verify",
        "artifacts provision",
        "runtime/providers",
        "provider-registry.json",
        "target-local",
    ];

    for surface in surfaces {
        let metadata = fs::metadata(&surface).expect("reachability surface exists");
        if metadata.is_dir() {
            for path in walk_files(&surface) {
                assert_surface_clean(&path, &forbidden);
            }
        } else {
            assert_surface_clean(&surface, &forbidden);
        }
    }
}

#[test]
fn runtime_surfaces_have_no_provider_fallback_commands() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository_root = root.parent().expect("manifest has a repository parent");
    let surfaces = [
        repository_root.join("install.sh"),
        repository_root.join("scripts"),
        root.join("src"),
    ];
    let forbidden = [
        "command -v rust-analyzer",
        "which rust-analyzer",
        "rustup toolchain install",
        "cargo install rust-analyzer",
        "npm install rust-analyzer",
        "brew install rust-analyzer",
        "apt-get install rust-analyzer",
        "rust-analyzer/releases/latest",
        "rust-analyzer/nightly",
        "direct-upstream",
        "direct_upstream",
        "global-registry",
        "global_registry",
    ];

    for surface in surfaces {
        let metadata = fs::metadata(&surface).expect("runtime surface exists");
        if metadata.is_dir() {
            for entry in walk_files(&surface) {
                assert_surface_clean(&entry, &forbidden);
            }
        } else {
            assert_surface_clean(&surface, &forbidden);
        }
    }
}

#[test]
fn provider_platform_paths_are_absolute_only_at_the_boundary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(root.is_absolute());
    assert!(root.join("src/repository_context_provider").is_dir());
}

fn assert_surface_clean(path: &Path, forbidden: &[&str]) {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return;
    };
    if !matches!(extension, "rs" | "sh" | "py") {
        return;
    }
    let contents = fs::read_to_string(path).expect("reachability source is UTF-8");
    for needle in forbidden {
        assert!(
            !contents.contains(needle),
            "{needle:?} unexpectedly appears in {}",
            path.display()
        );
    }
}

fn walk_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = fs::metadata(&path).expect("walk runtime surface");
        if metadata.is_file() {
            files.push(path);
            continue;
        }
        for entry in fs::read_dir(path).expect("read runtime surface") {
            pending.push(entry.expect("read runtime entry").path());
        }
    }
    files
}
