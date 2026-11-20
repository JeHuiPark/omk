//! CQRS Command and Query Pipelines and Vault-Index Reconciliation (`docs/architecture.md` §4).
//!
//! Coordinates [`DocumentStore`] and [`IndexStore`] through clean use cases:
//! - [`UpsertKnowledgeUseCase`] (Command Pipeline: Validator -> Vault Writer -> Indexer)
//! - [`SearchKnowledgeUseCase`] (Query Pipeline: Parser -> Retriever -> Synthesizer)
//! - [`ReadKnowledgeUseCase`] (Query Pipeline: Direct Document Retrieval)
//! - [`ReconcileReport`] / [`KnowledgePipeline::reconcile`] (Startup Auto-Recovery / `fsck`)

use crate::domain::{
    estimate_tokens, KnowledgeEntity, OmkError, ReadMetrics, ReadTimings, ReconcileTimings,
    SearchMetrics, SearchQuery, SearchResultSnippet, SearchTimings, SourceClass, SourceProvenance,
    SyncMetadata, SyncStrategy, UpsertCommand, UpsertMetrics, UpsertResult, UpsertTimings,
};
use crate::index::IndexStore;
use crate::vault::{DocumentStore, LocalMarkdownDocumentStore};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

/// Summary report produced by startup or manual Vault-Index reconciliation (`fsck`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Number of Vault documents scanned.
    pub scanned_documents: usize,
    /// Number of missing or stale index entries rebuilt from the Vault SSOT.
    pub reindexed_count: usize,
    /// Number of orphaned index entries removed because their `.md` file no longer exists.
    pub removed_orphans_count: usize,
}

/// Unified CQRS Knowledge Pipeline orchestrating [`DocumentStore`] and [`IndexStore`].
#[derive(Clone)]
pub struct KnowledgePipeline {
    doc_store: Arc<dyn DocumentStore>,
    index_store: Arc<dyn IndexStore>,
}

impl KnowledgePipeline {
    /// Constructs a new [`KnowledgePipeline`] with injected [`DocumentStore`] and [`IndexStore`] traits.
    pub fn new(doc_store: Arc<dyn DocumentStore>, index_store: Arc<dyn IndexStore>) -> Self {
        Self {
            doc_store,
            index_store,
        }
    }

    /// Executes the Command Pipeline (`upsert_knowledge`): validates authority, applies the
    /// appropriate [`SyncStrategy`], writes atomically to [`DocumentStore`], and updates [`IndexStore`].
    ///
    /// # Errors
    /// - [`OmkError::InvalidParams`] if `title` or `content` is empty.
    /// - [`OmkError::AuthorityViolation`] if an `AGENT_INFERRED` command tries to overwrite a `USER_EXPLICIT` document.
    /// - [`OmkError::AccessDenied`] if the document ID attempts path traversal.
    /// - [`OmkError::InternalError`] on storage or index failure.
    pub fn upsert_knowledge(&self, cmd: UpsertCommand) -> Result<UpsertResult, OmkError> {
        let (res, _, _) = self.upsert_knowledge_timed(cmd)?;
        Ok(res)
    }

