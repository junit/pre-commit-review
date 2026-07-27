use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Debug, Deserialize)]
struct SpikeReport {
    schema_version: u8,
    kind: String,
    action: String,
    status: String,
    generation_key: Option<String>,
    symbols: usize,
    edges: usize,
    elapsed_ms: u64,
    output_bytes: usize,
    limitations: Vec<String>,
}

fn spike(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sqlite-storage-spike"))
        .args(arguments)
        .output()
        .expect("run sqlite storage spike")
}

fn generation_files(cache: &Path) -> Vec<PathBuf> {
    let graph_directory = cache.join("graphs");
    let Ok(entries) = std::fs::read_dir(graph_directory) else {
        return Vec::new();
    };
    let mut paths = entries
        .map(|entry| entry.expect("read graph entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "sqlite")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

#[test]
fn help_lists_build_query_doctor_and_benchmark() {
    let output = spike(&["--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    for command in ["build", "query", "doctor", "benchmark"] {
        assert!(stdout.contains(command), "missing {command}");
    }
}

#[test]
fn build_publishes_one_digest_named_generation() {
    let cache = tempfile::tempdir().unwrap();
    let output = spike(&[
        "build",
        "--cache-dir",
        cache.path().to_str().unwrap(),
        "--symbols",
        "4",
        "--edges",
        "6",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: SpikeReport = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report.schema_version, 1);
    assert_eq!(report.kind, "sqlite-storage-spike-report");
    assert_eq!(report.action, "build");
    assert_eq!(report.status, "completed");
    assert!(report.generation_key.is_some());
    assert_eq!(report.symbols, 4);
    assert_eq!(report.edges, 6);
    assert!(report.elapsed_ms < 60_000);
    assert_eq!(report.output_bytes, output.stdout.len());
    assert!(report.limitations.is_empty());
    let generations = generation_files(cache.path());
    assert_eq!(generations.len(), 1);
    let name = generations[0].file_name().unwrap().to_string_lossy();
    assert_eq!(name.len(), 64 + ".sqlite".len());
    assert!(name.ends_with(".sqlite"));
    assert!(name[..64]
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
}

#[test]
fn strict_argument_parser_rejects_duplicate_flags() {
    let cache = tempfile::tempdir().unwrap();
    let output = spike(&[
        "build",
        "--cache-dir",
        cache.path().to_str().unwrap(),
        "--symbols",
        "4",
        "--symbols",
        "5",
        "--edges",
        "6",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("duplicate --symbols"));
    assert!(generation_files(cache.path()).is_empty());
}
