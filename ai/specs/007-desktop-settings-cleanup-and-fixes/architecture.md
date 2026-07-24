# Architecture — 007 Desktop settings cleanup & broken-surface fixes

> Status: Architect gate passed. Companion to `spec.md`.
> Two recon corrections + confirmed backend-stub root causes below.

This is an umbrella cleanup. The Architect's job: (a) make the five
removal batches dead-reference-safe, (b) correct recon errors before they
cause wasted work, (c) keep the load-bearing web-client runtime layer out
of the A2 blast radius, and (d) record the **confirmed** root causes for
the C-fixes (they are stubbed Tauri commands, not UI bugs).

---

## 0. Two corrections to the spec's recon (read first)

1. **`Mod+Shift+E` is a *double* collision, not triple.** In
   `crates/agentum-desktop/ui/src/shared/keybindings.ts`: `file.exportPdf`
   → `Mod+Shift+E` (L194) and `sidebar.explorer.toggle` → `Mod+Shift+E`
   (L291). `app.forceReload` is `Mod+Shift+R` (L185), *not* E. C4's fix
   only separates exportPdf from explorer.
2. **A2's "backend pairing routes in `agentum-server`" do not exist.** The
   pairing/remote-runtime backend lives in the Tauri shell at
   `crates/agentum-desktop/src/commands/runtime.rs` and is **already a
   stub**. `agentum-server` needs no change for A2 (no-op there).

---

## CONFIRMED root causes for the C-fixes (parallel investigation)

The "Fix" items are **stubbed native commands** in the Tauri shell
(`crates/agentum-desktop/src/commands/`), unported during the
Electron→Tauri migration. The UI render/data paths are healthy.

- **C2 Stats & Usage** — `commands/stats.rs` `stats_get_summary() ->
  json!({})`. `commands/claude_usage.rs`, `codex_usage.rs`,
  `open_code_usage.rs` all return hardcoded `enabled:false` / zeroed
  summaries / empty daily arrays. Pane renders the "start tracking" empty
  state because every provider reports disabled. **Fix = re-port the usage
  scanners** (read `~/.claude` etc. / agentum store). **Large — split
  candidate (own spec).**
- **C3 Orchestration** — `commands/skills.rs` `skills_discover() ->
  {"skills":[]}` ("Skill discovery isn't ported; report none"). So every
  skill reads as not-installed. Secondary: `useInstalledAgentSkills.ts`
  exports `notifyInstalledAgentSkillsChanged()` but **nothing calls it**
  after install, so even a real probe would stay stale until window
  refocus. **Fix = port `skills_discover` (scan skill dirs) + fire the
  changed-event on install completion.** Medium.
- **C5 Git** — `commands/gh.rs` `gh_get_project_view_table() -> None` and
  `gh_list_project_views() -> Vec::new()` are stubbed → **projects always
  blank**. `gh_list_work_items` (L464-511) **is** implemented → **issues
  work when `gh` CLI is authenticated**; otherwise the UI lacks a clear
  unauthorized/empty state. **Fix = port the gh project commands + add a
  clear auth/empty state for issues.** Medium.
- **C1 Settings→Terminal** — `ManageSessionsSection.tsx` (rendered by
  `TerminalPane.tsx` L417) calls `api.pty.management.listSessions()` and
  destructures `result.sessions` (L195) with no null guard; the native
  command appears unimplemented → render throw blanks the pane (no error
  boundary wraps it; crash reporter is stubbed per the desktop audit).
  **Fix = guard the call + implement/port the pty-management command (or
  hide the section when unavailable).** Shallow-medium.

Implication: **C2/C3/C5 are backend porting work in Rust**, not settings
tweaks. Recommend: do C1 (guard) + C3 + C5(projects) in this spec; **lift
C2 (usage scanners) into its own spec** unless the user wants it inline.

---

## 1. Batch plan

- **Batch 1 — Removals (A1–A5) + B1 dead-toggle sweep.** Leaf-first;
  `tsc` + `vitest` + Vite build green after each feature.
- **Batch 2 — D1 settings palette + C4 shortcut dispatch.** Depends on
  Batch 1 (palette must not surface a removed section).
- **Batch 3 — Backend fixes (C1 guard, C3, C5; C2 deferred to own spec).**

Every removal is **leaf-first**: edit the consumers ("EDIT") before
deleting the leaf ("DELETE") so `tsc` never sees a dangling import.

### Batch 1A — Floating Workspace (A1)

Open-state is **local React state in `App.tsx`** (`floatingTerminalOpen`),
not a store slice — narrows A1's store surface to nothing.

