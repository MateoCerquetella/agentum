# Remote tmux session discovery & attach

Date: 2026-06-10
Status: approved

## Goal

When a project's repo lives on an SSH host, agentum lists the tmux
sessions already running on that host (project-related first) and lets
the user click one to open it as a live terminal tab, streamed through
the embedded server exactly like any agentum-managed session.

## Non-goals

- Full adoption of external sessions as first-class agent sessions
  (no watchdog signatures, no agent-task panel).
- Background polling of remote hosts.
- True `tmux attach` fidelity (status bar, prefix keys). We stream the
  active pane via pipe-pane, same as managed sessions.

## 1. Discovery (server)

- `host_runtime::list_tmux_sessions(host) -> Result<Vec<DiscoveredTmuxSession>>`
  in `crates/agentum-server/src/host_runtime.rs`. One SSH round trip
  over the existing ControlMaster path (`ssh_output`, 12s timeout):

  ```
  tmux list-panes -a -F '#{session_name}\t#{session_attached}\t#{session_created}\t#{pane_current_command}\t#{pane_current_path}'
  ```

  Pane lines are grouped by session name. Sessions named `agentum-*`
  are excluded (already managed). `no server running` / tmux missing
  → `Ok(vec![])`, not an error. Works for `HostKind::Local` too (runs
  the same command locally).

- Types:

  ```rust
  pub struct DiscoveredTmuxSession {
      pub name: String,
      pub attached: bool,
      pub created_at: Option<i64>,   // unix seconds
      pub panes: Vec<DiscoveredPane>,
  }
  pub struct DiscoveredPane {
      pub command: String,
      pub cwd: String,
  }
  ```

- Route: `GET /api/hosts/{id}/tmux-sessions?path=<repo_path>` in
  `crates/agentum-server/src/routes/hosts.rs`. The optional `path` is
  not a server-side filter: the response adds `related: bool` per
  session (true when any pane cwd is under `path`), so the UI renders
  "related first" plus a "show all" expander with no second call.

## 2. Attach (server)

- `POST /api/hosts/{id}/tmux-sessions/{name}/attach` creates a normal
  `Session` row:
  - `tool = "terminal"`
  - tmux target = the external session name (not `agentum-<uuid>`)
  - `workdir` = first pane's cwd
  - `host_id` = the host
  - marked **external** (cheapest mechanism at implementation time:
    a marker in `Session::flags` such as `--agentum-external-tmux`,
    or a column if flags prove awkward)
- Lifecycle rule: closing/deleting an external session record must
  **never kill the underlying tmux session** — only disarm pipe-pane
  and stop the remote tail.
- Streaming and input reuse existing machinery unchanged: idempotent
  `pipe_pane -o` arming on connect, `spawn_remote_pane_tail` over
  `~/.agentum/panes/<uuid>.log`, input via the existing send-keys path.
- Attach is idempotent client-side: if an agentum session already
  exists for (host, tmux name), return/focus it instead of creating a
  duplicate.

## 3. UI (desktop)

- New collapsible "Remote tmux" section in the project sidebar, shown
  only when the active project resolves to an SSH host (via the
  existing `connectionId → server host id` mapping in
  `server-host-client.ts`).
- Fetch on project activation and on manual refresh (refresh icon).
  No background polling.
- Related sessions first (name + current command + attached dot);
  "Show all (N)" reveals the rest.
- Click → attach route → opens as a normal terminal tab. Re-click
  focuses the existing tab (match on host + tmux name).
- State lives in `HostsSlice` (or a minimal extension of it).

## 4. Errors & edge cases

- Host unreachable / tmux missing → quiet inline message in the
  section; sidebar never blocks.
- Remote session killed while attached → existing "session ended"
  handling.
- Two clients attaching the same external session → fine,
  `pipe-pane -o` is idempotent.

## 5. Testing

- Unit: parsing of `list-panes -a -F` output (multi-session grouping,
  `agentum-*` exclusion, `no server running` → empty).
- Integration (local host kind): scratch tmux server, create a
  non-`agentum` session, assert discovery; attach; assert closing the
  agentum session leaves the tmux session alive.
