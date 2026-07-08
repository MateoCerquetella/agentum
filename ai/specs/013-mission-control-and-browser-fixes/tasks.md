# Tasks — Spec 013

Branch: `fix/mission-control-and-browser-fixes` (off `origin/develop` @ `75d03eaa`).

## F1 — Mission Control close redirect ✅ CODE-COMPLETE + GREEN

Redirect `activeView → 'activity'` when the **active** worktree is closed, so
Mission Control renders through the right-sidebar-suppressed `activity` slot
instead of the squeezed `terminal && !activeWorktreeId` fallback.

**Files:**
- `crates/agentum-desktop/ui/src/store/slices/worktree-close-view.ts` (new) —
  pure `viewAfterWorktreeClose(removedActiveWorktree, currentView)`; redirects to
  `'activity'` **only** from the `'terminal'` view (never yanks settings/tasks/…).
- `crates/agentum-desktop/ui/src/store/slices/worktree-close-view.test.ts` (new)
  — 4 cases (redirect / background-close no-op / already-activity / non-terminal
  views untouched).
- `crates/agentum-desktop/ui/src/store/slices/worktrees.ts` — stamped 3 cascade
  return-objects: batch remove (`~:729`), `removeWorktree` (`~:1351`), and the
  central `setActiveWorktree(null)` branch (`~:1980`) — the last covers 5 close
  callers (Terminal.tsx ×2, sleep-worktree-flow, TerminalPaneOverlayLayer,
  terminal-tab-actions, useTabGroupWorkspaceModel).
- `crates/agentum-desktop/ui/src/store/slices/tabs.ts` — stamped the
  `closeUnifiedTab` deactivate branch (`~:720`) — the 4th nulling path (found
  during anchor location; not in the original spec's known set).

**Exhaustiveness:** grepped every `activeWorktreeId: null` production site; all
covered (worktrees.ts batch/removeWorktree/setActiveWorktree + tabs.ts
closeUnifiedTab). `setActiveWorktree(null)` is the central chokepoint for the
component-level close callers. Selecting a worktree restores `activeView:'terminal'`
(`repos.ts:629`), so no sticky-'activity' hazard.

**Gate:**
- `bunx vitest run worktree-close-view.test.ts` → **4/4 pass**.
- `bun run build` (Vite) → **green** (1m45s).
- Regression: `store-session-cascades` + `tabs` + `worktrees` tests → **192/193
  pass**; the 1 failure (`drops browser tabs for invalid worktrees`,
  `webviewClose` on undefined) is **pre-existing** — verified failing identically
  with F1 edits reverted (Tauri-API-in-jsdom baseline).

**Deviation:** the `viewAfterWorktreeClose` helper adds a `currentView === 'terminal'`
guard beyond the architecture's simpler form — prevents yanking users off
settings/tasks/projects when a background process nulls the active worktree.

## F3 — Browser paste — PENDING (next developer iteration)
## F2 — Browser viewport + contain-aware clicks (+ first-frame spike) — PENDING
