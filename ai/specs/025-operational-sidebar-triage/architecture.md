# Architecture — Operational sidebar triage

## Current-state findings

- `Sidebar` composes `SidebarNav`, `SidebarHeader`, and `WorktreeList` inside the existing
  220–500 px resizable shell (`components/sidebar/index.tsx:23-61`). The header and list can
  share transient search state without moving it into Zustand or persistence.
- `WorktreeList` already owns visibility filtering, stable sorting, keyboard selection,
  activation/reveal, context menus, native drag, lineage, and TanStack virtualization
  (`components/sidebar/WorktreeList.tsx:3920-4520`, `5080-5205`). The rendered order is copied
  to `setVisibleWorktreeIds`, which keeps Cmd+1–9 aligned with the screen.
- `resolveWorktreeStatus` is the card-dot source of truth. It combines watchdog awaiting/working,
  fresh hook state, live PTY/title fallback, retained done, browser activity, and server session
  liveness with explicit precedence (`lib/worktree-status.ts:65-154`).
- `selectWorktreeAgentActivitySummary` batches/caches fresh hook and awaiting-input facts per
  worktree (`components/sidebar/worktree-agent-activity-summary.ts:45-132`), and
  `selectServerWorktreeActivity` supplies relaunch-safe server truth.
- `buildRows` produces the flat `Row[]` consumed by the virtualizer, while measured rows correct
  seeded estimates (`components/sidebar/worktree-list-groups.ts:595-760`,
  `components/sidebar/worktree-list-virtual-rows.ts:18-62`).
- `WorktreeCard` already owns the complete workspace interaction boundary: activation,
  selection, context menu, browser-tab drop, native worktree drag, rename, active/reveal styling,
  SSH state, and quick actions (`components/sidebar/WorktreeCard.tsx:65-188`, `620-1010`).
- Grouping is debounced-persisted by `App` (`App.tsx:900-945`) and hydrated by the UI slice
  (`store/slices/ui.ts:1584-1665`). The current one-time hosts-first migration deliberately
  overrides old `repo` defaults (`ui.ts:216-236`), which cannot be reused because AC 7 forbids
  overwriting an explicit persisted choice.
- The configured worktree-search action is `worktree.palette` (`shared/keybindings.ts:198-210`)
  and its app-level handler currently toggles the palette (`App.tsx:1195-1210`).

## Important decisions

### D1 — Add a real `operational` grouping value

Extend `WorktreeGroupBy`, `UISlice.groupBy`, `PersistedUIState.groupBy`, and the options menu with
`operational`. Set fresh in-memory and `getDefaultUIState()` defaults to `operational`. Replace
the hosts-first one-shot override with a defensive normalizer:

- known explicit values, including `host`, remain unchanged;
- legacy `parent` maps to `host`;
- absent/invalid values map to `operational`.

This is a renderer UI-preference enum extension, not a backend/SQLite/worktree metadata schema
change. It honors AC 7 even for an upgraded user who explicitly chose `repo` before the old
host-migration flag was stamped.

**Tradeoff:** upgraded users with any explicit grouping do not automatically see V2. We choose
preference safety over a forced rollout because the spec explicitly limits the new default to
unconfigured state. Operational remains directly selectable in Workspace Options.

### D2 — Classify from the shared status resolver, not from a parallel detector

Export a pure `resolveWorktreeStatusFromState(state, worktreeId)` seam from
`worktree-section-activity.ts` (renaming/generalizing its current private
`getSectionWorktreeStatus`). `WorktreeList` builds one status/fact map for the visible worktrees
from its existing epoch/topology snapshot. `operational-sidebar-model.ts` only maps resolved
status to presentation:

| Resolved status | Section | Label |
| --- | --- | --- |
| `permission` | Needs You | Needs input |
| `working` | Active | Working |
| `done` | Active | Ready |
| `active` | Active | Active |
| `inactive` | Settled | Settled |

`resolveWorktreeStatus` already implements permission > working > done > active > inactive, so
multi-pane disagreement produces one urgent winner and one section. Freshness remains owned by
existing selectors. No poll, timer-based status detector, or duplicated watchdog interpretation
is added.

### D3 — Build operational rows as standard virtual rows

Add `operational-sidebar-model.ts` with pure functions that:

1. normalize/search workspace display name, branch, project display name, and latest visible
   agent label;
2. partition each matching workspace exactly once;
3. sort Needs You and Active by pinned-first then attention/state timestamp (stable
   `lastActivityAt`/name fallback), and Settled by `lastActivityAt` descending;
4. emit three headers with full filtered counts plus item rows carrying
   `presentation: 'operational-rich' | 'operational-settled'` and resolved display facts;
5. cap emitted Settled item rows at three until transient `settledExpanded` is true.

