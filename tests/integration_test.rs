//! End-to-End Integration Tests across Vault File System, SQLite FTS5 Index,
//! Global Singleton Daemon, Stdio MCP Proxy, and CLI Multi-Harness UX (`EPIC-VAULT-01`,
//! `EPIC-DATA-01`, `EPIC-SEARCH-01`, `EPIC-IPC-01`, `EPIC-CLI-01`).

use omk::agent_prompt::{
    detect_installed_harnesses, inspect_all_harnesses, install_for_selection, install_skill,
    merge_mcp_config_file, HarnessKind, HarnessSelection,
};
use omk::daemon::{
    compute_telemetry_stats, write_structured_log_with_limits, DaemonConfig, DaemonServer,
    PidLockGuard,
};
use omk::proxy::{McpProxy, ProxyConfig};
use omk_core::domain::JsonValue;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::thread;

fn unique_test_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "omk_int_{}_{}_{}",
        label,
        std::process::id(),
        nanos
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

#[test]
fn test_singleton_pid_lock_prevents_duplicate_daemon() {
    let vault_dir = unique_test_dir("pidlock");
    let cfg = DaemonConfig::new(&vault_dir).expect("config");

    let _guard1 = PidLockGuard::acquire(&cfg.paths.pid_path).expect("first lock should succeed");
    let second = PidLockGuard::acquire(&cfg.paths.pid_path);
    assert!(
        second.is_err(),
        "Second daemon instance must be blocked by OS flock on daemon.pid"
    );

    drop(_guard1);
    let _guard3 =
        PidLockGuard::acquire(&cfg.paths.pid_path).expect("lock should succeed after drop");
    let _ = fs::remove_dir_all(&vault_dir);
}

