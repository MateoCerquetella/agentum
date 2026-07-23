- phase: entered authoring (from executing)
- PM gate (2026-07-13): spec 358 bundled two unrelated slices (SDD-loop MCP
  check-in + issue-hover Project-status chip) — goal had a literal "and",
  zero shared code path or verification surface. Split at the gate: spec 358
  narrowed to the SDD-loop slice (5 testable criteria, non-goals added,
  persona = Mateo mid-run); the chip rider moved to spec
  358b-issue-hover-project-status-chip, pending its own PM gate.
- authoring gate PASS (attempt 2): Narrowed to one slice at the gate: spec 358 = SDD loop stops on agentum_sdd_loop check-in (5 testable criteria, non-goals + persona added, grounded in routes/sdd.rs on develop); the unrelated issue-hover Project-status chip rider was split out to spec 358b (pending its own PM gate).
- phase: entered architecture (from authoring)
- architecture gate PASS (attempt 1): Plan grounded line-by-line at origin/develop tip (253173ad; all spec citations re-verified): 3 files in agentum-server — agentum_sdd_loop tool as thin view over a new agent_checkin seam reusing the toggle-off stop path, prompt-carried generation token for staleness, STATE.md belt before every injection, and a test-demanded deliver seam so all four AC tests run without tmux; sacred inject/settle mechanics and DEFAULT_MAX_STEPS untouched. Open question pinned: dedicated tool, not a report_status op. One flagged additive deviation: optional `generation` tool field, required by the spec's own stale-generation constraint.
- phase: entered decompose (from architecture)
- phase: entered executing (from decompose)
- phase: entered review (from executing)
- reviewer gate FAIL (attempt 1, 2026-07-13): reviewed the full diff
  355f0557..HEAD (mcp.rs, routes/sdd.rs, sdd.rs, verify.sh) against the spec
  and architecture.md. ACCEPTED: F1 — `agentum_sdd_loop` tool + pure parser +
  `agent_checkin` seam, ungated, staleness-guarded, single stop event via the
  shared `abort_and_announce` path (tests: advertised-regardless-of-gate,
  done-stops+agent_completed, no-loop no-op, stale-gen ignored, summary rides
  next step event); F2 — prompt embeds session id + generation + check-in
  instruction, "reply briefly" gone, no-MCP degrade clause present; F4 —
  DEFAULT_MAX_STEPS=10 / SETTLE_GRACE / SETTLE_TIMEOUT and all existing stop
  reasons byte-identical, `drive_sdd_loop_with` seam maps 1:1 onto the old
  arms; F5 — all four spec-named tests present, verify gate green (694/0 +
  fmt). The additive optional `generation` tool field is a NAMED deviation
  (architecture D2) — accepted. REJECTED (the fail): F3's parser
  `state_md_says_done` (routes/sdd.rs:554) keys on `current_phase` with
  exact-value equality, silently deviating from architecture.md, which pinned
  key `phase:` + first-token value extraction, grounded on the real file shape
  `- **phase:** done <!-- idle | … | done -->` (this repo's own ai/STATE.md,
  line 8, uses exactly that shape — key `phase`, inline enum comment on the
  same line). Against that file the belt NEVER fires — wrong key AND the
  inline comment defeats the equality check — so AC3's stop signal for
  MCP-unwired tools is dead in the flagship real-world case; only the
  playbook-literal `current_phase: done` synthetic shape works. The deviation
  is named nowhere (code comment, commit 87b3bb26, decisions.md, handoff).
  Fix: accept both keys (`phase` | `current_phase`) and parse the value as
  the first token so the inline comment can't mask `done` (keep the
  whole-line-field requirement so prose mentions still don't trip), + test
  lines for the real shape. Secondary note (not blocking on its own): qa.sh
  is still the scaffold pass-through, so the spec's qa scenario (loop stops
  after step 1 with `agent_completed`) was never scripted — the QA gate
  passed trivially; stage it or hand it to staging QA explicitly.
