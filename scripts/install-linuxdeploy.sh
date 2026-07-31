#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || -z "$1" ]]; then
  echo "usage: $0 <install-directory>" >&2
  exit 2
fi

version="1-alpha-20251107-1"
commit="cc7b864"
binary="linuxdeploy-x86_64.AppImage"
binary_sha256="c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d"
install_directory="$1"
binary_path="${install_directory}/${binary}"
system_name="${RUNNER_OS:-$(uname -s)}"
machine_name="${RUNNER_ARCH:-$(uname -m)}"

case "${system_name}/${machine_name}" in
  Linux/X64 | Linux/x86_64) ;;
  *)
    echo "unsupported linuxdeploy installer platform: ${system_name}/${machine_name}" >&2
    exit 2
    ;;
esac

mkdir -p "$install_directory"
curl \
  --proto '=https' \
  --tlsv1.2 \
  --fail \
  --silent \
  --show-error \
  --location \
  --retry 3 \
  --output "$binary_path" \
  "https://github.com/linuxdeploy/linuxdeploy/releases/download/${version}/${binary}"

printf '%s  %s\n' "$binary_sha256" "$binary_path" | sha256sum --check --status
chmod +x "$binary_path"

version_output="$(APPIMAGE_EXTRACT_AND_RUN=1 "$binary_path" --version 2>&1)"
if [[ "$version_output" != "linuxdeploy version 1-alpha (git commit ID ${commit}),"* ]]; then
  echo "unexpected linuxdeploy build: $version_output" >&2
  exit 1
fi
