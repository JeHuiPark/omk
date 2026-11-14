//! CLI Entry Point for `omk` (oh my knowledge) (`EPIC-CLI-01`, `EPIC-IPC-01`).
//!
//! Supports subcommands:
//! - `omk install [--vault <dir>] [--harness <spec>] [--target <dir>] [--json]`:
//!   Automatically detects or targets AI agent harnesses and provisions `SKILL.md` and MCP configs.
//! - `omk status [--vault <dir>] [--json]`:
//!   Read-only diagnostics for the Local Vault, SQLite index, background daemon, and harnesses.
//! - `omk reconcile [--vault <dir>] [--json]`:
//!   Runs Vault-Index reconciliation (`fsck`) and prints a summary report.
//! - `omk stop [--vault <dir>] [--json]`:
//!   Gracefully stops a running `omk daemon` for the Vault.
//! - `omk mcp [--vault <dir>]`:
//!   Runs the Stdio MCP Proxy (`EPIC-IPC-01`).
//! - `omk daemon [--vault <dir>]`:
//!   Runs the global singleton background daemon (`EPIC-IPC-01`).
//! - `omk help` / `--help` / `-h` / `--version` / `-V`:
//!   Displays human-friendly usage documentation or version information.

use omk::agent_prompt::{
    inspect_all_harnesses, install_for_selection, install_skill, HarnessProvisionReport,
    HarnessSelection,
};
use omk::daemon::{DaemonConfig, DaemonPaths, DaemonServer};
use omk::protocol::format_rpc_error;
use omk::proxy::{McpProxy, ProxyConfig};
use omk_core::domain::{JsonValue, OmkError};
use omk_core::index::{IndexStore, SqliteFtsIndexStore};
use omk_core::search::KnowledgePipeline;
use omk_core::vault::LocalMarkdownDocumentStore;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::raw::c_int;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const VERSION: &str = "0.1.0";

extern "C" {
    fn isatty(fd: c_int) -> c_int;
}

/// Returns `true` if standard input (`fd = 0`) is connected to an interactive terminal (TTY).
fn stdin_is_tty() -> bool {
    unsafe { isatty(0) != 0 }
}

fn default_home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    std::env::temp_dir()
}

fn default_vault_dir() -> PathBuf {
    if let Ok(env_vault) = std::env::var("OMK_VAULT_DIR") {
        if !env_vault.trim().is_empty() {
            return PathBuf::from(env_vault);
        }
    }
    default_home_dir().join(".omk").join("vault")
}

/// Parsed CLI command and options (`REQ-CLI-001`, `REQ-CLI-002`, `REQ-CLI-005`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subcommand {
    /// `omk install`
    Install,
    /// `omk status`
    Status,
    /// `omk reconcile`
    Reconcile,
    /// `omk stop`
    Stop,
    /// `omk mcp`
    Mcp {
        /// Whether `mcp` was explicitly requested on the command line.
        explicit: bool,
    },
    /// `omk daemon`
    Daemon,
    /// `omk help` / `--help` / `-h` (or bare `omk` in an interactive TTY).
    Help,
    /// `omk --version` / `-V`
    Version,
}

/// Validated command-line options.
#[derive(Debug, Clone)]
pub struct CliOptions {
    /// Selected subcommand.
    pub subcommand: Subcommand,
    /// Vault directory path (`--vault` / `-v`).
    pub vault_dir: PathBuf,
    /// Optional custom target directory (`--target` / `-t`).
    pub target_dir: Option<PathBuf>,
    /// Optional harness target specification (`--harness` / `-H`).
    pub harness_spec: Option<String>,
    /// Whether machine-readable single-line JSON output was requested (`--json`).
    pub json_output: bool,
}

