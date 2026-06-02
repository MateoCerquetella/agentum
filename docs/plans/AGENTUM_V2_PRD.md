# PRD - Agentum v2: One-Install, SSH Agentless, Tauri Desktop

**Owner:** Mateo Cerquetella
**Status:** Draft - ready to implement
**Target:** Claude Code (agentic coding agent)
**Repo:** https://github.com/MateoCerquetella/agentum

---

## 1. Context

Agentum is a self-hosted control plane for AI coding agents (Claude Code, Codex, OpenCode, etc). It runs as a Rust binary on a host, spawns agents inside tmux panes, and exposes a Svelte PWA over WebSocket so the user can watch and interact from any device including mobile. The agents survive client disconnections, laptop lid close, and network changes because tmux holds the session on the host.

**The problem this PRD solves:** today Agentum requires manually installing on every host you want to use. Setup is rustic (clone, build, configure). There is no desktop app — only a browser-accessible PWA. Electron-based competitors ship polished installers but cannot survive SSH disconnection because they do not use tmux. We want to close the polish gap while keeping the structural advantages (tmux persistence, single Rust binary).

**Three deliverables, three phases:**

1. **Install one-liner** (curl-based, like Tailscale/k3s/Bun) so any host installs in 30 seconds
2. **SSH agentless mode** so the user installs Agentum only on their local machine and manages agents on remote hosts via SSH + tmux without needing Agentum binary on those hosts
3. **Tauri desktop shell** so users get `Agentum.app` (macOS), `.AppImage` (Linux), `.msi` (Windows) — same Rust binary wrapped as a native window

The three phases are independent. Each delivers value standalone. Do them in order: 1 → 2 → 3.

---

## 2. Non-goals

These are explicitly out of scope for this PRD:

