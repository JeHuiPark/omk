//! Global Singleton Background Daemon (`omk daemon`, `EPIC-IPC-01` §3.1).
//!
//! Guarantees single-instance execution per Vault directory via OS `flock` + PID file,
//! runs startup Vault-Index reconciliation (`fsck`), listens on a Unix Domain Socket,
//! enforces backpressure (`-32001`), and writes structured JSON logs.

use crate::protocol::{format_rpc_error, McpDispatcher, RpcRequest};
use omk_core::domain::{KnowledgeEntity, OmkError};
use omk_core::index::SqliteFtsIndexStore;
use omk_core::search::KnowledgePipeline;
use omk_core::vault::LocalMarkdownDocumentStore;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::raw::c_int;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const LOCK_EX: c_int = 2;
const LOCK_NB: c_int = 4;
const LOCK_UN: c_int = 8;

extern "C" {
    fn flock(fd: c_int, operation: c_int) -> c_int;
}

/// Configuration paths for a Vault workspace and its associated daemon files.
#[derive(Debug, Clone)]
pub struct DaemonPaths {
    /// Root directory where `.md` knowledge files and `.omk/` metadata reside.
    pub vault_dir: PathBuf,
    /// Internal runtime directory (`<vault_dir>/.omk`).
    pub runtime_dir: PathBuf,
    /// SQLite FTS5 index file path (`<vault_dir>/.omk/index.db`).
    pub db_path: PathBuf,
    /// Unix Domain Socket path (`<vault_dir>/.omk/daemon.sock` or short `/tmp` fallback if path > 95 bytes).
    pub socket_path: PathBuf,
    /// Singleton lock & PID file (`<vault_dir>/.omk/daemon.pid`).
    pub pid_path: PathBuf,
    /// Structured JSON log file (`<vault_dir>/.omk/daemon.log`).
    pub log_path: PathBuf,
}

impl DaemonPaths {
    /// Resolves all daemon runtime paths for a given `vault_dir`.
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if directories cannot be created.
    pub fn resolve<P: AsRef<Path>>(vault_dir: P) -> Result<Self, OmkError> {
        let vdir = vault_dir.as_ref();
        fs::create_dir_all(vdir).map_err(|e| {
            OmkError::InternalError(format!(
                "Failed to create Vault directory '{}': {}",
                vdir.display(),
                e
            ))
        })?;
        let canonical_vault = fs::canonicalize(vdir).unwrap_or_else(|_| vdir.to_path_buf());
        let runtime_dir = canonical_vault.join(".omk");
        fs::create_dir_all(&runtime_dir).map_err(|e| {
            OmkError::InternalError(format!(
                "Failed to create runtime directory '{}': {}",
                runtime_dir.display(),
                e
            ))
        })?;

        let direct_sock = runtime_dir.join("daemon.sock");
        // Unix domain socket paths have a strict length limit (~104 bytes on macOS).
        let socket_path = if direct_sock.to_string_lossy().len() <= 92 {
            direct_sock
        } else {
            let mut hash: u64 = 0xcbf29ce484222325;
            for b in canonical_vault.to_string_lossy().as_bytes() {
                hash ^= u64::from(*b);
                hash = hash.wrapping_mul(0x100000001b3);
            }
            PathBuf::from(format!("/tmp/omk-{:016x}.sock", hash))
        };

        Ok(Self {
            vault_dir: canonical_vault,
            db_path: runtime_dir.join("index.db"),
            pid_path: runtime_dir.join("daemon.pid"),
            log_path: runtime_dir.join("daemon.log"),
            runtime_dir,
            socket_path,
        })
    }
}

/// Exclusive OS-level file lock guard (`flock`) ensuring singleton daemon execution.
pub struct PidLockGuard {
    file: File,
    pid_path: PathBuf,
}

impl PidLockGuard {
    /// Attempts to acquire a non-blocking exclusive `flock` on `pid_path` and writes the current PID.
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if another daemon already holds the lock.
    pub fn acquire(pid_path: &Path) -> Result<Self, OmkError> {
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(pid_path)
            .map_err(|e| {
                OmkError::InternalError(format!(
                    "Failed to open PID lock file '{}': {}",
                    pid_path.display(),
                    e
                ))
            })?;

        let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
        if rc != 0 {
            return Err(OmkError::InternalError(format!(
                "Daemon is already running (lock held on '{}')",
                pid_path.display()
            )));
        }

        let _ = file.set_len(0);
        let _ = writeln!(file, "{}", std::process::id());
        let _ = file.sync_all();

        Ok(Self {
            file,
            pid_path: pid_path.to_path_buf(),
        })
    }
}

impl Drop for PidLockGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = flock(self.file.as_raw_fd(), LOCK_UN);
        }
        let _ = fs::remove_file(&self.pid_path);
    }
}

/// Configuration options for the [`DaemonServer`].
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Resolved paths for the Vault, DB, socket, and logs.
    pub paths: DaemonPaths,
    /// Maximum concurrent in-flight requests before triggering Backpressure (`-32001`).
    pub max_in_flight: usize,
}

impl DaemonConfig {
    /// Creates a default [`DaemonConfig`] for `vault_dir` with `max_in_flight = 64`.
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if resolving paths fails.
    pub fn new<P: AsRef<Path>>(vault_dir: P) -> Result<Self, OmkError> {
        Ok(Self {
            paths: DaemonPaths::resolve(vault_dir)?,
            max_in_flight: 64,
        })
    }
}

/// Global Singleton Daemon managing the [`KnowledgePipeline`] over a Unix Domain Socket.
pub struct DaemonServer {
    config: DaemonConfig,
    dispatcher: Arc<McpDispatcher>,
    in_flight: Arc<AtomicUsize>,
    shutdown: Arc<AtomicBool>,
}

