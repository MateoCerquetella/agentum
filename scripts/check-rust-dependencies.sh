#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

if ! cargo audit --version >/dev/null 2>&1; then
  echo "cargo-audit is required (install with cargo install cargo-audit --locked)" >&2
  exit 2
fi
if ! cargo deny --version >/dev/null 2>&1; then
  echo "cargo-deny is required (install with cargo install cargo-deny --locked)" >&2
  exit 2
fi

# cargo-audit scans every lockfile entry, including target-only and disabled
# optional dependencies. Before applying the two reviewed exceptions below,
# prove they have not spread to a different version or dependency path. The
# quick-xml exception expires so an upstream patch cannot be deferred forever.
audit_json="$(mktemp)"
trap 'unlink "$audit_json"' EXIT
cargo audit --json >"$audit_json" || true
python3 - "$audit_json" <<'PY'
import datetime
import json
import re
import subprocess
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    report = json.load(handle)

vulnerabilities = {
    (
        item["advisory"]["id"],
        item["package"]["name"],
        item["package"]["version"],
    )
    for item in report.get("vulnerabilities", {}).get("list", [])
}
reviewed = {
    ("RUSTSEC-2026-0194", "quick-xml", "0.37.5"),
    ("RUSTSEC-2026-0195", "quick-xml", "0.37.5"),
    ("RUSTSEC-2023-0071", "rsa", "0.9.10"),
}
unexpected = vulnerabilities - reviewed
if unexpected:
    for advisory, package, version in sorted(unexpected):
        print(f"unreviewed Rust advisory: {advisory} {package} {version}", file=sys.stderr)
    raise SystemExit(1)

if any(package == "quick-xml" for _, package, _ in vulnerabilities):
    if datetime.date.today() > datetime.date(2026, 10, 26):
        print("quick-xml exception expired; update the Windows notification stack", file=sys.stderr)
        raise SystemExit(1)

    graph = subprocess.run(
        ["cargo", "tree", "--target", "all", "-i", "quick-xml@0.37.5", "--format", "{p}"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    packages = []
    for line in graph.splitlines():
        match = re.search(r"([A-Za-z0-9_-]+) v([0-9][^ (]*)", line)
        if match:
            packages.append(match.groups())
    expected_names = [
        "quick-xml",
        "tauri-winrt-notification",
        "notify-rust",
        "tauri-plugin-notification",
        "agentum-desktop",
    ]
    if [name for name, _ in packages] != expected_names:
        print("quick-xml advisory reached an unreviewed dependency path:", file=sys.stderr)
        print(graph, file=sys.stderr)
        raise SystemExit(1)
    expected_versions = {
        "quick-xml": "0.37.5",
        "tauri-winrt-notification": "0.7.2",
        "notify-rust": "4.18.0",
        "tauri-plugin-notification": "2.3.3",
    }
    if any(expected_versions.get(name, version) != version for name, version in packages):
        print("quick-xml exception dependency versions changed; review required", file=sys.stderr)
        raise SystemExit(1)

if any(package == "rsa" for _, package, _ in vulnerabilities):
    graph = subprocess.run(
        ["cargo", "tree", "--target", "all", "-i", "rsa@0.9.10", "--format", "{p}"],
        capture_output=True,
        text=True,
    )
    if graph.stdout.strip():
        print("vulnerable rsa crate became reachable:", file=sys.stderr)
        print(graph.stdout, file=sys.stderr)
        raise SystemExit(1)
PY

# The only reachable quick-xml use is Windows-only tauri-winrt-notification
# calling quick_xml::escape; neither affected parser API is invoked. RSA is an
# unreachable sqlx-mysql lock candidate while Agentum enables SQLite only.
cargo audit \
  --ignore RUSTSEC-2026-0194 \
  --ignore RUSTSEC-2026-0195 \
  --ignore RUSTSEC-2023-0071 \
  --ignore RUSTSEC-2024-0429
# cargo-audit already prints transitive informational notices. cargo-deny's
# reviewed duplicate warnings can span thousands of graph lines, so keep CI's
# second pass focused on policy failures and the final per-policy verdict.
cargo deny --locked -L error check advisories bans licenses sources
