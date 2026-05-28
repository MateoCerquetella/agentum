# PRD: SSH Host Readiness (TUI + CLI)

**Product:** agentum  
**Feature:** SSH host preflight / readiness  
**Audience:** Implementation agent (Claude Code or human)  
**Scope:** Terminal TUI + `agentum hosts` CLI + daemon API. **No dashboard.**  
**Status:** Ready for implementation  
**Depends on:** Existing SSH agentless hosts (migration `0018_hosts.sql`, `host_runtime`, `/api/hosts`, TUI Host field in New session)

---

## 1. Problem

Users run Agentum only on their Mac and manage remote machines (e.g. fresh Omarchy) via **SSH hosts** — no `agentum` binary on the remote.

Today:

- `agentum hosts test` only checks SSH + coarse `tmux`/`git` booleans.
- Agent availability is probed lazily in New session via `/api/agents?host_id=`.
- There is **no single preflight** that answers: “What is missing on this machine, required vs optional, and how do I install it?”

Users want **Hermes-style clarity**: connect → see full dependency report → copy install commands → optionally bootstrap system packages.

---

## 2. Goals

1. On **host add**, **host test**, and **TUI host selection**, run a **structured readiness check** over SSH.
2. Report **required** deps (`tmux`, `git`, SSH reachability) and **optional** deps (all `probed_tools()` agent CLIs).
3. Provide **install hints** per missing item (static, server-side; not auto-install agents).
4. **Phase 2:** Optional **bootstrap** of `tmux` + `git` via package manager after **explicit user confirmation**.
5. **Never** install or push the Agentum binary to SSH hosts.

---

## 3. Non-goals

- Dashboard UI changes (`dashboard/**` out of scope).
- Installing Agentum on SSH hosts.
- Auto-installing agent CLIs (`claude`, `codex`, `hermes`, etc.) without user action.
- Full remote watchdog / activity detection over SSH.
- Readiness for **Servers** (HTTP profiles to another `agentum serve`) — separate future work.
- Replacing `russh` with raw `ssh` is out of scope (keep current `ssh` subprocess approach in `host_runtime.rs`).

---

## 4. Concepts (do not confuse)

| Term | Meaning |
|------|---------|
| **Server / Profile** | Another Agentum daemon (`agentum serve`) reached over HTTP/WS. Remote **must** have Agentum. TUI: Ctrl-S. |
| **Host (SSH)** | Machine controlled from **local** `agentum serve` via `ssh` + `tmux`. Remote **must not** have Agentum. TUI: Host field in New session; CLI: `agentum hosts`. |

**This PRD applies only to SSH Hosts.**

---

## 5. User stories

### US-1: Add host and see what’s missing

> As a user on my Mac, when I run `agentum hosts add omarchy --user me --hostname omarchy.local`, I see a table of required and optional dependencies on Omarchy, with install hints for anything missing.

**Acceptance:**

- After save, readiness runs automatically (via local daemon API).
- Output shows ✓/✗ for `tmux`, `git`, and each probed agent.
- Exit code **1** if required deps missing (unless `--force` in phase 3).
- Prints suggested install command per missing item.

### US-2: TUI hosts manager

> As a TUI user, I press **Ctrl-H** to open a Hosts overlay, select an SSH host, and see the same readiness report without using the dashboard.

**Acceptance:**

- Overlay lists hosts with status dot (green / yellow / red) from last readiness.
- **Enter** or **t** runs/refreshes readiness for selected host.
- Detail pane shows required + agents + hints.
- **Esc** closes overlay.

### US-3: New session blocked without tmux/git

> When I spawn a session on an SSH host missing `tmux` or `git`, the TUI blocks submit and tells me to fix via Ctrl-H.

**Acceptance:**

- Tab-cycling Host in New session triggers readiness (or uses cache < TTL).
- Missing required deps → `form.error` with short message.
- Submit blocked until required deps pass (no “proceed anyway” in MVP).

### US-4: Bootstrap system deps (phase 2)

> On a fresh Arch box, I can confirm installation of `tmux` and `git` from the TUI or CLI without typing package commands manually.

