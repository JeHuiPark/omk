#!/usr/bin/env python3
"""Version SSOT and automated release governance engine for omk.

Uses Python 3.11+ standard library only (zero third-party dependencies).
Shared by CI Gate 1.4, release-pr.yml, and release.yml.

Subcommands:
    check-ssot            Verify Version SSOT, workspace inheritance, Cargo.lock,
                          protocol_spec.json sync, and zero hardcoded version literals.
    detect-release-commit Detect whether a commit subject is a release PR Squash merge
                          commit (`chore(release): Prepare vX.Y.Z`).
    plan                  Compute the next SemVer version and CHANGELOG entries from
                          Conventional Commits since the latest `v*` tag.
    bump                  Update Cargo.toml, Cargo.lock, docs/protocol_spec.json,
                          and CHANGELOG.md for a target version.
    verify-release        Run the 5-way Version Integrity Gate across tag, Cargo.toml,
                          Cargo.lock, docs/protocol_spec.json, and optional binary.
    extract-notes         Extract release notes for a `vX.Y.Z` tag from CHANGELOG.md.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tomllib
from dataclasses import asdict, dataclass
from typing import Any

SEMVER_RE = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
TAG_RE = re.compile(r"^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
RELEASE_COMMIT_RE = re.compile(
    r"^chore\(release\): Prepare v((?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*))(?:\s+\(#\d+\))?$"
)
COMMIT_HEADER_RE = re.compile(
    r"^(?P<type>feat|fix|docs|style|refactor|perf|test|chore)"
    r"(?:\((?P<scope>[a-zA-Z0-9._/-]+)\))?"
    r"(?P<breaking>!)?:\s+(?P<desc>.+)$"
)
BREAKING_FOOTER_RE = re.compile(r"^BREAKING[ -]CHANGE:\s*(.+)$", re.MULTILINE)

HARDCODED_PATTERNS = [
    (
        "hardcoded VERSION constant",
        re.compile(r'\bconst\s+[A-Z0-9_]*VERSION[A-Z0-9_]*\s*:\s*&str\s*=\s*"\d+\.\d+\.\d+"'),
    ),
    (
        "hardcoded JSON version literal",
        re.compile(r'"version"\s*:\s*"\d+\.\d+\.\d+"'),
    ),
    (
        "hardcoded Some(<semver>) assertion",
        re.compile(r'Some\(\s*"\d+\.\d+\.\d+"\s*\)'),
    ),
]

SECTION_ORDER = [
    "Breaking Changes",
    "Features",
    "Bug Fixes",
    "Performance",
    "Refactoring",
]

EXPECTED_PACKAGES = ("omk", "omk-core")


@dataclass(frozen=True)
class CommitEntry:
    sha: str
    commit_type: str
    scope: str | None
    description: str
    breaking: bool
    breaking_note: str | None = None


@dataclass(frozen=True)
class ReleasePlan:
    should_release: bool
    current_version: str
    next_version: str
    next_tag: str
    latest_tag: str | None
    bump_type: str
    release_branch: str
    pr_title: str
    commit_count: int
    sections: dict[str, list[str]]
    changelog_section: str


def emit_error(message: str) -> None:
    if os.environ.get("GITHUB_ACTIONS") == "true":
        print(f"::error::{message}", file=sys.stderr)
    else:
        print(f"ERROR: {message}", file=sys.stderr)


def parse_semver(version: str) -> tuple[int, int, int]:
    match = SEMVER_RE.match(version.strip())
    if not match:
        raise ValueError(f"Invalid strict SemVer string: {version!r} (expected X.Y.Z)")
    return int(match.group(1)), int(match.group(2)), int(match.group(3))


def format_semver(parts: tuple[int, int, int]) -> str:
    return f"{parts[0]}.{parts[1]}.{parts[2]}"


def parse_tag(tag: str) -> tuple[int, int, int]:
    match = TAG_RE.match(tag.strip())
    if not match:
        raise ValueError(f"Invalid release tag: {tag!r} (expected vX.Y.Z)")
    return int(match.group(1)), int(match.group(2)), int(match.group(3))


def detect_release_commit_subject(subject: str) -> str | None:
    match = RELEASE_COMMIT_RE.match(subject.strip())
    return match.group(1) if match else None


def read_toml(path: pathlib.Path) -> dict[str, Any]:
    with path.open("rb") as f:
        return tomllib.load(f)


def read_workspace_version(repo_root: pathlib.Path) -> str:
    root_toml = read_toml(repo_root / "Cargo.toml")
    ws_pkg = root_toml.get("workspace", {}).get("package", {})
    version = ws_pkg.get("version")
    if not isinstance(version, str):
        raise ValueError("Cargo.toml must define [workspace.package].version as a string")
    parse_semver(version)
    return version


def strip_rust_comments(source: str) -> str:
    """Strip Rust // and /* */ comments while preserving string literals."""
    out: list[str] = []
    i = 0
    n = len(source)
    while i < n:
        ch = source[i]
        nxt = source[i + 1] if i + 1 < n else ""

        # Line comment
        if ch == "/" and nxt == "/":
            while i < n and source[i] != "\n":
                i += 1
            continue

        # Block comment (supports nesting in Rust)
        if ch == "/" and nxt == "*":
            depth = 1
            i += 2
            while i < n and depth > 0:
                if source[i] == "/" and i + 1 < n and source[i + 1] == "*":
                    depth += 1
                    i += 2
                elif source[i] == "*" and i + 1 < n and source[i + 1] == "/":
                    depth -= 1
                    i += 2
                else:
                    if source[i] == "\n":
                        out.append("\n")
                    i += 1
            continue

        # Raw string literal: r"...", r#"..."#, br#"..."#
        if ch == "r" or (ch == "b" and nxt == "r"):
            start = i
            j = i + (2 if ch == "b" else 1)
            hashes = 0
            while j < n and source[j] == "#":
                hashes += 1
                j += 1
            if j < n and source[j] == '"':
                j += 1
                end_delim = '"' + ("#" * hashes)
                end_pos = source.find(end_delim, j)
                if end_pos == -1:
                    out.append(source[start:])
                    break
                end_idx = end_pos + len(end_delim)
                out.append(source[start:end_idx])
                i = end_idx
                continue

        # Normal string literal
        if ch == '"':
            out.append(ch)
            i += 1
            while i < n:
                c = source[i]
                out.append(c)
                if c == "\\" and i + 1 < n:
                    out.append(source[i + 1])
                    i += 2
                    continue
                i += 1
                if c == '"':
                    break
            continue

        out.append(ch)
        i += 1

    return "".join(out)


