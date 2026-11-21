//! MCP & JSON-RPC 2.0 Protocol Router and Message Formatting (`EPIC-IPC-01`).
//!
//! Supports both standard Model Context Protocol (`server/discover`, `initialize`,
//! `notifications/initialized`, `ping`, `tools/list`, `resources/list`,
//! `resources/templates/list`, `prompts/list`, `tools/call`) and direct JSON-RPC
//! tool invocations (`upsert_knowledge`, `search_knowledge`, `read_knowledge`, `reconcile`).

use omk_core::domain::{
    estimate_tokens, JsonValue, KnowledgeEntity, OmkError, ReadMetrics, ReadTimings,
    ReconcileTimings, SearchFilters, SearchMetrics, SearchQuery, SearchTimings, SourceClass,
    SourceType, SyncStrategy, UpsertCommand, UpsertMetrics, UpsertTimings,
};
use omk_core::search::{KnowledgePipeline, ReconcileReport};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

static TRACE_SEQ: AtomicU64 = AtomicU64::new(1);

/// Generates a unique 64-bit hexadecimal `trace_id` (`REQ-OBS-001`).
pub fn next_trace_id() -> String {
    let seq = TRACE_SEQ.fetch_add(1, Ordering::Relaxed);
    let (ms, _) = KnowledgeEntity::current_timestamps_ms();
    let pid = u64::from(std::process::id());
    let mixed = (ms as u64)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ pid.rotate_left(21)
        ^ seq.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    format!("{:016x}", mixed)
}

/// Session Proxy connection and retry span metadata attached or recorded per RPC call (`REQ-OBS-001`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxySpanContext {
    /// Trace identifier propagated from the Session Proxy (`omk mcp`) if present.
    pub trace_id: Option<String>,
    /// Original request byte length received on `stdin` before proxy framing.
    pub original_req_bytes: Option<usize>,
    /// Time spent establishing the UDS connection to the Vault Server (`us`).
    pub proxy_connect_us: u64,
    /// Whether `auto_spawn` was triggered due to a Vault Server cold start.
    pub cold_start: bool,
    /// Number of exponential backoff connection retries performed.
    pub connect_retries: u32,
    /// Total time spent sleeping in exponential backoff (`us`).
    pub backoff_wait_us: u64,
    /// Time spent in the proxy prior to writing the frame to the UDS socket (`us`).
    pub proxy_pre_send_us: u64,
}

/// Tool-specific microsecond-level Span timings (`REQ-OBS-001`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolTimings {
    /// Non-pipeline control/lifecycle RPC method (`server/discover`, `initialize`, `ping`, `tools/list`, etc.).
    None,
    /// `upsert_knowledge` spans.
    Upsert(UpsertTimings),
    /// `search_knowledge` spans.
    Search(SearchTimings),
    /// `read_knowledge` spans.
    Read(ReadTimings),
    /// `reconcile` spans.
    Reconcile(ReconcileTimings),
}

/// Tool-specific quantitative metrics (`REQ-OBS-002`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolMetrics {
    /// Non-pipeline RPC method.
    None,
    /// `upsert_knowledge` metrics.
    Upsert(UpsertMetrics),
    /// `search_knowledge` metrics.
    Search(SearchMetrics),
    /// `read_knowledge` metrics.
    Read(ReadMetrics),
    /// `reconcile` summary metrics.
    Reconcile(ReconcileReport),
}

