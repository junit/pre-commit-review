use crate::candidate::snapshot::CandidateSnapshot;
use crate::repository_context_provider::contract::{
    RustAnalyzerCrate, RustAnalyzerDependency, RustAnalyzerProjectModel, MAX_NODES,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const MODEL_POLICY: &str = "passive-cargo-linked-project/v1";
const TOML_PARSER: &str = "toml@1.1.3+spec-1.1.0";
const MAX_RETAINED_LIMITATIONS: usize = 998;
const MAX_RELATIVE_PATH_BYTES: usize = 3_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderModelLimits {
    pub max_files: usize,
    pub max_bytes: usize,
    pub max_file_bytes: usize,
}

impl Default for ProviderModelLimits {
    fn default() -> Self {
        Self {
            max_files: 1_000,
            max_bytes: 8 * 1024 * 1024,
            max_file_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelBuildError {
    pub code: &'static str,
    message: String,
}

impl ModelBuildError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl std::fmt::Display for ModelBuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ModelBuildError {}

#[derive(Debug)]
struct SnapshotFile {
    absolute_path: PathBuf,
    bytes: usize,
}

#[derive(Debug, Default, Deserialize)]
struct CargoManifest {
    package: Option<CargoPackage>,
    workspace: Option<CargoWorkspace>,
    lib: Option<CargoTarget>,
    #[serde(default, rename = "bin")]
    bins: Vec<CargoTarget>,
    #[serde(default, rename = "test")]
    tests: Vec<CargoTarget>,
    #[serde(default)]
    dependencies: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoPackage {
    name: Option<toml::Value>,
    edition: Option<toml::Value>,
    build: Option<toml::Value>,
    autobins: Option<bool>,
    autotests: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoWorkspace {
    members: Option<Vec<toml::Value>>,
}

#[derive(Debug, Default, Deserialize)]
struct CargoTarget {
    name: Option<toml::Value>,
    path: Option<toml::Value>,
    #[serde(rename = "proc-macro")]
    proc_macro: Option<toml::Value>,
}

#[derive(Debug)]
struct ParsedManifest {
    path: String,
    root: String,
    manifest: CargoManifest,
}

#[derive(Debug, Clone)]
struct DependencyFact {
    name: String,
    manifest_path: String,
}

#[derive(Debug)]
struct PackageFact<'a> {
    manifest_path: &'a str,
    root: &'a str,
    dependency_name: String,
    edition: String,
    manifest: &'a CargoManifest,
    dependencies: Vec<DependencyFact>,
}

#[derive(Debug, Clone)]
struct TargetFact {
    manifest_path: String,
    package_dependency_name: String,
    root_module: String,
    edition: String,
    kind: &'static str,
    label: String,
    dependencies: Vec<DependencyFact>,
}

#[derive(Debug)]
struct AcceptedTarget {
    fact: TargetFact,
    crate_id: String,
}

struct ModelBudget {
    limits: ProviderModelLimits,
    files: usize,
    bytes: usize,
    inputs: InputDigest,
}

impl ModelBudget {
    fn new(limits: ProviderModelLimits) -> Self {
        let mut inputs = InputDigest::new();
        inputs.push(MODEL_POLICY.as_bytes());
        inputs.push(TOML_PARSER.as_bytes());
        inputs.push(limits.max_files.to_string().as_bytes());
        inputs.push(limits.max_bytes.to_string().as_bytes());
        inputs.push(limits.max_file_bytes.to_string().as_bytes());
        Self {
            limits,
            files: 0,
            bytes: 0,
            inputs,
        }
    }

    fn read(
        &mut self,
        canonical_root: &Path,
        relative_path: &str,
        file: &SnapshotFile,
        limitations: &mut BTreeSet<String>,
    ) -> Result<Option<Vec<u8>>, ModelBuildError> {
        if self.files >= self.limits.max_files {
            self.inputs
                .record_skipped(relative_path, "file-budget", file.bytes);
            push_path_limitation(
                limitations,
                "provider-model-file-budget-exhausted",
                relative_path,
            );
            return Ok(None);
        }
        self.files += 1;
        if file.bytes > self.limits.max_file_bytes {
            self.inputs
                .record_skipped(relative_path, "file-too-large", file.bytes);
            push_path_limitation(limitations, "provider-model-file-too-large", relative_path);
            return Ok(None);
        }
        let Some(next_bytes) = self.bytes.checked_add(file.bytes) else {
            return Err(ModelBuildError::new(
                "provider-model-budget-overflow",
                "provider model byte accounting overflowed",
            ));
        };
        if next_bytes > self.limits.max_bytes {
            self.inputs
                .record_skipped(relative_path, "byte-budget", file.bytes);
            push_path_limitation(
                limitations,
                "provider-model-byte-budget-exhausted",
                relative_path,
            );
            return Ok(None);
        }

        let canonical_file = fs::canonicalize(&file.absolute_path).map_err(|_| {
            ModelBuildError::new(
                "provider-model-file-unavailable",
                "a provider model input cannot be canonicalized",
            )
        })?;
        if canonical_file == canonical_root || !canonical_file.starts_with(canonical_root) {
            return Err(ModelBuildError::new(
                "provider-model-path-escape",
                "a provider model input escapes the candidate snapshot",
            ));
        }
        let metadata = fs::symlink_metadata(&canonical_file).map_err(|_| {
            ModelBuildError::new(
                "provider-model-file-unavailable",
                "a provider model input cannot be inspected",
            )
        })?;
        if !metadata.file_type().is_file() || metadata.len() != file.bytes as u64 {
            return Err(ModelBuildError::new(
                "provider-model-file-changed",
                "a provider model input changed while it was inspected",
            ));
        }
        let mut reader = File::open(&canonical_file)
            .map_err(|_| {
                ModelBuildError::new(
                    "provider-model-file-unavailable",
                    "a provider model input cannot be opened",
                )
            })?
            .take(file.bytes as u64 + 1);
        let mut bytes = Vec::with_capacity(file.bytes);
        reader.read_to_end(&mut bytes).map_err(|_| {
            ModelBuildError::new(
                "provider-model-file-unavailable",
                "a provider model input cannot be read",
            )
        })?;
        if bytes.len() != file.bytes {
            return Err(ModelBuildError::new(
                "provider-model-file-changed",
                "a provider model input changed while it was read",
            ));
        }
        self.bytes = next_bytes;
        self.inputs.record_bytes(relative_path, &bytes);
        Ok(Some(bytes))
    }
}

struct InputDigest(Sha256);

impl InputDigest {
    fn new() -> Self {
        let mut value = Self(Sha256::new());
        value.push(b"repository-context-provider-model-input/v1");
        value
    }

    fn push(&mut self, value: &[u8]) {
        self.0.update((value.len() as u64).to_be_bytes());
        self.0.update(value);
    }

    fn record_bytes(&mut self, path: &str, bytes: &[u8]) {
        self.push(b"consumed");
        self.push(path.as_bytes());
        self.push(bytes);
    }

    fn record_skipped(&mut self, path: &str, reason: &str, bytes: usize) {
        self.push(b"skipped");
        self.push(path.as_bytes());
        self.push(reason.as_bytes());
        self.push(bytes.to_string().as_bytes());
    }

    fn finish(self) -> String {
        format!("{:x}", self.0.finalize())
    }
}

pub fn build_linked_project_model(
    snapshot: &CandidateSnapshot,
    limits: ProviderModelLimits,
) -> Result<RustAnalyzerProjectModel, ModelBuildError> {
    validate_limits(limits)?;
    snapshot.verify_unchanged().map_err(|_| {
        ModelBuildError::new(
            "provider-model-snapshot-stale",
            "candidate snapshot changed before model construction",
        )
    })?;
    let canonical_root = fs::canonicalize(snapshot.path()).map_err(|_| {
        ModelBuildError::new(
            "provider-model-snapshot-invalid",
            "candidate snapshot root cannot be canonicalized",
        )
    })?;
    let files = enumerate_snapshot_files(&canonical_root)?;
    let mut limitations = BTreeSet::new();
    let mut budget = ModelBudget::new(limits);
    let parsed = parse_manifests(&canonical_root, &files, &mut budget, &mut limitations)?;
    record_workspace_limitations(&parsed, &files, &mut limitations);

    let mut targets = Vec::new();
    for manifest in &parsed {
        let Some(package) = package_fact(manifest, &files, &mut limitations) else {
            continue;
        };
        collect_package_targets(&package, &files, &mut targets, &mut limitations);
    }
    targets.sort_by(|left, right| {
        left.root_module
            .cmp(&right.root_module)
            .then_with(|| left.kind.cmp(right.kind))
            .then_with(|| left.manifest_path.cmp(&right.manifest_path))
            .then_with(|| left.label.cmp(&right.label))
    });
    targets.dedup_by(|left, right| left.root_module == right.root_module);

    let mut accepted = Vec::new();
    for target in targets {
        if accepted.len() >= MAX_NODES {
            push_limitation(&mut limitations, "provider-model-crate-budget-exhausted");
            break;
        }
        let Some(file) = files.get(&target.root_module) else {
            push_path_limitation(
                &mut limitations,
                "provider-model-root-missing",
                &target.root_module,
            );
            continue;
        };
        let Some(bytes) =
            budget.read(&canonical_root, &target.root_module, file, &mut limitations)?
        else {
            continue;
        };
        if std::str::from_utf8(&bytes).is_err() {
            push_path_limitation(
                &mut limitations,
                "provider-model-source-invalid-utf8",
                &target.root_module,
            );
            continue;
        }
        let crate_id = crate_id(accepted.len(), &target);
        accepted.push(AcceptedTarget {
            fact: target,
            crate_id,
        });
    }
    if accepted.is_empty() {
        return Err(ModelBuildError::new(
            "provider-model-crates-empty",
            "snapshot metadata did not yield a bounded Rust crate root",
        ));
    }

    let mut library_ids = BTreeMap::new();
    for target in &accepted {
        if target.fact.kind == "lib" {
            library_ids.insert(
                target.fact.manifest_path.clone(),
                (
                    target.crate_id.clone(),
                    target.fact.package_dependency_name.clone(),
                ),
            );
        }
    }
    let mut crates = Vec::with_capacity(accepted.len());
    for target in &accepted {
        let dependencies = resolve_dependencies(target, &library_ids, &mut limitations);
        crates.push(RustAnalyzerCrate {
            crate_id: target.crate_id.clone(),
            root_module: target.fact.root_module.clone(),
            edition: target.fact.edition.clone(),
            dependencies,
        });
    }

    limitations.insert(format!(
        "provider-model-input-sha256:{}",
        budget.inputs.finish()
    ));
    let mut model = RustAnalyzerProjectModel {
        schema_version: 1,
        algorithm: "rust-analyzer-linked-project-v1".to_string(),
        digest: "0".repeat(64),
        target_triple: target_triple(),
        crates,
        cfg: Vec::new(),
        env: BTreeMap::new(),
        limitations: limitations.into_iter().collect(),
    };
    model.digest = model.canonical_sha256();
    model.validate().map_err(|_| {
        ModelBuildError::new(
            "provider-model-invalid",
            "constructed linked project model failed contract validation",
        )
    })?;
    snapshot.verify_unchanged().map_err(|_| {
        ModelBuildError::new(
            "provider-model-snapshot-stale",
            "candidate snapshot changed during model construction",
        )
    })?;
    Ok(model)
}

fn validate_limits(limits: ProviderModelLimits) -> Result<(), ModelBuildError> {
    if limits.max_files == 0 || limits.max_bytes == 0 || limits.max_file_bytes == 0 {
        return Err(ModelBuildError::new(
            "provider-model-limits-invalid",
            "provider model limits must be positive",
        ));
    }
    Ok(())
}

fn enumerate_snapshot_files(
    canonical_root: &Path,
) -> Result<BTreeMap<String, SnapshotFile>, ModelBuildError> {
    let mut files = BTreeMap::new();
    let mut directories = VecDeque::from([canonical_root.to_path_buf()]);
    while let Some(directory) = directories.pop_front() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|_| {
                ModelBuildError::new(
                    "provider-model-snapshot-inspection-failed",
                    "candidate snapshot directory cannot be inspected",
                )
            })?
            .map(|entry| {
                let entry = entry.map_err(|_| {
                    ModelBuildError::new(
                        "provider-model-snapshot-inspection-failed",
                        "candidate snapshot entry cannot be inspected",
                    )
                })?;
                let name = entry.file_name().into_string().map_err(|_| {
                    ModelBuildError::new(
                        "provider-model-path-invalid",
                        "candidate snapshot paths must be valid UTF-8",
                    )
                })?;
                Ok((name, entry))
            })
            .collect::<Result<Vec<_>, ModelBuildError>>()?;
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        for (name, entry) in entries {
            let path = entry.path();
            let relative = normalized_relative_path(canonical_root, &path)?;
            if relative
                .split('/')
                .any(|component| component.eq_ignore_ascii_case(".git"))
            {
                return Err(ModelBuildError::new(
                    "provider-model-git-path-forbidden",
                    "candidate snapshot contains a forbidden Git metadata path",
                ));
            }
            let file_type = entry.file_type().map_err(|_| {
                ModelBuildError::new(
                    "provider-model-snapshot-inspection-failed",
                    "candidate snapshot entry type cannot be inspected",
                )
            })?;
            if file_type.is_symlink() {
                return Err(ModelBuildError::new(
                    "provider-model-symlink-forbidden",
                    "candidate snapshot contains a symbolic link",
                ));
            }
            if file_type.is_dir() {
                directories.push_back(path);
                continue;
            }
            if !file_type.is_file() {
                return Err(ModelBuildError::new(
                    "provider-model-file-type-invalid",
                    "candidate snapshot contains a non-regular file",
                ));
            }
            if name.eq_ignore_ascii_case("rust-analyzer.toml") {
                return Err(ModelBuildError::new(
                    "provider-model-repository-configuration-forbidden",
                    "repository-controlled rust-analyzer configuration is forbidden",
                ));
            }
            let metadata = entry.metadata().map_err(|_| {
                ModelBuildError::new(
                    "provider-model-snapshot-inspection-failed",
                    "candidate snapshot file metadata cannot be inspected",
                )
            })?;
            let bytes = usize::try_from(metadata.len()).map_err(|_| {
                ModelBuildError::new(
                    "provider-model-file-too-large",
                    "candidate snapshot file length exceeds this platform",
                )
            })?;
            files.insert(
                relative,
                SnapshotFile {
                    absolute_path: path,
                    bytes,
                },
            );
        }
    }
    Ok(files)
}

fn normalized_relative_path(root: &Path, path: &Path) -> Result<String, ModelBuildError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        ModelBuildError::new(
            "provider-model-path-escape",
            "candidate snapshot entry escapes the snapshot root",
        )
    })?;
    let mut components = Vec::new();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(ModelBuildError::new(
                "provider-model-path-invalid",
                "candidate snapshot entry is not lexically normalized",
            ));
        };
        let component = component.to_str().ok_or_else(|| {
            ModelBuildError::new(
                "provider-model-path-invalid",
                "candidate snapshot paths must be valid UTF-8",
            )
        })?;
        if component.is_empty()
            || component.contains(['\\', ':', '\0', '\r', '\n'])
            || component == "."
            || component == ".."
        {
            return Err(ModelBuildError::new(
                "provider-model-path-invalid",
                "candidate snapshot path contains an unsupported component",
            ));
        }
        components.push(component);
    }
    let value = components.join("/");
    if value.is_empty() || value.len() > MAX_RELATIVE_PATH_BYTES {
        return Err(ModelBuildError::new(
            "provider-model-path-invalid",
            "candidate snapshot path is empty or exceeds the model boundary",
        ));
    }
    Ok(value)
}

