# CLAUDE.md — agentum codebase guide

> Living guide for Claude (and humans) working in this repo. Update it
> when you change architecture, add a crate, move a primitive, or
> introduce a non-obvious gotcha.

agentum is a self-hosted control plane for AI coding agents (Claude
Code, Codex, Gemini, Cursor, …). It boots a local daemon (`agentum
serve`) that owns:

- a SQLite database of session metadata
- a tmux server where each session is one pane running one agent CLI
- an HTTP/WS API that the TUI (`agentum terminal`) and the desktop app
  drive

A "session" is a `(name, workdir, tool, model, flags)` tuple. The
daemon spawns the right binary into a tmux pane and streams its
output to clients.

Two clients consume that API: the **TUI** (`agentum terminal`) boots
`agentum-server` in-process on an ephemeral loopback port — the same
embedded server the desktop uses — so it is self-contained (no separate
`agentum serve` daemon; remote machines are reached as SSH hosts); the
**desktop app** (the Tauri crate
`crates/agentum-desktop/`, with its Rust shell in `src/` and its
React/Vite UI in `ui/`) boots
`agentum-server` *in-process* on a loopback port (see
`agentum_server::serve_embedded_loopback`) so the webview drives the
exact same core. The daemon is API-only — there is no embedded web UI.
The marketing landing page lives in its own private repo (`agentum-www`), deployed separately
(Netlify), not served by the daemon.

---

## Crate map

```
crates/
  agentum-core/        # Shared types: Session, Status, Event, transcript types.
  agentum-store/       # SQLite repository (sqlx). Persists sessions, board, notes, channels, users, auth.
  agentum-tmux/        # Thin wrapper over tmux: new-session, send-keys, capture-pane, kill.
  agentum-watchdog/    # Background loop. Tails panes, emits Event::AgentFinished/AwaitingInput/Crashed.
  agentum-executor/    # ToolAdapter trait + per-agent argv builders. Owns YOLO marker translation.
  agentum-server/      # axum HTTP+WS API + TLS + auth + routes/. API-only (no embedded web UI).
  agentum-tui/         # Package `agentum-tui` (binary `agentum`). The TUI
                       #   (commands/terminal/, boots agentum-server in-process) + scriptable CLI.
  agentum-desktop/     # The desktop app, self-contained:
    src/               #   Tauri 2 Rust shell — embeds agentum-server in-process (loopback) and exposes
                       #   native commands (window, dialogs, clipboard, local PTY) to the webview.
    ui/                #   React + Vite SPA the webview loads; talks to native Tauri commands and,
                       #   increasingly, to the embedded agentum-server over HTTP/WS.
```

Each `crates/<x>/Cargo.toml` declares its deps; the workspace root
`Cargo.toml` pins shared versions.

> Note on PRD §6.3: the v2 PRD's three-crate diagram (`agentum-core`,
> `agentum-cli`, `agentum-desktop`) is conceptual. "agentum-core" in
> PRD parlance = the collection of backend crates above
> (`agentum-{core,store,tmux,watchdog,executor,server}`). The Tauri
> shell (`agentum-desktop`) depends on `agentum-server`, which
> transitively pulls in the rest. We kept the fine-grained split for
> compile-time parallelism and clearer ownership; the binary crate was
> renamed (`agentum` → `agentum-cli` in 2026-05, then both package and
> folder `agentum-cli` → `agentum-tui` in 2026-06 — the binary stays
> `agentum`).

---

## Critical: rebuild rhythm

The daemon is **API-only** — it serves no web UI, so there is no
compile-time asset embed. The two clients build independently:

- **TUI** (Rust): `cargo build --release`, then restart by re-running
  `agentum terminal` (it boots its own embedded server; `pkill agentum`
  first if a previous instance is still holding its loopback port).
  There's no hot reload; rebuild after touching
  `crates/agentum-tui/src/commands/terminal/*.rs`.
- **Desktop UI** (React/Vite): `npm run build --prefix crates/agentum-desktop/ui`
  (or `npm run dev --prefix crates/agentum-desktop/ui` for HMR). The Tauri shell
  loads it; `cargo build` the `agentum-desktop` crate after changing
  its Rust commands or the embedded-server boot in `src/lib.rs`.

The desktop boots `agentum-server` in-process, so a desktop session
and a TUI session connected to a local daemon share one SQLite store.

---

## Adding a new agent (tool adapter)

