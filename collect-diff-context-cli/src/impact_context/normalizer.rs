use crate::impact_context::adapters::text::{TextOutput, TextProvenance};
use crate::impact_context::adapters::tree_sitter_rust::{
    RustCallFact, RustSyntaxOutput, RustTextFact,
};
use crate::impact_context::contracts::{
    ChangedSymbol, Confidence, EdgeKind, ImpactEdge, ParseQuality, Resolution, SourceRange,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NormalizedFact {
    pub fact_id: String,
    pub provider_id: String,
    pub path: String,
    pub kind: String,
    pub rule_id: String,
    pub text: String,
    pub range: SourceRange,
    pub confidence: Confidence,
    pub resolution: Option<Resolution>,
    pub provenance: String,
    pub details: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NormalizedUnitFacts {
    pub path: String,
    pub changed_symbols: Vec<ChangedSymbol>,
    pub impact_edges: Vec<ImpactEdge>,
    pub facts: Vec<NormalizedFact>,
}

pub fn normalize_unit(
    path: &str,
    language: &str,
    syntax_provider_id: &str,
    text_provider_id: &str,
    syntax: Option<&RustSyntaxOutput>,
    text: Option<&TextOutput>,
) -> NormalizedUnitFacts {
    let mut symbols = BTreeMap::new();
    let mut edges = BTreeMap::new();
    let mut facts = BTreeMap::new();
    let mut caller_ids = BTreeMap::new();

    if let Some(syntax) = syntax {
        let symbol_confidence = symbol_confidence(syntax.parse_quality);
        for symbol in &syntax.changed_symbols {
            let symbol_id = stable_id(
                "impact-symbol/v1",
                &[
                    syntax_provider_id,
                    path,
                    &symbol.kind,
                    &range_identity(&symbol.range),
                    symbol.owner.as_deref().unwrap_or(""),
                    &symbol.name,
                ],
            );
            let normalized = ChangedSymbol {
                symbol_id: symbol_id.clone(),
                provider_id: syntax_provider_id.to_string(),
                path: path.to_string(),
                language: language.to_string(),
                kind: symbol.kind.clone(),
                name: symbol.name.clone(),
                owner: symbol.owner.clone(),
                signature: Some(symbol.signature.clone()),
                visibility: symbol.visibility.clone(),
                range: symbol.range.clone(),
                confidence: symbol_confidence,
            };
            merge_symbol(&mut symbols, normalized);
            caller_ids.insert(range_identity(&symbol.range), symbol_id.clone());

            let defines = make_edge(
                syntax_provider_id,
                path,
                EdgeKind::Defines,
                &format!("file:{path}"),
                Some(symbol_id.clone()),
                None,
                symbol.range.clone(),
                Resolution::Syntactic,
                structural_edge_confidence(syntax.parse_quality),
            );
            merge_edge(&mut edges, defines);
            if symbol
                .visibility
                .as_deref()
                .is_some_and(|visibility| visibility.starts_with("pub"))
            {
                let exports = make_edge(
                    syntax_provider_id,
                    path,
                    EdgeKind::Exports,
                    &format!("file:{path}"),
                    Some(symbol_id),
                    None,
                    symbol.range.clone(),
                    Resolution::Syntactic,
                    structural_edge_confidence(syntax.parse_quality),
                );
                merge_edge(&mut edges, exports);
            }
        }

        for import in &syntax.imports {
            let fact = syntax_text_fact(
                syntax_provider_id,
                path,
                "import",
                "rust-import",
                import,
                syntax.parse_quality,
            );
            merge_fact(&mut facts, fact);
            let edge = make_edge(
                syntax_provider_id,
                path,
                EdgeKind::Imports,
                &format!("file:{path}"),
                None,
                Some(import.text.clone()),
                import.range.clone(),
                Resolution::Syntactic,
                structural_edge_confidence(syntax.parse_quality),
            );
            merge_edge(&mut edges, edge);
        }
        for call in &syntax.calls {
            merge_edge(
                &mut edges,
                call_edge(
                    syntax_provider_id,
                    path,
                    call,
                    &caller_ids,
                    syntax.parse_quality,
                ),
            );
        }
        for macro_fact in &syntax.macros {
            let fact = syntax_text_fact(
                syntax_provider_id,
                path,
                "macro",
                "rust-macro",
                macro_fact,
                syntax.parse_quality,
            );
            merge_fact(&mut facts, fact);
            let edge = make_edge(
                syntax_provider_id,
                path,
                EdgeKind::Calls,
                &format!("file:{path}"),
                None,
                Some(macro_fact.text.clone()),
                macro_fact.range.clone(),
                Resolution::Unresolved,
                structural_edge_confidence(syntax.parse_quality),
            );
            merge_edge(&mut edges, edge);
        }
        for attribute in &syntax.attributes {
            merge_fact(
                &mut facts,
                syntax_text_fact(
                    syntax_provider_id,
                    path,
                    "attribute",
                    "rust-attribute",
                    attribute,
                    syntax.parse_quality,
                ),
            );
        }
    }

    if let Some(text) = text {
        for fact in &text.facts {
            let kind = format!("text:{}", fact.kind.as_str());
            let fact_id = stable_id(
                "impact-fact/v1",
                &[
                    text_provider_id,
                    path,
                    &kind,
                    &range_identity(&fact.range),
                    &fact.rule_id,
                    "textual",
                ],
            );
            merge_fact(
                &mut facts,
                NormalizedFact {
                    fact_id,
                    provider_id: text_provider_id.to_string(),
                    path: path.to_string(),
                    kind,
                    rule_id: fact.rule_id.clone(),
                    text: fact.match_text.clone(),
                    range: fact.range.clone(),
                    confidence: Confidence::Low,
                    resolution: None,
                    provenance: match fact.provenance {
                        TextProvenance::Textual => "textual".to_string(),
                    },
                    details: fact.details.clone(),
                },
            );
        }
    }

    NormalizedUnitFacts {
        path: path.to_string(),
        changed_symbols: symbols.into_values().collect(),
        impact_edges: edges.into_values().collect(),
        facts: facts.into_values().collect(),
    }
}

pub fn merge_normalized_units(
    path: &str,
    units: impl IntoIterator<Item = NormalizedUnitFacts>,
) -> NormalizedUnitFacts {
    let mut symbols = BTreeMap::new();
    let mut edges = BTreeMap::new();
    let mut facts = BTreeMap::new();
    for unit in units {
        for symbol in unit.changed_symbols {
            merge_symbol(&mut symbols, symbol);
        }
        for edge in unit.impact_edges {
            merge_edge(&mut edges, edge);
        }
        for fact in unit.facts {
            merge_fact(&mut facts, fact);
        }
    }
    NormalizedUnitFacts {
        path: path.to_string(),
        changed_symbols: symbols.into_values().collect(),
        impact_edges: edges.into_values().collect(),
        facts: facts.into_values().collect(),
    }
}

pub fn stable_id(namespace: &str, fields: &[&str]) -> String {
    let mut digest = Sha256::new();
    digest.update(namespace.as_bytes());
    digest.update([0]);
    for field in fields {
        digest.update(field.as_bytes());
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())[..16].to_string()
}

pub(crate) fn stable_local_symbol_id(
    kind: &str,
    owner_local_id: Option<&str>,
    name: &str,
    range: &SourceRange,
) -> String {
    stable_id(
        "rust-file-local-symbol/v1",
        &[
            kind,
            owner_local_id.unwrap_or(""),
            name,
            &range_identity(range),
        ],
    )
}

fn syntax_text_fact(
    provider_id: &str,
    path: &str,
    kind: &str,
    rule_id: &str,
    fact: &RustTextFact,
    quality: ParseQuality,
) -> NormalizedFact {
    let fact_id = stable_id(
        "impact-fact/v1",
        &[
            provider_id,
            path,
            kind,
            &range_identity(&fact.range),
            rule_id,
            "syntactic",
        ],
    );
    NormalizedFact {
        fact_id,
        provider_id: provider_id.to_string(),
        path: path.to_string(),
        kind: kind.to_string(),
        rule_id: rule_id.to_string(),
        text: fact.text.clone(),
        range: fact.range.clone(),
        confidence: structural_edge_confidence(quality),
        resolution: Some(Resolution::Syntactic),
        provenance: "syntactic".to_string(),
        details: BTreeMap::new(),
    }
}

fn call_edge(
    provider_id: &str,
    path: &str,
    call: &RustCallFact,
    caller_ids: &BTreeMap<String, String>,
    quality: ParseQuality,
) -> ImpactEdge {
    let from_symbol = call
        .caller_range
        .as_ref()
        .and_then(|range| caller_ids.get(&range_identity(range)))
        .cloned()
        .unwrap_or_else(|| format!("file:{path}"));
    make_edge(
        provider_id,
        path,
        EdgeKind::Calls,
        &from_symbol,
        None,
        Some(call.target.clone()),
        call.range.clone(),
        Resolution::Unresolved,
        structural_edge_confidence(quality),
    )
}

#[allow(clippy::too_many_arguments)]
fn make_edge(
    provider_id: &str,
    path: &str,
    kind: EdgeKind,
    from_symbol: &str,
    to_symbol: Option<String>,
    unresolved_target: Option<String>,
    range: SourceRange,
    resolution: Resolution,
    confidence: Confidence,
) -> ImpactEdge {
    let target = to_symbol
        .as_deref()
        .or(unresolved_target.as_deref())
        .unwrap_or("");
    let edge_id = stable_id(
        "impact-edge/v1",
        &[
            provider_id,
            path,
            edge_kind_name(kind),
            &range_identity(&range),
            from_symbol,
            target,
            resolution_name(resolution),
        ],
    );
    ImpactEdge {
        edge_id,
        kind,
        from_symbol: from_symbol.to_string(),
        to_symbol,
        unresolved_target,
        path: path.to_string(),
        range,
        provider_id: provider_id.to_string(),
        resolution,
        confidence,
    }
}

fn merge_symbol(symbols: &mut BTreeMap<String, ChangedSymbol>, symbol: ChangedSymbol) {
    match symbols.get(&symbol.symbol_id) {
        Some(existing)
            if confidence_rank(existing.confidence) >= confidence_rank(symbol.confidence) => {}
        _ => {
            symbols.insert(symbol.symbol_id.clone(), symbol);
        }
    }
}

fn merge_edge(edges: &mut BTreeMap<String, ImpactEdge>, edge: ImpactEdge) {
    match edges.get(&edge.edge_id) {
        Some(existing)
            if confidence_rank(existing.confidence) >= confidence_rank(edge.confidence) => {}
        _ => {
            edges.insert(edge.edge_id.clone(), edge);
        }
    }
}

fn merge_fact(facts: &mut BTreeMap<String, NormalizedFact>, fact: NormalizedFact) {
    match facts.get(&fact.fact_id) {
        Some(existing)
            if confidence_rank(existing.confidence) >= confidence_rank(fact.confidence) => {}
        _ => {
            facts.insert(fact.fact_id.clone(), fact);
        }
    }
}

fn symbol_confidence(quality: ParseQuality) -> Confidence {
    match quality {
        ParseQuality::Clean => Confidence::High,
        ParseQuality::Recovered => Confidence::Medium,
        ParseQuality::Degraded => Confidence::Low,
    }
}

fn structural_edge_confidence(quality: ParseQuality) -> Confidence {
    match quality {
        ParseQuality::Clean => Confidence::Medium,
        ParseQuality::Recovered | ParseQuality::Degraded => Confidence::Low,
    }
}

fn confidence_rank(confidence: Confidence) -> u8 {
    match confidence {
        Confidence::High => 3,
        Confidence::Medium => 2,
        Confidence::Low => 1,
    }
}

fn range_identity(range: &SourceRange) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}",
        range.start_line,
        range.start_column,
        range.end_line,
        range.end_column,
        range.start_byte,
        range.end_byte
    )
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

fn resolution_name(resolution: Resolution) -> &'static str {
    match resolution {
        Resolution::Syntactic => "syntactic",
        Resolution::Lexical => "lexical",
        Resolution::ResolvedReference => "resolved-reference",
        Resolution::Semantic => "semantic",
        Resolution::PolymorphicCandidate => "polymorphic-candidate",
        Resolution::Unresolved => "unresolved",
    }
}
