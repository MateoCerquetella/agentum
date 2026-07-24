# PRD — Desktop Settings Cleanup & Broken-Surface Fixes (spec 007)

> **Autonomous-execution PRD for Ralph mode.** This file is the single
> source of truth. It is self-contained: you do not need to read other
> files to start, though `spec.md` and `architecture.md` in this folder
> have extra rationale. Work top-to-bottom. Check off each task **in this
> file** as you complete it. Keep the build green at every step.

---

## 0. Ralph loop protocol (read every iteration)

Each iteration:
1. Open this file. Find the **first unchecked `[ ]` task** in §6 (Tasks),
   respecting phase order (Phase 1 → 2 → 3 → 4).
2. Do exactly that one task (or the smallest coherent sub-unit if the task
   is large). **Read each target file fresh** before editing — line numbers
   below are from a snapshot of branch `staging` and may have drifted; match
   on real content, not line numbers.
3. **Verify** with the task's verify command(s). The build MUST be green
   before you check the box. If red, fix the dangling refs/type errors your
   change caused until green.
4. Edit this file: change the task's `[ ]` to `[x]` and append a one-line
   note under it (what you did, build status). Update §1 Progress.
5. Commit: `git add -A && git commit` with a message like
   `chore(desktop): 007 <task-id> <summary>`. One commit per task. Branch is
   already `staging` — do NOT create PRs, do NOT push unless told.
6. If a task is **blocked** (ambiguous, needs a decision, or balloons beyond
   its phase), do NOT guess: mark it `[!]`, write a `BLOCKED:` note with the
   specific question/finding, skip to the next unblocked task in the same or
   a later phase, and keep going. Surface all `[!]` at the end.
7. Stop the loop when every Phase 1–3 task is `[x]` or `[!]` (Phase 4 is
   optional — see §6 Phase 4). Then write a final summary.

**Never** leave the tree with a red `npm run build`. **Never** touch the
keep-list in §4.

---

## 1. Progress (update each iteration)

```
Phase 1 (Removals A1–A5 + B1):   5 / 6   done; A4 BLOCKED [!]   (A1, A2, A3, A5, B1 done)
Phase 2 (Palette D1 + Shortcut C4): 3 / 3 done   (D1, C4a, C4b done — Phase 2 COMPLETE)
Phase 3 (Backend fixes C1,C3,C5):   3 / 3 done   (C1, C3, C5 done — Phase 3 COMPLETE; C5 full project table [!] deferred)
Phase 4 (C2 usage — optional):      0 / 1 tasks done   ([!] DEFERRED — own spec; needs human go-ahead)
Blocked [!]: A4 (Remove Pet) — delete-list dangles store/slices/ui.ts pet persistence (~79 lines) + api.pet; ui.ts not in A4 edit-list and is pre-existing-modified. Needs decision: full removal (own spec) vs narrow infra-keep. See A4 note.
Deferred [!]: C5 full ProjectV2 GraphQL table (CLI can't produce the shape) + GitLab projects (GitHub-only). C2 (usage scanners) — Phase 4 optional, own-spec-sized.
Last green build: C5 — cargo build -p agentum-desktop exit 0 (UI unchanged since C3) (2026-06-10)
```

---

## 2. Mission

The agentum desktop app's Settings surface ships sections that are
unfinished, abandoned, or contradict the "self-hosted, zero telemetry"
promise, and several real surfaces are broken. After this PRD:
- Settings shows only shipped, trusted sections (removals done).
- `Cmd+Shift+P` opens a settings command palette; `Cmd+,` (and other
  documented global shortcuts) actually fire.
- The broken native surfaces (Terminal pane, Orchestration skill state,
  GitHub projects/issues; usage stats in Phase 4) work or degrade
  gracefully.

All work is in `crates/agentum-desktop/` — the React/Vite SPA under `ui/`
and the Tauri Rust shell under `src/`. **No** changes to `agentum-server`,
the TUI (`agentum-cli`), or the marketing site.

---

## 3. Environment & build commands

Repo root: `/Users/mateocerquetella/Developer/projects/agentum` · branch `staging`.

```sh
# UI typecheck + build (PRIMARY gate — must be green to check off any task)
npm run build --prefix crates/agentum-desktop/ui
# Faster inner loop (typecheck only) — finish with the real build above
npx tsc --noEmit -p crates/agentum-desktop/ui
# UI tests (run affected specs; vitest)
npm run test --prefix crates/agentum-desktop/ui -- run
# Rust shell build (only after touching crates/agentum-desktop/src/**.rs)
cargo build -p agentum-desktop
```

If `crates/agentum-desktop/ui/node_modules` is missing, run
`npm install --prefix crates/agentum-desktop/ui` once first.

---

## 4. HARD BOUNDARIES — do NOT touch (will break the web client / other surfaces)

- `crates/agentum-desktop/ui/src/runtime/**` — runtime RPC transport.
- `crates/agentum-desktop/ui/src/web/**` — web client.
- `crates/agentum-desktop/ui/src/shared/runtime-*.ts`.
- Tauri commands `runtime_get_status`, `runtime_call`,
  `runtime_sync_window_graph`, and the driver getters in
  `crates/agentum-desktop/src/commands/runtime.rs` — **keep these**. Only
  the *pairing/environment-management* commands get deleted (Task A2).
- `crates/agentum-server/**` — no change in this PRD.
- `crates/agentum-cli/**` (the TUI) — no change.
- `web/` (marketing site) — no change.
- Any Settings pane NOT named for removal — do not redesign. Terminal/Stats
  are *fixed*, not redesigned.

**Single-registry invariant:** `ui/src/hooks/useSettingsNavigationMetadata.ts`
is the ONE source feeding the Settings sidebar, the Cmd+J palette, and the
new Cmd+Shift+P palette. Remove a section there → also remove it from the
`SettingsNavTarget` union (`ui/src/lib/settings-navigation-types.ts`) and the
render switch in `ui/src/components/settings/Settings.tsx`.

---

## 5. Global method

- **Leaf-first removal:** edit consumers (imports, renders, wiring) BEFORE
  deleting the leaf file, so `tsc` never sees a dangling import mid-task.
- **Read fresh, match content**, not line numbers.
- **One task = one commit = one green build.**
- **No new abstractions** (YAGNI). Reuse existing components/helpers.
- All paths below are relative to `crates/agentum-desktop/ui/src/` unless they
  start with `crates/`.

---

## 6. Tasks

### PHASE 1 — Removals (rip out the whole feature, not just the Settings entry)

