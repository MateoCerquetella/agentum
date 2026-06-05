# Desktop ADE — Hosts-first sidebar

**Date:** 2026-06-05
**Status:** Design approved → specs drafted, pending spec review. Split into 3
incremental specs: `ai/specs/002-sidebar-host-grouping` (grouping + host header +
count badge), `ai/specs/003-sidebar-host-metadata` (OS/arch line),
`ai/specs/004-sidebar-session-activity` (ctx% chip + PRIMARY slot + active card).
**Area:** `crates/agentum-desktop/ui/` (React + Vite SPA)

## 1. Summary

Bring the TUI's hierarchical session sidebar to the desktop app (ADE). The
desktop sidebar today is a flat, virtualized **worktree** list with optional
grouping (`repo` / `workspace-status` / `pr-status`) and **no host level at
all**. We restructure it so the primary layout is:

```
HOST  →  PROJECT (repo)  →  WORKTREE (session)
```

This mirrors the TUI's three-level tree (`Group` = host/profile → `Project` =
workdir → `Leaf` = session) built by `Tree::build_with_profiles()` in
`crates/agentum-cli/src/commands/terminal/app.rs`. The desktop already groups by
**repo** and already renders **worktree leaves**, so the new work is the **host
super-level** on top, plus two enrichments from the mockup: a host **OS/arch
metadata line** and an **active-session card** (last agent message + last tool
call).

Target layout:

```
HOSTS
└─ ▾ 🖥  studio        localhost · macOS 15 · M3 Max      ● 3
   └─ ▾ 📁 agentum                                          3
      ├─ ◐ worktree-isolation   mateo/worktrees   ctx 71% ⎇
      │     ┌─────────────────────────────────────────┐
      │     │ ✦ Wired the worktree help…        now    │  ← active-session card
      │     │ ⚒ Bash cargo clippy --all-t…             │
      │     └─────────────────────────────────────────┘
      ├─ ● review-dashboard-css  mateo/worktrees  ctx 58% ⎇
      └─ ● fix-status-dots       mateo/fixes      ctx 34% ⎇
└─ ▸ 🗄  forge          ssh forge.lan · Linux 6.9 · x86_64  ● 2
```

## 2. Goals / Non-goals

### Goals
- Host-first hierarchy (`host → repo → worktree`) as the **primary** sidebar
  layout, replacing the current grouping selector as the default.
- Per-host header: icon (local vs ssh), name, **OS/arch metadata line**,
  reachability dot + **session-count badge**, expand/collapse (persisted).
- Per-worktree leaf: existing name + branch + status dot, plus a **ctx % chip**
  and a render-ready (unwired) **`PRIMARY`** slot.
- **Active-session card** on the currently-selected worktree: last agent
  message + last tool call. Pure re-layout of existing store state — no new
  data plumbing.

### Non-goals (v1)
- Wiring `PRIMARY` to a real meaning. The badge slot renders but stays unwired
  until "primary" is defined in the design system. (Confirmed: it is currently a
  design-system element only.)
- Removing the old `workspace-status` / `pr-status` `groupBy` modes. Host-first
  becomes the default; whether to delete or demote the other modes is a
  follow-up, not a blocker.
- Any change to session/worktree creation, SSH connection, or worktree lineage
  logic. We consume existing state only.

## 3. Approach (Hybrid)

We chose **C — Hybrid** over (A) derive-everything and (B) full hosts slice:

- **Derive the structure** (`host → repo → worktree`) in the grouping builder
  from existing state (`repos` carry `connectionId`/`hostId`; SSH state lives in
  `sshConnectionStates` / `sshTargetLabels`).
- **Add a thin `hosts` slice** for only what isn't derivable: OS/arch
  system-info, reachability status, and per-host expand/collapse.
- A selector **joins** structure + slice metadata for rendering.

Rationale: mirrors both existing separations — the TUI keeps `app.hosts` +
`host_readiness_cache` separate from the session tree, and the desktop already
keeps `sshConnectionStates` separate from `repos`. Avoids duplicating SSH/repo
data; keeps the change incremental.

## 4. Architecture

### 4.1 New / changed UI components (`crates/agentum-desktop/ui/src/components/sidebar/`)

| Component | Responsibility | Depends on |
| --- | --- | --- |
| `HostGroupHeader.tsx` (new) | Render one host header row: `Monitor` (local) / `Server` (ssh) icon, name, OS/arch line, reachability dot + count badge, expand chevron. | `hosts` slice, host-keyed group from builder |
| `SessionActivityCard.tsx` (new) | Render the expanded card for `activeWorktreeId` only: last agent message + last tool call. | `useLatestAgentActivity(worktreeId)` |
| `WorktreeList.tsx` (changed) | Render the host-first row stream (host headers + existing repo/worktree rows); slot the card under the active leaf. | grouping builder, `activeWorktreeId` |
| existing repo-group header (reuse) | Project folder row + count. | unchanged |
| existing worktree leaf row (changed) | Add `ctx %` chip + render-ready `PRIMARY` slot next to existing name/branch/status dot. | `useLatestAgentActivity` for ctx% |

### 4.2 New / changed state (`crates/agentum-desktop/ui/src/store/`)

- **New thin slice** `slices/hosts.ts`:
  - `hostMetaById: Map<hostKey, { kind: 'local' | 'ssh'; label: string; os?: string; arch?: string }>`
  - `hostStatusById: Map<hostKey, 'reachable' | 'connecting' | 'down' | 'unknown'>`
  - `hostExpanded: Record<hostKey, boolean>` (persisted, default: local host expanded)
  - actions: `hydrateHosts()`, `setHostStatus()`, `toggleHostExpanded()`
