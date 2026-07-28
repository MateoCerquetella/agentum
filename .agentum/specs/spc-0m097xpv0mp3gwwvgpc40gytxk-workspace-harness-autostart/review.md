# Review — Spec 015: Workspace harness autostart

- **Spec:** 015-workspace-harness-autostart
- **Date:** 2026-07-13
- **Reviewer:** sdd-reviewer (independent SDD role loop, final phase)
- **Code under review:** commits `66f1e161` (f1) · `03f2eb2b` (f2) · `41cbeab8` (f3), branch `i-want-to-after-we-create-the-workspace-if-twe-h`, base `96bc4f42`, HEAD `cfe23f37` (docs commits above f3 touch no product code — verified via `git log 96bc4f42..HEAD`)
- **Inputs:** spec.md (D1–D6 locked) · architecture.md (§0–§9) · verification.md (PASS-WITH-DEFERRALS) · handoffs/04-tester-to-reviewer.md
- **Method:** independent read of the FULL product diff (`git diff 96bc4f42..HEAD -- crates/`, all 13 files) plus every server/runtime surface the new code mirrors or calls (`harness/types.rs::resolve_harness_dir`, `routes/util.rs::expand_with_home`, `routes/harness.rs::start` + `HarnessEngine::start` (`harness.rs:95`), `harness-client.ts::request/subscribeHarnessRunErrors`, `server-fs-client.ts::fsListEntries`, `WorkspaceAgentLauncher.tsx:35-45`, `worktrees.ts::buildWorktreePurgeState`, `Terminal.tsx:1566-1680` in JSX context). Gate numbers are tester-attested (verification.md); no gates re-run.
- **Verdict: SIGN-OFF** — 0 Blockers, 1 Should-fix (follow-up ticket), 4 Nits recorded. Details below.

---

## Focus 1 — Correctness of the new logic

**Async detection vs close/switch race — SUFFICIENT for every reachable case, one ms-scale residual (Nit 3).** The runner re-reads the store after ALL async resolution and drops the signal when the worktree is gone (`workspace-harness-offer.ts:96-100`: `Object.values(useAppStore.getState().worktreesByRepo ?? {}).flat().some((w) => w.id === opts.worktreeId)`), pinned by the mid-fetch-delete test (`workspace-harness-offer.test.ts:188-197`). The re-check verifies *presence*, not *creation generation*: a close-then-gated-recreate at the same id completing inside runner A's in-flight window would let A's late write land after B's purge (`:55` runs first in B). That window is two loopback HTTP round trips — humanly unreachable, and the damage is bounded to a banner whose accept is still user-confirmed and server-validated. Nit 3, same accepted class as architecture §5's double-register matrix.

**Two workspaces back-to-back — SOUND.** Each invocation is keyed by its own `worktreeId`; the slice is a record (`store/slices/workspace-harness-offer.ts:14`), `setWorkspaceHarnessOffer` spreads and writes one key (`:27-33`), and the banner selects only `harnessOfferByWorktreeId[worktreeId]` for the *active* worktree. Two concurrent runners on different ids cannot interfere; two on the same id write identical content (same path ⇒ same detection inputs).

**Stale-offer lifecycle — one real gap (Should-fix 1).** Worktree deletion does NOT purge the offer: `buildWorktreePurgeState` (`worktrees.ts:606-`) omits every other per-worktree map (`worktreeLineageById`, `tabsByWorktree`, …) but `harnessOfferByWorktreeId` is absent from its return — verified by grep (no file outside the new slice/banner/runner references the key). Mitigations that hold: an orphan entry is unreachable by the banner (it renders only for `activeWorktreeId`, which must be a live worktree), and every IN-APP recreation goes through `openCreatedWorkspace` → the runner's stale purge FIRST (`workspace-harness-offer.ts:51-55`, pinned at `workspace-harness-offer.test.ts` "purges a stale offer"). The leak needs an out-of-band reappearance of the same `${repoId}::${path}` id (external worktree creation picked up by refresh) — then a pre-deletion offer resurfaces outside any creation moment (against D2's spirit). Consequence is fail-safe: accept re-validates server-side (`HarnessEngine::start` → `HarnessConfig::load(&workdir)` errors → 400 → toast). One line in `buildWorktreePurgeState` + a pin closes it; follow-up ticket, not a blocker.

