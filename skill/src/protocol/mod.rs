//! MCP & JSON-RPC 2.0 Protocol Router and Message Formatting (`EPIC-IPC-01`).
//!
//! Supports both standard Model Context Protocol (`initialize`, `notifications/initialized`,
//! `ping`, `tools/list`, `tools/call`) and direct JSON-RPC tool invocations
//! (`upsert_knowledge`, `search_knowledge`, `read_knowledge`, `reconcile`).

use omk_core::domain::{
    JsonValue, KnowledgeEntity, OmkError, SearchFilters, SearchQuery, SourceClass, SourceType,
    SyncStrategy, UpsertCommand,
};
use omk_core::search::KnowledgePipeline;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Represents a parsed JSON-RPC 2.0 request.
#[derive(Debug, Clone)]
pub struct RpcRequest {
    /// Request ID (`null`, number, or string). `None` if the message is a notification.
    pub id: Option<JsonValue>,
    /// Method name.
    pub method: String,
    /// Method parameters object or value.
    pub params: JsonValue,
}

impl RpcRequest {
    /// Parses a raw JSON line into an [`RpcRequest`].
    ///
    /// # Errors
    /// Returns [`OmkError::ParseError`] on malformed JSON or [`OmkError::InvalidRequest`]
    /// if required JSON-RPC 2.0 fields are missing.
    pub fn parse(raw: &str) -> Result<Self, (Option<JsonValue>, OmkError)> {
        let val = JsonValue::parse(raw).map_err(|e| (None, e))?;
        let obj = val.as_object().ok_or_else(|| {
            (
                None,
                OmkError::InvalidRequest("JSON-RPC payload must be a JSON object".to_string()),
            )
        })?;

        let id = obj.get("id").cloned();

        if let Some(ver) = obj.get("jsonrpc") {
            if ver.as_str() != Some("2.0") {
                return Err((
                    id,
                    OmkError::InvalidRequest("Unsupported jsonrpc version; expected '2.0'".to_string()),
                ));
            }
        }

        let method = obj
            .get("method")
            .and_then(JsonValue::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| {
                (
                    id.clone(),
                    OmkError::InvalidRequest("Missing string field 'method'".to_string()),
                )
            })?;

        let params = obj
            .get("params")
            .cloned()
            .unwrap_or_else(|| JsonValue::Object(BTreeMap::new()));

        Ok(Self { id, method, params })
    }

    /// Returns `true` if this request mutates state and must be serialized through the Command Queue.
    pub fn is_write_command(&self) -> bool {
        match self.method.as_str() {
            "upsert_knowledge" | "reconcile" => true,
            "tools/call" => self
                .params
                .get("name")
                .and_then(JsonValue::as_str)
                .map(|name| matches!(name, "upsert_knowledge" | "reconcile"))
                .unwrap_or(false),
            _ => false,
        }
    }
}

/// Formats a successful JSON-RPC 2.0 response string.
pub fn format_rpc_success(id: JsonValue, result: JsonValue) -> String {
    let mut root = BTreeMap::new();
    root.insert("jsonrpc".to_string(), JsonValue::Str("2.0".to_string()));
    root.insert("id".to_string(), id);
    root.insert("result".to_string(), result);
    JsonValue::Object(root).to_json_string()
}

/// Formats a standardized JSON-RPC 2.0 error response string (`REQ-IPC-003`).
pub fn format_rpc_error(id: Option<JsonValue>, err: &OmkError) -> String {
    let mut err_obj = BTreeMap::new();
    err_obj.insert(
        "code".to_string(),
        JsonValue::Number(f64::from(err.rpc_code())),
    );
    err_obj.insert(
        "message".to_string(),
        JsonValue::Str(err.message().to_string()),
    );

    let mut root = BTreeMap::new();
    root.insert("jsonrpc".to_string(), JsonValue::Str("2.0".to_string()));
    root.insert("id".to_string(), id.unwrap_or(JsonValue::Null));
    root.insert("error".to_string(), JsonValue::Object(err_obj));
    JsonValue::Object(root).to_json_string()
}

/// Stateless MCP & JSON-RPC dispatcher backed by [`KnowledgePipeline`] with CQRS write serialization.
pub struct McpDispatcher {
    pipeline: KnowledgePipeline,
    command_lock: Mutex<()>,
}

impl McpDispatcher {
    /// Creates a new [`McpDispatcher`] wrapping the given [`KnowledgePipeline`].
    pub fn new(pipeline: KnowledgePipeline) -> Self {
        Self {
            pipeline,
            command_lock: Mutex::new(()),
        }
    }

