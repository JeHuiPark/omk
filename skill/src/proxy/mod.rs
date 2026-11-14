//! Lightweight Stdio Proxy (`omk mcp`, `EPIC-IPC-01` §3.1).
//!
//! Forwards JSON-RPC / MCP frames from `stdin` to the global `omk daemon` over a Unix Domain
//! Socket and writes responses to `stdout`. Includes auto-spawning of the background daemon,
//! exponential backoff connection retry, and strict socket deadlines (timeouts) returning
//! `-32000 (Daemon Unreachable)`.

use crate::daemon::DaemonPaths;
use crate::protocol::{format_rpc_error, next_trace_id, RpcRequest};
use omk_core::domain::{JsonValue, OmkError};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// End-to-End proxy span timings captured during a single [`McpProxy`] forwarding operation (`REQ-OBS-001`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyCallTimings {
    /// Unique trace identifier assigned to the forwarded request.
    pub trace_id: String,
    /// Time spent establishing the Unix Domain Socket connection (`us`).
    pub proxy_connect_us: u64,
    /// Whether `omk daemon` was auto-spawned due to a Cold Start.
    pub cold_start: bool,
    /// Number of exponential backoff connection retries performed.
    pub connect_retries: u32,
    /// Total time spent sleeping in exponential backoff (`us`).
    pub backoff_wait_us: u64,
    /// Time spent writing the request to the daemon socket and reading the response (`us`).
    pub ipc_roundtrip_us: u64,
    /// Total end-to-end duration inside the proxy for this line (`us`).
    pub proxy_total_us: u64,
}

/// Configuration for the Stdio [`McpProxy`].
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Resolved daemon/vault paths.
    pub paths: DaemonPaths,
    /// Per-request read/write timeout (default: 30 seconds).
    pub request_timeout: Duration,
    /// Maximum connection attempts using exponential backoff.
    pub max_connect_retries: u32,
    /// Base backoff delay (doubled on each retry).
    pub base_backoff_ms: u64,
    /// Whether to automatically spawn `omk daemon` in the background if the socket is unreachable.
    pub auto_spawn_daemon: bool,
}

impl ProxyConfig {
    /// Creates a default [`ProxyConfig`] for `vault_dir` (30s timeout, 6 retries, auto-spawn enabled).
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if resolving paths fails.
    pub fn new<P: AsRef<Path>>(vault_dir: P) -> Result<Self, OmkError> {
        Ok(Self {
            paths: DaemonPaths::resolve(vault_dir)?,
            request_timeout: Duration::from_secs(30),
            max_connect_retries: 6,
            base_backoff_ms: 25,
            auto_spawn_daemon: true,
        })
    }
}

/// Stdio-to-UDS MCP Proxy client.
pub struct McpProxy {
    config: ProxyConfig,
}

impl McpProxy {
    /// Constructs a new [`McpProxy`] with the provided [`ProxyConfig`].
    pub fn new(config: ProxyConfig) -> Self {
        Self { config }
    }

    /// Connects to the running daemon over Unix Domain Socket, auto-spawning the background
    /// daemon and retrying with exponential backoff if needed (`REQ-IPC-001`, `REQ-IPC-003`).
    ///
    /// # Errors
    /// Returns [`OmkError::DaemonUnreachable`] (`-32000`) if the daemon cannot be reached after all retries.
    pub fn connect_with_backoff(&self) -> Result<UnixStream, OmkError> {
        let (stream, _, _, _, _) = self.connect_with_backoff_timed()?;
        Ok(stream)
    }