**`normalizeWorkdir` dedupe — SOUND mirror.** Client (`workspace-harness-detect.ts:26-29`): trim, then strip trailing `/+` when `length > 1`. Server (`routes/util.rs:24-30`): `trim()`, then `trim_end_matches('/')` when `len() > 1`. Behaviorally identical for every realistic input; the `'//'→''` divergence (server rejects "workdir is empty", client returns `''`) is unreachable for absolute `worktree.path` values — concur with tester Info nit 2. Both sides normalized before comparison (`decideHarnessOffer`, `:106-108`); no symlink canonicalization on either side — the accepted residual matches `find_by_workdir`'s exposure exactly.

**Fallback vs `resolve_harness_dir` — CORRECT mirror, one fail-open-into-error edge (Nit 4).** Server (`harness/types.rs:25-35`): canonical `is_dir()` wins unconditionally. Client (`workspace-harness-detect.ts:72-78`): non-null canonical listing decides ALONE — so canonical-without-file + legacy-with-file ⇒ `{found:false}` (pinned, `workspace-harness-detect.test.ts:99-103`), never offering a run the engine can't load. The one divergence: `listOrNull` folds a *transient* error on an *existing* canonical dir to `null` (`workspace-harness-offer.ts:33-40`), letting a legacy hit through while the engine would resolve canonical. Consequence: cosmetic dir-name mislabel in the banner when canonical also holds a valid spec, or a fail-safe 400 toast when it doesn't. Nit.

## Focus 2 — Security / safety: CLEAN

- **Accept inputs:** `startHarness(offer.workdir)` posts `worktree.path` — the store's own record from the app's create flow, then server-side `expand_workdir` + `HarnessConfig::load` validation (`routes/harness.rs:143-150`). `runHarness(harness_id)` and `subscribeHarnessRunErrors(harness_id, …)` use the *server's* response id. No attacker-influenced input beyond the user's own workdir choice.
- **Toasts:** `request()` builds errors as `` `harness ${res.status} on ${path}${detail ? ` — ${detail}` : ''}` `` (`harness-client.ts:139-142`) — response body text only; the Authorization header is never echoed. `toast.success('Harness run started')` is static.
- **Banner rendering:** `harnessDir` is typed `HarnessDirName = typeof HARNESS_DIR | typeof LEGACY_HARNESS_DIR` (`workspace-harness-detect.ts:17`) — a const union, and it renders as a JSX text node (React-escaped) with no `dangerouslySetInnerHTML` anywhere in the component. No user-controlled string reaches the banner at all.

## Focus 3 — Invariants: ALL HELD

- **One launch path:** accept is `startHarness` → `runHarness` and nothing else (`workspace-harness-offer.ts:123-125`); no client-side agent spawning, no executor-adjacent imports. The drive loop (server) spawns agents through `spawn_agent_into_pane` as before.
- **Gate sacred:** no `/init` call, no feature-state writes, no `feature_list.json` touch — register + run only, pinned by the "exactly register + run" accept tests (`workspace-harness-offer.test.ts:231-238`).
- **No polling / zero re-detection:** grep confirms zero timers in the four new product files and exactly ONE product call site of `maybeOfferWorkspaceHarnessRun` (`open-created-workspace.ts:111`, `void`-fired). The banner performs no fetch on mount (store-select → render only). `subscribeHarnessRunErrors` is pre-existing, push-based (WS), one-shot, self-closing at 120 s (`harness-client.ts:383-402`) — not a poll.
- **D2 creation-moment-only:** `openCreatedWorkspace` is the only caller of the runner; the component only *clears*; the slice doc (`:7-8`) pins "Written ONLY by `maybeOfferWorkspaceHarnessRun`".
- **D5/D6 BEFORE any fs call:** structural order in the runner is purge (`:55`) → resolve worktree/connectionId (`:59-64`) → `shouldDetectHarnessSpec` gate (`:65-67`) → first `listOrNull` (`:70`). `shouldDetectHarnessSpec` checks `gatedRun` first, then `connectionId === null` (`workspace-harness-detect.ts:42-47`); zero-fs-call pins at `workspace-harness-offer.test.ts:94-106`.
- **Sacred files:** `useComposerState.ts` untouched (empty diff), `planCreatedWorkspaceOpen` byte-identical, harness-client diff exactly +3 `export` keywords (D4 names).

