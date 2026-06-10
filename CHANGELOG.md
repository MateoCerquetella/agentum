# Changelog

All notable changes to agentum are recorded here. The format is loosely based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.13.5] — 2026-06-10

### Changed
- **Password SSH hosts no longer need `sshpass` installed.** The daemon used to
  shell out to an external `sshpass` binary to feed a host's password to `ssh` —
  so a password host on a machine without `sshpass` failed at connect time with
  a bare *"No such file or directory (os error 2)"*. Password auth now goes
  through OpenSSH's own `SSH_ASKPASS` helper (`SSH_ASKPASS_REQUIRE=force`),
  which the stock `ssh` on every modern macOS/Linux supports — nothing to
  install. The password travels in the child process environment instead of on
  the command line, so it no longer appears in `ps` (a small security
  improvement over `sshpass -p`). `agentum doctor` no longer probes for
  `sshpass`.

## [0.13.3] — 2026-06-09

### Changed
- **Marketing landing page redesign** (`web/`): a new animated background and
  design-system polish. The app itself is unchanged from 0.13.2; `web/` deploys
  to Netlify independently of the desktop/CLI artifacts.

## [0.13.2] — 2026-06-09

### Fixed
- **Files in remote (SSH) workspaces open again.** Opening a file in an SSH
  workspace showed *"Unable to load file — No such file or directory (os error
  2)"*: the file *tree* listed over SSH, but the file *read* fell back to the
  local filesystem and ENOENT'd on the remote path. Added a host-aware
  `GET /api/fs/read` endpoint (reads over the connection via the daemon) and
  routed the editor's read through it, mirroring how directory listing already
  works.
- **The status-bar I/O-speed chip remembers its host.** The saved host was
  clobbered on reload: SSH host labels hydrate asynchronously, so the saved SSH
  choice was momentarily "unknown" and a fallback reset it to local *and
  persisted that*. The selection is now preserved and the chip snaps back to the
  chosen host once it reappears.

## [0.13.1] — 2026-06-09

### Fixed
- **macOS desktop bundles build again.** The macOS CI runner's node@22 aborted
  (SIGABRT) while closing file handles during the Vite UI build, which failed
  `beforeBuildCommand` and blocked every macOS release since v0.11.0 (Linux +
  CLI artifacts were unaffected). The desktop UI now builds under bun's runtime
  (`bun run --bun build`), sidestepping the node bug. No app behavior changes.

## [0.13.0] — 2026-06-09

A cleanup-and-repair release: two unused surfaces are gone, several
half-wired features now actually work, and a class of worktree errors that
broke source control is self-healed.

### Removed
- **Automations.** The scheduled/cron automations feature (page, sidebar
  entry, dispatch hooks, and backend commands) was removed — it was unused.
- **Toolbox + Space Analyzer.** The Toolbox dropdown and the disk-usage
  "Space Analyzer" were removed. **Skills** moved into the Help menu, where
  it stays one click away.

### Added
- **CLI registration actually registers now.** "Register `agentum`" locates a
  real `agentum` CLI binary (next to the app, on `PATH`, or in cargo/Homebrew
  dirs) and symlinks it into `/usr/local/bin` (macOS) or `~/.local/bin`
  (Linux). When no binary exists it stays honest and tells you how to install
  one, instead of the old fixed "not available in this build" stub.
- **Add Project: paste-a-path fallback.** The add-project dialog gained a
  manual folder-path field, so adding a project still works when the native
  folder picker can't open (e.g. Linux without a desktop portal).

### Fixed
- **"Branch compare failed" / `400 workdir does not exist`.** A session whose
  `…/.claude/worktrees/<name>` directory went missing (pruned out-of-band or a
  registry row that outlived its checkout) is now recreated from the parent
  repo instead of hard-failing — restoring branch compare and every git/
  terminal operation that opens the worktree. Applied at both session create
  and start.
- **Search shortcuts work again.** The command palette and quick-open
  shortcuts relied on an Electron main-process key path that the Tauri shell
  never replaced, so none of them fired. They're now handled in the renderer.
- **Send feedback reaches a human.** "Send feedback" opens a prefilled GitHub
  issue on the project repo instead of POSTing to a backend that wasn't wired
  up and always failed.
- **Docs link.** The Help → Docs link now points at the project README on
  GitHub (there is no hosted docs site).

## [0.11.0] — 2026-06-09

A desktop-focused release: remote SSH hosts get password auth and much faster
file/terminal operations, tmux-backed sessions become persistent and visible
throughout the UI, and the Linear connection is now live.

### Added
- **SSH hosts support password authentication.** Add a host with a password
  instead of a key. The old relay keep-alive toggle was removed.
- **Host-first New Workspace.** The New Workspace flow now leads with a host
  selector, guards against pointing at a non-git directory, and surfaces a
  friendly error when a workspace can't be created (spec 006).
- **Opt-in tmux persistence with silent auto-reattach.** Sessions can opt into
  running under tmux and will silently re-attach to the live tmux pane across
  reloads and reconnects instead of being killed and respawned (spec 005-C).
- **tmux is visible everywhere it matters.** Terminal tabs and host headers
  show a tmux glyph when a session is backed by a live tmux pane, with a hover
  tooltip listing the running sessions on that host. The status bar gained a
  per-host I/O speed chip with a host selector so you can watch throughput per
  machine.
- **Cleaner remote-host display.** Host IPs are hidden in the sidebar header
  and revealed on hover.
- **Linear integration is live.** The Linear connection surface is wired to the
  Linear GraphQL API, backed by an on-disk credential store.

### Fixed
- **Remote operations are much faster.** The remote file tree is host-aware and
  SSH connections are pooled via ControlMaster — with the control socket kept in
  a private `$XDG_RUNTIME_DIR`/`$HOME` dir rather than `$TMPDIR` — so repeated
  remote git/file calls reuse one connection.
- **Persistent terminals survive tab switches and reconnects.** Hidden panes are
  recovered from a snapshot instead of dropping their output, and the server
  reattaches to a live tmux session instead of killing + respawning it.
- **The per-host tmux glyph is now truthful and live.** It reflects actual open
  tmux panes (not persisted session rows), refreshes by polling, and maps
  sessions to their sidebar host from the repo list.
- **Hookless agents no longer look stuck.** Agents without status hooks (e.g.
  OpenCode) are detected as Working via active-redraw change detection.
- **Desktop polish.** The macOS dock icon is no longer oversized (the artwork is
  padded to ~80%), the custom topbar is draggable again
  (`data-tauri-drag-region` + `core:window:allow-start-dragging`), built-in
  notification sounds play instead of 404-ing (served via a Vite glob), and
  renderer error-boundary reports are persisted to `renderer-errors.log`.

### Internal
- **Repaired the release pipeline.** CI still built the standalone `dashboard/`
  crate, which was removed in the v0.10.11 thin-shell refactor — so desktop
  bundle builds failed and the v0.10.11 `.dmg`s had to be attached by hand. CI
  now builds the embedded Tauri UI (`crates/agentum-desktop/ui`) with bun for
  the mac/linux desktop apps, and `tauri-build` features are pinned to `[]`.

