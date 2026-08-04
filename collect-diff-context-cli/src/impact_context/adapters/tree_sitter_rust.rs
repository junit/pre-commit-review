use crate::candidate::ChangedRange;
use crate::impact_context::budget::{BudgetResource, BudgetTracker};
use crate::impact_context::contracts::{ParseQuality, Resolution, SourceRange};
use crate::impact_context::index::budget::{IndexBudgetTracker, IndexResource};
use crate::impact_context::normalizer::stable_local_symbol_id;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ops::ControlFlow;
use tree_sitter::{
    Node, ParseOptions, ParseState, Parser, Query, QueryCursor, QueryCursorOptions,
    QueryCursorState, StreamingIterator, Tree,
};

const RUST_FACT_QUERY: &str = r#"
(function_item name: (identifier) @definition.function)
(function_signature_item name: (identifier) @declaration.function)
(struct_item name: (type_identifier) @definition.struct)
(enum_item name: (type_identifier) @definition.enum)
(trait_item name: (type_identifier) @definition.trait)
(impl_item type: (_) @definition.impl.type) @definition.impl
(type_item name: (type_identifier) @definition.type)
(const_item name: (identifier) @definition.const)
(static_item name: (identifier) @definition.static)
(mod_item name: (identifier) @definition.module)
(closure_expression) @definition.closure
(use_declaration argument: (_) @import)
(call_expression function: (_) @call)
(macro_invocation macro: (_) @macro)
(attribute_item) @attribute
"#;

const RUST_REFERENCE_QUERY: &str = r#"
(scoped_identifier) @reference.path
(scoped_type_identifier) @reference.type_path
(identifier) @reference.identifier
(type_identifier) @reference.type
(field_identifier) @reference.field
"#;

