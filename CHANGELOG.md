# Changelog

All notable changes to agentum are recorded here. The format is loosely based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.7.8] — 2026-05-11

### Fixed
- **Duplicate sessions when two profiles share a daemon.** Pointing a
  loopback profile and a named profile at the same daemon (e.g.
  `""` and `macos` both on `127.0.0.1:8822`) produced two copies of
  every session in the sidebar, flickering as refresh events
  re-fired. The aggregator now keeps one entry per session id with
  a deterministic owner: active profile > any named profile >
  loopback > first-seen. Locked in by `merge_dedup_tests`.

### Changed
- **Sidebar rename: "Endpoints" → "Servers".** Less technical wording
  in the sidebar header, profile overlays, palette ("Switch
  server…"), and dashboard topbar / TokenGate. Code identifiers
  follow (`TreeSection::Servers`, `ServerStatus`, `servers_cursor`,
  `PendingAction::RemoveServer`, `UnreachableAction::AddServer`)
  so the codebase stays coherent. WebSocket-URL "endpoint" and
  selection-coord "endpoints" preserved — those refer to URLs and
  coordinates, not profiles.

## [0.7.4] — 2026-05-08

The "all in one control plane" release. The TUI and dashboard both
now connect to every configured profile in parallel and aggregate
their sessions into a unified view, grouped by endpoint.

### Added
- **Multi-endpoint fanout in the TUI.** At startup, every configured
  profile gets a non-interactive connect attempt in parallel. The
  default profile keeps the existing interactive auth path so a
  cold start still bootstraps; peers use cached credentials and
  degrade to `(unreachable)` / `(login needed)` placeholders in the
  sidebar when they can't authenticate. `App.clients` holds the
  per-profile `Client` map; `App.session_profile` tags each session
  with its owning profile name.
- **Sessions grouped by endpoint in the TUI sidebar.** `Tree::build`
  now groups by `(profile, workdir)` so each daemon's sessions
  cluster under their own header. The default profile sorts first;
  peers follow alphabetically. Group labels show
  `@profile · workdir-basename`.
- **Per-profile op routing.** All session ops — start, stop, kill,
  delete, terminal stream open, agent_tasks fetch — consult
  `App.client_for_session(id)` and talk to the right daemon. Peer
  endpoints' sessions are now fully operable from one TUI.
- **New Session form posts to the chosen profile's client.** Picking
  a different profile in the form's first field no longer triggers
  a soft restart; the create + start go straight to the chosen
  endpoint's `Client`. The new session gets its `profile` tag at
  creation time so it lands in the right sidebar group.
- **Endpoint status indicators in the sidebar.** Hollow vs filled
  dot, active vs default markers, and `unreachable` / `login needed`
  suffix labels show endpoint health at a glance.
- **Multi-endpoint fanout in the dashboard.** `lib/profiles.ts`
  gains `apiUrlForProfile`, `wsUrlForProfile`, and `fetchProfile`
  helpers. The sessions store calls all of them in parallel and
  tags each `Session` with `profile` + `profile_label`.
- **Endpoint pill on every fleet row.** Sessions from non-active
  profiles render an `@profile_label` pill in the FleetRow header
  so the unified view stays legible.
- **Per-session terminal routes WS to the owning endpoint.**
  `Terminal.svelte` looks the session up in the store, reads its
  profile tag, and uses `wsUrlForProfile` to stream from the right
  daemon. A dashboard tab can now drive sessions on any endpoint
  it has credentials for.

### Known limitations (follow-up)
- Live event stream (status dots, watchdog events) flows from the
  *active* profile only. Peer sessions update on manual `r` refresh
  in the TUI / on `loadSessions()` triggers in the dashboard. A
  per-profile event WS multiplexer is the obvious next step.
- Cross-host clipboard, OSC52, and similar terminal-side
  integrations target the active profile only.
- Profiles must be added (`agentum profiles add NAME URL`) and
  logged into individually; there's no bulk-credentials flow yet.

### Fixed
- **"Add a remote endpoint" now health-probes before saving.** The
  dashboard's unreachable card used to save any well-formed URL,
  reload, and bounce the user back to the same overlay with a
  stale entry to clean up. The form now hits `<url>/api/health`
  via `fetch` first (with a 4 s timeout) and refuses to save
  unless the response is a 200 with a recognisable
  `{"status":"ok"}` shape. Network / CORS / mixed-content errors
  are surfaced inline with hints (HTTPS-served dashboard hitting
  HTTP endpoint, etc.).
- **Cross-origin endpoint switching actually works in the
  browser.** Two compounding gates were silently blocking it:
  the daemon shipped no CORS headers (so `fetch` from a
  different origin was rejected pre-flight), and the
  dashboard's `Content-Security-Policy` `connect-src 'self'
  ws: wss:` blocked even reaching out to other origins. Added
  a permissive `tower_http::cors::CorsLayer` (Allow-Origin: any
  — bearer-token wall on the daemon side is the actual access
  gate; we don't use credentialed cookies, so wildcard is
  safe) and loosened CSP to `connect-src 'self' http: https:
  ws: wss:`. A dashboard hosted by daemon A can now talk to
  daemon B once the user adds B as a profile.

### Removed
- **"← back to landing page" link on the unreachable card.**
  The dashboard *is* the landing page; the link reloaded into
  the same gate. Replaced with a list of every other saved
  profile so users can switch endpoints without leaving the
  card (each row has a × to drop a stale entry).

### Changed
- **Mobile home page lifts the fleet into the viewport.** Hero stays
  on top in DOM order so the greeting reads first, but it's
  compressed on phone (host strip + spawn-incidents button
  hidden, narrative tightened) and the three summary cards drop
  entirely below 700px — their numbers are redundant once the
  fleet rows are visible. The fleet header trims to filter tabs
  only; sort and group default sensibly and aren't worth the
  chrome cost on a 4-inch viewport.
- **Mobile session-detail terminal goes near full-screen.** The
  right `SessionRail` (plan / KV / watchdog) is hidden below
  700px, and the redundant `term-bar` row drops too — the
  terminal canvas now fills almost the entire space between the
  toolbar and the sticky input row. Tablet (≤1100px) keeps the
  rail.

## [0.7.3] — 2026-05-08

Lands the TUI sidebar + spawn-form work that 0.7.2's release notes
described. The 0.7.2 binary on origin only carried the mobile
session-detail fix; the endpoint sidebar + form-survives-switch
features described in its commit message ship here.

### Added
- **Endpoints section in the TUI sidebar.** The tree pane now hosts
  two sections: an Endpoints list at the top (each configured
  profile, default marked) and the Sessions tree below. `j` / `k`
  flips between them at the boundaries; cached at startup into
  `app.profiles` so the file is read once, not per frame, and
  refreshed via `reload_profiles` after add / remove from any
  surface (overlay, sidebar action, CLI).
- **New Session form survives a profile switch.** Submitting the
  form against a different profile than the active one now
  carries the typed-in fields through the soft-restart via a new
  `PendingAfterSwitch::OpenNewSession` outcome. The `Profile`
  field gets normalised to the freshly-connected daemon so the
  user's next `Enter` creates immediately instead of ricocheting
  through another switch.
- **Profile is now the first field of the New Session form.** The
  user picks the endpoint before the folder so the spawn lands on
  the intended daemon by default. Tab cycles through configured
  profiles plus an `(current connection)` entry for ad-hoc
  `--api`/loopback runs. Submitting against a non-active profile
  triggers the soft restart described above; same-profile submits
  go straight to creation.
- **Sidebar endpoint actions: `a` to add, `d` to remove, Enter to
  switch.** When the cursor is in the Endpoints section, these
  keys do what they say on the tin without leaving the tree pane.
  Add reuses the same overlay form as Ctrl-O; remove guards the
  active profile so users can't accidentally orphan their session.

## [0.7.2] — 2026-05-08

### Fixed
- **More room for the terminal on the mobile session detail page.**
  Three small CSS changes that compound: SessionRail's mobile
  `max-height` drops from `50dvh` to `28dvh` (the rail's `.rb`
  body already scrolls, so users keep access to the full meta
  block via drag); the desktop `term-bar` row is hidden on phone
  because its info (tool / workdir / model / tmux / ctx%) is
  already covered by the toolbar pills and the rail's KV table —
  dropping the 30px chrome hands the height back to the
  terminal; `term-shell` gutters tighten from `8px 8px 50px` to
  `4px 4px 8px` (the absolute-bottom input bar's stub padding
  was already overridden separately on phone). Net effect: the
  xterm canvas fills near the chrome on a phone instead of
  fighting a half-screen rail and a redundant header strip.

## [0.7.1] — 2026-05-08

Patch on top of 0.7.0 closing the TUI ↔ dashboard parity gaps in the
profile + agent-gating story shipped there.

### Added
- **In-TUI endpoint switcher (`Ctrl-O`).** New `Overlay::Profiles`
  lists configured profiles with the active one marked, lets the user
  switch (Enter), add (`a`), or remove (`d`) endpoints without leaving
  the TUI. Switching triggers a soft restart of the run-loop —
  `commands::terminal::run` tears down the alt-screen, reconnects via
  `connect_once`, and re-enters `run_tui_session` against the new
  daemon. Surfaced in the command palette as "Switch endpoint…" too.
- **Active-profile indicator in the TUI title bar.** Title now reads
  `agentum · <session> · @<profile>` so users driving multiple
  agentum servers can tell at a glance which one's behind the active
  pane. Hidden when no profile is in play (loopback / ad-hoc `--api`).
- **Empty-daemon onboarding.** When `agentum terminal` can't reach a
  daemon (no `--api`, no `--profile`, no resolvable default,
  loopback unreachable) and stdin is a TTY, the CLI prints a
  numbered menu — `[1] Add a remote endpoint`, `[2] Retry`,
  `[3] Quit` — instead of failing hard. Picking add walks through
  name + URL + optional fingerprint, saves the profile, and retries
  the connection. Non-TTY callers (CI, scripts) keep the previous
  hard-bail behaviour.
- **Dashboard "no daemon reachable" recovery.** `TokenGate.svelte`'s
  `unreachable` state now shows an inline "Add endpoint" form
  alongside the retry button, so a user landing on a dead origin
  can point the dashboard at a different agentum without leaving
  the page. Saves to the same `localStorage` profile store the
  topbar `EndpointSwitcher` uses; submit reloads.
- **`(not installed on the daemon)` inline hint on the TUI's Tool
  field.** Mirrors the dashboard tile dimming. The hint replaces
  the cycle-order subtitle and tints the value red while the picker
  is parked on an uninstalled binary, so the user sees the gating
  reason without having to submit and wait for an error toast.
- **`opencode` and `aider` now appear in the agent-availability
  probe.** `agentum-executor` exposes a new `PASSTHROUGH_PROBED`
  list + `probed_tools()` iterator that `/api/agents` consumes, so
  the dashboard tiles and TUI Tab-cycle can grey out either one
  when its binary isn't on `PATH`. Their launch path stays through
  `PassthroughAdapter` (no hard-coded YOLO flag yet); promoting
  them to first-class adapters is a follow-up.
- **`agentum profiles` `--help` examples + post-add hint.** The
  subcommand grew an `EXAMPLES` block, and `agentum profiles add`
  now prints the next-step usage (`connect with: agentum terminal
  --profile NAME`, or `default profile is now NAME — run agentum
  terminal to connect`). Closes the "I added a profile, now what?"
  loop.
- **`CLAUDE.md`.** Top-level codebase guide for Claude (and humans):
  crate map, rebuild rhythm (the rust-embed compile-time gotcha),
  adapter-pattern checklist, YOLO marker translation table, agent
  gating story, profiles end-to-end, route layer reference,
  parity table between TUI and dashboard, common gotchas, quick
  reference commands.

### Fixed
- **Agent gating only applied to Cursor.** The dashboard's `TOOLS`
  array marked `opencode` and `aider` as `firstClass: false`, so
  they were never gated even though `/api/agents` could probe them.
  Now they're `firstClass: true`; `terminal` and `bash` stay
  always-available since they don't need a probe (`$SHELL` is
  universally present).

## [0.7.0] — 2026-05-08

Minor bump because of two new user-facing features (Cursor adapter +
named connection profiles) on top of the v0.6.x series.

### Added
- **Cursor agent adapter (`cursor`).** First-class support for the
  `cursor-agent` headless CLI: passes `--model` through, accepts
  free-form user flags, and translates the wire-format YOLO marker
  to Cursor's `--force` switch. Listed in the New Session form's
  Tab-cycle and the dashboard's tool dropdown. `cursor` joins the
  `YOLO_TOOLS` set since its adapter declares the correct per-tool
  flag (the v0.6.24 lesson — never add a tool to that set without
  an adapter mapping).
- **Named connection profiles for the TUI (`agentum profiles
  list/add/remove/use`).** Save multiple `agentum serve` endpoints
  (URL + pinned fingerprint + `--insecure` toggle) and switch
  between them with `agentum terminal --profile NAME` instead of
  retyping `--api https://…` every time. A `default` pointer lets
  `agentum terminal` (no flag) hit the right endpoint
  automatically; falls back to the loopback probe if no default
  is set. Storage is a TOML file alongside `known_hosts.toml` so
  cert pins and credentials cache hit the same per-host key.
- **`EndpointSwitcher` chip in the dashboard topbar.** Surfaces
  the active profile, opens a dropdown to switch / add / remove
  endpoints. Switching reloads the page (cheapest correct
  refresh — every store / WS / cached query reflects the new
  endpoint without per-store invalidation logic). Profile state
  lives in `localStorage` keyed by user.
- **`GET /api/agents` endpoint.** Single-shot probe of which
  first-class agent binaries are actually on the daemon's `PATH`.
  The TUI / dashboard hit it once on startup so the agent picker
  can grey out unavailable entries with a clear hint instead of
  silently letting the user spawn a session that crashes with
  `command not found` on the next `tmux send-keys`. The Tab-cycle
  on the `Tool` field skips unavailable entries.

### Fixed
- **Dashboard terminal scrolled the page upward indefinitely on
  session entry.** `_design.css` carried a leftover
  `.term-host .term { padding: 14px 18px 50px; overflow-y: auto;
  ... }` block from a static design preview that no longer
  exists. The selector matched the xterm host div in
  `Terminal.svelte` (whose class was also `term`), so FitAddon
  read a wrong row count *and* the host div itself became
  scrollable on top of xterm's internal viewport. Each repaint
  scrolled the outer host upward, dragging the page with it.
  Removed the dead block. `.term-shell` is now a flex container
  so xterm stretches to the available height instead of
  collapsing to its scrollback's intrinsic size.
- **TUI alt-screen sometimes went completely black with no
  visible text.** Two compounding writes-outside-ratatui bugs:
  (1) `sound::bell()` wrote `\x07` (BEL) directly to stdout when
  no system audio player was on PATH, bypassing ratatui's diff
  renderer — same anti-pattern the v0.6.33 `write_osc52` fix
  disabled. Inside tmux a raw byte mid-frame can split a
  neighbouring CSI escape and leave the alt-screen swallowing
  parameters until a final byte appears. (2) `init_tracing`
  routed every `tracing::*` event to *stderr*, which shares the
  TTY with the alt-screen. Any `info+` log from a dependency
  (tungstenite, rustls, h2…) landed directly on the rendered
  cells. Bell is now a no-op (visual notification still fires);
  TUI tracing now writes to `$XDG_CACHE_HOME/agentum/tui.log`
  via `init_tracing_for_tui` instead of stderr.

## [0.6.33] — 2026-05-07

### Added
- **WebSocket auto-reconnect with exponential backoff.** Both the
  events stream (`/api/events`) and the per-session terminal stream
  now reconnect automatically when the connection drops, with
  capped exponential backoff between attempts. New `Reconnecting`
  variants on `ConnState`, `TerminalMsg`, `EventMsg` carry the
  attempt count + delay so the status bar can show "reconnecting
  (try N)" instead of a final-looking "disconnected". The
  pre-first-connect `Closed` / `Error` no longer flashes
  Disconnected — gated on `was_connected`. Status bar shows a
  `⟳ reconnecting` chip and a centered overlay surfaces the
  current attempt number plus retry-in-N-seconds countdown.

### Changed
- **`Space` now selects + focuses the terminal in the tree
  sidebar; `Enter` is reserved for an upcoming multi-select mode.**
  Enter currently shows `multi-select coming soon — use Space to
  enter the terminal` so the keystroke isn't a silent no-op while
  users adjust. Space is what most file managers / browsers use
  for "preview / open" — Enter freed up for bulk-action UX.

### Fixed
- **Plan / Todos / Tasks panel was permanently empty when the
  agent's first turn started after agentum's slot bootstrap.**
  Pre-pin sessions (and any case where claude hadn't yet
  materialized `<agentum-uuid>.jsonl`) locked the slot onto a
  fallback transcript at creation time and never re-checked,
  even after claude finally wrote the pinned file. The
  per-session refresh path now promotes to `pinned_path` the
  moment it appears on disk, wiping the cursor + replayed state
  so the panel rehydrates from the agent's own transcript
  instead of staying pinned to a cross-pollinated stranger.
- **Plain `q` from the tree no longer quits the app.** Easy to
  fat-finger after typing into the terminal, and the filter
  prompt accepts `q` as a search character with no warning
  flag. Ctrl-Q remains the universal hard-quit (handled
  earlier in the dispatcher), and the command palette still
  carries an explicit Quit action.
- **Mouse-select-to-copy disabled to stop visible text corruption
  inside tmux.** The v0.6.31 OSC 52 implementation wrote the
  escape sequence directly to stdout from the input handler,
  *mid-frame*, while ratatui owned the screen. Inside tmux the
  sequence wasn't wrapped in DCS passthrough so tmux echoed it as
  literal text; outside tmux the raw write bypassed ratatui's
  diff renderer so disturbed cells stayed disturbed across the
  next draw. Disabled until the OSC 52 emit is reworked into a
  proper deferred between-frames flush queue. The in-buffer
  highlight still renders correctly during the drag — only the
  host-clipboard write is dropped.

## [0.6.32] — 2026-05-07

### Fixed
- **Sidebar dot stuck grey after the first Working→Idle cycle.**
  v0.6.30 added the `agent.working` event for the reverse Idle→
  Working transition, but a single dropped event (bus capacity
  exhaustion, transient WS hiccup, or a stale pre-v0.6.30 daemon)
  would pin the session in `app.idle` indefinitely — exactly the
  "works once then never again" symptom users hit. Three layered
  fixes so a missed event can't strand the indicator:
  1. **TUI optimistically clears `app.idle` / `app.awaiting_input`
     when the user types into a session.** A keypress is a strong
     local "the agent is working again" signal; believe it now and
     let server-side events confirm on the next watchdog tick.
     Self-heals the foreground-pane case without any server-side
     dependency.
  2. **Watchdog tick tightened from 5 s → 1 s.** The `tmux
     capture-pane` call is a few ms — 5× more invocations is still
     trivial, and now Working↔Idle transitions surface fast enough
     to feel instant. Also gives missed events a 5× faster
     recovery window.
  3. **Event bus capacity raised from 256 → 1024.** A slow client
     (focus-stolen TUI, brief network stall) used to drop events
     past the 256-message backlog; the activity-dot events live
     longer in the queue now so a momentary stall doesn't cost a
     state change.

## [0.6.31] — 2026-05-07

### Added
- **Mouse-select-to-copy inside the terminal pane.** Click-drag now
  highlights cells in real time and OSC 52 ships the text to the
  host terminal's clipboard on release. The status bar shows
  `copied N chars` for confirmation. When the inner program (claude
  code, vim, k9s, …) has its own mouse tracking on, **Shift+click-
  drag** bypasses forwarding and starts a selection — the same
  convention xterm / Alacritty / kitty / iTerm use. Works over SSH
  because the local terminal interprets OSC 52, not the daemon.
  Selection rendering uses `Modifier::REVERSED` so glyphs and
  per-cell colour are preserved; release clears it.

### Changed
- **Plan / todos / background-tasks panel updates instantly on
  agent navigation.** Fetches now run spawn-detached (the keystroke
  handler never blocks on HTTP), the cache is pre-warmed for every
  known session at startup so j/k is a pure cache hit after the
  first frame, and concurrent fetches for the same id are coalesced
  via an in-flight set. Newly-discovered sessions on the 5-second
  refresh are also primed in the background.
- **Lazygit follow-up is debounced through the tick loop** so a
  held-j burst across many repos fires exactly one PTY respawn at
  the user's settled destination instead of one per session. The
  120 ms window is below the human "instant" threshold for a single
  nav yet long enough to coalesce typematic keystrokes (~30 ms
  apart).

## [0.6.30] — 2026-05-07

### Fixed
- **Sidebar dot stayed grey while an agent was actively working
  again after a previous turn.** v0.6.28 added the muted `◌`
  sleeping dot, populated by `agent.finished` (Working→Idle), but
  the watchdog never emitted anything for the reverse Idle→Working
  transition — so the TUI kept the session pinned in its idle set
  forever and the dot stayed grey while the agent was visibly
  busy. Watchdog now emits a new `agent.working` event for that
  transition; the TUI clears the idle bit on receipt.
- **Idle dot was the same dim grey as the placeholder `—` and the
  tool/model label**, which made it easy to miss at a glance.
  Switched the colour from `p.muted` to `p.accent_alt` so a
  sleeping agent reads as a distinct cool-tone `◌` against the
  green/yellow/red of the other states.

## [0.6.29] — 2026-05-06

### Changed
- **Status bar — `connected` and live network throughput chips
  moved from the right-aligned cluster to the left, immediately
  after the workdir + tool chips.** Connection state belongs next
  to the path it applies to; pinning it to the left edge also
  keeps it from shifting horizontally as the right cluster grows
  and shrinks (lazygit toggle, transient status messages, theme
  name length). Lifetime IO totals, lazygit/theme/palette/help
  hints stay on the right.

## [0.6.28] — 2026-05-06

### Added
- **Muted `◌` sidebar dot for sleeping (idle) agents.** v0.6.27
  added the yellow `▲` for sessions awaiting a prompt, but a
  finished agent sitting at the prompt still rendered as a green
  `●` — visually identical to one that's actively working. The
  sidebar now distinguishes three states explicitly: green `●` for
  working, yellow `▲` for awaiting input, and a dim `◌` for idle.
  Crashed `✗` still wins. Single-cell glyph throughout so the row
  width stays stable — a 2-cell sleep emoji would have shifted the
  tool/model label by a column.

### Changed
- **`agent.input_resolved` now carries a `state` payload** of
  either `"working"` or `"idle"`, so a single event tells the TUI
  whether to flip the dot to green or to muted `◌` without waiting
  for a follow-up `agent.finished`. Older clients that ignored the
  payload still get the awaiting-input clear behaviour they
  already had — payload absence is treated as "leave the idle bit
  alone and let the next finished/working event settle it".

## [0.6.27] — 2026-05-06

### Added
- **Sidebar status dot now flips yellow when an agent is awaiting
  user input.** The watchdog already detected permission prompts
  (Claude's "Do you want to proceed?", etc.) and emitted a toast,
  but the sidebar leaf kept its green Running dot — easy to miss
  when you're tabbed away to lazygit or another pane. Sessions in
  the awaiting set now render as a yellow `▲` regardless of their
  persisted `Status`. Crashed still wins (red `✗`) so a dead pane
  never looks like it's just waiting; everything else falls
  through to the existing green/idle/stopped dots.

### Changed
- **Watchdog emits a new `agent.input_resolved` event** when a
  pane leaves the `AwaitingInput` activity state (back to Working
  or Idle). Existing transitions — `agent.finished` for
  Working→Idle and `agent.awaiting_input` for any→AwaitingInput —
  are unchanged. The new event lets clients clear "needs input"
  badges as soon as the prompt is answered, without waiting for a
  separate finished/working signal. Defensive cleanup is also
  wired on `agent.finished`, `session.stopped`, `session.crashed`,
  and `session.deleted` so the badge can never get stuck on a
  stale id after an event-bus lag or watchdog restart.

## [0.6.26] — 2026-05-06

### Fixed
- **Daemon restart caused "duplicate footer / duplicate content"
  corruption.** The `stream_positions` map (per-session log offsets
  for resume-aware reconnects) is in-memory only — `agentum serve`
  restart wipes it. The reconnect path looked up the saved offset
  with `unwrap_or(0)`, so after a restart any client that requested
  `resume=true` got the **entire log** shipped as "delta" and
  replayed on top of its still-cached parser state. Visible result:
  Claude's sticky footer (`▶▶ bypass permissions…`) baked into the
  middle of the scrollback, the response body duplicated below
  itself, and the prompt input rendered twice. Fixed by treating "no
  saved position" as "fall through to the fresh capture-pane
  snapshot path" — client gets `\x1b[2J\x1b[H` + a clean snapshot,
  parser resets cleanly to current truth.

- **Plan / Todos / Tasks panels stayed empty for pre-v0.6.25
  sessions.** v0.6.25 added `--session-id <agentum-uuid>` to the
  Claude launch and switched the transcript watcher from an mtime
  heuristic to the deterministic `<agentum-uuid>.jsonl` path. New
  sessions get their pinned file on the first turn — but sessions
  created **before** v0.6.25 have a claude that's writing to its own
  random UUID, so the pinned file never materializes and the panel
  watcher silently sits on a non-existent path. Re-introduced
  `latest_transcript_excluding(dir, exclude)` as a fallback: when
  the pinned path doesn't exist, the slot uses the most-recently-
  modified `*.jsonl` in the project dir instead. Post-pin sessions
  always hit the deterministic path first and never trip the
  fallback, so cross-pollination only matters for legacy sessions
  with multiple agents in one workdir — same trade-off the panel
  had pre-v0.6.25, restored only for the migration window.

## [0.6.25] — 2026-05-06

### Fixed
- **Reconnect / session-switch failed with `ws connect: HTTP error:
  200 OK`.** v0.6.21..=v0.6.24 built the resume-aware terminal-stream
  URL with the resume bit baked into the path string:

  ```rust
  let path = format!("/api/sessions/{id}/stream?resume=true");
  let url = ws_url(&base, &path, &token);
  ```

  Inside `ws_url`, `url::Url::set_path(...)` treats its argument as a
  path component and percent-encodes the `?` to `%3F`. The wire URL
  became `wss://host/api/sessions/{id}/stream%3Fresume=true?token=...`,
  which the daemon decoded to a literal path of
  `/api/sessions/{id}/stream?resume=true` — no match for the
  registered `/api/sessions/{id}/stream` route. The request fell
  through to the SPA fallback (`embed::static_handler`), which returns
  200 OK with index.html. tungstenite reported `HTTP error: 200 OK`
  and emitted `terminal stream closed` to the user.

  Only first connects (resume=false) worked — the broken path was
  reachable only via session switches and reconnects, which is why
  the bug looked like "I can't reconnect to my agents".

  `ws_url` now takes a separate `extra_query: &[(&str, &str)]` slice;
  callers pass `("resume", "true")` as a real query pair. Added
  regression tests asserting (a) `?resume=true` ends up in the query,
  not the path, and (b) the serialized URL contains no `%3F`. A
  `debug_assert!` in `ws_url` panics if a future caller smuggles a
  `?` into `path` again.

- **Errors overlay spammed with duplicate "terminal stream closed"
  lines.** Each keystroke into a dead WS channel hit `push_error`
  (app.rs:1808), so a typing burst against a disconnected agent
  produced 25+ identical entries. `push_error` now suppresses an
  exact-duplicate message pushed within 2 s of the previous one.
  Distinct messages and the same message after a quiet window still
  go through; this collapses bursts without losing live recurrences.

## [0.6.24] — 2026-05-06

### Fixed
- **YOLO mode crashed non-Claude agents.** Both clients (TUI + dashboard)
  pushed Claude's spelling — `--dangerously-skip-permissions` — into
  every session's flags list when YOLO was on, and each executor
  adapter forwarded `session.flags` verbatim. Codex sessions died
  immediately at launch with `error: unexpected argument
  '--dangerously-skip-permissions' found / tip: a similar argument
  exists: '--dangerously-bypass-approvals-and-sandbox'`. `opencode`
  had the same shape (listed in `YOLO_TOOLS` but no known flag).

  YOLO is now translated at adapter launch, not the wire: clients
  still push the canonical marker (`--dangerously-skip-permissions`)
  for back-compat, and `ToolAdapter::yolo_flag()` declares each
  binary's actual spelling. The translation is per-tool:

  - claude → `--dangerously-skip-permissions` (identity)
  - codex → `--dangerously-bypass-approvals-and-sandbox`
  - gemini → `--yolo`
  - tools without a known flag → marker is dropped (silent no-op
    rather than a launch crash)

  `opencode` is removed from `YOLO_TOOLS` in both clients until its
  flag is verified — clicking YOLO with opencode selected just hides
  the toggle now, instead of crashing the session on launch.

  Regression tests cover all three transformations + the drop path.

## [0.6.23] — 2026-05-06

### Fixed
- **Dashboard terminal pane initial-snapshot corruption.** The dashboard
  raced two async operations on connect: opening the WebSocket and
  probing `/api/health` for the `resize` capability. The WS open
  consistently won on localhost, so the first `sendResize()` in
  `ws.onopen` early-returned with `resizeSupported = false`, the
  server's 250 ms `INITIAL_RESIZE_WAIT` window expired with no resize
  received, and the daemon `capture-pane`d tmux at its pre-sized
  132×40 default. xterm rendered those bytes at the host element's
  width — every line wrapped wrong, Claude's sticky footer
  (`▶▶ bypass permissions…`) reflowed into the middle of the chat
  scrollback, and leading characters got eaten at line edges
  (`Searching` → `S  rching`). Probe is now serialized before the WS
  open: one extra HTTP round-trip (single-digit ms on localhost) buys
  a guaranteed correctly-sized resize as the first thing the WS sees.

### Added
- **Activity-state notifications: `agent.finished` and
  `agent.awaiting_input`.** The watchdog classifies each session's
  pane snapshot into Working / Idle / AwaitingInput from cheap
  pane-substring signatures the executor adapter declares
  (`busy_signature`, `awaiting_input_signatures`). Working → Idle
  emits `agent.finished`; (Working|Idle) → AwaitingInput emits
  `agent.awaiting_input`. Both surfaces (TUI + dashboard) toast on
  these events; `agent.finished` is suppressed when the user is
  already viewing the originating session, `agent.awaiting_input` is
  unconditional because it's a "you have to do something" signal.
  Permission-prompt detection beats busy detection — Claude keeps the
  spinner up while a prompt is open. Adapters without declared
  signatures stay in `Unknown` forever and never emit, so we never
  fire spurious finished/awaiting toasts on tools we don't recognize.

### Changed
- **Transcript parser tracks Claude Code's new task-tool family.**
  Recognises `TaskCreate` / `TaskUpdate` / `Agent` (formerly `Task`)
  alongside the legacy `TodoWrite` so newer transcripts produce
  populated agent-task panels. `TaskCreate` rows bind to the numeric
  `task_id` parsed from the matching `tool_result`; `TaskUpdate`
  patches by id; `status: deleted` removes the row. Legacy
  `TodoWrite` transcripts continue to render via the latest-call-wins
  path.

## [0.6.22] — 2026-05-06

### Added
- **Lazygit pinned to a far-right column with resizable width.**

## [0.6.21] — 2026-05-06

### Changed
- **Resume signal moved from wire frame to URL query.** v0.6.19/0.6.20
  put `{"resume":true}` on the WS as a JSON text frame and tried to
  protect old daemons via capability gating. Both layers can fail
  (probe timeouts, edge cases in capability advertisement, partial
  upgrades) and the failure mode types literal characters into the
  agent's prompt — a footgun bad enough that no amount of gating
  makes it acceptable.

  v0.6.21 puts the resume signal in the WS upgrade URL as a query
  string: `/api/sessions/{id}/stream?resume=true`. axum's `Query`
  extractor in old daemons silently drops unknown fields; the upgrade
  proceeds as before, the resume bit is just ignored, and there is
  no possible path for the signal to be forwarded to the agent's
  stdin. New daemons read the param and use the saved-position log
  delta path.

  Removed: `TermOut::Resume` enum variant, the `parse_resume` server
  helper, the `resume_supported` capability gate field, and the
  `client.capabilities()` startup probe (kept the method on `Client`
  itself for future use). The `"resume"` advertisement in
  `/api/health.capabilities` is left in for any external clients
  that may probe it.

  This is a wire-format simplification, not a behaviour change for
  matched-version client/daemon pairs — the in-memory log-position
  state and per-session parser cache from v0.6.19 still drive the
  fix.

## [0.6.20] — 2026-05-06

### Fixed
- **`{"resume":true}` typed into the agent's prompt against an old
  daemon.** v0.6.19 added the resume protocol but didn't gate the
  client-side emission on capability negotiation. Result: anyone
  running a v0.6.19 binary against a daemon < v0.6.19 (very common —
  `agentum update` swaps the binary, but the running `agentum serve`
  keeps old code in memory until killed) saw the new client send
  `{"resume":true}` text frames on every session-switch with cached
  state, and the old daemon's WS handler — which doesn't recognise
  the envelope — forwarded those frames as raw input keystrokes via
  `tmux send-keys`, typing the literal characters into the agent's
  prompt.

  Same gotcha v0.6.9 fixed for the `resize` envelope. Same fix:

  - **Server (`routes/health.rs`)**: append `"resume"` to the
    advertised capabilities list at `/api/health.capabilities`.
  - **Client (`app.rs`)**: probe capabilities once at startup, store
    `App.resume_supported`, gate the `TermOut::Resume` emit on the
    flag being true. Old daemons silently downgrade to the snapshot
    path (still gets the old behaviour, but no prompt corruption).

  As before, the protective effect of this gate only kicks in once the
  v0.6.20+ binary is the running daemon — `pkill -f "agentum serve"`
  after `agentum update` is still required.

## [0.6.19] — 2026-05-06

### Fixed
- **Session-switch wipes visible chat history.** The actual root cause
  of the recurring "switch away → switch back → content gone" reports.
  When the user switched away and back, the TUI client called
  `term.reset()` (destroying its vt100 parser), opened a fresh WS, and
  the daemon shipped a `tmux capture-pane` snapshot reflecting whatever
  the agent's UI looked like *right now*. After a task completes,
  claude code's UI is mostly empty (just task header + input box) — so
  the snapshot replay overwrote all the visible chat history with that
  near-empty state. No amount of resize/settle/snapshot-timing tuning
  could fix this because the *snapshot itself* was the wrong artefact
  to send.

  Two-piece fix:

  - **Client (`agentum/src/commands/terminal/app.rs`)**: keep a
    `parser_cache: HashMap<Uuid, TerminalPane>` on `App`. On switch,
    stash the current parser keyed by the old session id and restore
    one for the new selection (or install a fresh one if there's no
    cache hit). Replaces the previous `term.reset()` on switch.

  - **Server (`agentum-server/src/routes/sessions.rs`)**: track
    per-session log-file positions in a `Mutex<HashMap<Uuid, u64>>` on
    `AppState`. The WS handler now recognises a `{"resume":true}` text
    frame in its initial wait window — when present, it replays the
    log delta from the saved position instead of capturing a fresh
    snapshot. The TUI client emits this frame whenever it restored a
    parser from cache.

  Net effect: switching away and back no longer touches visible chat
  history. The bytes the agent emitted while the user was on the other
  session are replayed into the preserved parser, bringing it from the
  pre-switch state to the live tail without any
  `\x1b[2J\x1b[H`-clobber.

  Wire format addition: `{"resume":true}` text frames on the
  `/api/sessions/{id}/stream` WebSocket. Old daemons ignore unrecognised
  text frames (they're already treated as raw input by the legacy path,
  which is harmless for `{"resume":true}` since it's not a meaningful
  keystroke). Old clients never emit it, so they fall through to the
  existing snapshot path.

## [0.6.18] — 2026-05-06

### Fixed
- **Pre-size the tmux pane at session creation (132×40).** The resize
  protocol does its job once a client connects, but until then a fresh
  detached session lives at tmux's default 80×24 — and that's where
  the embedded agent (claude code, codex, opencode) launches and
  renders its first frames. Some ratatui apps don't reflow already-
  rendered chat history when the viewport later widens, so users were
  seeing text wrapped at ~70 cols stranded inside a much wider visible
  pane (the screenshot a user reported with `Bash(cargo check ...)`
  output narrow-wrapped while the pane had ~136 cols of empty space
  to its right).

  `agentum_tmux::new_session` now passes `-x 132 -y 40` to
  `tmux new-session`, so the pane starts at a width any modern client
  can comfortably display. When a client connects with a different
  size, the existing resize-window flow kicks in as before; tmux
  reflows lines on resize, so growing a pane just unwraps content
  rather than truncating it.

  Two constants exposed (`DEFAULT_PANE_COLS`, `DEFAULT_PANE_ROWS`)
  for easy tuning without code-spelunking.

## [0.6.17] — 2026-05-06

### Fixed
- **Session-switch corruption (third pass — proper fix).** The v0.6.15
  commit advertised a "poll log activity for post-resize settle" change
  but only landed the unrelated transcript-watcher hunk; the actual
  settle logic was lost in the diff. So the daemon was still doing the
  v0.6.14 fixed 80 ms post-SIGWINCH sleep, capturing during the
  embedded TUI's repaint burst, and shipping half-frames — characters
  from claude's 80×24-positioned status line still landing inside
  scrollback content (`s2ill ts` etc).

  This release lands the actual settle loop AND fixes a logic bug in
  the v0.6.15 design: the original would exit after two quiet polls
  even when no activity had ever been observed, so we'd return BEFORE
  the embedded process had even started reacting to SIGWINCH. The new
  logic requires activity to have been seen before treating quiet as
  "settled". When no activity occurs within 180 ms of the resize, we
  bail (resize was probably a no-op — size already matched). Hard
  cap: 400 ms.

  Connect-time latency:
  - no-op resize / size unchanged: ≈180 ms
  - SIGWINCH-triggered repaint: scales with actual repaint duration
  - active stream: 400 ms cap

## [0.6.15] — 2026-05-06

### Fixed
- **Half-painted pane on session switch.** v0.6.14 fixed the resize race
  but kept a fixed 80 ms post-SIGWINCH sleep before snapshotting. That's
  enough for an idle pane, but ratatui-based agents (claude code, codex,
  opencode) reacting to a real size change can take well over 100 ms to
  emit a full repaint. We were capturing mid-burst and shipping a frame
  that contained only what the agent had drawn so far — typically just
  a streaming indicator, with the input box and footer missing.
  User-visible symptom: switching to a session landed on a near-empty
  pane that never filled in.

  Replaced the fixed sleep with a poll on the pane log file's size.
  While the embedded process is emitting bytes, the pipe-pane log
  grows; when two consecutive 40 ms intervals show no growth, the
  repaint burst is considered settled and we capture. Capped at
  280 ms total so an actively-streaming agent doesn't hold connect
  open — at the cap we accept a mid-stream snapshot and let the
  live tail paint over it.

  Connect latency for an idle pane: ≈80 ms (two quiet polls).
  For a post-SIGWINCH repaint: scales with the actual repaint
  duration, capped at 280 ms.

## [0.6.14] — 2026-05-06

### Fixed
- **Embedded-TUI scrollback corruption on stream connect.** The WS handler
  was capturing `tmux capture-pane -e` and shipping the snapshot to the
  client *before* reading any input from the socket. Resize messages from
  the client (`{"resize":{"cols":N,"rows":N}}`) only landed after the
  snapshot was already in flight, so the snapshot — and the live bytes
  that followed — were dimensioned for tmux's stale pane size (80×24 for
  fresh detached sessions). The embedded TUI kept emitting cursor-position
  escapes against the wrong size, the client's vt100 parser placed those
  characters in the wrong cells, and status-line text like `esc to
  interrupt` overpainted scrollback content — leaving permanent artefacts
  like `okterrupt` in the parser's history. v0.6.7 added the resize
  protocol but kept this race; v0.6.11's high-fidelity snapshot replay
  made it dramatically more visible.

  The handler now waits up to 250 ms for the client's first resize text
  frame, applies it to tmux, settles for 80 ms so the embedded process
  can react to SIGWINCH and emit a fresh frame, *then* captures and
  ships the snapshot. Old clients that never send a resize fall through
  to the previous capture-at-current-size path. Any non-resize input
  that arrives during the wait window is buffered and forwarded to the
  pane after the snapshot, so no keystrokes are silently dropped.

  Single-file change in `crates/agentum-server/src/routes/sessions.rs`;
  no protocol or client changes required.

## [0.6.13] — 2026-05-06

Unified notifications across TUI and dashboard, with system sounds in
the terminal.

### Added
- **Bottom-left toast stack in the TUI.** Session lifecycle events now
  render as bordered, severity-coloured toasts (info/warn/error)
  stacked above the status bar, instead of a single text chip jammed
  inside it. Newest on top, FIFO-capped at 4, auto-expire by TTL
  (info 6s, warn 4s, error 12s — matching the dashboard).
  Implemented as an overlay so the layout never reflows when toasts
  come and go.
- **System sounds on TUI notifications.** Plays a per-severity sound
  via the platform's native player — `paplay`/`pw-play` on Linux
  (freedesktop `dialog-error.oga` / `dialog-warning.oga` /
  `dialog-information.oga`), `afplay` on macOS (`Sosumi` / `Funk` /
  `Glass`). Falls back to BEL (`\x07`) when no player is on PATH.
  Fire-and-forget via `tokio::process::Command`; no new Cargo
  dependencies. Mute with `--no-sound` or
  `AGENTUM_TUI_NO_SOUND=1`.
- **Dashboard `session.stopped` toast.** The web toast stack now
  surfaces clean stops (4s info), matching the TUI for parity.

### Changed
- **`watchdog.compact` events now toast in the TUI.** Previously
  silent; now mirrors the dashboard with a 6s info toast carrying
  the auto-compact reason.
- **`bus.lagged` surfaces as both a toast and an error-overlay
  entry.** Used to live only in the errors overlay; now also a
  warn toast so the user sees skipped events at the moment they
  happen.
- **`session.started` is silent in the TUI.** Matches the dashboard
  — bus replays on reconnect would otherwise spam the stack.

## [0.6.12] — 2026-05-06

Recent-errors overlay.

### Added
- **`e` opens a recent-errors log.** The status bar's red error chip
  has been a counter for ages — now it's also a doorway. Press `e`
  from tree focus (or pick "Errors · view recent error log" from the
  command palette) to see what actually went wrong, newest-first,
  with a relative timestamp on each entry. `j/k` · `PgUp/PgDn` ·
  `g/G` scroll, `c` clears, `Esc` / `e` dismisses.

### Changed
- **Failures route through `App::push_error` instead of
  `status_msg`.** The status bar gets overwritten by the next hint,
  which used to mean errors disappeared the moment something else
  happened. Lazygit PTY write failures, terminal-stream closures,
  session-start errors, and palette action failures now accumulate
  in the overlay's ring buffer so a busy session's history is
  reviewable.

## [0.6.11] — 2026-05-06

Stream snapshot on session switch.

### Fixed
- **Switching sessions no longer lands you on a fragmented half-frame.**
  The WS terminal stream used to backfill only the last 4 KB of the
  pane log on connect. For embedded TUIs that paint via cursor-position
  escapes (claude code, codex, opencode, k9s, …) 4 KB rarely contains a
  self-consistent screen — the parser ended up mid-redraw, so after
  flipping from `agentum-1` to `agentum-4` you'd see a stray "Musing…"
  line and a floating cursor, with the rest of the pane blank.
- **Fix:** the server now calls `tmux capture-pane -e` on stream open,
  prefixes a `\x1b[2J\x1b[H` (clear + cursor home) and replays the
  current visible frame *before* tailing the log. The vt100 parser on
  the client lands on a clean snapshot every time. The old 4 KB tail
  remains as a fallback for sessions that haven't been wired through
  tmux yet (early start-up window).

## [0.6.10] — 2026-05-06

VS Code-style keybindings + split panes for the TUI, plus `--yolo` on
the CLI, a `terminal` tool, and a fix for the hard-coded session
count in the tree title.

### Added
- **`agentum new --yolo`.** Appends `--dangerously-skip-permissions`
  for `claude`, `codex`, `opencode` (no-op for tools that don't
  recognize it). Brings the CLI in line with the TUI / dashboard
  YOLO toggles.
- **`tool=terminal`.** New executor adapter that launches `$SHELL`
  (falls back to `bash`). Listed in CLI tool help, TUI Tab cycle,
  and the dashboard's New Session datalist so picking "terminal"
  works the same way from every entry point.
- **Alacritty-style pad scroll.** Mouse capture is enabled at startup
  and the wheel / trackpad routes by what the inner program asked for:
  when an alt-screen TUI (claude code, vim, htop, k9s, …) has turned
  on xterm mouse tracking, scroll / click / drag events are forwarded
  to it as SGR escape sequences (DECSET 1006) — so claude code's own
  scrollback works. Otherwise, scroll-wheel ticks drive agentum's
  per-`TerminalPane` offset on top of vt100's 4096-line history, and
  any forwarded keystroke snaps the view back to live (matching
  Alacritty / kitty). A `↑ scroll N` badge in the pane title makes
  the local-scrollback state obvious. `Shift-PgUp` / `Shift-PgDn`
  do the same one page at a time without a pointer. Side-effect:
  native click-drag selection on the host terminal now needs `Shift`
  to bypass app-mode capture (the standard convention).
- **`Ctrl-E` toggles tree ↔ terminal.** Previous behavior only
  released focus to the tree. Now the second press flips back to the
  terminal pane (restoring the correct split side via
  `last_term_side`), so you can ping-pong without reaching for Tab
  or 1/2. Going back to the tree auto-drops fullscreen and unhides
  the sidebar so the tree you jumped to is actually visible.
- **`Ctrl-G` global lazygit toggle.** Plain `g` only toggled the
  pane when the tree was focused — inside the terminal it got
  forwarded to claude code. `Ctrl-G` works from any focus so you
  can pop lazygit mid-prompt without first releasing focus.
- **Split terminals (`Ctrl-\\`).** Mirrors VS Code's "Split Editor".
  Splits the focused terminal pane horizontally (or vertically on
  narrow terminals — &lt;80 cols) and clones the current selection
  into the new right slot. Each side has its own session, parser, and
  WebSocket stream; bytes are routed independently so the panes don't
  blur into each other. `Ctrl-Shift-]` / `Ctrl-Shift-[` cycle through
  Tree → Term → TermRight → Lazygit. Mutually exclusive with the
  lazygit pane (Ctrl-\\ refuses while lazygit is open and vice versa)
  — a 4-column layout doesn't fit on anything narrower than ~160 cols.
- **`Ctrl-W` close split.** Drops the right slot, snaps focus back to
  the left pane.
- **`Ctrl-Tab` flip to last session.** The most-used nav action when
  alternating between two agents. No-op if there's no prior session.
- **`Ctrl-B` toggle sidebar.** VS Code's "toggle primary side bar".
  Hides just the tree column; title and status bars stay.
- **`Ctrl-K` chord prefix** (VS Code parity). Currently bound:
  `Ctrl-K Z` toggles fullscreen ("zen"), `Ctrl-K B` toggles the
  sidebar. Stray prefixes auto-cancel on the next keystroke.
- **`/` filter sessions in the tree.** Press `/` from tree focus to
  start filtering; type to extend, Backspace to trim, Enter to
  commit, Esc to clear. Project groups with no matching sessions
  collapse out. The filter survives session-list refreshes. The
  active filter shows in the tree title bar.

### Changed
- **`Ctrl-K` no longer aliases the command palette.** Use `Ctrl-P`
  or `Ctrl-Shift-P`. The alias is freed so VS Code chord prefixes
  can land.

### Fixed
- **Tree title shows the real session count.** `draw_tree` was
  hard-coded to `" 1 sessions "` in every state. Now uses
  `app.sessions.len()` with proper singular/plural — projects with
  multiple groups no longer report `1 sessions` while listing
  several.
- **Enter on the YOLO checkbox in the new-session dialog now submits.**
  It used to toggle YOLO instead, so you could never spawn from that
  field without first reaching for Tab. Use `Space` to flip YOLO.

### Notes
- Tree-driven j/k targets the side last typed into (`last_term_side`),
  so when you release pane focus back to the tree (Ctrl-E) and start
  navigating, you keep driving the right pane until you focus the
  left explicitly.

## [0.6.9] — 2026-05-06

Capability negotiation for the terminal stream.

### Fixed
- **Resize messages no longer corrupt input on stale daemons.** v0.6.7
  added a `{"resize":…}` envelope on the WS terminal stream, but only
  the running `agentum serve` process actually applies it. Anyone
  whose daemon was still on ≤0.6.6 saw the new client write the JSON
  envelope and the old server forward it to `tmux send-keys` — which
  typed `{"resize":{"cols":N,"rows":N}}` straight into claude's
  prompt.
- The fix is server-side feature advertisement: `/api/health` now
  returns `capabilities: ["resize"]`, and both clients (TUI +
  dashboard) probe before sending. If `resize` is missing from the
  list, the client silently downgrades — degraded layout (the old
  empty-bottom problem) but no corrupted input. The TUI also
  surfaces a status message asking the host to run `agentum update`.

If you're hitting this: run `agentum update` on the host running
`agentum serve` and **restart the daemon** so it picks up the new
binary. `agentum update` only writes the file; the running process
keeps the old code in memory.

## [0.6.8] — 2026-05-05

TUI sidebar polish.

### Fixed
- **Duplicate project groups.** Sessions whose `workdir` differed only
  by a trailing `/` (`/x/proj` vs `/x/proj/`) showed up as two separate
  groups in the sidebar tree. Workdirs are now normalized before
  grouping.

### Changed
- **Sidebar shows project name, not full path.** Groups display the
  basename of the workdir (e.g. `agentum` instead of
  `/home/malloc/Developer/projects/agentum`). The full path is still
  visible in the title bar / status when the project is selected.
- **Sidebar is resizable.** `+` / `-` widen / narrow the tree pane
  in 4-column steps (clamped to 16 ≤ width ≤ 80, and the terminal
  pane keeps a 20-column floor on narrow terminals). Listed in the
  help overlay.

## [0.6.7] — 2026-05-05

Fixes the embedded-pane rendering corruption (overlapping characters,
truncated lines) that anyone using `agentum terminal` or the dashboard
session view would have hit.

### Fixed
- **PTY size mismatch.** The WebSocket terminal stream forwarded
  keystrokes but never told tmux the client's pane size, so detached
  sessions stayed clamped to the 80×24 default while the agentum TUI /
  dashboard rendered them at the actual pane size. Result: claude code
  and similar TUIs drew at the wrong width and characters overlapped.
- **Resize protocol.** Text frames over the WS now carry
  `{"resize":{"cols":N,"rows":N}}`. The daemon flips the tmux window
  to manual sizing and calls `resize-window -x N -y N`. Any frame that
  isn't this envelope still routes to `tmux send-keys -H` for input
  compatibility.
- **Both surfaces wired up.**
  - TUI: tracks the last sent size and pushes a resize on every layout
    change (including Shift-F fullscreen toggle).
  - Dashboard: pushes on `WebSocket.onopen`, on every `ResizeObserver`
    tick, on xterm's own `onResize`, and on `refit()`.

## [0.6.6] — 2026-05-05

Multi-mode installer + TUI fullscreen toggle.

### Added
- **Installer "Both" mode.** Third option in the interactive prompt
  (now the default) covers users who want the dashboard *and* the CLI
  workflow. Same single binary; the installer just prints both
  post-install guides. Available via `--mode both`,
  `INSTALL_MODE=both`, or `agentum update --mode both`.
- **Fullscreen toggle in `agentum terminal` / `lazyagentum`.**
  Shift-F hides the title bar, sidebar tree, and status row so the
  active session pane fills the viewport. Esc exits. Mirrors the web
  dashboard's Shift+F shortcut.

### Changed
- **Non-interactive default is `both`.** Previously CI/no-TTY runs
  silently picked `server`. They now install + show both guides,
  matching the interactive default.

## [0.6.5] — 2026-05-05

Installer fix + `agentum update`.

### Fixed
- **Installer rendered ANSI escapes literally.** `printf '%s' "$C_Y"` was
  emitting the literal string `\033[1;33m` because `%s` doesn't interpret
  backslash escapes (and POSIX `sh` lacks `$'\033'`). Color variables now
  hold a real ESC byte produced via `ESC=$(printf '\033')`, so all
  installer output renders correctly when piped through `sh`.

### Added
- **`agentum update`.** New subcommand re-runs the official installer
  (`releases/latest/download/install.sh`) in-place. Optional
  `--mode server|cli` to skip the prompt; `--force` to reinstall the
  current version.
- **Installer detects existing installs.** When `agentum` is already on
  disk it prints `updating vX → vY` and, if you're already on the latest,
  exits cleanly instead of re-downloading. Override with
  `AGENTUM_FORCE_UPDATE=1`.

## [0.6.4] — 2026-05-06

Interactive installer with mode selection.

### Changed
- **Installer overhaul.** `install.sh` now presents an interactive terminal
  UI asking whether to install the full Control Plane (server + dashboard +
  TLS) or just the lightweight Terminal CLI. Non-interactive/CI modes
  supported via `INSTALL_MODE=server|cli` env var or `--mode` flag.
- **README updated** with the new installer docs, mode comparison, and
  both interactive and CI usage examples.

## [0.6.3] — 2026-05-05

TUI interactivity overhaul. Panels now properly accept continuous input,
global shortcuts work from any focused pane, and notifications surface
session lifecycle events instantly.

### Fixed
- **Key repeat forwarding.** Holding a key is no longer a single press —
  `KeyEventKind::Repeat` events are now forwarded to the pane, fixing the
  "missing keystrokes" glitch that made typing feel laggy.
- **Global shortcuts in panes.** Shift-K, Shift-D, Shift-U, Shift-S now
  trigger kill/delete/start/stop even when the terminal or lazygit pane
  is focused. Shift-only keys bypass `key_to_bytes()` so they never get
  eaten by the running process.
- **Reliable panel cycling.** F5 (next panel) and F6 (previous panel)
  work as universal panel switchers for terminals that swallow Ctrl-]
  and Ctrl-[.

### Added
- **Plain terminal spawn.** Press `t` in the tree or use the palette to
  spawn a `bash` shell as a regular session. The dashboard now includes
  `bash` in the New Session tool suggestions.
- **Event notifications.** Session start, stop, and crash events now show
  a highlighted notification on the bottom-left status bar.
- **Spawn terminal palette action.** "Spawn plain terminal (bash)" in the
  command palette.

## [0.6.2] — 2026-05-05

YOLO mode and dashboard polish.

### Added
- **YOLO mode.** A checkbox in the New Session dialog and a toggle on the
  session detail page that appends `--dangerously-skip-permissions` for
  opencode, Claude Code, and Codex sessions. A `⚡ YOLO` badge appears on
  session cards, canvas tiles, and the detail header so you can see at a
  glance which agents are running with permissions auto-approved.
- **PATCH `/api/sessions/{id}`** endpoint to update session flags (only on
  idle/stopped sessions).

### Changed
- **DirPicker improved.** ArrowRight now drills into highlighted
  directories, and directory entries get separate click targets for
  "enter" (`›`) vs "select" (name). Hint bar updated accordingly.
- **Web mascot refreshed** with a new pixel-art look.

### Fixed
- **DirPicker Enter key** now commits the highlighted directory instead
  of drilling in, matching the double-click behavior.

## [0.6.1] — 2026-05-05

TUI navigation overhaul. The pane focus model fought the user — once inside
the terminal the bottom Input bar took two keystrokes to reach, the chrome
duplicated workdir/theme/palette hints in both top and bottom bars, and
lazygit silently failed against remote sessions. This release rebuilds the
keyboard map around two universal modifiers and trims the surface area.

### Changed
- **Universal panel cycle.** `Ctrl-]` next, `Ctrl-[` previous — work even
  when the terminal or lazygit pane is focused. Replaces the old
  `Ctrl-G` "release" / `F5`/`F6` alternates.
- **Project jump.** `Ctrl-1` … `Ctrl-9` moves the tree cursor to the Nth
  project (workdir) group, expands it, focuses the tree. Then arrows +
  `Enter` to pick a session.
- **Enter focuses the terminal.** Selecting a session leaf with `Enter`
  now also moves focus into the terminal pane, so the common
  pick-and-type flow is a single keypress.
- **Top bar slimmed.** Just `agentum · <session>`. Theme chip and
  `Ctrl-P palette` hint live exclusively in the bottom status bar — no
  more two-bar duplication.
- **Panel border titles** advertise the new shortcuts (`Ctrl-]` next, `2`
  / `3` direct focus).

### Removed
- **Bottom Input pane.** The terminal pane is already an interactive PTY,
  so the "compose-and-send" bar was redundant. `Focus::Input`, `app.input`,
  and `api::send_text` are gone.
- **`Ctrl-G`, `F5`/`F6`, and `i`** keybindings.

### Fixed
- **Lazygit on remote sessions.** `toggle_lazygit` now validates the
  selected session's workdir locally before spawning. If the path
  doesn't exist on this machine (typical when connected to
  `agentum serve` on another host), it falls back to `env::current_dir()`
  and surfaces the substitution in the status bar — instead of silently
  spawning a doomed child that exits milliseconds later.

