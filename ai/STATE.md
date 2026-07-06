# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 010-end-to-end-autonomous-flow
- **phase:** developer         <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (010 = the PRD spec, renumbered from 009 — 009 = wiki, RELEASED v0.59.0. PM-gated D1–D8; **Architect DONE 2026-07-06** — `architecture.md`, 9 calls resolved (D2 → sibling `github_projects.json`; probe-then-act close/reopen; id cache ships; new `routes/github_projects.rs` + `routes/provision.rs` + crate-root `github_projects.rs`; F2 = two arm hooks with ZERO seam-call-site edits), handoff `02-architect-to-developer.md`. Worktree merged origin/develop v0.59.0 mid-phase (`664ee365`); seam anchors re-verified. Developer: build F1 bind → F2 drive → F3 provision, one gated slice each; FF develop first.)
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
  `9d9be973`, 0 blockers; since RELEASED v0.57.0 `64053d4c`). All 18 focus
  items PASS w/ quoted evidence; 1 Should-fix = project-wide CI typecheck
  follow-up; 3 leave-as-is nits. (Full text in git history.)
- 2026-07-06 | Reviewer | **009-wiki SIGN-OFF → SHIP-READY** (their flow, on
  `wiki-remove-it-fomr-the-side`; since MERGED + RELEASED v0.59.0 `b62a9171`,
  PR #273, issue #272 closed). 2 Should-fix follow-ups live in #272.
  (Merged into this STATE at the 010 develop-merge; full text in git history.)
- 2026-07-06 | Spec | **010 (né 009) DRAFTED via /sdd-spec from Mateo's PRD
  "End-to-End Autonomous Flow (Chat → Issue → Work → QA)"**
  (`ai/specs/010-end-to-end-autonomous-flow/spec.md`; worktree FF'd to v0.58.3
  `388eaa66` first so line refs are current). Scoping finding: the PRD's §1
  canonical flow already SHIPPED (004/005/006/008/012 — issue create,
  start-work, spec materialization, spawn, settle, two-phase gate, retry
  budget); the real delta = **GitHub Projects v2 mirror + workspace
  provisioning**. Slices: F1 bind (one `gh api graphql` Status-field discovery
  → phase→optionId mapping, never-unmapped fallback) / F2 drive (projects arm
  INSIDE `apply_tracker_transition`+`apply_blocked_transition` — all call
  sites free, incl. MCP `agentum_report_status`; `done_closes_issue` knob) /
  F3 provision (repo-from-template + label pre-ensure + board create/bind +
  scaffold commit, run-twice idempotent). Desktop `gh_projects` READ commands
  reused by the wizard; board WRITES live server-side (desktop write stubs
  stay dead — spec-007 lesson). OUT: inbound webhooks/echo (none exist,
  board_sync.rs:14), `.agentum/result.json` (settle+gate IS the completion
  contract), GitHub App auth (Phase 2). PM gate PASS. Phase → pm.
- 2026-07-06 | PM | **010 PM GATE PASS → phase architect** (D1–D8 locked in
  spec §Decisions; handoff `01-pm-to-architect.md`; RENUMBERED 009→010
  mid-phase — sibling branch `wiki-remove-it-fomr-the-side` carries
  ship-ready `ai/specs/009-wiki-project-scoped`, loop driver + STATE + spec
  retargeted). All 9 gate items green; 30+ citations spot-verified at
  `388eaa66`; two drifts fixed (`create_feature` = task_sink.rs:124; seam =
  4 direct call sites / 6 transition points, not "five"). Locked: D1
  close-on-Done BOUND-only via `done_closes_issue` default-ON (supersedes
  004-D1 there; deliberate narrowing of the PRD's unconditional close —
  PR-driven repos keep `Closes #N`); D2 binding DAEMON-SIDE, mechanism =
  architect (github.json+passthrough / sibling file / store table à la
  `agentum_core::TrackerBinding` — seam already has `&Store`) under the
  HARD constraint that a Settings label save must never destroy a binding
  (found: `github_labels.rs::update_config` DROPS unknown github.json keys
  — clobber hazard); D3 drags overwritten Phase-1, no echo machinery; D4
  template default = goempirical starter, configurable; D5 board create
  ships, fallbacks VISIBLE, no option mutation; D6 one-slice = 3 increments,
  F1+F2 self-sufficient; D7 bind UI = one shared component, settings mount
  BEFORE the F3 wizard; D8 consent-gated plain push, no attribution trailer.
  Architect calls named: D2 mechanism, fuzzy internals, id cache,
  probe-vs-blind reopen, route home, template-mode repoId flow, knob default
  home. Next artifact: `architecture.md`.
- 2026-07-06 | Architect | **010 ARCHITECT GATE PASS → phase developer**
  (`architecture.md`; handoff `02-architect-to-developer.md`; mid-phase the
  worktree MERGED origin/develop v0.59.0 — STATE.md conflict resolved for
  active-spec-010, base now `664ee365`, all four seam anchors re-verified
  unchanged). Nine calls resolved: D2 → **(a2) sibling `github_projects.json`**
  (a1 github.json rejected on the verified clobber hazard + two-writer race;
  a3 TrackerBinding rejected on verified board_sync coupling — reuse would
  make Projects bindings pull-able); fuzzy = strip-to-alnum + disjoint
  exact-match synonym tables, NO substring, refusal names phases+options;
  id cache SHIPS in F2 (9 warm calls ≤ ceiling vs ~14 cold) w/
  invalidate-retry-once (correctness cache-free); close/reopen =
  probe-then-act BOTH directions, knob-gated (knob-OFF never probes —
  human-closed issues respected); routes = new `routes/github_projects.rs` +
  `routes/provision.rs`, domain in crate-root `github_projects.rs`
  (linear.rs precedent — keeps the F2 arm a two-line hook); template repoId
  produced inside goal-step Continue (isGoalStepReady untouched);
  `done_closes_issue` default lives on BoardBinding serde-default (one site);
  provision = 4th OPTIONAL_WORKSPACE_STEPS entry + modal `'provision'` phase;
  board-failure visibility = fold into existing `Skipped(reason)` +
  tracing::warn (zero call-site edits, rides drive.rs engine.log). F2 arm
  hook builds LAST after fake-gh suite green; AC 8 = zero test-file diffs.
  Two spec-constrained additions only (id cache; state-only .gitignore
  rewrite for the F3 commit). Developer: F1 → F2 → F3, one gated slice each.
