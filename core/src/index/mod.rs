//! SQLite FTS5 Search and Metadata Index Store (`EPIC-SEARCH-01`, `EPIC-DATA-01`).
//!
//! Defines the [`IndexStore`] abstraction and [`SqliteFtsIndexStore`] implementation using
//! direct C ABI bindings to system `libsqlite3` (zero external Rust crate dependencies)
//! with WAL mode enabled for concurrent read/write performance.

use crate::domain::{
    IndexedDocEntry, JsonValue, KnowledgeEntity, OmkError, SearchFilters, SearchResultSnippet,
    SearchTimings,
};
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int, c_void};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

const MAX_READER_POOL_SIZE: usize = 8;

/// Storage abstraction for full-text search and metadata indexing (`docs/architecture.md` §5.2).
pub trait IndexStore: Send + Sync {
    /// Inserts or updates a [`KnowledgeEntity`] in the search index.
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if the database transaction fails.
    fn upsert_index(&self, entity: &KnowledgeEntity) -> Result<(), OmkError>;

    /// Inserts or updates a [`KnowledgeEntity`] alongside its OS file metadata (`file_mtime_ns`, `file_size`)
    /// for incremental reconciliation (`REQ-PERF-002`, `MET-STATIC-04`).
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if the database transaction fails.
    fn upsert_index_with_meta(
        &self,
        entity: &KnowledgeEntity,
        _file_mtime_ns: i64,
        _file_size: i64,
    ) -> Result<(), OmkError> {
        self.upsert_index(entity)
    }

    /// Removes a document from the search index by ID.
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if the database operation fails.
    fn delete_index(&self, id: &str) -> Result<(), OmkError>;

    /// Atomically applies a batch of index upserts `(entity, file_mtime_ns, file_size)` and orphan deletions
    /// inside a single transaction (`REQ-PERF-002`, `MET-STATIC-05`).
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if the batch transaction fails.
    fn batch_reconcile(
        &self,
        upserts: &[(KnowledgeEntity, i64, i64)],
        delete_ids: &[String],
    ) -> Result<(), OmkError> {
        for (entity, mtime_ns, size) in upserts {
            self.upsert_index_with_meta(entity, *mtime_ns, *size)?;
        }
        for id in delete_ids {
            self.delete_index(id)?;
        }
        Ok(())
    }

    /// Searches indexed knowledge using an optional FTS5 query and metadata filters (`EPIC-SEARCH-01`).
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] or [`OmkError::InvalidParams`] if the query fails.
    fn search(
        &self,
        query: Option<&str>,
        filters: &SearchFilters,
        limit: u32,
    ) -> Result<Vec<SearchResultSnippet>, OmkError>;

    /// Searches indexed knowledge and returns both matched snippets and microsecond-level
    /// [`SearchTimings`] (`REQ-OBS-001`).
    ///
    /// # Errors
    /// Returns the same errors as [`IndexStore::search`].
    fn search_timed(
        &self,
        query: Option<&str>,
        filters: &SearchFilters,
        limit: u32,
    ) -> Result<(Vec<SearchResultSnippet>, SearchTimings), OmkError> {
        let start = Instant::now();
        let hits = self.search(query, filters, limit)?;
        let elapsed_us = start.elapsed().as_micros() as u64;
        Ok((
            hits,
            SearchTimings {
                fts_build_us: 0,
                db_open_us: 0,
                fts_query_us: elapsed_us,
                filter_post_us: 0,
                total_us: elapsed_us,
            },
        ))
    }

    /// Searches indexed knowledge with 2-stage `AND` -> `OR` BM25 fallback, returning
    /// `(matches, timings, fallback_used)` (`REQ-PERF-004`, `MET-STATIC-08`).
    ///
    /// # Errors
    /// Returns the same errors as [`IndexStore::search`].
    fn search_timed_with_fallback(
        &self,
        query: Option<&str>,
        filters: &SearchFilters,
        limit: u32,
    ) -> Result<(Vec<SearchResultSnippet>, SearchTimings, bool), OmkError> {
        let (hits, timings) = self.search_timed(query, filters, limit)?;
        Ok((hits, timings, false))
    }

    /// Returns a map of `id -> version_hash` for all indexed documents (used for startup reconciliation/`fsck`).
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if querying the index fails.
    fn list_index_hashes(&self) -> Result<BTreeMap<String, String>, OmkError>;

    /// Returns a map of `id -> IndexedDocEntry` (`version_hash`, `file_mtime_ns`, `file_size`)
    /// for incremental reconciliation (`REQ-PERF-002`, `MET-STATIC-04`).
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if querying the index fails.
    fn list_index_entries(&self) -> Result<BTreeMap<String, IndexedDocEntry>, OmkError> {
        let hashes = self.list_index_hashes()?;
        Ok(hashes
            .into_iter()
            .map(|(id, version_hash)| {
                (
                    id.clone(),
                    IndexedDocEntry {
                        id,
                        version_hash,
                        file_mtime_ns: 0,
                        file_size: 0,
                    },
                )
            })
            .collect())
    }
}

// ============================================================================
// Minimal, safe wrapper around system libsqlite3 C ABI
// ============================================================================

#[repr(C)]
struct Sqlite3Db {
    _private: [u8; 0],
}

#[repr(C)]
struct Sqlite3Stmt {
    _private: [u8; 0],
}

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;

type SqliteDestructor = Option<unsafe extern "C" fn(*mut c_void)>;

fn sqlite_transient() -> SqliteDestructor {
    unsafe { std::mem::transmute::<isize, SqliteDestructor>(-1) }
}

