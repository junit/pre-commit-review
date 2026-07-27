use crate::candidate::{CandidatePresence, RepoPath};
use crate::impact_context::adapters::tree_sitter_rust::{
    RustCallSiteFact, RustFileFacts, RustImportFact, RustLocalSymbolFact, RustModuleDeclarationFact,
};
use crate::impact_context::contracts::{
    Completeness, Confidence, EdgeKind, Resolution, SourceRange,
};
use crate::impact_context::index::budget::{IndexBudgetTracker, IndexResource};
use crate::impact_context::index::model::{
    FileFactKey, GraphEdge, GraphFile, GraphGenerationIdentity, GraphModule, GraphSymbol,
    IndexLimitation, RepositoryGraph, RepositoryManifest,
};
use crate::impact_context::index::project_model::RustProjectModel;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const PROVIDER_ID: &str = "rust-tree-sitter-resolver";
const PROVIDER_VERSION: &str = "rust-resolver/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustRepositoryFileFacts {
    pub path: RepoPath,
    pub key: FileFactKey,
    pub facts: RustFileFacts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustResolverError {
    pub code: &'static str,
    pub message: String,
}

impl RustResolverError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RustResolverError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RustResolverError {}

#[derive(Debug, Clone)]
struct ModuleState {
    graph: GraphModule,
    logical_path: Vec<String>,
    declaration_range: Option<SourceRange>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum BindingTarget {
    Module(Vec<String>),
    Symbol(String),
}

#[derive(Debug, Clone)]
struct ImportWork {
    path: RepoPath,
    module_id: String,
    from_symbol: Option<String>,
    import: RustImportFact,
}

struct ModuleBuild {
    states: Vec<ModuleState>,
    file_modules: BTreeMap<RepoPath, Vec<String>>,
    inline_modules: BTreeMap<(RepoPath, String), String>,
}

struct SymbolBuild {
    symbols: Vec<GraphSymbol>,
    local_symbol_ids: BTreeMap<(RepoPath, String), String>,
    owner_local_ids: BTreeMap<String, (RepoPath, String)>,
}

struct SymbolNamespaces {
    by_logical: BTreeMap<Vec<String>, Vec<String>>,
    logical_by_id: BTreeMap<String, Vec<String>>,
}

struct ImportResolution {
    bindings: BTreeMap<(String, String), BTreeSet<BindingTarget>>,
    exports: BTreeMap<Vec<String>, BTreeSet<BindingTarget>>,
    glob_modules: BTreeMap<String, BTreeSet<Vec<String>>>,
    edges: Vec<GraphEdge>,
}

struct CallLookup<'a> {
    symbols_by_logical: &'a BTreeMap<Vec<String>, Vec<String>>,
    symbol_logical: &'a BTreeMap<String, Vec<String>>,
    crate_names: &'a BTreeSet<String>,
    bindings: &'a BTreeMap<(String, String), BTreeSet<BindingTarget>>,
    exports: &'a BTreeMap<Vec<String>, BTreeSet<BindingTarget>>,
    glob_modules: &'a BTreeMap<String, BTreeSet<Vec<String>>>,
}