#### [x] Task A1 — Remove Floating Workspace (floating terminal feature)
> **Done (build green).** Deleted `components/floating-terminal/` (whole dir),
> `FloatingWorkspacePane.tsx`(+test), `floating-workspace-search.ts`,
> `lib/floating-workspace-terminal-actions.ts`(+test), `floating-workspace-shortcut-policy.ts`,
> `lib/floating-terminal.ts`, and the now-orphaned `lib/floating-workspace-tab-creation.ts`
> (sole importer was the deleted actions file). Edited all consumers leaf-first:
> App.tsx (state/refs/effects/keydown guards/render + StatusBar prop),
> Terminal.tsx + useIpcEvents.ts + keyboard-handlers.ts (removed dead
> `isFloatingWorkspacePanelFocused()` guard branches — panel no longer exists so
> the guard was always false), StatusBar.tsx (removed status-bar toggle + props),
> Settings.tsx + useSettingsNavigationMetadata.ts + settings-navigation-types.ts
> (removed the `floating-workspace` section + `SettingsNavTarget` member),
> shared/keybindings.ts(+test) (`floatingTerminal.toggle` gone),
> shared/window-shortcut-policy.ts(+test) (`toggleFloatingTerminal` variant gone),
> tauri/ui.ts + contract.ts + web-preload-api.ts (`onToggleFloatingTerminal` +
> `setFloatingTerminalInputFocused` removed), useIpcEvents.test.ts (mock + stubs +
> floating-focus test updated). Native: removed `ui_set_floating_terminal_input_focused`
> from ui.rs + lib.rs registration (sole TS caller was the deleted panel).
> **Scope kept narrow (infra preserved, not in A1 delete-list):** `FLOATING_TERMINAL_WORKTREE_ID`
> (the synthetic "app workspace" woven through editor/tabs/browser/terminals store slices
> + TerminalPane) stays; the `floatingTerminalEnabled` settings field stays; the app-namespace
> `app_get_floating_terminal_cwd`/markdown pickers stay (live caller: OnboardingInlineCommandTerminal).
> **Decision (KeybindingsFileActions):** the PRD called this a "demo dispatch row" but it is
> actually the "Edit File in Agentum" action that opens keybindings.json into the app workspace.
> Kept the button + open behavior; stripped only the floating-panel toggle mechanics (event
> dispatch, frame ref, `floatingTerminalEnabled` toggling). HUMAN MANUAL-CHECK: confirm the
> opened file still surfaces now that the floating overlay is gone (it opens into the app workspace).
> **Residual (out of A1 scope, harmless, compiles):** the `'floating-workspace'` *feature-interaction*
> id remains in `shared/feature-interactions.ts` + its mirror union in `store/slices/ui.ts` (a
> separate registry from the deleted panel; not a dangling ref). Verify: vite build exit 0,
> `cargo build -p agentum-desktop` exit 0, affected vitest (keybindings/window-shortcut-policy/
> useIpcEvents) 74/74 pass, AC grep shows zero refs to any deleted module/symbol.
Open-state is local React state in `App.tsx` (`floatingTerminalOpen`), not a
store slice.

DELETE:
- `components/floating-terminal/` (entire directory)
- `components/settings/FloatingWorkspacePane.tsx` (+ `FloatingWorkspacePane.test.tsx`)
- `components/settings/floating-workspace-search.ts`
- `lib/floating-workspace-terminal-actions.ts` (+ its `.test.ts`)
- `lib/floating-workspace-shortcut-policy.ts`
- `lib/floating-terminal.ts` (the `TOGGLE_FLOATING_TERMINAL_EVENT` constant)

EDIT (consumers — do these BEFORE the deletes above):
- `shared/keybindings.ts` — remove `floatingTerminal.toggle` from the
  `KeybindingActionId` union and its `KEYBINDING_DEFINITIONS` entry.
- `shared/keybindings.test.ts` — drop `floatingTerminal.toggle` assertions.
- `shared/window-shortcut-policy.ts` (+ `.test.ts`) — remove the
  `toggleFloatingTerminal` action variant and its branches.
- `App.tsx` — remove the import, `floatingTerminalOpen` state,
  `setFloatingTerminalOpenWithFocus`, the toggle/close/event `useEffect`s,
  the `isFloatingWorkspacePanelFocused`/`isFloatingWorkspaceTerminalInputTarget`
  guard blocks, the panel render, and any matching keydown dep-array entries.
- `hooks/useIpcEvents.ts` (+ `.test.ts`) — remove the
  `onToggleFloatingTerminal` import, subscription, dispatch, and all test stubs.
- `components/status-bar/StatusBar.tsx` — remove import + the toggle dispatch.
- `components/settings/KeybindingsFileActions.tsx` — remove import + the demo
  dispatch row that uses the floating-terminal event.
- `components/Terminal.tsx`, `components/terminal-pane/keyboard-handlers.ts` —
  remove any `isFloatingWorkspaceTerminalInputTarget`/floating refs (trace each).
- `tauri/ui.ts` + `tauri/contract.ts` + `web/web-preload-api.ts` — remove the
  `onToggleFloatingTerminal` (`ui-toggle-floating-terminal`) subscription,
  contract entry, and web noop stub.
- `hooks/useSettingsNavigationMetadata.ts` — remove the `floating-workspace`
  section entry + the `FLOATING_WORKSPACE_SEARCH_ENTRIES` import.
- `lib/settings-navigation-types.ts` — remove `'floating-workspace'` from
  `SettingsNavTarget`.
- `components/settings/Settings.tsx` — remove `FloatingWorkspacePane` import +
  its render block.
- Native: grep `crates/agentum-desktop/src/` for `floating_terminal` /
  `ui-toggle-floating-terminal` / `ui_set_floating_terminal_input_focused`;
  remove the command + its `lib.rs` `invoke_handler` registration **iff** no
  caller remains. If a caller remains, leave it and note it.

**AC:** No "Floating Workspace" section in Settings/search/Cmd+J; the floating
panel, its toggle button, and the `Toggle Floating Terminal` shortcut no longer
exist; no dead references.
**Verify:** `npm run build --prefix crates/agentum-desktop/ui` green; if native
touched, `cargo build -p agentum-desktop` green. Grep proves zero matches for
`FloatingTerminal`, `floating-workspace`, `onToggleFloatingTerminal`,
`floatingTerminal.toggle` outside deleted files.