DELETE:
- `ui/src/components/floating-terminal/` (entire dir)
- `ui/src/components/settings/FloatingWorkspacePane.tsx` + `.test.tsx`
- `ui/src/components/settings/floating-workspace-search.ts`
- `ui/src/lib/floating-workspace-terminal-actions.ts` + `.test.ts`
- `ui/src/lib/floating-workspace-shortcut-policy.ts`
- `ui/src/lib/floating-terminal.ts` (the `TOGGLE_FLOATING_TERMINAL_EVENT` const)

EDIT (verify line anchors against current file — read fresh):
- `shared/keybindings.ts` — remove `floatingTerminal.toggle` from the
  `KeybindingActionId` union + its `KEYBINDING_DEFINITIONS` entry.
- `shared/keybindings.test.ts` — drop `floatingTerminal.toggle` assertions.
- `shared/window-shortcut-policy.ts` + `.test.ts` — remove the
  `toggleFloatingTerminal` action variant + its branches.
- `App.tsx` — delete import, `floatingTerminalOpen` state,
  `setFloatingTerminalOpenWithFocus`, the toggle/close/event effects, the
  floating-focus guard blocks, the panel render, and matching keydown
  dep-array entries.
- `hooks/useIpcEvents.ts` + `.test.ts` — remove `onToggleFloatingTerminal`
  import/subscription/dispatch (and all test stubs).
- `components/status-bar/StatusBar.tsx` — remove import + dispatch.
- `components/settings/KeybindingsFileActions.tsx` — remove import + the
  demo dispatch row.
- `components/Terminal.tsx`, `components/terminal-pane/keyboard-handlers.ts`
  — remove any `isFloatingWorkspaceTerminalInputTarget`/floating refs
  (trace each first).
- `tauri/ui.ts` + `tauri/contract.ts` + `web/web-preload-api.ts` — remove
  `onToggleFloatingTerminal` (`ui-toggle-floating-terminal`) subscription,
  contract entry, and web stub.
- `hooks/useSettingsNavigationMetadata.ts` — remove the `floating-workspace`
  section entry + the `FLOATING_WORKSPACE_SEARCH_ENTRIES` import.
- `lib/settings-navigation-types.ts` — remove `'floating-workspace'`.
- `components/settings/Settings.tsx` — remove `FloatingWorkspacePane` import
  + its render block.
- Native: grep `crates/agentum-desktop/src/` for `floating_terminal` /
  `ui-toggle-floating-terminal` / `ui_set_floating_terminal_input_focused`;
  remove the command + `lib.rs` registration if no remaining caller.

### Batch 1B — Remote Agentum Servers (A2) UI

DELETE:
- `ui/src/components/settings/RuntimeEnvironmentsPane.tsx`
- `ui/src/components/settings/RuntimePairingUrlGenerator.tsx`
- `ui/src/components/settings/RuntimeAccessGrantList.tsx`
- `ui/src/components/settings/runtime-environments-search.ts`

EDIT:
- `hooks/useSettingsNavigationMetadata.ts` — remove `servers` section +
  `runtime-environments-search` import + the `runtimeEnvironmentsSearchEntry`
  local.
- `lib/settings-navigation-types.ts` — remove `'servers'`.
- `components/settings/Settings.tsx` — remove `RuntimeEnvironmentsPane`
  import + render block; remove `switchRuntimeEnvironment` wiring **iff**
  unused elsewhere.
- `store/slices/settings.ts` — remove `switchRuntimeEnvironment` **only
  after** confirming no remaining caller (grep `api.runtimeEnvironments` /
  `switchRuntimeEnvironment`).

**Backend (Tauri shell):** in `crates/agentum-desktop/src/commands/runtime.rs`,
delete the pairing/environment-management commands
(`runtime_environments_add_from_pairing_code`, `_remove`, `_list`) +
their `lib.rs` `invoke_handler` registrations. **KEEP** `runtime_get_status`,
`runtime_call`, `runtime_sync_window_graph`, and driver getters — the
web/runtime RPC transport depends on them.

**DO NOT TOUCH** `ui/src/runtime/*`, `ui/src/web/*`,
`shared/runtime-*.ts` — load-bearing for the web client (consumed by
`useComposerState.ts`, `useIssueMetadata.ts`).

### Batch 1C — Privacy & Telemetry (A3)

DELETE: `components/settings/PrivacyPane.tsx` (+ `.test.ts`),
`PrivacyDiagnostics*.tsx` (grep), `privacy-search.ts`.
EDIT: remove `privacy` section + `PRIVACY_PANE_SEARCH_ENTRIES` import in
`useSettingsNavigationMetadata.ts`; remove `'privacy'` from
`settings-navigation-types.ts`; remove `PrivacyPane` import+render in
`Settings.tsx`; sweep any diagnostic-bundle/telemetry store fields used
only by the pane.