pub fn resolve_rust_repository(
    repository_manifest: &RepositoryManifest,
    project_model: &RustProjectModel,
    file_facts: &[RustRepositoryFileFacts],
    identity: GraphGenerationIdentity,
    budget: &mut IndexBudgetTracker,
) -> Result<RepositoryGraph, RustResolverError> {
    repository_manifest.validate().map_err(|error| {
        RustResolverError::new(
            "rust-resolver-manifest-invalid",
            format!("repository manifest is invalid: {error}"),
        )
    })?;
    identity.validate().map_err(|error| {
        RustResolverError::new(
            "rust-resolver-identity-invalid",
            format!("graph identity is invalid: {error}"),
        )
    })?;
    if identity.candidate_manifest_digest != repository_manifest.digest {
        return Err(RustResolverError::new(
            "rust-resolver-manifest-identity-mismatch",
            "graph identity does not bind the repository manifest",
        ));
    }
    if identity.project_model_digest != project_model.digest {
        return Err(RustResolverError::new(
            "rust-resolver-project-model-identity-mismatch",
            "graph identity does not bind the Rust project model",
        ));
    }

    let mut limitations = Vec::new();
    if repository_manifest.completeness != Completeness::Complete {
        add_limitation(
            &mut limitations,
            "rust-resolver-manifest-partial",
            None,
            None,
        );
    }
    if project_model.completeness != Completeness::Complete {
        add_limitation(
            &mut limitations,
            "rust-resolver-project-model-partial",
            None,
            None,
        );
    }

    let facts_by_path = validate_file_facts(repository_manifest, file_facts)?;
    for file in facts_by_path.values() {
        if file.facts.parse_quality != crate::impact_context::contracts::ParseQuality::Clean {
            add_limitation(
                &mut limitations,
                "rust-resolver-recovered-syntax",
                Some(file.path.clone()),
                None,
            );
        }
        for code in &file.facts.limitations {
            add_limitation(&mut limitations, code, Some(file.path.clone()), None);
        }
        if file
            .facts
            .attributes
            .iter()
            .any(|attribute| attribute.name == "cfg" || attribute.name == "cfg_attr")
        {
            add_limitation(
                &mut limitations,
                "rust-resolver-cfg-conditional",
                Some(file.path.clone()),
                None,
            );
        }
    }

    let manifest_paths = repository_manifest
        .entries
        .iter()
        .filter(|entry| entry.presence == CandidatePresence::Present)
        .map(|entry| entry.path.clone())
        .collect::<BTreeSet<_>>();
    let module_build = build_modules(
        project_model,
        &facts_by_path,
        &manifest_paths,
        budget,
        &mut limitations,
    );
    let mut module_states = module_build.states;
    let file_modules = module_build.file_modules;
    let inline_modules = module_build.inline_modules;
    module_states.sort_by(|left, right| left.graph.module_id.cmp(&right.graph.module_id));
    let module_by_id = module_states
        .iter()
        .map(|module| (module.graph.module_id.clone(), module.clone()))
        .collect::<BTreeMap<_, _>>();

    let symbol_build = build_symbols(
        &facts_by_path,
        &file_modules,
        &inline_modules,
        &module_by_id,
        budget,
        &mut limitations,
    );
    let mut symbols = symbol_build.symbols;
    let local_symbol_ids = symbol_build.local_symbol_ids;
    let owner_local_ids = symbol_build.owner_local_ids;
    populate_owner_ids(&mut symbols, &local_symbol_ids, &owner_local_ids);
    symbols.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));

    let symbol_namespaces = build_symbol_namespaces(&symbols, &module_by_id);
    let modules_by_logical = module_states
        .iter()
        .map(|module| (module.logical_path.clone(), module.graph.module_id.clone()))
        .fold(
            BTreeMap::<Vec<String>, Vec<String>>::new(),
            |mut map, (path, id)| {
                map.entry(path).or_default().push(id);
                map
            },
        );
    let crate_names = project_model
        .roots
        .iter()
        .map(|root| root.crate_name.clone())
        .collect::<BTreeSet<_>>();

    let import_work = collect_import_work(&facts_by_path, &file_modules, &module_states, &symbols);
    let import_resolution = build_import_bindings(
        &import_work,
        &module_by_id,
        &modules_by_logical,
        &symbol_namespaces.by_logical,
        &crate_names,
        budget,
        &mut limitations,
    );
    let mut edges = import_resolution.edges;
    let call_lookup = CallLookup {
        symbols_by_logical: &symbol_namespaces.by_logical,
        symbol_logical: &symbol_namespaces.logical_by_id,
        crate_names: &crate_names,
        bindings: &import_resolution.bindings,
        exports: &import_resolution.exports,
        glob_modules: &import_resolution.glob_modules,
    };

    build_call_edges(
        &facts_by_path,
        &file_modules,
        &inline_modules,
        &module_by_id,
        &symbols,
        &local_symbol_ids,
        &call_lookup,
        budget,
        &mut limitations,
        &mut edges,
    );
    build_reference_edges(
        &facts_by_path,
        &file_modules,
        &inline_modules,
        &module_by_id,
        &local_symbol_ids,
        &call_lookup,
        budget,
        &mut limitations,
        &mut edges,
    );

    edges.sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
    edges.dedup_by(|left, right| left.edge_id == right.edge_id);
    limitations.sort_by(limitation_order);
    limitations.dedup();

    let files = repository_manifest
        .entries
        .iter()
        .map(|entry| GraphFile {
            path: entry.path.clone(),
            mode: entry.mode.clone(),
            presence: entry.presence,
            content_sha256: entry.content_sha256.clone(),
            file_fact_key: facts_by_path.get(&entry.path).map(|file| file.key.clone()),
            language: entry.language.clone(),
            module_id: file_modules
                .get(&entry.path)
                .and_then(|modules| modules.first())
                .cloned(),
        })
        .collect();
    let modules = module_states
        .into_iter()
        .map(|module| module.graph)
        .collect();
    let completeness = if limitations.is_empty() {
        Completeness::Complete
    } else {
        Completeness::Partial
    };
    Ok(RepositoryGraph {
        identity,
        files,
        modules,
        symbols,
        edges,
        completeness,
        limitations,
    })
}

fn validate_file_facts<'a>(
    repository_manifest: &RepositoryManifest,
    file_facts: &'a [RustRepositoryFileFacts],
) -> Result<BTreeMap<RepoPath, &'a RustRepositoryFileFacts>, RustResolverError> {
    let manifest_by_path = repository_manifest
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut result = BTreeMap::new();
    for file in file_facts {
        file.key.validate().map_err(|error| {
            RustResolverError::new(
                "rust-resolver-file-fact-key-invalid",
                format!("invalid FileFacts key for {}: {error}", file.path.as_str()),
            )
        })?;
        let Some(entry) = manifest_by_path.get(&file.path) else {
            return Err(RustResolverError::new(
                "rust-resolver-file-fact-path-unknown",
                format!(
                    "FileFacts path is absent from manifest: {}",
                    file.path.as_str()
                ),
            ));
        };
        if entry.presence != CandidatePresence::Present
            || entry.content_sha256.as_deref() != Some(file.key.content_sha256.as_str())
            || file.key.language != "rust"
        {
            return Err(RustResolverError::new(
                "rust-resolver-file-fact-identity-mismatch",
                format!("FileFacts identity mismatch for {}", file.path.as_str()),
            ));
        }
        if result.insert(file.path.clone(), file).is_some() {
            return Err(RustResolverError::new(
                "rust-resolver-file-fact-duplicate",
                format!("duplicate FileFacts path: {}", file.path.as_str()),
            ));
        }
    }
    Ok(result)
}

