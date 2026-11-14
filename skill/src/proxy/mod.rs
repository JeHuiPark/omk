//! Lightweight Stdio Proxy (`omk mcp`, `EPIC-IPC-01` §3.1).
//!
//! Forwards JSON-RPC / MCP frames from `stdin` to the global `omk daemon` over a Unix Domain
//! Socket and writes responses to `stdout`. Includes auto-spawning of the background daemon,
//! exponential backoff connection retry, and strict socket deadlines (timeouts) returning
//! `-32000 (Daemon Unreachable)`.

use crate::daemon::DaemonPaths;
use crate::protocol::{format_rpc_error, RpcRequest};
use omk_core::domain::OmkError;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

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
        if let Ok(stream) = self.try_connect_once() {
            return Ok(stream);
        }

        if self.config.auto_spawn_daemon {
            let _ = self.spawn_background_daemon();
        }

        let mut delay_ms = self.config.base_backoff_ms.max(5);
        for _attempt in 0..self.config.max_connect_retries {
            thread::sleep(Duration::from_millis(delay_ms));
            if let Ok(stream) = self.try_connect_once() {
                return Ok(stream);
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
    ///Never panics; if the daemon is unreachable or times out, formats a `-32000` JSON-RPC error response.
    pub fn forward_line(&self, raw_line: &str) -> Option<String> {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            return None;
        }

        // Validate basic JSON-RPC framing at the edge before forwarding if malformed
        let parsed_req = match RpcRequest::parse(trimmed) {
            Ok(req) => req,
            Err((id, err)) => return Some(format_rpc_error(id, &err)),
        };
        let is_notification =
            parsed_req.id.is_none() || parsed_req.method.starts_with("notifications/");
        let req_id = parsed_req.id.clone();

        let mut stream = match self.connect_with_backoff() {
            Ok(s) => s,
            Err(err) => {
                return if is_notification {
                    None
                } else {
                    Some(format_rpc_error(req_id, &err))
                };
            }
        };

        if let Err(e) = writeln!(stream, "{}", trimmed).and_then(|_| stream.flush()) {
            let err = OmkError::DaemonUnreachable(format!(
                "Daemon Unreachable: failed to write request to daemon socket: {}",
                e
            ));
            return if is_notification {
                None
            } else {
                Some(format_rpc_error(req_id, &err))
            };
        }

        if is_notification {
            return None;
        }

        let mut reader = BufReader::new(stream);
        let mut resp = String::new();
        match reader.read_line(&mut resp) {
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
        }
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
