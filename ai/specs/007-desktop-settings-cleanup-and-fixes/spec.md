# Spec: Desktop settings cleanup & broken-surface fixes

## Goal

A user opens the desktop app's Settings, finds only the sections we
actually ship and trust, reaches them by keyboard (Cmd+Shift+P + the
documented shortcuts), and every surface they land on works.

> Scope note: this is an **umbrella spec** (explicitly chosen over a
> 3-way split). It bundles three kinds of work — *remove*, *fix*,
> *add* — that all converge on the desktop Settings / shortcut /
> navigation surface. It intentionally exceeds the usual "fits on one
> screen" PM-gate rule; the Architect should slice it into
> implementation batches (see Notes → Suggested batches).

All work is in `crates/agentum-desktop/ui/` (the React/Vite SPA)
unless a fix's root cause proves otherwise.

---

## User Value

The Settings surface today shows sections that are unfinished,
abandoned, or contradict our "no telemetry, self-hosted" promise
(Privacy & Telemetry, Remote Agentum Servers, Floating Workspace,
Pet, Send feedback). Several real surfaces are also broken (the
Terminal settings pane, Stats & Usage, Orchestration skill install
state, keyboard shortcuts, the GitHub/GitLab issues & projects
browser). The result is an app that feels half-finished and
untrustworthy. After this spec, Settings is honest (only shipped
features remain), navigable by keyboard, and every remaining surface
does what it claims.

---

## Requirements

### A. Remove (rip out the whole feature, not just the Settings entry)

- **A1 — Floating Workspace.** Remove the `floating-workspace`
  Settings section *and* the underlying feature: the floating
  terminal/browser/markdown panel, its toggle button, its keybinding,
  and its store state.
- **A2 — Remote Agentum Servers.** Remove the `servers` Settings
  section, the remote-runtime pairing UI it hosts, **and** the
  backend pairing/remote-runtime routes in `agentum-server` (per
  user: rip the whole feature). Architect to assess whether any route
  is load-bearing for other surfaces before deleting; stub vs. delete
  is the Architect's call.
- **A3 — Privacy & Telemetry.** Remove the `privacy` Settings section
  (and its diagnostic-bundle controls). We do not track anything.
- **A4 — Pet (Experimental).** Remove the `Pet` experimental toggle
  *and* the Pet feature (overlay, status-bar segment, native pet
  command, models/cache).
- **A5 — Send feedback & Docs.** Remove both items from the sidebar
  Help menu (and the feedback dialog they open).

### B. Verify (don't remove)

- **B1 — Remaining Experimental toggles.** For each toggle that stays
  (Agents View, Terminal attention, Compact worktree cards, Symlinks
  on worktrees, Smart New Tab menu, and any hidden-group toggles),
  confirm flipping it actually turns its feature on/off. Remove or
  file a follow-up for any that are dead.

### C. Fix (broken surfaces — root cause TBD, fix to the observable done-state below)

- **C1 — Settings → Terminal pane.** The Terminal settings pane is
  broken; make it render and all its controls function.
- **C2 — Stats & Usage.** The `stats` pane is broken; make it load and
  display usage without error/blank state.
- **C3 — Orchestration skills install state.** After the user installs
  the orchestration skills, the UI keeps saying "not installed"; make
  the installed state reflect reality (re-probe after install).
- **C4 — Keyboard shortcuts.** Documented global shortcuts don't fire
  (e.g. Open Settings = Cmd+,). Make them dispatch. Resolve the
  `Mod+Shift+E` collision between `app.forceReload`,
  `sidebar.explorer.toggle`, and `file.exportPdf`.
- **C5 — Git issues & projects browser.** The GitHub/GitLab issues &
  projects views don't show / don't work; make them load and render.

### D. Add

- **D1 — Settings command palette.** `Cmd+Shift+P` opens a floating
  search bar (same interaction model as the Cmd+J palette) scoped to
  Agentum **settings**: typing filters settings sections, Enter
  navigates Settings to the chosen section.

---

## Acceptance Criteria

### Remove
- [ ] Settings shows **no** "Floating Workspace", "Remote Agentum
      Servers", or "Privacy & Telemetry" section (sidebar, search, and
      Cmd+J/settings-palette results).
- [ ] The floating terminal panel, its toggle button, and its
      `Toggle Floating Terminal` shortcut no longer exist; the app
      builds and runs with no dead references.
- [ ] The remote-runtime pairing UI is gone; switching/pairing remote
      Agentum servers is no longer offered in the desktop UI.
- [ ] Experimental no longer lists **Pet**; no animated pet overlay or
      status-bar pet segment can appear; the native pet command is
      removed.
- [ ] The sidebar Help menu no longer shows **Send feedback** or
      **Docs**; the feedback dialog is removed.

### Verify
- [ ] Each remaining Experimental toggle has been exercised; for each,
      the linked behavior changes when toggled (recorded pass/fail per
      toggle). Dead toggles are removed or have a follow-up spec noted.

### Fix
- [ ] Opening **Settings → Terminal** renders the pane with no console
      error; its controls (shell, renderer, sessions, behavior) display
      and persist changes.
- [ ] Opening **Settings → Stats & Usage** displays usage data (or an
      explicit, correct empty state) with no error/blank crash.
- [ ] After installing the orchestration skills, the Orchestration
      surface shows **Installed** (no manual app restart required).
- [ ] Pressing **Cmd+, (Open Settings)** opens Settings; at least the
      other Global shortcuts verified in C4 fire; no two actions silently
      share `Mod+Shift+E`.
- [ ] The GitHub/GitLab **issues** and **projects** views load and
      render real items (or a correct empty/unauthorized state) instead
      of staying blank.

