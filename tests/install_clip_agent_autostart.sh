#!/bin/sh
# Pin the gate logic of `scripts/install.sh::install_clip_agent_autostart`:
# - escape hatch via AGENTUM_INSTALL_NO_CLIP_AGENT
# - non-interactive (INTERACTIVE=false) skip
# - dry-run prints "would run: ..." instead of invoking the real subcommand
#
# Sources install.sh with `AGENTUM_INSTALL_SOURCE_ONLY=1` so the script
# defines every function but does NOT run banner/detect_platform/download.
#
# SPDX-License-Identifier: MIT
set -eu

# Syntax check first — catches typos before we even source.
bash -n scripts/install.sh

# Test 1: AGENTUM_INSTALL_NO_CLIP_AGENT skips the hook (escape hatch).
out=$(
    AGENTUM_INSTALL_SOURCE_ONLY=1 . scripts/install.sh
    INTERACTIVE=true INSTALL_DIR=/tmp AGENTUM_INSTALL_NO_CLIP_AGENT=1 \
        install_clip_agent_autostart
)
echo "$out" | grep -q "skipped" || {
    echo "FAIL: expected 'skipped' on AGENTUM_INSTALL_NO_CLIP_AGENT"
    exit 1
}

# Test 2: non-interactive (INTERACTIVE=false) skips the hook.
out=$(
    AGENTUM_INSTALL_SOURCE_ONLY=1 . scripts/install.sh
    INTERACTIVE=false INSTALL_DIR=/tmp install_clip_agent_autostart
)
echo "$out" | grep -q "skipped" || {
    echo "FAIL: expected 'skipped' when INTERACTIVE=false"
    exit 1
}

# Test 3: interactive + dry-run emits the "would run" line.
out=$(
    AGENTUM_INSTALL_SOURCE_ONLY=1 . scripts/install.sh
    # Fake an installed binary so the [ -x ] check passes.
    mkdir -p /tmp/agentum-install-test
    : >/tmp/agentum-install-test/agentum
    chmod +x /tmp/agentum-install-test/agentum
    INTERACTIVE=true INSTALL_DIR=/tmp/agentum-install-test \
        AGENTUM_INSTALL_DRY_RUN=1 install_clip_agent_autostart
    rm -rf /tmp/agentum-install-test
)
echo "$out" | grep -q "would run: .*/agentum clip-agent --install" || {
    echo "FAIL: expected 'would run: .../agentum clip-agent --install', got: $out"
    exit 1
}

echo "OK"
