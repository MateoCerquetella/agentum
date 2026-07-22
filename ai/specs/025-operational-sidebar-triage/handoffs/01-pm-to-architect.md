# Handoff — PM to Architect

- **Spec:** 025-operational-sidebar-triage
- **From:** PM
- **To:** Architect
- **Date:** 2026-07-22
- **Gate:** PASS

## Delivered

- PM-locked product contract in `spec.md` for one operational-triage sidebar slice.
- Nine observable acceptance criteria covering classification truth, composed filtering,
  rich/compact presentation, interaction parity, preference safety, accessibility, and gates.
- Explicit desktop-only scope, non-goals, existing-code reuse, risks, and harness wiring.

## Acceptance-criteria evidence

- **AC 1–2:** the spec defines mutually exclusive, precedence-ordered Needs You / Active /
  Settled sections grounded in `smart-attention.ts` and `worktree-agent-activity-summary.ts`.
- **AC 3–5:** search, project scoping, rich cards, and settled disclosure have observable
  filtering, count, ordering, truncation, and expansion outcomes.
- **AC 6–7:** existing workspace interaction behavior and persisted grouping choices are
  explicitly invariant.
- **AC 8–9:** supported widths, themes, keyboard order, contrast, focused tests, and the Vite
  production build have named verification outcomes.

## Verification

- `ai/skills/validate_handoff.md` checklist — PASS (9/9 items).
- `git diff --check` — PASS.

## Decisions and invariants

- This is one slice because every element supports the single operator action of triaging the
  next workspace from the left sidebar.
- Operational grouping becomes the default only when no preference exists; explicit persisted
  choices and alternate grouping modes remain untouched.
- Existing push-fed status detection and workspace interactions stay authoritative; no backend,
  polling, launch-path, or persistence-schema work is allowed.

## Remaining risks / next action

- Architect must define one shared interaction boundary for rich and compact rows and prove that
  variable-height operational rows preserve `WorktreeList` virtualization and reveal anchoring.