## [0.6.0] — 2026-05-05

Dashboard UI overhaul — coherent execution of `docs/DESIGN-SYSTEM.md`.

### Changed
- **Coral CTAs.** The "+ New session" button is the coral pill the
  design system specifies, not the activation-blue it accidentally
  inherited. Every primary action now hovers to electric blue —
  the universal activation signal — with the coral reserved for
  resting state.
- **Editorial typography.** New `eyebrow` / `display-1` / `display-2`
  utilities. Section headings now lead with an IBM Plex Mono uppercase
  micro-tag (`Overview`, `Activity`, `Empty`, `Build`) followed by a
  Space Grotesk display headline with `-0.035em` tracking — the
  "compressed, engineered" quality from § 3.
- **Status pill.** Real `<span class="status-dot">` element instead of
  glyph characters. Running state pulses subtly with a halo in the
  neon green token.
- **Sidebar active state.** Coral 2px rail slides in from the left on
  the active nav item; icon shifts to coral; eyebrow group label
  ("Navigation") sits above the link list. Brand wordmark gets a
  living status dot.
- **Topbar.** Sticky, backdrop-blurred over the canvas. Breadcrumb
  uses IBM Plex Mono with the active leaf in electric blue. Action
  buttons (palette / fullscreen / user) hover to the activation
  blue with full-pill geometry on the user chip.