#[link(name = "sqlite3")]
extern "C" {
    fn sqlite3_open(filename: *const c_char, pp_db: *mut *mut Sqlite3Db) -> c_int;
    fn sqlite3_close(db: *mut Sqlite3Db) -> c_int;
    fn sqlite3_errmsg(db: *mut Sqlite3Db) -> *const c_char;
    fn sqlite3_exec(
        db: *mut Sqlite3Db,
        sql: *const c_char,
        callback: Option<
            unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int,
        >,
        arg: *mut c_void,
        errmsg: *mut *mut c_char,
    ) -> c_int;
    fn sqlite3_free(ptr: *mut c_void);
    fn sqlite3_prepare_v2(
        db: *mut Sqlite3Db,
        z_sql: *const c_char,
        n_byte: c_int,
        pp_stmt: *mut *mut Sqlite3Stmt,
        pz_tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_step(stmt: *mut Sqlite3Stmt) -> c_int;
    fn sqlite3_reset(stmt: *mut Sqlite3Stmt) -> c_int;
    fn sqlite3_clear_bindings(stmt: *mut Sqlite3Stmt) -> c_int;
    fn sqlite3_finalize(stmt: *mut Sqlite3Stmt) -> c_int;
    fn sqlite3_bind_text(
        stmt: *mut Sqlite3Stmt,
        idx: c_int,
        val: *const c_char,
        n_byte: c_int,
        destructor: SqliteDestructor,
    ) -> c_int;
    fn sqlite3_bind_double(stmt: *mut Sqlite3Stmt, idx: c_int, val: c_double) -> c_int;
    fn sqlite3_bind_int64(stmt: *mut Sqlite3Stmt, idx: c_int, val: i64) -> c_int;
    fn sqlite3_column_text(stmt: *mut Sqlite3Stmt, i_col: c_int) -> *const u8;
    fn sqlite3_column_double(stmt: *mut Sqlite3Stmt, i_col: c_int) -> c_double;
    fn sqlite3_column_int64(stmt: *mut Sqlite3Stmt, i_col: c_int) -> i64;
}

struct SqliteConnection {
    db: *mut Sqlite3Db,
}

unsafe impl Send for SqliteConnection {}

impl SqliteConnection {
    fn open(path: &Path) -> Result<Self, OmkError> {
        let path_str = path.to_string_lossy();
        let c_path = CString::new(path_str.as_ref()).map_err(|_| {
            OmkError::InternalError("Database path contains interior NUL byte".to_string())
        })?;
        let mut db: *mut Sqlite3Db = ptr::null_mut();
        let rc = unsafe { sqlite3_open(c_path.as_ptr(), &mut db) };
        if rc != SQLITE_OK || db.is_null() {
            let msg = Self::extract_error_static(db);
            if !db.is_null() {
                unsafe {
                    sqlite3_close(db);
                }
            }
            return Err(OmkError::InternalError(format!(
                "Failed to open SQLite database '{}': {}",
                path.display(),
                msg
            )));
        }
        Ok(Self { db })
    }

    fn extract_error_static(db: *mut Sqlite3Db) -> String {
        if db.is_null() {
            return "null sqlite3 handle".to_string();
        }
        unsafe {
            let ptr = sqlite3_errmsg(db);
            if ptr.is_null() {
                "unknown sqlite3 error".to_string()
            } else {
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        }
    }

    fn last_error(&self) -> String {
        Self::extract_error_static(self.db)
    }

    fn exec(&self, sql: &str) -> Result<(), OmkError> {
        let c_sql = CString::new(sql)
            .map_err(|_| OmkError::InternalError("SQL contains NUL byte".to_string()))?;
        let mut errmsg: *mut c_char = ptr::null_mut();
        let rc = unsafe { sqlite3_exec(self.db, c_sql.as_ptr(), None, ptr::null_mut(), &mut errmsg) };
        if rc != SQLITE_OK {
            let msg = if !errmsg.is_null() {
                let s = unsafe { CStr::from_ptr(errmsg).to_string_lossy().into_owned() };
                unsafe {
                    sqlite3_free(errmsg as *mut c_void);
                }
                s
            } else {
                self.last_error()
            };
            return Err(OmkError::InternalError(format!("SQLite exec error: {}", msg)));
        }
        Ok(())
    }

    fn prepare(&self, sql: &str) -> Result<SqliteStatement<'_>, OmkError> {
        let c_sql = CString::new(sql)
            .map_err(|_| OmkError::InternalError("SQL contains NUL byte".to_string()))?;
        let mut stmt: *mut Sqlite3Stmt = ptr::null_mut();
        let rc = unsafe {
            sqlite3_prepare_v2(
                self.db,
                c_sql.as_ptr(),
                c_sql.as_bytes().len() as c_int,
                &mut stmt,
                ptr::null_mut(),
            )
        };
        if rc != SQLITE_OK || stmt.is_null() {
            return Err(OmkError::InternalError(format!(
                "SQLite prepare error: {}",
                self.last_error()
            )));
        }
        Ok(SqliteStatement { conn: self, stmt })
    }
}

impl Drop for SqliteConnection {
    fn drop(&mut self) {
        if !self.db.is_null() {
            unsafe {
                sqlite3_close(self.db);
            }
        }
    }
}

struct SqliteStatement<'a> {
    conn: &'a SqliteConnection,
    stmt: *mut Sqlite3Stmt,
}

impl<'a> SqliteStatement<'a> {
    fn bind_text(&self, idx: c_int, val: &str) -> Result<(), OmkError> {
        let rc = unsafe {
            sqlite3_bind_text(
                self.stmt,
                idx,
                val.as_ptr() as *const c_char,
                val.len() as c_int,
                sqlite_transient(),
            )
        };
        if rc != SQLITE_OK {
            return Err(OmkError::InternalError(format!(
                "SQLite bind_text failed: {}",
                self.conn.last_error()
            )));
        }
        Ok(())
    }

    fn bind_f64(&self, idx: c_int, val: f64) -> Result<(), OmkError> {
        let rc = unsafe { sqlite3_bind_double(self.stmt, idx, val) };
        if rc != SQLITE_OK {
            return Err(OmkError::InternalError(format!(
                "SQLite bind_double failed: {}",
                self.conn.last_error()
            )));
        }
        Ok(())
    }