    /// Executes the Command Pipeline (`upsert_knowledge`) and returns the result alongside
    /// microsecond-level [`UpsertTimings`] and quantitative [`UpsertMetrics`] (`REQ-OBS-001`, `REQ-OBS-002`).
    ///
    /// # Errors
    /// Returns the same errors as [`KnowledgePipeline::upsert_knowledge`].
    pub fn upsert_knowledge_timed(
        &self,
        cmd: UpsertCommand,
    ) -> Result<(UpsertResult, UpsertTimings, UpsertMetrics), OmkError> {
        let total_start = Instant::now();
        let val_start = Instant::now();

        let title = cmd.title.trim().to_string();
        if title.is_empty() {
            return Err(OmkError::InvalidParams(
                "Parameter 'title' must not be empty".to_string(),
            ));
        }
        if cmd.content.trim().is_empty() {
            return Err(OmkError::InvalidParams(
                "Parameter 'content' must not be empty".to_string(),
            ));
        }

        // Determine document ID
        let doc_id = match cmd.id {
            Some(ref explicit_id) if !explicit_id.trim().is_empty() => {
                let trimmed = explicit_id.trim();
                let stem = trimmed.strip_suffix(".md").unwrap_or(trimmed);
                if stem.contains("..")
                    || stem.contains('/')
                    || stem.contains('\\')
                    || stem.starts_with('.')
                {
                    return Err(OmkError::AccessDenied(format!(
                        "Unsafe document ID rejected: '{}'",
                        explicit_id
                    )));
                }
                LocalMarkdownDocumentStore::sanitize_id(stem)
            }
            _ => {
                if let Some(ref ext_id) = cmd.external_id {
                    if !ext_id.trim().is_empty() {
                        LocalMarkdownDocumentStore::sanitize_id(&format!(
                            "{}-{}",
                            cmd.provider, ext_id
                        ))
                    } else {
                        LocalMarkdownDocumentStore::sanitize_id(&title)
                    }
                } else {
                    LocalMarkdownDocumentStore::sanitize_id(&title)
                }
            }
        };

        let existing = match self.doc_store.get_document(&doc_id) {
            Ok(doc) => Some(doc),
            Err(OmkError::NotFound(_)) => None,
            Err(e) => return Err(e),
        };

        // 1. Validator: Authority check (REQ / EPIC-DATA-01 §2.1)
        if let Some(ref prev) = existing {
            if prev.source.class == SourceClass::UserExplicit
                && cmd.source_class == SourceClass::AgentInferred
            {
                return Err(OmkError::AuthorityViolation(format!(
                    "AGENT_INFERRED source cannot overwrite USER_EXPLICIT knowledge '{}'",
                    doc_id
                )));
            }
        }

        let strategy = cmd
            .sync_strategy
            .unwrap_or_else(|| cmd.source_type.default_sync_strategy());
        let authority = cmd
            .authority
            .unwrap_or_else(|| cmd.source_class.default_authority())
            .clamp(0.0, 1.0);
        let (now_secs, now_iso) = KnowledgeEntity::current_timestamps();

        // 2. Apply Sync Strategy (EPIC-DATA-01 §2.2)
        let (final_content, final_tags, final_state, status_str) = match existing {
            None => {
                let body = match strategy {
                    SyncStrategy::LinkReference => {
                        format_link_reference(&cmd.content, cmd.uri.as_deref(), cmd.external_id.as_deref())
                    }
                    _ => cmd.content.clone(),
                };
                (body, deduplicate_tags(&cmd.tags), cmd.state.clone(), "created")
            }
            Some(prev) => {
                let merged_tags = merge_tags(&prev.tags, &cmd.tags);
                match strategy {
                    SyncStrategy::UpsertFull => {
                        let next_state = cmd.state.clone().or(prev.state);
                        (cmd.content.clone(), merged_tags, next_state, "updated")
                    }
                    SyncStrategy::AppendOnly => {
                        let appended = format!(
                            "{}\n\n---\n**[Appended at {}]**\n{}",
                            prev.content.trim_end(),
                            now_iso,
                            cmd.content.trim()
                        );
                        let next_state = cmd.state.clone().or(prev.state);
                        (appended, merged_tags, next_state, "updated")
                    }
                    SyncStrategy::StatePatch => {
                        let next_state = cmd.state.clone().or(prev.state);
                        let state_banner = next_state
                            .as_deref()
                            .map(|s| format!("**[State: {} | Updated at {}]**\n", s, now_iso))
                            .unwrap_or_else(|| format!("**[Patched at {}]**\n", now_iso));
                        let patched = format!(
                            "{}\n\n---\n{}{}",
                            prev.content.trim_end(),
                            state_banner,
                            cmd.content.trim()
                        );
                        (patched, merged_tags, next_state, "updated")
                    }
                    SyncStrategy::LinkReference => {
                        let formatted = format_link_reference(
                            &cmd.content,
                            cmd.uri.as_deref().or(prev.source.uri.as_deref()),
                            cmd.external_id
                                .as_deref()
                                .or(prev.source.external_id.as_deref()),
                        );
                        let next_state = cmd.state.clone().or(prev.state);
                        (formatted, merged_tags, next_state, "updated")
                    }
                }
            }
        };

        let version_hash = KnowledgeEntity::compute_version_hash(
            &title,
            &final_content,
            &final_tags,
            final_state.as_deref(),
        );

        let content_bytes = final_content.len();
        let tag_count = final_tags.len();

        let entity = KnowledgeEntity {
            id: doc_id.clone(),
            title,
            source: SourceProvenance {
                class: cmd.source_class,
                source_type: cmd.source_type,
                provider: if cmd.provider.trim().is_empty() {
                    "local".to_string()
                } else {
                    cmd.provider.trim().to_string()
                },
                uri: cmd.uri,
                external_id: cmd.external_id,
            },
            sync: SyncMetadata {
                strategy,
                last_synced_at: now_iso,
                version_hash: version_hash.clone(),
            },
            authority,
            tags: final_tags,
            author: cmd.author,
            state: final_state,
            created_at: now_secs,
            content: final_content,
        };
        let validate_read_us = val_start.elapsed().as_micros() as u64;

        // 3. Vault Writer (SSOT) -> 4. Indexer (IndexStore)
        let (file_path, vault_write_us, vault_fsync_us, file_mtime_ns, file_size) =
            self.doc_store.put_document_with_meta(&entity)?;
        let idx_start = Instant::now();
        self.index_store
            .upsert_index_with_meta(&entity, file_mtime_ns, file_size)?;
        let index_write_us = idx_start.elapsed().as_micros() as u64;
        let total_us = total_start.elapsed().as_micros() as u64;

        let timings = UpsertTimings {
            validate_read_us,
            vault_write_us,
            vault_fsync_us,
            index_write_us,
            total_us,
        };
        let metrics = UpsertMetrics {
            raw_doc_id: doc_id.clone(),
            upsert_status: status_str.to_string(),
            source_class: cmd.source_class.as_str().to_string(),
            source_type: cmd.source_type.as_str().to_string(),
            sync_strategy: strategy.as_str().to_string(),
            tag_count,
            content_bytes,
            estimated_tokens: estimate_tokens(content_bytes),
        };

        Ok((
            UpsertResult {
                id: doc_id,
                status: status_str.to_string(),
                version_hash,
                file_path,
            },
            timings,
            metrics,
        ))
    }

