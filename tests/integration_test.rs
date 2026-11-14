//! End-to-End Integration Tests across Vault File System, SQLite FTS5 Index,
//! Global Singleton Daemon, and Stdio MCP Proxy (`EPIC-VAULT-01`, `EPIC-DATA-01`,
//! `EPIC-SEARCH-01`, `EPIC-IPC-01`).

use omk::agent_prompt::install_skill;
use omk::daemon::{DaemonConfig, DaemonServer, PidLockGuard};
use omk::proxy::{McpProxy, ProxyConfig};
use omk_core::domain::JsonValue;
use std::fs;
use std::path::PathBuf;
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