#### [x] Task A2 — Remove Remote Agentum Servers (pairing UI + pairing commands)
> **Done (build green).** Deleted `RuntimeEnvironmentsPane.tsx`,
> `RuntimePairingUrlGenerator.tsx`, `RuntimeAccessGrantList.tsx` (the latter two were
> only imported by the pane), `runtime-environments-search.ts`. Edited:
> Settings.tsx (removed the `servers` SettingsSection + import + the now-unused
> `switchRuntimeEnvironment` local), useSettingsNavigationMetadata.ts (removed the
> `servers` section, the `runtime-environments-search` import, the
> `runtimeEnvironmentsSearchEntry` local, and the now-unused `Server` icon import),
> settings-navigation-types.ts (`SettingsNavTarget` `'servers'` removed),
> useSettingsNavigationMetadata.test.ts (web-metadata assertion flipped to
> `not.toContain('servers')`). Native: removed `runtime_environments_list`,
> `runtime_environments_remove`, `runtime_environments_add_from_pairing_code` from
> runtime.rs + their 3 lib.rs registrations; KEPT `runtime_get_status`,
> `runtime_call`, `runtime_sync_window_graph`, `runtime_environments_call`,
> `runtime_environments_subscribe`, drivers (transport intact).
> **KEPT per PRD "keep if a caller remains":** `switchRuntimeEnvironment` store action
> in store/slices/settings.ts — production callers were all the deleted pairing UI, but
> `settings.test.ts` still exercises it and the `api.runtimeEnvironments` transport layer
> (HARD BOUNDARY) remains, so it stays (now effectively dead in production).
> **Boundary decision (web/**):** removing the 3 native commands left the TS bindings
> `api.runtimeEnvironments.{addFromPairingCode,list,remove}` (in tauri/runtimeEnvironments.ts
> + contract.ts + the web-client stub) without handlers. The PRD A2 edit-list does NOT
> include those files and `web/**` is a HARD BOUNDARY, so I left the TS bindings as dead
> no-callers (they compile — `call()` takes an arbitrary string; the only caller was the
> deleted pane). Added a why-comment in tauri/runtimeEnvironments.ts explaining this. This
> means the AC grep still finds `runtime_environments_add_from_pairing_code` as a dead TS
> *binding* (not a dangling import); removing it would require touching the web boundary.
> **Untouched (pre-existing-modified, not in A2 scope):** `store/slices/ui.ts` still lists
> `'servers'` in its (superset) settings-target union — harmless/type-safe, and that file
> was already modified before this work. Verify: vite build exit 0, cargo build exit 0,
> vitest (useSettingsNavigationMetadata + settings) 13/13 pass.
DELETE:
- `components/settings/RuntimeEnvironmentsPane.tsx`
- `components/settings/RuntimePairingUrlGenerator.tsx`
- `components/settings/RuntimeAccessGrantList.tsx`
- `components/settings/runtime-environments-search.ts`

EDIT:
- `hooks/useSettingsNavigationMetadata.ts` — remove the `servers` section, the
  `runtime-environments-search` import, and the `runtimeEnvironmentsSearchEntry`
  local.
- `lib/settings-navigation-types.ts` — remove `'servers'`.
- `components/settings/Settings.tsx` — remove `RuntimeEnvironmentsPane` import +
  render block.
- `store/slices/settings.ts` — remove `switchRuntimeEnvironment` **only if**
  grep shows no remaining caller (`grep -rn "switchRuntimeEnvironment\|api.runtimeEnvironments" crates/agentum-desktop/ui/src`). If a non-pairing caller remains (e.g. web client), KEEP it and note it.
- Backend: in `crates/agentum-desktop/src/commands/runtime.rs`, delete ONLY
  `runtime_environments_add_from_pairing_code`, `runtime_environments_remove`,
  `runtime_environments_list` (+ their `lib.rs` `invoke_handler` lines). **KEEP**
  `runtime_get_status`, `runtime_call`, `runtime_sync_window_graph`, drivers.
  Before deleting each, confirm its only TS caller was a deleted pairing file.

**AC:** No "Remote Agentum Servers" section; pairing UI gone; web-client runtime
transport intact; `agentum-server` unchanged.
**Verify:** UI build green; `cargo build -p agentum-desktop` green. Grep proves
no dangling `RuntimeEnvironmentsPane`/`runtime_environments_add_from_pairing_code`.

#### [x] Task A3 — Remove Privacy & Telemetry
> **Done (build green, UI-only — no native).** Deleted `PrivacyPane.tsx`,
> `PrivacyPane.test.ts`, `PrivacyDiagnosticsSection.tsx`,
> `PrivacyDiagnosticBundleControls.tsx`, `privacy-search.ts`. Edited:
> Settings.tsx (removed `privacy` SettingsSection + import), useSettingsNavigationMetadata.ts
> (removed `privacy` section + `PRIVACY_PANE_SEARCH_ENTRIES` import + now-unused `Lock` icon
> import), settings-navigation-types.ts (`SettingsNavTarget` `'privacy'` removed).
> **Store-field sweep result:** none to remove. PrivacyPane + the two diagnostics components
> use only local `useState` + shared plumbing — `lib/telemetry` (`getConsentState`/`setOptIn`/
> `PRIVACY_URL`, still used by FirstLaunchBanner), `settings.telemetry.optedIn` (shared with
> FirstLaunchBanner/TelemetryFirstLaunchSurface), and the `api.diagnostics`/`api.ui` namespaces.
> No pane-exclusive Zustand field exists.
> **Out of scope (left intact):** the telemetry opt-in *flow* (FirstLaunchBanner,
> TelemetryFirstLaunchSurface, lib/telemetry, the `settings.telemetry` field) stays — A3 only
> removes the Settings *pane*. The `api.diagnostics` namespace + native diagnostics commands are
> now orphaned (the deleted PrivacyDiagnosticsSection was their only UI caller) but removing them
> is not in A3's delete-list — left as harmless dead API surface.
> **Residual (comments only, out-of-scope files):** stale "see PrivacyPane.tsx" cross-reference
> comments remain in FirstLaunchBanner.tsx, TelemetryFirstLaunchSurface.tsx, lib/telemetry.ts
> (the telemetry-flow files, not in A3's edit-list). The `'privacy'` keyword in
> developer-permissions-search.ts (macOS-TCC) and the `'privacy'` member of the superset
> settings-target union in store/slices/ui.ts (pre-existing-modified) are unrelated. CODE refs
> to PrivacyPane/privacy-search/PrivacyDiagnostics* are ZERO. Verify: vite build exit 0,
> vitest (useSettingsNavigationMetadata) 6/6 pass.
DELETE: `components/settings/PrivacyPane.tsx` (+ `PrivacyPane.test.ts` if
present), `components/settings/PrivacyDiagnostics*.tsx` (grep the prefix),
`components/settings/privacy-search.ts`.
EDIT: `useSettingsNavigationMetadata.ts` (remove `privacy` section +
`PRIVACY_PANE_SEARCH_ENTRIES` import); `settings-navigation-types.ts` (remove
`'privacy'`); `Settings.tsx` (remove `PrivacyPane` import + render). Sweep any
diagnostic-bundle / telemetry-opt-in store field used ONLY by this pane and
remove it (grep before deleting).
**AC:** No "Privacy & Telemetry" section anywhere; no dead refs.
**Verify:** UI build green; grep clean for `PrivacyPane`/`privacy-search`.

#### [!] Task A4 — Remove Pet (Experimental toggle + feature)
> **BLOCKED — needs a decision (task balloons beyond its phase + internally inconsistent).**
> The Pet feature is far more integrated than A4's delete/edit-list assumes. Specifically
> `store/slices/ui.ts` (which A4 does NOT list, and which is currently pre-existing-modified
> with unrelated `hideDefaultBranchWorkspace` WIP) owns the entire pet-overlay *persistence*
> subsystem — ~79 pet lines: `petVisible`/`setPetVisible`, `petId`/`setPetId`,
> `customPets`/`addCustomPet`/`removeCustomPet`, `petSize`/`setPetSize`, plus the
> PersistedUIState hydration/migration. It also:
>   • imports `DEFAULT_PET_ID`/`isBundledPetId` from `components/pet/pet-models` and
>     `revokeCustomPetBlobUrl` from `components/pet/pet-blob-cache` (ui.ts:66-67) — so A4's
>     "DELETE `components/pet/` (entire dir)" would dangle ui.ts;
>   • calls `api.pet.delete(...)` (ui.ts:1344) — so A4's "delete `tauri/pet.ts` + native pet
>     command" would dangle ui.ts.
> `shared/types.ts` likewise declares `CustomPet`, `PET_SIZE_MIN/MAX`, and the PersistedUIState
> `petVisible/petId/customPets/petSize` fields (separate from the `experimentalPet` GlobalSettings
> flag A4 names), and `shared/constants.ts` seeds pet defaults in `getDefaultUIState`.
> **The internal inconsistency:** A4 says remove *only* the `experimentalPet` flag + its default,
> yet also "delete the entire `components/pet/` dir" — but the persistence layer (ui.ts/types.ts/
> constants.ts/migration) depends on that dir and on `api.pet`. There is NO build-green path that
> satisfies the AC ("native pet command gone") without non-trivial surgery on `store/slices/ui.ts`,
> which is (a) outside A4's edit-list and (b) holding the user's uncommitted WIP.
> **Decision needed (pick one):**
>   (a) **Full removal (own spec-sized):** rip out the whole pet persistence subsystem —
>       ui.ts pet slice (~79 lines) + types.ts (`CustomPet`/`PET_SIZE_*`/PersistedUIState pet
>       fields) + constants.ts `getDefaultUIState` pet defaults + migration + pet tests
>       (PetOverlay/pet-overlay-*/pet-agent-state) — alongside the toggle/overlay/segment/native.
>       Also resolve how to land this in the pre-existing-modified ui.ts (the `hideDefaultBranchWorkspace`
>       WIP).
>   (b) **Narrow "disable the surface" (A1-style infra-keep):** remove the `experimentalPet` toggle +
>       overlay render + status-bar segment + native command, but KEEP `components/pet/pet-models.ts`
>       + `pet-blob-cache.ts` and the ui.ts pet persistence as dead infra (like
>       `FLOATING_TERMINAL_WORKTREE_ID`). This does NOT literally "delete the entire dir" and still
>       requires removing one `api.pet.delete` line from ui.ts.
> Neither is "guessable" safely. Skipping to A5 per §0.6; resurfaced in the final summary.
DELETE: `components/pet/` (entire dir),
`components/status-bar/PetStatusSegment.tsx`, `tauri/pet.ts`.
EDIT:
- `components/settings/ExperimentalPane.tsx` — remove the Pet `SearchableSetting`
  block and the `showPet` line.
