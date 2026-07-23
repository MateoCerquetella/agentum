# Handoff — PM to Architect

- **Spec:** 028-bound-transcript-observers
- **From:** PM
- **To:** Architect
- **Date:** 2026-07-23
- **Gate:** PASS

## Delivered

- One performance slice tying user-visible degradation to unbounded historical Claude transcript
  observation, with local tmux fleet sampling explicitly deferred to Spec 029.
- Eight observable criteria covering side-effect-free listing, live versus historical reads,
  non-Claude behavior, reset-first semantics, lifecycle retirement, transcript compatibility, and
  bounded consumer shutdown.

## Acceptance-criteria evidence

- **AC 1–4:** Session-list and agent-task route outcomes name exact entry, directory, observer, and
  response expectations for historical, running, and non-Claude sessions.
- **AC 5–6:** Reset, stop, kill, delete, crash, and tool-change lifecycle behavior is explicit and
  testable before implementation.
- **AC 7–8:** Existing transcript isolation and wire contracts are protected while the notify
  transport and task lifetime become bounded.

## Verification

- `ai/skills/validate_handoff.md` — PASS (9/9 checklist items).
- `git diff --check` — PASS.

## Decisions and invariants

- Only a currently running Claude session may own a live observer.
- Historical reads synchronously refresh cached state and never trade correctness for observer
  avoidance.
- Reset is effective even as the first transcript interaction.
- Public HTTP schemas, event names/payloads, UUID pinning, and legacy fallback remain unchanged.

## Remaining risks / next action

- Architect must pin an atomic entry/observer attach seam and a one-way watchdog retirement hook
  that avoids a server/watchdog dependency cycle and cannot start observation.