- **Changed** `components/sidebar/worktree-list-groups.ts`:
  - Add `host-header` to the row-type discriminator (currently
    `'header' | 'item' | 'imported-worktrees-card'`).
  - Add `repoHostKey(repo)` helper — analogue of the TUI's `host_group_key()`
    (`app.rs`). Local repos (no `connectionId`) bucket under a synthetic
    `local` host key.
  - Emit rows in `host → repo → worktree` order; host-first is the default.
- **New selector** `useLatestAgentActivity(worktreeId)`:
  - Scans `agentStatusByPaneKey` entries belonging to the worktree (reuse the
    filtering already in `hooks/useWorktreeAgentRows.ts`), picks the
    most-recently-updated entry, returns
    `{ lastAssistantMessage, toolName, toolInput, contextUsagePercent }`.
  - This is the single source for the card **and** the leaf's ctx% chip
    (session-level ctx = aggregate of per-agent `contextUsagePercent`).
- **Reused unchanged:** `activeWorktreeId` (`store/slices/worktree-helpers.ts`),
  `agentStatusByPaneKey` (`store/slices/agent-status.ts`), `repos`
  (`slices/repos.ts`), `worktreesByRepo` (`slices/worktrees.ts`),
  `sshConnectionStates` / `sshTargetLabels` (`slices/ssh.ts`).

### 4.3 Data flow

```
/api/hosts (embedded server)  ─┐
HostSystemInfo (local OS/arch) ─┼─►  hosts slice  ──►  HostGroupHeader
uname probe (ssh OS/arch via   ─┘    (meta+status+expand)     ▲
  /api/hosts/{id}/test|readiness)                             │ join by hostKey
sshConnectionStates (live status) ──────────────────────────►│
                                                              │
repos + worktreesByRepo  ──►  worktree-list-groups  ──►  host→repo→worktree rows
                                                              │
agentStatusByPaneKey  ──►  useLatestAgentActivity  ──►  SessionActivityCard + ctx% chip
activeWorktreeId  ─────────────────────────────────►  (which leaf shows the card)
```

- Host metadata fetched on sidebar mount and refreshed on host/SSH events.
  OS/arch source: `HostSystemInfo` for local; SSH `uname` from the existing
  `/api/hosts/{id}/test` (returns `{ok, tmux, git, uname}`) or readiness probe.
  Cached in the slice so we probe lazily, not per render.
- **No new endpoints** are required for the active-session card — all four
  fields already exist in `agent-status-types.ts` (`lastAssistantMessage` ≤8KB,
  `toolName` ≤60, `toolInput` ≤160, `contextUsagePercent` 0–100).

## 5. Status / visual mapping

| Element | Source | Mockup rendering |
| --- | --- | --- |
| Session status dot | existing agent-state (`working` / `blocked`/awaiting / `done`/`idle`) | working = animated spinner (◐), awaiting = amber ●, idle/done = gray ● |
| Host reachability dot | `hostStatusById` | reachable = green; connecting = amber; down = gray |
| Host count badge | count of worktrees/sessions under the host | `● N` pill |
| ctx % chip | `contextUsagePercent` (aggregated) | `ctx 71%`, muted |
| Branch | `Worktree.branch` (existing) | `mateo/worktrees`, muted |
| Host icon | `kind` | `Monitor` (local) / `Server` (ssh) from lucide-react |
| `PRIMARY` | unwired slot | pill rendered only when a future flag is set |

Styling stays in the existing system: **Tailwind + CSS vars** (`--sidebar*`),
Radix primitives (Accordion/Tooltip/ScrollArea already present), lucide-react
icons. Reuse the existing `Accordion` for host/repo collapse.

## 6. Testing

- **Grouping builder** (`worktree-list-groups`): unit-test `repoHostKey()` and
  the host→repo→worktree row emission — local-only repos, mixed local+SSH,
  multiple repos per host, empty host, collapse state honored.
- **`useLatestAgentActivity`**: given several `agentStatusByPaneKey` entries for
  one worktree, returns the most-recent one's fields; returns empty when none.
- **ctx% aggregation**: correct session-level value from multiple per-agent
  percentages; undefined when no agent reports.
- **Render**: `HostGroupHeader` shows OS/arch line + count + correct icon per
  kind; `SessionActivityCard` renders only for `activeWorktreeId` and truncates
  long message/tool strings.
- Follow the repo's existing UI test setup; typecheck via `tsc`
  (`runtime/*` clients are alias-free per CLAUDE.md).

## 7. Build / verification

- UI: `npm run build --prefix crates/agentum-desktop/ui` (or `npm run dev` for
  HMR). Rebuild the `agentum-desktop` crate only if Rust shell commands change
  (this work is UI-only unless OS/arch needs a new native/server call).

## 8. Resolved decisions

- **Scope:** host-first **replaces** the current grouping as the primary layout.
- **v1 enrichments:** host OS/arch line + active-session card (the two that cost
  plumbing); branch/ctx%/status dots ride along since the data exists.
- **PRIMARY:** design-system element only — render-ready slot, **unwired** in v1.
- **Count badge:** **included** (trivial + in the mockup).
- **Card data:** no new plumbing — re-layout of `agentStatusByPaneKey`.

## 9. Open questions (non-blocking)

- Local-host OS/arch: confirm `HostSystemInfo` is reachable from the UI without
  a new native command; if not, add a tiny read-only command/endpoint. (SSH
  side already has `uname` via `/api/hosts/{id}/test`.)
- Fate of the old `workspace-status` / `pr-status` `groupBy` modes (delete vs
  keep behind a toggle) — decide during planning, not blocking v1.