**Acceptance:**

- `agentum hosts bootstrap omarchy --yes` or TUI **b** + Confirm runs one SSH command.
- Only `tmux` and `git`; never agent CLIs.
- Re-runs readiness after success.

---

## 6. Dependency model

### 6.1 Required (block spawn)

| ID | Check | Notes |
|----|--------|------|
| `ssh` | Connection succeeds | Implicit in readiness run |
| `tmux` | `command -v tmux` on remote | |
| `git` | `command -v git` on remote | Needed for worktrees / repo ops |

`ok = true` only when all required checks pass.

### 6.2 Optional — agents (warn / dim; block only if user picks unavailable tool)

Probe every tool from `agentum_executor::probed_tools()`:

- **First-class:** `claude`, `codex`, `cursor`, `gemini`, `hermes`
- **Passthrough probed:** `opencode`, `aider`

Use `agentum_executor::binary_for(tool)` for PATH lookup (e.g. `cursor` → `cursor-agent`).

### 6.3 Install hints

- **Server-side static table** (new module, e.g. `host_install_hints.rs`).
- Per agent: URL or one-line install instruction (no auto-run).
- Per required package: template from detected package manager:
  - `apt` → `sudo apt-get install -y tmux git`
  - `dnf` → `sudo dnf install -y tmux git`
  - `pacman` → `sudo pacman -S --needed tmux git`
  - `brew` → `brew install tmux git` (if remote is macOS over SSH)
  - `unknown` → generic “install tmux and git with your package manager”

---

## 7. Technical design

### 7.1 SSH preflight (one round trip)

**Location:** `crates/agentum-server/src/host_runtime.rs`

Add `readiness(host: &Host) -> Result<HostReadiness>`:

1. For `HostKind::Local`: run local `which` checks (same tiers; no SSH).
2. For `HostKind::Ssh`: run **one** SSH command with inline bash that:
   - Prints `uname -sr`
   - Detects package manager (`command -v apt-get`, `pacman`, `dnf`, `brew`)
   - For each required binary and each agent binary: `command -v` → JSON
   - Outputs **single line JSON** to stdout

**Constraints:**

- Reuse existing `SSH_TIMEOUT` (12s) and `ssh_command()` options (`BatchMode`, `ConnectTimeout=8`, etc.).
- Fix legacy `probe()` SSH branch: today it sets `tmux: true, git: true` on any SSH success — misleading.

**Do not** SCP or install any persistent script on the remote. Inline script only.

### 7.2 Data types

Add to `agentum-core` (preferred for sharing) or server-only with duplicate in CLI client:

```rust
pub struct HostReadiness {
    pub ok: bool,
    pub message: String,
    pub system: HostSystemInfo,
    pub required: Vec<DepCheck>,
    pub agents: Vec<AgentDepCheck>,
}

pub struct HostSystemInfo {
    pub uname: Option<String>,
    pub pkg_manager: String, // "apt" | "dnf" | "pacman" | "brew" | "unknown"
}

pub struct DepCheck {
    pub id: String,           // "tmux", "git"
    pub label: String,
    pub installed: bool,
    pub install_hint: Option<String>,
    pub bootstrapable: bool,  // true for tmux/git
}

pub struct AgentDepCheck {
    pub id: String,           // tool id
    pub binary: String,
    pub installed: bool,
    pub path: Option<String>,
    pub install_hint: Option<String>,
    pub bootstrapable: bool,  // always false
}
```

Keep existing `HostProbe` for backward compatibility or make `POST /test` return `HostReadiness` (prefer extending test to call readiness internally).

### 7.3 HTTP API

**File:** `crates/agentum-server/src/routes/hosts.rs`

| Method | Path | Body | Response |
|--------|------|------|----------|
| `GET` | `/api/hosts/{id}/readiness` | — | `HostReadiness` |
| `POST` | `/api/hosts/{id}/test` | — | `HostReadiness` (delegate to readiness) |
| `POST` | `/api/hosts/{id}/bootstrap` | `{ "items": ["tmux","git"], "confirm": true }` | `HostReadiness` (phase 2) |