- **Stat tiles.** Bigger Space Grotesk numerals with negative tracking,
  separate from uppercase mono labels. Live totals tint to neon green
  / red only when non-zero — the "signal lights" pattern.
- **Empty state.** Restructured as a refined card with a top-edge
  accent halo, eyebrow tag, display headline, monospace shell
  example, and a primary coral CTA inline.
- **Session card.** Status accent rail on the left edge (green for
  running, red for crashed), eyebrow tag, larger display name,
  coral tool badge, mono workdir, mono `last activity` timestamp.
  Action row uses the new `btn-subtle` 5px rectangles that hover to
  blue (or red for `rm`).
- **Atmosphere.** Faint scanline texture across the canvas, masked
  to fade out at the edges — the "nocturnal command center" feel.
- **Universal hover-to-blue.** Every interactive surface — links,
  pills, ghost buttons, chips — shifts to `#0052ef` on hover. One
  signal, used consistently.

### Removed
- Old `--accent` tinted active states that conflated CTA color with
  activation color. The two are now separate roles (`--cta` and
  `--accent`).

## [0.5.9] — 2026-05-05

### Changed
- **Dashboard restyled to the canonical design system.** Single dark
  palette tokens (`#0b0b0b` canvas, `#212121` surface, `#0052ef`
  electric blue interactive, `#f36458` coral CTA, `#19d600` neon green
  success). Space Grotesk + IBM Plex Mono pulled from Google Fonts to
  match the marketing landing. xterm.js panes now render with the
  same palette.