fn parse_manifests(
    canonical_root: &Path,
    files: &BTreeMap<String, SnapshotFile>,
    budget: &mut ModelBudget,
    limitations: &mut BTreeSet<String>,
) -> Result<Vec<ParsedManifest>, ModelBuildError> {
    let mut parsed = Vec::new();
    for (path, file) in files.iter().filter(|(path, _)| is_cargo_manifest(path)) {
        let Some(bytes) = budget.read(canonical_root, path, file, limitations)? else {
            continue;
        };
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(_) => {
                push_path_limitation(limitations, "provider-model-manifest-invalid-utf8", path);
                continue;
            }
        };
        let manifest = match toml::from_str::<CargoManifest>(text) {
            Ok(manifest) => manifest,
            Err(_) => {
                push_path_limitation(limitations, "provider-model-manifest-invalid", path);
                continue;
            }
        };
        parsed.push(ParsedManifest {
            path: path.clone(),
            root: manifest_root(path).to_string(),
            manifest,
        });
    }
    Ok(parsed)
}

fn record_workspace_limitations(
    manifests: &[ParsedManifest],
    files: &BTreeMap<String, SnapshotFile>,
    limitations: &mut BTreeSet<String>,
) {
    for parsed in manifests {
        let Some(workspace) = &parsed.manifest.workspace else {
            continue;
        };
        let Some(members) = &workspace.members else {
            continue;
        };
        for member in members {
            let Some(member) = member.as_str() else {
                push_path_limitation(
                    limitations,
                    "provider-model-workspace-member-unsupported",
                    &parsed.path,
                );
                continue;
            };
            if member.contains(['*', '?', '[', ']']) {
                push_path_limitation(
                    limitations,
                    "provider-model-workspace-glob-unsupported",
                    &parsed.path,
                );
                continue;
            }
            let Some(member_root) = join_relative(&parsed.root, member) else {
                push_path_limitation(
                    limitations,
                    "provider-model-workspace-member-unsupported",
                    &parsed.path,
                );
                continue;
            };
            let Some(member_manifest) = join_relative(&member_root, "Cargo.toml") else {
                continue;
            };
            if !files.contains_key(&member_manifest) {
                push_path_limitation(
                    limitations,
                    "provider-model-workspace-member-missing",
                    &member_manifest,
                );
            }
        }
    }
}