/// Complete telemetry observation captured for a single RPC invocation (`REQ-OBS-001`, `REQ-OBS-002`).
#[derive(Debug, Clone)]
pub struct RpcDispatchObservation {
    /// Unique trace ID for this request.
    pub trace_id: String,
    /// Top-level JSON-RPC method name (e.g., `"tools/call"`, `"search_knowledge"`, `"initialize"`).
    pub rpc_method: String,
    /// Resolved MCP tool name (`"upsert_knowledge"`, `"search_knowledge"`, `"read_knowledge"`, `"reconcile"`).
    pub tool_name: Option<String>,
    /// Whether this request mutates Vault/Index state via the Command Queue.
    pub is_write: bool,
    /// `"ok"` or `"error"`.
    pub status: String,
    /// Standardized JSON-RPC error code on failure (`-32700` .. `-32004`).
    pub error_code: Option<i32>,
    /// Privacy-safe error classification (`ParseError`, `NotFound`, `AuthorityViolation`, etc.).
    pub error_kind: Option<String>,
    /// Byte length of the incoming JSON-RPC request line.
    pub req_bytes: usize,
    /// Byte length of the outgoing JSON-RPC response line.
    pub resp_bytes: usize,
    /// UTF-8 byte length of the knowledge content/snippets processed.
    pub content_bytes: usize,
    /// Estimated LLM token count (`ceil(content_bytes / 4)`).
    pub estimated_tokens: usize,
    /// Raw document ID if applicable (to be salted-hashed by the logger under `REQ-OBS-003`).
    pub raw_doc_id: Option<String>,
    /// Raw search query if applicable (to be salted-hashed by the logger under `REQ-OBS-003`).
    pub raw_query: Option<String>,
    /// Proxy-side span context.
    pub proxy: ProxySpanContext,
    /// Time spent parsing the JSON-RPC frame (`us`).
    pub rpc_parse_us: u64,
    /// Time spent waiting for the CQRS `command_lock` mutex (`us`).
    pub lock_wait_us: u64,
    /// Total execution time inside the Vault Server dispatcher (`us`).
    pub daemon_total_us: u64,
    /// Sub-stage timings inside `omk_core`.
    pub tool_timings: ToolTimings,
    /// Quantitative tool metrics.
    pub tool_metrics: ToolMetrics,
}

/// Represents a parsed JSON-RPC 2.0 request.
#[derive(Debug, Clone)]
pub struct RpcRequest {
    /// Request ID (`null`, number, or string). `None` if the message is a notification.
    pub id: Option<JsonValue>,
    /// Method name.
    pub method: String,
    /// Method parameters object or value.
    pub params: JsonValue,
    /// Optional proxy span metadata extracted from `_omk_proxy`.
    pub proxy_context: ProxySpanContext,
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

        let mut proxy_context = ProxySpanContext::default();
        if let Some(p_obj) = obj.get("_omk_proxy").and_then(JsonValue::as_object) {
            proxy_context.trace_id = p_obj
                .get("trace_id")
                .and_then(JsonValue::as_str)
                .map(ToString::to_string);
            proxy_context.original_req_bytes = p_obj
                .get("req_bytes")
                .and_then(JsonValue::as_f64)
                .map(|n| n.max(0.0) as usize);
            proxy_context.proxy_connect_us = p_obj
                .get("proxy_connect_us")
                .and_then(JsonValue::as_f64)
                .map(|n| n.max(0.0) as u64)
                .unwrap_or(0);
            proxy_context.cold_start = p_obj
                .get("cold_start")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            proxy_context.connect_retries = p_obj
                .get("connect_retries")
                .and_then(JsonValue::as_f64)
                .map(|n| n.max(0.0) as u32)
                .unwrap_or(0);
            proxy_context.backoff_wait_us = p_obj
                .get("backoff_wait_us")
                .and_then(JsonValue::as_f64)
                .map(|n| n.max(0.0) as u64)
                .unwrap_or(0);
            proxy_context.proxy_pre_send_us = p_obj
                .get("proxy_pre_send_us")
                .and_then(JsonValue::as_f64)
                .map(|n| n.max(0.0) as u64)
                .unwrap_or(0);
        }