**Bootstrap (phase 2):**

- Reject if `confirm != true`.
- Reject items other than `tmux`, `git`.
- Build command from `pkg_manager` + `items`.
- Run via SSH with timeout; return stderr on failure.
- On success, call `store.update_host_seen(id)`.

**Auth:** Same as other `/api/hosts` routes (existing middleware).

### 7.4 Hint module

**New file:** `crates/agentum-server/src/host_install_hints.rs`

```rust
pub fn agent_install_hint(tool: &str) -> &'static str;
pub fn bootstrap_command(pkg_manager: &str, packages: &[&str]) -> Option<String>;
pub fn fill_hints(readiness: &mut HostReadiness);
```

Called after parsing remote JSON, before returning to client.

### 7.5 CLI

**Files:** `crates/agentum-cli/src/cli.rs`, `commands/hosts.rs`

Extend `HostsCmd`:

```rust
enum HostsCmd {
    List,
    Add { name, user, hostname, port, key },
    Test { name },
    Readiness { name },            // NEW
    Bootstrap { name, yes: bool }, // phase 2
    Rm { name },
    Forget { host },
}
```

**Behavior:**

- `hosts add`: after `create_host`, HTTP `GET /api/hosts/{id}/readiness` (requires `agentum serve` running). Print formatted table. Exit 1 if `!ok`.
- `hosts readiness <name>`: print table only.
- `hosts test <name>`: alias to readiness (keep friendly one-line summary for scripts).
- `hosts bootstrap <name> [--yes]`: phase 2.

**Output format (human):**

```text
Host: omarchy (ssh me@omarchy.local:22)
System: Linux 6.x · pkg_manager=pacman

REQUIRED
  [ ] tmux     — sudo pacman -S --needed tmux
  [x] git

AGENTS (optional)
  [ ] claude   — https://docs.anthropic.com/...
  [x] codex
  ...

Ready: no (1 required missing)
```

### 7.6 TUI

**Files:** `crates/agentum-cli/src/commands/terminal/app.rs`, `ui.rs`, `api.rs`

#### New overlay: `Overlay::Hosts`

**Keybinding:** `Ctrl-H` (document in Help overlay).

**State:**

```rust
struct HostsOverlay {
    hosts: Vec<Host>,
    cursor: usize,
    detail: Option<HostReadiness>,
    loading: bool,
    error: Option<String>,
}
```

**Keys:**

| Key | Action |
|-----|--------|
| Ctrl-H | Toggle / open Hosts overlay |
| ↑/↓ | Move selection |
| Enter, t | Fetch/refresh readiness for selected SSH host |
| a | Add host form → `create_host` → readiness |
| d | Delete host (with confirm if sessions exist) |
| b | Bootstrap (phase 2) → `Overlay::Confirm` |
| Esc | Close |

**App cache:** `host_readiness_cache: HashMap<Uuid, (Instant, HostReadiness)>` — optional TTL 5 min.

**Status dots in list:**

- Green: `readiness.ok`
- Yellow: required ok, some agents missing
- Red: required missing or SSH error

#### New Session integration

On `NewSessionField::Host` Tab cycle:

1. Call `client.host_readiness(host_id)` (not separate `/api/agents` SSH round trip if readiness includes agents — **prefer single SSH call**).
2. Update `agent_availability` from `readiness.agents`.
3. Set `form.error` if `!readiness.ok`.
4. Set `form.host_hint` for agent-only gaps (optional field on `NewSessionForm`).

On submit: if cached readiness for host has `!ok`, block and show error.

#### API client additions (`terminal/api.rs`)

```rust
async fn host_readiness(&self, id: Uuid) -> Result<HostReadiness>;
async fn bootstrap_host(&self, id: Uuid, items: &[&str]) -> Result<HostReadiness>; // phase 2
```

#### Command palette

Add entries: `hosts`, `host readiness`.

---

## 8. Implementation phases

### Phase 1 — Readiness (MVP) — ship this first

