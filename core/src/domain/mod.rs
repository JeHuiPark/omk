//! Domain models, DTOs, error types, and zero-dependency JSON/YAML utilities.
//!
//! Implements `EPIC-DATA-01` (Knowledge Data Model & Provenance) and standard error codes
//! mapped to `EPIC-IPC-01`.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// Classification of the knowledge origin and its baseline authority (`EPIC-DATA-01`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceClass {
    /// Explicitly instructed or written by the human user. Highest authority; protected from arbitrary AI overwrite.
    UserExplicit,
    /// Inferred autonomously by an AI agent during conversation. Mutable by agents.
    AgentInferred,
    /// Synchronized from an external business system (Wiki, Messenger, Issue Tracker, Code Repo).
    ExternalSync,
}

impl SourceClass {
    /// Converts the enum into its canonical string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UserExplicit => "USER_EXPLICIT",
            Self::AgentInferred => "AGENT_INFERRED",
            Self::ExternalSync => "EXTERNAL_SYNC",
        }
    }

    /// Parses a `SourceClass` from its canonical string representation.
    ///
    /// # Errors
    /// Returns [`OmkError::InvalidParams`] if the string is not a valid source class.
    pub fn from_str(raw: &str) -> Result<Self, OmkError> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "USER_EXPLICIT" => Ok(Self::UserExplicit),
            "AGENT_INFERRED" => Ok(Self::AgentInferred),
            "EXTERNAL_SYNC" => Ok(Self::ExternalSync),
            other => Err(OmkError::InvalidParams(format!(
                "Invalid source.class '{}': expected USER_EXPLICIT, AGENT_INFERRED, or EXTERNAL_SYNC",
                other
            ))),
        }
    }

    /// Returns the default authority score (0.0 to 1.0) for this source class.
    pub fn default_authority(&self) -> f64 {
        match self {
            Self::UserExplicit => 1.0,
            Self::ExternalSync => 0.9,
            Self::AgentInferred => 0.6,
        }
    }
}

/// Detailed source category determining the default synchronization strategy (`EPIC-DATA-01`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceType {
    /// Structured documentation (e.g., Notion, Confluence).
    Wiki,
    /// Time-ordered conversational stream (e.g., Slack, MS Teams).
    Messenger,
    /// Stateful work item with comments (e.g., Jira, GitHub Issues).
    IssueTracker,
    /// Versioned code or commit reference (e.g., GitHub, GitLab).
    CodeRepo,
    /// General local note or unclassified knowledge.
    General,
}

impl SourceType {
    /// Converts the enum into its canonical string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wiki => "WIKI",
            Self::Messenger => "MESSENGER",
            Self::IssueTracker => "ISSUE_TRACKER",
            Self::CodeRepo => "CODE_REPO",
            Self::General => "GENERAL",
        }
    }

    /// Parses a `SourceType` from its canonical string representation.
    ///
    /// # Errors
    /// Returns [`OmkError::InvalidParams`] if the string is not recognized.
    pub fn from_str(raw: &str) -> Result<Self, OmkError> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "WIKI" => Ok(Self::Wiki),
            "MESSENGER" => Ok(Self::Messenger),
            "ISSUE_TRACKER" => Ok(Self::IssueTracker),
            "CODE_REPO" => Ok(Self::CodeRepo),
            "GENERAL" | "WEB" | "" => Ok(Self::General),
            other => Err(OmkError::InvalidParams(format!(
                "Invalid source.type '{}': expected WIKI, MESSENGER, ISSUE_TRACKER, CODE_REPO, or GENERAL",
                other
            ))),
        }
    }

    /// Returns the default [`SyncStrategy`] associated with this source type (`EPIC-DATA-01`).
    pub fn default_sync_strategy(&self) -> SyncStrategy {
        match self {
            Self::Wiki | Self::General => SyncStrategy::UpsertFull,
            Self::Messenger => SyncStrategy::AppendOnly,
            Self::IssueTracker => SyncStrategy::StatePatch,
            Self::CodeRepo => SyncStrategy::LinkReference,
        }
    }
}

/// Synchronization strategy applied when updating existing knowledge (`EPIC-DATA-01`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStrategy {
    /// Overwrites the entire document body with the latest version.
    UpsertFull,
    /// Appends new content chronologically without destroying prior timeline entries.
    AppendOnly,
    /// Updates stateful metadata (such as issue status) while appending new activity/comments.
    StatePatch,
    /// Maintains code snippets and commit/URI references.
    LinkReference,
}

