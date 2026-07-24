# Tasks — Autonomous SDD roles in the Harness Engine (013)

Ordered for TDD: each task names its test first. All Rust lands in
`crates/agentum-server/src/harness.rs` unless noted. Keep the existing feature loop untouched.

## Core engine

- [ ] **T1 — `SpecPhase` enum + run phase state.** Add `SpecPhase`
  (`Authoring/Architecture/Decompose/Executing/Review/Done/Blocked/AwaitingConfirm`), persist
  `phase` + `phase_attempts` per spec next to `feature_list.json`. *Test:* serde round-trip +
  `HarnessStatus` surfaces `phase`.
- [ ] **T2 — `parse_gate_verdict`.** Tolerant scan for the last `HARNESS_GATE:` line →
  `GateVerdict::{Pass, Concerns(reason)}`; absent/malformed → `Concerns`. *Test:* PASS, CONCERNS
  with reason, missing, malformed, surrounded-by-prose, case variations — **none** yield Pass
  except an explicit PASS.
- [ ] **T3 — `role_prompt(role, spec_ctx)`.** Compose central `ai/roles/<role>.md` (read-only) +
  spec context + the fixed `HARNESS_GATE:` trailer + artifact instruction. *Test:* output
  contains the role text, the spec id, and the verdict instruction; resolves the central path,
  not a repo copy.
- [ ] **T4 — `run_role_gate(role, run)`.** Spawn via `spawn_agent_into_pane`, wait for settle,
  capture pane tail, `parse_gate_verdict`, retry ≤ `max_retries`, then `Blocked` (or
  `AwaitingConfirm` when `hitl_on_block`). *Test:* stub agent emitting PASS advances; stub
  emitting CONCERNS retries then blocks; **default run never enters `AwaitingConfirm`**.

## Sequencer + bridge

- [ ] **T5 — `drive_phases(run)`.** Sequence Authoring → Architecture → Decompose → Executing →
  Review → Done; delegate `Executing` to the existing `drive_inner` unchanged; call
  `plan_from_spec` at `Decompose` (agentless). *Test:* full advance with stub agents; **a
  feature-only run with no role config behaves exactly as today** (regression).
- [ ] **T6 — wire `POST /{id}/run` to `drive_phases`.** Preserve `claim_driver` double-run
  rejection. *Test:* second concurrent run rejected.

## Observability + durability

- [ ] **T7 — events.** Add `HarnessEvent::PhaseChanged { from, to }` and
  `GateResult { role, verdict, attempt }`; broadcast on each transition. *Test:* a WS client
  observes the ordered sequence for a full run.
- [ ] **T8 — decisions.md as the single log.** Append a structured entry (role, verdict,
  attempt, artifact ref, canonical "entered <phase>" marker) per transition via
  `append_decision`; **stop writing `STATE.md`**. *Test:* log contains one canonical marker per
  transition.
- [ ] **T9 — `rebuild_phase_from_decisions`.** On `scan_board` rehydrate, re-derive current phase
  from `decisions.md` + `feature_list.json`. *Test:* clear store → rescan → phase restored.

## Knob + UI

- [ ] **T10 — `hitl_on_block` knob.** Default `false` (fully unattended, incl. QA). When `true`,
  an exhausted gate enters `AwaitingConfirm`, resumes via existing `confirm_feature`. *Test:*
  both branches.
- [ ] **T11 — `PhaseStrip` in `HarnessEngine.tsx`.** Render the phase strip + latest verdict from
  the new events via `runtime/harness-client.ts`. *Check:* component renders without crash;
  manual desktop smoke.

## Gate

- [ ] **T12 — full autonomy E2E + green suite.** Stub-agent fixture drives a spec
  authoring → done with **zero** human pauses; assert no `AwaitingConfirm` on default config.
  Run `cargo test -p agentum-server --lib` green with no regression in existing harness tests.
- [ ] **T13 — self-review.** Re-read the diff for false-green holes, `STATE.md` leftovers, and
  phase/feature coupling; confirm `architecture.md` matches the implementation.
