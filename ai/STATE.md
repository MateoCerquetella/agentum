# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 015-workspace-harness-autostart
- **phase:** done         <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (015 **SHIP-READY — Reviewer SIGN-OFF 2026-07-13**, `review.md` commit `d390346a`, 0 blockers. 1 Should-fix = follow-up ticket (stale offer survives worktree deletion — `harnessOfferByWorktreeId` missing from `buildWorktreePurgeState`, `worktrees.ts:606`; fail-safe on accept). 4 leave-as-is nits. Product commits `66f1e161`+`03f2eb2b`+`41cbeab8`; gates 50/0 vitest + vite build, independently reproduced. **RELEASE = HUMAN (Mateo)**: merge PR → develop, promote → staging → main, browser QA (qa.sh scenario in handoff 02) — AC 2–6 runtime legs deferred there. Loop was fully autonomous; ai/roles/orchestrate/hitl scaffold files still MISSING repo-wide — ran from the sdd-orchestrate playbook + validate_handoff.md + 004–014 precedent.)
- **mode:** auto         <!-- HITL (human in the loop) | auto -->  (set by /sdd-loop 2026-07-01; NEEDS-HUMAN exit is the safety valve; RELEASE stays human-gated)
- **execution:** harness <!-- features land via the .harness/ engine + green gate -->

## Active send-backs

- **010-end-to-end-autonomous-flow** — RELEASED v0.60.0 (2026-07-06) with the
  AC-11 live custom-column board demo still PENDING and human-run (Mateo;
  evidence contract = issue #276 timeline + a demo-pass line here). Follow-up
  ticket #277 = the resolve_slug/gh_bin/BLOCKED_LABEL consolidation chore.
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

<!-- append one line per decision, newest last: `YYYY-MM-DD — <decision>`; keep only the last 5 (older history lives in git) -->- 2026-07-13 | Architect | **015 gate PASS → phase developer** (sub-agent;
  `architecture.md` 451 ln + handoff 02). Q1: ONE mount — Terminal.tsx root
  flex strip (`relative z-30 shrink-0`) after `EditorAutosaveController`,
  self-gates on offer slice keyed by worktreeId; launcher overlay is
  `absolute z-20` so the strip shows in BOTH auto-launch and empty-state
  paths; REJECTED launcher-only (D1), dual-mount, and the `:1666` legacy
  block (#313 invisibility trap). Q2: fire-and-forget
  `maybeOfferWorkspaceHarnessRun` at END of `openCreatedWorkspace` — both
  create paths converge (`useComposerState.ts:2533/:2750`) so ZERO
  useComposerState edits; zustand slice holds the RESOLVED offer (rejected
  pending-signal: non-reactive, banner must appear when async detection
  resolves). Detection mirrors `resolve_harness_dir` semantics (canonical dir
  present w/o feature_list ⇒ NO legacy fallback). Residuals accepted:
  quick-create "don't start a session" path yields no offer; symlink-spelling
  dedupe gap. Dev must NOT: touch useComposerState/planCreatedWorkspaceOpen
  pins, poll, pass hostId, jsdom, persist offers, rely on engine dedupe
  (`harness.rs:95` inserts unconditionally).
- 2026-07-13 | Developer | **015 f1+f2+f3 CODE-COMPLETE + GREEN → phase tester**
  (sub-agent; commits `66f1e161`/`03f2eb2b`/`41cbeab8` + docs `5f753260`;
  `tasks.md` + handoff `03-developer-to-tester.md`). Gates: f1 20/0, f2 45/0,
  f3 50/0 (4 targeted vitest files, bun) + vite build ✓ ×3 (~38s each).
  Scope audit (orchestrator): 15 files exactly per architecture §6, NO
  useComposerState/planCreatedWorkspace/Rust edits,
  open-created-workspace.test.ts additions-only (+39/-0). 3 deviations, all
  code-commented + tasks.md-logged: (1) acceptHarnessOffer swallows+toasts
  (no re-throw), banner busy-reset via .finally — observable behavior
  identical + pinned; (2) banner test mocks the offer lib (network-free
  render, sdd-bar precedent); (3) trigger pins = NEW describe block w/
  per-test mockClear (existing pins byte-identical). Env notes: worktree
  needed `bun install`; anchors had ZERO drift; zustand v5 SSR snapshot
  forces mocked-store banner testing (validates handoff prescription).
- 2026-07-13 | Tester | **015 verdict PASS-WITH-DEFERRALS → phase reviewer**
  (sub-agent; `verification.md` + handoff `04-tester-to-reviewer.md`, commit
  `d66d4c40`; 0 blockers, 0 should-fix, 3 info nits). Independently re-ran
  gates: vitest 50/0 exact per-file match (20+14+12+4), vite build exit 0.
  AC 1/7 PASS now; AC 2-6 PASS(deferred qa.sh/staging) with model logic
  pinned (dedupe+trailing-slash, gatedRun/local-only zero-fs-call, dismiss =
  zero client calls, detail-in-toast, ≤2 fs calls not-found). Deviations 3/3
  ACCURATE. Spot-checks 7/7 (incl. mount OUTSIDE legacy/#313 + split
  surface; close-race store re-read `offer.ts:93-100`). Sacred: EMPTY
  useComposerState diff; open-created-workspace.ts = import + trailing void
  call; test additions-only; harness-client exactly +3 exports. Nits: handoff
  03 STATE-claim wording; `normalizeWorkdir('//')` unreachable edge;
  POSIX-only join. Tracker: issue #301 → status/ready-to-test.
- 2026-07-13 | Reviewer | **015 SIGN-OFF → SHIP-READY, phase done**
  (sub-agent; `review.md` + spec Status → Done, commit `d390346a`; 0
  blockers). All 6 focus areas PASS w/ quoted evidence: close-race re-check
  sufficient for reachable cases; back-to-back creations sound (record-keyed
  slice); `normalizeWorkdir` = exact `expand_with_home` mirror; security
  clean (own workdir + server ids only, no auth in toasts, harnessDir const
  union escaped); invariants held (one launch path, register+run only — no
  /init, zero polling, D2/D5/D6 structurally pre-fs); mount layout shift IS
  the designed non-occlusion (shrink-0 vs flex-1 anchor, z-30 over z-20);
  React clean (.finally-after-unmount = React-18 no-op). **F1 Should-fix →
  follow-up ticket**: `harnessOfferByWorktreeId` missing from
  `buildWorktreePurgeState` (`worktrees.ts:606`) — stale offer survives
  worktree deletion (fail-safe on accept; one-line fix + pin). F2–F5 nits
  leave-as-is. All 3 dev deviations + 3 tester nits ruled leave-as-is.
  **RELEASE = HUMAN**: PR → develop, promote, browser QA (qa.sh scenario,
  handoff 02) for AC 2–6 runtime legs.
- 2026-07-13 | Release | **015 SHIPPED → v0.75.0** (Mateo: "release
  please"; browser QA of AC 2–6 runtime legs remains PENDING and human-run —
  shipped ahead per 008/010 precedent; evidence contract = a demo-pass line
  HERE + #301 timeline). PR **#353** merged → develop `8b797347`; promoted
  develop→staging `a789f9ab` → main `ee104eb7`; bump 0.74.3→0.75.0
  (Cargo.toml+lock+tauri.conf.json) + tag v0.75.0 (fires release.yml). Issue
  #301 closes via this commit's `Closes`. Follow-up: **#352** (offer
  purge-state miss, reviewer Should-fix).
