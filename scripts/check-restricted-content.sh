#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "usage: $0 /absolute/path/to/restricted-patterns PATH..." >&2
  exit 2
fi

patterns_file="$1"
shift
if [[ "$patterns_file" != /* || ! -f "$patterns_file" || -L "$patterns_file" ]]; then
  echo "restricted patterns must be an absolute regular file" >&2
  exit 2
fi

filtered_patterns="$(mktemp)"
chmod 600 "$filtered_patterns"
trap 'rm -f -- "$filtered_patterns"' EXIT
while IFS= read -r pattern || [[ -n "$pattern" ]]; do
  [[ -z "$pattern" || "$pattern" == \#* ]] && continue
  printf '%s\n' "$pattern" >> "$filtered_patterns"
done < "$patterns_file"
if [[ ! -s "$filtered_patterns" ]]; then
  echo "restricted pattern file contains no active patterns" >&2
  exit 2
fi

for target in "$@"; do
  if [[ ! -e "$target" || -L "$target" ]]; then
    echo "restricted-content scan target is missing or unsafe: $target" >&2
    exit 2
  fi
done

set +e
matches="$(rg --files-with-matches --text --hidden --no-messages \
  --glob '!.git' \
  --glob '!.git/**' \
  --glob '!**/node_modules/**' \
  --glob '!**/target/**' \
  --file "$filtered_patterns" \
  -- "$@" 2>/dev/null)"
scan_status=$?
set -e

if [[ $scan_status -eq 0 ]]; then
  echo "restricted content found in:" >&2
  printf '%s\n' "$matches" >&2
  exit 1
fi
if [[ $scan_status -ne 1 ]]; then
  echo "restricted-content scan failed" >&2
  exit "$scan_status"
fi
