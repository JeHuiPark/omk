//! Agent Skill Provisioning (`omk install`), Multi-Harness Auto-Detection (`REQ-CLI-004`),
//! Target Selection (`REQ-CLI-005`), Non-Destructive JSON Config Merging, and Proactive Grounding Prompt Templates.
//!
//! Provisions the `omk Skill` (`SKILL.md`) and registers the `omk` MCP server into supported
//! AI agent harnesses (`claude-code`, `claude-desktop`, `cursor`, `gemini`/`antigravity`,
//! `codex`, `opencode`, `windsurf`) without destroying existing user configuration.

use omk_core::domain::{JsonValue, OmkError};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Supported AI Agent Harness kinds (`EPIC-CLI-01` `REQ-CLI-004`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HarnessKind {
    /// Anthropic Claude Code CLI (`~/.claude`, `~/.claude.json`).
    ClaudeCode,
    /// Anthropic Claude Desktop App (`claude_desktop_config.json`).
    ClaudeDesktop,
    /// Cursor IDE (`~/.cursor`).
    Cursor,
    /// Google Antigravity / Gemini CLI (`~/.gemini`).
    Gemini,
    /// OpenAI Codex CLI (`~/.codex`).
    Codex,
    /// OpenCode CLI (`~/.config/opencode` or `~/.opencode`).
    OpenCode,
    /// Codeium Windsurf IDE (`~/.codeium/windsurf`).
    Windsurf,
}

impl HarnessKind {
    /// Returns all 7 well-known [`HarnessKind`] variants in canonical order.
    pub const fn all() -> &'static [HarnessKind] {
        &[
            Self::ClaudeCode,
            Self::ClaudeDesktop,
            Self::Cursor,
            Self::Gemini,
            Self::Codex,
            Self::OpenCode,
            Self::Windsurf,
        ]
    }

    /// Returns the canonical CLI slug for this harness (`REQ-CLI-004`).
    pub const fn slug(&self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::ClaudeDesktop => "claude-desktop",
            Self::Cursor => "cursor",
            Self::Gemini => "gemini",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Windsurf => "windsurf",
        }
    }

    /// Returns the human-readable product display name for this harness.
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::ClaudeDesktop => "Claude Desktop",
            Self::Cursor => "Cursor",
            Self::Gemini => "Gemini / Antigravity",
            Self::Codex => "Codex CLI",
            Self::OpenCode => "OpenCode",
            Self::Windsurf => "Windsurf",
        }
    }

    /// Parses a single harness identifier or alias (case-insensitive).
    ///
    /// # Errors
    /// Returns [`OmkError::InvalidParams`] if the harness name is unknown.
    pub fn from_name(raw: &str) -> Result<Self, OmkError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "claude-code" | "claudecode" | "claude_code" => Ok(Self::ClaudeCode),
            "claude-desktop" | "claudedesktop" | "claude_desktop" => Ok(Self::ClaudeDesktop),
            "cursor" => Ok(Self::Cursor),
            "gemini" | "antigravity" | "agy" | "gemini-cli" => Ok(Self::Gemini),
            "codex" | "codex-cli" => Ok(Self::Codex),
            "opencode" | "open-code" => Ok(Self::OpenCode),
            "windsurf" | "codeium" => Ok(Self::Windsurf),
            other => Err(OmkError::InvalidParams(format!(
                "Unknown harness '{}'. Supported values: auto, all, claude-code, claude-desktop, cursor, gemini (or antigravity/agy), codex, opencode, windsurf",
                other
            ))),
        }
    }

    /// Resolves the target file paths (MCP JSON config files and `SKILL.md` location) for this harness
    /// relative to the provided user `home_dir`.
    pub fn resolve_paths(&self, home_dir: &Path) -> HarnessPaths {
        match self {
            Self::ClaudeCode => HarnessPaths {
                kind: *self,
                detection_paths: vec![home_dir.join(".claude"), home_dir.join(".claude.json")],
                binary_hint: Some("claude"),
                mcp_config_paths: vec![home_dir.join(".claude.json")],
                skill_md_path: home_dir.join(".claude").join("skills").join("omk").join("SKILL.md"),
            },
            Self::ClaudeDesktop => {
                let app_dir = claude_desktop_dir(home_dir);
                HarnessPaths {
                    kind: *self,
                    detection_paths: vec![app_dir.clone()],
                    binary_hint: None,
                    mcp_config_paths: vec![app_dir.join("claude_desktop_config.json")],
                    skill_md_path: home_dir.join(".omk").join("skills").join("omk").join("SKILL.md"),
                }
            }
            Self::Cursor => {
                let cursor_dir = home_dir.join(".cursor");
                HarnessPaths {
                    kind: *self,
                    detection_paths: vec![cursor_dir.clone()],
                    binary_hint: None,
                    mcp_config_paths: vec![cursor_dir.join("mcp.json")],
                    skill_md_path: cursor_dir.join("skills").join("omk").join("SKILL.md"),
                }
            }
            Self::Gemini => {
                let gemini_dir = home_dir.join(".gemini");
                HarnessPaths {
                    kind: *self,
                    detection_paths: vec![gemini_dir.clone()],
                    binary_hint: None,
                    mcp_config_paths: vec![
                        gemini_dir.join("config").join("mcp_config.json"),
                        gemini_dir.join("settings.json"),
                    ],
                    skill_md_path: gemini_dir.join("skills").join("omk").join("SKILL.md"),
                }
            }
            Self::Codex => {
                let codex_dir = home_dir.join(".codex");
                HarnessPaths {
                    kind: *self,
                    detection_paths: vec![codex_dir.clone()],
                    binary_hint: Some("codex"),
                    mcp_config_paths: vec![codex_dir.join("mcp_config.json")],
                    skill_md_path: codex_dir.join("skills").join("omk").join("SKILL.md"),
                }
            }
            Self::OpenCode => {
                let cfg_opencode = home_dir.join(".config").join("opencode");
                let dot_opencode = home_dir.join(".opencode");
                HarnessPaths {
                    kind: *self,
                    detection_paths: vec![cfg_opencode.clone(), dot_opencode],
                    binary_hint: Some("opencode"),
                    mcp_config_paths: vec![cfg_opencode.join("opencode.json")],
                    skill_md_path: cfg_opencode.join("skills").join("omk").join("SKILL.md"),
                }
            }
            Self::Windsurf => {
                let ws_dir = home_dir.join(".codeium").join("windsurf");
                HarnessPaths {
                    kind: *self,
                    detection_paths: vec![ws_dir.clone()],
                    binary_hint: None,
                    mcp_config_paths: vec![ws_dir.join("mcp_config.json")],
                    skill_md_path: ws_dir.join("skills").join("omk").join("SKILL.md"),
                }
            }
        }
    }
}

