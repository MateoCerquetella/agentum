# Architecture Notes — Autonomous SDD roles in the Harness Engine (013)

## Components

All new logic lands in `crates/agentum-server/src/harness.rs` (the engine) with a thin surface
in `routes/harness.rs` and `agentum-desktop/ui/.../HarnessEngine.tsx`. Nothing in the existing
**feature** state machine changes; 013 adds a **spec-phase** layer *above* it.

1. **`SpecPhase` enum + run phase state** — new in `harness.rs`, beside the existing
   `FeatureState` (`harness.rs:65`). Variants: `Authoring`, `Architecture`, `Decompose`,
   `Executing`, `Review`, `Done`, plus `Blocked { reason }` and (opt-in) `AwaitingConfirm`.
   Persisted per spec as `phase` + `phase_attempts` on the run record next to `feature_list.json`
   (per-spec, never a global mutable file — consistent with 010b).

2. **Phase driver** — `drive_phases(run)` wraps today's feature loop. It sequences the role
   gates and delegates the `Executing` phase to the **existing** `drive_inner` (`harness.rs:1303`)
   unchanged. This is the only new top-level loop; `drive()`/`drive_inner()` keep their contract.

3. **Role gate runner** — `run_role_gate(role, run)`:
   - spawns the role-agent via `routes::sessions::spawn_agent_into_pane` (same helper the
     feature loop uses → YOLO, loopback `pane_env`, MCP wiring stay centralized),
   - waits for settle by subscribing to the session lifecycle bus (reuses the feature loop's
     settle logic: grace window + first `agent.awaiting_input`/`agent.finished`),
   - captures the pane tail and calls `parse_gate_verdict`,
   - returns `GateOutcome::{Pass, Concerns(reason)}`.

