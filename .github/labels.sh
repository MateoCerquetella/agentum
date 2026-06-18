#!/usr/bin/env bash
# Idempotent label sync for agentum. Re-run any time to add/update labels.
# Requires: gh (authenticated). Usage: ./.github/labels.sh
set -euo pipefail

label() { gh label create "$1" --color "$2" --description "$3" --force; }

# --- type/* : what kind of change ---------------------------------------
label "type/feat"     "a2eeef" "New feature or capability"
label "type/fix"      "d73a4a" "Bug fix"
label "type/perf"     "fbca04" "Performance improvement"
label "type/refactor" "c5def5" "Code change that neither fixes a bug nor adds a feature"
label "type/docs"     "0075ca" "Documentation only"
label "type/test"     "bfd4f2" "Adding or fixing tests"
label "type/chore"    "ededed" "Tooling, deps, CI, release plumbing"

# --- area/* : which part of the system (mirrors the crate map) -----------
label "area/desktop"   "5319e7" "Tauri desktop app (crates/agentum-desktop)"
label "area/tui"       "5319e7" "Terminal UI / CLI (crates/agentum-tui)"
label "area/server"    "5319e7" "HTTP+WS API (crates/agentum-server)"
label "area/executor"  "5319e7" "Tool adapters / YOLO translation (crates/agentum-executor)"
label "area/store"     "5319e7" "SQLite persistence (crates/agentum-store)"
label "area/tmux"      "5319e7" "tmux wrapper (crates/agentum-tmux)"
label "area/watchdog"  "5319e7" "Pane watchdog (crates/agentum-watchdog)"
label "area/core"      "5319e7" "Shared types (crates/agentum-core)"
label "area/harness"   "5319e7" "Harness Engine"
label "area/ci"        "5319e7" "CI / release / build"

# --- priority/* ----------------------------------------------------------
label "priority/p0" "b60205" "Critical — drop everything"
label "priority/p1" "d93f0b" "High — next up"
label "priority/p2" "fbca04" "Normal"
label "priority/p3" "0e8a16" "Low / someday"

echo "Labels synced."