        Ok(Self {
            id,
            method,
            params,
            proxy_context,
        })
    }

    /// Returns the resolved tool name if this request invokes one of the core `omk` tools.
    pub fn resolved_tool_name(&self) -> Option<String> {
        match self.method.as_str() {
            "upsert_knowledge" | "search_knowledge" | "read_knowledge" | "reconcile" => {
                Some(self.method.clone())
            }
            "tools/call" => self
                .params
                .get("name")
                .and_then(JsonValue::as_str)
                .map(ToString::to_string),
            _ => None,
        }
    }

    /// Returns the arguments object for tool execution (`params.arguments` for `tools/call`, or `params` directly).
    pub fn tool_arguments(&self) -> JsonValue {
        if self.method == "tools/call" {
            self.params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| JsonValue::Object(BTreeMap::new()))
        } else {
            self.params.clone()
        }
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
        self.handle_raw_line_observed(line).0
    }

    /// Processes a single raw JSON-RPC line and returns both the optional JSON-RPC response line
    /// and a detailed [`RpcDispatchObservation`] (`REQ-OBS-001`, `REQ-OBS-002`).
    pub fn handle_raw_line_observed(
        &self,
        line: &str,
    ) -> (Option<String>, Option<RpcDispatchObservation>) {
        let daemon_start = Instant::now();
        let parse_start = Instant::now();
        let req = match RpcRequest::parse(line) {
            Ok(r) => r,
            Err((id, err)) => {
                let rpc_parse_us = parse_start.elapsed().as_micros() as u64;
                let resp = format_rpc_error(id, &err);
                let daemon_total_us = daemon_start.elapsed().as_micros() as u64;
                let obs = RpcDispatchObservation {
                    trace_id: next_trace_id(),
                    rpc_method: "invalid".to_string(),
                    tool_name: None,
                    is_write: false,
                    status: "error".to_string(),
                    error_code: Some(err.rpc_code()),
                    error_kind: Some(err.kind_name().to_string()),
                    req_bytes: line.len(),
                    resp_bytes: resp.len(),
                    content_bytes: 0,
                    estimated_tokens: 0,
                    raw_doc_id: None,
                    raw_query: None,
                    proxy: ProxySpanContext::default(),
                    rpc_parse_us,
                    lock_wait_us: 0,
                    daemon_total_us,
                    tool_timings: ToolTimings::None,
                    tool_metrics: ToolMetrics::None,
                };
                return (Some(resp), Some(obs));
            }
        };
        let rpc_parse_us = parse_start.elapsed().as_micros() as u64;

        // Notifications (no `id` or `notifications/*`) do not produce a response
        if req.id.is_none() || req.method.starts_with("notifications/") {
            return (None, None);
        }
        let req_id = req.id.clone().unwrap_or(JsonValue::Null);
        let is_write = req.is_write_command();
        let tool_name = req.resolved_tool_name();
        let trace_id = req
            .proxy_context
            .trace_id
            .clone()
            .unwrap_or_else(next_trace_id);
        let req_bytes = req.proxy_context.original_req_bytes.unwrap_or(line.len());

        // Pre-extract raw doc ID or query if present so even failed calls can correlate safely
        let tool_args = req.tool_arguments();
        let pre_doc_id = tool_args
            .get("id")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);
        let pre_query = tool_args
            .get("query")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);

        let (lock_wait_us, outcome) = if is_write {
            // CQRS Command Queue: serialize write operations and measure lock contention (`REQ-OBS-001`)
            let lock_start = Instant::now();
            let _guard = self.command_lock.lock().ok();
            let wait_us = lock_start.elapsed().as_micros() as u64;
            (wait_us, self.dispatch_observed(&req))
        } else {
            // CQRS Query Workers: execute read operations concurrently
            (0u64, self.dispatch_observed(&req))
        };

        match outcome {
            Ok((res_val, timings, metrics, content_bytes, doc_id_opt, query_opt)) => {
                let resp_str = format_rpc_success(req_id, res_val);
                let daemon_total_us = daemon_start.elapsed().as_micros() as u64;
                let obs = RpcDispatchObservation {
                    trace_id,
                    rpc_method: req.method,
                    tool_name,
                    is_write,
                    status: "ok".to_string(),
                    error_code: None,
                    error_kind: None,
                    req_bytes,
                    resp_bytes: resp_str.len(),
                    content_bytes,
                    estimated_tokens: estimate_tokens(content_bytes),
                    raw_doc_id: doc_id_opt.or(pre_doc_id),
                    raw_query: query_opt.or(pre_query),
                    proxy: req.proxy_context,
                    rpc_parse_us,
                    lock_wait_us,
                    daemon_total_us,
                    tool_timings: timings,
                    tool_metrics: metrics,
                };
                (Some(resp_str), Some(obs))
            }
            Err(err) => {
                let resp_str = format_rpc_error(Some(req_id), &err);
                let daemon_total_us = daemon_start.elapsed().as_micros() as u64;
                let failed_metrics = if tool_name.as_deref() == Some("read_knowledge") {
                    ToolMetrics::Read(ReadMetrics {
                        raw_doc_id: pre_doc_id.clone().unwrap_or_default(),
                        found: false,
                        tag_count: 0,
                        source_class: "UNKNOWN".to_string(),
                        source_type: "UNKNOWN".to_string(),
                        content_bytes: 0,
                        estimated_tokens: 0,
                    })
                } else {
                    ToolMetrics::None
                };
                let obs = RpcDispatchObservation {
                    trace_id,
                    rpc_method: req.method,
                    tool_name,
                    is_write,
                    status: "error".to_string(),
                    error_code: Some(err.rpc_code()),
                    error_kind: Some(err.kind_name().to_string()),
                    req_bytes,
                    resp_bytes: resp_str.len(),
                    content_bytes: 0,
                    estimated_tokens: 0,
                    raw_doc_id: pre_doc_id,
                    raw_query: pre_query,
                    proxy: req.proxy_context,
                    rpc_parse_us,
                    lock_wait_us,
                    daemon_total_us,
                    tool_timings: ToolTimings::None,
                    tool_metrics: failed_metrics,
                };
                (Some(resp_str), Some(obs))
            }
        }
    }

    fn dispatch_observed(
        &self,
        req: &RpcRequest,
    ) -> Result<
        (
            JsonValue,
            ToolTimings,
            ToolMetrics,
            usize,
            Option<String>,
            Option<String>,
        ),
        OmkError,
    > {
        match req.method.as_str() {
            "server/discover" => Ok((
                build_discover_result(),
                ToolTimings::None,
                ToolMetrics::None,
                0,
                None,
                None,
            )),
            "initialize" => Ok((
                build_initialize_result(),
                ToolTimings::None,
                ToolMetrics::None,
                0,
                None,
                None,
            )),
            "ping" => Ok((
                JsonValue::Object(BTreeMap::new()),
                ToolTimings::None,
                ToolMetrics::None,
                0,
                None,
                None,
            )),
            "tools/list" => Ok((
                build_tools_list_result(),
                ToolTimings::None,
                ToolMetrics::None,
                0,
                None,
                None,
            )),
            "resources/list" => Ok((
                build_empty_list_result("resources"),
                ToolTimings::None,
                ToolMetrics::None,
                0,
                None,
                None,
            )),
            "resources/templates/list" => Ok((
                build_empty_list_result("resourceTemplates"),
                ToolTimings::None,
                ToolMetrics::None,
                0,
                None,
                None,
            )),
            "prompts/list" => Ok((
                build_empty_list_result("prompts"),
                ToolTimings::None,
                ToolMetrics::None,
                0,
                None,
                None,
            )),
            "tools/call" => self.handle_tools_call_observed(&req.params),
            "upsert_knowledge" => self.exec_upsert_observed(&req.params),
            "search_knowledge" => self.exec_search_observed(&req.params),
            "read_knowledge" => self.exec_read_observed(&req.params),
            "reconcile" => self.exec_reconcile_observed(),
            other => Err(OmkError::MethodNotFound(format!(
                "Unknown JSON-RPC method '{}'",
                other
            ))),
        }
    }

    fn handle_tools_call_observed(
        &self,
        params: &JsonValue,
    ) -> Result<
        (
            JsonValue,
            ToolTimings,
            ToolMetrics,
            usize,
            Option<String>,
            Option<String>,
        ),
        OmkError,
    > {
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

        let (structured, timings, metrics, content_bytes, doc_id_opt, query_opt) = match tool_name {
            "upsert_knowledge" => self.exec_upsert_observed(&args)?,
            "search_knowledge" => self.exec_search_observed(&args)?,
            "read_knowledge" => self.exec_read_observed(&args)?,
            "reconcile" => self.exec_reconcile_observed()?,
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
        Ok((
            JsonValue::Object(mcp_res),
            timings,
            metrics,
            content_bytes,
            doc_id_opt,
            query_opt,
        ))
    }

    fn exec_upsert_observed(
        &self,
        args: &JsonValue,
    ) -> Result<
        (
            JsonValue,
            ToolTimings,
            ToolMetrics,
            usize,
            Option<String>,
            Option<String>,
        ),
        OmkError,
    > {
        let cmd = parse_upsert_command(args)?;
        let (res, timings, metrics) = self.pipeline.upsert_knowledge_timed(cmd)?;
        let c_bytes = metrics.content_bytes;
        let doc_id = Some(res.id.clone());
        let mut map = BTreeMap::new();
        map.insert("id".to_string(), JsonValue::Str(res.id));
        map.insert("status".to_string(), JsonValue::Str(res.status));
        map.insert("version_hash".to_string(), JsonValue::Str(res.version_hash));
        map.insert("file_path".to_string(), JsonValue::Str(res.file_path));
        Ok((
            JsonValue::Object(map),
            ToolTimings::Upsert(timings),
            ToolMetrics::Upsert(metrics),
            c_bytes,
            doc_id,
            None,
        ))
    }

    fn exec_search_observed(
        &self,
        args: &JsonValue,
    ) -> Result<
        (
            JsonValue,
            ToolTimings,
            ToolMetrics,
            usize,
            Option<String>,
            Option<String>,
        ),
        OmkError,
    > {
        let query = parse_search_query(args)?;
        let (matches, timings, metrics) = self.pipeline.search_knowledge_timed(query)?;
        let c_bytes = metrics.content_bytes;
        let raw_q = metrics.raw_query.clone();
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
        Ok((
            JsonValue::Object(out),
            ToolTimings::Search(timings),
            ToolMetrics::Search(metrics),
            c_bytes,
            None,
            raw_q,
        ))
    }

    fn exec_read_observed(
        &self,
        args: &JsonValue,
    ) -> Result<
        (
            JsonValue,
            ToolTimings,
            ToolMetrics,
            usize,
            Option<String>,
            Option<String>,
        ),
        OmkError,
    > {
        let id = args
            .get("id")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| {
                OmkError::InvalidParams("Missing required string parameter 'id'".to_string())
            })?;
        let (entity, timings, metrics) = self.pipeline.read_knowledge_timed(id)?;
        let c_bytes = metrics.content_bytes;
        let doc_id = Some(entity.id.clone());
        Ok((
            entity_to_json(&entity),
            ToolTimings::Read(timings),
            ToolMetrics::Read(metrics),
            c_bytes,
            doc_id,
            None,
        ))
    }

    fn exec_reconcile_observed(
        &self,
    ) -> Result<
        (
            JsonValue,
            ToolTimings,
            ToolMetrics,
            usize,
            Option<String>,
            Option<String>,
        ),
        OmkError,
    > {
        let (report, timings) = self.pipeline.reconcile_timed()?;
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
        Ok((
            JsonValue::Object(map),
            ToolTimings::Reconcile(timings),
            ToolMetrics::Reconcile(report),
            0,
            None,
            None,
        ))
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

const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

fn build_initialize_result() -> JsonValue {
    let mut server_info = BTreeMap::new();
    server_info.insert("name".to_string(), JsonValue::Str("omk".to_string()));
    server_info.insert(
        "version".to_string(),
        JsonValue::Str(SERVER_VERSION.to_string()),
    );

    let mut capabilities = BTreeMap::new();
    capabilities.insert("tools".to_string(), JsonValue::Object(BTreeMap::new()));

    let mut root = BTreeMap::new();
    root.insert(
        "protocolVersion".to_string(),
        JsonValue::Str("2024-11-05".to_string()),
    );
    root.insert("capabilities".to_string(), JsonValue::Object(capabilities));
    root.insert("serverInfo".to_string(), JsonValue::Object(server_info));
    JsonValue::Object(root)
}

fn build_discover_result() -> JsonValue {
    let mut map = match build_initialize_result() {
        JsonValue::Object(m) => m,
        _ => BTreeMap::new(),
    };
    let tools = build_tools_list_result()
        .get("tools")
        .cloned()
        .unwrap_or_else(|| JsonValue::Array(Vec::new()));
    map.insert("tools".to_string(), tools);
    JsonValue::Object(map)
}

fn build_empty_list_result(key: &str) -> JsonValue {
    let mut map = BTreeMap::new();
    map.insert(key.to_string(), JsonValue::Array(Vec::new()));
    JsonValue::Object(map)
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