#[test]
fn test_proxy_returns_32000_when_daemon_unreachable() {
    let vault_dir = unique_test_dir("unreachable");
    let mut proxy_cfg = ProxyConfig::new(&vault_dir).expect("proxy config");
    proxy_cfg.auto_spawn_daemon = false;
    proxy_cfg.max_connect_retries = 2;
    proxy_cfg.base_backoff_ms = 5;

    let proxy = McpProxy::new(proxy_cfg);
    let resp_raw = proxy
        .forward_line(r#"{"jsonrpc":"2.0","id":99,"method":"ping"}"#)
        .expect("should return error response");
    let parsed = JsonValue::parse(&resp_raw).expect("valid json");
    let code = parsed
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(JsonValue::as_f64);
    assert_eq!(code, Some(-32000.0));

    let _ = fs::remove_dir_all(&vault_dir);
}

#[test]
fn test_end_to_end_daemon_proxy_mcp_and_cqrs_pipelines() {
    let vault_dir = unique_test_dir("e2e");
    let daemon_cfg = DaemonConfig::new(&vault_dir).expect("daemon config");
    let server = DaemonServer::initialize(daemon_cfg.clone()).expect("init daemon");
    let shutdown = server.shutdown_handle();

    let server_thread = thread::spawn(move || {
        server.run_blocking().expect("daemon run_blocking");
    });

    // Configure proxy pointing to the same Vault
    let mut proxy_cfg = ProxyConfig::new(&vault_dir).expect("proxy config");
    proxy_cfg.auto_spawn_daemon = false;
    proxy_cfg.max_connect_retries = 20;
    proxy_cfg.base_backoff_ms = 10;
    let proxy = McpProxy::new(proxy_cfg);

    // 1. MCP Initialize Handshake (REQ-IPC-001 / REQ-IPC-002)
    let init_resp = proxy
        .forward_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
        .expect("initialize response");
    let init_json = JsonValue::parse(&init_resp).expect("parse init json");
    assert_eq!(
        init_json
            .get("result")
            .and_then(|r| r.get("serverInfo"))
            .and_then(|s| s.get("name"))
            .and_then(JsonValue::as_str),
        Some("omk")
    );

    // 2. Tools list
    let list_resp = proxy
        .forward_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
        .expect("tools/list response");
    let list_json = JsonValue::parse(&list_resp).expect("parse list json");
    let tools_arr = list_json
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(JsonValue::as_array)
        .expect("tools array");
    assert_eq!(tools_arr.len(), 3);

    // 3. Upsert Knowledge via MCP tools/call (USER_EXPLICIT)
    let upsert_req = r#"{
        "jsonrpc":"2.0",
        "id":3,
        "method":"tools/call",
        "params":{
            "name":"upsert_knowledge",
            "arguments":{
                "id":"arch-cqrs-01",
                "title":"CQRS Storage Isolation Policy",
                "content":"DocumentStore is the SSOT Markdown Vault and IndexStore is SQLite FTS5.",
                "source":{"class":"USER_EXPLICIT","type":"WIKI","provider":"confluence"},
                "tags":["architecture","cqrs","storage"]
            }
        }
    }"#;
    let upsert_resp = proxy
        .forward_line(&upsert_req.replace('\n', " "))
        .expect("upsert response");
    let upsert_json = JsonValue::parse(&upsert_resp).expect("parse upsert");
    let status = upsert_json
        .get("result")
        .and_then(|r| r.get("structuredContent"))
        .and_then(|s| s.get("status"))
        .and_then(JsonValue::as_str);
    assert_eq!(status, Some("created"));

    // Verify physical .md file exists in Vault with YAML Frontmatter (REQ-VAULT-001)
    let md_path = daemon_cfg.paths.vault_dir.join("arch-cqrs-01.md");
    assert!(md_path.exists(), "Markdown file must exist on disk");
    let md_raw = fs::read_to_string(&md_path).expect("read md");
    assert!(md_raw.starts_with("---\n"));
    assert!(md_raw.contains("class: \"USER_EXPLICIT\""));

    // 4. Authority Violation Check (-32002): AGENT_INFERRED cannot overwrite USER_EXPLICIT
    let bad_overwrite = r#"{
        "jsonrpc":"2.0",
        "id":4,
        "method":"upsert_knowledge",
        "params":{
            "id":"arch-cqrs-01",
            "title":"CQRS Storage Isolation Policy",
            "content":"Hallucinated overwrite",
            "source":{"class":"AGENT_INFERRED","type":"GENERAL","provider":"agent"}
        }
    }"#;
    let bad_resp = proxy
        .forward_line(&bad_overwrite.replace('\n', " "))
        .expect("authority violation response");
    let bad_json = JsonValue::parse(&bad_resp).expect("parse error");
    assert_eq!(
        bad_json
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(JsonValue::as_f64),
        Some(-32002.0)
    );

    // 5. Path Traversal Protection (-32003, REQ-VAULT-002)
    let traversal_req = r#"{
        "jsonrpc":"2.0",
        "id":5,
        "method":"read_knowledge",
        "params":{"id":"../../etc/passwd"}
    }"#;
    let trav_resp = proxy
        .forward_line(&traversal_req.replace('\n', " "))
        .expect("traversal error");
    let trav_json = JsonValue::parse(&trav_resp).expect("parse traversal");
    assert_eq!(
        trav_json
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(JsonValue::as_f64),
        Some(-32003.0)
    );

    // 6. Search Knowledge via FTS5 + Tag Filter (REQ-SEARCH-001, REQ-SEARCH-002)
    let search_req = r#"{
        "jsonrpc":"2.0",
        "id":6,
        "method":"tools/call",
        "params":{
            "name":"search_knowledge",
            "arguments":{
                "query":"DocumentStore AND SQLite",
                "filters":{"tags":["cqrs"],"source_class":"USER_EXPLICIT"},
                "limit":5
            }
        }
    }"#;
    let search_resp = proxy
        .forward_line(&search_req.replace('\n', " "))
        .expect("search response");
    let search_json = JsonValue::parse(&search_resp).expect("parse search");
    let matches = search_json
        .get("result")
        .and_then(|r| r.get("structuredContent"))
        .and_then(|s| s.get("matches"))
        .and_then(JsonValue::as_array)
        .expect("matches array");
    assert_eq!(matches.len(), 1);
    assert_eq!(
        matches[0].get("id").and_then(JsonValue::as_str),
        Some("arch-cqrs-01")
    );

    // 7. Read Knowledge by ID (REQ-VAULT-002)
    let read_req = r#"{
        "jsonrpc":"2.0",
        "id":7,
        "method":"read_knowledge",
        "params":{"id":"arch-cqrs-01"}
    }"#;
    let read_resp = proxy
        .forward_line(&read_req.replace('\n', " "))
        .expect("read response");
    let read_json = JsonValue::parse(&read_resp).expect("parse read");
    assert_eq!(
        read_json
            .get("result")
            .and_then(|r| r.get("title"))
            .and_then(JsonValue::as_str),
        Some("CQRS Storage Isolation Policy")
    );

    // Shutdown daemon cleanly
    shutdown.store(true, Ordering::SeqCst);
    server_thread.join().expect("daemon thread join");

    let _ = fs::remove_dir_all(&vault_dir);
}