### Removed
- **Multi-theme dashboard registry.** Dropped the `terminal-dark`,
  `paperlight`, `obsidian-dark`, and `system` themes along with the
  ThemeSwitcher component, the `theme.ts` store, and the
  `Switch theme: …` entries in the command palette. Single canonical
  theme matches the TUI's `sanity` reduction. Server CSP allows
  `https://fonts.googleapis.com` and `https://fonts.gstatic.com` for
  the new font stack.

## [0.5.8] — 2026-05-05

### Added
- **Restored the SvelteKit web dashboard at `dashboard/`.** The
  in-browser dashboard (sidebar, session cards, draggable terminal
  pane, command palette, theme switcher) is back — moved from `web/`
  to a new `dashboard/` folder so the marketing landing at
  `web/index.html` stays untouched. The agentum-server re-embeds
  `dashboard/build/` via `rust-embed` and serves it at `/`. Build with
  `pnpm --dir dashboard install && pnpm --dir dashboard build` then
  rebuild the server.

### Changed
- CSP loosened back to allow inline scripts/styles for the SvelteKit
  bootstrap. The `default-src 'none'` posture introduced in v0.5.7
  was correct for a JSON-only API but broke the embedded SPA.
- CI / release workflows build the dashboard bundle before compiling
  the server. Cache key is `dashboard/pnpm-lock.yaml`.

