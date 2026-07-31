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
        self.assertEqual(version, "0.98.11")

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

    def test_publication_requires_available_secrets_without_platform_certificates(self) -> None:
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

    def test_updater_signing_and_adhoc_macos_validation_are_required(self) -> None:
        workflow = RELEASE.read_text(encoding="utf-8")
        config = json.loads(TAURI_CONFIG.read_text(encoding="utf-8"))
        self.assertIn("tool: tauri-cli@2.11.2", workflow)
        self.assertIn('test -n "$TAURI_SIGNING_PRIVATE_KEY"', workflow)
        self.assertIn('test -n "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD"', workflow)
        self.assertIn("Verify every updater signature against the embedded key", workflow)
        self.assertIn("agentum-updater-verify", workflow)
        self.assertEqual(config["bundle"]["macOS"]["signingIdentity"], "-")
        self.assertIn("Verify ad-hoc macOS release", workflow)
        self.assertIn('scripts/check-macos-release.sh "$APP_PATH" "dist/${STEM}.dmg"', workflow)
        self.assertLess(
            workflow.index("Verify ad-hoc macOS release"),
            workflow.index("actions/upload-artifact"),
        )
        for unsupported in (
            "Developer ID",
            "notarytool",
            "stapler",
            "Import Authenticode certificate",
            "Verify Authenticode signatures",
            "Remove Authenticode certificate material",
            "certificateThumbprint",
        ):
            self.assertNotIn(unsupported, workflow)

    def test_release_rust_dependency_resolution_is_lockfile_bound(self) -> None:
        workflow = RELEASE.read_text(encoding="utf-8")
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

    def test_homebrew_consumes_checksums_without_reading_the_private_draft(self) -> None:
        release = RELEASE.read_text(encoding="utf-8")

        self.assertEqual(release.count("name: release-homebrew-checksums"), 2)
        checksum_upload = release.index("name: release-homebrew-checksums")
        draft = release.index("Create private GitHub release draft")
        homebrew = release.index("name: bump homebrew cask")
        checksum_download = release.index("name: release-homebrew-checksums", homebrew)
        publish = release.index("name: publish verified release")
        self.assertLess(checksum_upload, draft)
        self.assertLess(homebrew, checksum_download)
        self.assertLess(checksum_download, publish)
        self.assertNotIn('gh release download "$TAG"', release)

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
        self.assertIn("scripts/check-macos-release.sh", release)

    def test_macos_targets_build_and_launch_on_matching_native_runners(self) -> None:
        release = RELEASE.read_text(encoding="utf-8")

        self.assertRegex(
            release,
            r"target: x86_64-apple-darwin\s+runner: macos-15-intel",
        )
        self.assertRegex(
            release,
            r"target: aarch64-apple-darwin\s+runner: macos-14",
        )
        audit = MACOS_RELEASE_AUDIT.read_text(encoding="utf-8")
        self.assertIn('test "$(uname -m)" = "$EXPECTED_HOST_ARCH"', audit)
        self.assertIn('"$INSTALLED_EXECUTABLE" >"$RUNTIME_LOG"', audit)
        self.assertIn("native runtime checks passed", audit)

    def test_macos_release_audit_enforces_adhoc_integrity_without_bypasses(self) -> None:
        audit = MACOS_RELEASE_AUDIT.read_text(encoding="utf-8")

        for required in (
            "codesign --verify --deep --strict",
            "Signature=adhoc",
            "TeamIdentifier=not set",
            "runtime",
            "[(,]runtime[),]",
            "spctl --assess --type execute",
            "hdiutil attach",
            "com.apple.quarantine",
            "Contents/Frameworks",
            'lipo "$FRAMEWORKS/$library" -verify_arch x86_64 arm64',
            "otool -L",
        ):
            self.assertIn(required, audit)
        for bypass in (
            "xattr -d com.apple.quarantine",
            "xattr -cr",
            "spctl --master-disable",
            "codesign --force --deep",
            "Developer ID Application:",
            "notarytool",
            "stapler",
        ):
            self.assertNotIn(bypass, audit)


if __name__ == "__main__":
    unittest.main()