fn claude_desktop_dir(home_dir: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home_dir
            .join("Library")
            .join("Application Support")
            .join("Claude")
    } else if cfg!(target_os = "windows") {
        if let Ok(appdata) = std::env::var("APPDATA") {
            if !appdata.trim().is_empty() {
                return PathBuf::from(appdata).join("Claude");
            }
        }
        home_dir.join("AppData").join("Roaming").join("Claude")
    } else {
        home_dir.join(".config").join("Claude")
    }
}

/// Resolved local paths for a specific [`HarnessKind`].
#[derive(Debug, Clone)]
pub struct HarnessPaths {
    /// Harness identifier.
    pub kind: HarnessKind,
    /// Directories or files whose existence indicates the harness is installed locally.
    pub detection_paths: Vec<PathBuf>,
    /// Optional binary name to check in `PATH` when `allow_path_lookup` is true.
    pub binary_hint: Option<&'static str>,
    /// Target JSON configuration files where `mcpServers.omk` is merged.
    pub mcp_config_paths: Vec<PathBuf>,
    /// Target `SKILL.md` file path for Proactive Grounding instructions.
    pub skill_md_path: PathBuf,
}

impl HarnessPaths {
    /// Checks whether this harness is installed on the local system.
    ///
    /// Checks `detection_paths` first; if `check_path_env` is `true`, also checks `PATH` for `binary_hint`.
    pub fn is_detected(&self, check_path_env: bool) -> bool {
        if self.detection_paths.iter().any(|p| p.exists()) {
            return true;
        }
        if check_path_env {
            if let Some(bin_name) = self.binary_hint {
                if binary_exists_in_path(bin_name) {
                    return true;
                }
            }
        }
        false
    }

