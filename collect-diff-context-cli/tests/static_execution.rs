use collect_diff_context_cli::static_analysis::contracts::StaticAnalysisProfile;
use serde_json::json;

fn valid_profile() -> serde_json::Value {
    json!({
        "schema_version": 1,
        "kind": "static_analysis_profile",
        "name": "fixture profile",
        "tool": {"name": "fixture", "version": "1.0"},
        "executable": {
            "path": "/opt/review/bin/fixture",
            "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        },
        "arguments": ["--format", "json"],
        "output_format": "normalized-json",
        "success_exit_codes": [0],
        "limits": {
            "timeout_seconds": 30,
            "max_output_bytes": 1048576,
            "max_snapshot_bytes": 10485760,
            "max_snapshot_files": 1000
        },
        "repository_configuration": "disabled",
        "network_access": "offline-required"
    })
}

#[test]
fn contracts_accept_valid_profile() {
    let profile: StaticAnalysisProfile = serde_json::from_value(valid_profile()).unwrap();
    profile.validate().unwrap();
}

#[test]
fn contracts_reject_unknown_profile_fields() {
    let mut profile = valid_profile();
    profile["limits"]["unexpected"] = json!(1);
    assert!(serde_json::from_value::<StaticAnalysisProfile>(profile).is_err());
}

#[test]
fn contracts_reject_invalid_profile_hash_and_bounds() {
    let mut profile = valid_profile();
    profile["executable"]["sha256"] = json!("ABCDEF");
    let profile: StaticAnalysisProfile = serde_json::from_value(profile).unwrap();
    assert!(profile.validate().is_err());

    let mut profile = valid_profile();
    profile["limits"]["timeout_seconds"] = json!(0);
    let profile: StaticAnalysisProfile = serde_json::from_value(profile).unwrap();
    assert!(profile.validate().is_err());
}

#[test]
fn contracts_reject_duplicate_exit_codes_and_nul_arguments() {
    let mut profile = valid_profile();
    profile["success_exit_codes"] = json!([0, 0]);
    let profile: StaticAnalysisProfile = serde_json::from_value(profile).unwrap();
    assert!(profile.validate().is_err());

    let mut profile = valid_profile();
    profile["arguments"] = json!(["bad\u{0}argument"]);
    let profile: StaticAnalysisProfile = serde_json::from_value(profile).unwrap();
    assert!(profile.validate().is_err());
}
