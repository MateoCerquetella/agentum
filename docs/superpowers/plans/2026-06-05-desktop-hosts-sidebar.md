# Hosts-first Desktop Sidebar — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure the desktop ADE sidebar into a `HOST → repo → worktree(session)` hierarchy (matching the TUI / the provided mockup), with a per-host OS metadata line, a reachability dot + session-count badge, and an active-session card showing the last agent message + last tool call.

**Architecture:** Approach C (Hybrid). The host→repo→worktree *structure* is derived in a pure post-processor (`groupRowsByHost`) that runs over the existing repo-grouped `Row[]` from `buildRows`. A thin `hosts` store slice (`hostMetaByKey`) caches only the per-host label + OS detail line (sourced from `/api/hosts` + `/api/hosts/{id}/readiness`), which isn't derivable from the repo list. Host collapse reuses the existing `collapsedGroups`/`toggleGroup` machinery. The active-session card and the per-leaf `ctx %` are a re-layout of existing `agentStatusByPaneKey` state via a new `selectLatestAgentActivity` selector — no new data plumbing.

**Tech Stack:** React 18 + TypeScript, Zustand store, Tailwind v4 + Radix + lucide-react, Vitest. Source root: `crates/agentum-desktop/ui/src/`.

---

## Conventions for this plan

- All paths are relative to `crates/agentum-desktop/ui/` unless noted. Run all UI commands from that directory: `cd crates/agentum-desktop/ui`.
- Test runner is Vitest. Run a single test file with `npx vitest run <path>`. Typecheck with `npx tsc --noEmit`. Build with `npm run build`.
- Commit after each task with the message shown in its final step.
- The server-facing runtime clients under `src/runtime/*` are alias-free (no `@/`); keep that convention in `server-host-client.ts`.

## File Structure

**New files:**
- `src/store/slices/hosts.ts` — thin `HostsSlice`: `hostMetaByKey` + `hydrateHosts()`.
- `src/store/slices/hosts.test.ts` — slice unit tests.
- `src/components/sidebar/worktree-latest-activity.ts` — `latestFromEntries()` + `selectLatestAgentActivity()`.
- `src/components/sidebar/worktree-latest-activity.test.ts` — selector tests.
- `src/components/sidebar/useLatestAgentActivity.ts` — React hook over the selector.
- `src/components/sidebar/HostGroupHeader.tsx` — host header row component.
- `src/components/sidebar/HostGroupHeader.test.tsx` — render tests.
- `src/components/sidebar/SessionActivityCard.tsx` — active-session card component.
- `src/components/sidebar/SessionActivityCard.test.tsx` — render tests.

**Modified files:**
- `src/runtime/server-host-client.ts` — add `getServerHostReadinessUname()` + export `ServerHost`.
- `src/store/slices/hosts.ts` is new; register it in `src/store/index.ts` and `src/store/types.ts`.
- `src/components/sidebar/worktree-list-groups.ts` — add `SidebarHost`, `HostHeaderRow`, `'host'` to `WorktreeGroupBy`, `hostKeyForRepo()`, `getHostHeaderKey()`, `groupRowsByHost()`.
- `src/components/sidebar/worktree-list-groups.test.ts` — tests for `groupRowsByHost`.
- `src/store/slices/ui.ts` — add `'host'` to `groupBy` union; default to `'host'`.
- `src/components/sidebar/WorktreeList.tsx` — subscribe to host state; wrap rows with `groupRowsByHost` in host mode; render `host-header` rows; render `SessionActivityCard` under the active leaf.
- `src/components/sidebar/WorktreeCardMeta.tsx` — add a `ctx %` chip.
- `src/components/sidebar/index.tsx` — call `hydrateHosts()` on mount + on SSH connection changes.

**Deferred (own follow-up spec, NOT in this plan):** rich `arch`/pretty-OS/CPU-model enrichment of `HostSystemInfo` for full `· x86_64` / `· M3 Max` fidelity (see Phase 5).

---

## Phase 1 — Hosts store slice + host metadata source

### Task 1: Add a readiness-uname helper to the server-host client

**Files:**
- Modify: `src/runtime/server-host-client.ts`

- [ ] **Step 1: Export `ServerHost` and add the readiness-uname helper**

In `src/runtime/server-host-client.ts`, change the `ServerHost` type declaration to be exported, and append the new helper after the existing `detectRemoteAgentsViaServer` function. Replace:

```ts
/** A server host as returned by `/api/hosts` (camelCase flattened kind). */
type ServerHost = {
```

with:

```ts
/** A server host as returned by `/api/hosts` (camelCase flattened kind). */
export type ServerHost = {
```

Then add this function immediately after the closing brace of `detectRemoteAgentsViaServer`:

```ts
/**
 * Read a host's OS one-liner from `/api/hosts/{id}/readiness` (`system.uname`,
 * e.g. "Linux 6.9" / "Darwin 24.5"). Best-effort — returns null on any failure
 * or when the daemon predates the field, so the sidebar header degrades to a
 * kind-only label rather than throwing.
 */
export async function getServerHostReadinessUname(hostId: string): Promise<string | null> {
  try {
    const readiness = await getJson<{ system?: { uname?: string | null } }>(
      `/api/hosts/${encodeURIComponent(hostId)}/readiness`
    )
    return readiness.system?.uname ?? null
  } catch (err) {
    console.warn('[agentum] failed to read host readiness uname', hostId, err)
    return null
  }
}
```

- [ ] **Step 2: Typecheck**

Run: `cd crates/agentum-desktop/ui && npx tsc --noEmit`
Expected: PASS (no new errors).

- [ ] **Step 3: Commit**

```bash
git add crates/agentum-desktop/ui/src/runtime/server-host-client.ts
git commit -m "feat(desktop): expose ServerHost + host readiness uname helper"
```

### Task 2: Create the hosts store slice (failing test first)

**Files:**
- Create: `src/store/slices/hosts.ts`
- Test: `src/store/slices/hosts.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/store/slices/hosts.test.ts`:

