//! SQLite FTS5 Search and Metadata Index Store (`EPIC-SEARCH-01`, `EPIC-DATA-01`).
//!
//! Defines the [`IndexStore`] abstraction and [`SqliteFtsIndexStore`] implementation using
//! direct C ABI bindings to system `libsqlite3` (zero external Rust crate dependencies)
//! with WAL mode enabled for concurrent read/write performance.

use crate::domain::{
    JsonValue, KnowledgeEntity, OmkError, SearchFilters, SearchResultSnippet, SearchTimings,
};
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int, c_void};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Mutex;
use std::time::Instant;

/// Storage abstraction for full-text search and metadata indexing (`docs/architecture.md` §5.2).
pub trait IndexStore: Send + Sync {
    /// Inserts or updates a [`KnowledgeEntity`] in the search index.
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if the database transaction fails.
    fn upsert_index(&self, entity: &KnowledgeEntity) -> Result<(), OmkError>;

    /// Removes a document from the search index by ID.
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if the database operation fails.
    fn delete_index(&self, id: &str) -> Result<(), OmkError>;

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

    /// Returns a map of `id -> version_hash` for all indexed documents (used for startup reconciliation/`fsck`).
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if querying the index fails.
    fn list_index_hashes(&self) -> Result<BTreeMap<String, String>, OmkError>;
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

/// SQLite + FTS5 implementation of [`IndexStore`] (`EPIC-DATA-01` §3.2, `EPIC-SEARCH-01`).
pub struct SqliteFtsIndexStore {
    db_path: PathBuf,
    writer: Mutex<SqliteConnection>,
}

impl SqliteFtsIndexStore {
    /// Opens or initializes the SQLite FTS5 index database at `db_path` with WAL mode enabled.
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
                 metadata TEXT NOT NULL
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_fts USING fts5(
                 id UNINDEXED,
                 title,
                 content,
                 tags
             );",
        )?;
        Ok(Self {
            db_path: path,
            writer: Mutex::new(conn),
        })
    }

    /// Opens a read-only/concurrent WAL connection for non-blocking parallel queries (`docs/architecture.md` §4.1).
    fn open_reader(&self) -> Result<SqliteConnection, OmkError> {
        let conn = SqliteConnection::open(&self.db_path)?;
        conn.exec("PRAGMA busy_timeout=5000; PRAGMA query_only=ON;")?;
        Ok(conn)
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
}