/// Strictly parses command-line arguments (`REQ-CLI-001`).
///
/// Rejects unknown subcommands, unknown flags, and flags missing required values.
fn parse_cli_args_from(args: &[String], is_tty: bool) -> Result<CliOptions, String> {
    let mut subcommand: Option<Subcommand> = None;
    let mut vault_dir = default_vault_dir();
    let mut target_dir: Option<PathBuf> = None;
    let mut harness_spec: Option<String> = None;
    let mut json_output = false;
    let mut help_requested = false;
    let mut version_requested = false;

    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--help" | "-h" => {
                help_requested = true;
                i += 1;
            }
            "--version" | "-V" => {
                version_requested = true;
                i += 1;
            }
            "--json" => {
                json_output = true;
                i += 1;
            }
            "--vault" | "-v" => {
                let val = args.get(i + 1).ok_or_else(|| {
                    format!(
                        "Option '{}' requires a directory path.\n[HINT] Example: omk status --vault ~/.omk/vault",
                        arg
                    )
                })?;
                if val.starts_with('-') {
                    return Err(format!(
                        "Option '{}' requires a directory path, but found flag '{}'.",
                        arg, val
                    ));
                }
                vault_dir = PathBuf::from(val);
                i += 2;
            }
            "--target" | "-t" => {
                let val = args.get(i + 1).ok_or_else(|| {
                    format!(
                        "Option '{}' requires a target directory path.\n[HINT] Example: omk install --target ./my-skill",
                        arg
                    )
                })?;
                if val.starts_with('-') {
                    return Err(format!(
                        "Option '{}' requires a directory path, but found flag '{}'.",
                        arg, val
                    ));
                }
                target_dir = Some(PathBuf::from(val));
                i += 2;
            }
            "--harness" | "-H" => {
                let val = args.get(i + 1).ok_or_else(|| {
                    format!(
                        "Option '{}' requires a harness name (auto, all, claude-code, claude-desktop, cursor, gemini, codex, opencode, windsurf).\n[HINT] Example: omk install --harness claude-code,cursor",
                        arg
                    )
                })?;
                if val.starts_with('-') {
                    return Err(format!(
                        "Option '{}' requires a harness name, but found flag '{}'.",
                        arg, val
                    ));
                }
                harness_spec = Some(val.clone());
                i += 2;
            }
            _ if arg.starts_with("--vault=") => {
                let val = &arg["--vault=".len()..];
                if val.trim().is_empty() {
                    return Err("Option '--vault=' cannot be empty.".to_string());
                }
                vault_dir = PathBuf::from(val);
                i += 1;
            }
            _ if arg.starts_with("--target=") => {
                let val = &arg["--target=".len()..];
                if val.trim().is_empty() {
                    return Err("Option '--target=' cannot be empty.".to_string());
                }
                target_dir = Some(PathBuf::from(val));
                i += 1;
            }
            _ if arg.starts_with("--harness=") => {
                let val = &arg["--harness=".len()..];
                if val.trim().is_empty() {
                    return Err("Option '--harness=' cannot be empty.".to_string());
                }
                harness_spec = Some(val.to_string());
                i += 1;
            }
            _ if arg.starts_with('-') => {
                return Err(format!(
                    "Unknown option '{}'.\n[HINT] Run 'omk --help' to see available commands and options.",
                    arg
                ));
            }
            "install" | "status" | "reconcile" | "stop" | "mcp" | "daemon" | "help" => {
                if subcommand.is_some() {
                    return Err(format!(
                        "Unexpected extra subcommand '{}'. Only one subcommand may be specified.",
                        arg
                    ));
                }
                subcommand = Some(match arg {
                    "install" => Subcommand::Install,
                    "status" => Subcommand::Status,
                    "reconcile" => Subcommand::Reconcile,
                    "stop" => Subcommand::Stop,
                    "mcp" => Subcommand::Mcp { explicit: true },
                    "daemon" => Subcommand::Daemon,
                    _ => Subcommand::Help,
                });
                i += 1;
            }
            other => {
                // Allow `omk help <subcommand>` gracefully
                if matches!(subcommand, Some(Subcommand::Help)) {
                    i += 1;
                    continue;
                }
                return Err(format!(
                    "Unknown command '{}'.\n[HINT] Did you mean one of: install, status, reconcile, stop, mcp, daemon, help? Run 'omk --help' for usage.",
                    other
                ));
            }
        }
    }

    let final_sub = if help_requested {
        Subcommand::Help
    } else if version_requested {
        Subcommand::Version
    } else if let Some(sub) = subcommand {
        sub
    } else if is_tty {
        // REQ-CLI-001: Invoking bare `omk` in an interactive terminal shows Help instead of hanging
        Subcommand::Help
    } else {
        // Non-TTY (piped / subprocess) bare `omk` runs the MCP stdio proxy for backwards compatibility
        Subcommand::Mcp { explicit: false }
    };

    Ok(CliOptions {
        subcommand: final_sub,
        vault_dir,
        target_dir,
        harness_spec,
        json_output,
    })
}