impl DaemonServer {
    /// Initializes the daemon stores, runs startup reconciliation (`fsck`), and constructs the server.
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if store initialization or reconciliation fails.
    pub fn initialize(config: DaemonConfig) -> Result<Self, OmkError> {
        let doc_store = Arc::new(LocalMarkdownDocumentStore::new(&config.paths.vault_dir)?);
        let index_store = Arc::new(SqliteFtsIndexStore::new(&config.paths.db_path)?);
        let pipeline = KnowledgePipeline::new(doc_store, index_store);

        // Startup Auto-Recovery / Reconciliation (docs/architecture.md §5.3)
        let report = pipeline.reconcile()?;
        write_structured_log(
            &config.paths.log_path,
            "INFO",
            "daemon_reconcile_completed",
            &format!(
                "scanned={}, reindexed={}, orphans_removed={}",
                report.scanned_documents, report.reindexed_count, report.removed_orphans_count
            ),
        );

        Ok(Self {
            config,
            dispatcher: Arc::new(McpDispatcher::new(pipeline)),
            in_flight: Arc::new(AtomicUsize::new(0)),
            shutdown: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Returns a clone of the shutdown flag so tests or control signals can stop the listener cleanly.
    pub fn shutdown_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown)
    }

    /// Acquires the singleton PID lock, binds the Unix Domain Socket, and serves incoming proxy connections.
    ///
    /// # Errors
    /// Returns [`OmkError::InternalError`] if the singleton lock is already held or socket binding fails.
    pub fn run_blocking(&self) -> Result<(), OmkError> {
        let _pid_guard = PidLockGuard::acquire(&self.config.paths.pid_path)?;

        if self.config.paths.socket_path.exists() {
            let _ = fs::remove_file(&self.config.paths.socket_path);
        }

        let listener = UnixListener::bind(&self.config.paths.socket_path).map_err(|e| {
            OmkError::InternalError(format!(
                "Failed to bind Unix Domain Socket '{}': {}",
                self.config.paths.socket_path.display(),
                e
            ))
        })?;
        listener.set_nonblocking(true).map_err(|e| {
            OmkError::InternalError(format!("Failed to set nonblocking listener: {}", e))
        })?;

        write_structured_log(
            &self.config.paths.log_path,
            "INFO",
            "daemon_started",
            &format!(
                "pid={}, socket={}",
                std::process::id(),
                self.config.paths.socket_path.display()
            ),
        );

        while !self.shutdown.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _addr)) => {
                    let dispatcher = Arc::clone(&self.dispatcher);
                    let in_flight = Arc::clone(&self.in_flight);
                    let max_in_flight = self.config.max_in_flight;
                    let log_path = self.config.paths.log_path.clone();
                    let shutdown = Arc::clone(&self.shutdown);

                    thread::spawn(move || {
                        handle_client_stream(
                            stream,
                            dispatcher,
                            in_flight,
                            max_in_flight,
                            log_path,
                            shutdown,
                        );
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(15));
                }
                Err(e) => {
                    write_structured_log(
                        &self.config.paths.log_path,
                        "ERROR",
                        "accept_failed",
                        &e.to_string(),
                    );
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }

        let _ = fs::remove_file(&self.config.paths.socket_path);
        write_structured_log(
            &self.config.paths.log_path,
            "INFO",
            "daemon_stopped",
            "graceful shutdown",
        );
        Ok(())
    }
}

fn handle_client_stream(
    stream: UnixStream,
    dispatcher: Arc<McpDispatcher>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: usize,
    log_path: PathBuf,
    shutdown: Arc<AtomicBool>,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(300)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    let reader_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut reader = BufReader::new(reader_stream);
    let mut writer = stream;
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if trimmed.contains("\"method\":\"daemon/shutdown\"")
                    || trimmed.contains("\"method\": \"daemon/shutdown\"")
                {
                    shutdown.store(true, Ordering::SeqCst);
                    let resp = r#"{"jsonrpc":"2.0","id":1,"result":{"status":"shutting_down"}}"#;
                    let _ = writeln!(writer, "{}", resp);
                    let _ = writer.flush();
                    break;
                }

                // Backpressure check (EPIC-IPC-01 REQ-IPC-003: -32001 Server Busy)
                let current = in_flight.fetch_add(1, Ordering::SeqCst);
                let response_opt = if current >= max_in_flight {
                    let req_id = RpcRequest::parse(trimmed).ok().and_then(|r| r.id);
                    let busy_err = OmkError::ServerBusy(format!(
                        "Daemon request queue saturated (in_flight={}, max={})",
                        current + 1,
                        max_in_flight
                    ));
                    write_structured_log(&log_path, "WARN", "backpressure_rejected", busy_err.message());
                    Some(format_rpc_error(req_id, &busy_err))
                } else {
                    dispatcher.handle_raw_line(trimmed)
                };
                in_flight.fetch_sub(1, Ordering::SeqCst);

                if let Some(resp_line) = response_opt {
                    if writeln!(writer, "{}", resp_line).is_err() || writer.flush().is_err() {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
}

/// Appends a single-line structured JSON log entry to `log_path` (`docs/architecture.md` §4.1).
pub fn write_structured_log(log_path: &Path, level: &str, event: &str, detail: &str) {
    let (_, iso) = KnowledgeEntity::current_timestamps();
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let safe_detail = detail.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = writeln!(
            file,
            r#"{{"timestamp":"{}","level":"{}","event":"{}","detail":"{}"}}"#,
            iso, level, event, safe_detail
        );
    }
}
