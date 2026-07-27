use crate::candidate::CandidatePresence;
use crate::impact_context::cache::file_facts::{
    set_private_file_permissions, sync_directory, CacheLayout, CacheLookup,
};
use crate::impact_context::cache::integrity::{
    canonical_graph_rows, graph_rows_root, CanonicalGraphRows,
};
use crate::impact_context::cache::locking::acquire_writer_lock;
use crate::impact_context::contracts::{
    Completeness, Confidence, EdgeKind, Resolution, SourceRange,
};
use crate::impact_context::index::budget::{IndexBudgetTracker, IndexResource};
use crate::impact_context::index::model::{
    GraphEdge, GraphGenerationIdentity, GraphSymbol, IndexLimitation, RepositoryGraph,
};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

const APPLICATION_ID: i32 = 0x5052_4349;
const DATABASE_SCHEMA_VERSION: i32 = 1;
const SQLITE_PAGE_BYTES: usize = 4_096;

#[derive(Debug, Clone)]
pub struct RepositoryGraphWriter {
    layout: CacheLayout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderLimits {
    pub maximum_database_bytes: u64,
    pub maximum_rows_per_query: usize,
    pub maximum_string_bytes: usize,
}

#[derive(Debug)]
pub struct RepositoryGraphReader {
    connection: Connection,
    identity: GraphGenerationIdentity,
    completeness: Completeness,
    limits: ReaderLimits,
    query_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphPublishOutcome {
    Published { path: PathBuf },
    Reused { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryGraphError {
    pub code: &'static str,
    pub message: String,
}

enum ReaderValidationError {
    Stale(&'static str),
    Corrupt(&'static str),
}

impl RepositoryGraphError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RepositoryGraphError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RepositoryGraphError {}

impl RepositoryGraphWriter {
    pub fn new(layout: CacheLayout) -> Self {
        Self { layout }
    }

    pub fn layout(&self) -> &CacheLayout {
        &self.layout
    }

    pub fn generation_path(
        &self,
        identity: &GraphGenerationIdentity,
    ) -> Result<PathBuf, RepositoryGraphError> {
        let key = identity.generation_key().map_err(|error| {
            RepositoryGraphError::new(
                "generation-identity-invalid",
                format!("invalid graph generation identity: {error}"),
            )
        })?;
        Ok(self.layout.graphs_dir.join(format!("{key}.sqlite")))
    }

    pub fn publish(
        &self,
        graph: &RepositoryGraph,
        budget: &mut IndexBudgetTracker,
    ) -> Result<GraphPublishOutcome, RepositoryGraphError> {
        validate_graph(graph)?;
        budget.check_deadline().map_err(budget_error)?;
        budget
            .observe(IndexResource::Symbols, graph.symbols.len())
            .map_err(budget_error)?;
        budget
            .observe(IndexResource::Edges, graph.edges.len())
            .map_err(budget_error)?;

        let rows = canonical_graph_rows(graph).map_err(|error| {
            RepositoryGraphError::new(
                "generation-canonicalization-failed",
                format!("cannot canonicalize repository graph: {error}"),
            )
        })?;
        let generation_key = graph.identity.generation_key().map_err(|error| {
            RepositoryGraphError::new(
                "generation-identity-invalid",
                format!("invalid graph generation identity: {error}"),
            )
        })?;
        let final_path = self.generation_path(&graph.identity)?;
        if final_path.exists() {
            return self.reuse_existing(&final_path, graph, &rows);
        }

        self.layout.ensure_private_directories().map_err(|error| {
            RepositoryGraphError::new("generation-cache-layout-failed", error.to_string())
        })?;
        let _lock = acquire_writer_lock(&self.layout, &generation_key, budget.remaining_deadline())
            .map_err(|error| RepositoryGraphError::new(error.code, error.message))?;
        budget.check_deadline().map_err(budget_error)?;
        if final_path.exists() {
            return self.reuse_existing(&final_path, graph, &rows);
        }

        let temporary = NamedTempFile::new_in(&self.layout.staging_dir).map_err(|error| {
            RepositoryGraphError::new(
                "generation-staging-create-failed",
                format!("cannot create graph staging file: {error}"),
            )
        })?;
        set_private_file_permissions(temporary.as_file()).map_err(|error| {
            RepositoryGraphError::new("generation-permission-failed", error.to_string())
        })?;
        write_generation(
            temporary.path(),
            graph,
            &rows,
            &generation_key,
            budget.budget().max_generation_bytes,
        )?;
        temporary.as_file().sync_all().map_err(|error| {
            RepositoryGraphError::new(
                "generation-sync-failed",
                format!("cannot sync graph staging file: {error}"),
            )
        })?;
        let database_bytes = fs::metadata(temporary.path())
            .map_err(|error| {
                RepositoryGraphError::new(
                    "generation-metadata-failed",
                    format!("cannot inspect graph staging file: {error}"),
                )
            })?
            .len();
        let database_bytes = usize::try_from(database_bytes).map_err(|_| {
            RepositoryGraphError::new(
                "index-generation-byte-budget-exhausted",
                "graph generation size exceeds this platform's addressable range",
            )
        })?;
        budget
            .observe(IndexResource::GenerationBytes, database_bytes)
            .map_err(budget_error)?;
        validate_generation(temporary.path(), graph, &rows)?;
        budget.check_deadline().map_err(budget_error)?;

        match temporary.persist_noclobber(&final_path) {
            Ok(_) => {
                sync_directory(&self.layout.graphs_dir).map_err(|error| {
                    RepositoryGraphError::new("generation-directory-sync-failed", error.to_string())
                })?;
                Ok(GraphPublishOutcome::Published { path: final_path })
            }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                self.reuse_existing(&final_path, graph, &rows)
            }
            Err(error) => Err(RepositoryGraphError::new(
                "generation-publish-failed",
                format!("cannot publish graph generation: {}", error.error),
            )),
        }
    }

    fn reuse_existing(
        &self,
        path: &Path,
        graph: &RepositoryGraph,
        rows: &CanonicalGraphRows,
    ) -> Result<GraphPublishOutcome, RepositoryGraphError> {
        validate_generation(path, graph, rows).map_err(|error| {
            RepositoryGraphError::new(
                "invalid-existing-generation",
                format!("existing immutable graph generation is invalid: {error}"),
            )
        })?;
        Ok(GraphPublishOutcome::Reused {
            path: path.to_path_buf(),
        })
    }
}

impl RepositoryGraphReader {
    pub fn read_identity_immutable(
        path: &Path,
        limits: ReaderLimits,
    ) -> Result<CacheLookup<GraphGenerationIdentity>, RepositoryGraphError> {
        validate_reader_limits(limits)?;
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CacheLookup::Miss)
            }
            Err(error) => {
                return Err(RepositoryGraphError::new(
                    "reader-metadata-failed",
                    format!("cannot inspect graph generation: {error}"),
                ))
            }
        };
        if !metadata.file_type().is_file() {
            return Ok(reader_corrupt("generation-not-regular"));
        }
        if metadata.len() > limits.maximum_database_bytes {
            return Ok(reader_corrupt("generation-database-too-large"));
        }
        let connection = match open_immutable_connection(path) {
            Ok(connection) => connection,
            Err(_) => return Ok(reader_corrupt("generation-open-failed")),
        };
        let identity_json =
            match connection.query_row("SELECT identity_json FROM generation_meta", [], |row| {
                row.get::<_, String>(0)
            }) {
                Ok(identity_json) => identity_json,
                Err(_) => return Ok(reader_corrupt("generation-metadata-invalid")),
            };
        if bounded_reader_text(
            &identity_json,
            limits.maximum_string_bytes.saturating_mul(16),
        )
        .is_err()
        {
            return Ok(reader_corrupt("generation-identity-too-large"));
        }
        let identity: GraphGenerationIdentity = match serde_json::from_str(&identity_json) {
            Ok(identity) => identity,
            Err(_) => return Ok(reader_corrupt("generation-identity-invalid")),
        };
        if identity.validate().is_err() {
            return Ok(reader_corrupt("generation-identity-invalid"));
        }
        let expected_key = identity.generation_key().map_err(|error| {
            RepositoryGraphError::new("reader-identity-invalid", error.to_string())
        })?;
        if generation_key_from_path(path).as_deref() != Some(expected_key.as_str()) {
            return Ok(CacheLookup::Stale {
                code: "generation-filename-stale".to_string(),
            });
        }
        Ok(CacheLookup::Hit(identity))
    }

    pub fn open_immutable(
        path: &Path,
        expected: &GraphGenerationIdentity,
        limits: ReaderLimits,
    ) -> Result<CacheLookup<Self>, RepositoryGraphError> {
        validate_reader_limits(limits)?;
        expected.validate().map_err(|error| {
            RepositoryGraphError::new(
                "reader-identity-invalid",
                format!("invalid expected graph identity: {error}"),
            )
        })?;
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CacheLookup::Miss)
            }
            Err(error) => {
                return Err(RepositoryGraphError::new(
                    "reader-metadata-failed",
                    format!("cannot inspect graph generation: {error}"),
                ))
            }
        };
        if !metadata.file_type().is_file() {
            return Ok(reader_corrupt("generation-not-regular"));
        }
        if metadata.len() > limits.maximum_database_bytes {
            return Ok(reader_corrupt("generation-database-too-large"));
        }
        let expected_key = expected.generation_key().map_err(|error| {
            RepositoryGraphError::new("reader-identity-invalid", error.to_string())
        })?;
        if generation_key_from_path(path).as_deref() != Some(expected_key.as_str()) {
            return Ok(CacheLookup::Stale {
                code: "generation-filename-stale".to_string(),
            });
        }
        let connection = match open_immutable_connection(path) {
            Ok(connection) => connection,
            Err(_) => return Ok(reader_corrupt("generation-open-failed")),
        };
        let (identity, completeness, query_only) =
            match validate_reader_metadata(&connection, expected, limits) {
                Ok(metadata) => metadata,
                Err(ReaderValidationError::Stale(code)) => {
                    return Ok(CacheLookup::Stale {
                        code: code.to_string(),
                    })
                }
                Err(ReaderValidationError::Corrupt(code)) => return Ok(reader_corrupt(code)),
            };
        Ok(CacheLookup::Hit(Self {
            connection,
            identity,
            completeness,
            limits,
            query_only,
        }))
    }

    pub fn identity(&self) -> &GraphGenerationIdentity {
        &self.identity
    }

    pub fn completeness(&self) -> Completeness {
        self.completeness
    }

    pub fn query_only(&self) -> bool {
        self.query_only
    }

    pub fn integrity_check(&self) -> Result<(), RepositoryGraphError> {
        let result: String = self
            .connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(sqlite_error)?;
        if result == "ok" {
            Ok(())
        } else {
            Err(RepositoryGraphError::new(
                "generation-integrity-check-failed",
                "SQLite integrity_check did not return ok",
            ))
        }
    }

    pub fn maximum_rows_per_query(&self) -> usize {
        self.limits.maximum_rows_per_query
    }

    pub fn outgoing(
        &self,
        symbol: &str,
        maximum_rows: usize,
    ) -> Result<Vec<GraphEdge>, RepositoryGraphError> {
        self.query_edges(symbol, maximum_rows, true)
    }

    pub fn incoming(
        &self,
        symbol: &str,
        maximum_rows: usize,
    ) -> Result<Vec<GraphEdge>, RepositoryGraphError> {
        self.query_edges(symbol, maximum_rows, false)
    }

    pub fn symbols_for_path(
        &self,
        path: &crate::candidate::RepoPath,
        maximum_rows: usize,
    ) -> Result<Vec<GraphSymbol>, RepositoryGraphError> {
        self.validate_query_row_limit(maximum_rows)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT canonical_json FROM symbols
                 WHERE path = ?1 ORDER BY symbol_id LIMIT ?2",
            )
            .map_err(sqlite_error)?;
        let mut rows = statement
            .query(params![
                path.as_str(),
                sqlite_integer(maximum_rows, "query row limit")?
            ])
            .map_err(sqlite_error)?;
        let mut symbols = Vec::new();
        while let Some(row) = rows.next().map_err(sqlite_error)? {
            let canonical = row_text(row, 0, self.limits.maximum_string_bytes.saturating_mul(16))?;
            let symbol: GraphSymbol =
                serde_json::from_str(&canonical).map_err(|_| row_corrupt())?;
            if symbol.path != *path
                || validate_hex(&symbol.symbol_id).is_err()
                || validate_hex(&symbol.module_id).is_err()
                || validate_range(&symbol.range).is_err()
            {
                return Err(row_corrupt());
            }
            symbols.push(symbol);
        }
        Ok(symbols)
    }

    pub fn symbol(&self, symbol_id: &str) -> Result<Option<GraphSymbol>, RepositoryGraphError> {
        validate_hex(symbol_id).map_err(|_| {
            RepositoryGraphError::new(
                "reader-symbol-id-invalid",
                "query symbol id must be 64 lowercase hex",
            )
        })?;
        let canonical = self
            .connection
            .query_row(
                "SELECT canonical_json FROM symbols WHERE symbol_id = ?1",
                [symbol_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(sqlite_error)?;
        let Some(canonical) = canonical else {
            return Ok(None);
        };
        bounded_reader_text(
            &canonical,
            self.limits.maximum_string_bytes.saturating_mul(16),
        )
        .map_err(|_| row_corrupt())?;
        let symbol: GraphSymbol = serde_json::from_str(&canonical).map_err(|_| row_corrupt())?;
        if symbol.symbol_id != symbol_id
            || validate_hex(&symbol.module_id).is_err()
            || validate_range(&symbol.range).is_err()
        {
            return Err(row_corrupt());
        }
        Ok(Some(symbol))
    }

    pub fn edges_for_path(
        &self,
        path: &crate::candidate::RepoPath,
        maximum_rows: usize,
    ) -> Result<Vec<GraphEdge>, RepositoryGraphError> {
        self.validate_query_row_limit(maximum_rows)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT edge_id, kind, from_symbol, to_symbol, unresolved_target, path,
                        start_line, start_column, end_line, end_column, start_byte, end_byte,
                        provider_id, provider_version, resolution, confidence, limitation_code,
                        canonical_json
                 FROM edges WHERE path = ?1 ORDER BY edge_id LIMIT ?2",
            )
            .map_err(sqlite_error)?;
        let mut rows = statement
            .query(params![
                path.as_str(),
                sqlite_integer(maximum_rows, "query row limit")?
            ])
            .map_err(sqlite_error)?;
        let mut edges = Vec::new();
        while let Some(row) = rows.next().map_err(sqlite_error)? {
            let edge = decode_edge_row(row, self.limits)?;
            if edge.path != *path {
                return Err(row_corrupt());
            }
            edges.push(edge);
        }
        Ok(edges)
    }

    fn query_edges(
        &self,
        symbol: &str,
        maximum_rows: usize,
        outgoing: bool,
    ) -> Result<Vec<GraphEdge>, RepositoryGraphError> {
        self.validate_query_row_limit(maximum_rows)?;
        validate_hex(symbol).map_err(|_| {
            RepositoryGraphError::new(
                "reader-symbol-id-invalid",
                "query symbol id must be 64 lowercase hex",
            )
        })?;
        let sql = if outgoing {
            "SELECT edge_id, kind, from_symbol, to_symbol, unresolved_target, path,
                    start_line, start_column, end_line, end_column, start_byte, end_byte,
                    provider_id, provider_version, resolution, confidence, limitation_code,
                    canonical_json
             FROM edges WHERE from_symbol = ?1 ORDER BY kind, edge_id LIMIT ?2"
        } else {
            "SELECT edge_id, kind, from_symbol, to_symbol, unresolved_target, path,
                    start_line, start_column, end_line, end_column, start_byte, end_byte,
                    provider_id, provider_version, resolution, confidence, limitation_code,
                    canonical_json
             FROM edges WHERE to_symbol = ?1 ORDER BY kind, edge_id LIMIT ?2"
        };
        let mut statement = self.connection.prepare(sql).map_err(sqlite_error)?;
        let mut rows = statement
            .query(params![
                symbol,
                sqlite_integer(maximum_rows, "query row limit")?
            ])
            .map_err(sqlite_error)?;
        let mut edges = Vec::new();
        while let Some(row) = rows.next().map_err(sqlite_error)? {
            edges.push(decode_edge_row(row, self.limits)?);
        }
        Ok(edges)
    }

    fn validate_query_row_limit(&self, maximum_rows: usize) -> Result<(), RepositoryGraphError> {
        if maximum_rows == 0 || maximum_rows > self.limits.maximum_rows_per_query {
            return Err(RepositoryGraphError::new(
                "reader-row-limit-invalid",
                "query row limit is zero or exceeds the reader limit",
            ));
        }
        Ok(())
    }
}

