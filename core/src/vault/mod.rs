//! Markdown-based Persistent Knowledge Storage (`EPIC-VAULT-01`).
//!
//! Defines the [`DocumentStore`] abstraction and [`LocalMarkdownDocumentStore`] implementation,
//! enforcing UTF-8 `.md` files with YAML Frontmatter, atomic file replacements, and strict
//! path traversal protection.

use crate::domain::{
    KnowledgeEntity, OmkError, SourceClass, SourceProvenance, SourceType, SyncMetadata,
    SyncStrategy,
};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Storage abstraction for the Single Source of Truth (SSOT) Markdown Vault (`docs/architecture.md` §5.1).
pub trait DocumentStore: Send + Sync {
    /// Persists a [`KnowledgeEntity`] as a Markdown file with YAML Frontmatter.
    ///
    /// # Errors
    /// Returns [`OmkError::AccessDenied`] if the ID attempts path traversal, or
    /// [`OmkError::InternalError`] on file system failure.
    fn put_document(&self, entity: &KnowledgeEntity) -> Result<String, OmkError>;

    /// Loads a [`KnowledgeEntity`] by its document ID.
    ///
    /// # Errors
    /// Returns [`OmkError::NotFound`] if the document does not exist, [`OmkError::AccessDenied`]
    /// if the ID is unsafe, or [`OmkError::InternalError`] on I/O failure.
    fn get_document(&self, id: &str) -> Result<KnowledgeEntity, OmkError>;

    /// Deletes a document from the Vault by ID.
    ///
    /// # Errors
    /// Returns [`OmkError::NotFound`] if the document is missing or [`OmkError::AccessDenied`]
    /// if the ID is invalid.
    fn delete_document(&self, id: &str) -> Result<(), OmkError>;

    /// Lists all [`KnowledgeEntity`] documents currently stored in the Vault (used for reconciliation/`fsck`).
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if reading the Vault directory fails.
    fn list_documents(&self) -> Result<Vec<KnowledgeEntity>, OmkError>;
}

/// Local file system implementation of [`DocumentStore`] storing `.md` files in a dedicated root directory.
#[derive(Debug, Clone)]
pub struct LocalMarkdownDocumentStore {
    root_dir: PathBuf,
}

impl LocalMarkdownDocumentStore {
    /// Creates or opens a local Markdown Vault at `root_dir`.
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if the directory cannot be created or canonicalized.
    pub fn new<P: AsRef<Path>>(root_dir: P) -> Result<Self, OmkError> {
        let path = root_dir.as_ref();
        fs::create_dir_all(path).map_err(|e| {
            OmkError::InternalError(format!(
                "Failed to initialize Vault directory '{}': {}",
                path.display(),
                e
            ))
        })?;
        let canonical = fs::canonicalize(path).map_err(|e| {
            OmkError::InternalError(format!(
                "Failed to canonicalize Vault directory '{}': {}",
                path.display(),
                e
            ))
        })?;
        Ok(Self {
            root_dir: canonical,
        })
    }