    /// Executes the Query Pipeline (`search_knowledge`): queries only [`IndexStore`] without touching
    /// the file system (`docs/architecture.md` §4.3, `EPIC-SEARCH-01`).
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if querying the index fails.
    pub fn search_knowledge(
        &self,
        query: SearchQuery,
    ) -> Result<Vec<SearchResultSnippet>, OmkError> {
        let (hits, _, _) = self.search_knowledge_timed(query)?;
        Ok(hits)
    }

    /// Executes the Query Pipeline (`search_knowledge`) and returns matched snippets alongside
    /// [`SearchTimings`] and [`SearchMetrics`] (`REQ-OBS-001`, `REQ-OBS-002`, `REQ-PERF-004`).
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if querying the index fails.
    pub fn search_knowledge_timed(
        &self,
        query: SearchQuery,
    ) -> Result<(Vec<SearchResultSnippet>, SearchTimings, SearchMetrics), OmkError> {
        let raw_q = query.query.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let query_present = raw_q.is_some();
        let mut query_token_count = 0usize;
        let mut query_has_boolean_op = false;
        if let Some(q_str) = raw_q {
            for tok in q_str.split_whitespace() {
                if tok.eq_ignore_ascii_case("AND") || tok.eq_ignore_ascii_case("OR") {
                    query_has_boolean_op = true;
                } else {
                    query_token_count += 1;
                }
            }
        }

        let (matches, timings, fallback_used) = self.index_store.search_timed_with_fallback(
            query.query.as_deref(),
            &query.filters,
            query.limit,
        )?;

        let content_bytes: usize = matches.iter().map(|m| m.snippet.len() + m.title.len()).sum();
        let result_count = matches.len();
        let metrics = SearchMetrics {
            raw_query: raw_q.map(ToString::to_string),
            query_present,
            query_token_count,
            query_has_boolean_op,
            filter_tag_count: query.filters.tags.len(),
            has_source_class_filter: query.filters.source_class.is_some(),
            has_source_type_filter: query.filters.source_type.is_some(),
            has_min_authority_filter: query.filters.min_authority.is_some(),
            requested_limit: query.limit,
            result_count,
            zero_hit: result_count == 0,
            fallback_used,
            content_bytes,
            estimated_tokens: estimate_tokens(content_bytes),
        };

        Ok((matches, timings, metrics))
    }

