# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 003-chat-issue-preview
- **phase:** developer   <!-- idle | spec | pm | architect | developer | tester | reviewer -->  (003 SERVER slice DONE+pushed; UI slice + /ship remain. 002 done/human-gated below.)
- **mode:** HITL         <!-- HITL (human in the loop) | auto -->
- **execution:** harness <!-- features land via the .harness/ engine + green gate -->

## Active send-backs

- **003-chat-issue-preview** — Drafted + PM-gated (`ai/specs/003-chat-issue-preview/spec.md`).
  Grounded on a FRESH `feat/chat-board-revamp` worktree off `origin/develop` @ v0.46.1
  (the `chat-revamp` worktree the user invoked from was **258 commits behind** — v0.26;
  even local `develop` is stale). Scope (Mateo): Chat gets an editable **draft before
  filing** — preview + edit + regenerate + confirm, with **full multi-issue control**
  (split one-issue-checklist vs one-per-task, provider GitHub/Linear, labels). Key
  finding: `chat_issues` (`chat.rs:950`) files atomically from a **second** extraction
  call, so the filed issue can differ from the prose shown; the draft pieces
  (`extract_feature_plan` `chat.rs:1197`, `compose_issue_body` `chat.rs:914`,
  `provider` field `chat.rs:846`) already exist — no endpoint returns a plan without
  filing. ⚠️ **One-slice caveat for architect:** labels + `per_task` split are
  deferrable to a 003b (Open-Q5); the spine (preview/edit/regenerate/confirm,
  single-issue GitHub) is the core increment. Board asks → roadmap 004-006. Ready for
  architect.
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

## Decision log

<!-- append one line per decision, newest last: `YYYY-MM-DD — <decision>` -->
- 2026-06-30 — Adopted the `ai/` SDD scaffold; retired `docs/superpowers/`.
  Execution runs through the Harness Engine (`.harness/`), gated by
  `verify.sh` + `qa.sh`.
- 2026-06-30 — From the "Ara" idea, scoped spec **001-autowiki** as the first
  slice (AutoWiki: generate + browse a repo wiki on demand). Deferred graph
  memory + autonomous PR-review bot; kept `examples/harness-demo` (live-test
  fixture).
- 2026-06-30 — Found `new-idea` was 237 commits behind `origin/develop` (HEAD
  v0.26). Re-based onto a fresh `feat/autowiki` worktree off `origin/develop` @
  `fe1a2a6a`; re-grounded reuse-vs-build on current code. Mermaid diagrams pulled
  into v1 (renderer already draws them); persistence is on-disk `.agentum/wiki/`.
  PM gate passed → phase `pm` (ready for architect).
- 2026-06-30 — Architect blueprint complete (`architecture.md`): job-model generate
  (returns `sessionId`); on-disk `.agentum/wiki/` + `.status.json`; reuse
  `MarkdownPreview` as-is; git-ignore via `.agentum/.gitignore`; widen
  `wait_for_settle`/`teardown_session`/`gather_repo_context` to `pub(crate)`. Phase
  → developer; scaffolded `.harness/`, building slices behind the verify gate.
- 2026-06-30 — Slice 1 (wiki-contract) GREEN: `crates/agentum-server/src/wiki.rs`
  (WikiIndex/WikiPageMeta, parse_wiki_index, is_valid_slug, build_wiki_prompt) +
  `lib.rs` mod; 9 unit tests pass, fmt-clean (reverted unrelated mcp.rs fmt drift).
  Gate run scoped to `wiki::` (full lib suite times out locally). Next: wiki-routes.
- 2026-06-30 — Slice 2 (wiki-routes) GREEN: `routes/wiki.rs` (GET list/page + POST
  generate via `spawn_agent_into_pane` + the QA-capture recipe; on-disk
  `.status.json`; slug-traversal guard; job-model returns sessionId). Widened
  `wait_for_settle`/`teardown_session`/`gather_repo_context` to pub(crate) +
  re-exported from `harness`. 14 wiki tests pass. Next: wiki-view (desktop UI).
