#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

if ! command -v gitleaks >/dev/null 2>&1; then
  echo "gitleaks 8.30.1 is required" >&2
  exit 2
fi
if ! gitleaks version 2>&1 | grep -Fq '8.30.1'; then
  echo "release secret scan requires exactly gitleaks 8.30.1" >&2
  exit 2
fi

scan_directory="$(mktemp -d)"
trap 'rm -rf -- "$scan_directory"' EXIT
source_archive="$scan_directory/agentum-release-source.tar"

# Archive precisely the tracked working-tree source. This includes staged or
# unstaged content a release owner is validating, excludes .git automatically,
# and gives gitleaks the same archive traversal path used by source packaging.
git ls-files -z --cached --others --exclude-standard \
  | while IFS= read -r -d '' source; do
      if [[ -e "$source" || -L "$source" ]]; then
        printf '%s\0' "$source"
      fi
    done \
  | tar --null --no-recursion --files-from=- -cf "$source_archive"

gitleaks dir \
  --config "$repo_root/.gitleaks.toml" \
  --max-archive-depth 1 \
  --no-banner \
  --redact \
  "$source_archive"