4. **Verdict parser** — `parse_gate_verdict(pane_text) -> GateVerdict`. Scans for the last
   `HARNESS_GATE:` line. `PASS` → `Pass`; `CONCERNS <reason>` → `Concerns`; absent/malformed →
   `Concerns("no verdict emitted")`. **Missing is never PASS** (false-green guard, mirrors the
   feature gate's "agent self-report is never sufficient").

5. **Role prompt assembly** — `role_prompt(role, spec_ctx)` composes: the central role playbook
   (`ai/roles/{pm,architect,reviewer}.md`, read-only, resolved centrally — not copied per-repo),
   the spec context (`spec.md` + prior `decisions.md`), and a fixed trailer instructing the agent
   to end with the `HARNESS_GATE:` line and to write its artifact (`spec.md` / `architecture.md`
   / review notes).

6. **Decompose step** — `Decompose` calls the existing `plan_from_spec` (`harness.rs:630`)
   directly (agentless), absorbing the old manual `agentum_harness_plan` MCP call.

7. **Events** — two new `HarnessEvent` variants: `PhaseChanged { from, to }` and
   `GateResult { role, verdict, attempt }`, broadcast on the existing `/api/harness/events` WS.

8. **Decision log integration** — every transition calls `append_decision` (`harness.rs:693`)
   with a structured entry; the engine **stops writing `STATE.md`**. `rebuild_phase_from_decisions`
   re-derives the current phase when `scan_board` (`harness.rs:549`) rehydrates a run.

9. **UI** — a `PhaseStrip` in `HarnessEngine.tsx` above the feature board, fed by the new events
   via `runtime/harness-client.ts`.

---

## APIs

No new REST routes are strictly required (runs kick off via the existing `POST /api/harness/{id}/run`).
Changes are additive:

- **`GET /api/harness/{id}`** — `HarnessStatus` gains `phase: SpecPhase` and `phase_attempts`.
- **`WS /api/harness/events`** — emits the new `PhaseChanged` / `GateResult` variants.
- **Run config** — `feature_list.json` run knobs gain `hitl_on_block: bool` (default `false`)
  and reuse existing `max_retries`, `agent_tool`, `agent_model` for role phases (v1: one config
  for all phases; per-phase override deferred).

**Internal interfaces (see `contracts/`):** the `HARNESS_GATE:` verdict grammar, the `SpecPhase`
transition table, and the two new event shapes.

---

## Data Flow

```
POST /{id}/run
   └─ drive_phases(run)
        ├─ Authoring     → spawn PM-agent → settle → parse_gate_verdict
        │                   PASS → append_decision + PhaseChanged → Architecture
        │                   CONCERNS → retry ≤ max_retries → Blocked (or AwaitingConfirm if hitl_on_block)
        ├─ Architecture  → spawn architect-agent → … (same gate)
        ├─ Decompose     → plan_from_spec()  (agentless; writes feature_list.json)
        ├─ Executing     → drive_inner()  ← EXISTING feature loop, WIP=1, verify.sh gate, unchanged
        ├─ Review        → spawn reviewer-agent → … (same gate)
        └─ Done          → append_decision + PhaseChanged
   (PhaseChanged / GateResult stream to HarnessEngine.tsx throughout)
```

Durability: phase + attempts persist per spec; on store-wipe + rescan,
`rebuild_phase_from_decisions` restores the current phase from `decisions.md` +
`feature_list.json` (no progress lost).

---

## Important Decisions

- **Thin phase machine *above* the feature machine.** Roles gate the whole spec; features gate
  one checkbox — different granularity. Keeping the phase layer separate and above means
  `FeatureState` and `drive_inner` are untouched (regression-safe), and the two machines never
  fight over one record.
- **Verdict-by-text, not by script.** A role's judgment ("is this spec sound?") has no
  `exit 0`. The role-agent emits a parseable `HARNESS_GATE:` line; the engine treats anything
  not-explicitly-PASS as a retry. This keeps the false-green guarantee the feature gate already
  enforces.
- **Autonomy is the default and supersedes 010.** `hitl_on_block` defaults `false`: gates
  auto-advance with no prompt, and an exhausted gate `Blocks` rather than waiting for a human —
  including at QA. 010's "pause for ONE human confirmation at QA" is now opt-in. (User decision,
  2026-06-18.)
- **Reuse over rebuild.** `spawn_agent_into_pane`, settle detection, `max_retries`,
  `AwaitingConfirm`/`confirm_feature`, `append_decision`, and the event bus are all reused; the
  net-new surface is the phase enum, the sequencer, the verdict parser, and two event variants.
- **Central, read-only role prompts.** Role playbooks stay in `ai/roles/*.md` and are *read*,
  not forked into the repo (010 §28). The engine consumes them; the user's cross-project SDD
  skills are unaffected.

---

## Risks

- **Settle detection for role-agents** — a PM/architect/reviewer agent may not signal
  `awaiting_input` the same way a coding agent does. Mitigation: reuse the grace-window +
  first-lifecycle-event logic, fall back to `settle_timeout_secs`, and key completion on the
  verdict line being present in the pane tail.
- **Verdict-parse fragility** — agents may format the line loosely. Mitigation: tolerant scan
  (case-insensitive, last-match, allow surrounding prose), but bias to `Concerns` on any
  ambiguity — never to PASS.
- **Phase/feature concurrency** — only one phase is active per spec, and `Executing` owns the
  WIP=1 feature loop; the sequencer must not let a later phase start while `drive_inner` runs.
  Single-threaded phase progression per run guards this.
- **Rebuild fidelity** — `decisions.md` must carry enough structure for
  `rebuild_phase_from_decisions` to be unambiguous (one canonical "entered <phase>" marker per
  transition).
- **Autonomy hides a lenient gate** — no human in the loop means a weak PM/architect agent could
  wave a bad spec through. Mitigated by committed gate artifacts + logged verdicts for *after*
  the-fact audit; GAN-style evaluator gates (deferred) harden it later.
