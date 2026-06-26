# Mission Control Stats — Phase 2 (frontend) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Mission Control (the `activity` view) the Usage & Stats dashboard and the default panel on every launch — reusing `<StatsPane/>` unchanged, relocating Landing's preflight + Add-Project bits, showing OpenCode as "Soon", and deleting the agent-activity feed prototype + `Landing.tsx`.

**Architecture:** Mission Control stops rendering the prototype agent feed and renders a new self-contained `MissionControlPage` instead. The page composes a header (preflight banner + Add Project / Create Workspace, ported from Landing), the prop-less `<StatsPane/>`, and a "Coming soon" section. The default `activeView` flips `terminal → activity`; the no-workspace terminal branch routes to the same page so the user is never stranded. The agent-status store, the `activity` view type/nav entry, the unread badge, and `activity-terminal-portal.ts` are all preserved.

**Tech Stack:** React 18 + TypeScript + Vite, zustand store (`useAppStore`), Tailwind, lucide-react icons, shadcn-style `ui/*` primitives, vitest (node env).

## Global Constraints

- **All commands run from `crates/agentum-desktop/ui/`** unless stated otherwise.
- **Build gate:** `npm run build` (= `vite build`) must pass after every task. This is the real typecheck — bare `tsc` cannot resolve the `@/` and `shared/*` aliases (Vite-only), so do NOT use `tsc`.
- **Test runner:** vitest, node environment, pure-function tests only. There is **no React render harness** (`@testing-library`/jsdom are not deps). Test extractable logic as `.test.ts` pure functions; verify JSX composition via `npm run build` + the manual step each task names. Run one file with `npx vitest run <path>`; the whole suite with `npx vitest run`.
- **Known pre-existing vitest noise:** ~7 files fail importing `@xterm/addon-ligatures`. That is NOT your change — never "fix" it; run targeted files by path.
- **Reuse `<StatsPane/>` unchanged.** Do not modify `StatsPane.tsx`.
- **Preserve:** the `activeView` union value `'activity'`, `openActivityPage`/`closeActivityPage`, the sidebar Mission Control nav entry + its unread badge, `useActivityUnreadCount.ts`, the agent-status store slice, and `components/activity/activity-terminal-portal.ts`.
- **Aliases:** `@` → `src`; the literal import prefix `../../../../shared` → `src/shared`. New files under `src/components/mission-control/` are at the same depth as `src/components/stats/`, so use `../../../../shared/...` for `shared` imports and `@/...` for everything else.
- **Commits:** conventional messages ending with `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Commit only the paths the task names (other agents share this checkout — never `git add -A`).

---

### Task 1: Extract `getPreflightIssues` into `src/lib/preflight-issues.ts`

Move the pure preflight helper out of `Landing.tsx` (which gets deleted in Task 5) into `lib/`, where pure helpers live and are unit-tested. Update Landing to import it so the build stays green between tasks.

**Files:**
- Create: `src/lib/preflight-issues.ts`
- Create: `src/lib/preflight-issues.test.ts`
- Modify: `src/components/Landing.tsx:18-61` (delete the local `PreflightIssue` type + `getPreflightIssues`, import them instead), `src/components/Landing.tsx:1-10` (add the import)

**Interfaces:**
- Produces: `export type PreflightIssue = { id: string; title: string; description: string; fixLabel: string; fixUrl: string }` and `export function getPreflightIssues(status: { git: { installed: boolean }; gh: { installed: boolean; authenticated: boolean } }): PreflightIssue[]`

- [ ] **Step 1: Write the failing test**

Create `src/lib/preflight-issues.test.ts`:

```ts
import { describe, expect, it } from 'vitest'
import { getPreflightIssues } from './preflight-issues'

const ALL_OK = { git: { installed: true }, gh: { installed: true, authenticated: true } }

