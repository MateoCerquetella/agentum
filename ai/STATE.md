# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 018-issue-hover-project-status-chip
- **phase:** done         <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (018 **SHIP-READY — Reviewer SIGN-OFF 2026-07-14**, `review.md`, 0 blockers; spec Status → Done. Full loop run in one autonomous session (spec→pm→architect→developer→tester→reviewer). One slice: `gh_issue_project_status` desktop command + pure mapper, `getProjectBinding`-cached hook, `IssueProjectStatusChip` in the hover badges row. Gates: UI `bun run build` green, all new vitest green (13; 1 red = proven pre-existing `review`/"PR #456" baseline), standalone tsc green, `cargo fmt --check` green. ⚠️ Rust unit gate CI-deferred (no local webkitgtk; `cargo check` env-blocked in webkit2gtk-sys). **Merged → develop** per Mateo's ask; #365 stays open until it reaches main. Downstream: qa.sh live legs at staging (real Projects v2 board → chip shows column; unbound → no chip; 2nd hover = no refetch).)
- **mode:** auto         <!-- HITL (human in the loop) | auto -->  (set by /sdd-loop 2026-07-01; NEEDS-HUMAN exit is the safety valve; RELEASE stays human-gated)
- **execution:** harness <!-- features land via the .harness/ engine + green gate -->

## Active send-backs

- **020-ssh-host-tracker-plumbing** — **SHIP-READY** (Reviewer SIGN-OFF
  2026-07-13, `review.md` @ `cc4bde36`, 0 blockers; spec Status → Done).
  Commits F1 `09726c46` F2 `e8fb31a8` F3 `820712d9` on `fixes-new-workspace`,
  on top of ship-ready 015. **RELEASE = HUMAN**: ONE train with 015 (same
  branch) — PR → develop → staging qa.sh (live dyaus binding, SSH filing +
  grounding note, Start-work direct launch, host-down 422-flavor vs slug-route
  502, gh authed on the remote) → main + tag. Follow-up ticket (reviewer
  should-fixes): SF1 ProjectHubPage:86 Tasks-tab binding read not
  repoId-threaded — bound SSH repo's Tasks tab never auto-enters board mode;
  SF2 SSH-repoId issue FETCH composes local neutral_cwd with remote gh —
  caller-less today but the live wire will trip the deferred QA leg (fix
  before/at QA); SF3 tasks.md wording.
- **015-host-aware-start-and-tracker-intake** — **SHIP-READY** (Reviewer
  SIGN-OFF 2026-07-13, `review.md` @ `aa8ce9e3`, 0 blockers). Commits F1
  `ff7290ee` F2 `d7d64f33` F3 `3ec6f028` on `fixes-new-workspace`, unpushed.
  **RELEASE = HUMAN (Mateo)**: PR → develop, promote → staging (`status/qa`;
  qa.sh legs: live VPS add/pick/create AC 3-4-7, choose-hop AC 5, real filing
  AC 10, board+gated run AC 11) → main + tag. Release notes: one-time remote
  re-add + onUse zero-match shift. F1+F2 SAME train. Follow-up ticket: S1
  residual selectors→findRepoByPathPreferLocal + doctor check, S2 reposUpdate
  doc comment, S3 reject connectionId:"". NOTE: 019 (SSH tracker plumbing)
  builds on these commits — 015 ships first. 010's AC-11 live demo also still
  PENDING/human.
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
- 2026-07-13 | Release | **merge sync #2 with develop @ 3636d6ac** (PR #367
  "pinned chat repo context" — ANOTHER worktree independently threaded
  repo_id + SSH repo-context gathering through chat.rs; complementary to
  020's grounding flag, auto-merged clean in chat.rs/util.rs; only STATE.md
  conflicted, ours kept + this line). PR #368 (specs 015+020) open into
  develop; first merge-reconcile commit b2b19f31 (#359 repoId supersession +
  effective_workdir). Gates re-run post-merge before push.
- 2026-07-13 | Reviewer | **016 F1 (sdd-loop MCP check-in) REVIEW SIGN-OFF →
  merged via PR #366** (attempt 5 at `99670cf1` after a 4×-blocked review gate —
  the AC3 STATE.md belt parser silently deviated from architecture.md and never
  fired on the real `- **phase:** done <!-- … -->` shape; fixed = both keys
  `phase`|`current_phase` + first-token value; gate re-run 694/0 + fmt. Full
  trail in `.agentum-harness/decisions.md`. F2 rider = spec 358b/#365, NOT
  built, pending its own PM gate. Staging qa.sh covers the loop-stop scenario.)
