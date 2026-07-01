# Project Vision — agentum

## What it is

agentum is a **self-hosted control plane for AI coding agents** (Claude Code,
Codex, Gemini, Cursor, …). It boots a local backend that owns a SQLite store of
session metadata, a tmux server where each session is one pane running one agent
CLI, and an HTTP/WS API. Two clients drive that API: the **desktop app** (Tauri —
this repo, `crates/agentum-desktop`) and the **TUI** (`agentum terminal`, in the
separate `agentum-tui` repo). Both embed `agentum-server` in-process on loopback,
so they drive the exact same core.

A *session* is a `(name, workdir, tool, model, flags)` tuple. The backend spawns
the right agent binary into a tmux pane and streams its output to clients.

## North star

The core loop is **run & watch agents, one feature at a time, behind a green
verification gate.** Everything else — Chat/Spec intake, the Kanban board,
worktree isolation, the in-app browser, MCP wiring, memory — exists to support
that loop.

## Who it's for

Engineers who want to drive multiple autonomous coding agents across multiple
projects (local and remote-over-SSH) from one cockpit, with the agents' work
tracked as GitHub issues and verified before it merges.

## How we work

- **Issue-first.** Every change starts as a labeled GitHub issue and lands as a
  PR that closes it. Branch flow: `develop → staging → main`.
- **Spec-driven.** Non-trivial work gets an `ai/specs/<NNN>-<name>/spec.md`
  before implementation (this scaffold).
- **Harness-executed.** Specs become a `.harness/feature_list.json` backlog the
  Harness Engine drives autonomously, advancing only when `verify.sh` (unit gate)
  and `qa.sh` (browser QA gate) are green.

See the repo-root `CLAUDE.md` for the authoritative architecture + workflow guide.