describe('getPreflightIssues', () => {
  it('returns no issues when git + gh are installed and authenticated', () => {
    expect(getPreflightIssues(ALL_OK)).toEqual([])
  })

  it('flags missing git', () => {
    const ids = getPreflightIssues({ ...ALL_OK, git: { installed: false } }).map((i) => i.id)
    expect(ids).toContain('git')
  })

  it('flags missing gh CLI (and not gh-auth)', () => {
    const ids = getPreflightIssues({
      ...ALL_OK,
      gh: { installed: false, authenticated: false }
    }).map((i) => i.id)
    expect(ids).toContain('gh')
    expect(ids).not.toContain('gh-auth')
  })

  it('flags unauthenticated gh when installed (and not gh)', () => {
    const ids = getPreflightIssues({
      ...ALL_OK,
      gh: { installed: true, authenticated: false }
    }).map((i) => i.id)
    expect(ids).toContain('gh-auth')
    expect(ids).not.toContain('gh')
  })
})
```

- [ ] **Step 2: Run the test, verify it FAILS**

Run: `npx vitest run src/lib/preflight-issues.test.ts`
Expected: FAIL — `Failed to resolve import "./preflight-issues"` (module does not exist yet).

- [ ] **Step 3: Create the module**

Create `src/lib/preflight-issues.ts` (verbatim move from `Landing.tsx:18-61`):

```ts
export type PreflightIssue = {
  id: string
  title: string
  description: string
  fixLabel: string
  fixUrl: string
}

export function getPreflightIssues(status: {
  git: { installed: boolean }
  gh: { installed: boolean; authenticated: boolean }
}): PreflightIssue[] {
  const issues: PreflightIssue[] = []

  if (!status.git.installed) {
    issues.push({
      id: 'git',
      title: 'Git is not installed',
      description: 'Git is required for Git projects, source control, and workspace management.',
      fixLabel: 'Install Git',
      fixUrl: 'https://git-scm.com/downloads'
    })
  }

  if (!status.gh.installed) {
    issues.push({
      id: 'gh',
      title: 'GitHub CLI is not installed',
      description: 'Agentum uses the GitHub CLI (gh) to show pull requests, issues, and checks.',
      fixLabel: 'Install GitHub CLI',
      fixUrl: 'https://cli.github.com'
    })
  } else if (!status.gh.authenticated) {
    issues.push({
      id: 'gh-auth',
      title: 'GitHub CLI is not authenticated',
      description: 'Run "gh auth login" in a terminal to connect your GitHub account.',
      fixLabel: 'Learn more',
      fixUrl: 'https://cli.github.com/manual/gh_auth_login'
    })
  }

  return issues
}
```

- [ ] **Step 4: Run the test, verify it PASSES**

Run: `npx vitest run src/lib/preflight-issues.test.ts`
Expected: PASS — `Test Files  1 passed`, `Tests  4 passed`.

- [ ] **Step 5: Update `Landing.tsx` to import the helper**

In `src/components/Landing.tsx`, delete the local `type PreflightIssue = {...}` block and the `function getPreflightIssues(...) {...}` (current lines 18-61). Add to the import block near the top (after line 9):

```ts
import { getPreflightIssues, type PreflightIssue } from '@/lib/preflight-issues'
```

Leave everything else in `Landing.tsx` unchanged (it still references `PreflightIssue` and `getPreflightIssues`, now imported).

- [ ] **Step 6: Build to verify Landing still compiles**

Run: `npm run build`
Expected: build succeeds (`✓ built in …`).

- [ ] **Step 7: Commit**

```bash
git add src/lib/preflight-issues.ts src/lib/preflight-issues.test.ts src/components/Landing.tsx
git commit -m "refactor(desktop): extract getPreflightIssues into lib

Pure preflight helper moves to src/lib/preflight-issues.ts (unit-tested)
ahead of Landing.tsx deletion in the Mission Control redesign.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: Soon-cards data module + `MissionControlPage`

Add the testable "Coming soon" card data, then build the page that composes the relocated header, `<StatsPane/>`, and the soon section.

**Files:**
- Create: `src/components/mission-control/mission-control-soon-cards.ts`
- Create: `src/components/mission-control/mission-control-soon-cards.test.ts`
- Create: `src/components/mission-control/MissionControlPage.tsx`

**Interfaces:**
- Consumes: `getPreflightIssues`, `PreflightIssue` (Task 1); `StatsPane` (`@/components/stats/StatsPane`); `Badge` (`@/components/ui/badge`); `useAppStore` (`@/store`); `api` (`@/tauri`); `isGitRepoKind` (`../../../../shared/repo-kind`).
- Produces: `export type MissionControlSoonCard = { id: string; title: string; description: string; icon: 'orchestration' | 'schedule' | 'cost' }`, `export const MISSION_CONTROL_SOON_CARDS: MissionControlSoonCard[]`, and `export default function MissionControlPage(): React.JSX.Element`.