def read_protocol_spec_versions(spec_path: pathlib.Path) -> tuple[dict[str, Any], list[str]]:
    errors: list[str] = []
    if not spec_path.exists():
        return {}, [f"Missing protocol specification file: {spec_path}"]
    try:
        data = json.loads(spec_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        return {}, [f"Invalid JSON in {spec_path}: {exc}"]

    versions: dict[str, Any] = {
        "$.version": data.get("version"),
        '$.lifecycle["server/discover"].result.serverInfo.version': (
            data.get("lifecycle", {})
            .get("server/discover", {})
            .get("result", {})
            .get("serverInfo", {})
            .get("version")
        ),
        '$.lifecycle["initialize"].result.serverInfo.version': (
            data.get("lifecycle", {})
            .get("initialize", {})
            .get("result", {})
            .get("serverInfo", {})
            .get("version")
        ),
    }
    return versions, errors


def read_cargo_lock_versions(lock_path: pathlib.Path) -> tuple[dict[str, str], list[str]]:
    errors: list[str] = []
    if not lock_path.exists():
        return {}, [f"Missing lockfile: {lock_path}"]
    try:
        lock_data = read_toml(lock_path)
    except tomllib.TOMLDecodeError as exc:
        return {}, [f"Invalid TOML in {lock_path}: {exc}"]

    pkg_versions: dict[str, str] = {}
    for pkg in lock_data.get("package", []):
        name = pkg.get("name")
        version = pkg.get("version")
        if isinstance(name, str) and isinstance(version, str):
            pkg_versions[name] = version
    return pkg_versions, errors


def check_ssot(repo_root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    root_cargo = repo_root / "Cargo.toml"
    if not root_cargo.exists():
        return [f"Missing workspace root Cargo.toml: {root_cargo}"]

    try:
        root_toml = read_toml(root_cargo)
    except tomllib.TOMLDecodeError as exc:
        return [f"Invalid TOML in Cargo.toml: {exc}"]

    ws_section = root_toml.get("workspace", {})
    ws_pkg = ws_section.get("package", {})
    ws_version = ws_pkg.get("version")
    if not isinstance(ws_version, str) or not SEMVER_RE.match(ws_version):
        errors.append(
            f"Cargo.toml [workspace.package].version must be strict SemVer (X.Y.Z), found: {ws_version!r}"
        )
        return errors

    # 1. Workspace members must inherit version, edition, and license from [workspace.package].
    members = ws_section.get("members", [])
    for required_member in ("core", "skill"):
        if required_member not in members:
            errors.append(f"Cargo.toml [workspace].members missing required member: {required_member!r}")

    for member in members:
        member_toml_path = repo_root / member / "Cargo.toml"
        if not member_toml_path.exists():
            errors.append(f"Missing member Cargo.toml: {member}/Cargo.toml")
            continue
        member_toml = read_toml(member_toml_path)
        pkg = member_toml.get("package", {})
        for field in ("version", "edition", "license"):
            val = pkg.get(field)
            if val != {"workspace": True}:
                errors.append(
                    f"{member}/Cargo.toml [package].{field} must inherit from workspace "
                    f"({field}.workspace = true), found: {val!r}"
                )

    # 2. Cargo.lock workspace packages must match [workspace.package].version.
    lock_versions, lock_errors = read_cargo_lock_versions(repo_root / "Cargo.lock")
    errors.extend(lock_errors)
    if not lock_errors:
        for pkg_name in EXPECTED_PACKAGES:
            actual = lock_versions.get(pkg_name)
            if actual != ws_version:
                errors.append(
                    f"Cargo.lock package {pkg_name!r} version mismatch: expected {ws_version!r}, found {actual!r}"
                )

    # 3. Verify Rust sources and integration tests do not hardcode versions.
    main_rs = repo_root / "skill" / "src" / "main.rs"
    if main_rs.exists():
        main_code = strip_rust_comments(main_rs.read_text(encoding="utf-8"))
        if 'const VERSION: &str = env!("CARGO_PKG_VERSION");' not in main_code:
            errors.append(
                'skill/src/main.rs must declare `const VERSION: &str = env!("CARGO_PKG_VERSION");`'
            )
    else:
        errors.append("Missing skill/src/main.rs")

    protocol_rs = repo_root / "skill" / "src" / "protocol" / "mod.rs"
    if protocol_rs.exists():
        protocol_code = strip_rust_comments(protocol_rs.read_text(encoding="utf-8"))
        if 'env!("CARGO_PKG_VERSION")' not in protocol_code:
            errors.append(
                'skill/src/protocol/mod.rs must derive serverInfo.version from env!("CARGO_PKG_VERSION")'
            )
    else:
        errors.append("Missing skill/src/protocol/mod.rs")

    scan_roots = [
        repo_root / "core" / "src",
        repo_root / "skill" / "src",
        repo_root / "tests",
    ]
    exact_version_re = re.compile(rf'"{re.escape(ws_version)}"')
    for scan_root in scan_roots:
        if not scan_root.exists():
            continue
        for rs_file in sorted(scan_root.rglob("*.rs")):
            rel = rs_file.relative_to(repo_root)
            stripped = strip_rust_comments(rs_file.read_text(encoding="utf-8"))
            for lineno, line in enumerate(stripped.splitlines(), start=1):
                if exact_version_re.search(line):
                    errors.append(
                        f'{rel}:{lineno}: hardcoded workspace version literal "{ws_version}" detected; '
                        'use env!("CARGO_PKG_VERSION") instead'
                    )
                    continue
                for label, pattern in HARDCODED_PATTERNS:
                    if pattern.search(line):
                        errors.append(
                            f'{rel}:{lineno}: {label} detected ({line.strip()}); '
                            'use env!("CARGO_PKG_VERSION") instead'
                        )
                        break

    # 4. Verify docs/protocol_spec.json 3 version fields match workspace version.
    spec_versions, spec_errors = read_protocol_spec_versions(repo_root / "docs" / "protocol_spec.json")
    errors.extend(spec_errors)
    for field_path, actual in spec_versions.items():
        if actual != ws_version:
            errors.append(
                f"docs/protocol_spec.json {field_path} mismatch: expected {ws_version!r}, found {actual!r}"
            )

    return errors


def git_run(repo_root: pathlib.Path, args: list[str]) -> str:
    proc = subprocess.run(
        ["git", *args],
        cwd=repo_root,
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed (exit {proc.returncode}): {proc.stderr.strip()}")
    return proc.stdout


def find_latest_semver_tag(repo_root: pathlib.Path, head_ref: str = "HEAD") -> str | None:
    raw = git_run(repo_root, ["tag", "--merged", head_ref, "--list", "v*"])
    candidates: list[tuple[tuple[int, int, int], str]] = []
    for line in raw.splitlines():
        tag = line.strip()
        if not tag:
            continue
        match = TAG_RE.match(tag)
        if match:
            semver = (int(match.group(1)), int(match.group(2)), int(match.group(3)))
            candidates.append((semver, tag))
    if not candidates:
        return None
    candidates.sort(key=lambda item: item[0])
    return candidates[-1][1]


def parse_conventional_commit(sha: str, subject: str, body: str) -> CommitEntry | None:
    subject_clean = subject.strip()
    if not subject_clean:
        return None
    if detect_release_commit_subject(subject_clean) is not None:
        return None

    match = COMMIT_HEADER_RE.match(subject_clean)
    if not match:
        return None

    commit_type = match.group("type")
    scope = match.group("scope")
    desc = match.group("desc").strip()
    breaking_bang = bool(match.group("breaking"))
    footer_match = BREAKING_FOOTER_RE.search(body)
    breaking = breaking_bang or (footer_match is not None)
    breaking_note = footer_match.group(1).strip() if footer_match else None

    return CommitEntry(
        sha=sha.strip(),
        commit_type=commit_type,
        scope=scope,
        description=desc,
        breaking=breaking,
        breaking_note=breaking_note,
    )


def collect_commits(repo_root: pathlib.Path, latest_tag: str | None, head_ref: str = "HEAD") -> list[CommitEntry]:
    rev_range = f"{latest_tag}..{head_ref}" if latest_tag else head_ref
    raw = git_run(
        repo_root,
        ["log", "--no-merges", "--format=%H%x1f%s%x1f%b%x1e", rev_range],
    )
    entries: list[CommitEntry] = []
    for record in raw.split("\x1e"):
        if not record.strip():
            continue
        parts = record.split("\x1f")
        if len(parts) < 2:
            continue
        sha = parts[0].strip()
        subject = parts[1].strip()
        body = parts[2] if len(parts) > 2 else ""
        entry = parse_conventional_commit(sha, subject, body)
        if entry is not None:
            entries.append(entry)
    return entries


def format_commit_bullet(entry: CommitEntry) -> str:
    short_sha = entry.sha[:7] if entry.sha else ""
    prefix = f"**{entry.scope}:** " if entry.scope else ""
    text = f"{prefix}{entry.description}"
    if entry.breaking and entry.breaking_note and entry.breaking_note != entry.description:
        text = f"{text} ({entry.breaking_note})"
    if short_sha:
        return f"- {text} (`{short_sha}`)"
    return f"- {text}"


def group_commits_for_changelog(commits: list[CommitEntry]) -> dict[str, list[str]]:
    grouped: dict[str, list[str]] = {name: [] for name in SECTION_ORDER}
    for entry in commits:
        if entry.breaking:
            grouped["Breaking Changes"].append(format_commit_bullet(entry))
        elif entry.commit_type == "feat":
            grouped["Features"].append(format_commit_bullet(entry))
        elif entry.commit_type == "fix":
            grouped["Bug Fixes"].append(format_commit_bullet(entry))
        elif entry.commit_type == "perf":
            grouped["Performance"].append(format_commit_bullet(entry))
        elif entry.commit_type == "refactor":
            grouped["Refactoring"].append(format_commit_bullet(entry))
        # docs, style, test, chore are omitted from CHANGELOG.md
    return {k: v for k, v in grouped.items() if v}


def render_changelog_section(
    version: str,
    release_date: str,
    sections: dict[str, list[str]],
) -> str:
    lines: list[str] = [f"## [v{version}] - {release_date}", ""]
    has_content = False
    for section_name in SECTION_ORDER:
        items = sections.get(section_name, [])
        if not items:
            continue
        has_content = True
        lines.append(f"### {section_name}")
        lines.append("")
        lines.extend(items)
        lines.append("")
    if not has_content:
        lines.append("### Maintenance")
        lines.append("")
        lines.append("- Maintenance release.")
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def compute_next_version(
    current_version: str,
    latest_tag: str | None,
    commits: list[CommitEntry],
    release_as: str | None = None,
    release_date: str | None = None,
) -> ReleasePlan:
    cur_tuple = parse_semver(current_version)
    latest_tuple = parse_tag(latest_tag) if latest_tag else None
    date_str = release_date or dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%d")
    sections = group_commits_for_changelog(commits)

    release_as_clean = (release_as or "").strip()
    if release_as_clean:
        target_tuple = parse_semver(release_as_clean)
        if target_tuple == (0, 0, 0):
            raise ValueError("release_as cannot be 0.0.0")
        if latest_tuple is not None and target_tuple <= latest_tuple:
            raise ValueError(
                f"release_as ({release_as_clean}) must be strictly greater than latest tag ({latest_tag})"
            )
        next_version = format_semver(target_tuple)
        bump_type = "manual"
        should_release = True
    else:
        has_breaking = any(c.breaking for c in commits)
        has_feat = any(c.commit_type == "feat" for c in commits)
        has_patch = any(c.commit_type in ("fix", "perf") for c in commits)

        if not (has_breaking or has_feat or has_patch):
            return ReleasePlan(
                should_release=False,
                current_version=current_version,
                next_version=current_version,
                next_tag=f"v{current_version}",
                latest_tag=latest_tag,
                bump_type="none",
                release_branch=f"chore/release-v{current_version}",
                pr_title=f"chore(release): Prepare v{current_version}",
                commit_count=len(commits),
                sections=sections,
                changelog_section="",
            )

        if latest_tuple is None:
            # Initial bootstrap release: keep Cargo.toml's initial version (e.g., 0.1.0)
            # instead of skipping straight to 0.2.0.
            next_version = current_version
            bump_type = "bootstrap"
            should_release = True
        else:
            base = max(cur_tuple, latest_tuple)
            major, minor, patch = base
            if has_breaking:
                if major == 0:
                    # Policy 3: protect 0.y.z from accidental 1.0.0 promotion.
                    next_tuple = (0, minor + 1, 0)
                    bump_type = "minor"
                else:
                    next_tuple = (major + 1, 0, 0)
                    bump_type = "major"
            elif has_feat:
                next_tuple = (major, minor + 1, 0)
                bump_type = "minor"
            else:
                next_tuple = (major, minor, patch + 1)
                bump_type = "patch"
            next_version = format_semver(next_tuple)
            should_release = True

    changelog_section = render_changelog_section(next_version, date_str, sections)
    return ReleasePlan(
        should_release=should_release,
        current_version=current_version,
        next_version=next_version,
        next_tag=f"v{next_version}",
        latest_tag=latest_tag,
        bump_type=bump_type,
        release_branch=f"chore/release-v{next_version}",
        pr_title=f"chore(release): Prepare v{next_version}",
        commit_count=len(commits),
        sections=sections,
        changelog_section=changelog_section,
    )


def update_root_cargo_toml(cargo_path: pathlib.Path, version: str) -> None:
    parse_semver(version)
    text = cargo_path.read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)
    in_ws_pkg = False
    replaced = False
    version_line_re = re.compile(r'^(\s*version\s*=\s*")([^"]+)(".*)$')

    new_lines: list[str] = []
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            in_ws_pkg = stripped == "[workspace.package]"
        elif in_ws_pkg and not replaced:
            m = version_line_re.match(line.rstrip("\r\n"))
            if m:
                newline = "\r\n" if line.endswith("\r\n") else ("\n" if line.endswith("\n") else "")
                line = f"{m.group(1)}{version}{m.group(3)}{newline}"
                replaced = True
        new_lines.append(line)

    if not replaced:
        raise RuntimeError(f"Failed to locate [workspace.package].version in {cargo_path}")
    cargo_path.write_text("".join(new_lines), encoding="utf-8")


def update_cargo_lock(lock_path: pathlib.Path, version: str, repo_root: pathlib.Path, run_cargo: bool = True) -> None:
    parse_semver(version)
    text = lock_path.read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)

    current_pkg: str | None = None
    updated_pkgs: set[str] = set()
    name_re = re.compile(r'^\s*name\s*=\s*"([^"]+)"\s*$')
    ver_re = re.compile(r'^(\s*version\s*=\s*")([^"]+)(".*)$')

    new_lines: list[str] = []
    for line in lines:
        stripped = line.strip()
        if stripped == "[[package]]":
            current_pkg = None
        else:
            nm = name_re.match(stripped)
            if nm:
                current_pkg = nm.group(1)
            elif current_pkg in EXPECTED_PACKAGES:
                vm = ver_re.match(line.rstrip("\r\n"))
                if vm:
                    newline = "\r\n" if line.endswith("\r\n") else ("\n" if line.endswith("\n") else "")
                    line = f"{vm.group(1)}{version}{vm.group(3)}{newline}"
                    updated_pkgs.add(current_pkg)
        new_lines.append(line)

    missing = set(EXPECTED_PACKAGES) - updated_pkgs
    if missing:
        raise RuntimeError(f"Failed to update packages {sorted(missing)} in {lock_path}")

    lock_path.write_text("".join(new_lines), encoding="utf-8")

    if run_cargo and shutil.which("cargo"):
        proc = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked"],
            cwd=repo_root,
            capture_output=True,
            text=True,
            check=False,
        )
        if proc.returncode != 0:
            raise RuntimeError(
                f"cargo metadata --locked failed after Cargo.lock update: {proc.stderr.strip()}"
            )


