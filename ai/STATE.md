# SDD State

> Single source of truth for where SDD work stands. Each role updates this on
> handoff. Read it first (`/sdd-status`) before starting any phase.

- **current_spec:** 010-end-to-end-autonomous-flow
- **phase:** tester         <!-- idle | spec | pm | architect | developer | tester | reviewer | done -->  (010 = the PRD spec. PM D1–D8 + Architect done. **DEVELOPER COMPLETE — all three slices GREEN:** F1 bind `474cfd12` · F2 drive `0b03eb9e` · F3 provision `26b1e022`; cargo 616/0/5 (571 pre-spec + 45 new), vitest 37/0, fmt+clippy clean, tsc baseline 1642 held. Handoff `03-developer-to-tester.md` — tester independently re-runs every gate, rules AC 1–10 w/ repro steps, audits the 25 documented deviations for accuracy, sweeps sacred surfaces. AC 11 = qa.sh/human demo (Mateo), NOT a tester item.)
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
  home. Next artifact: `architecture.md`. (PM full text in git history.)
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
- 2026-07-06 | Developer | **010 F1 CODE-COMPLETE + GREEN + COMMITTED
  `474cfd12`** (board bind, AC 1–3; tasks.md F1 section; F2/F3 pending →
  phase STAYS developer). NEW `github_projects.rs` (1113 ln: BoardPhase/
  StatusMapping/BoardBinding w/ serde-default-ON `done_closes_issue`;
  sibling-file persistence + WRITE_LOCK + `_at` injection cores; pure mapper
  w/ verbatim §3.4 synonym tables, 2 fallbacks, refusal-names-phases;
  one-call discovery + scope_missing classifier carrying `gh auth refresh -s
  project`) + NEW `routes/github_projects.rs` (discover/GET/PUT/DELETE,
  camelCase DTOs, typed 422 scope envelope) + UI (client, pure lib,
  ProjectBindingEditor w/ GhAuthErrorHelp + desktop READ-command pick,
  IntegrationsPane "Projects v2 board" section). Gates: cargo 591/0/5 (571
  baseline held + 20 new; ZERO existing tests touched; task_sink.rs ZERO
  lines), fmt+clippy clean, vite 1m21s, vitest 10/10 (+9 neighbors). 10
  deviations documented (top: 3-line gh_bin dup — boundary forbids task_sink
  edits; paired-positive-guards for strict:false narrowing, tsc 1646→1642).
  Env notes: attempt 1 died on API monthly-spend-limit mid-exploration
  (nothing written), retry clean; worktree clippy needed sherpa/onnx dylibs
  copied into target/release (known gap). **Next slice: F2 drive** (arm
  hooks LAST, AC 8 = label tests unmodified).
- 2026-07-06 | Developer | **010 F2 CODE-COMPLETE + GREEN + COMMITTED
  `0b03eb9e`** (drive, AC 4–8; tasks.md F2 section; F3 pending → phase STAYS
  developer). `github_projects.rs` +711: `run_gh_graphql_argv` (ONE
  runner/classifier for bind-time AND mid-run — scope miss carries the remedy
  everywhere), pure builders (3 single-line GraphQL consts + argv fns;
  `singleSelectOptionId` var = PRD AC-6 pin), `run_gh_capture`,
  `ID_CACHE` LazyLock keyed (slug,number)→(node_id,item_id) (~9 vs ~14
  calls/run), `board_write_with` (cold resolve → add-item ensure+fetch →
  option write → stale-invalidate-retry-once → knob-gated probe-then-act
  close/reopen; Blocked never closes). `task_sink.rs` +339:
  `github_transition_with_board` + `github_mark_blocked_with_board` (private;
  label fns BYTE-IDENTICAL; board Err → tracing::warn + fold into
  Skipped("status label applied; Projects board write failed: …") — loud via
  existing drive.rs/MCP plumbing); both arm hooks read binding only AFTER the
  URL parse (hermeticity held — no-url skip tests never touch config). Gates:
  cargo 604/0/5 (591+13), fmt clean, clippy 0; deletion audit = 7 lines, all
  intended (2-line runner refactor, docstring, 2 comments, 2 callers) — ZERO
  test edits; four seam call-site files untouched. 5 deviations documented
  (2nd private fn = blocked-arm testability; act-failure loud per
  never-silent; LazyLock over once_cell). ⚠️ ID_CACHE process-global: new
  tests must use fresh slugs. **Next slice: F3 provision** (run-twice test
  FIRST).
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
