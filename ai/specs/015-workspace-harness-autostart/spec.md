# Spec 015 — Workspace harness autostart

- **Number:** 015   <!-- drafted as 009 pre-merge; renumbered — develop already carries 009–014 -->
- **Status:** Done                <!-- Draft | PM | Architect | In progress | Done -->
- **Surface:** `crates/agentum-desktop/ui`
- **Author:** Claude (sdd-spec direct path, from GitHub issue #301 filed by Mateo)
- **Tracker:** GitHub [#301](https://github.com/MateoCerquetella/agentum/issues/301)
- **Date:** 2026-07-13

## Problem

When a new workspace is created in the desktop app, the flow ends at session
launch. If the chosen working directory already contains a harness spec
(`.agentum-harness/feature_list.json`, or the legacy
`.harness/feature_list.json`), nothing detects it or acts on it — the user must
manually open the Harness view, register the project, and kick off a run. The
"workspace creation → Harness run" hand-off never fires on its own, even when
every ingredient is already on disk.

## Goal

After a new workspace is created, detect a harness spec in its workdir and
offer a one-click, non-blocking "Start Harness run" that registers the project
and kicks off the drive loop.

## Users / personas

An engineer driving the desktop app who creates a workspace (usually a fresh
worktree) on a project that already carries a `.agentum-harness/` backlog —
resuming spec-driven work, or opening a repo a previous SDD loop scaffolded.
The moment: right after the composer closes, when today they would have to
hand-wire the Harness view before the engine does anything.

## Acceptance criteria

1. After a new workspace is created, the UI checks the session's workdir for
   `.agentum-harness/feature_list.json` (falling back to the legacy
   `.harness/feature_list.json`) via the existing `fsListEntries` client
   (`GET /api/fs/entries`); the check runs async and never delays the
   workspace/terminal surface from rendering.
2. When a spec file is found, a dismissible banner renders in the
   just-created workspace's view offering "Start Harness run" — visible
   **whether or not an agent was auto-launched** (the wizard's quick-create
   path auto-launches the chosen agent; the launcher empty-state only mounts
   when no agent was chosen) — and the terminal/launcher beneath it stays
   fully interactive (non-blocking).
3. Accepting the banner calls `POST /api/harness` (register) then
   `POST /api/harness/{id}/run` (drive loop) with no further manual steps; a
   failure of either call surfaces a toast carrying the server's error detail.
4. Dismissing the banner performs no writes: no harness is registered, nothing
   is persisted, and the session proceeds unchanged.
5. If the workdir is already registered with the engine (`GET /api/harness`
   lists it — client fn `listHarnesses()`), or the wizard's "Start gated run"
   toggle was armed for this creation, the banner does not render (PM D3/D6:
   hide, no link).
6. When no spec file is found, the creation flow is unchanged — no UI changes
   and no network calls beyond the single entries check — zero regression
   through `NewWorkspaceComposerModal.tsx` / `WorkspaceAgentLauncher.tsx`.
7. The detection decision is a pure helper with a vitest suite, and
   `npm run build --prefix crates/agentum-desktop/ui` completes without errors.

## Scope & non-goals (YAGNI)

- **In:** local workdirs; the new-workspace creation moment; both harness dir
  spellings; register + run on accept; dedupe against already-registered runs;
  error toast; unit tests for the pure helper.
- **Out:**
  - Remote/SSH worktrees — the engine reads the project dir from the server's
    local filesystem, so a banner there would offer a run that can't work.
  - Auto-running without user confirmation (the banner is an offer, never a
    silent kick-off).
  - Scaffolding a spec when none exists — the composer's existing
    `scaffoldSpec` toggle (spec 004) owns that path.
  - Persisting dismissals (dismiss = this mount only; no new storage).
  - New server routes — everything needed already exists.
  - TUI parity (separate `agentum-tui` repo).

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- `fsListEntries` (`crates/agentum-desktop/ui/src/runtime/server-fs-client.ts:52`)
  → `GET /api/fs/entries` (handler `crates/agentum-server/src/routes/fs.rs:180`,
  route registered `fs.rs:24`) — host-aware dir listing with `hidden: true`;
  the whole detection transport.
- `startHarness(workdir)` (`runtime/harness-client.ts:148`, `POST /api/harness`)
  and `runHarness(id)` (`harness-client.ts:286`, `POST /api/harness/{id}/run`) —
  exactly the register + run pair AC 3 needs. Both are currently
  module-private: **export them**, don't re-implement.
- `listHarnesses()` (`harness-client.ts:276`, `GET /api/harness`; each
  `HarnessStatus` carries `workdir` — `harness-client.ts:78`, server
  `harness.rs:696`) — the dedupe check for AC 5. Also module-private today;
  export it.
- `subscribeHarnessRunErrors` (`harness-client.ts:378`, exported) — spec 008
  F1's bridge surfacing a just-started run's early drive-phase failure; reuse
  after accept.
- Post-create seam: `lib/open-created-workspace.ts` (`planCreatedWorkspaceOpen`;
  launcher fallback only when `agent === null`, `gatedRun` suppresses plain
  deliveries at `:40-46`) — the creation moment the banner keys off.
- `WorkspaceAgentLauncher.tsx` — the no-session empty state (mounted at
  `components/Terminal.tsx:1578`, conditional at `:1576`) — ONE of the two
  post-create surfaces; most quick-creates auto-launch the agent and never
  mount it (its own docstring is outdated on this). Already imports `sonner`'s
  `toast`.
- Server dir constants + fallback semantics: `HARNESS_DIR` /
  `LEGACY_HARNESS_DIR` (`crates/agentum-server/src/harness/types.rs:16,19`) —
  mirror these two names client-side; never invent a third spelling.
- Pure-lib-with-unit-pins pattern: `lib/workspace-goal-step.ts` (spec 008 F3)
  — imitate its shape for the detection helper.

### Build new

- `lib/workspace-harness-detect.ts` — pure: given the workdir's fs entries,
  the registered harness workdirs, and the creation context (local? gated-run
  armed?), return `{ found, harnessDir, offer }`; plus its vitest suite.
- `HarnessSpecBanner` (small dismissible component: offer, accept, dismiss) —
  mounted at the workspace-view level for the just-created worktree so it
  shows regardless of agent auto-launch (exact placement = architect; PM D1).
- `export` keywords on the three existing harness-client functions
  (`startHarness`, `runHarness`, `listHarnesses`) — keep knip clean by
  actually consuming them.

## Risks & invariants

- **`useComposerState.ts` internals are off-limits** — spec 008 F3 held the
  "props only" line and F1's `initialStartGatedRunProp` path must stay intact.
  Detection lives on the post-create workspace surface, not in the composer
  hook.
- **One-shot check, never poll** — a single entries fetch on mount; no
  interval re-checking (same spirit as the push-based-streaming principle).
- **One launch path** — the run starts server-side through the harness routes;
  the UI never spawns agents itself (YOLO translation, `pane_env`, MCP wiring
  stay in `spawn_agent_into_pane`).
- **The gate is sacred** — accept only registers + runs; nothing may skip
  init/verify semantics or pre-mark features.
- **Duplicate drivers** — racing the wizard's "Start gated run" toggle
  (spec 005/013, `POST /api/harness/start-work`) or double-accepting must not
  create two drivers. `POST /{id}/run` already rejects double-run via
  `claim_driver`; AC 5 both hides the offer when the workdir is registered
  AND suppresses it outright when the gated-run toggle was armed for this
  creation (PM D6) — belt and braces, no ordering race.
- **Auto-launch reality** — the wizard's quick-create path auto-launches the
  chosen agent (`CreateWorkspaceWizard.tsx:344` → `openCreatedWorkspace`;
  launcher only mounts when `agent === null` or for gated runs). The banner
  must NOT live solely on `WorkspaceAgentLauncher.tsx` or most creates never
  see it (PM D1: workspace-view-level mount).
- **Stale map:** CLAUDE.md's `HarnessEngine.tsx` no longer exists
  (`components/harness/` now holds `ChatPage.tsx`) — no "view run" link in
  this slice (PM D3).

## Harness wiring (the gate)

- **feature_list.json entries:**
  1. `f1-detect-helper` — `lib/workspace-harness-detect.ts` + vitest suite.
  2. `f2-banner` — `HarnessSpecBanner` + workspace-view mount for the
     just-created worktree, fed by the helper (AC 1, 2, 6).
  3. `f3-register-run` — export `startHarness`/`runHarness`, wire accept →
     register + run + dedupe + error toast (AC 3, 4, 5).
- **`verify.sh` asserts:** `bunx vitest run` on the new suites
  (`workspace-harness-detect.test.ts` + banner tests) and
  `npm run build --prefix crates/agentum-desktop/ui` exits 0.
- **`qa.sh` asserts (browser QA):** create a workspace on a fixture dir
  containing `.agentum-harness/feature_list.json` → banner renders (AC 2);
  accept → `GET /api/harness` shows the run (AC 3); dismiss on a second
  fixture → `GET /api/harness` unchanged (AC 4); fixture without a spec → no
  banner, flow unchanged (AC 6).

## PM decisions (locked 2026-07-13; cheap for Mateo to veto later)

- **D1 — workspace-view mount.** The banner renders at the workspace-view
  level for the just-created worktree, visible whether or not an agent
  auto-launched. Exact component placement is the architect's (options:
  `Terminal.tsx` worktree-scoped strip, or a shared slot both the launcher
  and the tab surface render).
- **D2 — creation-moment trigger only.** Detection fires once per creation
  (keyed off the post-create open path, e.g. the `openCreatedWorkspace` seam),
  NOT on every launcher/workspace mount — no re-offers on relaunch, no fs
  calls on every activation. Issue #301 says "on new workspace creation";
  broader triggers are a future slice.
- **D3 — hide, never link.** Registered workdir ⇒ no banner. No "view run"
  link in this slice (harness view location drifted; keep the slice small).
- **D4 — canonical names.** Client dedupe uses `listHarnesses()` (the real
  fn name; the draft's `getHarnessStatuses` was wrong). Export exactly
  `startHarness`, `runHarness`, `listHarnesses`.
- **D5 — local-only.** No banner for SSH-host workdirs: the wizard can create
  remote worktrees but the engine reads the server-local FS only
  (`routes/harness.rs` `StartRequest` has no host field). Detection short-
  circuits before any fs call for non-local hosts.
- **D6 — gated-run suppression.** A creation with the "Start gated run"
  toggle armed never shows the banner (that path already registers + runs
  via `/api/harness/start-work`).

## Open questions (delegated to the Architect)

1. **Q1 — banner mount mechanics under D1:** where exactly the workspace-view
   strip renders in `Terminal.tsx` (or a small shared component both surfaces
   mount) so it survives the auto-launch path without touching
   `useComposerState` internals.
2. **Q2 — detection hand-off shape:** how the creation context (worktreeId,
   workdir, hostId, gatedRun) travels from the create flow to the banner —
   store slice vs module-level pending-signal (precedent:
   `lib/pending-session-prompt.ts` used by the launcher).