#[test]
fn test_skill_provisioning_creates_skill_md_and_mcp_config() {
    let base_dir = unique_test_dir("provision");
    let vault_dir = base_dir.join("vault");
    let target_dir = base_dir.join("skill_out");

    let res = install_skill(&target_dir, &vault_dir, None).expect("install skill");
    assert!(res.skill_md_path.exists());
    assert!(res.mcp_config_path.exists());

    let skill_md = fs::read_to_string(&res.skill_md_path).expect("read SKILL.md");
    assert!(skill_md.contains("Proactive Grounding"));
    assert!(skill_md.contains("search_knowledge"));

    let _ = fs::remove_dir_all(&base_dir);
}

#[test]
fn test_multi_harness_auto_detection_and_non_destructive_merge() {
    let home = unique_test_dir("multi_harness");
    let vault_dir = home.join(".omk").join("vault");
    let bin = PathBuf::from("/usr/local/bin/omk");

    // Simulate user having Claude Code (with an existing MCP server) and Cursor installed
    let claude_cfg = home.join(".claude.json");
    fs::write(
        &claude_cfg,
        r#"{"userTheme":"solarized","mcpServers":{"filesystem":{"command":"npx","args":["-y","server-fs"]}}}"#,
    )
    .expect("seed .claude.json");
    fs::create_dir_all(home.join(".cursor")).expect("seed .cursor");

    let detected = detect_installed_harnesses(&home, false);
    assert_eq!(detected, vec![HarnessKind::ClaudeCode, HarnessKind::Cursor]);

    // 1. Auto-detect install (REQ-CLI-004)
    let reports = install_for_selection(
        &HarnessSelection::Auto,
        &home,
        &vault_dir,
        Some(&bin),
        false,
    )
    .expect("auto install");
    assert_eq!(reports.len(), 2);

    // Verify non-destructive merge in .claude.json
    let claude_after = fs::read_to_string(&claude_cfg).expect("read merged .claude.json");
    let claude_json = JsonValue::parse(&claude_after).expect("parse merged .claude.json");
    assert_eq!(
        claude_json.get("userTheme").and_then(JsonValue::as_str),
        Some("solarized")
    );
    let servers = claude_json.get("mcpServers").expect("mcpServers object");
    assert!(
        servers.get("filesystem").is_some(),
        "Existing 'filesystem' MCP server must be preserved"
    );
    assert!(
        servers.get("omk").is_some(),
        "New 'omk' MCP server must be added"
    );
    assert!(home.join(".claude/skills/omk/SKILL.md").exists());
    assert!(home.join(".cursor/mcp.json").exists());
    assert!(home.join(".cursor/skills/omk/SKILL.md").exists());

    // 2. Explicit --harness all (REQ-CLI-005)
    let all_reports = install_for_selection(
        &HarnessSelection::All,
        &home,
        &vault_dir,
        Some(&bin),
        false,
    )
    .expect("install all harnesses");
    assert_eq!(all_reports.len(), 7);

    let statuses = inspect_all_harnesses(&home, false);
    assert_eq!(statuses.len(), 7);
    assert!(
        statuses.iter().all(|s| s.configured),
        "All 7 harnesses must be marked as Configured after --harness all"
    );

    let _ = fs::remove_dir_all(&home);
}

