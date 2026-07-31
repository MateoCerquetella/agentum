#!/usr/bin/env bash
# Verify the exact certificate-free macOS release payload. Agentum deliberately
# uses an ad-hoc signature for bundle integrity, so Gatekeeper rejection is an
# expected distribution property and the native executable is tested separately.

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
  aarch64-apple-darwin)
    EXPECTED_HOST_ARCH="arm64"
    EXPECTED_BINARY_ARCH="arm64"
    ;;
  x86_64-apple-darwin)
    EXPECTED_HOST_ARCH="x86_64"
    EXPECTED_BINARY_ARCH="x86_64"
    ;;
  *)
    echo "unsupported macOS target: $TARGET_TRIPLE" >&2
    exit 2
    ;;
esac

test "$(uname -m)" = "$EXPECTED_HOST_ARCH"

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
    if hdiutil detach "$MOUNT_DIR" -quiet \
      || hdiutil detach "$MOUNT_DIR" -force -quiet; then
      MOUNTED=false
    else
      echo "warning: could not detach temporary release-audit image at $MOUNT_DIR" >&2
    fi
  fi
  if [[ "$MOUNTED" != true \
    && "$WORK_DIR" == "$WORK_PARENT"/agentum-macos-release.* \
    && -d "$WORK_DIR" ]]; then
    find "$WORK_DIR" -depth -delete
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