Extend the exported `Row` item shape rather than creating a second list. Empty headers still
render so the queue always has exactly three named sections and truthful zero counts. Pinned
workspaces remain inside their truthful operational section; there is no fourth Pinned section.
Lineage nesting is disabled only in operational mode because parent and child may belong to
different operational sections; every workspace remains independently reachable.

The settled expand/collapse row is a new virtual row variant with a stable key. Full counts live
on the header and do not change when disclosure changes. `estimateRenderRowSize` gains explicit
estimates for rich, compact, and disclosure rows; existing `measureElement`, scroll-anchor
suppression, `viewportResetKey`, and reveal-by-worktree ID remain unchanged.

### D4 — Keep one interaction owner

Add an optional `presentation`/`operationalMeta` prop to `WorktreeCard`. The component retains
its existing outer interactive surface and handlers, but selects one of three bodies:

- existing/default body for all alternate grouping modes;
- rich operational body (project, status/age, display name, branch, agent);
- compact settled body (status mark, display name, relative activity age).

`VirtualizedWorktreeViewport.renderWorktreeRow` continues to pass the same activation,
selection, context-menu, drag, reveal, SSH, and lineage callbacks. There is no second card
component with copied behavior. Operational mode suppresses inline-agent expansion inside the
rich body because the required agent/status summary is already present; `SessionActivityCard`
continues to render for the active workspace below the row.

### D5 — Lift transient query; reuse persisted project filtering

`Sidebar` owns `operationalQuery` as component state and passes it to `SidebarHeader` and
`WorktreeList`. It is deliberately not persisted: closing/reopening the app restores the full
queue and cannot strand workspaces behind stale text.

When `groupBy === 'operational'`, `SidebarHeader` renders `OperationalSidebarControls` in place
of the current title-only row:

- search input, configured `worktree.palette` shortcut hint, and existing new-workspace action;
- All plus project chips backed by existing `filterRepoIds` / `setFilterRepoIds`;
- Workspace Options remains reachable for grouping, sorting, and non-project filters;
- an overflow dropdown containing every project that does not fit.

`operational-project-overflow.ts` implements a pure contiguous-prefix packing calculation. The
component measures the chip rail with `ResizeObserver`, always reserves All + overflow space,
prioritizes currently selected project chips, and puts the remaining projects in the dropdown.
At measurement failure/zero width it safely renders All + overflow, never loses a filter.

The existing `computeVisibleWorktreeIds` remains responsible for project/sleep/default-branch
filtering. The operational model applies text search afterward, so the filters compose. Its
ordered result is still passed to `setVisibleWorktreeIds`.

### D6 — Route the existing search action to the inline field only in V2

Add `lib/operational-sidebar-search-focus.ts` with one request event and retry-safe DOM focus
helper. In `App.tsx`, `worktree.palette` keeps opening the existing palette unless the sidebar is
open and `groupBy === 'operational'`; in that case it focuses/selects the inline input. The
SidebarNav Search button uses the same branch. Switching to another grouping restores the old
palette behavior and no user keybinding is changed.

### D7 — Share one relative-time clock

The operational header/list uses `useNow(30_000)` from `components/dashboard/useNow.ts` and a
small pure short-age formatter in `operational-sidebar-model.ts`. One shared tick refreshes all
visible labels. State age comes from the winning fresh agent entry when available; otherwise the
model uses `worktree.lastActivityAt`. Non-finite/missing timestamps omit the suffix rather than
showing misleading values.

## Components and exact file changes

### New

- `components/sidebar/operational-sidebar-model.ts` — pure search, partition, labels, ordering,
  counts, disclosure, and short-age formatting.
- `components/sidebar/operational-sidebar-model.test.ts` — precedence/exclusivity, stale inputs
  (through resolved statuses), search fields, counts, ordering, missing metadata, disclosure.
- `components/sidebar/OperationalSidebarControls.tsx` — inline search, chips, overflow, options,
  add action, accessible labels/roving tab order.
- `components/sidebar/OperationalSidebarControls.test.tsx` — composed project toggles, All,
  overflow, keyboard/focus, missing-width fallback.
- `components/sidebar/operational-project-overflow.ts` and `.test.ts` — deterministic packing.
- `lib/operational-sidebar-search-focus.ts` and `.test.ts` — event/focus routing.

### Modify

- `shared/types.ts` — add `operational` to persisted grouping union and optional operational row
  display types only if they are shared outside the sidebar.
- `shared/constants.ts` — fresh persisted UI default becomes `operational`.
- `store/slices/ui.ts` + `ui.test.ts` — runtime default, persisted normalizer, explicit-choice
  preservation, legacy `parent` handling, invalid/absent fallback.
- `components/sidebar/worktree-list-groups.ts` — grouping union and item/disclosure row variants;
  alternate `buildRows` behavior stays unchanged.
- `components/sidebar/worktree-list-virtual-rows.ts` + tests — seeded sizes and sticky behavior
  for operational rows.