- 2026-06-30 — Slice 3 (wiki-view) BUILD-GREEN: `runtime/wiki-client.ts` +
  `components/wiki/WikiPage.tsx` (reuses `MarkdownPreview` as-is; empty/running/
  failed/ready states; workdir via `splitWorktreeIdForFilesystem`; `[[Title]]` nav;
  3s poll for running→ready) + 6 store/nav edits (`BookText` rail). `bun install` +
  `npm run build` ✓ (9m23s). All 3 slices green at the unit/build gate; browser QA
  + commit/PR remain.
- 2026-06-30 — AutoWiki committed (`3a8dbf06`) + pushed; issue #182, PR #183 into
  develop (MERGEABLE; the merge was blocked by the safety classifier — user merges).
  An env reset then wiped the local autowiki worktree (work safe on origin).
- 2026-06-30 — Scoped spec **002-start-loads-spec** (Start an external ticket → the
  agent gets the spec, no internal board). Grounded on current develop in a fresh
  `feat/chat-spec-roundtrip` worktree off `origin/feat/autowiki` (a stale-reading
  subagent had to be discarded). Finding: chat creation already does
  title+body+external-only; the gap is **Start** not feeding the issue body to the
  agent. PM-gated; awaiting the user on 4 open questions.
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
- 2026-07-01 — 003 SERVER slice DONE (Ralph loop iter, Mateo re-authorized full
  build). Commits `5930bf27` (spec docs) + `86c618e1` (server), pushed to
  `origin/feat/chat-board-revamp`. Implemented: `POST /api/chat/issues/preview`
  (draft, files nothing) + shared `extract_plan()` + `chat_issues` accepts a
  client `plan` filed VERBATIM (skip re-extract) + `split` single/per_task via
  `plan_to_features()` + `labels`→`gh --label` (`NewFeature.labels`, Linear no-op).
  `cargo check -p agentum-server` + 25 touched unit tests green (fmt-clean; reverted
  unrelated gh.rs/cdp_driver.rs/mcp.rs fmt churn). REMAINING: (4) desktop UI draft
  panel in `ChatPage.tsx` + `chat-client.ts` preview/confirm + `npm run build`;
  then `/ship` (issue + PR into develop). Tagged release still Mateo-gated.
- 2026-07-01 — Ralph loop fired ("finish this spec and release with /ship").
  Spec *document* is finished (drafted + PM-gated + open questions listed — that IS
  the /sdd-spec deliverable). "Finish → release" would be the full gated SDD build
  (architect→dev→test→review) + a **human-gated release** (classifier blocks
  push/tag/self-merge without an explicit `Bash(...)` rule + Mateo present — verbal
  "go" ≠ enough; ephemeral env has wiped mid-session work before). **Mateo's call:
  FINALIZE THE SPEC ONLY** — do NOT run architect/build/release; he/the team drive it
  from here. Loop kept running as-is (will re-feed; standing instruction = hold at
  "spec finalized", don't auto-build/release). STANDING GATE for any future
  iteration: the build + release require Mateo to explicitly re-authorize (and the
  release still needs his presence + a permission rule).
- 2026-07-01 — Scoped spec **003-chat-issue-preview** from Mateo's ask ("chat
  creation is too simple — let me see/regenerate what gets created"). Re-grounded on
  a fresh `feat/chat-board-revamp` worktree off `origin/develop` @ v0.46.1 (the
  `chat-revamp` worktree the ask came from was 258 commits behind; two initial
  Explore passes on it read stale v0.26 code and were re-run on develop). Finding:
  Chat files atomically from a separate extraction call (no preview; filed ≠ shown);
  draft pieces already exist. Answers (Mateo): spec 003 = chat preview FIRST; depth =
  **full multi-issue control** (split/provider/labels); board end-state =
  **projects-first** (→ roadmap specs 004 Kanban-read, 005 status write-back, 006
  projects-first). PM gate passed (one-slice flagged: labels/split → possible 003b) →
  phase `pm`, ready for architect.
