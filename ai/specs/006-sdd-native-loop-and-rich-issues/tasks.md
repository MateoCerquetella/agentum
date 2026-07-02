# Spec 006 — tasks

Developer slice 1 (autonomous /sdd-loop iteration 3): **F1 + F4** implemented.
F2 + F3 are the next slice — routes/chat.rs, harness_roles/, and the harness
settings are untouched in this slice by scope contract.

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

## F2 — chat-sdd-shape (AC 4–5) — NEXT SLICE

- [ ] `FeaturePlan` gains serde-default `problem`/`goal`; `EXTRACT_INSTRUCTIONS`
      names them.
- [ ] `compose_issue_body` three-section rendering; byte-identical pin written
      FIRST against the pre-change literal.
- [ ] Preview endpoint + UI `DraftPlan` passthrough (C4).
- [ ] Round-trip fixture through `spec_md_from_issue` → `derive_backlog_from_spec`.
- [ ] Handoff-02 mandatory item: fake-gh wire test through the Github sink arm
      (`--body` non-empty, contains summary + a `- [ ]` line) + the
      stored-turn/DraftReview restore-path investigation (Mateo's empty-body
      report).

## F3 — roles-inherited (AC 6–8) — NEXT SLICE

- [ ] `SDD_ROLES_ENABLED_SETTING` (default ON) + `apply_start_work_knobs`.
- [ ] `HarnessSettings` GET-full / PUT-patch split (C2); pin test updated.
- [ ] `shared_tracker_provenance` + Decompose tracker fix (C1).
- [ ] Brief deltas (pm/architect/reviewer.md); verdict-contract pin written FIRST.
- [ ] Settings toggle + armed copy + composer settings fetch (AC 8).

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