### Batch 1D — Pet (A4)

DELETE: `components/pet/` (entire dir),
`components/status-bar/PetStatusSegment.tsx`, `tauri/pet.ts`.
EDIT: remove Pet `SearchableSetting` block + `showPet` in
`ExperimentalPane.tsx`; remove `pet` entry from `experimental-search.ts`;
remove `PetOverlay` lazy/render + `shouldRenderPetOverlay` in `App.tsx`;
remove import + `petEnabled` selector + render in `StatusBar.tsx`; remove
`experimentalPet` from `GlobalSettings` (`shared/types.ts` + settings
slice + `shared/constants.ts` default); remove pet event in
`shared/telemetry-events.ts`; remove pet namespace in `tauri/contract.ts`
+ web stub; native: remove pet command + `lib.rs` registration + bundled
pet model assets.

### Batch 1E — Send feedback & Docs (A5)

DELETE: `components/sidebar/SidebarFeedbackDialog.tsx`.
EDIT: `components/sidebar/SidebarToolbar.tsx` — remove
`SidebarFeedbackDialog` import, `DOCS_URL`, `feedbackOpen` state, the
"Send feedback" + "Docs" menu items, and the dialog render. Keep "Show
Onboarding", "Skills", admin "Restart Agentum".

### Batch 1 — B1 toggle verification

Exercise each remaining `ExperimentalPane.tsx` toggle
(`experimentalActivity`, terminal attention, compact worktree cards,
worktree symlinks, unified new-tab launcher, hidden group). For each,
grep the flag for a live consumer. Dead → remove with the Pet
field-removal pattern; risky → file a follow-up note. Don't redesign.

### Batch 2

CREATE: `components/settings/SettingsCommandPalette.tsx`.
EDIT: `shared/keybindings.ts` (add `settings.commandPalette` = `Mod+Shift+P`;
resolve `Mod+Shift+E`), `App.tsx` (add `app.settings` +
`settings.commandPalette` dispatch branches), `store/slices/ui.ts` (add
`'settings-command-palette'` to `activeModal`), keybinding/policy tests.

---

## 2. D1 — Settings command palette

**Decision: a NEW sibling component `SettingsCommandPalette.tsx`, not a
second mode of the cmd-j palette.** Reuses the cmd-j *data/navigation*
primitives, own thin shell.

Rationale: `WorktreeJumpPalette.tsx` already imports
`useSettingsNavigationMetadata` + `buildCmdJSettingsResults` and navigates
settings via `openSettingsTarget(target)` → `openSettingsPage()` →
`closeModal()` (≈L876-887). The drift-prone logic (results + navigation)
is shared and reused; a ~120-line sibling avoids threading a "settings
mode" through ~900 lines and can't regress Cmd+J. Cost: small palette-chrome
duplication (`components/ui/command`). Accepted.

Design:
- **Data:** `buildCmdJSettingsResults(useSettingsNavigationMetadata())` —
  the single source the sidebar + Cmd+J read, so removed sections (A1–A3)
  are auto-excluded (AC D, bullet 2) for free.
- **Open:** new `settings.commandPalette` = `Mod+Shift+P`, dispatched from
  `App.tsx onKeyDown` → `openModal('settings-command-palette')`;
  toggle-closes if open (mirror `worktree.palette`).
- **Navigate:** Enter/click → `openSettingsTarget` → `openSettingsPage` →
  `closeModal` (exact cmd-j path).
- **Close:** Esc via `CommandDialog onOpenChange` → `closeModal`.
- **Suppression:** define as `global` scope, no `allowInTerminal`. The
  existing `App.tsx onKeyDown` guards (`getKeybindingContext`,
  `isEditableTarget` early-return, terminal-policy gating) suppress it in
  terminal/browser/editable contexts — same as `worktree.palette`. No new
  suppression code.

---

## 3. C4 — Shortcut dispatch (root cause + fix)

**Root cause (traced):** the renderer keydown handler in `App.tsx`
(`onKeyDown`, ≈L1158-1428) matches ~20 actions but has **no branch for
`app.settings`**. The only path calling `openSettingsPage()` from a
shortcut is `hooks/useIpcEvents.ts` `api.ui.onOpenSettings(...)` (fires on
the `ui-open-settings` event). In Electron that event came from a
main-process before-input + `resolveWindowShortcutAction`. In the **Tauri
shell there is no equivalent** — no menu accelerator, no
before-input/`on_window_event`, no `ui-open-settings` emit.
`resolveWindowShortcutAction` (`shared/window-shortcut-policy.ts`) is
referenced **only by its own test** — orphaned in Tauri. So `Cmd+,`
reaches App.tsx's handler, finds no branch, falls through.