## Focus 4 — UX correctness at the mount

- **Layout shift when tabs exist:** yes, and it is the *designed* AC 2 mechanism, not a defect. The strip is a `shrink-0` flex child of the flex-col root (`Terminal.tsx:1567-1580`); the split-surface anchor sibling is `relative flex flex-1 min-w-0 min-h-0` (`:1643`), so the banner's appearance shrinks it by one strip height and the `absolute inset-0` worktree surfaces inside size to the shrunken anchor. Non-occluding by construction; a one-time xterm re-fit on appear/dismiss is the normal cost of any strip (SDD-bar precedent). The launcher overlay (`absolute inset-0 z-20`, `:1588`) covers the strip's region, which is exactly why `relative z-30` is load-bearing — for both painting AND hit-testing (positioned element) — pinned at `HarnessSpecBanner.test.tsx:41`.
- **Clear targets the RIGHT worktree on mid-detection switch:** the banner always receives the CURRENT `activeWorktreeId`; dismiss clears that same prop id (`HarnessSpecBanner.tsx:85-87`) and accept clears `offer.worktreeId` (`workspace-harness-offer.ts:127`) where the offer was selected BY the prop id — the two cannot diverge. An offer set for worktree A while B is active simply doesn't render until the user returns to A: correct.
- **Minor observation (part of Nit 5):** the component instance survives worktree switches (same tree position, no `key`), so `busy` from an in-flight accept on A briefly disables B's banner buttons. Milliseconds, fail-safe direction (blocks, never double-fires). Recorded, no action.

## Focus 5 — The 3 deviations + 3 tester nits

| Item | Ruling | Rationale |
|---|---|---|
| Dev-deviation 1: `acceptHarnessOffer` swallows after toasting (vs architecture §5 "on throw" at the caller) | **Leave-as-is** | Sole caller is the component; the settled promise is only the busy signal; documented at site (`workspace-harness-offer.ts:117-120`) and fully pinned (toast detail, slice KEPT, retryable, no unhandled rejection). A future retry-wrapper can add a throwing variant then. |
| Dev-deviation 2: banner test mocks `@/lib/workspace-harness-offer` | **Leave-as-is** | Test-only; keeps sonner + runtime clients out of the render import graph; commented at the mock site. |
| Dev-deviation 3: new describe + per-test `mockClear` | **Leave-as-is** | Protects the byte-identity of the pre-existing pins (zero deleted lines confirmed) rather than weakening them. |
| Tester nit 1: handoff 03's "no `ai/STATE.md`" claim inaccurate | **Leave-as-is** | Docs-only inaccuracy in a superseded handoff; product commits clean; recorded. |
| Tester nit 2: `normalizeWorkdir('//') → ''` | **Leave-as-is** | Unreachable for absolute `worktree.path`; server rejects the same input. |
| Tester nit 3: POSIX `'/'` path join | **Leave-as-is** | Linux/macOS targets; server `PathBuf` tolerates mixed separators. |

**Tester focus item 2 (dedupe timing window) — I CONFIRM the accepted matrix.** `POST /api/harness` inserts unconditionally (`HarnessEngine::start`, `harness.rs:95-120` — `HarnessConfig::load` validates the spec but nothing dedupes by workdir); the four layers (D6 pre-fs gate, AC 5 registered-hide, component `busy`, server `claim_driver` per id) close the double-DRIVER race for every single-window flow. The residual double-REGISTER (two windows; or `busy` reset by a full remount — view-switch away and back inside the accept round trip) can create a second idle/second-driver registration on the same workdir — architecture §5 names and accepts exactly this; Nit 5 records the remount variant.

## Focus 6 — React correctness of `HarnessSpecBanner`