fn render_help_text(default_vault: &Path) -> String {
    format!(
        r#"omk (oh my knowledge) v{ver}
Standalone Local Knowledge Vault & MCP Daemon/Proxy for AI Agents

USAGE:
  omk <COMMAND> [OPTIONS]

COMMANDS:
  install     Auto-detect AI agent harnesses and provision SKILL.md + MCP config
  status      Inspect Local Vault, SQLite index, background daemon, and harnesses
  reconcile   Verify and repair Vault Markdown <-> SQLite FTS5 index consistency
  stop        Gracefully stop the running background daemon for the Vault
  mcp         Run the Stdio MCP / JSON-RPC 2.0 proxy (used by AI agent harnesses)
  daemon      Run the global singleton background daemon directly
  help        Display this help message

OPTIONS:
  -H, --harness <SPEC>   Target harness(es) for 'install':
                         auto (default), all, or comma-separated list of:
                         claude-code, claude-desktop, cursor, gemini (or antigravity/agy),
                         codex, opencode, windsurf
  -v, --vault <DIR>      Path to Local Vault directory [default: {vault}]
  -t, --target <DIR>     Custom output directory for SKILL.md & mcp_config.json
      --json             Output machine-readable single-line JSON
  -V, --version          Display version information
  -h, --help             Display help information

EXAMPLES:
  omk install                                   # Auto-detect installed harnesses and provision
  omk install --harness claude-code,cursor      # Provision specific harnesses only
  omk install --harness all                     # Provision all 7 supported harnesses
  omk status                                    # Check Vault, daemon, and harness status
  omk reconcile                                 # Rebuild/sync SQLite index from Vault Markdown
"#,
        ver = VERSION,
        vault = default_vault.display()
    )
}

fn format_install_human(vault_dir: &Path, reports: &[HarnessProvisionReport]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "omk v{} - Skill & MCP Provisioning Complete\n",
        VERSION
    ));
    out.push_str(&format!("  Vault Directory : {}\n\n", vault_dir.display()));
    out.push_str("Provisioned Targets:\n");
    for rep in reports {
        out.push_str(&format!("  * [{}] {}\n", rep.harness, rep.display_name));
        out.push_str(&format!(
            "      Skill Guide : {}\n",
            rep.skill_md_path.display()
        ));
        for cfg in &rep.mcp_config_paths {
            out.push_str(&format!("      MCP Config  : {}\n", cfg.display()));
        }
        for bak in &rep.backup_paths {
            out.push_str(&format!(
                "      Backup (.bak): {} (preserved malformed config)\n",
                bak.display()
            ));
        }
    }
    out.push_str(
        "\nNext Step: Start your AI agent harness and talk naturally, or run 'omk status' to verify.",
    );
    out
}

fn format_install_json(vault_dir: &Path, reports: &[HarnessProvisionReport]) -> String {
    let first_skill = reports
        .first()
        .map(|r| r.skill_md_path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let first_mcp = reports
        .first()
        .and_then(|r| r.mcp_config_paths.first())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    let targets_arr: Vec<JsonValue> = reports
        .iter()
        .map(|r| {
            let mut m = BTreeMap::new();
            m.insert("harness".to_string(), JsonValue::Str(r.harness.clone()));
            m.insert(
                "display_name".to_string(),
                JsonValue::Str(r.display_name.clone()),
            );
            m.insert(
                "skill_md".to_string(),
                JsonValue::Str(r.skill_md_path.to_string_lossy().into_owned()),
            );
            m.insert(
                "mcp_configs".to_string(),
                JsonValue::Array(
                    r.mcp_config_paths
                        .iter()
                        .map(|p| JsonValue::Str(p.to_string_lossy().into_owned()))
                        .collect(),
                ),
            );
            JsonValue::Object(m)
        })
        .collect();

    let mut root = BTreeMap::new();
    root.insert(
        "status".to_string(),
        JsonValue::Str("installed".to_string()),
    );
    root.insert(
        "vault_dir".to_string(),
        JsonValue::Str(vault_dir.to_string_lossy().into_owned()),
    );
    root.insert("skill_md".to_string(), JsonValue::Str(first_skill));
    root.insert("mcp_config".to_string(), JsonValue::Str(first_mcp));
    root.insert("targets".to_string(), JsonValue::Array(targets_arr));
    JsonValue::Object(root).to_json_string()
}

fn count_vault_markdown_files(vault_dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(vault_dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| {
            let p = e.path();
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| !s.starts_with('.') && s.ends_with(".md"))
                    .unwrap_or(false)
        })
        .count()
}