- reviewer gate FAIL (attempt 2, 2026-07-13): re-reviewed the same diff
  355f0557..HEAD — no fix commits and no working-tree source changes landed
  since attempt 1, so its rejection stands, re-verified independently rather
  than from memory. Evidence: `state_md_says_done` (routes/sdd.rs) still
  requires key == `current_phase` with whole-value equality; architecture.md
  (lines 59–67) pins key `phase:` + first-token extraction (delimited by
  whitespace or `<`), grounded on the real shape `- **phase:** done
  <!-- idle | … | done -->` — which is exactly what this repo's own
  ai/STATE.md line 8 uses today. Against that shape the belt never fires
  (wrong key, and the inline comment defeats the equality check even with the
  right key), so AC3's stop signal for MCP-unwired tools is dead in the case
  it exists for; only the playbook-literal `current_phase: done` works. The
  deviation remains named nowhere (commit 87b3bb26 states the behavior but
  not that it diverges from the plan). F1/F2/F4/F5 remain ACCEPTED as in
  attempt 1 (constants and stop reasons re-checked byte-identical at HEAD;
  all spec-named tests present). Required fix unchanged: accept both keys
  (`phase` | `current_phase`), parse the value as its first token so the
  inline comment can't mask `done`, keep the whole-line-field requirement,
  and add parser tests for the real `- **phase:** done <!-- … -->` shape.
  Secondary (still not blocking alone): qa.sh remains the scaffold
  pass-through — the spec's QA scenario is unscripted.
- reviewer gate FAIL (attempt 3, 2026-07-13): identical diff re-verified —
  HEAD still 9567bba8, no source change in the working tree, so attempts 1–2's
  rejection stands on fresh evidence, not memory. `state_md_says_done`
  (routes/sdd.rs:553-561) still keys on `current_phase` with whole-value
  equality; architecture.md (specs/358…/architecture.md, lines 58–67) pins key
  `phase:` + first-token extraction delimited by whitespace or `<`, grounded
  on the real shape `- **phase:** done <!-- … -->` — exactly what this repo's
  own ai/STATE.md line 8 uses. Worse, the test at routes/sdd.rs:866 asserts
  `!state_md_says_done("phase: done")` — the architecture-pinned shape is now
  explicitly *rejected* by a test, cementing the silent deviation. Against the
  real file the AC3 belt never fires (wrong key; the inline enum comment also
  defeats the equality even with the right key). F1/F2/F4/F5 remain ACCEPTED
  (re-checked: DEFAULT_MAX_STEPS=10 and stop reasons unchanged at HEAD; all
  four spec-named tests present). Required fix unchanged: accept both keys
  (`phase` | `current_phase`), take the value's first token so the inline
  comment can't mask `done`, keep the whole-line-field requirement, flip the
  `phase: done` test from reject to accept, and add the real
  `- **phase:** done <!-- … -->` shape to the parser tests. Secondary
  (non-blocking): qa.sh is still the scaffold pass-through.
- reviewer gate FAIL (attempt 4, 2026-07-13): HEAD unchanged at 9567bba8, no
  source edits in the working tree — attempts 1–3's finding re-verified from
  the files, not inherited. Checked: architecture.md lines 52–67 pin key
  `phase:` + first-token extraction (delimited by whitespace or `<`),
  grounded on the real shape; ai/STATE.md line 7 in this very repo is
  `- **phase:** developer   <!-- idle | … | done -->` — against its done form
  the implemented `state_md_says_done` (routes/sdd.rs:553-561, key ==
  `current_phase`, whole-value equality) returns false, so the AC3 belt is
  dead for the flagship real case, and routes/sdd.rs:866 asserts
  `!state_md_says_done("phase: done")`, cementing the deviation in a test.
  NEW evidence this attempt: the key choice is not arbitrary —
  sdd-init.md:23 scaffolds `current_phase: idle` and sdd-orchestrate.md:28
  instructs `current_phase: done` — so BOTH shapes exist in the wild and the
  right parser is the union (also note: the architecture-pinned starts-with
  `phase:` parser would itself miss the playbook-literal `current_phase:`
  shape; the fix must accept both keys, which neither the plan nor the code
  does alone). The deviation remains named nowhere, so the "deviation named
  and justified, not silent" gate item fails regardless. F1/F2/F4/F5 remain
  ACCEPTED (re-checked at HEAD: tool ungated + staleness-guarded + single
  stop event; prompt embeds id+generation, "reply briefly" gone;
  DEFAULT_MAX_STEPS=10 asserted, settle constants + existing stop reasons
  unchanged, inject/settle pair wired verbatim; all four spec-named tests
  present). Required fix unchanged from attempt 1: accept both keys
  (`phase` | `current_phase`), take the value's first token so the inline
  comment can't mask `done`, keep the whole-line-field requirement, flip the
  `phase: done` test to accept, add the real `- **phase:** done <!-- … -->`
  shape to the tests, and name the both-keys decision in a comment.
  Secondary (non-blocking): qa.sh is still the scaffold pass-through.
  HARNESS NOTE: this is the 4th consecutive review of an identical diff —
  the run needs to route back to the developer with this fix, not re-review.
