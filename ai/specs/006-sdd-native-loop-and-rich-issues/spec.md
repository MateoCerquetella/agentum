# Spec 006 — SDD-native loop + rich issues (no install required)

- **Number:** 006
- **Status:** Done             <!-- Draft | PM | Architect | In progress | Done -->  (Reviewer sign-off 2026-07-02; release human-gated)
- **Surface:** `crates/agentum-server` (routes/github, routes/chat, harness roles/drive) + `crates/agentum-desktop/ui` (composer create-issue form, TaskPage issue detail)
- **Author:** Mateo Cerquetella (drafted with Claude)
- **Date:** 2026-07-02

## Problem

The loop released in v0.51.0 runs, but its inputs are thin and its process is
not the SDD process the user runs by hand. An issue created from the composer
or Tasks page lands **bare** — no description unless the user types one, and
**never any labels** (`routes/github.rs:237` hardcodes `labels: Vec::new()`);
the in-app detail view then shows "unknown opened this issue · No description
provided" (real example: #232). Chat's issues are richer (title + checklist +
labels) but not SDD-shaped — no Problem/Goal framing — so the worktree spec
generated from them is a checklist dump, not a spec. And the actual SDD role
loop (PM → Architect → Decompose → Execute → Review) exists **inside the
engine** (spec 013's role gates, verdict-file contract) but is default-OFF
with no UI surface — today the only way to get SDD behavior is to install
Claude-Code-side tooling (`/sdd-loop`, the ralph-loop plugin, an `ai/`
scaffold) on the operator's machine.

## Goal

A gated run started from the app follows the SDD role loop out of the box, fed
by issues that carry a real description, labels, and SDD-shaped acceptance
criteria — nothing to install.

## Users / personas

- **Mateo (or any user) starting work from the app:** clicks "Start gated run"
  and expects the same PM-gate → architecture → build → review rigor his
  hand-run `/sdd-loop` gives — without the app requiring Claude-Code plugins,
  slash commands, or a repo scaffold.
- **Anyone reading the tracker:** issue #232-style artifacts ("No description
  provided", zero labels, "unknown" author) make the board useless as a
  status/context surface — the issue is supposed to be the spec source and the
  live status board.

## Acceptance criteria

*Increment F1 — rich issue creation (composer/Tasks path parity with Chat):*

1. `POST /api/github/issues` accepts an optional `labels: Vec<String>` and
   threads it into `NewFeature.labels` (the `gh --label` plumbing already
   exists — `task_sink.rs:24-32`); absent/empty labels keep the wire and gh
   argv byte-identical (pinned by test).
2. The composer's "Create GitHub issue" form gains a label picker seeded from
   the repo's label set (reuse the existing label fetch the Tasks page uses,
   or a static `type/*`+`priority/*` fallback when unfetchable) and sends the
   selection; it renders the applied labels on the created-issue chip.
3. When the body field is left blank, the created issue's body is auto-filled
   from what the composer already has in hand — **context is defined as
   exactly the composer's typed agent-prompt field and note field** — rendered
   under a "## Context" heading (never an empty body when either field is
   non-blank); both blank still creates with no body (no new failure mode;
   pinned both ways).

*Increment F2 — Chat issues are SDD-shaped:*

4. The issue body is COMPOSED, not model-emitted (`compose_issue_body`,
   `routes/chat.rs:973`) — so: the extraction JSON (`EXTRACT_INSTRUCTIONS`,
   `:866`) gains optional `problem` and `goal` string fields (serde-default),
   and `compose_issue_body` renders `## Problem` / `## Goal` /
   `## Acceptance criteria` (`- [ ]` checklist from `tasks`, priorities
   preserved) when they're present; when absent, the rendered body is
   **byte-identical to today** (pinned) — so the one-issue contract, `labels`
   threading, sanitize handling, and every existing chat test stay green.
5. `spec_md_from_issue` over such a body produces a worktree spec whose AC
   checkboxes are exactly the issue's `- [ ]` lines (already true — pinned by
   a round-trip test with an SDD-shaped fixture body).

*Increment F3 — the SDD role loop, inherited (no install):*

