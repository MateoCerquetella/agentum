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

**Agentum SDD runs do not mutate linked work items while they run.** A run can
read a source reference, but comments, fields, transitions, commits, pushes,
pull requests, and releases occur only after the user confirms an exact
hash-bound Deliver preview.

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

### Desktop terminal host boundary

The desktop has two terminal transports. `Run in tmux (persist)` uses the
embedded server session/tmux stream. With persistence off, the native Tauri PTY
path is used; for an SSH repo that PTY runs an interactive OpenSSH child and the
shell itself still executes on the SSH target. The renderer passes the repo's
`connectionId` to `pty_spawn`, and the Rust handler must treat it as a hard
security boundary: an unknown, empty, or malformed remote target returns an
error and must never fall through to `default_shell_path()` on the desktop.

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
| `sdd.rs`       | `/api/sdd/*` + `/events` WS | Sole authoritative specification workflow: specs, runs, typed commands, artifacts, durable events, and delivery previews. |
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
  arm. MCP prompts, when exposed, are convenience text only; they never own or
  mutate SDD state outside the typed `/api/sdd` command contract.
- **Auth**: `/mcp` is **never public** — embedded and standalone daemons require
  a dedicated MCP bearer even when legacy HTTP automation uses `--no-auth`.
  Launch provisioning injects `Authorization: Bearer …` into Claude's combined
  MCP config and Codex's per-server configuration; SSH sessions receive the
  same authenticated endpoint through their reverse tunnel. The desktop UI
  uses a separate boot-scoped capability.
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

## Agentum SDD

Agentum has one provider-neutral owner for specification-driven work. Core
contracts and validation live in `agentum-core/src/sdd.rs`; normalized
persistence lives in store migration `0030_agentum_sdd.sql` and
`agentum-store/src/sdd.rs`; the public contract lives in
`agentum-server/src/routes/sdd.rs`.

### Public contract

The authoritative surface is:

- `POST /api/sdd/repos/{repo_id}/specs`
- `GET /api/sdd/repos/{repo_id}/specs`
- `GET /api/sdd/specs/{spec_id}`
- `POST /api/sdd/specs/{spec_id}/runs`
- `GET /api/sdd/runs/{run_id}`
- `POST /api/sdd/runs/{run_id}/commands`
- `GET /api/sdd/runs/{run_id}/artifacts`
- `GET /api/sdd/runs/{run_id}/events?after=<cursor>`
- `WS /api/sdd/events?repoId=<id>&after=<cursor>`

Mutating commands form a closed discriminated union. Every command includes a
caller-generated `requestId` and `expectedRevision`; duplicate requests
return the stored response and stale revisions fail without changing state.
State, artifact metadata, approvals, durable events, and outbox rows commit
atomically. Git, filesystem, process, model, and network work happens outside
that transaction and reports back through another revision-checked transition.

### Artifact contract

A saved specification owns exactly one portable project root:

```text
.agentum/
├── manifest.json
└── specs/
    └── spc-<ulid>-<slug>/
        └── spec.md
```

`.agentum`, the static manifest, the stable specification directory, and
`spec.md` are mandatory after save. `design.md`, `plan.json`,
`decisions.md`, and `review.md` are created only by their corresponding
phases and only when they contain real information. Runtime status, attempts,
approvals, leases, credentials, and external delivery state stay in SQLite.
Canceled or closed unsaved drafts create no repository files and no durable
run.

Canonical identity is uppercase `SPC-<26-character ULID>`; the lowercase path
slug is cosmetic. Artifact writes use containment checks, no-follow opens,
expected hashes, temporary publication, and filesystem synchronization.
External user edits become immutable revisions. A valid edit pauses the run and
invalidates approval and downstream artifacts; an invalid edit blocks without
overwriting the user's file.

### Execution boundary

Authoritative and disposable attempt worktrees live below Agentum's data
directory, never below the customer project. Every provider submits a typed
artifact or bounded diff from an isolated attempt. Agentum alone applies
accepted changes through capability grants, path leases, preimage hashes, a
patch journal, and rollback. Providers are launched through typed
`CommandSpec` values; generated shell strings are not executed.

SDD adapters must not create or read ambient provider configuration in a
customer repository. Claude, Codex, Cursor/Agent, Gemini, Hermes, OpenCode,
Aider, and custom integrations all implement the same provider-neutral result,
isolation, cancellation, timeout, and output-limit contract.

Runs advance through:

```text
specification → design → planning → implementation → verification
→ review → ready → delivery → completed
```

Ready means locally implemented, verified, and independently reviewed. It does
not mean committed, pushed, merged, released, or synchronized to a tracker.
Standard + Guarded pauses for authored-spec approval and then proceeds to Ready
unless an exception occurs. Interactive pauses after major phases. Autopilot
uses the explicit Start authorization for the current digest, but also stops at
Ready. High-risk work adds design and plan approval and cannot waive
verification. Review uses an isolated session distinct from implementation.

All external effects remain behind Deliver. The UI presents an expiring,
hash-bound preview of the selected commit, push, pull request, tracker, or
release actions. Partial and ambiguous failures stay retryable without
discarding Ready state.

### UI, fixtures, and migration

The desktop has one New Spec entry point and one Run Center with Overview,
Spec, Plan, Tasks, Evidence, Review, and Activity views. The phase rail is the
single place for next action, blockers, approval digest, workers, retries, and
delivery state.

`examples/sdd-demo/` is the zero-pollution fixture for the first vertical
slice. Its tests prove that canceling before save creates nothing, while saving
creates only `.agentum` in the external authoritative worktree and leaves the
source checkout unchanged.

The repository cutover is driven by `scripts/migrate-agentum-sdd.py`.
Preview inventories and hashes every explicitly retired source. Apply requires
an explicit absolute `--repo-root`, an external recovery archive, and an
externally supplied restricted-content pattern file. It rejects active old
runs and dirty sources, publishes validated native artifacts atomically, and
removes only the exact inventoried sources. A second successful run is a no-op.

---

## Common gotchas

- **HTML5 drag-and-drop is dead in the desktop webview (Linux/Windows)**:
  the Tauri shell keeps `dragDropEnabled` on (default) so OS file drops
  reach the screenshot-onto-terminal handler (`WindowEvent::DragDrop` in
  `agentum-desktop/src/lib.rs`) — and on Linux/Windows wry consumes the
  native drag loop for it, so in-page `dragstart`/`dragover`/`drop`
  never fire. Any new drag surface in the UI must use pointer events:
  `lib/use-kanban-pointer-drag.ts` for kanbans (contract in
  `lib/kanban-pointer-drag.ts`), or the bespoke hooks the sidebar uses.
  Don't "fix" it by disabling `dragDropEnabled` — that kills file drops.
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