impl SyncStrategy {
    /// Converts the enum into its canonical string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UpsertFull => "UPSERT_FULL",
            Self::AppendOnly => "APPEND_ONLY",
            Self::StatePatch => "STATE_PATCH",
            Self::LinkReference => "LINK_REFERENCE",
        }
    }

    /// Parses a `SyncStrategy` from its canonical string representation.
    ///
    /// # Errors
    /// Returns [`OmkError::InvalidParams`] if the string is not recognized.
    pub fn from_str(raw: &str) -> Result<Self, OmkError> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "UPSERT_FULL" => Ok(Self::UpsertFull),
            "APPEND_ONLY" => Ok(Self::AppendOnly),
            "STATE_PATCH" => Ok(Self::StatePatch),
            "LINK_REFERENCE" => Ok(Self::LinkReference),
            other => Err(OmkError::InvalidParams(format!(
                "Invalid sync_strategy '{}': expected UPSERT_FULL, APPEND_ONLY, STATE_PATCH, or LINK_REFERENCE",
                other
            ))),
        }
    }
}

/// Source provenance metadata attached to a [`KnowledgeEntity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProvenance {
    /// High-level authority classification.
    pub class: SourceClass,
    /// Detailed source category.
    pub source_type: SourceType,
    /// Provider name (e.g., `confluence`, `slack`, `jira`, `github`, `local`).
    pub provider: String,
    /// Optional external URI.
    pub uri: Option<String>,
    /// Optional external document/thread/issue identifier.
    pub external_id: Option<String>,
}

/// Synchronization and lifecycle metadata attached to a [`KnowledgeEntity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncMetadata {
    /// Strategy used when updating this entity.
    pub strategy: SyncStrategy,
    /// ISO-8601 / UTC timestamp string of the last synchronization.
    pub last_synced_at: String,
    /// Deterministic content hash (FNV-1a 64-bit hex) used for reconciliation (`fsck`).
    pub version_hash: String,
}

/// Core domain entity representing a knowledge document in `omk` (`EPIC-DATA-01`).
#[derive(Debug, Clone, PartialEq)]
pub struct KnowledgeEntity {
    /// Unique identifier of the knowledge document (safe filename stem).
    pub id: String,
    /// Human-readable title of the document.
    pub title: String,
    /// Provenance and origin metadata.
    pub source: SourceProvenance,
    /// Synchronization lifecycle metadata.
    pub sync: SyncMetadata,
    /// Authority score between 0.0 and 1.0.
    pub authority: f64,
    /// Categorization tags.
    pub tags: Vec<String>,
    /// Optional author identifier.
    pub author: Option<String>,
    /// Optional stateful status (e.g., `Open`, `Closed` for `ISSUE_TRACKER`).
    pub state: Option<String>,
    /// Unix epoch timestamp (seconds) when the entity was created or updated.
    pub created_at: i64,
    /// Markdown body content.
    pub content: String,
}