fn package_fact<'a>(
    parsed: &'a ParsedManifest,
    files: &BTreeMap<String, SnapshotFile>,
    limitations: &mut BTreeSet<String>,
) -> Option<PackageFact<'a>> {
    let package = parsed.manifest.package.as_ref()?;
    let name = required_string(
        package.name.as_ref(),
        "provider-model-package-name-invalid",
        &parsed.path,
        limitations,
    )?;
    let dependency_name = normalize_dependency_name(&name).filter(|value| is_identifier(value));
    let Some(dependency_name) = dependency_name else {
        push_path_limitation(
            limitations,
            "provider-model-package-name-invalid",
            &parsed.path,
        );
        return None;
    };
    let edition = match package.edition.as_ref() {
        None => "2015".to_string(),
        Some(toml::Value::String(value)) => value.clone(),
        Some(value) if is_workspace_inherited(value) => {
            push_path_limitation(
                limitations,
                "provider-model-workspace-inheritance-unsupported",
                &parsed.path,
            );
            return None;
        }
        Some(_) => {
            push_path_limitation(
                limitations,
                "provider-model-edition-unsupported",
                &parsed.path,
            );
            return None;
        }
    };
    if !matches!(edition.as_str(), "2015" | "2018" | "2021" | "2024") {
        push_path_limitation(
            limitations,
            "provider-model-edition-unsupported",
            &parsed.path,
        );
        return None;
    }

    let default_build =
        join_relative(&parsed.root, "build.rs").is_some_and(|path| files.contains_key(&path));
    match package.build.as_ref() {
        Some(value) if value.as_bool() == Some(false) => {}
        Some(value) if is_workspace_inherited(value) => {
            push_path_limitation(
                limitations,
                "provider-model-workspace-inheritance-unsupported",
                &parsed.path,
            );
            push_path_limitation(
                limitations,
                "provider-model-build-script-ignored",
                &parsed.path,
            );
        }
        Some(_) => push_path_limitation(
            limitations,
            "provider-model-build-script-ignored",
            &parsed.path,
        ),
        None if default_build => push_path_limitation(
            limitations,
            "provider-model-build-script-ignored",
            &parsed.path,
        ),
        None => {}
    }

    Some(PackageFact {
        manifest_path: &parsed.path,
        root: &parsed.root,
        dependency_name,
        edition,
        manifest: &parsed.manifest,
        dependencies: dependency_facts(parsed, limitations),
    })
}

