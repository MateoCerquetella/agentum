from __future__ import annotations

import os
import importlib.util
import json
from pathlib import Path, PureWindowsPath
import shutil
import stat
import subprocess
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "migrate-agentum-sdd.py"


def load_migration_module():
    spec = importlib.util.spec_from_file_location("agentum_sdd_migration", SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def migration_command(repo: Path, *arguments: object) -> list[str]:
    return [
        "python3",
        str(SCRIPT),
        "--repo-root",
        str(repo.resolve()),
        *(str(argument) for argument in arguments),
    ]


class MigrationTests(unittest.TestCase):
    def test_windows_database_probe_matches_projectdirs_roaming_data_root(self) -> None:
        module = load_migration_module()
        with (
            mock.patch.object(module.sys, "platform", "win32"),
            mock.patch.object(module.os, "name", "nt"),
            mock.patch.object(module, "Path", PureWindowsPath),
            mock.patch.dict(
                module.os.environ,
                {"APPDATA": "C:/Users/Test/AppData/Roaming"},
                clear=True,
            ),
        ):
            self.assertEqual(
                module.default_database_path().as_posix(),
                "C:/Users/Test/AppData/Roaming/agentum/data/db.sqlite",
            )

    def make_repo(self) -> tuple[tempfile.TemporaryDirectory, Path, Path, Path]:
        temporary = tempfile.TemporaryDirectory()
        base = Path(temporary.name)
        repo = base / "repo"
        archive = base / "archive"
        spec = repo / "ai" / "specs" / "001-demo" / "spec.md"
        spec.parent.mkdir(parents=True)
        spec.write_text("# Demo\n\nThe original historical body.\n", encoding="utf-8")
        (spec.parent / "architecture.md").write_text("# Demo design\n", encoding="utf-8")
        (spec.parent / "tasks.md").write_text(
            "# Tasks\n\n- [ ] Implement the demo (AC1)\n- [ ] Verify it\n", encoding="utf-8"
        )
        (repo / "spec.md").write_text("# Root workflow\n\nRoot historical body.\n", encoding="utf-8")
        (repo / "architecture.md").write_text("# Root design\n", encoding="utf-8")
        (repo / "execution-plan.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "goal": "Root goal",
                    "acceptance_criteria": [{"id": "AC1", "outcome": "It works"}],
                    "tasks": [
                        {
                            "id": "T1",
                            "objective": "Implement root workflow",
                            "acceptance_checks": ["AC1"],
                            "writable_files": ["src/demo.rs"],
                            "allowed_create_dirs": [],
                            "read_only": [{"path": "Cargo.toml", "symbols": []}],
                            "dependencies": [],
                            "targeted_gate": {
                                "command": "cargo test root_workflow",
                                "acceptance_checks": [],
                            },
                            "integration_task": False,
                        }
                    ],
                    "final_gates": [
                        {"command": "cargo test --workspace", "acceptance_checks": ["AC1"]}
                    ],
                }
            ),
            encoding="utf-8",
        )
        legacy_demo = repo / "examples" / "harness-demo"
        (legacy_demo / ".harness").mkdir(parents=True)
        demo_sources = {
            ".harness/AGENTS.md": "# Retired demo instructions\n",
            ".harness/feature_list.json": '{"features": []}\n',
            ".harness/handoff.md": "# Retired handoff\n",
            ".harness/init.sh": "#!/bin/sh\nexit 0\n",
            ".harness/verify.sh": "#!/bin/sh\nexit 0\n",
            "README.md": "# Retired demo\n",
        }
        for relative, content in demo_sources.items():
            destination = legacy_demo / relative
            destination.write_text(content, encoding="utf-8")
            if destination.suffix == ".sh":
                destination.chmod(0o755)
        patterns = base / "restricted-patterns.txt"
        patterns.write_text("THIS_PATTERN_DOES_NOT_MATCH\n", encoding="utf-8")
        subprocess.run(["git", "init", "-q", repo], check=True)
        subprocess.run(["git", "-C", repo, "config", "user.email", "test@example.invalid"], check=True)
        subprocess.run(["git", "-C", repo, "config", "user.name", "Migration Test"], check=True)
        subprocess.run(
            [
                "git",
                "-C",
                repo,
                "add",
                "ai",
                "spec.md",
                "architecture.md",
                "execution-plan.json",
                "examples/harness-demo",
            ],
            check=True,
        )
        subprocess.run(["git", "-C", repo, "commit", "-qm", "legacy"], check=True)
        return temporary, repo, archive, patterns

    def test_apply_archives_imports_and_second_run_is_noop(self) -> None:
        temporary, repo, archive, patterns = self.make_repo()
        self.addCleanup(temporary.cleanup)
        original = (repo / "ai/specs/001-demo/spec.md").read_bytes()
        result = subprocess.run(
            migration_command(
                repo,
                "--apply",
                "--archive-dir",
                archive,
                "--restricted-patterns",
                patterns,
            ),
            cwd=repo,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
        )
        self.assertIn("migrated 2 specifications", result.stdout)
        self.assertFalse((repo / "ai").exists())
        self.assertFalse((repo / "spec.md").exists())
        self.assertFalse((repo / "architecture.md").exists())
        self.assertFalse((repo / "execution-plan.json").exists())
        self.assertFalse((repo / "examples/harness-demo").exists())
        imported_specs = list((repo / ".agentum/specs").glob("*/spec.md"))
        self.assertEqual(len(imported_specs), 2)
        imported = next(path.read_text() for path in imported_specs if "Demo" in path.read_text())
        self.assertIn("The original historical body.", imported)
        demo_directory = next(path.parent for path in imported_specs if "Demo" in path.read_text())
        self.assertEqual((demo_directory / "design.md").read_text(), "# Demo design\n")
        demo_plan = json.loads((demo_directory / "plan.json").read_text())
        self.assertEqual([task["id"] for task in demo_plan["tasks"]], ["T-001", "T-002"])
        root_directory = next(path.parent for path in imported_specs if "Root workflow" in path.read_text())
        root_plan = json.loads((root_directory / "plan.json").read_text())
        self.assertEqual(root_plan["tasks"][0]["verification"][0]["program"], "cargo")
        self.assertEqual(root_plan["tasks"][-1]["id"], "MIG-FINAL-001")
        archived = next(archive.glob("*/ai/specs/001-demo/spec.md"))
        self.assertEqual(archived.read_bytes(), original)
        if os.name != "nt":
            archive_root = archived.parents[3]
            self.assertEqual(stat.S_IMODE(archive_root.stat().st_mode), 0o700)
            self.assertEqual(stat.S_IMODE(archived.parent.stat().st_mode), 0o700)
            self.assertEqual(stat.S_IMODE(archived.stat().st_mode), 0o600)
        inventory = json.loads((repo / "docs/migrations/agentum-sdd-inventory.json").read_text())
        self.assertEqual(inventory["sourceCount"], 12)
        self.assertEqual(
            {entry["path"] for entry in inventory["sources"]},
            {
                "ai/specs/001-demo/spec.md",
                "ai/specs/001-demo/architecture.md",
                "ai/specs/001-demo/tasks.md",
                "spec.md",
                "architecture.md",
                "execution-plan.json",
                "examples/harness-demo/.harness/AGENTS.md",
                "examples/harness-demo/.harness/feature_list.json",
                "examples/harness-demo/.harness/handoff.md",
                "examples/harness-demo/.harness/init.sh",
                "examples/harness-demo/.harness/verify.sh",
                "examples/harness-demo/README.md",
            },
        )
        demo_entries = [
            entry
            for entry in inventory["sources"]
            if entry["path"].startswith("examples/harness-demo/")
        ]
        self.assertEqual(len(demo_entries), 6)
        self.assertEqual(
            {entry["disposition"] for entry in demo_entries},
            {"historical-only", "intentionally ignored generated data"},
        )
        archived_demo = next(archive.glob("*/examples/harness-demo/README.md"))
        self.assertEqual(archived_demo.read_text(), "# Retired demo\n")
        self.assertEqual(len(inventory["artifactAssociations"]), 2)
        self.assertEqual(
            sum(len(value["artifacts"]) for value in inventory["artifactAssociations"]), 6
        )
        second = subprocess.run(
            migration_command(
                repo,
                "--apply",
                "--archive-dir",
                archive,
                "--restricted-patterns",
                patterns,
            ),
            cwd=repo,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
        )
        self.assertIn("already complete; no-op", second.stdout)

    def test_apply_refuses_untracked_content_without_deleting_it(self) -> None:
        temporary, repo, archive, patterns = self.make_repo()
        self.addCleanup(temporary.cleanup)
        user_file = repo / "ai" / "user-notes.md"
        user_file.write_text("keep me", encoding="utf-8")
        result = subprocess.run(
            migration_command(
                repo,
                "--apply",
                "--archive-dir",
                archive,
                "--restricted-patterns",
                patterns,
            ),
            cwd=repo,
            text=True,
            stderr=subprocess.PIPE,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(user_file.is_file())
        self.assertFalse((repo / ".agentum").exists())

    def test_apply_hash_accounts_demo_already_deleted_for_exact_replacement(self) -> None:
        temporary, repo, archive, patterns = self.make_repo()
        self.addCleanup(temporary.cleanup)
        shutil.rmtree(repo / "examples/harness-demo")
        replacement_files = {
            "README.md": "# Neutral SDD demo\n",
            "package.json": '{"scripts":{"test":"node --test"}}\n',
            "src/session-store.js": "export const sessions = new Map();\n",
            "test/session-store.test.js": "// neutral fixture test\n",
        }
        for relative, content in replacement_files.items():
            destination = repo / "examples/sdd-demo" / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(content, encoding="utf-8")

        subprocess.run(
            migration_command(
                repo,
                "--apply",
                "--archive-dir",
                archive,
                "--restricted-patterns",
                patterns,
            ),
            cwd=repo,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
        )
        inventory = json.loads((repo / "docs/migrations/agentum-sdd-inventory.json").read_text())
        demo_entries = [
            entry
            for entry in inventory["sources"]
            if entry["path"].startswith("examples/harness-demo/")
        ]
        self.assertEqual(len(demo_entries), 6)
        self.assertTrue(all(len(entry["sha256"]) == 64 for entry in demo_entries))
        self.assertEqual(
            next(archive.glob("*/examples/harness-demo/README.md")).read_text(),
            "# Retired demo\n",
        )

    def test_external_pattern_quarantines_an_entire_spec_family(self) -> None:
        temporary, repo, _archive, patterns = self.make_repo()
        self.addCleanup(temporary.cleanup)
        (repo / "ai/specs/001-demo/architecture.md").write_text(
            "# Demo design\n\nRESTRICTED-MARKER\n", encoding="utf-8"
        )
        subprocess.run(["git", "-C", repo, "add", "ai"], check=True)
        subprocess.run(["git", "-C", repo, "commit", "-qm", "restricted legacy"], check=True)
        patterns.write_text("RESTRICTED-MARKER\n", encoding="utf-8")
        result = subprocess.run(
            migration_command(repo, "--preview", "--restricted-patterns", patterns),
            cwd=repo,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
        )
        inventory = json.loads(result.stdout)
        family = [
            entry
            for entry in inventory["sources"]
            if entry["path"].startswith("ai/specs/001-demo/")
        ]
        self.assertEqual(len(family), 3)
        self.assertTrue(
            all(entry["disposition"] == "externally archived recovery material" for entry in family)
        )
        self.assertFalse(
            any(value["specSource"] == "ai/specs/001-demo/spec.md" for value in inventory["artifactAssociations"])
        )

    def test_apply_resumes_every_published_boundary_without_overwrite(self) -> None:
        for crash_point in ("post_archive", "post_publish", "post_report", "mid_delete"):
            with self.subTest(crash_point=crash_point):
                temporary, repo, archive, patterns = self.make_repo()
                self.addCleanup(temporary.cleanup)
                crashed = subprocess.run(
                    migration_command(
                        repo,
                        "--apply",
                        "--archive-dir",
                        archive,
                        "--restricted-patterns",
                        patterns,
                        "--test-crash-at",
                        crash_point,
                    ),
                    cwd=repo,
                    text=True,
                    stderr=subprocess.PIPE,
                    env={**os.environ, "AGENTUM_MIGRATION_ENABLE_TEST_CRASH": "1"},
                )
                self.assertNotEqual(crashed.returncode, 0)
                self.assertTrue((repo / ".agentum-migration-journal.json").is_file())
                published_identity = (
                    (repo / ".agentum").stat().st_dev,
                    (repo / ".agentum").stat().st_ino,
                ) if (repo / ".agentum").is_dir() else None
                if crash_point == "post_archive":
                    self.assertFalse((repo / ".agentum").exists())
                    self.assertFalse((repo / "docs/migrations/agentum-sdd-inventory.json").exists())
                elif crash_point == "post_publish":
                    self.assertTrue((repo / ".agentum").is_dir())
                    self.assertFalse((repo / "docs/migrations/agentum-sdd-inventory.json").exists())
                else:
                    self.assertTrue((repo / ".agentum").is_dir())
                    self.assertTrue((repo / "docs/migrations/agentum-sdd-inventory.json").is_file())

                subprocess.run(
                    migration_command(
                        repo,
                        "--apply",
                        "--archive-dir",
                        archive,
                        "--restricted-patterns",
                        patterns,
                    ),
                    cwd=repo,
                    check=True,
                    text=True,
                    stdout=subprocess.PIPE,
                )
                self.assertFalse((repo / ".agentum-migration-journal.json").exists())
                self.assertFalse((repo / "ai").exists())
                self.assertFalse((repo / "examples/harness-demo").exists())
                inventory = json.loads(
                    (repo / "docs/migrations/agentum-sdd-inventory.json").read_text()
                )
                self.assertEqual(inventory["sourceCount"], 12)
                archived_demo_files = [
                    path
                    for path in archive.glob("*/examples/harness-demo/**/*")
                    if path.is_file()
                ]
                self.assertEqual(len(archived_demo_files), 6)
                if published_identity is not None:
                    self.assertEqual(
                        ((repo / ".agentum").stat().st_dev, (repo / ".agentum").stat().st_ino),
                        published_identity,
                    )

    def test_resume_refuses_a_source_changed_after_checkpoint(self) -> None:
        temporary, repo, archive, patterns = self.make_repo()
        self.addCleanup(temporary.cleanup)
        subprocess.run(
            migration_command(
                repo,
                "--apply",
                "--archive-dir",
                archive,
                "--restricted-patterns",
                patterns,
                "--test-crash-at",
                "post_publish",
            ),
            cwd=repo,
            text=True,
            stderr=subprocess.PIPE,
            env={**os.environ, "AGENTUM_MIGRATION_ENABLE_TEST_CRASH": "1"},
        )
        changed = repo / "ai/specs/001-demo/spec.md"
        changed.write_text("# User changed this after the crash\n", encoding="utf-8")
        resumed = subprocess.run(
            migration_command(
                repo,
                "--apply",
                "--archive-dir",
                archive,
                "--restricted-patterns",
                patterns,
            ),
            cwd=repo,
            text=True,
            stderr=subprocess.PIPE,
        )
        self.assertNotEqual(resumed.returncode, 0)
        self.assertEqual(changed.read_text(), "# User changed this after the crash\n")
        self.assertTrue((repo / ".agentum-migration-journal.json").is_file())

    def test_caller_cwd_is_never_inferred_as_the_migration_target(self) -> None:
        temporary, target_repo, archive, patterns = self.make_repo()
        self.addCleanup(temporary.cleanup)
        caller_repo = Path(temporary.name) / "caller"
        caller_repo.mkdir()
        subprocess.run(["git", "init", "-q", caller_repo], check=True)
        target_before = (target_repo / "spec.md").read_bytes()

        missing_root = subprocess.run(
            [
                "python3",
                SCRIPT,
                "--apply",
                "--archive-dir",
                archive,
                "--restricted-patterns",
                patterns,
            ],
            cwd=caller_repo,
            text=True,
            stderr=subprocess.PIPE,
        )
        self.assertNotEqual(missing_root.returncode, 0)
        self.assertIn("--repo-root", missing_root.stderr)
        self.assertFalse((caller_repo / ".agentum").exists())
        self.assertFalse((target_repo / ".agentum").exists())
        self.assertEqual((target_repo / "spec.md").read_bytes(), target_before)

        preview = subprocess.run(
            migration_command(target_repo, "--preview", "--restricted-patterns", patterns),
            cwd=caller_repo,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
        )
        self.assertEqual(json.loads(preview.stdout)["sourceCount"], 12)
        self.assertFalse((caller_repo / ".agentum").exists())
        self.assertFalse((target_repo / ".agentum").exists())
        self.assertEqual((target_repo / "spec.md").read_bytes(), target_before)

    @unittest.skipIf(os.name == "nt", "symlink creation requires elevated privileges on Windows")
    def test_preview_rejects_tracked_symlink_source(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        repo = Path(temporary.name) / "repo"
        repo.mkdir()
        outside = Path(temporary.name) / "outside.md"
        outside.write_text("secret", encoding="utf-8")
        patterns = Path(temporary.name) / "restricted-patterns.txt"
        patterns.write_text("DOES-NOT-MATCH\n", encoding="utf-8")
        (repo / "ai").mkdir()
        (repo / "ai/spec.md").symlink_to(outside)
        subprocess.run(["git", "init", "-q", repo], check=True)
        subprocess.run(["git", "-C", repo, "add", "ai/spec.md"], check=True)
        result = subprocess.run(
            migration_command(repo, "--preview", "--restricted-patterns", patterns),
            cwd=repo,
            text=True,
            stderr=subprocess.PIPE,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("symlink", result.stderr.lower())


if __name__ == "__main__":
    unittest.main()