```ts
import { describe, it, expect } from 'vitest'
import { createStore } from 'zustand/vanilla'
import { createHostsSlice, type HostsSlice } from './hosts'

function makeStore() {
  return createStore<HostsSlice>()((...a) => ({ ...createHostsSlice(...(a as Parameters<typeof createHostsSlice>)) }))
}

describe('hosts slice', () => {
  it('starts with an empty host-meta map', () => {
    const store = makeStore()
    expect(store.getState().hostMetaByKey).toEqual({})
  })

  it('setHostMeta inserts and overwrites by key', () => {
    const store = makeStore()
    store.getState().setHostMeta('local', { key: 'local', kind: 'local', label: 'studio', detail: 'localhost · Darwin 24.5' })
    expect(store.getState().hostMetaByKey.local.label).toBe('studio')
    store.getState().setHostMeta('local', { key: 'local', kind: 'local', label: 'studio2' })
    expect(store.getState().hostMetaByKey.local.label).toBe('studio2')
    expect(store.getState().hostMetaByKey.local.detail).toBeUndefined()
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd crates/agentum-desktop/ui && npx vitest run src/store/slices/hosts.test.ts`
Expected: FAIL with "Cannot find module './hosts'".

- [ ] **Step 3: Implement the slice**

Create `src/store/slices/hosts.ts`:

```ts
import type { StateCreator } from 'zustand'
import type { AppState } from '../types'
import { useAppStore } from '@/store'
import {
  listServerHosts,
  resolveServerHostIdForConnection,
  getServerHostReadinessUname,
  type ServerHost
} from '@/runtime/server-host-client'

/** Stable key identifying a host in the sidebar tree. `local` for the daemon's
 *  own machine; `ssh:<connectionId>` for a remote repo's native SSH target. */
export type HostKey = string

export type HostMeta = {
  key: HostKey
  kind: 'local' | 'ssh'
  /** Display name on the host header (e.g. "studio", "forge"). */
  label: string
  /** Right-of-name OS line, e.g. "localhost · Darwin 24.5" or
   *  "ssh forge.lan · Linux 6.9". Undefined until readiness resolves. */
  detail?: string
}

export type HostsSlice = {
  /** Per-host label + OS detail, keyed by HostKey. The host→repo structure is
   *  derived from the repo list; this slice holds only what isn't derivable. */
  hostMetaByKey: Record<HostKey, HostMeta>
  setHostMeta: (key: HostKey, meta: HostMeta) => void
  /** Populate label + OS detail for the local host and every known SSH target.
   *  Best-effort: never throws into the UI. */
  hydrateHosts: () => Promise<void>
}

function unameDetail(prefix: string, uname: string | null): string {
  return uname ? `${prefix} · ${uname}` : prefix
}

export const createHostsSlice: StateCreator<AppState, [], [], HostsSlice> = (set, get) => ({
  hostMetaByKey: {},

  setHostMeta: (key, meta) =>
    set((s) => ({ hostMetaByKey: { ...s.hostMetaByKey, [key]: meta } })),

  hydrateHosts: async () => {
    // Local host: find the daemon's own host in the registry, read its uname.
    try {
      const hosts: ServerHost[] = await listServerHosts()
      const local = hosts.find((h) => h.kind === 'local')
      const localUname = local ? await getServerHostReadinessUname(local.id) : null
      get().setHostMeta('local', {
        key: 'local',
        kind: 'local',
        label: local?.name?.trim() || 'This Mac',
        detail: unameDetail('localhost', localUname)
      })
    } catch (err) {
      console.warn('[agentum] hydrateHosts: local host failed', err)
    }

    // SSH hosts: one entry per known native target (label from the store).
    const labels = useAppStore.getState().sshTargetLabels
    for (const [connectionId, label] of labels) {
      const key = `ssh:${connectionId}`
      // Seed the label immediately so the header renders before readiness lands.
      get().setHostMeta(key, { key, kind: 'ssh', label })
      try {
        const hostId = await resolveServerHostIdForConnection(connectionId)
        if (!hostId) continue
        const hosts: ServerHost[] = await listServerHosts()
        const host = hosts.find((h) => h.id === hostId)
        const uname = await getServerHostReadinessUname(hostId)
        const prefix = host?.hostname ? `ssh ${host.hostname}` : 'ssh'
        get().setHostMeta(key, { key, kind: 'ssh', label, detail: unameDetail(prefix, uname) })
      } catch (err) {
        console.warn('[agentum] hydrateHosts: ssh host failed', connectionId, err)
      }
    }
  }
})
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd crates/agentum-desktop/ui && npx vitest run src/store/slices/hosts.test.ts`
Expected: PASS (2 tests). The `hydrateHosts` network path is covered indirectly; the unit tests assert the synchronous `hostMetaByKey`/`setHostMeta` contract only.

- [ ] **Step 5: Commit**

```bash
git add crates/agentum-desktop/ui/src/store/slices/hosts.ts crates/agentum-desktop/ui/src/store/slices/hosts.test.ts
git commit -m "feat(desktop): hosts store slice (hostMetaByKey + hydrateHosts)"
```

### Task 3: Register the hosts slice in the store

**Files:**
- Modify: `src/store/types.ts`
- Modify: `src/store/index.ts`

- [ ] **Step 1: Add `HostsSlice` to `AppState`**

In `src/store/types.ts`, add the import after the `WorkspaceCleanupSlice` import (line 27):

```ts
import type { HostsSlice } from './slices/hosts'
```

and add `&\n  HostsSlice` to the end of the `AppState` intersection (after `WorkspaceCleanupSlice` on line 55):

```ts
  WorkspaceCleanupSlice &
  HostsSlice
```

- [ ] **Step 2: Wire the slice into the store factory**

In `src/store/index.ts`, add the import after the `createWorkspaceCleanupSlice` import (line 29):

```ts
import { createHostsSlice } from './slices/hosts'
```

and add the spread inside the `create<AppState>()` object after `...createWorkspaceCleanupSlice(...a)` (line 60):

```ts
  ...createWorkspaceCleanupSlice(...a),
  ...createHostsSlice(...a)
```

- [ ] **Step 3: Typecheck**

Run: `cd crates/agentum-desktop/ui && npx tsc --noEmit`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/agentum-desktop/ui/src/store/types.ts crates/agentum-desktop/ui/src/store/index.ts
git commit -m "feat(desktop): register hosts slice in the app store"
```

---

## Phase 2 — Pure host grouping (types + post-processor)

### Task 4: Add host types + `hostKeyForRepo` + `getHostHeaderKey`

**Files:**
- Modify: `src/components/sidebar/worktree-list-groups.ts`

- [ ] **Step 1: Add `'host'` to `WorktreeGroupBy`**

In `src/components/sidebar/worktree-list-groups.ts`, change line 32:

```ts
export type WorktreeGroupBy = 'none' | 'workspace-status' | 'repo' | 'pr-status'
```

to:

```ts
export type WorktreeGroupBy = 'none' | 'workspace-status' | 'repo' | 'pr-status' | 'host'
```

- [ ] **Step 2: Add the host types + helpers**

In the same file, immediately after the `WorktreeGroupBy` type (the line you just edited), add:

```ts
/** A host as rendered in the sidebar tree. `key` is `local` or `ssh:<id>`. */
export type SidebarHost = {
  key: string
  kind: 'local' | 'ssh'
  label: string
  detail?: string
  status?: 'reachable' | 'connecting' | 'down' | 'unknown'
}