The pattern: each tool implements `ToolAdapter` in
`crates/agentum-executor/src/adapters.rs`. Five files to touch (more
if it has a UI/dashboard surface):

1. **`crates/agentum-executor/src/adapters.rs`** — define an adapter
   struct + impl. Set `name()`, `launch()`, optional `yolo_flag()`,
   `compact_trigger()`, `crash_signatures()`, `busy_signature()`,
   `awaiting_input_signatures()`.
2. **`crates/agentum-executor/src/lib.rs`** — register in
   `adapter_for(tool)` match, add to `FIRST_CLASS` (or
   `PASSTHROUGH_PROBED` if you only want availability gating without
   a bespoke launch). Add a `binary_for(tool)` arm if the binary
   name disagrees with the tool id (e.g. cursor → cursor-agent).
3. **`crates/agentum/src/commands/terminal/app.rs`** — append to
   `TOOL_SUGGESTIONS` so the TUI Tab-cycle picks it up. If the
   adapter has a YOLO flag, also extend `YOLO_TOOLS`. Extend
   `is_probed_tool()` so the picker gates it.
4. **`crates/agentum/src/cli.rs`** — touch the `--tool` help text
   example string.
5. **`dashboard/src/lib/components/NewSessionDialog.svelte`** — add
   to the `TOOLS` array (`firstClass: true` if the binary should be
   gated; `yoloable: true` if `yolo_flag()` returns `Some`).

Tests live in `adapters.rs`'s `#[cfg(test)] mod tests`. Add at minimum
a "registry routes" assertion + a YOLO-translation test.

---

## YOLO marker translation