def update_protocol_spec(spec_path: pathlib.Path, version: str) -> None:
    parse_semver(version)
    original_text = spec_path.read_text(encoding="utf-8")
    data = json.loads(original_text)

    # Update the 3 canonical version fields while preserving minimal diff if possible.
    old_top = data.get("version")
    old_disc = (
        data.get("lifecycle", {})
        .get("server/discover", {})
        .get("result", {})
        .get("serverInfo", {})
        .get("version")
    )
    old_init = (
        data.get("lifecycle", {})
        .get("initialize", {})
        .get("result", {})
        .get("serverInfo", {})
        .get("version")
    )

    if old_top == old_disc == old_init and isinstance(old_top, str):
        pattern = re.compile(rf'("version"\s*:\s*"){re.escape(old_top)}(")')
        updated_text, count = pattern.subn(rf"\g<1>{version}\g<2>", original_text)
        if count == 3:
            parsed = json.loads(updated_text)
            if (
                parsed.get("version") == version
                and parsed["lifecycle"]["server/discover"]["result"]["serverInfo"]["version"] == version
                and parsed["lifecycle"]["initialize"]["result"]["serverInfo"]["version"] == version
            ):
                spec_path.write_text(updated_text, encoding="utf-8")
                return

    data["version"] = version
    data["lifecycle"]["server/discover"]["result"]["serverInfo"]["version"] = version
    data["lifecycle"]["initialize"]["result"]["serverInfo"]["version"] = version
    spec_path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def update_changelog_text(existing: str, version: str, new_section: str) -> str:
    parse_semver(version)
    section_clean = new_section.strip() + "\n"
    if not existing.strip():
        header = (
            "# Changelog\n\n"
            "All notable changes to `omk` will be documented in this file.\n\n"
            "The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),\n"
            "and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).\n\n"
            "## [Unreleased]\n"
        )
        return f"{header}\n{section_clean}"

    # Replace existing section for the same version if already present (idempotent updates).
    existing_ver_re = re.compile(
        rf"(?ms)^## \[v?{re.escape(version)}\][^\n]*\n.*?(?=^## \[|\Z)"
    )
    if existing_ver_re.search(existing):
        replaced = existing_ver_re.sub(section_clean + "\n", existing, count=1)
        return re.sub(r"\n{3,}", "\n\n", replaced).rstrip() + "\n"

    # Otherwise insert immediately after the ## [Unreleased] heading (preserving any unreleased content).
    unreleased_re = re.compile(r"(?m)^## \[Unreleased\]\s*$")
    match = unreleased_re.search(existing)
    if match:
        next_heading = re.search(r"(?m)^## \[", existing[match.end() :])
        if next_heading:
            insert_pos = match.end() + next_heading.start()
            before = existing[:insert_pos].rstrip()
            after = existing[insert_pos:].lstrip()
            return f"{before}\n\n{section_clean}\n{after}".rstrip() + "\n"
        before = existing.rstrip()
        return f"{before}\n\n{section_clean}"

    # Fallback: insert before the first ## [ heading or append.
    first_heading = re.search(r"(?m)^## \[", existing)
    if first_heading:
        before = existing[: first_heading.start()].rstrip()
        after = existing[first_heading.start() :].lstrip()
        return f"{before}\n\n{section_clean}\n{after}".rstrip() + "\n"

    return f"{existing.rstrip()}\n\n{section_clean}"


