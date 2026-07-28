import hashlib
import json
import re
import unittest
from pathlib import Path, PurePosixPath


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
OFFICIAL_ROOT = (
    REPOSITORY_ROOT
    / "crates"
    / "agentum-server"
    / "tests"
    / "fixtures"
    / "openspec"
    / "official"
)


class OpenSpecGoldenFixtureTests(unittest.TestCase):
    def test_pinned_public_fixture_matches_recorded_provenance(self) -> None:
        provenance_path = OFFICIAL_ROOT / "provenance.json"
        provenance = json.loads(provenance_path.read_text(encoding="utf-8"))

        self.assertEqual(provenance["schemaVersion"], 1)
        self.assertEqual(
            provenance["upstreamRepository"],
            "https://github.com/Fission-AI/OpenSpec",
        )
        commit = provenance["upstreamCommit"]
        self.assertRegex(commit, r"^[0-9a-f]{40}$")
        self.assertEqual(
            provenance["upstreamCommitUrl"],
            f"https://github.com/Fission-AI/OpenSpec/commit/{commit}",
        )
        self.assertEqual(provenance["license"], "MIT")
        self.assertEqual(provenance["upstreamLicensePath"], "LICENSE")
        self.assertEqual(
            provenance["upstreamLicenseSha256"],
            "c3c7235bea1214ab62df643473975c2e8b8848f528901a976693f7d069713e64",
        )
        self.assertEqual(
            provenance["upstreamFixturePath"],
            "openspec/changes/archive/2025-10-14-update-cli-init-enter-selection",
        )

        license_path = OFFICIAL_ROOT / provenance["licenseFile"]
        self.assertFalse(license_path.is_symlink())
        license_bytes = license_path.read_bytes()
        self.assertEqual(
            hashlib.sha256(license_bytes).hexdigest(),
            provenance["licenseSha256"],
        )
        self.assertIn(b"MIT License", license_bytes)
        self.assertIn(b"Copyright (c) 2024 OpenSpec Contributors", license_bytes)

        fixture_root = OFFICIAL_ROOT / provenance["fixtureRoot"]
        self.assertTrue(fixture_root.is_dir())
        expected_paths = []
        for entry in provenance["files"]:
            relative = PurePosixPath(entry["path"])
            self.assertFalse(relative.is_absolute())
            self.assertNotIn("..", relative.parts)
            self.assertLessEqual(len(relative.as_posix()), 240)
            self.assertTrue(all(len(part) <= 128 for part in relative.parts))
            path = fixture_root.joinpath(*relative.parts)
            self.assertFalse(path.is_symlink())
            self.assertTrue(path.is_file())
            self.assertEqual(
                hashlib.sha256(path.read_bytes()).hexdigest(), entry["sha256"]
            )
            expected_paths.append(relative.as_posix())

        actual_paths = sorted(
            path.relative_to(fixture_root).as_posix()
            for path in fixture_root.rglob("*")
            if path.is_file()
        )
        self.assertEqual(actual_paths, sorted(expected_paths))

    def test_fixture_is_the_documented_conventional_change_shape(self) -> None:
        provenance = json.loads(
            (OFFICIAL_ROOT / "provenance.json").read_text(encoding="utf-8")
        )
        fixture_root = OFFICIAL_ROOT / provenance["fixtureRoot"]
        proposal = (fixture_root / "proposal.md").read_text(encoding="utf-8")
        tasks = (fixture_root / "tasks.md").read_text(encoding="utf-8")
        delta = (fixture_root / "specs" / "cli-init" / "spec.md").read_text(
            encoding="utf-8"
        )

        self.assertIn("## Why", proposal)
        self.assertIn("## What Changes", proposal)
        self.assertRegex(tasks, re.compile(r"^- \[[ xX]\] ", re.MULTILINE))
        self.assertIn("## MODIFIED Requirements", delta)
        self.assertIn("### Requirement:", delta)
        self.assertIn("#### Scenario:", delta)

    def test_adapter_has_no_openspec_cli_runtime_dependency(self) -> None:
        manifests = [
            REPOSITORY_ROOT / "Cargo.toml",
            REPOSITORY_ROOT / "Cargo.lock",
            REPOSITORY_ROOT / "crates" / "agentum-server" / "Cargo.toml",
        ]
        dependency_text = "\n".join(
            path.read_text(encoding="utf-8") for path in manifests
        ).lower()
        self.assertNotIn('@fission-ai/openspec', dependency_text)
        self.assertNotRegex(dependency_text, r'(?m)^name\s*=\s*"openspec"\s*$')

        adapter = (
            REPOSITORY_ROOT
            / "crates"
            / "agentum-server"
            / "src"
            / "sdd"
            / "sources.rs"
        ).read_text(encoding="utf-8")
        self.assertNotRegex(adapter, r'Command::new\s*\(\s*"openspec"')
        self.assertNotRegex(adapter, r'program:\s*"openspec"')


if __name__ == "__main__":
    unittest.main()