## [0.10.11] — 2026-06-06

### Added
- **Edit and delete SSH hosts from the terminal UI.** The `Ctrl-H` hosts
  overlay can now edit a host's connection settings in place (`e`) and
  remove a host (`d`) behind a confirmation prompt — both also reachable
  from the command palette (`Ctrl-P` → "Edit host…" / "Delete host…").
  Editing pre-fills the form from the host (including the stored password
  for password auth) and preserves the host's id, so any sessions already
  attached to it stay put. Backed by a new `PUT /api/hosts/{id}` route.

### Fixed
- **Deleting an SSH host is now discoverable and safe.** Delete was bound
  to `d` but never advertised, had no confirmation, and surfaced the
  daemon's "host still has sessions" rejection as a surprise error. It's
  now listed in the overlay's help line and the command palette, and asks
  before removing.

## [0.10.10] — 2026-06-01

### Fixed
- **macOS desktop app no longer opens to a blank/unresponsive window.** The
  window loads the dashboard from the in-process daemon over
  `http://127.0.0.1:<port>`, but macOS App Transport Security blocks plain
  http in WKWebView — so the webview loaded nothing and the app looked hung.
  The macOS bundle now ships an Info.plist that allows local/loopback http
  loads (`bundle.macOS.infoPlist` + `exceptionDomain`).
- **macOS daemon finds tmux/git when launched from Finder.** A Finder-launched
  `.app` inherits a minimal PATH without Homebrew; the desktop binary now
  prepends `/opt/homebrew/bin` and `/usr/local/bin` so the bundled daemon can
  spawn agent sessions.

## [0.10.9] — 2026-06-01

### Fixed
- **Linux desktop release builds link again.** Tauri's `tray-icon` feature
  needs `libayatana-appindicator3-dev` at link time; the runner didn't have
  it, so the v0.10.8 Linux desktop build aborted with "Can't detect any
  appindicator library". Added the dep, set `APPIMAGE_EXTRACT_AND_RUN` for the
  (FUSE-less) AppImage tooling, and made AppImage best-effort so the release
  can't be blocked by it.

### Added
- **Installable desktop apps.** Releases now ship native installers built
  with Tauri: a macOS `.dmg` (Intel + Apple Silicon), a Linux `.deb` and a
  best-effort `.AppImage`, plus the **raw Linux desktop binary** (the artifact
  that runs on Arch and other non-deb distros), alongside the CLI tarballs.
- **`install.sh` asks what you want.** The installer now offers a choice —
  **CLI + terminal UI** (default) or the **desktop app** — and installs the
  right artifact for your platform (`.dmg` → /Applications on macOS,
  `.AppImage` → PATH or `.deb` → dpkg on Linux). Skip the prompt with
  `--cli` / `--desktop` or `AGENTUM_INSTALL_KIND`.

## [0.10.7] — 2026-06-01

### Fixed
- **Desktop release builds compile again.** The `Toggle DevTools` menu item
  called `WebviewWindow::{open,close,is}_devtools`, which are gated to
  `debug_assertions` or Tauri's `devtools` feature — so the `--release`
  build of `agentum-desktop` failed on the macOS runner and broke the
  v0.10.6 release. The `devtools` feature is now enabled, keeping the menu
  item working in shipped builds.

### Added
- **Desktop shell on Tauri 2.** `agentum-desktop` now boots the daemon
  in-process on a free loopback port and opens a native Tauri window on the
  embedded dashboard, with a system tray (hide-to-tray on close), a native
  menu bar (File/View/Help), the updater plugin, window-state persistence,
  and a `--headless` mode that runs the daemon with no window.
- **Native notifications.** The desktop app subscribes to the daemon's
  `/api/events` bus and raises OS notifications when an agent finishes, is
  awaiting input, or crashes — even while hidden to the tray.
- **GitHub/GitLab integration.** A per-session panel lists open PRs/MRs,
  issues, and CI checks for the session's branch, and can open a PR/MR from
  the current branch. Backed by `/api/sessions/{id}/forge/*` (origin
  detection + GitHub/GitLab REST). A personal access token is stored
  locally on the daemon (`<data_dir>/forge.json`, 0600) and never sent to
  clients.
- **Richer diff viewer.** The session git panel now renders a CodeMirror 6
  side-by-side diff with per-language syntax highlighting (replacing the
  plain-text unified diff), and supports staging/unstaging files against
  the real index before committing, via new
  `/api/sessions/{id}/git/{file,stage}` endpoints.

## [0.10.5] — 2026-05-30

### Fixed
- **New Session is local-first again — it no longer strands you on a
  remote host.** Pressing `n` while the sidebar cursor sat on an SSH host
  (e.g. `omarchy`) seeded the form's Host to that remote box, but the
  working directory fell back to the laptop's `$HOME`. The folder picker
  then tried to list a Mac path *on the SSH host* and failed with `400 Bad
  Request — remote fs: ssh/tmux exited with status …`, so you couldn't
  start a local session at all. New Session now defaults to **this
  machine** (local host, local `$HOME`) whenever no session is selected;
  targeting a host is one explicit `Tab` away in the merged Host field.
  Opening New Session from a *selected* remote session still inherits that
  session's host and workdir (which are consistent), as before.

## [0.10.4] — 2026-05-30