    /// Returns a reference to the underlying [`KnowledgePipeline`].
    pub fn pipeline(&self) -> &KnowledgePipeline {
        &self.pipeline
    }

    /// Processes a single raw JSON-RPC line and returns an optional JSON-RPC response line
    /// (`None` for notifications that do not expect a response).
    pub fn handle_raw_line(&self, line: &str) -> Option<String> {
        let req = match RpcRequest::parse(line) {
            Ok(r) => r,
            Err((id, err)) => return Some(format_rpc_error(id, &err)),
        };

        // Notifications (no `id` or `notifications/*`) do not produce a response
        if req.id.is_none() || req.method.starts_with("notifications/") {
            return None;
        }
        let req_id = req.id.clone().unwrap_or(JsonValue::Null);

        let outcome = if req.is_write_command() {
            // CQRS Command Queue: serialize write operations
            let _guard = self.command_lock.lock().ok();
            self.dispatch(&req)
        } else {
            // CQRS Query Workers: execute read operations concurrently
            self.dispatch(&req)
        };

        match outcome {
            Ok(res_val) => Some(format_rpc_success(req_id, res_val)),
            Err(err) => Some(format_rpc_error(Some(req_id), &err)),
        }
    }

    fn dispatch(&self, req: &RpcRequest) -> Result<JsonValue, OmkError> {
        match req.method.as_str() {
            "initialize" => Ok(build_initialize_result()),
            "ping" => Ok(JsonValue::Object(BTreeMap::new())),
            "tools/list" => Ok(build_tools_list_result()),
            "tools/call" => self.handle_tools_call(&req.params),
            "upsert_knowledge" => self.exec_upsert(&req.params),
            "search_knowledge" => self.exec_search(&req.params),
            "read_knowledge" => self.exec_read(&req.params),
            "reconcile" => self.exec_reconcile(),
            other => Err(OmkError::MethodNotFound(format!(
                "Unknown JSON-RPC method '{}'",
                other
            ))),
        }
    }

    fn handle_tools_call(&self, params: &JsonValue) -> Result<JsonValue, OmkError> {
        let tool_name = params
            .get("name")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| {
                OmkError::InvalidParams("Missing string parameter 'name' in tools/call".to_string())
            })?;
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| JsonValue::Object(BTreeMap::new()));

        let structured = match tool_name {
            "upsert_knowledge" => self.exec_upsert(&args)?,
            "search_knowledge" => self.exec_search(&args)?,
            "read_knowledge" => self.exec_read(&args)?,
            "reconcile" => self.exec_reconcile()?,
            other => {
                return Err(OmkError::MethodNotFound(format!(
                    "Unknown MCP tool '{}'",
                    other
                )))
            }
        };

        let text_serialized = structured.to_json_string();
        let mut text_block = BTreeMap::new();
        text_block.insert("type".to_string(), JsonValue::Str("text".to_string()));
        text_block.insert("text".to_string(), JsonValue::Str(text_serialized));

        let mut mcp_res = BTreeMap::new();
        mcp_res.insert(
            "content".to_string(),
            JsonValue::Array(vec![JsonValue::Object(text_block)]),
        );
        mcp_res.insert("structuredContent".to_string(), structured);
        mcp_res.insert("isError".to_string(), JsonValue::Bool(false));
        Ok(JsonValue::Object(mcp_res))
    }

    fn exec_upsert(&self, args: &JsonValue) -> Result<JsonValue, OmkError> {
        let cmd = parse_upsert_command(args)?;
        let res = self.pipeline.upsert_knowledge(cmd)?;
        let mut map = BTreeMap::new();
        map.insert("id".to_string(), JsonValue::Str(res.id));
        map.insert("status".to_string(), JsonValue::Str(res.status));
        map.insert("version_hash".to_string(), JsonValue::Str(res.version_hash));
        map.insert("file_path".to_string(), JsonValue::Str(res.file_path));
        Ok(JsonValue::Object(map))
    }

    fn exec_search(&self, args: &JsonValue) -> Result<JsonValue, OmkError> {
        let query = parse_search_query(args)?;
        let matches = self.pipeline.search_knowledge(query)?;
        let items: Vec<JsonValue> = matches
            .into_iter()
            .map(|m| {
                let mut obj = BTreeMap::new();
                obj.insert("id".to_string(), JsonValue::Str(m.id));
                obj.insert("file_path".to_string(), JsonValue::Str(m.file_path));
                obj.insert("title".to_string(), JsonValue::Str(m.title));
                obj.insert("snippet".to_string(), JsonValue::Str(m.snippet));
                obj.insert("source_class".to_string(), JsonValue::Str(m.source_class));
                obj.insert("source_type".to_string(), JsonValue::Str(m.source_type));
                obj.insert("authority".to_string(), JsonValue::Number(m.authority));
                obj.insert(
                    "tags".to_string(),
                    JsonValue::Array(m.tags.into_iter().map(JsonValue::Str).collect()),
                );
                JsonValue::Object(obj)
            })
            .collect();
        let mut out = BTreeMap::new();
        out.insert("matches".to_string(), JsonValue::Array(items));
        Ok(JsonValue::Object(out))
    }

    fn exec_read(&self, args: &JsonValue) -> Result<JsonValue, OmkError> {
        let id = args
            .get("id")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| {
                OmkError::InvalidParams("Missing required string parameter 'id'".to_string())
            })?;
        let entity = self.pipeline.read_knowledge(id)?;
        Ok(entity_to_json(&entity))
    }

    fn exec_reconcile(&self) -> Result<JsonValue, OmkError> {
        let report = self.pipeline.reconcile()?;
        let mut map = BTreeMap::new();
        map.insert(
            "scanned_documents".to_string(),
            JsonValue::Number(report.scanned_documents as f64),
        );
        map.insert(
            "reindexed_count".to_string(),
            JsonValue::Number(report.reindexed_count as f64),
        );
        map.insert(
            "removed_orphans_count".to_string(),
            JsonValue::Number(report.removed_orphans_count as f64),
        );
        Ok(JsonValue::Object(map))
    }
}