    /// Connects to the running daemon and returns `(stream, proxy_connect_us, cold_start, connect_retries, backoff_wait_us)`
    /// (`REQ-OBS-001`).
    ///
    /// # Errors
    /// Returns [`OmkError::DaemonUnreachable`] (`-32000`) if the daemon cannot be reached after all retries.
    pub fn connect_with_backoff_timed(&self) -> Result<(UnixStream, u64, bool, u32, u64), OmkError> {
        let connect_start = Instant::now();
        if let Ok(stream) = self.try_connect_once() {
            let connect_us = connect_start.elapsed().as_micros() as u64;
            return Ok((stream, connect_us, false, 0, 0));
        }

        let mut cold_start = false;
        if self.config.auto_spawn_daemon {
            if self.spawn_background_daemon().is_ok() {
                cold_start = true;
            }
        }

        let mut delay_ms = self.config.base_backoff_ms.max(5);
        let mut backoff_wait_us = 0u64;
        for attempt in 0..self.config.max_connect_retries {
            let sleep_start = Instant::now();
            thread::sleep(Duration::from_millis(delay_ms));
            backoff_wait_us += sleep_start.elapsed().as_micros() as u64;
            if let Ok(stream) = self.try_connect_once() {
                let connect_us = connect_start.elapsed().as_micros() as u64;
                return Ok((stream, connect_us, cold_start, attempt + 1, backoff_wait_us));
            }
            delay_ms = (delay_ms * 2).min(1000);
        }

        Err(OmkError::DaemonUnreachable(format!(
            "Daemon Unreachable: failed to connect to socket '{}' after {} retries",
            self.config.paths.socket_path.display(),
            self.config.max_connect_retries
        )))
    }

    fn try_connect_once(&self) -> Result<UnixStream, OmkError> {
        let stream = UnixStream::connect(&self.config.paths.socket_path).map_err(|e| {
            OmkError::DaemonUnreachable(format!(
                "Daemon socket connection failed ('{}'): {}",
                self.config.paths.socket_path.display(),
                e
            ))
        })?;
        let _ = stream.set_read_timeout(Some(self.config.request_timeout));
        let _ = stream.set_write_timeout(Some(self.config.request_timeout));
        Ok(stream)
    }