impl KnowledgeEntity {
    /// Computes a deterministic 64-bit FNV-1a hex hash over the entity's metadata and content.
    pub fn compute_version_hash(title: &str, content: &str, tags: &[String], state: Option<&str>) -> String {
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in title.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        for byte in content.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        for tag in tags {
            for byte in tag.as_bytes() {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
        if let Some(st) = state {
            for byte in st.as_bytes() {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
        format!("{:016x}", hash)
    }

    /// Generates a current Unix epoch timestamp in seconds and an ISO-like UTC timestamp string.
    pub fn current_timestamps() -> (i64, String) {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        (secs, format_unix_iso8601(secs))
    }

    /// Generates a current Unix epoch timestamp in milliseconds and an ISO-8601 UTC string
    /// with millisecond resolution (`YYYY-MM-DDTHH:MM:SS.mmmZ`, `REQ-OBS-001`).
    pub fn current_timestamps_ms() -> (i64, String) {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        (ms, format_unix_iso8601_ms(ms))
    }

    /// Serializes the dynamic business metadata of this entity into a JSON object string
    /// for storage in the SQLite `metadata` JSON column (`EPIC-DATA-01`).
    pub fn metadata_json(&self) -> String {
        let mut map = BTreeMap::new();
        map.insert("provider".to_string(), JsonValue::Str(self.source.provider.clone()));
        if let Some(ref uri) = self.source.uri {
            map.insert("uri".to_string(), JsonValue::Str(uri.clone()));
        }
        if let Some(ref ext_id) = self.source.external_id {
            map.insert("external_id".to_string(), JsonValue::Str(ext_id.clone()));
        }
        map.insert(
            "sync_strategy".to_string(),
            JsonValue::Str(self.sync.strategy.as_str().to_string()),
        );
        map.insert(
            "last_synced_at".to_string(),
            JsonValue::Str(self.sync.last_synced_at.clone()),
        );
        map.insert(
            "version_hash".to_string(),
            JsonValue::Str(self.sync.version_hash.clone()),
        );
        map.insert(
            "tags".to_string(),
            JsonValue::Array(self.tags.iter().cloned().map(JsonValue::Str).collect()),
        );
        if let Some(ref author) = self.author {
            map.insert("author".to_string(), JsonValue::Str(author.clone()));
        }
        if let Some(ref state) = self.state {
            map.insert("state".to_string(), JsonValue::Str(state.clone()));
        }
        JsonValue::Object(map).to_json_string()
    }
}

/// Converts Unix timestamp seconds into an ISO-8601 UTC string (`YYYY-MM-DDTHH:MM:SSZ`).
pub fn format_unix_iso8601(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = rem / 3600;
    let minute = (rem % 3600) / 60;
    let second = rem % 60;

    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, m, d, hour, minute, second
    )
}

/// Converts Unix timestamp milliseconds into an ISO-8601 UTC string with millisecond resolution
/// (`YYYY-MM-DDTHH:MM:SS.mmmZ`, `REQ-OBS-001`).
pub fn format_unix_iso8601_ms(epoch_ms: i64) -> String {
    let secs = epoch_ms.div_euclid(1000);
    let millis = epoch_ms.rem_euclid(1000);
    let base = format_unix_iso8601(secs);
    let stem = base.strip_suffix('Z').unwrap_or(&base);
    format!("{}.{:03}Z", stem, millis)
}

/// Command DTO for creating or updating knowledge (`upsert_knowledge`).
#[derive(Debug, Clone)]
pub struct UpsertCommand {
    /// Optional explicit document ID. If omitted, derived deterministically from `external_id` or `title`.
    pub id: Option<String>,
    /// Document title.
    pub title: String,
    /// Markdown body content.
    pub content: String,
    /// Provenance classification.
    pub source_class: SourceClass,
    /// Detailed source type.
    pub source_type: SourceType,
    /// Source provider name.
    pub provider: String,
    /// Optional source URI.
    pub uri: Option<String>,
    /// Optional external identifier.
    pub external_id: Option<String>,
    /// Optional explicit sync strategy override.
    pub sync_strategy: Option<SyncStrategy>,
    /// Optional explicit authority score override.
    pub authority: Option<f64>,
    /// Categorization tags.
    pub tags: Vec<String>,
    /// Optional author.
    pub author: Option<String>,
    /// Optional stateful status (e.g., `Open` / `Closed`).
    pub state: Option<String>,
}

/// Result DTO returned by [`UpsertCommand`] execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertResult {
    /// Final document ID.
    pub id: String,
    /// Status string (`created` or `updated`).
    pub status: String,
    /// Updated version hash.
    pub version_hash: String,
    /// Relative path within the Vault (`<id>.md`).
    pub file_path: String,
}

/// Filter parameters for searching knowledge (`EPIC-SEARCH-01`).
#[derive(Debug, Clone, Default)]
pub struct SearchFilters {
    /// Tags that matching documents must contain.
    pub tags: Vec<String>,
    /// Optional filter by `SourceClass`.
    pub source_class: Option<SourceClass>,
    /// Optional filter by `SourceType`.
    pub source_type: Option<SourceType>,
    /// Optional minimum authority score.
    pub min_authority: Option<f64>,
}

/// Query DTO for searching knowledge (`search_knowledge`).
#[derive(Debug, Clone)]
pub struct SearchQuery {
    /// Optional full-text search query string.
    pub query: Option<String>,
    /// Metadata filter criteria.
    pub filters: SearchFilters,
    /// Maximum number of matches to return.
    pub limit: u32,
}

/// Snippet and metadata returned for a single search hit (`EPIC-SEARCH-01`).
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResultSnippet {
    /// Document identifier.
    pub id: String,
    /// Relative file path (`<id>.md`).
    pub file_path: String,
    /// Document title.
    pub title: String,
    /// Relevant text snippet extracted from the body.
    pub snippet: String,
    /// Source class string.
    pub source_class: String,
    /// Source type string.
    pub source_type: String,
    /// Authority score.
    pub authority: f64,
    /// Document tags.
    pub tags: Vec<String>,
}

/// Standard error enumeration across all layers (`EPIC-IPC-01`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OmkError {
    /// `-32700` JSON parse error.
    ParseError(String),
    /// `-32600` Invalid JSON-RPC request structure.
    InvalidRequest(String),
    /// `-32601` Unknown RPC method or tool name.
    MethodNotFound(String),
    /// `-32602` Invalid method parameters or domain validation failure.
    InvalidParams(String),
    /// `-32603` Internal storage, database, or I/O error.
    InternalError(String),
    /// `-32000` Daemon unreachable or request timed out.
    DaemonUnreachable(String),
    /// `-32001` Daemon queue full (Backpressure fail-fast).
    ServerBusy(String),
    /// `-32002` Authority violation (e.g., `AGENT_INFERRED` attempting to overwrite `USER_EXPLICIT`).
    AuthorityViolation(String),
    /// `-32003` Path traversal attempt or unauthorized file access.
    AccessDenied(String),
    /// `-32004` Requested knowledge ID does not exist.
    NotFound(String),
}

impl OmkError {
    /// Returns the standardized JSON-RPC 2.0 error code (`EPIC-IPC-01`).
    pub fn rpc_code(&self) -> i32 {
        match self {
            Self::ParseError(_) => -32700,
            Self::InvalidRequest(_) => -32600,
            Self::MethodNotFound(_) => -32601,
            Self::InvalidParams(_) => -32602,
            Self::InternalError(_) => -32603,
            Self::DaemonUnreachable(_) => -32000,
            Self::ServerBusy(_) => -32001,
            Self::AuthorityViolation(_) => -32002,
            Self::AccessDenied(_) => -32003,
            Self::NotFound(_) => -32004,
        }
    }

