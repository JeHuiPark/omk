//! CLI Entry Point for `omk` (oh my knowledge).
//!
//! Supports subcommands:
//! - `omk mcp [--vault <dir>]` (Default when invoked with no subcommand): Runs the Stdio MCP Proxy.
//! - `omk daemon [--vault <dir>]`: Runs the global singleton background daemon.
//! - `omk install [--vault <dir>] [--target <dir>]`: Automatically provisions the `omk Skill` and global MCP config into the user's AI agent environment.
//! - `omk reconcile [--vault <dir>]`: Runs Vault-Index reconciliation (`fsck`) and prints a JSON report.
//! - `omk stop [--vault <dir>]`: Gracefully stops a running `omk daemon` for the Vault.

use omk::agent_prompt::{install_skill, render_mcp_config_json};
use omk::daemon::{DaemonConfig, DaemonPaths, DaemonServer};
use omk::protocol::format_rpc_error;
use omk::proxy::{McpProxy, ProxyConfig};
use omk_core::domain::OmkError;
use omk_core::index::SqliteFtsIndexStore;
use omk_core::search::KnowledgePipeline;
use omk_core::vault::LocalMarkdownDocumentStore;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn default_vault_dir() -> PathBuf {
    if let Ok(env_vault) = std::env::var("OMK_VAULT_DIR") {
        if !env_vault.trim().is_empty() {
            return PathBuf::from(env_vault);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".omk").join("vault");
    }
    std::env::temp_dir().join("omk-vault")
}

fn default_skill_target_dir(vault_dir: &PathBuf) -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".gemini").join("skills").join("omk");
    }
    vault_dir.join("skill")
}

fn parse_cli_args() -> (String, PathBuf, Option<PathBuf>) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut subcommand = "mcp".to_string();
    let mut vault_dir = default_vault_dir();
    let mut target_dir: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "daemon" | "mcp" | "install" | "reconcile" | "stop" | "help" | "--help" | "-h" => {
                subcommand = args[i].clone();
                i += 1;
            }
            "--vault" | "-v" => {
                if i + 1 < args.len() {
                    vault_dir = PathBuf::from(&args[i + 1]);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "--target" | "-t" => {
                if i + 1 < args.len() {
                    target_dir = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                } else {
                    i += 1;
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    (subcommand, vault_dir, target_dir)
}

fn run() -> Result<(), OmkError> {
    let (subcommand, vault_dir, target_dir) = parse_cli_args();

    match subcommand.as_str() {
        "daemon" => {
            let config = DaemonConfig::new(&vault_dir)?;
            let server = DaemonServer::initialize(config)?;
            server.run_blocking()
        }
        "mcp" => {
            let config = ProxyConfig::new(&vault_dir)?;
            let proxy = McpProxy::new(config);
            proxy.run_stdio_loop()
        }
        "install" => {
            let is_default_target = target_dir.is_none();
            let out_dir = target_dir.unwrap_or_else(|| default_skill_target_dir(&vault_dir));
            let res = install_skill(&out_dir, &vault_dir, None)?;

            // When installing to the user's default environment, also wire ~/.gemini/config/mcp_config.json
            if is_default_target {
                if let Ok(home) = std::env::var("HOME") {
                    let global_cfg_dir = PathBuf::from(home).join(".gemini").join("config");
                    if fs::create_dir_all(&global_cfg_dir).is_ok() {
                        let global_mcp_path = global_cfg_dir.join("mcp_config.json");
                        let existing = fs::read_to_string(&global_mcp_path).unwrap_or_default();
                        if existing.trim().is_empty() || existing.contains("\"omk\"") {
                            let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("omk"));
                            let _ = fs::write(
                                &global_mcp_path,
                                render_mcp_config_json(&bin, &vault_dir),
                            );
                        }
                    }
                }
            }

            println!(
                r#"{{"status":"installed","skill_md":"{}","mcp_config":"{}"}}"#,
                res.skill_md_path.display(),
                res.mcp_config_path.display()
            );
            Ok(())
        }
        "reconcile" => {
            let paths = DaemonPaths::resolve(&vault_dir)?;
            let doc_store = Arc::new(LocalMarkdownDocumentStore::new(&paths.vault_dir)?);
            let idx_store = Arc::new(SqliteFtsIndexStore::new(&paths.db_path)?);
            let pipeline = KnowledgePipeline::new(doc_store, idx_store);
            let report = pipeline.reconcile()?;
            println!(
                r#"{{"status":"reconciled","scanned_documents":{},"reindexed_count":{},"removed_orphans_count":{}}}"#,
                report.scanned_documents, report.reindexed_count, report.removed_orphans_count
            );
            Ok(())
        }
        "stop" => {
            let paths = DaemonPaths::resolve(&vault_dir)?;
            if let Ok(mut stream) = UnixStream::connect(&paths.socket_path) {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                let _ = writeln!(stream, r#"{{"jsonrpc":"2.0","id":1,"method":"daemon/shutdown"}}"#);
                let _ = stream.flush();
                let mut reader = BufReader::new(stream);
                let mut resp = String::new();
                let _ = reader.read_line(&mut resp);
                println!(r#"{{"status":"stopped"}}"#);
            } else {
                println!(r#"{{"status":"not_running"}}"#);
            }
            Ok(())
        }
        _ => {
            println!(
                r#"{{"name":"omk","version":"0.1.0","commands":["mcp","daemon","install","reconcile","stop"]}}"#
            );
            Ok(())
        }
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{}", format_rpc_error(None, &err));
        std::process::exit(1);
    }
}
