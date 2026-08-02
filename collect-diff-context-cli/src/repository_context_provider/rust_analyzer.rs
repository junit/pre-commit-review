use super::contract::{
    report_edge_id, report_symbol_id, CallDirection, ContextSymbol, PositionEncoding,
    ProviderLimitation, ProviderLimits, ProviderRange, RustAnalyzerProjectModel, SeedContextSymbol,
    SeedKind, SeedSymbol, SemanticCallEdge,
};
use super::json_rpc::{InboundMessage, ResponseOutcome, ServerRequest};
use super::session::{ManagedLspSession, SessionError};
use super::snapshot::{
    BoundCandidateSnapshot, LspRange, SnapshotFilePath, SnapshotSourceBudget, SnapshotUriMapper,
    SourceDocument,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tree_sitter::Parser;
use url::Url;

const MAX_SEMANTIC_SCAN_NODES: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Healthy,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustAnalyzerHandshake {
    pub position_encoding: PositionEncoding,
    pub readiness: Readiness,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustAnalyzerHandshakeError {
    pub code: &'static str,
    message: String,
}

impl RustAnalyzerHandshakeError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl std::fmt::Display for RustAnalyzerHandshakeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RustAnalyzerHandshakeError {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PositionEncodingPreference {
    #[default]
    ProductionDefault,
    #[cfg(feature = "test-fixture")]
    Exclusive(PositionEncoding),
}

impl PositionEncodingPreference {
    #[cfg(feature = "test-fixture")]
    pub(super) fn preferred(encoding: PositionEncoding) -> Self {
        Self::Exclusive(encoding)
    }

    fn protocol_names(self) -> Vec<&'static str> {
        let offered = match self {
            Self::ProductionDefault => vec![PositionEncoding::Utf8, PositionEncoding::Utf16],
            #[cfg(feature = "test-fixture")]
            Self::Exclusive(encoding) => vec![encoding],
        };
        offered
            .into_iter()
            .map(|encoding| match encoding {
                PositionEncoding::Utf8 => "utf-8",
                PositionEncoding::Utf16 => "utf-16",
            })
            .collect()
    }
}

pub fn initialize_and_gate(
    session: &mut ManagedLspSession,
    snapshot: &BoundCandidateSnapshot<'_>,
    model: &RustAnalyzerProjectModel,
    target_triple: &str,
) -> Result<RustAnalyzerHandshake, RustAnalyzerHandshakeError> {
    initialize_and_gate_with_position_encoding_preference(
        session,
        snapshot,
        model,
        target_triple,
        PositionEncodingPreference::default(),
    )
}

pub(super) fn initialize_and_gate_with_position_encoding_preference(
    session: &mut ManagedLspSession,
    snapshot: &BoundCandidateSnapshot<'_>,
    model: &RustAnalyzerProjectModel,
    target_triple: &str,
    position_encoding_preference: PositionEncodingPreference,
) -> Result<RustAnalyzerHandshake, RustAnalyzerHandshakeError> {
    let root_uri = Url::from_directory_path(snapshot.root()).map_err(|_| {
        RustAnalyzerHandshakeError::new("provider-uri-invalid", "snapshot root URI is invalid")
    })?;
    let linked_project = model
        .linked_project_value_at(snapshot.root())
        .map_err(|_| {
            RustAnalyzerHandshakeError::new(
                "provider-model-invalid",
                "linked project model invalid",
            )
        })?;
    let position_encodings = position_encoding_preference.protocol_names();
    let initialize_params = json!({
        "processId": Value::Null,
        "rootUri": root_uri.clone(),
        "workspaceFolders": [{"uri": root_uri, "name": "candidate"}],
        "capabilities": {
            "general": {"positionEncodings": position_encodings},
            "workspace": {"configuration": true},
            "textDocument": {"callHierarchy": {"dynamicRegistration": false}},
            "experimental": {"serverStatusNotification": true}
        },
        "initializationOptions": {
            "linkedProjects": [linked_project],
            "cargo": {
                "buildScripts": {"enable": false},
                "noDeps": true,
                "sysroot": null,
                "sysrootSrc": null,
                "target": target_triple
            },
            "procMacro": {"enable": false},
            "checkOnSave": false
        }
    });
    let initialize_id = session
        .send_request("initialize", initialize_params)
        .map_err(session_error)?;
    let capabilities = loop {
        match session.next_message().map_err(session_error)? {
            InboundMessage::Response(response) if response.id == initialize_id => {
                match response.outcome {
                    ResponseOutcome::Result(value) => break value,
                    ResponseOutcome::Error(_) => {
                        return Err(RustAnalyzerHandshakeError::new(
                            "provider-initialize-failed",
                            "rust-analyzer initialize request failed",
                        ));
                    }
                }
            }
            InboundMessage::Request(request) => {
                handle_server_request(session, &request).map_err(session_error)?;
            }
            InboundMessage::Notification(_) | InboundMessage::Response(_) => {}
        }
    };
    session
        .send_notification("initialized", json!({}))
        .map_err(session_error)?;

    let capabilities = capabilities.get("capabilities").ok_or_else(|| {
        RustAnalyzerHandshakeError::new(
            "provider-initialize-invalid",
            "initialize result capabilities missing",
        )
    })?;
    if !capabilities
        .get("callHierarchyProvider")
        .is_some_and(|value| !value.is_null() && value != &Value::Bool(false))
    {
        return Err(RustAnalyzerHandshakeError::new(
            "provider-capability-unavailable",
            "rust-analyzer call hierarchy capability is unavailable",
        ));
    }
    let position_encoding = parse_position_encoding(capabilities.get("positionEncoding"))?;
    let (readiness, limitations) = wait_for_quiescent(session)?;
    Ok(RustAnalyzerHandshake {
        position_encoding,
        readiness,
        limitations,
    })
}

fn wait_for_quiescent(
    session: &mut ManagedLspSession,
) -> Result<(Readiness, Vec<String>), RustAnalyzerHandshakeError> {
    let mut limitations = Vec::new();
    let readiness = loop {
        match session.next_message().map_err(session_error)? {
            InboundMessage::Notification(notification)
                if notification.method == "experimental/serverStatus" =>
            {
                let params = notification.params.ok_or_else(|| {
                    RustAnalyzerHandshakeError::new(
                        "provider-readiness-invalid",
                        "rust-analyzer readiness status is malformed",
                    )
                })?;
                let quiescent = params
                    .get("quiescent")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| {
                        RustAnalyzerHandshakeError::new(
                            "provider-readiness-invalid",
                            "rust-analyzer readiness status is malformed",
                        )
                    })?;
                if !quiescent {
                    continue;
                }
                match params.get("health").and_then(Value::as_str) {
                    Some("ok") => break Readiness::Healthy,
                    Some("warning") => {
                        limitations.push("rust-analyzer-readiness-warning".to_string());
                        break Readiness::Warning;
                    }
                    Some("error") => {
                        return Err(RustAnalyzerHandshakeError::new(
                            "provider-readiness-unavailable",
                            "rust-analyzer reports unhealthy readiness",
                        ));
                    }
                    _ => {
                        return Err(RustAnalyzerHandshakeError::new(
                            "provider-readiness-invalid",
                            "rust-analyzer readiness health is malformed",
                        ));
                    }
                }
            }
            InboundMessage::Request(request) => {
                handle_server_request(session, &request).map_err(session_error)?;
            }
            InboundMessage::Notification(_) | InboundMessage::Response(_) => {}
        }
    };
    Ok((readiness, limitations))
}

fn parse_position_encoding(
    value: Option<&Value>,
) -> Result<PositionEncoding, RustAnalyzerHandshakeError> {
    let Some(value) = value else {
        return Ok(PositionEncoding::Utf16);
    };
    let value = value.as_str().ok_or_else(|| {
        RustAnalyzerHandshakeError::new(
            "provider-position-encoding-invalid",
            "rust-analyzer position encoding is malformed",
        )
    })?;
    match value {
        "utf-8" => Ok(PositionEncoding::Utf8),
        "utf-16" => Ok(PositionEncoding::Utf16),
        _ => Err(RustAnalyzerHandshakeError::new(
            "provider-position-encoding-invalid",
            "rust-analyzer position encoding is unsupported",
        )),
    }
}

fn handle_server_request(
    session: &mut ManagedLspSession,
    request: &ServerRequest,
) -> Result<(), SessionError> {
    match request.method.as_str() {
        "workspace/configuration" => {
            let items = request
                .params
                .as_ref()
                .and_then(|params| params.get("items"))
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    SessionError::new(
                        "provider-server-request-invalid",
                        "configuration request malformed",
                    )
                })?;
            session.send_server_result(&request.id, Value::Array(vec![Value::Null; items.len()]))
        }
        "window/workDoneProgress/create" => session.send_server_result(&request.id, Value::Null),
        "workspace/applyEdit" => session.send_server_result(&request.id, json!({"applied": false})),
        "client/registerCapability" => {
            let registrations = request
                .params
                .as_ref()
                .and_then(|params| params.get("registrations"))
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    SessionError::new(
                        "provider-server-request-invalid",
                        "registration request malformed",
                    )
                })?;
            let all_allowed = registrations.iter().all(|registration| {
                registration
                    .get("method")
                    .and_then(Value::as_str)
                    .is_some_and(|method| method == "workspace/didChangeConfiguration")
            });
            if all_allowed {
                session.send_server_result(&request.id, Value::Null)
            } else {
                session.send_server_error(
                    &request.id,
                    -32601,
                    "dynamic registration is not allowed",
                )
            }
        }
        _ => session.send_server_error(&request.id, -32601, "unsupported server request"),
    }
}

