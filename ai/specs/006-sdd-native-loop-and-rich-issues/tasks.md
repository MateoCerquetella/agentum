# Spec 006 — tasks

Developer slice 1 (autonomous /sdd-loop iteration 3): **F1 + F4** implemented.
Developer slice 2 (autonomous /sdd-loop iteration 4): **F2 + F3** implemented —
all four features done. **Developer phase COMPLETE, ready for tester** (see the
bottom of this file for tester notes).

## F1 — rich-issue-create (AC 1–3)

- [x] `CreateIssueBody` gains `#[serde(default)] labels: Vec<String>` — threaded
      into `NewFeature.labels` (`routes/github.rs`); absent labels = `Vec::new()`,
      wire byte-identical (argv half already pinned by task_sink's
      `gh_create_argv_*`).
- [x] `GET /api/github/labels` route + `list_labels` handler — same host +
      `resolve_github_slug` as `create_issue` (typed 422 `no_github_repo` on
      miss), `gh label list --repo <slug> --json name --limit 100` via
      `gh_in_dir` from `neutral_cwd`; gh failure = plain 400 (client falls back).
- [x] Pure `parse_label_names` — skips nameless entries, case-insensitive sort,
      dedup.
- [x] `fetchGithubRepoLabels` in `runtime/github-issue-client.ts` — mirrors
      `fetchGithubIssueBody`'s abort shape, 6 s default budget.
- [x] `createGithubIssue` input gains `labels?: string[]` — omitted from the
      POST body when empty (pre-006 wire byte-identical).
- [x] New `lib/issue-context-body.ts` — pure `composeIssueContextBody`
      (both-blank → `undefined`; sections `['## Context', prompt?, '**Note:** …'?]`
      joined with `\n\n`, no trailing newline) + `STATIC_FALLBACK_LABELS`
      (the `type/*`+`priority/*` set from `.github/labels.sh`).
- [x] Composer state: `createIssueLabels` (reset on submit-success + form close)
      + `createIssueLabelOptions` (null = loading); fetch-on-open with
      `.catch(() => [...STATIC_FALLBACK_LABELS])`; `handleToggleCreateIssueLabel`.
- [x] Chip-toggle label row in `NewWorkspaceComposerCard`'s create-issue form,
      between the body textarea and the error row; selected chips render filled.
- [x] Blank-body fallback in `handleCreateIssueSubmit` via the EXISTING
      `agentPromptRef`/`noteRef` (deps grow only by `createIssueLabels`, never
      per keystroke); the `linkedContext` snapshot uses the same effective body.
- [x] Labels threaded into BOTH snapshot objects — the `applyLinkedWorkItem`
      cast (`GitHubWorkItem.labels` is required) and `LinkedWorkItemSummary`
      (gains optional `labels?: string[]`).
- [x] Created-issue chip renders a compact label row — `SmartWorkspaceNameSelection`
      gains optional `labels?: string[]`, populated from `linkedWorkItem.labels`
      in the `smartNameSelection` memo; rendered on the selection pill in
      `SmartWorkspaceNameField.tsx` (the component that shows `#<number> <title>`).
- [x] Tests: `create_issue_body_labels_default_empty`,
      `parse_label_names_maps_sorts_and_skips_nameless`, vitest
      `lib/issue-context-body.test.ts` (5 cases, exact strings); existing
      `gh_create_argv_*` pins untouched-green.

## F2 — chat-sdd-shape (AC 4–5)

- [x] Byte-identity pin `compose_issue_body_without_problem_goal_is_byte_identical`
      written FIRST against the pre-change code (full-string literal of the
      summary + 3-prioritized-tasks body) and run green BEFORE any edit.
- [x] `FeaturePlan` gains `#[serde(default)] problem: Option<String>` +
      `goal: Option<String>` (Deserialize-only stays).
- [x] `EXTRACT_INSTRUCTIONS` extended — schema names `"problem"`/`"goal"`, the
      two field specs appended after the ordering sentence, every existing
      sentence + the raw-JSON tail kept.
