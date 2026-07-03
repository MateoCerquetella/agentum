# Handoff 02 — Architect → Developer

- **Spec:** 008-finish-the-loop
- **Date:** 2026-07-03
- **From:** Architect (autonomous /sdd-loop iteration 2)
- **To:** Developer
- **Artifact:** `ai/specs/008-finish-the-loop/architecture.md` (seams line-verified on `0e6812f8`)

## Gate result

Architect gate: **PASS**. Boundaries explicit (§A may/must-not table per feature);
every seam cites `path:line`, self-checked on the worktree tip `0e6812f8`;
protected invariants confirmed untouched (one launch path, YOLO translation,
push streaming, best-effort tracker — §E); risks surfaced with mitigations;
decisions D-A/D-B/D-C change *how/where*, never product scope (D1–D9 intact);
per-feature build + `verify.sh`/`qa.sh` plan is concrete (§F).

## The one framing correction that changes everything (A1)

**The start-work path already shipped in spec 005.** `start_work`
(`harness.rs:508`), `ensure_spec_and_plan` (`:367`), `start_work_lock`
(`harness.rs:62`), the two-hop UI (`TaskPage.tsx:4529` → `useComposerState.ts:2279`
→ `harness-client.ts:171`), and spec 007's toast-on-every-skip
(`issue-side-effect-gate.ts:26`) are all present and correct. **F1 is
instrumentation + the D6 blocked escalation + a live test — NOT new plumbing.**
Do not rebuild the wire; make its *remaining silent points* loud.

## Architect decisions locked (do not re-litigate — see architecture.md §1)

- **D-A** — `status/blocked` is a fixed GitHub-only label with a NEW
  `apply_blocked_transition` sibling; `TrackerPhase` stays **four** variants
  (board/linear have no blocked column). One-of-five invariant lives purely at the
  GitHub-label remove-set.
- **D-B** — F2's stage is an **explicit `{mode, stage}` field** on `ChatRequest`
  (both `#[serde(default)]`), not server-derived from turn count. Server stays a
  pure `(mode, stage) → prompt` function; the client owns advancement.
- **D-C** — F3 is a **new thin `NewWorkspaceGoalStep.tsx`** fronting the modal;
  `useComposerState` internals are reused via props, never edited.
- **F-FLAG** — AC 2's never-silent guarantee requires `await_repl_ready → bool` +
  `inject_prompt → Result<bool>` (send sequence byte-identical). D5-permitted
  (behavior-preserving instrumentation), D5-gated: merges only with BOTH
  `harness_live_agent.rs` and the new `harness_start_work_live.rs` green.

## The four real silences to close (architecture.md §B.1)

1. **#15** `wait_for_settle` timeout → up to 1800 s silent hang. Fix:
   `→ SettleOutcome{Settled|Crashed|TimedOut}` + loud `Log` on `TimedOut`. **Not
   sacred — do this FIRST** (cheapest, biggest win, no live test needed).
2. **#16** blocked gate → no issue escalation (AC 4). Fix: `apply_blocked_transition`
   (D6) wired into `handle_gate_failure`'s blocked branch (`drive.rs:299`).
3. **#14a** `await_repl_ready` falls through unconfirmed → prompt fires blind
   (AC 2). Fix: the F-FLAG readiness bool. **Do LAST, behind both green live tests.**
4. **#2** composer armed `!repoId` guard returns silently (the #226 chat-origin
   edge, AC 1). Fix: `toast.error` when `startGatedRun` is armed and the guard trips.

## Build order (architecture.md §F)

F1 first and it ships alone. Within F1: step 1 (`SettleOutcome`) → step 2 (D6
blocked + argv tests) → step 4 (UI toasts + events bridge §B.5) → **step 3 last**
(the sacred readiness-bool, gated) → step 5 (`tests/harness_start_work_live.rs`,
which is what unlocks step 3's merge). Then F2, then F3 (independent of each other).

## Repo rules for the Developer (from CLAUDE.md)

- This is a **git worktree** (`finish-the-loop`) — stage only your files; never
  `git add -A`/`reset --hard`/`checkout`/`stash` in the shared checkout.
- Rebuild rhythm: `cargo test -p agentum-executor -p agentum-server --lib` for
  backend; `npm run build --prefix crates/agentum-desktop/ui` + vitest for UI.
- YOLO marker + one launch path are sacred; `apply_blocked_transition` must be
  `Ok(Skipped)`-never-`Err` (best-effort tracker contract).
- Commit per the repo rules at the end of the Developer phase (SDD compresses;
  spec.md + tasks.md carry the trail).

## Expected developer artifacts

Code + tests + `ai/specs/008-finish-the-loop/tasks.md` (root causes + what landed
where), building & testing green per the rebuild rhythm. The `#[ignore]` live
tests are the merge gate for the sacred F-FLAG change — wire and green them before
the `await_repl_ready`/`inject_prompt` bool passthrough.

## Recommended first step

Write `wait_for_settle → SettleOutcome` and its `TimedOut` loud-log — it closes
the largest silent hang, touches no sacred mechanic, and needs no live test. It is
the fastest path to a demonstrable "the loop no longer dies silently."
