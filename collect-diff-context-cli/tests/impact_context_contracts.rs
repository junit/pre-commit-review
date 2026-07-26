use collect_diff_context_cli::impact_context::contracts::ImpactContext;
use serde_json::{json, Value};

fn valid_context_value() -> Value {
    json!({
        "schema_version": 1,
        "kind": "impact_context",
        "scope": {
            "fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "source": "staged",
            "candidate_digest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        },
        "mode": "fast",
        "status": "completed",
        "providers": [{
            "provider_id": "1111111111111111",
            "provider_kind": "tree-sitter-rust",
            "provider_version": "0.24.2",
            "configuration_digest": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "status": "completed",
            "elapsed_ms": 1,
            "input_files": 1,
            "input_bytes": 12,
            "output_fact_count": 3,
            "cache_hits": 0,
            "cache_misses": 0,
            "cache_stale": 0,
            "cache_corrupt": 0,
            "limitation_ids": []
        }],
        "units": [{
            "manifest_unit_id": "file:src/lib.rs",
            "path": "src/lib.rs",
            "language": "rust",
            "content_sha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "content_bytes": 12,
            "presence": "present",
            "syntax_eligible": true,
            "syntax_status": "completed",
            "text_status": "completed",
            "parse_quality": "clean",
            "provider_ids": ["1111111111111111"],
            "changed_ranges": [{
                "start_line": 1,
                "start_column": 1,
                "end_line": 1,
                "end_column": 12,
                "start_byte": 0,
                "end_byte": 11
            }],
            "error_node_count": 0,
            "missing_node_count": 0,
            "parse_affected_ranges": [],
            "parse_affected_symbol_ids": [],
            "changed_symbol_ids": ["2222222222222222"],
            "limitation_ids": []
        }],
        "changed_symbols": [{
            "symbol_id": "2222222222222222",
            "provider_id": "1111111111111111",
            "path": "src/lib.rs",
            "language": "rust",
            "kind": "function",
            "name": "value",
            "owner": null,
            "signature": "pub fn value()",
            "visibility": "public",
            "range": {
                "start_line": 1,
                "start_column": 1,
                "end_line": 1,
                "end_column": 12,
                "start_byte": 0,
                "end_byte": 11
            },
            "confidence": "high"
        }],
        "impact_edges": [{
            "edge_id": "3333333333333333",
            "kind": "defines",
            "from_symbol": "file:src/lib.rs",
            "to_symbol": "2222222222222222",
            "unresolved_target": null,
            "path": "src/lib.rs",
            "range": {
                "start_line": 1,
                "start_column": 1,
                "end_line": 1,
                "end_column": 12,
                "start_byte": 0,
                "end_byte": 11
            },
            "provider_id": "1111111111111111",
            "resolution": "syntactic",
            "confidence": "high"
        }],
        "domain_summaries": [{
            "summary_id": "4444444444444444",
            "summary_kind": "interface-change",
            "path": "src/lib.rs",
            "symbol_id": "2222222222222222",
            "confidence": "high",
            "message": "Public function changed",
            "evidence_fact_ids": ["2222222222222222"]
        }],
        "coverage": {
            "total_candidate_files": 1,
            "changed_candidate_files": 1,
            "syntax_eligible_files": 1,
            "parsed_files": 1,
            "clean_parse_files": 1,
            "recovered_parse_files": 0,
            "degraded_parse_files": 0,
            "unsupported_files": 0,
            "resource_limited_files": 0,
            "unavailable_files": 0,
            "cache_hits": 0,
            "cache_misses": 0,
            "cache_stale": 0,
            "cache_corrupt": 0,
            "requested_graph_depth": 0,
            "reached_graph_depth": 0,
            "graph_index_completeness": "unavailable",
            "graph_query_completeness": "unavailable",
            "output_truncated": false
        },
        "limitations": [],
        "metrics": {
            "elapsed_ms": 1,
            "candidate_input_files": 1,
            "candidate_input_bytes": 12,
            "nodes_visited": 8,
            "max_nesting_depth": 2,
            "facts_emitted": 3,
            "edges_emitted": 1,
            "summaries_emitted": 1,
            "output_bytes": 1024
        }
    })
}

#[test]
fn valid_impact_context_deserializes_and_validates() {
    let context: ImpactContext = serde_json::from_value(valid_context_value()).unwrap();
    context.validate().unwrap();
}

fn assert_rejected(value: Value) {
    match serde_json::from_value::<ImpactContext>(value) {
        Ok(context) => assert!(context.validate().is_err(), "invalid context was accepted"),
        Err(_) => {}
    }
}

#[test]
fn unknown_fields_are_rejected() {
    let mut value = valid_context_value();
    value["units"][0]["unexpected"] = json!(true);
    assert_rejected(value);
}

#[test]
fn invalid_versions_kinds_and_scope_hashes_are_rejected() {
    for (pointer, replacement) in [
        ("/schema_version", json!(2)),
        ("/kind", json!("other")),
        ("/scope/fingerprint", json!("ABCDEF")),
        ("/scope/candidate_digest", json!("abc123")),
        ("/units/0/content_sha256", json!("ABCDEF")),
        ("/providers/0/configuration_digest", json!("abc123")),
    ] {
        let mut value = valid_context_value();
        *value.pointer_mut(pointer).unwrap() = replacement;
        assert_rejected(value);
    }
}

#[test]
fn absolute_parent_and_backslash_paths_are_rejected() {
    for path in [
        "/absolute.rs",
        "../escape.rs",
        "src/../escape.rs",
        "src\\lib.rs",
    ] {
        let mut value = valid_context_value();
        value["units"][0]["path"] = json!(path);
        assert_rejected(value);
    }
}

#[test]
fn zero_reversed_and_out_of_bounds_ranges_are_rejected() {
    for (field, replacement) in [
        ("start_line", json!(0)),
        ("start_column", json!(0)),
        ("end_line", json!(0)),
        ("end_column", json!(0)),
        ("start_line", json!(2)),
        ("start_byte", json!(12)),
        ("end_byte", json!(13)),
    ] {
        let mut value = valid_context_value();
        value["units"][0]["changed_ranges"][0][field] = replacement;
        assert_rejected(value);
    }
}

#[test]
fn duplicate_contract_ids_are_rejected() {
    for array_name in [
        "providers",
        "changed_symbols",
        "impact_edges",
        "domain_summaries",
    ] {
        let mut value = valid_context_value();
        let duplicate = value[array_name][0].clone();
        value[array_name].as_array_mut().unwrap().push(duplicate);
        assert_rejected(value);
    }

    let limitation = json!({
        "limitation_id": "5555555555555555",
        "code": "bounded",
        "provider_id": null,
        "path": null,
        "symbol_id": null,
        "reason": "Bounded input",
        "interpretation": "Some context may be absent",
        "improvable_in_deep_mode": true
    });
    let mut value = valid_context_value();
    value["limitations"] = json!([limitation.clone(), limitation]);
    assert_rejected(value);
}

#[test]
fn invalid_provider_status_is_rejected() {
    let mut value = valid_context_value();
    value["providers"][0]["status"] = json!("running");
    assert_rejected(value);
}

#[test]
fn syntactic_and_text_providers_cannot_claim_resolved_semantics() {
    for resolution in ["resolved-reference", "semantic", "polymorphic-candidate"] {
        let mut value = valid_context_value();
        value["impact_edges"][0]["resolution"] = json!(resolution);
        assert_rejected(value);
    }

    let mut value = valid_context_value();
    value["providers"][0]["provider_kind"] = json!("text-adapter");
    value["impact_edges"][0]["to_symbol"] = Value::Null;
    value["impact_edges"][0]["unresolved_target"] = json!("value");
    value["impact_edges"][0]["resolution"] = json!("unresolved");
    assert_rejected(value);
}

#[test]
fn edge_without_symbol_or_unresolved_target_is_rejected() {
    let mut value = valid_context_value();
    value["impact_edges"][0]["to_symbol"] = Value::Null;
    value["impact_edges"][0]["unresolved_target"] = Value::Null;
    assert_rejected(value);
}

#[test]
fn invalid_coverage_arithmetic_is_rejected() {
    for (field, replacement) in [
        ("total_candidate_files", json!(0)),
        ("changed_candidate_files", json!(2)),
        ("syntax_eligible_files", json!(2)),
        ("parsed_files", json!(2)),
        ("clean_parse_files", json!(0)),
        ("unsupported_files", json!(1)),
        ("reached_graph_depth", json!(1)),
    ] {
        let mut value = valid_context_value();
        value["coverage"][field] = replacement;
        assert_rejected(value);
    }
}

#[test]
fn review_coverage_and_verdict_fields_are_rejected() {
    for field in ["reviewed_units", "verdict", "blocking_candidate"] {
        let mut value = valid_context_value();
        value[field] = json!(true);
        assert_rejected(value);
    }
}

#[test]
fn all_top_level_statuses_serialize_to_the_contract_values() {
    for status in [
        "completed",
        "partial",
        "unavailable",
        "invalidated",
        "failed",
    ] {
        let mut value = valid_context_value();
        value["status"] = json!(status);
        let context: ImpactContext = serde_json::from_value(value).unwrap();
        assert_eq!(serde_json::to_value(context).unwrap()["status"], status);
    }
}

#[test]
fn all_provider_statuses_serialize_to_the_contract_values() {
    for status in [
        "completed",
        "partial",
        "unsupported",
        "timeout",
        "budget-exhausted",
        "stale",
        "invalid-output",
        "unavailable",
    ] {
        let mut value = valid_context_value();
        value["providers"][0]["status"] = json!(status);
        let context: ImpactContext = serde_json::from_value(value).unwrap();
        assert_eq!(
            serde_json::to_value(context).unwrap()["providers"][0]["status"],
            status
        );
    }
}
