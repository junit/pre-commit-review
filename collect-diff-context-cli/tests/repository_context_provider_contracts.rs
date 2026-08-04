use collect_diff_context_cli::repository_context_provider::contract::*;
use collect_diff_context_cli::review_scope::ReviewSource;
use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn trusted_path(path: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\trusted").join(path)
    } else {
        PathBuf::from("/trusted").join(path)
    }
}

fn provider_range(start: usize, end: usize) -> ProviderRange {
    ProviderRange {
        format: ProviderRangeFormat::Utf8ByteColumnsEndExclusiveV1,
        start_line: 1,
        start_column: start as u32 + 1,
        end_line: 1,
        end_column: end as u32 + 1,
        start_byte: start,
        end_byte: end,
    }
}

fn valid_profile() -> AuthorizedProviderProfile {
    let mut profile = AuthorizedProviderProfile {
        schema_version: 1,
        kind: "repository_context_provider_profile".to_string(),
        provider_kind: "rust-analyzer".to_string(),
        provider_version: "2026-07-27".to_string(),
        executable_sha256: digest('4'),
        configuration_sha256: digest('0'),
        target_triple: "x86_64-unknown-linux-gnu".to_string(),
        toolchain_mode: "none".to_string(),
        arguments: Vec::new(),
        hardening: ProviderHardening {
            cargo_build_scripts: false,
            cargo_no_deps: true,
            cargo_sysroot: None,
            cargo_sysroot_src: None,
            proc_macro: false,
            check_on_save: false,
            workspace_discovery: false,
            empty_path: true,
            server_status_notification: true,
        },
        maximum_limits: ProviderLimits::maximum(),
    };
    profile.configuration_sha256 = profile.canonical_configuration_sha256();
    profile
}

fn valid_project_model() -> RustAnalyzerProjectModel {
    let mut model = RustAnalyzerProjectModel {
        schema_version: 1,
        algorithm: "rust-analyzer-linked-project-v1".to_string(),
        digest: digest('0'),
        target_triple: "x86_64-unknown-linux-gnu".to_string(),
        crates: vec![
            RustAnalyzerCrate {
                crate_id: "crate-a".to_string(),
                root_module: "src/lib.rs".to_string(),
                edition: "2021".to_string(),
                dependencies: vec![RustAnalyzerDependency {
                    crate_id: "crate-b".to_string(),
                    name: "dep".to_string(),
                }],
            },
            RustAnalyzerCrate {
                crate_id: "crate-b".to_string(),
                root_module: "vendor/dep.rs".to_string(),
                edition: "2021".to_string(),
                dependencies: Vec::new(),
            },
        ],
        cfg: vec!["feature=\"api\"".to_string(), "unix".to_string()],
        env: BTreeMap::from([
            ("CRATE_NAME".to_string(), "app".to_string()),
            ("RUST_BACKTRACE".to_string(), "0".to_string()),
        ]),
        limitations: vec!["build-scripts-disabled".to_string()],
    };
    model.digest = model.canonical_sha256();
    model
}

fn valid_request() -> RepositoryContextProviderRequest {
    let profile = valid_profile();
    RepositoryContextProviderRequest {
        schema_version: 1,
        kind: "repository_context_provider_request".to_string(),
        candidate: CandidateBinding {
            source: ReviewSource::Staged,
            scope_fingerprint: digest('1'),
            candidate_digest: digest('2'),
            snapshot_root: trusted_path("candidate-snapshot"),
            snapshot_sha256: digest('3'),
            snapshot_files: 2,
            snapshot_bytes: 128,
            project_model_digest: valid_project_model().digest,
        },
        provider: ProviderBinding {
            kind: profile.provider_kind.clone(),
            version: profile.provider_version.clone(),
            profile_path: trusted_path("provider-profile.json"),
            profile_sha256: profile.sha256(),
            executable_path: trusted_path("bin/rust-analyzer"),
            executable_sha256: profile.executable_sha256.clone(),
            configuration_sha256: profile.configuration_sha256.clone(),
            target_triple: profile.target_triple.clone(),
            toolchain_mode: profile.toolchain_mode.clone(),
        },
        seeds: vec![SeedSymbol {
            changed_symbol_id: digest('8'),
            path: "src/lib.rs".to_string(),
            kind: SeedKind::Function,
            name: "seed".to_string(),
            symbol_range: provider_range(0, 10),
            selection_range: provider_range(3, 7),
            query_byte: 3,
        }],
        directions: vec![CallDirection::Incoming, CallDirection::Outgoing],
        limits: ProviderLimits::maximum(),
    }
}