    /// Checks whether `omk` is already configured in this harness (`mcpServers.omk` present in config).
    pub fn is_configured(&self) -> bool {
        let mcp_ok = self.mcp_config_paths.iter().any(|cfg_path| {
            if !cfg_path.exists() {
                return false;
            }
            if let Ok(raw) = fs::read_to_string(cfg_path) {
                if let Ok(val) = JsonValue::parse(&raw) {
                    return val
                        .get("mcpServers")
                        .and_then(|s| s.get("omk"))
                        .is_some();
                }
            }
            false
        });
        mcp_ok && self.skill_md_path.exists()
    }
}

fn binary_exists_in_path(bin_name: &str) -> bool {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(bin_name);
            if candidate.is_file() {
                return true;
            }
        }
    }
    false
}

/// Target selection strategy for `omk install --harness <spec>` (`REQ-CLI-005`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessSelection {
    /// Automatically detect installed harnesses under `HOME` (default when `--harness` is omitted).
    Auto,
    /// Provision all 7 supported harnesses regardless of prior installation.
    All,
    /// Provision the explicitly listed harnesses (creating directories if needed).
    Specific(Vec<HarnessKind>),
}

impl HarnessSelection {
    /// Parses a `--harness` specification string (`auto`, `all`, or comma-separated harness names).
    ///
    /// # Errors
    /// Returns [`OmkError::InvalidParams`] if the input is empty or contains an unknown harness name.
    pub fn parse(raw: &str) -> Result<Self, OmkError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(OmkError::InvalidParams(
                "Option '--harness' requires a value (auto, all, or comma-separated harness names)".to_string(),
            ));
        }
        if trimmed.eq_ignore_ascii_case("auto") {
            return Ok(Self::Auto);
        }
        if trimmed.eq_ignore_ascii_case("all") {
            return Ok(Self::All);
        }

        let mut list = Vec::new();
        for part in trimmed.split(',') {
            let p = part.trim();
            if p.is_empty() {
                continue;
            }
            if p.eq_ignore_ascii_case("all") {
                return Ok(Self::All);
            }
            if p.eq_ignore_ascii_case("auto") {
                return Ok(Self::Auto);
            }
            let kind = HarnessKind::from_name(p)?;
            if !list.contains(&kind) {
                list.push(kind);
            }
        }

        if list.is_empty() {
            return Err(OmkError::InvalidParams(
                "No valid harness name specified in '--harness'".to_string(),
            ));
        }
        Ok(Self::Specific(list))
    }
}

/// Output paths generated when provisioning the `omk Skill` into a single directory via [`install_skill`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionResult {
    /// Path to the generated `SKILL.md` instruction file.
    pub skill_md_path: PathBuf,
    /// Path to the generated `mcp_config.json` snippet.
    pub mcp_config_path: PathBuf,
}

/// Per-harness provisioning report produced during multi-harness installation (`REQ-CLI-004`, `REQ-CLI-005`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessProvisionReport {
    /// Canonical harness slug (`claude-code`, `cursor`, `custom`, `fallback`, etc.).
    pub harness: String,
    /// Human-readable harness name.
    pub display_name: String,
    /// Path to the provisioned `SKILL.md`.
    pub skill_md_path: PathBuf,
    /// Paths to the updated/created MCP configuration JSON files.
    pub mcp_config_paths: Vec<PathBuf>,
    /// Optional backup files (`.bak`) created if an existing config had malformed JSON.
    pub backup_paths: Vec<PathBuf>,
}

/// Status diagnostic entry for a single harness (`REQ-CLI-003` `omk status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessStatusInfo {
    /// Canonical harness slug.
    pub slug: &'static str,
    /// Human-readable harness display name.
    pub display_name: &'static str,
    /// Whether the harness was detected on the local machine.
    pub detected: bool,
    /// Whether `omk` is configured in the harness's MCP config and `SKILL.md`.
    pub configured: bool,
    /// Primary MCP config path for this harness.
    pub primary_mcp_config: PathBuf,
    /// `SKILL.md` path for this harness.
    pub skill_md_path: PathBuf,
}

