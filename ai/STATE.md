# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 009-wiki-project-scoped
- **phase:** reviewer     <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (009 Tester PASS-WITH-DEFERRALS 2026-07-06, `verification.md`, HEAD `2c3dc89d`; AC-4 ruled PASS-with-note + qa.sh wording amended in spec.md; handoff `04-tester-to-reviewer.md`. 008 RELEASED v0.57.0 `64053d4c`, #250 closed)
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
- 2026-07-06 | Developer | **009 F2 CODE-COMPLETE + COMMITTED `8f1b663c`**
  (wiki-quiet-probing, AC 4–6; F3 remains → phase STAYS developer). Sweep +
  `repoStatuses`/`RepoWikiStatus` deleted; pure `wiki-probe.ts` pins
  one-repo-only probing (+test); `AppState.wiki_keys` positive-only cache
  keyed `(repo_id,path,host_id)` — local = nil UUID (exact: LOCAL_HOST_ID
  already nil), `should_cache_wiki_key` decides on the REMOTE resolution
  (decoupled from key format), path-fallback never cached, lock never across
  `.await`; fs.rs `is_click_to_open_dir` + `ListQuery.prefetch` seam DORMANT
  (D3 audit recorded in tasks.md: no automatic protected-dir read at base —
  PR body must state it). 8 test-mod `fresh_state()` literals mechanically
  gained the field (compile-mandated). Gates: cargo 569/0/5 (+4 new), fmt
  clean own-files-only, clippy 0 warnings (type_complexity fixed via lib.rs
  alias), vite green, vitest wiki 2/2, AC-4 sweeps zero hits. F3 note:
  RUNNING_POLL_MS still present BY DESIGN (F3 deletes it, no fallback);
  `wiki-view-state.ts` absorbs `wiki-probe.ts`.
- 2026-07-06 | Developer | **009 F3 CODE-COMPLETE `fdfec986` → DEVELOPER
  PHASE DONE, phase → tester** (handoff `03-developer-to-tester.md`).
  `emit_wiki_updated` at all 4 write_status transitions (ready BEFORE
  embeddings) + run-scoped 2s `scan_pages_loop` in `select!` w/ settle
  (growth-only via pure `scan_grew`); Running GET carries `pages`; UI:
  `wiki-view-state.ts` reducer (absorbed wiki-probe; ready/failed events =
  REFETCH commands, never a flip — discriminator pin tested), poll DELETED
  no fallback, progressive TOC + banner, pageCache cleared running→ready.
  4 deviations, notably `rename_all_fields=camelCase` on WikiIndexResponse
  (variant fields were silently snake_case on the wire — real fix, wire-shape
  test pins it). Gates: cargo 571/0/5 (AC-9 tests UNMODIFIED 13/13 wiki),
  fmt/clippy clean, vite green, vitest wiki 14/14, all grep pins zero-hit.
  ⚠️ TESTER MUST RULE: mount = up to TWO same-repo GETs (probe + onOpen
  refetch) — one-REPO-only holds; F2 qa.sh wording says "exactly one read".
- 2026-07-06 | Tester | **009 verdict PASS-WITH-DEFERRALS → reviewer**
  (`verification.md`, HEAD `2c3dc89d`; handoff `04-tester-to-reviewer.md`).
  Independently re-ran EVERY gate: cargo 571/0/5 (AC-9 fns proven unmodified
  via diff-hunk analysis — pure-addition tests-mod hunk only), fmt clean,
  clippy 0, vite green, vitest 610 pass w/ 31-fail baseline corroborated vs
  pristine extract (same 7 files). All 9 ACs PASS (4 visual/live aspects
  deferred to qa.sh w/ repro steps, 008 precedent). **AC-4 RULED
  PASS-with-note**: worst case 2 same-repo GETs on mount (probe + onOpen
  refetch); intent holds (one-repo-only, cache absorbs the 2nd read); dedupe
  REJECTED (would risk the reconnect-heal refetch that justifies no-fallback);
  qa.sh wording AMENDED in spec.md; PR body must carry the deviation note +
  StrictMode dev-build caveat (prod = 2, dev = up to 4). 4 deviations audited
  ACCURATE (D4 not violated — event stays snake_case). Sacred surfaces all
  untouched (route list byte-identical). 0 blocking defects; 1 cosmetic
  (double-"Projects" heading, D2-locked) for reviewer. Phase → reviewer.
