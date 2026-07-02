# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 005-one-shot-issue-loop
- **phase:** developer   <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (005 Architect COMPLETE 2026-07-02, `architecture.md` gate 5/5, C1–C5 corrections, handoff 02 written; worktree `.claude/worktrees/finish-the-loop`, base `7e9afaa4`; 004 **RELEASED v0.49.0**. Installed-app spot-check still pending: chip, toggle, live label flip)
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
- 2026-07-02 | Developer | **005 slice 1: F2+F3+F4 GREEN** (`197a7bea`; 507/0
  lib tests, fmt clean, vite green). F2 spec_id stamp + prompt widening (two
  byte-identical pins written against the PRE-change function). F3 pure
  resolve_qa_mode + `harness.qa.agent_browser.enabled` store setting +
  GET/PUT /api/harness/settings + IntegrationsPane toggle; QA prompt →
  agentum_browser (verdict contract character-identical). F4
  agentum_report_status (ungated, never-Err text mapping, board-card wire
  test). Deviations logged in tasks.md: skill-name still present in the
  "Do NOT use" steer (test asserts no *instruction*); toggle is a standalone
  card (Linear editor renders only when connected); stale
  qa_mode/qa_agent_tool doc comments flagged for reviewer. Zero structural
  diffs in drive_inner/spawn (orchestrator-verified). Fresh-worktree gotcha:
  `bun install` before the vite gate. Phase stays developer → slice 2 = F1
  (start-work route + engine lock + composer/Tasks UI), slice 3 = F5.
- 2026-07-02 | Architect | **005 blueprint COMPLETE (`architecture.md`), gate
  PASS 5/5.** Route = `POST /api/harness/start-work` (harness namespace, not
  /api/workflows — YAGNI); shared `ensure_spec_and_plan` core (converge flag)
  serves start-work AND the 004 route, Todo-at-plan lives there (route layer
  has &Store); post-plan `update_backlog_knobs` seam; C1 pre-registration
  failures = HTTP toast (no nil-id events); C2 NO new InProgress call (drive
  already fires it at spawn); C3 QA knob = store setting
  `harness.qa.agent_browser.enabled` + GET/PUT /api/harness/settings (NOT a
  json file); C4 spec_id stamp in `plan_from_spec_inner` (MCP plan tool widens
  too, deliberate); C5 engine `start_work_lock` + already-running check before
  any fs write, stale-idle runs stopped+re-registered. resolve_qa_mode becomes
  pure (capability bit computed at caller). F5 colors key off PHASE not name;
  remove-set filtered by name (collision-safe); old-map labels = foreign,
  never touched. Orchestrator spot-verified seams. Phase → developer.
- 2026-07-02 | PM | **005 PM phase COMPLETE, gate PASS after 8 edits; D1–D4
  locked** (server-side start-work route; adoption-not-co-existence with ALL
  THREE plain-delivery paths skipped incl. issueCommand; QA knob = second
  opt-in door DEFAULT OFF — overrides draft AC 8 leaning, else non-web runs
  fail-closed at QA; global github.json state_map). Material findings: AC 6
  was unfirable (`plan_from_spec_inner` writes `spec_id: None`,
  `types.rs:895-898` — plan step must stamp it); AC 9 needs `{provider, id,
  url?, phase}` (Linear/board arms need the handle); AC 1 must converge on
  existing spec (never-overwrite 400 vs D5-toggle overlap/retries); 3 cites
  drifted (resolve_qa_mode → drive.rs:407-423; Todo-at-plan →
  board_goals.rs:604-616; draft-open → open-created-workspace.ts:52-66).
  Handoff `01-pm-to-architect.md`. Phase → architect.
- 2026-07-02 — **Spec 005 drafted (one-shot issue loop) + PM gate PASSED** from
  Mateo's ask (chat → issue → Start work → boards live even w/ custom statuses →
  spec in worktree → seamless prompt-injected agent loop → agentum_browser QA).
  Code-verified gap map on develop tip `1e259604`: the pieces all exist but the
  chain stalls after workspace creation — nothing registers/runs the engine in
  the new worktree (Harness page is a separate manual hop, `HarnessEngine.tsx:588`);
  composer opens the agent with a **draft** prompt (never submitted); the engine's
  feature prompt ignores the scaffolded spec (`harness/helpers.rs:33`); QA prompt
  steers to Playwright skill not `agentum_browser` (`helpers.rs:141`); no MCP/HTTP
  status verb for out-of-engine (/sdd-loop) agents; GitHub labels hardcoded (no
  LinearStateMap parity). Five increments F1–F5; 4 open questions (orchestration
  home, suppression-vs-adoption, QA default posture, state_map scope) carry
  recommendations for auto-resolution. Phase → pm→architect next.
- 2026-07-01 — 004 Reviewer **SIGN-OFF → SHIP-READY** (`review.md`). All 6
  focus items pass; invariants hold; "test suite unusually communicative;
  comment discipline exemplary". 0 Blockers. Follow-ups (non-blocking):
  narrow the `as unknown as GitHubWorkItem` cast (useComposerState.ts:1448);
  FILE A GHES ISSUE (transitions skip on non-github.com URLs — by design,
  name it); nits: debug-log the initial-Todo skip, scaffoldSpec reset-on-unlink.
  spec.md Status → Done. Phase → done. **Release = Mateo** (/ship: issue + PR
  fix-wiki→develop w/ Closes #N, staging browser QA — chip, toggle, live
  label flip ending OPEN with exactly status/done — then promote + tag).
