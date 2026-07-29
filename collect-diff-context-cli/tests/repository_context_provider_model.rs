#[allow(dead_code)]
mod support;

use collect_diff_context_cli::candidate::snapshot::{CandidateSnapshot, SnapshotLimits};
use collect_diff_context_cli::repository_context_provider::model::{
    build_linked_project_model, ProviderModelLimits,
};
use collect_diff_context_cli::review_scope::ReviewSource;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use support::GitRepo;
use tempfile::TempDir;

fn commit_fixture(repository: &GitRepo) -> Result<(), Box<dyn Error>> {
    repository.git(["add", "--", "."])?;
    repository.git(["commit", "-qm", "fixture"])?;
    Ok(())
}

fn snapshot(repository: &GitRepo) -> Result<CandidateSnapshot, Box<dyn Error>> {
    Ok(CandidateSnapshot::materialize(
        repository.path(),
        ReviewSource::Branch,
        SnapshotLimits {
            max_files: 64,
            max_bytes: 64 * 1024,
        },
    )?)
}

#[test]
fn single_package_builds_path_sorted_roots_editions_and_local_dependencies(
) -> Result<(), Box<dyn Error>> {
    let repository = GitRepo::new()?;
    repository.write(
        "Cargo.toml",
        br#"
[package]
name = "demo-app"
edition = "2021"

[dependencies]
helper = { path = "crates/helper" }

[[bin]]
name = "tool"
path = "src/bin/tool.rs"
"#,
    )?;
    repository.write(
        "crates/helper/Cargo.toml",
        b"[package]\nname = \"helper\"\nedition = \"2018\"\n",
    )?;
    repository.write("crates/helper/src/lib.rs", b"pub fn helper() {}\n")?;
    repository.write("src/lib.rs", b"pub fn library() { helper::helper(); }\n")?;
    repository.write("src/main.rs", b"fn main() {}\n")?;
    repository.write("src/bin/tool.rs", b"fn main() {}\n")?;
    repository.write("tests/api.rs", b"#[test] fn api() {}\n")?;
    commit_fixture(&repository)?;

    let build_snapshot = snapshot(&repository)?;
    let model = build_linked_project_model(&build_snapshot, ProviderModelLimits::default())?;

    model.validate()?;
    assert_eq!(
        model
            .crates
            .iter()
            .map(|item| item.root_module.as_str())
            .collect::<Vec<_>>(),
        vec![
            "crates/helper/src/lib.rs",
            "src/bin/tool.rs",
            "src/lib.rs",
            "src/main.rs",
            "tests/api.rs",
        ]
    );
    assert!(model
        .crates
        .windows(2)
        .all(|items| items[0].crate_id < items[1].crate_id));
    assert_eq!(
        model
            .crates
            .iter()
            .find(|item| item.root_module == "crates/helper/src/lib.rs")
            .unwrap()
            .edition,
        "2018"
    );
    let app_library = model
        .crates
        .iter()
        .find(|item| item.root_module == "src/lib.rs")
        .unwrap();
    assert_eq!(app_library.edition, "2021");
    assert_eq!(app_library.dependencies.len(), 1);
    assert_eq!(app_library.dependencies[0].name, "helper");
    assert_eq!(
        app_library.dependencies[0].crate_id,
        model
            .crates
            .iter()
            .find(|item| item.root_module == "crates/helper/src/lib.rs")
            .unwrap()
            .crate_id
    );
    Ok(())
}

#[test]
fn literal_workspace_members_are_discovered_in_path_order() -> Result<(), Box<dyn Error>> {
    let repository = GitRepo::new()?;
    repository.write(
        "Cargo.toml",
        b"[workspace]\nresolver = \"2\"\nmembers = [\"crates/zeta\", \"crates/alpha\"]\n",
    )?;
    repository.write(
        "crates/zeta/Cargo.toml",
        b"[package]\nname = \"zeta\"\nedition = \"2024\"\n",
    )?;
    repository.write("crates/zeta/src/lib.rs", b"pub fn zeta() {}\n")?;
    repository.write(
        "crates/alpha/Cargo.toml",
        b"[package]\nname = \"alpha\"\nedition = \"2021\"\n",
    )?;
    repository.write("crates/alpha/src/lib.rs", b"pub fn alpha() {}\n")?;
    commit_fixture(&repository)?;

    let snapshot = snapshot(&repository)?;
    let model = build_linked_project_model(&snapshot, ProviderModelLimits::default())?;

    assert_eq!(
        model
            .crates
            .iter()
            .map(|item| item.root_module.as_str())
            .collect::<Vec<_>>(),
        vec!["crates/alpha/src/lib.rs", "crates/zeta/src/lib.rs"]
    );
    assert!(!model
        .limitations
        .iter()
        .any(|code| code.contains("workspace-member-missing")));
    Ok(())
}