- `components/sidebar/worktree-section-activity.ts` + tests — export the existing shared
  state-to-status resolver.
- `components/sidebar/WorktreeList.tsx` — build fact/model maps, choose operational rows, own
  expanded state, preserve rendered ID cache, and render disclosure via the same viewport.
- `components/sidebar/WorktreeCard.tsx` + focused render tests — operational body variants inside
  the existing interaction boundary.
- `components/sidebar/SidebarHeader.tsx`, `components/sidebar/index.tsx`, and
  `components/sidebar/SidebarNav.tsx` — lift/query props and conditional control surface.
- `components/sidebar/SidebarWorkspaceOptionsMenu.tsx` + `SidebarNav.test.tsx` — expose/select
  Operational and retain alternate modes/search behavior.
- `App.tsx` + focused shortcut test — inline focus routing only when V2 is visible.

No Rust crate, server route, Tauri command, database migration, watchdog, adapter, or worktree
metadata file changes.

## Data and control flow

```text
push-fed store state ──► resolveWorktreeStatusFromState(worktree)
                              │
Worktree + Repo + latest agent facts + query
                              │
                              ▼
                 buildOperationalSidebarRows
                  ├─ Needs You header/items
                  ├─ Active header/items
                  └─ Settled header/first 3 or all + disclosure
                              │
                              ▼
              existing VirtualizedWorktreeViewport
                              │
                 existing WorktreeCard handlers
```

Project chip → existing `setFilterRepoIds` → `computeVisibleWorktreeIds` → operational search →
row build. Search shortcut/button → focus request → inline input → local query → row build. Live
agent transition → existing `agentStatusEpoch`/server activity update → status map recompute → a
workspace moves atomically between sections on the next render.

## Race, error, and performance handling

- Status maps are derived synchronously from one store snapshot per model rebuild; no workspace
  can occupy two sections during a React commit.
- Existing fresh-entry checks and server activity own stale-state behavior. The presentation
  model never extends a signal's lifetime.
- Query changes and project changes reset `settledExpanded` to false in one effect so an old
  expansion cannot create a surprising huge result set; active selection is untouched.
- A focus request during mount/sidebar reveal retries for two animation frames, matching
  `focusWorktreeSidebar`; failure is a no-op and the existing palette path is retained when V2
  is not mounted.
- ResizeObserver output is rounded and identity-compared before state writes to avoid resize
  feedback loops. Chip packing is O(projects) and status/model building is O(workspaces + agents).
- Virtual row keys are `operational:<section>`, `worktree.id`, and
  `operational:settled-disclosure`; transitions remount only the moved row/header neighborhood.
- Text uses `min-w-0`, `truncate`, and `overflow-x-hidden`; optional agent/age segments are
  omitted rather than rendered as em dashes.

## Acceptance criteria → implementation and verification

| AC | Implementation seam | Verification |
| --- | --- | --- |
| 1 | `buildOperationalSidebarRows`; standard header rows | `operational-sidebar-model.test.ts`: fixed order, zero/full counts, complete membership |
| 2 | shared `resolveWorktreeStatusFromState` + status mapping | model/section-activity tests: permission precedence, working/done/active/inactive, one row only |
| 3 | `OperationalSidebarControls`, existing repo filter setter, query model, focus helper | controls/model/App shortcut tests: all fields, composed filters, clear restore, overflow |
| 4 | `WorktreeCard` rich presentation + `useNow` | render tests: present/omitted facts, truthful labels/age, long-value truncation classes |
| 5 | settled model cap + disclosure row/local state | model/list tests: recent order, 3-row cap, exact N, expand/collapse preserves counts/query/filter/active ID |
| 6 | existing `WorktreeCard` outer surface and viewport callbacks | interaction render tests: activate, Enter, context menu, native/browser drag, selection, reveal, active styles |
| 7 | grouping union/default/normalizer + options menu | `ui.test.ts` and options test: absent→operational, every explicit value preserved, alternates reachable |
| 8 | constrained control/card CSS + semantic input/buttons/options | controls/card tests plus `qa.sh` keyboard walk and 220/500 px light/dark screenshots/contrast audit |
| 9 | focused suites and production build | `verify.sh` runs named Vitest files then `npm run build --prefix crates/agentum-desktop/ui` |

## Build order

1. Extend/normalize the grouping preference and add its tests.
2. Export the shared status resolver; implement/test the pure operational model and overflow
   packing.
3. Add conditional controls and shortcut focus routing with component tests.
4. Add row variants, WorktreeCard bodies, virtual estimates, and integrate `WorktreeList`.
5. Run focused suites, production build, and real-browser `qa.sh` scenarios.

## Architect gate verdict

**PASS.** Every acceptance criterion maps to an existing or named implementation seam and a
named verification path. All user-visible choices are resolved, alternate grouping behavior is
preserved, existing push/state/interaction/virtualization primitives are reused, and no backend
invariant is weakened.
