//! Interface and adapter library for `omk` (`skill` crate).
//!
//! Exposes the MCP / JSON-RPC 2.0 protocol handler, the Vault Server (`omk daemon`),
//! the lightweight Session Proxy (`omk mcp`), and the agent skill provisioning templates.

pub mod agent_prompt;
pub mod daemon;
pub mod protocol;
pub mod proxy;