#[test]
fn workspace_globs_and_inherited_fields_become_deterministic_limitations(
) -> Result<(), Box<dyn Error>> {
    let repository = GitRepo::new()?;
    repository.write(
        "Cargo.toml",
        br#"
[package]
name = "root"
edition = "2021"

[workspace]
members = ["crates/*", "crates/inherited"]
"#,
    )?;
    repository.write("src/lib.rs", b"pub fn root() {}\n")?;
    repository.write(
        "crates/inherited/Cargo.toml",
        b"[package]\nname.workspace = true\nedition.workspace = true\n",
    )?;
    repository.write("crates/inherited/src/lib.rs", b"pub fn inherited() {}\n")?;
    commit_fixture(&repository)?;

    let snapshot = snapshot(&repository)?;
    let first = build_linked_project_model(&snapshot, ProviderModelLimits::default())?;
    let repeated = build_linked_project_model(&snapshot, ProviderModelLimits::default())?;

    assert_eq!(first, repeated);
    assert!(first
        .limitations
        .iter()
        .any(|code| code.starts_with("provider-model-workspace-glob-unsupported:")));
    assert!(first
        .limitations
        .iter()
        .any(|code| code.starts_with("provider-model-workspace-inheritance-unsupported:")));
    assert!(first
        .limitations
        .windows(2)
        .all(|items| items[0] < items[1]));
    Ok(())
}

#[test]
fn malformed_and_oversized_manifests_are_bounded_limitations() -> Result<(), Box<dyn Error>> {
    let repository = GitRepo::new()?;
    repository.write(
        "Cargo.toml",
        b"[package]\nname = \"valid\"\nedition = \"2021\"\n",
    )?;
    repository.write("src/lib.rs", b"pub fn valid() {}\n")?;
    repository.write("broken/Cargo.toml", b"[package\nname = ???\n")?;
    repository.write(
        "oversized/Cargo.toml",
        format!(
            "[package]\nname = \"oversized\"\nedition = \"2021\"\n#{}\n",
            "x".repeat(256)
        ),
    )?;
    repository.write("oversized/src/lib.rs", b"pub fn oversized() {}\n")?;
    commit_fixture(&repository)?;

    let snapshot = snapshot(&repository)?;
    let model = build_linked_project_model(
        &snapshot,
        ProviderModelLimits {
            max_files: 64,
            max_bytes: 64 * 1024,
            max_file_bytes: 128,
        },
    )?;

    assert!(model
        .limitations
        .iter()
        .any(|code| code == "provider-model-manifest-invalid:broken/Cargo.toml"));
    assert!(model
        .limitations
        .iter()
        .any(|code| code == "provider-model-file-too-large:oversized/Cargo.toml"));
    assert_eq!(
        model
            .crates
            .iter()
            .map(|item| item.root_module.as_str())
            .collect::<Vec<_>>(),
        vec!["src/lib.rs"]
    );
    Ok(())
}

#[test]
fn truncated_limitations_retain_the_consumed_input_binding() -> Result<(), Box<dyn Error>> {
    let repository = GitRepo::new()?;
    let mut manifest = "[package]\nname = \"bounded\"\nedition = \"2021\"\n".to_string();
    for index in 0..1_005 {
        manifest.push_str(&format!(
            "\n[[bin]]\nname = \"missing_{index}\"\npath = \"missing/{index}.rs\"\n"
        ));
    }
    repository.write("Cargo.toml", manifest)?;
    repository.write("src/lib.rs", b"pub fn bounded() {}\n")?;
    commit_fixture(&repository)?;

    let snapshot = snapshot(&repository)?;
    let model = build_linked_project_model(&snapshot, ProviderModelLimits::default())?;

    assert!(model
        .limitations
        .iter()
        .any(|code| code == "provider-model-limitations-truncated"));
    assert!(model
        .limitations
        .iter()
        .any(|code| code.starts_with("provider-model-input-sha256:")));
    assert!(model.limitations.len() <= 1_000);
    Ok(())
}

