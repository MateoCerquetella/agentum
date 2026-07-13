# Tasks — Spec 015 (workspace harness autostart)

Build order = handoff 02 / `architecture.md` §8: f1 → f2 → f3, each slice
gated and committed independently. All paths relative to
`crates/agentum-desktop/ui/src/` unless noted. Developer: Claude (SDD role
loop), 2026-07-13, branch `i-want-to-after-we-create-the-workspace-if-twe-h`.

## f1 — `f1-detect-helper` (AC 1, 5, 6, 7) — ✅ commit `66f1e161`

- [x] **NEW `lib/workspace-harness-detect.ts`** (115 lines) — pure, IO-free
  (model: `lib/workspace-goal-step.ts`; only a type-only `FsFileEntry`
  import). Exactly the architecture §3 API: `HARNESS_DIR` /
  `LEGACY_HARNESS_DIR` / `FEATURE_LIST_FILE` (mirroring
  `crates/agentum-server/src/harness/types.rs:16,19`), `normalizeWorkdir`
  (trim + trailing-slash strip, the `expand_with_home` mirror — no `~`, no
  canonicalization), `shouldDetectHarnessSpec` (D6 gatedRun ⇒ false; D5
  connectionId string/undefined ⇒ false, null ⇒ true), `hasFeatureList`
  (name AND `kind === 'file'`), `detectHarnessSpec` (canonical-listing-
  present decides ALONE — the `resolve_harness_dir` fold), and
  `decideHarnessOffer` (AC 5 normalized dedupe), plus the three types.
- [x] **NEW `lib/workspace-harness-detect.test.ts`** (156 lines, 20 tests) —
  all §7 pins including the two load-bearing ones: a DIRECTORY named
  `feature_list.json` ⇒ false, and canonical-without-file + legacy-with-file
  ⇒ `{ found: false }`.

**Gate (green):** `bunx vitest run src/lib/workspace-harness-detect.test.ts`
→ **1 file / 20 tests passed**; `npm run build --prefix
crates/agentum-desktop/ui` → ✓ built in 38.27s, exit 0.

## f2 — `f2-banner` (AC 1, 2, 4, 6) — ✅ commit `03f2eb2b`

- [x] **NEW `store/slices/workspace-harness-offer.ts`** (44 lines) —
  `harnessOfferByWorktreeId` record + `setWorkspaceHarnessOffer` /
  `clearWorkspaceHarnessOffer` (tracker-phase pattern; clear is
  no-op-on-absent). Registered in `store/types.ts` (import + `AppState`
  intersection) and `store/index.ts` (import + spread) — 4 lines total.
- [x] **NEW `lib/workspace-harness-offer.ts`** — the runner
  `maybeOfferWorkspaceHarnessRun({worktreeId, gatedRun})`, §3 steps 1–6 in
  order: stale purge → store-resolved worktree/connectionId (launcher
  pattern) + `shouldDetectHarnessSpec` gate → canonical `fsListEntries`
  (`hidden: true`, never `hostId`) via catch-to-null `listOrNull` → legacy
  fetch ONLY when canonical failed → found-check → dedupe (f2 placeholder
  `[]`) → close-race re-check → slice write. One outer try/catch swallows.
- [x] **NEW `components/HarnessSpecBanner.tsx`** — named
  `HarnessSpecBannerView` (presentational; `{harnessDir, busy, onAccept,
  onDismiss}`; root `relative z-30 flex shrink-0 … border-b bg-card px-3
  py-2`; both actions disabled while busy) + default store-host
  `HarnessSpecBanner({worktreeId})` (renders null without an offer; dismiss
  = slice clear only; accept stubbed until f3).
- [x] **NEW `components/HarnessSpecBanner.test.tsx`** —
  `renderToStaticMarkup` pins (sdd-bar pattern, mocked `@/store`, no jsdom):
  host renders strip incl. the load-bearing `z-30` / renders `''` without an
  offer; view busy ⇒ exactly two `disabled=""`.
- [x] **NEW `lib/workspace-harness-offer.test.ts`** — runner pins (mocked
  `@/runtime/server-fs-client` + `@/runtime/harness-client`, REAL store
  seeded like `open-created-workspace.test.ts`): remote ⇒ zero fs calls;
  gatedRun ⇒ same; canonical-missing + legacy-hit ⇒ `.harness` offer (call
  args pinned); canonical-hit ⇒ ONE fs call; canonical-present-without-file
  ⇒ legacy never fetched; nothing found ⇒ no write + `listHarnesses` never
  called; close race ⇒ dropped; stale purge; unknown worktree ⇒ fail closed.
- [x] **EDIT `components/Terminal.tsx`** (+11) — import beside
  `CodexRestartChip` (`:66`); mount immediately after
  `<EditorAutosaveController />`, BEFORE the launcher conditional:
  `{activeView === 'terminal' && activeWorktreeId ? (<HarnessSpecBanner
  worktreeId={activeWorktreeId} />) : null}`. NOT the legacy block, NOT
  inside `WorktreeSplitSurface`. **No line drift** — all handoff anchors
  (`:1569`, `:1571/:1576`) matched this worktree exactly.
- [x] **EDIT `lib/open-created-workspace.ts`** (+5) — one import + one
  fire-and-forget line at the END of `openCreatedWorkspace`:
  `void maybeOfferWorkspaceHarnessRun({ worktreeId, gatedRun: gatedRun ===
  true })`. `planCreatedWorkspaceOpen` untouched.