- [x] `compose_issue_body`: present-but-blank = absent (trim + filter); `!sdd`
      → today's exact composition (pinned); `sdd` → summary lead → `## Problem`
      → `## Goal` → `## Acceptance criteria` with the IDENTICAL shared task-line
      rendering + same footer.
- [x] `chat_issues_preview` json gains `"problem"`/`"goal"` (nullable, C4).
- [x] UI passthrough (C4): `DraftPlan` gains `problem?`/`goal?`;
      `previewIssuesFromChat`'s explicit response mapping keeps them; the
      Confirm-side `createIssuesFromChat` plan rebuild forwards them (see
      deviation 2 — this rebuild is where they would otherwise drop).
- [x] Tests §3 items 1–7: byte-identical pin, blank-fallback, SDD-shape order,
      serde defaults, AC 5 round-trip (`spec_md_from_issue` →
      `derive_backlog_from_spec`, 3 features, no fallback checkbox),
      EXTRACT_INSTRUCTIONS guard; existing compose/plan_to_features tests
      untouched-green.
- [x] Handoff-02 mandatory (a): fake-gh wire test
      `chat_plan_body_reaches_gh_create_argv_non_empty` through
      `plan_to_features` → `TaskSink::Github.create_feature` — NUL-separated
      argv log; asserts `--body` non-empty, contains the summary AND a
      `- [ ] **[High]** …` line (see deviation 1 for the `gh_bin()` enabler).
- [x] Handoff-02 mandatory (b): stored-turn/DraftReview restore-path
      investigation — verdict below ("Stored-turn investigation").

## F3 — roles-inherited (AC 6–8 + C1)

- [x] Verdict-contract pin `role_prompt_verdict_contract_is_character_identical`
      written FIRST against the pre-change code (exact rendered
      `=== HOW TO RECORD YOUR VERDICT ===` block) and run green BEFORE any
      brief edit; still green after.
- [x] `SDD_ROLES_ENABLED_SETTING` const (`harness.sdd.roles.enabled`, default
      TRUE reads — note the OPPOSITE default from the QA knob).
- [x] `apply_start_work_knobs(list, agent_tool, agent_model, sdd_roles)` pure
      helper; called from `start_work`'s `update_backlog_knobs` closure with the
      setting read ONCE before (`unwrap_or(true)` on a store hiccup). `roles`
      only ever SET, never written false.
- [x] `HarnessSettings` gains `sdd_roles_enabled`; new `HarnessSettingsPatch`
      (two `Option<bool>`, serde-default); PUT writes only `Some` keys and
      returns the full effective settings via a shared `read_settings` (C2).
- [x] C1 fix: `shared_tracker_provenance(&FeatureList)` in `harness/types.rs`;
      Decompose (`drive.rs` `run_pre_feature_phases`) re-plans via
      `plan_from_spec_with_tracker` when it returns `Some` — the status-label
      trail survives roles-on runs. Zero diffs in `run_role_gate`/`decide_gate`/
      `parse_role_verdict`/the phase machine.
- [x] Brief deltas applied VERBATIM from architecture §4: `pm.md` gate-bullet
      block replaced (10 bullets); `architect.md` +3 bullets; `reviewer.md`
      +2 bullets; everything else byte-identical.
- [x] UI: `HarnessSettings` client type gains `sddRolesEnabled`;
      `setHarnessSettings` takes `Partial<HarnessSettings>` (existing one-field
      caller compiles unchanged); `SddRoleLoopToggle` in `IntegrationsPane`
      (clone of `BrowserQaGateToggle`, initial `useState(true)`), rendered
      beside it; composer fetches `getHarnessSettings()` when
      `canStartGatedRun` (keyed effect, cancellation guard, optimistic `true`
      on failure) and the armed copy names the role loop when enabled (AC 8).
- [x] Tests §4 items 1–7: `sdd_roles_setting_defaults_on_and_round_trips`,
      `start_work_knobs_stamp_roles_only_when_enabled`,
      `harness_settings_wire_shape_is_camel_case` updated to the exact
      two-field string, `harness_settings_patch_accepts_partial_puts`, the
      verdict-contract pin, `shared_tracker_provenance_reads_stamped_backlog`
      + `_none_when_unstamped`; all existing role/verdict/brief/start-work
      tests untouched-green.

