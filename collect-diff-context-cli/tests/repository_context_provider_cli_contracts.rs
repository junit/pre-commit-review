use collect_diff_context_cli::repository_context_provider::cli_contract::{
    ProviderRegistry, ProviderRegistryEntry, ProviderRunRequest,
};
use collect_diff_context_cli::repository_context_provider::contract::{
    CallDirection, ProviderLimits, ProviderRange, ProviderRangeFormat, SeedKind, SeedSymbol,
};
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

fn valid_entry(id: &str) -> ProviderRegistryEntry {
    ProviderRegistryEntry {
        provider_id: id.to_string(),
        provider_kind: "rust-analyzer".to_string(),
        provider_version: "2026-07-29".to_string(),
        target_triple: "x86_64-unknown-linux-gnu".to_string(),
        profile_path: trusted_path("profiles/rust-analyzer.json"),
        profile_sha256: digest('1'),
        executable_path: trusted_path("bin/rust-analyzer"),
        executable_sha256: digest('2'),
        configuration_sha256: digest('3'),
        toolchain_mode: "none".to_string(),
    }
}

fn valid_registry() -> ProviderRegistry {
    ProviderRegistry {
        schema_version: 1,
        kind: "repository_context_provider_registry".to_string(),
        entries: vec![valid_entry("rust-analyzer-local")],
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

fn valid_run_request() -> ProviderRunRequest {
    ProviderRunRequest {
        schema_version: 1,
        kind: "repository_context_provider_run_request".to_string(),
        seeds: vec![SeedSymbol {
            changed_symbol_id: digest('4'),
            path: "src/lib.rs".to_string(),
            kind: SeedKind::Function,
            name: "seed".to_string(),
            symbol_range: provider_range(0, 12),
            selection_range: provider_range(7, 11),
            query_byte: 7,
        }],
        directions: vec![CallDirection::Incoming, CallDirection::Outgoing],
        limits: ProviderLimits::maximum(),
    }
}

#[test]
fn valid_registry_and_run_request_round_trip() -> Result<(), Box<dyn Error>> {
    let registry = valid_registry();
    registry.validate()?;
    assert_eq!(
        serde_json::from_slice::<ProviderRegistry>(&serde_json::to_vec(&registry)?)?,
        registry
    );
    assert_eq!(
        registry.select("rust-analyzer-local")?,
        &registry.entries[0]
    );
    assert_eq!(registry.sha256(), registry.sha256());

    let request = valid_run_request();
    request.validate()?;
    request.validate_against(&ProviderLimits::maximum())?;
    assert_eq!(
        serde_json::from_slice::<ProviderRunRequest>(&serde_json::to_vec(&request)?)?,
        request
    );
    Ok(())
}

#[test]
fn registry_rejects_duplicate_ids_relative_paths_and_bad_digests() {
    let mut registry = valid_registry();
    registry.entries.push(valid_entry("rust-analyzer-local"));
    assert!(registry.validate().is_err());

    let mut registry = valid_registry();
    registry.entries[0].profile_path = PathBuf::from("relative/profile.json");
    assert!(registry.validate().is_err());

    let mut registry = valid_registry();
    registry.entries[0].executable_path = PathBuf::from("relative/rust-analyzer");
    assert!(registry.validate().is_err());

    let mut registry = valid_registry();
    registry.entries[0].profile_sha256 = "A".repeat(64);
    assert!(registry.validate().is_err());

    let mut registry = valid_registry();
    registry.entries[0].executable_sha256 = "a".repeat(63);
    assert!(registry.validate().is_err());
}

#[test]
fn registry_rejects_unknown_provider_and_unbounded_entry_count() {
    let mut registry = valid_registry();
    registry.entries[0].provider_kind = "clangd".to_string();
    assert!(registry.validate().is_err());

    let mut registry = valid_registry();
    registry.entries[0].toolchain_mode = "rustup".to_string();
    assert!(registry.validate().is_err());

    let registry = ProviderRegistry {
        entries: (0..17)
            .map(|index| valid_entry(&format!("provider-{index:02}")))
            .collect(),
        ..valid_registry()
    };
    assert!(registry.validate().is_err());
}

#[test]
fn run_request_rejects_empty_duplicate_and_raised_limits() {
    let mut request = valid_run_request();
    request.seeds.clear();
    assert!(request.validate().is_err());

    let mut request = valid_run_request();
    request.directions = vec![CallDirection::Incoming, CallDirection::Incoming];
    assert!(request.validate().is_err());

    let mut request = valid_run_request();
    request.limits.max_edges = 0;
    assert!(request.validate().is_err());

    let mut maxima = ProviderLimits::maximum();
    maxima.max_edges = 2;
    let request = valid_run_request();
    assert!(request.validate_against(&maxima).is_err());
}

#[test]
fn unknown_json_fields_are_rejected() {
    let mut registry = serde_json::to_value(valid_registry()).unwrap();
    registry
        .as_object_mut()
        .unwrap()
        .insert("unexpected".to_string(), serde_json::json!(true));
    assert!(serde_json::from_value::<ProviderRegistry>(registry).is_err());

    let mut request = serde_json::to_value(valid_run_request()).unwrap();
    request
        .as_object_mut()
        .unwrap()
        .insert("unexpected".to_string(), serde_json::json!(true));
    assert!(serde_json::from_value::<ProviderRunRequest>(request).is_err());
}