fn write_generation(
    path: &Path,
    graph: &RepositoryGraph,
    rows: &CanonicalGraphRows,
    generation_key: &str,
    maximum_generation_bytes: usize,
) -> Result<(), RepositoryGraphError> {
    let mut connection = Connection::open(path).map_err(sqlite_error)?;
    configure_staging(&connection, maximum_generation_bytes)?;
    create_schema(&connection)?;
    let transaction = connection.transaction().map_err(sqlite_error)?;
    insert_files(&transaction, graph, rows)?;
    insert_modules(&transaction, graph, rows)?;
    insert_symbols(&transaction, graph, rows)?;
    insert_edges(&transaction, graph, rows)?;
    insert_limitations(&transaction, graph, rows)?;
    transaction
        .execute(
            "INSERT INTO generation_meta(
                schema_version, generation_key, identity_json, completeness,
                file_count, module_count, symbol_count, edge_count, limitation_count,
                application_root
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                DATABASE_SCHEMA_VERSION,
                generation_key,
                rows.identity,
                rows.completeness,
                sqlite_integer(graph.files.len(), "file count")?,
                sqlite_integer(graph.modules.len(), "module count")?,
                sqlite_integer(graph.symbols.len(), "symbol count")?,
                sqlite_integer(graph.edges.len(), "edge count")?,
                sqlite_integer(graph.limitations.len(), "limitation count")?,
                graph_rows_root(rows),
            ],
        )
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)?;
    connection.close().map_err(|(_, error)| sqlite_error(error))
}

