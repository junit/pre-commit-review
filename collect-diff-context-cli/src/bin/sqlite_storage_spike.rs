use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::env;
use std::fmt::{Display, Formatter};
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;
use tempfile::NamedTempFile;

const APPLICATION_ID: i32 = 0x5043_5247;
const SCHEMA_VERSION: i32 = 1;
const MAX_SYMBOLS: usize = 2_000_000;
const MAX_EDGES: usize = 5_000_000;
const MAX_QUERY_EDGES: usize = 10_000;

#[derive(Serialize)]
struct SpikeReport {
    schema_version: u8,
    kind: &'static str,
    action: &'static str,
    status: &'static str,
    generation_key: Option<String>,
    symbols: usize,
    edges: usize,
    elapsed_ms: u64,
    output_bytes: usize,
    limitations: Vec<String>,
}

#[derive(Debug, Clone)]
struct BuildArgs {
    cache_dir: PathBuf,
    symbols: usize,
    edges: usize,
    crash_at: Option<CrashPoint>,
}

#[derive(Debug, Clone)]
struct QueryArgs {
    generation: PathBuf,
    symbol: String,
    direction: Direction,
    depth: usize,
    max_edges: usize,
}

#[derive(Debug, Clone)]
struct DoctorArgs {
    generation: PathBuf,
}