#[test]
fn test_cli_binary_ux_help_status_install_and_strict_error_handling() {
    let base_dir = unique_test_dir("cli_bin");
    let fake_home = base_dir.join("home");
    let vault_dir = base_dir.join("vault");
    fs::create_dir_all(fake_home.join(".codex")).expect("create fake .codex");

    let bin = env!("CARGO_BIN_EXE_omk");

    // 1. `omk --help` prints human-readable usage and exits 0 (REQ-CLI-001)
    let help_out = Command::new(bin)
        .arg("--help")
        .env("HOME", &fake_home)
        .output()
        .expect("run omk --help");
    assert!(help_out.status.success());
    let help_stdout = String::from_utf8_lossy(&help_out.stdout);
    assert!(help_stdout.contains("USAGE:"));
    assert!(help_stdout.contains("COMMANDS:"));
    assert!(help_stdout.contains("--harness"));

    // 2. Invalid subcommand or flag exits with code 2 and helpful error (REQ-CLI-001)
    let bad_out = Command::new(bin)
        .arg("unknown-subcommand")
        .env("HOME", &fake_home)
        .output()
        .expect("run omk unknown-subcommand");
    assert!(!bad_out.status.success());
    let bad_stderr = String::from_utf8_lossy(&bad_out.stderr);
    assert!(bad_stderr.contains("[ERROR]"));
    assert!(bad_stderr.contains("[HINT]"));

    // 3. `omk install --harness codex,windsurf` provisions both in human-friendly mode (REQ-CLI-002, REQ-CLI-005)
    let inst_out = Command::new(bin)
        .args([
            "install",
            "--vault",
            vault_dir.to_str().unwrap(),
            "--harness",
            "codex,windsurf",
        ])
        .env("HOME", &fake_home)
        .output()
        .expect("run omk install --harness");
    assert!(inst_out.status.success());
    let inst_stdout = String::from_utf8_lossy(&inst_out.stdout);
    assert!(inst_stdout.contains("Provisioned Targets:"));
    assert!(inst_stdout.contains("[codex]"));
    assert!(inst_stdout.contains("[windsurf]"));
    assert!(fake_home.join(".codex/mcp_config.json").exists());
    assert!(fake_home.join(".codeium/windsurf/mcp_config.json").exists());

    // 4. `omk status` (Human-readable) & `omk status --json` (Machine JSON) (REQ-CLI-003)
    let status_out = Command::new(bin)
        .args(["status", "--vault", vault_dir.to_str().unwrap()])
        .env("HOME", &fake_home)
        .output()
        .expect("run omk status");
    assert!(status_out.status.success());
    let status_stdout = String::from_utf8_lossy(&status_out.stdout);
    assert!(status_stdout.contains("[Local Vault]"));
    assert!(status_stdout.contains("[Background Daemon]"));
    assert!(status_stdout.contains("[AI Agent Harnesses]"));

    let status_json_out = Command::new(bin)
        .args(["status", "--vault", vault_dir.to_str().unwrap(), "--json"])
        .env("HOME", &fake_home)
        .output()
        .expect("run omk status --json");
    assert!(status_json_out.status.success());
    let status_json_raw = String::from_utf8_lossy(&status_json_out.stdout);
    let parsed_status = JsonValue::parse(status_json_raw.trim()).expect("valid status json");
    assert_eq!(
        parsed_status.get("status").and_then(JsonValue::as_str),
        Some("ok")
    );
    let harnesses_arr = parsed_status
        .get("harnesses")
        .and_then(JsonValue::as_array)
        .expect("harnesses array");
    assert_eq!(harnesses_arr.len(), 7);

    let _ = fs::remove_dir_all(&base_dir);
}

