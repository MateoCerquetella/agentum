from pathlib import Path
import os
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "check-restricted-content.sh"


class RestrictedContentScanTests(unittest.TestCase):
    def test_git_control_file_is_not_release_content(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            patterns = root / "patterns"
            target = root / "source"
            target.mkdir()
            patterns.write_text("private-worktree-path\n", encoding="utf-8")
            (target / ".git").write_text(
                "gitdir: /private-worktree-path/control\n", encoding="utf-8"
            )
            (target / "README.md").write_text("public source\n", encoding="utf-8")

            result = subprocess.run(
                [str(SCRIPT), str(patterns.resolve()), str(target)],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.patterns = self.root / "patterns"
        self.patterns.write_text("PRIVATE-MARKER-[0-9]+\n", encoding="utf-8")
        self.staging = self.root / "staging"
        self.staging.mkdir()

    def run_scan(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", SCRIPT, self.patterns, self.staging],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

    def test_clean_binary_and_text_staging_pass(self) -> None:
        (self.staging / "bundle.bin").write_bytes(b"\x00clean\xffpayload")
        (self.staging / "latest.json").write_text('{"version":"1.0.0"}\n')
        self.assertEqual(self.run_scan().returncode, 0)

    def test_match_reports_only_file_name_not_matching_content(self) -> None:
        leaked = self.staging / "bundle.bin"
        leaked.write_bytes(b"prefix\x00PRIVATE-MARKER-42\x00suffix")
        result = self.run_scan()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(str(leaked), result.stderr)
        self.assertNotIn("PRIVATE-MARKER-42", result.stderr)

    @unittest.skipIf(os.name == "nt", "symlink creation requires elevated privileges on Windows")
    def test_symlink_scan_target_is_rejected(self) -> None:
        outside = self.root / "outside"
        outside.mkdir()
        link = self.root / "linked-staging"
        link.symlink_to(outside, target_is_directory=True)
        result = subprocess.run(
            ["bash", SCRIPT, self.patterns, link],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.assertEqual(result.returncode, 2)


if __name__ == "__main__":
    unittest.main()
