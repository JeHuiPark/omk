# omk (oh my knowledge)

**English** | [한국어](README.ko.md)

A standalone local knowledge vault skill and MCP daemon/proxy for AI agents.

## Overview

`omk` enables AI agents to accumulate, search, and cross-reference persistent knowledge across conversations (**Proactive Grounding**). Built in pure Rust as a single standalone binary with zero external crate dependencies, it pairs human-readable Markdown files (Single Source of Truth) with a fast SQLite FTS5 search index.

## Quick Start

```bash
# 1. Build the standalone binary
make build

# 2. Provision omk Skill & MCP config (auto-detects your installed AI harnesses)
./target/release/omk install

# Or target specific harnesses explicitly:
# (claude-code, claude-desktop, cursor, gemini/antigravity, codex, opencode, windsurf, all)
./target/release/omk install --harness claude-code,cursor

# 3. Verify your Local Vault, background daemon, and harness status
./target/release/omk status
./target/release/omk stats
./target/release/omk bench --docs 3000 --turns 80
```

### How It Works in Daily Use

1. **Install Once (`omk install`):** Automatically detects your AI agent harnesses, non-destructively merges the `omk` MCP server into your existing configuration, and installs the `SKILL.md` instructions.
2. **Zero Manual CLI Calls:** After installation, you never need to run `omk` commands to save or search notes. Simply talk to your AI agent in natural language—your agent will autonomously search prior context and save new decisions into `~/.omk/vault` (override via `OMK_VAULT_DIR` or `--vault <dir>`).
3. **Inspect & Maintain Anytime:** Run `omk status` to check system health, `omk stats` to analyze privacy-preserving P50/P95 latency and search quality metrics, `omk bench` to verify deterministic static invariants (`MET-STATIC-01..08`) and synthetic workload SLAs, `omk reconcile` if you edit Markdown files in the Vault directly, or `omk --help` for full CLI options (full CLI specification in [`EPIC-CLI-01`](docs/requirements/features/EPIC-CLI-01.md), [`EPIC-OBS-01`](docs/requirements/features/EPIC-OBS-01.md), and [`EPIC-PERF-01`](docs/requirements/features/EPIC-PERF-01.md)).

## Documentation

For system architecture, protocol schemas, and requirements specifications, refer to the Single Source of Truth (SSOT) documents mapped in [`AGENTS.md`](AGENTS.md):

- [`CONSTITUTION.md`](CONSTITUTION.md): Core design principles and project governance
- [`docs/architecture.md`](docs/architecture.md): Daemon-Proxy IPC, CQRS pipeline, and storage isolation design
- [`docs/protocol_spec.json`](docs/protocol_spec.json): MCP & JSON-RPC 2.0 tool schemas and error codes
- [`docs/requirements/index.md`](docs/requirements/index.md): Vision, scope, and feature specifications (`EPIC-*`)

## 🙏 Acknowledgments

This project is deeply inspired by the ideas and concepts from [Andrej Karpathy's LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f).