    fn bind_i64(&self, idx: c_int, val: i64) -> Result<(), OmkError> {
        let rc = unsafe { sqlite3_bind_int64(self.stmt, idx, val) };
        if rc != SQLITE_OK {
            return Err(OmkError::InternalError(format!(
                "SQLite bind_int64 failed: {}",
                self.conn.last_error()
            )));
        }
        Ok(())
    }

    fn step(&self) -> Result<bool, OmkError> {
        let rc = unsafe { sqlite3_step(self.stmt) };
        match rc {
            SQLITE_ROW => Ok(true),
            SQLITE_DONE => Ok(false),
            _ => Err(OmkError::InternalError(format!(
                "SQLite step failed: {}",
                self.conn.last_error()
            ))),
        }
    }

    fn reset(&self) {
        unsafe {
            let _ = sqlite3_reset(self.stmt);
            let _ = sqlite3_clear_bindings(self.stmt);
        }
    }

    fn column_text(&self, col: c_int) -> String {
        unsafe {
            let ptr = sqlite3_column_text(self.stmt, col);
            if ptr.is_null() {
                String::new()
            } else {
                CStr::from_ptr(ptr as *const c_char)
                    .to_string_lossy()
                    .into_owned()
            }
        }
    }

    fn column_f64(&self, col: c_int) -> f64 {
        unsafe { sqlite3_column_double(self.stmt, col) }
    }

    fn column_i64(&self, col: c_int) -> i64 {
        unsafe { sqlite3_column_int64(self.stmt, col) }
    }
}

impl<'a> Drop for SqliteStatement<'a> {
    fn drop(&mut self) {
        if !self.stmt.is_null() {
            unsafe {
                sqlite3_finalize(self.stmt);
            }
        }
    }
}

struct PooledReader<'a> {
    pool: &'a Mutex<Vec<SqliteConnection>>,
    conn: Option<SqliteConnection>,
}

impl<'a> PooledReader<'a> {
    fn conn(&self) -> &SqliteConnection {
        self.conn.as_ref().expect("PooledReader connection active")
    }
}

impl<'a> Drop for PooledReader<'a> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            if let Ok(mut guard) = self.pool.lock() {
                if guard.len() < MAX_READER_POOL_SIZE {
                    guard.push(conn);
                }
            }
        }
    }
}

/// SQLite + FTS5 implementation of [`IndexStore`] (`EPIC-DATA-01` §3.2, `EPIC-SEARCH-01`, `EPIC-PERF-01`).
pub struct SqliteFtsIndexStore {
    db_path: PathBuf,
    writer: Mutex<SqliteConnection>,
    reader_pool: Mutex<Vec<SqliteConnection>>,
    open_counter: AtomicUsize,
    tx_commit_counter: AtomicUsize,
}

