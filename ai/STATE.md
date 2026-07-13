# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 020-ssh-host-tracker-plumbing
- **phase:** done         <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (020 **SHIP-READY — Reviewer SIGN-OFF 2026-07-13**, `review.md` @ `cc4bde36`, 0 blockers; spec Status → Done. Commits F1 `09726c46` F2 `e8fb31a8` F3 `820712d9` on `fixes-new-workspace`, on top of ship-ready 015. **RELEASE = HUMAN**: ONE train with 015 (same branch) — PR → develop → staging qa.sh (live dyaus binding, SSH filing + grounding note, Start-work direct launch, host-down 422-flavor vs slug-route 502, gh authed on the remote) → main + tag. Follow-up ticket (reviewer should-fixes): SF1 ProjectHubPage:86 Tasks-tab binding read not repoId-threaded — bound SSH repo's Tasks tab never auto-enters board mode; SF2 SSH-repoId issue FETCH composes local neutral_cwd with remote gh — caller-less today but the live wire will trip the deferred QA leg (fix before/at QA); SF3 tasks.md wording. 015's own release checklist + 010's AC-11 demo stay in Active send-backs.)
- **mode:** auto         <!-- HITL (human in the loop) | auto -->  (set by /sdd-loop 2026-07-01; NEEDS-HUMAN exit is the safety valve; RELEASE stays human-gated)
- **execution:** harness <!-- features land via the .harness/ engine + green gate -->

## Active send-backs

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
- 2026-07-13 | Developer | **020 F2 CODE-COMPLETE + GREEN + COMMITTED
  `e8fb31a8`** (slug-index-ssh, AC 5-7; tasks.md F2; phase STAYS developer →
  F3 last). NEW `GET /api/repos/{id}/slug` in repos.rs (registry path, no
  hint/workdir; 404 unknown / 422 no_github_remote / **502 host_unreachable**
  via pure `slug_reason_wire` — transport never masquerades as no-origin;
  behind require_token, no is_public change); `getServerRepoSlug` in
  server-repo-client.ts (run() throws on non-2xx = fail-closed); NEW pure
  import-free `lib/repo-slug-arm.ts` arm-picker (env-RPC > server-for-
  connectionId > native) wired into repo-slug-index.ts inside the existing
  try/catch (throw → null cached → EXCLUDED, AC 7); env-RPC + native arms
  byte-identical; slugByRepoId cache untouched. Gates: cargo 701/0/5 (696+5),
  fmt+clippy -D warnings clean, vite green, vitest 11/0 (arm-picker 4 +
  start-work-repo-match pins: sole-remote→direct, both-hosts→choose;
  classifier file UNTOUCHED). Test-first RED both sides. 1 real deviation
  (handler core extracted as `slug_on_host(&Host,&str)` so origin tests use a
  temp git repo, wire-identical). Tester note: host-down = real 502 — qa.sh
  can key on status not message. Orchestrator-gated PASS. F3 unblocked.
- 2026-07-13 | Developer | **020 F3 CODE-COMPLETE + GREEN + COMMITTED
  `820712d9` → DEVELOPER PHASE DONE, phase → tester** (intake-ssh-honest,
  AC 8-10; tasks.md F3; handoff `03-developer-to-tester.md`). Server:
  `DraftedIssue {body, grounded_repo, grounded_wiki}` (chat.rs) →
  always-present add-only `grounding: {repo, wiki}` on DraftBodyResponse +
  serde pin (github.rs). Clients: pure `bindingQuery`/`createIssuePayload`;
  repoId? on binding get/put/delete + issue create/fetch; grounding? on
  DraftedGithubIssueBody (labels route unwidened per spec). UI:
  ProjectBindingEditor repoId prop → 4 calls (ProjectHubPage,
  IntegrationsPane + sanctioned local-filter drop, CreateWorkspaceWizard
  trackerRepoId — workdir gate NOT relaxed); use-tracker-intake repoId on
  binding+file, draft stays slug-first (amended AC 8), grounding state +
  `deriveDraftGroundingNote` (renders ONLY when repo===false) + muted
  TrackerIntakePanel note. Gates: cargo 701/0/5 held, fmt+clippy clean, vite
  green, vitest 5 files 53/0 (015's 26 model cases UNMODIFIED, F2 arm-picker
  held). Test-first RED both sides. 5 deviations documented (top: serde pin
  amended in place; trackerRepoId extra hop through AgentStep). **All three
  slices green: F1 `09726c46` F2 `e8fb31a8` F3 `820712d9` → tester re-runs
  everything independently.**
- 2026-07-13 | Tester | **020 verdict PASS-WITH-DEFERRALS, 0 defects → phase
  reviewer** (`verification.md`, artifacts commit `a7275ad1`; handoff
  `04-tester-to-reviewer.md`). Independently reproduced ALL gates @
  `820712d9`: cargo 701/0/5 (delta arithmetic corroborated 687+9+5=701, F3
  one-for-one pin swap), fmt clean, clippy FORCED-recompile clean, vite
  39.3s, vitest 5 files 53/0 (015's 26 intent-model cases 0-deletion
  unmodified). Sacred proofs: EMPTY diffs on start-work-repo-match(+test),
  native gh.rs, the WHOLE board_goals.rs (resolve_github_slug/SlugReason
  trivially unchanged), task_sink.rs, auth.rs; both duplicate resolvers
  gone (repo-wide grep 0); wizard gate byte-same; env-RPC/native arms
  byte-identical bodies; zero serde-alias/is_public code changes. ACs
  1/2/4/5/7/10 PASS, 3/6/8/9 PASS(deferred live-SSH legs = qa.sh/staging).
  Key reads: ordering test GENUINELY pins unknown-repoId-beats-valid-hint;
  zero-I/O hint test sound; `grounding` non-optional on success. 15/15
  deviations ACCURATE. 6 spot-checks clean (502 hygiene: static messages,
  payload-free SlugReason; stale connectionId="" routes native/falsy;
  legacy no-repoId byte-identical). 4 nits non-blocking. Reviewer focus:
  cross-repo repoId/workdir ruling, file-leg unconditional repoId
  loud-failure, create_issue failure-ordering move.
- 2026-07-13 | Reviewer | **020 SIGN-OFF → SHIP-READY, phase → done**
  (`review.md` @ `cc4bde36`, 0 blockers; full `3ec6f028..820712d9` diff read
  hunk-by-hunk, sacred empty-diffs re-verified independently). All 10 focus
  items PASS w/ quoted evidence: mismatch ruling ACCEPTED (coherent pairs by
  construction — PATCH refuses identity edits, re-add mints new id; F2 route
  takes no workdir); file-leg loud 404 = D1-correct; create_issue ordering
  move unobservable (local-host still precedes any gh call, comment-pinned);
  absent-repoId byte-identical per route, unknown-repoId beats valid hint
  (test-pinned), only Some(id)→local edge is the 015-sacred no-host_id repo;
  wire 404/422/502 correct + SlugReason payload-free + &'static str messages
  → SSH stderr/token structurally can't leak; renderer fail-closed w/
  immutable cache key; grounding non-Option + note only on repo===false;
  D5 all 0-line diffs + hint precedence preserved; 6 add-only
  serde(default) widenings, zero aliases; require_token merge verified
  (lib.rs:336/349), no polling, no spawn code. 3 should-fixes → ONE
  follow-up ticket (SF1 Tasks-tab binding read unthreaded = next user
  dead-end; SF2 fetch's neutral_cwd×remote-gh compose = latent 400 on the
  caller-less wire, will trip deferred QA; SF3 docs wording). 4 leave-as-is
  nits. spec.md Status → Done. **RELEASE = HUMAN, one train with 015.**
- 2026-07-13 | Merge | **origin/develop @ `4a184993` merged into
  `fixes-new-workspace`** (44 commits, incl. v0.75.0 "015-workspace-harness-
  autostart" + v0.75.1 #359 SSH-binding wizard fix + sdd-bar/tracker-status
  work — all merged clean except the 020-overlap files). Reconciliation:
  #359's GET `host_id` param is SUPERSEDED by 020's `repoId` (D1: wire
  identity = the repo, never a client-asserted host) — the old
  `github_projects::resolve_slug` stays deleted; #359's expand-skip-for-SSH
  folded into `util::resolve_tracker_slug` (new host-aware
  `util::effective_workdir`, + the same fix in github.rs
  `fetch_github_issue`; +1 test pin); #359's `deriveTrackerBindingTarget`
  KEPT (wizard tracker section now reads bindings for SSH repos) but
  migrated to `{workdir, repoId, local}` — its 5 vitest pins adapted
  hostId→repoId/local, configure editor stays local-gated, F3
  `trackerRepoId`→ProjectBindingEditor threading preserved.
