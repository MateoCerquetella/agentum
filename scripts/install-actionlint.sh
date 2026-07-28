#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || -z "$1" ]]; then
  echo "usage: $0 <install-directory>" >&2
  exit 2
fi

version="1.7.7"
archive="actionlint_${version}_linux_amd64.tar.gz"
archive_sha256="023070a287cd8cccd71515fedc843f1985bf96c436b7effaecce67290e7e0757"
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
  "https://github.com/rhysd/actionlint/releases/download/v${version}/${archive}"

printf '%s  %s\n' "$archive_sha256" "$archive_path" | sha256sum --check --status
tar --extract --gzip --file "$archive_path" --directory "$install_directory" actionlint

version_output="$("$install_directory/actionlint" -version)"
if [[ "${version_output%%$'\n'*}" != "$version" ]]; then
  echo "expected actionlint ${version}, installed ${version_output}" >&2
  exit 1
fi