/// Renders the `SKILL.md` instruction document guiding AI agents to perform Proactive Grounding.
pub fn render_skill_markdown(binary_path: &Path, vault_dir: &Path) -> String {
    format!(
        r#"---
name: omk
description: Standalone local knowledge vault skill for AI agents. Use to proactively search, read, and accumulate persistent user/project knowledge across sessions (Proactive Grounding).
---

# omk (oh my knowledge) - Agent Skill Instructions

## 1. Purpose: Proactive Grounding
You have access to the `omk` Local Knowledge Vault located at `{vault}`.
The human user will NEVER call the `omk` CLI directly—YOU (the AI Agent) are responsible for transparently searching, reading, and saving knowledge on their behalf during natural conversation.

- **Before answering architectural, domain, or historical questions:** Proactively query `omk` (`search_knowledge` -> `read_knowledge`) to ground your response in prior project/user decisions.
- **When the user states a rule, preference, or architectural decision:** Proactively save it (`upsert_knowledge`) so knowledge compounds across sessions.

## 2. How to Call `omk` Tools
If the `omk` MCP tools (`search_knowledge`, `read_knowledge`, `upsert_knowledge`) are mounted in your toolset, call them directly.
If MCP tools are not yet mounted in the current session, invoke `{bin} mcp --vault {vault}` seamlessly via a one-line JSON-RPC command in your shell tool:

- **Search Knowledge (`search_knowledge`):**
  ```bash
  printf '%s\n' '{{"jsonrpc":"2.0","id":1,"method":"search_knowledge","params":{{"query":"<KEYWORDS>","filters":{{"tags":[]}},"limit":5}}}}' | {bin} mcp --vault {vault}
  ```
- **Read Knowledge (`read_knowledge`):**
  ```bash
  printf '%s\n' '{{"jsonrpc":"2.0","id":1,"method":"read_knowledge","params":{{"id":"<DOC_ID>"}}}}' | {bin} mcp --vault {vault}
  ```
- **Save / Update Knowledge (`upsert_knowledge`):**
  - `source.class`: `"USER_EXPLICIT"` (stated/approved by user, Authority 1.0), `"AGENT_INFERRED"` (inferred by agent, Authority 0.6), or `"EXTERNAL_SYNC"`.
  - `source.type`: `"WIKI"` (`UPSERT_FULL`), `"MESSENGER"` (`APPEND_ONLY`), `"ISSUE_TRACKER"` (`STATE_PATCH`), `"CODE_REPO"` (`LINK_REFERENCE`), or `"GENERAL"`.
  ```bash
  printf '%s\n' '{{"jsonrpc":"2.0","id":1,"method":"upsert_knowledge","params":{{"title":"<TITLE>","content":"<MARKDOWN_BODY>","source":{{"class":"USER_EXPLICIT","type":"WIKI","provider":"local"}},"tags":["<TAG>"]}}}}' | {bin} mcp --vault {vault}
  ```
"#,
        vault = vault_dir.display(),
        bin = binary_path.display()
    )
}

/// Builds the [`JsonValue`] object representing `"omk": { "command": "...", "args": ["mcp", "--vault", "..."] }`.
pub fn build_omk_mcp_server_entry(binary_path: &Path, vault_dir: &Path) -> JsonValue {
    let mut entry = BTreeMap::new();
    entry.insert(
        "command".to_string(),
        JsonValue::Str(binary_path.to_string_lossy().into_owned()),
    );
    entry.insert(
        "args".to_string(),
        JsonValue::Array(vec![
            JsonValue::Str("mcp".to_string()),
            JsonValue::Str("--vault".to_string()),
            JsonValue::Str(vault_dir.to_string_lossy().into_owned()),
        ]),
    );
    JsonValue::Object(entry)
}

/// Renders the standalone MCP server configuration JSON for AI clients (`mcp_config.json`).
pub fn render_mcp_config_json(binary_path: &Path, vault_dir: &Path) -> String {
    let mut servers = BTreeMap::new();
    servers.insert(
        "omk".to_string(),
        build_omk_mcp_server_entry(binary_path, vault_dir),
    );
    let mut root = BTreeMap::new();
    root.insert("mcpServers".to_string(), JsonValue::Object(servers));
    JsonValue::Object(root).to_pretty_json_string()
}