def extract_release_notes(changelog_text: str, tag: str) -> str:
    version = format_semver(parse_tag(tag))
    pattern = re.compile(
        rf"(?ms)^## \[v?{re.escape(version)}\][^\n]*\n(.*?)(?=^## \[|\Z)"
    )
    match = pattern.search(changelog_text)
    if not match:
        raise ValueError(f"Release section for {tag} (v{version}) not found in CHANGELOG.md")
    body = match.group(1).strip()
    if not body:
        raise ValueError(f"Release section for {tag} in CHANGELOG.md is empty")
    return body + "\n"


def verify_release(repo_root: pathlib.Path, tag: str, binary: pathlib.Path | None = None) -> list[str]:
    errors: list[str] = []
    try:
        expected_version = format_semver(parse_tag(tag))
    except ValueError as exc:
        return [str(exc)]

    # Base SSOT checks (workspace inheritance, Cargo.lock, no hardcoding, protocol_spec sync)
    ssot_errors = check_ssot(repo_root)
    errors.extend(ssot_errors)

    # Check 1 & 2: Tag vs Cargo.toml [workspace.package].version
    try:
        ws_version = read_workspace_version(repo_root)
        if ws_version != expected_version:
            errors.append(
                f"Tag/Cargo.toml version mismatch: tag={tag} expects {expected_version!r}, "
                f"Cargo.toml has {ws_version!r}"
            )
    except ValueError as exc:
        errors.append(str(exc))

    # Check 3: Tag vs Cargo.lock
    lock_versions, lock_errors = read_cargo_lock_versions(repo_root / "Cargo.lock")
    if not lock_errors:
        for pkg_name in EXPECTED_PACKAGES:
            if lock_versions.get(pkg_name) != expected_version:
                errors.append(
                    f"Tag/Cargo.lock mismatch for {pkg_name!r}: expected {expected_version!r}, "
                    f"found {lock_versions.get(pkg_name)!r}"
                )

    # Check 4: Tag vs docs/protocol_spec.json (3 fields)
    spec_versions, spec_errors = read_protocol_spec_versions(repo_root / "docs" / "protocol_spec.json")
    if not spec_errors:
        for field_path, actual in spec_versions.items():
            if actual != expected_version:
                errors.append(
                    f"Tag/protocol_spec.json mismatch at {field_path}: expected {expected_version!r}, "
                    f"found {actual!r}"
                )

    # Check 5: Runtime binary --version output
    if binary is not None:
        if not binary.exists():
            errors.append(f"Release binary not found for runtime verification: {binary}")
        else:
            proc = subprocess.run(
                [str(binary), "--version"],
                capture_output=True,
                text=True,
                check=False,
            )
            if proc.returncode != 0:
                errors.append(
                    f"Binary --version exited with code {proc.returncode}: {proc.stderr.strip()}"
                )
            else:
                expected_output = f"omk {expected_version}"
                actual_output = proc.stdout.strip()
                if actual_output != expected_output:
                    errors.append(
                        f"Binary --version output mismatch: expected {expected_output!r}, "
                        f"got {actual_output!r}"
                    )

    # Deduplicate while preserving order.
    deduped: list[str] = []
    seen: set[str] = set()
    for err in errors:
        if err not in seen:
            seen.add(err)
            deduped.append(err)
    return deduped


