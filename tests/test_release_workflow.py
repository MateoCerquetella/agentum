import json
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]
RELEASE = ROOT / ".github/workflows/release.yml"
WORKSPACE = ROOT / "Cargo.toml"
LOCKFILE = ROOT / "Cargo.lock"
TAURI_CONFIG = ROOT / "crates/agentum-desktop/tauri.conf.json"
MACOS_ENTITLEMENTS = ROOT / "crates/agentum-desktop/Entitlements.plist"
CHANGELOG = ROOT / "CHANGELOG.md"
SDD_BOUNDARY = ROOT / "scripts/check-sdd-boundary.sh"


def workspace_version() -> str:
    match = re.search(
        r'^\[workspace\.package\]\s+version = "([^"]+)"',
        WORKSPACE.read_text(encoding="utf-8"),
        re.MULTILINE,
    )
    if match is None:
        raise AssertionError("workspace package version is missing")
    return match.group(1)


class ReleaseWorkflowContractTests(unittest.TestCase):
    def test_release_workflow_pins_the_current_node24_checkout_action(self) -> None:
        workflows = [RELEASE]
        pin = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1"
        combined = "\n".join(path.read_text(encoding="utf-8") for path in workflows)
        self.assertEqual(combined.count(pin), 4)
        self.assertNotIn("actions/checkout@11d5960a326750d5838078e36cf38b85af677262", combined)

    def test_sdd_boundary_normalizes_windows_scan_paths_before_allowlisting(self) -> None:
        boundary = SDD_BOUNDARY.read_text(encoding="utf-8")
        scan = boundary.index('direct_workspace_callers="$({')
        allowlist = boundary.index('while IFS= read -r caller; do', scan)
        self.assertIn(r"| tr '\134' '/' | sort", boundary[scan:allowlist])

    def test_patch_release_version_is_consistent_everywhere(self) -> None:
        version = workspace_version()
        self.assertRegex(version, r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
        self.assertEqual(version, "0.98.6")

        config = json.loads(TAURI_CONFIG.read_text(encoding="utf-8"))
        self.assertEqual(config["version"], version)

        changelog = CHANGELOG.read_text(encoding="utf-8")
        headings = re.findall(r"^## \[([^]]+)]", changelog, re.MULTILINE)
        self.assertGreaterEqual(len(headings), 2)
        self.assertEqual(headings[0], version)
        previous = tuple(int(part) for part in headings[1].split("."))
        current = tuple(int(part) for part in version.split("."))
        self.assertEqual(current, (previous[0], previous[1], previous[2] + 1))

        release = RELEASE.read_text(encoding="utf-8")
        self.assertIn("Create the version-matched release", release)
        self.assertNotIn(f"Create the v{version} release", release)

        owned_packages = {
            "agentum-core",
            "agentum-desktop",
            "agentum-executor",
            "agentum-jira-broker",
            "agentum-server",
            "agentum-store",
            "agentum-tmux",
            "agentum-watchdog",
        }
        locked_versions: dict[str, str] = {}
        for block in LOCKFILE.read_text(encoding="utf-8").split("[[package]]"):
            name = re.search(r'^name = "([^"]+)"', block, re.MULTILINE)
            locked = re.search(r'^version = "([^"]+)"', block, re.MULTILINE)
            if name and locked and name.group(1) in owned_packages:
                locked_versions[name.group(1)] = locked.group(1)
        self.assertEqual(set(locked_versions), owned_packages)
        self.assertEqual(set(locked_versions.values()), {version})

    def test_publication_requires_available_secrets_without_os_certificates(self) -> None:
        release = RELEASE.read_text(encoding="utf-8")
        required = {
            "TAURI_SIGNING_PRIVATE_KEY",
            "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
            "HOMEBREW_TAP_DEPLOY_KEY",
            "AGENTUM_RESTRICTED_PATTERNS",
        }
        for secret in required:
            self.assertIn(f"      {secret}:\n        required: true", release)

        removed = {
            "APPLE_CERTIFICATE",
            "APPLE_CERTIFICATE_PASSWORD",
            "APPLE_SIGNING_IDENTITY",
            "APPLE_ID",
            "APPLE_PASSWORD",
            "APPLE_TEAM_ID",
            "WINDOWS_CERTIFICATE",
            "WINDOWS_CERTIFICATE_PASSWORD",
            "WINDOWS_CERTIFICATE_THUMBPRINT",
        }
        for secret in removed:
            self.assertNotIn(secret, release)

        self.assertIn('test "$GITHUB_REF" = "refs/tags/v${version}"', release)
        self.assertIn('test "$(git rev-parse HEAD)" = "$(git rev-parse refs/remotes/origin/main)"', release)
        self.assertIn('test "$(git cat-file -t "refs/tags/$TAG")" = "tag"', release)
        self.assertIn("'.verification.verified'", release)

    def test_updater_signing_remains_required_without_platform_signing_steps(self) -> None:
        workflow = RELEASE.read_text(encoding="utf-8")
        self.assertIn("tool: tauri-cli@2.11.2", workflow)
        self.assertIn('test -n "$TAURI_SIGNING_PRIVATE_KEY"', workflow)
        self.assertIn('test -n "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD"', workflow)
        self.assertIn("Verify every updater signature against the embedded key", workflow)
        self.assertIn("agentum-updater-verify", workflow)
        for removed in (
            "Import Developer ID certificate",
            "Remove isolated signing keychain",
            "Import Authenticode certificate",
            "Verify Authenticode signatures",
            "Remove Authenticode certificate material",
            "certificateThumbprint",
            "codesign",
            "spctl",
            "stapler",
        ):
            self.assertNotIn(removed, workflow)

    def test_release_rust_dependency_resolution_is_lockfile_bound(self) -> None:
        workflow = RELEASE.read_text(encoding="utf-8")
        self.assertIn(
            'cargo build --locked --release --target "$BUILD_TARGET" -p sherpa-rs',
            workflow,
        )
        self.assertEqual(workflow.count("cargo tauri build --target"), 1)
        self.assertEqual(workflow.count("--verbose -- --locked"), 1)
        self.assertEqual(workflow.count("-- --locked"), 1)
        self.assertNotIn("cargo tauri build --locked", workflow)

    def test_every_release_stage_receives_required_restricted_policy(self) -> None:
        release = RELEASE.read_text(encoding="utf-8")
        self.assertIn("AGENTUM_RESTRICTED_PATTERNS:\n        required: true", release)
        self.assertIn("Validate restricted-content release policy availability", release)
        bundle_scan = release.index("Scan generated bundles for restricted content")
        upload = release.index("actions/upload-artifact")
        self.assertLess(bundle_scan, upload)
        final_scan = release.index("Scan source package and final release staging")
        draft = release.index("Create private GitHub release draft")
        self.assertLess(final_scan, draft)
        self.assertIn("agentum-${ver}-source.tar.gz", release)
        self.assertIn("gitleaks dir --no-banner --redact --exit-code 1 dist", release)
        self.assertNotIn("StrictHostKeyChecking=accept-new", release)
        self.assertNotIn("~/.ssh", release)
        self.assertIn("StrictHostKeyChecking=yes", release)

    def test_linux_packages_require_sandbox_and_ship_checked_worker(self) -> None:
        config = json.loads(TAURI_CONFIG.read_text(encoding="utf-8"))
        self.assertEqual(config["bundle"]["linux"]["deb"]["depends"], ["bubblewrap"])
        self.assertEqual(config["bundle"]["linux"]["rpm"]["depends"], ["bubblewrap"])

        release = RELEASE.read_text(encoding="utf-8")
        worker = 'agentum-sdd-worker-${VER}-${OSARCH}'
        self.assertIn("--bin agentum-sdd-worker", release)
        self.assertIn(worker, release)
        self.assertIn('"agentum-sdd-worker-${ver}-linux-x64"', release)
        self.assertIn('agentum-sdd-worker ${VER} protocol=1', release)
        self.assertIn("dist/agentum-sdd-worker-*", release)

    def test_macos_bundle_configuration_preserves_runtime_boundaries(self) -> None:
        config = json.loads(TAURI_CONFIG.read_text(encoding="utf-8"))
        macos = config["bundle"]["macOS"]
        self.assertTrue(macos["hardenedRuntime"])
        self.assertEqual(macos["entitlements"], "Entitlements.plist")
        self.assertNotIn("windows", config["bundle"])

        entitlements = MACOS_ENTITLEMENTS.read_text(encoding="utf-8")
        self.assertIn("com.apple.security.device.audio-input", entitlements)
        self.assertNotIn("disable-library-validation", entitlements)
        self.assertNotIn("allow-unsigned-executable-memory", entitlements)

        release = RELEASE.read_text(encoding="utf-8")
        self.assertIn('test "$APP_PATH" = "$BUNDLE_DIR/macos/Agentum.app"', release)
        self.assertIn("CFBundleIdentifier", release)
        self.assertIn("NSMicrophoneUsageDescription", release)
        self.assertIn("NSAppTransportSecurity.NSAllowsLocalNetworking", release)
        self.assertIn('tar -tzf "dist/${STEM}.app.tar.gz"', release)
        self.assertIn('hdiutil verify "dist/${STEM}.dmg"', release)
        self.assertNotIn("TeamIdentifier", release)
        self.assertNotIn("codesign", release)
        self.assertNotIn("stapler", release)


if __name__ == "__main__":
    unittest.main()
