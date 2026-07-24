# Architecture — Spec 015: Workspace harness autostart

- **Spec:** `ai/specs/015-workspace-harness-autostart/spec.md` (Status: PM → Architect; D1–D6 locked)
- **Author:** Architect (SDD role loop), 2026-07-13
- **Grounding:** every `path:line` verified on this worktree (branch synced to `origin/develop`, HEAD `e7380875`). Paths are relative to `crates/agentum-desktop/ui/src/` unless prefixed with `crates/`.

## 0. Shape of the change

UI-only, one surface, zero server changes:

```
openCreatedWorkspace()  ──►  maybeOfferWorkspaceHarnessRun()   [NEW lib runner, fire-and-forget]
 (lib/open-created-           │ gate: gatedRun? remote? (pure)
  workspace.ts — BOTH         │ fsListEntries .agentum-harness → fallback .harness   (async)
  create paths converge       │ listHarnesses() dedupe (pure decision)
  here; useComposerState      ▼
  is NOT touched)         store slice harnessOfferByWorktreeId  [NEW, tracker-phase pattern]
                              │ (reactive)
                              ▼
Terminal.tsx root strip ──► HarnessSpecBanner (worktree-gated, dismissible)
                              │ accept
                              ▼
                    startHarness(workdir) → runHarness(id) → subscribeHarnessRunErrors(id)
                    (existing harness-client fns, exported)
```

Three features: **f1** pure detect helper, **f2** banner + mount + signal, **f3** register/run wiring.

---

## 1. Decision Q1 — banner mount: **single mount, Terminal.tsx root strip (flex child, `relative z-30`)**

### The component tree, as it actually is

`Terminal.tsx`'s return (`:1565`) is one worktree-agnostic root container
(`relative flex flex-col flex-1 … overflow-hidden`, hidden only when no
`activeWorktreeId`). Inside it, three sibling surfaces:

1. **Launcher empty-state** (`:1576-1580`): `absolute inset-0 z-20` overlay
   mounting `WorkspaceAgentLauncher` when `activeView === 'terminal' &&
   activeWorktreeId && activeWorktreeHasNoSurface` (`:272-276` — no tabs, no
   browser tabs, no files).
2. **Split-group surfaces** (`:1630-1664`): the REAL path — every active
   worktree gets a root group (`ensureWorktreeRootGroup`, `:250-258`), and
   `WorktreeSplitSurface` (`:1921`) renders `absolute inset-0` inside a nested
   `relative flex flex-1` anchor (`:1631`).
