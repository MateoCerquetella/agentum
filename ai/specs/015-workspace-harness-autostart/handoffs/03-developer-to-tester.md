# Handoff 03 — Developer → Tester (spec 015-workspace-harness-autostart)

- **Date:** 2026-07-13
- **From:** Developer (SDD role loop, worktree `i-want-to-…-twe-h`)
- **To:** Tester
- **Read first:** `spec.md` (AC 1–7, D1–D6), `architecture.md`, `tasks.md`
  (per-slice record + the 3 documented deviations).

## Commits (this branch, on top of `96bc4f42`)

| SHA | Slice | Contents |
|---|---|---|
| `66f1e161` | f1 | `lib/workspace-harness-detect.ts` + its test (pure model) |
| `03f2eb2b` | f2 | offer slice + runner + `HarnessSpecBanner` + Terminal root mount + `openCreatedWorkspace` trigger + tests |
| `41cbeab8` | f3 | 3 exports in `harness-client.ts`, real `listHarnesses` dedupe, `acceptHarnessOffer`, banner accept wiring + tests |

All new/edited product files are under `crates/agentum-desktop/ui/src/`;
docs under `ai/specs/015-workspace-harness-autostart/`. No other paths.

## Re-running every gate

From `crates/agentum-desktop/ui` (bun; `bun install` first if
`node_modules` is absent in your worktree):

```sh
bunx vitest run \
  src/lib/workspace-harness-detect.test.ts \
  src/lib/workspace-harness-offer.test.ts \
  src/lib/open-created-workspace.test.ts \
  src/components/HarnessSpecBanner.test.tsx
# expected: 4 files, 50 tests, all passed
npm run build --prefix crates/agentum-desktop/ui   # from repo root; exit 0
```

Do NOT gate on full vitest (~138 pre-existing fails) or bare tsc (~1650
pre-existing errors) — pre-broken develop baselines.

## Adversarial checklist

- **Byte-identical pins:** `git diff 96bc4f42..HEAD --
  crates/agentum-desktop/ui/src/lib/open-created-workspace.test.ts` must
  show ONLY added lines (a `grep '^-'` on the diff body is empty). The
  `planCreatedWorkspaceOpen` describe (old `:132-188`) is untouched.
- **Must-not files:** `git diff 96bc4f42..HEAD --stat` contains NO
  `hooks/useComposerState.ts`, no `crates/agentum-server/`, no Rust, no
  `ai/STATE.md`.
- **Exactly three new exports** in `runtime/harness-client.ts`: the diff is
  +3/-3 lines — `export` prepended to `startHarness`, `listHarnesses`,
  `runHarness`. No renames, no new fns there.
- **Mount placement** (`components/Terminal.tsx`): `HarnessSpecBanner`
  appears ONCE, immediately after `<EditorAutosaveController />` and before
  the launcher conditional; grep confirms it is NOT inside the legacy
  no-layout block (~old `:1666`) nor `WorktreeSplitSurface`.
- **No polling:** grep the three new lib/component files for
  `setInterval` / `setTimeout` — none (the only timers in the flow live in
  the pre-existing `subscribeHarnessRunErrors`).
- **Detection trigger is single-sourced:** `maybeOfferWorkspaceHarnessRun`
  has exactly one product call site (`lib/open-created-workspace.ts`, last
  line of `openCreatedWorkspace`, `void`-fired).
- **No new storage:** `store/slices/workspace-harness-offer.ts` is plain
  state; no persist middleware anywhere in the diff; dismiss writes nothing
  (pinned: zero harness-client calls).
- **Deviations (3, listed in tasks.md):** verify each has its code comment —
  `acceptHarnessOffer` swallow-after-toast (docstring in
  `lib/workspace-harness-offer.ts`), the extra banner-test mock
  (`components/HarnessSpecBanner.test.tsx`), the new trigger-pin describe
  (`lib/open-created-workspace.test.ts`). If you judge any a contract
  break, that is a QA finding — the observable pins are in the tests.
- **The resolve_harness_dir mirror:** the semantic worth re-checking by
  hand — `.agentum-harness/` dir present WITHOUT `feature_list.json` +
  `.harness/` WITH it must offer NOTHING (`detectHarnessSpec` pin) because
  the engine would prefer the empty canonical dir and fail to load.

## Deferred to qa.sh (browser QA — cannot be unit-pinned)

Per spec "Harness wiring" + handoff 02 gate note, on a live app:

1. **AC 2:** fixture dir with `.agentum-harness/feature_list.json` → create
   workspace (BOTH paths: agent auto-launch and "no agent" launcher) →
   banner renders; terminal/launcher beneath stays interactive.
2. **AC 1:** workspace surface renders before the banner appears (async).
3. **AC 3:** accept → `GET /api/harness` lists the run, state ≠ idle; kill
   the server mid-accept (or use a bogus workdir) → toast carries the
   server detail; banner stays and accept is retryable.
4. **AC 4:** second fixture → dismiss → `GET /api/harness` unchanged.
5. **AC 5:** pre-registered fixture (`POST /api/harness` first) → no
   banner; wizard "Start gated run" creation → no banner (D6).
6. **AC 6:** fixture WITHOUT a spec → no banner, no UI change; also the
   legacy-only fixture (`.harness/feature_list.json`) → banner shows the
   `.harness` spelling and a run starts from it.
7. **D5:** SSH-host workspace creation → no fs probe, no banner (needs a
   configured remote; skip if none in the QA env).

## Known accepted residuals (do not file as bugs)

- Quick-create "Don't start a session" path produces no offer (returns
  before `openCreatedWorkspace`).
- Symlink-diverging workdir spellings defeat the AC 5 dedupe (same exposure
  as the engine's `find_by_workdir`).
- A transient fs/server error during detection silently skips the offer
  (fail-closed); a `listHarnesses` failure on the found path also skips
  (the runner's outer catch) — offer-less, never broken.
- On accept success the host calls `setBusy(false)` after the slice clear
  unmounts it — a no-op in React 19, by design of the `.finally` reset.
