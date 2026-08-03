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
LINUXDEPLOY_INSTALLER = ROOT / "scripts/install-linuxdeploy.sh"
APPIMAGE_AUDIT = ROOT / "scripts/check-appimage-libraries.sh"
MACOS_RELEASE_AUDIT = ROOT / "scripts/check-macos-release.sh"
MACOS_RELEASE_NOTE = ROOT / ".github/release-macos-note.md"


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
        self.assertEqual(combined.count(pin), 3)
        self.assertNotIn("actions/checkout@11d5960a326750d5838078e36cf38b85af677262", combined)

    def test_sdd_boundary_normalizes_windows_scan_paths_before_allowlisting(self) -> None:
        boundary = SDD_BOUNDARY.read_text(encoding="utf-8")
        scan = boundary.index('direct_workspace_callers="$({')
        allowlist = boundary.index('while IFS= read -r caller; do', scan)
        self.assertIn(r"| tr '\134' '/' | sort", boundary[scan:allowlist])

    def test_patch_release_version_is_consistent_everywhere(self) -> None:
        version = workspace_version()
        self.assertRegex(version, r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
        self.assertEqual(version, "0.98.14")

        config = json.loads(TAURI_CONFIG.read_text(encoding="utf-8"))
        self.assertEqual(config["version"], version)
        self.assertEqual(config["identifier"], "dev.agentum.app")
        self.assertEqual(
            config["plugins"]["updater"]["pubkey"],
            "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDc5MTQ2QzM5QzkwQzUwQTEKUldTaFVBekpPV3dVZVhVNVc1NFBpVnBFeUpYcEE5NUEydkUxMFFxblhEV3VBUXM0QzlDUXo1K1oK",
        )

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

    def test_publication_requires_only_supported_release_secrets(self) -> None:
        release = RELEASE.read_text(encoding="utf-8")
        required = {
            "TAURI_SIGNING_PRIVATE_KEY",
            "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
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
            "HOMEBREW_TAP_DEPLOY_KEY",
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

    def test_macos_updater_signing_and_runtime_audit_are_release_gates(self) -> None:
        workflow = RELEASE.read_text(encoding="utf-8")
        config = json.loads(TAURI_CONFIG.read_text(encoding="utf-8"))
        self.assertIn("tool: tauri-cli@2.11.2", workflow)
        self.assertIn('test -n "$TAURI_SIGNING_PRIVATE_KEY"', workflow)
        self.assertIn('test -n "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD"', workflow)
        self.assertIn("Verify every updater signature against the embedded key", workflow)
        self.assertIn("agentum-updater-verify", workflow)
        self.assertEqual(config["bundle"]["macOS"]["signingIdentity"], "-")
        self.assertFalse(config["bundle"]["macOS"]["hardenedRuntime"])
        self.assertTrue(MACOS_RELEASE_AUDIT.exists())
        self.assertFalse(MACOS_RELEASE_NOTE.exists())
        audit = MACOS_RELEASE_AUDIT.read_text(encoding="utf-8")
        self.assertIn("verify_adhoc_without_runtime", audit)
        self.assertIn("flags=.*runtime", audit)
        self.assertIn("Agentum exited during native macOS runtime smoke", audit)
        for required in (
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
            "macos-15-intel",
            "macos-14",
            "check-macos-release.sh",
            "darwin-aarch64",
            "darwin-x86_64",
        ):
            self.assertIn(required, workflow)
        for unavailable_signing_path in (
            "APPLE_CERTIFICATE",
            "APPLE_SIGNING_IDENTITY",
            "notarytool",
            "stapler",
        ):
            self.assertNotIn(unavailable_signing_path, workflow)

    def test_release_rust_dependency_resolution_is_lockfile_bound(self) -> None:
        workflow = RELEASE.read_text(encoding="utf-8")
        self.assertIn('if [[ "$BUILD_TARGET" == *apple-darwin* ]]', workflow)
        self.assertIn(
            'cargo build --locked --release --target "$BUILD_TARGET" -p sherpa-rs',
            workflow,
        )
        self.assertEqual(workflow.count("cargo tauri build --target"), 1)
        self.assertEqual(workflow.count("--verbose -- --locked"), 1)
        self.assertIn('LD_LIBRARY_PATH="$GITHUB_WORKSPACE/target/$BUILD_TARGET/release', workflow)
        self.assertIn('"agentum-${ver}-linux-x64.AppImage.sig"', workflow)
        self.assertNotIn("AppImage.tar.gz", workflow)
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
        self.assertNotIn("StrictHostKeyChecking", release)
        self.assertNotIn("GIT_SSH_COMMAND", release)

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

    def test_linux_appimage_uses_pinned_exclusions_and_audits_the_final_bundle(self) -> None:
        release = RELEASE.read_text(encoding="utf-8")
        installer = LINUXDEPLOY_INSTALLER.read_text(encoding="utf-8")
        audit = APPIMAGE_AUDIT.read_text(encoding="utf-8")

        self.assertIn('version="1-alpha-20251107-1"', installer)
        self.assertIn(
            'binary_sha256="c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d"',
            installer,
        )
        self.assertIn("Install pinned linuxdeploy (Linux)", release)
        self.assertIn('"useLocalToolsDir":true', release)
        self.assertIn("LINUXDEPLOY_EXCLUDED_LIBRARIES", release)
        for library in (
            "libwayland-client.so*",
            "libwayland-cursor.so*",
            "libwayland-egl.so*",
            "libwayland-server.so*",
        ):
            self.assertIn(library, release)
            self.assertIn(library, audit)
        self.assertIn('scripts/check-appimage-libraries.sh "dist/${STEM}.AppImage"', release)
        self.assertIn("libsherpa-onnx-c-api.so", audit)
        self.assertIn("libonnxruntime.so", audit)

    def test_linux_appimage_installs_a_canonical_desktop_launcher(self) -> None:
        release = RELEASE.read_text(encoding="utf-8")

        install_check = release.index("Verify canonical Linux AppImage installation")
        restricted_scan = release.index("Scan generated bundles for restricted content")
        self.assertLess(install_check, restricted_scan)
        self.assertIn("install_linux_appimage", release[install_check:restricted_scan])
        self.assertIn("dev.agentum.app.desktop", release[install_check:restricted_scan])
        self.assertIn("agentum-desktop.png", release[install_check:restricted_scan])

    def test_checksum_manifest_does_not_hash_itself(self) -> None:
        release = RELEASE.read_text(encoding="utf-8")

        self.assertIn("! -name SHA256SUMS", release)

    def test_publication_depends_directly_on_the_verified_draft(self) -> None:
        release = RELEASE.read_text(encoding="utf-8")

        aggregate_start = release.index("\n  aggregate:\n")
        release_start = release.index("\n  release:\n")
        publish_start = release.index("\n  publish:\n")
        aggregate_job = release[aggregate_start:release_start]
        self.assertIn("name: verify complete release staging", aggregate_job)
        self.assertIn("name: verified-release-staging", aggregate_job)
        self.assertNotIn("contents: write", aggregate_job)
        self.assertNotIn(
            "if: inputs.publish && startsWith(github.ref, 'refs/tags/')",
            aggregate_job,
        )
        draft = release.index("Create private GitHub release draft")
        publish = release.index("name: publish verified release")
        self.assertLess(draft, publish)
        draft_job = release[release_start:publish_start]
        self.assertIn("    needs: aggregate", draft_job)
        self.assertIn("      contents: write", draft_job)
        publish_job = release[publish_start:]
        self.assertIn("    needs: release", publish_job)
        for retired in (
            "release-homebrew-checksums",
            "homebrew:",
            "bump homebrew cask",
            "homebrew-tap",
            "agentum.cask.tmpl",
        ):
            self.assertNotIn(retired, release)

    def test_certificate_free_macos_distribution_disables_hardened_runtime(self) -> None:
        config = json.loads(TAURI_CONFIG.read_text(encoding="utf-8"))
        macos = config["bundle"]["macOS"]
        self.assertFalse(macos["hardenedRuntime"])
        self.assertEqual(macos["entitlements"], "Entitlements.plist")
        self.assertEqual(macos["signingIdentity"], "-")
        self.assertNotIn("windows", config["bundle"])

        entitlements = MACOS_ENTITLEMENTS.read_text(encoding="utf-8")
        self.assertIn("com.apple.security.device.audio-input", entitlements)
        self.assertNotIn("disable-library-validation", entitlements)
        self.assertNotIn("allow-unsigned-executable-memory", entitlements)

        release = RELEASE.read_text(encoding="utf-8")
        self.assertIn("CFBundleIdentifier", release)
        self.assertIn("NSMicrophoneUsageDescription", release)
        self.assertIn("NSAppTransportSecurity.NSAllowsLocalNetworking", release)

    def test_release_matrix_contains_exactly_supported_native_targets(self) -> None:
        release = RELEASE.read_text(encoding="utf-8")

        targets = re.findall(r"^\s+- target: (\S+)$", release, re.MULTILINE)
        self.assertEqual(
            targets,
            [
                "x86_64-unknown-linux-gnu",
                "x86_64-apple-darwin",
                "aarch64-apple-darwin",
                "x86_64-pc-windows-msvc",
            ],
        )
        self.assertRegex(
            release,
            r"target: x86_64-unknown-linux-gnu\s+runner: ubuntu-22\.04",
        )
        self.assertRegex(
            release,
            r"target: x86_64-pc-windows-msvc\s+runner: windows-latest",
        )
        self.assertRegex(
            release,
            r"target: x86_64-apple-darwin\s+runner: macos-15-intel",
        )
        self.assertRegex(
            release,
            r"target: aarch64-apple-darwin\s+runner: macos-14",
        )

    def test_aggregation_and_updater_require_complete_macos_payloads(self) -> None:
        release = RELEASE.read_text(encoding="utf-8")

        for required in (
            '"agentum-${ver}-macos-arm64.dmg"',
            '"agentum-${ver}-macos-arm64.app.tar.gz"',
            '"agentum-${ver}-macos-arm64.app.tar.gz.sig"',
            '"agentum-${ver}-macos-x64.dmg"',
            '"agentum-${ver}-macos-x64.app.tar.gz"',
            '"agentum-${ver}-macos-x64.app.tar.gz.sig"',
            '"agentum-${ver}-windows-x64-setup.exe"',
            '"agentum-${ver}-windows-x64-setup.exe.sig"',
            '"agentum-${ver}-linux-x64.deb"',
            '"agentum-${ver}-linux-x64.rpm"',
            '"agentum-${ver}-linux-x64.AppImage"',
            '"agentum-${ver}-linux-x64.AppImage.sig"',
            '"agentum-desktop-${ver}-linux-x64"',
            '"agentum-sdd-worker-${ver}-linux-x64"',
            '"agentum-${ver}-source.tar.gz"',
            "unexpected release asset roster",
            'find dist -mindepth 1 -maxdepth 1 -type f',
            'emit darwin-aarch64 "agentum-${ver}-macos-arm64.app.tar.gz"',
            'emit darwin-x86_64  "agentum-${ver}-macos-x64.app.tar.gz"',
            'emit windows-x86_64 "agentum-${ver}-windows-x64-setup.exe"',
            'emit linux-x86_64   "agentum-${ver}-linux-x64.AppImage"',
            "test \"$(jq '.platforms | length' dist/latest.json)\" -eq 4",
            'darwin-aarch64,darwin-x86_64,linux-x86_64,windows-x86_64',
        ):
            self.assertIn(required, release)
        self.assertNotIn("Reject retired release payloads", release)
        self.assertNotIn("retired release payload", release)


if __name__ == "__main__":
    unittest.main()