    /// Returns the canonical root directory path of the Vault.
    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    /// Validates a document ID to prevent Path Traversal (`REQ-VAULT-002`) and resolves its `.md` path.
    ///
    /// # Errors
    /// Returns [`OmkError::AccessDenied`] if `id` contains path separators, `..`, or illegal characters.
    pub fn resolve_safe_path(&self, id: &str) -> Result<PathBuf, OmkError> {
        let trimmed = id.trim();
        if trimmed.is_empty() {
            return Err(OmkError::InvalidParams(
                "Document ID cannot be empty".to_string(),
            ));
        }
        let stem = trimmed.strip_suffix(".md").unwrap_or(trimmed);
        if stem.is_empty()
            || stem.contains("..")
            || stem.contains('/')
            || stem.contains('\\')
            || stem.contains('\0')
            || stem.starts_with('.')
        {
            return Err(OmkError::AccessDenied(format!(
                "Path traversal or unsafe document ID blocked: '{}'",
                id
            )));
        }
        if !stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return Err(OmkError::AccessDenied(format!(
                "Document ID '{}' contains disallowed characters (only [a-zA-Z0-9_-] allowed)",
                id
            )));
        }
        let target = self.root_dir.join(format!("{}.md", stem));
        if !target.starts_with(&self.root_dir) {
            return Err(OmkError::AccessDenied(format!(
                "Resolved path escapes Vault root: '{}'",
                id
            )));
        }
        Ok(target)
    }

    /// Sanitizes an arbitrary string (such as a title or external ID) into a safe document ID.
    pub fn sanitize_id(raw: &str) -> String {
        let mut out = String::new();
        let mut prev_sep = false;
        for ch in raw.trim().chars() {
            if ch.is_ascii_alphanumeric() {
                out.push(ch.to_ascii_lowercase());
                prev_sep = false;
            } else if matches!(ch, '-' | '_' | ' ' | ':' | '/') {
                if !out.is_empty() && !prev_sep {
                    out.push('-');
                    prev_sep = true;
                }
            }
        }
        while out.ends_with('-') {
            out.pop();
        }
        if out.is_empty() {
            let mut hash: u64 = 0xcbf29ce484222325;
            for b in raw.as_bytes() {
                hash ^= u64::from(*b);
                hash = hash.wrapping_mul(0x100000001b3);
            }
            format!("doc-{:016x}", hash)
        } else {
            out
        }
    }

    /// Serializes a [`KnowledgeEntity`] into Markdown with YAML Frontmatter (`EPIC-DATA-01` §3.1).
    pub fn render_markdown(entity: &KnowledgeEntity) -> String {
        let mut out = String::new();
        out.push_str("---\n");
        out.push_str(&format!("id: \"{}\"\n", escape_yaml_str(&entity.id)));
        out.push_str(&format!("title: \"{}\"\n", escape_yaml_str(&entity.title)));
        out.push_str("source:\n");
        out.push_str(&format!("  class: \"{}\"\n", entity.source.class.as_str()));
        out.push_str(&format!(
            "  type: \"{}\"\n",
            entity.source.source_type.as_str()
        ));
        out.push_str(&format!(
            "  provider: \"{}\"\n",
            escape_yaml_str(&entity.source.provider)
        ));
        if let Some(ref uri) = entity.source.uri {
            out.push_str(&format!("  uri: \"{}\"\n", escape_yaml_str(uri)));
        }
        if let Some(ref ext_id) = entity.source.external_id {
            out.push_str(&format!(
                "  external_id: \"{}\"\n",
                escape_yaml_str(ext_id)
            ));
        }
        out.push_str("sync:\n");
        out.push_str(&format!(
            "  strategy: \"{}\"\n",
            entity.sync.strategy.as_str()
        ));
        out.push_str(&format!(
            "  last_synced_at: \"{}\"\n",
            escape_yaml_str(&entity.sync.last_synced_at)
        ));
        out.push_str(&format!(
            "  version_hash: \"{}\"\n",
            escape_yaml_str(&entity.sync.version_hash)
        ));
        out.push_str("metrics:\n");
        out.push_str(&format!("  authority: {}\n", entity.authority));
        let tags_rendered: Vec<String> = entity
            .tags
            .iter()
            .map(|t| format!("\"{}\"", escape_yaml_str(t)))
            .collect();
        out.push_str(&format!("tags: [{}]\n", tags_rendered.join(", ")));
        if let Some(ref author) = entity.author {
            out.push_str(&format!("author: \"{}\"\n", escape_yaml_str(author)));
        }
        if let Some(ref state) = entity.state {
            out.push_str(&format!("state: \"{}\"\n", escape_yaml_str(state)));
        }
        out.push_str(&format!("created_at: {}\n", entity.created_at));
        out.push_str("---\n");
        out.push_str(&entity.content);
        if !entity.content.ends_with('\n') {
            out.push('\n');
        }
        out
    }

    /// Parses a Markdown file with YAML Frontmatter back into a [`KnowledgeEntity`].
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if the frontmatter is malformed.
    pub fn parse_markdown(fallback_id: &str, raw: &str) -> Result<KnowledgeEntity, OmkError> {
        let normalized = raw.strip_prefix('\u{feff}').unwrap_or(raw);
        let rest = normalized.strip_prefix("---\n").ok_or_else(|| {
            OmkError::InternalError(format!(
                "Document '{}' is missing opening YAML frontmatter delimiter '---'",
                fallback_id
            ))
        })?;
        let end_idx = rest.find("\n---\n").ok_or_else(|| {
            OmkError::InternalError(format!(
                "Document '{}' is missing closing YAML frontmatter delimiter '---'",
                fallback_id
            ))
        })?;

        let yaml_block = &rest[..end_idx];
        let body_raw = &rest[end_idx + 5..];
        let content = body_raw.strip_suffix('\n').unwrap_or(body_raw).to_string();

        let mut id = fallback_id.to_string();
        let mut title = fallback_id.to_string();
        let mut source_class = SourceClass::UserExplicit;
        let mut source_type = SourceType::General;
        let mut provider = "local".to_string();
        let mut uri = None;
        let mut external_id = None;
        let mut sync_strategy = None;
        let mut last_synced_at = "1970-01-01T00:00:00Z".to_string();
        let mut version_hash = String::new();
        let mut authority = None;
        let mut tags = Vec::new();
        let mut author = None;
        let mut state = None;
        let mut created_at: i64 = 0;

        let mut section = "";
        for line in yaml_block.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let is_indented = line.starts_with(' ') || line.starts_with('\t');
            if !is_indented {
                if trimmed == "source:" {
                    section = "source";
                    continue;
                } else if trimmed == "sync:" {
                    section = "sync";
                    continue;
                } else if trimmed == "metrics:" {
                    section = "metrics";
                    continue;
                } else {
                    section = "";
                }
            }

            if let Some((k, v)) = trimmed.split_once(':') {
                let key = k.trim();
                let val = unquote_yaml_str(v.trim());
                match (section, key) {
                    ("", "id") => id = val,
                    ("", "title") => title = val,
                    ("", "tags") => tags = parse_yaml_array(v.trim()),
                    ("", "author") => author = Some(val),
                    ("", "state") => state = Some(val),
                    ("", "created_at") => {
                        created_at = val.parse::<i64>().unwrap_or(0);
                    }
                    ("source", "class") => {
                        source_class = SourceClass::from_str(&val)?;
                    }
                    ("source", "type") => {
                        source_type = SourceType::from_str(&val)?;
                    }
                    ("source", "provider") => provider = val,
                    ("source", "uri") => uri = Some(val),
                    ("source", "external_id") => external_id = Some(val),
                    ("sync", "strategy") => {
                        sync_strategy = Some(SyncStrategy::from_str(&val)?);
                    }
                    ("sync", "last_synced_at") => last_synced_at = val,
                    ("sync", "version_hash") => version_hash = val,
                    ("metrics", "authority") => {
                        authority = val.parse::<f64>().ok();
                    }
                    _ => {}
                }
            }
        }

        let final_strategy = sync_strategy.unwrap_or_else(|| source_type.default_sync_strategy());
        let final_authority = authority.unwrap_or_else(|| source_class.default_authority());
        if version_hash.is_empty() {
            version_hash =
                KnowledgeEntity::compute_version_hash(&title, &content, &tags, state.as_deref());
        }

        Ok(KnowledgeEntity {
            id,
            title,
            source: SourceProvenance {
                class: source_class,
                source_type,
                provider,
                uri,
                external_id,
            },
            sync: SyncMetadata {
                strategy: final_strategy,
                last_synced_at,
                version_hash,
            },
            authority: final_authority,
            tags,
            author,
            state,
            created_at,
            content,
        })
    }
}