    fn spawn_background_daemon(&self) -> Result<(), OmkError> {
        let exe = std::env::current_exe().map_err(|e| {
            OmkError::InternalError(format!("Failed to determine current executable path: {}", e))
        })?;
        Command::new(exe)
            .arg("daemon")
            .arg("--vault")
            .arg(&self.config.paths.vault_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                OmkError::DaemonUnreachable(format!("Failed to auto-spawn omk daemon: {}", e))
            })?;
        Ok(())
    }

    /// Sends a single JSON-RPC line to the daemon and returns the optional response line.
    /// Never panics; if the daemon is unreachable or times out, formats a `-32000` JSON-RPC error response.
    pub fn forward_line(&self, raw_line: &str) -> Option<String> {
        self.forward_line_timed(raw_line).0
    }

    /// Sends a single JSON-RPC line to the daemon and returns both the optional response line
    /// and [`ProxyCallTimings`] (`REQ-OBS-001`).
    pub fn forward_line_timed(&self, raw_line: &str) -> (Option<String>, ProxyCallTimings) {
        let total_start = Instant::now();
        let trace_id = next_trace_id();
        let mut timings = ProxyCallTimings {
            trace_id: trace_id.clone(),
            ..Default::default()
        };

        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            return (None, timings);
        }

        // Validate basic JSON-RPC framing at the edge before forwarding if malformed
        let parsed_req = match RpcRequest::parse(trimmed) {
            Ok(req) => req,
            Err((id, err)) => {
                timings.proxy_total_us = total_start.elapsed().as_micros() as u64;
                return (Some(format_rpc_error(id, &err)), timings);
            }
        };
        let is_notification =
            parsed_req.id.is_none() || parsed_req.method.starts_with("notifications/");
        let req_id = parsed_req.id.clone();

        let mut stream = match self.connect_with_backoff_timed() {
            Ok((s, connect_us, cold_start, retries, backoff_us)) => {
                timings.proxy_connect_us = connect_us;
                timings.cold_start = cold_start;
                timings.connect_retries = retries;
                timings.backoff_wait_us = backoff_us;
                s
            }
            Err(err) => {
                timings.proxy_total_us = total_start.elapsed().as_micros() as u64;
                return if is_notification {
                    (None, timings)
                } else {
                    (Some(format_rpc_error(req_id, &err)), timings)
                };
            }
        };

        let pre_send_us = total_start.elapsed().as_micros() as u64;
        let payload_to_send = inject_proxy_context(
            trimmed,
            &trace_id,
            trimmed.len(),
            timings.proxy_connect_us,
            timings.cold_start,
            timings.connect_retries,
            timings.backoff_wait_us,
            pre_send_us,
        );

        let ipc_start = Instant::now();
        if let Err(e) = writeln!(stream, "{}", payload_to_send).and_then(|_| stream.flush()) {
            timings.ipc_roundtrip_us = ipc_start.elapsed().as_micros() as u64;
            timings.proxy_total_us = total_start.elapsed().as_micros() as u64;
            let err = OmkError::DaemonUnreachable(format!(
                "Daemon Unreachable: failed to write request to daemon socket: {}",
                e
            ));
            return if is_notification {
                (None, timings)
            } else {
                (Some(format_rpc_error(req_id, &err)), timings)
            };
        }

        if is_notification {
            timings.ipc_roundtrip_us = ipc_start.elapsed().as_micros() as u64;
            timings.proxy_total_us = total_start.elapsed().as_micros() as u64;
            return (None, timings);
        }

        let mut reader = BufReader::new(stream);
        let mut resp = String::new();
        let outcome = match reader.read_line(&mut resp) {
            Ok(0) => {
                let err = OmkError::DaemonUnreachable(
                    "Daemon Unreachable: connection closed before response".to_string(),
                );
                Some(format_rpc_error(req_id, &err))
            }
            Ok(_) => Some(resp.trim().to_string()),
            Err(e) => {
                let err = OmkError::DaemonUnreachable(format!(
                    "Daemon Unreachable or Timeout waiting for response: {}",
                    e
                ));
                Some(format_rpc_error(req_id, &err))
            }
        };
        timings.ipc_roundtrip_us = ipc_start.elapsed().as_micros() as u64;
        timings.proxy_total_us = total_start.elapsed().as_micros() as u64;
        (outcome, timings)
    }

    /// Runs the blocking Stdio event loop (`stdin` -> UDS -> `stdout`).
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] only on unrecoverable stdout failure.
    pub fn run_stdio_loop(&self) -> Result<(), OmkError> {
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        let mut line = String::new();
        let mut reader = stdin.lock();

        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    if let Some(resp) = self.forward_line(&line) {
                        writeln!(stdout, "{}", resp).map_err(|e| {
                            OmkError::InternalError(format!("Failed to write to stdout: {}", e))
                        })?;
                        stdout.flush().map_err(|e| {
                            OmkError::InternalError(format!("Failed to flush stdout: {}", e))
                        })?;
                    }
                }
                Err(e) => {
                    let err = OmkError::InternalError(format!("Stdin read error: {}", e));
                    let _ = writeln!(stdout, "{}", format_rpc_error(None, &err));
                    let _ = stdout.flush();
                    break;
                }
            }
        }
        Ok(())
    }
}

fn inject_proxy_context(
    raw_line: &str,
    trace_id: &str,
    req_bytes: usize,
    proxy_connect_us: u64,
    cold_start: bool,
    connect_retries: u32,
    backoff_wait_us: u64,
    proxy_pre_send_us: u64,
) -> String {
    let Ok(JsonValue::Object(mut obj)) = JsonValue::parse(raw_line) else {
        return raw_line.to_string();
    };
    let mut p_map = BTreeMap::new();
    p_map.insert("trace_id".to_string(), JsonValue::Str(trace_id.to_string()));
    p_map.insert(
        "req_bytes".to_string(),
        JsonValue::Number(req_bytes as f64),
    );
    p_map.insert(
        "proxy_connect_us".to_string(),
        JsonValue::Number(proxy_connect_us as f64),
    );
    p_map.insert("cold_start".to_string(), JsonValue::Bool(cold_start));
    p_map.insert(
        "connect_retries".to_string(),
        JsonValue::Number(f64::from(connect_retries)),
    );
    p_map.insert(
        "backoff_wait_us".to_string(),
        JsonValue::Number(backoff_wait_us as f64),
    );
    p_map.insert(
        "proxy_pre_send_us".to_string(),
        JsonValue::Number(proxy_pre_send_us as f64),
    );
    obj.insert("_omk_proxy".to_string(), JsonValue::Object(p_map));
    JsonValue::Object(obj).to_json_string()
}
