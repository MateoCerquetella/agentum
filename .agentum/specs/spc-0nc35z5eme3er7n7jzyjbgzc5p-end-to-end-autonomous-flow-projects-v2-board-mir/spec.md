---
schema: 1
id: SPC-0NC35Z5EME3ER7N7JZYJBGZC5P
revision: 1
title: End-to-End Autonomous Flow (Projects v2 board mirror + workspace provisioning)
source: legacy-import:ai/specs/010-end-to-end-autonomous-flow/spec.md@sha256:e96fed3f511df9ece1815ca8937e7f3da813ef35cc5ebdac964f950166737df1
---

# End-to-End Autonomous Flow (Projects v2 board mirror + workspace provisioning)

## Migration provenance

This historical specification was assigned a stable Agentum identity during the
v2 cutover. Its source is included below and its exact original bytes are also
preserved in the external recovery archive and accounted for by SHA-256.

## Requirements

- RQ-001 Preserve the historical specification's stable identity and source provenance.
- RQ-002 Treat this imported revision as historical context until a user explicitly reopens it.

## Acceptance criteria

- AC-001 The source path and SHA-256 match the migration inventory and recovery archive.
- AC-002 New work on this specification creates an immutable later revision through Agentum.

## Imported historical source

> # Spec 010 — End-to-End Autonomous Flow (Projects v2 board mirror + workspace provisioning)
>
> - **Number:** 010  <!-- renumbered from 009 (2026-07-06): ai/specs/009-wiki-project-scoped ships on branch wiki-remove-it-fomr-the-side -->
> - **Status:** Done  <!-- Reviewer SIGN-OFF 2026-07-06 (review.md, 0 blockers); release + AC-11 demo HUMAN-GATED -->
> - **Surface:** `crates/agentum-server` (task_sink, routes/github, harness scaffold) + `crates/agentum-desktop` (workspace wizard UI; existing `gh_projects` read commands)
> - **Author:** Mateo (PRD "Agentum: End-to-End Autonomous Flow" → `/sdd-spec` direct draft)
> - **Date:** 2026-07-06
>
> > **Source PRD.** This spec distills Mateo's PRD *"End-to-End Autonomous Flow
> > (Chat → Issue → Work → QA)"* (2026-07-06). The PRD's §0 instruction — "this
> > PRD does not reinvent the core" — is taken literally: code research shows the
> > canonical flow (§1) already shipped across specs 004/005/006/008/012. The
> > traceability table below accounts for every PRD section; this spec builds
> > only the delta. **Line refs verified at v0.58.3 (`388eaa66`); re-locate
> > before editing** (the 004 lesson).
>
> ## Problem
>
> agentum already drives the loop — chat files the issue, Start Work spawns the
> gated run, labels flip — but the movement is invisible where humans actually
> look. The GitHub **Projects v2 board never moves** (agentum only writes
> `status/*` labels; nobody's Kanban is label-powered), a Done feature leaves its
> **issue open**, and creating a new workspace starts **naked**: Mateo hand-creates
> the repo, labels, board, and spec scaffold on GitHub before agentum can drive
> anything. So he drags cards by hand after every run — the exact toil agentum
> exists to remove — and the flagship demo (a real board moving on its own)
> doesn't exist.
>
> ## Goal
>
> Make the already-shipped gated loop land on the user's **real GitHub Projects
> v2 board** end-to-end: bound (or born, repo-from-template) at workspace
> creation, moved by **option ID** on every transition — custom column names
> included — with the issue closed at Done.
>
> ## Users / personas
>
> - **Mateo, solo dogfooder** — feels it in two moments:
>   1. After Chat files issues and a run drives them green, his real GitHub
>      project board (the surface he and any stakeholder actually watch) still
>      shows every card in Backlog; he drags them by hand, and Done issues stay
>      open until he remembers to close them.
>   2. Creating a workspace for a new idea: agentum's wizard captures the goal
>      (008 F3), but repo, labels, board, and scaffold are still manual GitHub
>      chores before the first run can start — "born ready" is a promise the
>      product doesn't keep.
> - Secondary (why-it-matters, not a design target): demo audiences — a real
>   board moving live, custom columns and all, IS the demo.
>
> ## Acceptance criteria
>
> Ordered slices F1 → F3; each criterion independently gateable.
>
> **F1 — Bind: discover a board, resolve the mapping**
>
> 1. A server-side bind action, given a repo slug + a Projects v2 reference,
>    **persists** a per-repo binding `{project_id, status_field_id,
>    status_mapping}` where `status_mapping` maps every canonical phase —
>    `Todo | InProgress | ReadyToTest | Done | Blocked` — to a **single-select
>    option ID** discovered via one `gh api graphql` query of the project's
>    Status field. Resolution is fuzzy name-match first, then fallback to the
>    nearest earlier mapped phase (`ReadyToTest → InProgress`,
>    `Blocked → InProgress`); a binding with an unmapped phase is
>    unrepresentable (the constructor refuses to produce one).
> 2. Unit tests with a fake `gh` binary (the established `task_sink` pattern)
>    **assert** the resolved mapping for: (a) a default board
>    (Todo / In Progress / Done), (b) a custom board
>    (Backlog / Building / QA / Shipped) resolving ReadyToTest→"QA" and
>    Done→"Shipped", (c) a board with no ReadyToTest-like column falling back
>    to the InProgress option, and (d) a `gh` failure or missing Status field
>    returning an actionable error (including the missing-`project`-scope case),
>    never a partial binding.
> 3. The workspace surface (one shared mapping component: an edit surface
>    reachable for an existing workspace AND, with F3, a wizard step — D7)
>    **renders** the
>    resolved mapping as per-phase selects populated with the discovered option
>    names, persists edits, and offers re-discovery; project picking reuses the
>    existing desktop read commands (`gh_list_accessible_projects`,
>    `gh_resolve_project_ref`).
>
> **F2 — Drive: every transition writes the board**
>
> 4. When a binding exists for the issue's repo (slug parsed from
>    `tracker_url`, as the label arm already does), every
>    `apply_tracker_transition` call additionally **issues** the Projects
>    write: ensure the issue is on the board (`addProjectV2ItemById`,
>    idempotent), then `updateProjectV2ItemFieldValue` with the mapped option
>    ID. All existing call sites — four direct seam callers spanning six
>    transition points (drive.rs InProgress/ReadyToTest/Done via its one
>    `transition_tracker` wrapper, board_goals + harness Todo, MCP
>    `agentum_report_status`) — get this with **zero call-site edits** — the
>    arm lives inside the seam.
> 5. `apply_blocked_transition` with a binding **moves** the card to the
>    Blocked-mapped option, in addition to today's `status/blocked` label +
>    issue comment.
> 6. A Done transition on a bound workspace **closes** the issue
>    (`gh issue close`) when the binding's `done_closes_issue` knob is on
>    (wizard default: on); a later InProgress transition on a closed issue
>    **reopens** it. Unbound (label-only) flows keep today's contract — Done
>    stays label-only, closing remains the PR's `Closes #N` job.
>    *[Supersedes 004-D1 for bound workspaces — PM-confirmed: D1.]*
> 7. A failing `gh` Projects call **logs** (tracing + `HarnessEvent::Log` where
>    a run context exists) and the transition still returns `Ok` — asserted by
>    a fake-`gh`-exits-nonzero test. A red board write never blocks, fails, or
>    retries the gate (the best-effort tracker contract, extended).
> 8. Labels keep flipping exactly as today on bound workspaces (the Projects
>    arm is additive; the exactly-one-`status/*` invariant is untouched) —
>    existing label tests stay green unmodified.
>
> **F3 — Provision: a workspace is born ready**
>
> 9. The workspace creation flow **offers** "New repo from template"
>    (`gh repo create <owner>/<name> --template <template-repo>`; template
>    configurable, wizard-choice of owner) and "Adopt existing repo"; both
>    converge on one idempotent provisioning ensure that: ensures the five
>    canonical `status/*` labels (existing `gh_label_ensure_argv`), links an
>    existing Project v2 **or creates one** and binds it (F1), and scaffolds
>    `.agentum-harness/` — committed and pushed to the default branch when the
>    (default-on, explicitly visible) commit step is accepted.
> 10. Re-running provisioning against an already-provisioned repo **changes
>     nothing**: no duplicate labels, no second project, no new scaffold
>     commit — asserted by a run-twice test (fake `gh` + temp git repo)
>     verifying identical state and an unchanged commit count.
> 11. A workspace created through F3 **runs the existing loop with the board
>     moving**: a chat-filed issue lands in the Todo column; Start Work →
>     InProgress; unit-gate green → ReadyToTest; QA green → Done + issue
>     closed — observed on a real board with custom column names (qa.sh /
>     human-run demo, same class as 008 AC 12 — runner: Mateo; evidence: the
>     issue's timeline project-status + close events plus a demo-pass line
>     appended to `ai/STATE.md`).
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** the three slices above. GitHub only (Issues + Projects v2). One
>   binding per repo. Phase-1 auth = the user's existing `gh` CLI login (every
>   current GitHub write already rides it).
> - **Out:**
>   - **Inbound webhooks / echo suppression (PRD §9.3).** No inbound sync
>     exists (`routes/board_sync.rs:14`: "Self-hosted ⇒ no inbound webhooks");
>     agentum stays one-way authoritative — a human drag is simply overwritten
>     by the next agent-driven write. Phase 2, with the PRD's
>     `(node_id, expected_state, ts)` design when webhooks arrive.
>   - **`.agentum/result.json` polling contract (PRD §7).** agentum's settle
>     detection (`wait_for_settle`, drive.rs:1133) + the two-phase gate IS the
>     completion contract — external verification, strictly stronger than agent
>     self-report. Agents that want to push phases explicitly already have the
>     `agentum_report_status` MCP tool (mcp.rs:1199), which hits the same seam
>     and therefore gains Projects writes for free. No parallel file contract.
>   - **GitHub App auth** (PRD §13 Q4): Phase 2. Org-owned template creation
>     works through `gh` when the logged-in user has permission.
>   - **Auto-adding a missing "Ready to Test" option** to the user's Status
>     field (PRD §13 Q3): no safe single mutation exists; Phase 1 = fallback to
>     InProgress + a wizard hint to add the column manually.
>   - Linear/Jira parity for project boards (Linear state mapping already
>     exists and is untouched); multi-repo workspaces; multi-feature-per-chat
>     changes; TUI parity; new tool adapters; any change to the spawn path.
>
> ### PRD traceability (every § accounted for)
>
> | PRD § | Disposition |
> | --- | --- |
> | §1 canonical flow | **Shipped** (004/005/006/008): chat→issues `create_github_issues` (chat.rs:1584) via `TaskSink::create_feature` (task_sink.rs:124, GitHub arm :156–198); Start Work `start_work` (routes/harness.rs:508); spec materialization `spec_md_from_issue` (types.rs:1064); spawn+prompt `spawn_feature_agent` (drive.rs:414); settle (drive.rs:1133); verify+QA gates (drive.rs:185/222); retry budget `max_retries` (types.rs:113). |
> | §3 status projection | Layer 1 exists: `FeatureState` → `TrackerPhase` at the call sites. Layer 2 exists for labels (`GithubStateMap`) + Linear (`LinearStateMap`). **New: the Projects v2 option-ID layer (F1/F2).** |
> | §4 workspace creation | Partial: goal-first wizard (008 F3) + `scaffold_harness` (keep-existing). **New: template repo, label pre-ensure, board create/bind, scaffold commit (F3).** |
> | §5 chat→issue | Shipped (labels at creation via `NewFeature.labels`; Todo transition at plan). **New: board item add — F2's lazy ensure covers it.** |
> | §6 Start Work | Shipped: composer `createWorktree` → `startGatedWork`; worktree per card; InProgress at first spawn (drive.rs:129–133). |
> | §7 result contract | **Deliberate deviation** — see non-goals. |
> | §8 QA gate | Shipped (spec 012): `run_qa_agent_gate` (drive.rs:562), verdict file (helpers.rs:142), browser MCP; QA-red retry via `handle_gate_failure` (drive.rs:294). |
> | §9.1–9.2 GraphQL | **This spec (F1/F2).** |
> | §9.3 echo suppression | Out — see non-goals. |
> | §10 data model | Workspace ≈ worktree registry + new binding; Task ≈ `Feature` (`tracker_provider/url`, types.rs:85–88); IssueLink ≈ `tracker_url` + F2-resolved node/item ids; traits ≈ the `TaskSink`/`apply_tracker_transition` seam. |
> | §12 acceptance | Mapped into ACs 1–11 (PRD-AC7 echo suppression → out). |
> | §13 open questions | Answered under Open questions below. |
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - **The transition seam**: `apply_tracker_transition(store, provider,
>   tracker_id, tracker_url, phase)` (`task_sink.rs:692`) with real
>   linear/board/github arms, slug+number parsed from `tracker_url`, and
>   best-effort error contract. Its four direct call sites (`drive.rs:388` wrapper →
>   :129/:207/:268; `board_goals.rs:605`; `routes/harness.rs:425`;
>   `mcp.rs:1201`) must not be edited — F2 goes **inside** the seam.
> - **`TrackerPhase`** (`task_sink.rs:219`, 4 variants) + the Blocked side path
>   `apply_blocked_transition` (`task_sink.rs:771`, GitHub-only label+comment).
>   The canonical 5-phase vocabulary = these 4 + Blocked; do not grow the enum.
> - **Config-map precedent**: `GithubStateMap::from_env()` (`task_sink.rs:389`)
>   layering defaults → `<data dir>/Agentum/github.json` `state_map`
>   (`task_sink.rs:366`, `AGENTUM_GITHUB_CONFIG` override) → env; twin
>   `LinearStateMap` (`linear.rs:183/206`). The board binding follows this
>   file's pattern (see Open questions #2).
> - **gh argv-builder + fake-gh test pattern**: `gh_label_ensure_argv`
>   (`task_sink.rs:452`, idempotent `--force`), `gh_set_status_label_argv`
>   (:469), `gh_issue_comment_argv` (:532), creation argv (:820/:839), URL
>   parse (:877). F1/F2 add pure GraphQL argv builders in the same style.
> - **Projects v2 READ surface (desktop)**: `gh_projects.rs` — `graphql()`
>   wrapper (:136), `gh_resolve_project_ref` (:599),
>   `gh_list_accessible_projects` (:661) — real, read-only, registered. The
>   wizard's project picker reuses these; do not re-derive project refs.
> - **Issue routes**: `GET /api/github/issue` (`routes/github.rs:154`),
>   `POST /api/github/issues` (:212, returns `{provider, number, url, slug,
>   author}`), `GET /api/github/labels` (:359).
> - **Start-work orchestration**: `start_work` (`routes/harness.rs:508`) —
>   lock, fetch issue, `ensure_spec_and_plan` (Todo transition at :425), spawn
>   `drive`. Untouched by this spec.
> - **Scaffold**: `scaffold_harness` (`types.rs:678`) writing canonical
>   `.agentum-harness/` (`HARNESS_DIR`, types.rs:16; `.harness/` legacy
>   read-only fallback), keep-existing semantics — already idempotent. F3 adds
>   only the commit/push step around it.
> - **Wizard surface**: goal-first flow (`NewWorkspaceGoalStep.tsx`,
>   `lib/workspace-goal-step.ts`, 008 F3) + composer primitives
>   (`useComposerState.ts` — `createWorktree` :2458/:2664,
>   `maybeStartGatedRun` :2493/:2704, `maybeScaffoldSpecFromIssue` :2248;
>   `runtime/github-issue-client.ts:238`). F3 extends the optional-steps list;
>   008-D3 stands (composer stays the engine).
> - **MCP push path**: `agentum_report_status` → `tool_report_status`
>   (`mcp.rs:1199`) → the seam. Gains F2 automatically; no MCP change.
>
> ### Build new
>
> - **F1**: a `BoardBinding` type + Status-field discovery over `gh api
>   graphql` (server-side, from a neutral cwd like the label arm); a pure
>   fuzzy-mapper `phase → option` with the never-unmapped fallback; per-repo
>   persistence + bind/read/update routes; the wizard/settings mapping UI.
> - **F2**: the projects arm inside `apply_tracker_transition` /
>   `apply_blocked_transition`: pure mutation builders for
>   `addProjectV2ItemById` + `updateProjectV2ItemFieldValue` (+ issue node-id /
>   item-id resolution, cache optional), `gh issue close`/`reopen` argv +
>   the `done_closes_issue` knob; fake-gh tests per phase.
> - **F3**: `gh repo create --template` mode (**absent today** — verified, no
>   `repo create`/`--template` in server or desktop Rust); provision-time label
>   ensure loop (reusing :452); project create+bind; scaffold commit+push;
>   the run-twice idempotency test; wizard steps wiring it together.
>
> ## Risks & invariants
>
> - **The best-effort tracker contract is sacred**: a board/GraphQL hiccup is
>   logged and skipped, never a halted run or a red gate (AC 7). This is the
>   same contract the label arm honors (`task_sink.rs:613–620`).
> - **Zero spawn-path changes**: this spec touches tracker plumbing and the
>   wizard only. `spawn_agent_into_pane`, YOLO translation, `await_repl_ready`,
>   `inject_prompt` are out of bounds entirely.
> - **`gh` token scope**: Projects v2 needs the `project` scope, which default
>   `gh` logins often lack. Bind time must probe and fail **actionably**
>   (surface `gh auth refresh -s project`); mid-run writes log-and-continue
>   (AC 2d / AC 7). Never a silent skip (the 008 "never silent" doctrine).
> - **Option IDs, never names, at write time** (PRD AC 6). Names are only
>   fuzzy-match input at bind time. Column renames after bind ⇒ writes still
>   land (IDs are stable); re-discovery refreshes names in the UI.
> - **Desktop write stubs stay dead**: `gh_update_project_item_field` /
>   `gh_clear_project_item_field` (`gh.rs:1046/:1051`) are `not_available()`
>   stubs — do NOT implement board writes there. Writes live server-side so the
>   harness and MCP work headless and identically in the installed app (the
>   spec-007 stub lesson). Follow-up may delete the stubs.
> - **Label canon untouched**: exactly-one-`status/*` (five names incl.
>   blocked) stays as-is; the projects arm is additive (AC 8). Never touch
>   human-QA `status/qa*` labels (004-C4).
> - **Idempotency is a hard requirement** (PRD §4): labels via `--force`
>   ensure, item-add idempotent by API contract, scaffold keep-existing,
>   binding upsert, template-create skipped when the repo exists (AC 10).
> - **Committing to a user's default branch (F3) is outward-facing**: an
>   explicit, visible, default-on wizard step; plain push, never force; adopt
>   mode must show exactly what will be committed.
> - **Two GraphQL calls per transition** is the volume ceiling (≤ ~10 per
>   feature run) — no batching machinery; an id cache is an optimization, and
>   correctness must not depend on it.
> - **Line-ref drift**: refs are v0.58.3; the architect re-locates before
>   editing (004 lesson).
>
> ## Harness wiring (the gate)
>
> - **feature_list.json entries:** `010-f1-board-bind` (AC 1–3),
>   `010-f2-board-drive` (AC 4–8), `010-f3-workspace-provision` (AC 9–10);
>   AC 11 is the release demo gate (human / qa.sh), not a feature entry.
> - **`verify.sh` asserts:** `cargo test --workspace --lib` green including the
>   new fake-gh suites (discovery/mapping AC 2, per-phase mutation argv,
>   close/reopen knob, non-fatal-failure AC 7, provision run-twice AC 10) with
>   existing label tests unmodified; `cargo fmt --check` + clippy; vite build +
>   vitest for the wizard/mapping pure modules.
> - **`qa.sh` asserts (browser QA):** wizard renders template/adopt modes and
>   the mapping step shows discovered custom column names with per-phase
>   selects; bind failure (no `project` scope) surfaces the actionable error;
>   AC 11 live board demo on a custom-column project (requires network + the
>   browser-QA knob armed — `AGENTUM_BROWSER_VERIFY`, default OFF per 005-F3 —
>   else these pass vacuously and AC 11 falls to the human demo).
>
> ## Decisions (PM-locked, 2026-07-06)
>
> - **D1 — Done closes the issue on BOUND workspaces; 004-D1 stands everywhere
>   else.** A Done transition on a repo with a board binding closes the issue
>   (`gh issue close`) when the binding's `done_closes_issue` knob is on —
>   wizard default ON, per the PRD's explicit "QA green closes the GitHub
>   issue". A later InProgress transition on a closed bound issue reopens it.
>   This deliberately supersedes 004-D1 ("closing remains the PR's `Closes #N`
>   job", task_sink.rs:731–732) for bound workspaces ONLY: a Done column
>   holding an open issue makes the board lie, and the autonomous flow's
>   terminal evidence is QA-green, not a merged PR. Unbound (label-only) flows
>   keep today's contract byte-for-byte. Forbids: closing issues on unbound
>   repos; making a close/reopen failure fatal (best-effort holds); touching
>   the PR-`Closes #N` convention. *(Narrows the PRD's unconditional "QA green
>   closes the issue" to bound workspaces — flagged deliberately: the PRD's
>   context is board-bound flows, and an unconditional close would regress
>   004-D1 for PR-driven repos like agentum itself.)*
> - **D2 — The binding lives DAEMON-SIDE (not in-repo); the persistence
>   mechanism is the architect's call under one hard constraint.** The
>   transition seam is workdir-less (it has only `tracker_url`), so Phase-1
>   lookup must resolve from daemon-global state keyed by `owner/repo`; an
>   in-repo `.agentum-harness/board.json` would force a workdir through the
>   seam (rejected for Phase 1; an in-repo mirror is a named follow-up). PM
>   code finding that demotes the draft's "(a) github.json" from lock to
>   constraint: the desktop's `github_labels.rs::update_config` round-trips a
>   typed struct and silently DROPS unknown keys — a naïve `projects` key in
>   `github.json` would be erased by the next Settings label-name save. The
>   architect picks among (a1) `github.json` `projects` map + a passthrough
>   field in the desktop writer + a regression test that a Settings save
>   preserves bindings; (a2) a sibling single-writer file
>   (`github_projects.json`, the `linear.json` pattern); (a3) a store table
>   following `agentum_core::TrackerBinding` (lib.rs:610) — note the seam
>   already receives `&Store`, so (a3) needs zero signature changes. Forbids:
>   any mechanism where saving label names in Settings can destroy a binding;
>   in-repo-only persistence.
> - **D3 — Human drags are overwritten in Phase 1; agentum is one-way
>   authoritative.** agentum writes only at transitions, so a human drag
>   persists until the feature's next transition, then loses — no snap-back,
>   no cancel-on-drag, no conflict detection (board_sync.rs:14: self-hosted ⇒
>   no inbound webhooks; agentum cannot even see the drag). Revisit with the
>   PRD §9.3 `(node_id, expected_state, ts)` design when inbound sync exists.
>   Forbids: polling the board to detect drags; any Phase-1 echo-suppression
>   machinery.
> - **D4 — Default template = `goempirical/empirical-sdd-ddd-starter`,
>   configurable.** Ship the upstream starter as the default until a fork
>   exists; the wizard's template field is editable (any `owner/repo` the
>   user's `gh` can access) and the created repo's owner is an explicit wizard
>   choice. Forbids: hardcoding the template beyond a default constant;
>   blocking adopt mode on template availability.
> - **D5 — Board CREATE ships in the wizard; link-existing is not the only
>   path.** Create is one `gh project create` + the same F1 bind; the created
>   board carries GitHub's default Status options (Todo / In Progress / Done),
>   which the fuzzy mapper resolves with the ReadyToTest→InProgress and
>   Blocked→InProgress fallbacks — the wizard renders the resolved mapping
>   with those fallbacks VISIBLE, plus the hint to add a "Ready to Test"-like
>   column manually (auto-adding an option stays out of scope — no safe single
>   mutation). Forbids: silent fallbacks; any Status-field option mutation.
> - **D6 — One-slice ruling (formalizing the draft-time judgment call).** 010
>   is one user value — the shipped gated loop lands on the user's real
>   Projects v2 board — delivered as three ordered, independently-gateable
>   increments (F1 bind → F2 drive → F3 provision), the 008 shape. F1+F2 alone
>   deliver the headline value on a hand-bound existing repo; F3 ("born
>   ready") may ship separately if it slips. Forbids: coupling F2's gate to F3
>   surfaces; landing F3 first.
> - **D7 — F1's UI home is wizard-INDEPENDENT (sequencing consequence).** F1
>   lands before F3's wizard exists, and F2 must be dogfoodable on an existing
>   workspace (agentum's own repo), so the bind/mapping surface ships as one
>   shared component reachable for an EXISTING workspace (settings/edit
>   placement = architect's call); F3's wizard step later mounts the same
>   component. Resolves AC 3's "wizard step and/or settings pane" to: both,
>   one component. When auto-resolution refuses (unmappable phase), the
>   surface lets the user complete the mapping manually via the per-phase
>   selects — a refusal is a prompt to finish binding, never a dead end.
>   Forbids: a bind UI that exists only inside workspace creation.
> - **D8 — Committing to a user's default branch requires visible consent;
>   pushes are plain.** F3's scaffold commit/push is an explicit, default-ON,
>   declinable wizard step that names the target branch and lists exactly what
>   will be committed (adopt mode especially); push is never `--force`; a red
>   push is surfaced but non-fatal (the workspace stays usable, matching the
>   scaffold's existing non-fatal contract). The provisioning commit message
>   carries no AI-attribution trailer (standing repo-wide git rule). Forbids:
>   pushing without the step shown; force-push; failing workspace creation on
>   a red push.