def write_github_outputs(path: pathlib.Path, values: dict[str, Any]) -> None:
    with path.open("a", encoding="utf-8") as fh:
        for key, raw_val in values.items():
            if isinstance(raw_val, bool):
                val = "true" if raw_val else "false"
            elif raw_val is None:
                val = ""
            else:
                val = str(raw_val)
            if "\n" in val:
                delimiter = "EOF_RELEASE_GOV_OUTPUT"
                fh.write(f"{key}<<{delimiter}\n{val}\n{delimiter}\n")
            else:
                fh.write(f"{key}={val}\n")


def cmd_check_ssot(args: argparse.Namespace) -> int:
    repo_root = pathlib.Path(args.repo_root).resolve()
    errors = check_ssot(repo_root)
    if errors:
        for err in errors:
            emit_error(err)
        return 1
    ws_version = read_workspace_version(repo_root)
    print(
        f"Gate 1.4 passed: Version SSOT ({ws_version}) verified across Cargo.toml, "
        "member manifests, Cargo.lock, Rust sources/tests, and docs/protocol_spec.json."
    )
    return 0


def cmd_detect_release_commit(args: argparse.Namespace) -> int:
    repo_root = pathlib.Path(args.repo_root).resolve()
    subject = args.subject
    if subject is None:
        subject = git_run(repo_root, ["log", "-1", "--format=%s", args.commit]).strip()
    version = detect_release_commit_subject(subject)
    result = {
        "is_release": version is not None,
        "version": version or "",
        "tag": f"v{version}" if version else "",
        "subject": subject,
    }
    if args.github_output:
        write_github_outputs(pathlib.Path(args.github_output), result)
    if args.json:
        print(json.dumps(result, indent=2))
    else:
        if version:
            print(f"Release commit detected: v{version} ({subject})")
        else:
            print(f"Not a release commit: {subject}")
    return 0


