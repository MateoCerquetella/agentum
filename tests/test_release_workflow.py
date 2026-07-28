import json
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]
RELEASE = ROOT / ".github/workflows/release.yml"
CI = ROOT / ".github/workflows/ci.yml"
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
    def test_clean_runners_activate_real_cross_platform_sdd_boundaries(self) -> None:
        ci = CI.read_text(encoding="utf-8")
        self.assertIn("Use no-follow-safe test temp root (macOS)", ci)
        self.assertIn("printf 'TMPDIR=%s\\n' \"$RUNNER_TEMP\" >> \"$GITHUB_ENV\"", ci)
        self.assertIn("Enable and probe Bubblewrap sandbox (Linux)", ci)
        self.assertIn("kernel.apparmor_restrict_unprivileged_userns=0", ci)
        self.assertIn(
            "bwrap --die-with-parent --unshare-pid --ro-bind / / "
            "--proc /proc --dev /dev -- true",
            ci,
        )

    def test_workflows_pin_the_current_node24_checkout_action(self) -> None:
        workflows = [CI, RELEASE, ROOT / ".github/workflows/codeql.yml"]
        pin = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1"
        combined = "\n".join(path.read_text(encoding="utf-8") for path in workflows)
        self.assertEqual(combined.count(pin), 9)
        self.assertNotIn("actions/checkout@11d5960a326750d5838078e36cf38b85af677262", combined)

    def test_sdd_boundary_normalizes_windows_scan_paths_before_allowlisting(self) -> None:
        boundary = SDD_BOUNDARY.read_text(encoding="utf-8")
        scan = boundary.index('direct_workspace_callers="$({')
        allowlist = boundary.index('while IFS= read -r caller; do', scan)
        self.assertIn(r"| tr '\134' '/' | sort", boundary[scan:allowlist])

    def test_patch_release_version_is_consistent_everywhere(self) -> None:
        version = workspace_version()
        self.assertRegex(version, r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
        self.assertEqual(version, "0.98.2")

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

    def test_publication_requires_every_external_signing_secret(self) -> None:
        release = RELEASE.read_text(encoding="utf-8")
        ci = CI.read_text(encoding="utf-8")
        required = {
            "TAURI_SIGNING_PRIVATE_KEY",
            "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
            "APPLE_CERTIFICATE",
            "APPLE_CERTIFICATE_PASSWORD",
            "APPLE_SIGNING_IDENTITY",
            "APPLE_ID",
            "APPLE_PASSWORD",
            "APPLE_TEAM_ID",
            "WINDOWS_CERTIFICATE",
            "WINDOWS_CERTIFICATE_PASSWORD",
            "WINDOWS_CERTIFICATE_THUMBPRINT",
            "HOMEBREW_TAP_DEPLOY_KEY",
            "AGENTUM_RESTRICTED_PATTERNS",
        }
        for secret in required:
            self.assertIn(f"      {secret}:\n        required: true", release)
            self.assertIn(f"      {secret}: ${{{{ secrets.{secret} }}}}", ci)

        self.assertIn('test "$GITHUB_REF" = "refs/tags/v${version}"', release)
        self.assertIn('test "$(git rev-parse HEAD)" = "$(git rev-parse refs/remotes/origin/main)"', release)
        self.assertIn('test "$(git cat-file -t "refs/tags/$TAG")" = "tag"', release)
        self.assertIn("'.verification.verified'", release)

    def test_signing_keychain_is_imported_validated_and_always_removed(self) -> None:
        workflow = RELEASE.read_text(encoding="utf-8")
        imported = workflow.index("Import Developer ID certificate into an isolated keychain")
        built = workflow.index("Build desktop installers")
        removed = workflow.index("Remove isolated signing keychain")
        self.assertLess(imported, built)
        self.assertLess(built, removed)
        self.assertIn("security import", workflow)
        self.assertIn("security set-key-partition-list", workflow)
        self.assertIn("security find-identity", workflow)
        self.assertIn("tool: tauri-cli@2.11.2", workflow)
        self.assertIn("always() && contains(matrix.target, 'apple')", workflow)
        self.assertNotIn('echo "HOME=', workflow)
        self.assertEqual(workflow.count("-CertStoreLocation 'Cert:\\CurrentUser\\My'"), 1)
        self.assertIn("-notmatch '^[0-9A-F]{40}$'", workflow)
        self.assertIn('[[ "$normalized_thumbprint" =~ ^[0-9A-Fa-f]{40}$ ]]', workflow)
        self.assertNotIn("${WINDOWS_CERTIFICATE_THUMBPRINT}\"}}}", workflow)

        cleanup = workflow.index("Remove Authenticode certificate material (Windows)")
        cleanup_guard = workflow.index(
            "if ($thumbprint -match '^[0-9A-F]{40}$')", cleanup
        )
        cleanup_store_path = workflow.index(
            '$storePath = "Cert:\\CurrentUser\\My\\$thumbprint"', cleanup
        )
        self.assertLess(cleanup_guard, cleanup_store_path)

    def test_release_rust_dependency_resolution_is_lockfile_bound(self) -> None:
        workflow = RELEASE.read_text(encoding="utf-8")
        self.assertIn(
            'cargo build --locked --release --target "$BUILD_TARGET" -p sherpa-rs',
            workflow,
        )
        self.assertEqual(workflow.count("cargo tauri build --target"), 2)
        self.assertEqual(workflow.count("-- --locked"), 2)
        self.assertNotIn("cargo tauri build --locked", workflow)

    def test_every_release_stage_receives_required_restricted_policy(self) -> None:
        release = RELEASE.read_text(encoding="utf-8")
        ci = CI.read_text(encoding="utf-8")
        self.assertIn("AGENTUM_RESTRICTED_PATTERNS:\n        required: true", release)
        self.assertIn("Validate restricted-content release policy availability", release)
        self.assertIn(
            "AGENTUM_RESTRICTED_PATTERNS: ${{ secrets.AGENTUM_RESTRICTED_PATTERNS }}",
            ci,
        )
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

    def test_release_is_blocked_on_real_source_bound_provider_lifecycle(self) -> None:
        ci = CI.read_text(encoding="utf-8")
        release = RELEASE.read_text(encoding="utf-8")
        providers = ["claude", "codex", "agent", "gemini", "hermes", "opencode", "aider"]

        self.assertIn("runs-on: [self-hosted, linux, x64, agentum-provider-conformance]", ci)
        self.assertIn("provider-authority:", ci)
        self.assertIn("runs-on: ubuntu-latest", ci)
        self.assertIn("needs: [provider-authority, rust]", ci)
        self.assertIn("needs: [rust, provider-authority, provider-conformance]", ci)
        self.assertGreaterEqual(ci.count("if: startsWith(github.ref, 'refs/tags/v')"), 2)
        self.assertIn("repos/$REPO/branches/main", ci)
        self.assertIn("'.verification.verified'", ci)
        self.assertIn("agentum-sdd-provider-conformance", ci)
        self.assertIn('--source-revision "$GITHUB_SHA"', ci)
        self.assertIn("provider-conformance-${{ github.sha }}", ci)
        for provider in providers:
            self.assertEqual(ci.count(f"--provider {provider}"), 1)
            self.assertIn(f"--require-provider {provider}", ci)
            self.assertIn(f"--require-provider {provider}", release)
        self.assertIn("Build evidence verifier from authority-checked source", release)
        self.assertIn("Verify untrusted source-bound provider evidence as data", release)
        self.assertIn("sha256sum -c SHA256SUMS", release)
        self.assertIn("repos/$REPO/branches/main", release)
        self.assertNotIn("chmod 0500 agentum-sdd-provider-conformance", release)
        self.assertNotIn("./agentum-sdd-provider-conformance verify-report", release)
        self.assertNotIn("cp \"$runner\"", ci)
        self.assertIn('--source-revision "$GITHUB_SHA"', release)
        self.assertIn("refusing mixed evidence", ci)
        self.assertGreaterEqual(ci.count("report_size <= 2097152"), 1)
        self.assertGreaterEqual(release.count("report_size <= 2097152"), 1)
        authority = release.index("Validate version and publication authority")
        build_verifier = release.index("Build evidence verifier from authority-checked source")
        download = release.index("actions/download-artifact")
        verify_data = release.index("Verify untrusted source-bound provider evidence as data")
        self.assertLess(authority, build_verifier)
        self.assertLess(build_verifier, download)
        self.assertLess(download, verify_data)

    def test_windows_sdd_is_a_tested_remote_client_only_boundary(self) -> None:
        ci = CI.read_text(encoding="utf-8")
        self.assertIn("Enforce Windows remote-client-only SDD boundary", ci)
        self.assertIn("if: runner.os == 'Windows'", ci)
        self.assertIn(
            "cargo test --locked -p agentum-server windows_local_sdd_boundary -- --nocapture",
            ci,
        )

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

    def test_macos_hardened_runtime_carries_and_verifies_microphone_entitlement(self) -> None:
        config = json.loads(TAURI_CONFIG.read_text(encoding="utf-8"))
        macos = config["bundle"]["macOS"]
        self.assertTrue(macos["hardenedRuntime"])
        self.assertEqual(macos["entitlements"], "Entitlements.plist")

        entitlements = MACOS_ENTITLEMENTS.read_text(encoding="utf-8")
        self.assertIn("com.apple.security.device.audio-input", entitlements)
        self.assertNotIn("disable-library-validation", entitlements)
        self.assertNotIn("allow-unsigned-executable-memory", entitlements)

        release = RELEASE.read_text(encoding="utf-8")
        self.assertIn('codesign --display --entitlements "$ENTITLEMENTS_PATH" "$APP_PATH"', release)
        self.assertIn(
            "plutil -extract com.apple.security.device.audio-input raw",
            release,
        )
        self.assertIn('test "$APP_PATH" = "$BUNDLE_DIR/macos/Agentum.app"', release)
        self.assertIn("TeamIdentifier=$APPLE_TEAM_ID", release)
        self.assertIn(r"flags=.*\(runtime\)", release)
        self.assertIn("com.apple.security.cs.disable-library-validation", release)
        self.assertIn("NSAppTransportSecurity.NSAllowsLocalNetworking", release)


if __name__ == "__main__":
    unittest.main()