    /// Returns the human-readable error message.
    pub fn message(&self) -> &str {
        match self {
            Self::ParseError(m)
            | Self::InvalidRequest(m)
            | Self::MethodNotFound(m)
            | Self::InvalidParams(m)
            | Self::InternalError(m)
            | Self::DaemonUnreachable(m)
            | Self::ServerBusy(m)
            | Self::AuthorityViolation(m)
            | Self::AccessDenied(m)
            | Self::NotFound(m) => m,
        }
    }

    /// Returns a privacy-safe canonical error kind identifier free of any user input (`REQ-OBS-003`).
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::ParseError(_) => "ParseError",
            Self::InvalidRequest(_) => "InvalidRequest",
            Self::MethodNotFound(_) => "MethodNotFound",
            Self::InvalidParams(_) => "InvalidParams",
            Self::InternalError(_) => "InternalError",
            Self::DaemonUnreachable(_) => "DaemonUnreachable",
            Self::ServerBusy(_) => "ServerBusy",
            Self::AuthorityViolation(_) => "AuthorityViolation",
            Self::AccessDenied(_) => "AccessDenied",
            Self::NotFound(_) => "NotFound",
        }
    }
}

/// Microsecond-level Span timings for `upsert_knowledge` (`REQ-OBS-001`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpsertTimings {
    /// Time spent validating parameters, checking existing document, and verifying authority (`us`).
    pub validate_read_us: u64,
    /// Time spent rendering Markdown, writing the temporary file, and renaming (`us`).
    pub vault_write_us: u64,
    /// Time spent calling `fsync` (`sync_all`) on the temporary Vault file (`us`).
    pub vault_fsync_us: u64,
    /// Time spent updating the SQLite `knowledge_index` and `knowledge_fts` tables (`us`).
    pub index_write_us: u64,
    /// Total pipeline duration for the upsert operation (`us`).
    pub total_us: u64,
}

/// Quantitative non-PII metrics for `upsert_knowledge` (`REQ-OBS-002`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertMetrics {
    /// Final resolved document ID (must be hashed via [`salted_doc_ref`] before logging unless unredacted).
    pub raw_doc_id: String,
    /// `"created"` or `"updated"`.
    pub upsert_status: String,
    /// Canonical `SourceClass` string.
    pub source_class: String,
    /// Canonical `SourceType` string.
    pub source_type: String,
    /// Canonical `SyncStrategy` string.
    pub sync_strategy: String,
    /// Number of tags on the resulting document.
    pub tag_count: usize,
    /// UTF-8 byte length of the stored Markdown body.
    pub content_bytes: usize,
    /// Estimated LLM token count (`ceil(content_bytes / 4)`).
    pub estimated_tokens: usize,
}

