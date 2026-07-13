# Verification — Spec 015 (Tester phase, 2026-07-13)

- **Tester:** sdd-tester subagent (independent of the developer; all gates
  re-run, all diffs re-derived — handoff numbers NOT trusted).
- **HEAD:** `49279512` (f1 `66f1e161` · f2 `03f2eb2b` · f3 `41cbeab8` · docs
  `5f753260`/`49279512`); base `96bc4f42`.
- **Verdict:** **PASS-WITH-DEFERRALS** — no Blockers, no Should-fix, 3 Info
  nits. All 7 in-browser scenarios remain deferred to qa.sh/staging per spec
  "Harness wiring" (listed at the end).

## Gates (independently re-run)

| Command | Result |
|---|---|
| `bunx vitest run` on the 4 targeted files (bun runner, from `crates/agentum-desktop/ui`) | **4 files / 50 tests passed, 0 failed** (Duration 1.01s; re-run 988ms). Per-file: `workspace-harness-detect.test.ts` 20, `workspace-harness-offer.test.ts` 14, `open-created-workspace.test.ts` 12, `HarnessSpecBanner.test.tsx` 4 — matches the developer's claimed breakdown exactly. |
| `npm run build --prefix crates/agentum-desktop/ui` | ✓ built in 38.45s, **exit 0** (only the pre-existing chunk-size warning). |

Full vitest / bare tsc deliberately NOT gated (pre-broken develop baselines,
per handoff 02 + project memory). No cargo build (UI-only change).

Diff containment: `git diff 96bc4f42..HEAD --stat` = 13 product files (all
under `crates/agentum-desktop/ui/src/`, exactly architecture §6's 7 NEW + 6
EDIT) + `tasks.md` + `handoffs/03-developer-to-tester.md` + `ai/STATE.md`
(Info nit 1 — docs commit only). No Rust, no `agentum-server`, no
`useComposerState.ts`.

## Sacred surfaces

All CLEAN, re-derived from the actual diffs:

- **`hooks/useComposerState.ts`:** `git diff 96bc4f42..HEAD -- …` is **empty**
  (verified directly, not via `--stat`).
- **`lib/open-created-workspace.ts`:** diff is exactly one import
  (`maybeOfferWorkspaceHarnessRun`) + a 2-line comment + one trailing
  fire-and-forget line at the END of `openCreatedWorkspace`
  (`void maybeOfferWorkspaceHarnessRun({ worktreeId, gatedRun: gatedRun === true })`,
  now `:111`). `planCreatedWorkspaceOpen` untouched.
- **`lib/open-created-workspace.test.ts`:** `git diff … | grep '^-'` (minus
  the `---` header) matches **nothing** — zero deleted lines; additions are
  one import, one `vi.mock` block, and one NEW describe with 2 trigger pins.
  All pre-existing test bodies (incl. the `planCreatedWorkspaceOpen`
  `toEqual` pins) byte-identical.
- **`runtime/harness-client.ts`:** diff is exactly +3/−3 — the `export`
  keyword prepended to `startHarness` (`:148`), `listHarnesses` (`:276`),
  `runHarness` (`:286`). No renames, no new functions (D4 names).