- [ ] **Step 1: Write the failing test for the soon-cards data**

Create `src/components/mission-control/mission-control-soon-cards.test.ts`:

```ts
import { describe, expect, it } from 'vitest'
import { MISSION_CONTROL_SOON_CARDS } from './mission-control-soon-cards'

describe('MISSION_CONTROL_SOON_CARDS', () => {
  it('lists exactly three coming-soon capabilities', () => {
    expect(MISSION_CONTROL_SOON_CARDS).toHaveLength(3)
  })

  it('leads with Agent Orchestration', () => {
    expect(MISSION_CONTROL_SOON_CARDS[0]).toMatchObject({
      id: 'agent-orchestration',
      title: 'Agent Orchestration',
      icon: 'orchestration'
    })
  })

  it('has unique ids, a known icon, and non-empty copy per card', () => {
    const ids = MISSION_CONTROL_SOON_CARDS.map((c) => c.id)
    expect(new Set(ids).size).toBe(ids.length)
    for (const card of MISSION_CONTROL_SOON_CARDS) {
      expect(['orchestration', 'schedule', 'cost']).toContain(card.icon)
      expect(card.title.length).toBeGreaterThan(0)
      expect(card.description.length).toBeGreaterThan(0)
    }
  })
})
```

- [ ] **Step 2: Run the test, verify it FAILS**

Run: `npx vitest run src/components/mission-control/mission-control-soon-cards.test.ts`
Expected: FAIL — cannot resolve `./mission-control-soon-cards`.

- [ ] **Step 3: Create the soon-cards data module**

Create `src/components/mission-control/mission-control-soon-cards.ts`:

```ts
// Why: the icon is a string key (not a lucide component) so this data module
// stays import-light and node-testable; MissionControlPage maps the key to a
// component. Order matters — Agent Orchestration leads (the user's headline
// "soon" capability).
export type MissionControlSoonCard = {
  id: string
  title: string
  description: string
  icon: 'orchestration' | 'schedule' | 'cost'
}

export const MISSION_CONTROL_SOON_CARDS: MissionControlSoonCard[] = [
  {
    id: 'agent-orchestration',
    title: 'Agent Orchestration',
    description:
      'Coordinate multiple agents across worktrees with task hand-offs and decision gates.',
    icon: 'orchestration'
  },
  {
    id: 'scheduled-automations',
    title: 'Scheduled Automations',
    description: 'Run agents and verification gates on a schedule, hands-free.',
    icon: 'schedule'
  },
  {
    id: 'cost-alerts',
    title: 'Cost Alerts',
    description: 'Get notified when token spend crosses a budget you set.',
    icon: 'cost'
  }
]
```

- [ ] **Step 4: Run the test, verify it PASSES**

Run: `npx vitest run src/components/mission-control/mission-control-soon-cards.test.ts`
Expected: PASS — `Tests  3 passed`.

- [ ] **Step 5: Create `MissionControlPage.tsx`**

Create `src/components/mission-control/MissionControlPage.tsx`. The preflight banner + its check effect are a verbatim port of `Landing.tsx` (lines 169-195 and 205-259), now using the extracted helper:

