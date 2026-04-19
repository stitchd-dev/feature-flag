#!/usr/bin/env python3
"""Compute workspace crates affected by the current push/PR.

Reads changed files from git and propagates through the internal dependency
graph (including dev-dependencies) to find all crates whose tests must run.

Writes to $GITHUB_OUTPUT:
  test-packages  – space-joined "-p crate" flags for `cargo test / tarpaulin`
  crate-names    – space-joined plain crate names (for per-crate loops)
  has-changes    – "true" if any testable crate is affected, else "false"
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from collections import defaultdict, deque

# Crates excluded from testing (generated code / build tools).
EXCLUDE: frozenset[str] = frozenset({"stitchd-proto", "xtask"})
CRATE_DIR = "crates"


# ---------------------------------------------------------------------------
# Dependency graph from Cargo.toml files (no Rust toolchain needed)
# ---------------------------------------------------------------------------

def workspace_crates() -> list[str]:
    try:
        return [
            d for d in os.listdir(CRATE_DIR)
            if os.path.isdir(os.path.join(CRATE_DIR, d))
        ]
    except FileNotFoundError:
        return []


def internal_deps(crate: str) -> set[str]:
    """Return workspace crates that `crate` depends on (normal + dev deps)."""
    path = os.path.join(CRATE_DIR, crate, "Cargo.toml")
    try:
        content = open(path).read()
    except FileNotFoundError:
        return set()
    # Match any `stitchd-*` dep that has `path =` (i.e., is a workspace member)
    return {
        m.group(1)
        for m in re.finditer(r'(stitchd-[\w-]+)\s*=\s*\{[^}]*path\s*=', content)
    }


def build_reverse_graph(crates: list[str]) -> dict[str, set[str]]:
    """Build reverse dep map: dep → {crates that depend on dep}."""
    rev: dict[str, set[str]] = defaultdict(set)
    for crate in crates:
        for dep in internal_deps(crate):
            if dep in crates:
                rev[dep].add(crate)
    return dict(rev)


# ---------------------------------------------------------------------------
# Git helpers
# ---------------------------------------------------------------------------

def git_changed_files(base: str, head: str) -> list[str]:
    result = subprocess.run(
        ["git", "diff", "--name-only", base, head],
        capture_output=True, text=True, check=True,
    )
    return [f for f in result.stdout.splitlines() if f]


# ---------------------------------------------------------------------------
# Core logic
# ---------------------------------------------------------------------------

WORKSPACE_TRIGGERS = re.compile(
    r'^(Cargo\.(toml|lock)|\.cargo/|\.github/workflows/)'
)


def affected_crates(
    changed_files: list[str],
    all_crates: list[str],
    rev_graph: dict[str, set[str]],
) -> set[str]:
    testable = set(all_crates) - EXCLUDE

    # Workspace-level file changed → run everything
    if any(WORKSPACE_TRIGGERS.match(f) for f in changed_files):
        return testable

    # Find directly changed crates
    direct: set[str] = set()
    for f in changed_files:
        parts = f.split("/")
        if len(parts) >= 2 and parts[0] == CRATE_DIR:
            crate = parts[1]
            if crate in testable:
                direct.add(crate)

    if not direct:
        return set()

    # BFS: expand to all crates that (transitively) depend on a changed crate
    affected = set(direct)
    queue = deque(direct)
    while queue:
        crate = queue.popleft()
        for dependent in rev_graph.get(crate, set()):
            if dependent not in affected and dependent in testable:
                affected.add(dependent)
                queue.append(dependent)

    return affected


# ---------------------------------------------------------------------------
# GitHub API helpers
# ---------------------------------------------------------------------------

def last_successful_sha(workflow: str, branch: str) -> str | None:
    """Return the HEAD SHA of the most recent successful CI run on `branch`.

    Uses the `gh` CLI (available on all GitHub-hosted runners).  Returns None
    if no successful run exists yet (e.g., a brand-new repo or branch).
    """
    result = subprocess.run(
        [
            "gh", "run", "list",
            f"--workflow={workflow}",
            f"--branch={branch}",
            "--status=success",
            "--limit=1",
            "--json=headSha",
            "--jq=.[0].headSha",
        ],
        capture_output=True,
        text=True,
    )
    sha = result.stdout.strip()
    # `gh` returns "null" (jq output) when the list is empty
    return sha if sha and sha != "null" else None


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    event        = os.environ.get("GITHUB_EVENT_NAME", "push")
    before       = os.environ.get("BEFORE", "")
    base_sha     = os.environ.get("BASE_SHA", "")
    head_sha     = os.environ.get("HEAD_SHA", "HEAD")
    branch       = os.environ.get("BRANCH", "main")
    workflow     = os.environ.get("WORKFLOW", "ci.yml")
    output_file  = os.environ.get("GITHUB_OUTPUT", "/dev/stdout")

    all_crates = workspace_crates()
    rev_graph  = build_reverse_graph(all_crates)
    testable   = set(all_crates) - EXCLUDE

    if event == "pull_request":
        # PRs: diff against the branch point, not last green (PR base is authoritative)
        base = base_sha or None
    else:
        # Pushes: find the last SHA that had a green CI run on this branch.
        # Falls back to github.event.before (previous tip) if no successful run
        # exists yet, and to None (run everything) if before is the null SHA.
        green = last_successful_sha(workflow, branch)
        if green:
            base = green
            print(f"Last successful run: {green}")
        elif before and before != "0" * 40:
            base = before
            print(f"No successful run found; falling back to event.before: {before}")
        else:
            base = None  # new branch / initial push → run everything
            print("No base SHA available; running all crates")

    if base is None:
        crates = testable
    else:
        changed = git_changed_files(base, head_sha)
        print(f"Changed files ({len(changed)}): {changed[:10]}{'...' if len(changed) > 10 else ''}")
        crates = affected_crates(changed, all_crates, rev_graph)

    has_changes   = "true" if crates else "false"
    crate_names   = " ".join(sorted(crates))
    test_packages = " ".join(f"-p {c}" for c in sorted(crates))

    print(f"Affected crates: {sorted(crates)}")
    print(f"test-packages:   {test_packages}")

    with open(output_file, "a") as fh:
        fh.write(f"test-packages={test_packages}\n")
        fh.write(f"crate-names={crate_names}\n")
        fh.write(f"has-changes={has_changes}\n")


if __name__ == "__main__":
    main()
