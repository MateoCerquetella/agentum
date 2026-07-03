# Handoff 01 — PM → Architect

- **Spec:** 008-finish-the-loop
- **Date:** 2026-07-03
- **From:** PM (autonomous /sdd-loop iteration 1)
- **To:** Architect
- **Artifact:** `ai/specs/008-finish-the-loop/spec.md` (PM-gated; decisions D1–D9 locked)

## Gate result

PM gate (`ai/skills/validate_handoff.md`): **PASS** — all nine items green after
edits. Every code citation spot-verified against the current tree: `start_work`
route `harness.rs:46` / handler `:508`; `spawn_agent_into_pane`
`provision.rs:107`; Socratic prompt `chat.rs:335`, stream `:476`;
`compose_issue_body` `chat.rs:981–1047`; `deriveIssueSideEffectGate`
`issue-side-effect-gate.ts:26`; composer Card 1,004 lines /
`useComposerState.ts` 2,806 lines; `harness_live_agent.rs` exists (`#[ignore]`);
`POST /api/github/issues` `routes/github.rs:35`.

## Decisions locked (see spec "Decisions (PM-locked)")

D1 interview state client-side, server stateless (stage in the request). D2
Complex mode = same model/config, extended thinking NOT required. D3 goal-first
is a parallel entry that becomes the default — composer NOT deleted. D4
Fast/Complex chosen per-feature, no sticky preference. D5 F1 may instrument
`drive.rs` but the three autonomy mechanics change only with both live tests
green; no new spawn path. D6 `status/blocked` joins the 004 label canon (five
labels, one-per-issue). D7 AC 1/2 numbers are demo-project pins. D8 "persisted
spec" = the existing `spec_md_from_issue` round-trip, no chat-time file write.
D9 F3's optional "repo" step = worktree creation is optional, workdir is not.

## Material PM findings

1. **"Start gated run" is a two-hop UI path, not one button.** Tasks-page action
   `openComposerForItem(item,{startGatedRun:true})` (`TaskPage.tsx:4527–4535`)
   → composer toggle → `startGatedWork` after `createWorktree`
   (`useComposerState.ts:2273–2313`, `alreadyRunning` at `:2303`) → client
   `startGatedWork` (`harness-client.ts:171`). AC 1 now covers both hops + the
   friendly `alreadyRunning` state. Blueprint the never-silent guarantee across
   the whole path, not just the server handler.
2. **`start_work_lock` serializes the whole orchestration** (incl. the network
   `gh` fetch). AC 1's 2 s acknowledgment must originate from the UI pending
   state, not the HTTP response — a double-click blocks on the lock.
3. **`repoId: ''` degenerate edge (#226)** is squarely an AC 1 "never silent"
   case for the chat-origin start path — fix or visibly re-defer, not silent.

## What to blueprint (in F1 → F3 order)

1. **F1 (riskiest, do first) — the never-silent run path.** Map every failure
   point along `start_work` (`:508`) → plan/issue-fetch → `spawn_agent_into_pane`
   (`provision.rs:107`) → settle (`harness/types.rs:97–102`) → verify gate, and
   decide where each emits a visible `HarnessEvent`/toast (D5 boundary: instrument
   freely, but the three autonomy mechanics — YOLO push, `await_repl_ready`,
   two-step `inject_prompt` — change only with `harness_live_agent.rs` **and** the
   new start-work-leg live test green). Design the new live test that covers
   **issue → start-work → session opens → prompt lands** (the leg
   `harness_live_agent.rs` skips). Design the `status/blocked` label addition as
   an argv-builder extension to the 004 label mechanics (idempotent ensure-create,
   fixed color, one-`status/*`-per-issue over five labels). Confirm no
   `deriveIssueSideEffectGate` skip path (`issue-side-effect-gate.ts:26`) is
   silent on the start route.
2. **F2 — Fast/Complex intake.** Decide the stateless staged-interview mechanism
   (D1): explicit `stage` field in `ChatRequest` vs server-derived from turn
   count; where the five per-stage system prompts live; how convergence after
   pass 5 reaches the same draft/preview endpoint as Fast. Fast mode's system
   prompt stays byte-identical to today's (`chat.rs:335`) — pin it with a unit
   test (pre-006 body-pin technique). `NO_CREDS_MSG` (`chat.rs:76`) must surface
   on Complex mode's first turn.
3. **F3 — goal-first workspace.** Decide whether the goal-first entry is a new
   thin component fronting the composer or a new initial step inside
   `NewWorkspaceComposerModal` (D3: parallel default, composer not deleted).
   Required inputs = goal + workdir target (D9); worktree/scaffold/tracker are
   skippable steps reusing existing primitives + `POST /api/github/issues` /
   scaffold routes.

## Open architect calls (flag to Mateo only if genuinely blocked)

- D1: explicit-stage-in-request vs server-derived-stage — architect's call.
- D9's "workdir is required" reading is the only physically coherent one
  (session = `(name, workdir, …)`), but worth a one-line confirm to Mateo in the
  architecture note since the interview said "the 4 can be optional".

## Expected architect artifact

`ai/specs/008-finish-the-loop/architecture.md` — boundaries, seam signatures
(the never-silent instrumentation points, the stateless stage mechanism, the
`status/blocked` argv builder), tradeoffs, risks, and a per-feature build/test
plan (matching prior specs' `architecture.md` shape).