fn context_symbol(id: char, path: &str, name: &str, start: usize) -> ContextSymbol {
    ContextSymbol {
        symbol_id: digest(id),
        path: path.to_string(),
        kind: SeedKind::Function,
        name: name.to_string(),
        symbol_range: provider_range(start, start + 8),
        selection_range: provider_range(start + 1, start + 5),
    }
}

fn valid_report() -> RepositoryContextProviderReport {
    let request = valid_request();
    let seed = context_symbol('d', "src/lib.rs", "seed", 0);
    let related = context_symbol('e', "src/caller.rs", "caller", 10);
    RepositoryContextProviderReport {
        schema_version: 1,
        kind: "repository_context_provider_report".to_string(),
        candidate: ReportedCandidateBinding::from(&request.candidate),
        provider: ProviderExecutionRecord {
            kind: request.provider.kind,
            version: request.provider.version,
            profile_sha256: request.provider.profile_sha256,
            executable_sha256: request.provider.executable_sha256,
            configuration_sha256: request.provider.configuration_sha256,
            target_triple: request.provider.target_triple,
            toolchain_mode: request.provider.toolchain_mode,
            project_model_algorithm: valid_project_model().algorithm,
            negotiated_encoding: Some(PositionEncoding::Utf8),
        },
        status: RepositoryContextProviderStatus::Completed,
        index_completeness: ProviderCompleteness::Unknown,
        query_completeness: ProviderCompleteness::Complete,
        seed_symbols: vec![SeedContextSymbol {
            changed_symbol_id: digest('8'),
            symbol: seed.clone(),
        }],
        related_symbols: vec![related.clone()],
        edges: vec![SemanticCallEdge {
            edge_id: digest('f'),
            from_symbol: related.symbol_id,
            to_symbol: seed.symbol_id,
            call_site_path: "src/caller.rs".to_string(),
            call_site_range: provider_range(20, 24),
            kind: "calls".to_string(),
            resolution: "semantic".to_string(),
            confidence: "high".to_string(),
            provider_id: "rust-analyzer".to_string(),
            provider_version: "2026-07-27".to_string(),
        }],
        limitations: Vec::new(),
        isolation: ProviderIsolation {
            network: ProviderNetworkIsolation::BestEffortOffline,
            shell_enabled: false,
            original_repository_access: false,
        },
        metrics: ProviderMetrics {
            requests: 3,
            messages: 6,
            notifications: 1,
            server_requests: 0,
            invalid_messages: 0,
            call_ranges: 1,
            protocol_bytes: 1024,
            stderr_bytes: 0,
            source_bytes: 128,
            nodes: 2,
            edges: 1,
            report_bytes: 2048,
            elapsed_ms: 10,
            process_tree_peak_rss_bytes: 16 * 1024 * 1024,
            process_tree_sample_interval_ms: 100,
            process_tree_accounting:
                collect_diff_context_cli::provider_resources::ResourceAccountingStatus::Available,
        },
    }
}

#[test]
fn valid_request_profile_model_and_report_round_trip() -> Result<(), Box<dyn Error>> {
    let request = valid_request();
    request.validate()?;
    let profile = valid_profile();
    profile.validate()?;
    profile.validate_request(&request)?;
    let model = valid_project_model();
    model.validate()?;
    let report = valid_report();
    report.validate()?;

    assert_eq!(
        serde_json::from_slice::<RepositoryContextProviderRequest>(&serde_json::to_vec(&request)?)?,
        request
    );
    assert_eq!(
        serde_json::from_slice::<AuthorizedProviderProfile>(&serde_json::to_vec(&profile)?)?,
        profile
    );
    assert_eq!(
        serde_json::from_slice::<RustAnalyzerProjectModel>(&serde_json::to_vec(&model)?)?,
        model
    );
    assert_eq!(
        serde_json::from_slice::<RepositoryContextProviderReport>(&serde_json::to_vec(&report)?)?,
        report
    );
    assert_eq!(profile.sha256(), profile.sha256());
    Ok(())
}

#[test]
fn request_and_report_accept_current_scope_fingerprint_grammar() -> Result<(), Box<dyn Error>> {
    let mut request = valid_request();
    request.candidate.scope_fingerprint = "a".repeat(40);
    request.validate()?;

    let mut report = valid_report();
    report.candidate.scope_fingerprint = "b".repeat(40);
    report.validate()?;

    request.candidate.scope_fingerprint = "c".repeat(39);
    assert!(request.validate().is_err());
    report.candidate.scope_fingerprint = "D".repeat(40);
    assert!(report.validate().is_err());
    Ok(())
}

