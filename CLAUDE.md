# CLAUDE.md — agentum codebase guide

> Living guide for Claude (and humans) working in this repo. Update it
> when you change architecture, add a crate, move a primitive, or
> introduce a non-obvious gotcha.

> **Repo split (2026-06):** the CLI/TUI (package `agentum-tui`, binary
> `agentum`) was extracted into its own repository,
> [`github.com/mateocerquetella/agentum-tui`](https://github.com/mateocerquetella/agentum-tui).
> **This** repo is now the **desktop app** (`crates/agentum-desktop`)
> plus the shared backend crates it depends on (agentum-core, -store,
> -tmux, -watchdog, -executor, -server). The TUI repo depends on the
> same backend crates (currently by its own copy). Anything below that
> describes building or running the `agentum` CLI/TUI now happens in
> that separate repo — references are kept here for context.

agentum is a self-hosted control plane for AI coding agents (Claude
Code, Codex, Gemini, Cursor, …). It boots a local daemon (`agentum
serve`) that owns:

- a SQLite database of session metadata
- a tmux server where each session is one pane running one agent CLI
- an HTTP/WS API that the TUI (`agentum terminal`, separate repo) and
  the desktop app drive

A "session" is a `(name, workdir, tool, model, flags)` tuple. The
daemon spawns the right binary into a tmux pane and streams its
output to clients.

Two clients consume that API. The **TUI** (`agentum terminal`, now in
the separate `agentum-tui` repo) boots `agentum-server` in-process on
an ephemeral loopback port — the same embedded server the desktop uses
— so it is self-contained (no separate `agentum serve` daemon; remote
machines are reached as SSH hosts). The **desktop app** — the only
client that lives in **this** repo (the Tauri crate
`crates/agentum-desktop/`, with its Rust shell in `src/` and its
React/Vite UI in `ui/`) boots
`agentum-server` *in-process* on a loopback port (see
`agentum_server::serve_embedded_loopback`) so the webview drives the
exact same core. The daemon is API-only — there is no embedded web UI.
The marketing landing page lives in its own private repo (`agentum-www`), deployed separately
(Netlify), not served by the daemon.

---

## Contribution workflow (issue-first, always)

**Every change starts as a GitHub issue and lands as a PR that closes it.**
No "drive-by" commits to a feature branch without a tracked issue — the issue
is where the documentation and labels live.

1. **Open a documented issue first.** Use the templates in
   `.github/ISSUE_TEMPLATE/` (Summary, Motivation, Proposed approach,
   Acceptance criteria). Label it with `type/*` + `area/*` + `priority/*`
   (run `.github/labels.sh` once to sync the label set).
**Branch model:** `develop` (feature integration) → `staging` (QA) → `main`
(release, default branch). Feature work bases on `develop`; promotions move it
downstream toward `main`.

2. **Always work in a dedicated git worktree** — never `git checkout` a new
   branch in the shared checkout (many agents run concurrently here; in-place
   checkout disturbs their working trees). Base off `develop`:
   `git worktree add ../agentum-<kebab-desc> -b <type>/<kebab-desc> origin/develop`.
   Clean up with `git worktree remove <path>` after the PR merges.
3. **Implement + verify** (see "Critical: rebuild rhythm").
4. **Open a PR into `develop`** (`gh pr create --base develop`) with
   `Closes #<issue>` in the body **and** the commit message. Because `develop`
   isn't the default branch, this does **not** close the issue on merge — that's
   intentional (see step 5). The `.github/pull_request_template.md` enforces the link.
5. **Promote develop → staging (QA) → main (release).** A merge to `develop` is
   integration, not "done". Promote `develop` → `staging` to deploy to the staging
   environment; the ticket enters **QA** — label the issue `status/qa`, keep it
   open. When QA passes → relabel `status/qa-pass`, then release: promote
   `staging` → `main` and tag `vX.Y.Z` (the repo's release convention); the
   `Closes #<issue>` fires when the commit reaches `main` (the default branch),
   closing the issue. When QA fails → `status/qa-fail` + findings, loop back to
   step 2. Never close at the develop or staging merge.

Claude can drive the whole flow with the **`/ship <description>`** slash command
(`.claude/commands/ship.md`): it creates the labeled issue, branches,
implements, and opens the linked PR — in that order, without skipping the issue.

**Autonomous (Harness Engine) runs update the issue too.** When the harness
drives features autonomously, the linked GitHub issue is the live status board —
keep it current (see the Harness Engine section for the exact rule).

---

## Crate map

```
crates/
  agentum-core/        # Shared types: Session, Status, Event, transcript types.
  agentum-store/       # SQLite repository (sqlx). Persists sessions, board, notes, channels, users, auth.
  agentum-tmux/        # Thin wrapper over tmux: new-session, send-keys, capture-pane, kill.
  agentum-watchdog/    # Three bus-driven background workers: the per-session Watchdog (tails panes,
                       #   emits Event::AgentFinished/AwaitingInput/Crashed; in lib.rs), the goal-status
                       #   reconciler (reconciler.rs), and the watchdog→board-comment bridge (comment_bridge.rs).
  agentum-executor/    # ToolAdapter trait + per-agent argv builders. Owns YOLO marker translation.
  agentum-server/      # axum HTTP+WS API + TLS + auth + routes/. API-only (no embedded web UI).
  # agentum-tui/       # MOVED OUT (2026-06) → github.com/mateocerquetella/agentum-tui.
  #                    #   Package `agentum-tui` (binary `agentum`): the TUI
  #                    #   (commands/terminal/, boots agentum-server in-process) + scriptable CLI.
  #                    #   No longer in this workspace; the new repo depends on the backend crates above.
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

- **TUI** (Rust): builds in the **separate `agentum-tui` repo** now —
  `cargo build --release` there, then restart by re-running `agentum
  terminal` (it boots its own embedded server; `pkill agentum` first if
  a previous instance is still holding its loopback port). There's no
  hot reload; rebuild after touching that repo's
  `src/commands/terminal/*.rs`. Not built from this repo.
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
3. **TUI tool list** (in the separate `agentum-tui` repo —
   `src/commands/terminal/app.rs`) — append to `TOOL_SUGGESTIONS` so
   the TUI Tab-cycle picks it up. If the adapter has a YOLO flag, also
   extend `YOLO_TOOLS`. Extend `is_probed_tool()` so the picker gates
   it.
4. **TUI CLI help** (in the separate `agentum-tui` repo —
   `src/cli.rs`) — touch the `--tool` help text example string.
5. **`crates/agentum-desktop/ui/src`** — add the tool to the desktop
   UI's tool list (the React new-session surface) so it shows in the
   picker (`firstClass: true` if the binary should be gated;
   `yoloable: true` if `yolo_flag()` returns `Some`).

Steps 1–2 (the executor adapter) live in this repo; steps 3–4 (TUI)
live in the `agentum-tui` repo; step 5 (desktop UI) lives here.

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

> These paths live in the separate `agentum-tui` repo now
> (`github.com/mateocerquetella/agentum-tui`); the `crates/agentum/…`
> prefixes below are relative to that repo's crate, not this one.

- **Storage**: `$XDG_CONFIG_HOME/agentum/profiles.toml`. One
  `default = "name"` pointer plus `[profiles.<name>]` tables with
  `url`, optional `fingerprint`, optional `insecure`.
- **Module**: `src/commands/terminal/profiles.rs`
  (`Profiles::load/upsert/remove/set_default`).
- **CLI**: `agentum profiles list/add/rm/use` lives in
  `src/commands/profiles.rs`.
- **TUI flag**: `agentum terminal --profile NAME` resolves to the
  profile's URL+fingerprint before the loopback probe runs.
- **TUI overlay**: `Ctrl-S` opens `Overlay::Profiles`. Pick + Enter
  triggers a *soft restart* of the run-loop:
  `app::RunOutcome::SwitchProfile(name)` bubbles up to
  `commands::terminal::run`, which tears down the alt-screen,
  reconnects via `connect_once`, and re-enters `run_tui_session`.
  See `src/commands/terminal/mod.rs::run` for the loop.
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
| `sessions.rs`     | `/api/sessions/*` + `/stream` WS | CRUD + start/stop/kill + per-session WS. WS pane-streaming machinery is in `sessions/streaming.rs`; pane-env/MCP provisioning + the shared `spawn_agent_into_pane` launch path in `sessions/provision.rs`. |
| `events.rs`       | `/api/events` WS           | Global broadcast bus.          |
| `agents.rs`       | `/api/agents`              | Probes which tool binaries are on PATH. |
| `agent_tasks.rs`  | `/api/sessions/{id}/agent-tasks` | Plan/todos/tasks tail. |
| `host.rs`         | `/api/host/metrics`        | CPU+RAM samples; also broadcasts. |
| `fs.rs`           | `/api/fs/list`             | Workdir picker. |
| `mcp.rs`          | `/mcp`                     | agentum's own MCP server (see below). |
| `harness.rs`      | `/api/harness/*` + `/events` WS | Harness Engine: drive agents one feature at a time behind a verify gate (see below). |
| `sdd.rs`          | `/api/sdd/playbooks`, `/api/sessions/{id}/sdd/*` | Server-owned SDD playbooks: list, button inject, per-session SDD loop (see "SDD playbooks" below). |
| `git.rs`          | `/api/sessions/{id}/git/*` | Per-session git surface. Decomposed by domain into `git/` submodules — `history_routes`, `compare_routes` (commit/branch compare), `conflict_routes` (rebase/abort/discard/upstream), `sync_routes` (branches/log/fetch/pull/push), `write_routes` (stage/commit mutations), `content_routes` (diff/file), `file_links_routes` (remote URL/blob). The root keeps the router, shared git-exec/path-safety plumbing (`run_git`, `host_and_cwd_for`, `ensure_safe_relative`), and the shared status core (`parse_porcelain_z`/`GitStatus`) that `write_routes` reuses; submodules reach it via `use super::*`. |
| `board.rs`, `notes.rs`, `channels.rs`, `watchdog.rs`, `doctor.rs` | various | Self-explanatory. |

Shared route helpers live in `routes/util.rs` (`pub(crate)`): `parse_uuid`,
`expand_workdir`, `now_millis` — import these rather than re-defining them
per route module.

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
  `tools/call` / `prompts/list` / `prompts/get`, stateless, single
  `application/json` responses (no SSE;
  `GET /mcp` → 405). Each tool is a thin view over an existing
  route/store helper, never a reimplementation. Tools so far:
  `agentum_list_sessions`, `agentum_list_worktrees`,
  `agentum_send_message`, `agentum_check_messages` (the `orchestration`
  mailbox). Add a tool by appending to `tool_specs()` + a `call_tool`
  arm. The `prompts/*` surface serves the SDD playbooks (below) as native
  slash commands for clients that render MCP prompts (Claude Code,
  Gemini CLI).
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

## SDD playbooks (server-owned `/sdd-*`) — issue #313

The SDD workflow commands (`sdd-spec`, `sdd-spec-socratic`, `sdd-orchestrate`,
`sdd-status`, `sdd-handoff`, `sdd-init`) are **owned by agentum-server**, not
installed per-agent. They used to be untracked `.claude/commands/*.md` files
(gitignored → a fresh install never got them, and Claude-only); now the
canonical bodies live in `crates/agentum-server/src/sdd_playbooks/*.md`,
embedded via `include_str!` (registry: `src/sdd.rs`). A per-user override at
`~/.agentum/commands/<name>.md` (`$AGENTUM_HOME/commands` in tests) wins over
the embedded copy. **Edit the playbooks there, not in `~/.claude`.**

One registry, three delivery paths (all in-repo consumers read `crate::sdd`):

- **MCP** (`routes/mcp.rs`): the `agentum_sdd` tool (list/fetch — works in
  every MCP client) + `prompts/list`/`prompts/get` (native `/sdd-*` slash
  commands where the client renders MCP prompts). Any agentum-launched agent
  is MCP-wired (see auto-wiring above), so every agent on every install gets
  the same procedures.
- **SDD bar** (`routes/sdd.rs` + `agentum-desktop/ui/src/components/sdd/SddBar.tsx`):
  pill buttons (Spec / Spec Socratic / Continue / Status) under the active
  agent tab's terminal. A click previews the playbook, then
  `POST /api/sessions/{id}/sdd/inject` types a short **bootstrap line** into
  the pane ("call `agentum_sdd` and follow it") via the harness's two-step
  `inject_prompt`; tools with no MCP wiring (bash/aider/…) automatically get
  the **full playbook text** instead (`mode: full`).
- **SDD loop** (`routes/sdd.rs`): `POST /api/sessions/{id}/sdd/loop` toggles a
  per-session worker that re-injects `sdd-orchestrate` (autonomous mode) each
  time the agent settles (`agent.awaiting_input`/`agent.finished`, reusing
  `harness::wait_for_settle`), capped at `max_steps` (default 10). The state
  is **server-owned** (`AppState::sdd_loops`) and broadcast as
  `sdd.loop.started/step/stopped` on `/api/events` — the UI's rainbow Loop
  toggle renders whatever the server says, so it survives reloads and shows
  loops started by anyone. The worker stops on toggle-off, session
  stop/kill/crash, settle timeout, or step cap — every exit reason lands in
  the `sdd.loop.stopped` payload.

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
- **Engine** (`harness.rs` is now a module dir: `harness.rs` keeps `HarnessEngine`
  + the tests; the `.agentum-harness/` data types live in `harness/types.rs`,
  prompt/verdict helpers in `harness/helpers.rs`, and the drive loop +
  orchestration in `harness/drive.rs` — all re-exported so `harness::Foo` /
  `harness::drive` are unchanged): `HarnessEngine` holds in-memory runs + a
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
  true, in `harness/drive.rs` (the spawn + REPL-interaction path):
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
- **Issue is the status board (always)**: an autonomous run is tracked by a
  GitHub issue (the epic/feature), and the engine **keeps that issue updated** as
  it drives — this is non-negotiable for autonomous work, since no human is
  watching the pane. On each feature state transition, post/append to the issue:
  `coding` → "▶ starting <feature>", `verifying` → "🧪 gate running",
  `done` → "✅ <feature> green" (and check off the matching acceptance-criteria
  box in the issue body), `blocked` → "⛔ <feature> red after N retries" (apply
  the `priority/*` bump + a `blocked` note). When the final gate is green, close
  the issue (or let the PR's `Closes #N` do it) with a comment linking the run +
  `handoff.md`. This mirrors the chat→GitHub→harness pipeline (Spec 011, GH #19).
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

### SDD → Linear → QA pipeline (spec 012)

The full automated loop a user runs is: SDD intake (Chat page) → ticket created
in the tracker (Todo) → agent codes it (In Progress) → unit gate green (Ready to
Test) → browser QA gate green → ticket Done. The pieces:

- **Two-phase gate** in `harness::drive_inner`: the unit-test gate (`verify.sh`,
  existing) then the **browser QA gate** (`qa.sh`, new). BOTH must be green to
  advance; a red gate at either phase hands the error back to the agent and
  retries (shared `handle_gate_failure`). A missing `qa.sh` is a pass so non-web
  projects aren't blocked. `scaffold_harness` writes a `qa.sh` template that
  shows how to drive the `browser-verification-loop` skill for a web surface.
- **QA gate as a spawned agent (012b)**: `FeatureList.qa_mode` (`auto`/`script`/
  `agent`, default `auto`) picks how the QA gate runs. `agent` (or `auto` when no
  `qa.sh` and `AGENTUM_BROWSER_VERIFY` is set) makes `drive_inner` call
  `run_qa_agent_gate`, which spawns a browser-verification-loop agent
  (`spawn_qa_agent`) that writes a verdict file `.agentum-harness/qa/<id>.json`
  (`{"passed":bool,"summary":...}`); the harness reads it after the agent settles.
  A missing/garbled verdict FAILS the gate (inconclusive never advances to Done).
  `qa_agent_tool` overrides the QA CLI (default = the feature agent).
- **New feature state** `FeatureState::ReadyToTest` (between `Verifying` and
  `Done`) — set by `run_qa_once`; the in-app board has a "Ready to Test" column.
- **Tracker transitions** (`task_sink::apply_tracker_transition`,
  `TrackerPhase`): lifecycle events drive the ticket's state — Coding→InProgress,
  unit-green→ReadyToTest, QA-green→Done; planning sets Todo. Linear uses
  workflow-state transitions (`linear::transition_issue` + `LinearStateMap`,
  resolved by name); the internal Board moves card `status`
  (todo/doing/review/done); GitHub is a logged no-op for now. **Best-effort by
  contract**: a tracker hiccup is logged (`HarnessEvent::Log`), never halts the
  run. Each `Feature` carries `tracker_provider`/`tracker_url`, threaded from the
  goal's task sink in `routes::board_goals::plan_goal_harness`.
- **Linear state names** are configurable: `LinearStateMap` defaults to
  Todo / In Progress / Ready to Test / Done, overridable via the `linear.json`
  `state_map` (written by Settings) and `AGENTUM_LINEAR_STATE_*` env (highest
  precedence). A missing target state is a logged skip, not an error.

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
- **Boot revival vs watchdog ordering**: an OS reboot kills the local
  tmux server while the store still says `running`. At boot,
  `routes::sessions::boot_revive_dead_sessions` respawns those local
  panes through `spawn_agent_into_pane` (Claude resumes its
  conversation — the adapter swaps `--session-id` for `--resume` when
  the transcript exists). It runs on the SAME tokio task as the
  watchdog, strictly before it (`lib.rs::spawn_background_workers`) —
  start the watchdog earlier and it marks the not-yet-revived rows
  crashed. Non-resumable tools (codex/cursor/gemini) are deliberately
  NOT revived: a silently-fresh instance would hide the context loss.
- **Session streaming is push-based, never poll**: both local and
  remote `/stream` WS feed the client raw incremental pane bytes from
  a `tmux pipe-pane` log (RIS + one `capture-pane` snapshot on connect,
  then live deltas). Local tails the on-disk log; remote (SSH) tails a
  per-session log under `$HOME/.agentum/panes/<uuid>.log` over one
  persistent `ssh tail -f` channel (`host_runtime::spawn_remote_pane_tail`).
  Do **not** reintroduce the old `capture-pane`-every-N-ms full-snapshot
  poll for remote — it lagged ~700 ms and flickered (full-screen RIS
  repaint each tick). `pipe_pane` is armed at session start and re-armed
  on each connect.
- **`tmux pipe-pane -o` TOGGLES — it is NOT an idempotent arm** (issue
  #270). tmux always closes an existing pipe first; `-o` merely skips
  opening the replacement — so calling it against a live pipe DISARMS
  the stream. That blind re-arm blanked every new agent session at
  first connect and disarmed healthy sessions on app restart (the
  `pane_repair` sweep). Every arm path must probe `#{pane_pipe}` first
  and skip when live (`agentum_tmux::pipe_pane`, `remote_pipe_script`,
  `snapshot_with_offset_script` all do); when actually arming, use
  plain `pipe-pane` without `-o` so a lost race still ends armed.
  Never call `tmux pipe-pane -o` directly.

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
# Build the desktop app (this repo)
npm run build --prefix crates/agentum-desktop/ui
cargo build --release -p agentum-desktop

# Run the desktop app
cargo run -p agentum-desktop

# The TUI (`agentum terminal`) lives in the separate agentum-tui repo
# (github.com/mateocerquetella/agentum-tui); build/run it there.

# Tests (backend crates that remain in this repo)
cargo test -p agentum-executor -p agentum-server --lib
npm run build --prefix crates/agentum-desktop/ui
```