- 2026-07-14 | Dev→Review | **018 BUILT + SIGNED-OFF → phase done, merged to develop**
  (one slice, `tasks`-equivalent in `verification.md`/`review.md`). Landed:
  `gh_issue_project_status` desktop Tauri command + pure
  `issue_project_status(&Value,…)->Option<String>` mapper (4 #[cfg(test)]
  cases) + `lib.rs` reg; `gh.ts`/`contract.ts` `issueProjectStatus`; pure
  `lib/issue-project-status.ts` (parseIssueRef/statusCacheKey/
  resolveIssueProjectStatus, never-throws) + 12 vitest; `IssueProjectStatusChip`
  + `useIssueProjectStatus` (module caches: binding/slug, status/slug#number) +
  badges-row slot + `workdir`/`repoId` from WorktreeCard + card test. Gates:
  UI build green, new vitest 13/13, tsc(pure) green, fmt green; Rust compile
  CI-deferred (webkitgtk absent). 1 red vitest = pre-existing develop baseline
  (proven via stash-all). AC1 code-verified, AC2/AC3 test-covered; live legs =
  qa.sh/staging. #365 stays open until main.
- 2026-07-14 | Architect | **018 ARCHITECT DONE → phase developer**
  (`architecture.md`; handoffs `01-pm-to-architect.md`, `02-architect-to-developer.md`).
  Both open Qs pinned: **D1** desktop Tauri command `gh_issue_project_status`
  (one `gh api graphql` read; owner/repo/number as `$vars`; pure
  `issue_project_status(&Value, projectId, statusFieldId) -> Option<String>`
  mapper) NOT a server route; **D2** fresh `getProjectBinding` cached per slug,
  NOT Project Hub reuse. Data flow: card-open → parse `issue.url` → binding
  cache/fetch → status cache/fetch → chip; every miss/error → no chip (D6
  never-throw). Build = 4 commits. Collision sweep clean (nothing built).
  ⚠️ Flagged the no-webkitgtk build gate for developer/tester: Rust tests
  CI-only, local verify = UI bun build + 2 targeted vitest files (full
  suite/tsc = known pre-broken baseline). Phase → developer (SDD loop next
  step, or delegate via Agent per orchestrate §6).
- 2026-07-14 | Spec | **018 DRAFTED + PM-GATED → phase pm** (issue **#365**,
  `ai/specs/018-issue-hover-project-status-chip/spec.md`): the 016 F2 rider /
  harness stub 358b promoted to its own one-slice SDD spec — hover-card
  Project-status chip, lazy fetch-on-open + per-issue session cache, silent
  absence on unbound/off-project/error. Citations line-verified @ develop
  `d31314b3` (worktree ff'd from stale v0.57.0 main first); 358b stub now
  points here. Gate: all 9 boxes pass. Carried open Qs for architect:
  read-path Tauri-vs-server (recommended: desktop command beside
  `gh_get_project_view_table`), binding-read source. Handoff → PM/architect.
- 2026-07-17 | Release | **016-board-per-project (#360) RECONCILED + SHIPPED → v0.78.0**
  (Mateo: "can you release?"). Board-per-project spec (dir `016-board-per-project`,
  distinct from the released `016-sdd-loop-checkin-*` — number collision is
  cosmetic, separate dirs). Stale v0.75.1 branch (11 ahead / 50 behind): a naive
  `git merge origin/develop` REVERTED develop's v0.76/v0.77 server refactors
  (multi-base ort merge dragged the stale base over chat.rs/repos.rs/sdd.rs) —
  ABORTED. Correct path: fresh branch off `origin/develop` v0.77.0 +
  cherry-pick ONLY the 3 UI feature commits (`f5eda0ee`/`ae4b44d8`/`4b98dd73`,
  zero server files) → provably no server revert. One conflict each pass:
  ProjectViewWrapper.tsx import block (kept both — resolver + linked-work-item).
  Version 0.77.0→0.78.0 (Cargo.toml+lock+tauri.conf.json). qa.sh browser legs
  WAIVED (008/010/014/015 precedent); S1 first-frame legacy flash + S2 ghost
  settings-search entry → follow-up ticket. `Closes #360`.