fn configure_staging(
    connection: &Connection,
    maximum_generation_bytes: usize,
) -> Result<(), RepositoryGraphError> {
    let maximum_pages = maximum_generation_bytes.div_ceil(SQLITE_PAGE_BYTES).max(1);
    connection
        .pragma_update(
            None,
            "page_size",
            sqlite_integer(SQLITE_PAGE_BYTES, "page size")?,
        )
        .map_err(sqlite_error)?;
    connection
        .pragma_update(
            None,
            "max_page_count",
            sqlite_integer(maximum_pages, "maximum page count")?,
        )
        .map_err(sqlite_error)?;
    connection
        .pragma_update(None, "journal_mode", "DELETE")
        .map_err(sqlite_error)?;
    connection
        .pragma_update(None, "synchronous", "EXTRA")
        .map_err(sqlite_error)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(sqlite_error)?;
    connection
        .pragma_update(None, "trusted_schema", false)
        .map_err(sqlite_error)?;
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .map_err(sqlite_error)?;
    connection
        .pragma_update(None, "user_version", DATABASE_SCHEMA_VERSION)
        .map_err(sqlite_error)
}

fn create_schema(connection: &Connection) -> Result<(), RepositoryGraphError> {
    connection
        .execute_batch(
            "CREATE TABLE generation_meta (
                schema_version INTEGER PRIMARY KEY,
                generation_key TEXT NOT NULL,
                identity_json TEXT NOT NULL,
                completeness TEXT NOT NULL,
                file_count INTEGER NOT NULL,
                module_count INTEGER NOT NULL,
                symbol_count INTEGER NOT NULL,
                edge_count INTEGER NOT NULL,
                limitation_count INTEGER NOT NULL,
                application_root TEXT NOT NULL
            );
            CREATE TABLE files (
                path TEXT PRIMARY KEY,
                mode TEXT NOT NULL,
                presence TEXT NOT NULL,
                content_sha256 TEXT,
                file_fact_key_json TEXT,
                language TEXT,
                module_id TEXT,
                canonical_json TEXT NOT NULL,
                FOREIGN KEY(module_id) REFERENCES modules(module_id) DEFERRABLE INITIALLY DEFERRED
            );
            CREATE TABLE modules (
                module_id TEXT PRIMARY KEY,
                parent_module_id TEXT,
                crate_name TEXT NOT NULL,
                path TEXT NOT NULL REFERENCES files(path) DEFERRABLE INITIALLY DEFERRED,
                inline INTEGER NOT NULL,
                root_module INTEGER NOT NULL,
                resolution_status TEXT NOT NULL,
                canonical_json TEXT NOT NULL,
                FOREIGN KEY(parent_module_id) REFERENCES modules(module_id) DEFERRABLE INITIALLY DEFERRED
            );
            CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY,
                local_id TEXT NOT NULL,
                module_id TEXT NOT NULL REFERENCES modules(module_id) DEFERRABLE INITIALLY DEFERRED,
                path TEXT NOT NULL REFERENCES files(path) DEFERRABLE INITIALLY DEFERRED,
                language TEXT NOT NULL,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                owner_symbol_id TEXT,
                signature TEXT,
                visibility TEXT,
                start_line INTEGER NOT NULL,
                start_column INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                end_column INTEGER NOT NULL,
                start_byte INTEGER NOT NULL,
                end_byte INTEGER NOT NULL,
                confidence TEXT NOT NULL,
                canonical_json TEXT NOT NULL,
                FOREIGN KEY(owner_symbol_id) REFERENCES symbols(symbol_id) DEFERRABLE INITIALLY DEFERRED
            );
            CREATE TABLE edges (
                edge_id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                from_symbol TEXT NOT NULL REFERENCES symbols(symbol_id) DEFERRABLE INITIALLY DEFERRED,
                to_symbol TEXT REFERENCES symbols(symbol_id) DEFERRABLE INITIALLY DEFERRED,
                unresolved_target TEXT,
                path TEXT NOT NULL REFERENCES files(path) DEFERRABLE INITIALLY DEFERRED,
                start_line INTEGER NOT NULL,
                start_column INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                end_column INTEGER NOT NULL,
                start_byte INTEGER NOT NULL,
                end_byte INTEGER NOT NULL,
                provider_id TEXT NOT NULL,
                provider_version TEXT NOT NULL,
                resolution TEXT NOT NULL,
                confidence TEXT NOT NULL,
                limitation_code TEXT,
                canonical_json TEXT NOT NULL,
                CHECK ((to_symbol IS NULL) <> (unresolved_target IS NULL))
            );
            CREATE TABLE limitations (
                limitation_id TEXT PRIMARY KEY,
                sort_order INTEGER NOT NULL UNIQUE,
                code TEXT NOT NULL,
                path TEXT REFERENCES files(path) DEFERRABLE INITIALLY DEFERRED,
                symbol_id TEXT REFERENCES symbols(symbol_id) DEFERRABLE INITIALLY DEFERRED,
                reason TEXT NOT NULL,
                interpretation TEXT NOT NULL,
                canonical_json TEXT NOT NULL
            );
            CREATE INDEX edges_from_kind_id ON edges(from_symbol, kind, edge_id);
            CREATE INDEX edges_to_kind_id ON edges(to_symbol, kind, edge_id) WHERE to_symbol IS NOT NULL;
            CREATE INDEX edges_path_id ON edges(path, edge_id);
            CREATE INDEX symbols_path_id ON symbols(path, symbol_id);
            CREATE INDEX symbols_module_name ON symbols(module_id, name, symbol_id);",
        )
        .map_err(sqlite_error)
}

