# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 004-workspace-issue-loop
- **phase:** architect   <!-- idle | spec | pm | architect | developer | tester | reviewer -->  (004 PM gate passed; 002 parked at human-gated release)
- **mode:** auto         <!-- HITL (human in the loop) | auto -->  (set by /sdd-loop 2026-07-01; NEEDS-HUMAN exit is the safety valve)
- **execution:** harness <!-- features land via the .harness/ engine + green gate -->

## Active send-backs

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
  `apply_tracker_transition` (`task_sink.rs:278-282` is a logged no-op; drive.rs
  call sites already correct); (C) issue→`.agentum-harness/specs/<id>/spec.md`
  scaffold over an HTTP seam (helpers exist, MCP-only today). ✅ PM-locked D1–D5
  (Done=label-only, gh CLI writes, `status/*` canon labels, one spec built
  status-sync-first, scaffold opt-in/off); AC 4 softened — the GitHub arm needs
  the repo slug, so threading `feature.tracker_url` through the seam
  (`drive.rs:321`) is the one permitted drive.rs touch. Handoff:
  `ai/specs/004-workspace-issue-loop/handoffs/01-pm-to-architect.md`. Phase →
  architect.

## Decision log

<!-- append one line per decision, newest last: `YYYY-MM-DD — <decision>`; keep only the last 5 (older history lives in git) -->
- 2026-06-30 — 002 scope LOCKED (Mateo): creation is fine (installed app behind, not
  a bug) → **Start-only**; Start runs **directly off the external ticket** (no card,
  live body fetch). Ready for architect.
- 2026-06-30 — 002 architect blueprint complete (`architecture.md`). FINDING: the
  spec's Path A (board-card Start) has NO UI caller (dead code); the live "start a
  ticket" flow is Path B (Tasks "Use" → local PTY; snapshots Linear body, not
  GitHub). ⛔ R1 (human gate): Option A (new server "Start", spec-faithful) vs
  Option B (fix "Use", lighter, local-PTY). /loop paused for R1.
- 2026-06-30 — R1 → **Option B** (Mateo). Developer DONE + pushed (`e0faf420`):
  server `GET /api/github/issue` (`gh issue view --json title,body`, numeric-id
  guard, authed, outside `/api/board`) + UI client + GitHub linked-context snapshot
  + `openComposerForItem` folds the body into the agent prompt (graceful fallback).
  npm build + cargo test (453/0) green; AC-3 held. **/loop STOPPED at the
  human-gated release** (browser QA + merge/promote/tag = Mateo).
- 2026-07-01 — Drafted spec **004-workspace-issue-loop** from Mateo's ask (issue
  flow + status movement missing; workspace creation should create the GitHub
  issue + the spec). Research findings: GitHub tracker transition is a logged
  no-op (`task_sink.rs:278-282`) while `harness/drive.rs` already fires
  InProgress/ReadyToTest/Done at the right points; the composer can only LINK
  issues (create lives on Tasks/Chat, disconnected); local
  `POST /api/worktrees/create` drops linkedIssue/PR metadata (`worktrees.rs:351`);
  no code authors a spec.md (scaffold/plan helpers exist but are MCP-only, no
  HTTP seam); `board_sync.rs:456-478` already closes GitHub issues via
  `forge_send` REST (proven write path). PM gate passed → phase `pm`.
- 2026-07-01 — 004 PM gate PASSED (autonomous; loop-armed = adopt recommendations).
  D1 Done=label-only (no auto-close; per-repo toggle deferred). D2 transitions via
  gh CLI. D3 labels `status/todo|in-progress|ready-to-test|done` (ensure-created,
  fixed colors, exactly one per issue). D4 one spec, build order
  F1=github-status-transition first. D5 scaffold opt-in, off by default. Fixed:
  AC 4's "zero drive.rs changes" was unsatisfiable (seam gets only `feature.id`;
  GitHub arm needs the repo slug from `tracker_url` — `drive.rs:321` widening
  allowed). Mode → auto (/sdd-loop). Phase → architect.
