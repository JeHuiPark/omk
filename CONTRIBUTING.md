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

## Pull Request Process
1. Ensure your code follows the architectural guidelines outlined in [`docs/architecture.md`](docs/architecture.md).
2. Update the documentation (`README.md`, `docs/`) with details of any changes to the interface or architecture.
3. All PRs must pass the existing test suite (`cargo test`) and include new tests for new features.

## Coding Conventions & Testing
Please refer to the [Coding Conventions & Testing Strategy](docs/coding_conventions.md) document for rules regarding Rust documentation, error handling, and testing strategies. All code must adhere to these guidelines.