#[test]
fn test_merge_mcp_config_preserves_malformed_file_as_bak() {
    let dir = unique_test_dir("bak");
    let cfg = dir.join("mcp_config.json");
    fs::write(&cfg, "{broken:json").unwrap();

    let bak = merge_mcp_config_file(&cfg, &PathBuf::from("omk"), &dir.join("vault"))
        .expect("merge")
        .expect("bak path");
    assert!(bak.exists());
    assert_eq!(fs::read_to_string(&bak).unwrap(), "{broken:json");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_observability_spans_zero_pii_leak_and_salted_hashing() {
    let vault_dir = unique_test_dir("obs_privacy");
    let daemon_cfg = DaemonConfig::new(&vault_dir).expect("daemon config");
    let server = DaemonServer::initialize(daemon_cfg.clone()).expect("init daemon");
    let shutdown = server.shutdown_handle();

    // Verify telemetry.salt was created with 0600 permissions (REQ-OBS-003)
    assert!(daemon_cfg.paths.salt_path.exists());
    let salt_mode = fs::metadata(&daemon_cfg.paths.salt_path)
        .expect("salt metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(salt_mode, 0o600);

    let server_thread = thread::spawn(move || {
        server.run_blocking().expect("daemon run_blocking");
    });

    let mut proxy_cfg = ProxyConfig::new(&vault_dir).expect("proxy config");
    proxy_cfg.auto_spawn_daemon = false;
    proxy_cfg.max_connect_retries = 20;
    proxy_cfg.base_backoff_ms = 10;
    let proxy = McpProxy::new(proxy_cfg);

    // 1. Upsert with sensitive canary strings (REQ-OBS-001, REQ-OBS-002, REQ-OBS-003)
    let upsert_req = r#"{
        "jsonrpc":"2.0",
        "id":10,
        "method":"tools/call",
        "params":{
            "name":"upsert_knowledge",
            "arguments":{
                "id":"secret-doc-id-777",
                "title":"PRIVATE_TITLE_999",
                "content":"SECRET_PII_CONTENT_12345 with confidential details.",
                "source":{
                    "class":"USER_EXPLICIT",
                    "type":"WIKI",
                    "provider":"confluence",
                    "uri":"https://internal.secret.corp/pages/999",
                    "external_id":"CONF-SECRET-999"
                },
                "tags":["confidential-tag-xyz"],
                "author":"secret_author@company.com"
            }
        }
    }"#;
    let (upsert_resp, proxy_timings) =
        proxy.forward_line_timed(&upsert_req.replace('\n', " "));
    assert!(upsert_resp.is_some());
    assert!(proxy_timings.proxy_total_us >= proxy_timings.ipc_roundtrip_us);
    assert!(proxy_timings.proxy_connect_us > 0);

    // 2. Search with sensitive query token returning 0 hits twice (Zero-hit & Repeated Query Churn)
    let search_zero_req = r#"{
        "jsonrpc":"2.0",
        "id":11,
        "method":"tools/call",
        "params":{
            "name":"search_knowledge",
            "arguments":{
                "query":"SENSITIVE_QUERY_TOKEN AND nonexistent_keyword",
                "filters":{"tags":["confidential-tag-xyz"]},
                "limit":5
            }
        }
    }"#;
    let _ = proxy
        .forward_line(&search_zero_req.replace('\n', " "))
        .expect("zero-hit search 1");
    let _ = proxy
        .forward_line(&search_zero_req.replace('\n', " "))
        .expect("zero-hit search 2 (churn)");

    // 3. Search with hit
    let search_hit_req = r#"{
        "jsonrpc":"2.0",
        "id":12,
        "method":"search_knowledge",
        "params":{"query":"confidential","limit":5}
    }"#;
    let _ = proxy
        .forward_line(search_hit_req)
        .expect("hit search");

    // 4. Read knowledge (hit + not-found error with sensitive ID)
    let read_ok_req = r#"{
        "jsonrpc":"2.0",
        "id":13,
        "method":"read_knowledge",
        "params":{"id":"secret-doc-id-777"}
    }"#;
    let _ = proxy.forward_line(read_ok_req).expect("read ok");

    let read_err_req = r#"{
        "jsonrpc":"2.0",
        "id":14,
        "method":"read_knowledge",
        "params":{"id":"nonexistent-secret-id-888"}
    }"#;
    let _ = proxy.forward_line(read_err_req).expect("read error");

    shutdown.store(true, Ordering::SeqCst);
    server_thread.join().expect("daemon thread join");

    // Verify daemon.log exists, has 0600 permissions, and contains ZERO canary strings
    let raw_log = fs::read_to_string(&daemon_cfg.paths.log_path).expect("read daemon.log");
    let canaries = [
        "SECRET_PII_CONTENT_12345",
        "PRIVATE_TITLE_999",
        "SENSITIVE_QUERY_TOKEN",
        "secret_author@company.com",
        "secret-doc-id-777",
        "https://internal.secret.corp/pages/999",
        "CONF-SECRET-999",
        "confidential-tag-xyz",
        "nonexistent-secret-id-888",
    ];
    for canary in canaries {
        assert!(
            !raw_log.contains(canary),
            "Zero-PII violation: daemon.log leaked sensitive canary '{}'",
            canary
        );
    }

    // Verify salted hashes (doc# and qry#) and microsecond span fields exist in daemon.log
    assert!(raw_log.contains("\"doc_ref\":\"doc#"));
    assert!(raw_log.contains("\"query_ref\":\"qry#"));
    assert!(raw_log.contains("\"validate_read_us\":"));
    assert!(raw_log.contains("\"vault_write_us\":"));
    assert!(raw_log.contains("\"vault_fsync_us\":"));
    assert!(raw_log.contains("\"index_write_us\":"));
    assert!(raw_log.contains("\"fts_build_us\":"));
    assert!(raw_log.contains("\"db_open_us\":"));
    assert!(raw_log.contains("\"fts_query_us\":"));
    assert!(raw_log.contains("\"filter_post_us\":"));
    assert!(raw_log.contains("\"vault_read_us\":"));
    assert!(raw_log.contains("\"markdown_parse_us\":"));

    // Verify `omk stats` and `omk stats --json` and `omk stats --reset` via CLI binary (REQ-OBS-005)
    let bin = env!("CARGO_BIN_EXE_omk");
    let stats_out = Command::new(bin)
        .args([
            "stats",
            "--vault",
            vault_dir.to_str().unwrap(),
            "--slow-ms",
            "0",
        ])
        .output()
        .expect("run omk stats");
    assert!(stats_out.status.success());
    let stats_stdout = String::from_utf8_lossy(&stats_out.stdout);
    assert!(stats_stdout.contains("[1. Overview & Lifecycle Summary]"));
    assert!(stats_stdout.contains("[2. Tool Latency Breakdown (ms)]"));
    assert!(stats_stdout.contains("[3. Stage Bottleneck Attribution]"));
    assert!(stats_stdout.contains("[4. Agent Efficiency & Search Quality]"));
    assert!(stats_stdout.contains("[5. Slow Request Anomalies"));
    for canary in canaries {
        assert!(
            !stats_stdout.contains(canary),
            "Zero-PII violation: omk stats output leaked canary '{}'",
            canary
        );
    }

    let stats_json_out = Command::new(bin)
        .args([
            "stats",
            "--vault",
            vault_dir.to_str().unwrap(),
            "--slow-ms",
            "0",
            "--json",
        ])
        .output()
        .expect("run omk stats --json");
    assert!(stats_json_out.status.success());
    let stats_json_raw = String::from_utf8_lossy(&stats_json_out.stdout);
    for canary in canaries {
        assert!(
            !stats_json_raw.contains(canary),
            "Zero-PII violation: omk stats --json leaked canary '{}'",
            canary
        );
    }
    let parsed_stats = JsonValue::parse(stats_json_raw.trim()).expect("valid stats json");
    let overview = parsed_stats.get("overview").expect("overview object");
    assert_eq!(
        overview.get("total_requests").and_then(JsonValue::as_f64),
        Some(6.0)
    );
    assert_eq!(
        overview.get("succeeded_requests").and_then(JsonValue::as_f64),
        Some(5.0)
    );
    assert_eq!(
        overview.get("failed_requests").and_then(JsonValue::as_f64),
        Some(1.0)
    );
    let eff = parsed_stats
        .get("agent_efficiency")
        .expect("agent_efficiency object");
    assert_eq!(
        eff.get("total_searches").and_then(JsonValue::as_f64),
        Some(3.0)
    );
    assert_eq!(
        eff.get("zero_hit_searches").and_then(JsonValue::as_f64),
        Some(2.0)
    );
    assert_eq!(
        eff.get("repeated_query_churn").and_then(JsonValue::as_f64),
        Some(1.0)
    );

    // Verify `omk stats --reset --json` clears logs
    let reset_out = Command::new(bin)
        .args(["stats", "--vault", vault_dir.to_str().unwrap(), "--reset", "--json"])
        .output()
        .expect("run omk stats --reset");
    assert!(reset_out.status.success());
    let reset_json =
        JsonValue::parse(String::from_utf8_lossy(&reset_out.stdout).trim()).expect("reset json");
    assert_eq!(
        reset_json.get("status").and_then(JsonValue::as_str),
        Some("reset")
    );
    let after_reset = compute_telemetry_stats(&daemon_cfg.paths, None, 50.0);
    assert_eq!(after_reset.total_requests, 0);

    let _ = fs::remove_dir_all(&vault_dir);
}