- `components/settings/experimental-search.ts` — remove the `pet` entry from
  `EXPERIMENTAL_SEARCH_ENTRY` and `EXPERIMENTAL_PANE_SEARCH_ENTRIES`.
- `App.tsx` — remove `shouldRenderPetOverlay` import, the lazy `PetOverlay`,
  `renderPetOverlay`, and its render site.
- `components/status-bar/StatusBar.tsx` — remove import, the `petEnabled`
  selector, and the `{petEnabled && <PetStatusSegment/>}` render.
- `shared/types.ts` + `store/slices/settings.ts` + `shared/constants.ts` —
  remove the `experimentalPet` field and its default seeding.
- `shared/telemetry-events.ts` — remove the pet event id.
- `tauri/contract.ts` + `web/web-preload-api.ts` — remove the pet namespace/
  commands + web stub.
- Native: grep `crates/agentum-desktop/src/` for the pet command; remove it +
  `lib.rs` registration + any bundled pet model assets.
**AC:** Experimental no longer lists Pet; no pet overlay or status-bar segment
can render; native pet command gone.
**Verify:** UI build green; `cargo build -p agentum-desktop` green; grep clean
for `experimentalPet`, `PetOverlay`, `PetStatusSegment`.

#### [x] Task A5 — Remove "Send feedback" and "Docs" from Help menu
> **Done (build green, UI-only).** Deleted `SidebarFeedbackDialog.tsx`. In `SidebarToolbar.tsx`
> removed: the `SidebarFeedbackDialog` import, `DOCS_URL` const + its comment, the now-unused
> `openExternalUrl` helper, the `feedbackOpen` state, the "Send feedback" + "Docs" DropdownMenuItems,
> the `<SidebarFeedbackDialog/>` render, and the now-orphaned `MessageSquareText` + `ExternalLink`
> icon imports. KEPT "Show Onboarding", "Skills", and the admin "Restart Agentum" item (`api.app.restart`
> still wired). No tests referenced the removed items. Verify: vite build exit 0; AC grep
> (`SidebarFeedbackDialog`, `DOCS_URL`) ZERO matches.
DELETE: `components/sidebar/SidebarFeedbackDialog.tsx`.
EDIT: `components/sidebar/SidebarToolbar.tsx` — remove the
`SidebarFeedbackDialog` import, `DOCS_URL` const, `feedbackOpen` state, the
"Send feedback" `DropdownMenuItem`, the "Docs" `DropdownMenuItem`, and the
`<SidebarFeedbackDialog/>` render. KEEP "Show Onboarding", "Skills", and the
admin "Restart Agentum" item.
**AC:** Help menu shows no "Send feedback" / "Docs"; feedback dialog gone.
**Verify:** UI build green; grep clean for `SidebarFeedbackDialog`, `DOCS_URL`.

#### [x] Task B1 — Verify/sweep remaining Experimental toggles
> **Done (audit-only — every remaining toggle confirmed WIRED; zero removals; no code change).**
> Per-toggle verdict (grepped each flag across ui/src for a live consumer that reads the setting
> and changes behavior, excluding the ExperimentalPane toggle / GlobalSettings type / default seed):
>   • **Agents View `experimentalActivity`** — WIRED: `components/sidebar/SidebarNav.tsx:21`
>     (`settings?.experimentalActivity === true` gates the Agents sidebar entry) +
>     `store/slices/ui.ts:982,1025`.
>   • **Terminal attention `experimentalTerminalAttention`** — WIRED:
>     `components/terminal-pane/terminal-pane-attention-subscriptions.ts:15`,
>     `pty-connection.ts:886`, `use-notification-dispatch.ts:234`.
>   • **Compact worktree cards `experimentalCompactWorktreeCards`** — WIRED:
>     `components/sidebar/SidebarWorkspaceOptionsMenu.tsx:123` (compact vs detailed) +
>     `WorktreeCard.tsx:127`.
>   • **Symlinks on worktrees `experimentalWorktreeSymlinks`** — WIRED:
>     `components/settings/RepositoryPane.tsx:57` (gates symlink config UI).
>   • **Smart New Tab menu `experimentalUnifiedNewTabLauncher`** — WIRED:
>     `components/tab-bar/TabBar.tsx:225` (gates the smart new-tab menu).
>   • **HiddenExperimentalGroup** — a single disabled "Placeholder toggle" with NO backing settings
>     flag and NO consumer; intentional reserved slot ("Does nothing today"). Nothing to remove.
> (Pet toggle excluded — owned by the BLOCKED A4.) Result: no provably-dead toggle → no field
> removals → no code change. Build remains green (unchanged since A3; nothing to rebuild). No commit
> (audit produced no code change).
For each remaining toggle in `ExperimentalPane.tsx` (`experimentalActivity`
"Agents View", terminal attention, compact worktree cards, worktree symlinks,
unified new-tab launcher, and any `HiddenExperimentalGroup` toggles): grep the
flag name across `ui/src` to confirm a LIVE consumer exists (a component reads
the setting and changes behavior).
- Toggle has a live consumer → leave it; record "verified wired" in your note.
- Toggle is provably dead (no consumer) → remove it with the same field-removal
  pattern as Pet (pane block + search entry + `GlobalSettings` field + default).