fn insert_files(
    transaction: &Transaction<'_>,
    graph: &RepositoryGraph,
    rows: &CanonicalGraphRows,
) -> Result<(), RepositoryGraphError> {
    let mut statement = transaction
        .prepare(
            "INSERT INTO files(
                path, mode, presence, content_sha256, file_fact_key_json,
                language, module_id, canonical_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .map_err(sqlite_error)?;
    for (file, canonical) in graph.files.iter().zip(&rows.files) {
        statement
            .execute(params![
                file.path.as_str(),
                file.mode,
                scalar_text(&file.presence)?,
                file.content_sha256,
                optional_json(&file.file_fact_key)?,
                file.language,
                file.module_id,
                canonical,
            ])
            .map_err(sqlite_error)?;
    }
    Ok(())
}

fn insert_modules(
    transaction: &Transaction<'_>,
    graph: &RepositoryGraph,
    rows: &CanonicalGraphRows,
) -> Result<(), RepositoryGraphError> {
    let mut statement = transaction
        .prepare(
            "INSERT INTO modules(
                module_id, parent_module_id, crate_name, path, inline,
                root_module, resolution_status, canonical_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .map_err(sqlite_error)?;
    for (module, canonical) in graph.modules.iter().zip(&rows.modules) {
        statement
            .execute(params![
                module.module_id,
                module.parent_module_id,
                module.crate_name,
                module.path.as_str(),
                module.inline,
                module.root_module,
                module.resolution_status,
                canonical,
            ])
            .map_err(sqlite_error)?;
    }
    Ok(())
}

fn insert_symbols(
    transaction: &Transaction<'_>,
    graph: &RepositoryGraph,
    rows: &CanonicalGraphRows,
) -> Result<(), RepositoryGraphError> {
    let mut statement = transaction
        .prepare(
            "INSERT INTO symbols(
                symbol_id, local_id, module_id, path, language, kind, name,
                owner_symbol_id, signature, visibility, start_line, start_column,
                end_line, end_column, start_byte, end_byte, confidence, canonical_json
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17, ?18
            )",
        )
        .map_err(sqlite_error)?;
    for (symbol, canonical) in graph.symbols.iter().zip(&rows.symbols) {
        statement
            .execute(params![
                symbol.symbol_id,
                symbol.local_id,
                symbol.module_id,
                symbol.path.as_str(),
                symbol.language,
                symbol.kind,
                symbol.name,
                symbol.owner_symbol_id,
                symbol.signature,
                symbol.visibility,
                sqlite_integer_u32(symbol.range.start_line),
                sqlite_integer_u32(symbol.range.start_column),
                sqlite_integer_u32(symbol.range.end_line),
                sqlite_integer_u32(symbol.range.end_column),
                sqlite_integer(symbol.range.start_byte, "symbol start byte")?,
                sqlite_integer(symbol.range.end_byte, "symbol end byte")?,
                scalar_text(&symbol.confidence)?,
                canonical,
            ])
            .map_err(sqlite_error)?;
    }
    Ok(())
}

fn insert_edges(
    transaction: &Transaction<'_>,
    graph: &RepositoryGraph,
    rows: &CanonicalGraphRows,
) -> Result<(), RepositoryGraphError> {
    let mut statement = transaction
        .prepare(
            "INSERT INTO edges(
                edge_id, kind, from_symbol, to_symbol, unresolved_target, path,
                start_line, start_column, end_line, end_column, start_byte, end_byte,
                provider_id, provider_version, resolution, confidence, limitation_code,
                canonical_json
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17, ?18
            )",
        )
        .map_err(sqlite_error)?;
    for (edge, canonical) in graph.edges.iter().zip(&rows.edges) {
        statement
            .execute(params![
                edge.edge_id,
                scalar_text(&edge.kind)?,
                edge.from_symbol,
                edge.to_symbol,
                edge.unresolved_target,
                edge.path.as_str(),
                sqlite_integer_u32(edge.range.start_line),
                sqlite_integer_u32(edge.range.start_column),
                sqlite_integer_u32(edge.range.end_line),
                sqlite_integer_u32(edge.range.end_column),
                sqlite_integer(edge.range.start_byte, "edge start byte")?,
                sqlite_integer(edge.range.end_byte, "edge end byte")?,
                edge.provider_id,
                edge.provider_version,
                scalar_text(&edge.resolution)?,
                scalar_text(&edge.confidence)?,
                edge.limitation_code,
                canonical,
            ])
            .map_err(sqlite_error)?;
    }
    Ok(())
}

