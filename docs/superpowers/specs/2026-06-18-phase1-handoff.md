# Phase 1 Handoff — Desktop Nav Shell (un-trap the app)

- **Branch:** `feat/desktop-nav-shell` (based on `origin/develop`)
- **Issue:** #48
- **Full design:** `docs/superpowers/specs/2026-06-18-desktop-ux-redesign-design.md` (read it first)
- **Scope:** Phase 1 ONLY. Do not build Phase 2 (Board pipeline) or Phase 3 (Spec→tickets).

---

## ▶ Kickoff prompt (paste this into Claude Code on the VPS)

```
You are continuing a desktop UX redesign for the agentum repo, on branch
feat/desktop-nav-shell (already checked out, based on develop). Tracking issue: #48.

FIRST, read these two files end to end:
  - docs/superpowers/specs/2026-06-18-desktop-ux-redesign-design.md  (the full design)
  - docs/superpowers/specs/2026-06-18-phase1-handoff.md              (this task list + file map)

Then implement PHASE 1 ONLY: the persistent navigation shell that un-traps the app.
Do NOT build Phase 2 (Board/Kanban) or Phase 3 (Spec→tickets). Follow the repo's
CLAUDE.md workflow. The desktop UI is React/Vite under crates/agentum-desktop/ui/src.

Work in small steps, build after each with:
  npm run build --prefix crates/agentum-desktop/ui
Verify the app loads and you can reach every view AND always get back (no dead-ends).

When Phase 1 meets the acceptance criteria in the handoff, open a PR into develop:
  gh pr create --base develop   (body must contain: Closes #48)
Do not promote to staging/main. Ask before any destructive git operation.
```

---

## Mission (Phase 1)

Make the desktop app impossible to get stuck in, and make every screen explain
itself. This is almost pure front-end work in `crates/agentum-desktop/ui/src`.
No backend changes.

## Why (the bug, concretely)

- The sidebar button labeled **"Agents"** sets `activeView = 'activity'`, which
  **hides the whole sidebar** and renders a full-page view with **no back button** →
  the user is trapped (only `Cmd+B` escapes).
- The **labels lie**: "Agents" opens an activity feed; "Chat" opens a feature-spec
  intake. No view has a title or description.

## File map (from code research — verify line numbers, they drift)

| File | What's there | What to do |
| --- | --- | --- |
| `ui/src/store/slices/ui.ts` (~:439) | `activeView: 'terminal' \| 'settings' \| 'tasks' \| 'activity' \| 'skills' \| 'harness'` + open/close actions | Keep the union but treat **`activity` = Mission Control = home**. Clean up the open/close actions so navigating never hides the rail. |
| `ui/src/App.tsx` (~:1015-1019) | `showSidebar = activeView !== 'settings' && !== 'activity' && !== 'skills' && !== 'harness'` | **THE core fix.** Make the left rail render on **every** view. Remove the per-view hiding; the rail is always present. |
| `ui/src/components/sidebar/SidebarNav.tsx` | Rail buttons: Tasks / Agents / Chat / Search | Relabel + reorder to **Home (Mission Control) → Chat → Board(placeholder, Phase 2) → Settings**. Add a short description/tooltip per item. "Agents" → "Home / Mission Control". |
| `ui/src/components/sidebar/index.tsx` (~:88) | Buttons render only when sidebar open | Ensure the rail (at least a slim icon rail) is always visible; expand/collapse is fine, full-hide is not. |
| `ui/src/components/activity/ActivityPrototypePage.tsx` | Activity feed grouped Working/Blocked/Waiting/Done | This becomes **Mission Control (home)**. Add a title + one-line description. As *home* it needs no back button — but the **rail must stay** so it's not a trap. |
| `ui/src/components/harness/ChatPage.tsx` (~:198) | Has `← Back` (`closeHarnessPage()`) | Reuse this back pattern. Extract a shared **drill-in header** (Back + breadcrumb) and apply it to every non-home view. |
| `ui/src/components/TaskPage.tsx` (~:5170) | Has `← Back` (`closeTaskPage()`) | Same — fold into the shared header. |
| `ui/src/components/settings/Settings.tsx` (~:778) | Has `Back` (`closeSettingsPageWithPromptGuard()`) | Same. |
| `ui/src/lib/right-sidebar-visibility.ts`, `ui/src/lib/titlebar-worktree-history-controls.ts` | Visibility + history-arrow logic | Make back/forward + breadcrumb available on all drill-in views, not just terminal/tasks. |

## Suggested build order

1. **Rail always visible** — fix `App.tsx` `showSidebar` so the rail renders on
   every view. Immediately un-traps "Agents". (Smallest change, biggest relief.)
2. **Shared drill-in header** — extract a `<DrillInHeader back= title= breadcrumb= />`
   from the three existing back buttons; render it on every non-home view.
3. **Relabel + reorder the rail** — Home (Mission Control) → Chat → Board → Settings,
   plain labels + descriptions. Wire "Board" as a labeled placeholder for Phase 2
   (empty state: "Coming soon — your Kanban of agent tickets").
4. **Titles + descriptions + empty states** — every view gets a header line that
   says what it is.
5. **⌘K switcher** — a command palette to jump to any view/agent (a basic version is
   fine; check if one already exists before building).

## Acceptance criteria (Phase 1 done = all checked)

- [ ] The left rail is visible on **every** view — no view hides it.
- [ ] No dead-ends: every drill-in screen has **Back + breadcrumb**.
- [ ] **⌘K** opens a switcher from anywhere.
- [ ] Every view shows a **title + one-line description**; rail labels are plain.
- [ ] The old "Agents" trap is gone — that view is now **Mission Control (home)**.
- [ ] `npm run build --prefix crates/agentum-desktop/ui` passes.
- [ ] Manual click-through: you can reach Mission Control, Chat, Board, Settings, and
      any agent's workspace, and **always get back** to home in one click.

## Build / run / verify

```sh
# Build the desktop UI (the real gate — use this, not bare tsc)
npm run build --prefix crates/agentum-desktop/ui

# Iterate with HMR
npm run dev --prefix crates/agentum-desktop/ui

# Run the desktop app (needs cargo + bun on PATH; Vite on :1420)
cargo tauri dev    # from crates/agentum-desktop
```

Verification gotchas (pre-existing, not caused by this work):
- Bare `tsc` can't resolve `shared/*` (Vite-only) — rely on `npm run build`.
- ~7 vitest files fail on `@xterm/addon-ligatures` — pre-existing noise.

## Workflow (per repo CLAUDE.md)

- This is a **dedicated worktree** off `develop` — normal git here is safe (no foreign WIP).
- Commit in focused steps. Open the PR **into `develop`**:
  `gh pr create --base develop` with **`Closes #48`** in the body.
- Do **not** promote to staging/main — that's the maintainer's release step.

## Out of scope (later branches/issues)

- Phase 2 — Board (Kanban) view + card→worktree→agent start. The backend already
  exists (`/api/worktrees/create`, `board.rs:288` auto-spawn); the one gap is making
  card-start create a worktree instead of spawning in the repo dir.
- Phase 3 — Spec→tickets pipeline; Chat/Code/Card tabs in the agent workspace.
