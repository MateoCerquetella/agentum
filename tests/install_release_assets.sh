#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

export AGENTUM_INSTALL_SOURCE_ONLY=1
# The path is rooted by the explicit cd above; ShellCheck is invoked from that
# same repository root in CI.
source scripts/install.sh

for valid in v0.97.0 v1.0.0 v12.345.6789; do
  validate_release_tag "$valid"
done
for invalid in 0.97.0 v0.97 v0.97.0-rc1 v01.2.3 v1.02.3 v1.2.03 'v1.2.3;touch /tmp/x' ''; do
  if validate_release_tag "$invalid"; then
    echo "accepted hostile release tag: $invalid" >&2
    exit 1
  fi
done

VERSION=v0.97.0
RELEASE_VERSION=0.97.0
PLATFORM=macos-x64
test "$(asset_name dmg)" = "agentum-0.97.0-macos-x64.dmg"
PLATFORM=macos-arm64
test "$(asset_name dmg)" = "agentum-0.97.0-macos-arm64.dmg"
PLATFORM=linux-x64
test "$(asset_name appimage)" = "agentum-0.97.0-linux-x64.AppImage"
test "$(asset_name deb)" = "agentum-0.97.0-linux-x64.deb"
test "$(asset_name rpm)" = "agentum-0.97.0-linux-x64.rpm"
test "$(asset_name raw)" = "agentum-desktop-0.97.0-linux-x64"

test_tmp="$(mktemp -d)"
trap 'rm -rf -- "$test_tmp"' EXIT
TEMP_DIR="$test_tmp/download"
mkdir "$TEMP_DIR"
asset="agentum-0.97.0-linux-x64.AppImage"
printf 'verified desktop bundle\n' > "$TEMP_DIR/$asset"
(cd "$TEMP_DIR" && sha256sum "$asset" > SHA256SUMS)
verify_release_asset "$asset" "$TEMP_DIR/$asset"

if (printf '' > "$TEMP_DIR/SHA256SUMS"; verify_release_asset "$asset" "$TEMP_DIR/$asset") >/dev/null 2>&1; then
  echo "missing checksum entry was accepted" >&2
  exit 1
fi
(cd "$TEMP_DIR" && sha256sum "$asset" > SHA256SUMS)
checksum_line="$(cat "$TEMP_DIR/SHA256SUMS")"
printf '%s\n%s\n' "$checksum_line" "$checksum_line" > "$TEMP_DIR/SHA256SUMS"
if (verify_release_asset "$asset" "$TEMP_DIR/$asset") >/dev/null 2>&1; then
  echo "duplicate checksum entry was accepted" >&2
  exit 1
fi
printf '%064d  %s\n' 0 "$asset" > "$TEMP_DIR/SHA256SUMS"
if (verify_release_asset "$asset" "$TEMP_DIR/$asset") >/dev/null 2>&1; then
  echo "checksum mismatch was accepted" >&2
  exit 1
fi

# The version-matched SSH worker is a release asset even though desktop
# installation remains a separate action. Its documented remote install path
# must use the same exact-one checksum verification as desktop bundles.
worker_asset="agentum-sdd-worker-0.97.0-linux-x64"
printf 'verified remote worker\n' > "$TEMP_DIR/$worker_asset"
(cd "$TEMP_DIR" && sha256sum "$worker_asset" > SHA256SUMS)
verify_release_asset "$worker_asset" "$TEMP_DIR/$worker_asset"

metadata_case() (
  fetch_stdout() { printf '%s\n' "$1"; }
  RELEASE_API="$1"
  detect_latest_version
  test "$VERSION" = v0.97.0
)
metadata_case '{"tag_name":"v0.97.0"}'
for hostile in \
  '{"tag_name":"0.97.0"}' \
  '{"tag_name":"v0.97.0-rc1"}' \
  '{"tag_name":"v0.97.0","other":{"tag_name":"v9.9.9"}}' \
  '{"tag_name":"v0.97.0; id"}'; do
  if (fetch_stdout() { printf '%s\n' "$1"; }; RELEASE_API="$hostile"; detect_latest_version) >/dev/null 2>&1; then
    echo "hostile release metadata was accepted: $hostile" >&2
    exit 1
  fi
done

INSTALL_DIR="$test_tmp/bin"
mkdir "$INSTALL_DIR"
printf 'new desktop\n' > "$test_tmp/new"
install_linux_file "$test_tmp/new"
test -x "$INSTALL_DIR/agentum-desktop"
test "$(cat "$INSTALL_DIR/agentum-desktop")" = "new desktop"
outside="$test_tmp/outside"
printf 'outside\n' > "$outside"
rm "$INSTALL_DIR/agentum-desktop"
ln -s "$outside" "$INSTALL_DIR/agentum-desktop"
if (install_linux_file "$test_tmp/new") >/dev/null 2>&1; then
  echo "symlinked install target was accepted" >&2
  exit 1
fi
test "$(cat "$outside")" = "outside"

make_fake_appimage() {
  fake_path="$1"
  icon_contents="$2"
  cat > "$fake_path" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "--appimage-extract" ] && [ "${2:-}" = "Agentum.png" ]; then
  mkdir -p squashfs-root
  cp "$0.icon" squashfs-root/Agentum.png
  exit 0