## [0.5.7] — 2026-05-05

### Added
- **Command palette covers every dashboard action.** The palette
  (`Ctrl-P` / `Ctrl-K`) now exposes the full session lifecycle —
  *new*, *start (up)*, *stop*, *kill*, *delete* — alongside focus,
  lazygit, refresh, and quit. The keybinds (`n` / `u` / `s` / `K` /
  `D`) still work; the palette gives every action a discoverable,
  searchable entry point. New `session-lifecycle` group; included in
  the `>commands` filter.

### Changed
- **`web/` is now fully decoupled from the server.** The frontend lives
  on Netlify (or any static host); `agentum-server` is a JSON-only API.
  Dropped `rust-embed`, the build-time `web/build/` mirror, and the
  static-handler fallback. CSP tightened to `default-src 'none'` since
  the server no longer renders HTML. CI no longer runs `pnpm`. Operators
  who hit the API from a different origin will need to add a CORS layer
  scoped to their Netlify domain.

### Fixed
- **Terminal WS surfaces input errors.** When `agentum_tmux::send_bytes`
  failed (target gone, pipe closed) the byte was dropped silently. The
  WS handler now reports `[input dropped: …]` back to the client and
  closes the socket cleanly if the report itself can't be sent.
- **Release / CI workflows no longer try to build the deleted SvelteKit
  bundle.** `pnpm` setup and `web/pnpm-lock.yaml` cache key were
  failing the v0.5.6 release pipeline immediately. Both jobs are now
  Rust-only.