export const LOCAL_HOST_KEY = 'local'

/** Map a repo to its host key. Local repos (no SSH target) bucket under the
 *  synthetic local host. Mirrors the TUI's `host_group_key()`. */
export function hostKeyForRepo(repo: Repo | undefined): string {
  return repo?.connectionId ? `ssh:${repo.connectionId}` : LOCAL_HOST_KEY
}

/** Collapse-state key for a host header. Reuses the shared `collapsedGroups`
 *  set so host collapse rides the existing toggle/scroll-anchor machinery. */
export function getHostHeaderKey(hostKey: string): string {
  return `host:${hostKey}`
}
```

- [ ] **Step 3: Add `HostHeaderRow` to the `Row` union**

In the same file, add the `HostHeaderRow` type immediately before the existing `export type Row =` line (line 81), then extend the union:

```ts
export type HostHeaderRow = {
  type: 'host-header'
  key: string
  host: SidebarHost
  count: number
}

export type Row = GroupHeaderRow | WorktreeRow | ImportedWorktreesCardRow | HostHeaderRow
```

- [ ] **Step 4: Typecheck**

Run: `cd crates/agentum-desktop/ui && npx tsc --noEmit`
Expected: It MAY surface exhaustiveness errors where `groupBy` is switched on (e.g. `WorktreeList.tsx`, section-activity). That is expected and fixed in Phase 4 (host mode is normalized to `'repo'` before reaching those switches). If errors appear ONLY in files modified later in this plan, proceed. If errors appear in `worktree-list-groups.ts` itself, fix them — `buildRows`/`getGroupKeyForWorktree` already fall through to the PR branch for unknown values, which is harmless because host mode never calls them directly.

- [ ] **Step 5: Commit**

```bash
git add crates/agentum-desktop/ui/src/components/sidebar/worktree-list-groups.ts
git commit -m "feat(desktop): host types + hostKeyForRepo/getHostHeaderKey helpers"
```

### Task 5: Implement `groupRowsByHost` (failing test first)

**Files:**
- Modify: `src/components/sidebar/worktree-list-groups.ts`
- Test: `src/components/sidebar/worktree-list-groups.test.ts`

- [ ] **Step 1: Write the failing test**

Append to `src/components/sidebar/worktree-list-groups.test.ts` (add the import to the existing import block if not present):

```ts
import {
  groupRowsByHost,
  getHostHeaderKey,
  PINNED_GROUP_KEY,
  type Row,
  type SidebarHost
} from './worktree-list-groups'

