use crate::candidate::{CandidatePresence, RepoPath};
use crate::impact_context::cache::sqlite_generation::{
    RepositoryGraphError, RepositoryGraphReader,
};
use crate::impact_context::contracts::{Completeness, Confidence, EdgeKind, Resolution};
use crate::impact_context::index::budget::{
    IndexBudgetExhaustion, IndexBudgetTracker, IndexResource,
};
use crate::impact_context::index::model::{
    GraphEdge, GraphFile, GraphModule, GraphSymbol, IndexLimitation, RepositoryGraph,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryOverlay {
    pub base_generation_key: String,
    pub candidate_manifest_digest: String,
    pub path_tombstones: BTreeSet<RepoPath>,
    pub files: BTreeMap<RepoPath, GraphFile>,
    pub modules: BTreeMap<String, GraphModule>,
    pub symbols: BTreeMap<String, GraphSymbol>,
    pub outgoing_edges: BTreeMap<String, Vec<GraphEdge>>,
    pub incoming_edges: BTreeMap<String, Vec<GraphEdge>>,
    pub suppressed_base_edge_ids: BTreeSet<String>,
    pub completeness: Completeness,
    pub limitations: Vec<IndexLimitation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayError {
    pub code: &'static str,
    pub message: String,
}

impl OverlayError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for OverlayError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OverlayError {}

impl From<RepositoryGraphError> for OverlayError {
    fn from(error: RepositoryGraphError) -> Self {
        Self::new(error.code, error.message)
    }
}

pub fn build_repository_overlay(
    base: &RepositoryGraphReader,
    candidate: &RepositoryGraph,
    changed_paths: &BTreeSet<RepoPath>,
    budget: &mut IndexBudgetTracker,
) -> Result<RepositoryOverlay, OverlayError> {
    let base_generation_key = base.identity().generation_key().map_err(|error| {
        OverlayError::new(
            "overlay-base-identity-invalid",
            format!("cannot identify base repository graph: {error}"),
        )
    })?;
    let mut builder = OverlayBuilder {
        base,
        candidate,
        budget,
        overlay: RepositoryOverlay {
            base_generation_key,
            candidate_manifest_digest: candidate.identity.candidate_manifest_digest.clone(),
            path_tombstones: BTreeSet::new(),
            files: BTreeMap::new(),
            modules: BTreeMap::new(),
            symbols: BTreeMap::new(),
            outgoing_edges: BTreeMap::new(),
            incoming_edges: BTreeMap::new(),
            suppressed_base_edge_ids: BTreeSet::new(),
            completeness: merge_completeness(base.completeness(), candidate.completeness),
            limitations: candidate.limitations.clone(),
        },
        queued_paths: changed_paths.clone(),
        queue: changed_paths.iter().cloned().collect(),
        queried_symbols: BTreeSet::new(),
    };
    builder.build()?;
    Ok(builder.finish())
}

struct OverlayBuilder<'a> {
    base: &'a RepositoryGraphReader,
    candidate: &'a RepositoryGraph,
    budget: &'a mut IndexBudgetTracker,
    overlay: RepositoryOverlay,
    queued_paths: BTreeSet<RepoPath>,
    queue: VecDeque<RepoPath>,
    queried_symbols: BTreeSet<String>,
}

impl OverlayBuilder<'_> {
    fn build(&mut self) -> Result<(), OverlayError> {
        while let Some(path) = self.queue.pop_front() {
            if let Err(exhaustion) = self.budget.check_deadline() {
                self.record_exhaustion(exhaustion, Some(path));
                break;
            }
            if let Err(exhaustion) = self.budget.consume(IndexResource::OverlayPaths, 1) {
                self.record_exhaustion(exhaustion, Some(path));
                break;
            }
            self.process_path(&path)?;
        }
        Ok(())
    }

    fn process_path(&mut self, path: &RepoPath) -> Result<(), OverlayError> {
        self.overlay.path_tombstones.insert(path.clone());

        let base_symbols = self.query_symbols_for_path(path)?;
        for edge in self.query_edges_for_path(path)? {
            self.overlay.suppressed_base_edge_ids.insert(edge.edge_id);
        }

        self.insert_candidate_path(path)?;

        let target_deleted = !self.candidate_path_is_present(path);
        for symbol in base_symbols {
            if !self.queried_symbols.insert(symbol.symbol_id.clone()) {
                continue;
            }
            for edge in self.query_incoming(&symbol.symbol_id, path)? {
                if matches!(
                    edge.kind,
                    EdgeKind::Imports | EdgeKind::References | EdgeKind::Exports
                ) {
                    self.enqueue_path(edge.path.clone());
                }
                if target_deleted && !self.overlay.path_tombstones.contains(&edge.path) {
                    self.overlay
                        .suppressed_base_edge_ids
                        .insert(edge.edge_id.clone());
                    let unresolved = unresolved_deleted_target_edge(&edge, &symbol.symbol_id);
                    if self.consume_overlay_value(IndexResource::Edges, &unresolved, path)? {
                        self.insert_incoming_for(symbol.symbol_id.clone(), unresolved.clone());
                        self.insert_outgoing(unresolved);
                    }
                }
            }
        }
        Ok(())
    }

    fn insert_candidate_path(&mut self, path: &RepoPath) -> Result<(), OverlayError> {
        if let Some(file) = self
            .candidate
            .files
            .iter()
            .find(|file| file.path == *path && file.presence == CandidatePresence::Present)
            .cloned()
        {
            if !self.consume_overlay_value(IndexResource::Nodes, &file, path)? {
                return Ok(());
            }
            self.overlay.files.insert(path.clone(), file);
        }

        let modules: Vec<_> = self
            .candidate
            .modules
            .iter()
            .filter(|module| module.path == *path)
            .cloned()
            .collect();
        for module in modules {
            if !self.consume_overlay_value(IndexResource::Nodes, &module, path)? {
                break;
            }
            self.overlay
                .modules
                .insert(module.module_id.clone(), module);
        }

        let symbols: Vec<_> = self
            .candidate
            .symbols
            .iter()
            .filter(|symbol| symbol.path == *path)
            .cloned()
            .collect();
        for symbol in symbols {
            if !self.consume_candidate(IndexResource::Symbols, 1, path)
                || !self.consume_overlay_value(IndexResource::Nodes, &symbol, path)?
            {
                break;
            }
            self.overlay
                .symbols
                .insert(symbol.symbol_id.clone(), symbol);
        }

        let edges: Vec<_> = self
            .candidate
            .edges
            .iter()
            .filter(|edge| edge.path == *path)
            .cloned()
            .collect();
        for edge in edges {
            if !self.consume_overlay_value(IndexResource::Edges, &edge, path)? {
                break;
            }
            self.insert_edge(edge);
        }
        Ok(())
    }

    fn candidate_path_is_present(&self, path: &RepoPath) -> bool {
        self.candidate
            .files
            .iter()
            .any(|file| file.path == *path && file.presence == CandidatePresence::Present)
    }

    fn enqueue_path(&mut self, path: RepoPath) {
        if self.queued_paths.insert(path.clone()) {
            self.queue.push_back(path);
            let mut queued: Vec<_> = self.queue.drain(..).collect();
            queued.sort();
            self.queue.extend(queued);
        }
    }

    fn query_symbols_for_path(
        &mut self,
        path: &RepoPath,
    ) -> Result<Vec<GraphSymbol>, OverlayError> {
        let Some(limit) = self.query_limit(path) else {
            return Ok(Vec::new());
        };
        let symbols = self.base.symbols_for_path(path, limit)?;
        self.observe_query_result(symbols.len(), limit, path);
        Ok(symbols)
    }

    fn query_edges_for_path(&mut self, path: &RepoPath) -> Result<Vec<GraphEdge>, OverlayError> {
        let Some(limit) = self.query_limit(path) else {
            return Ok(Vec::new());
        };
        let edges = self.base.edges_for_path(path, limit)?;
        self.observe_query_result(edges.len(), limit, path);
        Ok(edges)
    }

    fn query_incoming(
        &mut self,
        symbol_id: &str,
        path: &RepoPath,
    ) -> Result<Vec<GraphEdge>, OverlayError> {
        let Some(limit) = self.query_limit(path) else {
            return Ok(Vec::new());
        };
        let edges = self.base.incoming(symbol_id, limit)?;
        self.observe_query_result(edges.len(), limit, path);
        Ok(edges)
    }

    fn query_limit(&mut self, path: &RepoPath) -> Option<usize> {
        let remaining = self.budget.amount(IndexResource::QueryRows).remaining;
        let limit = remaining.min(self.base.maximum_rows_per_query());
        if limit == 0 {
            if let Err(exhaustion) = self.budget.consume(IndexResource::QueryRows, 1) {
                self.record_exhaustion(exhaustion, Some(path.clone()));
            }
            None
        } else {
            Some(limit)
        }
    }

    fn observe_query_result(&mut self, rows: usize, limit: usize, path: &RepoPath) {
        if let Err(exhaustion) = self.budget.consume(IndexResource::QueryRows, rows) {
            self.record_exhaustion(exhaustion, Some(path.clone()));
        }
        if rows == limit {
            self.overlay.completeness = Completeness::Partial;
            self.overlay.limitations.push(IndexLimitation {
                code: "index-query-row-limit-reached".to_string(),
                path: Some(path.clone()),
                symbol_id: None,
                reason: "an indexed graph query reached its exact row limit".to_string(),
                interpretation: "additional base symbols or relationships may exist".to_string(),
            });
        }
    }

    fn consume_candidate(
        &mut self,
        resource: IndexResource,
        amount: usize,
        path: &RepoPath,
    ) -> bool {
        match self.budget.consume(resource, amount) {
            Ok(()) => true,
            Err(exhaustion) => {
                self.record_exhaustion(exhaustion, Some(path.clone()));
                false
            }
        }
    }

    fn consume_overlay_value<T: Serialize>(
        &mut self,
        resource: IndexResource,
        value: &T,
        path: &RepoPath,
    ) -> Result<bool, OverlayError> {
        if !self.consume_candidate(resource, 1, path) {
            return Ok(false);
        }
        let bytes = serde_json::to_vec(value).map_err(|error| {
            OverlayError::new(
                "overlay-value-serialization-failed",
                format!("cannot size repository overlay value: {error}"),
            )
        })?;
        Ok(self.consume_candidate(IndexResource::GenerationBytes, bytes.len(), path))
    }

    fn insert_edge(&mut self, edge: GraphEdge) {
        if let Some(target) = edge.to_symbol.clone() {
            self.insert_incoming_for(target, edge.clone());
        }
        self.insert_outgoing(edge);
    }

    fn insert_outgoing(&mut self, edge: GraphEdge) {
        self.overlay
            .outgoing_edges
            .entry(edge.from_symbol.clone())
            .or_default()
            .push(edge);
    }

    fn insert_incoming_for(&mut self, symbol_id: String, edge: GraphEdge) {
        self.overlay
            .incoming_edges
            .entry(symbol_id)
            .or_default()
            .push(edge);
    }

    fn record_exhaustion(&mut self, exhaustion: IndexBudgetExhaustion, path: Option<RepoPath>) {
        self.overlay.completeness = Completeness::Partial;
        self.overlay.limitations.push(IndexLimitation {
            code: exhaustion.code().to_string(),
            path,
            symbol_id: None,
            reason: "repository overlay resource budget was exhausted".to_string(),
            interpretation: "the candidate overlay and reverse-dependent closure are partial"
                .to_string(),
        });
    }

    fn finish(mut self) -> RepositoryOverlay {
        for edges in self.overlay.outgoing_edges.values_mut() {
            canonicalize_edges(edges);
        }
        for edges in self.overlay.incoming_edges.values_mut() {
            canonicalize_edges(edges);
        }
        canonicalize_limitations(&mut self.overlay.limitations);
        if !self.overlay.limitations.is_empty()
            && self.overlay.completeness == Completeness::Complete
        {
            self.overlay.completeness = Completeness::Partial;
        }
        self.overlay
    }
}