fn session_error(error: SessionError) -> RustAnalyzerHandshakeError {
    RustAnalyzerHandshakeError::new(error.code, "rust-analyzer session operation failed")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustAnalyzerTraversalError {
    pub code: &'static str,
    message: String,
}

impl RustAnalyzerTraversalError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl std::fmt::Display for RustAnalyzerTraversalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RustAnalyzerTraversalError {}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CallHierarchyTraversal {
    pub seed_symbols: Vec<SeedContextSymbol>,
    pub related_symbols: Vec<ContextSymbol>,
    pub edges: Vec<SemanticCallEdge>,
    pub limitations: Vec<ProviderLimitation>,
    pub source_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallHierarchyItem {
    name: String,
    kind: u32,
    #[serde(default)]
    detail: Option<String>,
    uri: Url,
    range: LspRange,
    selection_range: LspRange,
    #[serde(default)]
    data: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IncomingCall {
    from: CallHierarchyItem,
    from_ranges: Vec<LspRange>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutgoingCall {
    to: CallHierarchyItem,
    from_ranges: Vec<LspRange>,
}

#[derive(Debug, Clone)]
struct NormalizedItem {
    wire: CallHierarchyItem,
    symbol: ContextSymbol,
    path: String,
}

#[derive(Debug, Clone)]
struct TraversalNode {
    wire: CallHierarchyItem,
    symbol: ContextSymbol,
    path: String,
}

struct SourceCache<'a> {
    snapshot: &'a BoundCandidateSnapshot<'a>,
    mapper: SnapshotUriMapper,
    budget: SnapshotSourceBudget,
    sources: BTreeMap<String, Arc<[u8]>>,
    documents: BTreeMap<String, SourceDocument>,
}

impl<'a> SourceCache<'a> {
    fn new(
        snapshot: &'a BoundCandidateSnapshot<'a>,
        limits: &ProviderLimits,
    ) -> Result<Self, RustAnalyzerTraversalError> {
        let mapper = SnapshotUriMapper::new(snapshot.root()).map_err(snapshot_error)?;
        let budget =
            SnapshotSourceBudget::new(limits.max_source_file_bytes, limits.max_source_bytes)
                .map_err(snapshot_error)?;
        Ok(Self {
            snapshot,
            mapper,
            budget,
            sources: BTreeMap::new(),
            documents: BTreeMap::new(),
        })
    }

    fn load_path(&mut self, path: &str) -> Result<(), RustAnalyzerTraversalError> {
        if self.sources.contains_key(path) {
            return Ok(());
        }
        let path = SnapshotFilePath::new(path).map_err(snapshot_error)?;
        let bytes = self
            .snapshot
            .read_source(&path, &mut self.budget)
            .map_err(snapshot_error)?;
        let document = SourceDocument::new(Arc::clone(&bytes)).map_err(snapshot_error)?;
        self.sources.insert(path.as_str().to_string(), bytes);
        self.documents.insert(path.as_str().to_string(), document);
        Ok(())
    }

    fn path_for_uri(&mut self, uri: &Url) -> Result<String, RustAnalyzerTraversalError> {
        let path = self.mapper.to_file_path(uri).map_err(snapshot_error)?;
        let path = path.as_str().to_string();
        self.load_path(&path)?;
        Ok(path)
    }

    fn uri_for_path(&self, path: &str) -> Result<Url, RustAnalyzerTraversalError> {
        let path = SnapshotFilePath::new(path).map_err(snapshot_error)?;
        self.mapper.to_file_uri(&path).map_err(snapshot_error)
    }

    fn document(&self, path: &str) -> Option<&SourceDocument> {
        self.documents.get(path)
    }

    fn source(&self, path: &str) -> Option<&Arc<[u8]>> {
        self.sources.get(path)
    }

    fn consumed_bytes(&self, maximum: usize) -> usize {
        maximum.saturating_sub(self.budget.remaining_bytes())
    }
}

#[allow(clippy::too_many_arguments)]
pub fn traverse_call_hierarchy(
    session: &mut ManagedLspSession,
    snapshot: &BoundCandidateSnapshot<'_>,
    seeds: &[SeedSymbol],
    directions: &[CallDirection],
    limits: &ProviderLimits,
    encoding: PositionEncoding,
    binding_digest: &str,
    provider_id: &str,
    provider_version: &str,
) -> Result<CallHierarchyTraversal, RustAnalyzerTraversalError> {
    if seeds.is_empty() || directions.is_empty() {
        return Err(RustAnalyzerTraversalError::new(
            "provider-traversal-input-invalid",
            "call hierarchy traversal requires seeds and directions",
        ));
    }
    let mut output = CallHierarchyTraversal::default();
    let mut cache = SourceCache::new(snapshot, limits)?;
    let mut opened = BTreeSet::new();
    for seed in seeds {
        cache.load_path(&seed.path)?;
        if opened.insert(seed.path.clone()) {
            let uri = cache.uri_for_path(&seed.path)?;
            let source = cache.source(&seed.path).ok_or_else(|| {
                RustAnalyzerTraversalError::new(
                    "provider-source-missing",
                    "seed source is not available",
                )
            })?;
            let text = std::str::from_utf8(source).map_err(|_| {
                RustAnalyzerTraversalError::new(
                    "provider-source-invalid",
                    "seed source is not valid UTF-8",
                )
            })?;
            add_source_semantic_limitations(
                &mut output.limitations,
                seed,
                text,
                MAX_SEMANTIC_SCAN_NODES,
            );
            session
                .send_notification(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": "rust",
                            "version": 1,
                            "text": text,
                        }
                    }),
                )
                .map_err(traversal_session_error)?;
        }
    }

    let mut nodes = BTreeMap::<String, TraversalNode>::new();
    let mut seed_ids = BTreeSet::new();
    let mut frontiers = Vec::new();

    for seed in seeds {
        let uri = cache.uri_for_path(&seed.path)?;
        let document = cache.document(&seed.path).ok_or_else(|| {
            RustAnalyzerTraversalError::new("provider-source-missing", "seed source is missing")
        })?;
        let position = document
            .byte_to_lsp(seed.query_byte, encoding)
            .map_err(snapshot_error)?;
        let request_id = session
            .send_request(
                "textDocument/prepareCallHierarchy",
                json!({
                    "textDocument": {"uri": uri},
                    "position": position,
                }),
            )
            .map_err(traversal_session_error)?;
        let value = wait_for_response(session, request_id)?;
        let Some(value) = value else {
            add_limitation(
                &mut output.limitations,
                "seed-unresolved",
                "call hierarchy seed could not be resolved",
                None,
                Some(&seed.path),
            );
            continue;
        };
        let items: Vec<CallHierarchyItem> = serde_json::from_value(value).map_err(|_| {
            RustAnalyzerTraversalError::new(
                "provider-call-hierarchy-invalid",
                "prepare call hierarchy response is malformed",
            )
        })?;
        let mut matches = Vec::new();
        #[cfg(windows)]
        let mut mismatch_diagnostics = Vec::new();
        for item in items {
            let normalized = match normalize_item(&mut cache, item, binding_digest, encoding) {
                Ok(item) => item,
                Err(error) if is_recoverable_item_error(error.code) => {
                    add_limitation(
                        &mut output.limitations,
                        error.code,
                        "call hierarchy item was outside the candidate snapshot",
                        None,
                        Some(&seed.path),
                    );
                    continue;
                }
                Err(error) => return Err(error),
            };
            let path_matches = normalized.path == seed.path;
            let kind_matches = kind_compatible(seed.kind, normalized.symbol.kind);
            let symbol_range_contains =
                range_contains(&normalized.symbol.symbol_range, &seed.symbol_range);
            let selection_contains =
                range_contains_byte(&normalized.symbol.selection_range, seed.query_byte);
            #[cfg(windows)]
            mismatch_diagnostics.push(format!(
                "path_matches={path_matches},kind_matches={kind_matches},symbol_range_contains={symbol_range_contains},selection_contains={selection_contains},seed_path={:?},item_path={:?},seed_symbol_range={:?},item_symbol_range={:?},seed_query_byte={},item_selection_range={:?}",
                    seed.path,
                    normalized.path,
                    seed.symbol_range,
                    normalized.symbol.symbol_range,
                    seed.query_byte,
                    normalized.symbol.selection_range,
            ));
            if path_matches && kind_matches && symbol_range_contains && selection_contains {
                matches.push(normalized);
            }
        }
        if matches.is_empty() {
            #[cfg(windows)]
            let message = format!(
                "[DEBUG-task8b-seed-match] {}",
                mismatch_diagnostics.join(";")
            );
            #[cfg(not(windows))]
            let message = "call hierarchy seed did not resolve to exactly one symbol".to_string();
            add_limitation(
                &mut output.limitations,
                "seed-unresolved",
                &message,
                None,
                Some(&seed.path),
            );
            continue;
        }
        if matches.len() > 1 {
            add_limitation(
                &mut output.limitations,
                "seed-ambiguous",
                "call hierarchy seed matched multiple symbols",
                None,
                Some(&seed.path),
            );
            continue;
        }
        let normalized = matches.pop().expect("one matching seed");
        if !seed_ids.insert(normalized.symbol.symbol_id.clone()) {
            add_limitation(
                &mut output.limitations,
                "seed-symbol-duplicate",
                "multiple seeds resolved to one provider symbol",
                None,
                Some(&seed.path),
            );
            continue;
        }
        let node = TraversalNode {
            wire: normalized.wire,
            symbol: normalized.symbol.clone(),
            path: normalized.path,
        };
        nodes.insert(node.symbol.symbol_id.clone(), node.clone());
        output.seed_symbols.push(SeedContextSymbol {
            changed_symbol_id: seed.changed_symbol_id.clone(),
            symbol: normalized.symbol,
        });
        frontiers.push(node);
    }

    let mut visited = BTreeSet::<(CallDirection, String)>::new();
    let mut edge_ids = BTreeSet::new();
    let mut directions = directions.to_vec();
    directions.sort();
    directions.dedup();
    let mut frontier = frontiers;
    for depth in 0..limits.max_depth {
        frontier.sort_by(|left, right| left.symbol.symbol_id.cmp(&right.symbol.symbol_id));
        let mut next_frontier = Vec::new();
        for current in &frontier {
            for direction in &directions {
                if !visited.insert((*direction, current.symbol.symbol_id.clone())) {
                    continue;
                }
                let value = match request_calls(session, current, *direction) {
                    Ok(value) => value,
                    Err(error) if error.code == "provider-server-error" => {
                        add_limitation(
                            &mut output.limitations,
                            "call-hierarchy-request-failed",
                            "call hierarchy request failed",
                            None,
                            Some(&current.path),
                        );
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let calls = if value.is_null() {
                    Vec::new()
                } else if *direction == CallDirection::Incoming {
                    serde_json::from_value::<Vec<IncomingCall>>(value).map_err(|_| {
                        RustAnalyzerTraversalError::new(
                            "provider-call-hierarchy-invalid",
                            "incoming call hierarchy response is malformed",
                        )
                    })?
                } else {
                    let calls =
                        serde_json::from_value::<Vec<OutgoingCall>>(value).map_err(|_| {
                            RustAnalyzerTraversalError::new(
                                "provider-call-hierarchy-invalid",
                                "outgoing call hierarchy response is malformed",
                            )
                        })?;
                    calls
                        .into_iter()
                        .map(|call| IncomingCall {
                            from: call.to,
                            from_ranges: call.from_ranges,
                        })
                        .collect()
                };
                let mut normalized_calls = Vec::new();
                for call in calls {
                    let normalized =
                        match normalize_item(&mut cache, call.from, binding_digest, encoding) {
                            Ok(item) => item,
                            Err(error) if is_recoverable_item_error(error.code) => {
                                add_limitation(
                                    &mut output.limitations,
                                    error.code,
                                    "call hierarchy item was outside the candidate snapshot",
                                    None,
                                    Some(&current.path),
                                );
                                continue;
                            }
                            Err(error) => return Err(error),
                        };
                    let mut ranges = Vec::new();
                    let range_path = if *direction == CallDirection::Incoming {
                        normalized.path.as_str()
                    } else {
                        current.path.as_str()
                    };
                    let Some(document) = cache.document(range_path) else {
                        continue;
                    };
                    for range in call.from_ranges {
                        match document.lsp_range_to_provider(range, encoding) {
                            Ok(range) => ranges.push(range),
                            Err(_) => add_limitation(
                                &mut output.limitations,
                                "call-range-invalid",
                                "call hierarchy range was invalid",
                                None,
                                Some(range_path),
                            ),
                        }
                    }
                    ranges.sort_by_key(|range| {
                        (
                            range.start_byte,
                            range.end_byte,
                            range.start_line,
                            range.start_column,
                        )
                    });
                    ranges.dedup();
                    if !ranges.is_empty() {
                        normalized_calls.push((normalized, ranges));
                    }
                }
                normalized_calls.sort_by(|left, right| {
                    left.0
                        .symbol
                        .symbol_id
                        .cmp(&right.0.symbol.symbol_id)
                        .then_with(|| compare_range_lists(&left.1, &right.1))
                });
                for (normalized, ranges) in normalized_calls {
                    let target_id = normalized.symbol.symbol_id.clone();
                    let target_node = if let Some(node) = nodes.get(&target_id) {
                        node.clone()
                    } else {
                        if nodes.len() >= limits.max_nodes {
                            add_limitation(
                                &mut output.limitations,
                                "node-budget-exhausted",
                                "call hierarchy node budget was exhausted",
                                None,
                                Some(&normalized.path),
                            );
                            continue;
                        }
                        let node = TraversalNode {
                            wire: normalized.wire,
                            symbol: normalized.symbol.clone(),
                            path: normalized.path,
                        };
                        nodes.insert(target_id.clone(), node.clone());
                        if !seed_ids.contains(&target_id) {
                            output.related_symbols.push(node.symbol.clone());
                        }
                        node
                    };
                    for call_range in ranges {
                        if output.edges.len() >= limits.max_edges
                            || output.edges.len() >= limits.max_call_ranges
                        {
                            add_limitation(
                                &mut output.limitations,
                                "edge-budget-exhausted",
                                "call hierarchy edge budget was exhausted",
                                None,
                                Some(&current.path),
                            );
                            break;
                        }
                        let (from, to, call_path) = if *direction == CallDirection::Incoming {
                            (
                                &target_node.symbol,
                                &current.symbol,
                                target_node.path.as_str(),
                            )
                        } else {
                            (&current.symbol, &target_node.symbol, current.path.as_str())
                        };
                        let edge_id = report_edge_id(
                            binding_digest,
                            &from.symbol_id,
                            &to.symbol_id,
                            call_path,
                            &call_range,
                        )
                        .map_err(|_| {
                            RustAnalyzerTraversalError::new(
                                "provider-edge-id-invalid",
                                "call hierarchy edge ID could not be generated",
                            )
                        })?;
                        if !edge_ids.insert(edge_id.clone()) {
                            continue;
                        }
                        output.edges.push(SemanticCallEdge {
                            edge_id,
                            from_symbol: from.symbol_id.clone(),
                            to_symbol: to.symbol_id.clone(),
                            call_site_path: call_path.to_string(),
                            call_site_range: call_range,
                            kind: "calls".to_string(),
                            resolution: "semantic".to_string(),
                            confidence: "high".to_string(),
                            provider_id: provider_id.to_string(),
                            provider_version: provider_version.to_string(),
                        });
                    }
                    if depth + 1 < limits.max_depth
                        && target_id != current.symbol.symbol_id
                        && !next_frontier
                            .iter()
                            .any(|node: &TraversalNode| node.symbol.symbol_id == target_id)
                    {
                        next_frontier.push(target_node);
                    }
                }
            }
        }
        frontier = next_frontier;
        if frontier.is_empty() {
            break;
        }
    }

    output
        .seed_symbols
        .sort_by(|left, right| left.symbol.symbol_id.cmp(&right.symbol.symbol_id));
    output
        .related_symbols
        .sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
    output
        .related_symbols
        .dedup_by(|left, right| left.symbol_id == right.symbol_id);
    output
        .edges
        .sort_by(|left, right| left.edge_id.cmp(&right.edge_id));
    output.limitations.sort();
    output.limitations.dedup();
    output.source_bytes = cache.consumed_bytes(limits.max_source_bytes);
    Ok(output)
}

fn add_source_semantic_limitations(
    limitations: &mut Vec<ProviderLimitation>,
    seed: &SeedSymbol,
    source: &str,
    maximum_nodes: usize,
) {
    let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        add_limitation(
            limitations,
            "source-syntax-partial",
            "seed source could not be configured for bounded syntax analysis",
            None,
            Some(&seed.path),
        );
        return;
    }
    let Some(tree) = parser.parse(source, None) else {
        add_limitation(
            limitations,
            "source-syntax-partial",
            "seed source could not be parsed for bounded syntax analysis",
            None,
            Some(&seed.path),
        );
        return;
    };
    let root = tree.root_node();
    let Some(mut seed_node) =
        root.descendant_for_byte_range(seed.query_byte, seed.query_byte.saturating_add(1))
    else {
        add_limitation(
            limitations,
            "source-syntax-partial",
            "seed syntax could not be located in the bounded source",
            None,
            Some(&seed.path),
        );
        return;
    };
    while seed_node.kind() != "function_item" {
        let Some(parent) = seed_node.parent() else {
            add_limitation(
                limitations,
                "source-syntax-partial",
                "seed function syntax could not be located in the bounded source",
                None,
                Some(&seed.path),
            );
            return;
        };
        seed_node = parent;
    }

    let mut saw_dynamic_type = false;
    let mut saw_macro_invocation = false;
    let mut observed_nodes = 0_usize;
    let mut cursor = seed_node.walk();
    loop {
        observed_nodes = observed_nodes.saturating_add(1);
        match cursor.node().kind() {
            "dynamic_type" => saw_dynamic_type = true,
            "macro_invocation" => saw_macro_invocation = true,
            _ => {}
        }
        if observed_nodes >= maximum_nodes {
            add_limitation(
                limitations,
                "semantic-scan-budget-exhausted",
                "seed syntax exceeded the bounded semantic scan budget",
                None,
                Some(&seed.path),
            );
            break;
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                if saw_dynamic_type {
                    add_limitation(
                        limitations,
                        "dynamic-dispatch-partial",
                        "dynamic dispatch prevents a complete call hierarchy",
                        None,
                        Some(&seed.path),
                    );
                }
                if saw_macro_invocation {
                    add_limitation(
                        limitations,
                        "macro-invocation-partial",
                        "macro expansion prevents a complete call hierarchy",
                        None,
                        Some(&seed.path),
                    );
                }
                if tree.root_node().has_error() {
                    add_limitation(
                        limitations,
                        "source-syntax-partial",
                        "seed source contains syntax errors",
                        None,
                        Some(&seed.path),
                    );
                }
                return;
            }
        }
    }
}

