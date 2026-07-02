# Handoff 02 — Architect → Developer

- **Spec:** 005-one-shot-issue-loop
- **Date:** 2026-07-02
- **From:** Architect (autonomous /sdd-loop iteration 2)
- **To:** Developer
- **Artifact:** `ai/specs/005-one-shot-issue-loop/architecture.md` (gate PASSED 5/5; cites line-verified on worktree commit `7e9afaa4`)

## Gate result

Architect gate: **PASS** — concrete boundaries + seam signatures per feature;
tradeoffs with rejected alternatives (route naming, stop-vs-refresh for stale
runs, register-after-plan ordering); invariants addressed (one launch path,
gate sacred, best-effort tracker, registry serde hazard untouched-by-
construction, double-driver); per-feature unit-test plans with named tests;
spec contradictions surfaced as corrections C1–C5. Orchestrator spot-verified
the load-bearing seams independently (`claim_driver` harness.rs:440,
`release_driver` :452, `stop` :503, `MCP_ENABLED_SETTING` mcp.rs:68 + GET/PUT
precedent, `resolve_qa_mode` drive.rs:407 — takes `&HarnessConfig` today,
`linear_get/set_state_map` desktop linear.rs:468/482, `setting_get/set_bool`
store settings.rs:37/55, `ORCHESTRATION_ENABLED_SETTING` orchestration.rs:22).

## Corrections the developer MUST honor (C1–C5, full text in architecture.md §1)

- **C1:** pre-registration failures = HTTP error → composer toast (no
  pseudo-HarnessEvents with nil ids); post-registration failures ride the
  existing drive error path. Workspace never rolled back.
- **C2:** do NOT add an InProgress transition to the start-work seam —
  `drive_inner` already fires it at spawn (drive.rs:126-135). AC 1 is
  satisfied by wiring INTO the engine.
- **C3:** the F3 knob is a SQLite store setting
  (`harness.qa.agent_browser.enabled`) behind `GET/PUT /api/harness/settings`,
  NOT a json file. F5 keeps `github.json` (D4 locks it).
- **C4:** the `spec_id` stamp lands in `plan_from_spec_inner` — deliberately
  widens the MCP `agentum_harness_plan` output too; update the stale
  `types.rs:926` comment; spec-013 role gates unaffected (`roles` stays false).
- **C5:** start-work is serialized by an engine-level `start_work_lock`; the
  already-running check precedes ALL filesystem mutation; stale-idle runs are
  stopped + re-registered, never refreshed in place.

## Build order (F1→F5; F2/F4/F5 independently shippable)

Per architecture.md §8. Write the pins FIRST:
`feature_prompt_without_spec_is_byte_identical` (F2), then
`resolve_qa_mode_matrix` (F3). Each feature lands as one gated slice:
`cargo fmt --all` + `cargo test -p agentum-server --lib` +
`npm run build --prefix crates/agentum-desktop/ui` (and
`cargo build -p agentum-desktop` for the F5 Tauri commands) green before the
next feature starts. Update `tasks.md` per slice with deviations.

## Repo rules that bit previous specs (do not relearn)

- Dedicated worktree = this one (`.claude/worktrees/finish-the-loop`, branch
  `finish-the-loop` on develop tip). Stage only your files; never `git add -A`.
- `cargo fmt --all` before committing (hand-formatting reddens CI).
- Tauri commands take FLAT named params — a `request: Struct` param silently
  rejects the invoke (F5's two new commands).
- Tests touching user paths isolate via `AGENTUM_HOME`/`AGENTUM_GITHUB_CONFIG`
  (tempdir), taking the env lock if mutating env; prefer the pure
  `apply_layers`-style injection over env mutation.
- vitest suites that import xterm fail to load (pre-existing noise) — colocate
  pure helpers under `lib/` and test those.
- The registry `Worktree` struct must stay serde-alias-FREE (wipe hazard) —
  005 touches no registry code by design; keep it that way.

## Expected developer artifact

Code + tests per architecture.md §2–§6, `tasks.md` (per-feature checklist with
deviations logged), all gates green, committed to this worktree's branch in
per-feature (or per-slice) commits.
