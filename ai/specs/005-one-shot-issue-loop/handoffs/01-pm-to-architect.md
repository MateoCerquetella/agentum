# Handoff 01 — PM → Architect

- **Spec:** 005-one-shot-issue-loop
- **Date:** 2026-07-02
- **From:** PM (autonomous /sdd-loop iteration 1)
- **To:** Architect
- **Artifact:** `ai/specs/005-one-shot-issue-loop/spec.md` (PM-gated; decisions D1–D4 locked)

## Gate result

PM gate (`ai/skills/validate_handoff.md`): **PASS** — all items green after
eight applied edits. Load-bearing cites spot-verified against the tree
(develop tip `1e259604`); three drifted cites fixed (`resolve_qa_mode` is in
`harness/drive.rs:407-423` not harness.rs; Todo-at-plan precedent is
`board_goals.rs:604-616`; post-create draft-open is
`lib/open-created-workspace.ts:52-66`, WorkspaceAgentLauncher is the picker
mirror).

## Decisions locked (see spec "Decisions (PM-locked)")

D1 server-side start-work orchestration, one route, one failure surface.
D2 adoption not co-existence — composer agent/model become the engine's
`FeatureList` knobs; ALL THREE plain-delivery paths skipped (draft-open,
stash fallback, issueCommand launch). D3 QA capability stays opt-in — the new
Settings knob is a second opt-in door, default OFF (overrides the draft's
AC 8 leaning; evidence: default-capable would convert the non-web Script
skip-pass into a fail-closed QA agent gate). D4 global `github.json`
`state_map` mirroring `linear.json`; per-repo is a named follow-up.

## Material PM findings

1. **AC 6 was untestable as drafted:** the written backlog's `spec_id` is
   always `None` — `derive_backlog_from_spec` returns
   `..FeatureList::default()` (`harness/types.rs:895-898`), and
   `plan_from_spec_inner` (`:927-957`) never stamps it. AC 6 now requires the
   spec-from-issue plan step to stamp `FeatureList.spec_id`, and pins two
   byte-identical cases (no `spec_id`; explicit `feature.prompt` override,
   `helpers.rs:34-36`).
2. **AC 2 had two uncovered spawn side-doors:** besides the draft-open, the
   no-agent `stashPendingSessionPrompt` fallback and the `issueCommand`
   automation launch (`worktree-activation.ts:413-423`) can each put a second
   agent in the worktree. All three are now named skips.
3. **AC 1 double-scaffold collision:** `spec_from_issue` 400s on an existing
   spec (`routes/harness.rs:247-251`); the 004 D5 toggle fires first at
   `useComposerState.ts:2214`, and retries re-enter. AC 1 now requires
   convergence (plan from the existing spec), not an error.
4. **AC 9 argument shape:** `apply_tracker_transition` needs the provider's
   stable handle (`tracker_id`) — Linear/board arms can't work from a URL.
   Tool input is now `{provider, id, url?, phase}`.
5. **Knob-threading reality:** `FeatureList` has the knobs (`types.rs:115-158`)
   and `copy_knobs_from` (`:211-239`) shows the preserve pattern, but no
   existing seam writes them post-plan — the start-work seam owns that write.

## What to blueprint (build order F1→F5; F2/F4/F5 are independently shippable)

1. **F1 start-work orchestration (headline, riskiest):** route naming per D1
   (`POST /api/harness/start-work` vs `/api/workflows/*`); sequence
   scaffold+plan (existing handler logic, converging on existing spec) →
   `Todo` transition → post-plan knob write (`spec_id`, `agent_tool`,
   `agent_model`) → `HarnessEngine::start` (`harness.rs:75`) →
   run (`claim_driver` `:440` — surface the already-running case as a friendly
   state). UI: composer "Start gated run" + the three-path skip (D2) +
   Tasks-page row action via `openComposerForItem` (`TaskPage.tsx:2349`).
2. **F2 spec-aware prompt:** `spec_id` stamp in the plan step +
   `build_feature_prompt` widening; two byte-identical pins.
3. **F3 QA via agentum_browser:** `build_qa_prompt` rewrite
   (`helpers.rs:141-167`) + `resolve_qa_mode` knob (D3) — note the "embedded"
   fact lives in AppState, not `HarnessConfig`, so the function signature
   likely widens; design the knob's persistence (Settings-writable file, like
   `linear.json`).
4. **F4 `agentum_report_status`:** thin MCP arm over `apply_tracker_transition`
   (`{provider, id, url?, phase}`), never-`Err`, delegation-pinned like the
   other tools (`routes/mcp.rs:624-637` dispatch).
5. **F5 `GithubStateMap`:** mirror `LinearStateMap` (`linear.rs:182-257`);
   decide the color story for custom names (the fixed-color tuples in
   `GITHUB_STATUS_LABELS`, `task_sink.rs:247-252`, assume canonical names);
   exactly-one-configured-label invariant incl. map-changed-mid-flight.

## Expected architect artifact

`ai/specs/005-one-shot-issue-loop/architecture.md` — boundaries, seam
signatures, tradeoffs, risks, per-feature build/test plan (matching spec 004's
`architecture.md` shape).
