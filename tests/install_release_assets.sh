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

signature_log="$test_tmp/signature.log"
codesign() { printf 'codesign:%s\n' "${*: -1}" >> "$signature_log"; }
spctl() { printf 'spctl:%s\n' "${*: -1}" >> "$signature_log"; }
xcrun() { printf 'xcrun:%s\n' "${*: -1}" >> "$signature_log"; }
app="$test_tmp/Agentum.app"
mkdir "$app"
verify_macos_bundle "$app"
test "$(wc -l < "$signature_log")" -eq 3
grep -Fqx "codesign:$app" "$signature_log"
grep -Fqx "spctl:$app" "$signature_log"
grep -Fqx "xcrun:$app" "$signature_log"

echo "install release asset tests: PASS"