**Fix:** add the dispatch in the renderer handler (where every other
global chord already resolves), not by reviving the dead policy:
```ts
if (matchShortcut('app.settings')) {
  e.preventDefault(); notifyTerminalCapture('app.settings')
  useAppStore.getState().openSettingsPage(); return
}
```
Place near the other menu-group globals, before any right-sidebar
early-return so it fires on every view. Add the same for
`settings.commandPalette`. Keep the IPC `onOpenSettings` subscription
(serves a future native menu item). Timeboxed sweep: add missing renderer
branches for other documented-but-undispatched Global chords whose only
path was the orphaned policy (verify each; don't rebuild the policy layer).

**`Mod+Shift+E` collision:** `sidebar.explorer.toggle` **keeps** it (VS
Code-parity muscle memory). `file.exportPdf` **gives it up** → reassign to
`platformBindings([])` (unassigned; low-frequency, reachable via menu).
Update `keybindings.test.ts` fixtures; `findKeybindingConflicts` must
report zero `Mod+Shift+E` overlap after.

---

## 4. A2 backend decision (see §1B)

`agentum-server`: **no change**. Tauri shell `commands/runtime.rs`:
surgical-delete the pairing/environment commands, **keep** the runtime RPC
transport (`runtime_get_status`/`runtime_call`/`runtime_sync_window_graph`
+ drivers) — load-bearing for the web client. Grep
`api.runtimeEnvironments` callers before deleting any command.

---

## 5. Boundaries — what does NOT change

TUI (`agentum-cli`); marketing site (`agentum-www`/`web/`); embedded-server
boot (`serve_embedded_loopback`, `agentum-desktop/src/lib.rs`); the
web-client runtime layer (`ui/src/runtime/*`, `ui/src/web/*`,
`shared/runtime-*.ts`, `runtime_get_status`/`runtime_call`); every Settings
pane not named for removal (Terminal/Stats are *fixed*, not redesigned);
the keybinding parser/matcher/dispatch contract (only entries change); the
single-registry invariant (`useSettingsNavigationMetadata` stays the one
source for sidebar + Cmd+J + Cmd+Shift+P).

---

## 6. Key tradeoffs
1. D1 sibling vs cmd-j second mode → sibling (isolation > tiny chrome dup).
2. C4 fix in renderer vs reviving main-process policy → renderer (policy is
   dead in Tauri; reviving = large/risky).
3. A2 surgical-delete vs nuke-runtime-layer → surgical (transport is the
   web client's lifeline).
4. C2 inline vs own spec → **own spec** (usage scanners are a feature, not a
   cleanup).

## 7. Risks → mitigations
- **Dangling imports (removals)** → leaf-first; build + vitest per feature.
- **Cutting the web-client runtime layer (A2)** → explicit keep-list; grep
  callers before deleting any `runtime_*` command.
- **Shortcut regression (C4/D1)** → changes only in `keybindings.ts` + the
  single `App.tsx onKeyDown`; rely on match/conflict tests; verify
  `findKeybindingConflicts` clean on `Mod+Shift+E`/`Mod+Shift+P`.
- **C-fix balloons** → C2 already split out; C3/C5 timeboxed, promote if
  backend-deep.
- **Hidden render crashes mask verification** → verify done-states against
  `renderer-errors.log`/console (crash reporter is stubbed).

## 8. YAGNI check
No palette framework (reuse builders + one component + one keybinding); no
reviving the dead policy layer; no new backend for A2; no speculative
empty-state framework (reuse existing empty-states); removal not refactor
for dead toggles.

---

## Handoff notes for the Developer
- Batch 1 leaf-first, build-green per feature; read each file fresh (line
  anchors are from current `staging` and may drift).
- A2: do **not** hunt for routes in `agentum-server` — target is
  `agentum-desktop/src/commands/runtime.rs` pairing commands + the desktop
  pairing UI; keep the runtime RPC transport.
- C4: bug is a missing `app.settings` branch in `App.tsx onKeyDown`;
  `Mod+Comma` is defined but never dispatched in Tauri.
- `Mod+Shift+E` is double (exportPdf + explorer); explorer keeps it.
- C2/C3/C5 fixes are **stubbed Rust commands** in
  `agentum-desktop/src/commands/{stats,claude_usage,codex_usage,open_code_usage,skills,gh}.rs`
  — backend porting, not UI work. C2 (usage) → own spec.