/// Non-destructively merges the `mcpServers.omk` server configuration into an existing JSON config file
/// (`REQ-CLI-004`).
///
/// - If the file does not exist or is empty, creates a new JSON object with `mcpServers.omk`.
/// - If the file contains valid JSON object, preserves all top-level keys and all sibling `mcpServers` entries,
///   updating only `mcpServers.omk`.
/// - If the file contains malformed JSON, backs it up to `<path>.bak` before writing the clean config so user data is never lost.
///
/// # Errors
/// Returns [`OmkError::InternalError`] if creating parent directories or writing the config file fails.
pub fn merge_mcp_config_file(
    config_path: &Path,
    binary_path: &Path,
    vault_dir: &Path,
) -> Result<Option<PathBuf>, OmkError> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            OmkError::InternalError(format!(
                "Failed to create config directory '{}': {}",
                parent.display(),
                e
            ))
        })?;
    }

    let mut backup_created: Option<PathBuf> = None;
    let mut root_map = BTreeMap::new();

    if config_path.exists() {
        let existing_raw = fs::read_to_string(config_path).map_err(|e| {
            OmkError::InternalError(format!(
                "Failed to read existing MCP config '{}': {}",
                config_path.display(),
                e
            ))
        })?;
        let trimmed = existing_raw.trim();
        if !trimmed.is_empty() {
            match JsonValue::parse(trimmed) {
                Ok(JsonValue::Object(map)) => {
                    root_map = map;
                }
                _ => {
                    let bak_path = PathBuf::from(format!("{}.bak", config_path.display()));
                    let _ = fs::copy(config_path, &bak_path);
                    backup_created = Some(bak_path);
                }
            }
        }
    }

    let mut servers_map = match root_map.remove("mcpServers") {
        Some(JsonValue::Object(m)) => m,
        _ => BTreeMap::new(),
    };
    servers_map.insert(
        "omk".to_string(),
        build_omk_mcp_server_entry(binary_path, vault_dir),
    );
    root_map.insert("mcpServers".to_string(), JsonValue::Object(servers_map));

    let serialized = JsonValue::Object(root_map).to_pretty_json_string();
    fs::write(config_path, serialized).map_err(|e| {
        OmkError::InternalError(format!(
            "Failed to write MCP config '{}': {}",
            config_path.display(),
            e
        ))
    })?;

    Ok(backup_created)
}

/// Provisions the `omk Skill` (`SKILL.md` and `mcp_config.json`) into a specific `target_dir` (`REQ-CLI-005`).
///
/// Preserves existing entries in `target_dir/mcp_config.json` via non-destructive merge.
///
/// # Errors
/// Returns [`OmkError::InternalError`] if creating directories or writing files fails.
pub fn install_skill<P: AsRef<Path>, Q: AsRef<Path>>(
    target_dir: P,
    vault_dir: Q,
    binary_path: Option<&Path>,
) -> Result<ProvisionResult, OmkError> {
    let out_dir = target_dir.as_ref();
    let v_dir = vault_dir.as_ref();
    fs::create_dir_all(out_dir).map_err(|e| {
        OmkError::InternalError(format!(
            "Failed to create skill install directory '{}': {}",
            out_dir.display(),
            e
        ))
    })?;
    fs::create_dir_all(v_dir).map_err(|e| {
        OmkError::InternalError(format!(
            "Failed to create Vault directory '{}': {}",
            v_dir.display(),
            e
        ))
    })?;

    let resolved_bin = match binary_path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_exe().unwrap_or_else(|_| PathBuf::from("omk")),
    };

    let skill_md_path = out_dir.join("SKILL.md");
    let mcp_config_path = out_dir.join("mcp_config.json");

    fs::write(&skill_md_path, render_skill_markdown(&resolved_bin, v_dir)).map_err(|e| {
        OmkError::InternalError(format!(
            "Failed to write '{}': {}",
            skill_md_path.display(),
            e
        ))
    })?;
    merge_mcp_config_file(&mcp_config_path, &resolved_bin, v_dir)?;

    Ok(ProvisionResult {
        skill_md_path,
        mcp_config_path,
    })
}