fn count_indexed_documents(db_path: &Path) -> Option<usize> {
    if !db_path.exists() {
        return None;
    }
    let store = SqliteFtsIndexStore::new(db_path).ok()?;
    let hashes = store.list_index_hashes().ok()?;
    Some(hashes.len())
}

fn check_daemon_alive(paths: &DaemonPaths) -> (bool, Option<u32>) {
    let pid_opt = fs::read_to_string(&paths.pid_path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());

    if !paths.socket_path.exists() {
        return (false, pid_opt);
    }

    if let Ok(mut stream) = UnixStream::connect(&paths.socket_path) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        if writeln!(stream, r#"{{"jsonrpc":"2.0","id":1,"method":"ping"}}"#).is_ok()
            && stream.flush().is_ok()
        {
            let mut reader = BufReader::new(stream);
            let mut resp = String::new();
            if reader.read_line(&mut resp).unwrap_or(0) > 0 && resp.contains("\"result\"") {
                return (true, pid_opt);
            }
        }
    }
    (false, pid_opt)
}

fn run_status_command(vault_dir: &Path, json_output: bool) -> Result<(), OmkError> {
    let paths = DaemonPaths::inspect(vault_dir);
    let vault_exists = paths.vault_dir.exists();
    let md_count = count_vault_markdown_files(&paths.vault_dir);
    let idx_count_opt = count_indexed_documents(&paths.db_path);
    let (daemon_running, pid_opt) = check_daemon_alive(&paths);
    let home = default_home_dir();
    let harnesses = inspect_all_harnesses(&home, false);

    if json_output {
        let mut vault_obj = BTreeMap::new();
        vault_obj.insert(
            "path".to_string(),
            JsonValue::Str(paths.vault_dir.to_string_lossy().into_owned()),
        );
        vault_obj.insert("exists".to_string(), JsonValue::Bool(vault_exists));
        vault_obj.insert(
            "markdown_documents".to_string(),
            JsonValue::Number(md_count as f64),
        );
        vault_obj.insert(
            "index_db_exists".to_string(),
            JsonValue::Bool(paths.db_path.exists()),
        );
        vault_obj.insert(
            "indexed_documents".to_string(),
            JsonValue::Number(idx_count_opt.unwrap_or(0) as f64),
        );

        let mut daemon_obj = BTreeMap::new();
        daemon_obj.insert("running".to_string(), JsonValue::Bool(daemon_running));
        daemon_obj.insert(
            "status".to_string(),
            JsonValue::Str(if daemon_running { "running" } else { "stopped" }.to_string()),
        );
        daemon_obj.insert(
            "pid".to_string(),
            pid_opt
                .map(|p| JsonValue::Number(f64::from(p)))
                .unwrap_or(JsonValue::Null),
        );
        daemon_obj.insert(
            "socket_path".to_string(),
            JsonValue::Str(paths.socket_path.to_string_lossy().into_owned()),
        );

        let harness_arr: Vec<JsonValue> = harnesses
            .iter()
            .map(|h| {
                let mut m = BTreeMap::new();
                m.insert("harness".to_string(), JsonValue::Str(h.slug.to_string()));
                m.insert(
                    "display_name".to_string(),
                    JsonValue::Str(h.display_name.to_string()),
                );
                m.insert("detected".to_string(), JsonValue::Bool(h.detected));
                m.insert("configured".to_string(), JsonValue::Bool(h.configured));
                m.insert(
                    "mcp_config".to_string(),
                    JsonValue::Str(h.primary_mcp_config.to_string_lossy().into_owned()),
                );
                m.insert(
                    "skill_md".to_string(),
                    JsonValue::Str(h.skill_md_path.to_string_lossy().into_owned()),
                );
                JsonValue::Object(m)
            })
            .collect();

        let mut root = BTreeMap::new();
        root.insert("status".to_string(), JsonValue::Str("ok".to_string()));
        root.insert("vault".to_string(), JsonValue::Object(vault_obj));
        root.insert("daemon".to_string(), JsonValue::Object(daemon_obj));
        root.insert("harnesses".to_string(), JsonValue::Array(harness_arr));
        println!("{}", JsonValue::Object(root).to_json_string());
    } else {
        println!("omk v{} - System & Harness Status", VERSION);
        println!("========================================");
        println!("[Local Vault]");
        println!(
            "  Path              : {} ({})",
            paths.vault_dir.display(),
            if vault_exists {
                "initialized"
            } else {
                "not created yet"
            }
        );
        println!("  Markdown Docs     : {}", md_count);
        println!(
            "  SQLite FTS5 Index : {}",
            match idx_count_opt {
                Some(n) => format!("{} indexed ({})", n, paths.db_path.display()),
                None => "not initialized yet".to_string(),
            }
        );
        println!();
        println!("[Background Daemon]");
        if daemon_running {
            println!(
                "  State             : Running (PID: {}, Socket: {})",
                pid_opt
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                paths.socket_path.display()
            );
        } else {
            println!(
                "  State             : Stopped (auto-spawns on first MCP request)"
            );
        }
        println!();
        println!("[AI Agent Harnesses]");
        for h in &harnesses {
            let det_str = if h.detected { "Detected" } else { "Not Found" };
            let cfg_str = if h.configured {
                "Configured"
            } else {
                "Not Configured"
            };
            println!(
                "  - {:<16} ({:<20}) : {:<10} | {}",
                h.slug, h.display_name, det_str, cfg_str
            );
        }
    }
    Ok(())
}

