#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || -z "$1" ]]; then
  echo "usage: $0 <install-directory>" >&2
  exit 2
fi

version="15.2.0"
install_directory="$1"
system_name="${RUNNER_OS:-$(uname -s)}"
machine_name="${RUNNER_ARCH:-$(uname -m)}"

case "${system_name}/${machine_name}" in
  Linux/X64 | Linux/x86_64)
    archive="ripgrep-${version}-x86_64-unknown-linux-musl.tar.gz"
    archive_sha256="33e15bcf1624b25cdd2a55813a47a2f95dbe126268203e76aa6a585d1e7b149c"
    binary_name="rg"
    ;;
  macOS/X64 | Darwin/x86_64)
    archive="ripgrep-${version}-x86_64-apple-darwin.tar.gz"
    archive_sha256="af7825fcc69a2afc7a7aea55fc9af90e26421d8f20fe59df32e233c0b8a231c1"
    binary_name="rg"
    ;;
  macOS/ARM64 | Darwin/arm64 | Darwin/aarch64)
    archive="ripgrep-${version}-aarch64-apple-darwin.tar.gz"
    archive_sha256="3750b2e93f37e0c692657da574d7019a101c0084da05a790c83fd335bad973e4"
    binary_name="rg"
    ;;
  Windows/X64 | MINGW*/x86_64 | MSYS*/x86_64 | CYGWIN*/x86_64)
    archive="ripgrep-${version}-x86_64-pc-windows-msvc.zip"
    archive_sha256="71b2fef860abe467217a538ff31de02f5258807c0129f771846f87bd029aafc5"
    binary_name="rg.exe"
    ;;
  *)
    echo "unsupported ripgrep installer platform: ${system_name}/${machine_name}" >&2
    exit 2
    ;;
esac

archive_root="${archive%.tar.gz}"
archive_root="${archive_root%.zip}"
archive_path="${install_directory}/${archive}"
extract_directory="${install_directory}/archive"

mkdir -p "$install_directory" "$extract_directory"
curl \
  --proto '=https' \
  --tlsv1.2 \
  --fail \
  --silent \
  --show-error \
  --location \
  --retry 3 \
  --output "$archive_path" \
  "https://github.com/BurntSushi/ripgrep/releases/download/${version}/${archive}"

if command -v sha256sum >/dev/null 2>&1; then
  printf '%s  %s\n' "$archive_sha256" "$archive_path" | sha256sum --check --status
elif command -v shasum >/dev/null 2>&1; then
  actual_sha256="$(shasum --algorithm 256 "$archive_path")"
  actual_sha256="${actual_sha256%% *}"
  if [[ "$actual_sha256" != "$archive_sha256" ]]; then
    echo "ripgrep archive checksum mismatch" >&2
    exit 1
  fi
else
  echo "sha256sum or shasum is required" >&2
  exit 2
fi

case "$archive" in
  *.tar.gz)
    tar --extract --gzip --file "$archive_path" --directory "$extract_directory"
    ;;
  *.zip)
    unzip -q "$archive_path" -d "$extract_directory"
    ;;
esac

cp "$extract_directory/$archive_root/$binary_name" "$install_directory/$binary_name"
chmod +x "$install_directory/$binary_name"
version_output="$("$install_directory/$binary_name" --version)"
version_line="${version_output%%$'\n'*}"
if [[ "$version_line" != "ripgrep ${version}" && "$version_line" != "ripgrep ${version} "* ]]; then
  echo "expected ripgrep ${version}, installed ${version_output}" >&2
  exit 1
fi
