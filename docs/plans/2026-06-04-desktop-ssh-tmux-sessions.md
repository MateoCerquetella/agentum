# Desktop SSH + tmux sessions via embedded server host_runtime

**Goal:** the desktop opens remote (SSH) worktrees the same way `agentum terminal`
(the TUI) does — the agent runs in **tmux on the remote host over SSH**, streamed
back through the embedded server, with the live status / green-✓ done / finish
notification the local tmux sessions already have. Server `/api/hosts` is the
source of truth for remote hosts.

## What already exists (no work)
- `/api/hosts` — SSH host registry (create/test/bootstrap/install-agent).
- `POST /api/sessions` accepts `host_id`; the `HostKind::Ssh` branch runs the
  agent in tmux on the remote (`create_session_on_host`).
- `stream_remote_session` streams the remote pane via `host_runtime::capture_pane_ansi`.
- `host_runtime.rs` — full SSH+tmux backend (`ssh_stdout`, `capture_pane_*`,
  `send_*`, `resize_*`).
- Desktop server-session title handling (status / done / notification) — fires for
  ANY session that streams OSC titles, so it works for remote once titles flow.

## The gaps
1. Client `createSession` declares `host_id?` but **drops it** — only sends
   `{name, workdir, tool}`. → forward it.
2. `ensureWorkspaceSession` takes only `{workdir, tool}` — no host. → add `hostId`,
   match on `workdir+tool+host`.
3. `connectPaneServerSession` always creates a local session — must resolve the
   worktree's repo host and pass it.
4. `connectionId` (native ssh target id on the repo) ≠ server `host_id`. → mirror
   the native target into a server Host (idempotent by SSH coords) and resolve it.
5. Remote stream carries no OSC title (tmux set-titles off) — port the local
   `pane_title` injection to `stream_remote_session` (+ `host_runtime::pane_title`).

## Increments

### A — host-aware create + remote title (server + client core)
- `host_runtime::pane_title(host, target)`: Local → `agentum_tmux::pane_title`;
  Ssh → `ssh_stdout("tmux display-message -p -t {target} '#{pane_title}'")`.
- `stream_remote_session`: add a 400ms title-poll branch injecting
  `\x1b]0;<title>\x07` on change (mirror the local `/stream` injection).
- Client `createSession`: include `host_id` in the body when present.
- `ensureWorkspaceSession({workdir, tool, hostId?})`: match on host too; pass
  `host_id` to `createSession`.

### B — resolve repo connectionId → server host_id
- `resolveServerHostIdForConnection(connectionId)`: read the native ssh target,
  create-or-get a server Host by SSH coords (user@host:port), cache id→id.
- `connectPaneServerSession`: look up the worktree's repo `connectionId`; if set,
  resolve host_id and pass to `ensureWorkspaceSession`.

### C — status / done / notification
- No new work: the existing server-session title handling fires once remote
  titles stream. Verify end-to-end against a real host.

## Verification
- `cargo build` + UI build clean; existing LOCAL sessions still stream (no
  regression — title injection path is shared).
- Remote end-to-end needs a real SSH host (user has `multi-host-federation` etc.):
  open a remote worktree → agent spawns in remote tmux → streams back → sidebar
  shows working/idle/✓ + fires the finish notification.