## F4 — author-hydration (AC 9, D3)

- [x] `CreateIssueResponse` gains `author: Option<String>` — additive wire,
      serializes `"author":null` when absent.
- [x] Best-effort `authenticated_github_login(host)` — `gh api user --jq .login`
      from `neutral_cwd` via the same runner; called AFTER a successful create;
      any failure → `None` (warn-logged), never an error. No cache (stale across
      `gh auth switch`; creates are click-frequency).
- [x] Pure `parse_gh_login` — trimmed, non-empty stdout.
- [x] `CreatedGithubIssue` TS type gains `author: string | null`.
- [x] Both composer snapshots populate `author: created.author ?? null`;
      `LinkedWorkItemSummary` gains optional `author?: string | null`.
- [x] The dialog's `?? 'unknown'` fallback UNTOUCHED (D3); NO Tasks-LIST change
      (C3 — the LIST payload already carries author).
- [x] Tests: `create_issue_response_serializes_author_present_and_null`,
      `parse_gh_login_trims_and_rejects_empty`.

## Deviations (slice 1)

1. **`parse_label_names` sorts with `sort_by_key(|n| n.to_lowercase())`** instead
   of the blueprint's `sort_by` comparator sketch — clippy `-D warnings`
   (`unnecessary_sort_by`) rejects the comparator form. Behavior identical
   (stable, case-insensitive); the named test pins it.
2. **Label-options fetch is a keyed effect** (`[createIssueOpen, selectedRepoPath]`)
   rather than a fire inside `handleCreateIssueOpenChange(true)` — architecture
   §2 explicitly offered this as developer's choice; one mechanism covers both
   fetch-on-open and refetch-on-repo-change-while-open, with a cancellation guard.