#[derive(Debug, Clone)]
struct BenchmarkArgs {
    cache_dir: PathBuf,
    symbols: usize,
    edges: usize,
    queries: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrashPoint {
    BeforeCommit,
    AfterCommit,
    AfterSync,
    BeforePublish,
}

#[derive(Debug, Clone, Copy)]
enum Direction {
    Incoming,
    Outgoing,
}

#[derive(Debug)]
struct GenerationStats {
    generation_key: String,
    symbols: usize,
    edges: usize,
    application_root: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum PublishOutcome {
    Published,
    Reused,
}

#[derive(Debug)]
#[allow(dead_code)]
enum SpikeError {
    InvalidInput(String),
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    InvalidGeneration(String),
    InvalidExistingGeneration(String),
}

enum Command {
    Help,
    Build(BuildArgs),
    Query(QueryArgs),
    Doctor(DoctorArgs),
    Benchmark(BenchmarkArgs),
}

fn main() {
    if let Err(error) = run() {
        eprintln!("sqlite-storage-spike: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), SpikeError> {
    match parse_command(env::args().skip(1).collect())? {
        Command::Help => {
            print_help()?;
            Ok(())
        }
        Command::Build(arguments) => run_build(arguments),
        Command::Query(arguments) => {
            let _ = (
                arguments.generation,
                arguments.symbol,
                arguments.direction,
                arguments.depth,
                arguments.max_edges,
            );
            Err(SpikeError::InvalidInput("query is not implemented".into()))
        }
        Command::Doctor(arguments) => {
            let _ = arguments.generation;
            Err(SpikeError::InvalidInput("doctor is not implemented".into()))
        }
        Command::Benchmark(arguments) => {
            let _ = (
                arguments.cache_dir,
                arguments.symbols,
                arguments.edges,
                arguments.queries,
            );
            Err(SpikeError::InvalidInput(
                "benchmark is not implemented".into(),
            ))
        }
    }
}

fn parse_command(arguments: Vec<String>) -> Result<Command, SpikeError> {
    let Some(command) = arguments.first().map(String::as_str) else {
        return Ok(Command::Help);
    };
    if command == "--help" || command == "-h" {
        if arguments.len() == 1 {
            return Ok(Command::Help);
        }
        return Err(invalid("--help does not accept arguments"));
    }

    match command {
        "build" => parse_build(&arguments[1..]).map(Command::Build),
        "query" => parse_query(&arguments[1..]).map(Command::Query),
        "doctor" => parse_doctor(&arguments[1..]).map(Command::Doctor),
        "benchmark" => parse_benchmark(&arguments[1..]).map(Command::Benchmark),
        _ => Err(invalid(format!("unknown command: {command}"))),
    }
}

fn parse_build(arguments: &[String]) -> Result<BuildArgs, SpikeError> {
    let mut cache_dir = None;
    let mut symbols = None;
    let mut edges = None;
    let mut crash_at = None;
    let mut index = 0;
    while index < arguments.len() {
        let flag = &arguments[index];
        let value = required_value(arguments, index, flag)?;
        match flag.as_str() {
            "--cache-dir" => set_once(&mut cache_dir, absolute_path(value, flag)?, flag)?,
            "--symbols" => set_once(
                &mut symbols,
                bounded_usize(value, flag, 1, MAX_SYMBOLS)?,
                flag,
            )?,
            "--edges" => set_once(&mut edges, bounded_usize(value, flag, 0, MAX_EDGES)?, flag)?,
            "--crash-at" => set_once(&mut crash_at, parse_crash_point(value)?, flag)?,
            _ => return Err(invalid(format!("unknown build flag: {flag}"))),
        }
        index += 2;
    }
    Ok(BuildArgs {
        cache_dir: cache_dir.ok_or_else(|| invalid("missing --cache-dir"))?,
        symbols: symbols.ok_or_else(|| invalid("missing --symbols"))?,
        edges: edges.ok_or_else(|| invalid("missing --edges"))?,
        crash_at,
    })
}

fn parse_query(arguments: &[String]) -> Result<QueryArgs, SpikeError> {
    let mut generation = None;
    let mut symbol = None;
    let mut direction = None;
    let mut depth = None;
    let mut max_edges = None;
    let mut index = 0;
    while index < arguments.len() {
        let flag = &arguments[index];
        let value = required_value(arguments, index, flag)?;
        match flag.as_str() {
            "--generation" => set_once(&mut generation, absolute_path(value, flag)?, flag)?,
            "--symbol" => set_once(&mut symbol, nonempty(value, flag)?, flag)?,
            "--direction" => set_once(&mut direction, parse_direction(value)?, flag)?,
            "--depth" => set_once(&mut depth, bounded_usize(value, flag, 1, 2)?, flag)?,
            "--max-edges" => set_once(
                &mut max_edges,
                bounded_usize(value, flag, 1, MAX_QUERY_EDGES)?,
                flag,
            )?,
            _ => return Err(invalid(format!("unknown query flag: {flag}"))),
        }
        index += 2;
    }
    Ok(QueryArgs {
        generation: generation.ok_or_else(|| invalid("missing --generation"))?,
        symbol: symbol.ok_or_else(|| invalid("missing --symbol"))?,
        direction: direction.ok_or_else(|| invalid("missing --direction"))?,
        depth: depth.ok_or_else(|| invalid("missing --depth"))?,
        max_edges: max_edges.ok_or_else(|| invalid("missing --max-edges"))?,
    })
}

fn parse_doctor(arguments: &[String]) -> Result<DoctorArgs, SpikeError> {
    if arguments.len() != 2 || arguments[0] != "--generation" {
        return Err(invalid("doctor requires --generation <absolute-file>"));
    }
    Ok(DoctorArgs {
        generation: absolute_path(&arguments[1], "--generation")?,
    })
}

fn parse_benchmark(arguments: &[String]) -> Result<BenchmarkArgs, SpikeError> {
    let mut cache_dir = None;
    let mut symbols = None;
    let mut edges = None;
    let mut queries = None;
    let mut index = 0;
    while index < arguments.len() {
        let flag = &arguments[index];
        let value = required_value(arguments, index, flag)?;
        match flag.as_str() {
            "--cache-dir" => set_once(&mut cache_dir, absolute_path(value, flag)?, flag)?,
            "--symbols" => set_once(
                &mut symbols,
                bounded_usize(value, flag, 1, MAX_SYMBOLS)?,
                flag,
            )?,
            "--edges" => set_once(&mut edges, bounded_usize(value, flag, 0, MAX_EDGES)?, flag)?,
            "--queries" => set_once(
                &mut queries,
                bounded_usize(value, flag, 1, 1_000_000)?,
                flag,
            )?,
            _ => return Err(invalid(format!("unknown benchmark flag: {flag}"))),
        }
        index += 2;
    }
    Ok(BenchmarkArgs {
        cache_dir: cache_dir.ok_or_else(|| invalid("missing --cache-dir"))?,
        symbols: symbols.ok_or_else(|| invalid("missing --symbols"))?,
        edges: edges.ok_or_else(|| invalid("missing --edges"))?,
        queries: queries.ok_or_else(|| invalid("missing --queries"))?,
    })
}

fn required_value<'a>(
    arguments: &'a [String],
    index: usize,
    flag: &str,
) -> Result<&'a str, SpikeError> {
    arguments
        .get(index + 1)
        .map(String::as_str)
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| invalid(format!("missing value for {flag}")))
}

fn absolute_path(value: &str, flag: &str) -> Result<PathBuf, SpikeError> {
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(invalid(format!("{flag} must be absolute")));
    }
    Ok(path)
}