fn build_modules(
    project_model: &RustProjectModel,
    facts_by_path: &BTreeMap<RepoPath, &RustRepositoryFileFacts>,
    manifest_paths: &BTreeSet<RepoPath>,
    budget: &mut IndexBudgetTracker,
    limitations: &mut Vec<IndexLimitation>,
) -> ModuleBuild {
    let mut states = Vec::new();
    let mut file_modules = BTreeMap::<RepoPath, Vec<String>>::new();
    let mut inline_modules = BTreeMap::<(RepoPath, String), String>::new();
    let mut roots = project_model.roots.iter().collect::<Vec<_>>();
    roots.sort_by(|left, right| {
        left.source_path
            .cmp(&right.source_path)
            .then_with(|| left.crate_name.cmp(&right.crate_name))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    for root in roots {
        if budget.check_deadline().is_err() {
            add_limitation(limitations, "index-deadline-exhausted", None, None);
            break;
        }
        if !facts_by_path.contains_key(&root.source_path) {
            add_limitation(
                limitations,
                "rust-resolver-target-facts-missing",
                Some(root.source_path.clone()),
                None,
            );
            continue;
        }
        let module_id = module_id(None, &root.crate_name, &root.source_path, false);
        file_modules
            .entry(root.source_path.clone())
            .or_default()
            .push(module_id.clone());
        states.push(ModuleState {
            graph: GraphModule {
                module_id,
                parent_module_id: None,
                crate_name: root.crate_name.clone(),
                path: root.source_path.clone(),
                inline: false,
                root_module: true,
                resolution_status: "resolved".to_string(),
            },
            logical_path: vec![root.crate_name.clone()],
            declaration_range: None,
        });
    }

    let mut progress = true;
    while progress {
        progress = false;
        let state_by_id = states
            .iter()
            .map(|state| (state.graph.module_id.clone(), state.clone()))
            .collect::<BTreeMap<_, _>>();
        for (path, file) in facts_by_path {
            let mut declarations = file.facts.module_declarations.iter().collect::<Vec<_>>();
            declarations.sort_by(|left, right| {
                left.range
                    .start_byte
                    .cmp(&right.range.start_byte)
                    .then_with(|| left.name.cmp(&right.name))
            });
            for declaration in declarations {
                let Some(local_id) = module_local_id(&file.facts, declaration) else {
                    add_limitation(
                        limitations,
                        "rust-resolver-module-symbol-missing",
                        Some(path.clone()),
                        None,
                    );
                    continue;
                };
                let key = (path.clone(), local_id.clone());
                if inline_modules.contains_key(&key) {
                    continue;
                }
                let parent_id = declaration
                    .owner_local_id
                    .as_ref()
                    .and_then(|owner| inline_modules.get(&(path.clone(), owner.clone())))
                    .cloned()
                    .or_else(|| {
                        file_modules
                            .get(path)
                            .and_then(|modules| modules.first())
                            .cloned()
                    });
                let Some(parent_id) = parent_id else {
                    continue;
                };
                let Some(parent) = state_by_id.get(&parent_id) else {
                    continue;
                };
                let mut logical_path = parent.logical_path.clone();
                logical_path.push(declaration.name.clone());
                let module_path = if declaration.inline {
                    path.clone()
                } else {
                    let Some(module_path) = module_file_path(path, declaration, manifest_paths)
                    else {
                        add_limitation(
                            limitations,
                            "rust-resolver-module-file-missing",
                            Some(path.clone()),
                            None,
                        );
                        continue;
                    };
                    module_path
                };
                let id = module_id(
                    Some(&parent_id),
                    &declaration.name,
                    &module_path,
                    declaration.inline,
                );
                inline_modules.insert(key, id.clone());
                if !declaration.inline {
                    file_modules
                        .entry(module_path.clone())
                        .or_default()
                        .push(id.clone());
                }
                states.push(ModuleState {
                    graph: GraphModule {
                        module_id: id,
                        parent_module_id: Some(parent_id),
                        crate_name: parent.graph.crate_name.clone(),
                        path: module_path,
                        inline: declaration.inline,
                        root_module: false,
                        resolution_status: "resolved".to_string(),
                    },
                    logical_path,
                    declaration_range: declaration.inline.then(|| declaration.range.clone()),
                });
                progress = true;
            }
        }
    }
    for modules in file_modules.values_mut() {
        modules.sort();
        modules.dedup();
    }
    ModuleBuild {
        states,
        file_modules,
        inline_modules,
    }
}

fn build_symbols(
    facts_by_path: &BTreeMap<RepoPath, &RustRepositoryFileFacts>,
    file_modules: &BTreeMap<RepoPath, Vec<String>>,
    inline_modules: &BTreeMap<(RepoPath, String), String>,
    module_by_id: &BTreeMap<String, ModuleState>,
    budget: &mut IndexBudgetTracker,
    limitations: &mut Vec<IndexLimitation>,
) -> SymbolBuild {
    let mut symbols = Vec::new();
    let mut local_ids = BTreeMap::new();
    let mut owner_local_ids = BTreeMap::new();
    let mut exhausted = false;
    for (path, file) in facts_by_path {
        let facts_by_local = file
            .facts
            .symbols
            .iter()
            .map(|symbol| (symbol.local_id.as_str(), symbol))
            .collect::<BTreeMap<_, _>>();
        let mut local_symbols = file.facts.symbols.iter().collect::<Vec<_>>();
        local_symbols.sort_by(|left, right| left.local_id.cmp(&right.local_id));
        for fact in local_symbols {
            if let Err(exhaustion) = budget.check_deadline() {
                add_limitation(limitations, exhaustion.code(), None, None);
                exhausted = true;
                break;
            }
            let Some(module_id) =
                symbol_module_id(path, fact, &facts_by_local, file_modules, inline_modules)
            else {
                add_limitation(
                    limitations,
                    "rust-resolver-symbol-module-unresolved",
                    Some(path.clone()),
                    None,
                );
                continue;
            };
            if !module_by_id.contains_key(&module_id) {
                continue;
            }
            if let Err(exhaustion) = budget.consume(IndexResource::Symbols, 1) {
                add_limitation(limitations, exhaustion.code(), None, None);
                exhausted = true;
                break;
            }
            let id = symbol_id(&module_id, path, &fact.local_id);
            local_ids.insert((path.clone(), fact.local_id.clone()), id.clone());
            if let Some(owner_local_id) = &fact.owner_local_id {
                owner_local_ids.insert(id.clone(), (path.clone(), owner_local_id.clone()));
            }
            symbols.push(GraphSymbol {
                symbol_id: id,
                local_id: fact.local_id.clone(),
                module_id,
                path: path.clone(),
                language: "rust".to_string(),
                kind: fact.kind.clone(),
                name: fact.name.clone(),
                owner_symbol_id: None,
                signature: (!fact.signature.is_empty()).then(|| fact.signature.clone()),
                visibility: fact.visibility.clone(),
                range: fact.range.clone(),
                confidence: Confidence::Medium,
            });
        }
        if exhausted {
            break;
        }
    }
    SymbolBuild {
        symbols,
        local_symbol_ids: local_ids,
        owner_local_ids,
    }
}

fn populate_owner_ids(
    symbols: &mut [GraphSymbol],
    local_symbol_ids: &BTreeMap<(RepoPath, String), String>,
    owner_local_ids: &BTreeMap<String, (RepoPath, String)>,
) {
    for symbol in symbols {
        symbol.owner_symbol_id = owner_local_ids
            .get(&symbol.symbol_id)
            .and_then(|owner| local_symbol_ids.get(owner))
            .cloned();
    }
}

fn build_symbol_namespaces(
    symbols: &[GraphSymbol],
    module_by_id: &BTreeMap<String, ModuleState>,
) -> SymbolNamespaces {
    let by_id = symbols
        .iter()
        .map(|symbol| (symbol.symbol_id.as_str(), symbol))
        .collect::<BTreeMap<_, _>>();
    let mut namespaces = BTreeMap::<Vec<String>, Vec<String>>::new();
    let mut logical_by_id = BTreeMap::new();
    for symbol in symbols {
        let Some(module) = module_by_id.get(&symbol.module_id) else {
            continue;
        };
        let mut logical = module.logical_path.clone();
        let mut owners = Vec::new();
        let mut current = symbol
            .owner_symbol_id
            .as_deref()
            .and_then(|id| by_id.get(id).copied());
        while let Some(owner) = current {
            if owner.kind != "module" {
                owners.push(owner.name.clone());
            }
            current = owner
                .owner_symbol_id
                .as_deref()
                .and_then(|id| by_id.get(id).copied());
        }
        owners.reverse();
        logical.extend(owners);
        logical.push(symbol.name.clone());
        logical_by_id.insert(symbol.symbol_id.clone(), logical);
        if symbol.kind != "impl" {
            namespaces
                .entry(logical_by_id[&symbol.symbol_id].clone())
                .or_default()
                .push(symbol.symbol_id.clone());
        }
    }
    for ids in namespaces.values_mut() {
        ids.sort();
        ids.dedup();
    }
    SymbolNamespaces {
        by_logical: namespaces,
        logical_by_id,
    }
}

fn collect_import_work(
    facts_by_path: &BTreeMap<RepoPath, &RustRepositoryFileFacts>,
    file_modules: &BTreeMap<RepoPath, Vec<String>>,
    module_states: &[ModuleState],
    symbols: &[GraphSymbol],
) -> Vec<ImportWork> {
    let mut work = Vec::new();
    for (path, file) in facts_by_path {
        for import in &file.facts.imports {
            let Some(module_id) =
                module_for_range(path, &import.range, file_modules, module_states)
            else {
                continue;
            };
            let from_symbol = enclosing_symbol(path, &module_id, &import.range, symbols)
                .or_else(|| first_module_symbol(&module_id, symbols));
            work.push(ImportWork {
                path: path.clone(),
                module_id,
                from_symbol,
                import: import.clone(),
            });
        }
    }
    work.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| {
                left.import
                    .range
                    .start_byte
                    .cmp(&right.import.range.start_byte)
            })
            .then_with(|| left.import.segments.cmp(&right.import.segments))
    });
    work
}