#[test]
fn request_rejects_empty_seeds_duplicate_directions_and_raised_or_zero_limits() {
    let mut request = valid_request();
    request.seeds.clear();
    assert!(request.validate().is_err());

    let mut request = valid_request();
    request.directions = vec![CallDirection::Incoming, CallDirection::Incoming];
    assert!(request.validate().is_err());

    let mut request = valid_request();
    request.limits.max_depth = 3;
    assert!(request.validate().is_err());

    let mut request = valid_request();
    request.limits.max_edges = 0;
    assert!(request.validate().is_err());
}

#[test]
fn request_rejects_wrong_identity_digests_and_unsafe_paths() {
    let mut request = valid_request();
    request.schema_version = 2;
    assert!(request.validate().is_err());
    let mut request = valid_request();
    request.kind = "wrong".to_string();
    assert!(request.validate().is_err());
    let mut request = valid_request();
    request.provider.kind = "other-provider".to_string();
    assert!(request.validate().is_err());
    let mut request = valid_request();
    request.candidate.candidate_digest = digest('A');
    assert!(request.validate().is_err());
    let mut request = valid_request();
    request.candidate.snapshot_sha256 = "abc".to_string();
    assert!(request.validate().is_err());
    let mut request = valid_request();
    request.candidate.snapshot_root = PathBuf::from("relative");
    assert!(request.validate().is_err());
    let mut request = valid_request();
    request.provider.profile_path = request.candidate.snapshot_root.join("profile.json");
    assert!(request.validate().is_err());
    let mut request = valid_request();
    request.provider.executable_path = request.candidate.snapshot_root.join("rust-analyzer");
    assert!(request.validate().is_err());
    let mut request = valid_request();
    request.seeds[0].path = "/absolute.rs".to_string();
    assert!(request.validate().is_err());
    let mut request = valid_request();
    request.seeds[0].path = "src/../escape.rs".to_string();
    assert!(request.validate().is_err());
}

#[test]
fn request_requires_sorted_unique_seeds_and_valid_selection_query_ranges() {
    let mut request = valid_request();
    let mut second = request.seeds[0].clone();
    second.changed_symbol_id = digest('7');
    request.seeds.push(second);
    assert!(request.validate().is_err());

    let mut request = valid_request();
    request.seeds.push(request.seeds[0].clone());
    assert!(request.validate().is_err());

    let mut request = valid_request();
    request.seeds[0].selection_range = provider_range(9, 12);
    assert!(request.validate().is_err());

    let mut request = valid_request();
    request.seeds[0].query_byte = request.seeds[0].selection_range.end_byte;
    assert!(request.validate().is_err());

    let mut request = valid_request();
    request.seeds[0].symbol_range.end_byte = request.seeds[0].symbol_range.start_byte;
    assert!(request.validate().is_err());
}

#[test]
fn profile_requires_exact_hardening_maxima_and_request_bindings() {
    let mut profile = valid_profile();
    profile.hardening.cargo_build_scripts = true;
    assert!(profile.validate().is_err());
    let mut profile = valid_profile();
    profile.hardening.cargo_no_deps = false;
    assert!(profile.validate().is_err());
    let mut profile = valid_profile();
    profile.hardening.cargo_sysroot = Some("/toolchain".to_string());
    assert!(profile.validate().is_err());
    let mut profile = valid_profile();
    profile.hardening.proc_macro = true;
    assert!(profile.validate().is_err());
    let mut profile = valid_profile();
    profile.hardening.check_on_save = true;
    assert!(profile.validate().is_err());
    let mut profile = valid_profile();
    profile.hardening.empty_path = false;
    assert!(profile.validate().is_err());
    let mut profile = valid_profile();
    profile.toolchain_mode = "cargo".to_string();
    assert!(profile.validate().is_err());
    let mut profile = valid_profile();
    profile.arguments.push("Cargo.toml".to_string());
    assert!(profile.validate().is_err());
    let mut profile = valid_profile();
    profile.maximum_limits.max_depth = 1;
    assert!(profile.validate().is_err());

    for mutate in [
        "profile",
        "executable",
        "configuration",
        "target",
        "toolchain",
    ] {
        let profile = valid_profile();
        let mut request = valid_request();
        match mutate {
            "profile" => request.provider.profile_sha256 = digest('9'),
            "executable" => request.provider.executable_sha256 = digest('9'),
            "configuration" => request.provider.configuration_sha256 = digest('9'),
            "target" => request.provider.target_triple.push_str("-changed"),
            "toolchain" => request.provider.toolchain_mode = "cargo".to_string(),
            _ => unreachable!(),
        }
        assert!(profile.validate_request(&request).is_err(), "{mutate}");
    }
}

