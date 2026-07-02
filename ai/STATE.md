# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 004-workspace-issue-loop
- **phase:** developer   <!-- idle | spec | pm | architect | developer | tester | reviewer -->  (004 architect DONE; 002 + 003-chat-issue-preview parked at human-gated release)
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
  developer (build F1→F4, one gated slice each).

## Decision log

<!-- append one line per decision, newest last: `YYYY-MM-DD — <decision>`; keep only the last 5 (older history lives in git) -->
- 2026-07-01 — Ralph loop fired for 003 ("finish + release"): Mateo's call =
  FINALIZE THE SPEC ONLY then; **STANDING GATE: releases require Mateo present**
  (classifier blocks push/tag/self-merge without an explicit `Bash(...)` rule).
  [004's /sdd-loop arming IS the build re-authorization; release stays gated.]
- 2026-07-01 — 003 CODE COMPLETE + SHIPPED to develop. `vite build` GREEN (needed
  `--max-old-space-size=6144`). Issue **#198** + PR **#199**. Browser QA at
  STAGING; tagged release Mateo-gated. Board asks = Kanban/status/projects specs.
- 2026-07-01 — Drafted spec **004-workspace-issue-loop** from Mateo's ask (issue
  flow + status movement missing; workspace creation should create the GitHub
  issue + the spec). Findings: GitHub tracker transition is a logged no-op while
  drive.rs already fires InProgress/ReadyToTest/Done; composer can only LINK
  issues; `POST /api/worktrees/create` drops linked metadata; no code authors a
  spec.md (helpers MCP-only). PM gate passed → phase `pm`.
- 2026-07-01 — 004 PM gate PASSED (autonomous; loop-armed = adopt recommendations).
  D1 Done=label-only. D2 gh CLI. D3 labels `status/todo|in-progress|ready-to-test|
  done` (ensure-created, exactly one per issue). D4 build F1=status-transition
  first. D5 scaffold opt-in/off. AC 4 fixed (thread `tracker_url` through the
  seam). Mode → auto (/sdd-loop). Phase → architect.
- 2026-07-01 — 004 architect blueprint COMPLETE (`architecture.md`). Corrections
  C1–C5 (Tasks-page create is a local stub → new `POST /api/github/issues`; UI
  shim strips linked fields → widen 2 TS layers; `linkedPR` wire fix + registry
  NO-alias rule (duplicate-field → `read_worktrees` wipes to `[]`); remove only
  the 3 canonical labels; direct local gh, no Host). Confirmed `board_sync`
  cannot strip labels (PATCH = `{title,body,state}` only). Merged develop
  (+35 commits, incl. 003's `task_sink.rs` labels support) → line cites drift,
  re-locate before editing. Phase → developer.