fn insert_limitations(
    transaction: &Transaction<'_>,
    graph: &RepositoryGraph,
    rows: &CanonicalGraphRows,
) -> Result<(), RepositoryGraphError> {
    let mut statement = transaction
        .prepare(
            "INSERT INTO limitations(
                limitation_id, sort_order, code, path, symbol_id, reason, interpretation,
                canonical_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .map_err(sqlite_error)?;
    for (index, (limitation, canonical)) in
        graph.limitations.iter().zip(&rows.limitations).enumerate()
    {
        statement
            .execute(params![
                limitation_id(index, canonical),
                sqlite_integer(index, "limitation sort order")?,
                limitation.code,
                limitation.path.as_ref().map(|path| path.as_str()),
                limitation.symbol_id,
                limitation.reason,
                limitation.interpretation,
                canonical,
            ])
            .map_err(sqlite_error)?;
    }
    Ok(())
}

fn validate_reader_limits(limits: ReaderLimits) -> Result<(), RepositoryGraphError> {
    if limits.maximum_database_bytes == 0
        || limits.maximum_rows_per_query == 0
        || limits.maximum_string_bytes == 0
    {
        return Err(RepositoryGraphError::new(
            "reader-limits-invalid",
            "reader limits must be positive",
        ));
    }
    Ok(())
}

fn generation_key_from_path(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let key = name.strip_suffix(".sqlite")?;
    validate_hex(key).ok()?;
    Some(key.to_string())
}

fn open_immutable_connection(path: &Path) -> Result<Connection, RepositoryGraphError> {
    let text = path.to_str().ok_or_else(|| {
        RepositoryGraphError::new(
            "generation-path-not-utf8",
            "graph generation path is not UTF-8",
        )
    })?;
    let encoded = utf8_percent_encode(text, NON_ALPHANUMERIC);
    let uri = format!("file:{encoded}?mode=ro&immutable=1");
    let connection = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sqlite_error)?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(sqlite_error)?;
    connection
        .pragma_update(None, "trusted_schema", false)
        .map_err(sqlite_error)?;
    Ok(connection)
}

fn validate_reader_metadata(
    connection: &Connection,
    expected: &GraphGenerationIdentity,
    limits: ReaderLimits,
) -> Result<(GraphGenerationIdentity, Completeness, bool), ReaderValidationError> {
    let application_id: i32 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|_| ReaderValidationError::Corrupt("generation-header-invalid"))?;
    if application_id != APPLICATION_ID {
        return Err(ReaderValidationError::Corrupt(
            "generation-application-id-mismatch",
        ));
    }
    let user_version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| ReaderValidationError::Corrupt("generation-header-invalid"))?;
    if user_version != DATABASE_SCHEMA_VERSION {
        return Err(ReaderValidationError::Corrupt(
            "generation-schema-version-mismatch",
        ));
    }
    validate_reader_schema(connection)?;
    let metadata_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM generation_meta", [], |row| row.get(0))
        .map_err(|_| ReaderValidationError::Corrupt("generation-metadata-invalid"))?;
    if metadata_rows != 1 {
        return Err(ReaderValidationError::Corrupt(
            "generation-metadata-row-count-mismatch",
        ));
    }
    let meta: (i32, String, String, String, i64, i64, i64, i64, i64, String) = connection
        .query_row(
            "SELECT schema_version, generation_key, identity_json, completeness,
                    file_count, module_count, symbol_count, edge_count, limitation_count,
                    application_root
             FROM generation_meta",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                ))
            },
        )
        .map_err(|_| ReaderValidationError::Corrupt("generation-metadata-invalid"))?;
    if meta.0 != DATABASE_SCHEMA_VERSION {
        return Err(ReaderValidationError::Corrupt(
            "generation-schema-version-mismatch",
        ));
    }
    bounded_reader_text(&meta.1, limits.maximum_string_bytes)?;
    bounded_reader_text(&meta.2, limits.maximum_string_bytes.saturating_mul(16))?;
    bounded_reader_text(&meta.3, limits.maximum_string_bytes)?;
    bounded_reader_text(&meta.9, limits.maximum_string_bytes)?;
    validate_hex(&meta.1).map_err(|_| ReaderValidationError::Corrupt("generation-key-invalid"))?;
    validate_hex(&meta.9).map_err(|_| ReaderValidationError::Corrupt("generation-root-invalid"))?;
    let identity: GraphGenerationIdentity = serde_json::from_str(&meta.2)
        .map_err(|_| ReaderValidationError::Corrupt("generation-identity-invalid"))?;
    identity
        .validate()
        .map_err(|_| ReaderValidationError::Corrupt("generation-identity-invalid"))?;
    if &identity != expected {
        return Err(ReaderValidationError::Stale("generation-identity-stale"));
    }
    let expected_key = expected
        .generation_key()
        .map_err(|_| ReaderValidationError::Corrupt("generation-identity-invalid"))?;
    if meta.1 != expected_key {
        return Err(ReaderValidationError::Stale("generation-key-stale"));
    }
    let completeness: Completeness = serde_json::from_str(&meta.3)
        .map_err(|_| ReaderValidationError::Corrupt("generation-completeness-invalid"))?;
    for (table, stored) in [
        ("files", meta.4),
        ("modules", meta.5),
        ("symbols", meta.6),
        ("edges", meta.7),
        ("limitations", meta.8),
    ] {
        let stored = usize::try_from(stored)
            .map_err(|_| ReaderValidationError::Corrupt("generation-count-invalid"))?;
        let actual = reader_table_count(connection, table)?;
        if stored != actual {
            return Err(ReaderValidationError::Corrupt("generation-count-mismatch"));
        }
    }
    let query_only: i32 = connection
        .pragma_query_value(None, "query_only", |row| row.get(0))
        .map_err(|_| ReaderValidationError::Corrupt("generation-query-only-invalid"))?;
    if query_only != 1 {
        return Err(ReaderValidationError::Corrupt(
            "generation-query-only-invalid",
        ));
    }
    Ok((identity, completeness, true))
}

fn validate_reader_schema(connection: &Connection) -> Result<(), ReaderValidationError> {
    let tables = connection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<BTreeSet<_>, _>>()
        })
        .map_err(|_| ReaderValidationError::Corrupt("generation-schema-invalid"))?;
    let expected_tables = [
        "edges",
        "files",
        "generation_meta",
        "limitations",
        "modules",
        "symbols",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
    if tables != expected_tables {
        return Err(ReaderValidationError::Corrupt("generation-schema-invalid"));
    }
    let indexes = connection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'index' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<BTreeSet<_>, _>>()
        })
        .map_err(|_| ReaderValidationError::Corrupt("generation-index-invalid"))?;
    let expected_indexes = [
        "edges_from_kind_id",
        "edges_path_id",
        "edges_to_kind_id",
        "symbols_module_name",
        "symbols_path_id",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<BTreeSet<_>>();
    if indexes != expected_indexes {
        return Err(ReaderValidationError::Corrupt("generation-index-invalid"));
    }
    Ok(())
}

