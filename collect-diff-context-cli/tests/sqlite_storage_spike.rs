use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

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
    spike_command()
        .args(arguments)
        .output()
        .expect("run sqlite storage spike")
}

fn spike_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sqlite-storage-spike"))
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

fn build_fixture(cache: &Path, symbols: usize, edges: usize) -> (SpikeReport, PathBuf) {
    let output = spike(&[
        "build",
        "--cache-dir",
        cache.to_str().unwrap(),
        "--symbols",
        &symbols.to_string(),
        "--edges",
        &edges.to_string(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = serde_json::from_slice(&output.stdout).unwrap();
    let generations = generation_files(cache);
    assert_eq!(generations.len(), 1);
    (report, generations[0].clone())
}

fn doctor(generation: &Path) -> (Output, SpikeReport) {
    let output = spike(&["doctor", "--generation", generation.to_str().unwrap()]);
    let report = serde_json::from_slice(&output.stdout).unwrap();
    (output, report)
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

#[test]
fn build_reuses_an_existing_valid_generation() {
    let cache = tempfile::tempdir().unwrap();
    let (_, generation) = build_fixture(cache.path(), 4, 6);
    let before = std::fs::metadata(&generation).unwrap().modified().unwrap();
    std::thread::sleep(Duration::from_millis(100));

    let output = spike(&[
        "build",
        "--cache-dir",
        cache.path().to_str().unwrap(),
        "--symbols",
        "4",
        "--edges",
        "6",
    ]);

    assert!(output.status.success());
    assert_eq!(generation_files(cache.path()), vec![generation.clone()]);
    assert_eq!(
        std::fs::metadata(generation).unwrap().modified().unwrap(),
        before
    );
}

#[test]
fn build_never_replaces_an_existing_invalid_generation() {
    let cache = tempfile::tempdir().unwrap();
    let (_, generation) = build_fixture(cache.path(), 4, 6);
    std::fs::remove_file(&generation).unwrap();
    std::fs::write(&generation, b"not sqlite").unwrap();

    let output = spike(&[
        "build",
        "--cache-dir",
        cache.path().to_str().unwrap(),
        "--symbols",
        "4",
        "--edges",
        "6",
    ]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid-existing-generation"));
    assert_eq!(std::fs::read(generation).unwrap(), b"not sqlite");
}

#[test]
fn doctor_accepts_a_complete_generation() {
    let cache = tempfile::tempdir().unwrap();
    let (build, generation) = build_fixture(cache.path(), 4, 6);
    let (output, report) = doctor(&generation);
    assert!(output.status.success());
    assert_eq!(report.action, "doctor");
    assert_eq!(report.status, "completed");
    assert_eq!(report.generation_key, build.generation_key);
    assert_eq!(report.symbols, 4);
    assert_eq!(report.edges, 6);
}

#[test]
fn doctor_rejects_truncated_database() {
    let cache = tempfile::tempdir().unwrap();
    let (_, generation) = build_fixture(cache.path(), 4, 6);
    let length = std::fs::metadata(&generation).unwrap().len();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&generation)
        .unwrap()
        .set_len(length / 2)
        .unwrap();

    let (output, report) = doctor(&generation);
    assert!(!output.status.success());
    assert_eq!(report.action, "doctor");
    assert_eq!(report.status, "corrupt");
}

#[test]
fn doctor_rejects_generation_metadata_mismatch() {
    let cache = tempfile::tempdir().unwrap();
    let (_, generation) = build_fixture(cache.path(), 4, 6);
    let connection = rusqlite::Connection::open(&generation).unwrap();
    connection
        .pragma_update(None, "foreign_keys", false)
        .unwrap();
    connection
        .execute(
            "UPDATE generation_meta SET generation_key = 'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'",
            [],
        )
        .unwrap();
    drop(connection);

    let (output, report) = doctor(&generation);
    assert!(!output.status.success());
    assert_eq!(report.status, "corrupt");
    assert!(report
        .limitations
        .iter()
        .any(|code| code == "generation-key-mismatch"));
}

#[test]
fn doctor_rejects_foreign_key_and_root_digest_mismatch() {
    let foreign_key_cache = tempfile::tempdir().unwrap();
    let (_, foreign_key_generation) = build_fixture(foreign_key_cache.path(), 4, 6);
    let connection = rusqlite::Connection::open(&foreign_key_generation).unwrap();
    connection
        .pragma_update(None, "foreign_keys", false)
        .unwrap();
    connection
        .execute(
            "UPDATE edges SET to_symbol = 'missing-symbol' WHERE edge_id = 'edge-00000000'",
            [],
        )
        .unwrap();
    drop(connection);

    let (output, report) = doctor(&foreign_key_generation);
    assert!(!output.status.success());
    assert_eq!(report.status, "corrupt");
    assert!(report
        .limitations
        .iter()
        .any(|code| code == "foreign-key-mismatch"));

    let root_cache = tempfile::tempdir().unwrap();
    let (_, root_generation) = build_fixture(root_cache.path(), 4, 6);
    let connection = rusqlite::Connection::open(&root_generation).unwrap();
    connection
        .execute(
            "UPDATE generation_meta SET application_root = 'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff'",
            [],
        )
        .unwrap();
    drop(connection);

    let (output, report) = doctor(&root_generation);
    assert!(!output.status.success());
    assert_eq!(report.status, "corrupt");
    assert!(report
        .limitations
        .iter()
        .any(|code| code == "application-root-mismatch"));
}

#[test]
fn crash_points_never_publish_partial_generations_or_graph_sidecars() {
    for point in [
        "before-commit",
        "after-commit",
        "after-sync",
        "before-publish",
    ] {
        let cache = tempfile::tempdir().unwrap();
        let output = spike(&[
            "build",
            "--cache-dir",
            cache.path().to_str().unwrap(),
            "--symbols",
            "100",
            "--edges",
            "200",
            "--crash-at",
            point,
        ]);
        assert_eq!(output.status.code(), Some(99), "crash point {point}");
        for generation in generation_files(cache.path()) {
            let (doctor_output, report) = doctor(&generation);
            assert!(doctor_output.status.success(), "{point}: {report:?}");
        }
        let graph_directory = cache.path().join("graphs");
        if let Ok(entries) = std::fs::read_dir(graph_directory) {
            for entry in entries {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                assert!(
                    !name.ends_with("-journal")
                        && !name.ends_with("-wal")
                        && !name.ends_with("-shm"),
                    "{point} left graph sidecar {name}"
                );
            }
        }
    }
}

#[test]
fn query_traversal_is_bounded_and_creates_no_sidecars() {
    let cache = tempfile::tempdir().unwrap();
    let (_, generation) = build_fixture(cache.path(), 100, 200);
    let output = spike(&[
        "query",
        "--generation",
        generation.to_str().unwrap(),
        "--symbol",
        "symbol-00000000",
        "--direction",
        "outgoing",
        "--depth",
        "2",
        "--max-edges",
        "100",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: SpikeReport = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report.action, "query");
    assert_eq!(report.status, "completed");
    assert_eq!(report.edges, 4);

    let truncated = spike(&[
        "query",
        "--generation",
        generation.to_str().unwrap(),
        "--symbol",
        "symbol-00000000",
        "--direction",
        "outgoing",
        "--depth",
        "2",
        "--max-edges",
        "1",
    ]);
    assert!(truncated.status.success());
    let report: SpikeReport = serde_json::from_slice(&truncated.stdout).unwrap();
    assert_eq!(report.status, "partial");
    assert_eq!(report.edges, 1);
    assert_eq!(report.limitations, ["edge-budget-exhausted"]);

    let graph_directory = cache.path().join("graphs");
    assert!(std::fs::read_dir(graph_directory).unwrap().all(|entry| {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        !name.ends_with("-journal") && !name.ends_with("-wal") && !name.ends_with("-shm")
    }));
}

#[test]
fn reader_of_generation_a_does_not_wait_for_writer_of_generation_b() {
    let cache = tempfile::tempdir().unwrap();
    let (_, generation_a) = build_fixture(cache.path(), 100, 200);
    let mut readers = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        let child = spike_command()
            .args([
                "query",
                "--generation",
                generation_a.to_str().unwrap(),
                "--symbol",
                "symbol-00000000",
                "--direction",
                "outgoing",
                "--depth",
                "2",
                "--max-edges",
                "100",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        readers.push((started, child));
    }

    let writer = spike_command()
        .args([
            "build",
            "--cache-dir",
            cache.path().to_str().unwrap(),
            "--symbols",
            "10000",
            "--edges",
            "20000",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    for (started, reader) in readers {
        let output = reader.wait_with_output().unwrap();
        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_millis(750));
        let report: SpikeReport = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report.status, "completed");
    }
    let writer_output = writer.wait_with_output().unwrap();
    assert!(
        writer_output.status.success(),
        "{}",
        String::from_utf8_lossy(&writer_output.stderr)
    );
    assert_eq!(generation_files(cache.path()).len(), 2);
}