```tsx
import { useEffect, useState } from 'react'
import {
  AlertTriangle,
  BellRing,
  CalendarClock,
  ExternalLink,
  FolderPlus,
  GitBranchPlus,
  type LucideIcon,
  Workflow
} from 'lucide-react'
import { api } from '@/tauri'
import { useAppStore } from '@/store'
import { getPreflightIssues, type PreflightIssue } from '@/lib/preflight-issues'
import { StatsPane } from '@/components/stats/StatsPane'
import { Badge } from '@/components/ui/badge'
import { isGitRepoKind } from '../../../../shared/repo-kind'
import {
  MISSION_CONTROL_SOON_CARDS,
  type MissionControlSoonCard
} from './mission-control-soon-cards'

const SOON_ICONS: Record<MissionControlSoonCard['icon'], LucideIcon> = {
  orchestration: Workflow,
  schedule: CalendarClock,
  cost: BellRing
}

function PreflightBanner({ issues }: { issues: PreflightIssue[] }): React.JSX.Element {
  return (
    <div className="w-full space-y-3 rounded-lg border border-yellow-500/30 bg-yellow-500/5 p-4">
      <div className="flex items-center gap-2 text-yellow-500">
        <AlertTriangle className="size-4 shrink-0" />
        <span className="text-sm font-medium">Missing dependencies</span>
      </div>
      <div className="space-y-2.5">
        {issues.map((issue) => (
          <div key={issue.id} className="flex items-start justify-between gap-3">
            <div className="min-w-0">
              <p className="text-sm font-medium text-foreground">{issue.title}</p>
              <p className="mt-0.5 text-xs text-muted-foreground">{issue.description}</p>
            </div>
            <button
              className="inline-flex shrink-0 cursor-pointer items-center gap-1 text-xs font-medium text-blue-400 transition-colors hover:text-blue-300"
              onClick={() => api.shell.openUrl(issue.fixUrl)}
            >
              {issue.fixLabel}
              <ExternalLink className="size-3" />
            </button>
          </div>
        ))}
      </div>
    </div>
  )
}

export default function MissionControlPage(): React.JSX.Element {
  const repos = useAppStore((s) => s.repos)
  const openModal = useAppStore((s) => s.openModal)
  const canCreateWorktree = repos.length > 0
  const createTargetLabel =
    canCreateWorktree && repos.every((repo) => isGitRepoKind(repo)) ? 'Worktree' : 'Workspace'

  const [preflightIssues, setPreflightIssues] = useState<PreflightIssue[]>([])

  useEffect(() => {
    let cancelled = false
    const refreshPreflight = (force = false): void => {
      void api.preflight.check(force ? { force: true } : undefined).then((status) => {
        if (cancelled) {
          return
        }
        setPreflightIssues(getPreflightIssues(status))
      })
    }

    refreshPreflight()

    // Why: users often install/authenticate gh outside Agentum. Re-check when the
    // window becomes active again so the warning clears without relaunch.
    const handleWindowActive = (): void => {
      if (document.visibilityState === 'visible') {
        refreshPreflight(true)
      }
    }

    document.addEventListener('visibilitychange', handleWindowActive)
    window.addEventListener('focus', handleWindowActive)

    return () => {
      cancelled = true
      document.removeEventListener('visibilitychange', handleWindowActive)
      window.removeEventListener('focus', handleWindowActive)
    }
  }, [])

  useEffect(() => {
    if (preflightIssues.length === 0) {
      return
    }
    let cancelled = false
    // Why: some users complete `gh auth login` without leaving the window. Poll
    // only while a warning is visible so the banner self-clears.
    const intervalId = window.setInterval(() => {
      void api.preflight.check({ force: true }).then((status) => {
        if (cancelled) {
          return
        }
        setPreflightIssues(getPreflightIssues(status))
      })
    }, 30000)
    return () => {
      cancelled = true
      window.clearInterval(intervalId)
    }
  }, [preflightIssues.length])

  return (
    <div className="flex h-full flex-col overflow-hidden bg-background">
      <header className="flex items-center justify-between gap-3 border-b border-border/60 px-6 py-3">
        <div className="min-w-0">
          <h1 className="text-sm font-semibold text-foreground">Mission Control</h1>
          <p className="text-xs text-muted-foreground">Usage, cost, and agent activity at a glance.</p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <button
            className="inline-flex items-center gap-1.5 rounded-md border border-border/80 bg-secondary/70 px-3 py-1.5 text-sm font-medium text-foreground transition-colors hover:bg-accent"
            onClick={() => openModal('add-repo')}
          >
            <FolderPlus className="size-3.5" />
            Add Project
          </button>
          <button
            className="inline-flex items-center gap-1.5 rounded-md border border-border/80 bg-secondary/70 px-3 py-1.5 text-sm font-medium text-foreground transition-colors enabled:cursor-pointer enabled:hover:bg-accent disabled:cursor-not-allowed disabled:opacity-40"
            disabled={!canCreateWorktree}
            title={!canCreateWorktree ? 'Add a project first' : undefined}
            onClick={() => openModal('new-workspace-composer', { telemetrySource: 'unknown' })}
          >
            <GitBranchPlus className="size-3.5" />
            Create {createTargetLabel}
          </button>
        </div>
      </header>

      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-5xl space-y-6 p-6">
          {preflightIssues.length > 0 && <PreflightBanner issues={preflightIssues} />}

          <StatsPane />

          <section className="space-y-3">
            <h2 className="text-sm font-semibold text-foreground">Coming soon</h2>
            <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
              {MISSION_CONTROL_SOON_CARDS.map((card) => {
                const Icon = SOON_ICONS[card.icon]
                return (
                  <div
                    key={card.id}
                    className="cursor-default rounded-lg border border-dashed border-border/60 bg-card/30 p-4 opacity-80"
                  >
                    <div className="mb-2 flex items-center justify-between gap-2">
                      <span className="inline-flex size-8 items-center justify-center rounded-md border border-border/60 bg-card/60 text-muted-foreground">
                        <Icon className="size-4" />
                      </span>
                      <Badge variant="outline" className="shrink-0">
                        Soon
                      </Badge>
                    </div>
                    <h3 className="text-sm font-semibold text-foreground">{card.title}</h3>
                    <p className="mt-1 text-xs text-muted-foreground">{card.description}</p>
                  </div>
                )
              })}
            </div>
          </section>
        </div>
      </div>
    </div>
  )
}
```