- **Mount placement (#313 trap):** read in full JSX context
  (`components/Terminal.tsx:1566-1591`): `HarnessSpecBanner` appears exactly
  once (import `:67`, mount `:1578-1580`), as a normal flex child of the root
  container (`:1567`), immediately after `<EditorAutosaveController />`
  (`:1570`) and BEFORE the launcher overlay conditional (`:1587`,
  `absolute inset-0 z-20`). `WorktreeSplitSurface` mounts at `:1662` and is
  defined at `:1932`; the legacy no-layout fallback comes later still — the
  banner is inside neither.
- **No polling:** grep for `setInterval|setTimeout` across the 4 new product
  files (`workspace-harness-detect.ts`, `workspace-harness-offer.ts`,
  `HarnessSpecBanner.tsx`, `store/slices/workspace-harness-offer.ts`) — zero
  hits.
- **Single trigger:** `maybeOfferWorkspaceHarnessRun` has exactly one
  non-test product call site (`lib/open-created-workspace.ts:111`, `void`-fired).
- **No new storage:** slice is plain zustand state registered in
  `store/index.ts`/`store/types.ts` (+2 lines each); `useAppStore` remains a
  bare `create` — no persist middleware anywhere in the diff; dismiss =
  `clearWorkspaceHarnessOffer` only (`HarnessSpecBanner.tsx:85-87`).

## Per-AC verdicts

| AC | Verdict | Evidence |
|---|---|---|
| 1 | PASS (render-order deferred to qa.sh) | Runner uses the real `fsListEntries` → `GET /api/fs/entries` (`workspace-harness-offer.ts:33`, `{ hidden: true }`, never `hostId` — signature re-checked at `server-fs-client.ts:52-60`); canonical→legacy fallback order + exact call args pinned (`workspace-harness-offer.test.ts:120-139`); trigger is structurally non-blocking (`void`-fired, `open-created-workspace.ts:111`; once-per-create pins `open-created-workspace.test.ts` new describe). |
| 2 | PASS (deferred: live interactivity/both-paths) | Single root mount above BOTH the launcher overlay and split surfaces (`Terminal.tsx:1578` — read in context, see Sacred surfaces); `relative z-30 … shrink-0` strip with the z-30 pinned load-bearing (`HarnessSpecBanner.test.tsx:41`); busy disables exactly two controls (`:63`); renders `''` without an offer (`:46`). |
| 3 | PASS (deferred: live `GET /api/harness` state) | `acceptHarnessOffer` = `startHarness(workdir)` → `runHarness(harness_id)` and nothing else (`workspace-harness-offer.ts:124-125`); order pinned via `invocationCallOrder` (`workspace-harness-offer.test.ts:231-233`); failure toast carries server detail (`toast.error(error.message)`, detail pre-embedded by `request()`; pins `:250`, `:264` via `stringContaining`); `subscribeHarnessRunErrors('h-1', fn)` armed on success (`:238`). |
| 4 | PASS (deferred: live `GET /api/harness` unchanged) | Dismiss = slice clear ONLY (`HarnessSpecBanner.tsx:85-87`); pinned with ZERO harness-client calls (`workspace-harness-offer.test.ts:269-279`); nothing persisted (no persist middleware, slice doc `store/slices/workspace-harness-offer.ts:5-11`). |
| 5 | PASS (deferred: live pre-registered fixture) | `decideHarnessOffer` normalized dedupe incl. trailing-slash spelling (`workspace-harness-detect.test.ts:109-129`); runner-level dedupe via real `listHarnesses()` pinned (`workspace-harness-offer.test.ts:166-176`); D6 gatedRun ⇒ zero fs calls (`:101-106`); D5 remote ⇒ zero fs calls (`:94-99`). |
| 6 | PASS (deferred: live no-UI-change) | Not-found ⇒ no slice write + `listHarnesses` never called + exactly the entries check(s) (`workspace-harness-offer.test.ts:155-164`: 2 calls when both dirs missing; `:178-186`: 1 call when canonical present); banner host renders `''` with no offer; existing create-flow pins untouched (additions-only diff). |
| 7 | PASS | The gates themselves: 50/50 across the 4 targeted files (1.01s) + Vite build exit 0 (38.45s), both re-run by the tester. Detection decision is pure/IO-free (`workspace-harness-detect.ts` — only a type-only `FsFileEntry` import). |

## Deviations audit (3 claimed — all ACCURATE)

1. **`acceptHarnessOffer` swallows after toasting** (vs architecture §5 "On
   throw" at the caller): accurately described; code-commented at the site
   (`workspace-harness-offer.ts:117-120` names it a deviation + reason);
   behavior-preserving — the observable contract (detail-in-toast, slice
   KEPT, banner retryable, busy reset via the component's `.finally`,
   `HarnessSpecBanner.tsx:80-82`) is fully pinned
   (`workspace-harness-offer.test.ts:241-267`). No unhandled-rejection
   surface. ACCEPTED.
2. **Banner test additionally mocks `@/lib/workspace-harness-offer`**:
   accurate; commented at the mock site (`HarnessSpecBanner.test.tsx:24-25`,
   "no network at render time"); test-only, keeps `sonner` + both runtime
   clients out of the render import graph — consistent with the sdd-bar
   pattern's intent. ACCEPTED.
3. **Trigger pins in a NEW describe with per-test `mockClear`**: accurate —
   the added describe (`open-created-workspace.test.ts:135+`) clears the mock
   per test because the file's shared `afterEach` resets only the store, and
   the pre-existing tests (which also call `openCreatedWorkspace`) would
   otherwise pollute call counts; zero deleted lines confirmed. Test-only,
   protects the byte-identity rule rather than breaking it. ACCEPTED. (The
   site comment explains the mock's purpose; the `mockClear` rationale lives
   in tasks.md — sufficient.)

## Adversarial spot-checks (7)

1. **`resolve_harness_dir` mirror** — PASS. Server code re-read
   (`crates/agentum-server/src/harness/types.rs:25-35`): canonical dir
   existing wins unconditionally, feature_list or not. Client
   `detectHarnessSpec` (`workspace-harness-detect.ts:72-78`) decides from a
   non-null canonical listing ALONE; the trap case (canonical present WITHOUT
   the file + legacy WITH it ⇒ `{found:false}`) is pinned
   (`workspace-harness-detect.test.ts:99-103`) and re-pinned at runner level
   with "legacy never fetched" (`workspace-harness-offer.test.ts:178-186`).
2. **Close-race re-check** — PASS. The runner re-reads
   `useAppStore.getState()` AFTER all async resolution and drops the signal
   if the worktree is gone (`workspace-harness-offer.ts:93-100`); pinned by a
   mock that deletes the worktree mid-fetch
   (`workspace-harness-offer.test.ts:188-197`).
3. **Mount JSX context (#313)** — PASS. Read directly, not trusted from the
   handoff: see Sacred surfaces. One mount, root strip, outside the legacy
   block and `WorktreeSplitSurface`.
4. **Accept failure path** — PASS. `runHarness` never fires after a
   `startHarness` reject; slice entry kept (banner retryable); toast carries
   the server detail; `subscribeHarnessRunErrors` NOT armed on failure; busy
   resets via `.finally`. All pinned (`workspace-harness-offer.test.ts:241-267`).
5. **No polling** — PASS. Zero `setInterval`/`setTimeout` in the four new
   product files; detection is a one-shot creation-moment call (D2).
6. **Real client APIs, not invented ones** — PASS. `fsListEntries(path,
   {hidden, hostId?})` signature and `FsEntries.entries` shape re-checked
   (`server-fs-client.ts:52-60`); `HarnessStatus.workdir: string` exists
   (`harness-client.ts:78`); `subscribeHarnessRunErrors` was already exported
   (`:378`).
7. **Store wiring** — PASS. Slice registered in both `store/types.ts`
   (intersection) and `store/index.ts` (spread); `clearWorkspaceHarnessOffer`
   is no-op-on-absent (`store/slices/workspace-harness-offer.ts:35-43`), so
   the stale purge never churns state.

## knip / export check

All three newly-exported harness-client fns are consumed by non-test product
code (`lib/workspace-harness-offer.ts`): `listHarnesses` `:82`, `startHarness`
`:124`, `runHarness` `:125`. No orphan exports introduced.

## Defects (3 Info nits, 0 Blockers, 0 Should-fix)

1. **Info** — Handoff 03's adversarial checklist claims `git diff
   96bc4f42..HEAD --stat` contains "no `ai/STATE.md`", but it does: the
   phase-transition docs commit `49279512` updates STATE.md (phase
   developer→tester + a log entry). The three product commits are clean of
   it; the handoff claim is inaccurate as literally written, content benign.
2. **Info** — `normalizeWorkdir('//')` returns `''` where the server's
   `expand_with_home` rejects with "workdir is empty" (`routes/util.rs`).
   Unreachable for real `worktree.path` values (absolute paths); the mirror
   holds for every realistic input.
3. **Info** — The runner joins paths with a POSIX `'/'` string concat
   (`${base}/${HARNESS_DIR}`). Fine on Linux/macOS and the server's `PathBuf`
   tolerates mixed separators; noted only should a Windows desktop target
   ever matter.

## Deferred to qa.sh / staging (per spec Harness wiring + handoff 03)

1. AC 2 live: banner on BOTH create paths (agent auto-launch + no-agent
   launcher); terminal/launcher interactive beneath.
2. AC 1 live: workspace surface renders before the banner appears (async).
3. AC 3 live: accept ⇒ `GET /api/harness` lists the run, state ≠ idle;
   induced failure ⇒ toast detail + retryable banner.
4. AC 4 live: dismiss ⇒ `GET /api/harness` unchanged.
5. AC 5 live: pre-registered fixture ⇒ no banner; wizard gated-run creation ⇒
   no banner (D6).
6. AC 6 live: spec-less fixture ⇒ no banner/no UI change; legacy-only fixture
   ⇒ `.harness` spelling shown and run starts.
7. D5 live: SSH-host workspace creation ⇒ no fs probe, no banner (needs a
   configured remote).
