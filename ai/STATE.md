# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 015-workspace-harness-autostart
- **phase:** tester         <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (015 Developer phase DONE 2026-07-13: f1 `66f1e161` + f2 `03f2eb2b` + f3 `41cbeab8` + docs `5f753260`. Gates green per slice — final 50/0 across the 4 targeted vitest files + vite build ✓ ×3. 3 documented deviations (accept swallow-and-toast w/ .finally busy-reset; banner test mocks offer lib; trigger pins as new describe block). Orchestrator spot-check: scope = exactly the architected 15 files, no must-not files (useComposerState/planCreatedWorkspace/Rust untouched), open-created-workspace.test.ts additions-only. Tester: independently re-run gates + audit deviations + pin byte-identity.)
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
- 2026-07-06 | Developer | **010 F3 CODE-COMPLETE + GREEN + COMMITTED
  `26b1e022` → DEVELOPER PHASE DONE, phase → tester** (provision, AC 9–10;
  tasks.md F3; handoff `03-developer-to-tester.md`). NEW crate-root
  `provision.rs` (~1050 ln: template argv pins + `parse_project_create_output`
  frozen from REAL gh 2.92.0; `create_repo_from_template` probe⇒clone /
  missing⇒create --clone; `provision_repo` 4-step injectable ensure — own
  5-label loop over the two pub(crate)-widened builders, project
  link-or-create GUARDED by binding-exists, `scaffold_harness` wrapped,
  consent-gated commit w/ STATE-ONLY .gitignore rewrite + porcelain-empty
  no-commit + plain push red-nonfatal) + NEW `routes/provision.rs`
  (repo-from-template + workspace/provision, traversal-proof validators).
  UI: pure `workspace-provision-step.ts` (+15 vitest), 4th
  OPTIONAL_WORKSPACE_STEPS entry, goal-step template mode (registers via the
  TRACED existing `addRepoPath` action), modal-local 'provision' phase
  mounting the SHARED ProjectBindingEditor + D8 consent (exact 5-path list);
  `useComposerState`/`isGoalStepReady`/`initialComposerPhase` untouched.
  Gates: cargo 616/0/5 (604+12; run-twice AC-10 pin written test-first,
  proven RED first), deletion audit = exactly the 2 widening signatures,
  fmt+clippy clean, vite green, vitest 37/0 (only the 4-entry steps pin
  updated), tsc baseline 1642 EXACTLY held. 10 deviations documented (top:
  Option<ProjectChoice>; state_map injection = hermeticity; resolve_slug +
  BLOCKED_LABEL keep-in-sync dups). **All three slices green: F1 `474cfd12`
  F2 `0b03eb9e` F3 `26b1e022` → tester re-runs everything independently.**
- 2026-07-06 | Tester | **010 verdict PASS-WITH-DEFERRALS, 0 defects → phase
  reviewer** (`verification.md`, HEAD `bc4a7310`; handoff
  `04-tester-to-reviewer.md`). Independently reproduced ALL six gates: cargo
  616/0/5 (93.6s), FMT-CLEAN, clippy 0 warnings, vite 1m48s, vitest 37/37
  no-flake, bare-tsc EXACTLY 1642 (baseline held). ACs 1–10 PASS on READ
  evidence (test bodies inspected); AC 11 PASS(deferred: live custom-column
  board demo, qa.sh/human, runner Mateo — 008 precedent). Sacred surfaces
  PROVEN: label fns byte-identical base→HEAD (extracted + string-compared);
  empty diffs on all 4 seam call sites + useComposerState + harness/types +
  auth.rs + desktop gh/gh_projects/github_labels; task_sink's 7 deletions all
  accounted; TrackerPhase 4 variants; TransitionResult no new variant. 25/25
  deviations ACCURATE. 5 adversarial spot-checks clean (AC-7 fold, run-twice
  isolation incl. real rev-list equality, real git check-ignore, seam
  hermeticity — binding read strictly after the two early-return guards,
  unbound 5-invocation byte-identity). 5 Info nits (top: tasks.md F3 vitest
  per-file counts SWAPPED 12/15 not 15/12; 03-handoff github_labels.rs path
  missing `commands/` — tester re-proved at the real path; test-first RED
  narrative session-internal, not in git). Reviewer focus: 3 accepted
  dup-drift risks (gh_bin / BLOCKED_LABEL / resolve_slug), D2 residual
  honesty, Skipped-semantics legibility.
