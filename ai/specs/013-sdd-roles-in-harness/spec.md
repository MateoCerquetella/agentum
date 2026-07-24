# Spec: Autonomous SDD roles in the Harness Engine (013)

> **STATUS: READY (2026-06-18).** Full deliverable set complete — `spec.md` + `architecture.md`
> + `tasks.md` + `contracts/gate-protocol.md` (per `ai/specs/README.md`). Awaiting user sign-off,
> then implementation.
>
> Executes the **unbuilt half of 010** — the role-gate lifecycle (010 §Requirements "Unified
> state model", line 36) that was specified but never implemented (roles remain decorative in
> `STATE.md`; the engine only tracks *feature* states). **Revises** 010's "pause for ONE human
> confirmation at QA" default: per user decision the loop is **fully autonomous — zero human
> prompts between roles or at QA**. A human stop is a non-default opt-in safety net, never a
> routine checkpoint.

## Goal

Make the SDD role sequence (PM → Architect → Developer → Reviewer) **real, server-tracked
phases of the Harness Engine** that advance **automatically, with no prompt to continue**. The
existing feature loop (`pending → coding → verifying → done`) becomes the **Developer phase**;
new **agent-played role gates** wrap it on either side. Authoring a spec and running it to
green is **one unattended lifecycle on one board**, not a prompt-only `ai/` flow the server
can't see.

---

## User Value

**In one line:** open a spec, walk away — PM clarifies it, the architect validates it, the
harness builds and verifies each feature, the reviewer signs it off, all without stopping to
ask you anything.