fn request_calls(
    session: &mut ManagedLspSession,
    current: &TraversalNode,
    direction: CallDirection,
) -> Result<Value, RustAnalyzerTraversalError> {
    let method = match direction {
        CallDirection::Incoming => "callHierarchy/incomingCalls",
        CallDirection::Outgoing => "callHierarchy/outgoingCalls",
    };
    let id = session
        .send_request(method, json!({"item": current.wire.clone()}))
        .map_err(traversal_session_error)?;
    Ok(wait_for_response(session, id)?.unwrap_or(Value::Null))
}

fn wait_for_response(
    session: &mut ManagedLspSession,
    request_id: u64,
) -> Result<Option<Value>, RustAnalyzerTraversalError> {
    loop {
        match session.next_message().map_err(traversal_session_error)? {
            InboundMessage::Response(response) if response.id == request_id => {
                return match response.outcome {
                    ResponseOutcome::Result(value) => {
                        Ok(if value.is_null() { None } else { Some(value) })
                    }
                    ResponseOutcome::Error(_) => Err(RustAnalyzerTraversalError::new(
                        "provider-server-error",
                        "rust-analyzer returned a JSON-RPC error",
                    )),
                };
            }
            InboundMessage::Request(request) => {
                handle_server_request(session, &request).map_err(traversal_session_error)?;
            }
            InboundMessage::Notification(_) | InboundMessage::Response(_) => {}
        }
    }
}