impl DocumentStore for LocalMarkdownDocumentStore {
    fn put_document(&self, entity: &KnowledgeEntity) -> Result<String, OmkError> {
        let target_path = self.resolve_safe_path(&entity.id)?;
        let markdown = Self::render_markdown(entity);

        // Atomic write via temporary file in the same directory + fsync + rename
        let tmp_name = format!(
            ".{}.md.tmp.{}",
            entity.id,
            std::process::id()
        );
        let tmp_path = self.root_dir.join(tmp_name);

        {
            let mut file = File::create(&tmp_path).map_err(|e| {
                OmkError::InternalError(format!(
                    "Failed to create temporary Vault file '{}': {}",
                    tmp_path.display(),
                    e
                ))
            })?;
            file.write_all(markdown.as_bytes()).map_err(|e| {
                OmkError::InternalError(format!(
                    "Failed to write temporary Vault file '{}': {}",
                    tmp_path.display(),
                    e
                ))
            })?;
            file.sync_all().map_err(|e| {
                OmkError::InternalError(format!(
                    "Failed to fsync temporary Vault file '{}': {}",
                    tmp_path.display(),
                    e
                ))
            })?;
        }

        fs::rename(&tmp_path, &target_path).map_err(|e| {
            let _ = fs::remove_file(&tmp_path);
            OmkError::InternalError(format!(
                "Failed to atomically replace Vault file '{}': {}",
                target_path.display(),
                e
            ))
        })?;

        Ok(format!("{}.md", entity.id))
    }

    fn get_document(&self, id: &str) -> Result<KnowledgeEntity, OmkError> {
        let target_path = self.resolve_safe_path(id)?;
        if !target_path.exists() {
            return Err(OmkError::NotFound(format!(
                "Knowledge document '{}' not found in Vault",
                id
            )));
        }
        let raw = fs::read_to_string(&target_path).map_err(|e| {
            OmkError::InternalError(format!(
                "Failed to read Vault file '{}': {}",
                target_path.display(),
                e
            ))
        })?;
        let clean_id = id.trim().strip_suffix(".md").unwrap_or(id.trim());
        Self::parse_markdown(clean_id, &raw)
    }

