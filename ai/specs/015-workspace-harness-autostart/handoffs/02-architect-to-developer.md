# Handoff 02 — Architect → Developer (spec 015-workspace-harness-autostart)

- **Date:** 2026-07-13
- **From:** Architect (SDD role loop, worktree `i-want-to-…-twe-h`, HEAD `e7380875`)
- **To:** Developer
- **Read first:** `spec.md` (D1–D6 locked), then `architecture.md` (Q1/Q2
  closed there — root-strip mount + zustand offer slice; do not re-litigate).

## Deviations require documentation

If reality forces you off ANY design point below (file placement, function
signatures, mount point, test shape), write the deviation + reason into your
03 handoff AND a code comment at the site. Silent deviation = QA fail. Line
anchors were verified today on this worktree; if they drifted, anchor by the
named symbol and note the drift.

## Build order & per-slice definition of done

All paths relative to `crates/agentum-desktop/ui/src/` unless noted.
UI tests run with bun: `bunx vitest run <files>`. Full vitest (~138 fails) and
bare tsc (~1650 errors) are pre-broken baselines — never gate on them.

### f1 — detect helper (pure)

Create:
- `lib/workspace-harness-detect.ts` — exactly the API in architecture.md §3:
  `HARNESS_DIR`/`LEGACY_HARNESS_DIR`/`FEATURE_LIST_FILE` consts (mirror
  `crates/agentum-server/src/harness/types.rs:16,19` — never invent a third
  spelling), `normalizeWorkdir`, `shouldDetectHarnessSpec`, `hasFeatureList`,
  `detectHarnessSpec`, `decideHarnessOffer`, types `HarnessDirName`,
  `HarnessSpecDetection`, `WorkspaceHarnessOffer`. Type-only import of
  `FsFileEntry` from `runtime/server-fs-client.ts:32`. NO runtime/store imports.
  Header comment: pure, IO-free (model: `lib/workspace-goal-step.ts`).
- `lib/workspace-harness-detect.test.ts` — the pins in architecture.md §7,
  including the two load-bearing ones: dir-named-`feature_list.json` ⇒ false,
  and canonical-dir-without-file + legacy-with-file ⇒ `found: false`
  (mirrors `resolve_harness_dir`, `harness/types.rs:25-35`).

DoD: `bunx vitest run src/lib/workspace-harness-detect.test.ts` green;
`npm run build --prefix crates/agentum-desktop/ui` exits 0.

### f2 — banner + signal + mount

Create:
- `store/slices/workspace-harness-offer.ts` — slice per architecture.md §2
  (`harnessOfferByWorktreeId` record + set/clear). Copy the shape/style of
  `store/slices/tracker-phase.ts` (no-op-on-equal not required; keep it tiny).
- `lib/workspace-harness-offer.ts` — `maybeOfferWorkspaceHarnessRun({worktreeId,
  gatedRun})` per §3 runner steps 1–6 **in that order** (stale purge → gates →
  canonical listing → legacy only if canonical listing FAILED → found-check →
  `listHarnesses` dedupe [f3; pass `registeredWorkdirs: []` for now] →
  close-race re-check → set slice). One outer try/catch that swallows.
  `listOrNull` catches `fsListEntries` errors → null (missing dir =
  BadRequest at `crates/agentum-server/src/routes/fs.rs:209-211`). Never pass
  `hostId` (D5 already gated). Resolve worktree/connectionId from
  `useAppStore.getState()` — the `WorkspaceAgentLauncher.tsx:35-45` pattern.
- `components/HarnessSpecBanner.tsx` — named `HarnessSpecBannerView`
  (presentational, props `{harnessDir, busy, onAccept, onDismiss}`) + default
  store host `HarnessSpecBanner({worktreeId})` per §4. Strip root classes:
  `relative z-30 shrink-0 …` — `z-30` is load-bearing vs the launcher overlay's
  `z-20`. Accept handler = busy no-op stub until f3.
- `components/HarnessSpecBanner.test.tsx` — `renderToStaticMarkup` pins
  (pattern: `components/tab-group/TabGroupPanel.sdd-bar.test.tsx`; no jsdom).
- `lib/workspace-harness-offer.test.ts` — runner pins per §7 (mock
  `@/runtime/server-fs-client` + `@/runtime/harness-client`; real store seeded
  like `lib/open-created-workspace.test.ts:38-74`).

Edit:
- `store/types.ts` — import `WorkspaceHarnessOfferSlice` (beside `:29`), add to
  the `AppState` intersection (beside `:59`).
- `store/index.ts` — import (beside `:31`), spread `createWorkspaceHarnessOfferSlice(...a)`
  (beside `:64`).