describe('groupRowsByHost', () => {
  const hostForKey = (key: string): SidebarHost => ({
    key,
    kind: key === 'local' ? 'local' : 'ssh',
    label: key === 'local' ? 'studio' : 'forge',
    status: 'reachable'
  })

  // A repo header carries `.repo` (with optional connectionId) and a count.
  const repoHeader = (id: string, connectionId: string | null, count: number): Row =>
    ({ type: 'header', key: `repo:${id}`, label: id, count, tone: '', repo: { connectionId } } as unknown as Row)
  const item = (id: string, repoConnectionId: string | null): Row =>
    ({ type: 'item', worktree: { id }, repo: { connectionId: repoConnectionId }, depth: 0, lineageTrail: [], isLastLineageChild: false, lineageChildCount: 0 } as unknown as Row)

  it('inserts a host header above each host group, local first', () => {
    const rows: Row[] = [
      repoHeader('remote-repo', 'conn-1', 1),
      item('w1', 'conn-1'),
      repoHeader('local-repo', null, 1),
      item('w2', null)
    ]
    const out = groupRowsByHost(rows, hostForKey, new Set())
    expect(out.map((r) => r.type)).toEqual([
      'host-header', 'header', 'item', // local host first
      'host-header', 'header', 'item' // then ssh host
    ])
    expect((out[0] as { host: SidebarHost }).host.key).toBe('local')
    expect((out[3] as { host: SidebarHost }).host.key).toBe('ssh:conn-1')
  })

  it('sums repo header counts into the host count', () => {
    const rows: Row[] = [repoHeader('a', null, 2), item('w1', null), repoHeader('b', null, 3), item('w2', null)]
    const out = groupRowsByHost(rows, hostForKey, new Set())
    expect((out[0] as { count: number }).count).toBe(5)
  })

  it('hides a host group body when its host header is collapsed', () => {
    const rows: Row[] = [repoHeader('a', null, 1), item('w1', null)]
    const collapsed = new Set([getHostHeaderKey('local')])
    const out = groupRowsByHost(rows, hostForKey, collapsed)
    expect(out.map((r) => r.type)).toEqual(['host-header'])
  })

  it('keeps a leading Pinned section above all hosts', () => {
    const rows: Row[] = [
      { type: 'header', key: PINNED_GROUP_KEY, label: 'Pinned', count: 1, tone: '' } as Row,
      item('p1', null),
      repoHeader('a', null, 1),
      item('w1', null)
    ]
    const out = groupRowsByHost(rows, hostForKey, new Set())
    expect(out[0].type).toBe('header')
    expect((out[0] as { key: string }).key).toBe(PINNED_GROUP_KEY)
    expect(out[1].type).toBe('item')
    expect(out[2].type).toBe('host-header')
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd crates/agentum-desktop/ui && npx vitest run src/components/sidebar/worktree-list-groups.test.ts`
Expected: FAIL with "groupRowsByHost is not a function" (or an import error).

- [ ] **Step 3: Implement `groupRowsByHost`**

In `src/components/sidebar/worktree-list-groups.ts`, append at the end of the file:

```ts
type HostRowBlock = { hostKey: string; headerCount: number; rows: Row[] }

/**
 * Post-process a repo-grouped row list into a host-first tree. Operates purely
 * on the `Row[]` output of `buildRows('repo', ...)`:
 *   1. A leading "Pinned" section stays at the very top, above all hosts.
 *   2. Each repo header + its following non-header rows form a block; the
 *      block's host is derived from the repo header's `connectionId`.
 *   3. Blocks are bucketed by host (local first, then first-seen order) and
 *      emitted under a `host-header` row. A collapsed `host:<key>` hides its body.
 */
export function groupRowsByHost(
  repoRows: Row[],
  hostForKey: (hostKey: string) => SidebarHost,
  collapsedGroups: Set<string>
): Row[] {
  const result: Row[] = []
  const pinnedBlock: Row[] = []
  let i = 0

  if (repoRows[i]?.type === 'header' && (repoRows[i] as GroupHeaderRow).key === PINNED_GROUP_KEY) {
    pinnedBlock.push(repoRows[i])
    i += 1
    while (i < repoRows.length && repoRows[i].type !== 'header') {
      pinnedBlock.push(repoRows[i])
      i += 1
    }
  }

  const blocks: HostRowBlock[] = []
  let current: HostRowBlock | null = null
  for (; i < repoRows.length; i += 1) {
    const row = repoRows[i]
    if (row.type === 'header') {
      const header = row as GroupHeaderRow
      current = { hostKey: hostKeyForRepo(header.repo), headerCount: header.count, rows: [row] }
      blocks.push(current)
    } else if (current) {
      current.rows.push(row)
    } else {
      // Defensive: a body row with no preceding repo header (not expected in
      // repo mode) — anchor it under the local host as its own block.
      current = { hostKey: LOCAL_HOST_KEY, headerCount: 0, rows: [row] }
      blocks.push(current)
    }
  }

  const order: string[] = []
  const byHost = new Map<string, HostRowBlock[]>()
  for (const block of blocks) {
    if (!byHost.has(block.hostKey)) {
      byHost.set(block.hostKey, [])
      order.push(block.hostKey)
    }
    byHost.get(block.hostKey)!.push(block)
  }
  order.sort((a, b) => (a === LOCAL_HOST_KEY ? -1 : b === LOCAL_HOST_KEY ? 1 : 0))

  result.push(...pinnedBlock)
  for (const hostKey of order) {
    const hostBlocks = byHost.get(hostKey)!
    const headerKey = getHostHeaderKey(hostKey)
    result.push({
      type: 'host-header',
      key: headerKey,
      host: hostForKey(hostKey),
      count: hostBlocks.reduce((sum, block) => sum + block.headerCount, 0)
    })
    if (!collapsedGroups.has(headerKey)) {
      for (const block of hostBlocks) {
        result.push(...block.rows)
      }
    }
  }
  return result
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd crates/agentum-desktop/ui && npx vitest run src/components/sidebar/worktree-list-groups.test.ts`
Expected: PASS (all `groupRowsByHost` tests plus the pre-existing suite).

- [ ] **Step 5: Commit**

```bash
git add crates/agentum-desktop/ui/src/components/sidebar/worktree-list-groups.ts crates/agentum-desktop/ui/src/components/sidebar/worktree-list-groups.test.ts
git commit -m "feat(desktop): groupRowsByHost — host-first row post-processor"
```

---

## Phase 3 — Latest-activity selector + hook

### Task 6: `latestFromEntries` + `selectLatestAgentActivity` (failing test first)

**Files:**
- Create: `src/components/sidebar/worktree-latest-activity.ts`
- Test: `src/components/sidebar/worktree-latest-activity.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/components/sidebar/worktree-latest-activity.test.ts`:

```ts
import { describe, it, expect } from 'vitest'
import { latestFromEntries } from './worktree-latest-activity'
import type { AgentStatusEntry } from '../../../../shared/agent-status-types'

function entry(over: Partial<AgentStatusEntry>): AgentStatusEntry {
  return {
    state: 'working',
    prompt: '',
    updatedAt: 0,
    stateStartedAt: 0,
    paneKey: 'tab:leaf',
    stateHistory: [],
    ...over
  }
}

describe('latestFromEntries', () => {
  it('returns empty fields for no entries', () => {
    expect(latestFromEntries([])).toEqual({})
  })

  it('picks the entry with the greatest updatedAt', () => {
    const result = latestFromEntries([
      entry({ updatedAt: 10, lastAssistantMessage: 'old', toolName: 'Read' }),
      entry({ updatedAt: 30, lastAssistantMessage: 'Wired the worktree help', toolName: 'Bash', toolInput: 'cargo clippy', contextUsagePercent: 71 }),
      entry({ updatedAt: 20, lastAssistantMessage: 'mid' })
    ])
    expect(result).toEqual({
      lastAssistantMessage: 'Wired the worktree help',
      toolName: 'Bash',
      toolInput: 'cargo clippy',
      contextUsagePercent: 71
    })
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd crates/agentum-desktop/ui && npx vitest run src/components/sidebar/worktree-latest-activity.test.ts`
Expected: FAIL with "Cannot find module './worktree-latest-activity'".

- [ ] **Step 3: Implement the selector**

Create `src/components/sidebar/worktree-latest-activity.ts`:

```ts
import type { AgentStatusEntry } from '../../../../shared/agent-status-types'
import { selectLiveAgentStatusEntriesForWorktree } from './worktree-agent-row-selectors'

/** The fields the active-session card + leaf ctx% chip render. All optional —
 *  agents report tool/message/context independently. */
export type LatestAgentActivity = {
  lastAssistantMessage?: string
  toolName?: string
  toolInput?: string
  contextUsagePercent?: number
}

const EMPTY: LatestAgentActivity = {}

/** Pick the most-recently-updated agent entry's surface fields. Pure over an
 *  entry array so it's trivially unit-testable and reusable from the hook. */
export function latestFromEntries(entries: readonly AgentStatusEntry[]): LatestAgentActivity {
  let latest: AgentStatusEntry | undefined
  for (const entry of entries) {
    if (!latest || entry.updatedAt > latest.updatedAt) {
      latest = entry
    }
  }
  if (!latest) {
    return EMPTY
  }
  return {
    lastAssistantMessage: latest.lastAssistantMessage,
    toolName: latest.toolName,
    toolInput: latest.toolInput,
    contextUsagePercent: latest.contextUsagePercent
  }
}

export function selectLatestAgentActivity(
  state: Parameters<typeof selectLiveAgentStatusEntriesForWorktree>[0],
  worktreeId: string
): LatestAgentActivity {
  return latestFromEntries(selectLiveAgentStatusEntriesForWorktree(state, worktreeId))
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd crates/agentum-desktop/ui && npx vitest run src/components/sidebar/worktree-latest-activity.test.ts`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/agentum-desktop/ui/src/components/sidebar/worktree-latest-activity.ts crates/agentum-desktop/ui/src/components/sidebar/worktree-latest-activity.test.ts
git commit -m "feat(desktop): selectLatestAgentActivity — per-session last message/tool/ctx"
```

### Task 7: `useLatestAgentActivity` hook

**Files:**
- Create: `src/components/sidebar/useLatestAgentActivity.ts`

- [ ] **Step 1: Implement the hook**

Create `src/components/sidebar/useLatestAgentActivity.ts`:

```ts
import { useMemo } from 'react'
import { useShallow } from 'zustand/react/shallow'
import { useAppStore } from '@/store'
import { selectLiveAgentStatusEntriesForWorktree } from './worktree-agent-row-selectors'
import { latestFromEntries, type LatestAgentActivity } from './worktree-latest-activity'

/**
 * Last assistant message + tool call + context% for a worktree's most-recently
 * active agent pane. Narrows the subscription to THIS worktree's entries via
 * useShallow (same render-amplification guard as useWorktreeAgentRows), and
 * threads agentStatusEpoch so freshness boundaries recompute.
 */
export function useLatestAgentActivity(worktreeId: string): LatestAgentActivity {
  const entries = useAppStore(
    useShallow((s) => selectLiveAgentStatusEntriesForWorktree(s, worktreeId))
  )
  const agentStatusEpoch = useAppStore((s) => s.agentStatusEpoch)
  return useMemo(
    () => latestFromEntries(entries),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [entries, agentStatusEpoch]
  )
}
```

- [ ] **Step 2: Typecheck**

Run: `cd crates/agentum-desktop/ui && npx tsc --noEmit`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/agentum-desktop/ui/src/components/sidebar/useLatestAgentActivity.ts
git commit -m "feat(desktop): useLatestAgentActivity hook"
```

---

## Phase 4 — UI components + WorktreeList integration

### Task 8: `HostGroupHeader` component (failing test first)

**Files:**
- Create: `src/components/sidebar/HostGroupHeader.tsx`
- Test: `src/components/sidebar/HostGroupHeader.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `src/components/sidebar/HostGroupHeader.test.tsx`:

```tsx
import { describe, it, expect, vi } from 'vitest'
import { render, screen, fireEvent } from '@testing-library/react'
import { HostGroupHeader } from './HostGroupHeader'

describe('HostGroupHeader', () => {
  it('renders label, detail line and count', () => {
    render(
      <HostGroupHeader
        host={{ key: 'local', kind: 'local', label: 'studio', detail: 'localhost · Darwin 24.5', status: 'reachable' }}
        count={3}
        collapsed={false}
        onToggle={() => {}}
      />
    )
    expect(screen.getByText('studio')).toBeInTheDocument()
    expect(screen.getByText('localhost · Darwin 24.5')).toBeInTheDocument()
    expect(screen.getByText('3')).toBeInTheDocument()
  })

  it('fires onToggle on click', () => {
    const onToggle = vi.fn()
    render(
      <HostGroupHeader host={{ key: 'local', kind: 'local', label: 'studio' }} count={0} collapsed onToggle={onToggle} />
    )
    fireEvent.click(screen.getByRole('button'))
    expect(onToggle).toHaveBeenCalledOnce()
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd crates/agentum-desktop/ui && npx vitest run src/components/sidebar/HostGroupHeader.test.tsx`
Expected: FAIL with "Cannot find module './HostGroupHeader'".

- [ ] **Step 3: Implement the component**

Create `src/components/sidebar/HostGroupHeader.tsx`:

```tsx
import { ChevronDown, Monitor, Server } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { SidebarHost } from './worktree-list-groups'

const STATUS_DOT: Record<NonNullable<SidebarHost['status']>, string> = {
  reachable: 'bg-emerald-500',
  connecting: 'bg-amber-500',
  down: 'bg-zinc-400',
  unknown: 'bg-zinc-300'
}

export function HostGroupHeader({
  host,
  count,
  collapsed,
  onToggle
}: {
  host: SidebarHost
  count: number
  collapsed: boolean
  onToggle: () => void
}): JSX.Element {
  const Icon = host.kind === 'ssh' ? Server : Monitor
  const status = host.status ?? 'unknown'
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onToggle}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onToggle()
        }
      }}
      className="group flex h-8 w-full cursor-pointer items-center gap-1.5 px-1 text-left"
    >
      <ChevronDown
        className={cn(
          'size-3.5 shrink-0 text-muted-foreground transition-transform',
          collapsed && '-rotate-90'
        )}
      />
      <Icon className="size-4 shrink-0 text-muted-foreground" />
      <div className="flex min-w-0 flex-1 flex-col leading-tight">
        <span className="truncate text-sm font-semibold text-foreground">{host.label}</span>
        {host.detail ? (
          <span className="truncate text-[11px] text-muted-foreground">{host.detail}</span>
        ) : null}
      </div>
      <span className="ml-auto inline-flex items-center gap-1 rounded-full bg-sidebar-accent px-1.5 py-0.5 text-[11px] text-muted-foreground">
        <span className={cn('size-1.5 rounded-full', STATUS_DOT[status])} />
        {count}
      </span>
    </div>
  )
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd crates/agentum-desktop/ui && npx vitest run src/components/sidebar/HostGroupHeader.test.tsx`
Expected: PASS (2 tests). If `cn` is not at `@/lib/utils`, grep an existing sidebar component for the `cn` import path and match it.

- [ ] **Step 5: Commit**

```bash
git add crates/agentum-desktop/ui/src/components/sidebar/HostGroupHeader.tsx crates/agentum-desktop/ui/src/components/sidebar/HostGroupHeader.test.tsx
git commit -m "feat(desktop): HostGroupHeader component"
```

### Task 9: `SessionActivityCard` component (failing test first)

**Files:**
- Create: `src/components/sidebar/SessionActivityCard.tsx`
- Test: `src/components/sidebar/SessionActivityCard.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `src/components/sidebar/SessionActivityCard.test.tsx`:

```tsx
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen } from '@testing-library/react'
import type { LatestAgentActivity } from './worktree-latest-activity'

const activityMock = vi.fn<[], LatestAgentActivity>()
vi.mock('./useLatestAgentActivity', () => ({
  useLatestAgentActivity: () => activityMock()
}))

import { SessionActivityCard } from './SessionActivityCard'

describe('SessionActivityCard', () => {
  beforeEach(() => activityMock.mockReset())

  it('renders the last message and tool call', () => {
    activityMock.mockReturnValue({
      lastAssistantMessage: 'Wired the worktree help',
      toolName: 'Bash',
      toolInput: 'cargo clippy --all-targets'
    })
    render(<SessionActivityCard worktreeId="w1" />)
    expect(screen.getByText('Wired the worktree help')).toBeInTheDocument()
    expect(screen.getByText(/Bash cargo clippy --all-targets/)).toBeInTheDocument()
  })

  it('renders nothing when there is no activity', () => {
    activityMock.mockReturnValue({})
    const { container } = render(<SessionActivityCard worktreeId="w1" />)
    expect(container.firstChild).toBeNull()
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd crates/agentum-desktop/ui && npx vitest run src/components/sidebar/SessionActivityCard.test.tsx`
Expected: FAIL with "Cannot find module './SessionActivityCard'".

- [ ] **Step 3: Implement the component**

Create `src/components/sidebar/SessionActivityCard.tsx`:

```tsx
import { Sparkles, Wrench } from 'lucide-react'
import { useLatestAgentActivity } from './useLatestAgentActivity'

/** Expanded card shown under the currently-active session: last assistant
 *  message + last tool call. Pure re-layout of existing agent-status state —
 *  renders nothing until an agent reports activity for this worktree. */
export function SessionActivityCard({ worktreeId }: { worktreeId: string }): JSX.Element | null {
  const activity = useLatestAgentActivity(worktreeId)
  if (!activity.lastAssistantMessage && !activity.toolName) {
    return null
  }
  const toolLine = activity.toolName
    ? `${activity.toolName}${activity.toolInput ? ` ${activity.toolInput}` : ''}`
    : null
  return (
    <div className="mx-2 mb-1 rounded-lg border border-border/60 bg-card px-2.5 py-2 shadow-sm">
      {activity.lastAssistantMessage ? (
        <div className="flex items-start gap-1.5">
          <Sparkles className="mt-0.5 size-3 shrink-0 text-muted-foreground" />
          <span className="line-clamp-2 text-xs text-foreground">
            {activity.lastAssistantMessage}
          </span>
        </div>
      ) : null}
      {toolLine ? (
        <div className="mt-1 flex items-center gap-1.5">
          <Wrench className="size-3 shrink-0 text-muted-foreground" />
          <span className="truncate font-mono text-[11px] text-muted-foreground">{toolLine}</span>
        </div>
      ) : null}
    </div>
  )
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd crates/agentum-desktop/ui && npx vitest run src/components/sidebar/SessionActivityCard.test.tsx`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/agentum-desktop/ui/src/components/sidebar/SessionActivityCard.tsx crates/agentum-desktop/ui/src/components/sidebar/SessionActivityCard.test.tsx
git commit -m "feat(desktop): SessionActivityCard component"
```

### Task 10: Make `'host'` the default groupBy

**Files:**
- Modify: `src/store/slices/ui.ts`

- [ ] **Step 1: Add `'host'` to the `groupBy` union**

In `src/store/slices/ui.ts`, change line 595:

```ts
  groupBy: 'none' | 'workspace-status' | 'repo' | 'pr-status'
```

to:

```ts
  groupBy: 'none' | 'workspace-status' | 'repo' | 'pr-status' | 'host'
```

- [ ] **Step 2: Default new installs to host-first**

In the same file, change the initial value (line 1223):

```ts
  groupBy: 'repo',
```

to:

```ts
  groupBy: 'host',
```

- [ ] **Step 3: Default the rehydration fallback to host-first**

In the same file, in the persisted-state migration (around line 1493–1496), change the fallback so missing/legacy `'parent'` resolves to `'host'`:

```ts
        groupBy:
          (ui.groupBy as UISlice['groupBy'] | 'parent') === 'parent'
            ? 'host'
            : ((ui.groupBy as UISlice['groupBy']) ?? 'host'),
```

(Existing users who explicitly chose `'repo'`/`'pr-status'`/etc. keep their choice; only undefined/legacy values land on host-first.)

- [ ] **Step 4: Typecheck**

Run: `cd crates/agentum-desktop/ui && npx tsc --noEmit`
Expected: errors only in `WorktreeList.tsx` (the next task wires host mode). If `ui.test.ts` asserts the default groupBy, update that expectation to `'host'` and re-run `npx vitest run src/store/slices/ui.test.ts`.

- [ ] **Step 5: Commit**

```bash
git add crates/agentum-desktop/ui/src/store/slices/ui.ts
git commit -m "feat(desktop): default sidebar to host-first grouping"
```

### Task 11: Wire host grouping into `WorktreeList` (read-then-edit)

**Files:**
- Modify: `src/components/sidebar/WorktreeList.tsx`

This is the integration task in a 4,657-line file. Read the exact regions first, then apply each edit.

- [ ] **Step 1: Read the integration regions**

Read these spans so the edits land precisely:
- The import block (top of file, ~lines 60–80) — where `buildRows`, `Row`, `GroupHeaderRow` are imported from `./worktree-list-groups`.
- The `rows` useMemo (~lines 3862–3898) — the `buildRows(...)` call site.
- The store-subscription block near the other `useAppStore((s) => ...)` calls (search for `const groupBy = useAppStore((s) => s.groupBy)` ~line 3355 and `sshConnectionStates` usage ~line 2580).
- The virtual-row render switch (~lines 2543–2700) — where `if (row.type === 'header')` begins, to add the `host-header` branch above it.
- The `row.type === 'item'` render branch (search for `row.type === 'item'` within the `virtualItems.map`, ~line 2779+ region) — to insert the `SessionActivityCard` after the item's card markup.

- [ ] **Step 2: Add imports**

In the `./worktree-list-groups` import block, add `groupRowsByHost`, `getHostHeaderKey`, and the `SidebarHost` type. Then add component imports near the other sidebar-component imports:

```ts
import { HostGroupHeader } from './HostGroupHeader'
import { SessionActivityCard } from './SessionActivityCard'
import type { SidebarHost } from './worktree-list-groups'
import { groupRowsByHost, getHostHeaderKey } from './worktree-list-groups'
```

- [ ] **Step 3: Subscribe to host state + build `hostForKey`**

Near the `const groupBy = useAppStore((s) => s.groupBy)` subscription, add subscriptions to host metadata and SSH labels (re-use `sshConnectionStates` if already subscribed; otherwise add it):

```ts
const hostMetaByKey = useAppStore((s) => s.hostMetaByKey)
const sshTargetLabels = useAppStore((s) => s.sshTargetLabels)
// sshConnectionStates is already subscribed in this component; reuse it.
```

Then add a memoized resolver (place it just above the `rows` useMemo):

```ts
const hostForKey = useCallback(
  (hostKey: string): SidebarHost => {
    const meta = hostMetaByKey[hostKey]
    const isSsh = hostKey.startsWith('ssh:')
    const connectionId = isSsh ? hostKey.slice('ssh:'.length) : undefined
    const status: SidebarHost['status'] = !isSsh
      ? 'reachable'
      : (() => {
          const s = connectionId ? sshConnectionStates.get(connectionId)?.status : undefined
          return s === 'connected'
            ? 'reachable'
            : s === 'connecting'
              ? 'connecting'
              : s === 'error'
                ? 'down'
                : 'unknown'
        })()
    return {
      key: hostKey,
      kind: isSsh ? 'ssh' : 'local',
      label:
        meta?.label ??
        (isSsh ? (connectionId ? (sshTargetLabels.get(connectionId) ?? 'Remote host') : 'Remote host') : 'This Mac'),
      detail: meta?.detail,
      status
    }
  },
  [hostMetaByKey, sshConnectionStates, sshTargetLabels]
)
```

- [ ] **Step 4: Normalize host mode + wrap the rows**

The existing `rows` useMemo calls `buildRows(groupBy, ...)`. Host mode reuses the repo builder, then wraps. Introduce `effectiveGroupBy` (host → repo) for the inner builder and section-activity/ordering, and wrap the result.

Replace the `rows` useMemo body so it reads:

```ts
const effectiveGroupBy: WorktreeGroupBy = groupBy === 'host' ? 'repo' : groupBy
const rows: Row[] = useMemo(() => {
  const built = buildRows(
    effectiveGroupBy,
    worktrees,
    repoMap,
    prCache,
    effectiveCollapsedGroups,
    repoOrder,
    workspaceStatuses,
    projectGroupOrdering,
    worktreeLineageById,
    worktreeMap,
    true,
    settings,
    // Project-group nesting is out of scope for host-first v1; pass [] in host
    // mode so blocks are flat repo groups the host post-processor can bucket.
    groupBy === 'host' ? [] : projectGroups,
    placeholderRepoIds,
    importedWorktreesByRepo
  )
  return groupBy === 'host' ? groupRowsByHost(built, hostForKey, effectiveCollapsedGroups) : built
}, [
  effectiveGroupBy,
  groupBy,
  worktrees,
  repoMap,
  prCache,
  effectiveCollapsedGroups,
  repoOrder,
  workspaceStatuses,
  projectGroupOrdering,
  worktreeLineageById,
  worktreeMap,
  settings,
  projectGroups,
  placeholderRepoIds,
  importedWorktreesByRepo,
  hostForKey
])
```

Also update other reads of `groupBy` that feed the inner repo logic to use `effectiveGroupBy`, so host mode behaves repo-like for everything except the host wrap:
- `getProjectGroupOrdering(groupBy, sortBy)` → `getProjectGroupOrdering(effectiveGroupBy, sortBy)` (line ~3809). Move/duplicate the `effectiveGroupBy` computation above this call (it's a plain const, hoist it near the top of the component body so all consumers see it).
- The `buildWorktreeSectionActivitySummaries({ groupBy, ... })` call (line ~3839) → pass `groupBy: effectiveGroupBy`.
- `WorktreeGroupBy` must be imported as a type if not already (it is exported from `./worktree-list-groups`).

Leave the `viewportResetKey` and `renderedProjects`/sticky logic keyed on the raw `groupBy` — host mode still wants its own viewport reset key, and `renderedProjects` already guards on `row.repo != null` which host headers don't have.

- [ ] **Step 5: Render the `host-header` branch**

In the `virtualItems.map` callback, immediately after `if (!row) { return null }` and BEFORE `if (row.type === 'header')`, add:

```tsx
if (row.type === 'host-header') {
  return (
    <div
      key={vItem.key}
      role="presentation"
      data-worktree-virtual-row
      data-worktree-virtual-row-key={String(vItem.key)}
      data-index={vItem.index}
      ref={measureVirtualRowElement}
      className="absolute left-0 right-0 pt-1"
      style={{ transform: getVirtualRowTransform(vItem.start) }}
    >
      <HostGroupHeader
        host={row.host}
        count={row.count}
        collapsed={collapsedGroups.has(row.key)}
        onToggle={() => toggleGroupWithScrollAnchor(row.key)}
      />
    </div>
  )
}
```

(Host headers are intentionally non-sticky in v1 — the sticky-header machinery keys on `row.type === 'header'`. Sticky host headers are a follow-up.)

- [ ] **Step 6: Render the `SessionActivityCard` under the active leaf**

In the `row.type === 'item'` render branch, locate where the worktree card element is returned. Wrap the existing returned card so the activity card renders directly beneath it when the row is the active worktree. Concretely, find the `return ( ... )` for the item branch and change it to render a fragment:

```tsx
return (
  <>
    {/* existing worktree card element — left unchanged */}
    {EXISTING_CARD_JSX}
    {row.worktree.id === activeWorktreeId ? (
      <SessionActivityCard worktreeId={row.worktree.id} />
    ) : null}
  </>
)
```

Replace `EXISTING_CARD_JSX` with the card markup already present in that branch (do not retype it — wrap what's there). `activeWorktreeId` is available via the store; if it isn't already subscribed in this component, add `const activeWorktreeId = useAppStore((s) => s.activeWorktreeId)` near the other subscriptions. The virtualizer measures item heights dynamically via `measureVirtualRowElement`, so the added card height is measured automatically — no fixed-size change needed.

- [ ] **Step 7: Typecheck + build**

Run: `cd crates/agentum-desktop/ui && npx tsc --noEmit && npm run build`
Expected: PASS. Fix any remaining `groupBy` exhaustiveness errors by routing them through `effectiveGroupBy` as in Step 4.

- [ ] **Step 8: Commit**

```bash
git add crates/agentum-desktop/ui/src/components/sidebar/WorktreeList.tsx
git commit -m "feat(desktop): render host-first tree + active-session card in WorktreeList"
```

### Task 12: Add the per-leaf `ctx %` chip

**Files:**
- Modify: `src/components/sidebar/WorktreeCardMeta.tsx`

- [ ] **Step 1: Read the meta component**

Read `src/components/sidebar/WorktreeCardMeta.tsx` to find the props (it receives the worktree or worktreeId) and the row where the branch is rendered.

- [ ] **Step 2: Add the ctx% chip**

Import the hook and render a chip next to the branch. Add near the top:

```tsx
import { useLatestAgentActivity } from './useLatestAgentActivity'
```

Inside the component (using the worktree id available in props — adapt the name to the actual prop, e.g. `worktree.id`):

```tsx
const { contextUsagePercent } = useLatestAgentActivity(worktree.id)
```

and render, adjacent to the branch element:

```tsx
{typeof contextUsagePercent === 'number' ? (
  <span className="shrink-0 text-[11px] tabular-nums text-muted-foreground">
    ctx {Math.round(contextUsagePercent)}%
  </span>
) : null}
```

- [ ] **Step 3: Typecheck + build**

Run: `cd crates/agentum-desktop/ui && npx tsc --noEmit && npm run build`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/agentum-desktop/ui/src/components/sidebar/WorktreeCardMeta.tsx
git commit -m "feat(desktop): per-session ctx% chip in sidebar leaf"
```

### Task 13: Hydrate host metadata on sidebar mount

**Files:**
- Modify: `src/components/sidebar/index.tsx`

- [ ] **Step 1: Read the sidebar container**

Read `src/components/sidebar/index.tsx` (133 lines) to find the component body and existing `useEffect`/store usage.

- [ ] **Step 2: Call `hydrateHosts` on mount + on SSH connection changes**

Add near the other store subscriptions:

```tsx
const hydrateHosts = useAppStore((s) => s.hydrateHosts)
const sshConnectedGeneration = useAppStore((s) => s.sshConnectedGeneration)
```

and an effect (add `import { useEffect } from 'react'` if not present):

```tsx
useEffect(() => {
  void hydrateHosts()
}, [hydrateHosts, sshConnectedGeneration])
```

(`sshConnectedGeneration` bumps when any SSH target connects, so the host OS line refreshes once a remote becomes reachable. `hydrateHosts` is best-effort and dedupes via the `server-host-client` connection cache.)

- [ ] **Step 3: Build + manual verification**

Run: `cd crates/agentum-desktop/ui && npm run build`
Then rebuild and run the desktop app per CLAUDE.md (`cargo build -p agentum-desktop` + launch). Verify:
- The sidebar shows a HOST header (e.g. "This Mac / studio" with a `localhost · …` line) above the repo groups.
- Repos nest under their host; remote (SSH) repos appear under a separate `ssh …` host header.
- Selecting a session shows the activity card with last message + last tool call.
- Each leaf shows `ctx N%` when the agent reports context usage.
- Collapsing a host header hides its repos and persists across reloads.

- [ ] **Step 4: Commit**

```bash
git add crates/agentum-desktop/ui/src/components/sidebar/index.tsx
git commit -m "feat(desktop): hydrate host metadata on sidebar mount"
```

### Task 14: Full regression pass

- [ ] **Step 1: Run the sidebar + store test suites**

Run:
```bash
cd crates/agentum-desktop/ui
npx vitest run src/components/sidebar src/store/slices/hosts.test.ts src/store/slices/ui.test.ts
```
Expected: PASS. Investigate and fix any sidebar tests that assumed repo-first as the default layout (update their `groupBy` setup to `'repo'` explicitly, or to `'host'` where host-first is now correct).

- [ ] **Step 2: Typecheck + build**

Run: `cd crates/agentum-desktop/ui && npx tsc --noEmit && npm run build`
Expected: PASS.

- [ ] **Step 3: Commit any test fixups**

```bash
git add -A
git commit -m "test(desktop): align sidebar tests with host-first default"
```

---

## Phase 5 — Deferred: rich OS/arch + CPU enrichment (own follow-up spec)

The mockup's full fidelity (`· x86_64`, `· M3 Max`) is **not** shipped in v1 because the data doesn't exist yet: `agentum-core::HostSystemInfo` carries only `uname: Option<String>` (`uname -sr`, e.g. "Linux 6.9") — no architecture, pretty OS name, or CPU/chip model. v1 renders the truthful `localhost · Darwin 24.5` / `ssh forge.lan · Linux 6.9` line from that existing field.

Full fidelity is a backend change across the host-probe subsystem and deserves its own spec:
- Extend `HostSystemInfo` (in `crates/agentum-core/src/lib.rs`) with `#[serde(default)]` `arch: Option<String>`, `os_pretty: Option<String>`, `cpu_brand: Option<String>` (forward-compatible with older daemons).
- Populate them: local via `sysinfo` (already a dep of `agentum-server`: `System::long_os_version()`, `System::cpu_arch()`, `cpus()[0].brand()`); SSH via `uname -sm` + `/etc/os-release` (or `sw_vers` on macOS) in the existing readiness probe (`crates/agentum-server/src/host_runtime.rs`, `host_install_hints.rs`).
- Compose the richer detail line in `hosts.ts` (`getServerHostReadinessUname` → a `getServerHostReadinessSystem` returning all fields), degrading to the v1 `uname` line when the new fields are absent.

This is intentionally out of the current plan so v1 ships UI-only.

---

## Self-review notes

- **Spec coverage:** host→repo→worktree hierarchy (Tasks 4–5, 11) ✓; host OS metadata line (Tasks 1–2, 13) ✓; reachability dot + count badge (Tasks 5, 8, 11) ✓; per-session branch (existing) + ctx% (Task 12) + status dots (existing agent-status) ✓; active-session card (Tasks 6–9, 11) ✓; collapse per host & repo via shared `collapsedGroups` (Tasks 5, 11) ✓; `PRIMARY` deliberately omitted (spec non-goal) ✓; old groupBy modes preserved, host-first default (Task 10) ✓.
- **Type consistency:** `SidebarHost`/`HostHeaderRow`/`hostKeyForRepo`/`getHostHeaderKey`/`groupRowsByHost` defined in Task 4–5 and consumed unchanged in Tasks 8/11; `LatestAgentActivity`/`latestFromEntries`/`selectLatestAgentActivity`/`useLatestAgentActivity` defined Task 6–7, consumed Tasks 9/12; `HostsSlice`/`hostMetaByKey`/`hydrateHosts`/`setHostMeta` defined Task 2, registered Task 3, consumed Tasks 11/13.
- **Known read-then-edit tasks (11–13):** WorktreeList/WorktreeCardMeta/index.tsx are large or unread in full; each starts with an explicit read step and provides the exact snippet to insert, since reproducing those files verbatim is neither safe nor useful.