impl IndexStore for SqliteFtsIndexStore {
    fn upsert_index(&self, entity: &KnowledgeEntity) -> Result<(), OmkError> {
        let guard = self
            .writer
            .lock()
            .map_err(|_| OmkError::InternalError("Index writer mutex poisoned".to_string()))?;

        guard.exec("BEGIN IMMEDIATE TRANSACTION;")?;
        let result = (|| -> Result<(), OmkError> {
            {
                let stmt = guard.prepare(
                    "INSERT INTO knowledge_index (id, title, source_class, source_type, authority, created_at, metadata)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(id) DO UPDATE SET
                         title = excluded.title,
                         source_class = excluded.source_class,
                         source_type = excluded.source_type,
                         authority = excluded.authority,
                         created_at = excluded.created_at,
                         metadata = excluded.metadata;",
                )?;
                stmt.bind_text(1, &entity.id)?;
                stmt.bind_text(2, &entity.title)?;
                stmt.bind_text(3, entity.source.class.as_str())?;
                stmt.bind_text(4, entity.source.source_type.as_str())?;
                stmt.bind_f64(5, entity.authority)?;
                stmt.bind_i64(6, entity.created_at)?;
                stmt.bind_text(7, &entity.metadata_json())?;
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
        let (results, _) = self.search_timed(query, filters, limit)?;
        Ok(results)
    }

    fn search_timed(
        &self,
        query: Option<&str>,
        filters: &SearchFilters,
        limit: u32,
    ) -> Result<(Vec<SearchResultSnippet>, SearchTimings), OmkError> {
        let total_start = Instant::now();

        let build_start = Instant::now();
        let effective_limit = if limit == 0 { 10 } else { limit.min(100) };
        let fts_expr = query.and_then(Self::build_fts_query);
        let fts_build_us = build_start.elapsed().as_micros() as u64;

        let open_start = Instant::now();
        let reader = self.open_reader()?;
        let db_open_us = open_start.elapsed().as_micros() as u64;

        let mut results = Vec::new();
        let (fts_query_us, filter_post_us) = if let Some(ref expr) = fts_expr {
            let prep_start = Instant::now();
            let stmt = reader.prepare(
                "SELECT idx.id,
                        idx.title,
                        snippet(knowledge_fts, 2, '[', ']', '...', 16) AS snip,
                        idx.source_class,
                        idx.source_type,
                        idx.authority,
                        idx.metadata
                 FROM knowledge_fts
                 JOIN knowledge_index idx ON idx.id = knowledge_fts.id
                 WHERE knowledge_fts MATCH ?1
                 ORDER BY idx.authority DESC, bm25(knowledge_fts) ASC
                 LIMIT ?2;",
            )?;
            stmt.bind_text(1, expr)?;
            stmt.bind_i64(2, i64::from(effective_limit * 4))?;
            let prep_us = prep_start.elapsed().as_micros() as u64;
            let (step_us, post_us) =
                collect_filtered_rows(&stmt, filters, effective_limit as usize, &mut results)?;
            (prep_us + step_us, post_us)
        } else {
            let prep_start = Instant::now();
            let stmt = reader.prepare(
                "SELECT idx.id,
                        idx.title,
                        substr(fts.content, 1, 160) AS snip,
                        idx.source_class,
                        idx.source_type,
                        idx.authority,
                        idx.metadata
                 FROM knowledge_index idx
                 JOIN knowledge_fts fts ON fts.id = idx.id
                 ORDER BY idx.authority DESC, idx.created_at DESC
                 LIMIT ?1;",
            )?;
            stmt.bind_i64(1, i64::from(effective_limit * 4))?;
            let prep_us = prep_start.elapsed().as_micros() as u64;
            let (step_us, post_us) =
                collect_filtered_rows(&stmt, filters, effective_limit as usize, &mut results)?;
            (prep_us + step_us, post_us)
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
        ))
    }

    fn list_index_hashes(&self) -> Result<BTreeMap<String, String>, OmkError> {
        let reader = self.open_reader()?;
        let stmt = reader.prepare("SELECT id, metadata FROM knowledge_index;")?;
        let mut map = BTreeMap::new();
        while stmt.step()? {
            let id = stmt.column_text(0);
            let meta_raw = stmt.column_text(1);
            let version_hash = JsonValue::parse(&meta_raw)
                .ok()
                .and_then(|v| {
                    v.get("version_hash")
                        .and_then(JsonValue::as_str)
                        .map(ToString::to_string)
                })
                .unwrap_or_default();
            map.insert(id, version_hash);
        }
        Ok(map)
    }
}

fn collect_filtered_rows(
    stmt: &SqliteStatement<'_>,
    filters: &SearchFilters,
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
        let meta_raw = stmt.column_text(6);

        if let Some(req_class) = filters.source_class {
            if source_class != req_class.as_str() {
                post_us_total += post_start.elapsed().as_micros() as u64;
                continue;
            }
        }
        if let Some(req_type) = filters.source_type {
            if source_type != req_type.as_str() {
                post_us_total += post_start.elapsed().as_micros() as u64;
                continue;
            }
        }
        if let Some(min_auth) = filters.min_authority {
            if authority < min_auth {
                post_us_total += post_start.elapsed().as_micros() as u64;
                continue;
            }
        }

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

        if !filters.tags.is_empty() {
            let all_matched = filters
                .tags
                .iter()
                .all(|req_tag| tags.iter().any(|t| t.eq_ignore_ascii_case(req_tag)));
            if !all_matched {
                post_us_total += post_start.elapsed().as_micros() as u64;
                continue;
            }
        }

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
}
