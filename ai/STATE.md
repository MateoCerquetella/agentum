# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 009-wiki-project-scoped
- **phase:** developer    <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (009 Architect PASS 2026-07-06, `architecture.md` D-A1–D-A8, handoff `02-architect-to-developer.md`; build F1→F2→F3 one gated slice each; grounded at `388eaa66` v0.58.3. 008 RELEASED v0.57.0 `64053d4c`, #250 closed)
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
- 2026-07-06 | Spec | **009 drafted + PM-gated** (`009-wiki-project-scoped`,
  Mateo's ask): Wiki OFF the nav rail → new sidebar **Projects** group →
  hub-only access (`ProjectHubPage.tsx:181` embed is the survivor). Root cause
  of the "aggressive scanning / permission storm": the standalone hub's
  every-repo sweep (`WikiPage.tsx:175–189`, N × `git remote get-url` under
  ~/Documents etc.; recurrence across updates = unsigned app #230, OUT of
  scope). F1 rail swap, F2 quiet probing (sweep deleted + `resolve_target`
  key cache + fs-guard widened for automatic reads only), F3 `wiki.updated`
  on /api/events replacing the 3s poll + progressive page render (loud-failure
  001-AC-9 preserved). ⚠️ One-slice gate = pass-with-note (3 increments, one
  root cause, repo precedent 004/008). Open Qs: standalone view fate, Projects
  group shape vs groupBy='repo', which automatic reads remain, event naming.
  Phase → pm.
- 2026-07-06 | PM | **009 gate PASS → architect** (fresh sdd-pm subagent;
  handoff `01-pm-to-architect.md`). All 9 items pass ("one slice" =
  pass-with-note: 3 increments, one root cause, precedent 004/005/008); ~15
  citations line-verified at `388eaa66`; `Event.kind` = open dotted String
  (`agentum-core/src/lib.rs:429–437`) so `wiki.updated` needs no enum change.
  **D1–D4 LOCKED**: D1 delete standalone wiki view entirely (openWikiPage
  single caller, closeWikiPage zero); D2 Projects = separate always-visible
  rail section (groupBy is toggleable); D3 AC-6 expected as regression guard
  (PM audit: no other automatic protected-dir read; PR must state audit
  result); D4 one `wiki.updated` event `{repo_id,status,pages?}`. 6 mechanical
  spec edits applied by orchestrator (AC-1/3/4/7 tightened, qa.sh scoped per
  feature, Status→PM). Architect notes: self-invalidating cache key
  `(repo_id,path,connection_id)`; page-write detection = architect's pick;
  progressive TOC never flips the discriminator. Phase → architect.
- 2026-07-06 | Architect | **009 gate PASS → developer** (`architecture.md`
  D-A1–D-A8; handoff `02-architect-to-developer.md`; all citations re-verified,
  4 drifts corrected — resolve-zoom-target is in hooks/, FOUR write_status
  emission sites, fs test :631, **vite build does NOT typecheck** → D1 deletion
  + poll removal need verify.sh grep pins). Key designs: NEW sibling
  `SidebarProjectsNav.tsx` + pure `projects-nav-rows.ts` (ignores
  filterRepoIds, D-A8); WikiPage → embed-only w/ REQUIRED `pinnedRepoId`;
  `wiki_keys` composite-key cache `(repo_id,path,host_id)` positive-only
  (never cache path-fallback); `wiki.updated` via `state.bus.send` at 4 sites
  + run-scoped 2s `scan_pages_loop` in `select!` w/ settle (fs-notify REJECTED:
  remove_dir_all race); poll deleted with NO fallback (loopback: socket-down ⇒
  HTTP-down); Running GET gains `pages`; ready flips ONLY via validated GET
  (event = refetch command); fs.rs dormant `prefetch` seam. `activeView` NOT
  persisted (rehydration risk cleared). Phase → developer (F1 first).
- 2026-07-06 | Developer | **009 F1 CODE-COMPLETE + COMMITTED `b325c176`**
  (projects-sidebar-wiki-off-rail, AC 1–3; F2/F3 remain → phase STAYS
  developer). NEW `SidebarProjectsNav.tsx` + pure `projects-nav-rows.ts`
  (+5 vitest, hidden when repos empty) mounted in sidebar/index; D1 deletion
  inventory complete (rail item, 'wiki' unions, openWikiPage/closeWikiPage,
  App arm+lazy import, zoom-target, test list); WikiPage embed-only
  (`pinnedRepoId` REQUIRED, RepoRail/statusDot/standalone chrome deleted;
  sweep reduced to pinned-only — full deletion+repoStatuses = F2;
  RUNNING_POLL_MS = F3). Gates: vite green, vitest 12/12 new (31 dir fails
  PROVEN pre-existing vs pristine origin/develop baseline), cargo 565/0/5,
  D1 sweep clean (only hub-tab hit). 2 deviations documented (dead standalone
  empty state deleted; dead-at-base setActiveView left). ⚠️ for reviewer:
  possible double-"Projects" heading (SidebarHeader when groupBy='repo');
  verify.sh grep pins not yet wired into a .harness scaffold.