6. The start-work seam (`routes/harness.rs::start_work`) sets
   `FeatureList.roles = true` on the backlog it plans (via the existing
   `update_backlog_knobs` write), gated by a persisted setting
   `harness.sdd.roles.enabled` (Settings-writable, same
   `GET/PUT /api/harness/settings` surface as the QA knob; **read once at
   start-work plan time** — `roles` is a backlog knob stamped into
   `feature_list.json`, not a per-drive-tick read) — so a gated run executes
   spec 013's phases: PM verdict-gate on the spec → Architect verdict-gate →
   Decompose → per-feature Execute → Review verdict-gate, with the existing
   verdict-file fail-closed contract untouched.
7. The role briefs (`harness_roles/{pm,architect,reviewer}.md`) are aligned
   with the gate checklists in `ai/skills/validate_handoff.md` and
   `ai/skills/write_spec.md` (one-slice, testable ACs, grounded-in-code,
   invariants) — the architect diffs the current briefs against those
   checklists and specifies the exact deltas. Brief content is embedded via
   `include_str!`, so prompt BYTES change by design; the pin is the
   **verdict-file contract**: the "HOW TO RECORD YOUR VERDICT" lines of
   `build_role_prompt` and the `RoleVerdict` wire shape stay
   character-identical (pinned by test).
8. The composer's "Start gated run" armed copy names the role loop when the
   setting is on; the Harness page's existing roles strip
   (`HarnessEngine.tsx:292`) renders the phase for such runs (it already
   does — verified, not rebuilt).

*Increment F4 — issue detail credibility:*

9. The in-app issue detail no longer renders "unknown" for the author of an
   issue the current user just created. Root cause (PM-verified): the render
   is `workItem.author ?? 'unknown'` (`GitHubItemDialog.tsx:892/:938`), and
   the composer's just-created snapshot is built with only
   `{type, number, title, url}` (`useComposerState.ts:1455-1470`) — no
   `author`. Fix: the create response carries the authenticated login (server
   side — `gh` knows the user) and the snapshot populates it; the `??
   'unknown'` fallback stays for genuinely unknown authors. The architect
   confirms whether the Tasks LIST payload also lacks author for fresh rows.

## Scope & non-goals (YAGNI)

- **In:** the four increments; GitHub only for F1 (Linear create already has
  its own path); the roles knob default value decided at PM gate (see open
  questions).
- **Out:**
  - Porting the full `ai/` scaffold (STATE.md, handoff contracts, decision
    logs) into the product — the engine's verdict-file gates ARE the product
    equivalent; file-level parity is not the goal.
  - A Developer/Tester role split inside the engine (spec 013's phase set —
    PM/Architect/Review around the existing execute+verify+QA gates — stands;
    the unit gate and QA gate already play Tester).
  - Replacing `/sdd-loop` for repo-side development of agentum itself (that
    stays a Claude-Code workflow).
  - LLM-generated issue bodies in the composer path (F1's auto-fill is a
    deterministic assembly of context already in hand, not an agent call).
  - Label *creation* in the composer picker (existing repo labels only;
    ensure-create stays a transition-time concern, spec 004 D3).

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- **Label plumbing end-to-end:** `NewFeature.labels` + `gh --label` per label
  (`task_sink.rs:24-32`, spec 003); Chat threads it (`routes/chat.rs:903-906`,
  `plan_to_features` `:1031-1036`). F1 only widens `CreateIssueBody`
  (`routes/github.rs:166-172`) and the composer form.
- **The SDD role machine:** spec 013's `SpecPhase` state machine, `RoleKind`
  briefs, `build_role_prompt` + `RoleVerdict` fail-closed files
  (`harness/helpers.rs:57-109`), `run_pre_feature_phases`/`run_review_phase`
  in drive.rs — keyed on `config.features.roles && spec_id.is_some()`
  (`drive.rs:78-81`ff). Spec 005 already stamps `spec_id` on every planned
  backlog; F3 flips `roles` at plan time behind a knob.
- **The knob pattern:** `BROWSER_QA_ENABLED_SETTING` + `GET/PUT
  /api/harness/settings` + Settings toggle (spec 005 F3) — F3's
  `harness.sdd.roles.enabled` clones it.
- **The knob-write seam:** `update_backlog_knobs` (spec 005 F1) — F3 sets
  `roles` in the same post-plan write that sets agent/model.
- **Roles UI strip:** `HarnessEngine.tsx:292` already renders when
  `status.features.roles` — untouched.
- **Composer create-issue form:** `useComposerState.ts:1445-1470` +
  `createGithubIssue` client (`github-issue-client.ts:76`) — widened, not
  replaced.

### Build new