fn dependency_facts(
    parsed: &ParsedManifest,
    limitations: &mut BTreeSet<String>,
) -> Vec<DependencyFact> {
    let mut facts = Vec::new();
    for (name, value) in &parsed.manifest.dependencies {
        let Some(name) = normalize_dependency_name(name).filter(|value| is_identifier(value))
        else {
            push_path_limitation(
                limitations,
                "provider-model-dependency-unsupported",
                &parsed.path,
            );
            continue;
        };
        let Some(table) = value.as_table() else {
            push_path_limitation(
                limitations,
                "provider-model-external-dependencies-omitted",
                &parsed.path,
            );
            continue;
        };
        if table.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
            push_path_limitation(
                limitations,
                "provider-model-workspace-inheritance-unsupported",
                &parsed.path,
            );
            continue;
        }
        let Some(path) = table.get("path").and_then(toml::Value::as_str) else {
            push_path_limitation(
                limitations,
                "provider-model-external-dependencies-omitted",
                &parsed.path,
            );
            continue;
        };
        let Some(root) = join_relative(&parsed.root, path) else {
            push_path_limitation(
                limitations,
                "provider-model-dependency-path-unsupported",
                &parsed.path,
            );
            continue;
        };
        let Some(manifest_path) = join_relative(&root, "Cargo.toml") else {
            continue;
        };
        facts.push(DependencyFact {
            name,
            manifest_path,
        });
    }
    facts.sort_by(|left, right| {
        left.manifest_path
            .cmp(&right.manifest_path)
            .then_with(|| left.name.cmp(&right.name))
    });
    facts.dedup_by(|left, right| {
        left.manifest_path == right.manifest_path && left.name == right.name
    });
    facts
}

