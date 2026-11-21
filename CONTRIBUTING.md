# Contributing to omk (oh my knowledge)

First off, thank you for considering contributing to `omk`! It's people like you that make the open-source community such a great place to learn, inspire, and create.

## Commit Convention

We use [Conventional Commits](https://www.conventionalcommits.org/) for our commit messages. This leads to more readable messages that are easy to follow when looking through the project history.

### Format
Each commit message consists of a **header**, a **body**, and a **footer**.
```text
<type>[optional scope]: <description>

[optional body]

[optional footer(s)]
```

### Allowed Types
- **feat:** A new feature
- **fix:** A bug fix
- **docs:** Documentation only changes
- **style:** Changes that do not affect the meaning of the code (white-space, formatting, missing semi-colons, etc.)
- **refactor:** A code change that neither fixes a bug nor adds a feature
- **perf:** A code change that improves performance
- **test:** Adding missing tests or correcting existing tests
- **chore:** Changes to the build process or auxiliary tools and libraries

### Rules
1. **Use the imperative mood in the subject line:** "add feature" not "added feature" nor "adds feature".
2. **Capitalize the description:** Start the description with a capital letter.
3. **Do not end the subject line with a period.**
4. **Wrap the body at 72 characters.**
5. **Use the body to explain *what* and *why* vs. *how*.**
6. **Traceability:** If the commit implements a specific requirement, include the Requirement ID (`[Type]-[Domain]-[Seq]`) in the footer (e.g., `Refs: REQ-CORE-001`).

## Branch & Pull Request Workflow

### 1. Branch Policy & Naming Convention
- **No direct pushes to `main`:** `main` is protected. Every change must be made on a dedicated working branch and merged through a Pull Request.
- **Required branch name format (enforced by CI Gate 1):** `<type>/<short-kebab-slug>`
  - Pattern: `^(feat|fix|docs|style|refactor|perf|test|chore)/[a-z0-9][a-z0-9._-]*$`
  - Examples: `feat/cli-harness`, `fix/mcp-discover`, `docs/epic-obs-01`, `perf/sqlite-reader-pool`

### 2. Pull Request & Squash-Only Merge Process
1. Ensure your changes follow [`docs/architecture.md`](docs/architecture.md) and update SSOT documentation ([`docs/requirements/index.md`](docs/requirements/index.md), `docs/`, `README.md`) before modifying code (**Update First**).
2. Commit freely in small atomic or work-in-progress (WIP) increments on your working branch during development.
3. Open a Pull Request using [`.github/pull_request_template.md`](.github/pull_request_template.md):
   - Set the **PR title** to a valid Conventional Commits header (`<type>[optional scope]: <Capitalized description>` without a trailing period).
   - Include traceability references (`Refs: EPIC-... / REQ-...` or `Closes #...`) in the PR description or commits whenever changing `core/` or `skill/`.
4. Pass all 3 CI Merge Gates in [`.github/workflows/ci.yml`](.github/workflows/ci.yml):
   - **Gate 1:** Zero third-party dependencies, Knowledge Map & SSOT link integrity, branch naming, PR title, and traceability.
   - **Gate 2:** Zero compiler warnings and full workspace tests (`RUSTFLAGS="-D warnings" cargo test --workspace`).
   - **Gate 3:** Deterministic static invariants verification (`omk bench --docs 500 --turns 20 --json`).
5. **Squash and merge only:** Merge into `main` exclusively via **Squash and merge** so `main` stays linear with one clean commit per PR, using the Conventional Commits PR title as the commit header and preserving traceability footers in the commit body.
6. **Release Tagging (`v*`):** Release tags (e.g., `v0.1.0`) must only be created on commits already merged into `main`. [`.github/workflows/release.yml`](.github/workflows/release.yml) verifies `git merge-base --is-ancestor <tag_sha> origin/main` before building or publishing binaries.

## Coding Conventions & Testing
Please refer to the [Coding Conventions & Testing Strategy](docs/coding_conventions.md) document for rules regarding Rust documentation, error handling, and testing strategies. All code must adhere to these guidelines.
