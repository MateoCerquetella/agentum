# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 008-finish-the-loop
- **phase:** developer   <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (008 **F1 CODE-COMPLETE + GREEN** 2026-07-03, `tasks.md`; server 546/0/5 + executor 21/0 + vitest 14/0, fmt+clippy clean, 3 live-test binaries compile. Four silences closed: #15 SettleOutcome / #16 status/blocked / #14a readiness-bool (sacred, D5 human-live-test merge gate) / #2 armed-!repoId toast. **Phase STAYS developer** — F2 (chat Fast/Complex) + F3 (goal-first workspace) are the next developer iterations; advance to tester only when all three slices are code-complete. 007 RELEASED v0.55.0; 006 RELEASED v0.54.0)
- **mode:** auto         <!-- HITL (human in the loop) | auto -->  (set by /sdd-loop 2026-07-01; NEEDS-HUMAN exit is the safety valve; RELEASE stays human-gated)
- **execution:** harness <!-- features land via the .harness/ engine + green gate -->

## Active send-backs

- **003-chat-issue-preview** — CODE COMPLETE + SHIPPED to develop (issue **#198**,
  PR **#199**, `feat/chat-board-revamp`). All 4 increments gated. ⏭ Browser QA at
  STAGING + tagged release = Mateo-gated. [Merged into this worktree 2026-07-01;
  note it added `NewFeature.labels` + `gh --label` to `task_sink.rs`/`chat.rs` —
  spec 004's cited line numbers may have drifted.] Roadmap asks: specs
  Kanban-read / status write-back / projects-first (numbers now shifted by 004).
- **001-autowiki** — COMMITTED (`3a8dbf06`) + **PR #183** (OPEN, into develop),
  issue #182. Browser QA pending downstream (staging). [Local autowiki worktree was
  lost to an env reset; work is safe on `origin/feat/autowiki`.]
- **002-start-loads-spec** — Drafted + PM-gated. Finding: Chat issue-creation
  ALREADY sets title+body+external-only on develop (`chat.rs:914/1050`); the real
  gap is the **Start** side (`build_card_prompt`, `board_goals.rs:861`, uses card
  columns — never the issue/spec; Start is internal-board-coupled). Scope LOCKED:
  Start-only, external-ticket-direct (no card), live body fetch. **Architect DONE**
  (`architecture.md`). ⛔ **R1 needs Mateo:** the spec's Path A (board-card Start) is
  DEAD (no UI caller); the live "start a ticket" flow is Path B (Tasks "Use" → local
  PTY; gets Linear body, not GitHub). Option A (new server "Start", spec-faithful) vs
  Option B (fix "Use", local-PTY). Developer phase gated on R1. ✅ **R1 → Option B; Developer DONE** (`e0faf420`):
  `routes/github.rs` `GET /api/github/issue` + UI fetch → linkedContext → prompt;
  npm build + cargo test (453/0) green; AC-3 verified. NOT runtime/browser-verified.
  Release = human-gated.
- **004-workspace-issue-loop** — Drafted + PM-gated (2026-07-01). Three increments:
  (A) composer "Create GitHub issue" + worktree registry persists linked metadata
  (`worktrees.rs:249/351` drops it today); (B) real GitHub arm for
  `apply_tracker_transition` (`task_sink.rs` no-op arm; drive.rs call sites already
  correct); (C) issue→`.agentum-harness/specs/<id>/spec.md` scaffold over an HTTP
  seam (helpers exist, MCP-only today). ✅ PM-locked D1–D5 (Done=label-only, gh CLI
  writes, `status/*` canon labels, one spec built status-sync-first, scaffold
  opt-in/off); AC 4 softened — thread `feature.tracker_url` through the seam.
  ✅ **Architect DONE** (`architecture.md`, line-verified pre-merge): widened
  `apply_tracker_transition(…, tracker_url: Option<&str>, …)` serving BOTH callers
  (drive.rs + board_goals initial-Todo); URL-authoritative slug+number; pure gh
  argv builders + fake-gh subprocess tests; C1 Tasks-page create = local STUB →
  F3 = new `POST /api/github/issues`; C2 two UI client layers strip linked fields;
  C3 `linkedPR`/`linkedPr` wire fix, NO registry-struct alias (wipe hazard);
  C4 remove only the 3 canonical labels (never `status/qa*`); C5 direct local gh
  from `neutral_cwd` (no Host in seam). F4 = `POST /api/harness/spec-from-issue`,
  keep-existing spec semantics, `plan_from_spec_with_tracker`. ⚠️ 35 develop
  commits merged in AFTER line verification — re-locate lines before editing.
  Handoffs: `01-pm-to-architect.md`, `02-architect-to-developer.md`. Phase →
  developer (build F1→F4, one gated slice each). ✅ **F1+F2 GREEN** (slice 1,
  `85c48e0d`). ✅ **F3+F4 GREEN** (slice 2): `POST /api/github/issues` +
  composer create-issue affordance (chip renders pre-worktree);
  `fetch_github_issue` + `spec_md_from_issue`/`issue_spec_id` +
  `POST /api/harness/spec-from-issue` (never-overwrite) + `scaffoldSpec`
  toggle (OFF default) in both submit paths; `plan_from_spec_with_tracker`
  stamps provider+url (MCP `plan_from_spec` unchanged). Gate: 494/0 lib tests,
  fmt+check clean, vite build ✓ 1m04s. **Developer phase COMPLETE → tester**
  (handoff `03-developer-to-tester.md`; browser QA of chip/toggle/live-label =
  qa.sh/staging, not the tester phase).

## Decision log

<!-- append one line per decision, newest last: `YYYY-MM-DD — <decision>`; keep only the last 5 (older history lives in git) -->
- 2026-07-02 | Reviewer | **006 SIGN-OFF → SHIP-READY** (`review.md`). All 6
  focus items pass (four pins re-verified character-level; deviation-2 fix
  confirmed COMPLETE — createIssuesFromChat is the only plan-constructing
  caller; C1 one-hunk discipline + documented first-pair invariant; opposite
  defaults named in read_settings' doc; brief deltas verbatim). 0 Blockers,
  0 new Should-fix; 4 nits (stale gh_bin doc contradicted by the two
  env-locked tests; parse_label_names comment overstates stable-sort dedup;
  empty-label-repo note for staging QA; refs-not-state rationale lives only
  in a comment) — Nits 1-2 fold into #226's docs pass. "The riskiest
  correction (C4) was caught landing at the wrong seam and fixed at the real
  one with the architecture's error documented rather than papered over."
  spec.md Status → Done. Phase → done. **RELEASE = Mateo.**
- 2026-07-03 | Analyst | **008-finish-the-loop drafted + PM-gated** (Socratic
  five-pass interview with Mateo). Problem: the core loop breaks at the last
  step — Start gated run on a GitHub issue opens no session / inert agent,
  silently. Locked: WHO = solo dogfooder (3 pain moments); WHAT = hands-off
  issue→green + Chat gets ⚡Fast/🧠Complex intake buttons (Complex = staged
  five-pass Socratic, one pass per turn — today's chat.rs:335 single-prompt
  "short Socratic" is NOT this) + goal-first workspace with optional steps;
  WHY = the loop IS the product + blocks demos; DONE = full pipeline demo in
  the INSTALLED release app (all 12 ACs); RISK#1 = silent regression → new
  live test for the issue→start-work→prompt-lands leg. Shape: one spec,
  slices F1→F3 (house style). Gate note: "one slice" checked as one outcome
  in 3 gateable increments, per Mateo's explicit shape decision. Phase → pm.
- 2026-07-03 | PM | **008 PM-gated → architect** (`spec.md`, handoff
  `01-pm-to-architect.md`). All 9 gate items PASS after edits; every code
  citation spot-verified. Locked D1–D9: interview state client-side/server
  stateless (D1); Complex mode same model, no forced thinking (D2); goal-first
  = parallel default, composer not deleted (D3); Fast/Complex per-feature, no
  sticky (D4); F1 may instrument drive.rs but the 3 autonomy mechanics change
  only with BOTH live tests green + no new spawn path (D5); `status/blocked`
  joins the 004 label canon (D6); AC 1/2 numbers are demo-project pins (D7);
  "persisted spec" = existing `spec_md_from_issue` round-trip, no chat-time
  file write (D8); F3 optional-repo = worktree optional, workdir required (D9).
  Rewrote AC 1/2/7/12 from untestable to observable (2s ack / 15s session /
  60s pane output / one-pass-per-turn / Mateo-run installed demo w/ label-trail
  evidence). Key finding: Start-gated-run is a TWO-HOP UI path
  (TaskPage→pre-armed composer→startGatedWork), not one button — never-silent
  must span both hops. Phase → architect.
- 2026-07-03 | Architect | **008 Architect-gated → developer**
  (`architecture.md`, seams line-verified on `0e6812f8`; handoff
  `02-architect-to-developer.md`). **A1 framing correction: the start-work path
  already shipped in spec 005** — F1 is instrumentation + the D6 blocked
  escalation + a live test, NOT new plumbing. Four real silences to close:
  #15 `wait_for_settle` 1800s silent hang → `SettleOutcome` (do first, not
  sacred); #16 blocked gate → no issue escalation → `apply_blocked_transition`
  (D6); #14a `await_repl_ready` falls through → prompt fires blind (AC 2) →
  readiness bool (F-FLAG, sacred, gated on BOTH live tests); #2 composer armed
  `!repoId` guard silent (#226 edge) → toast. Decisions: D-A `status/blocked` =
  GitHub-only label sibling, `TrackerPhase` stays 4 variants; D-B explicit
  `{mode,stage}` on ChatRequest (server stateless); D-C thin `NewWorkspaceGoalStep`
  fronts the composer. New live test `harness_start_work_live.rs` covers the
  issue→route→session→prompt leg. Phase → developer.
- 2026-07-03 | Developer | **008 F1 CODE-COMPLETE + GREEN** (`tasks.md`; F1 only,
  F2/F3 deferred to next developer iterations). Built in architecture order:
  Step1 `wait_for_settle→SettleOutcome` loud-log ×4 sites (#15 1800s hang);
  Step2 `apply_blocked_transition`+`status/blocked` GitHub-only label, TrackerPhase
  stays 4 (D-A), remove-set widened to 5-minus-target, `record_feature_failure`→
  (blocked,attempts) (#16); Step4 pure `start-gated-run-precondition`/`composer-modal-props`
  + armed-!repoId toast + server-error-detail + `subscribeHarnessRunErrors` bridge
  (#2 #226 edge, #5); Step5 `#[ignore]` `harness_start_work_live{,_roles}.rs` +
  `gh_in_dir` honors AGENTUM_GH_BIN; Step3 (sacred, LAST) `await_repl_ready→bool`
  + `inject_prompt→Result<bool>`, send-sequence BYTE-IDENTICAL, loud readiness log
  ×4 (#14a). Gates: server 546/0/5, executor 21/0, fmt+clippy clean, vite green,
  vitest 14/0. 4 documented deviations. ⚠️ Step3 D5 merge gate = the 2 live tests
  green is a HUMAN pre-release step (real claude, not CI-runnable). Phase STAYS
  developer → F2 next.