    /// Executes the single-document Query Pipeline (`read_knowledge`): retrieves the full
    /// [`KnowledgeEntity`] from [`DocumentStore`] (`REQ-VAULT-002`).
    ///
    /// # Errors
    /// Returns [`OmkError::NotFound`] if the document does not exist or [`OmkError::AccessDenied`]
    /// if the ID attempts path traversal.
    pub fn read_knowledge(&self, id: &str) -> Result<KnowledgeEntity, OmkError> {
        let (doc, _, _) = self.read_knowledge_timed(id)?;
        Ok(doc)
    }

    /// Executes the single-document Query Pipeline (`read_knowledge`) and returns the entity
    /// alongside [`ReadTimings`] and [`ReadMetrics`] (`REQ-OBS-001`, `REQ-OBS-002`).
    ///
    /// # Errors
    /// Returns [`OmkError::NotFound`] if the document does not exist or [`OmkError::AccessDenied`]
    /// if the ID attempts path traversal.
    pub fn read_knowledge_timed(
        &self,
        id: &str,
    ) -> Result<(KnowledgeEntity, ReadTimings, ReadMetrics), OmkError> {
        let total_start = Instant::now();
        let (entity, vault_read_us, markdown_parse_us) = self.doc_store.get_document_timed(id)?;
        let total_us = total_start.elapsed().as_micros() as u64;
        let content_bytes = entity.content.len();
        let timings = ReadTimings {
            vault_read_us,
            markdown_parse_us,
            total_us,
        };
        let metrics = ReadMetrics {
            raw_doc_id: entity.id.clone(),
            found: true,
            tag_count: entity.tags.len(),
            source_class: entity.source.class.as_str().to_string(),
            source_type: entity.source.source_type.as_str().to_string(),
            content_bytes,
            estimated_tokens: estimate_tokens(content_bytes),
        };
        Ok((entity, timings, metrics))
    }

    /// Reconciles the [`DocumentStore`] (SSOT) and [`IndexStore`] by comparing file metadata
    /// (`file_mtime_ns`, `file_size`) and `version_hash` values, rebuilding missing or outdated
    /// index entries and pruning orphaned entries in a single batched transaction (`REQ-PERF-002`).
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if reading the Vault or updating the index fails.
    pub fn reconcile(&self) -> Result<ReconcileReport, OmkError> {
        let (rep, _) = self.reconcile_timed()?;
        Ok(rep)
    }