fn unresolved_deleted_target_edge(base: &GraphEdge, target_symbol: &str) -> GraphEdge {
    let mut digest = Sha256::new();
    hash_component(&mut digest, b"repository-overlay-deleted-target/v1");
    hash_component(&mut digest, base.edge_id.as_bytes());
    hash_component(&mut digest, target_symbol.as_bytes());
    GraphEdge {
        edge_id: format!("{:x}", digest.finalize()),
        kind: base.kind,
        from_symbol: base.from_symbol.clone(),
        to_symbol: None,
        unresolved_target: Some(target_symbol.to_string()),
        path: base.path.clone(),
        range: base.range.clone(),
        provider_id: "repository-overlay".to_string(),
        provider_version: "repository-overlay/v1".to_string(),
        resolution: Resolution::Unresolved,
        confidence: Confidence::Low,
        limitation_code: Some("repository-overlay-target-deleted".to_string()),
    }
}

fn hash_component(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn merge_completeness(left: Completeness, right: Completeness) -> Completeness {
    match (left, right) {
        (Completeness::Unavailable, _) | (_, Completeness::Unavailable) => {
            Completeness::Unavailable
        }
        (Completeness::Partial, _) | (_, Completeness::Partial) => Completeness::Partial,
        (Completeness::Complete, Completeness::Complete) => Completeness::Complete,
    }
}

fn canonicalize_edges(edges: &mut Vec<GraphEdge>) {
    edges.sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
    edges.dedup_by(|left, right| left.edge_id == right.edge_id);
}

fn canonicalize_limitations(limitations: &mut Vec<IndexLimitation>) {
    limitations.sort_by(|left, right| limitation_key(left).cmp(&limitation_key(right)));
    limitations.dedup();
}

fn limitation_key(limitation: &IndexLimitation) -> (&str, &str, &str, &str, &str) {
    (
        limitation.code.as_str(),
        limitation.path.as_ref().map(RepoPath::as_str).unwrap_or(""),
        limitation.symbol_id.as_deref().unwrap_or(""),
        limitation.reason.as_str(),
        limitation.interpretation.as_str(),
    )
}
