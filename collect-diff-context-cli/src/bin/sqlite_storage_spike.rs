use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fmt::{Display, Formatter};
use std::io::Write;
use std::path::{Path, PathBuf};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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

struct QueryOutcome {
    edges: usize,
    visited_symbols: usize,
    partial: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublishOutcome {
    Published,
    Reused,
}

#[derive(Debug)]
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
    match run() {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("sqlite-storage-spike: {error}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<i32, SpikeError> {
    match parse_command(env::args().skip(1).collect())? {
        Command::Help => {
            print_help()?;
            Ok(0)
        }
        Command::Build(arguments) => {
            run_build(arguments)?;
            Ok(0)
        }
        Command::Query(arguments) => {
            run_query(arguments)?;
            Ok(0)
        }
        Command::Doctor(arguments) => run_doctor(arguments),
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

fn run_doctor(arguments: DoctorArgs) -> Result<i32, SpikeError> {
    let started = Instant::now();
    let expected_key = expected_generation_key(&arguments.generation)?;
    match open_immutable(&arguments.generation)
        .and_then(|connection| validate_generation(&connection, &expected_key))
    {
        Ok(stats) => {
            write_report(SpikeReport {
                schema_version: SCHEMA_VERSION as u8,
                kind: "sqlite-storage-spike-report",
                action: "doctor",
                status: "completed",
                generation_key: Some(stats.generation_key),
                symbols: stats.symbols,
                edges: stats.edges,
                elapsed_ms: duration_ms(started.elapsed()),
                output_bytes: 0,
                limitations: Vec::new(),
            })?;
            Ok(0)
        }
        Err(error) => {
            write_report(SpikeReport {
                schema_version: SCHEMA_VERSION as u8,
                kind: "sqlite-storage-spike-report",
                action: "doctor",
                status: "corrupt",
                generation_key: Some(expected_key),
                symbols: 0,
                edges: 0,
                elapsed_ms: duration_ms(started.elapsed()),
                output_bytes: 0,
                limitations: vec![error.code().to_owned()],
            })?;
            Ok(2)
        }
    }
}

fn run_query(arguments: QueryArgs) -> Result<(), SpikeError> {
    let started = Instant::now();
    let generation_key = expected_generation_key(&arguments.generation)?;
    let connection = open_immutable(&arguments.generation)?;
    validate_generation(&connection, &generation_key)?;
    let outcome = query_graph(&connection, &arguments)?;
    write_report(SpikeReport {
        schema_version: SCHEMA_VERSION as u8,
        kind: "sqlite-storage-spike-report",
        action: "query",
        status: if outcome.partial {
            "partial"
        } else {
            "completed"
        },
        generation_key: Some(generation_key),
        symbols: outcome.visited_symbols,
        edges: outcome.edges,
        elapsed_ms: duration_ms(started.elapsed()),
        output_bytes: 0,
        limitations: if outcome.partial {
            vec!["edge-budget-exhausted".to_owned()]
        } else {
            Vec::new()
        },
    })
}

fn query_graph(connection: &Connection, arguments: &QueryArgs) -> Result<QueryOutcome, SpikeError> {
    let mut frontier = vec![arguments.symbol.clone()];
    let mut visited = BTreeSet::new();
    let mut accepted_edges = BTreeSet::new();
    let mut partial = false;

    for _ in 0..arguments.depth {
        frontier.sort();
        frontier.dedup();
        let mut next_frontier = Vec::new();
        for symbol in std::mem::take(&mut frontier) {
            if !visited.insert((arguments.direction, symbol.clone())) {
                continue;
            }
            let remaining = arguments.max_edges.saturating_sub(accepted_edges.len());
            if remaining == 0 {
                partial = true;
                break;
            }
            let rows = query_adjacent(
                connection,
                &symbol,
                arguments.direction,
                remaining.saturating_add(1),
            )?;
            if rows.len() > remaining {
                partial = true;
            }
            for (edge_id, adjacent) in rows.into_iter().take(remaining) {
                accepted_edges.insert(edge_id);
                next_frontier.push(adjacent);
            }
            if partial {
                break;
            }
        }
        if partial || next_frontier.is_empty() {
            break;
        }
        frontier = next_frontier;
    }

    Ok(QueryOutcome {
        edges: accepted_edges.len(),
        visited_symbols: visited.len(),
        partial,
    })
}

fn query_adjacent(
    connection: &Connection,
    symbol: &str,
    direction: Direction,
    maximum_rows: usize,
) -> Result<Vec<(String, String)>, SpikeError> {
    let sql = match direction {
        Direction::Outgoing => {
            "SELECT edge_id, to_symbol FROM edges
             WHERE from_symbol = ?1 ORDER BY edge_id LIMIT ?2"
        }
        Direction::Incoming => {
            "SELECT edge_id, from_symbol FROM edges
             WHERE to_symbol = ?1 ORDER BY edge_id LIMIT ?2"
        }
    };
    let mut statement = connection.prepare(sql)?;
    let rows = statement
        .query_map(
            params![symbol, sqlite_integer(maximum_rows, "query row limit")?],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn build_generation(
    arguments: &BuildArgs,
) -> Result<(GenerationStats, PublishOutcome, PathBuf), SpikeError> {
    let graph_directory = arguments.cache_dir.join("graphs");
    let staging_directory = arguments.cache_dir.join("staging");
    std::fs::create_dir_all(&graph_directory)?;
    std::fs::create_dir_all(&staging_directory)?;
    let staging = NamedTempFile::new_in(&staging_directory)?;
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
    crash_if(arguments.crash_at, CrashPoint::BeforeCommit);
    transaction.commit()?;
    crash_if(arguments.crash_at, CrashPoint::AfterCommit);
    connection.close().map_err(|(_, error)| error)?;
    staging.as_file().sync_all()?;
    crash_if(arguments.crash_at, CrashPoint::AfterSync);

    let staging_reader = open_immutable(staging.path())?;
    let stats = validate_generation(&staging_reader, &generation_key)?;
    drop(staging_reader);
    crash_if(arguments.crash_at, CrashPoint::BeforePublish);

    let final_path = graph_directory.join(format!("{generation_key}.sqlite"));
    let outcome = publish_noclobber(staging, &final_path)?;
    Ok((stats, outcome, final_path))
}

fn crash_if(actual: Option<CrashPoint>, expected: CrashPoint) {
    if actual == Some(expected) {
        std::process::exit(99);
    }
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

fn validate_generation(
    connection: &Connection,
    expected_key: &str,
) -> Result<GenerationStats, SpikeError> {
    let application_id: i32 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    if application_id != APPLICATION_ID {
        return Err(invalid_generation("application-id-mismatch"));
    }
    let user_version: i32 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if user_version != SCHEMA_VERSION {
        return Err(invalid_generation("schema-version-mismatch"));
    }

    let metadata_rows: i64 =
        connection.query_row("SELECT COUNT(*) FROM generation_meta", [], |row| row.get(0))?;
    if metadata_rows != 1 {
        return Err(invalid_generation("metadata-row-count-mismatch"));
    }
    let (schema_version, generation_key, symbols, edges, stored_root): (
        i32,
        String,
        i64,
        i64,
        String,
    ) = connection.query_row(
        "SELECT schema_version, generation_key, symbol_count, edge_count, application_root
         FROM generation_meta",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    if schema_version != SCHEMA_VERSION {
        return Err(invalid_generation("schema-version-mismatch"));
    }
    if generation_key != expected_key {
        return Err(invalid_generation("generation-key-mismatch"));
    }

    let symbols = usize_from_sql(symbols, "symbol-count-invalid")?;
    let edges = usize_from_sql(edges, "edge-count-invalid")?;
    let queried_symbols: i64 =
        connection.query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))?;
    let queried_edges: i64 =
        connection.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))?;
    if usize_from_sql(queried_symbols, "symbol-count-invalid")? != symbols {
        return Err(invalid_generation("symbol-count-mismatch"));
    }
    if usize_from_sql(queried_edges, "edge-count-invalid")? != edges {
        return Err(invalid_generation("edge-count-mismatch"));
    }

    let mut foreign_keys = connection.prepare("PRAGMA foreign_key_check")?;
    if foreign_keys.query([])?.next()?.is_some() {
        return Err(invalid_generation("foreign-key-mismatch"));
    }
    integrity_check(connection)?;

    let computed_root = application_root(connection)?;
    if stored_root != computed_root {
        return Err(invalid_generation("application-root-mismatch"));
    }
    Ok(GenerationStats {
        generation_key,
        symbols,
        edges,
        application_root: computed_root,
    })
}

fn application_root(connection: &Connection) -> Result<String, SpikeError> {
    let symbol_count: i64 =
        connection.query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))?;
    let edge_count: i64 =
        connection.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))?;
    let symbol_count = usize_from_sql(symbol_count, "symbol-count-invalid")?;
    let edge_count = usize_from_sql(edge_count, "edge-count-invalid")?;

    let mut digest = Sha256::new();
    update_digest(&mut digest, b"sqlite-storage-spike/v1");
    update_digest(&mut digest, b"application-root");
    update_digest(&mut digest, &(symbol_count as u64).to_le_bytes());
    update_digest(&mut digest, &(edge_count as u64).to_le_bytes());

    let mut symbols = connection
        .prepare("SELECT symbol_id, path, start_line, end_line FROM symbols ORDER BY symbol_id")?;
    let mut symbol_rows = symbols.query([])?;
    while let Some(row) = symbol_rows.next()? {
        let symbol_id: String = row.get(0)?;
        let path: String = row.get(1)?;
        let start_line: i64 = row.get(2)?;
        let end_line: i64 = row.get(3)?;
        update_digest(&mut digest, symbol_id.as_bytes());
        update_digest(&mut digest, path.as_bytes());
        update_digest(
            &mut digest,
            &u64_from_sql(start_line, "symbol-range-invalid")?.to_le_bytes(),
        );
        update_digest(
            &mut digest,
            &u64_from_sql(end_line, "symbol-range-invalid")?.to_le_bytes(),
        );
    }

    let mut edges =
        connection.prepare("SELECT edge_id, from_symbol, to_symbol FROM edges ORDER BY edge_id")?;
    let mut edge_rows = edges.query([])?;
    while let Some(row) = edge_rows.next()? {
        let edge_id: String = row.get(0)?;
        let from_symbol: String = row.get(1)?;
        let to_symbol: String = row.get(2)?;
        update_digest(&mut digest, edge_id.as_bytes());
        update_digest(&mut digest, from_symbol.as_bytes());
        update_digest(&mut digest, to_symbol.as_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn integrity_check(connection: &Connection) -> Result<(), SpikeError> {
    let mut statement = connection.prepare("PRAGMA integrity_check")?;
    let checks = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if checks.as_slice() != ["ok"] {
        return Err(invalid_generation("sqlite-integrity-check-failed"));
    }
    Ok(())
}

fn publish_noclobber(
    staging: NamedTempFile,
    final_path: &Path,
) -> Result<PublishOutcome, SpikeError> {
    staging.as_file().sync_all()?;
    match staging.persist_noclobber(final_path) {
        Ok(_) => Ok(PublishOutcome::Published),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let expected_key = expected_generation_key(final_path)?;
            match open_immutable(final_path)
                .and_then(|connection| validate_generation(&connection, &expected_key))
            {
                Ok(_) => Ok(PublishOutcome::Reused),
                Err(validation_error) => Err(SpikeError::InvalidExistingGeneration(format!(
                    "invalid-existing-generation:{}",
                    validation_error.code()
                ))),
            }
        }
        Err(error) => Err(SpikeError::Io(error.error)),
    }
}

fn open_immutable(path: &Path) -> Result<Connection, SpikeError> {
    let path = path
        .to_str()
        .ok_or_else(|| invalid("generation path is not UTF-8"))?;
    let encoded = utf8_percent_encode(path, NON_ALPHANUMERIC);
    let uri = format!("file:{encoded}?mode=ro&immutable=1");
    let connection = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "query_only", true)?;
    connection.pragma_update(None, "trusted_schema", false)?;
    Ok(connection)
}