3. **Legacy no-layout fallback** (`:1666-1823`): only when NO mounted worktree
   has a layout — in practice near-dead (see the #313 lesson below).

So yes — **there is a worktree-scoped container both paths share: the root
container itself.** Both the launcher overlay and the split surfaces are
absolutely positioned against it; a normal flex child inserted before them is
visible in every path and shrinks the `flex-1` surface anchors instead of
occluding them.

### Decision

Mount ONCE at the root, immediately after `<EditorAutosaveController />`
(`:1569`), before the launcher conditional (`:1571`):

```tsx
{activeView === 'terminal' && activeWorktreeId ? (
  <HarnessSpecBanner worktreeId={activeWorktreeId} />
) : null}
```

`HarnessSpecBanner` renders `null` unless the offer slice has an entry for
exactly this `worktreeId` (so switching worktrees hides/re-shows it correctly,
and it never renders for a workspace that wasn't just created). When it
renders, its wrapper is a **flex strip**: `relative z-30 shrink-0` + border-b.
`relative z-30` is load-bearing: the launcher overlay is `absolute inset-0
z-20` over the whole root box, so an unpositioned strip would paint UNDER it in
the empty-state path; `z-30` puts the strip above the overlay while the
launcher's self-centered card (`WorkspaceAgentLauncher.tsx:103`) stays clear of
the top strip and fully interactive.

### Tradeoffs weighed

| Option | Verdict |
|---|---|
| **Root flex strip, single mount (CHOSEN)** | One mount covers launcher path, split-surface path, AND legacy path. Non-occluding: surfaces shrink via flex (`flex-1` anchors at `:1631`, `:1687`), so the terminal beneath stays fully interactive (AC 2). Renders `null` → zero layout change when no offer (AC 6). |
| Dual mount à la `CodexRestartChip` (`:1720` legacy + `:1962` split surface) | The existing per-worktree-chip precedent, but it does NOT cover the launcher path (the chip floats over panes; the launcher overlay would hide a third mount's need anyway), needs a third mount for the launcher, and inside `WorktreeSplitSurface` children stack `absolute inset-0` — a strip there would occlude the top terminal rows. Three mounts, worse layering. |
| Launcher-only mount | Rejected by PM D1 — quick-create auto-launches (`useComposerState.ts:2533/2750` → `launchAgentInNewTab`), so the launcher never mounts for most creates. |
| Legacy-branch mount (`:1666` block) | **The #313 trap.** `TabGroupPanel.sdd-bar.test.tsx:1-6` documents that the SDD bar shipped invisible for two releases because it lived only in the legacy fallback, which never renders once a worktree has a root group (always, per `:257`). Never mount a new surface only there. |

Constraints honored: `useComposerState` internals untouched (the mount is in
`Terminal.tsx`); pane/terminal rendering is never delayed — the banner is
render-`null` until the **async** detection resolves into the slice, and
detection itself is fired post-create, off the render path entirely (§2).

---

## 2. Decision Q2 — creation-context hand-off: **zustand slice for the offer, triggered from `openCreatedWorkspace` (zero `useComposerState` edits)**

### Where the creation moment actually is

Both create paths converge on `openCreatedWorkspace`
(`lib/open-created-workspace.ts:72`): the full composer submit calls it at
`useComposerState.ts:2533`, the wizard quick-create (`CreateWorkspaceWizard.tsx:343
→ submitQuick`) at `useComposerState.ts:2750`. It already receives
`{ worktreeId, gatedRun }` — and `workdir`/`connectionId` are resolvable from
the store by `worktreeId` (the `WorkspaceAgentLauncher.tsx:35-45` pattern:
`worktreesByRepo` → `worktree.path`/`repoId` → `repos[].connectionId`). So the
trigger is **one additive fire-and-forget line at the end of
`openCreatedWorkspace`** — a lib file, not the composer hook. The anticipated
"1-2 line call-site addition inside useComposerState" is **not needed at all**;
the 008 "props only" invariant is honored by construction.

```ts
// open-created-workspace.ts, end of openCreatedWorkspace():
void maybeOfferWorkspaceHarnessRun({ worktreeId, gatedRun: gatedRun === true })
```

(The quick-create "Don't start a session" path returns before
`openCreatedWorkspace` (`useComposerState.ts:2732-2740`) → no offer. Accepted
residual, consistent with D2's creation-*open* trigger.)

### Both candidates, investigated

| | (a) module pending-signal (`lib/pending-session-prompt.ts`) | (b) zustand slice (CHOSEN) |
|---|---|---|
| Consumption model | **Read-once at an imperative event**: the launcher calls `takePendingSessionPrompt` inside a click handler (`WorkspaceAgentLauncher.tsx:83`). Its own docstring: "never rendered reactively" — the Map works *because* nothing needs to re-render when it's set. | The banner must **appear spontaneously** when async detection resolves — a render-time signal. A zustand selector re-renders the host exactly then. |
| Making (a) reactive | Requires hand-rolling a subscriber list on the module Map — reinventing zustand without devtools/test ergonomics. | Free. |
| Precedent | Fits prompts (event-consumed). | `store/slices/tracker-phase.ts` (spec 014, freshest slice): tiny record-keyed slice, no-op-on-equal writes, registered at `store/types.ts:29,59` + `store/index.ts:31,64`. `store/slices/worktree-close-view.ts` turned out to be a pure fn, not a stateful slice — tracker-phase is the real template. |
| Lifecycle | Stale Map entries are harmless only because they're read-once. | Offer must survive banner unmount/remount (switch away and back before dismissing — still the same creation, D2-legal) and be clearable from accept/dismiss. Slice does this naturally; store has **no persist middleware** (`store/index.ts` — plain `create`), so nothing outlives the app session (spec: no new storage). |

**Division of labor:** the *runner* (`lib/workspace-harness-offer.ts`, new)
does the async work and writes the slice **only on a positive offer**; the
slice holds the RESOLVED offer, not a raw "pending" flag — the banner component
stays dumb (select → render → accept/dismiss). Signal shape:

```ts
// store/slices/workspace-harness-offer.ts
export type WorkspaceHarnessOfferSlice = {
  harnessOfferByWorktreeId: Record<string, WorkspaceHarnessOffer> // lib type, §3
  setWorkspaceHarnessOffer: (offer: WorkspaceHarnessOffer) => void
  clearWorkspaceHarnessOffer: (worktreeId: string) => void // dismiss/accept/stale-purge
}
```

`hostId`/`connectionId` never travels — it's a *gate input* resolved inside the
runner at detection time (D5 short-circuits before any fs call), not banner
state. `gatedRun` travels only from `openCreatedWorkspace` into the runner (D6
suppresses before any fs call); it never reaches the slice.

---

## 3. f1 — `lib/workspace-harness-detect.ts` (pure, IO-free)

Model: `lib/workspace-goal-step.ts` (pure transforms, header comment, no
runtime imports; type-only import of `FsFileEntry` from
`runtime/server-fs-client.ts:32` is allowed — erased at build).

```ts
export const HARNESS_DIR = '.agentum-harness'        // mirrors harness/types.rs:16
export const LEGACY_HARNESS_DIR = '.harness'         // mirrors harness/types.rs:19
export const FEATURE_LIST_FILE = 'feature_list.json'
export type HarnessDirName = typeof HARNESS_DIR | typeof LEGACY_HARNESS_DIR

/** Mirror of the server's expand_workdir normalization (routes/util.rs:24-42):
 *  trim; strip trailing '/' unless the whole path is '/'. No canonicalization —
 *  neither does the server. */
export function normalizeWorkdir(path: string): string

/** Pre-fs gate: D6 (gatedRun ⇒ false) and D5 (connectionId string = SSH ⇒
 *  false; undefined = worktree/repo not found ⇒ false; null = local ⇒ true). */
export function shouldDetectHarnessSpec(ctx: {
  gatedRun: boolean
  connectionId: string | null | undefined
}): boolean

/** entries.some(name === FEATURE_LIST_FILE && kind === 'file') — fs.rs follows
 *  symlinks for kind (:236-247), so a symlinked spec file still counts. */
export function hasFeatureList(entries: FsFileEntry[]): boolean

export type HarnessSpecDetection =
  | { found: false }
  | { found: true; harnessDir: HarnessDirName }

/** Fold the two listings (null = dir missing/unlistable). CRITICAL semantics:
 *  mirrors resolve_harness_dir (harness/types.rs:25-35) — if the CANONICAL dir
 *  exists (listing succeeded), decide from it ALONE; the legacy dir is only
 *  consulted when canonical is absent. Otherwise the banner could offer a
 *  legacy run that the engine (which prefers an existing .agentum-harness/)
 *  cannot load. */
export function detectHarnessSpec(
  canonicalEntries: FsFileEntry[] | null,
  legacyEntries: FsFileEntry[] | null
): HarnessSpecDetection

export type WorkspaceHarnessOffer = {
  worktreeId: string
  workdir: string          // worktree.path, un-normalized (what we POST)
  harnessDir: HarnessDirName
}

/** AC 5 dedupe + final offer. registeredWorkdirs = HarnessStatus.workdir
 *  values (harness-client.ts:78; server serializes the expand_workdir'd
 *  PathBuf, harness.rs:433-438 — absolute, no trailing slash). Compare
 *  normalizeWorkdir(workdir) against normalizeWorkdir(each). */
export function decideHarnessOffer(input: {
  detection: HarnessSpecDetection
  worktreeId: string
  workdir: string
  registeredWorkdirs: string[]
}): WorkspaceHarnessOffer | null
```

**Path-normalization ground truth:** `POST /api/harness` runs `expand_workdir`
(`routes/harness.rs:143` → `routes/util.rs:19`): trim + trailing-slash strip +
`~` expansion, **no** symlink canonicalization. `GET /api/harness` returns that
stored path verbatim. The UI's `worktree.path` is absolute (store fixtures:
`open-created-workspace.test.ts:19`). So client-side `normalizeWorkdir` =
trim + trailing-slash strip is an exact mirror; `~` never appears in
`worktree.path` and comes back pre-expanded from the server. Symlink-diverging
spellings (e.g. `/home` vs `/var/home`) are an accepted residual — same
exposure `find_by_workdir` (`harness.rs:83`) already has.

### The runner — `lib/workspace-harness-offer.ts` (thin IO shell)

```ts
export async function maybeOfferWorkspaceHarnessRun(opts: {
  worktreeId: string
  gatedRun: boolean
}): Promise<void>
```

1. **Stale purge first**: `clearWorkspaceHarnessOffer(opts.worktreeId)` —
   worktree ids are `${repoId}::${path}` (`shared/types.ts:227`), so a
   close-then-recreate at the same path reuses the id; without the purge a
   pre-close offer could leak into a new gated creation (D6 violation).
2. Resolve `worktree`/`connectionId` from `useAppStore.getState()`
   (launcher pattern, `WorkspaceAgentLauncher.tsx:35-45`); gate via
   `shouldDetectHarnessSpec` → return silently.
3. `const canonical = await listOrNull(join(workdir, HARNESS_DIR))` where
   `listOrNull` wraps `fsListEntries(dir, { hidden: true })` (`server-fs-client.ts:52`)
   and **catches → null**. Decision: catch-and-treat-as-absent — a missing dir
   is `fs::metadata` failure → `ApiError::BadRequest("path error: …")`
   (`crates/agentum-server/src/routes/fs.rs:209-211`) → `getJson` throws
   (`server-http.ts:21-23`); distinguishing "missing" from "server down" doesn't
   change the v1 outcome (no banner, fail-closed, AC 6). No `hostId` is ever
   passed — D5 guaranteed local by step 2.
4. Only if canonical listing **failed** (dir absent): fetch legacy. Then
   `detectHarnessSpec(...)`; `found: false` → return (AC 6: ≤2 fs calls, no
   other network).
5. `const registered = await listHarnesses()` (only on the found path),
   `decideHarnessOffer(...)` → null ⇒ return.
6. **Close-race re-check**: worktree still in the store? No ⇒ drop the signal.
   Yes ⇒ `setWorkspaceHarnessOffer(offer)`.

Everything is wrapped in one `try/catch` that swallows (fire-and-forget; a
failure means no banner, never a broken create flow).

---

## 4. f2 — `HarnessSpecBanner` component

**Placement: `components/HarnessSpecBanner.tsx`** — beside its consumer
(`Terminal.tsx`) and its sibling surface `WorkspaceAgentLauncher.tsx`, matching
the "new files beside their consumers" convention. Not `components/harness/`
(that's the Chat page now — stale-map note in spec) and not
`components/new-workspace/` (composer-modal models; the banner outlives the
composer).

Two exports in one file:

- **`HarnessSpecBannerView`** (named; pure presentational): props
  `{ harnessDir: string; busy: boolean; onAccept: () => void; onDismiss: () => void }`.
  Strip layout (`relative z-30 shrink-0`, `border-b bg-card px-3 py-2`, small
  text + primary "Start Harness run" button + ✕ dismiss); `FirstLaunchBanner.tsx`
  is the copy/`inFlight` styling precedent (but ours is a flex strip, not
  `fixed`). Both buttons disabled while `busy` (double-accept guard, D6
  belt-and-braces alongside server `claim_driver`).
- **`HarnessSpecBanner`** (default; store host): props `{ worktreeId }`;
  selector `useAppStore((s) => s.harnessOfferByWorktreeId[worktreeId])` →
  `null` when absent. Dismiss = `clearWorkspaceHarnessOffer(worktreeId)` —
  local state + consume-once, **no writes anywhere** (AC 4). Accept = f3.

Render condition, end to end: offer exists for the active worktree AND
`activeView === 'terminal'` (gated at the mount, §1). Dismissal is
mount-session memory via slice removal; only the runner ever re-sets it, and
only on a creation (D2 — no re-offers on relaunch/activation).

---

## 5. f3 — register + run wiring

- **Exports** (`runtime/harness-client.ts`): add `export` to `startHarness`
  (`:148`), `listHarnesses` (`:276`), `runHarness` (`:286`). Names locked by D4.
  knip stays clean because all three gain consumers (runner + accept flow).
- **Accept flow** — `acceptHarnessOffer(offer)` lives in
  `lib/workspace-harness-offer.ts` (exported, unit-testable; the component
  calls it and owns only `busy` state):
  1. `const { harness_id } = await startHarness(offer.workdir)`
  2. `await runHarness(harness_id)`
  3. `toast.success('Harness run started')`;
     `clearWorkspaceHarnessOffer(offer.worktreeId)`
  4. `void subscribeHarnessRunErrors(harness_id, (m) => toast.error(\`Harness run failed: ${m}\`))`
  5. On throw: `toast.error` with `error.message` — the `request()` helper
     (`harness-client.ts:129-145`) already appends the server's response text
     (`— {detail}`), satisfying AC 3's "server error detail" with zero parsing.
     Banner stays mounted (retryable), `busy` resets.
- **`subscribeHarnessRunErrors` — INCLUDED in v1** (not YAGNI): signature
  confirmed id-scoped with a bounded self-closing window
  (`harness-client.ts:378-403`), and `runHarness` returns before the drive loop
  does anything (bg task), so the most common failure class — a red `init.sh`
  seconds later — would otherwise vanish. The gated-run path already does
  exactly this (`useComposerState.ts:2328-2330`); it costs one line and reuses
  exported plumbing.
- **Duplicate-driver belt-and-braces:** engine `start` does NOT dedupe by
  workdir (`harness.rs:95` inserts unconditionally; `find_by_workdir:83` is
  start-work's guard) — so AC 5's client-side hide (registered ⇒ no banner) +
  D6 (gated creation ⇒ no signal) + `busy` (no double-accept) + server
  `claim_driver` (no double-run of one id) together close the race matrix.

---

## 6. Files (create/edit)

| File | f | Role |
|---|---|---|
| **NEW** `lib/workspace-harness-detect.ts` | f1 | Pure model (§3) |
| **NEW** `lib/workspace-harness-detect.test.ts` | f1 | Vitest pins (§7) |
| **NEW** `lib/workspace-harness-offer.ts` | f2/f3 | Runner + `acceptHarnessOffer` |
| **NEW** `lib/workspace-harness-offer.test.ts` | f2/f3 | Runner + accept pins |
| **NEW** `store/slices/workspace-harness-offer.ts` | f2 | Offer slice (§2) |
| **NEW** `components/HarnessSpecBanner.tsx` | f2/f3 | View + host (§4) |
| **NEW** `components/HarnessSpecBanner.test.tsx` | f2 | `renderToStaticMarkup` pins |
| EDIT `store/types.ts` (`:29` imports, `:59` intersection) | f2 | Register slice type |
| EDIT `store/index.ts` (`:31` imports, `:64` spread) | f2 | Register slice |
| EDIT `components/Terminal.tsx` (import near `:66`; mount after `:1569`) | f2 | Root-strip mount (§1) |
| EDIT `lib/open-created-workspace.ts` (import; 1 line at end of `openCreatedWorkspace`, after `:106`) | f2 | Trigger |
| EDIT `lib/open-created-workspace.test.ts` | f2 | `vi.mock` the runner + pin the call |
| EDIT `runtime/harness-client.ts` (`:148,:276,:286`) | f3 | `export` ×3 |

No server files. No `useComposerState.ts` edits. No changes to
`planCreatedWorkspaceOpen` (its `toEqual` pins at
`open-created-workspace.test.ts:132-188` stay byte-identical).

---

## 7. Test strategy (bun; `bunx vitest run <file>`; jsdom-free)

Precedent check: `components/tab-group/TabGroupPanel.sdd-bar.test.tsx` does
component testing via **`renderToStaticMarkup`** (react-dom/server) + `vi.mock`
— no jsdom (vite.config.ts declares no test environment; default node). That
is cheap enough to use for the banner's markup; everything decision-shaped
stays in pure functions regardless.

**f1 — `workspace-harness-detect.test.ts`** (pure, no mocks):
- `normalizeWorkdir`: trailing slash stripped; bare `/` kept; whitespace
  trimmed; idempotent — mirrors `expand_with_home` (`routes/util.rs:24-42`).
- `shouldDetectHarnessSpec`: gatedRun ⇒ false (D6); `connectionId: 'ssh-1'` ⇒
  false (D5); `undefined` ⇒ false (unknown worktree = fail closed); `null` ⇒ true.
- `hasFeatureList`: file hit; **dir named `feature_list.json` ⇒ false**; empty.
- `detectHarnessSpec`: canonical hit; legacy fallback when canonical `null`;
  both null ⇒ not found; **canonical dir present WITHOUT the file + legacy
  present WITH it ⇒ `found: false`** (the resolve_harness_dir-mirror pin — the
  case that would offer an unloadable run).
- `decideHarnessOffer`: registered exact match ⇒ null; trailing-slash variant
  still matches (normalization); unregistered ⇒ offer carries
  `worktreeId/workdir/harnessDir`; `found:false` ⇒ null.

**f2** — `workspace-harness-offer.test.ts` (node; `vi.mock`
`@/runtime/server-fs-client` + `@/runtime/harness-client`; REAL store seeded
like `open-created-workspace.test.ts:38-74`):
- remote repo (`connectionId: 'c1'`) ⇒ zero `fsListEntries` calls, no slice write.
- gatedRun ⇒ same.
- canonical listing throws (missing dir), legacy has the file ⇒ offer set with
  `harnessDir: '.harness'`.
- canonical has the file ⇒ ONE `fsListEntries` call only.
- nothing found ⇒ no slice write, `listHarnesses` never called (AC 6).
- worktree removed from store before resolve ⇒ signal dropped (close race).
- stale offer purged at runner start.
Plus `open-created-workspace.test.ts`: `vi.mock('@/lib/workspace-harness-offer')`,
pin one call per create path with `{ worktreeId, gatedRun }` (and
`gatedRun: true` passthrough). Plus `HarnessSpecBanner.test.tsx`
(`renderToStaticMarkup`, sdd-bar pattern): view renders the offer text +
"Start Harness run" + dismiss; `busy` disables both.

**f3** — extend `workspace-harness-offer.test.ts` for `acceptHarnessOffer`:
- happy path: `startHarness` then `runHarness(harness_id)` in order; success
  toast; slice entry cleared; `subscribeHarnessRunErrors` called with the id.
- `startHarness` rejects (Error message carries server detail) ⇒ `toast.error`
  message contains the detail; slice entry KEPT.
- `runHarness` rejects ⇒ same.
- dismiss: `clearWorkspaceHarnessOffer` only — assert zero harness-client calls
  (AC 4).

### AC → assertion map (verify.sh / qa.sh)

| AC | verify.sh (`bunx vitest run` + UI build) | qa.sh (browser) |
|---|---|---|
| 1 | runner tests: async, ≤2 entries calls, fallback order; trigger is fire-and-forget (`void`, structural) | fixture with spec: workspace surface renders before banner appears |
| 2 | banner markup test; single root mount (structural, §1) | banner visible on auto-launch create AND no-agent create; terminal interactive beneath |
| 3 | `acceptHarnessOffer` order/payload/toast-detail pins | accept ⇒ `GET /api/harness` lists the run, state ≠ idle |
| 4 | dismiss ⇒ zero client calls, slice cleared | dismiss on 2nd fixture ⇒ `GET /api/harness` unchanged |
| 5 | `decideHarnessOffer` dedupe + D6/D5 gate pins | pre-registered fixture ⇒ no banner |
| 6 | not-found ⇒ no slice write / no extra calls; `planCreatedWorkspaceOpen` pins untouched | fixture without spec ⇒ no banner, flow unchanged |
| 7 | the gate itself | — |

`verify.sh` = `bunx vitest run src/lib/workspace-harness-detect.test.ts
src/lib/workspace-harness-offer.test.ts src/lib/open-created-workspace.test.ts
src/components/HarnessSpecBanner.test.tsx` &&
`npm run build --prefix crates/agentum-desktop/ui`.

---

## 8. Build order f1 → f2 → f3, gates per slice

1. **f1** — detect helper + tests. Gate: its vitest file + UI build.
2. **f2** — slice + registration, runner (dedupe input `registeredWorkdirs: []`
   placeholder until f3 exports `listHarnesses`), trigger line, banner
   (accept handler stubbed to no-op busy), Terminal mount. Gate: f1+f2 vitest
   files + `open-created-workspace.test.ts` + UI build.
3. **f3** — three exports, `acceptHarnessOffer`, runner gains `listHarnesses()`
   dedupe, banner accept wired. Gate: all four vitest files + UI build.

**Pre-broken baselines (do NOT gate on):** full `bunx vitest run` (~138–139
fails) and bare `tsc` (~1650 errors) are red on develop (per project memory /
PM handoff); gates are the targeted suites + the Vite build only.

---

## 9. Risks & accepted residuals

- **Duplicate drivers** — closed by four independent layers (§5): D6 gate
  before any fs call, AC 5 registered-hide, `busy` double-click guard, server
  `claim_driver`. Engine `start` itself does not dedupe (`harness.rs:95`) — do
  not rely on it.
- **Path normalization** — trailing-slash/`~` handled by `normalizeWorkdir`
  (server mirror); symlink-spelling divergence accepted (same as the engine's
  own `find_by_workdir`).
- **Async detection race** — workspace closed before the result: runner
  re-checks the store before writing; stale same-id offers purged at runner
  start (worktree id = `repoId::path`, reusable).
- **`.agentum-harness/` present but no `feature_list.json`** — detection says
  no (mirrors engine load semantics); the composer's scaffold path owns that
  case (spec non-goal).
- **fs error ≠ missing dir** — both fold to "absent" (fail-closed, no banner).
  Accepted: a transient 500 during creation silently skips the offer.
- **Legacy-branch invisibility (#313)** — the mount is ABOVE both surface
  branches; never move it into the `:1666` fallback block.
- **Do not touch:** `useComposerState.ts`, `planCreatedWorkspaceOpen`'s return
  shape, the launcher overlay's `absolute inset-0 z-20` structure, harness
  server routes, `spawn_agent_into_pane`/launch path (the UI never spawns).

Open questions: none — Q1 (root flex strip, single mount, `z-30`) and Q2
(zustand offer slice + `openCreatedWorkspace` trigger) are closed above.
Feature order in `feature_list.json` stays `f1-detect-helper` → `f2-banner` →
`f3-register-run`.