## [0.5.6] — 2026-05-05

### Fixed
- **Cannot use the inner terminal — silent keystroke drops.** When the
  Term pane was focused but the WS terminal stream wasn't connected
  (`term_in == None`), every keypress was swallowed without feedback.
  The pane felt frozen and there was no way to escape because Ctrl-C
  was also being "forwarded" into the void. The dispatcher now surfaces
  a status-bar error in both failure modes (no stream / stream closed)
  and tells the user how to release focus or quit.
- **Ctrl-Q is a universal hard-quit.** Runs before any pane forwarding
  so the user always has an escape hatch — even when the WS is dead and
  Ctrl-C would otherwise disappear into the SIGINT pipe.

### Changed
- **Removed the top title bar.** The bar duplicated the workdir, theme
  chip, and `Ctrl-P palette` hint already present in the bottom status
  bar. Layout is now a single horizontal split (tree + body) plus the
  status line — matches the agentmux reference and frees a row for the
  panes.
- **Single canonical theme (`sanity`).** The multi-theme registry
  (`system` / `midnight` / `dusk` / `slate` / `paper` / `mono`) has
  been retired in favour of one disciplined dark palette. See
  `docs/DESIGN-SYSTEM.md`. `~/.local/share/agentum/theme` and
  `$AGENTUM_THEME` are still read for back-compat but their value is
  ignored. The `T` key and `@theme` palette filter are gone.
