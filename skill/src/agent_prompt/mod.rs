//! Agent Skill Provisioning (`omk install`) and Proactive Grounding Prompt Templates.
//!
//! Automatically provisions the `omk Skill` (`SKILL.md`) and registers the `omk` MCP server
//! into the user's AI agent environment (`~/.gemini/config/mcp_config.json` and `~/.gemini/skills/omk/SKILL.md`)
//! so the human user never needs to invoke the CLI manually during daily usage.

use omk_core::domain::OmkError;
use std::fs;
use std::path::{Path, PathBuf};

/// Output paths generated when provisioning the `omk Skill` via `omk install`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionResult {
    /// Path to the generated `SKILL.md` instruction file.
    pub skill_md_path: PathBuf,
    /// Path to the generated `mcp_config.json` snippet.
    pub mcp_config_path: PathBuf,
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

/// Renders the MCP server configuration JSON for AI clients (`~/.gemini/config/mcp_config.json`, Claude Desktop, Cursor, etc.).
pub fn render_mcp_config_json(binary_path: &Path, vault_dir: &Path) -> String {
    format!(
        r#"{{
  "mcpServers": {{
    "omk": {{
      "command": "{}",
      "args": ["mcp", "--vault", "{}"]
    }}
  }}
}}
"#,
        binary_path
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\""),
        vault_dir
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    )
}

/// Provisions the `omk Skill` (`SKILL.md` and `mcp_config.json`) into `target_dir`.
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
    fs::write(&mcp_config_path, render_mcp_config_json(&resolved_bin, v_dir)).map_err(|e| {
        OmkError::InternalError(format!(
            "Failed to write '{}': {}",
            mcp_config_path.display(),
            e
        ))
    })?;

    Ok(ProvisionResult {
        skill_md_path,
        mcp_config_path,
    })
}