fn bounded_usize(
    value: &str,
    flag: &str,
    minimum: usize,
    maximum: usize,
) -> Result<usize, SpikeError> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| invalid(format!("invalid integer for {flag}")))?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(invalid(format!("{flag} must be in {minimum}..={maximum}")));
    }
    Ok(parsed)
}

fn nonempty(value: &str, flag: &str) -> Result<String, SpikeError> {
    if value.is_empty() {
        return Err(invalid(format!("{flag} must not be empty")));
    }
    Ok(value.to_owned())
}

fn set_once<T>(slot: &mut Option<T>, value: T, flag: &str) -> Result<(), SpikeError> {
    if slot.replace(value).is_some() {
        return Err(invalid(format!("duplicate {flag}")));
    }
    Ok(())
}

fn parse_crash_point(value: &str) -> Result<CrashPoint, SpikeError> {
    match value {
        "before-commit" => Ok(CrashPoint::BeforeCommit),
        "after-commit" => Ok(CrashPoint::AfterCommit),
        "after-sync" => Ok(CrashPoint::AfterSync),
        "before-publish" => Ok(CrashPoint::BeforePublish),
        _ => Err(invalid("unknown crash point")),
    }
}

fn parse_direction(value: &str) -> Result<Direction, SpikeError> {
    match value {
        "incoming" => Ok(Direction::Incoming),
        "outgoing" => Ok(Direction::Outgoing),
        _ => Err(invalid("direction must be incoming or outgoing")),
    }
}

fn print_help() -> Result<(), SpikeError> {
    const HELP: &str =
        "sqlite-storage-spike\n\ncommands:\n  build\n  query\n  doctor\n  benchmark\n";
    std::io::stdout().write_all(HELP.as_bytes())?;
    Ok(())
}

fn run_build(arguments: BuildArgs) -> Result<(), SpikeError> {
    let started = Instant::now();
    let (stats, outcome, _path) = build_generation(&arguments)?;
    let _ = (&stats.application_root, outcome);
    write_report(SpikeReport {
        schema_version: SCHEMA_VERSION as u8,
        kind: "sqlite-storage-spike-report",
        action: "build",
        status: "completed",
        generation_key: Some(stats.generation_key),
        symbols: stats.symbols,
        edges: stats.edges,
        elapsed_ms: duration_ms(started.elapsed()),
        output_bytes: 0,
        limitations: Vec::new(),
    })
}

fn build_generation(
    arguments: &BuildArgs,
) -> Result<(GenerationStats, PublishOutcome, PathBuf), SpikeError> {
    let _ = arguments.crash_at;
    let graph_directory = arguments.cache_dir.join("graphs");
    std::fs::create_dir_all(&graph_directory)?;
    let staging = NamedTempFile::new_in(&graph_directory)?;
    let mut connection = Connection::open(staging.path())?;
    configure_staging(&connection)?;
    create_schema(&connection)?;

    let generation_key = fixture_digest("generation", arguments.symbols, arguments.edges);
    let application_root = fixture_digest("application-root", arguments.symbols, arguments.edges);
    let transaction = connection.transaction()?;
    {
        let mut insert_symbol = transaction.prepare(
            "INSERT INTO symbols(symbol_id, path, start_line, end_line) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for index in 0..arguments.symbols {
            let line = sqlite_integer(index + 1, "symbol line")?;
            insert_symbol.execute(params![
                symbol_id(index),
                format!("src/module-{:03}.rs", index % 128),
                line,
                line,
            ])?;
        }
    }
    {
        let mut insert_edge = transaction
            .prepare("INSERT INTO edges(edge_id, from_symbol, to_symbol) VALUES (?1, ?2, ?3)")?;
        for index in 0..arguments.edges {
            insert_edge.execute(params![
                edge_id(index),
                symbol_id(index % arguments.symbols),
                symbol_id((index.saturating_mul(17).saturating_add(1)) % arguments.symbols),
            ])?;
        }
    }
    transaction.execute(
        "INSERT INTO generation_meta(
            schema_version, generation_key, symbol_count, edge_count, application_root
        ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            SCHEMA_VERSION,
            generation_key,
            sqlite_integer(arguments.symbols, "symbol count")?,
            sqlite_integer(arguments.edges, "edge count")?,
            application_root,
        ],
    )?;
    transaction.commit()?;
    connection.close().map_err(|(_, error)| error)?;
    staging.as_file().sync_all()?;

    let final_path = graph_directory.join(format!("{generation_key}.sqlite"));
    staging
        .persist(&final_path)
        .map_err(|error| SpikeError::Io(error.error))?;
    Ok((
        GenerationStats {
            generation_key,
            symbols: arguments.symbols,
            edges: arguments.edges,
            application_root,
        },
        PublishOutcome::Published,
        final_path,
    ))
}