- **Pane title hints advertise `Ctrl-Q`.** Both the Term and Lazygit
  pane titles now show `Ctrl-G release · Ctrl-Q quit` when focused.

### Removed
- The SvelteKit `web/` SPA. Replaced with a single static landing page
  at `web/index.html`. The TUI is now the supported front-end.

## [0.5.4] — 2026-05-05

### Added
- **Global panel-cycle hotkeys that work even with a pane focused.**
  Until now, pressing `1`/`2`/`3`/`4` or `[`/`]` while focus was on the
  Term or Lazygit pane forwarded the keystroke to claude code instead of
  switching panel — you had to press `Ctrl-G` first to release. Two new
  bindings cycle globally, intercepted before the pane-forward branch:
  - `Ctrl-]` / `F6` — next panel
  - `Ctrl-[` / `F5` — previous panel

  These mirror the "next/previous panel" intent from
  [lazydocker](https://github.com/jesseduffield/lazydocker)'s
  keybindings doc. Plain `[` / `]` still work in the tree (and still get
  swallowed by claude code when typing) so we don't break terminal-side
  bracket usage in vim/neovim/etc.

### Fixed
- **Themes now apply to the inner terminal pane.** Empty cells inside
  `tui_term::PseudoTerminal` were rendering at `Color::Reset`, leaving
  "holes" through to the host terminal background on every theme other
  than `system`. Both `draw_terminal` and `draw_lazygit` now pass a base
  style (`bg(panel_bg).fg(fg)`) so untouched cells take the theme's
  panel colours. The `system` theme is unaffected (its `panel_bg` is
  already `Color::Reset` by design).

## [0.5.3] — 2026-05-05

TUI new-session form reaches feature parity with the web `NewSessionDialog`.
v0.5.1 shipped a 4-field form (name / workdir / tool / model); the web
dialog has six controls + a directory browser. Now the TUI matches.

### Added
- **Tool field cycles through suggestions** with Tab. Mirrors the web's
  `<datalist>` of `claude / codex / opencode / aider`. Wraps after the
  last entry; Shift-Tab still walks back through fields normally.
- **Extra args field** parses `key=value` pairs the same way the web
  does: whitespace-separated tokens, `key=true` becomes `--key`, anything
  else becomes `--key=value`. Stripped of leading `--` if you typed it.
- **"Start immediately" toggle** — checkbox-equivalent (`[x]` / `[ ]`).
  Default on, unchecked = create-only (idle).
- **Directory picker sub-overlay**, opened by pressing Enter while on
  the Workdir field. Reads from `/api/fs/list`, shows up to 14 dirs at
  a time, navigates with arrows + Enter, Backspace to go up, `a` to
  accept the currently-listed directory as the workdir, Esc to cancel
  back to the form.

### Internal
- `Overlay::NewSession(NewSessionForm)` boxed (`Box<NewSessionForm>`) to
  silence `clippy::large_enum_variant` — the form grew past 264 bytes
  with the picker state added.
- New `parse_args_field` helper (in `terminal::app`), unit-testable in
  isolation if a regression appears.

## [0.5.2] — 2026-05-05

### Added
- **`system` theme** — inherits the host terminal's actual colour scheme.
  Backgrounds resolve to `Color::Reset` (the host's bg paints through),
  foreground / accent slots use the **named** ANSI colours
  (`Cyan`, `Yellow`, `Red`, …) which terminals colourise from their own
  16-colour palette. Set Alacritty / iTerm / WezTerm / Ghostty to your
  preferred theme and agentum follows automatically — same model
  alacritty uses for its own UI chrome and the one
  [opencode](https://github.com/sst/opencode) ships under the
  `system` name.
- `system` is now the default theme. The previous "system" sentinel
  (which sniffed `COLORFGBG` and resolved to `midnight` / `paper`) is
  gone — `system` is a real registry entry now.
- `auto` accepted as an alias for `system` in saved files and
  `$AGENTUM_THEME` for back-compat.

## [0.5.1] — 2026-05-05

Session lifecycle from inside the TUI. Previously the dashboard could
only watch sessions; you'd drop back to the shell and run
`agentum new …` / `agentum kill …` to actually manage them. Now the
whole loop lives in the dashboard.

### Added
- **`n` — new session**. Opens an inline form with four fields
  (name / workdir / tool / model). Tab/↓ moves between fields,
  Shift-Tab/↑ goes back, Enter creates **and** auto-starts the session.
  The workdir defaults to the currently-selected session's workdir,
  so "another agent in the same repo" is two keystrokes.
- **`u` — start (up)** the selected session, with a confirm dialog.
- **`s` — stop** the selected session (graceful: SIGTERM → SIGKILL after
  5s), with a confirm dialog.
- **`K` — kill** immediately. Confirm dialog uses a red border to
  distinguish destructive actions.
- **`D` — delete**. Confirm dialog. Auto-passes `force=true` when the
  session is currently running so you don't have to stop-then-delete.
- API client gained matching `create_session`, `start_session`,
  `stop_session`, `kill_session`, `delete_session` methods.

### Internal
- `Overlay` enum lost its `Copy` derive (the new `NewSession(Form)` and
  `Confirm(PendingAction)` variants own owned strings). Existing call
  sites only rely on `==` and `Clone`, both still derived.
- Help overlay updated with the five new keys.

## [0.5.0] — 2026-05-05

The remote-access release. Lets you actually use `agentum terminal` against
a daemon on your VPS / Tailscale node — previously the TUI's TLS verifier
only accepted self-signed certs from loopback hosts, so anything off the
local box failed the handshake with `invalid peer certificate: UnknownIssuer`.

### Added
- **SSH-style trust-on-first-use for remote daemons.** First contact with
  a non-loopback `https://` host fetches the server cert, prints its
  SHA-256 fingerprint, and asks you to confirm it matches the line
  `agentum serve` prints on the host TTY. Accepting persists the pin to
  `$XDG_CONFIG_HOME/agentum/known_hosts.toml` (mode 0600). Subsequent
  connects verify silently; mismatches abort loudly.
- **`--fingerprint AB:CD:…`** to skip the prompt (CI / scripted setup).
- **`--insecure`** as the explicit escape hatch for throwaway local
  testing. Prints a yellow `WARNING` to stderr.
- **Per-host token cache.** Single-file `cli_token` is replaced by
  `credentials.toml` keyed by `host:port` so logging into your VPS no
  longer clobbers your local-dev token. Mode 0600.
- **First-run onboarding wizard** in the SPA + Settings pane with a
  remote-access info panel (fingerprint, install hints).
- **`agentum hosts`** subcommand to list and remove pinned hosts.
- New server modules: `headers`, `logging`, `ratelimit`, `routes/cert`.
- New migration `0006_auth_session_expiry.sql` so auth tokens expire.
- `docs/REMOTE-ACCESS.md`.

### Changed
- WS auth path: previously accepted any cert without verification (the
  opposite extreme). Now uses the same per-host pin as HTTP requests.
- Server-side auth, transport, and on-disk surface hardened: stricter
  password hashing, expiring sessions, and tighter cookie / header
  policy.

### Fixed
- `agentum terminal --api https://<remote>:8822` against a host with a
  self-signed cert no longer bails with `UnknownIssuer`. With the TOFU
  pin it Just Works (after the first y/N).

## [0.4.3] — 2026-05-05

Command palette now matches the **Fresh** terminal IDE
([getfresh.dev](https://getfresh.dev/) · [sinelaw/fresh](https://github.com/sinelaw/fresh))
prefix-routing model that I should have looked at before shipping v0.4.0.

### Changed
- **Prefix-routed palette.** `Ctrl-P` (or `Ctrl-K`) opens the picker; the
  first character of the query routes to a slice:
  - (no prefix) — fuzzy across everything
  - `>` — commands only (focus / theme / lazygit / refresh / quit)
  - `#` — sessions only (Fresh's buffer-switcher analog)
  - `@` — themes only
- The active mode is shown as a chip next to the query (`commands`,
  `sessions`, `themes`, `all`) and as a suffix in the title bar.
- Bottom of the overlay shows a Fresh-style hints line:
  `> commands  # sessions  @ themes  ↑↓ move  ⏎ run  Esc close`.
- Multi-token queries match independently — typing `theme mid` finds
  "Theme: midnight" the same way Fresh's `feat group` matches
  `features/groups/view.tsx`.

## [0.4.2] — 2026-05-05

### Fixed
- One more pre-existing clippy issue surfaced by rustc 1.95's newer
  `collapsible_match` lint (CI uses `dtolnay/rust-toolchain@stable`,
  local dev was on 1.94 which didn't ship this lint yet). Collapsed the
  command-palette `KeyCode::Char` arm into a match-guard form.

## [0.4.1] — 2026-05-05

### Fixed
- Two pre-existing clippy errors that surfaced under CI's strict
  `clippy --all-targets --all-features -- -D warnings`:
  - `agentum_server::routes::fs::list` used `sort_by` where
    `sort_by_key` is more concise.
  - `agentum_server::routes::sessions::stream_session` had an `if !b.is_empty()`
    inside a `Some(Ok(Message::Binary(b)))` arm — collapsed into a guard
    on the match.
- These were the last gating issues for a green CI badge after v0.4.0.

## [0.4.0] — 2026-05-05

The IDE-feel release. Real backgrounds, a VSCode-style command palette
in the TUI, and a registry of named themes you can switch from anywhere.

### Added
- **Command palette in the TUI.** `Ctrl-P` (or `Ctrl-K`) from anywhere —
  including with a pane focused — opens a fuzzy picker over every action
  in the dashboard: focus jumps, refresh, lazygit toggle, theme picks,
  and every active session. Type to subsequence-filter, ↑/↓ to move,
  Enter to run, Esc to close. The action list rebuilds each frame so
  dynamic entries (sessions, themes) are always live.
- **Five named themes**, each with real layered backgrounds (body, panel,
  surface, chrome) instead of border-color swaps:
  - `midnight` — Tokyo-Night-inspired deep blue, soft fg, blue/violet
    accents (default)
  - `dusk` — One-Dark-style warm charcoal
  - `slate` — cool charcoal with neon cyan + magenta accents
  - `paper` — warm-paper light scheme
  - `mono` — high-contrast black/white
- The "system" theme name is preserved as a sentinel that resolves to
  `midnight` on dark hosts and `paper` on light ones (sniffed via
  `COLORFGBG`).

### Changed
- **TUI palette overhaul.** `Palette` now carries `body_bg`, `panel_bg`,
  `surface_bg`, `chrome_bg`, `fg_strong`, `accent_alt`, `subtle`, and
  `cursor_fg` — and every panel block paints a real background so the
  TUI looks like an IDE, not a borders-on-default shell.
- The status bar gets fully chip-styled (workdir / tool / connection /
  errors / lazygit / theme / Ctrl-P hint / help). Title bar shows the
  active theme as a pill.

### Fixed
- Pre-existing clippy warning in `agentum_server::routes::fs::resolve`
  (`if_same_then_else`) — collapsed the `is_empty()` and `== "~"` arms
  so CI's `-D warnings` clippy step actually passes.

## [0.3.1] — 2026-05-05

### Fixed
- The README's recommended installer one-liner —
  `curl -fsSL https://github.com/.../releases/latest/download/install.sh | sh`
  — has been broken since v0.1.0 because `install.sh` was never attached
  to releases (only the per-target tarballs and `SHA256SUMS` were).
  `release.yml` now uploads `scripts/install.sh` alongside the tarballs.

## [0.3.0] — 2026-05-05

The interactive-terminal release. The dashboard's terminal pane is now
two-way (you can actually use claude code from inside `agentum terminal`),
the lazygit side pane works, the SPA gets a fullscreen mode + multi-pane
canvas, and the TUI ships dark/light/system themes plus lazydocker-style
panel navigation.

### Added
- **Bidirectional terminal stream.** `WS /api/sessions/{id}/stream` now
  accepts inbound binary/text frames and forwards them to the tmux pane via
  `tmux send-keys -H` (raw hex bytes). xterm.js in the SPA and the ratatui
  TUI both round-trip keystrokes — including Ctrl-C, arrow keys, and
  paste — into the running pty.
- `agentum_tmux::send_bytes` — chunked hex-pair `send-keys -H` helper used
  by the WS handler so every byte is delivered literally without tmux's
  key-name parsing in the way.
- **TUI themes.** `dark` / `light` / `system` palettes in
  `agentum/commands/terminal/theme.rs`. `system` sniffs `COLORFGBG` and
  falls back to dark. Persisted to `$XDG_DATA_HOME/agentum/theme`,
  overridable via `$AGENTUM_THEME`. `T` cycles at runtime.
- **Lazydocker-style navigation in the TUI.** `1`/`2`/`3`/`4` jump to the
  tree / terminal / input / lazygit panel; `[` and `]` (or `Tab` /
  `Shift-Tab`) cycle. Panel titles are numbered for discoverability.
- **Fullscreen mode in the SPA.** `Shift+F` toggles a chrome-less layout
  (sidebar + topbar hidden, page body fills the viewport). Persisted in
  localStorage, exitable via `Esc` or the floating `⤢ exit` button.
- **`/terminals` canvas route.** Multi-pane terminal arrangement with
  draggable/resizable panels and per-session layout persisted in
  localStorage (`agentum_canvas_layout_v1`). `bringToFront` z-order,
  optional maximize.
- New `TerminalPanel.svelte` wrapper around `Terminal.svelte` for canvas
  use. Sidebar gains a Terminals entry; Topbar gains a fullscreen button.

### Changed
- **TUI key model.** `Ctrl-C` now only quits when no pane is focused;
  inside the terminal or lazygit pane it's a real SIGINT to whatever's
  running. `Ctrl-G` releases focus from either pane back to the tree
  (previously lazygit-only).
- **Server stream loop refactor.** Pane-log tailing moved to a dedicated
  task feeding an mpsc; the request loop multiplexes inbound socket frames
  and outbound pane bytes so a chatty pane never starves keystrokes (and
  vice versa).
- `password-hash` enables the `getrandom` feature so first-boot credential
  generation works on hosts without an explicit RNG dependency in scope.

### Fixed
- **Lazygit side pane was input-dead after the first keystroke.**
  `LocalPty::write` called `portable_pty::take_writer` on every key, but
  that API is one-shot — every call after the first failed with
  `cannot take writer more than once`. The writer is now taken once at
  spawn time and cached behind a `Mutex`, with a `flush()` after each
  write so keys can't get stuck in an internal buffer.
- The remote terminal pane in the TUI was effectively read-only — pressing
  keys with `Focus::Term` did nothing. The pane now forwards every key
  through the WS to the running process.

## [0.2.2] — 2026-05-05

### Fixed
- `x86_64-apple-darwin` release job was stuck queueing on `macos-13` runners
  (which GitHub is deprecating and queues heavily). Moved to `macos-14`
  (Apple Silicon) and cross-compile to Intel.

## [0.2.1] — 2026-05-05

### Fixed
- Release builds for macOS (both Intel and Apple Silicon) and `aarch64`
  Linux were failing because `sqlx-sqlite-unbundled` couldn't link against
  the runners' system libsqlite3 (Apple's shipped sqlite is too old; the
  cross-build container has no `sqlite3.h`). Switched the `sqlx-sqlite`
  feature to `sqlite` (bundled) so cargo compiles libsqlite3 inline.
  Adds ~30s to the first build but produces a fully self-contained binary
  with no system sqlite dependency.

## [0.2.0] — 2026-05-05

Adds an interactive terminal dashboard alongside the existing browser SPA,
plus a username/password auth refactor and a working `curl | sh` install path.

### Added
- `agentum terminal` (alias `agentum tui`) — ratatui-based terminal dashboard
  that talks to a running `agentum serve` over the same HTTP/WS API the
  Svelte SPA uses. Workdir-grouped session tree on the left, live ANSI
  terminal pane on the right, message input at the bottom, status bar with
  connection state + error counter. Mirrors the agentmux look on top of
  agentum's existing data model.
- `lazyagentum` shim binary — drops you straight into the dashboard with
  no subcommands, the way `lazygit` works.
- `--api <URL>` flag for pointing the dashboard at a non-default daemon.
- `agentum new --pick / -P` — interactive workdir picker via `lf`.
- New session dialog in the web UI with directory browsing.

### Changed
- **Auth refactor.** Replaces the static `$XDG_DATA_HOME/agentum/auth_token`
  file with username/password + Argon2id-hashed credentials and per-user
  session tokens stored in `auth_sessions`. First run prompts for
  registration. Session tokens are cached in
  `$XDG_DATA_HOME/agentum/cli_token` (chmod 0600) for subsequent CLI runs.
- `agentum` is now a `lib` + `bin` crate so multiple binaries
  (`agentum`, `lazyagentum`) share the same CLI plumbing.

### Fixed
- `scripts/install.sh` now matches the asset names produced by
  `release.yml` (Rust target triples) and verifies against the published
  `SHA256SUMS` file.

### Notes
- The TUI accepts the daemon's self-signed cert only for localhost. Remote
  HTTPS daemons (Tailscale, SSH-tunneled, etc.) need either an `--insecure`
  opt-in (not yet implemented) or trust-on-first-use cert pinning.
- Lazygit-style side pane (local PTY) modules are present but not yet
  wired into the layout; ship target is v0.3.0.

## [0.1.0] — 2026-05-04

The first release. Implements every phase of the original PRD v2.

### Sessions
- `agentum new / up / down / kill / rm / ls / ps / open / tail / send / keys`
  CLI surface.
- tmux adapter: `has_session`, `new_session`, `kill_session`, `capture_pane`,
  `send_keys`, `pipe_pane`, `pane_pid`, `graceful_stop` (SIGTERM → SIGKILL
  after 5 s → `kill-session`).
- pane bytes captured to `$XDG_CACHE_HOME/agentum/sessions/<id>.log` via
  `tmux pipe-pane`.
- Live terminal stream over `WS /api/sessions/{id}/stream` rendered by
  xterm.js with theme-aware palette swap.
- `POST /api/sessions/{id}/send` with `{text?, keys?, append_enter?}`.

### Executor adapters (Phase 2b)
- `ToolAdapter` trait with built-in adapters for **Claude**, **Codex**,
  **Gemini**, **Hermes**, plus a **passthrough** for any other binary.
- Each adapter declares its `compact_trigger` and `crash_signatures` so the
  watchdog speaks the right dialect.

### Watchdog
- Per-session reconciler ticks every 5 s. Applies the watchdog rules:
  `Context low.*<\s*50%` → `/compact` (5 min cooldown), crash signatures →
  `Crashed` + `session.crashed{reason:pane_exited|signature}` event.
- Events broadcast on a `tokio::sync::broadcast` bus and persisted to the
  `events` table.

### Board (Phase 7)
- `board_items` table with auto-derived `AG-N` keys.
- Atomic claim via `UPDATE … WHERE claimed_by IS NULL` — second claimer
  gets 409.
- `/api/board` REST surface; native HTML5 drag-drop kanban with optimistic
  local moves.

### Notes + Channels (Phase 8)
- Markdown notes via CodeMirror 6, auto-save on 800 ms idle / blur.
- 1:1 channels between sessions with canonicalized pair (UNIQUE).
- `POST /api/channels/{id}/messages` emits `message.posted` on the existing
  event bus — UI receives messages live without a per-channel WS.

### Frontend (Phases 3, 4, 9)
- SvelteKit 2 + Svelte 5 SPA, embedded into the Rust binary via `rust-embed`.
- **Terminal Dark** + **Paperlight** + **System** themes (palettes
  verbatim). Persisted in localStorage.
- `⌘K` / `Ctrl+K` command palette over pages, sessions, board items, notes,
  and the built-in commands. `?` opens a keyboard-shortcut sheet.
- Service worker pre-caches the immutable bundle; SPA shell falls back from
  the cache when offline.

### Auth + TLS (Phase 5)
- 32-byte URL-safe base64 bearer token at
  `$XDG_DATA_HOME/agentum/auth_token` (chmod 0600), rotated with
  `agentum auth rotate`. Middleware re-reads per request.
- rustls TLS via axum-server with self-signed cert + key generated by rcgen
  on first boot.
- Plain-HTTP cert-server on `:8823` returning the PEM for trust-on-first-use.
- WS auth accepts `Authorization: Bearer …` OR `?token=…` query param.

### Storage
- SQLite (WAL, `synchronous=NORMAL`, FK enforcement) at
  `$XDG_DATA_HOME/agentum/db.sqlite`.
- Migrations:
  - `0001_initial.sql` — `sessions`
  - `0002_events.sql` — `events`
  - `0003_board.sql` — `board_items`
  - `0004_notes_channels.sql` — `notes` + `channels` + `messages`

### Build defence
- `.cargo/config.toml` sets `CC=/usr/bin/gcc` as a non-overriding default so
  rustls's crypto backends (ring + aws-lc-rs) compile through broken `cc`
  shims that may exist on a contributor's `$PATH`.

### Known limitations (post-v0.1)
- No multi-user / RBAC. Single-user, single bearer token only.
- No SaaS hosting / cloud sync.
- No native mobile app — PWA only.
- No plugin / extension marketplace.
- No cross-machine cluster orchestration.

[0.1.0]: https://github.com/mateocerquetella/agentum/releases/tag/v0.1.0
