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

    def test_utf16le_windows_pe_match_is_detected(self) -> None:
        leaked = self.staging / "windows-resource.exe"
        leaked.write_bytes(b"MZ" + "PRIVATE-MARKER-42".encode("utf-16-le"))
        result = self.run_scan()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(str(leaked), result.stderr)
        self.assertNotIn("PRIVATE-MARKER-42", result.stderr)

    def test_non_pe_binary_is_not_reinterpreted_as_windows_utf16(self) -> None:
        archive = self.staging / "linux-package.tar.gz"
        archive.write_bytes("PRIVATE-MARKER-42".encode("utf-16-le"))
        result = self.run_scan()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_regex_does_not_match_across_non_printable_binary_bytes(self) -> None:
        self.patterns.write_text("PRIVATE.MARKER\n", encoding="utf-8")
        (self.staging / "compressed-installer.bin").write_bytes(
            b"header\x00PRIVATE\x01MARKER\x00payload"
        )
        result = self.run_scan()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_dmg_container_bytes_are_not_scanned_as_text(self) -> None:
        (self.staging / "Agentum.dmg").write_bytes(
            b"compressed-random-bytes\x00PRIVATE-MARKER-42\xff"
        )
        unpacked = self.staging / "Agentum.app" / "Contents" / "MacOS"
        unpacked.mkdir(parents=True)
        (unpacked / "agentum-desktop").write_bytes(b"\x00clean-app-payload\xff")
        self.assertEqual(self.run_scan().returncode, 0)

    def test_unpacked_macos_bundle_content_is_still_scanned(self) -> None:
        (self.staging / "Agentum.dmg").write_bytes(b"opaque-container")
        unpacked = self.staging / "Agentum.app" / "Contents" / "Resources"
        unpacked.mkdir(parents=True)
        leaked = unpacked / "configuration.txt"
        leaked.write_text("PRIVATE-MARKER-42\n", encoding="utf-8")
        result = self.run_scan()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(str(leaked), result.stderr)

    def test_explicit_bundle_under_target_directory_is_scanned(self) -> None:
        bundle = self.root / "target" / "release" / "bundle"
        bundle.mkdir(parents=True)
        leaked = bundle / "installer.exe"
        leaked.write_bytes(b"prefix\x00PRIVATE-MARKER-42\x00suffix")
        result = subprocess.run(
            ["bash", SCRIPT, self.patterns, bundle],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(str(leaked), result.stderr)

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
