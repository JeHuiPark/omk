<!-- You may fill out this PR description in either Korean or English. -->

## Summary

<!-- Briefly describe what changed and why. -->

- 

## Traceability

<!-- Include related Requirement IDs or Issue numbers (e.g., Refs: EPIC-PERF-01 / REQ-PERF-006, Closes #1). -->
<!-- For docs or maintenance PRs without a requirement ID, briefly note the reason. -->

- `Refs: `

## Merge Checklist

### 1. PR Title & Traceability ([`CONTRIBUTING.md`](CONTRIBUTING.md), [`CONSTITUTION.md`](CONSTITUTION.md) §3)
- [ ] **Conventional PR Title (Squash-only):** The PR title follows `<type>[optional scope]: <Capitalized description>` without a trailing period (this becomes the commit header on `main`).
- [ ] **Traceability:** Requirement IDs (`Refs: EPIC-... / REQ-...`) or issue links (`Closes #...` / `Refs: #...`) are included in this PR description or commits for any `core/` or `skill/` changes.

### 2. Documentation & Governance ([`CONSTITUTION.md`](CONSTITUTION.md) §2–§4)
- [ ] **Update First (SSOT):** Single source of truth documents ([`docs/requirements/index.md`](docs/requirements/index.md), relevant `EPIC-*` specs, [`docs/architecture.md`](docs/architecture.md)) were updated before code changes.
- [ ] **Knowledge Map:** If a new rule or guide document was added, its path and one-line summary were registered in [`AGENTS.md`](AGENTS.md). *(if applicable)*
- [ ] **Zero External Dependencies:** No third-party crates were added to `core/Cargo.toml` or `skill/Cargo.toml`, and `skill` depends only on local `omk-core`. *(If an exception was approved, document the trade-offs below.)*

### 3. Quality & Verification ([`docs/coding_conventions.md`](docs/coding_conventions.md), [`EPIC-PERF-01`](docs/requirements/features/EPIC-PERF-01.md))
- [ ] **Zero Warnings & Tests:** `RUSTFLAGS="-D warnings" cargo test --workspace` passes with 0 compiler warnings and all unit/integration tests green.
- [ ] **Static Invariants:** `make build && ./target/release/omk bench --docs 500 --turns 20 --json` passes all 8 deterministic static invariants (`"all_invariants_passed": true`).
- [ ] **Coding Conventions:** Public (`pub`) items have Rustdoc (`///`) comments and follow error-handling rules in [`docs/coding_conventions.md`](docs/coding_conventions.md).

## Exceptions & Reviewer Notes (Optional)

<!-- Note any approved governance exceptions, interface changes, or areas for reviewers to focus on. -->
