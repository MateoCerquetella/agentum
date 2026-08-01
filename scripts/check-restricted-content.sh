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
candidates="$(mktemp)"
printable_strings="$(mktemp)"
matches_file="$(mktemp)"
chmod 600 "$filtered_patterns" "$candidates" "$printable_strings" "$matches_file"
trap 'rm -f -- "$filtered_patterns" "$candidates" "$printable_strings" "$matches_file"' EXIT
while IFS= read -r pattern || [[ -n "$pattern" ]]; do
  [[ -z "$pattern" || "$pattern" == \#* ]] && continue
  printf '%s\n' "$pattern" >> "$filtered_patterns"
done < "$patterns_file"
if [[ ! -s "$filtered_patterns" ]]; then
  echo "restricted pattern file contains no active patterns" >&2
  exit 2
fi

for target in "$@"; do
  if [[ ! -e "$target" || -L "$target" || ( ! -f "$target" && ! -d "$target" ) ]]; then
    echo "restricted-content scan target is missing or unsafe: $target" >&2
    exit 2
  fi
done

for required_command in find grep head rg strings; do
  if ! command -v "$required_command" >/dev/null 2>&1; then
    echo "restricted-content scan requires $required_command" >&2
    exit 2
  fi
done

for target in "$@"; do
  if [[ -f "$target" ]]; then
    printf '%s\0' "$target" >> "$candidates"
    continue
  fi

  scan_root="$target"
  if [[ "$scan_root" != /* ]]; then
    scan_root="./$scan_root"
  fi
  if ! find "$scan_root" \
    \( -name .git -o -name node_modules \) -prune -o \
    -type f -print0 >> "$candidates"; then
    echo "restricted-content scan failed while enumerating $target" >&2
    exit 2
  fi
done

while IFS= read -r -d '' candidate; do
  scan_input="$candidate"
  if ! LC_ALL=C grep -Iq . -- "$candidate"; then
    : > "$printable_strings"
    if ! LC_ALL=C strings -a -n 1 -- "$candidate" > "$printable_strings"; then
      echo "restricted-content scan failed while extracting strings from $candidate" >&2
      exit 2
    fi
    if ! binary_magic="$(LC_ALL=C head -c 2 -- "$candidate")"; then
      echo "restricted-content scan failed while identifying $candidate" >&2
      exit 2
    fi
    if [[ "$binary_magic" == "MZ" ]]; then
      if ! LC_ALL=C strings -a -n 1 -e l -- "$candidate" >> "$printable_strings"; then
        echo "restricted-content scan failed while extracting UTF-16 strings from $candidate" >&2
        exit 2
      fi
    fi
    scan_input="$printable_strings"
  fi

  set +e
  rg --quiet --no-messages --file "$filtered_patterns" -- "$scan_input"
  scan_status=$?
  set -e

  if [[ $scan_status -eq 0 ]]; then
    printf '%s\n' "$candidate" >> "$matches_file"
  elif [[ $scan_status -ne 1 ]]; then
    echo "restricted-content scan failed while scanning $candidate" >&2
    exit "$scan_status"
  fi
done < "$candidates"

if [[ -s "$matches_file" ]]; then
  echo "restricted content found in:" >&2
  cat "$matches_file" >&2
  exit 1
fi
