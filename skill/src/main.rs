//! CLI Entry Point for `omk` (oh my knowledge) (`EPIC-CLI-01`, `EPIC-IPC-01`, `EPIC-OBS-01`).
//!
//! Supports subcommands:
//! - `omk install [--vault <dir>] [--harness <spec>] [--target <dir>] [--json]`:
//!   Automatically detects or targets AI agent harnesses and provisions `SKILL.md` and MCP configs.
//! - `omk status [--vault <dir>] [--json]`:
//!   Read-only diagnostics for the Local Vault, SQLite index, Vault Server (`omk daemon`), harnesses, and observability.
//! - `omk stats [--vault <dir>] [--last <N>] [--slow-ms <ms>] [--reset] [--json]`:
//!   Privacy-preserving performance and agent efficiency analytics (`REQ-OBS-005`).
//! - `omk bench [--docs <N>] [--turns <T>] [--json]`:
//!   Deterministic static invariant (`MET-STATIC-01` .. `08`) and synthetic persona workload benchmark (`REQ-PERF-006`).
//! - `omk reconcile [--vault <dir>] [--json]`:
//!   Runs Vault-Index reconciliation (`fsck`) and prints a summary report.
//! - `omk stop [--vault <dir>] [--json]`:
//!   Gracefully stops the running Vault Server (`omk daemon`) for the Vault.
//! - `omk mcp [--vault <dir>]`:
//!   Runs the Session Proxy (`EPIC-IPC-01`).
//! - `omk daemon [--vault <dir>]`:
//!   Runs the Vault Server (`omk daemon`, `EPIC-IPC-01`).
//! - `omk help` / `--help` / `-h` / `--version` / `-V`:
//!   Displays human-friendly usage documentation or version information.

use omk::agent_prompt::{
    inspect_all_harnesses, install_for_selection, install_skill, HarnessProvisionReport,
    HarnessSelection,
};
use omk::daemon::{
    compute_telemetry_stats, reset_telemetry_logs, run_synthetic_benchmark, BenchmarkReport,
    DaemonConfig, DaemonPaths, DaemonServer, TelemetryStatsReport,
};
use omk::protocol::format_rpc_error;
use omk::proxy::{McpProxy, ProxyConfig};
use omk_core::domain::{sanitize_system_path, JsonValue, OmkError};
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

/// Parsed CLI command and options (`REQ-CLI-001`, `REQ-CLI-002`, `REQ-CLI-005`, `REQ-OBS-005`, `REQ-PERF-006`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subcommand {
    /// `omk install`
    Install,
    /// `omk status`
    Status,
    /// `omk stats`
    Stats,
    /// `omk bench`
    Bench,
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
    /// Optional limit of recent RPC events to analyze in `omk stats` (`--last <N>`).
    pub last_n: Option<usize>,
    /// Slow request anomaly threshold in milliseconds for `omk stats` (`--slow-ms <ms>`, default 50.0).
    pub slow_threshold_ms: f64,
    /// Whether to reset telemetry logs in `omk stats` (`--reset`).
    pub reset_stats: bool,
    /// Number of synthetic documents to seed in `omk bench` (`--docs <N>`, default 3000).
    pub bench_docs: usize,
    /// Number of simulated user turns in `omk bench` (`--turns <T>`, default 80).
    pub bench_turns: usize,
    /// Whether machine-readable single-line JSON output was requested (`--json`).
    pub json_output: bool,
}