fn expected_generation_key(path: &Path) -> Result<String, SpikeError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("generation path has no UTF-8 filename"))?;
    let key = name
        .strip_suffix(".sqlite")
        .ok_or_else(|| invalid("generation filename must end in .sqlite"))?;
    if key.len() != 64
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("generation filename must be 64 lowercase hex"));
    }
    Ok(key.to_owned())
}

fn fixture_digest(domain: &str, symbols: usize, edges: usize) -> String {
    let mut digest = Sha256::new();
    update_digest(&mut digest, b"sqlite-storage-spike/v1");
    update_digest(&mut digest, domain.as_bytes());
    update_digest(&mut digest, &(symbols as u64).to_le_bytes());
    update_digest(&mut digest, &(edges as u64).to_le_bytes());
    for index in 0..symbols {
        update_digest(&mut digest, symbol_id(index).as_bytes());
        update_digest(
            &mut digest,
            format!("src/module-{:03}.rs", index % 128).as_bytes(),
        );
        update_digest(&mut digest, &((index + 1) as u64).to_le_bytes());
        update_digest(&mut digest, &((index + 1) as u64).to_le_bytes());
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

fn invalid_generation(code: impl Into<String>) -> SpikeError {
    SpikeError::InvalidGeneration(code.into())
}

fn sqlite_integer(value: usize, field: &str) -> Result<i64, SpikeError> {
    i64::try_from(value).map_err(|_| invalid(format!("{field} exceeds SQLite integer range")))
}

fn usize_from_sql(value: i64, code: &'static str) -> Result<usize, SpikeError> {
    usize::try_from(value).map_err(|_| invalid_generation(code))
}

fn u64_from_sql(value: i64, code: &'static str) -> Result<u64, SpikeError> {
    u64::try_from(value).map_err(|_| invalid_generation(code))
}

impl SpikeError {
    fn code(&self) -> &str {
        match self {
            Self::InvalidInput(_) => "invalid-input",
            Self::Io(_) => "io-error",
            Self::Sqlite(_) => "sqlite-error",
            Self::InvalidGeneration(code) => code,
            Self::InvalidExistingGeneration(_) => "invalid-existing-generation",
        }
    }
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