fn reader_table_count(
    connection: &Connection,
    table: &str,
) -> Result<usize, ReaderValidationError> {
    let sql = match table {
        "files" => "SELECT COUNT(*) FROM files",
        "modules" => "SELECT COUNT(*) FROM modules",
        "symbols" => "SELECT COUNT(*) FROM symbols",
        "edges" => "SELECT COUNT(*) FROM edges",
        "limitations" => "SELECT COUNT(*) FROM limitations",
        _ => return Err(ReaderValidationError::Corrupt("generation-table-invalid")),
    };
    let count: i64 = connection
        .query_row(sql, [], |row| row.get(0))
        .map_err(|_| ReaderValidationError::Corrupt("generation-count-invalid"))?;
    usize::try_from(count).map_err(|_| ReaderValidationError::Corrupt("generation-count-invalid"))
}

fn bounded_reader_text(value: &str, maximum: usize) -> Result<(), ReaderValidationError> {
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(ReaderValidationError::Corrupt("generation-string-invalid"));
    }
    Ok(())
}

fn decode_edge_row(
    row: &rusqlite::Row<'_>,
    limits: ReaderLimits,
) -> Result<GraphEdge, RepositoryGraphError> {
    let edge_id = row_text(row, 0, limits.maximum_string_bytes)?;
    let kind_text = row_text(row, 1, limits.maximum_string_bytes)?;
    let from_symbol = row_text(row, 2, limits.maximum_string_bytes)?;
    let to_symbol = optional_row_text(row, 3, limits.maximum_string_bytes)?;
    let unresolved_target = optional_row_text(row, 4, limits.maximum_string_bytes)?;
    let path_text = row_text(row, 5, limits.maximum_string_bytes)?;
    let range = SourceRange {
        start_line: row_u32(row, 6)?,
        start_column: row_u32(row, 7)?,
        end_line: row_u32(row, 8)?,
        end_column: row_u32(row, 9)?,
        start_byte: row_usize(row, 10)?,
        end_byte: row_usize(row, 11)?,
    };
    let provider_id = row_text(row, 12, limits.maximum_string_bytes)?;
    let provider_version = row_text(row, 13, limits.maximum_string_bytes)?;
    let resolution_text = row_text(row, 14, limits.maximum_string_bytes)?;
    let confidence_text = row_text(row, 15, limits.maximum_string_bytes)?;
    let limitation_code = optional_row_text(row, 16, limits.maximum_string_bytes)?;
    let canonical = row_text(row, 17, limits.maximum_string_bytes.saturating_mul(16))?;
    validate_hex(&edge_id).map_err(|_| row_corrupt())?;
    validate_hex(&from_symbol).map_err(|_| row_corrupt())?;
    if let Some(target) = &to_symbol {
        validate_hex(target).map_err(|_| row_corrupt())?;
    }
    if to_symbol.is_some() == unresolved_target.is_some() {
        return Err(row_corrupt());
    }
    validate_range(&range).map_err(|_| row_corrupt())?;
    let path = crate::candidate::RepoPath::new(path_text).map_err(|_| row_corrupt())?;
    let edge = GraphEdge {
        edge_id,
        kind: parse_edge_kind(&kind_text)?,
        from_symbol,
        to_symbol,
        unresolved_target,
        path,
        range,
        provider_id,
        provider_version,
        resolution: parse_resolution(&resolution_text)?,
        confidence: parse_confidence(&confidence_text)?,
        limitation_code,
    };
    let canonical_edge: GraphEdge = serde_json::from_str(&canonical).map_err(|_| row_corrupt())?;
    if canonical_edge != edge {
        return Err(row_corrupt());
    }
    Ok(edge)
}

fn row_text(
    row: &rusqlite::Row<'_>,
    index: usize,
    maximum: usize,
) -> Result<String, RepositoryGraphError> {
    let value: String = row.get(index).map_err(|_| row_corrupt())?;
    if value.is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(row_corrupt());
    }
    Ok(value)
}

fn optional_row_text(
    row: &rusqlite::Row<'_>,
    index: usize,
    maximum: usize,
) -> Result<Option<String>, RepositoryGraphError> {
    let value: Option<String> = row.get(index).map_err(|_| row_corrupt())?;
    if value.as_deref().is_some_and(|value| {
        value.is_empty() || value.len() > maximum || value.chars().any(char::is_control)
    }) {
        return Err(row_corrupt());
    }
    Ok(value)
}

fn row_u32(row: &rusqlite::Row<'_>, index: usize) -> Result<u32, RepositoryGraphError> {
    let value: i64 = row.get(index).map_err(|_| row_corrupt())?;
    u32::try_from(value).map_err(|_| row_corrupt())
}

fn row_usize(row: &rusqlite::Row<'_>, index: usize) -> Result<usize, RepositoryGraphError> {
    let value: i64 = row.get(index).map_err(|_| row_corrupt())?;
    usize::try_from(value).map_err(|_| row_corrupt())
}

fn parse_edge_kind(value: &str) -> Result<EdgeKind, RepositoryGraphError> {
    match value {
        "defines" => Ok(EdgeKind::Defines),
        "references" => Ok(EdgeKind::References),
        "imports" => Ok(EdgeKind::Imports),
        "exports" => Ok(EdgeKind::Exports),
        "calls" => Ok(EdgeKind::Calls),
        "implements" => Ok(EdgeKind::Implements),
        "overrides" => Ok(EdgeKind::Overrides),
        _ => Err(row_corrupt()),
    }
}

fn parse_resolution(value: &str) -> Result<Resolution, RepositoryGraphError> {
    match value {
        "syntactic" => Ok(Resolution::Syntactic),
        "lexical" => Ok(Resolution::Lexical),
        "resolved-reference" => Ok(Resolution::ResolvedReference),
        "semantic" => Ok(Resolution::Semantic),
        "polymorphic-candidate" => Ok(Resolution::PolymorphicCandidate),
        "unresolved" => Ok(Resolution::Unresolved),
        _ => Err(row_corrupt()),
    }
}

fn parse_confidence(value: &str) -> Result<Confidence, RepositoryGraphError> {
    match value {
        "high" => Ok(Confidence::High),
        "medium" => Ok(Confidence::Medium),
        "low" => Ok(Confidence::Low),
        _ => Err(row_corrupt()),
    }
}

fn row_corrupt() -> RepositoryGraphError {
    RepositoryGraphError::new(
        "generation-row-corrupt",
        "repository graph row violates the strict schema",
    )
}

fn reader_corrupt<T>(code: &str) -> CacheLookup<T> {
    CacheLookup::Corrupt {
        code: code.to_string(),
    }
}