/// Microsecond-level Span timings for `search_knowledge` (`REQ-OBS-001`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchTimings {
    /// Time spent parsing and sanitizing the FTS5 query expression (`us`).
    pub fts_build_us: u64,
    /// Time spent opening the concurrent SQLite reader connection and setting PRAGMAs (`us`).
    pub db_open_us: u64,
    /// Time spent preparing and stepping SQLite FTS5 / index statements (`us`).
    pub fts_query_us: u64,
    /// Time spent parsing row metadata JSON, applying filters, and building snippets (`us`).
    pub filter_post_us: u64,
    /// Total pipeline duration for the search operation (`us`).
    pub total_us: u64,
}

/// Quantitative non-PII metrics for `search_knowledge` (`REQ-OBS-002`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMetrics {
    /// Raw query string if provided (must be hashed via [`salted_query_ref`] before logging).
    pub raw_query: Option<String>,
    /// Whether a non-empty search query was provided.
    pub query_present: bool,
    /// Number of whitespace-separated query tokens.
    pub query_token_count: usize,
    /// Whether the query explicitly included `AND` or `OR` boolean operators.
    pub query_has_boolean_op: bool,
    /// Number of filter tags specified.
    pub filter_tag_count: usize,
    /// Whether a `source_class` filter was specified.
    pub has_source_class_filter: bool,
    /// Whether a `source_type` filter was specified.
    pub has_source_type_filter: bool,
    /// Whether a `min_authority` filter was specified.
    pub has_min_authority_filter: bool,
    /// Requested `limit` parameter.
    pub requested_limit: u32,
    /// Number of matched documents returned.
    pub result_count: usize,
    /// `true` if `result_count == 0`.
    pub zero_hit: bool,
    /// Total UTF-8 byte length across all returned snippets.
    pub content_bytes: usize,
    /// Estimated LLM token count across all returned snippets.
    pub estimated_tokens: usize,
}

/// Microsecond-level Span timings for `read_knowledge` (`REQ-OBS-001`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadTimings {
    /// Time spent reading the `.md` file from disk (`us`).
    pub vault_read_us: u64,
    /// Time spent parsing YAML Frontmatter and Markdown body (`us`).
    pub markdown_parse_us: u64,
    /// Total pipeline duration for the read operation (`us`).
    pub total_us: u64,
}

/// Quantitative non-PII metrics for `read_knowledge` (`REQ-OBS-002`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadMetrics {
    /// Requested document ID (must be hashed via [`salted_doc_ref`] before logging).
    pub raw_doc_id: String,
    /// Whether the document was found.
    pub found: bool,
    /// Number of tags on the document.
    pub tag_count: usize,
    /// Canonical `SourceClass` string.
    pub source_class: String,
    /// Canonical `SourceType` string.
    pub source_type: String,
    /// UTF-8 byte length of the document body.
    pub content_bytes: usize,
    /// Estimated LLM token count (`ceil(content_bytes / 4)`).
    pub estimated_tokens: usize,
}

/// Microsecond-level Span timings for `reconcile` (`REQ-OBS-001`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileTimings {
    /// Time spent scanning and parsing all `.md` files in the Vault (`us`).
    pub vault_scan_us: u64,
    /// Time spent querying existing `version_hash` entries from SQLite (`us`).
    pub index_hash_scan_us: u64,
    /// Time spent re-indexing missing or stale documents (`us`).
    pub reindex_write_us: u64,
    /// Time spent deleting orphaned index entries (`us`).
    pub orphan_prune_us: u64,
    /// Total reconciliation duration (`us`).
    pub total_us: u64,
}

/// Estimates the LLM token count from a UTF-8 byte length (`ceil(bytes / 4)`, `REQ-OBS-002`).
pub fn estimate_tokens(byte_len: usize) -> usize {
    if byte_len == 0 {
        0
    } else {
        (byte_len + 3) / 4
    }
}

