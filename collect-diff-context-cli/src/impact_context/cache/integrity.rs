use crate::impact_context::adapters::tree_sitter_rust::{
    RustAttributeFact, RustCallSiteFact, RustFileFacts, RustImportFact, RustLocalSymbolFact,
    RustModuleDeclarationFact, RustReferenceFact,
};
use crate::impact_context::contracts::SourceRange;
use crate::impact_context::index::model::{FileFactKey, RepositoryGraph};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(crate) fn file_fact_key_digest(key: &FileFactKey) -> Result<String, String> {
    key.validate().map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    hash_component(&mut digest, b"file-facts/v1");
    hash_component(&mut digest, key.language.as_bytes());
    hash_component(&mut digest, key.content_sha256.as_bytes());
    hash_component(&mut digest, key.grammar_version.as_bytes());
    hash_component(&mut digest, key.query_digest.as_bytes());
    hash_component(&mut digest, key.adapter_version.as_bytes());
    hash_component(&mut digest, key.normalization_rules_digest.as_bytes());
    hash_component(&mut digest, &key.schema_version.to_be_bytes());
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn payload_digest(payload: &[u8]) -> String {
    format!("{:x}", Sha256::digest(payload))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CanonicalGraphRows {
    pub identity: String,
    pub completeness: String,
    pub files: Vec<String>,
    pub modules: Vec<String>,
    pub symbols: Vec<String>,
    pub edges: Vec<String>,
    pub limitations: Vec<String>,
}

pub(crate) fn canonical_graph_rows(graph: &RepositoryGraph) -> Result<CanonicalGraphRows, String> {
    Ok(CanonicalGraphRows {
        identity: serde_json::to_string(&graph.identity).map_err(|error| error.to_string())?,
        completeness: serde_json::to_string(&graph.completeness)
            .map_err(|error| error.to_string())?,
        files: serialize_rows(&graph.files)?,
        modules: serialize_rows(&graph.modules)?,
        symbols: serialize_rows(&graph.symbols)?,
        edges: serialize_rows(&graph.edges)?,
        limitations: serialize_rows(&graph.limitations)?,
    })
}

pub(crate) fn graph_rows_root(rows: &CanonicalGraphRows) -> String {
    let mut digest = GraphRowsRootHasher::new(&rows.identity, &rows.completeness);
    for group in [
        &rows.files,
        &rows.modules,
        &rows.symbols,
        &rows.edges,
        &rows.limitations,
    ] {
        digest.start_group(group.len());
        for row in group {
            digest.push_row(row);
        }
    }
    digest.finish()
}

pub(crate) struct GraphRowsRootHasher {
    digest: Sha256,
}

impl GraphRowsRootHasher {
    pub(crate) fn new(identity: &str, completeness: &str) -> Self {
        let mut digest = Sha256::new();
        hash_component(&mut digest, b"repository-graph-application-root/v1");
        hash_component(&mut digest, identity.as_bytes());
        hash_component(&mut digest, completeness.as_bytes());
        Self { digest }
    }

    pub(crate) fn start_group(&mut self, row_count: usize) {
        hash_component(&mut self.digest, &(row_count as u64).to_be_bytes());
    }

    pub(crate) fn push_row(&mut self, canonical_row: &str) {
        hash_component(&mut self.digest, canonical_row.as_bytes());
    }

    pub(crate) fn finish(self) -> String {
        format!("{:x}", self.digest.finalize())
    }
}

fn serialize_rows<T: serde::Serialize>(rows: &[T]) -> Result<Vec<String>, String> {
    rows.iter()
        .map(|row| serde_json::to_string(row).map_err(|error| error.to_string()))
        .collect()
}

pub(crate) fn canonical_file_facts(facts: &RustFileFacts) -> RustFileFacts {
    let mut facts = facts.clone();
    facts.symbols.sort_by(symbol_order);
    facts.imports.sort_by(import_order);
    facts.references.sort_by(reference_order);
    facts.calls.sort_by(call_order);
    facts.module_declarations.sort_by(module_order);
    facts.attributes.sort_by(attribute_order);
    facts.recovery_ranges.sort_by_key(range_key);
    facts.recovery_ranges.dedup();
    facts.limitations.sort();
    facts.limitations.dedup();
    facts
}

pub(crate) fn validate_file_facts(facts: &RustFileFacts) -> Result<(), String> {
    if canonical_file_facts(facts) != *facts {
        return Err("file facts vectors must be deterministically sorted".to_string());
    }
    let fact_count = facts
        .symbols
        .len()
        .saturating_add(facts.imports.len())
        .saturating_add(facts.references.len())
        .saturating_add(facts.calls.len())
        .saturating_add(facts.module_declarations.len())
        .saturating_add(facts.attributes.len());
    if facts.metrics.facts_emitted != fact_count {
        return Err("file facts metric count does not match payload".to_string());
    }

    let mut symbol_ids = BTreeSet::new();
    for symbol in &facts.symbols {
        validate_range(&symbol.range, facts.metrics.source_bytes)?;
        validate_text(&symbol.local_id, 128, "local symbol id")?;
        validate_text(&symbol.kind, 128, "symbol kind")?;
        validate_text(&symbol.name, 1_000, "symbol name")?;
        validate_text(&symbol.signature, 1_000, "symbol signature")?;
        if !symbol_ids.insert(symbol.local_id.as_str()) {
            return Err("file facts contain duplicate local symbol ids".to_string());
        }
    }
    for symbol in &facts.symbols {
        if symbol
            .owner_local_id
            .as_deref()
            .is_some_and(|owner| !symbol_ids.contains(owner))
        {
            return Err("local symbol owner does not exist".to_string());
        }
    }
    for import in &facts.imports {
        validate_range(&import.range, facts.metrics.source_bytes)?;
        validate_segments(&import.segments, "import")?;
        if let Some(alias) = &import.alias {
            validate_text(alias, 1_000, "import alias")?;
        }
    }
    for reference in &facts.references {
        validate_range(&reference.range, facts.metrics.source_bytes)?;
        validate_text(&reference.name, 1_000, "reference name")?;
        validate_text(&reference.role, 128, "reference role")?;
        validate_segments(&reference.qualifier, "reference qualifier")?;
    }
    for call in &facts.calls {
        validate_range(&call.range, facts.metrics.source_bytes)?;
        validate_text(&call.callee, 1_000, "call callee")?;
        validate_text(&call.call_kind, 128, "call kind")?;
        validate_segments(&call.qualifier, "call qualifier")?;
    }
    for module in &facts.module_declarations {
        validate_range(&module.range, facts.metrics.source_bytes)?;
        validate_text(&module.name, 1_000, "module name")?;
        if let Some(path) = &module.path_override {
            validate_text(path, 1_000, "module path override")?;
        }
    }
    for attribute in &facts.attributes {
        validate_range(&attribute.range, facts.metrics.source_bytes)?;
        validate_text(&attribute.name, 1_000, "attribute name")?;
        if attribute.arguments.len() > 64 {
            return Err("attribute argument count exceeds 64".to_string());
        }
        for argument in &attribute.arguments {
            validate_text(argument, 1_000, "attribute argument")?;
        }
    }
    for range in &facts.recovery_ranges {
        validate_range(range, facts.metrics.source_bytes)?;
    }
    if facts.limitations.len() > 1_000 {
        return Err("file facts limitation count exceeds 1000".to_string());
    }
    for limitation in &facts.limitations {
        validate_text(limitation, 200, "file facts limitation")?;
    }
    Ok(())
}

fn validate_segments(segments: &[String], context: &str) -> Result<(), String> {
    if segments.len() > 256 {
        return Err(format!("{context} segment count exceeds 256"));
    }
    for segment in segments {
        validate_text(segment, 1_000, context)?;
    }
    Ok(())
}

fn validate_text(value: &str, maximum_chars: usize, context: &str) -> Result<(), String> {
    if value.is_empty() || value.chars().count() > maximum_chars {
        return Err(format!(
            "{context} is empty or exceeds {maximum_chars} characters"
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{context} contains control characters"));
    }
    Ok(())
}

fn validate_range(range: &SourceRange, source_bytes: usize) -> Result<(), String> {
    if range.start_line == 0
        || range.start_column == 0
        || range.end_line == 0
        || range.end_column == 0
        || range.start_byte > range.end_byte
        || range.end_byte > source_bytes
        || range.start_line > range.end_line
        || (range.start_line == range.end_line && range.start_column > range.end_column)
    {
        return Err("file facts contain an invalid source range".to_string());
    }
    Ok(())
}

fn hash_component(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn symbol_order(left: &RustLocalSymbolFact, right: &RustLocalSymbolFact) -> std::cmp::Ordering {
    range_key(&left.range)
        .cmp(&range_key(&right.range))
        .then_with(|| left.kind.cmp(&right.kind))
        .then_with(|| left.name.cmp(&right.name))
        .then_with(|| left.local_id.cmp(&right.local_id))
}

fn import_order(left: &RustImportFact, right: &RustImportFact) -> std::cmp::Ordering {
    range_key(&left.range)
        .cmp(&range_key(&right.range))
        .then_with(|| left.segments.cmp(&right.segments))
        .then_with(|| left.alias.cmp(&right.alias))
        .then_with(|| left.glob.cmp(&right.glob))
        .then_with(|| left.public.cmp(&right.public))
}

fn reference_order(left: &RustReferenceFact, right: &RustReferenceFact) -> std::cmp::Ordering {
    range_key(&left.range)
        .cmp(&range_key(&right.range))
        .then_with(|| left.role.cmp(&right.role))
        .then_with(|| left.qualifier.cmp(&right.qualifier))
        .then_with(|| left.name.cmp(&right.name))
}

fn call_order(left: &RustCallSiteFact, right: &RustCallSiteFact) -> std::cmp::Ordering {
    range_key(&left.range)
        .cmp(&range_key(&right.range))
        .then_with(|| left.call_kind.cmp(&right.call_kind))
        .then_with(|| left.qualifier.cmp(&right.qualifier))
        .then_with(|| left.callee.cmp(&right.callee))
}

fn module_order(
    left: &RustModuleDeclarationFact,
    right: &RustModuleDeclarationFact,
) -> std::cmp::Ordering {
    range_key(&left.range)
        .cmp(&range_key(&right.range))
        .then_with(|| left.name.cmp(&right.name))
}

fn attribute_order(left: &RustAttributeFact, right: &RustAttributeFact) -> std::cmp::Ordering {
    range_key(&left.range)
        .cmp(&range_key(&right.range))
        .then_with(|| left.name.cmp(&right.name))
        .then_with(|| left.arguments.cmp(&right.arguments))
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