fn entity_to_json(entity: &KnowledgeEntity) -> JsonValue {
    let meta = JsonValue::parse(&entity.metadata_json()).unwrap_or(JsonValue::Null);
    let mut map = BTreeMap::new();
    map.insert("id".to_string(), JsonValue::Str(entity.id.clone()));
    map.insert(
        "file_path".to_string(),
        JsonValue::Str(format!("{}.md", entity.id)),
    );
    map.insert("title".to_string(), JsonValue::Str(entity.title.clone()));
    map.insert(
        "content".to_string(),
        JsonValue::Str(entity.content.clone()),
    );
    map.insert(
        "source_class".to_string(),
        JsonValue::Str(entity.source.class.as_str().to_string()),
    );
    map.insert(
        "source_type".to_string(),
        JsonValue::Str(entity.source.source_type.as_str().to_string()),
    );
    map.insert("authority".to_string(), JsonValue::Number(entity.authority));
    map.insert("metadata".to_string(), meta);
    JsonValue::Object(map)
}

fn parse_upsert_command(args: &JsonValue) -> Result<UpsertCommand, OmkError> {
    let id = args
        .get("id")
        .and_then(JsonValue::as_str)
        .map(ToString::to_string);
    let title = args
        .get("title")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| OmkError::InvalidParams("Missing required parameter 'title'".to_string()))?
        .to_string();
    let content = args
        .get("content")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| OmkError::InvalidParams("Missing required parameter 'content'".to_string()))?
        .to_string();

    let (source_class, source_type, provider, uri, external_id) =
        if let Some(src_obj) = args.get("source") {
            let class_str = src_obj
                .get("class")
                .and_then(JsonValue::as_str)
                .unwrap_or("USER_EXPLICIT");
            let class = SourceClass::from_str(class_str)?;
            let type_str = src_obj
                .get("type")
                .and_then(JsonValue::as_str)
                .unwrap_or("GENERAL");
            let stype = SourceType::from_str(type_str)?;
            let prov = src_obj
                .get("provider")
                .and_then(JsonValue::as_str)
                .unwrap_or("local")
                .to_string();
            let u = src_obj
                .get("uri")
                .and_then(JsonValue::as_str)
                .map(ToString::to_string);
            let ext = src_obj
                .get("external_id")
                .and_then(JsonValue::as_str)
                .map(ToString::to_string);
            (class, stype, prov, u, ext)
        } else {
            let class_str = args
                .get("source_class")
                .and_then(JsonValue::as_str)
                .unwrap_or("USER_EXPLICIT");
            let class = SourceClass::from_str(class_str)?;
            let type_str = args
                .get("source_type")
                .and_then(JsonValue::as_str)
                .unwrap_or("GENERAL");
            let stype = SourceType::from_str(type_str)?;
            let prov = args
                .get("provider")
                .and_then(JsonValue::as_str)
                .unwrap_or("local")
                .to_string();
            let u = args
                .get("uri")
                .and_then(JsonValue::as_str)
                .map(ToString::to_string);
            let ext = args
                .get("external_id")
                .and_then(JsonValue::as_str)
                .map(ToString::to_string);
            (class, stype, prov, u, ext)
        };

    let sync_strategy = match args.get("sync_strategy").and_then(JsonValue::as_str) {
        Some(s) => Some(SyncStrategy::from_str(s)?),
        None => None,
    };

    let authority = args.get("authority").and_then(JsonValue::as_f64);

    let tags = args
        .get("tags")
        .and_then(JsonValue::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default();

    let author = args
        .get("author")
        .and_then(JsonValue::as_str)
        .map(ToString::to_string);
    let state = args
        .get("state")
        .and_then(JsonValue::as_str)
        .map(ToString::to_string);

    Ok(UpsertCommand {
        id,
        title,
        content,
        source_class,
        source_type,
        provider,
        uri,
        external_id,
        sync_strategy,
        authority,
        tags,
        author,
        state,
    })
}

