use collect_diff_context_cli::artifacts::{
    contract::canonical_json,
    provider::{write_rust_analyzer_pack, RustAnalyzerPackOptions},
    writer::{write_core_pack, write_gitleaks_pack, CorePackOptions, GitleaksPackOptions},
};
use std::{collections::BTreeMap, env, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    match run() {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("artifact pack writer: {error}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<String, String> {
    let mut arguments = env::args().skip(1);
    let kind = arguments
        .next()
        .ok_or_else(|| "missing pack kind (gitleaks, rust-analyzer, or core)".to_string())?;
    if matches!(kind.as_str(), "-h" | "--help") {
        return Ok(usage().to_string());
    }
    let mut options = BTreeMap::new();
    while let Some(name) = arguments.next() {
        if !name.starts_with("--") {
            return Err(format!("unexpected argument: {name}"));
        }
        let value = arguments
            .next()
            .ok_or_else(|| format!("{name} requires a value"))?;
        if options.insert(name.clone(), value).is_some() {
            return Err(format!("duplicate argument: {name}"));
        }
    }
    let allowed: &[&str] = match kind.as_str() {
        "gitleaks" => &[
            "--platform-id",
            "--pack-version",
            "--source-root",
            "--manifest",
            "--source-lock",
            "--binary",
            "--output",
            "--record-output",
            "--manifest-output",
        ],
        "core" => &[
            "--platform-id",
            "--pack-version",
            "--source-root",
            "--manifest",
            "--revocations",
            "--output",
            "--record-output",
        ],
        "rust-analyzer" => &[
            "--platform-id",
            "--pack-version",
            "--source-lock",
            "--generator-config",
            "--output",
            "--manifest-output",
            "--sbom-output",
        ],
        _ => return Err(format!("unsupported pack kind: {kind}")),
    };
    if let Some(name) = options
        .keys()
        .find(|name| !allowed.contains(&name.as_str()))
    {
        return Err(format!("unknown argument: {name}"));
    }
    let required = |name: &str| {
        options
            .get(name)
            .cloned()
            .ok_or_else(|| format!("missing required argument: {name}"))
    };
    let platform_id = required("--platform-id")?;
    let pack_version = required("--pack-version")?;
    let output = absolute_path(&required("--output")?)?;
    let record_output = options
        .get("--record-output")
        .map(|path| absolute_path(path))
        .transpose()?;
    let manifest_output = options
        .get("--manifest-output")
        .map(|path| absolute_path(path))
        .transpose()?;
    let sbom_output = options
        .get("--sbom-output")
        .map(|path| absolute_path(path))
        .transpose()?;

    match kind.as_str() {
        "gitleaks" => {
            let source_root = absolute_path(&required("--source-root")?)?;
            let manifest = absolute_path(&required("--manifest")?)?;
            let source_lock = absolute_path(&required("--source-lock")?)?;
            let binary = absolute_path(&required("--binary")?)?;
            let record = write_gitleaks_pack(&GitleaksPackOptions {
                platform_id: &platform_id,
                pack_version: &pack_version,
                source_root: &source_root,
                manifest_path: &manifest,
                source_lock_path: &source_lock,
                binary_path: &binary,
                output_path: &output,
                record_output: record_output.as_deref(),
                manifest_output: manifest_output.as_deref(),
            })?;
            String::from_utf8(canonical_json(&record).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())
        }
        "core" => {
            let source_root = absolute_path(&required("--source-root")?)?;
            let manifest = absolute_path(&required("--manifest")?)?;
            let revocations = absolute_path(&required("--revocations")?)?;
            let record = write_core_pack(&CorePackOptions {
                platform_id: &platform_id,
                pack_version: &pack_version,
                source_root: &source_root,
                manifest_path: &manifest,
                revocations_path: &revocations,
                output_path: &output,
                record_output: record_output.as_deref(),
            })?;
            String::from_utf8(canonical_json(&record).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())
        }
        "rust-analyzer" => {
            let source_lock = absolute_path(&required("--source-lock")?)?;
            let generator_config = absolute_path(&required("--generator-config")?)?;
            let built = write_rust_analyzer_pack(&RustAnalyzerPackOptions {
                platform_id: &platform_id,
                pack_version: &pack_version,
                source_lock_path: &source_lock,
                generator_config_path: &generator_config,
                output_path: &output,
                manifest_output: manifest_output.as_deref(),
                sbom_output: sbom_output.as_deref(),
            })?;
            String::from_utf8(
                canonical_json(&built.release_metadata()).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())
        }
        _ => unreachable!(),
    }
}

fn absolute_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!("path must be absolute: {value}"));
    }
    Ok(path)
}

fn usage() -> &'static str {
    "Usage: artifact-pack-writer gitleaks|rust-analyzer|core --platform-id ID --pack-version VERSION --output /absolute/pack.tar.gz [kind options]"
}