fn collect_package_targets(
    package: &PackageFact<'_>,
    files: &BTreeMap<String, SnapshotFile>,
    targets: &mut Vec<TargetFact>,
    limitations: &mut BTreeSet<String>,
) {
    let Some(default_lib) = join_relative(package.root, "src/lib.rs") else {
        push_path_limitation(
            limitations,
            "provider-model-target-path-unsupported",
            package.manifest_path,
        );
        return;
    };
    if let Some(lib) = &package.manifest.lib {
        if lib
            .proc_macro
            .as_ref()
            .is_some_and(|value| value.as_bool() == Some(true))
        {
            push_path_limitation(
                limitations,
                "provider-model-proc-macro-ignored",
                package.manifest_path,
            );
        } else if lib.proc_macro.is_some() {
            push_path_limitation(
                limitations,
                "provider-model-target-field-unsupported",
                package.manifest_path,
            );
        }
        let root = optional_target_path(package, lib, &default_lib, "lib", limitations);
        add_target(
            package,
            root,
            "lib",
            "lib",
            true,
            files,
            targets,
            limitations,
        );
    } else if files.contains_key(&default_lib) {
        add_target(
            package,
            Some(default_lib),
            "lib",
            "lib",
            false,
            files,
            targets,
            limitations,
        );
    }

    for bin in &package.manifest.bins {
        let name = optional_string(
            bin.name.as_ref(),
            "provider-model-target-field-unsupported",
            package.manifest_path,
            limitations,
        );
        let default = name
            .as_deref()
            .and_then(|name| join_relative(package.root, &format!("src/bin/{name}.rs")));
        let root = optional_target_path(
            package,
            bin,
            default.as_deref().unwrap_or(""),
            "bin",
            limitations,
        );
        let label = name.unwrap_or_else(|| "bin".to_string());
        add_target(
            package,
            root,
            "bin",
            &label,
            true,
            files,
            targets,
            limitations,
        );
    }
    if package
        .manifest
        .package
        .as_ref()
        .is_none_or(|value| value.autobins != Some(false))
    {
        if let Some(main) = join_relative(package.root, "src/main.rs") {
            if files.contains_key(&main) {
                add_target(
                    package,
                    Some(main),
                    "bin",
                    "main",
                    false,
                    files,
                    targets,
                    limitations,
                );
            }
        }
        discover_targets(package, files, "src/bin", "bin", targets, limitations);
    }

    for test in &package.manifest.tests {
        let name = optional_string(
            test.name.as_ref(),
            "provider-model-target-field-unsupported",
            package.manifest_path,
            limitations,
        );
        let default = name
            .as_deref()
            .and_then(|name| join_relative(package.root, &format!("tests/{name}.rs")));
        let root = optional_target_path(
            package,
            test,
            default.as_deref().unwrap_or(""),
            "test",
            limitations,
        );
        let label = name.unwrap_or_else(|| "test".to_string());
        add_target(
            package,
            root,
            "test",
            &label,
            true,
            files,
            targets,
            limitations,
        );
    }
    if package
        .manifest
        .package
        .as_ref()
        .is_none_or(|value| value.autotests != Some(false))
    {
        discover_targets(package, files, "tests", "test", targets, limitations);
    }
}