- 2026-07-06 | Reviewer | **010 SIGN-OFF → SHIP-READY** (`review.md`, HEAD
  `8aa8a2d2`, 0 blockers). All 20 focus items PASS w/ quoted evidence: the 3
  accepted dup-drift risks rule sufficient-for-now (ONE consolidation
  follow-up: resolve_slug→routes/util.rs per repo convention; gh_bin single
  owner; BLOCKED_LABEL pub(crate) import or pin-test); D2 two-process RMW
  residual documented HONESTLY (WRITE_LOCK is process-local, all writers
  server-side, TUI has no bind surface, lost write re-bindable); Skipped-fold
  strings name what landed + what failed + the remedy (AC-7 pin carries
  `gh auth refresh -s project` into the run log); D1 knob-gated close/reopen
  w/ ONE default site + unbound byte-identity; D3 zero echo/poll machinery;
  D5 no option mutation (only ADD_ITEM + UPDATE_STATUS mutations exist);
  D7 one component two mounts + refusal→manual-selects; D8 consent commit,
  plain push, no AI trailer; no shell injection (argv-exec everywhere,
  owner_node closed literal set, login always $var); traversal
  unrepresentable; no token leakage (constructed scope message, 240/400-char
  stderr bounds); no new is_public; option IDs never names at write; Err
  cannot escape either github arm; id-cache correctness-independent.
  1 Should-fix = the consolidation chore ticket (post-freeze). 5 leave-as-is
  nits (stale "three skippable" doc word; validate_owner leading '-';
  WRITE_LOCK comment x-ref; close-act fold phrasing; tasks.md count swap —
  recorded in verification.md). spec.md Status → Done. Phase → done.
  **RELEASE = HUMAN** (PR → develop + promote + AC-11 live board demo,
  runner Mateo).
- 2026-07-06 | Release | **010 SHIPPING → v0.60.0** (Mateo: "ship it";
  AC-11 live board demo remains PENDING and human-run — shipped ahead of it
  per Mateo's call, 008 precedent). Issue **#276** (feature, closes via the
  release commit's `Closes` on main) + **#277** (reviewer Should-fix:
  consolidate resolve_slug/gh_bin/BLOCKED_LABEL keep-in-sync dups). Flow:
  version bump 0.59.1→0.60.0 (Cargo.toml+lock+tauri.conf.json only) → PR →
  develop → staging → main (pushed separately) → tag v0.60.0 (fires
  release.yml). Evidence contract for AC-11 stays: issue timeline
  project-status + close events + a demo-pass line HERE.
- 2026-07-13 | Spec/Orchestrator | **015 DRAFTED from GH #301 + loop started
  (autonomous)** (`015-workspace-harness-autostart/spec.md`, commit `cac37bab`;
  sdd-spec direct path — ask recovered from the worktree's linked issue). Slice:
  after new-workspace creation, detect `.agentum-harness/feature_list.json`
  (+ legacy `.harness/`) in the workdir via existing `GET /api/fs/entries`,
  non-blocking "Start Harness run" offer → existing `POST /api/harness` +
  `/{id}/run` (export module-private harness-client fns; no new routes).
  Orchestration decisions logged: (a) worktree branched from stale main → ff'd
  onto origin/develop `40030d8b` BEFORE role work; (b) renumbered 009→015
  (develop carries 009–014, incl. two 013s/014s); (c) `ai/roles/*`,
  `ai/skills/orchestrate.md`, `ai/orchestration/hitl_policy.md`,
  `ai/contracts/templates/*` DON'T EXIST on any branch — running the loop from
  the sdd-orchestrate playbook + validate_handoff.md + specs 004–014 artifact
  precedent (deviation, flag to Mateo); (d) author-run PM gate on the draft =
  PASS. Phase = pm IN FLIGHT: overlap re-check vs 010–014 + line re-verify
  (draft cited the now-DELETED composer files; wizard `CreateWorkspaceWizard.tsx`
  replaced them). Issue #301 = live status board (comment posted).
- 2026-07-13 | PM | **015 PM gate PASS → phase architect** (handoff
  `01-pm-to-architect.md`; fact-check by Explore sub-agent, evidence quoted
  there). NON-DUPLICATE verified: develop's neighbors are scaffold-on-create
  (010 F3 `workspace-provision-step.ts`, writes `.agentum-harness/`, excludes
  feature_list.json) + issue-first gated run (`start_work`) — neither detects
  an existing feature_list. 3 citation fixes (fs.rs:180 not :143;
  `listHarnesses` not `getHarnessStatuses`; Terminal.tsx:1578 not :1569).
  KEY re-decision D1: wizard quick-create AUTO-LAUNCHES the agent
  (`CreateWorkspaceWizard.tsx:344`; launcher mounts only for agent===null /
  gated) → banner must be workspace-view-level, NOT launcher-only. D2
  creation-moment trigger · D3 hide-never-link · D4 export
  startHarness/runHarness/listHarnesses · D5 local-only (engine StartRequest
  has no host field) · D6 gated-run suppression. Q1 (mount mechanics) + Q2
  (context hand-off: store slice vs pending-signal à la
  `pending-session-prompt.ts`) → architect.
- 2026-07-13 | Architect | **015 gate PASS → phase developer** (sub-agent;
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
