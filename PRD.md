# Agentum — Product Requirements Document (PRD)

> **Status:** Beta  
> **Author:** Mateo Cerquetella  
> **Date:** May 2026  
> **Target audience:** AI coding assistants & developers implementing this product

---

## 1. Product Overview

**Agentum** is a self-hosted control plane for orchestrating multiple AI coding agents (Claude Code, Codex, OpenCode, Cursor, Gemini, and more) from a single terminal interface. It solves the problem of "dead time" — when a developer closes their laptop, AI agents stop working. Agentum keeps agents running persistently on a VPS, accessible from anywhere via a terminal UI (TUI) and a PWA for mobile.

**One-liner:** One terminal to manage all your AI coding agents, across all your projects, even when your laptop is closed.

---

## 2. Problem Statement

### Current Pain Points

1. **AI agents die when the laptop closes.** Claude Code, Codex, OpenCode all run locally. Close the lid → work stops. Go to the supermarket, visit family, step away → stalled progress.

2. **Terminal sprawl.** Each AI agent requires its own terminal window. Managing 3+ agents across multiple projects means 6+ terminal tabs/windows. Context-switching is painful.

3. **No mobile visibility.** Claude Code has `--remote`, but OpenCode and Codex don't. Checking agent progress from a phone requires SSH + tmux attach — impractical on mobile.

4. **Vendor lock-in / SaaS dependency.** Existing solutions are either SaaS (monthly fees, data leaves your machine) or platform-specific (one agent only).

### The Core Question

> How can AI coding agents keep working even when your laptop is closed, without paying for SaaS, and be manageable from a single interface — including from a phone?

---

## 3. Target Users

| Persona | Need |
|---------|------|
| **Solo developer** | Keep Claude Code running while away from desk; check progress from phone |
| **Power user (multi-agent)** | Run Claude Code + OpenCode + Codex simultaneously on different projects |
| **Self-hoster** | Refuses SaaS; owns hardware; wants full data sovereignty |
| **Mobile-first developer** | Wants to monitor/kill/restart agents from phone without SSH |

---

## 4. User Stories

### Epic 1 — Persistent Agents

> **As a developer**, I want my AI coding agents to keep running on a VPS when I close my MacBook, so that work continues while I'm away.

**Acceptance criteria:**
- Agent sessions survive laptop sleep/shutdown
- Agent output is viewable upon reconnection
- Multiple agents can run concurrently on the same VPS

### Epic 2 — Single TUI

> **As a developer**, I want to manage all my AI agent sessions from one terminal window, so I don't need multiple terminal tabs.

**Acceptance criteria:**
- TUI shows all active sessions with status (running, awaiting input, crashed, done)
- Can create new sessions from TUI (pick agent type + project directory)
- Can attach to any session's live output
- Can kill/restart sessions
- Keyboard-driven navigation (vim-style or arrow keys)

### Epic 3 — Mobile Access (PWA)

> **As a developer**, I want to check my AI agents' progress from my phone, without installing an app, so I can monitor work from anywhere.

**Acceptance criteria:**
- PWA accessible via HTTPS on the VPS
- Shows live status of all tmux sessions
- Can view terminal output (read-only initially)
- Can kill/restart sessions
- Push notifications when agents complete tasks or crash
- Installable to home screen (PWA manifest)

### Epic 4 — Multi-Project & Multi-Agent

> **As a developer**, I want to run different AI agents on different projects simultaneously, switching between them easily.

**Acceptance criteria:**
- Session picker shows all available agent binaries (Claude Code, Codex, OpenCode, etc.)
- Each session has its own working directory
- Sessions can target different git repos
- Agent-specific flags are handled per-tool (e.g. `--dangerously-skip-permissions` for Claude, `--dangerously-bypass-approvals-and-sandbox` for Codex)

---

## 5. Functional Requirements

### 5.1 Daemon (`agentum serve`)

| ID | Requirement | Priority |
|----|-------------|----------|
| F1 | Boot a tmux server on startup | P0 |
| F2 | Spawn AI agent CLI inside a tmux pane per session | P0 |
| F3 | Stream pane output to connected clients via WebSocket | P0 |
| F4 | Persist session metadata (name, workdir, tool, model, flags) in SQLite | P0 |
| F5 | Emit events: `AgentFinished`, `AwaitingInput`, `Crashed` | P1 |
| F6 | Auto-kill idle sessions after configurable timeout | P2 |
| F7 | Support connection profiles for multiple VPS endpoints | P1 |

### 5.2 TUI (`agentum terminal`)

| ID | Requirement | Priority |
|----|-------------|----------|
| F8 | Keyboard-driven terminal UI (Rust + Svelte) | P0 |
| F9 | List all active sessions with live status indicators | P0 |
| F10 | Create new sessions: pick tool, project directory, model, flags | P0 |
| F11 | Toggle YOLO mode per session (auto-approve tool calls) | P0 |
| F12 | View live session output (scrollable) | P0 |
| F13 | Kill / restart sessions | P0 |
| F14 | Profile switcher for multi-VPS setups | P1 |
| F15 | YOLO marker translation: map Claude's `--dangerously-skip-permissions` to each agent's equivalent flag | P0 |

