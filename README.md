# omk (oh my knowledge)

A standalone local knowledge vault skill and MCP daemon/proxy for AI agents.

## Overview

`omk` enables AI agents to accumulate, search, and cross-reference persistent knowledge across conversations (**Proactive Grounding**). Built in pure Rust as a standalone binary with zero external crate dependencies, it combines human-readable Markdown files (Single Source of Truth) with a high-speed SQLite FTS5 index.

## Architecture Highlights

- **Global Daemon & Stdio Proxy (`EPIC-IPC-01`):**
  - `omk daemon`: Singleton background process protected by OS `flock` (`daemon.pid`), handling all storage writes and queries over a Unix Domain Socket with backpressure protection (`-32001`).
  - `omk mcp`: Lightweight Stdio MCP proxy spawned per AI agent session with auto-daemon spawning, exponential backoff retry, and strict socket timeouts (`-32000`).
- **CQRS Pipeline & Storage Isolation (`EPIC-VAULT-01`, `EPIC-SEARCH-01`):**
  - `DocumentStore` (`LocalMarkdownDocumentStore`): Atomic Markdown + YAML Frontmatter file writes and strict path traversal protection (`-32003`).
  - `IndexStore` (`SqliteFtsIndexStore`): SQLite WAL mode + FTS5 full-text search + JSON metadata filtering.
  - Startup Auto-Recovery (`reconcile`): Rebuilds missing or stale index entries by comparing Vault `version_hash` values on daemon startup.
- **Provenance & Sync Strategies (`EPIC-DATA-01`):**
  - Enforces authority boundaries (`USER_EXPLICIT`, `AGENT_INFERRED`, `EXTERNAL_SYNC`) and source-specific sync strategies (`UPSERT_FULL`, `APPEND_ONLY`, `STATE_PATCH`, `LINK_REFERENCE`).

## Quick Start

```bash
# Build release binary
make build

# Run full unit & integration test suite
make test

# Provision the omk Skill (SKILL.md + mcp_config.json)
./target/release/omk install --vault ~/.omk/vault --target ~/.omk/skill

# Start MCP Stdio Proxy (auto-spawns background daemon if needed)
./target/release/omk mcp --vault ~/.omk/vault
```

## Documentation

See [`AGENTS.md`](AGENTS.md) for the complete Knowledge Map, including [`CONSTITUTION.md`](CONSTITUTION.md), [`docs/architecture.md`](docs/architecture.md), [`docs/protocol_spec.json`](docs/protocol_spec.json), and [`docs/requirements/index.md`](docs/requirements/index.md).

## 🙏 Acknowledgments

This project is deeply inspired by the ideas and concepts from [Andrej Karpathy's LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f).
