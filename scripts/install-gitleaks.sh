#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || -z "$1" ]]; then
  echo "usage: $0 <install-directory>" >&2
  exit 2
fi

version="8.30.1"
archive="gitleaks_${version}_linux_x64.tar.gz"
archive_sha256="551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb"
install_directory="$1"
archive_path="${install_directory}/${archive}"

mkdir -p "$install_directory"
curl \
  --proto '=https' \
  --tlsv1.2 \
  --fail \
  --silent \
  --show-error \
  --location \
  --retry 3 \
  --output "$archive_path" \
  "https://github.com/gitleaks/gitleaks/releases/download/v${version}/${archive}"

printf '%s  %s\n' "$archive_sha256" "$archive_path" | sha256sum --check --status
tar --extract --gzip --file "$archive_path" --directory "$install_directory" gitleaks

installed_version="$("$install_directory/gitleaks" version)"
if [[ "$installed_version" != "$version" ]]; then
  echo "expected gitleaks ${version}, installed ${installed_version}" >&2
  exit 1
fi