- [x] **EDIT `lib/open-created-workspace.test.ts`** (+39, zero removed
  lines) — `vi.mock('@/lib/workspace-harness-offer')` + a NEW describe with
  the two trigger pins (once-per-create with `{worktreeId, gatedRun:
  false}`; `gatedRun: true` passthrough). Existing `toEqual` pins
  byte-identical (verified: `git diff … | grep '^-'` → empty).

**Gate (green):** `bunx vitest run` on all four files → **4 files / 45
tests passed**; UI build ✓ 37.37s, exit 0; `git diff --stat` shows **no
`hooks/useComposerState.ts`**.

## f3 — `f3-register-run` (AC 3, 4, 5) — ✅ commit `41cbeab8`

- [x] **EDIT `runtime/harness-client.ts`** (+3/-3) — `export` added to
  exactly `startHarness` (`:148`), `listHarnesses` (`:276`), `runHarness`
  (`:286`) — the D4 names, nothing else changed.
- [x] **EDIT `lib/workspace-harness-offer.ts`** (final 139 lines) — dedupe
  placeholder replaced with `(await listHarnesses()).map((h) => h.workdir)`
  (found path only); new exported `acceptHarnessOffer(offer)`:
  `startHarness(workdir)` → `runHarness(harness_id)` → success toast +
  slice clear + `void subscribeHarnessRunErrors(harness_id, …)` (bounded,
  id-scoped — the spec 008 F1 precedent); failure ⇒ `toast.error` with
  `error.message` (server detail pre-embedded by `request()`), slice KEPT.
- [x] **EDIT `components/HarnessSpecBanner.tsx`** (final 100 lines) — accept
  wired: busy guard → `void acceptHarnessOffer(offer).finally(() =>
  setBusy(false))`; banner stays mounted (retryable) on failure.
- [x] **EDIT the two test files** — banner test additionally mocks
  `@/lib/workspace-harness-offer` (render-time import graph stays
  network-free); runner test (final 280 lines) gains the §7 f3 pins:
  register→run call order (`invocationCallOrder`), success toast + slice
  cleared + `subscribeHarnessRunErrors('h-1', fn)`, startHarness-rejects ⇒
  detail-in-toast + run-never-fires + slice kept, runHarness-rejects ⇒ same,
  dismiss ⇒ slice cleared with ZERO harness-client calls (AC 4), plus a
  runner-level AC 5 pin (registered trailing-slash spelling ⇒ no offer,
  `listHarnesses` called once).

**Gate (green):** `bunx vitest run` on all four files → **4 files / 50
tests passed** (20 detect + 14 runner/accept + 12 open-created + 4 banner);
UI build ✓ 39.07s, exit 0. knip-clean by construction: all three new
exports are consumed (runner + accept flow).

## Deviations (each also commented at the code site)

1. **`acceptHarnessOffer` swallows its failure instead of leaving the throw
   to the caller** (architecture §5 step 5 reads "On throw: toast.error …").
   The toast (with the server detail) fires INSIDE `acceptHarnessOffer`, the
   error is then swallowed, and the settled promise is the component's
   signal to reset `busy` via `.finally`. Reason: the component's only job
   is the busy flag — no caller try/catch, no possible unhandled rejection,
   and the failure surface stays in one unit-testable place. Observable
   behavior is identical and pinned (detail-in-toast, slice kept, banner
   retryable). Comment at `lib/workspace-harness-offer.ts`
   (`acceptHarnessOffer` docstring).
2. **Banner test mocks `@/lib/workspace-harness-offer`** (not in the
   handoff's mock list). `renderToStaticMarkup` never fires `onClick`, so
   the accept flow — and with it `sonner` + both runtime clients — is kept
   out of the component test's import graph, matching the sdd-bar pattern's
   "no network at render time" rule. Comment at the mock site in
   `components/HarnessSpecBanner.test.tsx`.
3. **Trigger pins live in a NEW describe block with per-test `mockClear`**
   rather than assertions added to the existing `openCreatedWorkspace`
   tests: the file's shared `afterEach` has no `clearAllMocks`, and
   touching the existing tests would risk the byte-identical-pins rule.
   Zero lines of the pre-existing file were removed.

## Invariants held

- No edits to `hooks/useComposerState.ts` (checked via `git diff --stat`
  before every commit).
- Existing `toEqual` pins in `lib/open-created-workspace.test.ts` are
  byte-identical (additive-only diff, zero removed lines).
- No polling/intervals; detection fires exactly once, from
  `openCreatedWorkspace` only (D2).
- No jsdom; component test is `renderToStaticMarkup` only.
- Exactly three new exports in `runtime/harness-client.ts` (D4 names).
- Accept = `POST /api/harness` + `POST /{id}/run`, nothing else; the UI
  never spawns agents; no server/Rust changes.
- No persistence of offers/dismissals (plain zustand, no persist
  middleware).
- Mount is the Terminal.tsx root strip — not the `:1666` legacy block, not
  `WorktreeSplitSurface` (#313).
- Client path handling = `normalizeWorkdir` only (trim + trailing-slash);
  no `~` expansion, no symlink canonicalization.

## Environment notes

- `node_modules` was absent in this worktree → `bun install` (591 packages)
  before the first gate; `bunx vitest run` (bun) is the runner, per project
  memory.
- Full vitest (~138 fails) and bare tsc (~1650 errors) remain the pre-broken
  develop baselines — not gated on, per handoff.

## Developer phase — ✅ COMPLETE (f1+f2+f3), 2026-07-13 → tester
