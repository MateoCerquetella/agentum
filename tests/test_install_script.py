from pathlib import Path
import subprocess
import unittest


ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "scripts/install.sh"


class InstallScriptContractTests(unittest.TestCase):
    def run_sourced(self, script: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["sh", "-c", script, "installer-contract", str(INSTALLER)],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_darwin_fails_before_network_or_filesystem_setup(self) -> None:
        result = self.run_sourced(
            r'''
            uname() {
              case "$1" in
                -s) printf '%s\n' Darwin ;;
                -m) printf '%s\n' arm64 ;;
              esac
            }
            curl() { printf '%s\n' NETWORK_CALLED >&2; exit 99; }
            wget() { printf '%s\n' NETWORK_CALLED >&2; exit 99; }
            export AGENTUM_INSTALL_SOURCE_ONLY=1
            installer="$1"
            set --
            . "$installer"
            main
            ''',
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unsupported platform: Darwin arm64", result.stderr)
        self.assertNotIn("NETWORK_CALLED", result.stderr)

    def test_linux_x86_64_remains_the_supported_shell_install_target(self) -> None:
        result = self.run_sourced(
            r'''
            uname() {
              case "$1" in
                -s) printf '%s\n' Linux ;;
                -m) printf '%s\n' x86_64 ;;
              esac
            }
            export AGENTUM_INSTALL_SOURCE_ONLY=1
            installer="$1"
            set --
            . "$installer"
            detect_platform
            printf '%s\n' "$PLATFORM"
            ''',
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "linux-x64")

    def test_installer_contains_no_retired_macos_delivery_code(self) -> None:
        installer = INSTALLER.read_text(encoding="utf-8")

        for retired in (
            "macos-x64",
            "macos-arm64",
            "install_macos",
            "verify_macos_bundle",
            "hdiutil",
            "Agentum.app",
            "MOUNT_POINT",
        ):
            self.assertNotIn(retired, installer)
        self.assertNotRegex(installer, r"(?m)^APPLICATIONS_DIR=")


if __name__ == "__main__":
    unittest.main()