    /// Reconciles the [`DocumentStore`] and [`IndexStore`] and returns both the [`ReconcileReport`]
    /// and microsecond-level [`ReconcileTimings`] (`REQ-OBS-001`, `REQ-PERF-002`, `MET-STATIC-04`, `MET-STATIC-05`).
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if reading the Vault or updating the index fails.
    pub fn reconcile_timed(&self) -> Result<(ReconcileReport, ReconcileTimings), OmkError> {
        let total_start = Instant::now();

        let hash_start = Instant::now();
        let index_entries = self.index_store.list_index_entries()?;
        let index_hash_scan_us = hash_start.elapsed().as_micros() as u64;

        let scan_start = Instant::now();
        let mut seen_ids = BTreeSet::new();
        let mut upserts_to_batch: Vec<(KnowledgeEntity, i64, i64)> = Vec::new();
        let mut meta_backfills: Vec<(KnowledgeEntity, i64, i64)> = Vec::new();
        let scanned_documents: usize;

        if let Some(meta_list) = self.doc_store.list_document_metadata()? {
            scanned_documents = meta_list.len();
            for meta in meta_list {
                seen_ids.insert(meta.id.clone());
                let is_unchanged = match index_entries.get(&meta.id) {
                    Some(idx_entry) => {
                        meta.file_mtime_ns > 0
                            && meta.file_size > 0
                            && idx_entry.file_mtime_ns == meta.file_mtime_ns
                            && idx_entry.file_size == meta.file_size
                            && !idx_entry.version_hash.is_empty()
                    }
                    None => false,
                };
                if is_unchanged {
                    // MET-STATIC-04: Skip reading and parsing unchanged Markdown files
                    continue;
                }

                if let Ok(doc) = self.doc_store.get_document(&meta.id) {
                    seen_ids.insert(doc.id.clone());
                    let needs_reindex = match index_entries.get(&doc.id) {
                        Some(idx_entry) => idx_entry.version_hash != doc.sync.version_hash,
                        None => true,
                    };
                    if needs_reindex {
                        upserts_to_batch.push((doc, meta.file_mtime_ns, meta.file_size));
                    } else {
                        meta_backfills.push((doc, meta.file_mtime_ns, meta.file_size));
                    }
                }
            }
        } else {
            let vault_docs = self.doc_store.list_documents()?;
            scanned_documents = vault_docs.len();
            for doc in vault_docs {
                seen_ids.insert(doc.id.clone());
                let needs_reindex = match index_entries.get(&doc.id) {
                    Some(idx_entry) => idx_entry.version_hash != doc.sync.version_hash,
                    None => true,
                };
                if needs_reindex {
                    upserts_to_batch.push((doc, 0, 0));
                }
            }
        }
        let vault_scan_us = scan_start.elapsed().as_micros() as u64;

        let prune_start = Instant::now();
        let mut orphans_to_delete = Vec::new();
        for indexed_id in index_entries.keys() {
            if !seen_ids.contains(indexed_id) {
                orphans_to_delete.push(indexed_id.clone());
            }
        }
        let orphan_prune_us = prune_start.elapsed().as_micros() as u64;

        let reindex_start = Instant::now();
        let reindexed_count = upserts_to_batch.len();
        let removed_orphans_count = orphans_to_delete.len();
        for (doc, mtime_ns, size) in &meta_backfills {
            let _ = self
                .index_store
                .upsert_index_with_meta(doc, *mtime_ns, *size);
        }
        // MET-STATIC-05: Execute all K upserts and orphan deletions in a single SQLite transaction
        self.index_store
            .batch_reconcile(&upserts_to_batch, &orphans_to_delete)?;
        let reindex_write_us = reindex_start.elapsed().as_micros() as u64;
        let total_us = total_start.elapsed().as_micros() as u64;

        Ok((
            ReconcileReport {
                scanned_documents,
                reindexed_count,
                removed_orphans_count,
            },
            ReconcileTimings {
                vault_scan_us,
                index_hash_scan_us,
                reindex_write_us,
                orphan_prune_us,
                total_us,
            },
        ))
    }
}

fn deduplicate_tags(tags: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for t in tags {
        let clean = t.trim().to_string();
        if !clean.is_empty() && !out.iter().any(|existing: &String| existing.eq_ignore_ascii_case(&clean)) {
            out.push(clean);
        }
    }
    out
}

fn merge_tags(existing: &[String], incoming: &[String]) -> Vec<String> {
    let mut combined = existing.to_vec();
    for t in incoming {
        let clean = t.trim().to_string();
        if !clean.is_empty()
            && !combined
                .iter()
                .any(|prev: &String| prev.eq_ignore_ascii_case(&clean))
        {
            combined.push(clean);
        }
    }
    combined
}