- **Selector granularity:** `useAppStore((s) => s.harnessOfferByWorktreeId[worktreeId])` selects one record entry (stable reference between writes; zustand `Object.is` equality) and `(s) => s.clearWorkspaceHarnessOffer` a stable function — no over-rendering, no unstable-snapshot loops. `clearWorkspaceHarnessOffer` returns `s` unchanged when the key is absent (`store/slices/workspace-harness-offer.ts:36-38`) — zustand skips notify on identity, so no churn.
- **setState-after-unmount:** `void acceptHarnessOffer(offer).finally(() => setBusy(false))` can fire after unmount; React 18 makes this a silent no-op (no leak — no subscription is held by the component). `acceptHarnessOffer` never rejects (swallow-after-toast), so the `void`ed chain cannot become an unhandled rejection.
- **Handler safety:** `handleAccept` guards `!offer || busy` before setting busy; `useCallback` deps (`[busy, offer]`, `[clearOffer, worktreeId]`) are complete. On success the slice clear unmounts the VIEW (host returns null) while the host component stays mounted — `busy` state remains coherent.

---

## Findings summary

| # | Ruling | Finding |
|---|---|---|
| 1 | **Should-fix** (follow-up ticket) | `harnessOfferByWorktreeId` is missing from `buildWorktreePurgeState` (`worktrees.ts:606-`): a stale offer survives worktree deletion and can resurface if the same `${repoId}::${path}` id reappears out-of-band (external recreation + refresh) — a D2-spirit leak. Every in-app path is covered by the runner's purge-first; consequence is fail-safe (server re-validates on accept). Fix = one `omitByWorktree` line + a pin. |
| 2 | Nit (record) | Repo-record-missing folds to local: `state.repos?.find(...)?.connectionId ?? null` (`workspace-harness-offer.ts:62-64`) cannot distinguish "repo found, connectionId absent" (= local, the common shape) from "repo record gone" (architecture said fail-closed). Identical to the established `WorkspaceAgentLauncher.tsx:37-44` fold; worst case is a local fs listing on an app-known path. |
| 3 | Nit (record) | Close-then-gated-recreate inside the runner's in-flight window: the presence-only re-check (`:96-100`) lets creation A's late offer land after creation B's purge. Window is two loopback round trips — humanly unreachable; damage bounded by user-confirmed accept + server validation. Same accepted class as the §5 matrix. |
| 4 | Nit (record) | `listOrNull` folds a transient error on an EXISTING canonical dir to "absent", letting a legacy offer through where the engine resolves canonical — cosmetic dir-name mislabel or fail-safe 400. Inverse direction of the accepted fs-error residual. |
| 5 | Nit (record) | `busy` is component-local: it persists across worktree switches (briefly disabling another worktree's banner — fail-safe) but resets on a full remount (view-switch during an in-flight accept), reopening the narrow double-register window architecture §5 already accepts. |

## Feedback Contract summary

**What worked well:** the pure-model/IO-shell split makes every decision (D5/D6 gate, `resolve_harness_dir` mirror, dedupe) an exhaustively-pinned pure function; the purge-first + close-race re-check in the runner shows real adversarial thinking; the single root-strip mount with the load-bearing `z-30` pin avoids the #313 trap by construction; the +3-`export`-only harness-client diff and empty `useComposerState` diff are exemplary containment.

**Risks:** five bounded findings above; the 7 qa.sh live scenarios remain genuinely unexecuted until staging.

**Blockers:** none.

**Follow-up ticket (one ticket):**
1. Add `harnessOfferByWorktreeId: omitByWorktree(s.harnessOfferByWorktreeId)` to `buildWorktreePurgeState` + a cascade pin (Finding 1).
2. Optionally split the repo-missing fold from the connectionId-absent fold in the runner (Finding 2, 2 lines).

## Final verdict

**SIGN-OFF.** All 7 ACs delivered at the unit level with the spec-defined qa.sh deferrals; D1–D6 honored (D2/D5/D6 verified structurally, not just by tests); all repo and spec invariants held (one launch path, gate sacred, zero polling/re-detection, no new storage); security surface clean; the three developer deviations and three tester nits all correctly ruled leave-as-is; one Should-fix and four Nits recorded, none shippable-blocking. Spec 015 is ready to promote; the qa.sh live checklist and the follow-up ticket are owned by the human release step.