/// Provisions a single [`HarnessKind`] under `home_dir` (`REQ-CLI-004`, `REQ-CLI-005`).
///
/// # Errors
/// Returns [`OmkError::InternalError`] if creating directories or writing files fails.
pub fn provision_harness(
    kind: HarnessKind,
    home_dir: &Path,
    vault_dir: &Path,
    binary_path: &Path,
) -> Result<HarnessProvisionReport, OmkError> {
    fs::create_dir_all(vault_dir).map_err(|e| {
        OmkError::InternalError(format!(
            "Failed to create Vault directory '{}': {}",
            vault_dir.display(),
            e
        ))
    })?;

    let paths = kind.resolve_paths(home_dir);
    if let Some(skill_parent) = paths.skill_md_path.parent() {
        fs::create_dir_all(skill_parent).map_err(|e| {
            OmkError::InternalError(format!(
                "Failed to create skill directory '{}': {}",
                skill_parent.display(),
                e
            ))
        })?;
    }
    fs::write(
        &paths.skill_md_path,
        render_skill_markdown(binary_path, vault_dir),
    )
    .map_err(|e| {
        OmkError::InternalError(format!(
            "Failed to write SKILL.md '{}': {}",
            paths.skill_md_path.display(),
            e
        ))
    })?;

    let mut backups = Vec::new();
    for mcp_path in &paths.mcp_config_paths {
        if let Some(bak) = merge_mcp_config_file(mcp_path, binary_path, vault_dir)? {
            backups.push(bak);
        }
    }

    Ok(HarnessProvisionReport {
        harness: kind.slug().to_string(),
        display_name: kind.display_name().to_string(),
        skill_md_path: paths.skill_md_path,
        mcp_config_paths: paths.mcp_config_paths,
        backup_paths: backups,
    })
}

/// Detects installed harnesses under `home_dir` (`REQ-CLI-004`).
pub fn detect_installed_harnesses(home_dir: &Path, check_path_env: bool) -> Vec<HarnessKind> {
    HarnessKind::all()
        .iter()
        .copied()
        .filter(|kind| kind.resolve_paths(home_dir).is_detected(check_path_env))
        .collect()
}

/// Provisions harnesses according to [`HarnessSelection`] (`Auto`, `All`, or `Specific`).
///
/// In `Auto` mode, if no well-known harness directory is detected under `home_dir`, falls back to
/// provisioning the universal `~/.omk/skills/omk` and `~/.gemini` environments (`REQ-CLI-004`).
///
/// # Errors
/// Returns [`OmkError::InternalError`] if writing skill or config files fails.
pub fn install_for_selection(
    selection: &HarnessSelection,
    home_dir: &Path,
    vault_dir: &Path,
    binary_path: Option<&Path>,
    check_path_env: bool,
) -> Result<Vec<HarnessProvisionReport>, OmkError> {
    let resolved_bin = match binary_path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_exe().unwrap_or_else(|_| PathBuf::from("omk")),
    };

    let targets: Vec<HarnessKind> = match selection {
        HarnessSelection::Auto => detect_installed_harnesses(home_dir, check_path_env),
        HarnessSelection::All => HarnessKind::all().to_vec(),
        HarnessSelection::Specific(list) => list.clone(),
    };

    let mut reports = Vec::new();
    if targets.is_empty() {
        // Fallback when no known harness is detected in Auto mode (`REQ-CLI-004`)
        let omk_fallback_dir = home_dir.join(".omk").join("skills").join("omk");
        let res = install_skill(&omk_fallback_dir, vault_dir, Some(&resolved_bin))?;
        reports.push(HarnessProvisionReport {
            harness: "fallback".to_string(),
            display_name: "Universal Fallback (~/.omk)".to_string(),
            skill_md_path: res.skill_md_path,
            mcp_config_paths: vec![res.mcp_config_path],
            backup_paths: Vec::new(),
        });
        let gemini_report =
            provision_harness(HarnessKind::Gemini, home_dir, vault_dir, &resolved_bin)?;
        reports.push(gemini_report);
    } else {
        for kind in targets {
            let report = provision_harness(kind, home_dir, vault_dir, &resolved_bin)?;
            reports.push(report);
        }
    }

    Ok(reports)
}

