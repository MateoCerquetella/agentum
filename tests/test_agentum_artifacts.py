from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "check-agentum-artifacts.py"
SPEC = "SPC-01ARZ3NDEKTSV4RRFFQ69G5FAV"
DIRECTORY = "spc-01arz3ndektsv4rrffq69g5fav-demo"

module_spec = importlib.util.spec_from_file_location("agentum_artifact_check", SCRIPT)
assert module_spec and module_spec.loader
artifact_check = importlib.util.module_from_spec(module_spec)
module_spec.loader.exec_module(artifact_check)


class ArtifactBoundaryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name) / ".agentum"
        spec_dir = self.root / "specs" / DIRECTORY
        spec_dir.mkdir(parents=True)
        (self.root / "manifest.json").write_text(
            json.dumps(
                {
                    "format": "agentum-sdd",
                    "schemaVersion": 1,
                    "artifactSetId": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
                }
            ),
            encoding="utf-8",
        )
        (spec_dir / "spec.md").write_text(
            f"---\nschema: 1\nid: {SPEC}\nrevision: 1\ntitle: Demo\n---\n\n"
            "# Demo\n\n- RQ-001 Do it.\n- AC-001 It is done.\n",
            encoding="utf-8",
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_valid_root(self) -> None:
        artifact_check.validate(self.root)

    def test_missing_specs_is_rejected(self) -> None:
        for child in (self.root / "specs" / DIRECTORY).iterdir():
            child.unlink()
        (self.root / "specs" / DIRECTORY).rmdir()
        (self.root / "specs").rmdir()
        with self.assertRaisesRegex(ValueError, "only manifest.json and specs"):
            artifact_check.validate(self.root)

    def test_empty_specs_root_is_rejected(self) -> None:
        for child in (self.root / "specs" / DIRECTORY).iterdir():
            child.unlink()
        (self.root / "specs" / DIRECTORY).rmdir()
        with self.assertRaisesRegex(ValueError, "at least one"):
            artifact_check.validate(self.root)

    def test_nested_unexpected_file_is_rejected(self) -> None:
        nested = self.root / "specs" / DIRECTORY / "evidence"
        nested.mkdir()
        (nested / "secret.txt").write_text("x", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "unexpected artifact"):
            artifact_check.validate(self.root)

    def test_mismatched_frontmatter_id_is_rejected(self) -> None:
        path = self.root / "specs" / DIRECTORY / "spec.md"
        path.write_text(path.read_text().replace(SPEC, "SPC-01BX5ZZKBKACTAV9WEVGEMMVRZ"))
        with self.assertRaisesRegex(ValueError, "does not match"):
            artifact_check.validate(self.root)

    @unittest.skipIf(os.name == "nt", "symlink creation requires elevated privileges on Windows")
    def test_symlinked_spec_is_rejected(self) -> None:
        path = self.root / "specs" / DIRECTORY / "spec.md"
        target = Path(self.temporary.name) / "outside.md"
        target.write_text(path.read_text(), encoding="utf-8")
        path.unlink()
        path.symlink_to(target)
        with self.assertRaisesRegex(ValueError, "regular file"):
            artifact_check.validate(self.root)


if __name__ == "__main__":
    unittest.main()
