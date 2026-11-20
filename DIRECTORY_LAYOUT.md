# omk (oh my knowledge) - Directory Layout

This document defines the logical and physical workspace structure of the project and the role of each module.

## Module Layout
- **`core/`:** Responsible for the core domain models, storage traits/implementations, and CQRS pipelines. (Refer to [`docs/architecture.md`](docs/architecture.md) for dependency rules and detailed constraints).
  - **`core/src/domain/`:** Pure domain entities (`KnowledgeEntity`, `SourceProvenance`, `SyncStrategy`), DTOs, and domain error types.
  - **`core/src/vault/`:** `DocumentStore` trait and `LocalMarkdownDocumentStore` implementation (atomic file writes, YAML frontmatter parsing, path traversal prevention).
  - **`core/src/index/`:** `IndexStore` trait and `SqliteFtsIndexStore` implementation (SQLite WAL mode, FTS5 full-text indexing, JSON metadata filtering).
  - **`core/src/search/`:** CQRS Command (`UpsertKnowledgeUseCase`) and Query (`SearchKnowledgeUseCase`, `ReadKnowledgeUseCase`) pipelines and Vault-Index reconciliation logic.
- **`skill/`:** Acts exclusively as an interface wrapper and adapter, calling `core` to communicate with external agents via MCP / JSON-RPC over Daemon & Stdio Proxy.
  - **`skill/src/protocol/`:** JSON-RPC 2.0 and MCP request/response parser, serializer, and error code mapping.
  - **`skill/src/daemon/`:** Background Vault Server (`omk daemon`) with PID lock, Unix Domain Socket listener, CQRS queue separation, and backpressure control.
  - **`skill/src/proxy/`:** Lightweight Session Proxy (`omk mcp`) with auto-spawn, deadline timeout, and exponential backoff retry.
  - **`skill/src/agent_prompt/`:** Skill provisioning (`omk install`) and agent instruction template generator for Proactive Grounding.
- **`tests/`:** Contains integration tests that verify end-to-end interactions across the file system Vault, SQLite FTS5 index, and Daemon-Proxy MCP protocol.
- **`docs/`:** Houses project documentation, including architectural design, system rules, and protocols.
  - **`docs/requirements/`:** The single source of truth for all requirements, containing vision, scope, and detailed feature specifications.