1. Types + `host_install_hints.rs`
2. `host_runtime::readiness()` + JSON parse tests
3. `GET /api/hosts/{id}/readiness`; wire `POST .../test` to same
4. CLI: `hosts readiness`, upgrade `hosts add` + `hosts test`
5. TUI: `Overlay::Hosts` (Ctrl-H), New session hooks, submit guard
6. Manual test: Mac + SSH VM

**Definition of done:** User can add Omarchy, see full missing list in CLI and TUI, cannot spawn without tmux+git.

### Phase 2 — Bootstrap

1. `POST /api/hosts/{id}/bootstrap`
2. CLI `hosts bootstrap --yes`
3. TUI **b** + confirm + re-test

### Phase 3 — Polish (optional)

- `hosts add --force`
- Background readiness refresh in TUI
- “Proceed anyway” for power users
- Local host row in Ctrl-H overlay

---

## 9. Files to modify (checklist)

```
crates/agentum-core/src/lib.rs              # HostReadiness types
crates/agentum-server/src/host_runtime.rs   # readiness SSH script
crates/agentum-server/src/host_install_hints.rs  # NEW
crates/agentum-server/src/routes/hosts.rs   # routes
crates/agentum-server/src/lib.rs            # mod hints
crates/agentum-cli/src/cli.rs               # HostsCmd variants
crates/agentum-cli/src/commands/hosts.rs    # CLI impl
crates/agentum-cli/src/commands/terminal/api.rs
crates/agentum-cli/src/commands/terminal/app.rs
crates/agentum-cli/src/commands/terminal/ui.rs
```

**Do not modify:** `dashboard/**`

---

## 10. Testing

### Automated

- Unit: parse sample JSON from preflight script → `HostReadiness`
- Unit: `bootstrap_command()` for each pkg manager
- Unit: `fill_hints()` sets bootstrapable only on tmux/git
- Route: `GET readiness` returns 404 for unknown host id
- CLI: table formatter snapshot test (optional)

### Manual

1. Mac with `agentum serve`, SSH to fresh Arch VM without tmux.
2. `agentum hosts add vm ...` → see tmux missing + pacman hint.
3. `agentum terminal` → Ctrl-H → red dot → Enter → same detail.
4. Install tmux/git manually → re-test → green → New session on host works.

---

## 11. Edge cases

| Case | Behavior |
|------|----------|
| `agentum serve` not running when CLI `hosts add` | Error: start serve first (MVP) |
| SSH auth failure | `ok=false`, message = stderr, no bootstrap |
| SSH timeout | `ok=false`, message = timeout |
| Host has running sessions, delete | Existing store guard; surface error in TUI |
| Local host (`00000000-...`) in overlay | Show local readiness; no SSH |
| Old daemon without `/readiness` | TUI/CLI fall back to legacy `HostProbe` (optional compat) |

---

## 12. Security

- Bootstrap requires explicit `confirm: true` in API and TUI Confirm overlay.
- Never run bootstrap without user action.
- SSH stays `BatchMode=yes`; no password prompts in daemon.
- Bootstrap commands logged at `info` level with host name (not passwords).

---

## 13. Reference: existing code

- Host CRUD: `crates/agentum-store/src/lib.rs` (`list_hosts`, `create_host`, …)
- SSH ops: `crates/agentum-server/src/host_runtime.rs`
- Agent probe list: `agentum_executor::probed_tools()`, `binary_for()`
- TUI host field: `NewSessionForm::host_id`, `cycle_host()`, `draw_new_session_overlay`
- PRD SSH vision: `docs/plans/AGENTUM_V2_PRD.md` §5.4–5.6 (bootstrap-or-abort on host add)

---

## 14. Success metrics

- Fresh remote host: user sees complete dependency report in < 15s (one SSH round trip).
- Zero false “ready” when tmux is missing (fix current probe bug).
- No dashboard changes required for workflow completion.

---

## 15. Handoff note for implementer

Start with **Phase 1 only**. Run `cargo test` on touched crates and manually verify Ctrl-H + `hosts add` on a real SSH host. Ask before adding dashboard or auto-installing agent CLIs.
