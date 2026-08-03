#!/usr/bin/env bash
# Validate the exact certificate-free macOS payload that users install. The
# outer app and every nested Mach-O must be ad-hoc signed without Hardened
# Runtime; enabling it without a shared Developer ID Team ID makes dyld reject
# the bundled ONNX libraries before main starts.

set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: $0 <Agentum.app> <agentum.dmg> <target-triple> <version>" >&2
  exit 2
fi

APP_PATH="$1"
DMG_PATH="$2"
TARGET_TRIPLE="$3"
EXPECTED_VERSION="$4"

test "$(uname -s)" = "Darwin"
test -d "$APP_PATH"
test -f "$DMG_PATH"
[[ "$EXPECTED_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]

case "$TARGET_TRIPLE" in
  aarch64-apple-darwin) EXPECTED_ARCH="arm64" ;;
  x86_64-apple-darwin) EXPECTED_ARCH="x86_64" ;;
  *)
    echo "unsupported macOS target: $TARGET_TRIPLE" >&2
    exit 2
    ;;
esac
test "$(uname -m)" = "$EXPECTED_ARCH"

WORK_PARENT="${RUNNER_TEMP:-/tmp}"
WORK_DIR="$(mktemp -d "$WORK_PARENT/agentum-macos-release.XXXXXX")"
MOUNT_DIR="$WORK_DIR/mount"
INSTALL_DIR="$WORK_DIR/install"
INSTALLED_APP="$INSTALL_DIR/Agentum.app"
RUNTIME_LOG="$WORK_DIR/runtime.log"
APP_PID=""
MOUNTED=false

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  if [[ -n "$APP_PID" ]] && kill -0 "$APP_PID" 2>/dev/null; then
    kill -TERM "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
  fi
  if [[ "$MOUNTED" == true ]]; then
    hdiutil detach "$MOUNT_DIR" -quiet \
      || hdiutil detach "$MOUNT_DIR" -force -quiet \
      || true
  fi
  if [[ -d "$WORK_DIR" ]]; then
    find "$WORK_DIR" -depth -delete
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

verify_adhoc_without_runtime() {
  local candidate="$1"
  local signature
  codesign --verify --strict --verbose=4 "$candidate"
  signature="$(codesign --display --verbose=4 "$candidate" 2>&1)"
  grep -Fq 'Signature=adhoc' <<<"$signature"
  grep -Fq 'TeamIdentifier=not set' <<<"$signature"
  if grep -Eq '^Authority=|flags=.*runtime' <<<"$signature"; then
    echo "invalid certificate-free signing policy: $candidate" >&2
    printf '%s\n' "$signature" >&2
    exit 1
  fi
}

INFO_PLIST="$APP_PATH/Contents/Info.plist"
EXECUTABLE="$APP_PATH/Contents/MacOS/agentum-desktop"
FRAMEWORKS="$APP_PATH/Contents/Frameworks"
test -f "$INFO_PLIST"
test -x "$EXECUTABLE"
test -d "$FRAMEWORKS"

test "$(plutil -extract CFBundleExecutable raw "$INFO_PLIST")" = "agentum-desktop"
test "$(plutil -extract CFBundleIdentifier raw "$INFO_PLIST")" = "dev.agentum.app"
test "$(plutil -extract CFBundleShortVersionString raw "$INFO_PLIST")" = "$EXPECTED_VERSION"
test "$(plutil -extract CFBundleVersion raw "$INFO_PLIST")" = "$EXPECTED_VERSION"
test "$(plutil -extract LSMinimumSystemVersion raw "$INFO_PLIST")" = "11.0"
test -n "$(plutil -extract NSMicrophoneUsageDescription raw "$INFO_PLIST")"
test "$(plutil -extract NSAppTransportSecurity.NSAllowsLocalNetworking raw "$INFO_PLIST")" = "true"

lipo "$EXECUTABLE" -verify_arch "$EXPECTED_ARCH"
file "$EXECUTABLE" | grep -Fq 'Mach-O 64-bit executable'

EXPECTED_DYLIBS=(
  libonnxruntime.1.17.1.dylib
  libsherpa-onnx-c-api.dylib
  libsherpa-onnx-cxx-api.dylib
)
for library in "${EXPECTED_DYLIBS[@]}"; do
  test -f "$FRAMEWORKS/$library"
  lipo "$FRAMEWORKS/$library" -verify_arch "$EXPECTED_ARCH"
done
test "$(find "$FRAMEWORKS" -maxdepth 1 -type f -name '*.dylib' | wc -l | tr -d ' ')" -eq 3

otool -L "$EXECUTABLE" | grep -Fq '@rpath/libonnxruntime.1.17.1.dylib'
otool -L "$EXECUTABLE" | grep -Fq '@rpath/libsherpa-onnx-c-api.dylib'
otool -l "$EXECUTABLE" | grep -A2 LC_RPATH | grep -Eq '@(loader_path|executable_path)/../Frameworks'

codesign --verify --deep --strict --verbose=4 "$APP_PATH"
MACHO_COUNT=0
while IFS= read -r -d '' candidate; do
  if file -b "$candidate" | grep -Fq 'Mach-O'; then
    MACHO_COUNT=$((MACHO_COUNT + 1))
    verify_adhoc_without_runtime "$candidate"
  fi
done < <(find "$APP_PATH/Contents/MacOS" "$FRAMEWORKS" -type f -print0)
test "$MACHO_COUNT" -ge 4

ENTITLEMENTS="$WORK_DIR/entitlements.plist"
codesign --display --entitlements :- "$EXECUTABLE" >"$ENTITLEMENTS" 2>/dev/null
plutil -lint "$ENTITLEMENTS" >/dev/null
ENTITLEMENTS_JSON="$(plutil -convert json -o - "$ENTITLEMENTS")"
test "$(jq -r '."com.apple.security.device.audio-input"' <<<"$ENTITLEMENTS_JSON")" = "true"
for forbidden in \
  com.apple.security.get-task-allow \
  com.apple.security.cs.disable-library-validation \
  com.apple.security.cs.allow-unsigned-executable-memory; do
  if jq -e --arg key "$forbidden" 'has($key)' <<<"$ENTITLEMENTS_JSON" >/dev/null; then
    echo "forbidden release entitlement: $forbidden" >&2
    exit 1
  fi
done

hdiutil verify "$DMG_PATH"
mkdir -p "$MOUNT_DIR" "$INSTALL_DIR"
hdiutil attach -readonly -nobrowse -mountpoint "$MOUNT_DIR" "$DMG_PATH" >/dev/null
MOUNTED=true
test -d "$MOUNT_DIR/Agentum.app"
test -L "$MOUNT_DIR/Applications"
test "$(readlink "$MOUNT_DIR/Applications")" = "/Applications"
ditto --rsrc --extattr "$MOUNT_DIR/Agentum.app" "$INSTALLED_APP"
codesign --verify --deep --strict --verbose=4 "$INSTALLED_APP"
verify_adhoc_without_runtime "$INSTALLED_APP"

"$INSTALLED_APP/Contents/MacOS/agentum-desktop" >"$RUNTIME_LOG" 2>&1 &
APP_PID=$!
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    wait "$APP_PID" || true
    cat "$RUNTIME_LOG" >&2
    echo "Agentum exited during native macOS runtime smoke" >&2
    exit 1
  fi
  sleep 1
done

kill -TERM "$APP_PID"
wait "$APP_PID" 2>/dev/null || true
APP_PID=""
echo "certificate-free macOS release passed for $TARGET_TRIPLE"
