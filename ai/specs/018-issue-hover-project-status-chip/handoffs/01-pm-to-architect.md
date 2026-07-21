# Handoff 01 — PM → Architect (spec 018)

- **Spec:** `ai/specs/018-issue-hover-project-status-chip/spec.md`
- **Tracker:** https://github.com/MateoCerquetella/agentum/issues/365
- **PM gate:** PASS — all nine boxes (`ai/skills/validate_handoff.md`),
  self-run at draft 2026-07-14.

## Gate result

| Gate item | Verdict |
| --------- | ------- |
| One slice | PASS — a single chip; the SDD-loop half stayed in spec 016/358. |
| Problem before solution | PASS — "has to open GitHub to see the board column". |
| Persona named | PASS — Mateo scanning the sidebar. |
| Acceptance criteria testable | PASS — renders / renders-nothing / fetched-and-cached, all observable. |
| Non-goals stated | PASS — no editing, no Linear, no poll. |
| Grounded in code | PASS — binding route, `gh_projects.rs` GraphQL, hover card all cited @ develop `d31314b3`. |
| Invariants respected | PASS — server stays API-only; no poll; fetch-on-open + cache. |
| Harness wiring present | PASS — one feature entry, verify.sh + qa.sh defined. |
| STATE updated | PASS — current_spec 018, phase pm, decision line. |

## Notes for the architect

- Two open questions carried in the spec: **(Q1) read path** — desktop Tauri
  command vs server route (spec recommends desktop); **(Q2) binding source** —
  fresh cached `getProjectBinding` vs reuse Project Hub state. Pin both.
- Worktree was fast-forwarded from stale v0.57.0 main to develop `d31314b3`
  before drafting; citations are against that. Re-verify if develop moves.