impl SqliteFtsIndexStore {
    /// Opens or initializes the SQLite FTS5 index database at `db_path` with WAL mode,
    /// incremental metadata columns, and filter pushdown indexes enabled (`REQ-PERF-002`, `REQ-PERF-003`).
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if schema initialization or WAL configuration fails.
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, OmkError> {
        let path = db_path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                OmkError::InternalError(format!(
                    "Failed to create directory for SQLite DB '{}': {}",
                    path.display(),
                    e
                ))
            })?;
        }
        let conn = SqliteConnection::open(&path)?;
        conn.exec(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS knowledge_index (
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL,
                 source_class TEXT NOT NULL,
                 source_type TEXT NOT NULL,
                 authority REAL NOT NULL,
                 created_at INTEGER NOT NULL,
                 metadata TEXT NOT NULL,
                 version_hash TEXT NOT NULL DEFAULT '',
                 file_mtime_ns INTEGER NOT NULL DEFAULT 0,
                 file_size INTEGER NOT NULL DEFAULT 0,
                 tags_norm TEXT NOT NULL DEFAULT '|'
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_fts USING fts5(
                 id UNINDEXED,
                 title,
                 content,
                 tags
             );",
        )?;

        // Idempotent column migrations for databases created prior to EPIC-PERF-01
        let _ = conn.exec(
            "ALTER TABLE knowledge_index ADD COLUMN version_hash TEXT NOT NULL DEFAULT '';",
        );
        let _ = conn.exec(
            "ALTER TABLE knowledge_index ADD COLUMN file_mtime_ns INTEGER NOT NULL DEFAULT 0;",
        );
        let _ = conn
            .exec("ALTER TABLE knowledge_index ADD COLUMN file_size INTEGER NOT NULL DEFAULT 0;");
        let _ = conn.exec(
            "ALTER TABLE knowledge_index ADD COLUMN tags_norm TEXT NOT NULL DEFAULT '|';",
        );
        conn.exec(
            "CREATE INDEX IF NOT EXISTS idx_knowledge_filter_sort
             ON knowledge_index(source_class, source_type, authority DESC, created_at DESC);",
        )?;

        Ok(Self {
            db_path: path,
            writer: Mutex::new(conn),
            reader_pool: Mutex::new(Vec::with_capacity(MAX_READER_POOL_SIZE)),
            open_counter: AtomicUsize::new(0),
            tx_commit_counter: AtomicUsize::new(0),
        })
    }

    /// Returns the cumulative number of `sqlite3_open` calls made by reader queries (`REQ-PERF-006`, `MET-STATIC-06`).
    pub fn open_count(&self) -> usize {
        self.open_counter.load(Ordering::SeqCst)
    }

    /// Returns the cumulative number of SQLite write transaction commits (`REQ-PERF-006`, `MET-STATIC-05`).
    pub fn tx_commit_count(&self) -> usize {
        self.tx_commit_counter.load(Ordering::SeqCst)
    }

    /// Resets the static instrumentation counters (`open_count` and `tx_commit_count`) to zero
    /// and clears cached reader connections so subsequent queries are measured from a clean state (`REQ-PERF-006`).
    pub fn reset_counters(&self) {
        if let Ok(mut guard) = self.reader_pool.lock() {
            guard.clear();
        }
        self.open_counter.store(0, Ordering::SeqCst);
        self.tx_commit_counter.store(0, Ordering::SeqCst);
    }

    /// Acquires a pooled read-only WAL connection, opening a new connection only on pool miss (`REQ-PERF-003`, `MET-STATIC-06`).
    fn acquire_reader(&self) -> Result<PooledReader<'_>, OmkError> {
        if let Ok(mut guard) = self.reader_pool.lock() {
            if let Some(conn) = guard.pop() {
                return Ok(PooledReader {
                    pool: &self.reader_pool,
                    conn: Some(conn),
                });
            }
        }
        let conn = SqliteConnection::open(&self.db_path)?;
        conn.exec("PRAGMA busy_timeout=5000; PRAGMA query_only=ON;")?;
        self.open_counter.fetch_add(1, Ordering::SeqCst);
        Ok(PooledReader {
            pool: &self.reader_pool,
            conn: Some(conn),
        })
    }

    /// Formats a slice of tags into a canonical pipe-delimited lowercase string (`|tag1|tag2|`)
    /// for SQL `INSTR(idx.tags_norm, '|tag|') > 0` pushdown filtering (`REQ-PERF-003`, `MET-STATIC-07`).
    pub fn normalize_tags_column(tags: &[String]) -> String {
        if tags.is_empty() {
            return "|".to_string();
        }
        let mut out = String::from("|");
        for t in tags {
            let clean = t.trim().to_ascii_lowercase();
            if !clean.is_empty() {
                out.push_str(&clean);
                out.push('|');
            }
        }
        out
    }

    /// Normalizes a user/agent search string into a safe FTS5 `MATCH` expression supporting `AND` / `OR` (`REQ-SEARCH-001`).
    pub fn build_fts_query(raw: &str) -> Option<String> {
        let tokens: Vec<&str> = raw.split_whitespace().collect();
        if tokens.is_empty() {
            return None;
        }
        let mut out_parts: Vec<String> = Vec::new();
        let mut prev_was_op = true;
        for tok in tokens {
            let upper = tok.to_ascii_uppercase();
            if (upper == "AND" || upper == "OR") && !prev_was_op {
                out_parts.push(upper);
                prev_was_op = true;
                continue;
            }
            let cleaned: String = tok
                .chars()
                .filter(|c| !matches!(c, '"' | '\'' | '(' | ')' | '*' | ':' | '^' | '-'))
                .collect();
            if cleaned.is_empty() {
                continue;
            }
            if !prev_was_op && !out_parts.is_empty() {
                let last = out_parts.last().map(String::as_str).unwrap_or("");
                if last != "AND" && last != "OR" {
                    out_parts.push("AND".to_string());
                }
            }
            out_parts.push(format!("\"{}\"", cleaned));
            prev_was_op = false;
        }
        if prev_was_op && matches!(out_parts.last().map(String::as_str), Some("AND" | "OR")) {
            out_parts.pop();
        }
        if out_parts.is_empty() {
            None
        } else {
            Some(out_parts.join(" "))
        }
    }

    /// Builds a 2nd-stage `OR` FTS5 query when the input has $\ge 2$ keywords and no explicit
    /// `AND` / `OR` operators (`REQ-PERF-004`, `MET-STATIC-08`).
    pub fn build_fts_or_fallback_query(raw: &str) -> Option<String> {
        let mut cleaned_tokens = Vec::new();
        for tok in raw.split_whitespace() {
            if tok.eq_ignore_ascii_case("AND") || tok.eq_ignore_ascii_case("OR") {
                return None;
            }
            let cleaned: String = tok
                .chars()
                .filter(|c| !matches!(c, '"' | '\'' | '(' | ')' | '*' | ':' | '^' | '-'))
                .collect();
            if !cleaned.is_empty() {
                cleaned_tokens.push(format!("\"{}\"", cleaned));
            }
        }
        if cleaned_tokens.len() >= 2 {
            Some(cleaned_tokens.join(" OR "))
        } else {
            None
        }
    }
}

impl IndexStore for SqliteFtsIndexStore {
    fn upsert_index(&self, entity: &KnowledgeEntity) -> Result<(), OmkError> {
        self.upsert_index_with_meta(entity, 0, 0)
    }