### Fixed
- **Remote workdir listing no longer 400s on fish/zsh login shells.**
  Opening New Session against an SSH host (or Tab-cycling onto one) fetches
  `$HOME` on that box via `/api/fs/list`. That listing script is POSIX
  `sh`, but it was handed straight to the remote *login* shell — so a host
  whose user logs into **fish** or zsh rejected its `case` / `$(…)` /
  `${#}` syntax, and the form showed `couldn't list host home: 400 Bad
  Request — remote fs: …` even though the host reported "ready" (readiness
  already wrapped its probe in `sh -c`; this path didn't). The script is
  now wrapped in `sh -c`, matching every other remote command (readiness,
  bootstrap, agent install, tmux), so the remote login shell no longer
  matters.

### Changed
- **New Session merges "Servers" and "Host" into one picker.** The form
  had two adjacent fields — a *Servers* (daemon) field and a *Host* (SSH)
  field — which read as redundant now that the sidebar already folds the
  local machine and every SSH host into one HOSTS list. They're now a
  single **Host** field that cycles `this machine + the SSH hosts the
  daemon drives`, mirroring the sidebar exactly. "this machine" renders as
  the daemon's hostname and is just a peer of any SSH host in the wheel;
  the local case keeps the worktree default. Finishes the servers→hosts
  merge that 0.10.2 started in the sidebar.
- **Hosts overlay: `Enter` closes a host that's already ready.** Once a
  host checks out green, `Enter` dismisses the overlay instead of
  re-running the probe — the common "added it, watched the checks pass,
  done" flow. `t` still forces a re-check, and a not-yet-checked or
  not-ready host still probes on `Enter`. The detail pane shows a `press
  Enter to close` hint when the host is ready.

## [0.10.3] — 2026-05-30

### Added
- **Folder picker for the SSH key path.** In the add-host form (Host
  Manager → `a`), pressing `Enter` on the **Key path** field (key/agent
  auth) opens the same directory picker the New Session "Working
  directory" field uses, browsing the daemon's filesystem — navigate with
  ↑/↓ and →/Enter, ←/Backspace to go up, `a`/`s` to accept. Shares one
  `dir_picker_step` tree-walker with the workdir picker.

### Fixed
- **New Session no longer defaults to the wrong host.** Opening the form
  now seeds the **Host** field from the current context — the selected
  session's host, or the host highlighted in the sidebar (e.g. `omarchy`)
  — instead of always falling back to the local daemon. The seeded
  working directory is resolved as `$HOME` on the *target* host so it's a
  real path there.

## [0.10.2] — 2026-05-29

### Changed
- **Servers and hosts are one concept now: hosts.** The TUI no longer has
  a separate "servers" (remote-daemon) list. There is a single local
  daemon; every other machine is an SSH **host** the daemon drives. The
  sidebar's top section is **HOSTS** (this machine + each SSH host),
  sessions group by host, and the add flow (`a` / `Ctrl-S`) opens the SSH
  host form (Name · User · Hostname · Port · Auth) — the User/Port fields
  the old "add server" form lacked. This removes the dead-end where a
  daemon-less remote added as a "server" sat as "unreachable — no pinned
  fingerprint" with no in-TUI recovery. Removed the multi-daemon fanout,
  remote-daemon TLS pinning/TOFU, per-daemon tokens, profile switching,
  and the endpoint switcher from the TUI. The hosts overlay gained a `d`
  key to remove an SSH host.

### Added
- **Desktop shell (`agentum-desktop`).** The placeholder Tauri crate is now
  a real native binary: it boots `agentum-server` in-process on a free
  loopback port (plain HTTP, auth disabled — only this machine can reach a
  loopback bind), waits for it to start listening, then opens a native
  webview window (wry + tao) on the embedded dashboard. GUI deps are
  isolated to this crate so the CLI/Linux release is unaffected. macOS
  binary ships in the release tarballs; run with `agentum-desktop`.

### Removed
- Dead remote-daemon code left over from the servers→hosts merge
  (multi-profile connect/fanout helpers, the synthetic-loopback sidebar
  row, the `RemoveServer` confirm action). Workspace builds with zero
  dead-code warnings.

## [0.10.1] — 2026-05-29

### Fixed
- **Local server no longer demands a login.** A loopback daemon runs
  with `--no-auth` by default, but the TUI's *non-interactive* connect
  paths (the sidebar fanout `try_connect_profile` and the implicit
  `try_connect_loopback`) only looked for a cached bearer token — finding
  none, they flagged the server "login needed" and never built a client.
  Both now probe `/api/auth/status` first and connect with the `no-auth`
  bearer when the server has auth disabled, matching the interactive
  path. This also fixes **agents showing as unrecognized** on the local
  server: with no client, `/api/agents` was never queried, so the New
  Session form had no availability data.
- **Installer no longer appears to hang.** The expose / autostart
  prompts used `read` with no timeout, so a fresh `curl … | sh` install
  could sit blocked waiting on input. `read_input` now takes a `-t`
  timeout (default 60s, override with `AGENTUM_INSTALL_READ_TIMEOUT`) and
  falls back to the displayed default, so the installer self-resolves to
  the recommended choice instead of looking stuck.

## [0.10.0] — 2026-05-29

### Added
- **Add SSH hosts from the TUI.** The Ctrl-H hosts overlay now has an
  add-host form (`a`): Name · User · Hostname · Port · Auth · Secret. The
  `Auth` field toggles (Space / ←→) between **key/agent** and **password**.
  On save it `POST`s the host and drops you back on it so Enter/`t` checks
  readiness and `i` installs the missing deps + agent CLIs — full TUI
  parity with `agentum hosts add`. No more dropping to the CLI to register
  a machine.
- **SSH password auth.** New `SshAuth::Password` end to end (core enum,
  store migration `0019` adding `hosts.secret`, and `host_runtime` shelling
  through `sshpass`). Password hosts run ssh with `BatchMode=no` and forced
  `PreferredAuthentications=password`; key/agent hosts are unchanged. The
  password is stored at rest in the local SQLite DB only, never sent to
  remotes or printed (`hosts list` shows just `password`). **Requires
  `sshpass` on the daemon machine** — `agentum doctor` now reports whether
  it's present (a soft check; only matters for password hosts).

### Changed
- **One install, no "mode" question.** `install.sh` no longer asks "will
  this machine run agents? / connect to a remote" — there is only one
  install: this machine runs the daemon. To control other machines you run
  `agentum hosts add` (or the TUI add-host form), which SSHes in and
  provisions them; agentum is never installed on the remote. The dead
  `agentum update --mode server|cli|both` flag was removed (still accepted
  and ignored by `install.sh` for back-compat).
- **No login on a personal machine.** `agentum serve` now defaults to
  no-auth when bound to loopback (only this machine can reach it, so a
  username/password login is pure friction). Auth stays required when bound
  to a LAN / `0.0.0.0` address unless you pass `--no-auth`. The installer no
  longer runs an `agentum auth setup` prompt; LAN-exposed daemons still
  prompt to create an admin account on first dashboard load.

## [0.9.3] — 2026-05-29

### Fixed
- **Claude sessions failed to spawn.** Every Claude session aborted with `error: unknown option '--hook-post-tool-use'` — Claude Code has no such CLI flag; hooks are configured through settings. The daemon now registers the `tool_done` PostToolUse hook via `--settings` (which *adds* to the user's settings rather than replacing them). Verified end-to-end. (`agentum-server`)

### Security
- **Bootstrap password now uses the OS CSPRNG.** The loopback `local` user's password was generated from a `SystemTime`/pid/stack-address-seeded splitmix64 PRNG — predictable to a local attacker. Replaced with `getrandom` (OS CSPRNG). (`agentum-cli`)
- **`--insecure` confined to loopback.** Disabling TLS certificate verification (`--insecure` flag or a profile's `insecure = true`) is now refused for non-loopback hosts; remote daemons must pin a certificate with `--fingerprint` instead. Prevents silently accepting any cert on a MITM-exposed network path. (`agentum-cli`)

## [0.9.2] — 2026-05-29

### Fixed
- **macOS x86_64 release binaries.** `release.yml` installed the build target onto the floating `@stable` toolchain, but `rust-toolchain.toml` pins the build to 1.94.1 — so the `x86_64-apple-darwin` cross-target on the arm64 mac runner had no std and failed with `can't find crate for core`, silently blocking the GitHub Release for v0.9.0 and v0.9.1. The release workflow now pins the toolchain to 1.94.1 (matching `ci.yml`) and adds the matrix target to it. First release to actually publish the v0.9.x binaries.

## [0.9.1] — 2026-05-29

### Fixed
- **Reproducible release binaries.** The `v0.9.0` tag was cut with a `Cargo.lock` still pinning the workspace crates at `0.8.13` while the manifest was already at `0.9.0`. Regenerated the lockfile so published binaries build reproducibly from a matching manifest + lock. No functional changes since `v0.9.0`.

## [0.9.0] — 2026-05-29

### Added
- **Claude account usage tracking.** The daemon now exposes `/api/usage` with plan quota, band colors (🟢<70% / 🟡70–90% / 🔴>90%), refresh interval, and graceful degradation when the upstream API is unavailable.
- **Consolidated hosts install flow.** Removed the separate `hosts bootstrap` and `hosts install-agent` CLI commands + TUI b/i keys. Replaced with a single `agentum hosts setup <host>` (CLI) / `i` (TUI) that checks readiness and installs missing deps + agents in one pass. `--yes` flag for CI.

### Changed
- **TUI model placeholder** bumped from `claude-sonnet-4` to `claude-opus-4-8`.
- **Opus model examples** bumped across CLI + dashboard to `claude-opus-4-8`.

### Fixed
- **macOS clippy:** dropped needless `return` in clip-agent log path and profiles path.
- **CI:** pinned Rust toolchain to 1.94.1 for reproducible fmt/clippy across local and CI.

## [0.8.13] — 2026-05-28

### Added
- **SSH-agentless hosts.** The local daemon can now drive sessions on
  remote machines over SSH without installing an `agentum` binary on the
  remote — it runs `tmux`/`git`/the agent CLIs there directly. New
  `hosts` table (migration `0018_hosts.sql`, with a default `local`
  host), `/api/hosts` CRUD + `/api/hosts/{id}/test` probe routes, the
  `host_runtime` SSH executor, an `agentum hosts list/add/test/rm` CLI,
  a Host field on the TUI's New-session form, and host selection +
  agent-availability probing in the dashboard's New-session dialog.
- **Worktree-by-default in the New-session form.** The TUI New-session
  form gained an "Isolate in git worktree" toggle that defaults **on**:
  each session spins up its own `git worktree` (own branch + checkout at
  `<repo>-worktrees/agentum-<name>`) so several agents can run against
  one repo in parallel without stomping each other's branch/stash.
  Toggle off with Space for a non-git workdir; it's forced off when an
  explicit remote host is selected (worktrees are local-host only for
  now). The dashboard's equivalent checkbox now also defaults on, for
  parity.

## [0.8.11] — 2026-05-26

### Fixed
- **TUI sidebar agent dot was stuck on green for any session whose
  connect-time replay snapshot was missing.** The dot color used to
  fall back to `status_dot(Status::Running) = green ●` whenever the
  session was not in `app.idle` / `app.awaiting_input` — but those
  sets are only populated by inbound `agent.*` events. A fresh-
  connected TUI, a daemon that returned an empty
  `latest_agent_event_per_session` snapshot, or a session that had
  not yet emitted a state event all read as a misleading pulsing
  green for the lifetime of the session. Same class of bug as
  v0.7.49's dashboard regression, this time in the terminal client.
  Green now requires *positive evidence* the agent is working
  (new `app.working: HashSet<Uuid>` populated by `agent.working`
  and the `state: working` variant of `agent.input_resolved`). A
  Running tmux pane with no known agent state reads as a neutral
  muted dot until the watchdog emits its first observation
  (typically within ~1 s).

## [0.7.63] — 2026-05-18

### Fixed
- **Dashboard terminal stayed stuck on the previous session after
  navigating to a new one.** SvelteKit reuses the `+page.svelte`
  instance across route param changes (`/sessions/A → /sessions/B`),
  and `Terminal.svelte`'s `onMount` only fires once — so the xterm
  + WebSocket stayed bound to session A while the user was looking
  at B's URL. The terminal pane is now keyed by session id, so
  every navigation tears down the old xterm + WS and mounts a
  fresh pair against the new session's stream.

## [0.7.62] — 2026-05-18

### Added
- **Per-server version chip in the TUI sidebar and the dashboard
  EndpointSwitcher + sidebar.** Every server row now renders a
  `v0.7.62`-style label next to its name. Matching the local CLI's
  version reads as muted/informational; a mismatch flips to the
  warning color so fleet drift is visible at a glance — handy when
  the user runs `agentum update` on one machine and forgets to
  refresh the daemon on the others.
- **`Client::health()` returns the parsed `/api/health` body**
  instead of a bare unit result, surfacing `version`, `hostname`,
  and `capabilities` to callers. `try_connect_profile` and the
  active-profile boot path both capture the version into
  `ClientEntry.version` so the sidebar render is a pure local read.

## [0.7.61] — 2026-05-18

### Fixed
- **TUI sidebar started hidden every launch** if it had ever been
  toggled off (Ctrl-B), the user toggled it via the settings overlay,
  or a synced `tui_prefs.toml` carried `sidebar_hidden = true` over
  from another machine. The sidebar is the primary navigation
  surface; users entering the TUI to a blank tree column reasonably
  read it as the app being broken. The hidden flag is now
  session-local: every fresh launch shows the sidebar; Ctrl-B during
  a session folds it; next launch comes back visible. Mid-session
  prefs writes still take effect, so the Settings-overlay toggle
  isn't useless — it just doesn't survive a restart.

## [0.7.60] — 2026-05-18

### Fixed
- **Dashboard `agent.finished` had no audible cue out of the box.**
  v0.7.59 unmuted the OS-banner chime, but the browser-notification
  toggle (`tweaks.notifyBrowser`) defaulted to `false` *and* the OS
  permission was never requested, so the user had to dig through
  Settings to get anything to ring. Now: a 250 ms in-page Web Audio
  chime fires alongside the toast (works without any permission), the
  OS-banner toggle defaults `true`, and the first event lazily kicks
  the browser permission prompt so opt-in is one click instead of
  three menu screens. `agent.awaiting_input` rings with a higher tone.
- **Finished toast/chime felt sluggish.** The Working→Idle→Working
  debounce was 2500 ms — enough that users staring at the sidebar dot
  thought no notification was coming. Watchdog now does its own
  classification debouncing (v0.7.51); the dashboard's safety net is
  down to 800 ms.
- **TUI sidebar dot was hollow gray (`○`) for any peer server that
  wasn't the active target**, even when that peer was reachable. A
  user with 3+ servers couldn't tell at a glance which were up. The
  dot now encodes reachability via color only — solid green for every
  Live server, red for unreachable, yellow for login-needed — and
  active vs inactive is conveyed by the bold label + cursor highlight
  the row already had.

## [0.7.59] — 2026-05-18

### Fixed
- **`agent.finished` browser notifications were silent**, so a long
  Claude run could complete with the dashboard tab in the background
  and the user got zero audible cue — the OS banner appeared muted
  while there is no in-app audio stack to compensate. OS notifications
  for every kind now ring; `urgent` still controls whether they fire
  with the tab foregrounded, audibility is decoupled.
- **Topbar server chip defaulted to green for `unknown` status**, so
  newly-added profiles looked reachable before the first `/api/health`
  probe even completed. The chip now reads from the same dotClass
  used in the sidebar (active profile → live WS state, peers → fleet
  probe) and shows gray when the probe hasn't returned yet.
- **TUI sidebar dots stayed green for peer servers that had gone
  unreachable mid-session.** The periodic refresh probes every peer's
  `list_sessions` endpoint but only updated `http_fail_count` for the
  active profile; per-peer `ClientEntry.status` now flips to
  `Unreachable` on probe failure (and back to `Live` on recovery) so
  the sidebar tells the truth at a glance.

## [0.7.58] — 2026-05-18

### Fixed
- **Non-interactive `--mode host` installs silently picked loopback bind**,
  so `curl … | INSTALL_MODE=host sh` on a VPS would install a daemon
  that was unreachable from the tailnet/LAN: `agentum profiles add` from
  another machine got "connection refused" on `:8822`, and the dashboard
  fingerprint check never completed. `ask_expose` returned `"loopback"`
  whenever `INTERACTIVE=false` regardless of operator intent.

### Added
- **`--expose lan|loopback` flag and `AGENTUM_EXPOSE` env var** on
  `install.sh` so non-interactive runs can opt into LAN bind without a
  TTY prompt. VPS one-liner is now:
  ```sh
  curl -fsSL https://.../install.sh | INSTALL_MODE=host AGENTUM_EXPOSE=lan sh
  ```
  Interactive runs are unchanged. Default is still `loopback` so laptop
  installs stay safe.

## [0.7.43] — 2026-05-14

### Fixed
- **Admin account wizard rendered on a single line** when launched
  from the installer. The username, password, and confirm prompts
  each used `eprint!` to stderr while the surrounding boxes used
  `println!` to stdout — different buffering, and under some
  terminal modes the three prompts collapsed into one line so the
  password step looked like it had been skipped (and silently
  failed when the user hit Enter without realising they were still
  in the wizard). All prompts now go through stdout with explicit
  flushes, and the wizard retries on short/mismatched passwords
  instead of bailing out so the install can't end without an admin.

### Changed
- **Autostart prompt is now two options instead of three.**
  `[1] Start now and auto-start at login (recommended)` /
  `[2] Not now`. The "background only" middle ground is gone — if
  you want it to run, you almost always want it to come back after
  reboot too, and the prior split made that the second choice.
- **macOS gets a real LaunchAgent** instead of falling back to
  `nohup`. The installer now writes
  `~/Library/LaunchAgents/dev.agentum.daemon.plist` with
  `RunAtLoad` + `KeepAlive`, registers it with
  `launchctl bootstrap gui/<uid>`, and writes logs to
  `~/Library/Logs/agentum/daemon.log`. So "auto-start at login"
  works on Macs now, not just Linux. `agentum uninstall` removes
  the plist and `launchctl bootout`s it before wiping files.

## [0.7.42] — 2026-05-14

### Added
- **`agentum uninstall`**: removes the binary plus everything the
  daemon wrote to disk (SQLite DB, TLS material, daemon logs, pane
  log cache). On Linux also stops + disables the systemd user unit
  if one was registered by the installer. By default keeps user
  data (profiles, credentials, pinned hosts) so a reinstall lands
  you back on your remote servers — pass `--all` to wipe those too.
  Flags: `-y/--yes` (skip confirmation), `--all` (include user
  data), `--dry-run` (preview without removing). Best-effort
  shutdown of any running `agentum serve` via `pkill` before file
  removal.

## [0.7.41] — 2026-05-14

### Fixed
- **Installer order**: "Create admin account?" and "Start agentum now?"
  now fire *before* the "Get started" reference card. Users were being
  shown a list of steps and then asked questions — the order is
  reversed so by the time they see the reference card, the setup
  prompts are already behind them.
- **Stale `’` literal**: the autostart prompt's "No — I'll start
  it manually later" line was rendering as literal `’` because
  POSIX `printf '...'` doesn't interpret unicode escapes. Switched to
  a plain ASCII apostrophe inside double quotes.
- **Reference card simplified**: dropped the "Start the server"
  step (the autostart prompt already handles that), reformatted as
  `Dashboard / Terminal / New agent / Health` keys instead of a
  numbered checklist.

### Changed
- "Create admin account now?" prompt collapsed to one line. The
  prior version had two grey help lines explaining you could also
  register later — that's already covered by the binary's own help
  and `agentum auth setup` is discoverable.

## [0.7.40] — 2026-05-14

### Fixed
- `scripts/install.sh` header comment pointed at a non-existent
  `agentum.dev` domain. Canonical install URL has always been
  `https://github.com/mateocerquetella/agentum/releases/latest/download/install.sh`
  (used by the README, landing page, and `agentum update`); the
  stale comment is now corrected. Runtime behavior unchanged.
- `cargo fmt` drift across `agentum-server::lib`, `agentum::commands::{auth,serve}`,
  and `agentum::commands::terminal::{app,ui}` so the `ci` workflow
  passes again. No logic changes — `cargo fmt --all`-only.

## [0.7.39] — 2026-05-14

### Changed
- **Installer: one question instead of three.** The wizard used to ask
  `server | cli | both`, then autostart, then auth setup — three
  decisions framed in vocabulary new users hadn't earned yet. The
  prompt is now a single question: *"Will this machine run agents?"*.
  Same binary either way; only the post-install flow forks.
  - **Host mode** (default): existing auth + autostart prompts, plus
    a `local` profile auto-registered against the detected LAN IP so
    `agentum terminal` connects without any extra setup.
  - **Client mode**: skips auth and autostart entirely, inline-prompts
    for a remote daemon URL, and writes a `remote` profile pointing at
    it with `--insecure --set-default`.
  - Legacy `--mode server|cli|both` tokens are still accepted (they
    fold into `host`/`client`) so existing automation keeps working.
- **Dashboard URL uses LAN IP, not 127.0.0.1.** The installer now
  probes for a routable LAN address (macOS via `ipconfig getifaddr`
  + route fallback; Linux via `hostname -I` + `ip route` fallback)
  and uses it in the success message + the `local` profile. Other
  devices on the same network can hit the dashboard without an
  extra round of "wait, what's my IP?". Falls back to `127.0.0.1`
  with a warning when nothing routable is found.
- Multi-server / bidirectional control is now a one-line tip at the
  end of the wizard instead of a concept the first-run user has to
  understand up front. Power-user territory stays one
  `agentum profiles add` away.

### Added
- `agentum auth setup` — interactive wizard (or non-interactive
  `--username` / `--password` for CI) that writes the first admin
  account directly to the DB without needing a running server. Also
  auto-runs from `agentum serve` when the daemon detects zero users
  on boot and stdin is a terminal.

### Notes
- Bundles previously unreleased work from v0.7.37 / v0.7.38 (auth
  wizard, HTTPS→HTTP loopback fallback, `--no-auth` flag) under the
  bumped Cargo.toml version. Tags v0.7.37 and v0.7.38 were cut
  without bumping the manifest; this release re-aligns the two.

## [0.7.27] — 2026-05-12

### Added
- **`agentum terminal` auto-spawns a local `agentum serve` sidecar
  and auto-bootstraps an account on a fresh host.** You shouldn't
  have to remember `agentum serve &` and a manual `agentum auth add`
  before launching the TUI on your own machine — running on the host
  you're already at should just work. Concretely:
  - If the loopback (127.0.0.1:8822) doesn't answer at startup, the
    TUI now forks `agentum serve` in the background under its own
    process group (so it survives the alt-screen exit and gets
    reused next launch) and waits up to 3 s for it to come up.
    Daemon logs land in `$XDG_STATE_HOME/agentum/autoserve.log`.
  - Once the daemon is reachable, if `auth/status` reports
    `needs_setup = true` (zero users registered), the TUI calls
    `POST /api/auth/register` with username `local` and a random
    32-char password, caches the bearer token in
    `credentials.toml`, and walks straight into the alt-screen
    without a login prompt. The anonymous-register fast path is
    strictly gated to 127.0.0.1 / localhost / ::1 — never fires
    on a remote daemon (which would be a security footgun).
  - Cached tokens from prior runs are still preferred, so
    subsequent launches don't re-register or re-issue tokens.
  - Auto-start is skipped when `--profile` / `--api` is explicit —
    those callers know exactly which daemon they want.

  Net effect: `agentum terminal` on a fresh Mac with nothing
  pre-configured drops straight into the TUI driving the local
  daemon, no `agentum serve &`, no "add a server" prompt, no
  signup screen.

## [0.7.26] — 2026-05-12

### Changed
- **`agentum terminal` prefers the local loopback over a configured
  remote `default` when both are reachable.** A user with
  `default = "omarchy"` in profiles.toml who runs the TUI on their
  Mac was getting omarchy as the active connection — so "this
  machine" appeared in the Servers sidebar but was Unreachable /
  LoginNeeded, and the New Session form's Profile cycle to local
  bailed with `this machine isn't connected — try Ctrl-O to re-add`.
  The most common reason to launch the TUI is to drive the machine
  you're sitting at, so the resolution order is now:
  1. Explicit `--profile <name>` always wins.
  2. Otherwise, if `agentum serve` answers on 127.0.0.1:8822 (~500 ms
     bounded probe), prefer it as the active connection.
  3. Otherwise, fall back to `profiles.toml`'s `default = …`.
  4. Otherwise, fall through to the connect-or-onboard loop.
  The configured `default` still gets used when local isn't running
  (so a laptop offline-mode falls back to your VPS cleanly), and
  the configured profile is still added to the sidebar as a peer
  via the existing fanout — you just don't get stuck *as* it when
  your local box is live.

## [0.7.25] — 2026-05-11

### Fixed
- **New Session form's Profile field can now Tab-cycle to "this
  machine" even when the active connection is a remote daemon.**
  Before: launching `agentum terminal` with `default = "omarchy"` in
  profiles.toml left `app.clients` populated only with the `omarchy`
  entry. The form's Tab handler computed `wheel_size = 1` and
  short-circuited to `next_field()` instead of cycling — so a Mac
  user driving their Linux box couldn't spawn a session on the Mac
  from inside the TUI. Now a non-interactive loopback probe runs
  alongside the existing peer fanout at boot; if a local
  `agentum serve` answers it's added to `app.clients` under the `""`
  key as a peer (and its sessions are merged into the unified
  sidebar). If nothing's listening on 127.0.0.1:8822 the entry
  still shows up as Unreachable so the user sees "this machine"
  exists as an option instead of having no idea why Tab won't cycle.

## [0.7.24] — 2026-05-11

### Fixed
- **`/clear` detection now lives in the transcript parser, not the
  TUI keystroke shadow.** The v0.7.21 client-side line shadow
  couldn't survive Claude Code's slash-command picker (Up/Down to
  navigate, Tab to autocomplete, picker-Enter to commit) — the
  buffer cleared on arrows, so a user who typed `/cle` and Enter
  on the picker highlight never tripped the match. `apply_line` in
  `agentum-core::transcript` now scans user text blocks for the
  `<command-name>…</command-name>` envelope Claude Code injects for
  every slash run and resets `AgentTaskState` to default on `/clear`
  or `/compact`. Works regardless of how the command was entered
  and on every client (TUI, dashboard) because the wipe lives in
  the daemon's transcript watcher. The client-side shadow +
  `/agent-tasks/reset` endpoint stay as a fast path so the local
  panel goes blank without waiting for the FS-watcher tick.
- **Lazygit pane no longer stays pinned to a stale project when the
  selected session's workdir isn't a local directory.** Previously
  `refresh_lazygit_for_selection` would silently bail on
  `!is_dir()` — typical for remote sessions where the daemon's
  workdir is a Linux path that doesn't exist on the macOS TUI host
  — leaving lazygit attached to whatever project it was spawned in.
  Now it translates the foreign home prefix (`/home/<u>/…` or
  `/Users/<u>/…`) into the local `$HOME` so a user with parallel
  `~/Developer/projects/<name>` checkouts on both machines follows
  into the local copy. If no local equivalent exists, the pane is
  dropped and `lazygit_cwd` cleared so the next switch into a
  local-workdir session re-spawns cleanly instead of staying on a
  totally unrelated project.

## [0.7.23] — 2026-05-11

### Changed
- **Loopback row reads as the host's hostname instead of
  `MY MACHINE (<os>)`.** A user running the TUI directly on their
  Linux box (Omarchy) got `MY MACHINE (linux)` and read it as
  "weirdly aggressive shouting" — and worse, "MY MACHINE" sounded
  Mac-specific to them so they couldn't tell why their VPS sessions
  were appearing under it (answer: the TUI was running *on* the VPS,
  so "the machine where the TUI is" was the VPS, but the label
  didn't communicate that). Now the row reads as the system's
  hostname (e.g., `omarchy`, `mateo-mac`), with mDNS / LAN suffixes
  (`.local`, `.lan`, …) trimmed and the result lowercased. Cached
  behind a `OnceLock` so we don't fork a `hostname` subprocess every
  frame. Falls back to `local` when the system `hostname` command
  is unavailable. Named profiles continue to read as `@<name>`.
- **No more bold on the loopback's server header.** The colour
  difference (`fg_strong` for the local row vs `accent_alt` for
  remote rows) is enough — bolding the local row on top of that
  amplified the "shouting" effect.

## [0.7.22] — 2026-05-11

### Changed
- **Sessions sidebar is a three-level tree again — server → project →
  session.** v0.7.19 collapsed each server's sessions into a flat
  list with a trailing workdir badge, which made a fleet across
  multiple projects unscannable. The tree is now:
  - **Server** header (`MY MACHINE (<os>)` for the loopback,
    `@<profile>` for named profiles) with the total session count.
  - **Project** sub-header per workdir under each server, showing the
    workdir basename + the session count for that project.
  - **Session** leaves indented under their project, with the name,
    status dot, and tool label (no more trailing workdir badge —
    project identity now lives in the project header where it belongs).
- **Loopback row reads as `MY MACHINE (<os>)`** instead of "this
  machine". `<os>` comes from `std::env::consts::OS` (`macos`,
  `linux`, …) so the local row materially says which OS the TUI is
  running on, and the local row is visually distinct from any
  `@<remote>` row. Replaces the literal in the Servers panel, the
  Sessions tree header, the New Session form's profile field, and
  every "can't reach <x>" / "<x> isn't connected" status message.
  Named profiles keep the `@<name>` prefix.
- **Collapse cascades inward.** Pressing `h` / `←` on a session row
  folds the parent project (so a single keystroke hides siblings
  without burying the rest of the server). On a project row it folds
  the project; on a server row it folds the server. `l` / `→` opens
  the nearest closed level, and on an already-open server it
  expands every project inside in one keystroke.
- **Per-server and per-project fold state survives tree rebuilds.**
  `refresh_sessions` now snapshots both levels under namespaced
  `server::` / `project::` keys so the session-list refresh that
  fires on every WS event doesn't reset every fold the user just
  set.

## [0.7.21] — 2026-05-11

### Added
- **Plan / Todos / Tasks panel mirrors `/clear` in the agent.** Typing
  `/clear` (or `\clear`) inside the terminal pane and pressing Enter
  now wipes the right-side panel in lockstep with the agent's own
  context wipe. Implemented in two pieces:
  - The TUI shadows each session's current input line as a best-effort
    line buffer (per-session, behind `app.term_input_lines`). On
    Enter, a trimmed buffer that equals `/clear` triggers an immediate
    local cache clear + a fire-and-forget POST to the daemon. The
    shadow resets on Esc, Ctrl-anything, arrow keys, and after every
    Enter so partial typing or history recall doesn't false-positive.
  - New daemon endpoint `POST /api/sessions/{id}/agent-tasks/reset`
    resets the `TranscriptStore` slot to `AgentTaskState::default()`
    **and** fast-forwards the file cursor to the current end-of-file
    so a subsequent FS-watcher refresh doesn't repaint the cleared
    state from the already-written transcript. Broadcasts
    `agent_tasks.updated` so other connected clients (dashboard, peer
    TUIs) also see the wipe.

## [0.7.20] — 2026-05-11

### Fixed
- **CI clippy is green again.** Rust 1.95 added the
  `collapsible_match` lint, which fired on two `KeyCode::*` match
  arms in the TUI that wrapped their body in an `if`. Refactored
  both into match guards (`KeyCode::Down if ... =>` and
  `KeyCode::Backspace if filter.pop().is_some() =>`). Same
  behaviour, just collapsed one level. Verified locally with
  `cargo clippy --workspace --all-targets -- -D warnings`.

## [0.7.19] — 2026-05-11

### Changed
- **Sessions tree groups by server, not (server × workdir).** Previously
  a fleet with 3 workdirs on `@vps` produced 3 `@vps · workdir` headers
  interleaved with `this machine`'s headers — pretty hard to tell at a
  glance which sessions belonged to which machine. Now each server
  collapses to a single top-level header (`this machine`, `@vps`, …)
  with every session it owns nested beneath, sorted by `(workdir,
  name)` so same-project sessions still cluster visually. The workdir
  basename moved from the group header to a trailing badge on each
  leaf row so project context stays visible.
- **Enter on a Server row now jumps the cursor to that server's group
  in the Sessions tree** instead of soft-restarting with that profile
  active. The whole fleet's sessions are already rendered together in
  one tree, so Enter is a navigation, not a re-target. Active-profile
  switching (which only matters as the *default* server for new spawns
  and the `t` plain-bash shortcut) stays available via Ctrl-O
  (Profiles overlay), the New Session form's Profile field, or
  `agentum profiles use <name>`.

## [0.7.18] — 2026-05-11

### Fixed
- **`agentum terminal` no longer hard-fails on `default = ""` in
  `profiles.toml`.** A stale or hand-edited empty default would
  resolve to `Some("")`, which then failed the `profiles.get("")`
  lookup and bailed with the unhelpful `profile `` not found`
  message — leaving the user unable to launch the TUI without
  editing the TOML by hand. The default-name reader now treats an
  empty (or whitespace-only) string the same as a missing field
  and falls through to the loopback probe.
- **Workdir Tab in the New Session form stays on the field when it
  hits an ambiguous fork.** Pressing Tab on a path like `~/D` with
  Desktop/Documents/Developer present (no common prefix to extend)
  used to bump the cursor out to the Tool field — making it
  impossible to continue typing the path without first re-focusing
  Workdir. The function returned `false` (the "advance" signal)
  while its comment said "no-op"; now it returns `true` so the
  field stays selected, matching bash readline. Empty workdir or
  path with no `/` still advances the field as before.

## [0.7.17] — 2026-05-11

### Fixed
- **CI is green again.** v0.7.15/v0.7.16 landed with rustfmt drift across
  20 files (agentum-core, agentum-server, agentum-store, agentum-watchdog,
  and the agentum CLI/TUI). `cargo fmt --all -- --check` failed in CI
  and blocked the release pipeline. Pure whitespace + line-wrapping
  changes — no logic moved. Verified clean with `cargo build`,
  `cargo clippy`, and `cargo test --workspace --lib` (81 tests pass).

### Added
- **TUI multi-select indicator + help overlay.** The `app.rs` half of
  the multi-select feature landed in v0.7.15, but the rendering layer
  was missing: checked rows had no visual cue and the help overlay
  still advertised "Enter — multi-select (WIP — coming soon)". The
  Sessions tree now renders a bold `[x]` accent on every checked
  leaf, and the help overlay describes the real keybindings:
  `Enter` checks/unchecks, `u`/`s`/`K`/`x`/`D` act on the checked set
  (or fall through to the cursor row when nothing is checked), and
  `Esc` clears the checks before falling through to filter/fullscreen.

## [0.7.16] — 2026-05-11

### Added
- **Dashboard parity with the TUI's "this machine" + Servers picker.**
  Three matching pieces, lifted directly from the
  v0.7.13–v0.7.15 TUI work:
  - The sidebar now has a permanent **Servers** section above the
    Sessions list. Every configured profile renders (loopback labels
    as "this machine"), the active one is tagged, and clicking
    another row switches the dashboard's active endpoint via the
    same reload path the topbar `EndpointSwitcher` uses.
  - The **New Session** dialog gained a **Servers** field (tiles
    above the Agent picker). Selecting a server changes which
    daemon the spawn POSTs against, re-probes `/api/agents` for
    that server's installed tools, and refetches `$HOME` via
    `/api/fs/list` so the **Working directory** field follows
    the picked server — same contract as the TUI's Tab cycle.
  - The legacy `request()` is unchanged; new profile-pinned
    siblings `listDirOn`, `createSessionOn`, `startSessionOn`,
    `listAgentsOn` route through `fetchProfile` so the dialog can
    target a non-active endpoint without flipping the topbar.
    `requestOn` (a `request()`-shaped helper for arbitrary
    profiles) backs all four.

## [0.7.15] — 2026-05-11

### Fixed
- **`n` pre-fills the form with a matching (profile, workdir) pair.**
  Previously the form opened with `profile = active server` and
  `workdir = selected session's workdir`, even if that session was
  owned by a different daemon. The user would then submit and either
  hit a "path doesn't exist" error or land on an empty workdir on
  the active server. Now: when a session is selected, the form's
  Servers field is pre-filled with that session's *owning* profile
  (via `App::profile_for_session`) so the pre-filled workdir is a
  real path on the chosen daemon.
- **Tab on the Servers field always advances when there's nothing
  to cycle to.** v0.7.14's check (`if names.is_empty() && !has_local`)
  trapped the cursor on the Profile field in the
  loopback-with-zero-peers case (names empty, has_local true,
  wheel of just `[""]`). The condition is now `wheel_size <= 1`,
  matching what `cycle_profile` actually sees: zero or one entry =
  nothing to cycle = advance to the next field.

## [0.7.14] — 2026-05-11

### Fixed
- **Workdir actually follows the Servers cycle now.** v0.7.13's
  resolver kept a fallback that, when the user launched with
  `--profile vps1` and Tab-cycled to "this machine" in the New
  Session form, silently routed `/api/fs/list` through the active
  vps1 client and returned vps1's `$HOME` — making it look like
  the workdir wasn't moving with the cycle. The empty "" entry is
  now only included in the cycle wheel when a real local-loopback
  client is connected (`app.clients` has a `""` key); otherwise
  Tab walks straight between configured peers. Successful refetches
  clear any prior inline error; failed ones still surface the
  reason and the workdir-stays-put outcome.
- **`NewSessionForm::cycle_profile` takes a `has_local: bool` flag.**
  Mechanical signature change to thread the loopback-availability
  signal from the caller; the form's behaviour is otherwise the
  same. Locked under five new unit tests
  (`cycle_profile_tests::*`) covering the loopback / no-loopback /
  multi-peer / unknown-starting-profile combinations.

## [0.7.13] — 2026-05-11

### Fixed
- **New Session form: duplicate title removed.** The overlay used to
  render "New session" twice — once as the overlay-box border title,
  once as a `head()` line inside the box. The inner heading is gone;
  the box title remains as the single label.
- **"this machine" is now a permanent row in the SERVERS sidebar.**
  Previously the section showed `(no servers — press a)` when no
  peer profiles were configured, and configured peers only when
  they were. Cursor 0 is now a synthetic "this machine" entry
  (mapping to the empty / loopback profile) that always renders,
  with configured peers following at cursors 1..N. Navigation,
  Enter (switch profile), `d` / Ctrl-D (remove) and bounds checks
  in `app.rs` were updated to account for the offset; deleting the
  "this machine" row is rejected with an explicit status message.
- **Workdir follows the Servers cycle in the New Session form.**
  Tab-cycling the Servers field now resolves the target client
  through `app.clients` first (the empty key holds the real local
  loopback when one is connected) and only falls back to the
  run-loop's client for the empty case. Failed `/api/fs/list`
  fetches surface inline (`couldn't reach @vps1: …`) instead of
  silently leaving the workdir on the previous server's `$HOME`,
  and peers that aren't connected get a clear
  `@vps1 isn't connected — try Ctrl-O to re-add` hint.

## [0.7.12] — 2026-05-11

### Fixed
- **Plain `d` on a server entry now also confirms before removing.**
  v0.7.11 added `Ctrl-D` with a y/N prompt but left lowercase `d`
  wired to a direct `store.remove` — the muscle-memory key was still
  the silent-delete one. Both keys now route through the same
  `RemoveServer` confirmation.
- **No more "● crashed" toast / banner echo after the user kills a
  session.** The watchdog emits `session.crashed` microseconds after
  the row is deleted (the tmux pane vanishing trips its detector),
  which read as "killing it crashed it" — confusing for an
  intentional action. A new `recently_killed` set in `App` is
  populated in `execute_action` and consumed in `apply_event`'s
  crash branch so the echo gets dropped silently. When the killed
  session is the currently-selected one, selection drops to `None`
  and the term pane resets — no more auto-jumping to another
  crashed session and resurrecting the same banner.
- **CLI-side server removals (`agentum profiles rm vps`) now
  propagate to running TUIs.** The 5s refresh tick reloads
  `profiles.toml` from disk, so the sidebar's Servers section stays
  in sync with out-of-band edits without forcing a TUI restart.

## [0.7.11] — 2026-05-11

### Added
- **`Ctrl-D` on the tree deletes the row under the cursor.** Routes
  by section: on Sessions it raises the Kill confirmation, on Servers
  it raises the RemoveServer confirmation. Either way the existing
  y/N prompt acts as the double-check before anything is actually
  removed. When a terminal pane (not the tree) is focused, `Ctrl-D`
  still forwards EOF to the running agent as before.

### Fixed
- **TUI profile-switch login no longer reads as "nothing happened."**
  Switching to a `login needed` server prompted for username/password
  on the host TTY after exiting the alt-screen, but the password was
  echoed in plaintext and any failure flashed by in <100ms before the
  TUI redrew on the previous profile. Now: passwords are masked (via
  `rpassword`), the prompt shows a `signing in…` / `✓ signed in` /
  `✗ <error>` banner, and the user gets up to 3 attempts before
  giving up. On the outer switch-profile failure path, the TUI now
  pauses on `press Enter to continue…` so the error is actually
  readable.

### Changed
- **New-session form: `Profile` → `Servers`, with `this machine` as
  an explicit peer.** The empty/loopback entry used to render as
  `(current connection)`, which read as a stale fallback rather than
  "the local daemon." It now shows as `this machine` so Tab cycles
  `this machine` ↔ `@vps1` ↔ `@vps2` with every target looking like
  the same shape.
- **`Working directory` moved directly below `Servers` in the form.**
  Matches the mental order "which agentum, then which folder."
  Cycling Servers (which already refetches `$HOME` from the picked
  daemon) now lands the cursor on the workdir it just populated.
- **`n` on the tree pre-fills workdir from the daemon's `$HOME`, not
  the laptop's.** When driving a remote profile from macOS, the old
  behaviour pre-filled `/Users/you` which doesn't exist on a Linux
  VPS. The form now calls `/api/fs/list` to resolve the server-side
  `$HOME` and uses that; falls back to local `$HOME` on network
  error so cold/offline daemons don't leave the field empty.

## [0.7.10] — 2026-05-11

### Added
- **HTTP-failure detection feeds the reconnect banner.** v0.7.9 only
  watched the events-bus WebSocket; this release adds the second
  trigger from the original spec: when HTTP fetches start failing
  while the WS is still nominally connected (TCP keepalive lag —
  daemon may be hung or the path went stale), the banner surfaces
  with a distinct "daemon not responding" copy. Dashboard counts
  consecutive `fetch` throws + 5xx responses; TUI counts consecutive
  periodic `list_sessions` poll failures. Same `>= 2` debounce as
  the WS path.

## [0.7.9] — 2026-05-11

### Added
- **Offline / reconnecting banner across TUI and dashboard.** When the
  events-bus WebSocket fails to reconnect after its first retry, both
  surfaces now show a persistent indicator until the connection is
  back. TUI gets a 1-row strip below the title bar; dashboard gets a
  sticky strip below the topbar. Threshold is `attempt >= 2` so a
  sub-second blip doesn't flicker the layout. Pre-existing TUI
  reconnect overlay is gated on the same threshold for consistency.

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
