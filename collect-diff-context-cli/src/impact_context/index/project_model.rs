use crate::candidate::{CandidateBytes, CandidateError, CandidatePresence, RepoPath};
use crate::impact_context::contracts::{Completeness, UnitStatus};
use crate::impact_context::index::budget::{IndexBudgetTracker, IndexResource};
use crate::impact_context::index::manifest::RepositoryManifestSource;
use crate::impact_context::index::model::RepositoryManifest;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const PROJECT_MODEL_POLICY: &str = "passive-cargo-project-model/v1";
const TOML_PARSER_ID: &str = "toml@1.1.3+spec-1.1.0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustProjectModel {
    pub digest: String,
    pub packages: Vec<RustPackageModel>,
    pub roots: Vec<RustTargetRoot>,
    pub consumed_files: Vec<ProjectModelFile>,
    pub completeness: Completeness,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustPackageModel {
    pub package_name: String,
    pub manifest_path: RepoPath,
    pub package_root: RepoPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustTargetRoot {
    pub package_name: String,
    pub kind: String,
    pub source_path: RepoPath,
    pub crate_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectModelFile {
    pub path: RepoPath,
    pub content_sha256: Option<String>,
    pub content_bytes: Option<usize>,
    pub status: UnitStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectModelError {
    pub code: &'static str,
    pub message: String,
}

impl ProjectModelError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProjectModelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProjectModelError {}

pub trait ProjectModelSource {
    fn read_bounded(
        &self,
        path: &RepoPath,
        maximum_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError>;
}

impl<T> ProjectModelSource for T
where
    T: RepositoryManifestSource + ?Sized,
{
    fn read_bounded(
        &self,
        path: &RepoPath,
        maximum_bytes: usize,
    ) -> Result<CandidateBytes, CandidateError> {
        RepositoryManifestSource::read_bounded(self, path, maximum_bytes)
    }
}

#[derive(Debug, Default, Deserialize)]
struct CargoManifest {
    package: Option<CargoPackage>,
    lib: Option<CargoTarget>,
    #[serde(default, rename = "bin")]
    bins: Vec<CargoTarget>,
    workspace: Option<CargoWorkspace>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoPackage {
    name: Option<toml::Value>,
    build: Option<toml::Value>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoTarget {
    name: Option<toml::Value>,
    path: Option<toml::Value>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoWorkspace {
    members: Option<Vec<toml::Value>>,
}

struct ParsedManifest {
    path: RepoPath,
    root: RepoPath,
    bytes: Vec<u8>,
    manifest: CargoManifest,
}

pub fn build_rust_project_model<T: ProjectModelSource + ?Sized>(
    source: &T,
    repository_manifest: &RepositoryManifest,
    budget: &mut IndexBudgetTracker,
) -> Result<RustProjectModel, ProjectModelError> {
    repository_manifest.validate().map_err(|error| {
        ProjectModelError::new(
            "project-model-manifest-invalid",
            format!("repository manifest is invalid: {error}"),
        )
    })?;
    let mut limitations = Vec::new();
    if repository_manifest.completeness != Completeness::Complete {
        push_limitation(&mut limitations, "project-model-candidate-manifest-partial");
    }
    if let Err(exhaustion) = budget.check_deadline() {
        push_limitation(&mut limitations, exhaustion.code());
        return Ok(empty_model(repository_manifest, limitations));
    }

    let manifest_entries = repository_manifest
        .entries
        .iter()
        .filter(|entry| {
            entry.presence == CandidatePresence::Present
                && entry.status == UnitStatus::Completed
                && is_cargo_manifest(&entry.path)
        })
        .collect::<Vec<_>>();
    let candidate_paths = repository_manifest
        .entries
        .iter()
        .filter(|entry| {
            entry.presence == CandidatePresence::Present && entry.status == UnitStatus::Completed
        })
        .map(|entry| entry.path.clone())
        .collect::<BTreeSet<_>>();
    let manifest_paths = manifest_entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<BTreeSet<_>>();

    let mut parsed = Vec::new();
    let mut consumed_files = Vec::new();
    let mut digest_inputs = Vec::new();
    for entry in manifest_entries {
        if let Err(exhaustion) = budget.check_deadline() {
            push_limitation(&mut limitations, exhaustion.code());
            break;
        }
        if let Err(exhaustion) = budget.consume(IndexResource::ProjectModelFiles, 1) {
            push_limitation(&mut limitations, exhaustion.code());
            break;
        }
        let declared_bytes = entry.content_bytes.unwrap_or(0);
        if let Err(exhaustion) = budget.consume(IndexResource::ProjectModelBytes, declared_bytes) {
            consumed_files.push(ProjectModelFile {
                path: entry.path.clone(),
                content_sha256: None,
                content_bytes: None,
                status: UnitStatus::BudgetExhausted,
            });
            push_path_limitation(&mut limitations, exhaustion.code(), &entry.path);
            break;
        }
        let maximum_bytes = budget
            .budget()
            .max_file_bytes
            .min(budget.budget().max_project_model_bytes);
        let content = match source.read_bounded(&entry.path, maximum_bytes) {
            Ok(content) => content,
            Err(error) => {
                consumed_files.push(ProjectModelFile {
                    path: entry.path.clone(),
                    content_sha256: None,
                    content_bytes: None,
                    status: UnitStatus::Unavailable,
                });
                push_path_limitation(
                    &mut limitations,
                    "project-model-manifest-unavailable",
                    &entry.path,
                );
                digest_inputs.push((entry.path.clone(), error.to_string().into_bytes()));
                continue;
            }
        };
        if entry.content_sha256.as_deref() != Some(content.sha256.as_str())
            || entry.content_bytes != Some(content.bytes.len())
        {
            consumed_files.push(ProjectModelFile {
                path: entry.path.clone(),
                content_sha256: None,
                content_bytes: None,
                status: UnitStatus::Unavailable,
            });
            push_path_limitation(
                &mut limitations,
                "project-model-manifest-identity-mismatch",
                &entry.path,
            );
            continue;
        }
        consumed_files.push(ProjectModelFile {
            path: entry.path.clone(),
            content_sha256: Some(content.sha256.clone()),
            content_bytes: Some(content.bytes.len()),
            status: UnitStatus::Completed,
        });
        digest_inputs.push((entry.path.clone(), content.bytes.clone()));
        let text = match std::str::from_utf8(&content.bytes) {
            Ok(text) => text,
            Err(_) => {
                push_path_limitation(
                    &mut limitations,
                    "project-model-manifest-invalid-utf8",
                    &entry.path,
                );
                continue;
            }
        };
        let manifest = match toml::from_str::<CargoManifest>(text) {
            Ok(manifest) => manifest,
            Err(_) => {
                push_path_limitation(
                    &mut limitations,
                    "project-model-manifest-invalid",
                    &entry.path,
                );
                continue;
            }
        };
        parsed.push(ParsedManifest {
            root: manifest_root(&entry.path)?,
            path: entry.path.clone(),
            bytes: content.bytes,
            manifest,
        });
    }

    if parsed.is_empty() && limitations.is_empty() {
        push_limitation(&mut limitations, "project-model-manifest-unavailable");
    }
    record_workspace_limitations(&parsed, &manifest_paths, &mut limitations);

    let mut packages = Vec::new();
    let mut roots = Vec::new();
    for parsed_manifest in &parsed {
        let Some(package) = parsed_manifest.manifest.package.as_ref() else {
            continue;
        };
        if package.build.as_ref().is_some_and(build_script_enabled) {
            push_path_limitation(
                &mut limitations,
                "project-model-build-script-ignored",
                &parsed_manifest.path,
            );
        }
        let Some(package_name) = string_field(
            package.name.as_ref(),
            "project-model-workspace-inheritance-unsupported",
            &parsed_manifest.path,
            &mut limitations,
        ) else {
            continue;
        };
        let package_model = RustPackageModel {
            package_name: package_name.clone(),
            manifest_path: parsed_manifest.path.clone(),
            package_root: parsed_manifest.root.clone(),
        };
        add_package_roots(
            &package_model,
            &parsed_manifest.manifest,
            &candidate_paths,
            &mut roots,
            &mut limitations,
        );
        packages.push(package_model);
    }

    packages.sort_by(|left, right| left.manifest_path.cmp(&right.manifest_path));
    roots.sort_by(|left, right| {
        left.source_path
            .cmp(&right.source_path)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.package_name.cmp(&right.package_name))
            .then_with(|| left.crate_name.cmp(&right.crate_name))
    });
    roots.dedup_by(|left, right| {
        left.source_path == right.source_path
            && left.kind == right.kind
            && left.package_name == right.package_name
            && left.crate_name == right.crate_name
    });
    consumed_files.sort_by(|left, right| left.path.cmp(&right.path));
    limitations.sort();
    limitations.dedup();
    let digest = project_model_digest(&digest_inputs, &limitations);
    let completeness = if limitations.is_empty() {
        Completeness::Complete
    } else {
        Completeness::Partial
    };
    let _consumed_manifest_bytes = parsed.iter().fold(0_usize, |total, manifest| {
        total.saturating_add(manifest.bytes.len())
    });
    Ok(RustProjectModel {
        digest,
        packages,
        roots,
        consumed_files,
        completeness,
        limitations,
    })
}

fn add_package_roots(
    package: &RustPackageModel,
    manifest: &CargoManifest,
    candidate_paths: &BTreeSet<RepoPath>,
    roots: &mut Vec<RustTargetRoot>,
    limitations: &mut Vec<String>,
) {
    let default_crate_name = crate_name(&package.package_name);
    let explicit_lib_path = manifest
        .lib
        .as_ref()
        .and_then(|target| target.path.as_ref());
    if let Some(lib) = &manifest.lib {
        let crate_name = string_field(
            lib.name.as_ref(),
            "project-model-target-field-unsupported",
            &package.manifest_path,
            limitations,
        )
        .unwrap_or_else(|| default_crate_name.clone());
        if let Some(path) = string_field(
            lib.path.as_ref(),
            "project-model-target-field-unsupported",
            &package.manifest_path,
            limitations,
        ) {
            add_explicit_root(
                package,
                "lib",
                &path,
                &crate_name,
                candidate_paths,
                roots,
                limitations,
            );
        } else if explicit_lib_path.is_none() {
            add_conventional_root(
                package,
                "lib",
                "src/lib.rs",
                &crate_name,
                candidate_paths,
                roots,
            );
        }
    } else {
        add_conventional_root(
            package,
            "lib",
            "src/lib.rs",
            &default_crate_name,
            candidate_paths,
            roots,
        );
    }

    if manifest.bins.is_empty() {
        add_conventional_root(
            package,
            "bin",
            "src/main.rs",
            &default_crate_name,
            candidate_paths,
            roots,
        );
        add_discovered_roots(package, "src/bin/", "bin", candidate_paths, roots);
    } else {
        for bin in &manifest.bins {
            let name = string_field(
                bin.name.as_ref(),
                "project-model-target-field-unsupported",
                &package.manifest_path,
                limitations,
            );
            let path = string_field(
                bin.path.as_ref(),
                "project-model-target-field-unsupported",
                &package.manifest_path,
                limitations,
            )
            .or_else(|| name.as_ref().map(|name| format!("src/bin/{name}.rs")));
            let Some(path) = path else {
                push_path_limitation(
                    limitations,
                    "project-model-target-field-unsupported",
                    &package.manifest_path,
                );
                continue;
            };
            let crate_name = name
                .map(|name| crate_name(&name))
                .unwrap_or_else(|| crate_name_from_path(&path));
            add_explicit_root(
                package,
                "bin",
                &path,
                &crate_name,
                candidate_paths,
                roots,
                limitations,
            );
        }
    }
    add_discovered_roots(package, "tests/", "test", candidate_paths, roots);
}

fn add_conventional_root(
    package: &RustPackageModel,
    kind: &str,
    relative_path: &str,
    crate_name: &str,
    candidate_paths: &BTreeSet<RepoPath>,
    roots: &mut Vec<RustTargetRoot>,
) {
    if let Ok(path) = join_package_path(&package.package_root, relative_path) {
        if candidate_paths.contains(&path) {
            roots.push(RustTargetRoot {
                package_name: package.package_name.clone(),
                kind: kind.to_string(),
                source_path: path,
                crate_name: crate_name.to_string(),
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn add_explicit_root(
    package: &RustPackageModel,
    kind: &str,
    relative_path: &str,
    crate_name: &str,
    candidate_paths: &BTreeSet<RepoPath>,
    roots: &mut Vec<RustTargetRoot>,
    limitations: &mut Vec<String>,
) {
    let path = match join_package_path(&package.package_root, relative_path) {
        Ok(path) => path,
        Err(_) => {
            push_path_limitation(
                limitations,
                "project-model-target-path-unsupported",
                &package.manifest_path,
            );
            return;
        }
    };
    if !candidate_paths.contains(&path) {
        push_path_limitation(limitations, "project-model-target-missing", &path);
        return;
    }
    roots.push(RustTargetRoot {
        package_name: package.package_name.clone(),
        kind: kind.to_string(),
        source_path: path,
        crate_name: crate_name.to_string(),
    });
}

fn add_discovered_roots(
    package: &RustPackageModel,
    relative_prefix: &str,
    kind: &str,
    candidate_paths: &BTreeSet<RepoPath>,
    roots: &mut Vec<RustTargetRoot>,
) {
    let prefix = if package.package_root.as_str() == "." {
        relative_prefix.to_string()
    } else {
        format!("{}/{relative_prefix}", package.package_root.as_str())
    };
    for path in candidate_paths {
        let Some(relative) = path.as_str().strip_prefix(&prefix) else {
            continue;
        };
        if relative.is_empty() || relative.contains('/') || !relative.ends_with(".rs") {
            continue;
        }
        roots.push(RustTargetRoot {
            package_name: package.package_name.clone(),
            kind: kind.to_string(),
            source_path: path.clone(),
            crate_name: crate_name_from_path(relative),
        });
    }
}

fn record_workspace_limitations(
    parsed: &[ParsedManifest],
    manifest_paths: &BTreeSet<RepoPath>,
    limitations: &mut Vec<String>,
) {
    for manifest in parsed {
        let Some(workspace) = &manifest.manifest.workspace else {
            continue;
        };
        let Some(members) = &workspace.members else {
            continue;
        };
        for member in members {
            let Some(member) = member.as_str() else {
                push_path_limitation(
                    limitations,
                    "project-model-workspace-member-unsupported",
                    &manifest.path,
                );
                continue;
            };
            if member.contains(['*', '?', '[', ']']) {
                push_path_limitation(
                    limitations,
                    "project-model-workspace-glob-unsupported",
                    &manifest.path,
                );
                continue;
            }
            let Ok(member_root) = join_package_path(&manifest.root, member) else {
                push_path_limitation(
                    limitations,
                    "project-model-workspace-member-unsupported",
                    &manifest.path,
                );
                continue;
            };
            let Ok(member_manifest) = join_package_path(&member_root, "Cargo.toml") else {
                continue;
            };
            if !manifest_paths.contains(&member_manifest) {
                push_path_limitation(
                    limitations,
                    "project-model-workspace-member-missing",
                    &member_manifest,
                );
            }
        }
    }
}

fn string_field(
    value: Option<&toml::Value>,
    unsupported_code: &str,
    manifest_path: &RepoPath,
    limitations: &mut Vec<String>,
) -> Option<String> {
    match value {
        Some(value) if value.is_str() => value.as_str().map(str::to_string),
        Some(value)
            if value
                .as_table()
                .and_then(|table| table.get("workspace"))
                .and_then(toml::Value::as_bool)
                == Some(true) =>
        {
            push_path_limitation(limitations, unsupported_code, manifest_path);
            None
        }
        Some(_) => {
            push_path_limitation(limitations, unsupported_code, manifest_path);
            None
        }
        None => None,
    }
}

fn build_script_enabled(value: &toml::Value) -> bool {
    value.as_bool() != Some(false)
}

fn manifest_root(path: &RepoPath) -> Result<RepoPath, ProjectModelError> {
    let Some((root, _)) = path.as_str().rsplit_once('/') else {
        return RepoPath::new(".").map_err(|error| {
            ProjectModelError::new("project-model-path-invalid", error.to_string())
        });
    };
    RepoPath::new(root)
        .map_err(|error| ProjectModelError::new("project-model-path-invalid", error.to_string()))
}

fn join_package_path(root: &RepoPath, relative: &str) -> Result<RepoPath, CandidateError> {
    let value = if root.as_str() == "." {
        relative.to_string()
    } else {
        format!("{}/{relative}", root.as_str())
    };
    RepoPath::new(value)
}

fn crate_name(package_name: &str) -> String {
    package_name.replace('-', "_")
}

fn crate_name_from_path(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .strip_suffix(".rs")
        .unwrap_or(path)
        .replace('-', "_")
}

fn is_cargo_manifest(path: &RepoPath) -> bool {
    path.as_str() == "Cargo.toml" || path.as_str().ends_with("/Cargo.toml")
}

fn project_model_digest(inputs: &[(RepoPath, Vec<u8>)], limitations: &[String]) -> String {
    let mut inputs = inputs.iter().collect::<Vec<_>>();
    inputs.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    hash_component(&mut digest, b"rust-project-model/v1");
    hash_component(&mut digest, PROJECT_MODEL_POLICY.as_bytes());
    hash_component(&mut digest, TOML_PARSER_ID.as_bytes());
    for (path, bytes) in inputs {
        hash_component(&mut digest, path.as_str().as_bytes());
        hash_component(&mut digest, bytes);
    }
    for limitation in limitations {
        hash_component(&mut digest, limitation.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn empty_model(
    repository_manifest: &RepositoryManifest,
    mut limitations: Vec<String>,
) -> RustProjectModel {
    limitations.sort();
    limitations.dedup();
    RustProjectModel {
        digest: project_model_digest(&[], &limitations),
        packages: Vec::new(),
        roots: Vec::new(),
        consumed_files: repository_manifest
            .entries
            .iter()
            .filter(|entry| is_cargo_manifest(&entry.path))
            .map(|entry| ProjectModelFile {
                path: entry.path.clone(),
                content_sha256: None,
                content_bytes: None,
                status: UnitStatus::BudgetExhausted,
            })
            .collect(),
        completeness: Completeness::Partial,
        limitations,
    }
}

fn push_limitation(limitations: &mut Vec<String>, code: &str) {
    limitations.push(code.to_string());
}

fn push_path_limitation(limitations: &mut Vec<String>, code: &str, path: &RepoPath) {
    limitations.push(format!("{code}:{}", path.as_str()));
}

fn hash_component(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}