fn normalize_item(
    cache: &mut SourceCache<'_>,
    item: CallHierarchyItem,
    binding_digest: &str,
    encoding: PositionEncoding,
) -> Result<NormalizedItem, RustAnalyzerTraversalError> {
    if item.name.is_empty() || item.name.len() > 1_024 {
        return Err(RustAnalyzerTraversalError::new(
            "provider-call-item-invalid",
            "call hierarchy item name is invalid",
        ));
    }
    if item
        .detail
        .as_ref()
        .is_some_and(|detail| detail.len() > 4_096)
    {
        return Err(RustAnalyzerTraversalError::new(
            "provider-call-item-invalid",
            "call hierarchy item detail is unbounded",
        ));
    }
    if let Some(data) = item.data.as_ref() {
        let bytes = serde_json::to_vec(data).map_err(|_| {
            RustAnalyzerTraversalError::new(
                "provider-call-item-invalid",
                "call hierarchy item data is invalid",
            )
        })?;
        if bytes.len() > 64 * 1024 {
            return Err(RustAnalyzerTraversalError::new(
                "provider-call-item-invalid",
                "call hierarchy item data is unbounded",
            ));
        }
    }
    let path = cache.path_for_uri(&item.uri)?;
    let document = cache.document(&path).ok_or_else(|| {
        RustAnalyzerTraversalError::new(
            "provider-source-missing",
            "call hierarchy source is missing",
        )
    })?;
    let symbol_range = document
        .lsp_range_to_provider(item.range, encoding)
        .map_err(snapshot_error)?;
    let selection_range = document
        .lsp_range_to_provider(item.selection_range, encoding)
        .map_err(snapshot_error)?;
    let kind = seed_kind_for_lsp(item.kind).ok_or_else(|| {
        RustAnalyzerTraversalError::new(
            "provider-call-kind-invalid",
            "call hierarchy item kind is not supported",
        )
    })?;
    let symbol_id = report_symbol_id(
        binding_digest,
        &path,
        kind,
        &item.name,
        &symbol_range,
        &selection_range,
    )
    .map_err(|_| {
        RustAnalyzerTraversalError::new(
            "provider-call-item-invalid",
            "call hierarchy item range is invalid",
        )
    })?;
    let name = item.name.clone();
    Ok(NormalizedItem {
        wire: item,
        symbol: ContextSymbol {
            symbol_id,
            path: path.clone(),
            kind,
            name,
            symbol_range,
            selection_range,
        },
        path,
    })
}

