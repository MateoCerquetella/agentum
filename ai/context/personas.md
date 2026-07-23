# Personas

## Persona 1 — Solo Developer

### Goals
- Keep a single agent (e.g. Claude Code) running while away from the desk.
- Check progress from a phone without SSH.

### Pain Points
- Agents die when the laptop sleeps/closes.
- No mobile visibility into agent state.

### Main Workflows
- Spawn a session, attach to live output, kill/restart when it wanders.
- Glance at status from the PWA; get notified on completion/crash.

---

## Persona 2 — Power User (Multi-Agent)

### Goals
- Run Claude + Codex + OpenCode simultaneously on different projects.
- Switch between them without terminal sprawl.

### Pain Points
- 6+ terminal tabs to manage 3 agents across projects.
- Per-agent flag differences (YOLO spellings) are error-prone.

### Main Workflows
- Create sessions per (tool, project) with correct flags + YOLO toggle.
- Use board/channels to hand off tasks across agents.

---

## Persona 3 — Self-Hoster

### Goals
- Own the hardware and all data; no SaaS, no subscriptions, no telemetry.

### Pain Points
- Existing solutions are cloud-hosted or vendor-locked.

### Main Workflows
- `agentum serve` on a VPS/old box; trust the self-signed cert from a
  phone via the plain-HTTP cert server; manage via TUI + dashboard.

---

## Persona 4 — Mobile-First Developer

### Goals
- Monitor / kill / restart agents from a phone, no SSH.

### Pain Points
- `ssh + tmux attach` is impractical on mobile.

### Main Workflows
- Install the PWA to home screen; watch live terminals over WS; act on
  push notifications.
