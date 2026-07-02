# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 005-one-shot-issue-loop
- **phase:** done        <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (005 **SHIP-READY** — Reviewer SIGN-OFF 2026-07-02, zero Blockers, `review.md`; commits `197a7bea`/`ae8bf467`/`3b0a00d0` on `.claude/worktrees/finish-the-loop`. RELEASE = Mateo: issue + PR finish-the-loop→develop w/ Closes #N in the commit MESSAGE, staging browser QA per verification.md deferred list, promote + tag. 004 RELEASED v0.49.0)
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
- 2026-07-02 | Reviewer | **005 SIGN-OFF → SHIP-READY** (`review.md`). All 6
  focus items pass (C5 handler read straight-line incl. all three claim arms;
  never-Err verified in github arm/Todo branch/report_status_text; all four
  regression pins are REAL literals; flat-Tauri+camelCase verified end-to-end;
  UI matches composer/pane patterns). 0 Blockers. Should-fix follow-ups:
  (1) stale old-QA docs cluster (types.rs/helpers.rs/drive.rs comments) —
  docs-only pass at ship or fast-follow; (2) file the pre-existing Linear
  snake_case state-map bug (IntegrationsPane.tsx:83-89 vs linear.rs:482-486);
  (3) NEW: find_by_workdir exact-PathBuf equality + no canonicalization →
  symlink-aliased workdir spellings can double-register (pre-existing class,
  named issue). 5 nits (let _ = list shrug; ""-specId; HarnessCompleted noise;
  toggle survives unlink; lock held across gh fetch). "The four regression
  pins are honest literals — the net actually catches drift." spec.md Status
  → Done. Phase → done. **RELEASE = Mateo** (issue + PR w/ Closes in commit
  MESSAGE, staging browser QA, promote + tag).
- 2026-07-02 | Tester | **005 verdict PASS 10/10 ACs** (`verification.md`).
  Independently re-ran everything: 518/0/5 lib (124.4s), scoped
  task_sink 26 / harness 80 / mcp 25, vite 1m54s, vitest 10/0, desktop check
  green, fmt clean. Read assertion BODIES, not names; all 14 developer
  deviations audited ACCURATE. Cross-cutting: auth.rs + worktrees.rs diffs
  EMPTY, drive.rs = blueprint hunks only, mcp.rs insert-only, exactly ONE
  env-mutating test (locked). 5 Info findings, none blocking: (1) pre-existing
  Linear snake_case state-map save bug CONFIRMED (file issue at ship);
  (2) stale qa docs in types.rs + scaffold template; (3) no handler-level
  start_work e2e test (seam-pinned, 004-accepted class); (4) alreadyRunning
  specId can be "" for pre-F2 runs; (5) HarnessCompleted{success:false} noise
  on stale-idle re-registration. GUI + live label flips = deferred to
  qa.sh/staging by contract. Handoff `04-tester-to-reviewer.md`. Phase →
  reviewer.
- 2026-07-02 | Developer | **005 slice 3: F5 GREEN — developer phase
  COMPLETE** (`3b0a00d0`; 518/0 lib, cargo check -p agentum-desktop green,
  vite green). GithubStateMap (defaults→github.json→env via pure
  `apply_layers`; label_for/labels; phase-keyed `github_status_color`);
  argv builders widened (name-filtered remove-set, dedup, byte-identical
  default pin honored pre/post); flat-arg `github_get/set_state_map` Tauri
  commands + unconditional Settings editor. Todo-at-plan test hardened with
  `AGENTUM_GITHUB_CONFIG`→absent-file pin (github arm now reads from_env).
  🐛 PRE-EXISTING found, not fixed: Linear editor sends snake_case invoke
  keys that never bind → every Linear save silently clears
  in_progress/ready_to_test overrides — file an issue at ship time. Handoff
  `03-developer-to-tester.md`. Phase → tester.
- 2026-07-02 | Developer | **005 slice 2: F1 GREEN** (`ae8bf467`; 512/0 lib,
  vite 1m03s, vitest 10/0). POST /api/harness/start-work + shared
  `ensure_spec_and_plan` (Todo-at-plan inherited by 004 route, test-pinned
  never-overwrite 400) + `start_work_lock`/`find_by_workdir` +
  `update_backlog_knobs`; composer toggle + three-path skip
  (`planCreatedWorkspaceOpen`) + TaskPage row action. drive_inner + run-route
  spawn: ZERO diffs (orchestrator-verified drive.rs untouched). 8 deviations
  in tasks.md (notable: no release_driver needed — nothing fallible after the
  fresh claim; deleted-run claim Err falls through to fresh registration;
  fake-gh Todo test under TEST_ENV_LOCK asserts exactly one
  `--add-label status/todo` edit). NOT GUI-verified (toggle/dropdown/toasts =
  unit+build pinned; browser QA = qa.sh/staging). Phase stays developer →
  slice 3 = F5 (GithubStateMap + github.json + Settings card).
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