fn seed_kind_for_lsp(kind: u32) -> Option<SeedKind> {
    match kind {
        6 | 9 => Some(SeedKind::Method),
        12 => Some(SeedKind::Function),
        _ => None,
    }
}

fn kind_compatible(expected: SeedKind, actual: SeedKind) -> bool {
    match expected {
        SeedKind::Function | SeedKind::FunctionDeclaration => actual == SeedKind::Function,
        SeedKind::Method | SeedKind::MethodDeclaration => actual == SeedKind::Method,
        SeedKind::AssociatedFunction | SeedKind::AssociatedFunctionDeclaration => {
            matches!(actual, SeedKind::Function | SeedKind::Method)
        }
    }
}

fn range_contains(outer: &ProviderRange, inner: &ProviderRange) -> bool {
    outer.start_byte <= inner.start_byte
        && inner.end_byte <= outer.end_byte
        && (outer.start_line, outer.start_column) <= (inner.start_line, inner.start_column)
        && (inner.end_line, inner.end_column) <= (outer.end_line, outer.end_column)
}

fn range_contains_byte(range: &ProviderRange, byte: usize) -> bool {
    range.start_byte <= byte && byte < range.end_byte
}

fn compare_range_lists(left: &[ProviderRange], right: &[ProviderRange]) -> std::cmp::Ordering {
    left.iter()
        .map(|range| {
            (
                range.start_byte,
                range.end_byte,
                range.start_line,
                range.start_column,
                range.end_line,
                range.end_column,
            )
        })
        .cmp(right.iter().map(|range| {
            (
                range.start_byte,
                range.end_byte,
                range.start_line,
                range.start_column,
                range.end_line,
                range.end_column,
            )
        }))
}