- [ ] **Step 6: Build to verify the page compiles**

Run: `npm run build`
Expected: build succeeds. (The page isn't rendered anywhere yet — Task 4 wires it. This step only proves it type-checks.)

- [ ] **Step 7: Commit**

```bash
git add src/components/mission-control/mission-control-soon-cards.ts src/components/mission-control/mission-control-soon-cards.test.ts src/components/mission-control/MissionControlPage.tsx
git commit -m "feat(desktop): MissionControlPage — stats dashboard + soon section

New self-contained page: relocated preflight banner + Add Project /
Create Workspace, the reused <StatsPane/>, and three 'Soon' cards
(Agent Orchestration, Scheduled Automations, Cost Alerts). Not wired
into routing yet (Task: rewire App).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: OpenCode usage pane shows "Soon"

Make the OpenCode tab a deliberate "Soon" state (its backend stays stubbed in Phase 1). Preserve the existing full pane as `OpenCodeUsagePaneImpl` (exported, ready to re-wire when OpenCode lands) so there are no unused-variable / unreachable-code lint failures.

**Files:**
- Modify: `src/components/stats/OpenCodeUsagePane.tsx:1-30` (add `Badge` import), `:81` (rename the exported function to `OpenCodeUsagePaneImpl`), and append a new `OpenCodeUsagePane` wrapper after it.

**Interfaces:**
- Consumes: `Badge` (`../ui/badge`).
- Produces: `export function OpenCodeUsagePane(): React.JSX.Element` (the "Soon" card) and `export function OpenCodeUsagePaneImpl(): React.JSX.Element` (the preserved full pane). `StatsPane` already imports `{ OpenCodeUsagePane }` — unchanged.

- [ ] **Step 1: Add the `Badge` import**

In `src/components/stats/OpenCodeUsagePane.tsx`, add to the imports (after the `StatCard` import, line 30):

```ts
import { Badge } from '../ui/badge'
```

- [ ] **Step 2: Rename the existing component to `OpenCodeUsagePaneImpl`**

Change line 81 from:

```tsx
export function OpenCodeUsagePane(): React.JSX.Element {
```

to:

```tsx
export function OpenCodeUsagePaneImpl(): React.JSX.Element {
```

Leave the entire body of that function unchanged.

- [ ] **Step 3: Append the new `OpenCodeUsagePane` "Soon" wrapper**

At the END of the file (after the closing `}` of `OpenCodeUsagePaneImpl`), add:

```tsx

// Why: OpenCode usage scanning isn't ported yet (Phase 1 shipped Claude + Codex).
// Render a deliberate "Soon" state. OpenCodeUsagePaneImpl above keeps the full
// pane intact (exported) so re-enabling is a one-line swap once the scanner lands.
export function OpenCodeUsagePane(): React.JSX.Element {
  return (
    <div className="rounded-lg border border-border/60 bg-card/40 p-4">
      <div className="flex items-start justify-between gap-4">
        <div className="space-y-1.5">
          <h3 className="text-sm font-semibold text-foreground">OpenCode Usage Tracking</h3>
          <p className="text-sm text-muted-foreground">
            OpenCode usage analytics are coming soon.
          </p>
        </div>
        <Badge variant="outline" className="shrink-0">
          Soon
        </Badge>
      </div>
    </div>
  )
}
```

- [ ] **Step 4: Build to verify (rename + wrapper + Badge import all type-check)**

Run: `npm run build`
Expected: build succeeds. (No unused/unreachable errors: `OpenCodeUsagePaneImpl` is exported, `OpenCodeUsagePane` is imported by `StatsPane`.)

- [ ] **Step 5: Manual verification**

Launch the app (`cargo run -p agentum-desktop` from the repo root, or the dev launcher), open Settings → Stats & Usage (or Mission Control after Task 4), switch the Usage Analytics selector to **OpenCode**, and confirm it shows the "OpenCode Usage Tracking — coming soon" card with a **Soon** badge (no enable toggle, no data table).

- [ ] **Step 6: Commit**

```bash
git add src/components/stats/OpenCodeUsagePane.tsx
git commit -m "feat(desktop): OpenCode usage pane shows 'Soon'

OpenCode scanning isn't ported (Phase 1 = Claude + Codex). The pane now
renders a 'Soon' card; the full pane is preserved as OpenCodeUsagePaneImpl
for a one-line re-enable later.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Rewire `App.tsx` to render the dashboard + flip the default view

Make the `activity` view render `MissionControlPage`, flip the default `activeView` to `'activity'`, and route the no-workspace terminal branch to the dashboard (replacing Landing) so the user is never stranded.

**Files:**
- Modify: `src/App.tsx:216` (drop the `Landing` lazy import), `:221` (replace the `ActivityPrototypePage` lazy import with `MissionControlPage`), `:1745` and `:1747` (render switch).
- Modify: `src/store/slices/ui.ts:832` (default `activeView`).

**Interfaces:**
- Consumes: `MissionControlPage` (default export from Task 2).

- [ ] **Step 1: Swap the lazy imports in `App.tsx`**

In `src/App.tsx`, delete line 216:

```tsx
const Landing = lazy(() => import('./components/Landing'))
```

and replace line 221:

```tsx
const ActivityPrototypePage = lazy(() => import('./components/activity/ActivityPrototypePage'))
```

with:

```tsx
const MissionControlPage = lazy(() => import('./components/mission-control/MissionControlPage'))
```

- [ ] **Step 2: Update the render switch in `App.tsx`**

Replace the activity line (1745):

```tsx
                          {activeView === 'activity' ? <ActivityPrototypePage /> : null}
```

with:

```tsx
                          {activeView === 'activity' ? <MissionControlPage /> : null}
```

Replace the Landing line (1747):

```tsx
                          {activeView === 'terminal' && !activeWorktreeId ? <Landing /> : null}
```

with:

```tsx
                          {/* No workspace selected on the terminal view → fall back to
                              Mission Control (Landing.tsx removed; the dashboard needs no
                              workspace) so the user is never stranded on a blank pane. */}
                          {activeView === 'terminal' && !activeWorktreeId ? <MissionControlPage /> : null}
```

- [ ] **Step 3: Flip the default view in `ui.ts`**

In `src/store/slices/ui.ts`, change line 832 from:

```ts
  activeView: 'terminal',
```

to:

```ts
  // Why: Mission Control (the stats dashboard) is the home surface and opens on
  // every cold start. activeView is NOT persisted, so this initializer governs.
  activeView: 'activity',
```

- [ ] **Step 4: Build to verify**

Run: `npm run build`
Expected: build succeeds. `Landing` and `ActivityPrototypePage` are no longer imported in `App.tsx` (their files still exist — deleted in Task 5 — but are now unreferenced).

- [ ] **Step 5: Manual verification**

Launch the app. Confirm: (a) on cold start it opens on **Mission Control** showing the stats dashboard (not the old agent feed, not the AGENTUM landing splash); (b) the sidebar **Mission Control** (Radar) entry highlights and shows the same page; (c) the Chat / Board nav entries still work and the Mission Control unread badge still appears when an agent transitions.

- [ ] **Step 6: Commit**

```bash
git add src/App.tsx src/store/slices/ui.ts
git commit -m "feat(desktop): Mission Control is the default dashboard view

activity view now renders MissionControlPage; default activeView flips
terminal→activity (opens first on every launch); the no-workspace
terminal branch falls back to Mission Control instead of Landing.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Delete the feed prototype + Landing; guard the unread badge

Remove the now-unreferenced agent-feed prototype and `Landing.tsx`. Add a pure regression test proving the sidebar Mission Control unread badge still computes from the store (independent of the deleted feed).

**Files:**
- Delete: `src/components/activity/ActivityPrototypePage.tsx`
- Delete: `src/components/activity/ActivityPrototypePage.test.ts`
- Delete: `src/components/Landing.tsx`
- Create: `src/components/activity/useActivityUnreadCount.test.ts`

**Interfaces:**
- Consumes: `countActivityUnread` (`./useActivityUnreadCount`), `AgentStatusEntry` (`../../../../shared/agent-status-types`).

- [ ] **Step 1: Write the failing regression test**

Create `src/components/activity/useActivityUnreadCount.test.ts`:

```ts
import { describe, expect, it } from 'vitest'
import type { AgentStatusEntry } from '../../../../shared/agent-status-types'
import { countActivityUnread } from './useActivityUnreadCount'

// Minimal entry covering only the fields countActivityUnread reads in
// 'sidebar-badge' mode (state, stateStartedAt, paneKey). Cast through unknown so
// the test does not depend on the full AgentStatusEntry shape.
function doneEntry(paneKey: string, stateStartedAt: number): AgentStatusEntry {
  return {
    paneKey,
    state: 'done',
    stateStartedAt,
    stateHistory: []
  } as unknown as AgentStatusEntry
}

const EMPTY = {
  acknowledgedAgentsByPaneKey: {},
  agentStatusByPaneKey: {},
  migrationUnsupportedByPtyId: {},
  retainedAgentsByPaneKey: {},
  worktreesByRepo: {}
}

describe('countActivityUnread (badge survives feed deletion)', () => {
  it('counts an unacknowledged done agent from store state alone', () => {
    const source = {
      ...EMPTY,
      agentStatusByPaneKey: { 'tab-1:leaf-1': doneEntry('tab-1:leaf-1', 1000) }
    }
    expect(countActivityUnread(source, 'sidebar-badge')).toBe(1)
  })

  it('does not count it once acknowledged after the state started', () => {
    const source = {
      ...EMPTY,
      agentStatusByPaneKey: { 'tab-1:leaf-1': doneEntry('tab-1:leaf-1', 1000) },
      acknowledgedAgentsByPaneKey: { 'tab-1:leaf-1': 2000 }
    }
    expect(countActivityUnread(source, 'sidebar-badge')).toBe(0)
  })

  it('returns 0 for an empty store', () => {
    expect(countActivityUnread(EMPTY, 'sidebar-badge')).toBe(0)
  })
})
```

- [ ] **Step 2: Run the test, verify it PASSES (the helper already exists)**

Run: `npx vitest run src/components/activity/useActivityUnreadCount.test.ts`
Expected: PASS — `Tests  3 passed`. (This codifies that the badge logic is independent of `ActivityPrototypePage` before we delete it.)

- [ ] **Step 3: Delete the prototype feed + its test + Landing**

```bash
git rm src/components/activity/ActivityPrototypePage.tsx \
       src/components/activity/ActivityPrototypePage.test.ts \
       src/components/Landing.tsx
```

(`useActivityUnreadCount.ts` and `activity-terminal-portal.ts` stay in `src/components/activity/`.)

- [ ] **Step 4: Build to verify nothing dangles**

Run: `npm run build`
Expected: build succeeds. A failure here means a missed importer — grep `git grep -n "ActivityPrototypePage\|components/Landing"` and fix before continuing (per the verified blast radius, the only importers were `App.tsx`, already rewired in Task 4, and the deleted test).

- [ ] **Step 5: Re-run the regression test (still green after deletion)**

Run: `npx vitest run src/components/activity/useActivityUnreadCount.test.ts`
Expected: PASS — `Tests  3 passed`.

- [ ] **Step 6: Commit**

```bash
git add src/components/activity/useActivityUnreadCount.test.ts
git commit -m "feat(desktop): delete agent-feed prototype + Landing

Mission Control replaced the ActivityPrototypePage feed and Landing.tsx;
both are now unreferenced and removed. Adds a regression test proving the
sidebar unread badge (countActivityUnread) computes from the store alone,
independent of the deleted feed.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] `npm run build` succeeds.
- [ ] `npx vitest run src/lib/preflight-issues.test.ts src/components/mission-control/mission-control-soon-cards.test.ts src/components/activity/useActivityUnreadCount.test.ts` — all green.
- [ ] Manual: cold-start opens Mission Control; stats render (real Claude + Codex data once Phase 1 ships; "no data" placeholders otherwise); OpenCode tab shows "Soon"; the three Soon cards render; Add Project / Create Workspace open their modals; sidebar nav + unread badge intact.
