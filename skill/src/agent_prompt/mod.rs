//! Agent Skill Provisioning (`omk install`) and Proactive Grounding Prompt Templates.
//!
//! Generates the `omk Skill` package (`SKILL.md` and MCP configuration snippet) as defined in
//! [`docs/requirements/glossary.md`](../../../docs/requirements/glossary.md) and
//! [`docs/requirements/vision_and_scope.md`](../../../docs/requirements/vision_and_scope.md).

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
Before answering architectural, domain, or historical questions—or when a user introduces important new decisions—you MUST proactively query and update `omk` so knowledge compounds across conversations.

## 2. Available MCP Tools
- **`search_knowledge`**: Fast SQLite FTS5 full-text search and metadata (`tags`, `source_class`, `source_type`, `min_authority`) filtering. Always run `search_knowledge` first to discover relevant context snippets without loading full files.
- **`read_knowledge`**: Reads the full Markdown body and YAML Frontmatter metadata of a specific document by `id` when deeper context is needed.
- **`upsert_knowledge`**: Persists or updates knowledge according to source provenance:
  - `USER_EXPLICIT`: Knowledge explicitly stated or confirmed by the human user (Authority: `1.0`). Protected from arbitrary `AGENT_INFERRED` overwrite.
  - `AGENT_INFERRED`: Insights or summaries inferred by you during problem solving (Authority: `0.6`).
  - `EXTERNAL_SYNC`: Knowledge synchronized from external systems (`WIKI`, `MESSENGER`, `ISSUE_TRACKER`, `CODE_REPO`).

## 3. MCP Server Invocation
`omk` runs via a lightweight Stdio Proxy (`{bin} mcp --vault {vault}`) backed by a global singleton daemon (`{bin} daemon --vault {vault}`).
"#,
        vault = vault_dir.display(),
        bin = binary_path.display()
    )
}

/// Renders the MCP server configuration JSON for AI clients (Claude Desktop, Cursor, etc.).
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
        binary_path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\""),
        vault_dir.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
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