Today the merge is ~70% plumbed (`harness.rs:49` resolves `.agentum-harness/`; `plan_from_spec`
at `harness.rs:630` turns a spec's acceptance-criteria checkboxes into `feature_list.json`),
but the **authoring/role half is invisible to the server** — it lives as Claude Code prompts +
a hand-edited `STATE.md`, and role gates never gate anything. The cost of leaving it: the
"thinking" half (the *why*) and the "doing" half (the *proof*) are two mental models, two
sources of truth (`STATE.md` vs `decisions.md`), and the role sequence is decorative. Persona:
the **self-hoster** who wants a spec to drive itself end-to-end while the lid is closed (the
core agentum promise) — extended from "agents keep running" to "the whole SDD loop keeps
running."

---

## Requirements

**Phase machine (the new layer)**
- **A run carries a `phase` above its feature backlog:** `authoring → architecture → decompose
  → executing → review → done` — server-owned, one enum in `harness.rs`, persisted per spec
  (alongside `feature_list.json`, not a separate global file).
- **The existing feature loop *is* the Developer phase.** `executing` runs today's
  `pending→coding→verifying→done` WIP=1 loop **unchanged**; the new phases sit before/after it.
  No regression to the feature engine.
- **`decompose` is automatic, agentless:** the lifecycle calls the existing `plan_from_spec`
  (`harness.rs:630`) to derive the backlog — absorbing the old manual `agentum_harness_plan`
  MCP step. The harness never sits idle waiting for a hand-call.

**Agent-played role gates (auto-advance, no prompt)**
- **Each role phase spawns a role-agent** via the *same* `spawn_agent_into_pane` helper the
  feature loop uses (YOLO, loopback `pane_env`, MCP wiring stay centralized), prompted with the
  central role playbook (`ai/roles/pm.md` / `architect.md` / `reviewer.md`) — **delivered as a
  prompt/MCP context, never copied into the repo**.
- **A gate passes by artifact + verdict, not a script.** The role-agent must produce its gate
  artifact (PM → refined `spec.md` with acceptance-criteria checkboxes; Architect →
  `architecture.md`; Reviewer → review notes) **and** a machine-readable final line
  `HARNESS_GATE: PASS` or `HARNESS_GATE: CONCERNS <reason>`. The engine parses it.
- **Auto-advance is mandatory; there is no "continue?" prompt.** `PASS` → advance + append to
  `decisions.md`. `CONCERNS`/missing-verdict → retry the role-agent up to `max_retries` (the
  existing per-feature counter, reused). Retries exhausted → `Blocked` with the reason.
- **Human stop is opt-in, never default.** The `AwaitingConfirm` machinery is repurposed as a
  **non-default** safety net: only when a run sets `hitl_on_block: true` does an exhausted gate
  pause for a human instead of blocking. Default runs are fully unattended **including QA** —
  this **supersedes 010's HITL-at-QA default**.

**State, observability, continuity**
- **One decision log, not two.** Every phase transition (which role ran, verdict, attempt
  count, artifact ref) auto-appends to `.agentum-harness/decisions.md` (`append_decision`,
  `harness.rs:693`). `STATE.md` is **retired** — its role/phase pointer is now derived from the
  engine and rendered read-only if needed.
- **New `HarnessEvent` variants** — `PhaseChanged { from, to }` and `GateResult { role,
  verdict, attempt }` — on the existing `/api/harness/events` WS, so the board reflects role
  progress live.
- **Rebuildable:** clearing agentum's store and rescanning `.agentum-harness/` restores the
  current phase from `decisions.md` + `feature_list.json` (no progress lost), consistent with
  010b's per-spec durable model.

**Surface (minimal this round)**
- **`HarnessEngine.tsx` grows a phase strip** above the feature board showing
  authoring → architecture → executing → review → done with the active phase highlighted and
  the latest gate verdict. No new page; the dedicated end-to-end UI flow stays in 010/C scope.

---

## Acceptance Criteria

- [ ] A `SpecPhase` enum (`authoring/architecture/decompose/executing/review/done`) exists in `harness.rs`, is persisted per spec, and is unit-tested through a full advance with stub role-agents
- [ ] `executing` delegates to the **existing** feature loop unchanged — a feature-only run (no role phases configured) behaves exactly as today (regression test green)
- [ ] The lifecycle calls `plan_from_spec` automatically at `decompose`; a run started from a spec with acceptance-criteria checkboxes populates `feature_list.json` with **no** manual `agentum_harness_plan` call
- [ ] A role gate spawns its agent through `spawn_agent_into_pane` and advances on a parsed `HARNESS_GATE: PASS` line — verified by a `#[cfg(test)]` test with a stub agent that emits PASS
- [ ] A role gate that emits `CONCERNS` (or no verdict) retries up to `max_retries`, then transitions to `Blocked` — **never** prompts a human on a default run (autonomy assertion)
- [ ] With `hitl_on_block: false` (default), a full spec advances authoring → … → done with **zero** human-confirm pauses, including at QA — asserted end-to-end against a stub-agent fixture
- [ ] With `hitl_on_block: true`, an exhausted gate enters `AwaitingConfirm` and resumes via the existing `confirm_feature` path
- [ ] Every phase transition appends a structured entry (role, verdict, attempt, artifact ref) to `.agentum-harness/decisions.md`; `STATE.md` is no longer written by the engine
- [ ] `PhaseChanged` and `GateResult` events broadcast on `/api/harness/events`; a WS test observes the sequence for a full run
- [ ] Clearing the store and rescanning `.agentum-harness/` restores the run's current phase (rebuild test)
- [ ] `HarnessEngine.tsx` renders a phase strip with the active phase + latest verdict, fed by the new events (component renders without crash; manual desktop check)
- [ ] `cargo test -p agentum-server --lib` is green; no regression in existing harness tests

---

## Dependencies

- **Harness Engine** (`harness.rs`, `/api/harness/*`, `HarnessEngine.tsx`) — the execution +
  verify-gate + auto-state half this **wraps**. Reuses `spawn_agent_into_pane`, settle
  detection, `max_retries`, `AwaitingConfirm`/`confirm_feature`, `append_decision`, the event
  bus.
- **`plan_from_spec` / `derive_backlog_from_spec`** (`harness.rs:587,630`) — the spec→backlog
  bridge, now driven by the lifecycle at `decompose`.
- **Central role playbooks** (`ai/roles/*.md`, delivered via prompt/MCP) — the role-agent
  prompts; **not** copied per-repo (consistent with 010 §28).
- **Spec 010** (parent vision) — 013 completes 010c's role-gate merge and revises its
  HITL-at-QA default to autonomous.

---

## Risks

- **Two state machines layered** — spec-phase above feature-state. The granularity mismatch
  (roles gate the *whole spec*; features gate *one checkbox*) is the core design risk; keep the
  phase machine **thin and above**, never per-feature, so the existing engine is untouched.
- **Verdict parsing is softer than `exit 0`** — a role-agent could emit a malformed/absent
  verdict. Treat missing/unparseable as `CONCERNS` (retry), never as a silent PASS — false-green
  must be impossible for role gates too.
- **Autonomy hides a bad gate** — with no human stop, a too-lenient PM/architect agent could
  wave through a weak spec. Mitigation: gate artifacts are committed + logged (`decisions.md`),
  so a human can audit *after* without being *in* the loop; later, GAN-style evaluator gates
  (deferred) can harden this.
- **Role prompt drift vs the global skills** — `ai/roles/*.md` are also the user's
  cross-project SDD skills; the engine consuming them must read, not fork, them. Keep delivery
  read-only/central.
- **Scope creep into 010/C** — full desktop flow, retiring the `ai/` skill world, and
  evaluator gates are **out of scope**; this round is the engine phase-machine + minimal strip.

---

## Notes

- **Decision (user, 2026-06-18):** role gates are **agent-played and auto-advance with zero
  prompts to continue**; a human stop is opt-in (`hitl_on_block`), default off — including at
  QA. This **supersedes 010's "pause for ONE human confirmation at QA"** default.
- **Decision (user):** ship the **spec first, then the code** — this spec is the contract for
  the implementation that follows.
- **Why this is the real seam:** the plumbing (`.agentum-harness/`, `plan_from_spec`) already
  exists; what's missing is the server *knowing* about the authoring/role half at all. 013
  pulls it across the line into the same lifecycle the harness already owns.
- **Meta property:** this spec's own acceptance-criteria checkboxes are written to be
  `plan_from_spec`-derivable — 013 can be driven by the very engine it describes once built.
- **Out of scope:** dedicated end-to-end desktop UI flow; retiring the global `ai/` SDD skills;
  GAN-style evaluator gates; per-phase agent/model overrides (reuse the run's `agent_tool`/
  `agent_model` for all phases in v1).