/// Inspects the detection and configuration status of all 7 well-known harnesses (`REQ-CLI-003`).
pub fn inspect_all_harnesses(home_dir: &Path, check_path_env: bool) -> Vec<HarnessStatusInfo> {
    HarnessKind::all()
        .iter()
        .map(|kind| {
            let paths = kind.resolve_paths(home_dir);
            let detected = paths.is_detected(check_path_env);
            let configured = paths.is_configured();
            let primary_mcp_config = paths
                .mcp_config_paths
                .into_iter()
                .next()
                .unwrap_or_else(|| home_dir.join("mcp_config.json"));
            HarnessStatusInfo {
                slug: kind.slug(),
                display_name: kind.display_name(),
                detected,
                configured,
                primary_mcp_config,
                skill_md_path: paths.skill_md_path,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "omk_harness_ut_{}_{}_{}",
            label,
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp home");
        dir
    }

    #[test]
    fn test_non_destructive_json_merge_preserves_existing_servers_and_keys() {
        let home = temp_home("merge");
        let cfg_path = home.join(".cursor").join("mcp.json");
        fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();

        let existing = r#"{
  "theme": "dark",
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"]
    }
  }
}"#;
        fs::write(&cfg_path, existing).unwrap();

        let bin = PathBuf::from("/usr/local/bin/omk");
        let vault = home.join(".omk").join("vault");
        let bak = merge_mcp_config_file(&cfg_path, &bin, &vault).expect("merge");
        assert!(bak.is_none());

        let merged_raw = fs::read_to_string(&cfg_path).unwrap();
        let parsed = JsonValue::parse(&merged_raw).expect("valid merged json");
        assert_eq!(parsed.get("theme").and_then(JsonValue::as_str), Some("dark"));
        let servers = parsed.get("mcpServers").unwrap();
        assert!(servers.get("github").is_some(), "must preserve github server");
        assert_eq!(
            servers
                .get("omk")
                .and_then(|o| o.get("command"))
                .and_then(JsonValue::as_str),
            Some("/usr/local/bin/omk")
        );

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn test_malformed_json_creates_backup_file() {
        let home = temp_home("malformed");
        let cfg_path = home.join(".claude.json");
        fs::write(&cfg_path, "{ invalid json ").unwrap();

        let bin = PathBuf::from("/usr/local/bin/omk");
        let vault = home.join("vault");
        let bak = merge_mcp_config_file(&cfg_path, &bin, &vault)
            .expect("merge should recover")
            .expect("backup path should be returned");
        assert!(bak.exists());
        assert_eq!(fs::read_to_string(&bak).unwrap(), "{ invalid json ");

        let fixed_raw = fs::read_to_string(&cfg_path).unwrap();
        let parsed = JsonValue::parse(&fixed_raw).expect("valid json after recovery");
        assert!(parsed.get("mcpServers").and_then(|s| s.get("omk")).is_some());

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn test_auto_detection_and_specific_harness_selection() {
        let home = temp_home("autodetect");
        let vault = home.join("vault");
        let bin = PathBuf::from("/usr/local/bin/omk");

        // Create only .cursor and .claude directories
        fs::create_dir_all(home.join(".cursor")).unwrap();
        fs::create_dir_all(home.join(".claude")).unwrap();

        let detected = detect_installed_harnesses(&home, false);
        assert_eq!(detected, vec![HarnessKind::ClaudeCode, HarnessKind::Cursor]);

        let reports = install_for_selection(
            &HarnessSelection::Auto,
            &home,
            &vault,
            Some(&bin),
            false,
        )
        .expect("auto install");
        assert_eq!(reports.len(), 2);
        assert!(home.join(".claude.json").exists());
        assert!(home.join(".cursor/mcp.json").exists());

        // Specific harness that wasn't pre-installed (windsurf)
        let sel = HarnessSelection::parse("windsurf,antigravity").expect("parse selection");
        let spec_reports =
            install_for_selection(&sel, &home, &vault, Some(&bin), false).expect("specific install");
        assert_eq!(spec_reports.len(), 2);
        assert!(home.join(".codeium/windsurf/mcp_config.json").exists());
        assert!(home.join(".gemini/config/mcp_config.json").exists());

        let statuses = inspect_all_harnesses(&home, false);
        let configured_count = statuses.iter().filter(|s| s.configured).count();
        assert_eq!(configured_count, 4);

        let _ = fs::remove_dir_all(&home);
    }
}