fn execute_cli(opts: CliOptions, is_tty: bool) -> Result<(), OmkError> {
    match opts.subcommand {
        Subcommand::Version => {
            if opts.json_output {
                println!(r#"{{"name":"omk","version":"{}"}}"#, VERSION);
            } else {
                println!("omk {}", VERSION);
            }
            Ok(())
        }
        Subcommand::Help => {
            if opts.json_output {
                println!(
                    r#"{{"name":"omk","version":"{}","commands":["install","status","reconcile","stop","mcp","daemon","help"]}}"#,
                    VERSION
                );
            } else {
                print!("{}", render_help_text(&opts.vault_dir));
            }
            Ok(())
        }
        Subcommand::Daemon => {
            let config = DaemonConfig::new(&opts.vault_dir)?;
            let server = DaemonServer::initialize(config)?;
            server.run_blocking()
        }
        Subcommand::Mcp { explicit } => {
            if explicit && is_tty {
                eprintln!(
                    "[omk] Starting MCP stdio proxy (waiting for JSON-RPC input on stdin... Press Ctrl+C to exit)"
                );
            }
            let config = ProxyConfig::new(&opts.vault_dir)?;
            let proxy = McpProxy::new(config);
            proxy.run_stdio_loop()
        }
        Subcommand::Install => {
            let mut reports = Vec::new();
            let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("omk"));

            if let Some(ref custom_dir) = opts.target_dir {
                let res = install_skill(custom_dir, &opts.vault_dir, Some(&bin))?;
                reports.push(HarnessProvisionReport {
                    harness: "custom".to_string(),
                    display_name: "Custom Target Directory".to_string(),
                    skill_md_path: res.skill_md_path,
                    mcp_config_paths: vec![res.mcp_config_path],
                    backup_paths: Vec::new(),
                });
            }

            if opts.target_dir.is_none() || opts.harness_spec.is_some() {
                let selection = match opts.harness_spec {
                    Some(ref raw) => HarnessSelection::parse(raw)?,
                    None => HarnessSelection::Auto,
                };
                let home = default_home_dir();
                let mut harness_reports = install_for_selection(
                    &selection,
                    &home,
                    &opts.vault_dir,
                    Some(&bin),
                    false,
                )?;
                reports.append(&mut harness_reports);
            }

            if opts.json_output {
                println!("{}", format_install_json(&opts.vault_dir, &reports));
            } else {
                println!("{}", format_install_human(&opts.vault_dir, &reports));
            }
            Ok(())
        }
        Subcommand::Status => run_status_command(&opts.vault_dir, opts.json_output),
        Subcommand::Reconcile => {
            let paths = DaemonPaths::resolve(&opts.vault_dir)?;
            let doc_store = Arc::new(LocalMarkdownDocumentStore::new(&paths.vault_dir)?);
            let idx_store = Arc::new(SqliteFtsIndexStore::new(&paths.db_path)?);
            let pipeline = KnowledgePipeline::new(doc_store, idx_store);
            let report = pipeline.reconcile()?;
            if opts.json_output {
                println!(
                    r#"{{"status":"reconciled","scanned_documents":{},"reindexed_count":{},"removed_orphans_count":{}}}"#,
                    report.scanned_documents, report.reindexed_count, report.removed_orphans_count
                );
            } else {
                println!("omk v{} - Vault & Index Reconciliation Complete", VERSION);
                println!("  Vault Directory   : {}", paths.vault_dir.display());
                println!("  Scanned Documents : {}", report.scanned_documents);
                println!("  Reindexed Entries : {}", report.reindexed_count);
                println!("  Removed Orphans   : {}", report.removed_orphans_count);
            }
            Ok(())
        }
        Subcommand::Stop => {
            let paths = DaemonPaths::inspect(&opts.vault_dir);
            let stopped = if paths.socket_path.exists() {
                if let Ok(mut stream) = UnixStream::connect(&paths.socket_path) {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                    let _ = writeln!(
                        stream,
                        r#"{{"jsonrpc":"2.0","id":1,"method":"daemon/shutdown"}}"#
                    );
                    let _ = stream.flush();
                    let mut reader = BufReader::new(stream);
                    let mut resp = String::new();
                    let _ = reader.read_line(&mut resp);
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if opts.json_output {
                if stopped {
                    println!(r#"{{"status":"stopped"}}"#);
                } else {
                    println!(r#"{{"status":"not_running"}}"#);
                }
            } else if stopped {
                println!(
                    "omk daemon stopped gracefully (Vault: {}).",
                    paths.vault_dir.display()
                );
            } else {
                println!(
                    "omk daemon is not currently running (Vault: {}).",
                    paths.vault_dir.display()
                );
            }
            Ok(())
        }
    }
}

fn main() {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let is_tty = stdin_is_tty();

    let opts = match parse_cli_args_from(&raw_args, is_tty) {
        Ok(o) => o,
        Err(msg) => {
            eprintln!("[ERROR] {}", msg);
            std::process::exit(2);
        }
    };

    let json_mode = opts.json_output || matches!(opts.subcommand, Subcommand::Mcp { .. });
    if let Err(err) = execute_cli(opts, is_tty) {
        if json_mode {
            eprintln!("{}", format_rpc_error(None, &err));
        } else {
            eprintln!("[ERROR] {}", err.message());
            eprintln!("[HINT] Run 'omk --help' or 'omk status' to check your configuration.");
        }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bare_omk_in_tty_returns_help_instead_of_hanging_in_mcp() {
        let opts = parse_cli_args_from(&[], true).expect("parse empty in tty");
        assert_eq!(opts.subcommand, Subcommand::Help);

        let non_tty_opts = parse_cli_args_from(&[], false).expect("parse empty in non-tty");
        assert_eq!(non_tty_opts.subcommand, Subcommand::Mcp { explicit: false });
    }

    #[test]
    fn test_strict_cli_parser_rejects_unknown_flags_and_subcommands() {
        let bad_sub = vec!["instal".to_string()];
        let err1 = parse_cli_args_from(&bad_sub, true).expect_err("should reject typo subcommand");
        assert!(err1.contains("Unknown command 'instal'"));

        let bad_flag = vec!["install".to_string(), "--unknown-flag".to_string()];
        let err2 = parse_cli_args_from(&bad_flag, true).expect_err("should reject unknown flag");
        assert!(err2.contains("Unknown option '--unknown-flag'"));

        let missing_val = vec!["install".to_string(), "--harness".to_string()];
        let err3 =
            parse_cli_args_from(&missing_val, true).expect_err("should reject missing flag value");
        assert!(err3.contains("requires a harness name"));
    }

    #[test]
    fn test_cli_parser_accepts_harness_and_json_options() {
        let args = vec![
            "install".to_string(),
            "--harness".to_string(),
            "claude-code,cursor".to_string(),
            "--json".to_string(),
        ];
        let opts = parse_cli_args_from(&args, true).expect("valid install args");
        assert_eq!(opts.subcommand, Subcommand::Install);
        assert_eq!(opts.harness_spec.as_deref(), Some("claude-code,cursor"));
        assert!(opts.json_output);
    }
}
