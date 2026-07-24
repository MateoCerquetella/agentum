# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 029-local-watchdog-fleet-scheduler
- **phase:** architect  <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (029 PM PASS; handoff `01-pm-to-architect.md`.)
- **mode:** auto         <!-- HITL (human in the loop) | auto -->  (set by /sdd-loop 2026-07-01; NEEDS-HUMAN exit is the safety valve; RELEASE stays human-gated)
- **execution:** harness <!-- features land via the .harness/ engine + green gate -->

## Active send-backs

- **020-ssh-host-tracker-plumbing** — **SHIP-READY** (Reviewer SIGN-OFF
  2026-07-13, `review.md` @ `cc4bde36`, 0 blockers; spec Status → Done).
  Commits F1 `09726c46` F2 `e8fb31a8` F3 `820712d9` on `fixes-new-workspace`,
  on top of ship-ready 015. **RELEASE = HUMAN**: ONE train with 015 (same
  branch) — PR → develop → staging qa.sh (live dyaus binding, SSH filing +
  grounding note, Start-work direct launch, host-down 422-flavor vs slug-route
  502, gh authed on the remote) → main + tag. Follow-up ticket (reviewer
  should-fixes): SF1 ProjectHubPage:86 Tasks-tab binding read not
  repoId-threaded — bound SSH repo's Tasks tab never auto-enters board mode;
  SF2 SSH-repoId issue FETCH composes local neutral_cwd with remote gh —
  caller-less today but the live wire will trip the deferred QA leg (fix
  before/at QA); SF3 tasks.md wording.
- **015-host-aware-start-and-tracker-intake** — **SHIP-READY** (Reviewer
  SIGN-OFF 2026-07-13, `review.md` @ `aa8ce9e3`, 0 blockers). Commits F1
  `ff7290ee` F2 `d7d64f33` F3 `3ec6f028` on `fixes-new-workspace`, unpushed.
  **RELEASE = HUMAN (Mateo)**: PR → develop, promote → staging (`status/qa`;
  qa.sh legs: live VPS add/pick/create AC 3-4-7, choose-hop AC 5, real filing
  AC 10, board+gated run AC 11) → main + tag. Release notes: one-time remote
  re-add + onUse zero-match shift. F1+F2 SAME train. Follow-up ticket: S1
  residual selectors→findRepoByPathPreferLocal + doctor check, S2 reposUpdate
  doc comment, S3 reject connectionId:"". NOTE: 019 (SSH tracker plumbing)
  builds on these commits — 015 ships first. 010's AC-11 live demo also still
  PENDING/human.
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
- 2026-07-23 | Developer | **028 final stale-request fix PASS → phase tester**.
  A weak keyed lifecycle boundary linearizes agent-task loads with stop/kill,
  delete, and tool-patch retirement; 21 isolated and 839 backend tests pass.
- 2026-07-23 | Tester | **028 final retry PASS → phase reviewer**. Fresh
  verification closes AC 1–8 with 22 focused tests, 21 isolated QA tests, and
  839 green backend workspace tests; stale-request races are green.
- 2026-07-23 | Reviewer | **028 SIGN-OFF → done** (0 blockers). Full lock,
  generation, resource-bound, compatibility, and safety audit passes; release
  remains human-gated with one Rust helper API release-note caveat.
- 2026-07-23 | Spec | **029 drafted → phase pm**. One local fleet scheduler and
  nonce/pane-framed tmux batch contract target ≤2 healthy-cycle local invocations
  while preserving SSH behavior, cadences, events, and stale-removal safety.
- 2026-07-23 | PM | **029 PASS → phase architect**. The 11-AC contract is one
  bounded local performance slice; ≤2 invocations, typed framed outcomes,
  cadence/event compatibility, SSH preservation, and stale authority are fixed.
