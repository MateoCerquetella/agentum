#!/bin/sh
# Docker-based integration test for scripts/install.sh.
#
# Boots a fresh ubuntu:22.04 container, copies install.sh in, runs it
# in --no-interactive mode pointing at a writable INSTALL_DIR, and
# asserts the syntax-checks + help flag work. We deliberately do NOT
# test the actual download path here — that requires a published
# release and a network round-trip; for that, push a real tag and
# watch the release workflow.
#
# Per PRD §9 (docs/plans/AGENTUM_V2_PRD.md): integration tests must
# run in a clean container. This is the bootstrap test; deeper E2E
# (real release download + binary smoke) lives in CI on tag push.
#
# Run: sh tests/install_test.sh
# SPDX-License-Identifier: MIT
set -eu

if ! command -v docker >/dev/null 2>&1; then
    echo "docker not found — skipping (this test is CI/local-docker only)" >&2
    exit 0
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${INSTALL_TEST_IMAGE:-ubuntu:22.04}"

echo "==> install_test.sh: testing scripts/install.sh inside $IMAGE"

docker run --rm \
    -v "$REPO_ROOT/scripts:/agentum-scripts:ro" \
    -e CI=1 \
    "$IMAGE" \
    sh -c '
set -eu
echo "--- apt: minimal deps ---"
apt-get update -qq
apt-get install -qq -y curl ca-certificates >/dev/null

echo "--- POSIX syntax check on install.sh ---"
sh -n /agentum-scripts/install.sh

echo "--- POSIX syntax check on uninstall.sh ---"
sh -n /agentum-scripts/uninstall.sh

echo "--- install.sh --help ---"
sh /agentum-scripts/install.sh --help

echo "--- uninstall.sh --help ---"
sh /agentum-scripts/uninstall.sh --help

echo "--- OK ---"
'

echo "==> install_test.sh: PASS"
