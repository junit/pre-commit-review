#![cfg(feature = "test-fixture")]

use std::path::Path;

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
fn provider_platform_paths_are_absolute_only_at_the_boundary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(root.is_absolute());
    assert!(root.join("src/repository_context_provider").is_dir());
}