#[test]
fn test_log_rotation_enforces_max_size_and_backup_retention() {
    let vault_dir = unique_test_dir("log_rotate");
    let cfg = DaemonConfig::new(&vault_dir).expect("config");
    let log_path = &cfg.paths.log_path;

    // Set a 500-byte limit with max 3 backups (`daemon.log.1` .. `daemon.log.3`)
    for i in 0..25 {
        write_structured_log_with_limits(
            log_path,
            "INFO",
            "rotation_test_event",
            &format!("entry_index={} padding_data_for_rotation_check", i),
            500,
            3,
        );
    }

    assert!(log_path.exists());
    assert!(PathBuf::from(format!("{}.1", log_path.display())).exists());
    assert!(PathBuf::from(format!("{}.2", log_path.display())).exists());
    assert!(PathBuf::from(format!("{}.3", log_path.display())).exists());
    assert!(
        !PathBuf::from(format!("{}.4", log_path.display())).exists(),
        "Backups beyond max_log_backups (3) must be pruned"
    );
    let active_len = fs::metadata(log_path).expect("log metadata").len();
    assert!(
        active_len <= 500,
        "Active daemon.log size ({}) must not exceed max_bytes (500)",
        active_len
    );

    let _ = fs::remove_dir_all(&vault_dir);
}
