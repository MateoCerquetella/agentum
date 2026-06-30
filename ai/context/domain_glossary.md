# Domain Glossary — agentum

- **Session** — a `(name, workdir, tool, model, flags)` tuple; one tmux pane
  running one agent CLI. The atom of work.
- **Adapter (`ToolAdapter`)** — per-agent integration (Claude, Codex, Gemini,
  Cursor, …) in `agentum-executor`; owns `launch()`, crash/busy/awaiting-input
  signatures, and the per-tool YOLO flag spelling.
- **YOLO marker** — the canonical `--dangerously-skip-permissions` flag clients
  push to enable permission-skipping; adapters translate it per tool.
- **Worktree** — an isolated `git worktree` per branch/card so concurrent agents
  don't disturb each other's working trees. Created via `POST /api/worktrees/create`.
- **Watchdog** — background loop tailing panes; emits `agent.finished`,
  `agent.awaiting_input`, `session.crashed` on the global event bus.
- **Harness Engine** — verification-gated runner. Reads a project's `.harness/`
  folder and drives agents one feature at a time. Routes under `/api/harness/*`.
- **`.harness/` contract** — `AGENTS.md` (prompt preamble), `feature_list.json`
  (ordered backlog + per-feature state, the source of truth), `init.sh` (smoke
  test), `verify.sh` (unit gate), `qa.sh` (browser QA gate), `handoff.md`.
- **Gate** — `verify.sh` (exit 0 = green = advance) then `qa.sh` (browser QA).
  Both must be green to reach `done`.
- **Feature state** — `pending → coding → verifying → ready_to_test → done`
  (or `blocked`). Written back into `feature_list.json` as the engine runs.
- **Profile / Endpoint** — a named agentum server target (URL + fingerprint) so
  one client can drive multiple backends.
- **Embedded server** — the desktop and TUI each boot `agentum-server` in-process
  on a loopback port (`serve_embedded_loopback`); same core, no separate daemon.
- **MCP tool** — an agentum capability exposed over `POST /mcp` (e.g.
  `agentum_list_sessions`, `agentum_browser`) so any agent gets it
  agent-agnostically over the same transport it uses for Playwright.
