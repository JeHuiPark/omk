# OMV (Oh My Vault) - Directory Layout

This document defines the logical and physical workspace structure of the project and the role of each module.

## Module Layout
- **`core/`:** Responsible for the core business logic. (Refer to [`docs/architecture.md`](docs/architecture.md) for dependency rules and detailed constraints).
- **`skill/`:** Acts exclusively as an interface wrapper and adapter, calling `core` to communicate with external agents via JSON-RPC.
- **`tests/`:** Contains integration tests that verify interactions with external systems, such as file system I/O or search engines.
- **`docs/`:** Houses project documentation, including architectural design, system rules, and protocols.
  - **`docs/requirements/`:** The single source of truth for all requirements, containing vision, scope, and detailed feature specifications.