### 5.3 Dashboard (SvelteKit SPA)

| ID | Requirement | Priority |
|----|-------------|----------|
| F16 | Web dashboard served from daemon (embedded SPA) | P0 |
| F17 | Session list with real-time status (WebSocket) | P0 |
| F18 | Create new sessions via dialog (tool picker, directory input) | P0 |
| F19 | View session output in browser | P1 |
| F20 | Multi-endpoint support (profile switcher, localStorage) | P1 |
| F21 | Responsive design (desktop-focused, mobile-friendly) | P2 |

### 5.4 PWA (Mobile)

| ID | Requirement | Priority |
|----|-------------|----------|
| F22 | Standalone PWA served from VPS over HTTPS | P1 |
| F23 | View live terminal sessions (read-only, polling-based) | P1 |
| F24 | Kill / restart sessions from mobile | P2 |
| F25 | Push notifications on agent completion/crash | P2 |
| F26 | Installable to home screen with manifest + service worker | P1 |
| F27 | No App Store dependency — pure PWA | P0 |

### 5.5 Agent Adapters

| ID | Requirement | Priority |
|----|-------------|----------|
| F28 | Pluggable agent adapter system (`ToolAdapter` trait) | P0 |
| F29 | Adapters for: Claude Code, Codex, OpenCode, Cursor, Gemini, Aider, Hermes | P0 |
| F30 | Per-agent binary probing (is the CLI installed?) | P1 |
| F31 | Per-agent YOLO flag translation | P0 |
| F32 | Per-agent crash/awaiting-input signature detection | P1 |
| F33 | Passthrough mode for unsupported agents (generic shell wrapper) | P2 |

---

## 6. Non-Functional Requirements

| ID | Requirement | Target |
|----|-------------|--------|
| NF1 | **Self-hosted** — zero external dependencies at runtime | Must |
| NF2 | **Performance** — TUI < 100ms latency for session list refresh | Should |
| NF3 | **Reliability** — daemon survives agent crashes without data loss | Must |
| NF4 | **Security** — HTTPS for dashboard/PWA, auth token for API | Must |
| NF5 | **Resource usage** — daemon < 100MB RAM idle, < 500MB with 5 agents | Should |
| NF6 | **Binary size** — TUI binary < 15MB (no Electron) | Should |
| NF7 | **Cross-platform** — daemon and TUI run on Linux (primary), macOS (secondary) | Must/Should |
| NF8 | **No telemetry** — zero data leaves the VPS | Must |

---

## 7. Architecture

### High-Level

```
┌─────────────┐     Tailscale/WireGuard      ┌──────────────────────────┐
│  MacBook    │ ◄──────────────────────────► │  VPS (Arch Linux)        │
│             │   Bidirectional sync of       │                          │
│  Developer/ │   Developer/ folder           │  ┌────────────────────┐  │
│             │                               │  │  agentum serve     │  │
│  Claude Code│                               │  │  (daemon)          │  │
│  (local)    │                               │  │  ├─ SQLite DB      │  │
└─────────────┘                               │  │  ├─ tmux server    │  │
                                              │  │  ├─ HTTP API :8822 │  │
        │                                     │  │  ├─ WS API :8822   │  │
        │                                     │  │  └─ TLS cert :8823 │  │
        ▼                                     │  └────────────────────┘  │
┌─────────────┐                               │                          │
│  Phone/PWA  │ ◄──── HTTPS ────────────────► │  Dashboard (embedded SPA)│
│  (browser)  │                               │  PWA (static files)      │
└─────────────┘                               └──────────────────────────┘
```

### Component Architecture

```
crates/
  agentum-core/        # Shared types: Session, Status, Event
  agentum-store/       # SQLite repository (sqlx)
  agentum-tmux/        # tmux wrapper: new-session, send-keys, capture-pane, kill
  agentum-watchdog/    # Background loop: tails panes, emits events
  agentum-executor/    # ToolAdapter trait + per-agent argv builders + YOLO translation
  agentum-server/      # axum HTTP+WS API + TLS + auth + embedded SPA
  agentum/             # CLI binary + TUI (commands/terminal/)

dashboard/             # SvelteKit SPA → builds to dashboard/build/ → embedded in daemon
```

### Data Flow

1. User creates session via TUI, dashboard, or API
2. `agentum-executor` builds the CLI command for the selected agent
3. `agentum-tmux` spawns a new tmux pane running that command
4. `agentum-watchdog` tails the pane, parses output for events
5. Events are emitted via WebSocket to connected clients
6. Session metadata is persisted in SQLite

---

## 8. Technical Stack