- iOS / Android native apps (deferred to a future PRD; Tauri 2 supports this but it requires App Store signing infra we don't want to handle yet)
- Cloud-hosted / managed Agentum (SaaS)
- Replacing tmux with containers, Firecracker, or any other isolation mechanism
- Reimplementing heavyweight IDE features (Design Mode, embedded Chromium browser, file editor with full IDE features) — Agentum stays focused on orchestration and observability, not on being an IDE
- Multi-user / team features
- Auth provider integrations (OAuth, SSO) — local auth only for now
- A Windows-specific server build (Windows is a client-only target via Tauri; the server runs only on Linux/macOS)

---

## 3. Success criteria

- Fresh Linux VPS goes from zero to running Agentum in **under 60 seconds** with a single command, no prompts
- User can add a remote host via SSH and start an agent on it without installing Agentum on that host
- Closing the local Agentum client (PWA tab, .app window, or SSH disconnect) does **not** kill any agent on any host
- Reconnecting after disconnect (any duration) reattaches to the running agent and shows accumulated output
- `Agentum.app` opens on macOS/Linux/Windows with a double-click, starts the local server on a free port, and shows the UI in a native window
- Single binary stays under 25 MB compressed for the CLI; Tauri bundle under 30 MB
- All three phases shippable independently; no phase requires the next one

---

## 4. Phase 1: Install one-liner

### 4.1 User story

> As a developer, I want to install Agentum on any Linux or macOS host with a single curl command, so I can stop manually cloning and building the repo.

### 4.2 What ships

- `install.sh` at the repo root, also served at `https://raw.githubusercontent.com/MateoCerquetella/agentum/main/install.sh`
- Cross-compiled binary releases for: `linux-amd64`, `linux-arm64`, `darwin-amd64`, `darwin-arm64`
- GitHub Actions workflow that builds and uploads all four binaries on every git tag
- Updated README with the one-line install at the top

### 4.3 Install script behavior

The script must:

1. Detect OS via `uname -s` (linux, darwin); fail with a clear message on anything else
2. Detect arch via `uname -m`; map `x86_64` → `amd64`, `aarch64`/`arm64` → `arm64`; fail on anything else
3. Determine install dir: AGENTUM_INSTALL_DIR override → /usr/local/bin (if root/sudo) → $HOME/.local/bin (with PATH warning)
4. Download the matching binary from the latest GitHub release using `curl -fsSL`
5. Verify the download via SHA256 against a `checksums.txt` from the same release
6. `chmod +x` and move into the install dir
7. If Linux and running as root: install a systemd unit at `/etc/systemd/system/agentum.service`, daemon-reload, enable --now
8. If macOS: install a launchd plist at `~/Library/LaunchAgents/dev.agentum.daemon.plist` and `launchctl load` it
9. Print install path, service status, URL to PWA, link to docs

Idempotent. No interactive prompts.

### 4.4 Systemd unit template

```ini
[Unit]
Description=Agentum control plane
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/agentum serve --config /etc/agentum/config.toml
Restart=on-failure
RestartSec=5
User=agentum
Group=agentum
WorkingDirectory=/var/lib/agentum
Environment="AGENTUM_DATA_DIR=/var/lib/agentum"

[Install]
WantedBy=multi-user.target
```

Script must `useradd -r -m -d /var/lib/agentum agentum` if the user does not exist.

### 4.5 GitHub Actions release workflow

Trigger: `on: push: tags: ['v*']`. Matrix build for the four targets. `cross` for Linux cross-compilation, `macos-14` arm64 + `macos-13` amd64.

Artifacts:
- `agentum-linux-amd64`, `agentum-linux-arm64`, `agentum-darwin-amd64`, `agentum-darwin-arm64`
- `checksums.txt`, `install.sh`

### 4.6 Uninstall

`uninstall.sh` removes binary + service unit, prompts about data dir (default keep).

### 4.7 README changes

```sh
curl -fsSL https://raw.githubusercontent.com/MateoCerquetella/agentum/main/install.sh | sh
```

Or from source: `cargo install --path .`

---

## 5. Phase 2: SSH agentless mode

### 5.1 User story

> As a developer with multiple Linux servers, I want to install Agentum only locally and manage agents on remote hosts via SSH without installing Agentum on each one. Agents must keep running if my laptop closes or SSH drops.

### 5.2 What ships

- "Hosts" abstraction (Local + Ssh kinds)
- `host add` CLI + PWA flow
- SSH connection management using `russh` (pure Rust)
- Remote tmux session lifecycle: create detached, attach via PTY over WebSocket, detach cleanly
- Optional bootstrap installer (tmux, git) on remote via package manager

### 5.3 Data model additions

```rust
struct Host { id: Uuid, name: String, kind: HostKind, created_at: DateTime<Utc>, last_seen: Option<DateTime<Utc>> }

enum HostKind {
    Local,
    Ssh { user: String, hostname: String, port: u16, auth: SshAuth, proxy_jump: Option<Box<SshConnection>> },
}

enum SshAuth { Key { path: PathBuf }, Agent }

struct Agent {
    id: Uuid, host_id: Uuid, name: String, command: String,
    worktree_path: PathBuf, tmux_session: String, status: AgentStatus, created_at: DateTime<Utc>,
}
```

### 5.4 SSH connection lifecycle

**On `host add`:** dial via russh → probe `command -v tmux && command -v git && uname -sr` → bootstrap-or-abort → persist.

**On `agent create`:** resolve worktree on host (`git worktree add`) → open channel → `tmux new-session -d -s agentum-<id> -c <wt> '<cmd>'` (the `-d` is critical) → persist with status Running → close channel.

**On `agent attach`:** open channel with PTY → `tmux attach -t agentum-<id>` → pipe channel ⇄ WebSocket. On client disconnect: close channel; tmux session survives.

**On `agent detach`:** server detects WS close, sends `Ctrl-b d`, closes channel. tmux persists.

**On `agent kill`:** `ssh user@host 'tmux kill-session -t agentum-<id>'`.

### 5.5 Connection pooling

Pool russh connections per host (multiplexed channels). Exponential-backoff reconnect on drop. Health-check every 30s with `tmux ls`.

### 5.6 Bootstrap (opt-in)

Detect package manager: apt-get / dnf / pacman / brew. Always ask user. Never silent.

### 5.7 Worktree management on remote hosts

Per-host default repos dir OR ad-hoc absolute path. Worktrees in `<repo>-worktrees/<branch>` sibling. Reuse existing worktree code over SSH.

### 5.8 Failure modes

- Host unreachable → mark `unreachable`, retry with backoff
- SSH auth failure → surface, no auto-retry
- Disk full → catch, surface
- tmux server crashed → restart tmux, mark agent `lost`
- Two clients attach simultaneously → allow (tmux handles it)

### 5.9 UI changes in the PWA

- "Hosts" tab with status indicators (green/yellow/red)
- "Add host" modal
- Agent creation modal: host dropdown
- Host name as agent card subtitle
- Filter view: "Agents on this host"

---

## 6. Phase 3: Tauri desktop shell

### 6.1 User story

> I want Agentum.app on Mac (or .AppImage / .msi) opening a native window with the UI, the local server running automatically. I don't want to remember a localhost URL.

### 6.2 What ships

- Tauri project wrapping the Svelte PWA
- Rust server lives **inside** the Tauri app (in-process tokio task, not sidecar)
- Bundles: macOS .dmg/.app (arm64+amd64), Linux .AppImage/.deb, Windows .msi
- CI workflow to build all desktop targets on tag
- `--headless` flag so the same binary runs as CLI server OR windowed app

### 6.3 Architecture

```
agentum/
├── crates/
│   ├── agentum-core/      # server logic
│   ├── agentum-cli/       # CLI binary
│   └── agentum-desktop/   # Tauri app, depends on agentum-core
├── frontend/              # Svelte PWA (shared)
└── install.sh
```

### 6.4 Tauri app behavior

On launch: free port (start 8080) → start agentum-core in tokio task on 127.0.0.1 → open window → on window close: shut down server, save state, exit.

Tauri menu items: Open data dir, Open logs, Restart server, Check for updates, Quit.

### 6.5 Data directory

- macOS: `~/Library/Application Support/Agentum/`
- Linux: `$XDG_DATA_HOME/agentum/` or `~/.local/share/agentum/`
- Windows: `%APPDATA%\Agentum\`

Use the `directories` crate. CLI mode on Linux server uses `/var/lib/agentum` via systemd `Environment=`.

### 6.6 Build pipeline

`desktop-release.yml` on tags `v*-desktop`:
- `macos-14`: arm64 dmg + .app
- `macos-13`: amd64 dmg + .app
- `ubuntu-22.04`: .AppImage + .deb
- `windows-latest`: .msi

No signing/notarization yet. Right-click → Open the first time on Mac.

### 6.7 Updates

Tauri updater plugin → `latest.json` on GitHub release. Daily check. Notify; apply on next restart. No silent updates.

### 6.8 Same binary, two faces

`agentum serve` on a server = no window. `Agentum.app` double-clicked = same core + Tauri window. PWA embedded in Tauri binary for offline use.

---

## 7. Technical choices and rationale

- **russh** over openssh subprocess: pure Rust, no system dep, channel multiplexing, programmatic PTY/reconnect.
- **tmux** over screen/dtach: ubiquitous, control-mode API, respects user's `~/.tmux.conf`, bootstrap is one apt-get away.
- **Tauri** over Electron: ~10 MB vs ~150 MB bundle. Rust-native. Tauri 2 supports mobile later.
- **No Windows server**: tmux doesn't run on Windows. Windows users use the Tauri desktop client + SSH to Linux/macOS.
- **SQLite** for state: small data set, bundled, single-file backup.

---

## 8. Implementation order

Strict order. Do not start phase N+1 before phase N is shipped and tagged.

1. **Set up the workspace** (`agentum-core`, `agentum-cli`, `agentum-desktop` crates). Move existing code into `agentum-core`. Make sure the CLI binary still works exactly as before.
2. **Cross-compile pipeline** for the four CLI targets (Linux + macOS, amd64 + arm64). GitHub Actions matrix. Verify a release upload works end-to-end.
3. **Install script.** Test on a fresh Ubuntu container, a fresh Debian VM, a fresh macOS user account.
4. **Tag v0.X.0** with Phase 1 complete. Post on r/selfhosted with the new install story.
5. **Host abstraction in the data model.** Migrate existing local-only data to the new schema (everything becomes "local host" with a default UUID).
6. **SSH connection layer** using russh. Get a `host add` + `host test` flow working from the CLI first. PWA integration second.
7. **Remote tmux session lifecycle.** Test thoroughly: create, attach, detach, reconnect, kill.
8. **PTY-over-WebSocket** path for SSH hosts. Reuse the local PTY-over-WS code as much as possible — the only difference is where the PTY lives.
9. **UI for hosts** in the PWA.
10. **Bootstrap** flow (apt/dnf/pacman/brew detection). Last because it's optional polish.
11. **Tag v0.X+1.0** with Phase 2 complete.
12. **Tauri scaffold.** Get the empty Tauri window opening with `agentum-core` started in-process.
13. **Tray icon, menu, updater.**
14. **Bundle pipeline.** Build .dmg, .AppImage, .deb, .msi on tag.
15. **Tag v0.X+2.0** with Phase 3 complete.

---

## 9. Out-of-band notes for the implementing agent

- Rust backend + Svelte frontend. Don't introduce a new language/framework without asking.
- Prefer small, well-documented commits over large refactors.
- All public CLI commands must have `--help`. Use `clap` derive macros.
- Surfaced errors must be actionable. `thiserror` for error types, `anyhow` only at binary boundary.
- Integration tests for install.sh (Docker, fresh ubuntu image). Unit tests for SSH layer using russh's test server.
- Logs to stderr in CLI mode, rotated file in daemon mode. Never stdout.
- Phase 2 demands reviewing every filesystem path for local-vs-remote correctness.

---

## 10. Open questions to answer before starting Phase 2

1. SSH key passphrase prompt in PWA or only via ssh-agent? **Recommend: ssh-agent only** (no secrets in web UI).
2. Support bootstrap on hosts without sudo? **Recommend: require sudo for bootstrap.**
3. "Shared agents" model where multiple clients see/attach to agents on a host? **Recommend: yes** (host is source of truth; any client with credentials may attach).

End of PRD.