3. **`SmartWorkspaceNameField.tsx` touched** (not in §2's boundaries table): the
   chip that shows `#<number> <title>` is that component's selection pill, so the
   "one-component addition at that site" (arch §2, AC 2 render) lands there —
   optional `labels` on `SmartWorkspaceNameSelection` + a compact chip row.
4. **`labels` set unconditionally on both snapshots** (possibly `[]`), not only
   when non-empty — `GitHubWorkItem.labels` is required anyway, and the chip
   renders only when `labels?.length`, so empty selection still shows no row
   (the pinned AC 2 behavior).

## Gate results (slice 1, 2026-07-02)

- `cargo fmt --all` — clean.
- `cargo test -p agentum-server --lib` — **522 passed, 0 failed, 5 ignored**
  (100.90 s, final-state run after the clippy fix; `routes::github` 7/7).
- `cargo clippy --workspace --all-targets -- -D warnings` — **green** (48.72 s)
  after deviation 1.
- `npm run build --prefix crates/agentum-desktop/ui` — **built in 2m 17s**
  (pre-existing chunk-size warnings only).
- `npx vitest run src/lib/issue-context-body.test.ts` — **5 passed** (368 ms).

## Deviations (slice 2)

1. **`TaskSink::Github`'s create arm now spawns `gh_bin()` instead of a literal
   `"gh"`** (`task_sink.rs`). Not in §3's boundaries table, but the handoff-02
   mandatory wire test needs to intercept the real spawn, and `AGENTUM_GH_BIN`
   is already the production knob the transition arm honors — this makes create
   consistent with transitions. Behavior without the env var is byte-identical
   (`gh_bin()` defaults to `"gh"`).
2. **`createIssuesFromChat` (chat-client.ts) explicitly forwards
   `problem`/`goal`** in its plan rebuild. Architecture §3/C4 said "Confirm's
   POST already spreads the stored object", but the client function REBUILDS
   the plan as `{title, summary, tasks}` (dropping unknown fields) before
   posting — without this addition the SDD fields would silently drop on every
   Confirm. This is the C4 passthrough landing at its real seam.
3. **The env-locked wire test uses `TEST_ENV_LOCK` +
   `#[allow(clippy::await_holding_lock)]`** — sanctioned by handoff 02 for the
   unavoidable case: `create_feature` spawns internally (no injectable program
   parameter), so env mutation is the only interception point. Same accepted
   pattern as `ensure_spec_and_plan_fires_todo_at_plan`. All other new tests
   are pure or tempdir-Store (no env mutation).
4. **`shared_tracker_provenance` tests live in `harness.rs`'s `surface_tests`**
   (not a new types.rs tests mod) — keeps the tests-mod-at-EOF rule trivially
   satisfied and sits beside the existing `copy_knobs_preserves_config_but_not_features`.

## Stored-turn investigation (handoff-02 mandatory item b, Mateo's empty-description report)

**Verdict: NOT reproducible — there is no draft-plan restore path that could
lose `summary`/`tasks` before Confirm.** Evidence:

- `draftPlan` is ephemeral React state (`ChatPage.tsx` —
  `useState<DraftPlan | null>(null)`); its only non-null assignment is the
  fresh `previewIssuesFromChat` response in `openPreview`. It is never
  persisted.
- Stored turns persist only `content`/`thinking`/`filed`
  (`runtime/chat-history.ts` — `StoredTurn`); `FiledResult` carries created
  issue titles/urls/failures, **never a plan**. No code path rebuilds a
  `DraftPlan` from a `StoredTurn`, so reopening a stored conversation cannot
  reach Confirm without a fresh Preview (fresh extraction).
- Draft edits spread siblings (`{ ...p, ...patch }` in `patchPlan`/`patchTask`)
  — no field dropping mid-edit.
- The one real dropped-field seam on the Confirm path is
  `createIssuesFromChat`'s explicit plan rebuild (chat-client.ts): it already
  forwarded `title`/`summary`/`tasks` explicitly (so today's fields could not
  drop), and it WOULD have dropped the new `problem`/`goal` — fixed in this
  slice (deviation 2).
- Server-side, an empty `--body` cannot come from this chain:
  `compose_issue_body` always emits the heading + checklist + footer (a
  no-tasks plan is a 422 before any create), and the new fake-gh wire test
  pins the argv end-to-end. If Mateo's empty-description issue recurs, the
  cause is outside the plan→gh chain (e.g. a different create surface).

## Gate results (slice 2, 2026-07-02)

- Pins-first method honored: both pins written and run green against the
  pre-change code before any edit (`compose_issue_body_without_problem_goal_is_byte_identical`,
  `role_prompt_verdict_contract_is_character_identical` — 2 passed, 527
  filtered out at that point).
- `cargo fmt --all` — clean (reformatted `read_settings`/test literals only).
- `cargo test -p agentum-server --lib` — **535 passed, 0 failed, 5 ignored**
  (164.23 s; up from slice 1's 522 — 13 new tests: 7 in `routes::chat`, 3 in
  `routes::harness` (the wire pin was updated in place, not added), 3 in
  `harness::surface_tests`).
- `cargo clippy --workspace --all-targets -- -D warnings` — **green** (44.31 s).
- `NODE_OPTIONS=--max-old-space-size=3072 npm run build --prefix crates/agentum-desktop/ui`
  — **built in 2m 4s** (pre-existing chunk-size warnings only).

## Tester notes (developer phase COMPLETE)

- **Env-lock usage:** exactly one new test mutates env
  (`chat_plan_body_reaches_gh_create_argv_non_empty`, `AGENTUM_GH_BIN` under
  `TEST_ENV_LOCK`); everything else is pure/tempdir.
- **NOT GUI-verified** (unit + build gates only): the Settings "SDD role loop
  on gated runs" toggle rendering + optimistic write; the composer armed-copy
  switch (role-loop wording when the knob is ON); the chat Preview → Confirm
  round-trip filing an SDD-shaped body against a real GitHub repo; the
  PhaseStrip showing PM/Architect/Review on a start-work run; the C1
  regression (status/* label still flips at InProgress on a roles-on run) —
  qa.sh items per architecture §7.
- **Different defaults trap:** `setting_get_bool(BROWSER_QA…, false)` vs
  `setting_get_bool(SDD_ROLES…, true)` — both pinned by tests.
- **Wire compat pinned:** old one-field PUT bodies still parse
  (`harness_settings_patch_accepts_partial_puts`); GET is the exact two-field
  camelCase string.
