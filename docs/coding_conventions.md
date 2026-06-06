# OMV (Oh My Vault) - Coding Conventions & Testing Strategy

This document outlines the coding standards, conventions, and testing strategies required when developing the `omv` project.

## 1. Documentation
- **Rustdoc (`///`):** Writing documentation is mandatory for all `public` modules, structs, traits, and functions. 
- **Error Documentation:** If there is a possibility of exceptions or errors, `# Errors` and `# Panics` sections must be explicitly stated to clarify constraints and side effects.

## 2. Architecture & Stability
- **Architecture Guidelines:** Follow [`docs/architecture.md`](architecture.md) strictly for system design rules.
- **Defensive Programming:** Implement system-level defensive logic strictly, such as preventing stream deadlocks and verifying file I/O permissions.
- **Error Handling:** The process must not panic or die unexpectedly. It must return clear, JSON-formatted error logs so that the calling agent can recognize the cause of the failure.

## 3. Testing Strategy
- **Core Logic (Unit Tests):** Use trait-based dependency injection or `mockall` for fast unit tests (`#[cfg(test)]`) of core business logic.
- **External Interfaces (Integration Tests):** Code paths that interact with the file system I/O or search engines should be tested separately via integration tests located in the root `tests/` directory.

## 4. General Style
- Adhere to the standard `rustfmt` formatting.
- Variable, function names, system logs, comments, and standard OSS docs must be in English.