def cmd_plan(args: argparse.Namespace) -> int:
    repo_root = pathlib.Path(args.repo_root).resolve()
    current_version = read_workspace_version(repo_root)
    latest_tag = find_latest_semver_tag(repo_root, args.head_ref)
    commits = collect_commits(repo_root, latest_tag, args.head_ref)
    plan = compute_next_version(
        current_version=current_version,
        latest_tag=latest_tag,
        commits=commits,
        release_as=args.release_as,
        release_date=args.date,
    )
    plan_dict = asdict(plan)

    if args.github_output:
        write_github_outputs(
            pathlib.Path(args.github_output),
            {
                "should_release": plan.should_release,
                "current_version": plan.current_version,
                "next_version": plan.next_version,
                "next_tag": plan.next_tag,
                "latest_tag": plan.latest_tag or "",
                "bump_type": plan.bump_type,
                "release_branch": plan.release_branch,
                "pr_title": plan.pr_title,
                "commit_count": plan.commit_count,
            },
        )

    if args.json:
        print(json.dumps(plan_dict, indent=2))
    else:
        if plan.should_release:
            print(
                f"Release planned: {plan.current_version} -> {plan.next_version} "
                f"(bump={plan.bump_type}, latest_tag={plan.latest_tag or 'none'}, "
                f"commits={plan.commit_count}, branch={plan.release_branch})"
            )
        else:
            print(
                f"No release needed (current={plan.current_version}, "
                f"latest_tag={plan.latest_tag or 'none'}, commits={plan.commit_count})."
            )
    return 0