/// Computes a deterministic, salted one-way hash reference (`doc#<16-hex>`) for a document ID
/// so correlation analysis can be performed without exposing raw IDs or titles (`REQ-OBS-003`).
pub fn salted_doc_ref(salt: u64, doc_id: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325 ^ salt. rotate_left(17);
    for b in b"omk:doc:" {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for b in doc_id.trim().as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash ^= salt;
    hash = hash.wrapping_mul(0x100000001b3);
    format!("doc#{:016x}", hash)
}

/// Computes a deterministic, salted one-way hash reference (`qry#<16-hex>`) for a search query
/// after normalizing whitespace and ASCII case (`REQ-OBS-003`).
pub fn salted_query_ref(salt: u64, raw_query: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325 ^ salt.rotate_right(13);
    for b in b"omk:qry:" {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let mut first = true;
    for tok in raw_query.split_whitespace() {
        if !first {
            hash ^= u64::from(b' ');
            hash = hash.wrapping_mul(0x100000001b3);
        }
        for b in tok.bytes() {
            hash ^= u64::from(b.to_ascii_lowercase());
            hash = hash.wrapping_mul(0x100000001b3);
        }
        first = false;
    }
    hash ^= salt;
    hash = hash.wrapping_mul(0x100000001b3);
    format!("qry#{:016x}", hash)
}

/// Sanitizes file system paths inside log strings by replacing the Vault path with `<VAULT>`,
/// the home directory path with `<HOME>`, and any `/Users/<username>` or `/home/<username>`
/// prefix with `<HOME>` (`REQ-OBS-003`).
pub fn sanitize_system_path(raw: &str, vault_dir: Option<&str>, home_dir: Option<&str>) -> String {
    let mut out = raw.to_string();
    if let Some(v) = vault_dir {
        let trimmed = v.trim_end_matches('/');
        if !trimmed.is_empty() {
            out = out.replace(trimmed, "<VAULT>");
        }
    }
    if let Some(h) = home_dir {
        let trimmed = h.trim_end_matches('/');
        if !trimmed.is_empty() {
            out = out.replace(trimmed, "<HOME>");
        }
    }
    for prefix in ["/Users/", "/home/"] {
        while let Some(idx) = out.find(prefix) {
            let after_prefix = idx + prefix.len();
            let rest = &out[after_prefix..];
            let user_len = rest
                .find(|c: char| c == '/' || c == '"' || c == '\'' || c.is_whitespace())
                .unwrap_or(rest.len());
            if user_len == 0 {
                break;
            }
            let end_idx = after_prefix + user_len;
            out.replace_range(idx..end_idx, "<HOME>");
        }
    }
    out
}

impl fmt::Display for OmkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.rpc_code(), self.message())
    }
}

impl std::error::Error for OmkError {}

// ============================================================================
// Zero-dependency JSON AST, Parser, and Serializer
// ============================================================================

/// Lightweight, self-contained JSON value representation.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    /// `null`
    Null,
    /// `true` or `false`
    Bool(bool),
    /// Numeric value (f64)
    Number(f64),
    /// UTF-8 string
    Str(String),
    /// Ordered JSON array
    Array(Vec<JsonValue>),
    /// Key-value JSON object
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    /// Parses a JSON string into a [`JsonValue`].
    ///
    /// # Errors
    /// Returns [`OmkError::ParseError`] if the input is not valid JSON.
    pub fn parse(input: &str) -> Result<Self, OmkError> {
        let mut parser = JsonParser::new(input);
        let value = parser.parse_value()?;
        parser.skip_whitespace();
        if !parser.is_eof() {
            return Err(OmkError::ParseError(
                "Unexpected trailing characters after JSON value".to_string(),
            ));
        }
        Ok(value)
    }

    /// Serializes the [`JsonValue`] into a compact single-line JSON string.
    pub fn to_json_string(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out);
        out
    }

    /// Serializes the [`JsonValue`] into a human-readable, indented multi-line JSON string.
    pub fn to_pretty_json_string(&self) -> String {
        let mut out = String::new();
        self.write_pretty_json(&mut out, 0);
        out.push('\n');
        out
    }

    fn write_pretty_json(&self, out: &mut String, indent: usize) {
        let pad = "  ".repeat(indent);
        let child_pad = "  ".repeat(indent + 1);
        match self {
            Self::Null | Self::Bool(_) | Self::Number(_) | Self::Str(_) => {
                self.write_json(out);
            }
            Self::Array(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                } else {
                    out.push_str("[\n");
                    for (i, item) in items.iter().enumerate() {
                        if i > 0 {
                            out.push_str(",\n");
                        }
                        out.push_str(&child_pad);
                        item.write_pretty_json(out, indent + 1);
                    }
                    out.push('\n');
                    out.push_str(&pad);
                    out.push(']');
                }
            }
            Self::Object(map) => {
                if map.is_empty() {
                    out.push_str("{}");
                } else {
                    out.push_str("{\n");
                    for (i, (k, v)) in map.iter().enumerate() {
                        if i > 0 {
                            out.push_str(",\n");
                        }
                        out.push_str(&child_pad);
                        out.push('"');
                        escape_json_into(k, out);
                        out.push_str("\": ");
                        v.write_pretty_json(out, indent + 1);
                    }
                    out.push('\n');
                    out.push_str(&pad);
                    out.push('}');
                }
            }
        }
    }

    fn write_json(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Self::Number(n) => {
                if n.is_finite() {
                    if n.fract() == 0.0 && *n >= (i64::MIN as f64) && *n <= (i64::MAX as f64) {
                        out.push_str(&format!("{}", *n as i64));
                    } else {
                        out.push_str(&format!("{}", n));
                    }
                } else {
                    out.push_str("null");
                }
            }
            Self::Str(s) => {
                out.push('"');
                escape_json_into(s, out);
                out.push('"');
            }
            Self::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_json(out);
                }
                out.push(']');
            }
            Self::Object(map) => {
                out.push('{');
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push('"');
                    escape_json_into(k, out);
                    out.push_str("\":");
                    v.write_json(out);
                }
                out.push('}');
            }
        }
    }

    /// Helper to extract an object reference.
    pub fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            Self::Object(m) => Some(m),
            _ => None,
        }
    }

    /// Helper to extract a string slice.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Helper to extract a number as `f64`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Helper to extract a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Helper to extract an array slice.
    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            Self::Array(a) => Some(a.as_slice()),
            _ => None,
        }
    }

    /// Looks up a field in a JSON object.
    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.as_object().and_then(|m| m.get(key))
    }
}

