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
    / "empirical"
    / "official"
)
PINNED_COMMIT = "d8ee7e1bdaa53bfc92e278524a40e61d16125f64"


class EmpiricalGoldenFixtureTests(unittest.TestCase):
    def test_pinned_protocol_fixture_matches_recorded_provenance(self) -> None:
        provenance = json.loads(
            (OFFICIAL_ROOT / "provenance.json").read_text(encoding="utf-8")
        )

        self.assertEqual(
            provenance["repository"],
            "https://github.com/MateoCerquetella/empirical-sdd",
        )
        self.assertEqual(provenance["commit"], PINNED_COMMIT)
        self.assertEqual(provenance["protocol"], "0.20")
        self.assertEqual(provenance["schemaVersion"], 4)
        self.assertEqual(provenance["license"], "MIT")
        self.assertEqual(
            provenance["licenseUrl"],
            f"https://github.com/MateoCerquetella/empirical-sdd/blob/{PINNED_COMMIT}/LICENSE",
        )

        expected_paths = []
        for relative_name, digest in provenance["files"].items():
            relative = PurePosixPath(relative_name)
            self.assertFalse(relative.is_absolute())
            self.assertNotIn("..", relative.parts)
            path = OFFICIAL_ROOT.joinpath(*relative.parts)
            self.assertFalse(path.is_symlink())
            self.assertTrue(path.is_file())
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), digest)
            expected_paths.append(relative.as_posix())

        actual_paths = sorted(
            path.relative_to(OFFICIAL_ROOT).as_posix()
            for path in OFFICIAL_ROOT.rglob("*")
            if path.is_file() and path.name != "provenance.json"
        )
        self.assertEqual(actual_paths, sorted(expected_paths))

    def test_adapter_is_documented_as_local_artifact_intake(self) -> None:
        documentation = (REPOSITORY_ROOT / "docs" / "AGENTUM_SDD.md").read_text(
            encoding="utf-8"
        )
        self.assertIn(".empirical/specs/<feature>", documentation)
        self.assertIn("Empirical artifact-intake compatibility", documentation)
        self.assertIn(PINNED_COMMIT, documentation)
        self.assertIn("never executes Empirical", documentation)
        self.assertIn("remote intake", documentation)
        self.assertIn("Creation re-reads and re-normalizes the source", documentation)
        self.assertIn("does not claim compatibility with Empirical's runtime", documentation)

        license_bytes = (OFFICIAL_ROOT / "LICENSE").read_bytes()
        self.assertIn(b"MIT License", license_bytes)
        self.assertIn(b"Copyright (c) 2026 Mateo Cerquetella", license_bytes)

        adapter = (
            REPOSITORY_ROOT
            / "crates"
            / "agentum-server"
            / "src"
            / "sdd"
            / "sources.rs"
        ).read_text(encoding="utf-8")
        self.assertIn("pub fn import_empirical", adapter)
        self.assertNotRegex(adapter, re.compile(r'Command::new\s*\(\s*"empirical"'))
        self.assertNotRegex(adapter, re.compile(r'program:\s*"empirical"'))

        manifests = [
            REPOSITORY_ROOT / "Cargo.toml",
            REPOSITORY_ROOT / "Cargo.lock",
            REPOSITORY_ROOT / "crates" / "agentum-server" / "Cargo.toml",
            REPOSITORY_ROOT / "crates" / "agentum-desktop" / "ui" / "package.json",
            REPOSITORY_ROOT / "crates" / "agentum-desktop" / "ui" / "bun.lock",
        ]
        dependency_text = "\n".join(
            path.read_text(encoding="utf-8") for path in manifests
        ).lower()
        self.assertNotIn("empirical-sdd", dependency_text)

    def test_desktop_exposes_the_canonical_empirical_source(self) -> None:
        model = (
            REPOSITORY_ROOT
            / "crates"
            / "agentum-desktop"
            / "ui"
            / "src"
            / "components"
            / "sdd"
            / "run-center-model.ts"
        ).read_text(encoding="utf-8")
        self.assertIn("id: 'empirical'", model)
        self.assertIn("label: 'Empirical'", model)
        self.assertIn(".empirical/specs/example", model)


if __name__ == "__main__":
    unittest.main()
