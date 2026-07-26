use crate::candidate::ChangedRange;
use crate::impact_context::budget::{BudgetResource, BudgetTracker};
use crate::impact_context::contracts::{ParseQuality, Resolution, SourceRange};
use serde::Serialize;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

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
        let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .map_err(|error| RustAdapterError::new(format!("cannot load Rust grammar: {error}")))?;
        let query = Query::new(&language, RUST_FACT_QUERY).map_err(|error| {
            RustAdapterError::new(format!("cannot compile Rust query: {error}"))
        })?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| RustAdapterError::new("Tree-sitter returned no Rust syntax tree"))?;

        let mut errors = Vec::new();
        let mut error_node_count = 0;
        let mut missing_node_count = 0;
        let mut nodes_visited = 0;
        let mut max_nesting_depth = 0;
        let mut limitation_codes = Vec::new();
        let mut traversal_complete = true;
        let mut stack = vec![(tree.root_node(), 1usize)];
        while let Some((node, depth)) = stack.pop() {
            if let Err(exhaustion) = budget.consume(BudgetResource::Nodes, 1) {
                push_unique(&mut limitation_codes, exhaustion.code());
                traversal_complete = false;
                break;
            }
            if let Err(exhaustion) = budget.observe(BudgetResource::NestingDepth, depth) {
                push_unique(&mut limitation_codes, exhaustion.code());
                traversal_complete = false;
                break;
            }
            nodes_visited += 1;
            max_nesting_depth = max_nesting_depth.max(depth);
            if node.is_error() {
                error_node_count += 1;
                errors.push(source_range(node));
            }
            if node.is_missing() {
                missing_node_count += 1;
                errors.push(source_range(node));
            }
            for index in (0..node.child_count()).rev() {
                if let Some(child) = node.child(index as u32) {
                    stack.push((child, depth + 1));
                }
            }
        }
        sort_dedup_ranges(&mut errors);

        let mut captures = Vec::new();
        if traversal_complete {
            let capture_names = query.capture_names();
            let mut cursor = QueryCursor::new();
            cursor.set_match_limit(65_536);
            let mut matches = cursor.matches(&query, tree.root_node(), source);
            while let Some(query_match) = matches.next() {
                for capture in query_match.captures {
                    captures.push((capture_names[capture.index as usize], capture.node));
                }
            }
            if cursor.did_exceed_match_limit() {
                push_unique(&mut limitation_codes, "tree-sitter-query-match-limit");
            }
        }

        let mut changed_symbols = Vec::new();
        for (capture, node) in captures.iter().copied() {
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
        calls.sort_by(|left, right| range_key(&left.range).cmp(&range_key(&right.range)));
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