/// Strictly parses command-line arguments (`REQ-CLI-001`, `REQ-OBS-005`, `REQ-PERF-006`).
///
/// Rejects unknown subcommands, unknown flags, and flags missing required values.
fn parse_cli_args_from(args: &[String], is_tty: bool) -> Result<CliOptions, String> {
    let mut subcommand: Option<Subcommand> = None;
    let mut vault_dir = default_vault_dir();
    let mut target_dir: Option<PathBuf> = None;
    let mut harness_spec: Option<String> = None;
    let mut last_n: Option<usize> = None;
    let mut slow_threshold_ms: f64 = 50.0;
    let mut reset_stats = false;
    let mut bench_docs: usize = 3000;
    let mut bench_turns: usize = 80;
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
            "--reset" => {
                reset_stats = true;
                i += 1;
            }
            "--docs" => {
                let val = args.get(i + 1).ok_or_else(|| {
                    "Option '--docs' requires a positive integer document count.\n[HINT] Example: omk bench --docs 3000"
                        .to_string()
                })?;
                let parsed = val.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| {
                    format!(
                        "Option '--docs' requires a positive integer, but found '{}'.",
                        val
                    )
                })?;
                bench_docs = parsed;
                i += 2;
            }
            "--turns" => {
                let val = args.get(i + 1).ok_or_else(|| {
                    "Option '--turns' requires a positive integer turn count.\n[HINT] Example: omk bench --turns 80"
                        .to_string()
                })?;
                let parsed = val.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| {
                    format!(
                        "Option '--turns' requires a positive integer, but found '{}'.",
                        val
                    )
                })?;
                bench_turns = parsed;
                i += 2;
            }
            "--last" => {
                let val = args.get(i + 1).ok_or_else(|| {
                    "Option '--last' requires a positive integer count.\n[HINT] Example: omk stats --last 100"
                        .to_string()
                })?;
                let parsed = val.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| {
                    format!(
                        "Option '--last' requires a positive integer, but found '{}'.",
                        val
                    )
                })?;
                last_n = Some(parsed);
                i += 2;
            }
            "--slow-ms" => {
                let val = args.get(i + 1).ok_or_else(|| {
                    "Option '--slow-ms' requires a threshold in milliseconds.\n[HINT] Example: omk stats --slow-ms 25"
                        .to_string()
                })?;
                let parsed = val
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite() && *n >= 0.0)
                    .ok_or_else(|| {
                        format!(
                            "Option '--slow-ms' requires a non-negative number, but found '{}'.",
                            val
                        )
                    })?;
                slow_threshold_ms = parsed;
                i += 2;
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
            _ if arg.starts_with("--docs=") => {
                let val = &arg["--docs=".len()..];
                let parsed = val.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| {
                    format!(
                        "Option '--docs=' requires a positive integer, but found '{}'.",
                        val
                    )
                })?;
                bench_docs = parsed;
                i += 1;
            }
            _ if arg.starts_with("--turns=") => {
                let val = &arg["--turns=".len()..];
                let parsed = val.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| {
                    format!(
                        "Option '--turns=' requires a positive integer, but found '{}'.",
                        val
                    )
                })?;
                bench_turns = parsed;
                i += 1;
            }
            _ if arg.starts_with("--last=") => {
                let val = &arg["--last=".len()..];
                let parsed = val.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| {
                    format!(
                        "Option '--last=' requires a positive integer, but found '{}'.",
                        val
                    )
                })?;
                last_n = Some(parsed);
                i += 1;
            }
            _ if arg.starts_with("--slow-ms=") => {
                let val = &arg["--slow-ms=".len()..];
                let parsed = val
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite() && *n >= 0.0)
                    .ok_or_else(|| {
                        format!(
                            "Option '--slow-ms=' requires a non-negative number, but found '{}'.",
                            val
                        )
                    })?;
                slow_threshold_ms = parsed;
                i += 1;
            }
            _ if arg.starts_with('-') => {
                return Err(format!(
                    "Unknown option '{}'.\n[HINT] Run 'omk --help' to see available commands and options.",
                    arg
                ));
            }
            "install" | "status" | "stats" | "bench" | "reconcile" | "stop" | "mcp"
            | "daemon" | "help" => {
                if subcommand.is_some() {
                    return Err(format!(
                        "Unexpected extra subcommand '{}'. Only one subcommand may be specified.",
                        arg
                    ));
                }
                subcommand = Some(match arg {
                    "install" => Subcommand::Install,
                    "status" => Subcommand::Status,
                    "stats" => Subcommand::Stats,
                    "bench" => Subcommand::Bench,
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
                    "Unknown command '{}'.\n[HINT] Did you mean one of: install, status, stats, bench, reconcile, stop, mcp, daemon, help? Run 'omk --help' for usage.",
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
        // Non-TTY (piped / subprocess) bare `omk` runs the MCP Session Proxy for backwards compatibility
        Subcommand::Mcp { explicit: false }
    };

    Ok(CliOptions {
        subcommand: final_sub,
        vault_dir,
        target_dir,
        harness_spec,
        last_n,
        slow_threshold_ms,
        reset_stats,
        bench_docs,
        bench_turns,
        json_output,
    })
}

fn render_help_text(default_vault: &Path) -> String {
    format!(
        r#"omk (oh my knowledge) v{ver}
Standalone Local Knowledge Vault, Vault Server & Session Proxy for AI Agents

USAGE:
  omk <COMMAND> [OPTIONS]

COMMANDS:
  install     Auto-detect AI agent harnesses and provision SKILL.md + MCP config
  status      Inspect Local Vault, SQLite index, Vault Server, and harnesses
  stats       Analyze execution latency percentiles, stage bottlenecks, and search quality
  bench       Verify deterministic static invariants (MET-STATIC-01..08) & synthetic workload SLAs
  reconcile   Verify and repair Vault Markdown <-> SQLite FTS5 index consistency
  stop        Gracefully stop the running Vault Server (omk daemon) for the Vault
  mcp         Run the Session Proxy (Stdio MCP / JSON-RPC 2.0, used by AI agent harnesses)
  daemon      Run the Vault Server (omk daemon) directly
  help        Display this help message

OPTIONS:
  -H, --harness <SPEC>   Target harness(es) for 'install':
                         auto (default), all, or comma-separated list of:
                         claude-code, claude-desktop, cursor, gemini (or antigravity/agy),
                         codex, opencode, windsurf
  -v, --vault <DIR>      Path to Local Vault directory [default: {vault}]
  -t, --target <DIR>     Custom output directory for SKILL.md & mcp_config.json
      --last <N>         Analyze only the most recent N RPC requests in 'omk stats'
      --slow-ms <MS>     Threshold in ms for slow request anomalies in 'omk stats' [default: 50]
      --reset            Clear local telemetry log files in 'omk stats'
      --docs <N>         Number of synthetic documents to seed in 'omk bench' [default: 3000]
      --turns <T>        Number of simulated user turns in 'omk bench' [default: 80]
      --json             Output machine-readable single-line JSON
  -V, --version          Display version information
  -h, --help             Display help information

EXAMPLES:
  omk install                                   # Auto-detect installed harnesses and provision
  omk install --harness claude-code,cursor      # Provision specific harnesses only
  omk install --harness all                     # Provision all 7 supported harnesses
  omk status                                    # Check Local Vault, Vault Server, and harness status
  omk stats                                     # View P50/P95 latency & stage bottleneck report
  omk bench --docs 3000 --turns 80              # Run 1-year Vault (3000 docs) & 8h day (80 turns) benchmark
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
    let obs_stats = compute_telemetry_stats(&paths, None, 50.0);
    let all_lat = obs_stats
        .latency_by_tool
        .get("all")
        .cloned()
        .unwrap_or_default();

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

        let mut obs_obj = BTreeMap::new();
        obs_obj.insert(
            "total_requests".to_string(),
            JsonValue::Number(obs_stats.total_requests as f64),
        );
        obs_obj.insert(
            "failed_requests".to_string(),
            JsonValue::Number(obs_stats.failed_requests as f64),
        );
        obs_obj.insert(
            "tool_requests".to_string(),
            JsonValue::Number(obs_stats.tool_requests as f64),
        );
        obs_obj.insert(
            "tool_succeeded_requests".to_string(),
            JsonValue::Number(obs_stats.tool_succeeded_requests as f64),
        );
        obs_obj.insert(
            "tool_failed_requests".to_string(),
            JsonValue::Number(obs_stats.tool_failed_requests as f64),
        );
        obs_obj.insert(
            "control_requests".to_string(),
            JsonValue::Number(obs_stats.control_requests as f64),
        );
        obs_obj.insert(
            "control_succeeded_requests".to_string(),
            JsonValue::Number(obs_stats.control_succeeded_requests as f64),
        );
        obs_obj.insert(
            "control_failed_requests".to_string(),
            JsonValue::Number(obs_stats.control_failed_requests as f64),
        );
        obs_obj.insert(
            "cold_starts".to_string(),
            JsonValue::Number(obs_stats.cold_starts as f64),
        );
        obs_obj.insert(
            "p95_latency_ms".to_string(),
            JsonValue::Number(round_f64(all_lat.p95_ms)),
        );

        let mut root = BTreeMap::new();
        root.insert("status".to_string(), JsonValue::Str("ok".to_string()));
        root.insert("vault".to_string(), JsonValue::Object(vault_obj));
        root.insert("daemon".to_string(), JsonValue::Object(daemon_obj));
        root.insert("harnesses".to_string(), JsonValue::Array(harness_arr));
        root.insert("observability".to_string(), JsonValue::Object(obs_obj));
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
        println!("[Vault Server]");
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
        println!("[Observability Summary]");
        println!(
            "  Tool Calls        : {} total ({} ok, {} errors)",
            obs_stats.tool_requests,
            obs_stats.tool_succeeded_requests,
            obs_stats.tool_failed_requests
        );
        println!(
            "  Control RPCs      : {} total ({} ok, {} errors)",
            obs_stats.control_requests,
            obs_stats.control_succeeded_requests,
            obs_stats.control_failed_requests
        );
        println!(
            "  Logged Requests   : {} total ({} ok, {} errors, {} cold starts)",
            obs_stats.total_requests,
            obs_stats.succeeded_requests,
            obs_stats.failed_requests,
            obs_stats.cold_starts
        );
        println!(
            "  Latency (P50/P95) : {:.2} ms / {:.2} ms (run 'omk stats' for detailed breakdown)",
            all_lat.p50_ms, all_lat.p95_ms
        );
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

fn round_f64(val: f64) -> f64 {
    (val * 1000.0).round() / 1000.0
}

fn run_stats_command(
    vault_dir: &Path,
    last_n: Option<usize>,
    slow_threshold_ms: f64,
    reset: bool,
    json_output: bool,
) -> Result<(), OmkError> {
    let paths = DaemonPaths::inspect(vault_dir);
    if reset {
        let removed = reset_telemetry_logs(&paths);
        if json_output {
            println!(
                r#"{{"status":"reset","removed_files":{}}}"#,
                removed
            );
        } else {
            println!(
                "omk v{} - Telemetry Logs Reset Complete (removed {} log file(s)).",
                VERSION, removed
            );
        }
        return Ok(());
    }

    let report = compute_telemetry_stats(&paths, last_n, slow_threshold_ms);
    if json_output {
        println!("{}", format_stats_json(&report));
    } else {
        print!("{}", format_stats_human(&paths, &report, slow_threshold_ms));
    }
    Ok(())
}

fn format_stats_json(report: &TelemetryStatsReport) -> String {
    let mut err_map = BTreeMap::new();
    for (code, cnt) in &report.error_counts {
        err_map.insert(code.to_string(), JsonValue::Number(*cnt as f64));
    }
    let mut tool_err_map = BTreeMap::new();
    for (code, cnt) in &report.tool_error_counts {
        tool_err_map.insert(code.to_string(), JsonValue::Number(*cnt as f64));
    }
    let mut control_err_map = BTreeMap::new();
    for (code, cnt) in &report.control_error_counts {
        control_err_map.insert(code.to_string(), JsonValue::Number(*cnt as f64));
    }

    let mut overview = BTreeMap::new();
    overview.insert(
        "total_requests".to_string(),
        JsonValue::Number(report.total_requests as f64),
    );
    overview.insert(
        "succeeded_requests".to_string(),
        JsonValue::Number(report.succeeded_requests as f64),
    );
    overview.insert(
        "failed_requests".to_string(),
        JsonValue::Number(report.failed_requests as f64),
    );
    overview.insert("error_counts".to_string(), JsonValue::Object(err_map));
    overview.insert(
        "tool_requests".to_string(),
        JsonValue::Number(report.tool_requests as f64),
    );
    overview.insert(
        "tool_succeeded_requests".to_string(),
        JsonValue::Number(report.tool_succeeded_requests as f64),
    );
    overview.insert(
        "tool_failed_requests".to_string(),
        JsonValue::Number(report.tool_failed_requests as f64),
    );
    overview.insert(
        "tool_error_counts".to_string(),
        JsonValue::Object(tool_err_map),
    );
    overview.insert(
        "control_requests".to_string(),
        JsonValue::Number(report.control_requests as f64),
    );
    overview.insert(
        "control_succeeded_requests".to_string(),
        JsonValue::Number(report.control_succeeded_requests as f64),
    );
    overview.insert(
        "control_failed_requests".to_string(),
        JsonValue::Number(report.control_failed_requests as f64),
    );
    overview.insert(
        "control_error_counts".to_string(),
        JsonValue::Object(control_err_map),
    );
    overview.insert(
        "metrics_scope".to_string(),
        JsonValue::Str(report.metrics_scope.clone()),
    );
    overview.insert(
        "daemon_starts".to_string(),
        JsonValue::Number(report.daemon_starts as f64),
    );
    overview.insert(
        "cold_starts".to_string(),
        JsonValue::Number(report.cold_starts as f64),
    );
    overview.insert(
        "backpressure_rejections".to_string(),
        JsonValue::Number(report.backpressure_rejections as f64),
    );
    overview.insert(
        "startup_reconciles".to_string(),
        JsonValue::Number(report.startup_reconciles as f64),
    );
    overview.insert(
        "startup_reconcile_avg_ms".to_string(),
        JsonValue::Number(round_f64(report.startup_reconcile_avg_ms)),
    );
    overview.insert(
        "startup_reconcile_max_ms".to_string(),
        JsonValue::Number(round_f64(report.startup_reconcile_max_ms)),
    );

    let mut latency_obj = BTreeMap::new();
    for (tool, dist) in &report.latency_by_tool {
        let mut d = BTreeMap::new();
        d.insert("count".to_string(), JsonValue::Number(dist.count as f64));
        d.insert("avg_ms".to_string(), JsonValue::Number(round_f64(dist.avg_ms)));
        d.insert("p50_ms".to_string(), JsonValue::Number(round_f64(dist.p50_ms)));
        d.insert("p95_ms".to_string(), JsonValue::Number(round_f64(dist.p95_ms)));
        d.insert("p99_ms".to_string(), JsonValue::Number(round_f64(dist.p99_ms)));
        d.insert("max_ms".to_string(), JsonValue::Number(round_f64(dist.max_ms)));
        latency_obj.insert(tool.clone(), JsonValue::Object(d));
    }

    let mut attr_obj = BTreeMap::new();
    for (tool, stages) in &report.stage_attribution {
        let arr: Vec<JsonValue> = stages
            .iter()
            .map(|s| {
                let mut m = BTreeMap::new();
                m.insert("stage".to_string(), JsonValue::Str(s.stage.clone()));
                m.insert("avg_ms".to_string(), JsonValue::Number(round_f64(s.avg_ms)));
                m.insert(
                    "share_pct".to_string(),
                    JsonValue::Number(round_f64(s.share_pct)),
                );
                JsonValue::Object(m)
            })
            .collect();
        attr_obj.insert(tool.clone(), JsonValue::Array(arr));
    }

    let mut eff_obj = BTreeMap::new();
    eff_obj.insert(
        "total_searches".to_string(),
        JsonValue::Number(report.total_searches as f64),
    );
    eff_obj.insert(
        "zero_hit_searches".to_string(),
        JsonValue::Number(report.zero_hit_searches as f64),
    );
    eff_obj.insert(
        "zero_hit_rate_pct".to_string(),
        JsonValue::Number(round_f64(report.zero_hit_rate_pct)),
    );
    eff_obj.insert(
        "avg_search_results".to_string(),
        JsonValue::Number(round_f64(report.avg_search_results)),
    );
    eff_obj.insert(
        "repeated_query_churn".to_string(),
        JsonValue::Number(report.repeated_query_churn as f64),
    );
    eff_obj.insert(
        "req_bytes_avg".to_string(),
        JsonValue::Number(round_f64(report.req_bytes_avg)),
    );
    eff_obj.insert(
        "resp_bytes_avg".to_string(),
        JsonValue::Number(round_f64(report.resp_bytes_avg)),
    );
    eff_obj.insert(
        "tokens_avg".to_string(),
        JsonValue::Number(round_f64(report.tokens_avg)),
    );
    eff_obj.insert(
        "tokens_p95".to_string(),
        JsonValue::Number(report.tokens_p95 as f64),
    );
    eff_obj.insert(
        "tokens_max".to_string(),
        JsonValue::Number(report.tokens_max as f64),
    );

    let slow_arr: Vec<JsonValue> = report
        .slow_requests
        .iter()
        .map(|s| {
            let mut m = BTreeMap::new();
            m.insert("timestamp".to_string(), JsonValue::Str(s.timestamp.clone()));
            m.insert("trace_id".to_string(), JsonValue::Str(s.trace_id.clone()));
            m.insert("operation".to_string(), JsonValue::Str(s.operation.clone()));
            m.insert(
                "total_ms".to_string(),
                JsonValue::Number(round_f64(s.total_ms)),
            );
            m.insert(
                "dominant_span".to_string(),
                JsonValue::Str(s.dominant_span.clone()),
            );
            m.insert(
                "target_ref".to_string(),
                JsonValue::Str(s.target_ref.clone()),
            );
            JsonValue::Object(m)
        })
        .collect();

    let mut root = BTreeMap::new();
    root.insert("status".to_string(), JsonValue::Str("ok".to_string()));
    root.insert("overview".to_string(), JsonValue::Object(overview));
    root.insert("latency_ms".to_string(), JsonValue::Object(latency_obj));
    root.insert("stage_attribution".to_string(), JsonValue::Object(attr_obj));
    root.insert("agent_efficiency".to_string(), JsonValue::Object(eff_obj));
    root.insert("slow_requests".to_string(), JsonValue::Array(slow_arr));
    JsonValue::Object(root).to_json_string()
}

fn format_stats_human(
    paths: &DaemonPaths,
    report: &TelemetryStatsReport,
    slow_threshold_ms: f64,
) -> String {
    let mut out = String::new();
    let home = std::env::var("HOME").ok();
    let vault_str = paths.vault_dir.to_string_lossy().into_owned();
    let safe_log_path = sanitize_system_path(
        &paths.log_path.to_string_lossy(),
        Some(&vault_str),
        home.as_deref(),
    );

    out.push_str(&format!(
        "omk v{} - Execution Observability & Performance Diagnostics (Privacy-Preserving)\n",
        VERSION
    ));
    out.push_str("===============================================================================\n");
    out.push_str(&format!("  Telemetry Source  : {}\n\n", safe_log_path));

    // Section 1: Overview & Lifecycle Summary
    out.push_str("[1. Overview & Lifecycle Summary]\n");
    out.push_str(&format!(
        "  Knowledge Tool Calls  : {} ({} succeeded, {} failed)\n",
        report.tool_requests, report.tool_succeeded_requests, report.tool_failed_requests
    ));
    out.push_str(&format!(
        "  Session Control RPCs  : {} ({} succeeded, {} failed)\n",
        report.control_requests, report.control_succeeded_requests, report.control_failed_requests
    ));
    out.push_str(&format!(
        "  Total RPC Requests    : {} ({} succeeded, {} failed)\n",
        report.total_requests, report.succeeded_requests, report.failed_requests
    ));
    out.push_str(&format!(
        "  Vault Server Lifecycle: {} starts | {} cold starts (auto-spawn) | {} backpressure rejections\n",
        report.daemon_starts, report.cold_starts, report.backpressure_rejections
    ));
    out.push_str(&format!(
        "  Startup Reconcile     : {} runs (Avg: {:.2} ms, Max: {:.2} ms)\n",
        report.startup_reconciles, report.startup_reconcile_avg_ms, report.startup_reconcile_max_ms
    ));
    if !report.error_counts.is_empty() {
        let errs: Vec<String> = report
            .error_counts
            .iter()
            .map(|(code, cnt)| format!("{}: {}", code, cnt))
            .collect();
        out.push_str(&format!("  Error Code Breakdown  : {}\n", errs.join(", ")));
        if !report.tool_error_counts.is_empty() {
            let tool_errs: Vec<String> = report
                .tool_error_counts
                .iter()
                .map(|(code, cnt)| format!("{}: {}", code, cnt))
                .collect();
            out.push_str(&format!("    - Tool Calls        : {}\n", tool_errs.join(", ")));
        }
        if !report.control_error_counts.is_empty() {
            let ctrl_errs: Vec<String> = report
                .control_error_counts
                .iter()
                .map(|(code, cnt)| format!("{}: {}", code, cnt))
                .collect();
            out.push_str(&format!(
                "    - Session Control   : {}\n",
                ctrl_errs.join(", ")
            ));
        }
    }
    out.push('\n');

    // Section 2: Tool Latency Breakdown
    out.push_str("[2. Tool Latency Breakdown (ms)]\n");
    out.push_str("  Tool               Count     Avg ms     P50 ms     P95 ms     P99 ms     Max ms\n");
    out.push_str("  -------------------------------------------------------------------------------\n");
    for key in [
        "all",
        "search_knowledge",
        "upsert_knowledge",
        "read_knowledge",
        "reconcile",
    ] {
        let d = report.latency_by_tool.get(key).cloned().unwrap_or_default();
        out.push_str(&format!(
            "  {:<18} {:>5} {:>10.2} {:>10.2} {:>10.2} {:>10.2} {:>10.2}\n",
            key, d.count, d.avg_ms, d.p50_ms, d.p95_ms, d.p99_ms, d.max_ms
        ));
    }
    out.push_str("  -------------------------------------------------------------------------------\n");
    let ctrl_d = report
        .latency_by_tool
        .get("control_rpc")
        .cloned()
        .unwrap_or_default();
    out.push_str(&format!(
        "  {:<18} {:>5} {:>10.2} {:>10.2} {:>10.2} {:>10.2} {:>10.2}\n",
        "control_rpc",
        ctrl_d.count,
        ctrl_d.avg_ms,
        ctrl_d.p50_ms,
        ctrl_d.p95_ms,
        ctrl_d.p99_ms,
        ctrl_d.max_ms
    ));
    out.push('\n');

    // Section 3: Stage Bottleneck Attribution
    out.push_str("[3. Stage Bottleneck Attribution]\n");
    for key in ["upsert_knowledge", "search_knowledge", "read_knowledge"] {
        if let Some(stages) = report.stage_attribution.get(key) {
            let parts: Vec<String> = stages
                .iter()
                .map(|s| format!("{}: {:.2}ms ({:.1}%)", s.stage, s.avg_ms, s.share_pct))
                .collect();
            out.push_str(&format!("  * {:<17} -> {}\n", key, parts.join(" | ")));
        }
    }
    out.push('\n');

    // Section 4: Agent Efficiency & Search Quality
    out.push_str("[4. Agent Efficiency & Search Quality]\n");
    out.push_str(&format!(
        "  Search Zero-Hit Rate  : {:.1}% ({} / {} searches returned 0 matches)\n",
        report.zero_hit_rate_pct, report.zero_hit_searches, report.total_searches
    ));
    out.push_str(&format!(
        "  Avg Search Matches    : {:.2} results/search | Repeated Query Churn: {}\n",
        report.avg_search_results, report.repeated_query_churn
    ));
    out.push_str(&format!(
        "  Payload & Tokens      : Req Avg {:.0} B | Resp Avg {:.0} B | Tokens Avg {:.1} / P95 {} / Max {} (scope: {})\n\n",
        report.req_bytes_avg,
        report.resp_bytes_avg,
        report.tokens_avg,
        report.tokens_p95,
        report.tokens_max,
        report.metrics_scope
    ));

    // Section 5: Slow Request Anomalies
    out.push_str(&format!(
        "[5. Slow Request Anomalies (Threshold >= {:.1} ms)]\n",
        slow_threshold_ms
    ));
    if report.slow_requests.is_empty() {
        out.push_str("  None (all observed requests completed below threshold).\n");
    } else {
        for s in &report.slow_requests {
            out.push_str(&format!(
                "  - [{}] {:<16} {:>7.2} ms | Dominant: {:<22} | Ref: {}\n",
                s.timestamp, s.operation, s.total_ms, s.dominant_span, s.target_ref
            ));
        }
    }

    out
}

fn run_bench_command(docs_count: usize, turns_count: usize, json_output: bool) -> Result<(), OmkError> {
    let report = run_synthetic_benchmark(docs_count, turns_count)?;
    if json_output {
        println!("{}", format_bench_json(&report));
    } else {
        print!("{}", format_bench_human(&report));
    }
    if !report.all_invariants_passed {
        return Err(OmkError::InternalError(
            "One or more static invariants (MET-STATIC-01..08) failed during benchmark".to_string(),
        ));
    }
    Ok(())
}

fn format_bench_json(report: &BenchmarkReport) -> String {
    let inv_arr: Vec<JsonValue> = report
        .static_invariants
        .iter()
        .map(|c| {
            let mut m = BTreeMap::new();
            m.insert("id".to_string(), JsonValue::Str(c.id.clone()));
            m.insert("name".to_string(), JsonValue::Str(c.name.clone()));
            m.insert("observed".to_string(), JsonValue::Str(c.observed.clone()));
            m.insert("target".to_string(), JsonValue::Str(c.target.clone()));
            m.insert("passed".to_string(), JsonValue::Bool(c.passed));
            JsonValue::Object(m)
        })
        .collect();

    let encode_dist = |d: &omk::daemon::LatencyDistribution| -> JsonValue {
        let mut m = BTreeMap::new();
        m.insert("count".to_string(), JsonValue::Number(d.count as f64));
        m.insert("avg_ms".to_string(), JsonValue::Number(round_f64(d.avg_ms)));
        m.insert("p50_ms".to_string(), JsonValue::Number(round_f64(d.p50_ms)));
        m.insert("p95_ms".to_string(), JsonValue::Number(round_f64(d.p95_ms)));
        m.insert("p99_ms".to_string(), JsonValue::Number(round_f64(d.p99_ms)));
        m.insert("max_ms".to_string(), JsonValue::Number(round_f64(d.max_ms)));
        JsonValue::Object(m)
    };

    let mut sla_obj = BTreeMap::new();
    sla_obj.insert(
        "search_knowledge".to_string(),
        encode_dist(&report.search_latency),
    );
    sla_obj.insert(
        "read_knowledge".to_string(),
        encode_dist(&report.read_latency),
    );
    sla_obj.insert(
        "upsert_knowledge".to_string(),
        encode_dist(&report.upsert_latency),
    );
    sla_obj.insert(
        "cold_start_ready_ms".to_string(),
        JsonValue::Number(round_f64(report.cold_start_ready_ms)),
    );
    sla_obj.insert(
        "incremental_reconcile_ms".to_string(),
        JsonValue::Number(round_f64(report.incremental_reconcile_ms)),
    );
    sla_obj.insert(
        "full_reconcile_ms".to_string(),
        JsonValue::Number(round_f64(report.full_reconcile_ms)),
    );
    sla_obj.insert(
        "total_workload_ms".to_string(),
        JsonValue::Number(round_f64(report.total_workload_ms)),
    );

    let mut root = BTreeMap::new();
    root.insert(
        "status".to_string(),
        JsonValue::Str(
            if report.all_invariants_passed {
                "ok"
            } else {
                "failed"
            }
            .to_string(),
        ),
    );
    root.insert(
        "seeded_docs".to_string(),
        JsonValue::Number(report.seeded_docs as f64),
    );
    root.insert(
        "simulated_turns".to_string(),
        JsonValue::Number(report.simulated_turns as f64),
    );
    root.insert(
        "total_rpc_calls".to_string(),
        JsonValue::Number(report.total_rpc_calls as f64),
    );
    root.insert(
        "all_invariants_passed".to_string(),
        JsonValue::Bool(report.all_invariants_passed),
    );
    root.insert("static_invariants".to_string(), JsonValue::Array(inv_arr));
    root.insert("workload_slas".to_string(), JsonValue::Object(sla_obj));
    JsonValue::Object(root).to_json_string()
}

fn format_bench_human(report: &BenchmarkReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "omk v{} - Deterministic Static Invariant & Synthetic Persona Workload Benchmark\n",
        VERSION
    ));
    out.push_str("===============================================================================\n");
    out.push_str(&format!(
        "  Workload Profile  : {} Vault Docs | {} User Turns ({} total RPC calls)\n",
        report.seeded_docs, report.simulated_turns, report.total_rpc_calls
    ));
    out.push_str(&format!(
        "  Invariant Status  : {}\n\n",
        if report.all_invariants_passed {
            "ALL 8 STATIC INVARIANTS PASSED (100% Deterministic)"
        } else {
            "FAILED (See invariant details below)"
        }
    ));

    out.push_str("[1. Static / Deterministic Complexity Invariants (MET-STATIC-01 .. 08)]\n");
    for c in &report.static_invariants {
        let badge = if c.passed { "PASS" } else { "FAIL" };
        out.push_str(&format!(
            "  [{}] {:<14} {:<52}\n         Observed: {:<36} | Target: {}\n",
            badge, c.id, c.name, c.observed, c.target
        ));
    }
    out.push('\n');

    out.push_str("[2. Synthetic Persona Workload Latency SLAs (ms)]\n");
    out.push_str("  Operation                 Count     Avg ms     P50 ms     P95 ms     P99 ms     Max ms\n");
    out.push_str("  --------------------------------------------------------------------------------------\n");
    for (label, d) in [
        ("search_knowledge (E2E)", &report.search_latency),
        ("read_knowledge (E2E)", &report.read_latency),
        ("upsert_knowledge (E2E)", &report.upsert_latency),
    ] {
        out.push_str(&format!(
            "  {:<25} {:>5} {:>10.2} {:>10.2} {:>10.2} {:>10.2} {:>10.2}\n",
            label, d.count, d.avg_ms, d.p50_ms, d.p95_ms, d.p99_ms, d.max_ms
        ));
    }
    out.push('\n');
    out.push_str("  Lifecycle & Reconciliation Metrics:\n");
    out.push_str(&format!(
        "    * Vault Server Cold Start Ready Time : {:>8.2} ms (SLA P95 <= 50.0 ms)\n",
        report.cold_start_ready_ms
    ));
    out.push_str(&format!(
        "    * Incremental Reconcile (No-op)      : {:>8.2} ms (SLA P95 <= 30.0 ms)\n",
        report.incremental_reconcile_ms
    ));
    out.push_str(&format!(
        "    * Full Index Rebuild (1 Batched Tx)  : {:>8.2} ms (SLA P95 <= 800.0 ms)\n",
        report.full_reconcile_ms
    ));
    out.push_str(&format!(
        "    * Total Daily Workload ({} Turns)    : {:>8.2} ms (SLA <= 1,000.0 ms / day)\n",
        report.simulated_turns, report.total_workload_ms
    ));
    out
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
                    r#"{{"name":"omk","version":"{}","commands":["install","status","stats","bench","reconcile","stop","mcp","daemon","help"]}}"#,
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
                    "[omk] Starting MCP Session Proxy (waiting for JSON-RPC input on stdin... Press Ctrl+C to exit)"
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
        Subcommand::Stats => run_stats_command(
            &opts.vault_dir,
            opts.last_n,
            opts.slow_threshold_ms,
            opts.reset_stats,
            opts.json_output,
        ),
        Subcommand::Bench => run_bench_command(opts.bench_docs, opts.bench_turns, opts.json_output),
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
                    "Vault Server (omk daemon) stopped gracefully (Vault: {}).",
                    paths.vault_dir.display()
                );
            } else {
                println!(
                    "Vault Server (omk daemon) is not currently running (Vault: {}).",
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

    #[test]
    fn test_cli_parser_accepts_stats_and_options() {
        let args = vec![
            "stats".to_string(),
            "--last".to_string(),
            "25".to_string(),
            "--slow-ms=12.5".to_string(),
            "--reset".to_string(),
            "--json".to_string(),
        ];
        let opts = parse_cli_args_from(&args, true).expect("valid stats args");
        assert_eq!(opts.subcommand, Subcommand::Stats);
        assert_eq!(opts.last_n, Some(25));
        assert_eq!(opts.slow_threshold_ms, 12.5);
        assert!(opts.reset_stats);
        assert!(opts.json_output);
    }

    #[test]
    fn test_cli_parser_accepts_bench_and_options() {
        let args = vec![
            "bench".to_string(),
            "--docs".to_string(),
            "3000".to_string(),
            "--turns=80".to_string(),
            "--json".to_string(),
        ];
        let opts = parse_cli_args_from(&args, true).expect("valid bench args");
        assert_eq!(opts.subcommand, Subcommand::Bench);
        assert_eq!(opts.bench_docs, 3000);
        assert_eq!(opts.bench_turns, 80);
        assert!(opts.json_output);
    }
}
