# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 007-issue-detail-and-generated-descriptions
- **phase:** reviewer    <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (007 Tester PASS 9/9 ACs 2026-07-02, `verification.md` + handoff 01; all 3 root causes confirmed against base+head; commit `96c98955`. 006 RELEASED v0.54.0)
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
- 2026-07-02 | Tester | **007 verdict PASS 9/9 ACs** (`verification.md`).
  Independent re-runs: server 539/0/5, desktop 75/0/4 (+3 gh mapping tests),
  clippy -D warnings green, vite 1m36s, vitest 10/10. All 3 tasks.md root
  causes CONFIRMED against base `27f29f1c` (read the base None-stubs directly)
  + head. Sacred surfaces clean (drive/helpers/task_sink diffs EMPTY;
  harness.rs = only the .gitignore pin; auth.rs empty). 4 deviations accurate.
  4 Info findings none blocking (degenerate repoId:'' edge if Chat-filed w/o
  pinnedRepo AND workspaceId — early-returns w/o error surface; comment-id
  URL-fragment dependence; synthetic gh fixtures; armed-ineligible double
  action). Handoff `01-tester-to-reviewer.md`. Phase → reviewer.
- 2026-07-02 | Developer | **007 fixes + feature GREEN** (`96c98955`; compressed
  SDD, spec.md+tasks.md carry the trail). ROOT CAUSES: (1) detail page's ONLY
  data source `gh_work_item_details()` was a STUB returning None (gh.rs:516),
  null cached as loaded-success, header read the un-hydrated prop — GitHub had
  everything, the app showed 'unknown/No description'; (2) toggles: four
  diverging silent gates + armed state outliving eligibility + ChatPage
  hand-off missing repoId → armed-but-skipped total no-op. FIXED: real gh view
  --json hydration + visible errors; pure `deriveIssueSideEffectGate` feeding
  all paths + skip-reason toasts + disarm on repo-switch/unlink. FEATURE:
  POST /api/github/issues/draft-body (LLM, chat plumbing, SDD-shaped) +
  'Generate description' button (fills textarea, never files silently).
  BONUS: `.agentum-harness/.gitignore` self-ignore (the 'bad inside the
  worktree' fix). NOT GUI-verified. Phase → tester.
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
- 2026-07-02 | Tester | **006 verdict PASS 9/9 ACs + C1** (`verification.md`).
  Independent re-runs: 535/0/5 lib (139.3s), clippy -D warnings green, vite
  2m23s, vitest 15/0, scoped chat 39/harness 86/github 32/task_sink 26. All
  8 deviations audited ACCURATE — incl. confirming the architecture's
  "Confirm spreads the plan verbatim" claim was WRONG (base
  createIssuesFromChat rebuilt {title,summary,tasks}; deviation-2 fix at the
  rebuild seam is correct). Stored-turn "not reproducible" verdict audited
  SOUND (draftPlan ephemeral, StoredTurn persists no plan). F2 byte pin
  traced character-by-character vs the base commit. Sacred surfaces clean
  (drive.rs = one C1 hunk; helpers.rs untouched; auth/registry diffs empty;
  exactly TWO env-locked tests). 5 Info findings none blocking (labels-empty
  repo shows no chips; armed-copy staleness; 422→fallback fold; first-pair
  provenance; author-fetch ordering pre-existing). GUI + live C1 label-flip
  = deferred to qa.sh/staging. Handoff `04-tester-to-reviewer.md`. Phase →
  reviewer.
- 2026-07-02 | Developer | **006 slice 2: F2+F3 GREEN — developer phase
  COMPLETE** (`358347dc`; 535/0 lib, clippy -D warnings green, vite 2m04s;
  pins written FIRST, verified green pre-edit). F2 problem/goal extraction +
  three-section compose (absent = byte-identical pinned) + preview/DraftPlan
  passthrough; 🔑 DEVIATION 2: the Confirm path REBUILDS the plan client-side
  (architecture's "spreads verbatim" was wrong) — problem/goal forwarded
  explicitly, else silently dropped. Mandatory item: fake-gh wire test pins
  plan→--body (non-empty, summary+checklist); stored-turn restore = NOT
  REPRODUCIBLE (no path can lose plan fields — draftPlan is ephemeral state,
  StoredTurn persists content/thinking/filed only). F3 roles inherited:
  SDD_ROLES_ENABLED_SETTING default TRUE (opposite of QA knob, both pinned),
  apply_start_work_knobs, GET-full/PUT-patch split, brief deltas verbatim,
  verdict contract character-pinned, C1 shared_tracker_provenance fix in
  Decompose (drive.rs diff = that one hunk, sacred fns untouched —
  orchestrator-verified). Handoff `03-developer-to-tester.md`. Phase → tester.