fn is_recoverable_item_error(code: &str) -> bool {
    matches!(
        code,
        "provider-uri-invalid"
            | "provider-uri-stale"
            | "provider-uri-outside-snapshot"
            | "provider-uri-non-utf8"
            | "provider-source-missing"
            | "provider-source-type-invalid"
            | "provider-source-invalid"
            | "provider-source-encoding-invalid"
            | "provider-position-invalid"
            | "provider-position-normalized"
            | "provider-range-invalid"
            | "provider-range-mismatch"
            | "provider-call-kind-invalid"
    )
}

fn add_limitation(
    limitations: &mut Vec<ProviderLimitation>,
    code: &str,
    message: &str,
    changed_symbol_id: Option<&str>,
    path: Option<&str>,
) {
    limitations.push(ProviderLimitation {
        code: code.chars().take(128).collect(),
        message: message.chars().take(4_096).collect(),
        changed_symbol_id: changed_symbol_id.map(str::to_string),
        path: path.map(str::to_string),
    });
}

fn snapshot_error(_error: impl std::fmt::Display) -> RustAnalyzerTraversalError {
    RustAnalyzerTraversalError::new(
        "provider-snapshot-boundary",
        "snapshot boundary rejected data",
    )
}

fn traversal_session_error(error: SessionError) -> RustAnalyzerTraversalError {
    RustAnalyzerTraversalError::new(error.code, "rust-analyzer session operation failed")
}