fi
exit 0
EOF
  printf '%s\n' "$icon_contents" > "$fake_path.icon"
  chmod +x "$fake_path"
}

INSTALL_DIR="$test_tmp/app image % bin"
LINUX_APPLICATIONS_DIR="$test_tmp/xdg/applications"
LINUX_ICON_DIR="$test_tmp/xdg/icons/hicolor/256x256/apps"
fake_v1="$test_tmp/agentum-v1.AppImage"
make_fake_appimage "$fake_v1" "agentum icon v1"
TEMP_DIR="$test_tmp/appimage-install-v1"
mkdir "$TEMP_DIR"
(cd "$test_tmp" && install_linux_appimage "agentum-v1.AppImage")
desktop_entry="$LINUX_APPLICATIONS_DIR/dev.agentum.app.desktop"
installed_icon="$LINUX_ICON_DIR/agentum-desktop.png"
test -x "$INSTALL_DIR/agentum-desktop"
cmp "$fake_v1" "$INSTALL_DIR/agentum-desktop"
cmp "$fake_v1.icon" "$installed_icon"
grep -Fqx "Exec=\"$test_tmp/app image %% bin/agentum-desktop\"" "$desktop_entry"
grep -Fqx "StartupWMClass=Agentum-desktop" "$desktop_entry"
if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$desktop_entry"
fi

# A later AppImage replaces the canonical executable, launcher, and icon.
fake_v2="$test_tmp/agentum-v2.AppImage"
make_fake_appimage "$fake_v2" "agentum icon v2"
printf '%s\n' '# version two' >> "$fake_v2"
TEMP_DIR="$test_tmp/appimage-install-v2"
mkdir "$TEMP_DIR"
install_linux_appimage "$fake_v2"
cmp "$fake_v2" "$INSTALL_DIR/agentum-desktop"
cmp "$fake_v2.icon" "$installed_icon"
grep -Fqx "Exec=\"$test_tmp/app image %% bin/agentum-desktop\"" "$desktop_entry"

# An unsafe desktop target is rejected before the canonical binary changes.
saved_desktop="$test_tmp/saved.desktop"
cp "$desktop_entry" "$saved_desktop"
outside_desktop="$test_tmp/outside.desktop"
printf '%s\n' 'outside desktop' > "$outside_desktop"
rm "$desktop_entry"
ln -s "$outside_desktop" "$desktop_entry"
TEMP_DIR="$test_tmp/appimage-install-desktop-symlink"
mkdir "$TEMP_DIR"
if (install_linux_appimage "$fake_v1") >/dev/null 2>&1; then
  echo "symlinked desktop target was accepted" >&2
  exit 1
fi
cmp "$fake_v2" "$INSTALL_DIR/agentum-desktop"
test "$(cat "$outside_desktop")" = "outside desktop"

# The same fail-closed rule applies to the icon target.
rm "$desktop_entry"
cp "$saved_desktop" "$desktop_entry"
outside_icon="$test_tmp/outside.png"
printf '%s\n' 'outside icon' > "$outside_icon"
rm "$installed_icon"
ln -s "$outside_icon" "$installed_icon"
TEMP_DIR="$test_tmp/appimage-install-icon-symlink"
mkdir "$TEMP_DIR"
if (install_linux_appimage "$fake_v1") >/dev/null 2>&1; then
  echo "symlinked icon target was accepted" >&2
  exit 1
fi
cmp "$fake_v2" "$INSTALL_DIR/agentum-desktop"
test "$(cat "$outside_icon")" = "outside icon"

if (desktop_exec_argument "$test_tmp/line"$'\n'"break") >/dev/null 2>&1; then
  echo "desktop executable path with a line break was accepted" >&2
  exit 1
fi
if (validate_install_directory "$test_tmp/line"$'\n'"break") >/dev/null 2>&1; then
  echo "install directory with a line break was accepted" >&2
  exit 1
fi

# A bundle without the required icon fails before replacing the installed app.
rm "$installed_icon"
printf '%s\n' 'agentum icon v2' > "$installed_icon"
missing_icon="$test_tmp/agentum-missing-icon.AppImage"
printf '%s\n' '#!/bin/sh' 'exit 0' > "$missing_icon"
chmod +x "$missing_icon"
TEMP_DIR="$test_tmp/appimage-install-missing-icon"
mkdir "$TEMP_DIR"
if (install_linux_appimage "$missing_icon") >/dev/null 2>&1; then
  echo "AppImage without a launcher icon was accepted" >&2
  exit 1
fi
cmp "$fake_v2" "$INSTALL_DIR/agentum-desktop"

app="$test_tmp/Agentum.app"
mkdir -p "$app/Contents/MacOS"
printf 'bundle metadata\n' > "$app/Contents/Info.plist"
verify_macos_bundle "$app"
rm "$app/Contents/Info.plist"
if (verify_macos_bundle "$app") >/dev/null 2>&1; then
  echo "macOS bundle without Info.plist was accepted" >&2
  exit 1
fi

echo "install release asset tests: PASS"
