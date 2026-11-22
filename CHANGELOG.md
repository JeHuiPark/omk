# Changelog

All notable changes to `omk` will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [v0.2.0] - 2026-11-22

### Features

- **cli:** Add disk safety thresholds and confirmation prompt to omk bench (#7) (de4d8e2)

## [v0.1.0] - 2026-11-21

### Features

- **obs:** Implement end-to-end span telemetry, Zero-PII redaction, log rotation, and omk stats (cfa1b85)
- **skill:** Implement human-friendly CLI UX, status command, and multi-harness provisioning (784ea87)
- **skill:** Auto-provision global MCP config and agent instructions on omk install (36750a3)
- **skill:** Implement MCP protocol router, singleton daemon, Stdio proxy, and skill installer (fad1984)
- **core:** Implement domain models, Vault DocumentStore, SQLite FTS5 IndexStore, and CQRS pipeline (f717648)

### Bug Fixes

- **skill:** Support MCP server/discover and separate tool vs control RPC stats (2a94cfc)

### Performance

- Implement persistent IPC, Fast-Bind, incremental reconcile, SQLite reader pool, and omk bench (27c41c1)

### Refactoring

- Unify Vault Server and Session Proxy terminology across docs, CLI, and tests (26faf75)