#[test]
fn project_model_rejects_digest_order_dependency_and_identity_errors() {
    let mut model = valid_project_model();
    model.schema_version = 2;
    model.digest = model.canonical_sha256();
    assert!(model.validate().is_err());
    let mut model = valid_project_model();
    model.digest = digest('0');
    assert!(model.validate().is_err());
    let mut model = valid_project_model();
    model.target_triple.push_str("-changed");
    assert!(model.validate().is_err());
    let mut model = valid_project_model();
    model.crates.reverse();
    model.digest = model.canonical_sha256();
    assert!(model.validate().is_err());
    let mut model = valid_project_model();
    model.crates[0].dependencies[0].crate_id = "missing".to_string();
    model.digest = model.canonical_sha256();
    assert!(model.validate().is_err());
    let mut model = valid_project_model();
    model.crates[1].crate_id = model.crates[0].crate_id.clone();
    model.digest = model.canonical_sha256();
    assert!(model.validate().is_err());
    let mut model = valid_project_model();
    model.crates[0].root_module = "../lib.rs".to_string();
    model.digest = model.canonical_sha256();
    assert!(model.validate().is_err());
    let mut model = valid_project_model();
    model.crates[0].edition = "future".to_string();
    model.digest = model.canonical_sha256();
    assert!(model.validate().is_err());
    let mut model = valid_project_model();
    let dependency = model.crates[0].dependencies[0].clone();
    model.crates[0].dependencies.push(dependency);
    model.digest = model.canonical_sha256();
    assert!(model.validate().is_err());
    let mut model = valid_project_model();
    model.cfg = vec!["z".to_string(), "a".to_string()];
    model.digest = model.canonical_sha256();
    assert!(model.validate().is_err());
}

#[test]
fn report_keeps_seed_mapping_and_related_symbols_separate() {
    let mut report = valid_report();
    report
        .related_symbols
        .push(report.seed_symbols[0].symbol.clone());
    assert!(report.validate().is_err());

    let mut report = valid_report();
    report.edges[0].from_symbol = "missing".to_string();
    assert!(report.validate().is_err());
}

#[test]
fn report_rejects_invalid_status_completeness_facts_and_semantics() {
    let mut report = valid_report();
    report.index_completeness = ProviderCompleteness::Complete;
    assert!(report.validate().is_err());
    let mut report = valid_report();
    report.provider.kind = "other-provider".to_string();
    assert!(report.validate().is_err());
    let mut report = valid_report();
    report.status = RepositoryContextProviderStatus::Partial;
    assert!(report.validate().is_err());
    let mut report = valid_report();
    report.status = RepositoryContextProviderStatus::Unavailable;
    report.query_completeness = ProviderCompleteness::Unavailable;
    assert!(report.validate().is_err());

    for field in ["kind", "resolution", "confidence"] {
        let mut report = valid_report();
        match field {
            "kind" => report.edges[0].kind = "references".to_string(),
            "resolution" => report.edges[0].resolution = "syntactic".to_string(),
            "confidence" => report.edges[0].confidence = "low".to_string(),
            _ => unreachable!(),
        }
        assert!(report.validate().is_err(), "{field}");
    }
}

#[test]
fn report_rejects_unavailable_or_unbounded_resource_metrics() {
    let mut report = valid_report();
    report.metrics.process_tree_accounting =
        collect_diff_context_cli::provider_resources::ResourceAccountingStatus::Unavailable;
    assert!(report.validate().is_err());

    let mut report = valid_report();
    report.metrics.process_tree_sample_interval_ms = 101;
    assert!(report.validate().is_err());

    let mut report = valid_report();
    report.metrics.process_tree_peak_rss_bytes = 2 * 1024 * 1024 * 1024 + 2;
    assert!(report.validate().is_err());

    let mut report = valid_report();
    report.metrics.process_tree_peak_rss_bytes = 2 * 1024 * 1024 * 1024 + 1;
    assert!(report.validate().is_err());
}

