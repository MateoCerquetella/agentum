from __future__ import annotations

import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


FIXTURE = Path(__file__).resolve().parents[1] / "examples" / "sdd-demo"
FORBIDDEN_ROOTS = {
    ".agentum",
    ".agentum-harness",
    ".aider.conf.yml",
    ".aiderignore",
    ".claude",
    ".codex",
    ".cursor",
    ".env",
    ".env.local",
    ".gemini",
    ".harness",
    ".hermes",
    ".opencode",
    ".planning",
    ".cursorrules",
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    "ai",
    "docs",
    "opencode.json",
    "openspec",
}


def tree_hash(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(value for value in root.rglob("*") if value.is_file() and ".git" not in value.parts):
        digest.update(path.relative_to(root).as_posix().encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def git(root: Path, *arguments: str) -> str:
    return subprocess.run(
        ["git", "-C", root, *arguments],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    ).stdout.strip()


class SddDemoFixtureTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.repository = self.base / "demo-shop"
        shutil.copytree(FIXTURE, self.repository)
        git(self.repository, "init", "-q")
        git(self.repository, "config", "user.email", "fixture@example.invalid")
        git(self.repository, "config", "user.name", "SDD Fixture")
        git(self.repository, "add", ".")
        git(self.repository, "commit", "-qm", "fixture")

    def test_unsaved_cancel_creates_no_artifact_or_durable_git_change(self) -> None:
        before = tree_hash(self.repository)
        # Closing an unsaved New Spec draft invokes no create API. This fixture
        # pins the filesystem side of that contract: setup itself is read-only.
        self.assertFalse((self.repository / ".agentum").exists())
        self.assertEqual(git(self.repository, "status", "--porcelain"), "")
        self.assertEqual(tree_hash(self.repository), before)

    def test_saved_authoring_is_confined_to_external_agentum_root(self) -> None:
        before = tree_hash(self.repository)
        authoritative = self.base / "agentum-data" / "worktrees" / "repo" / "run" / "authoritative"
        authoritative.parent.mkdir(parents=True)
        git(self.repository, "worktree", "add", "--detach", str(authoritative), "HEAD")
        spec_directory = (
            authoritative
            / ".agentum"
            / "specs"
            / "spc-01arz3ndektsv4rrffq69g5fav-refresh-access-tokens"
        )
        spec_directory.mkdir(parents=True)
        (authoritative / ".agentum" / "manifest.json").write_text(
            json.dumps(
                {
                    "format": "agentum-sdd",
                    "schemaVersion": 1,
                    "artifactSetId": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
                }
            )
            + "\n",
            encoding="utf-8",
        )
        (spec_directory / "spec.md").write_text(
            "---\nschema: 1\nid: SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV\n"
            "revision: 1\ntitle: Refresh access tokens\n---\n\n"
            "# Refresh access tokens\n\n"
            "- RQ-001 Refresh access tokens without interrupting active sessions.\n"
            "- AC-001 Existing sessions remain active throughout refresh.\n",
            encoding="utf-8",
        )

        self.assertFalse((self.repository / ".agentum").exists())
        self.assertEqual(git(self.repository, "status", "--porcelain"), "")
        self.assertEqual(tree_hash(self.repository), before)
        self.assertEqual(git(authoritative, "status", "--porcelain"), "?? .agentum/")
        observed_roots = {path.name for path in authoritative.iterdir() if path.name != ".git"}
        expected_roots = {path.name for path in FIXTURE.iterdir()} | {".agentum"}
        self.assertEqual(observed_roots, expected_roots)

    def test_fixture_has_no_ambient_agent_configuration(self) -> None:
        self.assertTrue(FIXTURE.is_dir())
        self.assertFalse(FORBIDDEN_ROOTS & {path.name for path in FIXTURE.iterdir()})


if __name__ == "__main__":
    unittest.main()