- review gate BLOCKED after 4 attempts: AC3 belt parser silently deviates from architecture.md and never fires on the real STATE.md shape: state_md_says_done (routes/sdd.rs:553) requires key `current_phase` with whole-value equality, but the real file (this repo's ai/STATE.md line 7) uses `- **phase:** done <!-- ... -->` — fix by accepting both keys (phase|current_phase) and taking the value's first token so the inline comment can't mask `done`, flip the `phase: done` test to accept, and name the decision; 4th identical re-review — route to developer.
- phase: entered blocked (from review)
- phase: entered review (from blocked)
- developer fix (2026-07-13, routed per attempt-4 HARNESS NOTE): `99670cf1`
  reworks `state_md_says_done` exactly as the review demanded — both keys
  (`phase` | `current_phase`, case-insensitive union; sdd-init scaffolds one,
  real STATE.md uses the other), value = first meaningful token delimited by
  whitespace or `<` (inline enum comment can neither mask nor fake `done`),
  whole-line-field requirement kept, `phase: done` test flipped to accept,
  real `- **phase:** done <!-- … -->` shapes added to the parser tests, and
  the both-keys + first-token decision named in the doc comment. NAMED
  consequence of the pinned first-token rule: out-of-contract free-form
  `current_phase: done pending qa` now reads as done (former reject-test
  removed) — a false stop is the safe direction for a backstop.
- review gate PASS (attempt 5): the single blocking finding is fixed at
  `99670cf1` — the AC3 belt now fires on this repo's own ai/STATE.md shape
  (`- **phase:** done <!-- idle | … | done -->`) and on the playbook-literal
  `current_phase: done`; negative case (`- **phase:** pm <!-- … | done -->`)
  proven by test. F1/F2/F4/F5 unchanged (diff touches only the parser + its
  tests; DEFAULT_MAX_STEPS/settle constants/stop reasons untouched). Gate
  re-run green: 694/0 lib tests + fmt clean. Secondary note resolved by
  explicit handoff: qa.sh stays the scaffold pass-through — the loop-stop QA
  scenario (loop on a done spec stops after step 1 with `agent_completed`)
  goes to staging QA / the release human, per this repo's qa.sh convention.
- phase: entered done (from review)
- 2026-07-21 [ai/spec-024 reviewer]: SIGN-OFF after reconciling the completed
  issue-407 Harness patch with the `ai/` role loop. Confirmed repo-only
  normalized swatch wiring, theme-owned interaction contrast, non-project
  exclusion, fallback tests, 108/108 focused Vitest in both Developer and
  Tester phases, two green production builds, and clean diff checks. Live
  screenshots remain release/manual QA; no code send-back.
- phase: entered authoring (from executing) [run 379, 2026-07-17]
- authoring gate PASS (attempt 1) [run 379]: One-slice per-project tracker picker; persona, user value, and non-goals added; ACs made testable and re-grounded (hardcoded "github" in routes/harness.rs + TaskSink::select env/auto-probe are the seams; per-project issue-URL fields dropped as they contradict the per-feature tracker_url flow).
- phase: entered architecture (from authoring) [run 379]
- 2026-07-17: worktree was 235 commits stale (v0.57.0) — rebased onto origin/develop v0.78.0 before authoring the repo spec (stale-base lesson, memory). Scaffold commit dropped; develop`s tracked .agentum-harness restored; run-379 live files preserved.
- architecture gate PASS (attempt 1): Plan grounded at bb25a97d (incl. the TrackerEmit-era transition seam): UI-owned Repo.trackerProvider mirroring issueSourcePreference, request-threaded like agentTool/agentModel into start-work/spec-from-issue/harness-plan, a pure TrackerChoice parse + a logged-skip "none" arm in transition_inner; drive.rs/types.rs/linear.rs untouched; every AC mapped to a named component and test.
- phase: entered decompose (from architecture)
- phase: entered executing (from decompose)
- 2026-07-17 [run 379]: RECONCILE spec↔architecture — architecture.md (committed 2ffed72d) predates Mateo's live amendment; it targets a RepositoryPane settings picker + "none" option + start-work/spec-from-issue threading only. Adopted FROM it into spec 021: field name `Repo.trackerProvider` and D3 precedence (explicit pin > AGENTUM_TASK_SINK; env governs auto/absent — hermetic tests send no field). Architect must reconcile TO the amended spec: add the shared TrackerSection (New Issue + Chat DraftReview) + the chat resolve_provider seam; DROP the "none" machinery (Mateo: "choose between github and linear"); D1 (UI-owned persistence + request threading) stands, extended to the chat filing path.
- 2026-07-17 [run 379, F5]: feature text predates the amendment ("with tracker 'None'… logged no-op"); executed under the amended spec instead — DROPPED the none machinery rather than implementing it. UI: 'none' removed from TrackerProviderPreference + TRACKER_PROVIDER_OPTIONS + settings copy; resolveTrackerProviderPreference degrades a pre-amendment persisted 'none' to 'auto' (tested). Server already aligned (parse_tracker_choice rejects 'none', F3/F4). Auto/absent untouched; vitest 10/10 + UI build green; no Rust changed so the cargo gate stands as F4 left it (52ce3135).
- 2026-07-17 [ralph iter 2]: editable Board column shipped (gh_issue_project_status → +itemId+options, one round trip; detail Board row = move popover via updateProjectV2ItemFieldValue; embedded Tasks tab defaults provider from trackerProvider pin). pick→auto-bind DEFERRED: putProjectBinding demands a validated five-phase statusMapping — auto-guessing would mis-route status writes; the ProjectBindingEditor stays the binding door. Gates: desktop 88/0 + fmt (worktree needed sherpa+onnx dylib cp — known), UI build.
- 2026-07-17 [iter, Mateo hover+merge feedback]: hover card shows Board status + labels only (dropped redundant "State: Open"); status load = stale-while-revalidate (paint cached column instantly, refetch in bg) + prefetch at card render (open:true) — no more visible lag; Tracker tab FOLDED into Tasks as a collapsible strip (one surface; legacy tracker deep-link opens it expanded + seeds task preselection); TrackerIntakePanel now SHOWS "Files into <provider> · Linear connected/not" + defaults provider from repo.trackerProvider pin; ProjectPicker got a "No board for this project (clear pick)" exit (unremovable wrong pick was the "still showing agentum" trap). UI-only; vitest 36/36 + build.
- 2026-07-17 [Mateo label-dup]: bound repo → status/* LABEL is REDUNDANT with the Projects Status field. Fix in github_transition_with_board: bound = board write is primary + STRIP labels (gh_clear_status_labels_argv, no add); board FAILURE falls back to label (never lose the signal). Blocked path untouched (blocked may not be a board column; its strip-set clears on recovery). Also fixed a pre-existing order-fragile ensure-create assert (sorted). Server 732/0 + fmt.
- 2026-07-17 [Mateo PR column]: PR-open already fired TrackerPhase::InReview (tracker_sync.rs) but it FOLDED onto the in_progress board option (spec 012 F3) — no separate slot. Added a real in_review slot END-TO-END: StatusMapping.in_review (#[serde(default)], empty→in_progress fallback = byte-identical for old bindings); IN_REVIEW_SYNONYMS (moved review/inreview/readyforreview OUT of ready_to_test + added pr/pullrequest/codereview — QA/test stays ready_to_test); ResolvedMapping.in_review + resolver (matches Review/PR col, else FellBack to in_progress); DTOs + validate (in_review OPTIONAL, 5 still required); provision _from_resolved carry it. UI: BoardPhaseKey+inReview, EDITABLE_BOARD_PHASES (renders 6) vs BOARD_PHASES (5 required for Save), OPTIONAL_BOARD_PHASES=[inReview], editor row "Not tracked (uses In Progress)". Existing bindings unchanged until RE-DISCOVER. Gates: server 736/0, binding vitest 12/12, UI build.
- 2026-07-20 [run 395 manual recovery]: status-strip task completed after the
  architecture role timed out. Every harness-owned feature/QA/role terminal now
  shows live phase plus executing n/N progress through the existing harness
  status/event channel; ordinary sessions are excluded. Focused Vitest 4/4 and
  Vite production build green; standalone tsc remains baseline-red on unrelated
  missing shared modules.
- 2026-07-20 [run 394]: resumed the existing Chat-agent scaffolding and completed
  the Settings picker, global persistence, installed-agent default resolution,
  Codex Responses backend, and typed unavailable-agent errors.
- 2026-07-20 [run 394 integration]: reconciled the feature with current develop's
  remote repo-context SSE events, control-marker intake advancement, and
  repo/wiki-grounded draft response without dropping either behavior.
- phase: entered authoring (from executing)
- authoring gate PASS (attempt 1): Spec 407 is a single grounded sidebar-color slice with a named operator, explicit scope, and five observable acceptance criteria.
- phase: entered architecture (from authoring)
- architecture gate PASS (attempt 1): Grounded minimal plan reuses the existing normalized repo color and badge primitives, preserves theme-owned contrast, maps every criterion to named tests, and limits production changes to the actual repo-header renderer.
- phase: entered decompose (from architecture)
- phase: entered executing (from decompose)
- phase: entered review (from executing)
- reviewer gate PASS (attempt 1, 2026-07-21): reviewed the Spec 407 diff against
  `.agentum-harness/specs/407-fix-colors-in-sidebar-on-projects-now-th/architecture.md`.
  ACCEPTED: `WorktreeList.tsx` keeps normalized `repoHeaderColor` on the existing
  `RepoIconGlyph`, adds the architecture-specified unconditional repo-only
  `RepoBadgeMark` (`size-2 shrink-0`), and leaves selected/open backgrounds and
  rings theme-neutral while explicitly inheriting `text-sidebar-foreground`.
  `project-header-color.test.ts` covers configured, missing, null, empty, and
  invalid values through the existing neutral fallback; the source-contract
  tests pin the mark, state separation, theme foreground, and truncating layout.
  No data/store/theme/navigation boundary changed, matching architecture.md with
  no deviation. Final evidence: `.agentum-harness/handoff.md` records the exact
  UI build criterion as successful, and `crates/agentum-desktop/ui/dist` was
  produced after the earlier zero-duration Vitest cache entry.
- review gate PASS (attempt 1): Project headers now retain a normalized configured-color swatch across neutral theme-owned states, with fallback coverage and a successful desktop UI build recorded.
- phase: entered done (from review)
- phase: entered authoring (from done)
- authoring gate PASS (attempt 1): Spec 431 is one tracker-authoritative work-selection slice with five observable criteria, explicit non-goals, grounded reuse, preserved invariants, and no duplicate existing spec.
- phase: entered architecture (from authoring)
- architecture scope corrected by user decision: Spec 431 targets the existing GitHub and Linear project tracker and transition seams only; adding another provider is deferred.
- architecture gate PASS: The internal-board removal plan is grounded in existing project-scoped providers, named transition seams, route isolation, and legacy-row compatibility tests.
- phase: entered authoring (from blocked)
- authoring gate PASS (attempt 1): Single tracker-authoritative work-selection slice with a named persona, observable criteria, explicit non-goals, grounded reuse, preserved invariants, and no competing spec.
- phase: entered architecture (from authoring)
- architecture gate PASS (attempt 1): Grounded plan removes internal-board UI, routes, sink/store access, and reconcilers while preserving external tracker flows and inert legacy data, with every criterion mapped to named tests.
- phase: entered decompose (from architecture)
- phase: entered executing (from decompose)
- phase: entered review (from executing)
- reviewer gate FAIL (attempt 1, 2026-07-22): reviewed the Spec 431 working-tree
  diff against `ai/specs/027-remove-internal-workspace-board/architecture.md`.
  ACCEPTED: `TaskPage.tsx` removes the board sync callback/actions and deletes
  `runtime/board-client.ts`; `ProjectTasksPage.tsx` keeps GitHub/Linear views
  with explicit tracker empty-state copy and structural coverage; server route
  registration and board route/store/core/watchdog modules are removed; the
  real-router 404 matrix, external-only sink enum, legacy-provider best-effort
  test, and legacy-row reopen/inertness test are present; migrations are
  unchanged. REJECTED: the deletion is incomplete and silently deviates from
  Architecture Components 2 and 3. `runtime/harness-client.ts:211-228` retains
  dead `PlanGoalHarnessResult`/`planGoalHarness` code that still POSTs to the
  retired `/api/board/goals/{id}/harness-plan` route and advertises the internal
  board, while `crates/agentum-server/tests/board_server_sync_016a.rs`,
  `goal_cards_end_to_end.rs`, and `card_session_binding_e2e.rs` remain and assert
  successful board APIs/reference removed store and watchdog symbols. This is
  dead, broken board code, and the architecture explicitly requires all three
  suites be deleted. The green handoff does not cover the gap:
  `.agentum-harness/verify.sh` runs only `cargo test -p agentum-server --lib`
  plus fmt (not either full command required by AC5), and `handoff.md` records
  693 server-lib tests but no desktop build output. Required fix: remove the
  stale harness client helper/comments and the three obsolete integration test
  files, then make the verification gate execute and record both exact AC5
  commands.
- reviewer gate FAIL (attempt 2, 2026-07-22): re-reviewed the Spec 431 diff
  against `ai/specs/027-remove-internal-workspace-board/architecture.md` without
  re-running the already-completed verify gate. ACCEPTED: the attempt-1 dead
  harness-plan client and three board integration suites are now deleted;
  `verify.sh` runs both exact AC5 commands; the tracker-only Tasks UI, real-router
  404 matrix, external-only sink signatures, inert legacy-row test, removed
  board persistence/workers, and unchanged migrations match the architecture.
  REJECTED: live docs still advertise working `/api/board` CRUD and an active
  `board_items` model (`docs/API.md:24-33`, `docs/DATA-MODEL.md:34-43`), while
  `runtime/harness-client.ts:50-52` still advertises `board` as a supported
  tracker and an internal-board null URL. The latter directly violates
  Architecture Component 3 (`architecture.md:113-118`), and the former leaves
  undocumented product/maintenance debt after those routes were removed.
  Required fix: update current docs to mark legacy board tables compatibility-
  only/remove the retired API, and remove the stale supported-board wire comments.
- reviewer gate FAIL (attempt 3, 2026-07-22): reviewed the current Spec 431 diff
  against `ai/specs/027-remove-internal-workspace-board/architecture.md` without
  rerunning the completed verify gate. ACCEPTED: the tracker-only Tasks UI and
  settings empty state remain covered by `ProjectTasksPage.test.tsx`; the real
  router's `/api/board*` 404 matrix, external-only `TaskSink`, store-free
  transition signatures, legacy-provider non-writing behavior, inert legacy-row
  reopen test, removed board routes/store/watchdog modules, unchanged migrations,
  and exact AC5 commands in `verify.sh` match the spec and architecture. REJECTED:
  attempt 2's documentation/maintenance gap is unchanged: `docs/API.md:24-33`
  still advertises live `/api/board` CRUD, `docs/DATA-MODEL.md:34-43` still
  presents `board_items` as an active model, and
  `runtime/harness-client.ts:49-52` plus `task_sink.rs:31-34` still describe
  internal-board providers/keys as supported runtime contracts. This conflicts
  with Architecture Component 3's external-only contract and D3's requirement
  that board storage be compatibility-only, leaving silent technical debt.
  Required fix: remove the retired API documentation, label the retained schema
  as inert migration compatibility, and update the stale wire/model comments to
  describe GitHub/Linear only (while retaining legacy input tolerance where the
  architecture explicitly requires it).
- reviewer gate FAIL (attempt 4, 2026-07-22): reviewed the current Spec 431 diff
  and existing verify handoff against
  `ai/specs/027-remove-internal-workspace-board/architecture.md` without rerunning
  tests. ACCEPTED: tracker-only Tasks UI/empty-state coverage, unregistered
  `/api/board*` routes, external-only/store-free transition seams, inert legacy
  row coverage, removed board persistence/reconcilers, unchanged migrations,
  and both exact AC5 commands recorded green. REJECTED: the prior maintenance
  gap is still present: `docs/API.md:24-33` advertises live board CRUD,
  `docs/DATA-MODEL.md:34-43` presents `board_items` as active rather than inert
  compatibility data, `runtime/harness-client.ts:50-52` and
  `task_sink.rs:31-34` advertise board-backed tracker values, and the live SDD
  playbooks still instruct agents to accept board cards
  (`sdd-orchestrate.md:30-34`, `sdd-spec.md:15-19`). This contradicts the
  architecture's external-only runtime contract and compatibility-only storage
  boundary. Required fix: retire the board from current docs/comments/playbooks
  while preserving only explicitly documented legacy input/schema tolerance.
- review gate BLOCKED after 4 attempts: Retire the internal board from current API/data-model docs and live harness/SDD contracts, leaving it documented only as legacy schema/input compatibility.
- phase: entered blocked (from review)
- phase: entered authoring (from blocked)
- authoring gate PASS (attempt 1): One tracker-authoritative work-selection slice now has a named persona, six observable criteria including current docs and SDD contracts, explicit non-goals, grounded reuse, preserved invariants, and no competing spec.
- phase: entered architecture (from authoring)
- architecture gate PASS (attempt 1): Grounded external-tracker-only plan names current UI, router, tracker, persistence, and documentation seams; preserves legacy data safely; and maps every acceptance criterion to focused tests plus required builds.
- phase: entered decompose (from architecture)
- phase: entered executing (from decompose)
- phase: entered review (from executing)
- reviewer gate FAIL (attempt 1, 2026-07-22): reviewed the Spec 431 working-tree
  diff against `architecture.md` and the completed verification handoff without
  rerunning tests. ACCEPTED: `TaskPage.tsx` removes the internal sync path and
  `ProjectTasksPage.tsx` retains GitHub/Linear plus an explicit settings-linked
  empty state; the real router has a `/api/board*` 404 matrix; `TaskSink` and
  tracker transitions are external-only and store-free while legacy `board`
  metadata skips without writing; board CRUD/routes/reconcilers are deleted,
  migrations remain unchanged, and the legacy-row reopen snapshot test is
  present. Current API/data-model docs and embedded SDD playbooks also contain
  the intended external-only wording, and `handoff.md` records the required
  workspace library test and desktop build gate as passed. REJECTED: the
  architecture-required regression test
  `sdd::tests::current_work_item_docs_are_external_only_and_legacy_schema_is_labeled`
  is absent from `crates/agentum-server/src/sdd.rs` (its test module ends with
  only the existing registry/prompt tests at lines 185-262), despite
  `architecture.md:122-127,239,262-264` naming it as the mitigation for current
  documentation drifting back to the retired board contract. Required fix: add
  that focused structural test over the embedded live playbooks and both current
  docs, as specified by the architecture.
- reviewer gate PASS (attempt 2, 2026-07-22): re-reviewed the current Spec 431
  diff against `spec.md` and `architecture.md` without rerunning tests. The sole
  attempt-1 gap is closed by
  `sdd::tests::current_docs_and_sdd_playbooks_keep_internal_board_compatibility_only`:
  it reads the current API/data-model docs, resolves the architecture-named
  `sdd-spec` and `sdd-orchestrate` playbooks, rejects retired `/api/board` and
  board-card advertising, and requires the retained schema's explicit legacy
  compatibility label; retry evidence records 1 passed / 0 failed. The already
  accepted tracker-only Tasks UI, real-router 404 matrix, external-only/store-free
  tracker seams, bounded legacy-provider handling, inert legacy-row coverage,
  removed CRUD/reconcilers, unchanged migrations, workspace library suite, and
  desktop build remain aligned with the architecture. No undocumented debt,
  unjustified complexity, or silent architectural deviation remains.
- review gate PASS (attempt 2): Spec 431 is complete: tracker-only UI/runtime removal, inert legacy compatibility, current docs/playbooks, focused regression coverage, workspace library tests, and desktop build all satisfy the spec and architecture.
- phase: entered done (from review)
- phase: entered authoring (from done)
- authoring gate PASS (attempt 1): Spec 437 is one testable gated-run topbar persistence slice grounded in the existing desktop UI and harness event flow.
- phase: entered architecture (from authoring)
- architecture gate PASS (attempt 1): Grounded minimal plan preserves the existing single root topbar and fixes workdir-scoped snapshot continuity with ordered event refreshes and focused regressions.
- phase: entered executing (from architecture)
- developer implementation complete (2026-07-23): `useWorktreeHarnessRun` now retains confirmed snapshots by normalized workdir, rejects stale and foreign responses, preserves the selected snapshot during event refreshes, and evicts only on an authoritative list miss. Focused continuity and exactly-one-region regressions were added.
- phase: entered review (from executing)
- review gate PASS (attempt 1): 12 focused Vitest checks, the harness QA entrypoint, and the desktop UI production build all pass; the existing single `GatedRunBar` production mount and component markup remain unchanged.
- phase: entered done (from review)