/// Escapes special characters in a string according to RFC 8259.
pub fn escape_json_into(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
}

struct JsonParser<'a> {
    chars: Vec<char>,
    pos: usize,
    _marker: &'a str,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
            _marker: input,
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn next_char(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue, OmkError> {
        self.skip_whitespace();
        let ch = self
            .peek()
            .ok_or_else(|| OmkError::ParseError("Unexpected end of JSON input".to_string()))?;
        match ch {
            '"' => self.parse_string().map(JsonValue::Str),
            '{' => self.parse_object(),
            '[' => self.parse_array(),
            't' | 'f' => self.parse_bool(),
            'n' => self.parse_null(),
            '-' | '0'..='9' => self.parse_number(),
            other => Err(OmkError::ParseError(format!(
                "Unexpected character '{}' at position {}",
                other, self.pos
            ))),
        }
    }

    fn parse_string(&mut self) -> Result<String, OmkError> {
        let open = self.next_char();
        if open != Some('"') {
            return Err(OmkError::ParseError("Expected '\"'".to_string()));
        }
        let mut out = String::new();
        while let Some(ch) = self.next_char() {
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let esc = self.next_char().ok_or_else(|| {
                        OmkError::ParseError("Unterminated escape sequence".to_string())
                    })?;
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{08}'),
                        'f' => out.push('\u{0C}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let mut hex = String::with_capacity(4);
                            for _ in 0..4 {
                                let hc = self.next_char().ok_or_else(|| {
                                    OmkError::ParseError("Incomplete unicode escape".to_string())
                                })?;
                                hex.push(hc);
                            }
                            let code = u32::from_str_radix(&hex, 16).map_err(|_| {
                                OmkError::ParseError(format!("Invalid unicode escape \\u{}", hex))
                            })?;
                            if let Some(decoded) = char::from_u32(code) {
                                out.push(decoded);
                            }
                        }
                        other => {
                            return Err(OmkError::ParseError(format!(
                                "Invalid escape character '\\{}'",
                                other
                            )))
                        }
                    }
                }
                c => out.push(c),
            }
        }
        Err(OmkError::ParseError("Unterminated JSON string".to_string()))
    }

    fn parse_object(&mut self) -> Result<JsonValue, OmkError> {
        self.next_char(); // '{'
        self.skip_whitespace();
        let mut map = BTreeMap::new();
        if self.peek() == Some('}') {
            self.next_char();
            return Ok(JsonValue::Object(map));
        }
        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.skip_whitespace();
            if self.next_char() != Some(':') {
                return Err(OmkError::ParseError(
                    "Expected ':' after object key".to_string(),
                ));
            }
            let val = self.parse_value()?;
            map.insert(key, val);
            self.skip_whitespace();
            match self.next_char() {
                Some('}') => break,
                Some(',') => continue,
                _ => {
                    return Err(OmkError::ParseError(
                        "Expected ',' or '}' in object".to_string(),
                    ))
                }
            }
        }
        Ok(JsonValue::Object(map))
    }

    fn parse_array(&mut self) -> Result<JsonValue, OmkError> {
        self.next_char(); // '['
        self.skip_whitespace();
        let mut arr = Vec::new();
        if self.peek() == Some(']') {
            self.next_char();
            return Ok(JsonValue::Array(arr));
        }
        loop {
            let val = self.parse_value()?;
            arr.push(val);
            self.skip_whitespace();
            match self.next_char() {
                Some(']') => break,
                Some(',') => continue,
                _ => {
                    return Err(OmkError::ParseError(
                        "Expected ',' or ']' in array".to_string(),
                    ))
                }
            }
        }
        Ok(JsonValue::Array(arr))
    }

    fn parse_bool(&mut self) -> Result<JsonValue, OmkError> {
        if self.chars[self.pos..].starts_with(&['t', 'r', 'u', 'e']) {
            self.pos += 4;
            Ok(JsonValue::Bool(true))
        } else if self.chars[self.pos..].starts_with(&['f', 'a', 'l', 's', 'e']) {
            self.pos += 5;
            Ok(JsonValue::Bool(false))
        } else {
            Err(OmkError::ParseError("Invalid boolean literal".to_string()))
        }
    }

    fn parse_null(&mut self) -> Result<JsonValue, OmkError> {
        if self.chars[self.pos..].starts_with(&['n', 'u', 'l', 'l']) {
            self.pos += 4;
            Ok(JsonValue::Null)
        } else {
            Err(OmkError::ParseError("Invalid null literal".to_string()))
        }
    }

    fn parse_number(&mut self) -> Result<JsonValue, OmkError> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() || matches!(ch, '.' | 'e' | 'E' | '+' | '-') {
                self.pos += 1;
            } else {
                break;
            }
        }
        let s: String = self.chars[start..self.pos].iter().collect();
        let num = s
            .parse::<f64>()
            .map_err(|_| OmkError::ParseError(format!("Invalid number '{}'", s)))?;
        Ok(JsonValue::Number(num))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_class_and_type_defaults() {
        assert_eq!(SourceClass::UserExplicit.default_authority(), 1.0);
        assert_eq!(SourceClass::ExternalSync.default_authority(), 0.9);
        assert_eq!(SourceClass::AgentInferred.default_authority(), 0.6);

        assert_eq!(
            SourceType::Wiki.default_sync_strategy(),
            SyncStrategy::UpsertFull
        );
        assert_eq!(
            SourceType::Messenger.default_sync_strategy(),
            SyncStrategy::AppendOnly
        );
        assert_eq!(
            SourceType::IssueTracker.default_sync_strategy(),
            SyncStrategy::StatePatch
        );
        assert_eq!(
            SourceType::CodeRepo.default_sync_strategy(),
            SyncStrategy::LinkReference
        );
    }

    #[test]
    fn test_json_roundtrip() {
        let raw = r#"{"a":1,"b":true,"c":"hello \"world\"\n","d":["x","y"],"e":null}"#;
        let parsed = JsonValue::parse(raw).expect("valid json");
        assert_eq!(parsed.get("a").and_then(JsonValue::as_f64), Some(1.0));
        assert_eq!(parsed.get("b").and_then(JsonValue::as_bool), Some(true));
        assert_eq!(
            parsed.get("c").and_then(JsonValue::as_str),
            Some("hello \"world\"\n")
        );
        let serialized = parsed.to_json_string();
        let reparsed = JsonValue::parse(&serialized).expect("reparsed");
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn test_privacy_salted_hashing_and_path_sanitization() {
        let salt = 0x1234_5678_9abc_def0;
        let d1 = salted_doc_ref(salt, "secret-project-alpha");
        let d2 = salted_doc_ref(salt, "secret-project-alpha");
        let d3 = salted_doc_ref(salt ^ 1, "secret-project-alpha");
        assert!(d1.starts_with("doc#"));
        assert_eq!(d1.len(), 20);
        assert_eq!(d1, d2);
        assert_ne!(d1, d3);
        assert!(!d1.contains("secret"));

        let q1 = salted_query_ref(salt, "  Sensitive   Token  ");
        let q2 = salted_query_ref(salt, "sensitive token");
        assert!(q1.starts_with("qry#"));
        assert_eq!(q1, q2);
        assert!(!q1.contains("sensitive"));

        let raw_msg = "socket=/Users/alice/.omk/vault/.omk/daemon.sock home=/home/bob/work";
        let clean = sanitize_system_path(
            raw_msg,
            Some("/Users/alice/.omk/vault"),
            Some("/Users/alice"),
        );
        assert!(!clean.contains("alice"));
        assert!(!clean.contains("bob"));
        assert!(clean.contains("<VAULT>/.omk/daemon.sock"));
        assert!(clean.contains("<HOME>/work"));
    }
}
