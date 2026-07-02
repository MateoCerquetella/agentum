# Architecture Principles — agentum

> The invariants. Break these and you reintroduce a bug we already paid for. The
> repo-root `CLAUDE.md` is the long-form version; this is the spec-author's
> checklist.

## Crate boundaries (this repo = desktop + backend)

- `agentum-core` — shared types (Session, Status, Event, transcript types).
- `agentum-store` — SQLite repository (sqlx).
- `agentum-tmux` — thin tmux wrapper (new-session, send-keys, capture-pane, kill).
- `agentum-watchdog` — tails panes, emits `agent.*` lifecycle events.
- `agentum-executor` — `ToolAdapter` trait + per-agent argv; owns YOLO translation.
- `agentum-server` — axum HTTP+WS API. **API-only — no embedded web UI.**
- `agentum-desktop` — Tauri shell (`src/`) + React/Vite UI (`ui/`); embeds
  `agentum-server` in-process on a loopback port.
- The TUI lives in the separate `agentum-tui` repo.

## Non-negotiable invariants

1. **One launch path.** All agent spawns go through
   `routes::sessions::spawn_agent_into_pane` — YOLO translation, loopback
   `pane_env`, the Claude `--settings` hook, and MCP wiring stay centralized.
2. **YOLO marker translation.** Clients always push the canonical Claude marker
   `--dangerously-skip-permissions` into `Session::flags`; each adapter's
   `launch()` translates it via `translate_yolo_marker`. Never push tool-specific
   YOLO spellings from a client (root cause of the v0.6.23 codex crash).
3. **Push-based streaming, never poll.** `/stream` WS feeds raw incremental pane
   bytes from a `tmux pipe-pane` log. Do not reintroduce
   `capture-pane`-every-N-ms full-snapshot polling.
4. **Per-session Claude UUID.** `ClaudeAdapter` pins `--session-id <uuid>` so two
   sessions in one workdir don't share a transcript / cross-pollinate todos.
5. **Adapter, not special-case.** New agents implement `ToolAdapter`; register in
   `adapter_for` + `FIRST_CLASS` / `PASSTHROUGH_PROBED`. (See CLAUDE.md
   "Adding a new agent.")
6. **MCP over skills.** agentum exposes its own capabilities as MCP tools
   (`routes/mcp.rs`) so any agent gets them agent-agnostically; prefer adding an
   MCP tool over a per-agent skill file.

## Execution model (the gate is sacred)

- The **Harness Engine** drives features one at a time. A feature advances only
  when BOTH gates are green: `verify.sh` (unit) then `qa.sh` (browser QA). A red
  gate hands the error back to the agent and retries; it never silently advances.
- Autonomy mechanics that must not regress: YOLO mandatory, the workspace-trust
  dialog auto-accepted, two-step prompt submit. (See CLAUDE.md "Harness Engine".)
- **The GitHub issue is the live status board** for any autonomous run — keep it
  updated on every feature state transition.

## Build rhythm

- Desktop UI: `npm run build --prefix crates/agentum-desktop/ui` (Vite).
- Backend: `cargo build -p agentum-desktop` after Rust changes.
- Tests: `cargo test --workspace --lib` (green on Linux + macOS). Tests that touch
  user paths isolate via `AGENTUM_HOME` (a temp dir), not `XDG_*`.
