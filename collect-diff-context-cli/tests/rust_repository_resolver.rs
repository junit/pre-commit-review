use collect_diff_context_cli::candidate::{CandidatePresence, RepoPath};
use collect_diff_context_cli::impact_context::adapters::tree_sitter_rust::{
    RustFileFacts, TreeSitterRustAdapter,
};
use collect_diff_context_cli::impact_context::contracts::{
    Completeness, EdgeKind, Resolution, UnitStatus,
};
use collect_diff_context_cli::impact_context::index::budget::{IndexBudget, IndexBudgetTracker};
use collect_diff_context_cli::impact_context::index::model::{
    FileFactKey, GraphGenerationIdentity, RepositoryGraph, RepositoryLocator, RepositoryManifest,
    RepositoryManifestEntry,
};
use collect_diff_context_cli::impact_context::index::project_model::{
    ProjectModelFile, RustPackageModel, RustProjectModel, RustTargetRoot,
};
use collect_diff_context_cli::impact_context::index::resolver::rust::{
    resolve_rust_repository, RustRepositoryFileFacts,
};
use collect_diff_context_cli::review_scope::ReviewSource;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;

const BASIC_PATHS: &[&str] = &[
    "Cargo.toml",
    "src/api.rs",
    "src/auth.rs",
    "src/lib.rs",
    "tests/auth_flow.rs",
];
const AMBIGUOUS_PATHS: &[&str] = &[
    "Cargo.toml",
    "src/a.rs",
    "src/b.rs",
    "src/caller.rs",
    "src/lib.rs",
];

fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn repeated_digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

#[derive(Clone)]
struct ResolverFixture {
    manifest: RepositoryManifest,
    project_model: RustProjectModel,
    file_facts: Vec<RustRepositoryFileFacts>,
    identity: GraphGenerationIdentity,
}

impl ResolverFixture {
    fn resolve(&self, budget: IndexBudget) -> RepositoryGraph {
        let mut tracker = IndexBudgetTracker::new(budget);
        resolve_rust_repository(
            &self.manifest,
            &self.project_model,
            &self.file_facts,
            self.identity.clone(),
            &mut tracker,
        )
        .unwrap()
    }
}