fn format_link_reference(content: &str, uri: Option<&str>, external_id: Option<&str>) -> String {
    let mut out = content.trim().to_string();
    if uri.is_some() || external_id.is_some() {
        out.push_str("\n\n### References\n");
        if let Some(u) = uri {
            out.push_str(&format!("- URI: {}\n", u));
        }
        if let Some(ext) = external_id {
            out.push_str(&format!("- Ref ID: `{}`\n", ext));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{SearchFilters, SourceType};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// In-memory mock implementation of [`DocumentStore`] for fast isolated unit testing (`docs/coding_conventions.md` §3).
    #[derive(Default)]
    struct MockDocumentStore {
        docs: Mutex<BTreeMap<String, KnowledgeEntity>>,
    }

    impl DocumentStore for MockDocumentStore {
        fn put_document(&self, entity: &KnowledgeEntity) -> Result<String, OmkError> {
            self.docs
                .lock()
                .unwrap()
                .insert(entity.id.clone(), entity.clone());
            Ok(format!("{}.md", entity.id))
        }

        fn get_document(&self, id: &str) -> Result<KnowledgeEntity, OmkError> {
            self.docs
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .ok_or_else(|| OmkError::NotFound(format!("Not found: {}", id)))
        }

        fn delete_document(&self, id: &str) -> Result<(), OmkError> {
            self.docs
                .lock()
                .unwrap()
                .remove(id)
                .map(|_| ())
                .ok_or_else(|| OmkError::NotFound(format!("Not found: {}", id)))
        }

        fn list_documents(&self) -> Result<Vec<KnowledgeEntity>, OmkError> {
            Ok(self.docs.lock().unwrap().values().cloned().collect())
        }
    }

    /// In-memory mock implementation of [`IndexStore`] for isolated CQRS unit tests.
    #[derive(Default)]
    struct MockIndexStore {
        indexed: Mutex<BTreeMap<String, KnowledgeEntity>>,
    }

    impl IndexStore for MockIndexStore {
        fn upsert_index(&self, entity: &KnowledgeEntity) -> Result<(), OmkError> {
            self.indexed
                .lock()
                .unwrap()
                .insert(entity.id.clone(), entity.clone());
            Ok(())
        }

        fn delete_index(&self, id: &str) -> Result<(), OmkError> {
            self.indexed.lock().unwrap().remove(id);
            Ok(())
        }

        fn search(
            &self,
            query: Option<&str>,
            _filters: &SearchFilters,
            limit: u32,
        ) -> Result<Vec<SearchResultSnippet>, OmkError> {
            let map = self.indexed.lock().unwrap();
            let q = query.unwrap_or("").to_lowercase();
            let mut out = Vec::new();
            for e in map.values() {
                if q.is_empty()
                    || e.title.to_lowercase().contains(&q)
                    || e.content.to_lowercase().contains(&q)
                {
                    out.push(SearchResultSnippet {
                        id: e.id.clone(),
                        file_path: format!("{}.md", e.id),
                        title: e.title.clone(),
                        snippet: e.content.clone(),
                        source_class: e.source.class.as_str().to_string(),
                        source_type: e.source.source_type.as_str().to_string(),
                        authority: e.authority,
                        tags: e.tags.clone(),
                    });
                }
                if out.len() >= limit as usize {
                    break;
                }
            }
            Ok(out)
        }

        fn list_index_hashes(&self) -> Result<BTreeMap<String, String>, OmkError> {
            Ok(self
                .indexed
                .lock()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.sync.version_hash.clone()))
                .collect())
        }
    }

    #[test]
    fn test_authority_protection_blocks_agent_overwrite_of_user_explicit() {
        let doc_store = Arc::new(MockDocumentStore::default());
        let idx_store = Arc::new(MockIndexStore::default());
        let pipeline = KnowledgePipeline::new(doc_store, idx_store);

        let user_cmd = UpsertCommand {
            id: Some("core-rule".to_string()),
            title: "Core Rule".to_string(),
            content: "Never delete production tables.".to_string(),
            source_class: SourceClass::UserExplicit,
            source_type: SourceType::General,
            provider: "local".to_string(),
            uri: None,
            external_id: None,
            sync_strategy: None,
            authority: None,
            tags: vec!["policy".to_string()],
            author: None,
            state: None,
        };
        let res = pipeline.upsert_knowledge(user_cmd).expect("user upsert");
        assert_eq!(res.status, "created");

        let agent_cmd = UpsertCommand {
            id: Some("core-rule".to_string()),
            title: "Core Rule".to_string(),
            content: "Agent thinks dropping tables is fine.".to_string(),
            source_class: SourceClass::AgentInferred,
            source_type: SourceType::General,
            provider: "agent".to_string(),
            uri: None,
            external_id: None,
            sync_strategy: None,
            authority: None,
            tags: vec![],
            author: None,
            state: None,
        };
        let err = pipeline
            .upsert_knowledge(agent_cmd)
            .expect_err("should block agent overwrite");
        assert!(matches!(err, OmkError::AuthorityViolation(_)));
    }

    #[test]
    fn test_sync_strategies_append_and_state_patch() {
        let doc_store = Arc::new(MockDocumentStore::default());
        let idx_store = Arc::new(MockIndexStore::default());
        let pipeline = KnowledgePipeline::new(doc_store, idx_store);

        // 1. MESSENGER -> APPEND_ONLY
        pipeline
            .upsert_knowledge(UpsertCommand {
                id: Some("slack-thread-1".to_string()),
                title: "Incident Thread".to_string(),
                content: "Initial alert triggered.".to_string(),
                source_class: SourceClass::ExternalSync,
                source_type: SourceType::Messenger,
                provider: "slack".to_string(),
                uri: None,
                external_id: Some("T-1".to_string()),
                sync_strategy: None,
                authority: None,
                tags: vec!["incident".to_string()],
                author: None,
                state: None,
            })
            .expect("first slack msg");

        pipeline
            .upsert_knowledge(UpsertCommand {
                id: Some("slack-thread-1".to_string()),
                title: "Incident Thread".to_string(),
                content: "Root cause identified as DNS timeout.".to_string(),
                source_class: SourceClass::ExternalSync,
                source_type: SourceType::Messenger,
                provider: "slack".to_string(),
                uri: None,
                external_id: Some("T-1".to_string()),
                sync_strategy: None,
                authority: None,
                tags: vec!["dns".to_string()],
                author: None,
                state: None,
            })
            .expect("second slack msg");

        let thread_doc = pipeline.read_knowledge("slack-thread-1").expect("read");
        assert!(thread_doc.content.contains("Initial alert triggered."));
        assert!(thread_doc
            .content
            .contains("Root cause identified as DNS timeout."));
        assert_eq!(thread_doc.tags, vec!["incident", "dns"]);

        // 2. ISSUE_TRACKER -> STATE_PATCH
        pipeline
            .upsert_knowledge(UpsertCommand {
                id: Some("jira-101".to_string()),
                title: "Fix memory leak".to_string(),
                content: "Reported by QA.".to_string(),
                source_class: SourceClass::ExternalSync,
                source_type: SourceType::IssueTracker,
                provider: "jira".to_string(),
                uri: None,
                external_id: Some("JIRA-101".to_string()),
                sync_strategy: None,
                authority: None,
                tags: vec!["bug".to_string()],
                author: None,
                state: Some("Open".to_string()),
            })
            .expect("create issue");

        pipeline
            .upsert_knowledge(UpsertCommand {
                id: Some("jira-101".to_string()),
                title: "Fix memory leak".to_string(),
                content: "Patched in PR #42.".to_string(),
                source_class: SourceClass::ExternalSync,
                source_type: SourceType::IssueTracker,
                provider: "jira".to_string(),
                uri: None,
                external_id: Some("JIRA-101".to_string()),
                sync_strategy: None,
                authority: None,
                tags: vec![],
                author: None,
                state: Some("Closed".to_string()),
            })
            .expect("patch issue");

        let issue_doc = pipeline.read_knowledge("jira-101").expect("read issue");
        assert_eq!(issue_doc.state.as_deref(), Some("Closed"));
        assert!(issue_doc.content.contains("Reported by QA."));
        assert!(issue_doc.content.contains("Patched in PR #42."));
    }

    #[test]
    fn test_reconcile_rebuilds_missing_index_and_prunes_orphans() {
        let doc_store = Arc::new(MockDocumentStore::default());
        let idx_store = Arc::new(MockIndexStore::default());
        let pipeline = KnowledgePipeline::new(doc_store.clone(), idx_store.clone());

        let entity = KnowledgeEntity {
            id: "vault-only-doc".to_string(),
            title: "Vault Only".to_string(),
            source: SourceProvenance {
                class: SourceClass::UserExplicit,
                source_type: SourceType::General,
                provider: "local".to_string(),
                uri: None,
                external_id: None,
            },
            sync: SyncMetadata {
                strategy: SyncStrategy::UpsertFull,
                last_synced_at: "2026-07-17T00:00:00Z".to_string(),
                version_hash: "vhash123".to_string(),
            },
            authority: 1.0,
            tags: vec![],
            author: None,
            state: None,
            created_at: 1,
            content: "Unindexed content".to_string(),
        };
        doc_store.put_document(&entity).unwrap();

        let orphan = KnowledgeEntity {
            id: "orphan-idx".to_string(),
            ..entity.clone()
        };
        idx_store.upsert_index(&orphan).unwrap();

        let report = pipeline.reconcile().expect("reconcile");
        assert_eq!(report.scanned_documents, 1);
        assert_eq!(report.reindexed_count, 1);
        assert_eq!(report.removed_orphans_count, 1);

        let hashes = idx_store.list_index_hashes().unwrap();
        assert!(hashes.contains_key("vault-only-doc"));
        assert!(!hashes.contains_key("orphan-idx"));
    }
}