    fn upsert_index_with_meta(
        &self,
        entity: &KnowledgeEntity,
        file_mtime_ns: i64,
        file_size: i64,
    ) -> Result<(), OmkError> {
        let guard = self
            .writer
            .lock()
            .map_err(|_| OmkError::InternalError("Index writer mutex poisoned".to_string()))?;

        // REQ-PERF-005: Check if an entry with identical version_hash already exists to avoid redundant FTS5 re-tokenization
        if !entity.sync.version_hash.is_empty() {
            let chk = guard.prepare(
                "SELECT version_hash, file_mtime_ns, file_size FROM knowledge_index WHERE id = ?1;",
            )?;
            chk.bind_text(1, &entity.id)?;
            if chk.step()? {
                let existing_hash = chk.column_text(0);
                let existing_mtime = chk.column_i64(1);
                let existing_size = chk.column_i64(2);
                if existing_hash == entity.sync.version_hash {
                    if file_mtime_ns > 0
                        && (existing_mtime != file_mtime_ns || existing_size != file_size)
                    {
                        let upd = guard.prepare(
                            "UPDATE knowledge_index SET file_mtime_ns = ?2, file_size = ?3 WHERE id = ?1;",
                        )?;
                        upd.bind_text(1, &entity.id)?;
                        upd.bind_i64(2, file_mtime_ns)?;
                        upd.bind_i64(3, file_size)?;
                        upd.step()?;
                    }
                    return Ok(());
                }
            }
        }

        guard.exec("BEGIN IMMEDIATE TRANSACTION;")?;
        let result = (|| -> Result<(), OmkError> {
            let tags_norm = Self::normalize_tags_column(&entity.tags);
            {
                let stmt = guard.prepare(
                    "INSERT INTO knowledge_index (
                         id, title, source_class, source_type, authority, created_at,
                         metadata, version_hash, file_mtime_ns, file_size, tags_norm
                     )
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT(id) DO UPDATE SET
                         title = excluded.title,
                         source_class = excluded.source_class,
                         source_type = excluded.source_type,
                         authority = excluded.authority,
                         created_at = excluded.created_at,
                         metadata = excluded.metadata,
                         version_hash = excluded.version_hash,
                         file_mtime_ns = excluded.file_mtime_ns,
                         file_size = excluded.file_size,
                         tags_norm = excluded.tags_norm;",
                )?;
                stmt.bind_text(1, &entity.id)?;
                stmt.bind_text(2, &entity.title)?;
                stmt.bind_text(3, entity.source.class.as_str())?;
                stmt.bind_text(4, entity.source.source_type.as_str())?;
                stmt.bind_f64(5, entity.authority)?;
                stmt.bind_i64(6, entity.created_at)?;
                stmt.bind_text(7, &entity.metadata_json())?;
                stmt.bind_text(8, &entity.sync.version_hash)?;
                stmt.bind_i64(9, file_mtime_ns)?;
                stmt.bind_i64(10, file_size)?;
                stmt.bind_text(11, &tags_norm)?;
                stmt.step()?;
            }
            {
                let del_fts = guard.prepare("DELETE FROM knowledge_fts WHERE id = ?1;")?;
                del_fts.bind_text(1, &entity.id)?;
                del_fts.step()?;
            }
            {
                let ins_fts = guard.prepare(
                    "INSERT INTO knowledge_fts (id, title, content, tags) VALUES (?1, ?2, ?3, ?4);",
                )?;
                let joined_tags = entity.tags.join(" ");
                ins_fts.bind_text(1, &entity.id)?;
                ins_fts.bind_text(2, &entity.title)?;
                ins_fts.bind_text(3, &entity.content)?;
                ins_fts.bind_text(4, &joined_tags)?;
                ins_fts.step()?;
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                guard.exec("COMMIT;")?;
                self.tx_commit_counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            Err(e) => {
                let _ = guard.exec("ROLLBACK;");
                Err(e)
            }
        }
    }

    fn delete_index(&self, id: &str) -> Result<(), OmkError> {
        let guard = self
            .writer
            .lock()
            .map_err(|_| OmkError::InternalError("Index writer mutex poisoned".to_string()))?;

        guard.exec("BEGIN IMMEDIATE TRANSACTION;")?;
        let res = (|| -> Result<(), OmkError> {
            let del_idx = guard.prepare("DELETE FROM knowledge_index WHERE id = ?1;")?;
            del_idx.bind_text(1, id)?;
            del_idx.step()?;

            let del_fts = guard.prepare("DELETE FROM knowledge_fts WHERE id = ?1;")?;
            del_fts.bind_text(1, id)?;
            del_fts.step()?;
            Ok(())
        })();

        match res {
            Ok(()) => {
                guard.exec("COMMIT;")?;
                self.tx_commit_counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            Err(e) => {
                let _ = guard.exec("ROLLBACK;");
                Err(e)
            }
        }
    }

    fn batch_reconcile(
        &self,
        upserts: &[(KnowledgeEntity, i64, i64)],
        delete_ids: &[String],
    ) -> Result<(), OmkError> {
        if upserts.is_empty() && delete_ids.is_empty() {
            return Ok(());
        }

        let guard = self
            .writer
            .lock()
            .map_err(|_| OmkError::InternalError("Index writer mutex poisoned".to_string()))?;

        guard.exec("BEGIN IMMEDIATE TRANSACTION;")?;
        let res = (|| -> Result<(), OmkError> {
            if !upserts.is_empty() {
                let mut existing_ids = std::collections::BTreeSet::new();
                {
                    let existing_stmt = guard.prepare("SELECT id FROM knowledge_index;")?;
                    while existing_stmt.step()? {
                        existing_ids.insert(existing_stmt.column_text(0));
                    }
                }

                let upsert_stmt = guard.prepare(
                    "INSERT INTO knowledge_index (
                         id, title, source_class, source_type, authority, created_at,
                         metadata, version_hash, file_mtime_ns, file_size, tags_norm
                     )
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     ON CONFLICT(id) DO UPDATE SET
                         title = excluded.title,
                         source_class = excluded.source_class,
                         source_type = excluded.source_type,
                         authority = excluded.authority,
                         created_at = excluded.created_at,
                         metadata = excluded.metadata,
                         version_hash = excluded.version_hash,
                         file_mtime_ns = excluded.file_mtime_ns,
                         file_size = excluded.file_size,
                         tags_norm = excluded.tags_norm;",
                )?;
                let del_fts = guard.prepare("DELETE FROM knowledge_fts WHERE id = ?1;")?;
                let ins_fts = guard.prepare(
                    "INSERT INTO knowledge_fts (id, title, content, tags) VALUES (?1, ?2, ?3, ?4);",
                )?;

                for (entity, file_mtime_ns, file_size) in upserts {
                    let tags_norm = Self::normalize_tags_column(&entity.tags);
                    upsert_stmt.reset();
                    upsert_stmt.bind_text(1, &entity.id)?;
                    upsert_stmt.bind_text(2, &entity.title)?;
                    upsert_stmt.bind_text(3, entity.source.class.as_str())?;
                    upsert_stmt.bind_text(4, entity.source.source_type.as_str())?;
                    upsert_stmt.bind_f64(5, entity.authority)?;
                    upsert_stmt.bind_i64(6, entity.created_at)?;
                    upsert_stmt.bind_text(7, &entity.metadata_json())?;
                    upsert_stmt.bind_text(8, &entity.sync.version_hash)?;
                    upsert_stmt.bind_i64(9, *file_mtime_ns)?;
                    upsert_stmt.bind_i64(10, *file_size)?;
                    upsert_stmt.bind_text(11, &tags_norm)?;
                    upsert_stmt.step()?;

                    if existing_ids.contains(&entity.id) {
                        del_fts.reset();
                        del_fts.bind_text(1, &entity.id)?;
                        del_fts.step()?;
                    }

                    ins_fts.reset();
                    let joined_tags = entity.tags.join(" ");
                    ins_fts.bind_text(1, &entity.id)?;
                    ins_fts.bind_text(2, &entity.title)?;
                    ins_fts.bind_text(3, &entity.content)?;
                    ins_fts.bind_text(4, &joined_tags)?;
                    ins_fts.step()?;
                }
            }

            if !delete_ids.is_empty() {
                let del_idx = guard.prepare("DELETE FROM knowledge_index WHERE id = ?1;")?;
                let del_fts = guard.prepare("DELETE FROM knowledge_fts WHERE id = ?1;")?;
                for id in delete_ids {
                    del_idx.reset();
                    del_idx.bind_text(1, id)?;
                    del_idx.step()?;

                    del_fts.reset();
                    del_fts.bind_text(1, id)?;
                    del_fts.step()?;
                }
            }
            Ok(())
        })();

        match res {
            Ok(()) => {
                guard.exec("COMMIT;")?;
                self.tx_commit_counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            Err(e) => {
                let _ = guard.exec("ROLLBACK;");
                Err(e)
            }
        }
    }

    fn search(
        &self,
        query: Option<&str>,
        filters: &SearchFilters,
        limit: u32,
    ) -> Result<Vec<SearchResultSnippet>, OmkError> {
        let (results, _, _) = self.search_timed_with_fallback(query, filters, limit)?;
        Ok(results)
    }

    fn search_timed(
        &self,
        query: Option<&str>,
        filters: &SearchFilters,
        limit: u32,
    ) -> Result<(Vec<SearchResultSnippet>, SearchTimings), OmkError> {
        let (results, timings, _) = self.search_timed_with_fallback(query, filters, limit)?;
        Ok((results, timings))
    }

    fn search_timed_with_fallback(
        &self,
        query: Option<&str>,
        filters: &SearchFilters,
        limit: u32,
    ) -> Result<(Vec<SearchResultSnippet>, SearchTimings, bool), OmkError> {
        let total_start = Instant::now();

        let build_start = Instant::now();
        let effective_limit = if limit == 0 { 10 } else { limit.min(100) };
        let fts_expr = query.and_then(Self::build_fts_query);
        let fallback_or_expr = query.and_then(Self::build_fts_or_fallback_query);
        let fts_build_us = build_start.elapsed().as_micros() as u64;

        let open_start = Instant::now();
        let pooled = self.acquire_reader()?;
        let reader = pooled.conn();
        let db_open_us = open_start.elapsed().as_micros() as u64;

        let mut results = Vec::new();
        let mut fallback_used = false;

        let (fts_query_us, filter_post_us) = if let Some(ref expr) = fts_expr {
            let (q_us, p_us) = execute_pushed_down_search(
                reader,
                Some(expr),
                false,
                filters,
                effective_limit,
                &mut results,
            )?;

            // REQ-PERF-004 (MET-STATIC-08): 2-Stage Adaptive FTS5 Recall Fallback (`AND` -> `OR` BM25)
            if results.is_empty() {
                if let Some(ref or_expr) = fallback_or_expr {
                    fallback_used = true;
                    let (q2_us, p2_us) = execute_pushed_down_search(
                        reader,
                        Some(or_expr),
                        true,
                        filters,
                        effective_limit,
                        &mut results,
                    )?;
                    (q_us + q2_us, p_us + p2_us)
                } else {
                    (q_us, p_us)
                }
            } else {
                (q_us, p_us)
            }
        } else {
            execute_pushed_down_search(reader, None, false, filters, effective_limit, &mut results)?
        };

        let total_us = total_start.elapsed().as_micros() as u64;
        Ok((
            results,
            SearchTimings {
                fts_build_us,
                db_open_us,
                fts_query_us,
                filter_post_us,
                total_us,
            },
            fallback_used,
        ))
    }

    fn list_index_hashes(&self) -> Result<BTreeMap<String, String>, OmkError> {
        let entries = self.list_index_entries()?;
        Ok(entries
            .into_iter()
            .map(|(k, v)| (k, v.version_hash))
            .collect())
    }

    fn list_index_entries(&self) -> Result<BTreeMap<String, IndexedDocEntry>, OmkError> {
        let pooled = self.acquire_reader()?;
        let reader = pooled.conn();
        let stmt = reader.prepare(
            "SELECT id, version_hash, file_mtime_ns, file_size, metadata FROM knowledge_index;",
        )?;
        let mut map = BTreeMap::new();
        while stmt.step()? {
            let id = stmt.column_text(0);
            let col_hash = stmt.column_text(1);
            let file_mtime_ns = stmt.column_i64(2);
            let file_size = stmt.column_i64(3);
            let version_hash = if !col_hash.is_empty() {
                col_hash
            } else {
                let meta_raw = stmt.column_text(4);
                JsonValue::parse(&meta_raw)
                    .ok()
                    .and_then(|v| {
                        v.get("version_hash")
                            .and_then(JsonValue::as_str)
                            .map(ToString::to_string)
                    })
                    .unwrap_or_default()
            };
            map.insert(
                id.clone(),
                IndexedDocEntry {
                    id,
                    version_hash,
                    file_mtime_ns,
                    file_size,
                },
            );
        }
        Ok(map)
    }
}

/// Executes an FTS5 or index-only query with all metadata filters (`source_class`, `source_type`,
/// `min_authority`, `tags`) pushed down into the SQL `WHERE` clause before `LIMIT` (`REQ-PERF-003`, `MET-STATIC-07`).
fn execute_pushed_down_search(
    reader: &SqliteConnection,
    fts_expr: Option<&str>,
    bm25_primary_sort: bool,
    filters: &SearchFilters,
    effective_limit: u32,
    out: &mut Vec<SearchResultSnippet>,
) -> Result<(u64, u64), OmkError> {
    let prep_start = Instant::now();
    let mut sql = String::with_capacity(512);
    if fts_expr.is_some() {
        sql.push_str(
            "SELECT idx.id,
                    idx.title,
                    snippet(knowledge_fts, 2, '[', ']', '...', 16) AS snip,
                    idx.source_class,
                    idx.source_type,
                    idx.authority,
                    idx.tags_norm,
                    idx.metadata
             FROM knowledge_fts
             JOIN knowledge_index idx ON idx.id = knowledge_fts.id
             WHERE knowledge_fts MATCH ?1",
        );
    } else {
        sql.push_str(
            "SELECT idx.id,
                    idx.title,
                    substr(fts.content, 1, 160) AS snip,
                    idx.source_class,
                    idx.source_type,
                    idx.authority,
                    idx.tags_norm,
                    idx.metadata
             FROM knowledge_index idx
             JOIN knowledge_fts fts ON fts.id = idx.id
             WHERE 1=1",
        );
    }

    let mut next_param: c_int = if fts_expr.is_some() { 2 } else { 1 };

    let class_param = if filters.source_class.is_some() {
        let p = next_param;
        next_param += 1;
        sql.push_str(&format!(" AND idx.source_class = ?{}", p));
        Some(p)
    } else {
        None
    };

    let type_param = if filters.source_type.is_some() {
        let p = next_param;
        next_param += 1;
        sql.push_str(&format!(" AND idx.source_type = ?{}", p));
        Some(p)
    } else {
        None
    };

    let auth_param = if filters.min_authority.is_some() {
        let p = next_param;
        next_param += 1;
        sql.push_str(&format!(" AND idx.authority >= ?{}", p));
        Some(p)
    } else {
        None
    };

    let mut tag_params = Vec::with_capacity(filters.tags.len());
    for _ in &filters.tags {
        let p = next_param;
        next_param += 1;
        sql.push_str(&format!(" AND INSTR(idx.tags_norm, ?{}) > 0", p));
        tag_params.push(p);
    }

    let limit_param = next_param;
    if fts_expr.is_some() {
        if bm25_primary_sort {
            sql.push_str(&format!(
                " ORDER BY bm25(knowledge_fts) ASC, idx.authority DESC LIMIT ?{};",
                limit_param
            ));
        } else {
            sql.push_str(&format!(
                " ORDER BY idx.authority DESC, bm25(knowledge_fts) ASC LIMIT ?{};",
                limit_param
            ));
        }
    } else {
        sql.push_str(&format!(
            " ORDER BY idx.authority DESC, idx.created_at DESC LIMIT ?{};",
            limit_param
        ));
    }

    let stmt = reader.prepare(&sql)?;
    if let Some(expr) = fts_expr {
        stmt.bind_text(1, expr)?;
    }
    if let (Some(p), Some(req_class)) = (class_param, filters.source_class) {
        stmt.bind_text(p, req_class.as_str())?;
    }
    if let (Some(p), Some(req_type)) = (type_param, filters.source_type) {
        stmt.bind_text(p, req_type.as_str())?;
    }
    if let (Some(p), Some(min_auth)) = (auth_param, filters.min_authority) {
        stmt.bind_f64(p, min_auth)?;
    }
    for (p, req_tag) in tag_params.into_iter().zip(filters.tags.iter()) {
        let needle = format!("|{}|", req_tag.trim().to_ascii_lowercase());
        stmt.bind_text(p, &needle)?;
    }
    stmt.bind_i64(limit_param, i64::from(effective_limit))?;

    let prep_us = prep_start.elapsed().as_micros() as u64;
    let (step_us, post_us) = collect_matched_rows(&stmt, effective_limit as usize, out)?;
    Ok((prep_us + step_us, post_us))
}

fn collect_matched_rows(
    stmt: &SqliteStatement<'_>,
    max_items: usize,
    out: &mut Vec<SearchResultSnippet>,
) -> Result<(u64, u64), OmkError> {
    let mut step_us_total = 0u64;
    let mut post_us_total = 0u64;
    loop {
        let step_start = Instant::now();
        let has_row = stmt.step()?;
        step_us_total += step_start.elapsed().as_micros() as u64;
        if !has_row {
            break;
        }

        let post_start = Instant::now();
        let id = stmt.column_text(0);
        let title = stmt.column_text(1);
        let snippet = stmt.column_text(2);
        let source_class = stmt.column_text(3);
        let source_type = stmt.column_text(4);
        let authority = stmt.column_f64(5);
        let meta_raw = stmt.column_text(7);

        let tags: Vec<String> = JsonValue::parse(&meta_raw)
            .ok()
            .and_then(|v| {
                v.get("tags").and_then(JsonValue::as_array).map(|arr| {
                    arr.iter()
                        .filter_map(|item| item.as_str().map(ToString::to_string))
                        .collect()
                })
            })
            .unwrap_or_default();

        out.push(SearchResultSnippet {
            file_path: format!("{}.md", id),
            id,
            title,
            snippet,
            source_class,
            source_type,
            authority,
            tags,
        });
        post_us_total += post_start.elapsed().as_micros() as u64;

        if out.len() >= max_items {
            break;
        }
    }
    Ok((step_us_total, post_us_total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{SourceClass, SourceProvenance, SourceType, SyncMetadata, SyncStrategy};

    #[test]
    fn test_sqlite_fts5_search_and_metadata_filters() {
        let db_path =
            std::env::temp_dir().join(format!("omk_idx_test_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db_path);
        let store = SqliteFtsIndexStore::new(&db_path).expect("init sqlite store");

        let e1 = KnowledgeEntity {
            id: "doc-payment".to_string(),
            title: "Payment Gateway Architecture".to_string(),
            source: SourceProvenance {
                class: SourceClass::ExternalSync,
                source_type: SourceType::Wiki,
                provider: "confluence".to_string(),
                uri: None,
                external_id: Some("PG-1".to_string()),
            },
            sync: SyncMetadata {
                strategy: SyncStrategy::UpsertFull,
                last_synced_at: "2026-07-17T10:00:00Z".to_string(),
                version_hash: "hash1".to_string(),
            },
            authority: 0.9,
            tags: vec!["payment".to_string(), "stripe".to_string()],
            author: None,
            state: None,
            created_at: 100,
            content: "Stripe webhook verification and idempotency keys for settlement.".to_string(),
        };

        let e2 = KnowledgeEntity {
            id: "doc-auth".to_string(),
            title: "OAuth2 Token Rotation".to_string(),
            source: SourceProvenance {
                class: SourceClass::UserExplicit,
                source_type: SourceType::General,
                provider: "local".to_string(),
                uri: None,
                external_id: None,
            },
            sync: SyncMetadata {
                strategy: SyncStrategy::UpsertFull,
                last_synced_at: "2026-07-17T11:00:00Z".to_string(),
                version_hash: "hash2".to_string(),
            },
            authority: 1.0,
            tags: vec!["security".to_string(), "oauth".to_string()],
            author: None,
            state: None,
            created_at: 200,
            content: "Refresh tokens must rotate on every exchange using SHA-256.".to_string(),
        };

        store.upsert_index(&e1).expect("upsert e1");
        store.upsert_index(&e2).expect("upsert e2");

        // 1. Full-text search with AND/OR
        let hits = store
            .search(Some("Stripe AND idempotency"), &SearchFilters::default(), 10)
            .expect("fts search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "doc-payment");
        assert!(hits[0].snippet.contains("[Stripe]"));

        // 2. Metadata tag filter
        let tag_hits = store
            .search(
                None,
                &SearchFilters {
                    tags: vec!["security".to_string()],
                    ..Default::default()
                },
                10,
            )
            .expect("tag search");
        assert_eq!(tag_hits.len(), 1);
        assert_eq!(tag_hits[0].id, "doc-auth");

        // 3. Reconciliation hashes
        let hashes = store.list_index_hashes().expect("list hashes");
        assert_eq!(hashes.get("doc-payment").map(String::as_str), Some("hash1"));
        assert_eq!(hashes.get("doc-auth").map(String::as_str), Some("hash2"));

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn test_static_invariants_pool_reuse_filter_pushdown_and_or_fallback() {
        let db_path = std::env::temp_dir().join(format!(
            "omk_idx_perf_invariants_{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&db_path);
        let store = SqliteFtsIndexStore::new(&db_path).expect("init sqlite store");

        // Seed 60 high-authority documents without the 'rare-incident' tag
        // and 5 low-authority (0.6) documents WITH the 'rare-incident' tag.
        let mut batch = Vec::new();
        for i in 0..65 {
            let is_rare = i >= 60;
            let entity = KnowledgeEntity {
                id: format!("doc-{:03}", i),
                title: format!("Document {}", i),
                source: SourceProvenance {
                    class: if is_rare {
                        SourceClass::AgentInferred
                    } else {
                        SourceClass::UserExplicit
                    },
                    source_type: SourceType::Wiki,
                    provider: "local".to_string(),
                    uri: None,
                    external_id: None,
                },
                sync: SyncMetadata {
                    strategy: SyncStrategy::UpsertFull,
                    last_synced_at: "2026-11-21T00:00:00Z".to_string(),
                    version_hash: format!("vhash-{}", i),
                },
                authority: if is_rare { 0.6 } else { 1.0 },
                tags: if is_rare {
                    vec!["rare-incident".to_string(), "triage".to_string()]
                } else {
                    vec!["general".to_string()]
                },
                author: None,
                state: None,
                created_at: 1000 + (i as i64),
                content: if i == 64 {
                    "Kubernetes ingress controller timeout during canary rollout.".to_string()
                } else {
                    format!("Standard engineering note number {}.", i)
                },
            };
            batch.push((entity, 123_456_789, 512));
        }

        // MET-STATIC-05: 65 documents batch-reconciled in exactly 1 SQLite transaction commit
        store.reset_counters();
        store.batch_reconcile(&batch, &[]).expect("batch reconcile");
        assert_eq!(store.tx_commit_count(), 1, "MET-STATIC-05 failed");

        // REQ-PERF-005: Duplicate upsert with identical version_hash must short-circuit (0 extra tx commits)
        store
            .upsert_index_with_meta(&batch[0].0, 123_456_789, 512)
            .expect("noop duplicate upsert");
        assert_eq!(store.tx_commit_count(), 1, "REQ-PERF-005 noop short-circuit failed");

        // MET-STATIC-07: SQL WHERE filter pushdown must find all 5 low-authority 'rare-incident' docs
        // even when limit=5 (which previously would have been cut off by 60 authority=1.0 rows!).
        store.reset_counters();
        let rare_hits = store
            .search(
                None,
                &SearchFilters {
                    tags: vec!["rare-incident".to_string()],
                    ..Default::default()
                },
                5,
            )
            .expect("pushed down tag search");
        assert_eq!(rare_hits.len(), 5, "MET-STATIC-07 filter pushdown recall must be 100%");

        // MET-STATIC-08: Multi-token query where 3 tokens match doc-064 and 1 token does not exist
        // must automatically fall back from strict AND (0 hits) to OR + BM25 ranking.
        let (fb_hits, _, fallback_used) = store
            .search_timed_with_fallback(
                Some("Kubernetes ingress rollout nonexistent_keyword_xyz"),
                &SearchFilters::default(),
                5,
            )
            .expect("fallback search");
        assert!(fallback_used, "MET-STATIC-08 should trigger 2nd-stage OR fallback");
        assert!(!fb_hits.is_empty(), "MET-STATIC-08 fallback must return partial match");
        assert_eq!(fb_hits[0].id, "doc-064");

        // MET-STATIC-06: Across 50 sequential searches, sqlite3_open must be called at most once
        for _ in 0..48 {
            let _ = store
                .search(Some("engineering"), &SearchFilters::default(), 5)
                .expect("pooled search");
        }
        assert_eq!(
            store.open_count(),
            1,
            "MET-STATIC-06 reader connection pool must open at most 1 connection for sequential queries"
        );

        let _ = std::fs::remove_file(&db_path);
    }
}
