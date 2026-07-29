use super::contract::{
    sha256_json, validate_absolute_path, validate_sha256, validate_target, validate_text,
    CallDirection, ContractError, ProviderLimits, SeedSymbol,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

const MAX_REGISTRY_ENTRIES: usize = 16;
const MAX_PROVIDER_ID_BYTES: usize = 256;
const MAX_VERSION_BYTES: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliContractError {
    pub code: &'static str,
    message: String,
}

impl CliContractError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into().chars().take(512).collect(),
        }
    }
}

impl std::fmt::Display for CliContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliContractError {}

impl From<ContractError> for CliContractError {
    fn from(error: ContractError) -> Self {
        Self::new(error.code, error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRegistry {
    pub schema_version: u8,
    pub kind: String,
    pub entries: Vec<ProviderRegistryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRegistryEntry {
    pub provider_id: String,
    pub provider_kind: String,
    pub provider_version: String,
    pub target_triple: String,
    pub profile_path: PathBuf,
    pub profile_sha256: String,
    pub executable_path: PathBuf,
    pub executable_sha256: String,
    pub configuration_sha256: String,
    pub toolchain_mode: String,
}

impl ProviderRegistry {
    pub fn validate(&self) -> Result<(), CliContractError> {
        if self.schema_version != 1 {
            return cli_error(
                "provider-registry-schema-invalid",
                "registry schema_version must equal 1",
            );
        }
        if self.kind != "repository_context_provider_registry" {
            return cli_error(
                "provider-registry-kind-invalid",
                "registry kind is not recognized",
            );
        }
        if self.entries.is_empty() || self.entries.len() > MAX_REGISTRY_ENTRIES {
            return cli_error(
                "provider-registry-entries-invalid",
                "registry must contain between one and sixteen entries",
            );
        }
        let mut provider_ids = BTreeSet::new();
        for entry in &self.entries {
            entry.validate()?;
            if !provider_ids.insert(entry.provider_id.as_str()) {
                return cli_error(
                    "provider-registry-id-duplicate",
                    "registry provider ids must be unique",
                );
            }
        }
        Ok(())
    }

    pub fn sha256(&self) -> String {
        sha256_json(self)
    }

    pub fn select(&self, provider_id: &str) -> Result<&ProviderRegistryEntry, CliContractError> {
        self.validate()?;
        self.entries
            .iter()
            .find(|entry| entry.provider_id == provider_id)
            .ok_or_else(|| {
                CliContractError::new(
                    "provider-registry-entry-missing",
                    "requested provider id is not present in the registry",
                )
            })
    }
}

impl ProviderRegistryEntry {
    fn validate(&self) -> Result<(), CliContractError> {
        validate_provider_id(&self.provider_id)?;
        if self.provider_kind != "rust-analyzer" {
            return cli_error(
                "provider-registry-provider-invalid",
                "registry provider kind must equal rust-analyzer",
            );
        }
        validate_text(
            &self.provider_version,
            MAX_VERSION_BYTES,
            "provider version",
        )?;
        validate_target(&self.target_triple)?;
        validate_absolute_path(&self.profile_path, "profile path")?;
        validate_absolute_path(&self.executable_path, "executable path")?;
        validate_sha256(&self.profile_sha256, "profile digest")?;
        validate_sha256(&self.executable_sha256, "executable digest")?;
        validate_sha256(&self.configuration_sha256, "configuration digest")?;
        if self.toolchain_mode != "none" {
            return cli_error(
                "provider-registry-toolchain-forbidden",
                "registry toolchain mode must equal none",
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRunRequest {
    pub schema_version: u8,
    pub kind: String,
    pub seeds: Vec<SeedSymbol>,
    pub directions: Vec<CallDirection>,
    pub limits: ProviderLimits,
}

impl ProviderRunRequest {
    pub fn validate(&self) -> Result<(), CliContractError> {
        if self.schema_version != 1 {
            return cli_error(
                "provider-run-request-schema-invalid",
                "run request schema_version must equal 1",
            );
        }
        if self.kind != "repository_context_provider_run_request" {
            return cli_error(
                "provider-run-request-kind-invalid",
                "run request kind is not recognized",
            );
        }
        self.limits.validate()?;
        if self.seeds.is_empty() || self.seeds.len() > self.limits.max_seeds {
            return cli_error(
                "provider-run-request-seeds-invalid",
                "run request seeds must be non-empty and within max_seeds",
            );
        }
        for pair in self.seeds.windows(2) {
            if pair[0].changed_symbol_id >= pair[1].changed_symbol_id {
                return cli_error(
                    "provider-run-request-seeds-order-invalid",
                    "run request seeds must be sorted with unique ids",
                );
            }
        }
        for seed in &self.seeds {
            seed.validate()?;
        }
        if self.directions.is_empty() || self.directions.len() > 2 {
            return cli_error(
                "provider-run-request-directions-invalid",
                "run request directions must be non-empty",
            );
        }
        for pair in self.directions.windows(2) {
            if pair[0] >= pair[1] {
                return cli_error(
                    "provider-run-request-directions-order-invalid",
                    "run request directions must be sorted and unique",
                );
            }
        }
        Ok(())
    }

    pub fn validate_against(&self, maxima: &ProviderLimits) -> Result<(), CliContractError> {
        self.validate()?;
        maxima.validate()?;
        macro_rules! within {
            ($field:ident) => {
                if self.limits.$field > maxima.$field {
                    return cli_error(
                        "provider-run-request-limit-raised",
                        concat!("run request exceeds authorized ", stringify!($field)),
                    );
                }
            };
        }
        within!(deadline_ms);
        within!(max_depth);
        within!(max_seeds);
        within!(max_requests);
        within!(max_pending_requests);
        within!(max_messages);
        within!(max_notifications);
        within!(max_server_requests);
        within!(max_invalid_messages);
        within!(max_call_ranges);
        within!(max_header_bytes);
        within!(max_frame_bytes);
        within!(max_protocol_bytes);
        within!(max_stderr_bytes);
        within!(max_total_output_bytes);
        within!(max_source_file_bytes);
        within!(max_source_bytes);
        within!(max_nodes);
        within!(max_edges);
        within!(max_report_bytes);
        Ok(())
    }
}

fn validate_provider_id(value: &str) -> Result<(), CliContractError> {
    validate_text(value, MAX_PROVIDER_ID_BYTES, "provider id")?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return cli_error(
            "provider-registry-id-invalid",
            "provider id must use ASCII letters, digits, dot, underscore, or hyphen",
        );
    }
    Ok(())
}

fn cli_error<T>(code: &'static str, message: impl Into<String>) -> Result<T, CliContractError> {
    Err(CliContractError::new(code, message))
}
