# Spec 358 — SDD loop stops on agent check-in over MCP

> Originated from GitHub issue https://github.com/MateoCerquetella/agentum/issues/358
> (title garbled/truncated; full ask supplied by Mateo in-session). Authored via
> `/sdd-spec` 2026-07-13; **narrowed to one slice at the PM gate 2026-07-13** —
> the issue-hover Project-status chip rider was split out to
> `.agentum-harness/specs/358b-issue-hover-project-status-chip/spec.md` (its own
> spec, pending its own PM gate). Full SDD document:
> `ai/specs/016-sdd-loop-checkin-and-issue-project-status/spec.md` — this file
> is the harness-facing mirror; the checkboxes below ARE the backlog
> (`derive_backlog_from_spec` makes one feature per checkbox, built in order).

## Problem

Mateo toggles the per-session SDD loop on an agent pane in the agentum desktop
app and walks away expecting autonomy. The loop re-injects the orchestrator
bootstrap on **every settle until a fixed 10-step cap** (`DEFAULT_MAX_STEPS`,
`routes/sdd.rs:60` on develop) because the server cannot know the work
finished — the completion signal lives only in the agent's reply text. A spec
that finishes on step 2 still receives up to 8 more "SDD loop step N" prompts;
when Mateo returns, the transcript is full of duplicate prompts and the agent
has re-declared completion or invented unrequested work. The session is
annoying to read and the agent ends up confused.

## User value

The SDD loop ends the moment the work is done — no duplicate step prompts, no
invented follow-on work — with the 10-step cap demoted to a safety backstop.

## Goal (one slice)

The SDD loop stops the moment the agent reports the work done, via a pull
check-in over a new MCP tool, instead of blindly re-injecting until the cap.

## Acceptance criteria

- [ ] New MCP tool `agentum_sdd_loop` accepts `{session, done, summary?}` and is registered beside `agentum_report_status` (available whenever MCP is on, NOT behind the orchestration gate): `done:true` on a session with an active loop removes the loop handle before the next injection and emits `sdd.loop.stopped` with reason `agent_completed`; `done:false` leaves the loop running and lands `summary` on the `sdd.loop.step` event payload; a check-in for a session with no active loop (or from a stale loop generation) returns success and stops nothing.
- [ ] `sdd::loop_step_prompt` (`sdd.rs:165` on develop) embeds the session id (explicit-id pattern, same as `agentum_report_status`) and instructs the agent to END every step by calling `agentum_sdd_loop` — `done:true` when the `ai/STATE.md` phase is done / nothing actionable, `done:false` otherwise — replacing the unread "reply briefly" instruction.
- [ ] `drive_sdd_loop` (`routes/sdd.rs:350` on develop) reads `ai/STATE.md` in the session workdir before each injection and stops the loop with reason `state_done` when the phase is done (belt for MCP-unwired tools like bash/aider); a missing or unparseable file falls through silently to today's behavior.
- [ ] A loop whose agent never checks in still stops exactly as today: `DEFAULT_MAX_STEPS` (`routes/sdd.rs:60`) and settle-timeout behavior stay byte-for-byte, and all existing stop reasons remain unchanged.
- [ ] Unit tests in `agentum-server` cover: stop-on-done before the next inject; `state_done` from `ai/STATE.md`; the step prompt contains the check-in instruction and the session id; a no-check-in loop still ends at `max_steps`.

## Non-goals (out of scope)

- The issue hover-card Project-status chip — split out to spec
  `358b-issue-hover-project-status-chip`; this spec touches no desktop UI.
- No change to the sacred autonomy mechanics: two-step `inject_prompt`, settle
  detection, YOLO handling. The check-in is purely additive.
- No change to the `DEFAULT_MAX_STEPS` value or settle-timeout defaults.
- No parsing of completion out of the agent's reply text — the MCP check-in
  and the `ai/STATE.md` belt are the only new stop signals.
- No TUI work (the TUI lives in the separate `agentum-tui` repo).

## Constraints / invariants

- Loop-side failures (tool errors, `ai/STATE.md` read errors) never crash the
  worker — always fall through to today's behavior.
- A stale generation's check-in must not stop a successor loop on the same
  session.
- **Branch from fresh `origin/develop`** — this worktree's checkout is
  v0.57.0-era and does not contain `routes/sdd.rs`. Code citations are against
  `origin/develop` @ `bee8dc2d`.

## Verification (the gate)

- `verify.sh`: `cargo test -p agentum-server --lib` (including the four new
  loop tests) + `cargo fmt --check`.
- `qa.sh`: run the SDD loop on an already-done spec — the loop stops after
  step 1 with reason `agent_completed` and no second injected prompt appears
  in the pane.

## Open questions

1. Tool shape: dedicated `agentum_sdd_loop` (recommended) vs. an op on
   `agentum_report_status` — architect to pin.
