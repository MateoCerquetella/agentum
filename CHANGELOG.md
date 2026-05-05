# Changelog

All notable changes to agentum are recorded here. The format is loosely based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