fn optional_target_path(
    package: &PackageFact<'_>,
    target: &CargoTarget,
    default: &str,
    _kind: &str,
    limitations: &mut BTreeSet<String>,
) -> Option<String> {
    match target.path.as_ref() {
        Some(toml::Value::String(value)) => join_relative(package.root, value).or_else(|| {
            push_path_limitation(
                limitations,
                "provider-model-target-path-unsupported",
                package.manifest_path,
            );
            None
        }),
        Some(value) if is_workspace_inherited(value) => {
            push_path_limitation(
                limitations,
                "provider-model-workspace-inheritance-unsupported",
                package.manifest_path,
            );
            None
        }
        Some(_) => {
            push_path_limitation(
                limitations,
                "provider-model-target-field-unsupported",
                package.manifest_path,
            );
            None
        }
        None if !default.is_empty() => Some(default.to_string()),
        None => {
            push_path_limitation(
                limitations,
                "provider-model-target-field-unsupported",
                package.manifest_path,
            );
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn add_target(
    package: &PackageFact<'_>,
    root: Option<String>,
    kind: &'static str,
    label: &str,
    explicit: bool,
    files: &BTreeMap<String, SnapshotFile>,
    targets: &mut Vec<TargetFact>,
    limitations: &mut BTreeSet<String>,
) {
    let Some(root_module) = root else {
        return;
    };
    if Path::new(&root_module)
        .extension()
        .and_then(|value| value.to_str())
        != Some("rs")
    {
        push_path_limitation(
            limitations,
            "provider-model-target-type-unsupported",
            package.manifest_path,
        );
        return;
    }
    if !files.contains_key(&root_module) {
        if explicit {
            push_path_limitation(limitations, "provider-model-root-missing", &root_module);
        }
        return;
    }
    targets.push(TargetFact {
        manifest_path: package.manifest_path.to_string(),
        package_dependency_name: package.dependency_name.clone(),
        root_module,
        edition: package.edition.clone(),
        kind,
        label: label.to_string(),
        dependencies: package.dependencies.clone(),
    });
}

fn discover_targets(
    package: &PackageFact<'_>,
    files: &BTreeMap<String, SnapshotFile>,
    relative_directory: &str,
    kind: &'static str,
    targets: &mut Vec<TargetFact>,
    limitations: &mut BTreeSet<String>,
) {
    let Some(directory) = join_relative(package.root, relative_directory) else {
        return;
    };
    let prefix = format!("{directory}/");
    for path in files.keys() {
        let Some(relative) = path.strip_prefix(&prefix) else {
            continue;
        };
        let direct_file = !relative.contains('/') && relative.ends_with(".rs");
        let nested_main = relative
            .strip_suffix("/main.rs")
            .is_some_and(|name| !name.is_empty() && !name.contains('/'));
        if !(direct_file || kind == "bin" && nested_main) {
            continue;
        }
        let label = relative
            .strip_suffix(".rs")
            .or_else(|| relative.strip_suffix("/main.rs"))
            .unwrap_or(relative);
        add_target(
            package,
            Some(path.clone()),
            kind,
            label,
            false,
            files,
            targets,
            limitations,
        );
    }
}

fn resolve_dependencies(
    target: &AcceptedTarget,
    library_ids: &BTreeMap<String, (String, String)>,
    limitations: &mut BTreeSet<String>,
) -> Vec<RustAnalyzerDependency> {
    let mut candidates = Vec::new();
    if target.fact.kind != "lib" {
        if let Some((crate_id, name)) = library_ids.get(&target.fact.manifest_path) {
            candidates.push((name.clone(), crate_id.clone()));
        }
    }
    for dependency in &target.fact.dependencies {
        let Some((crate_id, _)) = library_ids.get(&dependency.manifest_path) else {
            push_path_limitation(
                limitations,
                "provider-model-local-dependency-missing",
                &target.fact.manifest_path,
            );
            continue;
        };
        candidates.push((dependency.name.clone(), crate_id.clone()));
    }
    candidates.sort();
    let mut names = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut dependencies = Vec::new();
    for (name, crate_id) in candidates {
        if crate_id == target.crate_id
            || !names.insert(name.clone())
            || !ids.insert(crate_id.clone())
        {
            push_path_limitation(
                limitations,
                "provider-model-dependency-duplicate",
                &target.fact.manifest_path,
            );
            continue;
        }
        dependencies.push(RustAnalyzerDependency { crate_id, name });
    }
    dependencies.sort_by(|left, right| {
        left.crate_id
            .cmp(&right.crate_id)
            .then_with(|| left.name.cmp(&right.name))
    });
    dependencies
}

fn required_string(
    value: Option<&toml::Value>,
    code: &'static str,
    manifest_path: &str,
    limitations: &mut BTreeSet<String>,
) -> Option<String> {
    let result = optional_string(value, code, manifest_path, limitations);
    if result.is_none() && value.is_none() {
        push_path_limitation(limitations, code, manifest_path);
    }
    result
}

fn optional_string(
    value: Option<&toml::Value>,
    code: &'static str,
    manifest_path: &str,
    limitations: &mut BTreeSet<String>,
) -> Option<String> {
    match value {
        Some(value) if value.as_str().is_some() => value.as_str().map(str::to_string),
        Some(value) if is_workspace_inherited(value) => {
            push_path_limitation(
                limitations,
                "provider-model-workspace-inheritance-unsupported",
                manifest_path,
            );
            None
        }
        Some(_) => {
            push_path_limitation(limitations, code, manifest_path);
            None
        }
        None => None,
    }
}

fn is_workspace_inherited(value: &toml::Value) -> bool {
    value
        .as_table()
        .and_then(|table| table.get("workspace"))
        .and_then(toml::Value::as_bool)
        == Some(true)
}

fn join_relative(root: &str, relative: &str) -> Option<String> {
    if relative.is_empty()
        || relative.contains(['\\', ':', '\0', '\r', '\n'])
        || Path::new(relative).is_absolute()
    {
        return None;
    }
    let mut components = if root == "." {
        Vec::new()
    } else {
        root.split('/').map(str::to_string).collect::<Vec<_>>()
    };
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(value) => {
                let value = value.to_str()?;
                if value.is_empty() || value == "." || value == ".." {
                    return None;
                }
                components.push(value.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                components.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    let value = components.join("/");
    (!value.is_empty() && value.len() <= MAX_RELATIVE_PATH_BYTES).then_some(value)
}

fn manifest_root(path: &str) -> &str {
    path.rsplit_once('/').map_or(".", |(root, _)| root)
}

fn is_cargo_manifest(path: &str) -> bool {
    path == "Cargo.toml" || path.ends_with("/Cargo.toml")
}

fn normalize_dependency_name(value: &str) -> Option<String> {
    let value = value.replace('-', "_");
    (!value.is_empty()).then_some(value)
}

fn is_identifier(value: &str) -> bool {
    value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn crate_id(index: usize, target: &TargetFact) -> String {
    let mut digest = InputDigest::new();
    digest.push(target.manifest_path.as_bytes());
    digest.push(target.root_module.as_bytes());
    digest.push(target.kind.as_bytes());
    digest.push(target.label.as_bytes());
    digest.push(target.edition.as_bytes());
    let digest = digest.finish();
    format!("crate-{index:08}-{}", &digest[..16])
}

fn push_path_limitation(limitations: &mut BTreeSet<String>, code: &str, path: &str) {
    push_limitation(limitations, &format!("{code}:{path}"));
}

fn push_limitation(limitations: &mut BTreeSet<String>, value: &str) {
    if limitations.contains(value) {
        return;
    }
    if limitations.len() < MAX_RETAINED_LIMITATIONS {
        limitations.insert(value.to_string());
    } else {
        limitations.insert("provider-model-limitations-truncated".to_string());
    }
}

fn target_triple() -> String {
    if cfg!(all(
        target_arch = "x86_64",
        target_os = "linux",
        target_env = "musl"
    )) {
        "x86_64-unknown-linux-musl".to_string()
    } else if cfg!(all(target_arch = "x86_64", target_os = "linux")) {
        "x86_64-unknown-linux-gnu".to_string()
    } else if cfg!(all(
        target_arch = "aarch64",
        target_os = "linux",
        target_env = "musl"
    )) {
        "aarch64-unknown-linux-musl".to_string()
    } else if cfg!(all(target_arch = "aarch64", target_os = "linux")) {
        "aarch64-unknown-linux-gnu".to_string()
    } else if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
        "aarch64-apple-darwin".to_string()
    } else if cfg!(all(target_arch = "x86_64", target_os = "macos")) {
        "x86_64-apple-darwin".to_string()
    } else if cfg!(all(
        target_arch = "x86_64",
        target_os = "windows",
        target_env = "gnu"
    )) {
        "x86_64-pc-windows-gnu".to_string()
    } else if cfg!(all(target_arch = "x86_64", target_os = "windows")) {
        "x86_64-pc-windows-msvc".to_string()
    } else {
        format!(
            "{}-unknown-{}",
            std::env::consts::ARCH,
            std::env::consts::OS
        )
    }
}
