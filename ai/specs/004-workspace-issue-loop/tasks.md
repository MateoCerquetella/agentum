# Spec 004 — Tasks

Status board for the developer phase. Build order per D4: F1 → F2 → F3 → F4.

## Checklist

- [x] **F1 — github-status-transition** (AC 3–5) — DONE, unit gate green
  - [x] TDD: `gh_set_status_label_argv_adds_one_removes_exactly_the_other_three`
        written first, confirmed red (E0425), then implemented.
  - [x] `GITHUB_STATUS_LABELS` table + `github_status_label(phase)` (D3 colors:
        todo/ededed, in-progress/1d76db, ready-to-test/fbca04, done/0e8a16).
  - [x] Pure argv builders `gh_label_ensure_argv` (8 tokens incl. `--force`) +
        `gh_set_status_label_argv` (add target, remove exactly the other three
        canonical labels — C4: foreign `status/*` never named).
  - [x] `github_slug_and_number_from_issue_url` — https://github.com only,
        tolerates trailing slash/query/fragment, rejects `/pull/`, non-numeric.
  - [x] `gh_bin()` (AGENTUM_GH_BIN → "gh") + `run_gh` (neutral_cwd, 30s
        `tokio::time::timeout` → "gh timed out", stderr capped ~240 chars).
  - [x] `github_transition_with(program, …)` — 4 non-fatal ensure-creates, then
        ONE `issue edit` decides Applied/Skipped. NEVER returns Err (AC 5).
  - [x] Seam widened: `apply_tracker_transition(store, provider, tracker_id,
        tracker_url: Option<&str>, phase)`; board/linear ignore the new param.
  - [x] Call sites: `harness/drive.rs::transition_tracker` threads
        `feature.tracker_url.as_deref()` (one logical change; rustfmt reflowed
        the call vertically — control flow untouched); `board_goals.rs`
        initial-Todo passes `url.as_deref()` (literally one line).
  - [x] Tests: 8 per architecture §2 — label uniqueness, ensure-argv shape,
        add-one/remove-three invariant, URL parser accept/reject,
        `github_transition_without_url_is_skipped` (REPLACES
        `github_transition_is_a_logged_noop`), fake-gh Applied (5 invocations,
        edit last), fake-gh failure→Skipped(stderr), arity updates to the two
        board-arm tests. Fake-gh tests are `#[cfg(unix)]`, program passed
        explicitly — no env mutation.

- [x] **F2 — worktree-linked-metadata** (AC 2) — DONE, unit gate green
  - [x] `CreateBody` +3 `#[serde(default)]` fields; `linked_pr` carries
        `alias = "linkedPR"` (C3). Alias on CreateBody ONLY.
  - [x] `create()` persists all three into the registry row (was hard-coded
        None) → `{worktree}` response serializes them.
  - [x] Detected-scan wire key fixed: `"linkedPr"` → `"linkedPR"` (the key the
        UI actually reads, shared/types.ts:233; old key had zero readers).
  - [x] `canonical_meta_key("linkedPR") → "linkedPr"` applied in `update_meta`
        before the insert loop (post-create edits hit the typed field instead
        of shadowing it in `extra`).
  - [x] NO serde alias added to the registry `Worktree` struct (wipe hazard —
        architecture §3); regression-guarded by the extended serialization test
        (pins on-disk keys, asserts `linkedPR` absent).
  - [x] TS: `tauri/worktrees.ts` create-shim forwards
        linkedIssue/linkedPR/linkedLinearIssue (was stripping them — C2);
        `runtime/server-worktree-client.ts` `worktreesCreate` args widened.
  - [x] Tests: `create_body_accepts_ui_linked_keys` (+ `linkedPr` variant),
        `create_body_defaults_absent_linked_fields`,
        `canonical_meta_key_maps_linkedPR`, extended
        `worktree_serializes_camel_case_and_flattens_extra`.

