# CLAUDE.md — agentum codebase guide

> Living guide for Claude (and humans) working in this repo. Update it
> when you change architecture, add a crate, move a primitive, or
> introduce a non-obvious gotcha.

agentum is a self-hosted control plane for AI coding agents (Claude
Code, Codex, Gemini, Cursor, …). It boots a local daemon (`agentum
serve`) that owns:

- a SQLite database of session metadata
- a tmux server where each session is one pane running one agent CLI
- an HTTP/WS API the dashboard (Svelte) and TUI (`agentum terminal`)
  drive

A "session" is a `(name, workdir, tool, model, flags)` tuple. The
daemon spawns the right binary into a tmux pane and streams its
output to clients.

---

## Crate map

```
crates/
  agentum-core/        # Shared types: Session, Status, Event, transcript types.
  agentum-store/       # SQLite repository (sqlx). Persists sessions, board, notes, channels, users, auth.
  agentum-tmux/        # Thin wrapper over tmux: new-session, send-keys, capture-pane, kill.
  agentum-watchdog/    # Background loop. Tails panes, emits Event::AgentFinished/AwaitingInput/Crashed.
  agentum-executor/    # ToolAdapter trait + per-agent argv builders. Owns YOLO marker translation.
  agentum-server/      # axum HTTP+WS API + TLS + auth + routes/. Embeds the dashboard SPA.
  agentum/             # CLI binary. Houses the TUI under commands/terminal/.

dashboard/             # SvelteKit SPA. Builds to dashboard/build/, embedded into the daemon.
```

Each `crates/<x>/Cargo.toml` declares its deps; the workspace root
`Cargo.toml` pins shared versions.

---

## Critical: rebuild rhythm

The dashboard SPA is **embedded into the daemon at compile time** via
`rust-embed` (`crates/agentum-server/src/embed.rs`). After any change
under `dashboard/src/`, you must:

```sh
npm run build --prefix dashboard   # writes dashboard/build/
cargo build --release              # bakes dashboard/build/ into the binary
pkill agentum && agentum serve     # restart whatever was running
```

If you skip step 2, your running daemon serves the OLD bundle.

The TUI binary is the same: `cargo build` again after touching
`crates/agentum/src/commands/terminal/*.rs`. There's no hot reload.

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
| `board.rs`, `notes.rs`, `channels.rs`, `watchdog.rs`, `doctor.rs` | various | Self-explanatory. |

Auth middleware (`crate::auth::require_token`) is applied at the
top-level router merge — see `lib.rs::router`. Public paths are
listed in `auth.rs::is_public`. WS clients pass the bearer token as
`?token=` because browsers can't set headers on upgrade.

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

---

## Conventions

- **Comments**: write *why*, not *what*. Add a short comment when
  the line encodes a decision, an invariant, or a workaround. Don't
  paraphrase the code.
- **Tests**: `cargo test --workspace --lib` covers everything.
  Pre-existing breakage in `agentum-store`'s lib tests
  (NewBoardItem field churn) is a known issue, unrelated to most
  changes.
- **Frontend tests**: `npm run check --prefix dashboard`
  (`svelte-check` + tsc).
- **Clippy / fmt**: workspace runs cargo fmt; please match
  surrounding style.

---

## Quick reference

```sh
# Build everything
cargo build --release
npm run build --prefix dashboard
cargo build --release   # rebake the embedded SPA

# Run the daemon
agentum serve

# Run the TUI against the local daemon
agentum terminal

# Run the TUI against a remote profile
agentum profiles add vps https://my-vps.example.com:8822 --set-default
agentum terminal --profile vps

# Run with mute
AGENTUM_TUI_NO_SOUND=1 agentum terminal

# Tests
cargo test -p agentum-executor -p agentum-server -p agentum --lib
npm run check --prefix dashboard
```