#[allow(clippy::too_many_arguments)]
fn build_import_bindings(
    work: &[ImportWork],
    module_by_id: &BTreeMap<String, ModuleState>,
    modules_by_logical: &BTreeMap<Vec<String>, Vec<String>>,
    symbols_by_logical: &BTreeMap<Vec<String>, Vec<String>>,
    crate_names: &BTreeSet<String>,
    budget: &mut IndexBudgetTracker,
    limitations: &mut Vec<IndexLimitation>,
) -> ImportResolution {
    let mut bindings = BTreeMap::<(String, String), BTreeSet<BindingTarget>>::new();
    let mut exports = BTreeMap::<Vec<String>, BTreeSet<BindingTarget>>::new();
    let mut glob_modules = BTreeMap::<String, BTreeSet<Vec<String>>>::new();
    let mut edges = Vec::new();
    let mut glob_counts = BTreeMap::<String, usize>::new();
    for item in work {
        let Some(source_module) = module_by_id.get(&item.module_id) else {
            continue;
        };
        let targets = resolve_path_targets(
            source_module,
            &item.import.segments,
            modules_by_logical,
            symbols_by_logical,
            crate_names,
            &exports,
        );
        if item.import.glob {
            *glob_counts.entry(item.module_id.clone()).or_default() += 1;
            for target in &targets {
                if let BindingTarget::Module(path) = target {
                    glob_modules
                        .entry(item.module_id.clone())
                        .or_default()
                        .insert(path.clone());
                }
            }
            if targets.is_empty() {
                add_limitation(
                    limitations,
                    "rust-resolver-glob-import-unresolved",
                    Some(item.path.clone()),
                    None,
                );
            }
            continue;
        }
        let alias = item
            .import
            .alias
            .clone()
            .or_else(|| item.import.segments.last().cloned());
        let Some(alias) = alias else {
            continue;
        };
        for target in &targets {
            bindings
                .entry((item.module_id.clone(), alias.clone()))
                .or_default()
                .insert(target.clone());
            if item.import.public {
                let mut export_path = source_module.logical_path.clone();
                export_path.push(alias.clone());
                exports
                    .entry(export_path)
                    .or_default()
                    .insert(target.clone());
            }
            if let (Some(from_symbol), BindingTarget::Symbol(to_symbol)) =
                (item.from_symbol.as_ref(), target)
            {
                let edge = make_edge(
                    EdgeKind::Imports,
                    from_symbol,
                    Some(to_symbol.clone()),
                    None,
                    &item.path,
                    &item.import.range,
                    Resolution::ResolvedReference,
                    Confidence::Medium,
                    None,
                );
                push_edge(edges.as_mut(), edge, budget, limitations);
                if item.import.public {
                    let edge = make_edge(
                        EdgeKind::Exports,
                        from_symbol,
                        Some(to_symbol.clone()),
                        None,
                        &item.path,
                        &item.import.range,
                        Resolution::ResolvedReference,
                        Confidence::Medium,
                        None,
                    );
                    push_edge(edges.as_mut(), edge, budget, limitations);
                }
            }
        }
        if targets.is_empty() {
            add_limitation(
                limitations,
                "rust-resolver-import-unresolved",
                Some(item.path.clone()),
                None,
            );
        }
    }
    for (module_id, count) in glob_counts {
        if count > 1 {
            let path = module_by_id
                .get(&module_id)
                .map(|module| module.graph.path.clone());
            add_limitation(
                limitations,
                "rust-resolver-glob-import-ambiguous",
                path,
                None,
            );
        }
    }
    ImportResolution {
        bindings,
        exports,
        glob_modules,
        edges,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_call_edges(
    facts_by_path: &BTreeMap<RepoPath, &RustRepositoryFileFacts>,
    file_modules: &BTreeMap<RepoPath, Vec<String>>,
    inline_modules: &BTreeMap<(RepoPath, String), String>,
    module_by_id: &BTreeMap<String, ModuleState>,
    symbols: &[GraphSymbol],
    local_symbol_ids: &BTreeMap<(RepoPath, String), String>,
    call_lookup: &CallLookup<'_>,
    budget: &mut IndexBudgetTracker,
    limitations: &mut Vec<IndexLimitation>,
    edges: &mut Vec<GraphEdge>,
) {
    for (path, file) in facts_by_path {
        let facts_by_local = file
            .facts
            .symbols
            .iter()
            .map(|symbol| (symbol.local_id.as_str(), symbol))
            .collect::<BTreeMap<_, _>>();
        let mut calls = file.facts.calls.iter().collect::<Vec<_>>();
        calls.sort_by(|left, right| {
            left.range
                .start_byte
                .cmp(&right.range.start_byte)
                .then_with(|| left.callee.cmp(&right.callee))
        });
        for call in calls {
            if budget.check_deadline().is_err() {
                add_limitation(limitations, "index-deadline-exhausted", None, None);
                return;
            }
            let source_symbol = call
                .caller_local_id
                .as_ref()
                .and_then(|local_id| local_symbol_ids.get(&(path.clone(), local_id.clone())))
                .cloned();
            let source_module_id = call
                .caller_local_id
                .as_ref()
                .and_then(|local_id| facts_by_local.get(local_id.as_str()).copied())
                .and_then(|fact| {
                    symbol_module_id(path, fact, &facts_by_local, file_modules, inline_modules)
                })
                .or_else(|| {
                    file_modules
                        .get(path)
                        .and_then(|modules| modules.first())
                        .cloned()
                });
            let Some(source_module_id) = source_module_id else {
                continue;
            };
            let source_symbol =
                source_symbol.or_else(|| first_module_symbol(&source_module_id, symbols));
            let Some(source_symbol) = source_symbol else {
                continue;
            };
            let Some(source_module) = module_by_id.get(&source_module_id) else {
                continue;
            };

            if call.call_kind == "method" {
                add_unresolved_call(
                    edges,
                    limitations,
                    budget,
                    path,
                    call,
                    &source_symbol,
                    "rust-resolver-method-call-unresolved",
                    Resolution::PolymorphicCandidate,
                );
                continue;
            }
            if call.call_kind == "macro" {
                add_unresolved_call(
                    edges,
                    limitations,
                    budget,
                    path,
                    call,
                    &source_symbol,
                    "rust-resolver-macro-call-unresolved",
                    Resolution::Unresolved,
                );
                continue;
            }

            let targets =
                resolve_name_targets(source_module, &call.qualifier, &call.callee, call_lookup);
            if targets.len() == 1 {
                let target = targets.iter().next().cloned().unwrap();
                let call_edge = make_edge(
                    EdgeKind::Calls,
                    &source_symbol,
                    Some(target.clone()),
                    None,
                    path,
                    &call.range,
                    Resolution::ResolvedReference,
                    Confidence::Medium,
                    None,
                );
                push_edge(edges, call_edge, budget, limitations);
                let reference_edge = make_edge(
                    EdgeKind::References,
                    &source_symbol,
                    Some(target),
                    None,
                    path,
                    &call.range,
                    Resolution::ResolvedReference,
                    Confidence::Medium,
                    None,
                );
                push_edge(edges, reference_edge, budget, limitations);
            } else if targets.len() > 1 {
                add_unresolved_call(
                    edges,
                    limitations,
                    budget,
                    path,
                    call,
                    &source_symbol,
                    "rust-resolver-call-polymorphic",
                    Resolution::PolymorphicCandidate,
                );
            } else if !is_external_or_builtin(call) {
                add_unresolved_call(
                    edges,
                    limitations,
                    budget,
                    path,
                    call,
                    &source_symbol,
                    "rust-resolver-call-unresolved",
                    Resolution::Unresolved,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_reference_edges(
    facts_by_path: &BTreeMap<RepoPath, &RustRepositoryFileFacts>,
    file_modules: &BTreeMap<RepoPath, Vec<String>>,
    inline_modules: &BTreeMap<(RepoPath, String), String>,
    module_by_id: &BTreeMap<String, ModuleState>,
    local_symbol_ids: &BTreeMap<(RepoPath, String), String>,
    lookup: &CallLookup<'_>,
    budget: &mut IndexBudgetTracker,
    limitations: &mut Vec<IndexLimitation>,
    edges: &mut Vec<GraphEdge>,
) {
    for (path, file) in facts_by_path {
        let facts_by_local = file
            .facts
            .symbols
            .iter()
            .map(|symbol| (symbol.local_id.as_str(), symbol))
            .collect::<BTreeMap<_, _>>();
        let mut references = file.facts.references.iter().collect::<Vec<_>>();
        references.sort_by(|left, right| {
            left.range
                .start_byte
                .cmp(&right.range.start_byte)
                .then_with(|| left.name.cmp(&right.name))
        });
        for reference in references {
            let Some(owner_local_id) = &reference.owner_local_id else {
                continue;
            };
            let Some(source_symbol) = local_symbol_ids
                .get(&(path.clone(), owner_local_id.clone()))
                .cloned()
            else {
                continue;
            };
            let Some(owner_fact) = facts_by_local.get(owner_local_id.as_str()).copied() else {
                continue;
            };
            let Some(source_module_id) = symbol_module_id(
                path,
                owner_fact,
                &facts_by_local,
                file_modules,
                inline_modules,
            ) else {
                continue;
            };
            let Some(source_module) = module_by_id.get(&source_module_id) else {
                continue;
            };
            let targets =
                resolve_name_targets(source_module, &reference.qualifier, &reference.name, lookup);
            if targets.len() == 1 {
                let target = targets.iter().next().cloned().unwrap();
                let edge = make_edge(
                    EdgeKind::References,
                    &source_symbol,
                    Some(target),
                    None,
                    path,
                    &reference.range,
                    Resolution::ResolvedReference,
                    Confidence::Medium,
                    None,
                );
                push_edge(edges, edge, budget, limitations);
            } else if targets.len() > 1 {
                add_limitation(
                    limitations,
                    "rust-resolver-reference-polymorphic",
                    Some(path.clone()),
                    Some(source_symbol.clone()),
                );
                let edge = make_edge(
                    EdgeKind::References,
                    &source_symbol,
                    None,
                    Some(reference.name.clone()),
                    path,
                    &reference.range,
                    Resolution::PolymorphicCandidate,
                    Confidence::Low,
                    Some("rust-resolver-reference-polymorphic".to_string()),
                );
                push_edge(edges, edge, budget, limitations);
            }
        }
    }
}

fn resolve_name_targets(
    source_module: &ModuleState,
    qualifier: &[String],
    name: &str,
    lookup: &CallLookup<'_>,
) -> BTreeSet<String> {
    let mut targets = BTreeSet::new();
    if qualifier.is_empty() {
        if let Some(bound) = lookup
            .bindings
            .get(&(source_module.graph.module_id.clone(), name.to_string()))
        {
            for target in bound {
                if let BindingTarget::Symbol(symbol) = target {
                    targets.insert(symbol.clone());
                }
            }
        }
        let mut local = source_module.logical_path.clone();
        local.push(name.to_string());
        extend_symbol_targets(
            &mut targets,
            &local,
            lookup.symbols_by_logical,
            lookup.exports,
        );
        if let Some(globs) = lookup.glob_modules.get(&source_module.graph.module_id) {
            for module in globs {
                let mut path = module.clone();
                path.push(name.to_string());
                extend_symbol_targets(
                    &mut targets,
                    &path,
                    lookup.symbols_by_logical,
                    lookup.exports,
                );
            }
        }
        return targets;
    }

    if let Some(bound) = lookup
        .bindings
        .get(&(source_module.graph.module_id.clone(), qualifier[0].clone()))
    {
        for target in bound {
            let mut path = match target {
                BindingTarget::Module(path) => path.clone(),
                BindingTarget::Symbol(symbol) => {
                    let Some(path) = lookup.symbol_logical.get(symbol) else {
                        continue;
                    };
                    path.clone()
                }
            };
            path.extend(qualifier.iter().skip(1).cloned());
            path.push(name.to_string());
            extend_symbol_targets(
                &mut targets,
                &path,
                lookup.symbols_by_logical,
                lookup.exports,
            );
        }
    }
    let mut full = qualifier.to_vec();
    full.push(name.to_string());
    for path in absolute_paths(source_module, &full, lookup.crate_names) {
        extend_symbol_targets(
            &mut targets,
            &path,
            lookup.symbols_by_logical,
            lookup.exports,
        );
    }
    targets
}

fn resolve_path_targets(
    source_module: &ModuleState,
    segments: &[String],
    modules_by_logical: &BTreeMap<Vec<String>, Vec<String>>,
    symbols_by_logical: &BTreeMap<Vec<String>, Vec<String>>,
    crate_names: &BTreeSet<String>,
    exports: &BTreeMap<Vec<String>, BTreeSet<BindingTarget>>,
) -> BTreeSet<BindingTarget> {
    let mut targets = BTreeSet::new();
    for path in absolute_paths(source_module, segments, crate_names) {
        if modules_by_logical.contains_key(&path) {
            targets.insert(BindingTarget::Module(path.clone()));
        }
        if let Some(symbols) = symbols_by_logical.get(&path) {
            targets.extend(symbols.iter().cloned().map(BindingTarget::Symbol));
        }
        if let Some(exported) = exports.get(&path) {
            targets.extend(exported.iter().cloned());
        }
    }
    targets
}

fn absolute_paths(
    source_module: &ModuleState,
    segments: &[String],
    crate_names: &BTreeSet<String>,
) -> Vec<Vec<String>> {
    if segments.is_empty() {
        return Vec::new();
    }
    let mut paths = BTreeSet::new();
    match segments[0].as_str() {
        "crate" => {
            let mut path = vec![source_module.graph.crate_name.clone()];
            path.extend(segments.iter().skip(1).cloned());
            paths.insert(path);
        }
        "self" => {
            let mut path = source_module.logical_path.clone();
            path.extend(segments.iter().skip(1).cloned());
            paths.insert(path);
        }
        "super" => {
            let mut path = source_module.logical_path.clone();
            let mut index = 0;
            while segments
                .get(index)
                .is_some_and(|segment| segment == "super")
            {
                if path.len() > 1 {
                    path.pop();
                }
                index += 1;
            }
            path.extend(segments.iter().skip(index).cloned());
            paths.insert(path);
        }
        first if crate_names.contains(first) => {
            paths.insert(segments.to_vec());
        }
        _ => {
            let mut relative = source_module.logical_path.clone();
            relative.extend(segments.iter().cloned());
            paths.insert(relative);
            let mut crate_relative = vec![source_module.graph.crate_name.clone()];
            crate_relative.extend(segments.iter().cloned());
            paths.insert(crate_relative);
        }
    }
    paths.into_iter().collect()
}

fn extend_symbol_targets(
    targets: &mut BTreeSet<String>,
    path: &[String],
    symbols_by_logical: &BTreeMap<Vec<String>, Vec<String>>,
    exports: &BTreeMap<Vec<String>, BTreeSet<BindingTarget>>,
) {
    if let Some(symbols) = symbols_by_logical.get(path) {
        targets.extend(symbols.iter().cloned());
    }
    if let Some(exported) = exports.get(path) {
        for target in exported {
            if let BindingTarget::Symbol(symbol) = target {
                targets.insert(symbol.clone());
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn add_unresolved_call(
    edges: &mut Vec<GraphEdge>,
    limitations: &mut Vec<IndexLimitation>,
    budget: &mut IndexBudgetTracker,
    path: &RepoPath,
    call: &RustCallSiteFact,
    source_symbol: &str,
    code: &str,
    resolution: Resolution,
) {
    add_limitation(
        limitations,
        code,
        Some(path.clone()),
        Some(source_symbol.to_string()),
    );
    let edge = make_edge(
        EdgeKind::Calls,
        source_symbol,
        None,
        Some(call.callee.clone()),
        path,
        &call.range,
        resolution,
        Confidence::Low,
        Some(code.to_string()),
    );
    push_edge(edges, edge, budget, limitations);
}

fn push_edge(
    edges: &mut Vec<GraphEdge>,
    edge: GraphEdge,
    budget: &mut IndexBudgetTracker,
    limitations: &mut Vec<IndexLimitation>,
) {
    match budget.consume(IndexResource::Edges, 1) {
        Ok(()) => edges.push(edge),
        Err(exhaustion) => add_limitation(limitations, exhaustion.code(), None, None),
    }
}

#[allow(clippy::too_many_arguments)]
fn make_edge(
    kind: EdgeKind,
    from_symbol: &str,
    to_symbol: Option<String>,
    unresolved_target: Option<String>,
    path: &RepoPath,
    range: &SourceRange,
    resolution: Resolution,
    confidence: Confidence,
    limitation_code: Option<String>,
) -> GraphEdge {
    let mut digest = Sha256::new();
    hash_component(&mut digest, b"rust-repository-edge/v1");
    hash_component(&mut digest, edge_kind_name(kind).as_bytes());
    hash_component(&mut digest, from_symbol.as_bytes());
    hash_component(&mut digest, to_symbol.as_deref().unwrap_or("").as_bytes());
    hash_component(
        &mut digest,
        unresolved_target.as_deref().unwrap_or("").as_bytes(),
    );
    hash_component(&mut digest, path.as_str().as_bytes());
    hash_component(&mut digest, &range.start_byte.to_be_bytes());
    hash_component(&mut digest, &range.end_byte.to_be_bytes());
    GraphEdge {
        edge_id: format!("{:x}", digest.finalize()),
        kind,
        from_symbol: from_symbol.to_string(),
        to_symbol,
        unresolved_target,
        path: path.clone(),
        range: range.clone(),
        provider_id: PROVIDER_ID.to_string(),
        provider_version: PROVIDER_VERSION.to_string(),
        resolution,
        confidence,
        limitation_code,
    }
}

fn module_local_id(
    facts: &RustFileFacts,
    declaration: &RustModuleDeclarationFact,
) -> Option<String> {
    facts
        .symbols
        .iter()
        .find(|symbol| {
            symbol.kind == "module"
                && symbol.name == declaration.name
                && symbol.owner_local_id == declaration.owner_local_id
                && symbol.range == declaration.range
        })
        .or_else(|| {
            facts.symbols.iter().find(|symbol| {
                symbol.kind == "module"
                    && symbol.name == declaration.name
                    && symbol.owner_local_id == declaration.owner_local_id
            })
        })
        .map(|symbol| symbol.local_id.clone())
}

fn module_file_path(
    source_path: &RepoPath,
    declaration: &RustModuleDeclarationFact,
    manifest_paths: &BTreeSet<RepoPath>,
) -> Option<RepoPath> {
    let directory = source_path
        .as_str()
        .rsplit_once('/')
        .map(|(directory, _)| directory)
        .unwrap_or("");
    let candidates = if let Some(path) = &declaration.path_override {
        vec![join_repo_path(directory, path)?]
    } else {
        vec![
            join_repo_path(directory, &format!("{}.rs", declaration.name))?,
            join_repo_path(directory, &format!("{}/mod.rs", declaration.name))?,
        ]
    };
    candidates
        .into_iter()
        .find(|candidate| manifest_paths.contains(candidate))
}

fn join_repo_path(directory: &str, relative: &str) -> Option<RepoPath> {
    let value = if directory.is_empty() {
        relative.to_string()
    } else {
        format!("{directory}/{relative}")
    };
    RepoPath::new(value).ok()
}

fn symbol_module_id(
    path: &RepoPath,
    symbol: &RustLocalSymbolFact,
    facts_by_local: &BTreeMap<&str, &RustLocalSymbolFact>,
    file_modules: &BTreeMap<RepoPath, Vec<String>>,
    inline_modules: &BTreeMap<(RepoPath, String), String>,
) -> Option<String> {
    let mut owner = symbol.owner_local_id.as_deref();
    while let Some(local_id) = owner {
        if let Some(module_id) = inline_modules.get(&(path.clone(), local_id.to_string())) {
            return Some(module_id.clone());
        }
        owner = facts_by_local
            .get(local_id)
            .and_then(|owner_symbol| owner_symbol.owner_local_id.as_deref());
    }
    file_modules
        .get(path)
        .and_then(|modules| modules.first())
        .cloned()
}

fn module_for_range(
    path: &RepoPath,
    range: &SourceRange,
    file_modules: &BTreeMap<RepoPath, Vec<String>>,
    modules: &[ModuleState],
) -> Option<String> {
    modules
        .iter()
        .filter(|module| {
            module.graph.path == *path
                && module.graph.inline
                && module
                    .declaration_range
                    .as_ref()
                    .is_some_and(|module_range| range_contains(module_range, range))
        })
        .min_by_key(|module| {
            module
                .declaration_range
                .as_ref()
                .map(|module_range| {
                    module_range
                        .end_byte
                        .saturating_sub(module_range.start_byte)
                })
                .unwrap_or(usize::MAX)
        })
        .map(|module| module.graph.module_id.clone())
        .or_else(|| file_modules.get(path).and_then(|ids| ids.first()).cloned())
}

fn enclosing_symbol(
    path: &RepoPath,
    module_id: &str,
    range: &SourceRange,
    symbols: &[GraphSymbol],
) -> Option<String> {
    symbols
        .iter()
        .filter(|symbol| {
            symbol.path == *path
                && symbol.module_id == module_id
                && range_contains(&symbol.range, range)
        })
        .min_by_key(|symbol| {
            symbol
                .range
                .end_byte
                .saturating_sub(symbol.range.start_byte)
        })
        .map(|symbol| symbol.symbol_id.clone())
}

fn first_module_symbol(module_id: &str, symbols: &[GraphSymbol]) -> Option<String> {
    symbols
        .iter()
        .filter(|symbol| symbol.module_id == module_id)
        .min_by(|left, right| {
            left.range
                .start_byte
                .cmp(&right.range.start_byte)
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        })
        .map(|symbol| symbol.symbol_id.clone())
}

fn range_contains(outer: &SourceRange, inner: &SourceRange) -> bool {
    outer.start_byte <= inner.start_byte && inner.end_byte <= outer.end_byte
}

fn module_id(parent: Option<&str>, name: &str, path: &RepoPath, inline: bool) -> String {
    let mut digest = Sha256::new();
    hash_component(&mut digest, b"rust-repository-module/v1");
    hash_component(&mut digest, parent.unwrap_or("").as_bytes());
    hash_component(&mut digest, name.as_bytes());
    hash_component(&mut digest, path.as_str().as_bytes());
    hash_component(&mut digest, &[u8::from(inline)]);
    format!("{:x}", digest.finalize())
}

fn symbol_id(module_id: &str, path: &RepoPath, local_id: &str) -> String {
    let mut digest = Sha256::new();
    hash_component(&mut digest, b"rust-repository-symbol/v1");
    hash_component(&mut digest, module_id.as_bytes());
    hash_component(&mut digest, path.as_str().as_bytes());
    hash_component(&mut digest, local_id.as_bytes());
    format!("{:x}", digest.finalize())
}

fn is_external_or_builtin(call: &RustCallSiteFact) -> bool {
    matches!(
        call.callee.as_str(),
        "assert" | "assert_eq" | "assert_ne" | "format" | "println" | "vec"
    )
}

fn add_limitation(
    limitations: &mut Vec<IndexLimitation>,
    code: &str,
    path: Option<RepoPath>,
    symbol_id: Option<String>,
) {
    let (reason, interpretation) = limitation_text(code);
    let limitation = IndexLimitation {
        code: code.to_string(),
        path,
        symbol_id,
        reason: reason.to_string(),
        interpretation: interpretation.to_string(),
    };
    if !limitations.contains(&limitation) {
        limitations.push(limitation);
    }
}

fn limitation_text(code: &str) -> (&'static str, &'static str) {
    match code {
        "rust-resolver-glob-import-ambiguous" => (
            "multiple glob imports can bind the same name",
            "matching references remain polymorphic candidates",
        ),
        "rust-resolver-method-call-unresolved" => (
            "method dispatch requires type and trait information",
            "the call is syntactic and is not a confirmed target",
        ),
        "rust-resolver-macro-call-unresolved" => (
            "macro expansion is outside the passive resolver",
            "the call is recorded without claiming an expanded target",
        ),
        "rust-resolver-cfg-conditional" => (
            "conditional compilation can change the visible graph",
            "relationships are valid only for the parsed candidate text",
        ),
        _ => (
            "repository resolution was bounded or incomplete",
            "the graph preserves available evidence without claiming completeness",
        ),
    }
}

fn limitation_order(left: &IndexLimitation, right: &IndexLimitation) -> std::cmp::Ordering {
    (
        left.code.as_str(),
        left.path.as_ref().map(RepoPath::as_str).unwrap_or(""),
        left.symbol_id.as_deref().unwrap_or(""),
        left.reason.as_str(),
        left.interpretation.as_str(),
    )
        .cmp(&(
            right.code.as_str(),
            right.path.as_ref().map(RepoPath::as_str).unwrap_or(""),
            right.symbol_id.as_deref().unwrap_or(""),
            right.reason.as_str(),
            right.interpretation.as_str(),
        ))
}

fn edge_kind_name(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Defines => "defines",
        EdgeKind::References => "references",
        EdgeKind::Imports => "imports",
        EdgeKind::Exports => "exports",
        EdgeKind::Calls => "calls",
        EdgeKind::Implements => "implements",
        EdgeKind::Overrides => "overrides",
    }
}

fn hash_component(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}