#[test]
fn model_digest_binds_consumed_bytes_and_limit_policy() -> Result<(), Box<dyn Error>> {
    let repository = GitRepo::new()?;
    repository.write(
        "Cargo.toml",
        b"[package]\nname = \"digest\"\nedition = \"2021\"\n",
    )?;
    repository.write("src/lib.rs", b"pub fn value() -> u8 { 1 }\n")?;
    commit_fixture(&repository)?;

    let first_snapshot = snapshot(&repository)?;
    let first = build_linked_project_model(&first_snapshot, ProviderModelLimits::default())?;
    let repeated = build_linked_project_model(&first_snapshot, ProviderModelLimits::default())?;
    assert_eq!(first.digest, repeated.digest);

    let alternate_policy = build_linked_project_model(
        &first_snapshot,
        ProviderModelLimits {
            max_files: 63,
            max_bytes: 63 * 1024,
            max_file_bytes: 8 * 1024,
        },
    )?;
    assert_ne!(first.digest, alternate_policy.digest);

    repository.write("src/lib.rs", b"pub fn value() -> u8 { 2 }\n")?;
    repository.git(["add", "--", "src/lib.rs"])?;
    repository.git(["commit", "-qm", "change consumed bytes"])?;
    let changed_snapshot = snapshot(&repository)?;
    let changed = build_linked_project_model(&changed_snapshot, ProviderModelLimits::default())?;
    assert_ne!(first.digest, changed.digest);
    Ok(())
}

struct PathGuard {
    previous: Option<OsString>,
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::env::set_var("PATH", previous);
        } else {
            std::env::remove_var("PATH");
        }
    }
}

#[test]
fn repository_build_configuration_and_toolchain_processes_are_never_executed(
) -> Result<(), Box<dyn Error>> {
    let repository = GitRepo::new()?;
    let marker_root = TempDir::new()?;
    let build_marker = marker_root.path().join("build-script-called");
    let tool_marker = marker_root.path().join("toolchain-called");
    repository.write(
        "Cargo.toml",
        b"[package]\nname = \"safe\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
    )?;
    repository.write("src/lib.rs", b"pub fn safe() {}\n")?;
    write_executable(
        &repository.path().join("build.rs"),
        &marker_script(&build_marker),
    )?;
    commit_fixture(&repository)?;
    let process_snapshot = snapshot(&repository)?;

    let tools = TempDir::new()?;
    install_fake_tool(tools.path(), "cargo", &tool_marker)?;
    install_fake_tool(tools.path(), "rustc", &tool_marker)?;
    let previous = std::env::var_os("PATH");
    let mut paths = vec![tools.path().to_path_buf()];
    if let Some(previous) = previous.as_ref() {
        paths.extend(std::env::split_paths(previous));
    }
    std::env::set_var("PATH", std::env::join_paths(paths)?);
    let _guard = PathGuard { previous };

    let model = build_linked_project_model(&process_snapshot, ProviderModelLimits::default())?;
    assert!(model
        .limitations
        .iter()
        .any(|code| code == "provider-model-build-script-ignored:Cargo.toml"));
    assert!(!build_marker.exists());
    assert!(!tool_marker.exists());
    let implementation = include_str!("../src/repository_context_provider/model.rs");
    assert!(!implementation.contains("std::process"));
    assert!(!implementation.contains("Command::new"));

    let configured = GitRepo::new()?;
    configured.write(
        "Cargo.toml",
        b"[package]\nname = \"configured\"\nedition = \"2021\"\n",
    )?;
    configured.write("src/lib.rs", b"pub fn configured() {}\n")?;
    write_executable(
        &configured.path().join("rust-analyzer.toml"),
        &marker_script(&build_marker),
    )?;
    commit_fixture(&configured)?;
    let configured_snapshot = snapshot(&configured)?;
    let error = build_linked_project_model(&configured_snapshot, ProviderModelLimits::default())
        .unwrap_err();
    assert_eq!(
        error.code,
        "provider-model-repository-configuration-forbidden"
    );
    assert!(!build_marker.exists());
    assert!(!tool_marker.exists());
    Ok(())
}

fn marker_script(marker: &Path) -> String {
    #[cfg(unix)]
    {
        format!("#!/bin/sh\nprintf called > '{}'\n", marker.display())
    }
    #[cfg(windows)]
    {
        format!("@echo called>\"{}\"\r\n", marker.display())
    }
}

fn install_fake_tool(directory: &Path, name: &str, marker: &Path) -> Result<(), Box<dyn Error>> {
    #[cfg(unix)]
    let path = directory.join(name);
    #[cfg(windows)]
    let path = directory.join(format!("{name}.bat"));
    write_executable(&path, &marker_script(marker))
}

fn write_executable(path: &Path, contents: &str) -> Result<(), Box<dyn Error>> {
    fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}
