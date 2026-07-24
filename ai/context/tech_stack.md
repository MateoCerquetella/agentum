# Tech Stack

## Frontend

- Svelte 5 / SvelteKit 2 SPA (embedded into the daemon via `rust-embed`)
- CodeMirror 6 (notes editor)
- PWA: service worker (offline shell) + web app manifest
- Pure-CSS theme engine (Terminal Dark, Paperlight)

---

## Backend

- Rust (daemon, TUI, executor)
- axum (HTTP + WebSocket API), tokio runtime
- TUI: ratatui-style terminal UI under `crates/agentum-cli/commands/terminal/`

---

## Database

- SQLite via `sqlx` (WAL mode)
- XDG-compliant path: `$XDG_DATA_HOME/agentum/db.sqlite` (Linux + macOS)

---

## Infrastructure

- tmux — session isolation (one pane per agent)
- rustls + rcgen — self-signed TLS, auto-generated on first boot;
  plain-HTTP cert server on :8823 for trust-on-first-use
- WireGuard / Tailscale — host connectivity
- Syncthing — bidirectional folder sync
- Distribution: single static binary (`cargo install`, `curl | sh`, tarball)

---

## AI Tools (orchestrated agents)

- First-class adapters: Claude Code, Codex, Gemini, Hermes
- Additional: Cursor (`cursor-agent`)
- Passthrough / unverified YOLO: OpenCode, Aider
- Anything else on `$PATH` via generic passthrough
