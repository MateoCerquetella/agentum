# Changelog

All notable changes to agentum are recorded here. The format is loosely based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
