# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 395-sdd-live-status-strip
- **phase:** done         <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (Issue #395 status-strip interpretation CODE-COMPLETE + GREEN 2026-07-20: live phase and executing n/N progress on feature, QA, and role sessions; ordinary sessions excluded; event-driven refresh. Focused Vitest 4/4 and Vite production build green. Earlier `009-sdd-loop-agent-injection` draft marked Superseded as an alternate issue-title interpretation.)
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
- 2026-07-03 | Developer | **008 F2 CODE-COMPLETE + GREEN** (chat Fast/Complex
  intake, AC 5–8; tasks.md F2 section). Server (`chat.rs`): `IntakeMode{Fast,
  Socratic}` + `{mode,stage}` serde-default on ChatRequest; `intake_grounding_blocks`
  extracted VERBATIM (Fast byte-identical, pinned); `build_intake_instructions`
  router; `socratic_stage_instructions`+`socratic_pass_body` (5 passes WHO/WHAT/
  WHY/done/risks, reflect-back, stage5→"Preview issues"); `chat_auth_gate` shared
  no-creds gate (Complex surfaces NO_CREDS by construction). Client: pure
  `lib/socratic-intake.ts` reducer (one pass/turn, cap5, Fast never advances) +
  localStorage `Conversation.intake` (D1 no new table) + two ChatPage buttons +
  Enter-stays-Fast. Gates: server 552/0/5 (F1 546 held), vitest 10/0 new, fmt+
  clippy clean, vite green; full vitest 139-fail = PROVEN pre-existing baseline
  (0 new). Invariants held: interviewer_instructions byte-identical, compose_issue_body
  untouched (D8), stateless (D1), no forced thinking (D2), no sticky (D4), F1/F3
  surfaces untouched. Phase STAYS developer → **F3 (goal-first workspace) LAST**.
- 2026-07-03 | Developer | **008 F3 CODE-COMPLETE → SPEC DONE, phase → tester**
  (goal-first workspace, AC 9–11; tasks.md F3 section; handoff
  `03-developer-to-tester.md`). New pure `lib/workspace-goal-step.ts`
  (deriveWorkspaceGoalSeed/isGoalStepReady/firstGoalStepBlocker/
  OPTIONAL_WORKSPACE_STEPS/shouldStartAtGoalStep/revealDetails) + thin
  `NewWorkspaceGoalStep.tsx` (goal textarea + reused RepoCombobox workdir) +
  modal renders it as default first screen, "Skip to details" → today's composer
  (D3); goal+workdir required, worktree/scaffold/tracker skippable (D9);
  `useComposerState` NEVER edited (props only), F1's initialStartGatedRunProp
  intact. Gates: vite+tsc green, `workspace-goal-step.test.ts` 15/0, F1+F2+F3
  pure suites 34/0 (F1/F2 held), full vitest +15 passing 0 new (139 baseline).
  AC 11 full run = qa.sh/human. 3 deviations documented. **F1+F2+F3 all
  code-complete → tester.**
- 2026-07-03 | Tester | **008 verdict PASS-WITH-DEFERRALS** (`verification.md`,
  HEAD `9423b86f`; handoff `04-tester-to-reviewer.md`). Independently re-ran
  every gate — server 552/0/5, executor 21/0, fmt+clippy clean, 3 live binaries
  compile #[ignore], vite green, spec-008 vitest 34/0; full vitest 139-fail
  baseline corroborated PRE-EXISTING via 4 methods (disjoint set / no-reference
  grep / diff-scope / failure-kind). All 11 deviations ACCURATE against code;
  sacred surfaces clean (inject_prompt send-sequence byte-identical, await_repl_ready
  poll logic unchanged, apply_blocked_transition Ok-Skipped-never-Err, compose_issue_body
  + useComposerState internals untouched, F1 initialStartGatedRunProp preserved).
  NO defect, no AC FAIL. AC 5–10 PASS now; AC 1–4/11 PASS(deferred qa.sh+D5 live);
  AC 12 PASS(deferred Mateo installed demo). 4 Info nits (top: "tsc"=vite-transpile
  +vitest not full tsc; 139 baseline real+out-of-scope). Phase → reviewer.
- 2026-07-03 | Reviewer | **008 SIGN-OFF → SHIP-READY** (`review.md`, HEAD
  `9d9be973`, 0 blockers). All 18 focus items PASS w/ quoted evidence: both D5
  sacred mechanics behavior-preserving line-by-line (`inject_prompt` send-sequence
  + `await_repl_ready` poll/trust unchanged, only return type); `apply_blocked_transition`
  never-`Err` + honest 5-name remove-set (board can't lie either direction); no
  D6 shell injection (argv exec); Fast byte-identical (construction + pin); live
  test asserts the REAL leg (MARKER in pane = prompt landed, not hollow); F3
  preserves F1 Tasks hop; no new `is_public` holes; D1–D9 honored. 1 Should-fix
  = project-wide CI typecheck (vite≠full tsc), NOT a 008 defect → follow-up
  ticket. 3 leave-as-is nits. spec.md Status → Done. Phase → done. **RELEASE =
  HUMAN** (promote + D5 live tests + qa.sh + AC-12 installed demo).
- 2026-07-20 | Spec | **009 DRAFTED, THEN SUPERSEDED BY RUN AUTHORING** (`/sdd-spec`
  direct ask = issue **#395** "SDD-loop is not injecting in other agents";
  `tracker:` frontmatter set). Root cause grounded in code: `await_repl_ready`
  (`drive.rs:967`) matches Claude-only trust/footer strings → every non-Claude
  role prompt fires BLIND after ~56 s (`inject_prompt` :1037; call sites
  :162/:361/:611/:750); the desktop's per-agent delivery knowledge
  (`tui-agent-config.ts`) never reaches the server path. Scope LOCKED: per-tool
  readiness+trust via the adapter layer (`agentum-executor`, principle 5),
  Claude sacred sequence BYTE-IDENTICAL (008 D5 live-test gate), per-dispatch
  landed/blind logging (the "check the status of the SDD" half), one
  `#[ignore]` live test on `agent_tool: "cline"`. 4 open questions recorded
  ("1/10" decode, "SDD toolbar" referent, tool coverage, SSH fallback).
  Tracker mirror SKIPPED (MCP tool unavailable) — non-blocking per playbook.
  The later `.agentum-harness` authoring gate resolved the ambiguous issue as
  the live per-session SDD phase + n/N status strip; that later contract is the
  shipped scope, while injection delivery remains a possible follow-up.
- 2026-07-20 | Developer/Tester | **ISSUE #395 STATUS STRIP COMPLETE + GREEN.**
  Added a run-aware strip to every server-backed terminal session, resolves
  feature/QA/role names by harness token + workdir, excludes ordinary sessions,
  derives executing progress from backlog order, and refreshes through the
  existing event stream with burst-safe coalescing. Focused Vitest 4/4; Vite
  production build PASS. Standalone tsc remains baseline-red on missing shared
  modules across the copied frontend and has no errors attributable to this slice.
