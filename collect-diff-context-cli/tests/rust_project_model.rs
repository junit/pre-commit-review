use collect_diff_context_cli::candidate::{
    CandidateBytes, CandidateError, CandidatePresence, RepoPath,
};
use collect_diff_context_cli::impact_context::contracts::{Completeness, UnitStatus};
use collect_diff_context_cli::impact_context::index::budget::{IndexBudget, IndexBudgetTracker};
use collect_diff_context_cli::impact_context::index::model::{
    RepositoryLocator, RepositoryManifest, RepositoryManifestEntry,
};
use collect_diff_context_cli::impact_context::index::project_model::{
    build_rust_project_model, ProjectModelSource, RustProjectModel,
};
use collect_diff_context_cli::review_scope::ReviewSource;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::OsString;
use std::path::Path;
use tempfile::TempDir;

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

#[derive(Default)]
struct MemoryProjectSource {
    files: BTreeMap<RepoPath, Vec<u8>>,
}

impl MemoryProjectSource {
    fn insert(&mut self, path: &str, bytes: impl AsRef<[u8]>) {
        self.files
            .insert(RepoPath::new(path).unwrap(), bytes.as_ref().to_vec());
    }
}

impl ProjectModelSource for MemoryProjectSource {
    fn read_bounded(
        &self,
        path: &RepoPath,
        maximum_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError> {
        let bytes = self
            .files
            .get(path)
            .ok_or_else(|| RepoPath::new("").unwrap_err())?;
        if bytes.len() > maximum_bytes {
            return Err(CandidateError::byte_limit_exceeded(path, maximum_bytes));
        }
        Ok(CandidateBytes {
            sha256: format!("{:x}", Sha256::digest(bytes)),
            binary: bytes.iter().take(8192).any(|byte| *byte == 0),
            bytes: bytes.clone(),
        })
    }
}

fn manifest(source: &MemoryProjectSource) -> RepositoryManifest {
    let entries = source
        .files
        .iter()
        .map(|(path, bytes)| RepositoryManifestEntry {
            path: path.clone(),
            mode: "100644".to_string(),
            presence: CandidatePresence::Present,
            content_sha256: Some(format!("{:x}", Sha256::digest(bytes))),
            content_bytes: Some(bytes.len()),
            language: path
                .as_str()
                .ends_with(".rs")
                .then(|| "rust".to_string())
                .or_else(|| path.as_str().ends_with(".toml").then(|| "toml".to_string())),
            status: UnitStatus::Completed,
            limitation_codes: Vec::new(),
        })
        .collect();
    RepositoryManifest {
        locator: RepositoryLocator {
            source: ReviewSource::Staged,
            object_format: "sha1".to_string(),
            base_tree: Some(std::iter::repeat_n('1', 40).collect()),
            index_manifest_digest: Some(digest('2')),
            overlay_candidate_digest: digest('3'),
        },
        digest: digest('4'),
        entries,
        completeness: Completeness::Complete,
        limitations: Vec::new(),
    }
}

fn build(source: &MemoryProjectSource) -> RustProjectModel {
    let mut budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
    build_rust_project_model(source, &manifest(source), &mut budget).unwrap()
}

#[test]
fn single_package_discovers_conventional_lib_main_bin_and_test_roots() {
    let mut source = MemoryProjectSource::default();
    source.insert("Cargo.toml", b"[package]\nname = \"demo-app\"\n");
    source.insert("src/lib.rs", b"pub fn lib() {}\n");
    source.insert("src/main.rs", b"fn main() {}\n");
    source.insert("src/bin/admin.rs", b"fn main() {}\n");
    source.insert("tests/auth.rs", b"#[test] fn auth() {}\n");

    let model = build(&source);

    assert_eq!(model.completeness, Completeness::Complete);
    assert_eq!(model.packages.len(), 1);
    assert_eq!(model.packages[0].package_name, "demo-app");
    assert_eq!(model.packages[0].manifest_path.as_str(), "Cargo.toml");
    assert_eq!(model.packages[0].package_root.as_str(), ".");
    assert_eq!(
        model
            .roots
            .iter()
            .map(|root| (
                root.kind.as_str(),
                root.source_path.as_str(),
                root.crate_name.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("bin", "src/bin/admin.rs", "admin"),
            ("lib", "src/lib.rs", "demo_app"),
            ("bin", "src/main.rs", "demo_app"),
            ("test", "tests/auth.rs", "auth"),
        ]
    );
}

#[test]
fn explicit_lib_and_bin_paths_override_conventional_roots() {
    let mut source = MemoryProjectSource::default();
    source.insert(
        "Cargo.toml",
        br#"
[package]
name = "demo"

[lib]
name = "core_api"
path = "custom/core.rs"

[[bin]]
name = "runner"
path = "cmd/run.rs"
"#,
    );
    source.insert("custom/core.rs", b"pub fn core() {}\n");
    source.insert("cmd/run.rs", b"fn main() {}\n");
    source.insert("src/lib.rs", b"pub fn ignored() {}\n");
    source.insert("src/main.rs", b"fn main() {}\n");
    source.insert("src/bin/ignored.rs", b"fn main() {}\n");

    let model = build(&source);
    assert_eq!(
        model
            .roots
            .iter()
            .map(|root| (
                root.kind.as_str(),
                root.source_path.as_str(),
                root.crate_name.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("bin", "cmd/run.rs", "runner"),
            ("lib", "custom/core.rs", "core_api"),
        ]
    );
}

#[test]
fn literal_workspace_members_are_path_sorted() {
    let mut source = MemoryProjectSource::default();
    source.insert(
        "Cargo.toml",
        b"[workspace]\nmembers = [\"crates/zeta\", \"crates/alpha\"]\n",
    );
    source.insert("crates/zeta/Cargo.toml", b"[package]\nname = \"zeta\"\n");
    source.insert("crates/zeta/src/lib.rs", b"pub fn zeta() {}\n");
    source.insert("crates/alpha/Cargo.toml", b"[package]\nname = \"alpha\"\n");
    source.insert("crates/alpha/src/lib.rs", b"pub fn alpha() {}\n");

    let model = build(&source);
    assert_eq!(
        model
            .packages
            .iter()
            .map(|package| package.manifest_path.as_str())
            .collect::<Vec<_>>(),
        vec!["crates/alpha/Cargo.toml", "crates/zeta/Cargo.toml"]
    );
    assert_eq!(
        model
            .roots
            .iter()
            .map(|root| root.source_path.as_str())
            .collect::<Vec<_>>(),
        vec!["crates/alpha/src/lib.rs", "crates/zeta/src/lib.rs"]
    );
}

#[test]
fn workspace_globs_and_inherited_fields_are_partial_not_executed() {
    let mut source = MemoryProjectSource::default();
    source.insert("Cargo.toml", b"[workspace]\nmembers = [\"crates/*\"]\n");
    source.insert(
        "crates/member/Cargo.toml",
        b"[package]\nname.workspace = true\n",
    );
    source.insert("crates/member/src/lib.rs", b"pub fn member() {}\n");

    let model = build(&source);
    assert_eq!(model.completeness, Completeness::Partial);
    assert!(model
        .limitations
        .iter()
        .any(|code| code.contains("workspace-glob-unsupported")));
    assert!(model
        .limitations
        .iter()
        .any(|code| code.contains("workspace-inheritance-unsupported")));
}

#[test]
fn malformed_and_oversized_manifests_are_bounded_limitations() {
    let mut malformed = MemoryProjectSource::default();
    malformed.insert("Cargo.toml", b"[package\nname = ???\n");
    let malformed_model = build(&malformed);
    assert_eq!(malformed_model.completeness, Completeness::Partial);
    assert!(malformed_model
        .limitations
        .iter()
        .any(|code| code.contains("manifest-invalid")));

    let mut oversized = MemoryProjectSource::default();
    oversized.insert("Cargo.toml", b"[package]\nname = \"oversized\"\n");
    let mut limits = IndexBudget::deep_defaults();
    limits.max_project_model_bytes = 8;
    let mut budget = IndexBudgetTracker::new(limits);
    let oversized_model =
        build_rust_project_model(&oversized, &manifest(&oversized), &mut budget).unwrap();
    assert_eq!(oversized_model.completeness, Completeness::Partial);
    assert!(oversized_model
        .limitations
        .iter()
        .any(|code| code.contains("project-model-byte-budget-exhausted")));
}

#[test]
fn project_model_digest_binds_exact_consumed_manifest_bytes_and_policy() {
    let mut first = MemoryProjectSource::default();
    first.insert("Cargo.toml", b"[package]\nname = \"first\"\n");
    first.insert("src/lib.rs", b"pub fn value() {}\n");
    let first_model = build(&first);
    let repeated = build(&first);
    assert_eq!(first_model.digest, repeated.digest);

    let mut second = MemoryProjectSource::default();
    second.insert("Cargo.toml", b"[package]\nname = \"second\"\n");
    second.insert("src/lib.rs", b"pub fn value() {}\n");
    let second_model = build(&second);
    assert_ne!(first_model.digest, second_model.digest);
    assert_ne!(
        first_model.digest,
        format!(
            "{:x}",
            Sha256::digest(first.files[&RepoPath::new("Cargo.toml").unwrap()].as_slice())
        )
    );
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
fn project_model_never_invokes_cargo_or_repository_commands() -> Result<(), Box<dyn Error>> {
    let tools = TempDir::new()?;
    let marker = tools.path().join("cargo-called");
    install_fake_cargo(tools.path(), &marker)?;
    let previous = std::env::var_os("PATH");
    let mut paths = vec![tools.path().to_path_buf()];
    if let Some(previous) = previous.as_ref() {
        paths.extend(std::env::split_paths(previous));
    }
    std::env::set_var("PATH", std::env::join_paths(paths)?);
    let _guard = PathGuard { previous };

    let mut source = MemoryProjectSource::default();
    source.insert("Cargo.toml", b"[package]\nname = \"safe\"\n");
    source.insert("src/lib.rs", b"pub fn safe() {}\n");
    let model = build(&source);

    assert_eq!(model.completeness, Completeness::Complete);
    assert!(!marker.exists(), "passive project parsing invoked cargo");
    Ok(())
}

#[cfg(unix)]
fn install_fake_cargo(directory: &Path, marker: &Path) -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;
    let executable = directory.join("cargo");
    std::fs::write(
        &executable,
        format!(
            "#!/bin/sh\nprintf called > '{}'\nexit 99\n",
            marker.display()
        ),
    )?;
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(windows)]
fn install_fake_cargo(directory: &Path, marker: &Path) -> Result<(), Box<dyn Error>> {
    std::fs::write(
        directory.join("cargo.bat"),
        format!("@echo called>\"{}\"\r\n@exit /b 99\r\n", marker.display()),
    )?;
    Ok(())
}
