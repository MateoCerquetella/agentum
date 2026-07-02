# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 004-workspace-issue-loop
- **phase:** done        <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (004 SIGNED OFF, SHIP-READY; release human-gated. 002 + 003 parked at human-gated release)
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
- 2026-07-01 — 004 architect blueprint COMPLETE (`architecture.md`). Corrections
  C1–C5 (Tasks-page create is a local stub → new `POST /api/github/issues`; UI
  shim strips linked fields → widen 2 TS layers; `linkedPR` wire fix + registry
  NO-alias rule (duplicate-field → `read_worktrees` wipes to `[]`); remove only
  the 3 canonical labels; direct local gh, no Host). Confirmed `board_sync`
  cannot strip labels (PATCH = `{title,body,state}` only). Merged develop
  (+35 commits, incl. 003's `task_sink.rs` labels support) → line cites drift,
  re-locate before editing. Phase → developer.
- 2026-07-01 — 004 Developer slice 2: **F3+F4 GREEN**; developer phase
  COMPLETE. F3: `POST /api/github/issues` (blank-title 400, 422
  `no_github_repo`, TaskSink create, id→i64) + composer affordance
  (Card-rendered, only when unlinked + local git) → chip pre-worktree. F4:
  `fetch_github_issue` refactor (+url, no wire change), `spec_md_from_issue`
  (control-strip, 64KiB cap, fallback AC via real derive), traversal-proof
  `issue_spec_id`, `POST /api/harness/spec-from-issue` (never-overwrite),
  `scaffoldSpec` toggle OFF-default in both submit paths (non-fatal failure),
  `plan_from_spec_with_tracker` stamps provider+url (delegation pinned).
  Gate: 494/0 tests, fmt+check clean, vite ✓ 1m04s. 5 deviations accepted
  (Card-not-Modal markup; documented allow(dead_code) on FetchedIssue.slug;
  tests in harness.rs surface_tests per convention; typed conditional;
  pure-gate title test). Phase → tester.
- 2026-07-01 — 004 Tester: **7/7 ACs PASS** (`verification.md`). Independently
  re-ran the full suite (494/0/5) + 4 scoped suites + vite (✓ 1m11s) + the
  auth.rs no-`is_public` diff (empty). Exactness confirmed by reading code:
  label table verbatim, remove-set can never name `status/qa*`, NO close path
  in the arm (D1), all 6 failure paths → `Ok(Skipped)` (no `?`/`Err`).
  Commit-attributed the drive.rs range diff: spec-004 = only the
  `transition_tracker` widening (second hunk = pre-spec `05abe6f1`). 4 Info
  findings (GHES URLs skip silently; no handler-level 400 tests; no dedicated
  30s-timeout test; the attribution note) — none blocking. GUI behaviors
  (chip, toggle, live label flip) = qa.sh/staging gate. **ADVANCE → reviewer**
  (handoff `04-tester-to-reviewer.md`).
- 2026-07-01 — 004 Reviewer **SIGN-OFF → SHIP-READY** (`review.md`). All 6
  focus items pass; invariants hold; "test suite unusually communicative;
  comment discipline exemplary". 0 Blockers. Follow-ups (non-blocking):
  narrow the `as unknown as GitHubWorkItem` cast (useComposerState.ts:1448);
  FILE A GHES ISSUE (transitions skip on non-github.com URLs — by design,
  name it); nits: debug-log the initial-Todo skip, scaffoldSpec reset-on-unlink.
  spec.md Status → Done. Phase → done. **Release = Mateo** (/ship: issue + PR
  fix-wiki→develop w/ Closes #N, staging browser QA — chip, toggle, live
  label flip ending OPEN with exactly status/done — then promote + tag).
- 2026-07-01 — 004 Developer slice 1: **F1+F2 GREEN** (486/0 lib tests, fmt +
  check clean). F1: `GITHUB_STATUS_LABELS` + pure gh argv builders +
  `github_slug_and_number_from_issue_url` + `run_gh` (30s timeout) +
  `github_transition_with` in task_sink; seam widened with `tracker_url`
  (both callers, one logical line each); every failure → `Ok(Skipped)`, never
  `Err`. F2: `CreateBody`+3 (`linkedPR` alias), registry persistence,
  detected-scan emits `linkedPR`, `canonical_meta_key`, NO registry-struct
  alias; 2 TS layers forward the fields. Vite build deferred to the F3/F4
  slice. Deviations logged in tasks.md (rustfmt reflow; gh empty-stderr
  message; URL parser also strips #fragment). RELEASE stays human-gated.