| Layer | Technology |
|-------|-----------|
| **Language** | Rust (daemon, TUI, executor) |
| **TUI framework** | Svelte (rendered in terminal via custom renderer) |
| **Web dashboard** | SvelteKit (SPA, embedded via `rust-embed`) |
| **HTTP/WS server** | axum (Rust) |
| **Database** | SQLite via sqlx |
| **Session isolation** | tmux (one pane per agent) |
| **Networking** | Tailscale + WireGuard for VPS connectivity |
| **File sync** | Syncthing (bidirectional Developer/ folder sync) |
| **PWA** | Service Worker + Web App Manifest |

---

## 9. Current State & Roadmap

### ✅ Done (Beta)

- [x] Daemon with SQLite persistence
- [x] tmux session management (create, attach, kill, capture)
- [x] Agent adapters: Claude Code, Codex, OpenCode, Cursor, Gemini
- [x] YOLO marker translation across all agents
- [x] TUI with session list, create, attach, kill
- [x] SvelteKit dashboard (embedded in daemon)
- [x] Agent binary probing + availability gating
- [x] Connection profiles (multi-VPS)
- [x] Bidirectional folder sync (Syncthing)
- [x] Landing page (built with v0 + DeepSeek + Cursor + Claude)
- [x] Claude Design (OSS) installed on VPS for prototyping

### 🔄 In Progress

- [ ] PWA terminal viewer (mobile)
- [ ] PWA session manager (mobile)
- [ ] Push notifications for agent events

### 📋 Planned

- [ ] Polish remaining TUI rough edges
- [ ] PWA install prompt + offline support
- [ ] Agent output search / filtering
- [ ] Session templates (pre-configured agent + project combos)
- [ ] Public beta release
- [ ] Agent marketplace / community adapters
- [ ] macOS daemon support (currently Linux-only)

---

## 10. Out of Scope (v1)

- Cloud-hosted version (self-hosted only, by design)
- Native iOS/Android apps (PWA only)
- Agent-to-agent communication / multi-agent collaboration
- Built-in code editor (use external editor + LazyGit for diffs)
- User authentication with OAuth/OIDC (simple token auth for now)
- GPU/ML model hosting (agentum orchestrates agents, doesn't run models)
- Windows support

---

## 11. Success Metrics

| Metric | Target |
|--------|--------|
| Time from idea to running agent | < 30 seconds |
| Agents survive laptop sleep | 100% |
| TUI session list refresh | < 100ms |
| Multiple concurrent agents | ≥ 5 stable |
| PWA load time (cached) | < 2 seconds |
| Daemon memory (5 agents) | < 500MB |
| Zero-data-leaving-VPS | 100% |

---

## 12. Design Principles

1. **Self-hosted or die.** No SaaS. No subscriptions. No telemetry. User owns everything.
2. **Terminal-first.** The TUI is the primary interface. Web dashboard is secondary.
3. **One binary.** No complex install. `cargo build --release` → one binary that does everything.
4. **Agent-agnostic.** Any CLI tool that fits the adapter pattern works. No vendor lock-in.
5. **Mobile as companion.** PWA for monitoring, not primary workflow. Terminal wins.
6. **Keyboard-driven.** Every action must be doable without a mouse.

---

## 13. Appendix: Agent Adapter Interface

```rust
pub trait ToolAdapter {
    /// Unique tool identifier (e.g., "claude", "codex")
    fn name(&self) -> &'static str;

    /// Build the CLI command to launch the agent
    fn launch(&self, session: &Session) -> Command;

    /// Optional YOLO flag for this tool (e.g., "--dangerously-skip-permissions")
    fn yolo_flag(&self) -> Option<&'static str> { None }

    /// Trigger phrase that puts agent in "compact" mode
    fn compact_trigger(&self) -> Option<&'static str> { None }

    /// Patterns that indicate the agent crashed
    fn crash_signatures(&self) -> Vec<&'static str>;

    /// Pattern that indicates the agent is busy working
    fn busy_signature(&self) -> Option<&'static str> { None }

    /// Patterns that indicate the agent is waiting for user input
    fn awaiting_input_signatures(&self) -> Vec<&'static str>;

    /// Binary name on $PATH (defaults to name())
    fn binary(&self) -> &'static str { self.name() }
}
```

### Per-Tool YOLO Flags

| Tool     | Flag                                          |
|----------|-----------------------------------------------|
| claude   | `--dangerously-skip-permissions`              |
| codex    | `--dangerously-bypass-approvals-and-sandbox`  |
| cursor   | `--force`                                     |
| gemini   | `--yolo`                                      |
| hermes   | `--yolo`                                      |
| opencode | (none — unverified)                           |
| aider    | (none — unverified)                           |

---

*This PRD is designed to be given to any AI coding assistant as a complete specification for implementing Agentum or its features. It contains enough context, architecture, and technical detail to start building immediately.*