### Add
- [ ] Pressing **Cmd+Shift+P** opens a floating palette; typing filters
      settings sections; Enter (or click) navigates Settings to that
      section; Esc closes it.
- [ ] The settings palette reuses `useSettingsNavigationMetadata` so its
      list cannot drift from the Settings sidebar, and it does **not**
      list any removed section (A1–A3).

---

## Dependencies

- None (no prior spec blocks this). Specs 002–006 (sidebar/host work)
  are unrelated surfaces.

---

## Risks

- **Umbrella scope.** Larger than a normal spec; the five "Fix" items
  (C1–C5) have unknown root cause and could each balloon. Mitigation:
  Architect slices into batches; each Fix gets a timeboxed
  investigation before committing to an approach. If any Fix proves
  deep, split it into its own spec rather than stalling the cleanup.
- **Feature removal blast radius.** "Rip out the whole feature" for
  Floating Workspace and Servers touches components, store slices,
  keybindings, native commands, and tests. Risk of dangling imports /
  broken builds. Mitigation: remove leaf-first, lean on `tsc`/Vite
  build + existing tests (e.g. `FloatingWorkspacePane.test.tsx`,
  `PrivacyPane.test.ts`) to catch references; delete or update those
  tests in the same change.
- **Servers backend coupling.** A2 now removes the backend
  pairing/remote-runtime routes too. Risk: a route may be shared with
  SSH-host or connection-profile flows. Mitigation: Architect traces
  callers before deleting; if a route is load-bearing, stub/guard it
  instead of deleting and note it.
- **Shortcut dispatch is shared.** `shared/keybindings.ts` feeds main,
  renderer, browser guests, and Settings. A dispatch fix (C4) or a new
  binding (D1) must not regress terminal/browser scoping. Mitigation:
  keep changes in the central registry + its dispatch hook; rely on the
  keybinding match tests.
- **Cmd+Shift+P availability.** Must not collide with an existing
  binding or a native menu accelerator, and should be suppressed inside
  terminal/browser input contexts like the other global chords.

---

## Notes

### Known anchors (from codebase reconnaissance)

- **Settings/Cmd+J registry (single source of truth):**
  `ui/src/hooks/useSettingsNavigationMetadata.ts` — section entries for
  `floating-workspace`, `servers`, `privacy` live here; removing them
  here drops them from both the sidebar and Cmd+J. `SettingsNavTarget`
  union: `ui/src/lib/settings-navigation-types.ts`.
- **Panes:** `FloatingWorkspacePane.tsx`, `RuntimeEnvironmentsPane.tsx`
  (+ `RuntimePairingUrlGenerator.tsx`, `RuntimeAccessGrantList.tsx`,
  `runtime-environments-search.ts`), `PrivacyPane.tsx`
  (+ `PrivacyDiagnostics*.tsx`, `privacy-search.ts`),
  `ExperimentalPane.tsx` (Pet block ~L42–79; toggle
  `settings.experimentalPet`).
- **Floating terminal feature:** `ui/src/components/floating-terminal/*`,
  `FloatingTerminalToggleButton.tsx`,
  `lib/floating-workspace-terminal-actions.ts`,
  `components/settings/floating-workspace-search.ts`; keybinding
  `floatingTerminal.toggle` (`Mod+Alt+A`) in `shared/keybindings.ts`.
- **Pet feature:** `components/pet/*`, `components/status-bar/PetStatusSegment.tsx`,
  `tauri/pet.ts`.
- **Help menu (feedback/docs):** `components/sidebar/SidebarToolbar.tsx`
  (`Send feedback` → `SidebarFeedbackDialog`; `Docs` → `DOCS_URL`).
- **Keybindings:** `shared/keybindings.ts` — `app.settings` =
  `Mod+Comma` (conflictGroup `menu`); `settings.search` = `Mod+F`;
  `worktree.palette` = Cmd+J. `Mod+Shift+E` is triple-claimed
  (`app.forceReload`, `sidebar.explorer.toggle`, `file.exportPdf`).
- **Open-settings wiring:** `store/slices/ui.ts`
  (`openSettingsPage` / `setSettingsNavigationTarget`), `tauri/ui.ts`
  (`onOpenSettings`), `hooks/useIpcEvents.ts`.
- **Stats:** `components/stats/StatsPane.tsx` (+ usage panes).
- **Orchestration:** `components/settings/OrchestrationPane.tsx`,
  `OrchestrationSetupCard.tsx`, `AgentSkillSetupPanel.tsx`,
  `shared/agent-feature-install-commands.ts`,
  `shared/agents-orchestration-steps.ts`.
- **Git issues/projects:** `components/github/`, `components/gitlab/`,
  `components/github-project/`.

### Suggested implementation batches (for the Architect)
1. **Removals (A1–A5)** — lowest risk, do first; one batch per feature,
   delete-and-build-green.
2. **Settings palette (D1)** — depends on removals landing so it can't
   surface a dead section.
3. **Shortcut dispatch (C4)** — pairs naturally with D1 (same registry).
4. **Surface fixes (C1, C2, C3, C5)** — independent; each is a
   timeboxed investigate-then-fix slice; promote any deep one to its
   own spec.
5. **Experimental verification (B1)** — manual pass; fold removals of
   dead toggles into batch 1's pattern.

### Out of scope
- The TUI (`agentum terminal`) — this spec is desktop-only (except the
  shared `agentum-server` route removal for A2).
- The marketing site (`agentum-www`).
- Redesigning any pane that already works (Terminal/Stats fixes are
  "make it work", not "redesign").