    fn delete_document(&self, id: &str) -> Result<(), OmkError> {
        let target_path = self.resolve_safe_path(id)?;
        if !target_path.exists() {
            return Err(OmkError::NotFound(format!(
                "Knowledge document '{}' not found in Vault",
                id
            )));
        }
        fs::remove_file(&target_path).map_err(|e| {
            OmkError::InternalError(format!(
                "Failed to delete Vault file '{}': {}",
                target_path.display(),
                e
            ))
        })
    }

    fn list_documents(&self) -> Result<Vec<KnowledgeEntity>, OmkError> {
        let entries = fs::read_dir(&self.root_dir).map_err(|e| {
            OmkError::InternalError(format!(
                "Failed to read Vault directory '{}': {}",
                self.root_dir.display(),
                e
            ))
        })?;
        let mut docs = Vec::new();
        for entry_res in entries {
            let entry = entry_res.map_err(|e| {
                OmkError::InternalError(format!("Failed to read directory entry: {}", e))
            })?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let file_name = match path.file_name().and_then(|s| s.to_str()) {
                Some(name) if !name.starts_with('.') && name.ends_with(".md") => name,
                _ => continue,
            };
            let stem = file_name.strip_suffix(".md").unwrap_or(file_name);
            let raw = fs::read_to_string(&path).map_err(|e| {
                OmkError::InternalError(format!(
                    "Failed to read Vault file '{}': {}",
                    path.display(),
                    e
                ))
            })?;
            if let Ok(entity) = Self::parse_markdown(stem, &raw) {
                docs.push(entity);
            }
        }
        docs.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(docs)
    }
}

fn escape_yaml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

fn unquote_yaml_str(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        let inner = &trimmed[1..trimmed.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                if let Some(esc) = chars.next() {
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        other => out.push(other),
                    }
                }
            } else {
                out.push(ch);
            }
        }
        out
    } else if trimmed.len() >= 2 && trimmed.starts_with('\'') && trimmed.ends_with('\'') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

fn parse_yaml_array(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    if inner.trim().is_empty() {
        return Vec::new();
    }
    inner
        .split(',')
        .map(|item| unquote_yaml_str(item.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_traversal_blocked() {
        let tmp = std::env::temp_dir().join(format!("omk_vault_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let store = LocalMarkdownDocumentStore::new(&tmp).expect("create store");

        assert!(matches!(
            store.resolve_safe_path("../secret"),
            Err(OmkError::AccessDenied(_))
        ));
        assert!(matches!(
            store.resolve_safe_path("/etc/passwd"),
            Err(OmkError::AccessDenied(_))
        ));
        assert!(matches!(
            store.resolve_safe_path("nested/file"),
            Err(OmkError::AccessDenied(_))
        ));
        assert!(store.resolve_safe_path("valid-doc_01").is_ok());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_markdown_roundtrip_and_atomic_put() {
        let tmp = std::env::temp_dir().join(format!("omk_vault_rt_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let store = LocalMarkdownDocumentStore::new(&tmp).expect("create store");

        let entity = KnowledgeEntity {
            id: "payment-arch-01".to_string(),
            title: "Payment System Architecture".to_string(),
            source: SourceProvenance {
                class: SourceClass::ExternalSync,
                source_type: SourceType::Wiki,
                provider: "confluence".to_string(),
                uri: Some("https://confluence.example.com/pages/123".to_string()),
                external_id: Some("PAGE-123".to_string()),
            },
            sync: SyncMetadata {
                strategy: SyncStrategy::UpsertFull,
                last_synced_at: "2026-07-17T16:15:00Z".to_string(),
                version_hash: "a1b2c3d4e5f60718".to_string(),
            },
            authority: 0.9,
            tags: vec!["architecture".to_string(), "payment".to_string()],
            author: Some("alice@company.com".to_string()),
            state: None,
            created_at: 1_752_768_900,
            content: "# Overview\nThis is the payment architecture specification.".to_string(),
        };

        let rel_path = store.put_document(&entity).expect("put document");
        assert_eq!(rel_path, "payment-arch-01.md");

        let loaded = store.get_document("payment-arch-01").expect("get document");
        assert_eq!(loaded, entity);

        let all = store.list_documents().expect("list documents");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0], entity);

        let _ = fs::remove_dir_all(&tmp);
    }
}