- `components/Terminal.tsx` — import beside `:66`; mount immediately after
  `<EditorAutosaveController />` (`:1569`), BEFORE the launcher conditional
  (`:1571`): `{activeView === 'terminal' && activeWorktreeId ? (<HarnessSpecBanner
  worktreeId={activeWorktreeId} />) : null}`. **Do NOT mount inside the
  `:1666` legacy block or inside `WorktreeSplitSurface` — the #313 trap.**
- `lib/open-created-workspace.ts` — one import + one line at the END of
  `openCreatedWorkspace` (after `:106`):
  `void maybeOfferWorkspaceHarnessRun({ worktreeId, gatedRun: gatedRun === true })`.
  Nothing else in this file changes; `planCreatedWorkspaceOpen` (`:34-49`) is
  untouched.
- `lib/open-created-workspace.test.ts` — add
  `vi.mock('@/lib/workspace-harness-offer', …)` and pin: called once per create
  with `{worktreeId, gatedRun}`; `gatedRun: true` passes through. Existing
  `toEqual` pins (`:132-188`) must stay byte-identical.

DoD: `bunx vitest run src/lib/workspace-harness-detect.test.ts
src/lib/workspace-harness-offer.test.ts src/lib/open-created-workspace.test.ts
src/components/HarnessSpecBanner.test.tsx` green; UI build exits 0; no edits to
`hooks/useComposerState.ts` (verify with `git diff --stat`).

### f3 — register + run

Edit:
- `runtime/harness-client.ts` — add `export` to `startHarness` (`:148`),
  `listHarnesses` (`:276`), `runHarness` (`:286`). Exactly these names (D4).
- `lib/workspace-harness-offer.ts` — runner: replace the `[]` placeholder with
  `await listHarnesses()` → `.map(h => h.workdir)` (only reached on the found
  path). Add exported `acceptHarnessOffer(offer)` per architecture.md §5:
  startHarness → runHarness → success toast + clear slice +
  `void subscribeHarnessRunErrors(harness_id, m => toast.error(…))`
  (usage precedent: `hooks/useComposerState.ts:2328-2330`); on throw:
  `toast.error` with `error.message` (server detail already embedded by
  `request()`, `harness-client.ts:129-145`), keep the slice entry.
- `components/HarnessSpecBanner.tsx` — wire accept: set busy → await
  `acceptHarnessOffer` → reset busy on failure (banner stays, retryable).
- `lib/workspace-harness-offer.test.ts` — add the f3 pins (§7): call order,
  detail-in-toast, clear-on-success/keep-on-failure, subscribe-with-id,
  dismiss ⇒ zero client calls.

DoD: all four vitest files green; UI build exits 0; knip-clean by construction
(all three exports consumed).

## Gate commands (verify.sh content)

```sh
bunx vitest run \
  src/lib/workspace-harness-detect.test.ts \
  src/lib/workspace-harness-offer.test.ts \
  src/lib/open-created-workspace.test.ts \
  src/components/HarnessSpecBanner.test.tsx \
&& npm run build --prefix crates/agentum-desktop/ui
```

qa.sh (browser, spec's scenario): fixture dir with
`.agentum-harness/feature_list.json` → create workspace → banner renders (AC 2);
accept → `GET /api/harness` shows the run (AC 3); second fixture → dismiss →
`GET /api/harness` unchanged (AC 4); fixture without spec → no banner (AC 6);
pre-registered fixture → no banner (AC 5).

## Must NOT do

- No edits to `hooks/useComposerState.ts` (the Q2 design needs none — both
  create paths converge on `openCreatedWorkspace`, `useComposerState.ts:2533`
  and `:2750` call it already).
- No polling / intervals / re-detection on mount or activation (D2: the runner
  fires once per creation, from `openCreatedWorkspace` only).
- No spawning agents from the UI; accept = `POST /api/harness` +
  `POST /{id}/run`, nothing else (gate is sacred).
- No new server routes or Rust changes.
- No `~`-expansion or symlink canonicalization client-side beyond
  `normalizeWorkdir` (trim + trailing-slash) — mirror `routes/util.rs:24-42`,
  nothing more.
- No jsdom, no new test environment config; `renderToStaticMarkup` only.
- No persisting dismissals or offers (store has no persist middleware — keep it
  that way for this slice).
- Do not gate on full vitest or bare tsc (pre-broken baselines).

## Known accepted residuals (do not "fix" in this slice)

- Quick-create "Don't start a session" path (`useComposerState.ts:2732-2740`)
  produces no offer (returns before `openCreatedWorkspace`).
- Symlink-diverging workdir spellings can defeat dedupe (same exposure as the
  engine's `find_by_workdir`).
- A transient fs/server error during detection silently skips the offer
  (fail-closed).