fn configure_staging(connection: &Connection) -> Result<(), SpikeError> {
    connection.pragma_update(None, "journal_mode", "DELETE")?;
    connection.pragma_update(None, "synchronous", "EXTRA")?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "trusted_schema", false)?;
    connection.pragma_update(None, "application_id", APPLICATION_ID)?;
    connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn create_schema(connection: &Connection) -> Result<(), SpikeError> {
    connection.execute_batch(
        "CREATE TABLE generation_meta (
            schema_version INTEGER PRIMARY KEY,
            generation_key TEXT NOT NULL,
            symbol_count INTEGER NOT NULL,
            edge_count INTEGER NOT NULL,
            application_root TEXT NOT NULL
        );
        CREATE TABLE symbols (
            symbol_id TEXT PRIMARY KEY,
            path TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            end_line INTEGER NOT NULL
        );
        CREATE TABLE edges (
            edge_id TEXT PRIMARY KEY,
            from_symbol TEXT NOT NULL REFERENCES symbols(symbol_id),
            to_symbol TEXT NOT NULL REFERENCES symbols(symbol_id)
        );
        CREATE INDEX edges_from_id ON edges(from_symbol, edge_id);
        CREATE INDEX edges_to_id ON edges(to_symbol, edge_id);",
    )?;
    Ok(())
}

fn fixture_digest(domain: &str, symbols: usize, edges: usize) -> String {
    let mut digest = Sha256::new();
    update_digest(&mut digest, b"sqlite-storage-spike/v1");
    update_digest(&mut digest, domain.as_bytes());
    update_digest(&mut digest, &symbols.to_le_bytes());
    update_digest(&mut digest, &edges.to_le_bytes());
    for index in 0..symbols {
        update_digest(&mut digest, symbol_id(index).as_bytes());
        update_digest(
            &mut digest,
            format!("src/module-{:03}.rs", index % 128).as_bytes(),
        );
        update_digest(&mut digest, &(index + 1).to_le_bytes());
    }
    for index in 0..edges {
        update_digest(&mut digest, edge_id(index).as_bytes());
        update_digest(&mut digest, symbol_id(index % symbols).as_bytes());
        update_digest(
            &mut digest,
            symbol_id((index.saturating_mul(17).saturating_add(1)) % symbols).as_bytes(),
        );
    }
    format!("{:x}", digest.finalize())
}

fn update_digest(digest: &mut Sha256, bytes: &[u8]) {
    digest.update(bytes.len().to_le_bytes());
    digest.update(bytes);
}

fn symbol_id(index: usize) -> String {
    format!("symbol-{index:08}")
}

fn edge_id(index: usize) -> String {
    format!("edge-{index:08}")
}

fn write_report(mut report: SpikeReport) -> Result<(), SpikeError> {
    let bytes = loop {
        let bytes = serde_json::to_vec(&report)
            .map_err(|error| SpikeError::InvalidGeneration(error.to_string()))?;
        if bytes.len() == report.output_bytes {
            break bytes;
        }
        report.output_bytes = bytes.len();
    };
    std::io::stdout().write_all(&bytes)?;
    Ok(())
}

fn duration_ms(duration: std::time::Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn invalid(message: impl Into<String>) -> SpikeError {
    SpikeError::InvalidInput(message.into())
}

fn sqlite_integer(value: usize, field: &str) -> Result<i64, SpikeError> {
    i64::try_from(value).map_err(|_| invalid(format!("{field} exceeds SQLite integer range")))
}

impl Display for SpikeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(message)
            | Self::InvalidGeneration(message)
            | Self::InvalidExistingGeneration(message) => formatter.write_str(message),
            Self::Io(error) => Display::fmt(error, formatter),
            Self::Sqlite(error) => Display::fmt(error, formatter),
        }
    }
}

impl From<std::io::Error> for SpikeError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for SpikeError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl std::error::Error for SpikeError {}