- [x] **F3 — composer-create-issue** (AC 1) — DONE, unit gate green
  - [x] `POST /api/github/issues` in `routes/github.rs` per the blueprint
        contract (`CreateIssueBody{title, body?, workdir, slug?}` camelCase →
        `CreateIssueResponse{provider:"github", number:i64, url, slug}`).
        Flow: blank-title 400 → `resolve_github_slug` (miss → typed 422
        `no_github_repo`, chat.rs's exact envelope) →
        `TaskSink::Github.create_feature(SinkCtx{slug: Some})` →
        `map_sink_error` → `FeatureRef.id` → i64 (junk = 500). Registered in
        `router()`; `is_public` untouched (rides `require_token`).
  - [x] `board_goals::map_sink_error` widened to `pub(crate)` (one word +
        doc note).
  - [x] UI: `createGithubIssue()` in `runtime/github-issue-client.ts`;
        `useComposerState` gains the mini-form state/handlers
        (`canCreateGithubIssue` = nothing linked + local git repo; success →
        `applyLinkedWorkItem` + `setLinkedWorkItem` with typed-body
        `linkedContext` snapshot + `setLinkedIssue`; failure → inline error,
        zero state change); affordance + inline form rendered in
        `NewWorkspaceComposerCard` (the modal's shared markup surface) under
        the Name field.
  - [x] Tests: `create_issue_rejects_blank_title`,
        `issue_number_parses_digits_and_rejects_junk`.

- [x] **F4 — spec-from-issue-scaffold** (AC 6–7) — DONE, unit gate green
  - [x] `routes/github.rs::get_issue` core extracted into `pub(crate)
        fetch_github_issue(state, workdir, number, slug_hint) →
        FetchedIssue{title, body, url, slug}` (`url` added to the `gh --json`
        fields; `get_issue` delegates — no wire change; `slug` carries a
        documented `#[allow(dead_code)]`, no in-tree reader yet).
  - [x] `harness/types.rs`: `spec_md_from_issue` (verbatim body; C0/C1
        stripped, `\t`→two spaces, `\r` dropped; 64 KiB cap + `[truncated]`;
        fallback `## Acceptance criteria\n\n- [ ] <title>` decided by running
        the REAL `derive_backlog_from_spec` on the assembled doc; checkbox
        lines stay bare) + `issue_spec_id` (`<number>-<slug>`, `[a-z0-9-]`
        alphabet, 40-char cap, fallback `issue` — traversal-proof) +
        `plan_from_spec` refactored to `plan_from_spec_inner(…, tracker:
        Option<(&str,&str)>)`; `plan_from_spec` delegates `None` (MCP tool
        unchanged); `plan_from_spec_with_tracker` stamps
        `tracker_provider`/`tracker_url` on EVERY feature (F1 prerequisite).
  - [x] `routes/harness.rs`: `POST /api/harness/spec-from-issue`
        (`SpecFromIssueRequest{workdir, number, slug?, plan=true}` →
        `SpecFromIssueResponse{specId, specPath, written, features?}`).
        `expand_workdir` + `is_dir` → server fetch → `scaffold_harness`
        (idempotent) → existing spec.md = 400 "already exists — not
        overwriting" (never overwrite) → write → optional tracker-stamped
        plan.
  - [x] UI (D5): `scaffoldSpec` toggle (default **false**) in
        `useComposerState`, rendered only when a github.com issue is linked
        AND the target repo is a local git repo; shared
        `maybeScaffoldSpecFromIssue(worktree, submitLinkedWorkItem)` called
        after `createWorktree` in BOTH `submit` and `submitQuick` (before the
        skip-session early return; uses `result.worktree.path`); failure →
        console + toast, non-fatal. `scaffoldSpecFromIssue()` client fn added.
  - [x] Tests (the 6 from architecture §5, in `harness.rs::surface_tests`):
        checkbox round-trip incl. checked→Done; fallback AC; control-strip +
        64 KiB cap; traversal-proof id (incl. cap + fallback cases); tracker
        stamping on every feature + on-disk; `plan_from_spec` delegation
        unchanged (no stamping).

## Verify gate results (F1+F2 slice, 2026-07-01)

- `cargo fmt --all` → `cargo fmt --all -- --check` clean.
- `cargo test -p agentum-server --lib` → **486 passed; 0 failed; 5 ignored**
  (~75s wall, well under the 8-minute budget; ignored = pre-existing live/gh
  tests). Scoped runs: `task_sink` 19 passed / 1 ignored; `routes::worktrees`
  12 passed.
- `cargo check -p agentum-server` → clean, no warnings.
- vite build deferred to the end of the developer phase (ran below).

## Verify gate results (F3+F4 + authoritative UI build, 2026-07-01)

- `cargo fmt --all` → `cargo fmt --all -- --check` clean.
- `cargo test -p agentum-server --lib` → **494 passed; 0 failed; 5 ignored**
  (~68s wall; +8 new: 2 github.rs, 6 harness surface_tests).
- `cargo check -p agentum-server` → clean, no warnings.
- `NODE_OPTIONS=--max-old-space-size=6144 npm run build --prefix
  crates/agentum-desktop/ui` → **green** ("✓ built"), covering slice 1's TS
  widenings + F3/F4 UI. Pre-existing chunk-size warnings only.

## Notes for F3/F4 (read before starting)

- Nothing staged/committed — the orchestrator commits per green slice.
- **F1's GitHub arm reads slug AND number from `tracker_url`** — F4's
  `plan_from_spec_with_tracker` must stamp `tracker_url` on every derived
  feature or transitions skip with "feature has no tracker_url".
- `run_gh`/`github_transition_with`/`gh_bin` are private fns in
  `task_sink.rs`; F3's endpoint goes through `TaskSink::Github::create_feature`
  (existing), NOT these.
- F3: `routes/board_goals.rs::map_sink_error` needs a one-word `pub(crate)`
  (architecture §4). F3 is a NEW `POST /api/github/issues` — the Tasks-page
  path is a local stub (C1).
- F4: go through the `plan_from_spec` inner refactor, NOT
  `write_backlog_from_features`; `- [ ]` lines stay bare (prefixing breaks
  `derive_backlog_from_spec`); keep-existing spec.md → 409-style BadRequest.
- No `is_public` additions; new routes ride the global `require_token` layer.
- The line numbers in architecture.md predate the 35-commit develop merge —
  grep symbols, don't trust `:line`.