fn fixture(name: &str, paths: &[&str], package_name: &str) -> ResolverFixture {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repository_index")
        .join(name);
    let files = paths
        .iter()
        .map(|path| {
            (
                RepoPath::new(*path).unwrap(),
                std::fs::read(root.join(path)).unwrap(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    fixture_from_files(files, package_name)
}

fn fixture_from_files(files: BTreeMap<RepoPath, Vec<u8>>, package_name: &str) -> ResolverFixture {
    let manifest_digest = digest(
        &files
            .iter()
            .flat_map(|(path, bytes)| {
                [path.as_str().as_bytes(), bytes.as_slice()]
                    .concat()
                    .into_iter()
            })
            .collect::<Vec<_>>(),
    );
    let entries = files
        .iter()
        .map(|(path, bytes)| RepositoryManifestEntry {
            path: path.clone(),
            mode: "100644".to_string(),
            presence: CandidatePresence::Present,
            content_sha256: Some(digest(bytes)),
            content_bytes: Some(bytes.len()),
            language: path
                .as_str()
                .ends_with(".rs")
                .then(|| "rust".to_string())
                .or_else(|| path.as_str().ends_with(".toml").then(|| "toml".to_string())),
            status: UnitStatus::Completed,
            limitation_codes: Vec::new(),
        })
        .collect::<Vec<_>>();
    let manifest = RepositoryManifest {
        locator: RepositoryLocator {
            source: ReviewSource::Staged,
            object_format: "sha1".to_string(),
            base_tree: Some(std::iter::repeat_n('1', 40).collect()),
            index_manifest_digest: Some(repeated_digest('2')),
            overlay_candidate_digest: repeated_digest('3'),
        },
        digest: manifest_digest.clone(),
        entries,
        completeness: Completeness::Complete,
        limitations: Vec::new(),
    };
    let crate_name = package_name.replace('-', "_");
    let mut roots = vec![RustTargetRoot {
        package_name: package_name.to_string(),
        kind: "lib".to_string(),
        source_path: RepoPath::new("src/lib.rs").unwrap(),
        crate_name: crate_name.clone(),
    }];
    if files.contains_key(&RepoPath::new("tests/auth_flow.rs").unwrap()) {
        roots.push(RustTargetRoot {
            package_name: package_name.to_string(),
            kind: "test".to_string(),
            source_path: RepoPath::new("tests/auth_flow.rs").unwrap(),
            crate_name: "auth_flow".to_string(),
        });
    }
    let project_model_digest = digest(package_name.as_bytes());
    let project_model = RustProjectModel {
        digest: project_model_digest.clone(),
        packages: vec![RustPackageModel {
            package_name: package_name.to_string(),
            manifest_path: RepoPath::new("Cargo.toml").unwrap(),
            package_root: RepoPath::new(".").unwrap(),
        }],
        roots,
        consumed_files: vec![ProjectModelFile {
            path: RepoPath::new("Cargo.toml").unwrap(),
            content_sha256: files
                .get(&RepoPath::new("Cargo.toml").unwrap())
                .map(|bytes| digest(bytes)),
            content_bytes: files
                .get(&RepoPath::new("Cargo.toml").unwrap())
                .map(Vec::len),
            status: UnitStatus::Completed,
        }],
        completeness: Completeness::Complete,
        limitations: Vec::new(),
    };
    let file_facts = files
        .iter()
        .filter(|(path, _)| path.as_str().ends_with(".rs"))
        .map(|(path, bytes)| {
            let mut budget = IndexBudgetTracker::new(IndexBudget::deep_defaults());
            let facts = TreeSitterRustAdapter::analyze_index(bytes, &mut budget).unwrap();
            RustRepositoryFileFacts {
                path: path.clone(),
                key: FileFactKey {
                    language: "rust".to_string(),
                    content_sha256: digest(bytes),
                    grammar_version: "tree-sitter-rust@0.24.0".to_string(),
                    query_digest: repeated_digest('4'),
                    adapter_version: "tree-sitter-rust-index/v1".to_string(),
                    normalization_rules_digest: repeated_digest('5'),
                    schema_version: 1,
                },
                facts,
            }
        })
        .collect::<Vec<_>>();
    let identity = GraphGenerationIdentity {
        graph_schema_version: 1,
        candidate_manifest_digest: manifest_digest,
        project_model_digest,
        resolver_digest: repeated_digest('6'),
        adapter_query_digest: repeated_digest('7'),
        file_facts_manifest_digest: digest(package_name.replace('-', "_").as_bytes()),
        normalization_rules_digest: repeated_digest('8'),
    };
    ResolverFixture {
        manifest,
        project_model,
        file_facts,
        identity,
    }
}

fn basic() -> ResolverFixture {
    fixture("basic", BASIC_PATHS, "fixture")
}

fn ambiguous() -> ResolverFixture {
    fixture("ambiguous", AMBIGUOUS_PATHS, "ambiguous-fixture")
}

fn symbol_id(graph: &RepositoryGraph, path: &str, name: &str) -> String {
    graph
        .symbols
        .iter()
        .find(|symbol| symbol.path.as_str() == path && symbol.name == name)
        .unwrap_or_else(|| panic!("missing symbol {path}::{name}"))
        .symbol_id
        .clone()
}

fn assert_resolved_call(graph: &RepositoryGraph, from: &str, to: &str) {
    assert!(graph.edges.iter().any(|edge| {
        edge.kind == EdgeKind::Calls
            && edge.from_symbol == from
            && edge.to_symbol.as_deref() == Some(to)
            && edge.resolution == Resolution::ResolvedReference
    }));
}

#[test]
fn resolves_crate_self_super_alias_group_and_reexport_paths() {
    let graph = basic().resolve(IndexBudget::deep_defaults());
    let login = symbol_id(&graph, "src/api.rs", "login");
    let validate = symbol_id(&graph, "src/auth.rs", "validate_token");
    let nested = symbol_id(&graph, "src/lib.rs", "nested_validate");
    let via_self = symbol_id(&graph, "src/lib.rs", "via_self");

    assert_resolved_call(&graph, &login, &validate);
    assert_resolved_call(&graph, &login, &nested);
    assert_resolved_call(&graph, &via_self, &nested);
    assert!(graph.edges.iter().any(|edge| {
        edge.kind == EdgeKind::Imports
            && edge.path.as_str() == "tests/auth_flow.rs"
            && edge.to_symbol.as_deref() == Some(validate.as_str())
    }));
}

#[test]
fn builds_parent_child_modules_for_inline_and_file_modules() {
    let graph = basic().resolve(IndexBudget::deep_defaults());
    let root = graph
        .modules
        .iter()
        .find(|module| module.path.as_str() == "src/lib.rs" && module.root_module)
        .unwrap();
    let api = graph
        .modules
        .iter()
        .find(|module| module.path.as_str() == "src/api.rs")
        .unwrap();
    let auth = graph
        .modules
        .iter()
        .find(|module| module.path.as_str() == "src/auth.rs")
        .unwrap();
    let nested = graph
        .modules
        .iter()
        .find(|module| {
            module.path.as_str() == "src/lib.rs"
                && module.inline
                && module.parent_module_id.as_deref() == Some(root.module_id.as_str())
        })
        .unwrap();
    let inner = graph
        .modules
        .iter()
        .find(|module| {
            module.path.as_str() == "src/lib.rs"
                && module.inline
                && module.parent_module_id.as_deref() == Some(nested.module_id.as_str())
        })
        .unwrap();

    assert_eq!(
        api.parent_module_id.as_deref(),
        Some(root.module_id.as_str())
    );
    assert_eq!(
        auth.parent_module_id.as_deref(),
        Some(root.module_id.as_str())
    );
    assert_eq!(
        nested.parent_module_id.as_deref(),
        Some(root.module_id.as_str())
    );
    assert_eq!(
        inner.parent_module_id.as_deref(),
        Some(nested.module_id.as_str())
    );
}

#[test]
fn resolves_unique_free_and_associated_function_calls() {
    let graph = basic().resolve(IndexBudget::deep_defaults());
    let login = symbol_id(&graph, "src/api.rs", "login");
    let validate = symbol_id(&graph, "src/auth.rs", "validate_token");
    let associated = symbol_id(&graph, "src/auth.rs", "validate");

    assert_resolved_call(&graph, &login, &validate);
    assert_resolved_call(&graph, &login, &associated);
}

#[test]
fn records_reverse_imports_and_references() {
    let graph = basic().resolve(IndexBudget::deep_defaults());
    let login = symbol_id(&graph, "src/api.rs", "login");
    let validate = symbol_id(&graph, "src/auth.rs", "validate_token");
    let default_allowed = symbol_id(&graph, "src/api.rs", "default_allowed");
    let default_value = symbol_id(&graph, "src/auth.rs", "DEFAULT_ALLOWED");

    assert!(graph.edges.iter().any(|edge| {
        edge.kind == EdgeKind::Imports && edge.to_symbol.as_deref() == Some(validate.as_str())
    }));
    assert!(graph.edges.iter().any(|edge| {
        edge.kind == EdgeKind::References
            && edge.from_symbol == login
            && edge.to_symbol.as_deref() == Some(validate.as_str())
    }));
    assert!(graph.edges.iter().any(|edge| {
        edge.kind == EdgeKind::Calls
            && edge.from_symbol == login
            && edge.to_symbol.as_deref() == Some(validate.as_str())
    }));
    assert!(graph.edges.iter().any(|edge| {
        edge.kind == EdgeKind::References
            && edge.from_symbol == default_allowed
            && edge.to_symbol.as_deref() == Some(default_value.as_str())
    }));
}

#[test]
fn glob_duplicate_method_trait_macro_and_cfg_cases_remain_honestly_partial() {
    let graph = ambiguous().resolve(IndexBudget::deep_defaults());

    assert_eq!(graph.completeness, Completeness::Partial);
    for target in ["parse", "len", "debug"] {
        assert!(graph.edges.iter().any(|edge| {
            edge.kind == EdgeKind::Calls
                && edge.unresolved_target.as_deref() == Some(target)
                && matches!(
                    edge.resolution,
                    Resolution::PolymorphicCandidate | Resolution::Unresolved
                )
        }));
    }
    assert!(graph
        .limitations
        .iter()
        .any(|limitation| limitation.code == "rust-resolver-glob-import-ambiguous"));
    assert!(graph
        .limitations
        .iter()
        .any(|limitation| limitation.code == "rust-resolver-method-call-unresolved"));
    assert!(graph
        .limitations
        .iter()
        .any(|limitation| limitation.code == "rust-resolver-macro-call-unresolved"));
    assert!(graph
        .limitations
        .iter()
        .any(|limitation| limitation.code == "rust-resolver-cfg-conditional"));
}

#[test]
fn rename_delete_and_module_move_change_generation_relationships() {
    let initial = basic();
    let initial_graph = initial.resolve(IndexBudget::deep_defaults());
    let fixture_root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repository_index/basic");
    let mut moved_files = BASIC_PATHS
        .iter()
        .map(|path| {
            (
                RepoPath::new(*path).unwrap(),
                std::fs::read(fixture_root.join(path)).unwrap(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let auth = moved_files
        .remove(&RepoPath::new("src/auth.rs").unwrap())
        .unwrap();
    moved_files.insert(RepoPath::new("src/security.rs").unwrap(), auth);
    let lib = moved_files
        .get_mut(&RepoPath::new("src/lib.rs").unwrap())
        .unwrap();
    *lib = String::from_utf8(lib.clone())
        .unwrap()
        .replace("mod auth", "mod security")
        .replace("auth::", "security::")
        .into_bytes();
    let api = moved_files
        .get_mut(&RepoPath::new("src/api.rs").unwrap())
        .unwrap();
    *api = String::from_utf8(api.clone())
        .unwrap()
        .replace("auth::", "security::")
        .into_bytes();
    let moved_graph =
        fixture_from_files(moved_files, "fixture").resolve(IndexBudget::deep_defaults());

    let initial_validate = symbol_id(&initial_graph, "src/auth.rs", "validate_token");
    let moved_validate = symbol_id(&moved_graph, "src/security.rs", "validate_token");
    assert_ne!(initial_validate, moved_validate);
    assert_ne!(initial_graph.edges, moved_graph.edges);
}

#[test]
fn resolver_output_is_deterministic_under_manifest_order_changes() {
    let fixture = basic();
    let first = fixture.resolve(IndexBudget::deep_defaults());
    let mut reordered = fixture.clone();
    reordered.file_facts.reverse();
    let second = reordered.resolve(IndexBudget::deep_defaults());

    assert_eq!(first, second);
}

#[test]
fn resolver_budget_exhaustion_preserves_partial_graph_and_limitations() {
    let mut budget = IndexBudget::deep_defaults();
    budget.max_symbols = 4;
    budget.max_edges = 4;
    let graph = basic().resolve(budget);

    assert_eq!(graph.completeness, Completeness::Partial);
    assert!(!graph.symbols.is_empty());
    assert!(graph.symbols.len() <= 4);
    assert!(graph.edges.len() <= 4);
    assert!(graph.limitations.iter().any(|limitation| {
        limitation.code == "index-symbol-budget-exhausted"
            || limitation.code == "index-edge-budget-exhausted"
    }));
}

#[allow(dead_code)]
fn _assert_facts_are_owned(_: RustFileFacts) {}