fn parse_search_query(args: &JsonValue) -> Result<SearchQuery, OmkError> {
    let query = args
        .get("query")
        .and_then(JsonValue::as_str)
        .map(ToString::to_string);
    let limit = args
        .get("limit")
        .and_then(JsonValue::as_f64)
        .map(|n| n.max(1.0) as u32)
        .unwrap_or(10);

    let mut filters = SearchFilters::default();
    if let Some(f_obj) = args.get("filters") {
        if let Some(arr) = f_obj.get("tags").and_then(JsonValue::as_array) {
            filters.tags = arr
                .iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect();
        }
        if let Some(sc) = f_obj.get("source_class").and_then(JsonValue::as_str) {
            if !sc.trim().is_empty() {
                filters.source_class = Some(SourceClass::from_str(sc)?);
            }
        }
        if let Some(st) = f_obj.get("source_type").and_then(JsonValue::as_str) {
            if !st.trim().is_empty() {
                filters.source_type = Some(SourceType::from_str(st)?);
            }
        }
        filters.min_authority = f_obj.get("min_authority").and_then(JsonValue::as_f64);
    }

    Ok(SearchQuery {
        query,
        filters,
        limit,
    })
}

fn build_initialize_result() -> JsonValue {
    let raw = r#"{
        "protocolVersion": "2024-11-05",
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "omk", "version": "0.1.0" }
    }"#;
    JsonValue::parse(raw).unwrap_or(JsonValue::Null)
}

fn build_tools_list_result() -> JsonValue {
    let raw = r#"{
        "tools": [
            {
                "name": "upsert_knowledge",
                "description": "Creates or updates knowledge in the Local Vault according to source provenance and sync strategy.",
                "inputSchema": {
                    "type": "object",
                    "required": ["title", "content", "source"],
                    "properties": {
                        "id": { "type": "string" },
                        "title": { "type": "string" },
                        "content": { "type": "string" },
                        "source": {
                            "type": "object",
                            "required": ["class"],
                            "properties": {
                                "class": { "type": "string", "enum": ["USER_EXPLICIT", "AGENT_INFERRED", "EXTERNAL_SYNC"] },
                                "type": { "type": "string", "enum": ["WIKI", "MESSENGER", "ISSUE_TRACKER", "CODE_REPO", "GENERAL"] },
                                "provider": { "type": "string" },
                                "uri": { "type": "string" },
                                "external_id": { "type": "string" }
                            }
                        },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "state": { "type": "string" }
                    }
                }
            },
            {
                "name": "search_knowledge",
                "description": "Searches indexed knowledge using FTS5 full-text search and metadata filters.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "filters": {
                            "type": "object",
                            "properties": {
                                "tags": { "type": "array", "items": { "type": "string" } },
                                "source_class": { "type": "string" },
                                "source_type": { "type": "string" },
                                "min_authority": { "type": "number" }
                            }
                        },
                        "limit": { "type": "integer" }
                    }
                }
            },
            {
                "name": "read_knowledge",
                "description": "Reads a single knowledge document and its complete metadata by ID.",
                "inputSchema": {
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string" }
                    }
                }
            }
        ]
    }"#;
    JsonValue::parse(raw).unwrap_or(JsonValue::Null)
}