fn validate_generation(
    path: &Path,
    graph: &RepositoryGraph,
    expected_rows: &CanonicalGraphRows,
) -> Result<(), RepositoryGraphError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        RepositoryGraphError::new(
            "generation-metadata-failed",
            format!("cannot inspect graph generation: {error}"),
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(RepositoryGraphError::new(
            "generation-not-regular",
            "graph generation is not a regular file",
        ));
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sqlite_error)?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(sqlite_error)?;
    connection
        .pragma_update(None, "trusted_schema", false)
        .map_err(sqlite_error)?;
    let application_id: i32 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(sqlite_error)?;
    if application_id != APPLICATION_ID {
        return Err(invalid_generation("generation-application-id-mismatch"));
    }
    let user_version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sqlite_error)?;
    if user_version != DATABASE_SCHEMA_VERSION {
        return Err(invalid_generation("generation-schema-version-mismatch"));
    }
    let generation_key = graph.identity.generation_key().map_err(|error| {
        RepositoryGraphError::new("generation-identity-invalid", error.to_string())
    })?;
    let meta: (i32, String, String, String, i64, i64, i64, i64, i64, String) = connection
        .query_row(
            "SELECT schema_version, generation_key, identity_json, completeness,
                    file_count, module_count, symbol_count, edge_count, limitation_count,
                    application_root
             FROM generation_meta",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                ))
            },
        )
        .map_err(sqlite_error)?;
    if meta.0 != DATABASE_SCHEMA_VERSION
        || meta.1 != generation_key
        || meta.2 != expected_rows.identity
        || meta.3 != expected_rows.completeness
    {
        return Err(invalid_generation("generation-metadata-mismatch"));
    }
    let counts = [
        ("files", meta.4, expected_rows.files.len()),
        ("modules", meta.5, expected_rows.modules.len()),
        ("symbols", meta.6, expected_rows.symbols.len()),
        ("edges", meta.7, expected_rows.edges.len()),
        ("limitations", meta.8, expected_rows.limitations.len()),
    ];
    for (table, stored, expected) in counts {
        if usize_from_sql(stored)? != expected || table_count(&connection, table)? != expected {
            return Err(invalid_generation("generation-count-mismatch"));
        }
    }
    let actual_rows = load_canonical_rows(&connection, &meta.2, &meta.3)?;
    if actual_rows != *expected_rows || graph_rows_root(&actual_rows) != meta.9 {
        return Err(invalid_generation("generation-application-root-mismatch"));
    }
    let mut foreign_keys = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(sqlite_error)?;
    if foreign_keys
        .query([])
        .map_err(sqlite_error)?
        .next()
        .map_err(sqlite_error)?
        .is_some()
    {
        return Err(invalid_generation("generation-foreign-key-mismatch"));
    }
    let integrity: String = connection
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .map_err(sqlite_error)?;
    if integrity != "ok" {
        return Err(invalid_generation("generation-integrity-check-failed"));
    }
    Ok(())
}

fn load_canonical_rows(
    connection: &Connection,
    identity: &str,
    completeness: &str,
) -> Result<CanonicalGraphRows, RepositoryGraphError> {
    Ok(CanonicalGraphRows {
        identity: identity.to_string(),
        completeness: completeness.to_string(),
        files: load_text_rows(connection, "SELECT canonical_json FROM files ORDER BY path")?,
        modules: load_text_rows(
            connection,
            "SELECT canonical_json FROM modules ORDER BY module_id",
        )?,
        symbols: load_text_rows(
            connection,
            "SELECT canonical_json FROM symbols ORDER BY symbol_id",
        )?,
        edges: load_text_rows(
            connection,
            "SELECT canonical_json FROM edges ORDER BY edge_id",
        )?,
        limitations: load_text_rows(
            connection,
            "SELECT canonical_json FROM limitations ORDER BY sort_order",
        )?,
    })
}

fn load_text_rows(
    connection: &Connection,
    sql: &'static str,
) -> Result<Vec<String>, RepositoryGraphError> {
    connection
        .prepare(sql)
        .map_err(sqlite_error)?
        .query_map([], |row| row.get(0))
        .map_err(sqlite_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sqlite_error)
}

fn table_count(connection: &Connection, table: &str) -> Result<usize, RepositoryGraphError> {
    let sql = match table {
        "files" => "SELECT COUNT(*) FROM files",
        "modules" => "SELECT COUNT(*) FROM modules",
        "symbols" => "SELECT COUNT(*) FROM symbols",
        "edges" => "SELECT COUNT(*) FROM edges",
        "limitations" => "SELECT COUNT(*) FROM limitations",
        _ => return Err(invalid_generation("generation-table-invalid")),
    };
    let count: i64 = connection
        .query_row(sql, [], |row| row.get(0))
        .map_err(sqlite_error)?;
    usize_from_sql(count)
}

