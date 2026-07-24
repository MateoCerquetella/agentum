# Handoff 04 — Tester → Reviewer (spec 015-workspace-harness-autostart)

- **Date:** 2026-07-13
- **From:** Tester (independent SDD role loop; all gates re-run, all sacred
  surfaces re-derived from the actual diffs — nothing taken from handoff 03
  on trust)
- **To:** Reviewer
- **Verdict:** **PASS-WITH-DEFERRALS** (0 Blockers, 0 Should-fix, 3 Info
  nits). Full evidence: `../verification.md`.
- **HEAD verified:** `49279512`; product commits `66f1e161` (f1),
  `03f2eb2b` (f2), `41cbeab8` (f3); base `96bc4f42`.

## Gate numbers (tester's own runs)

- `bunx vitest run` on the 4 targeted files: **4 files / 50 tests, 0 fail**
  (1.01s; per-file 20 detect + 14 offer/accept + 12 open-created + 4 banner —
  exactly the developer's claimed split).
- `npm run build --prefix crates/agentum-desktop/ui`: **exit 0**, ✓ 38.45s.
- Neither full vitest nor tsc nor cargo was gated (pre-broken baselines /
  UI-only change, per handoff 02).

## What the reviewer should focus on

1. **The swallow-after-toast deviation** (`acceptHarnessOffer`,
   `lib/workspace-harness-offer.ts:117-138`). I judged it
   behavior-preserving and fully pinned, but it is the only place the
   implementation knowingly diverges from architecture §5's letter ("On
   throw" at the caller). If the reviewer wants error propagation for future
   callers (e.g. a retry-with-backoff wrapper), this is the seam to flag —
   today the settled promise is the component's only signal.
2. **Dedupe timing window (design-accepted, worth eyes):** `listHarnesses()`
   runs once at detection time; a harness registered between banner-render
   and accept is not re-checked at accept. The layered guards (D6 gate, AC 5
   hide, `busy`, server `claim_driver` on `/run`) close the double-DRIVER
   race, but `POST /api/harness` itself inserts unconditionally
   (`harness.rs:95` per architecture §5) — a late double-accept across two
   app windows could register the same workdir twice (one idle, one
   running). Architecture explicitly accepts this matrix; confirm you agree.
3. **UX copy/placement judgment call:** the banner strip renders above the
   launcher overlay via `z-30` (pinned). On very short windows the strip
   plus the launcher card could feel tight — pure design taste, no
   functional issue found.
4. **Info nit 1:** handoff 03's "no `ai/STATE.md` in the diff" claim is
   false for the full range (the phase-transition docs commit `49279512`
   touches it; product commits are clean). Benign, but worth knowing the
   developer's adversarial checklist had one inaccurate line.

## Unresolved risks (all accepted residuals, none new)

- Symlink-diverging workdir spellings defeat the AC 5 dedupe (same exposure
  as the engine's `find_by_workdir`).
- Transient fs/server error during detection silently skips the offer
  (fail-closed by design); a `listHarnesses` failure post-detection also
  skips via the outer catch.
- Quick-create "Don't start a session" path never fires the runner (returns
  before `openCreatedWorkspace`) — per D2, no offer there.
- POSIX `'/'` path join in the runner (Info nit 3) — irrelevant on the
  supported platforms.

## Deferred items (qa.sh / staging — cannot be unit-pinned)

1. AC 2: banner on both create paths (auto-launch + launcher); surface
   beneath stays interactive.
2. AC 1: workspace surface renders before the async banner appears.
3. AC 3: accept ⇒ `GET /api/harness` lists the run (state ≠ idle); induced
   failure ⇒ toast carries server detail, banner retryable.
4. AC 4: dismiss ⇒ `GET /api/harness` unchanged.
5. AC 5: pre-registered fixture ⇒ no banner; gated-run creation ⇒ no banner.
6. AC 6: spec-less fixture ⇒ no banner/no change; legacy-only fixture ⇒
   `.harness` spelling + run starts from it.
7. D5: SSH-host creation ⇒ no fs probe, no banner (needs a configured
   remote in the QA env).

## Sacred-surface attestation (re-derived)

`useComposerState.ts` diff empty; `open-created-workspace.ts` = import +
trailing `void` call only; `open-created-workspace.test.ts` additions-only
(zero deleted lines, pre-existing pins byte-identical);
`harness-client.ts` = exactly +3/−3 `export` keywords (D4 names); single
`HarnessSpecBanner` mount at the Terminal.tsx root strip (read in JSX
context — not the legacy block, not `WorktreeSplitSurface`); zero
polling/timers in the new files; no persist middleware; the
`resolve_harness_dir` mirror verified against
`crates/agentum-server/src/harness/types.rs:25-35` and pinned in tests.