The TUI and dashboard both push the canonical Claude marker
`--dangerously-skip-permissions` into `Session::flags` whenever the
user toggles YOLO mode, **regardless of which tool the session
targets**. Each adapter's `launch()` calls
`agentum_executor::translate_yolo_marker(&flags, self.yolo_flag())`,
which substitutes the per-tool flag (or drops the marker entirely
when the adapter doesn't expose one).

Per-tool spellings (canonical):

| Tool     | Flag                                        |
| -------- | ------------------------------------------- |
| claude   | `--dangerously-skip-permissions` (identity) |
| codex    | `--dangerously-bypass-approvals-and-sandbox`|
| cursor   | `--force`                                   |
| gemini   | `--yolo`                                    |
| hermes   | `--yolo`                                    |
| opencode | (unverified — currently `None`)             |
| aider    | (unverified — currently `None`)             |

**Don't push different spellings into `Session::flags` from any
client.** That defeats the translation layer and was the root cause
of the v0.6.23 codex crash.

---

## Agent installation gating

`/api/agents` returns `[{name, binary, available, yolo_flag, path}]`
for every tool in `agentum_executor::probed_tools()`. The dashboard
fetches it on `NewSessionDialog` open and dims unavailable tiles;
the TUI fetches it once at startup of the run-loop (see
`app::run_loop`'s `client.list_agents()` call).

To probe a tool that's NOT first-class but should be gated, add it
to `PASSTHROUGH_PROBED` in `crates/agentum-executor/src/lib.rs` —
no adapter needed.

`terminal` and `bash` deliberately stay un-probed: shells are
universally available and don't need the round trip.

---

## Connection profiles (multi-endpoint)

Users can target multiple agentum servers from one TUI/dashboard
without retyping the URL. Two layers:

### CLI / TUI

- **Storage**: `$XDG_CONFIG_HOME/agentum/profiles.toml`. One
  `default = "name"` pointer plus `[profiles.<name>]` tables with
  `url`, optional `fingerprint`, optional `insecure`.
- **Module**: `crates/agentum/src/commands/terminal/profiles.rs`
  (`Profiles::load/upsert/remove/set_default`).
- **CLI**: `agentum profiles list/add/rm/use` lives in
  `crates/agentum/src/commands/profiles.rs`.
- **TUI flag**: `agentum terminal --profile NAME` resolves to the
  profile's URL+fingerprint before the loopback probe runs.
- **TUI overlay**: `Ctrl-S` opens `Overlay::Profiles`. Pick + Enter
  triggers a *soft restart* of the run-loop:
  `app::RunOutcome::SwitchProfile(name)` bubbles up to
  `commands::terminal::run`, which tears down the alt-screen,
  reconnects via `connect_once`, and re-enters `run_tui_session`.
  See `crates/agentum/src/commands/terminal/mod.rs::run` for the
  loop.
- **Active-profile indicator**: rendered in the title bar
  (`ui::draw_title`) as `· @vps`.

### Dashboard

- **Storage**: `localStorage` keys `agentum_profiles` (JSON array)
  + `agentum_active` (string id). The legacy single-token slot
  `agentum_token` is mirror-written by `setActiveToken` for
  backwards-compat with code paths that still read it directly.
- **Module**: `dashboard/src/lib/profiles.ts`
  (`profiles` writable, `getActiveProfile()`, `apiUrl(path)`,
  `wsUrl(path)`).
- **All HTTP** flows through `apiUrl(path)` in `api.ts`'s
  `request()`. **All WS** flows through `wsUrl(path)`
  (`api.streamUrl`, `api.eventsUrl`, the events bus's
  `eventsUrlForActiveProfile`).
- **UI**: `dashboard/src/lib/components/EndpointSwitcher.svelte` in
  the topbar. Switching reloads the page so every store + WS
  re-initialises against the new origin (cheaper and more reliable
  than per-store invalidation).
- **First-run gate**: when `probeAuth() === 'unreachable'`,
  `TokenGate.svelte` shows an inline "Add endpoint" form instead of
  the login prompt.

### TUI/dashboard parity table

| Feature                        | TUI                       | Dashboard               |
| ------------------------------ | ------------------------- | ----------------------- |
| Profile add/list/remove        | `agentum profiles …` + Ctrl-S overlay | EndpointSwitcher in topbar |
| Active profile indicator       | title bar `· @name`       | chip in topbar          |
| Empty-daemon onboarding        | numbered prompt before alt-screen | inline form on TokenGate's unreachable card |
| Agent installation gating      | "(not installed)" hint on Tool field | tile dimmed + tooltip |
| Switch profile mid-session     | Ctrl-S → soft restart of run-loop | switch chip → page reload |

---

## API routes layer

All HTTP/WS routes live in `crates/agentum-server/src/routes/`:

| File              | Path                       | Notes                          |
| ----------------- | -------------------------- | ------------------------------ |
| `health.rs`       | `/api/health`              | Public; no auth.               |
| `auth.rs`         | `/api/auth/*`              | login/register/me/logout.      |
| `cert.rs`         | `/api/cert/fingerprint`    | Public; for TOFU bootstrap.    |
| `sessions.rs`     | `/api/sessions/*` + `/stream` WS | The fat one. CRUD + start/stop/kill + per-session WS. |
| `events.rs`       | `/api/events` WS           | Global broadcast bus.          |
| `agents.rs`       | `/api/agents`              | Probes which tool binaries are on PATH. |
| `agent_tasks.rs`  | `/api/sessions/{id}/agent-tasks` | Plan/todos/tasks tail. |
| `host.rs`         | `/api/host/metrics`        | CPU+RAM samples; also broadcasts. |
| `fs.rs`           | `/api/fs/list`             | Workdir picker. |
| `mcp.rs`          | `/mcp`                     | agentum's own MCP server (see below). |
| `harness.rs`      | `/api/harness/*` + `/events` WS | Harness Engine: drive agents one feature at a time behind a verify gate (see below). |
| `board.rs`, `notes.rs`, `channels.rs`, `watchdog.rs`, `doctor.rs` | various | Self-explanatory. |

Auth middleware (`crate::auth::require_token`) is applied at the
top-level router merge — see `lib.rs::router`. Public paths are
listed in `auth.rs::is_public`. WS clients pass the bearer token as
`?token=` because browsers can't set headers on upgrade.

---

## agentum as an MCP server (skills → MCP)

agentum exposes its own capabilities as **MCP tools** so *any* agent
(Claude, Codex, …) gets them over the same streamable-HTTP transport it
already uses for Playwright — agent-agnostic, app-owned, no per-agent
skill files. This supersedes the old "install a skill into
`~/.claude/skills`" model.

- **Server** (`routes/mcp.rs`): a hand-rolled streamable-HTTP JSON-RPC
  server at `POST /mcp` — `initialize` / `ping` / `tools/list` /
  `tools/call`, stateless, single `application/json` responses (no SSE;
  `GET /mcp` → 405). Each tool is a thin view over an existing
  route/store helper, never a reimplementation. Tools so far:
  `agentum_list_sessions`, `agentum_list_worktrees`,
  `agentum_send_message`, `agentum_check_messages` (the `orchestration`
  mailbox). Add a tool by appending to `tool_specs()` + a `call_tool`
  arm.
- **Auth**: `/mcp` is **not** public — it requires the bearer token on a
  networked daemon. It's reachable on the embedded loopback server
  because that runs `no_auth` (loopback-bound). For an authed standalone
  daemon, the launch wiring must inject the token as an `Authorization`
  header (TODO).
- **Auto-wiring** (`mcp_provision.rs`): every *local* Claude/Codex launch
  is wired to the agentum MCP **by default** (it's free — the running
  server), plus Playwright when `AGENTUM_BROWSER_VERIFY` is set. The
  generalized seam (`agentum_executor::McpProvision` holds
  `Vec<McpServer>`) writes ONE combined `--mcp-config` file (Claude) or a
  `-c` block per server (Codex). The URL comes from `state.api_base_url`
  (correct for the embedded TUI/desktop server; a standalone daemon on a
  non-default port falls back to `:8822`, same gap as `pane_env`).
- **Skill removal is a separate, desktop-UI effort**: the skill-install
  surface lives entirely in `agentum-desktop/ui/src` (TS/React —
  `SkillsPage.tsx`, `tauri/skills.ts`, `agent-feature-install-commands.ts`,
  …) with NO Rust/compile coupling to the repo `skills/` dir. Removing it
  needs the npm build env to verify; do it after the remaining skill
  capabilities (computer-use, scheduling, browser, orchestration DAG) are
  ported to MCP tools, else those capabilities are lost.

---

## Harness Engine (`/api/harness/*`)

A **verification-gated** agent runner. Point it at a project dir that
contains a `.harness/` folder and it drives real agents one feature at a
time, blocking advancement on a red gate.

- **`.harness/` contract** (all under the project root):
  - `AGENTS.md` — instructions prepended to every feature prompt.
  - `feature_list.json` — the ordered backlog + per-feature `state`
    (`pending`/`coding`/`verifying`/`done`/`blocked`), plus run knobs
    (`agent_tool`, `agent_model`, `max_retries`, `settle_*`). The engine
    **writes state back here** as it runs — it is the single source of truth.
  - `init.sh` — environment smoke-test, run once; non-zero aborts the run.
  - `verify.sh` — **the gate**, run after each feature with
    `$HARNESS_FEATURE_ID` set. exit 0 = green (advance + write handoff),
    non-zero = red (block + retry). Falls back to `npm run verify`, then
    to a pass if neither exists.
  - `handoff.md` — overwritten after each green gate.
- **Engine** (`harness.rs`): `HarnessEngine` holds in-memory runs + a
  `broadcast` event bus; the state machine (load/verify/mark-done/block)
  is decoupled from spawning so it's unit-testable with stub `verify.sh`.
  [`harness::drive`] is the live loop: init → for each pending feature
  {spawn agent → wait-for-settle → verify gate → advance / retry / block}.
- **Real agents, one launch path**: `spawn_feature_agent` goes through
  `routes::sessions::spawn_agent_into_pane` — the *same* helper the `start`
  route uses (extracted in this work) — so YOLO translation, loopback
  `pane_env`, the Claude `--settings` hook, and MCP wiring stay centralized.
  Settle detection subscribes to the session lifecycle bus and waits for the
  first `agent.awaiting_input`/`agent.finished` after a grace window (an early
  settle inside the grace window is remembered, not discarded — otherwise a
  fast feature would hang until `settle_timeout_secs`).
- **Autonomy mechanics (hard-won, don't regress)**: an autonomous run can only
  work if the agent never blocks on a human. Three non-obvious pieces make that
  true, all in `harness.rs`:
  1. **YOLO is mandatory** — `spawn_feature_agent` pushes
     `agentum_executor::YOLO_MARKER` into the session flags (`agent_yolo`,
     default true). Without it the agent stops at the first permission prompt
     and never reaches the gate.
  2. **Workspace-trust dialog** — Claude shows "Do you trust this folder?" on a
     fresh workdir and `--dangerously-skip-permissions` does **not** skip it
     (only non-interactive `-p` does). `await_repl_ready` watches the pane,
     accepts the dialog (Enter on the default "Yes"), and waits for the idle
     REPL footer — also outlasting an MCP-slowed boot (a fixed sleep is too
     fragile for both).
  3. **Prompt submit is two-step** — `inject_prompt` types the prompt with NO
     trailing Enter, pauses (`SUBMIT_DELAY`), then sends a bare Enter. A single
     combined `send-keys "<text>" Enter` is swallowed by the REPL's
     bracketed-paste handling for a multi-line prompt: the text lands in the
     input box (often collapsed to a "[Pasted text]" block) but never executes.
  A `#[ignore]` live test (`tests/harness_live_agent.rs`) drives a **real**
  Claude agent end-to-end against `examples/harness-demo/` and asserts the gate
  goes green; run it with
  `AGENTUM_BROWSER_VERIFY=1 cargo test -p agentum-server --test harness_live_agent -- --ignored --nocapture`.
- **Routes** (`routes/harness.rs`): `POST /api/harness` (register),
  `GET` (list/status), `POST /{id}/run` (kick off `drive` as a bg task,
  rejects double-run via `claim_driver`), `POST /{id}/init`,
  `POST /{id}/verify` (manual one-shot gate), `GET /{id}/files`,
  `DELETE /{id}`, and `WS /api/harness/events` (live `HarnessEvent` stream).
- **Desktop UI**: `agentum-desktop/ui/src/components/harness/HarnessEngine.tsx`
  (sidebar **Harness** entry; `activeView === 'harness'`) — feature board,
  an unmistakable verification-gate banner, the `.harness/` file viewer, and
  a live event log, all fed by `runtime/harness-client.ts` over the embedded
  loopback server. A runnable example lives in `examples/harness-demo/`.

---

## Common gotchas

- **rust-embed compile-time**: see "Critical: rebuild rhythm" above.
- **YOLO marker**: never push tool-specific YOLO flag spellings from
  the TUI/dashboard. Always push the Claude marker; let the adapter
  translate.
- **Claude session UUID**: `ClaudeAdapter::launch()` pins
  `--session-id <agentum_uuid>` so transcripts land in a unique file
  per session. Without this, two sessions in the same workdir share
  one transcript and the agent-tasks panel cross-pollinates todos.
- **Capabilities probe**: pre-v0.6.7 daemons don't return
  `capabilities` from `/api/health`. The TUI client treats absence
  as "no optional features supported".
- **Profile token migration**: the legacy `agentum_token`
  localStorage key is read on first load and migrated into a `local`
  profile. Newly added profiles get fresh tokens earned at login.
- **Cargo.lock drift**: `Cargo.lock` gets updated whenever a dep
  changes. Commit it; we ship binaries from CI and reproducibility
  matters.
- **Session streaming is push-based, never poll**: both local and
  remote `/stream` WS feed the client raw incremental pane bytes from
  a `tmux pipe-pane` log (RIS + one `capture-pane` snapshot on connect,
  then live deltas). Local tails the on-disk log; remote (SSH) tails a
  per-session log under `$HOME/.agentum/panes/<uuid>.log` over one
  persistent `ssh tail -f` channel (`host_runtime::spawn_remote_pane_tail`).
  Do **not** reintroduce the old `capture-pane`-every-N-ms full-snapshot
  poll for remote — it lagged ~700 ms and flickered (full-screen RIS
  repaint each tick). `pipe_pane` is armed at session start and re-armed
  idempotently (`-o`) on each connect.

---

## Conventions

- **Comments**: write *why*, not *what*. Add a short comment when
  the line encodes a decision, an invariant, or a workaround. Don't
  paraphrase the code.
- **Tests**: `cargo test --workspace --lib` covers everything and is
  green on Linux and macOS. Tests that touch user paths
  (profiles/board_goals/planner) isolate via `AGENTUM_HOME` (a temp
  dir) rather than `XDG_*`, which `directories` ignores on macOS.
- **Frontend build**: `npm run build --prefix crates/agentum-desktop/ui` (Vite). The
  server-facing TS runtime clients are plain (no `@/` aliases), so they
  can also be typechecked directly with `tsc`.
- **Clippy / fmt**: workspace runs cargo fmt; please match
  surrounding style.

---

## Quick reference

```sh
# Build everything
cargo build --release
npm run build --prefix dashboard
cargo build --release   # rebake the embedded SPA

# Run the TUI (boots its own embedded server in-process)
agentum terminal

# Run with mute
AGENTUM_TUI_NO_SOUND=1 agentum terminal

# Tests
cargo test -p agentum-executor -p agentum-server -p agentum --lib
npm run check --prefix dashboard
```