- `labels` on `CreateIssueBody` + the composer label picker + body auto-fill
  assembly (F1).
- The chat extraction-prompt section spec + SDD-shaped fixture tests (F2).
- `SDD_ROLES_ENABLED_SETTING` + `roles: true` in start-work's knob write +
  settings wire widening + brief refresh (F3).
- The author-hydration fix in the Tasks issue detail (F4 — investigate first;
  the fix lands wherever the data actually goes missing).

## Risks & invariants

- **Verdict-file fail-closed contract (sacred):** F3 changes WHEN the role
  gates run (knob at plan time), never HOW — `parse_role_verdict`,
  fail-closed on missing/garbled, and the phase machine are untouched.
- **Role gates add agent spawns per run** (PM + Architect + Review) — cost
  and wall-clock triple-check is why the knob exists; the default matters
  (open question 1).
- **Chat contract stability:** F2 is a prompt change — the extraction JSON
  shape, `sanitize_messages`, and one-issue semantics must stay pinned by the
  existing routes/chat tests.
- **Wire compat:** absent `labels` in `POST /api/github/issues` must be
  byte-identical to today (serde default; pinned).
- **Best-effort tracker + one-launch-path:** untouched by all four
  increments.
- **`/sdd-loop` divergence:** the in-app role briefs and the repo's
  `ai/skills` checklists can drift apart — F3 copies the gate CONTENT now and
  notes the single-source follow-up; do not block on solving doc unification.

## Harness wiring (the gate)

- **feature_list.json entries:** `F1 rich-issue-create` → `F2 chat-sdd-shape`
  → `F3 roles-inherited` → `F4 author-hydration`.
- **`verify.sh` asserts:** `cargo test -p agentum-server --lib` green with:
  labels-threading + absent-labels-byte-identical pins (F1); SDD-shaped
  fixture → `spec_md_from_issue` round-trip (F2); knob default + `roles`
  stamped only when enabled + brief-refresh prompt pins (F3). Plus
  `cargo clippy --workspace --all-targets -- -D warnings` (v0.51.0 tag
  lesson) and `npm run build --prefix crates/agentum-desktop/ui`.
- **`qa.sh` asserts:** composer create-issue with labels + blank body → the
  GitHub issue shows description and labels, detail view shows the author;
  chat-created issue body has the three sections; a Start-gated-run with the
  roles knob ON shows PM/Architect/Review phases in the Harness strip and
  blocks on a failing PM verdict.

## Decisions (PM-locked)

> Auto-resolved (autonomous run, 2026-07-02, PM phase run inline by the
> orchestrator — the dispatched sdd-pm died on the account spend limit after
> its verification pass began; all cites below were re-verified inline).
> Overridable by a human note in `ai/STATE.md`.

1. **D1 — The roles setting defaults ON, scoped to start-work-planned
   backlogs only.** `harness.sdd.roles.enabled` defaults `true`; it is read
   exactly once, inside `start_work`'s post-plan knob write — so manually
   registered runs (Harness page, MCP scaffold/plan) are NEVER touched and
   today's behavior there regresses nothing. Rationale: the ask is literally
   "inherited so the user doesn't need to install/enable anything"; start-work
   runs always have a spec for the PM gate to read; the cost (3 extra agent
   spawns per run: PM, Architect, Review) is the product working as designed,
   and the Settings toggle is the global opt-out. This deliberately diverges
   from 005-D3's default-OFF precedent — that knob changed a PASS into a
   possible FAIL for non-web projects; this one only adds gates to a flow
   that is new surface (005) with no installed base.
2. **D2 — Label picker: live fetch behind a thin new seam, static fallback.**
   No repo-label fetch exists in the UI runtime today (PM-verified); add
   `GET /api/github/labels?workdir=…` (thin `gh label list --json name`
   wrapper, same slug resolution as the create route) and fall back to the
   static `type/*` + `priority/*` set when it errors. No label creation from
   the picker (spec 004 D3: ensure-create stays a transition-time concern).
3. **D3 — F4 fix lands server + snapshot, not the dialog.** The create
   response (`CreateIssueResponse`) gains the authenticated login (the `gh`
   CLI knows its user); the composer populates `author` on the snapshot it
   builds at `useComposerState.ts:1455-1470`. The dialog's `?? 'unknown'`
   fallback stays. The architect confirms whether the Tasks LIST payload also
   drops author for fresh rows and, if so, fixes the hydration there too.