def cmd_bump(args: argparse.Namespace) -> int:
    repo_root = pathlib.Path(args.repo_root).resolve()
    target_version = format_semver(parse_semver(args.version))
    date_str = args.date or dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%d")

    latest_tag = find_latest_semver_tag(repo_root, args.head_ref)
    commits = collect_commits(repo_root, latest_tag, args.head_ref)
    sections = group_commits_for_changelog(commits)
    new_section = render_changelog_section(target_version, date_str, sections)

    update_root_cargo_toml(repo_root / "Cargo.toml", target_version)
    update_cargo_lock(
        repo_root / "Cargo.lock",
        target_version,
        repo_root=repo_root,
        run_cargo=not args.skip_cargo_lock_check,
    )
    update_protocol_spec(repo_root / "docs" / "protocol_spec.json", target_version)

    changelog_path = repo_root / "CHANGELOG.md"
    existing_changelog = changelog_path.read_text(encoding="utf-8") if changelog_path.exists() else ""
    updated_changelog = update_changelog_text(existing_changelog, target_version, new_section)
    changelog_path.write_text(updated_changelog, encoding="utf-8")

    print(
        f"Bumped version to {target_version} across Cargo.toml, Cargo.lock, "
        f"docs/protocol_spec.json, and CHANGELOG.md ({date_str})."
    )
    return 0