- Unsure / removal looks risky → leave it and list it as `[!]` for manual review.
**AC:** Every remaining Experimental toggle is either confirmed wired or removed;
ambiguous ones surfaced.
**Verify:** UI build green; a written per-toggle verdict in this file's note.

---

### PHASE 2 — Settings command palette + shortcut dispatch

#### [x] Task D1 — Add Cmd+Shift+P settings command palette
> **Done (build green).** Created `components/settings/SettingsCommandPalette.tsx` — reuses the
> proven Cmd+J path: `useSettingsNavigationMetadata()` → `buildCmdJSettingsResults(sections)` for
> the list (so Phase-1-removed sections are auto-excluded via the single registry), rendered with
> `components/ui/command` (`CommandDialog`/`CommandInput`/`CommandList`/`CommandEmpty`/`CommandItem`),
> `shouldFilter={false}` + `rankCmdJMiddleResults` (settings-only) for typed filtering, empty-query
> shows all sections by `order`. Select → `closeModal()` → `openSettingsTarget(target)` →
> `openSettingsPage()` (mirrors WorktreeJumpPalette.handleSelectSettings, incl. the `repo-<id>` →
> repo-pane mapping). Esc closes via `onOpenChange`. Open state = new `activeModal` member
> `'settings-command-palette'` (added to the union in `store/slices/ui.ts` + to `LAZY_MODAL_IDS` in
> `lazy-modal-mount-state.ts`). App.tsx: lazy import + a `settings.commandPalette` onKeyDown branch
> (placed right after `worktree.palette`, so it inherits the editable/terminal-context suppression
> from the guards above) + the lazy mount next to WorktreeJumpPalette.
> `shared/keybindings.ts`: added action `settings.commandPalette` (title "Open Settings Search",
> group "Global", scope `global`, NO `allowInTerminal`, default `platformBindings(['Mod+Shift+P'])`).
> Confirmed `Mod+Shift+P` is otherwise unclaimed (only `Mod+Shift+Plus` exists) — `findKeybindingConflicts`
> stays `[]` (keybindings.test.ts:145/194 still pass). Added a test in
> `useSettingsNavigationMetadata.test.ts` asserting `buildCmdJSettingsResults(buildSettingsNavigationMetadata(...))`
> section ids exclude `floating-workspace`/`servers`/`privacy`. Verify: vite build exit 0; vitest
> (keybindings + window-shortcut-policy + useSettingsNavigationMetadata) 50/50 pass.
> **Manual-check for human:** can't drive the GUI here — please confirm pressing Cmd+Shift+P opens the
> palette, typing filters, Enter/click navigates Settings, and Esc closes.
Reuse the proven Cmd+J path. Reference `components/WorktreeJumpPalette.tsx`
(it already imports `useSettingsNavigationMetadata` + `buildCmdJSettingsResults`
and navigates via `openSettingsTarget(target)` → `openSettingsPage()` →
`closeModal()`).
CREATE `components/settings/SettingsCommandPalette.tsx`:
- `const sections = useSettingsNavigationMetadata()` →
  `buildCmdJSettingsResults(sections)` for the result list (auto-excludes the
  A1–A3 sections removed in Phase 1 — that's the point of reusing the registry).
- Render with `components/ui/command` (`CommandDialog`/`CommandInput`/`CommandList`).
- Open state driven by a new `activeModal` member `'settings-command-palette'`
  (add to the union in `store/slices/ui.ts`).
- Enter/click on a result → `openSettingsTarget(result.target)` →
  `openSettingsPage()` → `closeModal()`. Esc → `closeModal()` via
  `onOpenChange`.
- Mount it next to `WorktreeJumpPalette` in `App.tsx`.
EDIT `shared/keybindings.ts`: add action `settings.commandPalette`, title
"Open Settings Search", group "Settings"/"Global", scope `global` (NO
`allowInTerminal`), default `platformBindings(['Mod+Shift+P'])`. Confirm
`Mod+Shift+P` is otherwise unclaimed (`findKeybindingConflicts` clean).
EDIT `App.tsx onKeyDown`: add a branch — if `settings.commandPalette` matches,
`preventDefault`, toggle `openModal('settings-command-palette')` (mirror how
`worktree.palette` toggles).
**AC:** `Cmd+Shift+P` opens the palette; typing filters settings sections; Enter/
click navigates Settings to that section; Esc closes; it lists NO removed
section; suppressed inside terminal/browser/editable inputs (inherited from the
existing onKeyDown guards).
**Verify:** UI build green; add/extend a small test that
`buildCmdJSettingsResults(useSettingsNavigationMetadata())` excludes
floating-workspace/servers/privacy. `npm run test --prefix crates/agentum-desktop/ui -- run` green for changed specs.

#### [x] Task C4a — Fix shortcut dispatch (Open Settings + sweep)
> **Done (build green, App.tsx only).** Added three renderer onKeyDown branches (placed right
> before the `if (!canRevealRightSidebar) return` early-return, after the `isEditableTarget` guard
> at App.tsx:1128, so they fire on every view but never in editable inputs):
>   • **`app.settings`** (Cmd/Ctrl+,) → `openSettingsPage()` — the named primary fix; replaces the
>     stale "Cmd+N handled via main-process" comment.
>   • **`workspace.create`** (Cmd/Ctrl+N) → mirrors the inert `onOpenNewWorkspace` IPC handler
>     (quiet when `repos.length===0`, no-op while the composer is open) → `openModal('new-workspace-composer', {telemetrySource:'shortcut'})`.
>     The IPC handler stays as the inert mirror.
>   • **`workspace.delete`** (Cmd/Ctrl+Shift+Backspace) → mirrors `onDeleteCurrentWorkspace`
>     (only terminal view, no other modal, has activeWorktreeId) → `runWorktreeDelete(...)` which is
>     itself confirmation-gated, so no silent destruction.
> Imported `runWorktreeDelete` from `components/sidebar/delete-worktree-flow`. The kept IPC
> subscriptions (onOpenSettings/onOpenNewWorkspace/onDeleteCurrentWorkspace) remain untouched.
> **Sweep — deliberately SKIPPED (verified live or out of scope), per "only add ones with no live
> path / do NOT rebuild the policy layer":**
>   • `voice.dictation` — LIVE: DictationController.tsx has its own window keydown listener
>     (keybindingMatchesAction('voice.dictation') @286).
>   • app-level `zoom.in/out/reset` — partial LIVE paths (PdfViewer.tsx zoom when a PDF is focused;
>     terminal zoom via onTerminalZoom). A global handler would double-fire — skipped to avoid that.
>   • `tab.previousRecent` (switchRecentTab) — LIVE in Terminal.tsx onKeyDown.
>   • Cmd/Ctrl+1-9 `jumpToWorktreeIndex` + `jumpToTabIndex` — these are IMPLICIT digit chords detected
>     by the policy (platformPrimaryModifier + digit), not keybinding-action ids. Restoring them means
>     reimplementing the policy's digit-detection = "rebuild the policy layer", which the task forbids.
>     Left dead (documented here for a follow-up if desired).
>   • `file.exportPdf` — its binding is being unassigned by Task C4b; no keydown to add.
>   • `app.forceReload` — force-reloading the webview discards terminal/editor state and isn't a named
>     fix; left out conservatively.
> Already-live (no action needed): worktree.history.back/forward, worktree.palette, worktree.quickOpen,
> view.tasks, sidebar.{left,right,explorer,search,sourceControl,checks,ports}.toggle, settings.commandPalette.
> Verify: vite build exit 0; vitest (keybindings + window-shortcut-policy) 43/43 pass.
> **Manual-check for human:** can't drive the GUI — please confirm Cmd+, opens Settings, Cmd+N opens the
> new-workspace composer, and Cmd+Shift+Backspace prompts to delete the current workspace.
Root cause: `App.tsx onKeyDown` matches ~20 actions but has **no `app.settings`
branch**; `Mod+Comma` is defined in `keybindings.ts` but never dispatched in the
Tauri build (the old Electron main-process path `resolveWindowShortcutAction` is
orphaned — referenced only by its own test).
EDIT `App.tsx onKeyDown`: add (near the other menu-group globals, before any
right-sidebar early-return so it fires on every view):
```ts
if (matchShortcut('app.settings')) {
  e.preventDefault(); notifyTerminalCapture?.('app.settings')
  useAppStore.getState().openSettingsPage(); return
}
```
(Use the same `matchShortcut`/keybindingMatchesAction helper the surrounding
branches use.) Keep the IPC `onOpenSettings` subscription in `useIpcEvents.ts`.
Then SWEEP: for each documented Global chord whose only dispatch was the orphaned
policy, add the missing renderer branch (verify each against the existing handler
list first — only add ones with no live path). Do NOT rebuild the policy layer.
**AC:** `Cmd+,` opens Settings; the other verified Global shortcuts fire.
**Verify:** UI build green; keybinding tests green. (Manual: pressing Cmd+, opens
Settings — note this for the human if you can't drive the GUI.)

#### [x] Task C4b — Resolve the `Mod+Shift+E` collision
> **Done (build green).** Reassigned `file.exportPdf` default from `platformBindings(['Mod+Shift+E'])`
> to `platformBindings([])` in keybindings.ts (now menu-only — still reachable via the
> `onExportPdfRequested` IPC; added a why-comment). `sidebar.explorer.toggle` keeps Mod+Shift+E
> (VS Code parity).
> **Test fixtures updated (both required by the verify):**
>   • `keybindings.test.ts` "reports customized renderer conflicts…" — the Mod+Shift+E conflict
>     `arrayContaining` dropped `'file.exportPdf'`; now `['sidebar.explorer.toggle','worktree.palette']`.
>   • `window-shortcut-policy.test.ts` "routes menu-backed actions…" — removed the now-invalid
>     `Cmd+Shift+E → exportPdf` assertion (file.exportPdf has no chord to resolve from); kept the
>     `forceReload` menu-backed example.
> **Note on `findKeybindingConflicts`:** it only flags a conflict when a *customized* action shares a
> group+binding, so the default (no-override) result was already `[]` even before this change (the
> AC's "zero default overlap on Mod+Shift+E" held via the `conflictGroup: 'menu'` bucket). After the
> reassignment the default stays `[]` and Mod+Shift+E unambiguously belongs to sidebar.explorer.toggle.
> Verify: vite build exit 0; vitest (keybindings + window-shortcut-policy) 43/43 pass.
Two actions default to `Mod+Shift+E`: `file.exportPdf` and
`sidebar.explorer.toggle`. `sidebar.explorer.toggle` KEEPS it (VS Code parity).
`file.exportPdf` → reassign to `platformBindings([])` (unassigned; reachable via
menu). Update any `keybindings.test.ts` fixture that asserts the old conflict.
**AC:** `findKeybindingConflicts(...)` reports zero default overlap on
`Mod+Shift+E`.
**Verify:** UI build green; `npm run test --prefix crates/agentum-desktop/ui -- run` green for `keybindings`/`window-shortcut-policy` specs.

---

### PHASE 3 — Backend fixes (stubbed Tauri commands; Rust work)

> These were CONFIRMED as stubbed native commands left unported during the
> Electron→Tauri migration — the UI is healthy. Before writing a scanner from
> scratch, `git log`-search and grep for any prior implementation to port from
> (old Electron `main`/`dashboard` code in history, `agentum-store`,
> `agentum-core`, the `ai/specs/001-claude-account-usage` spec). Reuse the shape
> the UI already expects.

#### [x] Task C1 — Settings → Terminal pane: stop the render crash
> **Done (build green; native + renderer).** Real root cause (not "unimplemented"): the native
> `pty_management_list_sessions` (pty.rs) returned a **bare `Vec<Value>` array**, but the renderer
> reads `result.sessions` → `undefined` → `setSessions(undefined)` → `sessions.length` (line 219)
> threw during render → blanked the pane (no error boundary). (`PtyManagementSession` is a type-only
> import to a non-existent `preload/api-types`, so it's erased at build — vite builds fine while the
> shape mismatch crashes at runtime.)
> FIX (both, per the PRD's "preferred: implement the minimal real listing" since `state.ptys` is a
> real source):
>   • **Native** `pty_management_list_sessions` → returns the wrapped `{ "sessions": [...] }` shape with
>     the fields the renderer renders: `sessionId`, `cwd`, `isAlive` (derived from the child via
>     `try_wait()` → `Ok(None)` means running, matching the kill_one child-access pattern), plus
>     `shellState`/`state` so the status dot reflects liveness. Return type `Vec<Value>` → `Value`.
>   • **Renderer** `ManageSessionsSection.refresh()` → guards `result?.sessions ?? []` (keeps `sessions`
>     a real array so a stub/older daemon or future drift can't crash the render). The existing
>     try/catch + console.error + "No sessions" empty state stay.
> AC: Terminal settings pane now renders; the sessions table shows live PTYs (green/running) or a clean
> "No sessions" empty state; genuine failures still degrade via the catch (toast), not a crash.
> Verify: cargo build -p agentum-desktop exit 0; vite build exit 0.
> **Manual-check for human:** can't drive the GUI — open Settings → Terminal with ≥1 live terminal and
> confirm the pane renders, lists the session(s), and Kill/Restart work.
Root cause: `components/settings/ManageSessionsSection.tsx` calls
`api.pty.management.listSessions()` and destructures `result.sessions` with no
guard; the native pty-management command appears unimplemented → throw blanks the
pane (no error boundary wraps it).
FIX (smallest safe): (a) guard the call — null-check `result?.sessions ?? []`,
wrap the fetch in try/catch with a console.error + an inline "sessions
unavailable" empty state; (b) check whether the native command exists in
`crates/agentum-desktop/src/commands/` — if it's a stub/missing, EITHER implement
the minimal real listing (preferred if a tmux/pty source exists to port) OR hide
`ManageSessionsSection` when the command is unavailable so the rest of the pane
renders. Do not redesign the pane.
**AC:** Opening Settings → Terminal renders with no console error; shell/renderer/
sessions/behavior controls display and persist; if session listing is genuinely
unavailable, it shows a clean empty state instead of crashing.
**Verify:** UI build green; `cargo build -p agentum-desktop` green if native
touched; note manual-check needed for the human.

#### [x] Task C3 — Orchestration skills: detect installed + refresh after install
> **Done (build green; native + UI; 2 Rust unit tests pass).**
> **(1) Native `skills_discover` ported** (`commands/skills.rs`): scans the global home skills dir
> `~/.claude/skills` (where `npx skills add --global` writes; confirmed it exists on disk, and the
> renderer's `GLOBAL_AGENT_SKILL_SOURCE_KINDS = ['home']`). Each subdir containing a `SKILL.md`
> becomes a `DiscoveredSkill` in the exact `shared/skills.ts` shape — `{ id, name (=dir basename, which
> is what hasInstalledAgentSkill matches on), description (best-effort frontmatter line-scan, no YAML
> dep), providers:['claude'], sourceKind:'home', sourceLabel, rootPath, directoryPath, skillFilePath,
> installed:true, fileCount, updatedAt }` — plus a `sources:[{...,exists}]` entry and `scannedAt`.
> Return type `Vec<Value>` → `Value` (`{ skills, sources, scannedAt }`). Logic extracted into a testable
> `discover_home_skills(root)` core; added 2 unit tests (`discovers_installed_home_skill`,
> `ignores_dirs_without_skill_file_and_missing_roots`) against temp dirs (process-id unique, no
> clock/random — both banned in workspace tests). Used `dirs::home_dir()` (already a desktop dep).
> **(2) Re-probe on install completion:** added an `onExit?` callback to `OnboardingInlineCommandTerminal`
> fired from `TerminalPane.onPtyExit` (the install shell exiting is the best in-app completion signal);
> `AgentSkillSetupPanel` passes `onExit={() => notifyInstalledAgentSkillsChanged()}`, which clears the
> module cache + dispatches the change event so every `useInstalledAgentSkill` re-probes. With the native
> fix, the hook's existing focus + Re-check paths ALSO now return real data (before, they re-probed but
> always saw `[]`), so "Installed" appears without an app restart.
> Verify: cargo test -p agentum-desktop (skills) 2/2 pass + compiles; vite build exit 0.
> **Manual-check for human:** can't drive the GUI — run the orchestration install, then confirm the
> Orchestration surface flips to "Installed" without restarting the app.
Root causes: (1) `crates/agentum-desktop/src/commands/skills.rs`
`skills_discover` returns `{"skills":[]}` ("isn't ported"); (2) the UI never
calls `notifyInstalledAgentSkillsChanged()` after an install completes, so even a
real probe stays stale until window refocus.
FIX:
- Port `skills_discover` in `commands/skills.rs` to actually scan the agent-skill
  source directories (home/global skill dirs the install command writes to) and
  return entries in the shape `hooks/useInstalledAgentSkills.ts` expects
  (`{skills:[{name,source,...}], sources:[...], scannedAt}`). Match
  `hasInstalledAgentSkill`'s matching logic and `GLOBAL_AGENT_SKILL_SOURCE_KINDS`.
- Fire `notifyInstalledAgentSkillsChanged()` when the install terminal completes:
  trace the install-completion path (`components/settings/AgentSkillSetupPanel.tsx`
  / `OrchestrationSetupCard.tsx` / the inline command terminal's close/exit
  handler ~`OnboardingInlineCommandTerminal.tsx`) and call it there so the hook
  re-probes.
**AC:** After installing the orchestration skills, the Orchestration surface shows
"Installed" without a manual app restart (re-probe happens on install completion).
**Verify:** `cargo build -p agentum-desktop` green; UI build green; add a Rust unit
test for `skills_discover` against a temp skills dir if feasible.

#### [x] Task C5 — Git: GitHub projects + issues states  (AC met via the "correct state" branch; full project table [!] DEFERRED)
> **Done — AC satisfied (build green, native only).** Discovery: the renderer is ALREADY fully
> equipped to render the typed states — `ProjectViewWrapper`'s `ErrorState`→`GhAuthErrorHelp`
> (rich `gh auth login`/`gh auth refresh` remediation, env-token detection) for `auth_required`/
> `scope_missing`, `ProjectPicker` likewise, and the issues board already shows the
> `errors.issues` banner from the (working) `gh_list_work_items`. The bug was purely that the
> project-read stubs returned the WRONG SHAPE — `None` / bare `Vec` — instead of the
> `{ ok:false, error: GitHubProjectViewError }` envelope every ProjectV2 result type uses, so the
> renderer fell to a messy generic/empty path (the "silent blank").
> FIX (native `gh.rs`): made the four project-read commands return auth-aware classified envelopes
> (added a `gh auth status` helper): `gh_list_project_views`, `gh_get_project_view_table`,
> `gh_list_accessible_projects`, `gh_resolve_project_ref` now return `{ok:false,error:{type:'auth_required',…}}`
> when `gh` is missing/not-logged-in (→ renderer shows GhAuthErrorHelp) and a clear
> `{type:'unknown', message:"…aren't available in this build yet…"}` when authenticated. Changed
> `fn`→`async fn` and `Vec<Value>`/`Option<Value>`→`Value`. No UI change needed (the existing
> classified-state renderers do the rest). **Issues** already loads real items or shows the
> permission_denied banner — verified the path (`github.ts` consumes `errors.issues`).
> **AC check:** issues load real items OR show a classified banner; projects show the rich
> "Authenticate gh" remediation (unauth) or a clear "not available yet" (authed) — never a silent
> blank. ✓
> **[!] DEFERRED (own follow-up):** the FULL `GetProjectViewTableResult.ok:true` path (loading real
> project rows) needs a ProjectV2 **GraphQL** normalizer in Rust — `gh project item-list --format json`
> can NOT produce the renderer's GraphQL-shaped `GitHubProjectTable` (it doesn't expose field IDs,
> single-select option IDs/colors, iteration metadata, `fieldValuesByFieldId`, `parentIssue`,
> `issueType`, or `position`). This is the "API client" the gh.rs header already says is deferred;
> it's a spec-sized effort. Until then, authenticated users get the clear "not available yet" state.
> **[!] GitLab projects: out of scope** per this task's Note — projects are GitHub-only in current
> code; no `glab` project path exists to implement.
> Verify: cargo build -p agentum-desktop exit 0; UI unchanged since C3 (no UI files touched).
> **Manual-check for human (needs gh):** with an authenticated `gh`, open Git → Projects → confirm it
> shows "not available yet" (not blank); with `gh` logged out, confirm the Authenticate-gh remediation
> appears; confirm Issues still load.
Root causes: `crates/agentum-desktop/src/commands/gh.rs`
`gh_get_project_view_table() -> None` and `gh_list_project_views() -> Vec::new()`
are stubbed → projects always blank. `gh_list_work_items` IS implemented but
auth-gated, with no clear unauthorized/empty state in the UI.
FIX:
- Implement `gh_list_project_views` and `gh_get_project_view_table` in `gh.rs` by
  shelling out to the `gh` CLI (`gh project list` / `gh project item-list ...
  --format json`) and parsing into the shape
  `store/slices/github.ts` + `components/github-project/ProjectViewWrapper.tsx`
  expect. Follow the existing `gh_list_work_items`/`gh_list` (L~236-511) pattern
  for CLI invocation, owner/repo resolution, JSON parse, and error handling.
- Add a clear UI state for unauthenticated / no-`gh`-CLI in the issues + projects
  views (reuse `integrations-pane-status.ts` connection state — show "Connect
  GitHub" / "Authenticate `gh`" instead of a silent blank).
**AC:** GitHub issues and projects load real items, OR show a correct
empty/unauthorized state; never a silent blank.
**Verify:** `cargo build -p agentum-desktop` green; UI build green; manual note for
the human (needs an authenticated `gh`).
**Note:** GitLab projects appear GitHub-only in current code — if no `glab`
project path exists, leave GitLab projects out of scope and note it `[!]`.

---

### PHASE 4 — OPTIONAL / LARGE (defer unless explicitly continued)

#### [!] Task C2 — Usage stats scanners (own-spec-sized; do last, or skip)
> **[!] DEFERRED — own spec (Phase 4 optional; no human go-ahead given).** Per §6 Phase 4, C2 is
> only started "if Phases 1–3 are fully `[x]`/`[!]` and the human said to continue into C2." Phases
> 1–3 are now complete, but no continue instruction was given, so per the task's own rule this is
> marked DEFERRED and the loop stops. Re-porting `stats_get_summary` + the claude/codex/open-code
> usage scanners (reading each provider's local usage logs and aggregating daily/summary) is a
> FEATURE, not cleanup — the architect recommended a separate spec. Resume only on explicit
> human request.
Stubbed: `crates/agentum-desktop/src/commands/stats.rs`
`stats_get_summary()->json!({})`; `claude_usage.rs`, `codex_usage.rs`,
`open_code_usage.rs` all return `enabled:false`/zeroed/empty. Re-porting the
usage scanners (read `~/.claude` etc. usage logs, aggregate daily/summary) is a
FEATURE, not a cleanup — the architect recommends a separate spec.
**Only start this if Phases 1–3 are fully `[x]`/`[!]` and the human said to
continue into C2.** Otherwise mark `[!] DEFERRED: own spec` and stop.
If doing it: port each scanner to read the provider's local usage data and return
the shape `store/slices/{claude,codex,open-code}-usage.ts` +
`components/stats/usage-overview-model.ts` expect; wire `stats_get_summary` to the
agentum store's event/session counts. Verify with `cargo build -p agentum-desktop`
+ a unit test per scanner against fixture dirs.

---

## 7. Definition of done

- Phase 1: no Floating Workspace / Remote Agentum Servers / Privacy & Telemetry /
  Pet sections; no Send feedback / Docs Help items; remaining Experimental toggles
  confirmed-wired or removed. UI build + `cargo build -p agentum-desktop` green.
- Phase 2: `Cmd+Shift+P` opens a working settings palette excluding removed
  sections; `Cmd+,` opens Settings; no `Mod+Shift+E` default conflict. Tests green.
- Phase 3: Terminal pane renders (no crash); Orchestration shows Installed after
  install; GitHub projects/issues load or show a correct state. Rust + UI builds
  green.
- Phase 4: done OR explicitly deferred `[!]`.
- Every change committed on `staging`, one commit per task, every commit building
  green. No PRs/pushes unless the human asks.
- A final summary listing every `[x]` and every `[!]` with its blocker.

## 8. Verify matrix (quick reference)

| Touched | Must run |
|---|---|
| any `ui/src/**` | `npm run build --prefix crates/agentum-desktop/ui` |
| tests changed/affected | `npm run test --prefix crates/agentum-desktop/ui -- run` |
| any `crates/agentum-desktop/src/**.rs` | `cargo build -p agentum-desktop` |
| keybindings/palette | keybinding + window-shortcut-policy vitest specs |

---

## 9. FINAL SUMMARY (loop stopped — every Phase 1–3 task is `[x]` or `[!]`)

Loop terminated per §0.7. Every commit is on `staging`, one task per commit, each building green.
No PRs/pushes (not requested). The 4 pre-existing-modified WIP files
(`worktree-card-compact-agents.tsx`, `shared/constants.ts`, `store/slices/ui.ts`,
`agentum-server/.../worktrees.rs`) were preserved untouched and excluded from every commit
(D1 needed a one-line `ui.ts` union add — landed via a temp-revert so the user's WIP stayed intact).

### Completed `[x]`
| Task | Commit | Verify |
|---|---|---|
| A1 Remove Floating Workspace | `2054438` | vite 0 · cargo 0 · vitest 74/74 |
| A2 Remove Remote Agentum Servers (pairing) | `b82d608` | vite 0 · cargo 0 · vitest 13/13 |
| A3 Remove Privacy & Telemetry | `d7f1b08` | vite 0 · vitest 6/6 |
| A5 Remove Send feedback + Docs (Help menu) | `4654a9f` | vite 0 |
| B1 Verify Experimental toggles (audit) | (no code change) | all 5 wired; no removals |
| D1 Cmd+Shift+P settings command palette | `fdf40a9` | vite 0 · vitest 50/50 |
| C4a Dispatch app.settings + workspace.create/delete | `76367d6` | vite 0 · vitest 43/43 |
| C4b Unassign file.exportPdf (Mod+Shift+E) | `1275d2b` | vite 0 · vitest 43/43 |
| C1 Manage Sessions render crash (pty shape) | `e14a61d` | cargo 0 · vite 0 |
| C3 Discover installed skills + re-probe | `74991c7` | cargo test 2/2 · vite 0 |
| C5 GitHub Projects auth/empty state | `070c0d2` | cargo 0 · UI unchanged |

### Flagged `[!]` (need human decision / out of scope)
- **A4 Remove Pet — BLOCKED.** A4's "delete `components/pet/` dir" + "delete `tauri/pet.ts`/native"
  dangles `store/slices/ui.ts` (which owns ~79 lines of pet *persistence* — `petVisible`/`petId`/
  `customPets`/`petSize` + migration, imports `pet-models`/`pet-blob-cache`, calls `api.pet.delete`),
  a file NOT in A4's edit-list and currently holding the user's WIP. No build-green path to the AC
  without feature-sized surgery on a WIP file. Decision needed: (a) full removal as its own spec, or
  (b) narrow infra-keep (A1-style). See the A4 note.
- **C5 full ProjectV2 table — DEFERRED.** AC met via the "correct state" branch (auth/empty states,
  no silent blank); loading real project ROWS needs a ProjectV2 GraphQL normalizer in Rust (the CLI
  can't produce the shape). Own follow-up spec.
- **C5 GitLab projects — OUT OF SCOPE** (GitHub-only in current code; no `glab` project path).
- **C2 Usage stats scanners — DEFERRED** (Phase 4 optional; own-spec-sized; needs explicit human
  go-ahead, not given).

### Manual checks for the human (GUI couldn't be driven here)
- D1: Cmd+Shift+P opens the palette, filters, navigates, Esc closes.
- C4a: Cmd+, opens Settings; Cmd+N opens the new-workspace composer; Cmd+Shift+Backspace prompts delete.
- C1: Settings → Terminal with ≥1 live terminal lists the session(s); Kill/Restart work.
- C3: run the orchestration install → surface flips to "Installed" without an app restart.
- C5 (needs `gh`): authed → Projects shows "not available yet" (not blank); logged-out → Authenticate-gh
  remediation; Issues still load.