const MAX_ATTRIBUTE_ARGUMENTS: usize = 64;
const MAX_FACT_TEXT_CHARS: usize = 1_000;
const MAX_PATH_SEGMENTS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RustSymbolFact {
    pub kind: String,
    pub name: String,
    pub owner: Option<String>,
    pub signature: String,
    pub visibility: Option<String>,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RustTextFact {
    pub text: String,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RustCallFact {
    pub target: String,
    pub caller_range: Option<SourceRange>,
    pub range: SourceRange,
    pub resolution: Resolution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RustSyntaxOutput {
    pub parse_quality: ParseQuality,
    pub error_node_count: usize,
    pub missing_node_count: usize,
    pub affected_ranges: Vec<SourceRange>,
    pub overlapping_changed_symbols: Vec<String>,
    pub changed_symbols: Vec<RustSymbolFact>,
    pub imports: Vec<RustTextFact>,
    pub calls: Vec<RustCallFact>,
    pub macros: Vec<RustTextFact>,
    pub attributes: Vec<RustTextFact>,
    pub nodes_visited: usize,
    pub max_nesting_depth: usize,
    pub limitation_codes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustLocalSymbolFact {
    pub local_id: String,
    pub kind: String,
    pub name: String,
    pub owner_local_id: Option<String>,
    pub signature: String,
    pub visibility: Option<String>,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustImportFact {
    pub segments: Vec<String>,
    pub alias: Option<String>,
    pub glob: bool,
    pub public: bool,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustReferenceFact {
    pub name: String,
    pub qualifier: Vec<String>,
    pub role: String,
    pub owner_local_id: Option<String>,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustCallSiteFact {
    pub callee: String,
    pub qualifier: Vec<String>,
    pub call_kind: String,
    pub caller_local_id: Option<String>,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustAttributeFact {
    pub name: String,
    pub arguments: Vec<String>,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustModuleDeclarationFact {
    pub name: String,
    pub inline: bool,
    pub path_override: Option<String>,
    pub owner_local_id: Option<String>,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustFileFactMetrics {
    pub nodes_visited: usize,
    pub max_nesting_depth: usize,
    pub facts_emitted: usize,
    pub source_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustFileFacts {
    pub parse_quality: ParseQuality,
    pub symbols: Vec<RustLocalSymbolFact>,
    pub imports: Vec<RustImportFact>,
    pub references: Vec<RustReferenceFact>,
    pub calls: Vec<RustCallSiteFact>,
    pub module_declarations: Vec<RustModuleDeclarationFact>,
    pub attributes: Vec<RustAttributeFact>,
    pub recovery_ranges: Vec<SourceRange>,
    pub limitations: Vec<String>,
    pub metrics: RustFileFactMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustAdapterError {
    message: String,
}

impl RustAdapterError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RustAdapterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RustAdapterError {}

pub struct TreeSitterRustAdapter;

impl TreeSitterRustAdapter {
    pub fn analyze(
        source: &[u8],
        changed_ranges: &[ChangedRange],
        budget: &mut BudgetTracker,
    ) -> Result<RustSyntaxOutput, RustAdapterError> {
        budget
            .check_deadline()
            .map_err(|exhaustion| RustAdapterError::new(exhaustion.code()))?;
        let (_language, mut parser, query) = rust_parser_and_query(RUST_FACT_QUERY)?;
        let tree = {
            let mut parse_progress = |_: &ParseState| {
                if budget.check_deadline().is_err() {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            };
            parse_rust_source(&mut parser, source, &mut parse_progress)
        };
        let tree = tree.ok_or_else(|| {
            if budget.deadline_exhausted() {
                RustAdapterError::new("deadline-exhausted")
            } else {
                RustAdapterError::new("Tree-sitter returned no Rust syntax tree")
            }
        })?;

        let mut limitation_codes = Vec::new();
        let traversal = traverse_recovery(tree.root_node(), |depth| {
            budget
                .check_deadline()
                .map_err(|exhaustion| exhaustion.code())?;
            budget
                .consume(BudgetResource::Nodes, 1)
                .map_err(|exhaustion| exhaustion.code())?;
            budget
                .observe(BudgetResource::NestingDepth, depth)
                .map_err(|exhaustion| exhaustion.code())
        });
        if let Some(code) = traversal.limitation {
            push_unique(&mut limitation_codes, code);
        }
        let errors = traversal.recovery_ranges;
        let error_node_count = traversal.error_node_count;
        let missing_node_count = traversal.missing_node_count;
        let nodes_visited = traversal.nodes_visited;
        let max_nesting_depth = traversal.max_nesting_depth;
        let traversal_complete = traversal.limitation.is_none();

        let mut captures = Vec::new();
        if traversal_complete {
            let capture_names = query.capture_names();
            let mut cursor = QueryCursor::new();
            cursor.set_match_limit(65_536);
            {
                let mut query_progress = |_: &QueryCursorState| {
                    if budget.check_deadline().is_err() {
                        ControlFlow::Break(())
                    } else {
                        ControlFlow::Continue(())
                    }
                };
                let options = QueryCursorOptions::new().progress_callback(&mut query_progress);
                let mut matches =
                    cursor.matches_with_options(&query, tree.root_node(), source, options);
                while let Some(query_match) = matches.next() {
                    for capture in query_match.captures {
                        captures.push((capture_names[capture.index as usize], capture.node));
                    }
                }
            }
            if budget.deadline_exhausted() {
                push_unique(&mut limitation_codes, "deadline-exhausted");
            }
            if cursor.did_exceed_match_limit() {
                push_unique(&mut limitation_codes, "tree-sitter-query-match-limit");
            }
        }

        let mut changed_symbols = Vec::new();
        for (capture, node) in captures.iter().copied() {
            if budget.check_deadline().is_err() {
                push_unique(&mut limitation_codes, "deadline-exhausted");
                break;
            }
            let Some(mut symbol) = symbol_from_capture(capture, node, source) else {
                continue;
            };
            if !node_intersects_changes(symbol.range.clone(), changed_ranges) {
                continue;
            }
            if budget.consume(BudgetResource::Facts, 1).is_err() {
                push_unique(&mut limitation_codes, "fact-budget-exhausted");
                break;
            }
            if symbol.kind == "function" && symbol.owner.is_some() {
                symbol.kind = "method".to_string();
            }
            changed_symbols.push(symbol);
        }
        changed_symbols.sort_by(symbol_order);
        changed_symbols.dedup_by(|left, right| {
            left.kind == right.kind
                && left.name == right.name
                && left.owner == right.owner
                && left.range == right.range
        });

        let mut imports = Vec::new();
        let mut calls = Vec::new();
        let mut macros = Vec::new();
        let mut attributes = Vec::new();
        for (capture, node) in captures.iter().copied() {
            if budget.check_deadline().is_err() {
                push_unique(&mut limitation_codes, "deadline-exhausted");
                break;
            }
            let limitation_code = match capture {
                "import" => (!push_text_fact(&mut imports, node, source, budget))
                    .then_some("fact-budget-exhausted"),
                "call" => {
                    let range = source_range(node);
                    let caller_range = innermost_caller(&changed_symbols, &range);
                    if caller_range.is_none()
                        && !node_intersects_changes(range.clone(), changed_ranges)
                    {
                        None
                    } else if budget.consume(BudgetResource::Facts, 1).is_err() {
                        Some("fact-budget-exhausted")
                    } else if calls.len() >= budget.budget().max_edges {
                        Some("edge-budget-exhausted")
                    } else {
                        calls.push(RustCallFact {
                            target: bounded_node_text(node, source),
                            caller_range,
                            range,
                            resolution: Resolution::Unresolved,
                        });
                        None
                    }
                }
                "macro" => (!push_text_fact(&mut macros, node, source, budget))
                    .then_some("fact-budget-exhausted"),
                "attribute" => (!push_text_fact(&mut attributes, node, source, budget))
                    .then_some("fact-budget-exhausted"),
                _ => None,
            };
            if let Some(code) = limitation_code {
                push_unique(&mut limitation_codes, code);
                break;
            }
        }
        sort_dedup_text_facts(&mut imports);
        sort_dedup_text_facts(&mut macros);
        sort_dedup_text_facts(&mut attributes);
        calls.sort_by_key(|call| range_key(&call.range));
        calls.dedup_by(|left, right| {
            left.target == right.target
                && left.caller_range == right.caller_range
                && left.range == right.range
        });

        let overlaps_change = errors
            .iter()
            .any(|range| node_intersects_changes(range.clone(), changed_ranges));
        let parse_quality = if errors.is_empty() {
            ParseQuality::Clean
        } else if overlaps_change {
            push_unique(
                &mut limitation_codes,
                "syntax-recovery-overlaps-changed-structure",
            );
            ParseQuality::Degraded
        } else {
            push_unique(
                &mut limitation_codes,
                "syntax-recovery-outside-changed-structure",
            );
            ParseQuality::Recovered
        };
        let mut overlapping_changed_symbols = changed_symbols
            .iter()
            .filter(|symbol| {
                errors
                    .iter()
                    .any(|range| ranges_overlap(&symbol.range, range))
            })
            .map(symbol_display_name)
            .collect::<Vec<_>>();
        overlapping_changed_symbols.sort();
        overlapping_changed_symbols.dedup();
        limitation_codes.sort();

        Ok(RustSyntaxOutput {
            parse_quality,
            error_node_count,
            missing_node_count,
            affected_ranges: errors,
            overlapping_changed_symbols,
            changed_symbols,
            imports,
            calls,
            macros,
            attributes,
            nodes_visited,
            max_nesting_depth,
            limitation_codes,
        })
    }

    pub fn analyze_index(
        source: &[u8],
        budget: &mut IndexBudgetTracker,
    ) -> Result<RustFileFacts, RustAdapterError> {
        let mut limitations = Vec::new();
        if let Err(exhaustion) = budget.check_deadline() {
            push_unique(&mut limitations, exhaustion.code());
            return Ok(empty_index_facts(source.len(), limitations));
        }
        if let Err(exhaustion) = budget.observe(IndexResource::FileBytes, source.len()) {
            push_unique(&mut limitations, exhaustion.code());
            return Ok(empty_index_facts(source.len(), limitations));
        }
        if let Err(exhaustion) = budget.consume(IndexResource::ParseBytes, source.len()) {
            push_unique(&mut limitations, exhaustion.code());
            return Ok(empty_index_facts(source.len(), limitations));
        }

        let (language, mut parser, query) = rust_parser_and_query(RUST_FACT_QUERY)?;
        let tree = {
            let mut parse_progress = |_: &ParseState| {
                if budget.check_deadline().is_err() {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            };
            parse_rust_source(&mut parser, source, &mut parse_progress)
        };
        let Some(tree) = tree else {
            if budget.check_deadline().is_err() {
                push_unique(&mut limitations, "index-deadline-exhausted");
                return Ok(empty_index_facts(source.len(), limitations));
            }
            return Err(RustAdapterError::new(
                "Tree-sitter returned no Rust syntax tree",
            ));
        };

        let traversal = traverse_recovery(tree.root_node(), |_| {
            budget
                .check_deadline()
                .map_err(|exhaustion| exhaustion.code())?;
            budget
                .consume(IndexResource::Nodes, 1)
                .map_err(|exhaustion| exhaustion.code())
        });
        if let Some(code) = traversal.limitation {
            push_unique(&mut limitations, code);
        }
        let mut recovery_ranges = traversal.recovery_ranges;
        sort_dedup_ranges(&mut recovery_ranges);
        if traversal.limitation.is_some() {
            return Ok(RustFileFacts {
                parse_quality: ParseQuality::Degraded,
                symbols: Vec::new(),
                imports: Vec::new(),
                references: Vec::new(),
                calls: Vec::new(),
                module_declarations: Vec::new(),
                attributes: Vec::new(),
                recovery_ranges,
                limitations: sorted_unique(limitations),
                metrics: RustFileFactMetrics {
                    nodes_visited: traversal.nodes_visited,
                    max_nesting_depth: traversal.max_nesting_depth,
                    facts_emitted: 0,
                    source_bytes: source.len(),
                },
            });
        }

        let captures = collect_index_captures(&query, &tree, source, budget, &mut limitations);
        let all_symbols = build_index_symbols(&captures, source);
        let import_candidates = build_import_facts(&captures, source);
        let module_candidates = build_module_facts(&captures, &all_symbols, source);
        let attribute_candidates = build_attribute_facts(&captures, source);
        let call_candidates = build_call_facts(&captures, &all_symbols, source);
        let reference_candidates = build_reference_facts(
            &language,
            &tree,
            source,
            budget,
            &all_symbols,
            &mut limitations,
        )?;

        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        let mut references = Vec::new();
        let mut calls = Vec::new();
        let mut module_declarations = Vec::new();
        let mut attributes = Vec::new();
        let mut facts_available =
            append_index_facts(&mut symbols, all_symbols, budget, &mut limitations);
        if facts_available {
            facts_available =
                append_index_facts(&mut imports, import_candidates, budget, &mut limitations);
        }
        if facts_available {
            facts_available = append_index_facts(
                &mut module_declarations,
                module_candidates,
                budget,
                &mut limitations,
            );
        }
        if facts_available {
            facts_available = append_index_facts(
                &mut attributes,
                attribute_candidates,
                budget,
                &mut limitations,
            );
        }
        if facts_available {
            facts_available =
                append_index_facts(&mut calls, call_candidates, budget, &mut limitations);
        }
        if facts_available {
            append_index_facts(
                &mut references,
                reference_candidates,
                budget,
                &mut limitations,
            );
        }

        sort_index_facts(
            &mut symbols,
            &mut imports,
            &mut references,
            &mut calls,
            &mut module_declarations,
            &mut attributes,
        );
        let limitations = sorted_unique(limitations);
        let facts_emitted = symbols
            .len()
            .saturating_add(imports.len())
            .saturating_add(references.len())
            .saturating_add(calls.len())
            .saturating_add(module_declarations.len())
            .saturating_add(attributes.len());
        let parse_quality = if !limitations.is_empty() {
            ParseQuality::Degraded
        } else if recovery_ranges.is_empty() {
            ParseQuality::Clean
        } else {
            ParseQuality::Recovered
        };

        Ok(RustFileFacts {
            parse_quality,
            symbols,
            imports,
            references,
            calls,
            module_declarations,
            attributes,
            recovery_ranges,
            limitations,
            metrics: RustFileFactMetrics {
                nodes_visited: traversal.nodes_visited,
                max_nesting_depth: traversal.max_nesting_depth,
                facts_emitted,
                source_bytes: source.len(),
            },
        })
    }
}

struct RecoveryTraversal {
    recovery_ranges: Vec<SourceRange>,
    error_node_count: usize,
    missing_node_count: usize,
    nodes_visited: usize,
    max_nesting_depth: usize,
    limitation: Option<&'static str>,
}

#[derive(Clone, Copy)]
struct IndexCapture<'tree> {
    name: &'static str,
    node: Node<'tree>,
}

struct RawIndexSymbol {
    base: RustSymbolFact,
    has_self_parameter: bool,
}

fn rust_parser_and_query(
    query_source: &str,
) -> Result<(tree_sitter::Language, Parser, Query), RustAdapterError> {
    let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|error| RustAdapterError::new(format!("cannot load Rust grammar: {error}")))?;
    let query = Query::new(&language, query_source)
        .map_err(|error| RustAdapterError::new(format!("cannot compile Rust query: {error}")))?;
    Ok((language, parser, query))
}

fn parse_rust_source<F>(parser: &mut Parser, source: &[u8], progress: &mut F) -> Option<Tree>
where
    F: FnMut(&ParseState) -> ControlFlow<()>,
{
    let mut read_source = |offset: usize, _| source.get(offset..).unwrap_or_default();
    parser.parse_with_options(
        &mut read_source,
        None,
        Some(ParseOptions::new().progress_callback(progress)),
    )
}

fn traverse_recovery<F>(root: Node<'_>, mut visit: F) -> RecoveryTraversal
where
    F: FnMut(usize) -> Result<(), &'static str>,
{
    let mut recovery_ranges = Vec::new();
    let mut error_node_count = 0;
    let mut missing_node_count = 0;
    let mut nodes_visited = 0;
    let mut max_nesting_depth = 0;
    let mut limitation = None;
    let mut stack = vec![(root, 1_usize)];
    while let Some((node, depth)) = stack.pop() {
        if let Err(code) = visit(depth) {
            limitation = Some(code);
            break;
        }
        nodes_visited += 1;
        max_nesting_depth = max_nesting_depth.max(depth);
        if node.is_error() {
            error_node_count += 1;
            recovery_ranges.push(source_range(node));
        }
        if node.is_missing() {
            missing_node_count += 1;
            recovery_ranges.push(source_range(node));
        }
        for index in (0..node.child_count()).rev() {
            if let Some(child) = node.child(index as u32) {
                stack.push((child, depth + 1));
            }
        }
    }
    sort_dedup_ranges(&mut recovery_ranges);
    RecoveryTraversal {
        recovery_ranges,
        error_node_count,
        missing_node_count,
        nodes_visited,
        max_nesting_depth,
        limitation,
    }
}

fn collect_index_captures<'tree>(
    query: &Query,
    tree: &'tree Tree,
    source: &[u8],
    budget: &mut IndexBudgetTracker,
    limitations: &mut Vec<String>,
) -> Vec<IndexCapture<'tree>> {
    let capture_names = query.capture_names();
    let maximum_captures = budget
        .budget()
        .max_facts
        .saturating_add(budget.budget().max_symbols)
        .saturating_add(1);
    let mut captures = Vec::new();
    let mut cursor = QueryCursor::new();
    cursor.set_match_limit(65_536);
    {
        let mut query_progress = |_: &QueryCursorState| {
            if budget.check_deadline().is_err() {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let options = QueryCursorOptions::new().progress_callback(&mut query_progress);
        let mut matches = cursor.matches_with_options(query, tree.root_node(), source, options);
        'matches: while let Some(query_match) = matches.next() {
            for capture in query_match.captures {
                if captures.len() >= maximum_captures {
                    push_unique(limitations, "index-fact-budget-exhausted");
                    break 'matches;
                }
                let name = match capture_names[capture.index as usize] {
                    "definition.function" => "definition.function",
                    "declaration.function" => "declaration.function",
                    "definition.struct" => "definition.struct",
                    "definition.enum" => "definition.enum",
                    "definition.trait" => "definition.trait",
                    "definition.impl.type" => "definition.impl.type",
                    "definition.impl" => "definition.impl",
                    "definition.type" => "definition.type",
                    "definition.const" => "definition.const",
                    "definition.static" => "definition.static",
                    "definition.module" => "definition.module",
                    "definition.closure" => "definition.closure",
                    "import" => "import",
                    "call" => "call",
                    "macro" => "macro",
                    "attribute" => "attribute",
                    _ => continue,
                };
                captures.push(IndexCapture {
                    name,
                    node: capture.node,
                });
            }
        }
    }
    if budget.check_deadline().is_err() {
        push_unique(limitations, "index-deadline-exhausted");
    }
    if cursor.did_exceed_match_limit() {
        push_unique(limitations, "index-tree-sitter-query-match-limit");
    }
    captures
}

fn build_index_symbols(captures: &[IndexCapture<'_>], source: &[u8]) -> Vec<RustLocalSymbolFact> {
    let mut raw = captures
        .iter()
        .filter_map(|capture| {
            let base = symbol_from_capture(capture.name, capture.node, source)?;
            let item = if capture.name == "definition.impl" || capture.name == "definition.closure"
            {
                capture.node
            } else {
                item_ancestor(capture.node)?
            };
            Some(RawIndexSymbol {
                base,
                has_self_parameter: has_self_parameter(item),
            })
        })
        .collect::<Vec<_>>();
    raw.sort_by(|left, right| {
        left.base
            .range
            .start_byte
            .cmp(&right.base.range.start_byte)
            .then_with(|| right.base.range.end_byte.cmp(&left.base.range.end_byte))
            .then_with(|| left.base.kind.cmp(&right.base.kind))
            .then_with(|| left.base.name.cmp(&right.base.name))
    });
    raw.dedup_by(|left, right| {
        left.base.kind == right.base.kind
            && left.base.name == right.base.name
            && left.base.range == right.base.range
    });

    let mut symbols: Vec<RustLocalSymbolFact> = Vec::new();
    for raw_symbol in raw {
        let owner = innermost_owner(&symbols, &raw_symbol.base.range);
        let kind = index_symbol_kind(
            &raw_symbol.base.kind,
            owner.map(|symbol| symbol.kind.as_str()),
            raw_symbol.has_self_parameter,
        );
        let owner_local_id = owner.map(|symbol| symbol.local_id.clone());
        let local_id = stable_local_symbol_id(
            kind,
            owner_local_id.as_deref(),
            &raw_symbol.base.name,
            &raw_symbol.base.range,
        );
        symbols.push(RustLocalSymbolFact {
            local_id,
            kind: kind.to_string(),
            name: raw_symbol.base.name,
            owner_local_id,
            signature: raw_symbol.base.signature,
            visibility: raw_symbol.base.visibility,
            range: raw_symbol.base.range,
        });
    }
    symbols
}

fn has_self_parameter(item: Node<'_>) -> bool {
    let Some(parameters) = item.child_by_field_name("parameters") else {
        return false;
    };
    let mut stack = vec![parameters];
    while let Some(node) = stack.pop() {
        if node.kind() == "self_parameter" {
            return true;
        }
        for index in 0..node.named_child_count() {
            if let Some(child) = node.named_child(index as u32) {
                stack.push(child);
            }
        }
    }
    false
}

fn index_symbol_kind(
    base_kind: &str,
    owner_kind: Option<&str>,
    has_self_parameter: bool,
) -> &'static str {
    match (base_kind, owner_kind, has_self_parameter) {
        ("function", Some("impl" | "trait"), true) => "method",
        ("function", Some("impl" | "trait"), false) => "associated-function",
        ("function-declaration", Some("trait"), true) => "method-declaration",
        ("function-declaration", Some("trait"), false) => "associated-function-declaration",
        ("function", _, _) => "function",
        ("function-declaration", _, _) => "function-declaration",
        ("struct", _, _) => "struct",
        ("enum", _, _) => "enum",
        ("trait", _, _) => "trait",
        ("impl", _, _) => "impl",
        ("type", _, _) => "type",
        ("const", _, _) => "const",
        ("static", _, _) => "static",
        ("module", _, _) => "module",
        ("closure", _, _) => "closure",
        _ => "unknown",
    }
}

fn innermost_owner<'a>(
    symbols: &'a [RustLocalSymbolFact],
    range: &SourceRange,
) -> Option<&'a RustLocalSymbolFact> {
    symbols
        .iter()
        .filter(|symbol| strictly_contains(&symbol.range, range))
        .min_by_key(|symbol| {
            symbol
                .range
                .end_byte
                .saturating_sub(symbol.range.start_byte)
        })
}

fn owner_local_id(symbols: &[RustLocalSymbolFact], range: &SourceRange) -> Option<String> {
    innermost_owner(symbols, range).map(|symbol| symbol.local_id.clone())
}

fn strictly_contains(outer: &SourceRange, inner: &SourceRange) -> bool {
    outer.start_byte <= inner.start_byte
        && outer.end_byte >= inner.end_byte
        && (outer.start_byte < inner.start_byte || outer.end_byte > inner.end_byte)
}

fn build_import_facts(captures: &[IndexCapture<'_>], source: &[u8]) -> Vec<RustImportFact> {
    let mut declarations = BTreeSet::new();
    let mut imports = Vec::new();
    for capture in captures.iter().filter(|capture| capture.name == "import") {
        let Some(declaration) = ancestor_of_kind(capture.node, "use_declaration") else {
            continue;
        };
        if !declarations.insert(range_key(&source_range(declaration))) {
            continue;
        }
        let public = visibility_for_item(declaration, source).is_some();
        if let Some(argument) = declaration.child_by_field_name("argument") {
            flatten_use(argument, &[], public, source, &mut imports);
        }
    }
    imports
}

fn flatten_use(
    node: Node<'_>,
    prefix: &[String],
    public: bool,
    source: &[u8],
    imports: &mut Vec<RustImportFact>,
) {
    match node.kind() {
        "scoped_use_list" => {
            let mut next_prefix = prefix.to_vec();
            if let Some(path) = node.child_by_field_name("path") {
                next_prefix.extend(path_segments(path, source));
            }
            if let Some(list) = node.child_by_field_name("list") {
                flatten_use(list, &next_prefix, public, source, imports);
            }
        }
        "use_list" => {
            for index in 0..node.named_child_count() {
                if let Some(child) = node.named_child(index as u32) {
                    flatten_use(child, prefix, public, source, imports);
                }
            }
        }
        "use_as_clause" => {
            let mut segments = prefix.to_vec();
            if let Some(path) = node.child_by_field_name("path") {
                segments.extend(path_segments(path, source));
            }
            let alias = node
                .child_by_field_name("alias")
                .map(|alias| bounded_node_text(alias, source));
            if !segments.is_empty() {
                imports.push(RustImportFact {
                    segments,
                    alias,
                    glob: false,
                    public,
                    range: source_range(node),
                });
            }
        }
        "use_wildcard" => {
            let mut segments = prefix.to_vec();
            for index in 0..node.named_child_count() {
                if let Some(child) = node.named_child(index as u32) {
                    segments.extend(path_segments(child, source));
                }
            }
            imports.push(RustImportFact {
                segments,
                alias: None,
                glob: true,
                public,
                range: source_range(node),
            });
        }
        "self" if !prefix.is_empty() => imports.push(RustImportFact {
            segments: prefix.to_vec(),
            alias: None,
            glob: false,
            public,
            range: source_range(node),
        }),
        _ => {
            let mut segments = prefix.to_vec();
            segments.extend(path_segments(node, source));
            if !segments.is_empty() {
                imports.push(RustImportFact {
                    segments,
                    alias: None,
                    glob: false,
                    public,
                    range: source_range(node),
                });
            }
        }
    }
}

fn path_segments(node: Node<'_>, source: &[u8]) -> Vec<String> {
    bounded_node_text(node, source)
        .split("::")
        .map(|segment| segment.trim().trim_matches('{').trim_matches('}'))
        .filter(|segment| !segment.is_empty() && *segment != "*")
        .map(|segment| truncate_chars(segment.to_string(), MAX_FACT_TEXT_CHARS))
        .take(MAX_PATH_SEGMENTS)
        .collect()
}

fn build_module_facts(
    captures: &[IndexCapture<'_>],
    symbols: &[RustLocalSymbolFact],
    source: &[u8],
) -> Vec<RustModuleDeclarationFact> {
    captures
        .iter()
        .filter(|capture| capture.name == "definition.module")
        .filter_map(|capture| {
            let item = item_ancestor(capture.node)?;
            let range = source_range(item);
            let symbol = symbols
                .iter()
                .find(|symbol| symbol.kind == "module" && symbol.range == range)?;
            Some(RustModuleDeclarationFact {
                name: bounded_node_text(capture.node, source),
                inline: item.child_by_field_name("body").is_some(),
                path_override: module_path_override(item, source),
                owner_local_id: symbol.owner_local_id.clone(),
                range,
            })
        })
        .collect()
}

fn module_path_override(item: Node<'_>, source: &[u8]) -> Option<String> {
    let mut sibling = item.prev_named_sibling();
    while let Some(node) = sibling {
        if node.kind() != "attribute_item" {
            break;
        }
        let text = bounded_node_text(node, source);
        if text.starts_with("#[path") {
            let (_, value) = text.split_once('=')?;
            let value = value.trim().trim_end_matches(']').trim().trim_matches('"');
            return (!value.is_empty())
                .then(|| truncate_chars(value.to_string(), MAX_FACT_TEXT_CHARS));
        }
        sibling = node.prev_named_sibling();
    }
    None
}

fn build_attribute_facts(captures: &[IndexCapture<'_>], source: &[u8]) -> Vec<RustAttributeFact> {
    captures
        .iter()
        .filter(|capture| capture.name == "attribute")
        .filter_map(|capture| attribute_fact(capture.node, source))
        .collect()
}

fn attribute_fact(node: Node<'_>, source: &[u8]) -> Option<RustAttributeFact> {
    let attribute = if node.kind() == "attribute_item" {
        node.named_child(0)?
    } else {
        node
    };
    let text = bounded_node_text(attribute, source);
    let body = text
        .trim()
        .trim_start_matches("#![")
        .trim_start_matches("#[")
        .trim_end_matches(']')
        .trim();
    let split = body.find(['(', '=']).unwrap_or(body.len());
    let name = body[..split].trim();
    if name.is_empty() {
        return None;
    }
    let arguments = if split == body.len() {
        Vec::new()
    } else {
        let argument = body[split..]
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .trim_start_matches('=')
            .trim();
        if argument.is_empty() {
            Vec::new()
        } else {
            vec![truncate_chars(
                argument.split_whitespace().collect::<Vec<_>>().join(" "),
                MAX_FACT_TEXT_CHARS,
            )]
        }
    };
    Some(RustAttributeFact {
        name: truncate_chars(name.to_string(), MAX_FACT_TEXT_CHARS),
        arguments: arguments
            .into_iter()
            .take(MAX_ATTRIBUTE_ARGUMENTS)
            .collect(),
        range: source_range(node),
    })
}

fn build_call_facts(
    captures: &[IndexCapture<'_>],
    symbols: &[RustLocalSymbolFact],
    source: &[u8],
) -> Vec<RustCallSiteFact> {
    let mut calls = Vec::new();
    for capture in captures {
        let (callee, qualifier, call_kind, range) = match capture.name {
            "call" => {
                let call =
                    ancestor_of_kind(capture.node, "call_expression").unwrap_or(capture.node);
                let (callee, qualifier, call_kind) = call_target(capture.node, source);
                (callee, qualifier, call_kind, source_range(call))
            }
            "macro" => {
                let invocation =
                    ancestor_of_kind(capture.node, "macro_invocation").unwrap_or(capture.node);
                let mut segments = path_segments(capture.node, source);
                let callee = segments.pop().unwrap_or_default();
                (
                    callee,
                    segments,
                    "macro".to_string(),
                    source_range(invocation),
                )
            }
            _ => continue,
        };
        if callee.is_empty() {
            continue;
        }
        calls.push(RustCallSiteFact {
            callee,
            qualifier,
            call_kind,
            caller_local_id: owner_local_id(symbols, &range),
            range,
        });
    }
    calls
}

fn call_target(mut node: Node<'_>, source: &[u8]) -> (String, Vec<String>, String) {
    if node.kind() == "generic_function" {
        if let Some(function) = node.child_by_field_name("function") {
            node = function;
        }
    }
    match node.kind() {
        "field_expression" => {
            let callee = node
                .child_by_field_name("field")
                .map(|field| bounded_node_text(field, source))
                .unwrap_or_default();
            let qualifier = node
                .child_by_field_name("value")
                .map(|value| path_segments(value, source))
                .unwrap_or_default();
            (callee, qualifier, "method".to_string())
        }
        "scoped_identifier" | "scoped_type_identifier" => {
            let mut segments = path_segments(node, source);
            let callee = segments.pop().unwrap_or_default();
            (callee, segments, "function".to_string())
        }
        "identifier" | "type_identifier" => (
            bounded_node_text(node, source),
            Vec::new(),
            "function".to_string(),
        ),
        _ => (
            bounded_node_text(node, source),
            Vec::new(),
            "indirect".to_string(),
        ),
    }
}

fn build_reference_facts(
    language: &tree_sitter::Language,
    tree: &Tree,
    source: &[u8],
    budget: &mut IndexBudgetTracker,
    symbols: &[RustLocalSymbolFact],
    limitations: &mut Vec<String>,
) -> Result<Vec<RustReferenceFact>, RustAdapterError> {
    let query = Query::new(language, RUST_REFERENCE_QUERY).map_err(|error| {
        RustAdapterError::new(format!("cannot compile Rust reference query: {error}"))
    })?;
    let capture_names = query.capture_names();
    let mut references = Vec::new();
    let mut cursor = QueryCursor::new();
    cursor.set_match_limit(65_536);
    {
        let mut query_progress = |_: &QueryCursorState| {
            if budget.check_deadline().is_err() {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let options = QueryCursorOptions::new().progress_callback(&mut query_progress);
        let mut matches = cursor.matches_with_options(&query, tree.root_node(), source, options);
        'matches: while let Some(query_match) = matches.next() {
            for capture in query_match.captures {
                let capture_name = capture_names[capture.index as usize];
                let Some(reference) = reference_fact(capture_name, capture.node, source, symbols)
                else {
                    continue;
                };
                if references.len() >= budget.amount(IndexResource::Facts).remaining {
                    push_unique(limitations, "index-fact-budget-exhausted");
                    break 'matches;
                }
                references.push(reference);
            }
        }
    }
    if budget.check_deadline().is_err() {
        push_unique(limitations, "index-deadline-exhausted");
    }
    if cursor.did_exceed_match_limit() {
        push_unique(limitations, "index-tree-sitter-query-match-limit");
    }
    Ok(references)
}

fn reference_fact(
    capture: &str,
    node: Node<'_>,
    source: &[u8],
    symbols: &[RustLocalSymbolFact],
) -> Option<RustReferenceFact> {
    if is_definition_name(node)
        || within_ancestor(node, &["use_declaration", "attribute_item"])
        || within_field(node, "call_expression", "function")
        || within_field(node, "macro_invocation", "macro")
        || within_binding_pattern(node)
    {
        return None;
    }
    if matches!(capture, "reference.identifier" | "reference.type")
        && within_ancestor(node, &["scoped_identifier", "scoped_type_identifier"])
    {
        return None;
    }
    if matches!(capture, "reference.path" | "reference.type_path")
        && node.parent().is_some_and(|parent| {
            matches!(
                parent.kind(),
                "scoped_identifier" | "scoped_type_identifier"
            )
        })
    {
        return None;
    }

    let range = source_range(node);
    let (name, qualifier, role) = match capture {
        "reference.path" => {
            let mut segments = path_segments(node, source);
            let name = segments.pop()?;
            (name, segments, "qualified")
        }
        "reference.type_path" => {
            let mut segments = path_segments(node, source);
            let name = segments.pop()?;
            (name, segments, "type")
        }
        "reference.type" => (bounded_node_text(node, source), Vec::new(), "type"),
        "reference.field" => {
            let qualifier = node
                .parent()
                .and_then(|parent| parent.child_by_field_name("value"))
                .map(|value| path_segments(value, source))
                .unwrap_or_default();
            (bounded_node_text(node, source), qualifier, "field")
        }
        "reference.identifier" => (bounded_node_text(node, source), Vec::new(), "value"),
        _ => return None,
    };
    Some(RustReferenceFact {
        name,
        qualifier,
        role: role.to_string(),
        owner_local_id: owner_local_id(symbols, &range),
        range,
    })
}

fn is_definition_name(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    let definition_parent = matches!(
        parent.kind(),
        "function_item"
            | "function_signature_item"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "type_item"
            | "const_item"
            | "static_item"
            | "mod_item"
            | "field_declaration"
            | "enum_variant"
    );
    definition_parent
        && parent
            .child_by_field_name("name")
            .is_some_and(|name| same_node(name, node))
}

fn within_binding_pattern(node: Node<'_>) -> bool {
    [
        ("let_declaration", "pattern"),
        ("parameter", "pattern"),
        ("closure_parameters", "pattern"),
        ("for_expression", "pattern"),
        ("match_arm", "pattern"),
    ]
    .iter()
    .any(|(kind, field)| within_field(node, kind, field))
}

fn within_field(mut node: Node<'_>, ancestor_kind: &str, field: &str) -> bool {
    while let Some(parent) = node.parent() {
        if parent.kind() == ancestor_kind {
            return parent
                .child_by_field_name(field)
                .is_some_and(|field_node| contains_node(field_node, node));
        }
        node = parent;
    }
    false
}

fn within_ancestor(mut node: Node<'_>, kinds: &[&str]) -> bool {
    while let Some(parent) = node.parent() {
        if kinds.contains(&parent.kind()) {
            return true;
        }
        node = parent;
    }
    false
}

fn contains_node(outer: Node<'_>, inner: Node<'_>) -> bool {
    outer.start_byte() <= inner.start_byte() && outer.end_byte() >= inner.end_byte()
}

fn same_node(left: Node<'_>, right: Node<'_>) -> bool {
    left.kind() == right.kind()
        && left.start_byte() == right.start_byte()
        && left.end_byte() == right.end_byte()
}

fn ancestor_of_kind<'tree>(mut node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    loop {
        if node.kind() == kind {
            return Some(node);
        }
        node = node.parent()?;
    }
}

fn append_index_facts<T>(
    target: &mut Vec<T>,
    candidates: Vec<T>,
    budget: &mut IndexBudgetTracker,
    limitations: &mut Vec<String>,
) -> bool {
    for candidate in candidates {
        if let Err(exhaustion) = budget.consume(IndexResource::Facts, 1) {
            push_unique(limitations, exhaustion.code());
            return false;
        }
        target.push(candidate);
    }
    true
}

fn sort_index_facts(
    symbols: &mut Vec<RustLocalSymbolFact>,
    imports: &mut Vec<RustImportFact>,
    references: &mut Vec<RustReferenceFact>,
    calls: &mut Vec<RustCallSiteFact>,
    modules: &mut Vec<RustModuleDeclarationFact>,
    attributes: &mut Vec<RustAttributeFact>,
) {
    symbols.sort_by(|left, right| {
        range_key(&left.range)
            .cmp(&range_key(&right.range))
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.name.cmp(&right.name))
    });
    symbols.dedup_by(|left, right| left.local_id == right.local_id);
    imports.sort_by(|left, right| {
        range_key(&left.range)
            .cmp(&range_key(&right.range))
            .then_with(|| left.segments.cmp(&right.segments))
            .then_with(|| left.alias.cmp(&right.alias))
            .then_with(|| left.glob.cmp(&right.glob))
    });
    imports.dedup();
    references.sort_by(|left, right| {
        range_key(&left.range)
            .cmp(&range_key(&right.range))
            .then_with(|| left.role.cmp(&right.role))
            .then_with(|| left.qualifier.cmp(&right.qualifier))
            .then_with(|| left.name.cmp(&right.name))
    });
    references.dedup();
    calls.sort_by(|left, right| {
        range_key(&left.range)
            .cmp(&range_key(&right.range))
            .then_with(|| left.call_kind.cmp(&right.call_kind))
            .then_with(|| left.qualifier.cmp(&right.qualifier))
            .then_with(|| left.callee.cmp(&right.callee))
    });
    calls.dedup();
    modules.sort_by(|left, right| {
        range_key(&left.range)
            .cmp(&range_key(&right.range))
            .then_with(|| left.name.cmp(&right.name))
    });
    modules.dedup();
    attributes.sort_by(|left, right| {
        range_key(&left.range)
            .cmp(&range_key(&right.range))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.arguments.cmp(&right.arguments))
    });
    attributes.dedup();
}

fn empty_index_facts(source_bytes: usize, limitations: Vec<String>) -> RustFileFacts {
    RustFileFacts {
        parse_quality: ParseQuality::Degraded,
        symbols: Vec::new(),
        imports: Vec::new(),
        references: Vec::new(),
        calls: Vec::new(),
        module_declarations: Vec::new(),
        attributes: Vec::new(),
        recovery_ranges: Vec::new(),
        limitations: sorted_unique(limitations),
        metrics: RustFileFactMetrics {
            nodes_visited: 0,
            max_nesting_depth: 0,
            facts_emitted: 0,
            source_bytes,
        },
    }
}

fn sorted_unique(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

fn symbol_from_capture(capture: &str, node: Node<'_>, source: &[u8]) -> Option<RustSymbolFact> {
    if capture == "definition.impl.type" {
        return None;
    }
    let (kind, item, name) = match capture {
        "definition.function" => (
            "function",
            item_ancestor(node)?,
            bounded_node_text(node, source),
        ),
        "declaration.function" => (
            "function-declaration",
            item_ancestor(node)?,
            bounded_node_text(node, source),
        ),
        "definition.struct" => (
            "struct",
            item_ancestor(node)?,
            bounded_node_text(node, source),
        ),
        "definition.enum" => (
            "enum",
            item_ancestor(node)?,
            bounded_node_text(node, source),
        ),
        "definition.trait" => (
            "trait",
            item_ancestor(node)?,
            bounded_node_text(node, source),
        ),
        "definition.impl" => {
            let item = node;
            let name = item
                .child_by_field_name("type")
                .map(|child| bounded_node_text(child, source))
                .unwrap_or_else(|| "<impl>".to_string());
            ("impl", item, name)
        }
        "definition.type" => (
            "type",
            item_ancestor(node)?,
            bounded_node_text(node, source),
        ),
        "definition.const" => (
            "const",
            item_ancestor(node)?,
            bounded_node_text(node, source),
        ),
        "definition.static" => (
            "static",
            item_ancestor(node)?,
            bounded_node_text(node, source),
        ),
        "definition.module" => (
            "module",
            item_ancestor(node)?,
            bounded_node_text(node, source),
        ),
        "definition.closure" => {
            let range = source_range(node);
            (
                "closure",
                node,
                format!("<closure@{}:{}>", range.start_line, range.start_column),
            )
        }
        _ => return None,
    };
    Some(RustSymbolFact {
        kind: kind.to_string(),
        name,
        owner: owner_for_item(item, source),
        signature: signature_for_item(item, source),
        visibility: visibility_for_item(item, source),
        range: source_range(item),
    })
}

fn item_ancestor(mut node: Node<'_>) -> Option<Node<'_>> {
    loop {
        if node.kind().ends_with("_item") {
            return Some(node);
        }
        node = node.parent()?;
    }
}

fn owner_for_item(item: Node<'_>, source: &[u8]) -> Option<String> {
    let mut ancestor = item.parent();
    while let Some(node) = ancestor {
        match node.kind() {
            "impl_item" => return Some(signature_for_item(node, source)),
            "trait_item" => {
                return node
                    .child_by_field_name("name")
                    .map(|name| bounded_node_text(name, source))
            }
            _ => ancestor = node.parent(),
        }
    }
    None
}

fn signature_for_item(item: Node<'_>, source: &[u8]) -> String {
    let end = item
        .child_by_field_name("body")
        .map(|body| body.start_byte())
        .unwrap_or(item.end_byte());
    normalize_whitespace(&source[item.start_byte().min(source.len())..end.min(source.len())])
}

fn visibility_for_item(item: Node<'_>, source: &[u8]) -> Option<String> {
    (0..item.named_child_count())
        .filter_map(|index| item.named_child(index as u32))
        .find(|child| child.kind() == "visibility_modifier")
        .map(|child| bounded_node_text(child, source))
}

fn push_text_fact(
    facts: &mut Vec<RustTextFact>,
    node: Node<'_>,
    source: &[u8],
    budget: &mut BudgetTracker,
) -> bool {
    if budget.consume(BudgetResource::Facts, 1).is_err() {
        return false;
    }
    facts.push(RustTextFact {
        text: bounded_node_text(node, source),
        range: source_range(node),
    });
    true
}

fn innermost_caller(symbols: &[RustSymbolFact], range: &SourceRange) -> Option<SourceRange> {
    symbols
        .iter()
        .filter(|symbol| {
            symbol.range.start_byte <= range.start_byte && symbol.range.end_byte >= range.end_byte
        })
        .min_by_key(|symbol| {
            symbol
                .range
                .end_byte
                .saturating_sub(symbol.range.start_byte)
        })
        .map(|symbol| symbol.range.clone())
}

fn symbol_display_name(symbol: &RustSymbolFact) -> String {
    match &symbol.owner {
        Some(owner) => format!("{owner}::{}", symbol.name),
        None => symbol.name.clone(),
    }
}

fn node_intersects_changes(range: SourceRange, changes: &[ChangedRange]) -> bool {
    changes
        .iter()
        .any(|change| range.start_line <= change.end_line && range.end_line >= change.start_line)
}

fn ranges_overlap(left: &SourceRange, right: &SourceRange) -> bool {
    left.start_byte <= right.end_byte && right.start_byte <= left.end_byte
}

fn source_range(node: Node<'_>) -> SourceRange {
    let start = node.start_position();
    let end = node.end_position();
    SourceRange {
        start_line: start.row as u32 + 1,
        start_column: start.column as u32 + 1,
        end_line: end.row as u32 + 1,
        end_column: end.column as u32 + 1,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
    }
}

fn bounded_node_text(node: Node<'_>, source: &[u8]) -> String {
    let start = node.start_byte().min(source.len());
    let end = node.end_byte().min(source.len()).max(start);
    truncate_chars(
        String::from_utf8_lossy(&source[start..end]).into_owned(),
        1_000,
    )
}

fn normalize_whitespace(source: &[u8]) -> String {
    let normalized = String::from_utf8_lossy(source)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    truncate_chars(normalized, 1_000)
}

fn truncate_chars(value: String, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        value
    } else {
        value.chars().take(maximum).collect()
    }
}

fn symbol_order(left: &RustSymbolFact, right: &RustSymbolFact) -> std::cmp::Ordering {
    range_key(&left.range)
        .cmp(&range_key(&right.range))
        .then_with(|| left.kind.cmp(&right.kind))
        .then_with(|| left.name.cmp(&right.name))
}

fn sort_dedup_text_facts(facts: &mut Vec<RustTextFact>) {
    facts.sort_by(|left, right| {
        range_key(&left.range)
            .cmp(&range_key(&right.range))
            .then_with(|| left.text.cmp(&right.text))
    });
    facts.dedup_by(|left, right| left.text == right.text && left.range == right.range);
}

fn sort_dedup_ranges(ranges: &mut Vec<SourceRange>) {
    ranges.sort_by_key(range_key);
    ranges.dedup();
}

fn range_key(range: &SourceRange) -> (usize, usize, u32, u32, u32, u32) {
    (
        range.start_byte,
        range.end_byte,
        range.start_line,
        range.start_column,
        range.end_line,
        range.end_column,
    )
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}