def cmd_verify_release(args: argparse.Namespace) -> int:
    repo_root = pathlib.Path(args.repo_root).resolve()
    binary = pathlib.Path(args.binary).resolve() if args.binary else None
    errors = verify_release(repo_root, args.tag, binary=binary)
    if errors:
        for err in errors:
            emit_error(err)
        return 1
    checks = "5/5 (including runtime binary --version)" if binary else "4/4 (pre-build manifests & specs)"
    print(f"Version Integrity Gate passed for {args.tag}: {checks}.")
    return 0


def cmd_extract_notes(args: argparse.Namespace) -> int:
    repo_root = pathlib.Path(args.repo_root).resolve()
    changelog_path = repo_root / "CHANGELOG.md"
    if not changelog_path.exists():
        emit_error(f"Missing CHANGELOG.md at {changelog_path}")
        return 1
    try:
        notes = extract_release_notes(changelog_path.read_text(encoding="utf-8"), args.tag)
    except ValueError as exc:
        emit_error(str(exc))
        return 1

    if args.output:
        out_path = pathlib.Path(args.output)
        out_path.write_text(notes, encoding="utf-8")
    else:
        sys.stdout.write(notes)
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="release_gov.py",
        description="Version SSOT and automated release governance engine for omk.",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p_check = sub.add_parser("check-ssot", help="Verify Version SSOT and zero hardcoded version literals.")
    p_check.add_argument("--repo-root", default=".", help="Repository root directory.")
    p_check.set_defaults(func=cmd_check_ssot)

    p_detect = sub.add_parser(
        "detect-release-commit",
        help="Detect if a commit subject is a release PR Squash commit.",
    )
    p_detect.add_argument("--repo-root", default=".", help="Repository root directory.")
    p_detect.add_argument("--commit", default="HEAD", help="Git commit ref to inspect.")
    p_detect.add_argument("--subject", default=None, help="Explicit subject string (skips git log).")
    p_detect.add_argument("--json", action="store_true", help="Emit JSON output.")
    p_detect.add_argument("--github-output", default=None, help="Path to $GITHUB_OUTPUT file.")
    p_detect.set_defaults(func=cmd_detect_release_commit)

    p_plan = sub.add_parser("plan", help="Plan next SemVer version from Conventional Commits.")
    p_plan.add_argument("--repo-root", default=".", help="Repository root directory.")
    p_plan.add_argument("--head-ref", default="HEAD", help="Git ref to evaluate up to.")
    p_plan.add_argument("--release-as", default=None, help="Explicit SemVer override (e.g. 1.0.0).")
    p_plan.add_argument("--date", default=None, help="Release date YYYY-MM-DD (defaults to UTC today).")
    p_plan.add_argument("--json", action="store_true", help="Emit JSON output.")
    p_plan.add_argument("--github-output", default=None, help="Path to $GITHUB_OUTPUT file.")
    p_plan.set_defaults(func=cmd_plan)

    p_bump = sub.add_parser("bump", help="Bump version in Cargo.toml, Cargo.lock, protocol_spec.json, CHANGELOG.md.")
    p_bump.add_argument("--repo-root", default=".", help="Repository root directory.")
    p_bump.add_argument("--version", required=True, help="Target SemVer version (X.Y.Z).")
    p_bump.add_argument("--head-ref", default="HEAD", help="Git ref used to build CHANGELOG entries.")
    p_bump.add_argument("--date", default=None, help="Release date YYYY-MM-DD (defaults to UTC today).")
    p_bump.add_argument(
        "--skip-cargo-lock-check",
        action="store_true",
        help="Skip running `cargo metadata --locked` after updating Cargo.lock.",
    )
    p_bump.set_defaults(func=cmd_bump)

    p_verify = sub.add_parser("verify-release", help="Run the 5-way Version Integrity Gate.")
    p_verify.add_argument("--repo-root", default=".", help="Repository root directory.")
    p_verify.add_argument("--tag", required=True, help="Release tag (vX.Y.Z).")
    p_verify.add_argument("--binary", default=None, help="Optional path to compiled omk binary.")
    p_verify.set_defaults(func=cmd_verify_release)

    p_notes = sub.add_parser("extract-notes", help="Extract CHANGELOG.md notes for a release tag.")
    p_notes.add_argument("--repo-root", default=".", help="Repository root directory.")
    p_notes.add_argument("--tag", required=True, help="Release tag (vX.Y.Z).")
    p_notes.add_argument("--output", default=None, help="Optional output file path.")
    p_notes.set_defaults(func=cmd_extract_notes)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return args.func(args)
    except Exception as exc:  # noqa: BLE001
        emit_error(str(exc))
        return 1


if __name__ == "__main__":
    sys.exit(main())