fn validate_graph(graph: &RepositoryGraph) -> Result<(), RepositoryGraphError> {
    graph.identity.validate().map_err(|error| {
        RepositoryGraphError::new(
            "generation-identity-invalid",
            format!("invalid graph identity: {error}"),
        )
    })?;
    if graph.completeness == Completeness::Unavailable {
        return Err(RepositoryGraphError::new(
            "generation-unavailable-not-persistable",
            "unavailable repository graphs cannot be persisted",
        ));
    }
    if graph.completeness == Completeness::Complete && !graph.limitations.is_empty() {
        return Err(RepositoryGraphError::new(
            "complete-generation-has-limitations",
            "complete repository graphs cannot contain limitations",
        ));
    }
    if graph.completeness == Completeness::Partial
        && !graph
            .limitations
            .iter()
            .any(|limitation| limitation.path.is_some() || limitation.symbol_id.is_some())
    {
        return Err(RepositoryGraphError::new(
            "partial-generation-omissions-required",
            "partial repository graphs require path- or symbol-scoped omissions",
        ));
    }
    validate_sorted(
        graph.files.iter().map(|file| file.path.as_str()),
        "file paths",
    )?;
    validate_sorted(
        graph.modules.iter().map(|module| module.module_id.as_str()),
        "module ids",
    )?;
    validate_sorted(
        graph.symbols.iter().map(|symbol| symbol.symbol_id.as_str()),
        "symbol ids",
    )?;
    validate_sorted(
        graph.edges.iter().map(|edge| edge.edge_id.as_str()),
        "edge ids",
    )?;

    let files = graph
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    let modules = graph
        .modules
        .iter()
        .map(|module| module.module_id.as_str())
        .collect::<BTreeSet<_>>();
    let symbols = graph
        .symbols
        .iter()
        .map(|symbol| symbol.symbol_id.as_str())
        .collect::<BTreeSet<_>>();
    for file in &graph.files {
        if file.mode.len() != 6 || !file.mode.bytes().all(|byte| matches!(byte, b'0'..=b'7')) {
            return Err(invalid_graph("generation-file-mode-invalid"));
        }
        match file.presence {
            CandidatePresence::Present => {
                let Some(content_sha256) = &file.content_sha256 else {
                    return Err(invalid_graph("generation-file-content-missing"));
                };
                validate_hex(content_sha256)?;
                if let Some(key) = &file.file_fact_key {
                    key.validate().map_err(|error| {
                        RepositoryGraphError::new(
                            "generation-file-fact-key-invalid",
                            error.to_string(),
                        )
                    })?;
                    if key.content_sha256 != *content_sha256 {
                        return Err(invalid_graph("generation-file-fact-key-mismatch"));
                    }
                }
            }
            CandidatePresence::Deleted | CandidatePresence::Gitlink => {
                if file.content_sha256.is_some() || file.file_fact_key.is_some() {
                    return Err(invalid_graph("generation-non-file-content-invalid"));
                }
            }
        }
        if file
            .module_id
            .as_deref()
            .is_some_and(|module| !modules.contains(module))
        {
            return Err(invalid_graph("generation-file-module-missing"));
        }
    }
    for module in &graph.modules {
        validate_hex(&module.module_id)?;
        if !files.contains(module.path.as_str()) {
            return Err(invalid_graph("generation-module-file-missing"));
        }
        if module
            .parent_module_id
            .as_deref()
            .is_some_and(|parent| !modules.contains(parent))
        {
            return Err(invalid_graph("generation-parent-module-missing"));
        }
    }
    for symbol in &graph.symbols {
        validate_hex(&symbol.symbol_id)?;
        if !files.contains(symbol.path.as_str()) || !modules.contains(symbol.module_id.as_str()) {
            return Err(invalid_graph("generation-symbol-owner-missing"));
        }
        if symbol
            .owner_symbol_id
            .as_deref()
            .is_some_and(|owner| !symbols.contains(owner))
        {
            return Err(invalid_graph("generation-owner-symbol-missing"));
        }
        validate_range(&symbol.range)?;
    }
    for edge in &graph.edges {
        validate_hex(&edge.edge_id)?;
        if !symbols.contains(edge.from_symbol.as_str())
            || edge
                .to_symbol
                .as_deref()
                .is_some_and(|target| !symbols.contains(target))
            || !files.contains(edge.path.as_str())
        {
            return Err(invalid_graph("generation-edge-owner-missing"));
        }
        if edge.to_symbol.is_some() == edge.unresolved_target.is_some() {
            return Err(invalid_graph("generation-edge-target-invalid"));
        }
        validate_range(&edge.range)?;
    }
    validate_limitations(&graph.limitations, &files, &symbols)
}

fn validate_limitations(
    limitations: &[IndexLimitation],
    files: &BTreeSet<&str>,
    symbols: &BTreeSet<&str>,
) -> Result<(), RepositoryGraphError> {
    let mut previous: Option<(&str, &str, &str, &str, &str)> = None;
    for limitation in limitations {
        let key = (
            limitation.code.as_str(),
            limitation
                .path
                .as_ref()
                .map(|path| path.as_str())
                .unwrap_or(""),
            limitation.symbol_id.as_deref().unwrap_or(""),
            limitation.reason.as_str(),
            limitation.interpretation.as_str(),
        );
        if previous.is_some_and(|previous| previous >= key) {
            return Err(invalid_graph("generation-limitations-unsorted"));
        }
        previous = Some(key);
        if limitation
            .path
            .as_ref()
            .is_some_and(|path| !files.contains(path.as_str()))
            || limitation
                .symbol_id
                .as_deref()
                .is_some_and(|symbol| !symbols.contains(symbol))
        {
            return Err(invalid_graph("generation-limitation-owner-missing"));
        }
    }
    Ok(())
}

fn validate_sorted<'a>(
    values: impl Iterator<Item = &'a str>,
    field: &str,
) -> Result<(), RepositoryGraphError> {
    let mut previous = None;
    for value in values {
        if previous.is_some_and(|previous_value| previous_value >= value) {
            return Err(RepositoryGraphError::new(
                "generation-order-invalid",
                format!("{field} must be sorted and unique"),
            ));
        }
        previous = Some(value);
    }
    Ok(())
}

fn validate_hex(value: &str) -> Result<(), RepositoryGraphError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_graph("generation-digest-invalid"));
    }
    Ok(())
}

fn validate_range(range: &SourceRange) -> Result<(), RepositoryGraphError> {
    if range.start_line == 0
        || range.start_column == 0
        || range.end_line == 0
        || range.end_column == 0
        || range.start_line > range.end_line
        || (range.start_line == range.end_line && range.start_column > range.end_column)
        || range.start_byte > range.end_byte
    {
        return Err(invalid_graph("generation-range-invalid"));
    }
    Ok(())
}

fn scalar_text<T: Serialize>(value: &T) -> Result<String, RepositoryGraphError> {
    serde_json::to_value(value)
        .map_err(|error| {
            RepositoryGraphError::new(
                "generation-value-encode-failed",
                format!("cannot encode graph value: {error}"),
            )
        })?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| invalid_graph("generation-value-not-string"))
}

fn optional_json<T: Serialize>(value: &Option<T>) -> Result<Option<String>, RepositoryGraphError> {
    value
        .as_ref()
        .map(|value| {
            serde_json::to_string(value).map_err(|error| {
                RepositoryGraphError::new(
                    "generation-value-encode-failed",
                    format!("cannot encode graph value: {error}"),
                )
            })
        })
        .transpose()
}

fn limitation_id(index: usize, canonical: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"repository-graph-limitation/v1");
    digest.update((index as u64).to_be_bytes());
    digest.update((canonical.len() as u64).to_be_bytes());
    digest.update(canonical.as_bytes());
    format!("{:x}", digest.finalize())
}

fn sqlite_integer(value: usize, field: &str) -> Result<i64, RepositoryGraphError> {
    i64::try_from(value).map_err(|_| {
        RepositoryGraphError::new(
            "generation-integer-overflow",
            format!("{field} exceeds SQLite integer range"),
        )
    })
}

fn sqlite_integer_u32(value: u32) -> i64 {
    i64::from(value)
}

fn usize_from_sql(value: i64) -> Result<usize, RepositoryGraphError> {
    usize::try_from(value).map_err(|_| invalid_generation("generation-count-invalid"))
}

fn budget_error(
    exhaustion: crate::impact_context::index::budget::IndexBudgetExhaustion,
) -> RepositoryGraphError {
    RepositoryGraphError::new(exhaustion.code(), exhaustion.code())
}

fn sqlite_error(error: rusqlite::Error) -> RepositoryGraphError {
    if matches!(
        &error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::DiskFull
    ) {
        return RepositoryGraphError::new(
            "index-generation-byte-budget-exhausted",
            "SQLite graph generation exceeded the configured page budget",
        );
    }
    RepositoryGraphError::new(
        "generation-sqlite-error",
        format!("SQLite graph generation failed: {error}"),
    )
}

fn invalid_graph(code: &'static str) -> RepositoryGraphError {
    RepositoryGraphError::new(code, code)
}

fn invalid_generation(code: &'static str) -> RepositoryGraphError {
    RepositoryGraphError::new(code, code)
}