verify_adhoc_signature() {
  local candidate="$1"
  local signature
  codesign --verify --strict --verbose=4 "$candidate"
  signature="$(codesign --display --verbose=4 "$candidate" 2>&1)"
  grep -Fq 'Signature=adhoc' <<<"$signature"
  grep -Fq 'TeamIdentifier=not set' <<<"$signature"
  if grep -q '^Authority=' <<<"$signature"; then
    echo "certificate-backed signature is forbidden for this release: $candidate" >&2
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
test "$(plutil -extract CFBundlePackageType raw "$INFO_PLIST")" = "APPL"
test "$(plutil -extract CFBundleShortVersionString raw "$INFO_PLIST")" = "$EXPECTED_VERSION"
test "$(plutil -extract CFBundleVersion raw "$INFO_PLIST")" = "$EXPECTED_VERSION"
test "$(plutil -extract LSMinimumSystemVersion raw "$INFO_PLIST")" = "11.0"
test -n "$(plutil -extract NSMicrophoneUsageDescription raw "$INFO_PLIST")"
test "$(plutil -extract NSAppTransportSecurity.NSAllowsLocalNetworking raw "$INFO_PLIST")" = "true"

for forbidden_ats in NSAllowsArbitraryLoads NSAllowsArbitraryLoadsInWebContent; do
  if plutil -extract "NSAppTransportSecurity.$forbidden_ats" raw "$INFO_PLIST" >/dev/null 2>&1; then
    echo "forbidden packaged ATS exception: $forbidden_ats" >&2
    exit 1
  fi
done

test "$(lipo -archs "$EXECUTABLE")" = "$EXPECTED_BINARY_ARCH"
file "$EXECUTABLE" | grep -Fq "Mach-O 64-bit executable $EXPECTED_BINARY_ARCH"

EXPECTED_DYLIBS=(
  libonnxruntime.1.17.1.dylib
  libsherpa-onnx-c-api.dylib
  libsherpa-onnx-cxx-api.dylib
)
for library in "${EXPECTED_DYLIBS[@]}"; do
  test -f "$FRAMEWORKS/$library"
  lipo "$FRAMEWORKS/$library" -verify_arch x86_64 arm64
done
test "$(find "$FRAMEWORKS" -maxdepth 1 -type f -name '*.dylib' | wc -l | tr -d ' ')" -eq 3

otool -L "$EXECUTABLE" | grep -Fq '@rpath/libonnxruntime.1.17.1.dylib'
otool -L "$EXECUTABLE" | grep -Fq '@rpath/libsherpa-onnx-c-api.dylib'
otool -l "$EXECUTABLE" | grep -A2 LC_RPATH | grep -Eq '@(loader_path|executable_path)/../Frameworks'

codesign --verify --deep --strict --verbose=4 "$APP_PATH"
verify_adhoc_signature "$APP_PATH"
APP_SIGNATURE="$(codesign --display --verbose=4 "$APP_PATH" 2>&1)"
if ! grep -Eq '^flags=.*[(,]runtime[),]' <<<"$APP_SIGNATURE"; then
  echo "hardened runtime flag is missing from app signature" >&2
  exit 1
fi

MACHO_COUNT=0
while IFS= read -r -d '' candidate; do
  if file -b "$candidate" | grep -Fq 'Mach-O'; then
    MACHO_COUNT=$((MACHO_COUNT + 1))
    verify_adhoc_signature "$candidate"
  fi
done < <(find "$APP_PATH/Contents/MacOS" "$FRAMEWORKS" -type f -print0)
test "$MACHO_COUNT" -ge 4

ENTITLEMENTS="$WORK_DIR/entitlements.plist"
codesign --display --entitlements :- "$EXECUTABLE" >"$ENTITLEMENTS" 2>/dev/null
plutil -lint "$ENTITLEMENTS" >/dev/null
ENTITLEMENTS_JSON="$(plutil -convert json -o - "$ENTITLEMENTS")"
test "$(jq -r '."com.apple.security.device.audio-input"' <<<"$ENTITLEMENTS_JSON")" = "true"
for forbidden_entitlement in \
  com.apple.security.get-task-allow \
  com.apple.security.cs.disable-library-validation \
  com.apple.security.cs.allow-unsigned-executable-memory; do
  if jq -e --arg key "$forbidden_entitlement" 'has($key)' <<<"$ENTITLEMENTS_JSON" >/dev/null; then
    echo "forbidden release entitlement: $forbidden_entitlement" >&2
    exit 1
  fi
done

hdiutil verify "$DMG_PATH"
if DMG_SIGNATURE="$(codesign --display --verbose=4 "$DMG_PATH" 2>&1)"; then
  grep -Fq 'Signature=adhoc' <<<"$DMG_SIGNATURE"
  if grep -q '^Authority=' <<<"$DMG_SIGNATURE"; then
    echo "certificate-backed DMG signature is forbidden for this release" >&2
    exit 1
  fi
fi

if spctl --assess --type execute --verbose=4 "$APP_PATH"; then
  echo "ad-hoc application unexpectedly passed Gatekeeper assessment" >&2
  exit 1
fi

mkdir -p "$MOUNT_DIR" "$INSTALL_DIR"
hdiutil attach -readonly -nobrowse -mountpoint "$MOUNT_DIR" "$DMG_PATH" >/dev/null
MOUNTED=true

MOUNTED_APPS=()
while IFS= read -r -d '' mounted_app; do
  MOUNTED_APPS+=("$mounted_app")
done < <(find "$MOUNT_DIR" -mindepth 1 -maxdepth 1 -type d -name '*.app' -print0)
test "${#MOUNTED_APPS[@]}" -eq 1
test "${MOUNTED_APPS[0]}" = "$MOUNT_DIR/Agentum.app"
test -L "$MOUNT_DIR/Applications"
test "$(readlink "$MOUNT_DIR/Applications")" = "/Applications"

ditto --rsrc --extattr "$MOUNT_DIR/Agentum.app" "$INSTALLED_APP"
INSTALLED_EXECUTABLE="$INSTALLED_APP/Contents/MacOS/agentum-desktop"
test -x "$INSTALLED_EXECUTABLE"
test "$(lipo -archs "$INSTALLED_EXECUTABLE")" = "$EXPECTED_BINARY_ARCH"
codesign --verify --deep --strict --verbose=4 "$INSTALLED_APP"
verify_adhoc_signature "$INSTALLED_APP"

QUARANTINE_VALUE="0081;$(printf '%x' "$(date +%s)");GitHub_Actions;Agentum_Release_Gate"
xattr -w com.apple.quarantine "$QUARANTINE_VALUE" "$INSTALLED_APP"
test -n "$(xattr -p com.apple.quarantine "$INSTALLED_APP")"
if spctl --assess --type execute --verbose=4 "$INSTALLED_APP"; then
  echo "quarantined ad-hoc application unexpectedly passed Gatekeeper assessment" >&2
  exit 1
fi

"$INSTALLED_EXECUTABLE" >"$RUNTIME_LOG" 2>&1 &
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
for _ in 1 2 3 4 5; do
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    break
  fi
  sleep 1
done
if kill -0 "$APP_PID" 2>/dev/null; then
  kill -KILL "$APP_PID"
fi
wait "$APP_PID" 2>/dev/null || true
APP_PID=""

echo "ad-hoc macOS integrity and native runtime checks passed for $TARGET_TRIPLE"