#[test]
fn report_rejects_unsorted_duplicate_unbounded_and_oversized_data() {
    let mut report = valid_report();
    report
        .related_symbols
        .push(context_symbol('c', "src/a.rs", "a", 30));
    assert!(report.validate().is_err());
    let mut report = valid_report();
    report.seed_symbols.push(report.seed_symbols[0].clone());
    assert!(report.validate().is_err());
    let mut report = valid_report();
    report.limitations.push(ProviderLimitation {
        code: "bounded".to_string(),
        message: "x".repeat(4_097),
        changed_symbol_id: None,
        path: None,
    });
    assert!(report.validate().is_err());
    let mut report = valid_report();
    report.metrics.report_bytes = ProviderLimits::maximum().max_report_bytes + 1;
    assert!(report.validate().is_err());
}

#[test]
fn unknown_json_fields_are_rejected_for_top_level_and_nested_contracts() {
    fn add_unknown<T: serde::Serialize>(value: &T) -> Vec<u8> {
        let mut value = serde_json::to_value(value).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unknown".to_string(), serde_json::Value::Bool(true));
        serde_json::to_vec(&value).unwrap()
    }

    assert!(
        serde_json::from_slice::<RepositoryContextProviderRequest>(&add_unknown(&valid_request()))
            .is_err()
    );
    assert!(
        serde_json::from_slice::<AuthorizedProviderProfile>(&add_unknown(&valid_profile()))
            .is_err()
    );
    assert!(
        serde_json::from_slice::<RustAnalyzerProjectModel>(&add_unknown(&valid_project_model()))
            .is_err()
    );
    assert!(
        serde_json::from_slice::<RepositoryContextProviderReport>(&add_unknown(&valid_report()))
            .is_err()
    );

    let mut request = serde_json::to_value(valid_request()).unwrap();
    request["candidate"]["unknown"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<RepositoryContextProviderRequest>(request).is_err());
}

#[test]
fn deterministic_ids_bind_every_report_local_component() {
    let request = valid_request();
    let binding = request
        .binding_digest(&valid_project_model().algorithm)
        .unwrap();
    let report = valid_report();
    let symbol = &report.seed_symbols[0].symbol;
    let first = report_symbol_id(
        &binding,
        &symbol.path,
        symbol.kind,
        &symbol.name,
        &symbol.symbol_range,
        &symbol.selection_range,
    )
    .unwrap();
    let changed = report_symbol_id(
        &binding,
        &symbol.path,
        symbol.kind,
        "changed",
        &symbol.symbol_range,
        &symbol.selection_range,
    )
    .unwrap();
    assert_ne!(first, changed);
    let edge = &report.edges[0];
    assert_ne!(
        report_edge_id(
            &binding,
            &edge.from_symbol,
            &edge.to_symbol,
            &edge.call_site_path,
            &edge.call_site_range,
        )
        .unwrap(),
        report_edge_id(
            &binding,
            &edge.to_symbol,
            &edge.from_symbol,
            &edge.call_site_path,
            &edge.call_site_range,
        )
        .unwrap()
    );
}

#[test]
fn errors_are_standard_bounded_errors() {
    fn assert_error<T: Error>() {}
    assert_error::<ContractError>();
    assert_error::<ProfileError>();
    assert_error::<ProjectModelError>();

    let mut request = valid_request();
    request.seeds[0].name = "x".repeat(10_000);
    let error = request.validate().unwrap_err();
    assert!(error.to_string().len() <= 512);
    assert!(!error.code.is_empty());
}

#[test]
fn provider_schemas_are_draft_2020_12_and_strict_at_every_object() {
    let schemas = [
        include_str!("../schemas/repository-context-provider-request.schema.json"),
        include_str!("../schemas/repository-context-provider-profile.schema.json"),
        include_str!("../schemas/repository-context-project-model.schema.json"),
        include_str!("../schemas/repository-context-provider-report.schema.json"),
    ];

    fn assert_strict_objects(value: &serde_json::Value) {
        match value {
            serde_json::Value::Object(object) => {
                if object.get("type").and_then(serde_json::Value::as_str) == Some("object") {
                    assert_eq!(
                        object.get("additionalProperties"),
                        Some(&serde_json::Value::Bool(false))
                    );
                }
                for child in object.values() {
                    assert_strict_objects(child);
                }
            }
            serde_json::Value::Array(array) => {
                for child in array {
                    assert_strict_objects(child);
                }
            }
            _ => {}
        }
    }

    for schema in schemas {
        let value: serde_json::Value = serde_json::from_str(schema).unwrap();
        assert_eq!(
            value["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_strict_objects(&value);
    }
    let report = schemas[3];
    for forbidden in [
        "\"snapshot_root\"",
        "\"raw_stderr\"",
        "\"raw_json_rpc\"",
        "\"raw_uri\"",
        "\"opaque_data\"",
    ] {
        assert!(!report.contains(forbidden), "{forbidden}");
    }
}
