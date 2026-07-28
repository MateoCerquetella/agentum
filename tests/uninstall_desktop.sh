#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

test_tmp="$(mktemp -d)"
trap 'rm -rf -- "$test_tmp"' EXIT
export AGENTUM_UNINSTALL_HOME="$test_tmp/test-home"
export XDG_BIN_HOME="$AGENTUM_UNINSTALL_HOME/bin"
export XDG_DATA_HOME="$AGENTUM_UNINSTALL_HOME/data"
export XDG_CONFIG_HOME="$AGENTUM_UNINSTALL_HOME/config"
export XDG_CACHE_HOME="$AGENTUM_UNINSTALL_HOME/cache"
export XDG_STATE_HOME="$AGENTUM_UNINSTALL_HOME/state"
export AGENTUM_INSTALL_SOURCE_ONLY=
export AGENTUM_UNINSTALL_SOURCE_ONLY=1
mkdir -p "$AGENTUM_UNINSTALL_HOME" "$XDG_BIN_HOME"

# The path is rooted by the explicit cd above; ShellCheck is invoked from that
# same repository root in CI.
source scripts/uninstall.sh

configured_product_name="$(python3 -c 'import json; print(json.load(open("crates/agentum-desktop/tauri.conf.json", encoding="utf-8"))["productName"])')"
test "$configured_product_name" = "Agentum"
test "$PACKAGE_NAME" = "$(printf '%s' "$configured_product_name" | tr '[:upper:]' '[:lower:]')"

printf 'desktop\n' > "$INSTALL_DIR/agentum-desktop"
printf 'keep\n' > "$INSTALL_DIR/unrelated"
remove_standalone_linux
test ! -e "$INSTALL_DIR/agentum-desktop"
test -f "$INSTALL_DIR/unrelated"

mkdir -p \
  "$XDG_DATA_HOME/agentum" \
  "$XDG_DATA_HOME/Agentum" \
  "$XDG_CONFIG_HOME/agentum" \
  "$XDG_CACHE_HOME/agentum" \
  "$XDG_STATE_HOME/agentum" \
  "$XDG_DATA_HOME/unrelated"
PURGE_DATA=false
purge_known_data
test -d "$XDG_DATA_HOME/agentum"
PURGE_DATA=true
purge_known_data
test ! -e "$XDG_DATA_HOME/agentum"
test ! -e "$XDG_DATA_HOME/Agentum"
test ! -e "$XDG_CONFIG_HOME/agentum"
test ! -e "$XDG_CACHE_HOME/agentum"
test ! -e "$XDG_STATE_HOME/agentum"
test -d "$XDG_DATA_HOME/unrelated"

outside="$test_tmp/outside"
mkdir "$outside"
printf 'keep\n' > "$outside/important"
ln -s "$outside" "$XDG_DATA_HOME/agentum"
if (purge_known_directory "$XDG_DATA_HOME/agentum") >/dev/null 2>&1; then
  echo "symlinked data directory was purged" >&2
  exit 1
fi
test -f "$outside/important"

if (purge_known_directory "$AGENTUM_UNINSTALL_HOME/not-agentum") >/dev/null 2>&1; then
  echo "unknown purge target was accepted" >&2
  exit 1
fi

mkdir -p "$test_tmp/symlink-outside/agentum"
printf 'keep\n' > "$test_tmp/symlink-outside/agentum/important"
ln -s "$test_tmp/symlink-outside" "$AGENTUM_UNINSTALL_HOME/symlink-parent"
if (purge_known_directory "$AGENTUM_UNINSTALL_HOME/symlink-parent/agentum") >/dev/null 2>&1; then
  echo "symlinked data parent was followed" >&2
  exit 1
fi
test -f "$test_tmp/symlink-outside/agentum/important"

mkdir -p "$test_tmp/traversal-outside/agentum"
printf 'keep\n' > "$test_tmp/traversal-outside/agentum/important"
if (purge_known_directory "$AGENTUM_UNINSTALL_HOME/data/../../traversal-outside/agentum") >/dev/null 2>&1; then
  echo "traversal data target was accepted" >&2
  exit 1
fi
test -f "$test_tmp/traversal-outside/agentum/important"

darwin_directories="$(
  uname() { printf 'Darwin\n'; }
  known_data_directories
)"
for expected in \
  "$AGENTUM_UNINSTALL_HOME/Library/Application Support/agentum" \
  "$AGENTUM_UNINSTALL_HOME/Library/Application Support/Agentum" \
  "$AGENTUM_UNINSTALL_HOME/Library/Application Support/dev.agentum.app" \
  "$AGENTUM_UNINSTALL_HOME/Library/Caches/agentum" \
  "$AGENTUM_UNINSTALL_HOME/Library/Caches/dev.agentum.app"; do
  printf '%s\n' "$darwin_directories" | grep -Fqx "$expected"
done

echo "uninstall desktop tests: PASS"
