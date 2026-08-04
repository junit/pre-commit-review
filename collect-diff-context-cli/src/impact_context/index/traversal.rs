use crate::impact_context::cache::sqlite_generation::{
    RepositoryGraphError, RepositoryGraphReader,
};
use crate::impact_context::contracts::{Completeness, Confidence, EdgeKind};
use crate::impact_context::index::model::{GraphEdge, IndexLimitation};
use crate::impact_context::index::overlay::RepositoryOverlay;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TraversalDirection {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraversalRequest {
    pub roots: Vec<String>,
    pub directions: BTreeSet<TraversalDirection>,
    pub edge_kinds: BTreeSet<EdgeKind>,
    pub maximum_depth: usize,
    pub maximum_rows: usize,
    pub maximum_nodes: usize,
    pub maximum_edges: usize,
    pub maximum_bytes: usize,
    pub deadline: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraversalResult {
    pub edges: Vec<GraphEdge>,
    pub reached_depth: usize,
    pub rows_read: usize,
    pub nodes_visited: usize,
    pub bytes_read: usize,
    pub index_completeness: Completeness,
    pub query_completeness: Completeness,
    pub output_truncated: bool,
    pub limitations: Vec<IndexLimitation>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraversalError {
    pub code: &'static str,
    pub message: String,
}

impl TraversalError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TraversalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TraversalError {}

impl From<RepositoryGraphError> for TraversalError {
    fn from(error: RepositoryGraphError) -> Self {
        Self::new(error.code, error.message)
    }
}

pub fn traverse_repository_graph(
    base: &RepositoryGraphReader,
    overlay: Option<&RepositoryOverlay>,
    request: &TraversalRequest,
) -> Result<TraversalResult, TraversalError> {
    validate_request(request)?;
    if let Some(overlay) = overlay {
        let base_key = base.identity().generation_key().map_err(|error| {
            TraversalError::new(
                "traversal-base-identity-invalid",
                format!("cannot identify base repository graph: {error}"),
            )
        })?;
        if overlay.base_generation_key != base_key {
            return Err(TraversalError::new(
                "traversal-overlay-base-mismatch",
                "repository overlay does not belong to the opened base generation",
            ));
        }
    }

    TraversalBuilder::new(base, overlay, request).run()
}

struct TraversalBuilder<'a> {
    base: &'a RepositoryGraphReader,
    overlay: Option<&'a RepositoryOverlay>,
    request: &'a TraversalRequest,
    started: Instant,
    output_edges: BTreeMap<String, GraphEdge>,
    overlay_edge_ids: BTreeSet<String>,
    output_bytes: usize,
    seen_nodes: BTreeSet<String>,
    visited_lookups: BTreeSet<(TraversalDirection, String, EdgeKind)>,
    rows_read: usize,
    reached_depth: usize,
    query_completeness: Completeness,
    output_truncated: bool,
    query_halted: bool,
    limitations: Vec<IndexLimitation>,
}

impl<'a> TraversalBuilder<'a> {
    fn new(
        base: &'a RepositoryGraphReader,
        overlay: Option<&'a RepositoryOverlay>,
        request: &'a TraversalRequest,
    ) -> Self {
        let overlay_edge_ids = overlay
            .into_iter()
            .flat_map(|overlay| {
                overlay
                    .outgoing_edges
                    .values()
                    .chain(overlay.incoming_edges.values())
                    .flatten()
            })
            .map(|edge| edge.edge_id.clone())
            .collect();
        Self {
            base,
            overlay,
            request,
            started: Instant::now(),
            output_edges: BTreeMap::new(),
            overlay_edge_ids,
            output_bytes: 0,
            seen_nodes: BTreeSet::new(),
            visited_lookups: BTreeSet::new(),
            rows_read: 0,
            reached_depth: 0,
            query_completeness: Completeness::Complete,
            output_truncated: false,
            query_halted: false,
            limitations: overlay
                .map(|overlay| overlay.limitations.clone())
                .unwrap_or_default(),
        }
    }

    fn run(mut self) -> Result<TraversalResult, TraversalError> {
        let mut frontier = BTreeSet::new();
        for root in self.request.roots.iter().cloned().collect::<BTreeSet<_>>() {
            if self.seen_nodes.len() >= self.request.maximum_nodes {
                self.mark_query_partial(
                    "index-node-budget-exhausted",
                    Some(root),
                    "the traversal node budget was exhausted",
                    "additional repository graph nodes were not visited",
                );
                break;
            }
            self.seen_nodes.insert(root.clone());
            frontier.insert(root);
        }

        let mut depth = 0;
        while !frontier.is_empty() && depth < self.request.maximum_depth {
            if !self.check_deadline(None) {
                break;
            }
            let current = std::mem::take(&mut frontier);
            let mut next = BTreeSet::new();
            for symbol in current {
                if !self.check_deadline(Some(symbol.clone())) {
                    break;
                }
                for direction in &self.request.directions {
                    if !self.check_deadline(Some(symbol.clone())) {
                        break;
                    }
                    let relationships = self.lookup(*direction, &symbol)?;
                    if !relationships.is_empty() {
                        self.reached_depth = self.reached_depth.max(depth + 1);
                    }
                    for edge in relationships {
                        self.consider_output(&edge)?;
                        if let Some(neighbor) = neighbor(*direction, &edge) {
                            self.consider_neighbor(neighbor, &mut next);
                        }
                    }
                    if self.query_halted {
                        break;
                    }
                }
                if self.query_halted {
                    break;
                }
            }
            if self.query_halted {
                break;
            }
            frontier = next;
            depth += 1;
        }

        if !frontier.is_empty() && depth >= self.request.maximum_depth {
            self.mark_query_partial(
                "index-graph-depth-budget-exhausted",
                frontier.iter().next().cloned(),
                "the traversal depth budget was exhausted",
                "relationships beyond the reached depth were not queried",
            );
        }

        let index_completeness = self.index_completeness();
        let elapsed_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut edges: Vec<_> = self.output_edges.into_values().collect();
        edges.sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
        canonicalize_limitations(&mut self.limitations);
        Ok(TraversalResult {
            edges,
            reached_depth: self.reached_depth,
            rows_read: self.rows_read,
            nodes_visited: self.seen_nodes.len(),
            bytes_read: self.output_bytes,
            index_completeness,
            query_completeness: self.query_completeness,
            output_truncated: self.output_truncated,
            limitations: self.limitations,
            elapsed_ms,
        })
    }

    fn lookup(
        &mut self,
        direction: TraversalDirection,
        symbol: &str,
    ) -> Result<Vec<GraphEdge>, TraversalError> {
        let unvisited_kinds: BTreeSet<_> = self
            .request
            .edge_kinds
            .iter()
            .copied()
            .filter(|kind| {
                !self
                    .visited_lookups
                    .contains(&(direction, symbol.to_string(), *kind))
            })
            .collect();
        if unvisited_kinds.is_empty() {
            return Ok(Vec::new());
        }
        for kind in &unvisited_kinds {
            self.visited_lookups
                .insert((direction, symbol.to_string(), *kind));
        }

        let mut merged = BTreeMap::new();
        if let Some(overlay) = self.overlay {
            let relationships = match direction {
                TraversalDirection::Incoming => overlay.incoming_edges.get(symbol),
                TraversalDirection::Outgoing => overlay.outgoing_edges.get(symbol),
            };
            if let Some(relationships) = relationships {
                for edge in relationships {
                    if unvisited_kinds.contains(&edge.kind) {
                        merge_edge(&mut merged, edge.clone())?;
                    }
                }
            }
        }

        if !self.base_lookup_is_fully_replaced(direction, symbol) {
            let base_rows = self.read_base(direction, symbol)?;
            for edge in base_rows {
                if unvisited_kinds.contains(&edge.kind) && !self.base_edge_is_suppressed(&edge) {
                    merge_edge(&mut merged, edge)?;
                }
            }
        }
        Ok(merged.into_values().collect())
    }

    fn read_base(
        &mut self,
        direction: TraversalDirection,
        symbol: &str,
    ) -> Result<Vec<GraphEdge>, TraversalError> {
        let remaining = self.request.maximum_rows.saturating_sub(self.rows_read);
        if remaining == 0 {
            self.mark_query_partial(
                "index-query-row-budget-exhausted",
                Some(symbol.to_string()),
                "the traversal row budget was exhausted",
                "additional indexed relationships were not queried",
            );
            self.query_halted = true;
            return Ok(Vec::new());
        }
        let reader_limit = self.base.maximum_rows_per_query();
        let limit = remaining.min(reader_limit);
        let rows = match direction {
            TraversalDirection::Incoming => self.base.incoming(symbol, limit),
            TraversalDirection::Outgoing => self.base.outgoing(symbol, limit),
        }?;
        self.rows_read = self.rows_read.checked_add(rows.len()).ok_or_else(|| {
            TraversalError::new(
                "traversal-row-count-overflow",
                "repository traversal row count overflowed",
            )
        })?;
        if rows.len() == limit {
            let (code, reason) = if limit == remaining {
                (
                    "index-query-row-budget-exhausted",
                    "the traversal row budget was exhausted",
                )
            } else {
                (
                    "index-query-row-limit-reached",
                    "an indexed graph lookup reached the immutable reader row limit",
                )
            };
            self.mark_query_partial(
                code,
                Some(symbol.to_string()),
                reason,
                "additional indexed relationships may exist",
            );
            self.query_halted = true;
        }
        Ok(rows)
    }

    fn base_lookup_is_fully_replaced(&self, direction: TraversalDirection, symbol: &str) -> bool {
        if direction != TraversalDirection::Outgoing {
            return false;
        }
        self.overlay.is_some_and(|overlay| {
            overlay
                .symbols
                .get(symbol)
                .is_some_and(|replacement| overlay.path_tombstones.contains(&replacement.path))
        })
    }

    fn base_edge_is_suppressed(&self, edge: &GraphEdge) -> bool {
        self.overlay.is_some_and(|overlay| {
            overlay.suppressed_base_edge_ids.contains(&edge.edge_id)
                || overlay.path_tombstones.contains(&edge.path)
                || self.overlay_edge_ids.contains(&edge.edge_id)
        })
    }

    fn consider_neighbor(&mut self, neighbor: String, next: &mut BTreeSet<String>) {
        if self.seen_nodes.contains(&neighbor) {
            return;
        }
        if self.seen_nodes.len() >= self.request.maximum_nodes {
            self.mark_query_partial(
                "index-node-budget-exhausted",
                Some(neighbor),
                "the traversal node budget was exhausted",
                "additional repository graph nodes were not visited",
            );
            return;
        }
        self.seen_nodes.insert(neighbor.clone());
        next.insert(neighbor);
    }

    fn consider_output(&mut self, edge: &GraphEdge) -> Result<(), TraversalError> {
        if let Some(existing) = self.output_edges.get(&edge.edge_id) {
            if !candidate_is_preferred(edge, existing)? {
                return Ok(());
            }
            let existing_bytes = canonical_edge_bytes(existing)?.len();
            let replacement_bytes = canonical_edge_bytes(edge)?.len();
            let retained_bytes = self.output_bytes.saturating_sub(existing_bytes);
            let next = retained_bytes
                .checked_add(replacement_bytes)
                .ok_or_else(|| {
                    TraversalError::new(
                        "traversal-byte-count-overflow",
                        "repository traversal byte count overflowed",
                    )
                })?;
            if next > self.request.maximum_bytes {
                self.mark_output_truncated(
                    "index-output-byte-budget-exhausted",
                    Some(edge.from_symbol.clone()),
                    "the traversal byte output budget was exhausted",
                    "a higher-confidence duplicate relationship could not replace the retained row",
                );
                return Ok(());
            }
            self.output_bytes = next;
            self.output_edges.insert(edge.edge_id.clone(), edge.clone());
            return Ok(());
        }
        if self.output_edges.len() >= self.request.maximum_edges {
            self.mark_output_truncated(
                "index-edge-budget-exhausted",
                Some(edge.from_symbol.clone()),
                "the traversal edge output budget was exhausted",
                "additional queried relationships were omitted from output",
            );
            return Ok(());
        }
        let bytes = canonical_edge_bytes(edge)?;
        let next = self.output_bytes.checked_add(bytes.len()).ok_or_else(|| {
            TraversalError::new(
                "traversal-byte-count-overflow",
                "repository traversal byte count overflowed",
            )
        })?;
        if next > self.request.maximum_bytes {
            self.mark_output_truncated(
                "index-output-byte-budget-exhausted",
                Some(edge.from_symbol.clone()),
                "the traversal byte output budget was exhausted",
                "additional queried relationships were omitted from output",
            );
            return Ok(());
        }
        self.output_bytes = next;
        self.output_edges.insert(edge.edge_id.clone(), edge.clone());
        Ok(())
    }

    fn check_deadline(&mut self, symbol_id: Option<String>) -> bool {
        if self.started.elapsed() >= self.request.deadline {
            self.mark_query_partial(
                "index-deadline-exhausted",
                symbol_id,
                "the traversal deadline was exhausted",
                "the bounded repository graph query stopped before completion",
            );
            self.query_halted = true;
            false
        } else {
            true
        }
    }

    fn index_completeness(&self) -> Completeness {
        let overlay = self
            .overlay
            .map(|overlay| overlay.completeness)
            .unwrap_or(Completeness::Complete);
        merge_completeness(self.base.completeness(), overlay)
    }

    fn mark_query_partial(
        &mut self,
        code: &str,
        symbol_id: Option<String>,
        reason: &str,
        interpretation: &str,
    ) {
        self.query_completeness = Completeness::Partial;
        self.limitations.push(IndexLimitation {
            code: code.to_string(),
            path: None,
            symbol_id,
            reason: reason.to_string(),
            interpretation: interpretation.to_string(),
        });
    }

    fn mark_output_truncated(
        &mut self,
        code: &str,
        symbol_id: Option<String>,
        reason: &str,
        interpretation: &str,
    ) {
        self.output_truncated = true;
        self.limitations.push(IndexLimitation {
            code: code.to_string(),
            path: None,
            symbol_id,
            reason: reason.to_string(),
            interpretation: interpretation.to_string(),
        });
    }
}

fn validate_request(request: &TraversalRequest) -> Result<(), TraversalError> {
    for root in &request.roots {
        if root.len() != 64
            || !root
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(TraversalError::new(
                "traversal-root-invalid",
                "traversal roots must be 64 lowercase hex symbol ids",
            ));
        }
    }
    Ok(())
}

fn neighbor(direction: TraversalDirection, edge: &GraphEdge) -> Option<String> {
    match direction {
        TraversalDirection::Incoming => Some(edge.from_symbol.clone()),
        TraversalDirection::Outgoing => edge.to_symbol.clone(),
    }
}

fn merge_edge(
    merged: &mut BTreeMap<String, GraphEdge>,
    candidate: GraphEdge,
) -> Result<(), TraversalError> {
    match merged.get(&candidate.edge_id) {
        Some(existing) if !candidate_is_preferred(&candidate, existing)? => {}
        _ => {
            merged.insert(candidate.edge_id.clone(), candidate);
        }
    }
    Ok(())
}

fn candidate_is_preferred(
    candidate: &GraphEdge,
    existing: &GraphEdge,
) -> Result<bool, TraversalError> {
    let candidate_rank = confidence_rank(candidate.confidence);
    let existing_rank = confidence_rank(existing.confidence);
    if candidate_rank != existing_rank {
        return Ok(candidate_rank > existing_rank);
    }
    Ok(canonical_edge_bytes(candidate)? < canonical_edge_bytes(existing)?)
}

fn confidence_rank(confidence: Confidence) -> u8 {
    match confidence {
        Confidence::High => 3,
        Confidence::Medium => 2,
        Confidence::Low => 1,
    }
}

fn canonical_edge_bytes(edge: &GraphEdge) -> Result<Vec<u8>, TraversalError> {
    serde_json::to_vec(edge).map_err(|error| {
        TraversalError::new(
            "traversal-edge-serialization-failed",
            format!("cannot serialize repository graph edge: {error}"),
        )
    })
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

fn canonicalize_limitations(limitations: &mut Vec<IndexLimitation>) {
    limitations.sort_by(|left, right| limitation_key(left).cmp(&limitation_key(right)));
    limitations.dedup();
}

fn limitation_key(limitation: &IndexLimitation) -> (&str, &str, &str, &str, &str) {
    (
        limitation.code.as_str(),
        limitation
            .path
            .as_ref()
            .map(crate::candidate::RepoPath::as_str)
            .unwrap_or(""),
        limitation.symbol_id.as_deref().unwrap_or(""),
        limitation.reason.as_str(),
        limitation.interpretation.as_str(),
    )
}
