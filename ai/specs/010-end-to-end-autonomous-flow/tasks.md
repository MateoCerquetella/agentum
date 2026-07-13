# Spec 010 — Developer tasks (F1 board bind + F2 board drive + F3 provision)

- **Spec:** 010-end-to-end-autonomous-flow
- **Features:** **F1** — bind (AC 1–3, committed `474cfd12`); **F2** — drive
  (AC 4–8, committed `0b03eb9e`); **F3** — provision (AC 9–10, this
  iteration; AC 11 = the human/qa.sh demo, not a build item).
- **Role:** Developer (sdd-developer)
- **Date:** 2026-07-06
- **Base:** worktree `prd-agentum-end-to-end-autonomous` (F1 on tip `07ea5f53`
  / origin/develop v0.59.0; F2 on tip `e271d833`, origin/develop v0.59.1
  merged; F3 on tip `2dbc63cf`)

> **Scope guardrail:** three gated slices — F1 and F2 (below, committed), then
> **F3 only** in this iteration: repo-from-template + the ONE injectable
> idempotent `provision_repo` ensure + the wizard provision phase. F3 touches
> zero seam-call-site files, zero F2 seam code (`github_projects.rs` itself is
> UNTOUCHED by F3 — the provision core got its own runner), and only the two
> allowed `pub(crate)` widenings in `task_sink.rs`.

F1 was built in the architecture's §8 order: **types+persistence → the pure
mapper (tests first) → discovery+classifier → routes → UI**.

---

## F1 — what was built

### Step 1 — `crates/agentum-server/src/github_projects.rs` (NEW, domain module)

The `linear.rs` precedent: domain logic at crate root, routes thin, task_sink
calls in later (F2).

- **Types:** `BoardPhase` (5 variants, projects-local) + `From<TrackerPhase>`
  (4 arms); `StatusMapping` (five REQUIRED `String` option-id fields —
  unmapped phase unrepresentable; a stored file missing a phase fails
  deserialization → reads as "no binding", AC 2d); `StatusNames` (five-name
  twin, all-default so stale metadata can't brick a binding); `BoardBinding`
  with `#[serde(default = "default_true")] done_closes_issue` (D1's ON default
  in ONE definition site, §7.7) + optional display metadata
  (title/owner/ownerType/number/optionNames).
- **Persistence (D2 = a2):** `github_projects_config_path()`
  (`AGENTUM_GITHUB_PROJECTS_CONFIG` env override →
  `<data_local_dir|data_dir>/Agentum/github_projects.json` — the
  `task_sink::github_config_path` pattern), `read_bindings_at`
  (absent/garbled → Default), `binding_for_slug` (lowercased key, fresh read
  per call), `upsert_binding`, `remove_binding`, `static WRITE_LOCK` guarding
  the RMW. File shape: `{ "bindings": { "<lowercase slug>": BoardBinding } }`.
- **The pure fuzzy mapper (built test-first):** `normalize` (lowercase, keep
  `[a-z0-9]`), the five disjoint synonym tables VERBATIM from architecture
  §3.4, `resolve_status_mapping(&[StatusOption])` — exact-normalized match
  only (no substring), first-hit in discovery order, exactly two fallbacks
  (RTT→InProgress, Blocked→InProgress) flagged `MatchVia::FellBack`, refusal
  naming the unmapped core phase(s) + the option names (never partial).
  `unmapped_core_phases()` is the pure sibling the discover route reports on
  the wire.
- **Discovery:** `gh_graphql_argv` (pure; `-f` strings / `-F` ints — the
  desktop `gh_projects.rs::graphql` argv discipline), `run_gh_graphql`
  (tokio Command from `task_sink::neutral_cwd()`, 30 s timeout mirroring
  `run_gh`, parses stdout JSON first — `errors[]` → classifier, no JSON →
  stderr classifier), `parse_discovery` (pure, fixture-tested; org OR user
  root; null project → `not_found`; missing/non-single-select Status field →
  actionable `no_status_field`), `discover_status_field(program, owner,
  owner_type, number)` — ONE call, owner node validated+interpolated
  (`owner_node`, user fallback), login always `$owner`.
- **Classifier:** `ProjectsError { kind: &'static str, message }`, kinds
  `scope_missing | auth_required | not_found | no_status_field |
  network_error | unknown`. The `scope_missing` message is the CONSTRUCTED
  remedy: `` "GitHub Projects needs the `project` token scope. Run: gh auth
  refresh -s project" `` (both classifier paths).
- Registered in `lib.rs` (`pub mod github_projects;`).

### Step 2 — `crates/agentum-server/src/routes/github_projects.rs` (NEW)

Per architecture §3.5, camelCase DTOs, all authed (NO `is_public` changes):

- `POST /api/github/project-binding/discover` `{owner, ownerType, number}` →
  `{projectId, title, statusFieldId, options, resolved | null,
  unmappedPhases}`. A mapper refusal is NOT a route error (D7): `resolved:
  null` + the unmapped phases → the UI prompts manual selects.
  `scope_missing` → typed **422** `{error:{code,message}}` (the
  `no_github_repo` envelope precedent, github.rs:239–244); other classified
  kinds → the same envelope at **400**.
- `GET /api/github/project-binding?workdir&slug` → `{slug, binding | null}`.
- `PUT /api/github/project-binding` — validates all five option ids non-empty
  (pure `validate_status_mapping`, 400 naming the blank phases), absent
  `doneClosesIssue` → ON via the binding type's one `default_true` site →
  upsert → `{slug, binding}`.
- `DELETE /api/github/project-binding?workdir&slug` → 204 (idempotent).
- Slug resolution: shared `resolve_slug` helper over
  `board_goals::resolve_github_slug` with the `{workdir, slug?}` contract +
  the typed `no_github_repo` 422 — exactly the sibling github.rs routes.
- One `.merge(routes::github_projects::router())` in `lib.rs::router` next to
  `routes::github::router()`; `pub mod github_projects;` in `routes/mod.rs`.

### Step 3 — UI

- **`ui/src/runtime/github-projects-client.ts`** (NEW): `discoverProjectStatus`,
  `getProjectBinding`, `putProjectBinding`, `deleteProjectBinding` — the
  `apiUrl` + `authHeaders` + AbortController pattern of
  `github-issue-client.ts`. `GithubProjectsBindingError` carries the typed
  envelope's `code` so the editor branches on
  `scope_missing`/`auth_required`. Plain relative imports (runtime clients
  stay `@/`-alias-free).
- **`ui/src/lib/github-projects-binding.ts`** (NEW, pure — no DOM/IPC):
  `reduceBindingSelection` (the select-state reducer), `selectionFromResolved`
  (refusal → all-empty, never partial), `selectionForRebind` (stored ids that
  still exist survive re-discovery; deleted columns heal from `resolved`),
  `mappingComplete` (the Save gate), `fallbackHints` (D5 — per-FellBack-phase
  visible hint naming the fallback + the add-column recovery),
  `optionNamesForSelection`, `BOARD_PHASES`/`BOARD_PHASE_LABELS`.
- **`ui/src/components/github-projects/ProjectBindingEditor.tsx`** (NEW, the
  D7 shared component; props `{workdir, slug?, onBound?}`): loads the stored
  binding per repo; unbound → project pick (list via
  `api.gh.listAccessibleProjects()`, paste via
  `api.gh.resolveProjectRef({input})` — the registered desktop READ commands,
  writes stay server-side) → auto-discover → five per-phase selects
  preselected from `resolved` with FellBack hint text, refusal → empty
  selects + completion prompt; `done_closes_issue` toggle;
  Save (PUT) / Re-discover (stored project ref) / Unbind.
  `scope_missing`/`auth_required` render the existing `GhAuthErrorHelp`
  (which embeds the `gh auth refresh -s project` remediation + diagnostics).
- **`ui/src/components/settings/IntegrationsPane.tsx`**: the GitHub card
  gains a "Projects v2 board" section (`GithubProjectsBoardEditor`, an
  in-file section component per the pane's `GithubStatusLabelsEditor` idiom):
  a local-repo selector (bindings resolve via the server's LOCAL host)
  mounting `ProjectBindingEditor` for the picked repo — the
  wizard-independent surface F2 dogfoods on; F3's wizard step mounts the SAME
  component later.

### Step 4 — tests (all green)

`ui/src/lib/github-projects-binding.test.ts` (NEW, vitest ×10) + 20 new Rust
tests (17 in `github_projects.rs`, 3 in `routes/github_projects.rs`) — the
full list is in the developer handoff; every §8-named F1 test exists.

---

## Deviations from architecture.md (F1)

1. **Test name** `gh_graphql_argv_uses_f_for_strings_big_f_for_ints` (§8
   named `…_F_for_ints`) — a literal capital `F` in a test fn name trips
   `non_snake_case`; behavior pinned identically. Risk: none.
2. **Path-injected persistence cores** `binding_for_slug_at` /
   `upsert_binding_at` / `remove_binding_at` (`pub(crate)`) added under the
   public API — the architecture's own "hermetic tests by injection, never
   env mutation" requirement needs a path seam; the public fns delegate to
   them with the default path. Risk: none (additive, crate-private).
3. **`gh_bin()` duplicated into `github_projects.rs`** instead of reusing
   task_sink's private one — the sacred boundary allows only a `neutral_cwd`
   widening in task_sink (which turned out unnecessary: `neutral_cwd` was
   already `pub(crate)` at task_sink.rs:873). Three lines, same
   `AGENTUM_GH_BIN` knob. Risk: a future divergence between the two fns;
   mitigated by the comment cross-linking them.
4. **`unmapped_core_phases()` added** (pure, not in the §3 API sketch) — the
   discover route's `unmappedPhases` wire field needs the phase list without
   string-parsing the mapper's `Err`. Risk: none (pure, tested).
5. **`parseProjectInput` NOT imported** — it is module-private in
   `ProjectPicker.tsx:739` (not exported); exporting it would touch an
   out-of-boundary file. The paste input goes straight to
   `api.gh.resolveProjectRef({input})`, whose Rust side
   (`gh_projects.rs::parse_project_ref`) performs the identical parse. Zero
   duplication, zero foreign-file edits. Risk: one extra IPC round-trip on
   paste — negligible.
6. **Classified non-scope errors return the typed envelope at 400** (§3.5
   says "classified 400 otherwise" without pinning the body): keeping the
   `{error:{code,message}}` shape at 400 lets the UI branch `auth_required`
   → `GhAuthErrorHelp` with one parser. Risk: none (400 preserved; body
   strictly more informative).
7. **The settings mount is an in-file section component**
   (`GithubProjectsBoardEditor` inside IntegrationsPane.tsx) rather than a
   literal one-line mount — the repo selector has to live somewhere, and the
   pane's established idiom is in-file section components
   (`LinearStateMapEditor`, `GithubStatusLabelsEditor`). Risk: none.
8. **Extra UI pure fns** `selectionForRebind` + `optionNamesForSelection`
   (tested): re-discovery on a bound repo preserves surviving hand-edits, and
   PUT ships `optionNames` display metadata. Risk: none (pure, additive).
9. **Bound-view knob toggle PUTs immediately** using the stored binding's
   fields (no forced re-discover just to flip `done_closes_issue`) — within
   AC 3's "persists edits". Risk: none.
10. **Discriminant narrowing workaround in `ProjectBindingEditor.tsx`**: the
   ui tsconfig has `strict: false`, under which only THEN-branch narrowing of
   `ok: true | false` unions applies (else/exclusion doesn't — probe-verified;
   the existing `ProjectPicker.tsx` has the same latent miss, masked by its
   unresolved-import baseline). The two result-handling spots use paired
   positive guards (`if (res.ok) {…} if (res.ok === false) {…}`) so the file
   is tsc-clean under BOTH strict modes (repo tsc baseline 1646 → 1642 with
   this component; zero errors attributable to F1 files). Risk: none.

## Gate results (F1)

| Gate | Command | Result |
|---|---|---|
| Unit | `cargo test -p agentum-server --lib` | **591 passed, 0 failed** (571 baseline + 20 new; zero existing tests modified) |
| Fmt | `cargo fmt --all` then `cargo fmt --all --check` | clean |
| Clippy | `cargo clippy --workspace` | 0 warnings, exit 0 (worktree env note: the desktop crate's build script needs the sherpa/onnxruntime dylibs copied into `target/release/` — known worktree gap, environment-only) |
| UI build | `npm run build --prefix crates/agentum-desktop/ui` | green (1m 23s; `bun install` first — node_modules was absent) |
| Vitest | `npx vitest run src/lib/github-projects-binding.test.ts` | **10 passed** (+ neighboring `integrations-pane-status` / `github-issue-client` suites re-run green; full-suite pre-existing failing baseline untouched) |

---

## F2 — what was built (this iteration; AC 4–8)

Built in the architecture's §8 F2 order: **pure builders + runner (test-first)
→ `board_write_with` + id cache + probe-gated close/reopen (fake-gh suite) →
the two seam fns → the two arm hooks LAST**, with the full suite green before
the hooks landed.

### Step 1 — `crates/agentum-server/src/github_projects.rs` (the write machinery)

- **Pure builders (argv-pinned):** `issue_node_id_query_args(owner, name,
  number)`, `add_item_mutation_args(project_id, content_id)` (idempotent
  ensure-AND-fetch — also AC 11's "chat-filed issue lands in Todo" lazy
  ensure), `update_status_mutation_args(project_id, item_id, field_id,
  option_id)` (`singleSelectOptionId` rides a `String!` var — **option IDs,
  never names**, PRD AC 6); `gh_issue_state_argv` (probe:
  `issue view N --repo slug --json state --jq .state` → bare `OPEN`/`CLOSED`),
  `gh_issue_close_argv`, `gh_issue_reopen_argv`. The three GraphQL operations
  are single-line consts so fake-gh call logs stay one line per invocation.
- **Runners:** `run_gh_capture(program, args)` — the stdout-carrying sibling
  of `task_sink::run_gh` (untouched), same 30 s timeout + neutral cwd +
  ~240-char stderr truncation. `run_gh_graphql` refactored into a thin wrapper
  over a new argv-level `run_gh_graphql_argv` so F1 discovery and every F2
  write ride **ONE runner + ONE classifier** (§4.2 step 2) — a mid-run scope
  miss classifies to the same actionable `gh auth refresh -s project` message
  bind-time gets.
- **The id cache (§7.3):** `static ID_CACHE: LazyLock<Mutex<IdCacheMap>>`,
  process-lifetime, keyed `(lowercase slug, number)` → `(issue_node_id,
  item_id)`, populated only on success (`ensure_item_cold`). No TTL; a dead
  item id heals via invalidate-and-retry-once. Keeps a bound feature run at
  ~9 gh calls (vs ~14 cold) — inside the spec's ≤ ~10 ceiling.
- **`board_write_with(program, binding, slug, number, phase)`** implementing
  §4.2 steps 1–6: cache lookup → cold resolve (node-id query →
  `addProjectV2ItemById`) → the option write every call → stale-cache
  self-heal (option-write failure on a CACHED id ⇒ invalidate + retry ONCE
  cold) → knob-gated `close_or_reopen_for`. Returns `Ok(())` or a reason
  string; never panics, never propagates past the string.
- **`close_or_reopen_for` (§7.4):** probe-then-act BOTH directions, only for
  `Done`/`InProgress` (Blocked and the rest are structural no-ops); knob OFF
  never probes (a human-closed issue on a knob-off binding is respected);
  probe failure = `tracing::warn` + skip; act failure = the returned reason.

### Step 2 — `crates/agentum-server/src/task_sink.rs` (the seam)

- **`github_transition_with_board`** (private) — §4.1's sketch verbatim in
  behavior: the byte-identical `github_transition_with` label path first;
  `binding: None` returns its result untouched; a board `Err(reason)` folds
  into `Skipped("status label applied; Projects board write failed: …")` /
  `Skipped("{why}; Projects board write failed: …")` + `tracing::warn` — loud
  through `drive.rs::transition_tracker`'s existing log line and the MCP
  tool's `skipped:` text with **zero call-site edits** (§7.9).
- **`github_mark_blocked_with_board`** (private) — the blocked sibling:
  `github_mark_blocked_with` (byte-identical) + `board_write_with(…,
  BoardPhase::Blocked)`, identical fold. No close/reopen on Blocked.
- **The two arm hooks (LAST):** both github arms read
  `github_projects::binding_for_slug(&slug)` **only after the URL parse**
  (the same hermeticity discipline as `GithubStateMap::from_env()` — the
  no-url/unparseable skip tests never touch the config files) and call the
  `_with_board` fns. The 004-D1 comment above the pipeline arm now notes the
  010-D1 supersession for bound repos.
- **Doc-only:** `TransitionResult::Skipped`'s docstring widened to "not fully
  applied — the reason names what did and didn't land". No new variant; no new
  pub API; `TrackerPhase` stays 4 variants.

### Step 3 — tests (13 new; every §8-named F2 test exists)

In `github_projects.rs` (10): `issue_node_id_query_args_shape`,
`add_item_mutation_args_shape`, `update_status_mutation_uses_option_id`,
`gh_issue_close_reopen_state_argv_shapes`,
`board_write_with_fake_gh_cold_is_three_graphql_calls`,
`board_write_second_call_hits_cache_one_call`,
`board_write_invalidates_stale_item_and_retries_once_cold`,
`done_closes_open_issue_and_skips_closed`, `in_progress_reopens_closed_only`,
`knob_off_never_probes_closes_or_reopens`. In `task_sink.rs` (3):
`github_transition_with_board_unbound_is_byte_identical` (the exact
5-invocation log + no GraphQL), 
`github_transition_with_board_board_failure_is_skipped_note_still_ok` (the
AC-7 pin, with the scope remedy in the reason),
`blocked_arm_moves_card_to_blocked_option_with_fake_gh`. The board fake-gh
switches on query content for canned JSON per operation and answers the
`issue view` probe; bindings/programs inject explicitly — no env mutation.
**Cache isolation:** `ID_CACHE` is process-global and the test binary is one
process, so every F2 test uses its OWN slug (no `#[cfg(test)]` clear helper —
the established global-state pattern).

## Deviations from architecture.md (F2)

1. **Two private seam fns instead of the boundary table's "one"**
   (`github_mark_blocked_with_board` mirrors `github_transition_with_board`):
   §4.1 says the blocked arm "gets the same pattern", and the named test
   `blocked_arm_moves_card_to_blocked_option_with_fake_gh` must inject the
   binding explicitly (the hermeticity rule forbids env mutation) — an
   inlined combine in `apply_blocked_transition` would be untestable without
   it. Zero new pub API either way. Risk: none (both private, same fold).
2. **`run_gh_graphql` internally refactored** into `run_gh_graphql_argv` +
   a 2-line wrapper so the pure builders and the ONE-runner rule compose;
   discovery behavior unchanged (all F1 tests green unmodified). Risk: none.
3. **Probe argv carries `--jq .state`** (architecture §4.2 step 6's exact
   probe shape) so stdout is the bare state token; parsing is
   `trim().trim_matches('"')` + uppercase, defensive against a quoted token.
   Risk: none.
4. **Close/reopen ACT failure returns `Err(reason)`** (folds into the Skipped
   note); only the PROBE failure is the architecture's "skip with a warn".
   The act-failure side wasn't pinned; surfacing it follows the 008
   never-silent doctrine and stays best-effort (the card move already
   landed). Risk: none (loud-but-Ok either way).
5. **`LazyLock` (std) instead of the sketch's `once_cell::Lazy`** — the
   crate's existing precedent (`usage.rs`); once_cell isn't a dependency.
   Risk: none.

## Gate results (F2)

| Gate | Command | Result |
|---|---|---|
| Unit | `cargo test -p agentum-server --lib` | **604 passed, 0 failed, 5 ignored** (591 baseline + 13 new; re-run green after fmt) |
| AC 8 proof | `git diff --stat` / deleted-lines audit | only `github_projects.rs` + `task_sink.rs` changed; the 7 deleted lines = the 2-line runner refactor, the `Skipped` docstring, 2 arm comment lines, the 2 arm caller lines — **zero test edits**, `github_transition_with`/`github_mark_blocked_with` byte-identical |
| Fmt | `cargo fmt --all` then `cargo fmt --all --check` | clean |
| Clippy | `cargo clippy --workspace` | 0 warnings |
| UI | — | not required (no UI files touched) |

## F3 — what was built (this iteration; AC 9–10)

Built in the architecture's §8 F3 order: **argv pins + template mode
(test-first) → `provision_repo` core (the run-twice test was written and RUN
RED against a stubbed commit step — 2 failures on the stub — BEFORE the
commit step was implemented; the handoff's test-first discipline, provable
from the session) → routes + `lib.rs` merge → UI (pure module → goal-step
template mode → provision step + modal phase → client fns)**.

### Step 1 — `crates/agentum-server/src/provision.rs` (NEW, domain core)

Home decision (the prompt left it open): a **new crate-root `provision.rs`**,
NOT inside `github_projects.rs` — provisioning spans labels (task_sink),
boards (github_projects), the harness scaffold and git, so it is its own
domain; the `linear.rs`/`github_projects.rs` precedent (domain at crate root,
routes thin) applies, and `github_projects.rs` (1.8k lines of F1/F2 seam
code) stays byte-untouched.

- **Argv pins (pure):** `gh_repo_create_from_template_argv`
  (`["repo","create",slug,"--template",tpl,"--private"|"--public","--clone"]`),
  `gh_repo_clone_argv`, `gh_repo_view_argv` (the existence probe),
  `gh_project_create_argv`
  (`["project","create","--owner",owner,"--title",title,"--format","json"]`).
- **`parse_project_create_output`:** JSON → the created project's `number`
  (the one field discovery needs). The `--format json` field names were
  **verified against the REAL local gh 2.92.0** (`gh project list --format
  json`, read-only — same per-project serialization as create) and frozen in
  the fixture: `{closed,fields,id,items,number,owner,public,readme,
  shortDescription,title,url}`. Garbage/missing number → an `Err` quoting the
  output (never a panic).
- **`create_repo_from_template(program, owner, name, template, directory,
  private)`** per §5.1: `target/.git` exists ⇒ `created:false`; `gh repo
  view` probe exists ⇒ clone; missing ⇒ create `--clone`; cwd = directory;
  post-condition check that the clone landed. gh stderr surfaces VERBATIM
  (bounded at 400 chars) — the "template not marked template" case reads
  unedited.
- **`ProvisionCtx { program, bindings_path, workdir, slug, project:
  Option<ProjectChoice>, status_mapping, done_closes_issue, commit_scaffold,
  state_map }` → `provision_repo` → `ProvisionReport { labels, project,
  binding, scaffold, commit }`**, each step independent + best-effort:
  1. **labels** — provision's OWN 5-ensure loop (4 configured via the
     `pub(crate)`-widened `gh_label_ensure_argv` + `github_status_color`, +
     the fixed blocked label) — `github_transition_with`'s pinned ensure
     sequence untouched (AC 8). `--force` ⇒ re-runs converge; `changed` is
     structurally `false` (gh doesn't report created-vs-updated).
  2. **project link-or-create GUARDED by `binding_for_slug_at`** — an
     existing binding ⇒ both steps `changed:false "already bound"`, create
     AND discovery skipped entirely (THE AC-10 "no second project" rule).
     Else Link ⇒ F1 `discover_status_field`; Create ⇒ project-create →
     parse number → discovery; `status_mapping` override wins else
     `resolve_status_mapping` (fallbacks OK — a fresh board's default
     Todo/In Progress/Done resolves with the two locked fallbacks);
     constructor + `upsert_binding_at`. Discovery/mapper failures carry the
     classified message (incl. the `gh auth refresh -s project` remedy)
     into the step detail.
  3. **scaffold** — `scaffold_harness(workdir)` UNTOUCHED (wrapped);
     `changed` mirrors its written list.
  4. **commit (only when `commit_scaffold`)** — rewrite
     `.agentum-harness/.gitignore` from the blanket `*` to the STATE-ONLY
     ignore (`feature_list.json`, `handoff.md`, `qa/` stay ignored — §6.8),
     write-if-different; `git add` the exact 5 contract paths
     (`COMMIT_PATHS`, the server twin of the UI's
     `provisionCommitFileList()`); porcelain-empty ⇒ `committed:false`, NO
     commit (the AC-10 unchanged-count mechanism) and no push; else
     `git commit -m "chore: provision agentum harness scaffold"` (no
     AI-attribution trailer) + `git push origin HEAD` plain, never
     `--force`; red push ⇒ `pushed:false` + error, NON-fatal; branch =
     `rev-parse --abbrev-ref HEAD` reported. Consent OFF keeps the blanket
     `*` untouched.
- Registered `pub mod provision;` in `lib.rs`.

### Step 2 — `crates/agentum-server/src/routes/provision.rs` (NEW, thin routes)

- `POST /api/github/repo-from-template` `{owner, name, templateRepo,
  directory, visibility?}` → `{slug, path, created}`. Pure validators
  (tested): repo name = one path segment (traversal unrepresentable), owner =
  bare login, template = `owner/repo`, visibility ∈ {private (default),
  public}. `expand_workdir` + `is_dir` guard on directory. A gh failure = 400
  with gh's stderr verbatim.
- `POST /api/workspace/provision` `{workdir, slug?, project?: link|create,
  statusMapping?, doneClosesIssue?, commitScaffold}` → `ProvisionReport`
  (single-word fields ⇒ camelCase = the derived serde output; no rename
  layer needed). `expand_workdir` + `is_dir`; slug via a local `resolve_slug`
  following the F1 route's approach (`board_goals::resolve_github_slug` +
  the typed `no_github_repo` 422); `project` disambiguated by a pure
  `project_choice` (create needs title, link needs number — named 400s); a
  PRESENT-but-partial statusMapping = named 400. Absent `doneClosesIssue` →
  ON via `github_projects::default_true` (the one D1 definition site).
  Local-host only; all authed; NO `is_public` changes.
- One `.merge(routes::provision::router())` in `lib.rs`; `pub mod provision;`
  in `routes/mod.rs`.

### Step 3 — UI

- **`ui/src/lib/workspace-provision-step.ts`** (NEW, pure):
  `DEFAULT_TEMPLATE_REPO` (`goempirical/empirical-sdd-ddd-starter`, D4 UI
  constant), `provisionCommitFileList()` (the exact 5 paths, branch-agnostic),
  `deriveTemplateRepoName` (= `slugifyGoalName`), `isTemplateModeReady` /
  `firstTemplateModeBlocker` (never-silent gating), the `ProvisionReport`
  wire types, `summarizeProvisionReport` (5 lines; red push = warning naming
  the error + "push manually").
- **`ui/src/lib/workspace-goal-step.ts`**: `OptionalWorkspaceStepId` widened
  with `'provision'`; the 4th `OPTIONAL_WORKSPACE_STEPS` entry appended
  (`{id:'provision', …, skippable:true, primitive:'provisionWorkspace'}`).
  `isGoalStepReady` / `GoalStepInputs` / `initialComposerPhase` /
  `ComposerModalPhase` UNTOUCHED (diff-verified: only the doc comment, the
  type widening, and the appended entry).
- **`ui/src/runtime/github-projects-client.ts`**: +`createRepoFromTemplate`
  and `provisionWorkspace` (same `apiUrl`+`authHeaders`+AbortController
  pattern; 180 s defaults — create/clone/push ride the network); report
  types imported from the pure lib.
- **`NewWorkspaceGoalStep.tsx`**: workdir-target mode toggle — "Existing
  project" (today's combobox JSX preserved) | "New repo from template"
  (owner / name live-seeded from the goal until hand-edited / template
  default-editable / directory + Browse via `api.repos.pickFolder` / private-
  public). Template Continue: `createRepoFromTemplate` (spinner; inline
  verbatim error) → register the clone via the store's **`addRepoPath`** —
  traced from the add-repo dialog's submit (`AddRepoDialog.tsx:356
  → store/slices/repos.ts:398`), the SAME action, no parallel registration —
  → `onContinue(goal, repo.id)`.
- **`NewWorkspaceComposerModal.tsx`**: phase state widened modal-LOCALLY to
  `ComposerModalPhase | 'provision'`; goal-first Continue → `'provision'` →
  details; "Skip to details" and opinionated opens (`initialComposerPhase`
  untouched) never see it; `provisionWorkdir` = the chosen repo's ROOT path
  (worktree creation happens later in the composer). `QuickTabBody` and every
  `useComposerState` prop byte-identical.
- **`NewWorkspaceProvisionStep.tsx`** (NEW): mounts the SHARED
  `ProjectBindingEditor` (D7's second mount) for link mode; a create-board
  form (owner/ownerType/title, prefilled from the resolved slug) for D5's
  create mode; the D8 consent checklist — commit toggle default ON naming
  the target branch ("the project's current branch"; the authoritative
  branch name renders in the post-run report) + the exact 5-path file list;
  "Provision & continue" runs the ensure and renders the per-step report
  inline (failures = amber warnings, "creation continues"); "Skip" always
  available; a repo with no GitHub origin gets a visible skip-able notice,
  never a dead end.

### Step 4 — tests (17 new: 12 Rust + 5 vitest describe-blocks)

`provision.rs` (9): `gh_repo_create_from_template_argv_shape`,
`gh_repo_clone_argv_shape` (incl. the probe argv),
`gh_project_create_argv_shape`, `parse_project_create_output_frozen_fixture`,
`provision_run_twice_changes_nothing` (the AC-10 pin: temp git repo + bare
origin + logging fake gh + injected bindings path; run 2 = no `project
create`, no graphql, binding file byte-identical, scaffold `changed:false`,
`rev-list --count` equal), `provision_skips_commit_when_consent_off` (+ the
§6.8 blanket-`*`-intact assert), `provision_red_push_is_nonfatal_and_reported`,
`provision_with_existing_binding_never_creates_a_project`,
`gitignore_rewrite_is_write_if_different_and_keeps_state_ignored` (REAL
`git check-ignore` proves state ignored + contract files trackable).
`routes/provision.rs` (3): `project_choice_parses_link_and_create_and_rejects_malformed`,
`repo_name_and_visibility_validation`,
`provision_request_wire_shape_and_partial_mapping_rejected`.
Vitest `workspace-provision-step.test.ts` (15 tests): the exact-5-paths pin
(+ never lists engine state), `deriveTemplateRepoName`, template gating
(order, traversal/template rejection, D4 constant), `summarizeProvisionReport`
(green names the branch; red push = warning + error + "push manually"; failed
step keeps detail; consent-off/no-change = "no new commit").

## Deviations from architecture.md (F3)

1. **Domain home = new crate-root `provision.rs`** (§5.1's header put the
   core inside `routes/provision.rs`; the developer prompt made it my call).
   Routes stay thin per the repo's `linear.rs` precedent; `github_projects.rs`
   stays untouched. Risk: none.
2. **`ProvisionCtx.project` is `Option<ProjectChoice>`** (blueprint: required
   `Link|Create`). `None` = "no board requested" (`ok:true, changed:false`,
   pointing at the Settings mount) — needed because link mode binds through
   the SHARED editor (the binding then already exists server-side), so the
   provision call must be expressible WITHOUT fabricating a Link from
   possibly-absent stored metadata; also gives non-board repos a labels+
   scaffold+commit path. The AC-10 guard is unchanged (run-twice passes
   `Some(Create)` both runs and pins no-second-project). Risk: none
   (strictly widens; the required-shape requests behave per blueprint).
3. **`ProvisionCtx.state_map` added** (injected `GithubStateMap`; route =
   `from_env()`, tests = `Default`) — without it the labels step would read
   the USER's real `github.json` inside tests, violating the handoff's
   hermeticity rule; mirrors F2's seam-`map` injection. Risk: none.
4. **`BLOCKED_LABEL` tuple duplicated** into provision.rs
   (`task_sink::GITHUB_BLOCKED_LABEL` is private; only the two fn widenings
   were allowed there) — the F1 `gh_bin()` duplication precedent, cross-link
   comment both ways not needed since task_sink is boundary-frozen; comment
   in provision.rs says "keep in sync". Risk: drift, mitigated by comment.
5. **Test name `parse_project_create_output_frozen_fixture`** (§8 named it
   `parse_project_create_output`) — a test fn cannot share the imported fn's
   name in the same module (E0255); the F1 naming-deviation precedent.
   Risk: none.
6. **Own `run_in` runner** (program+args+cwd, 120 s, 400-char verbatim
   stderr) instead of reusing `github_projects::run_gh_capture` — that one is
   private, pinned to `neutral_cwd()` and 30 s; template create/clone need a
   caller cwd and a network-sized bound, and widening it would touch F2 seam
   code. Risk: none (discovery still rides the ONE F1/F2 graphql runner).
7. **Labels `changed` is always `false`** — `gh label create --force` is a
   converging ensure with no created-vs-updated signal; reporting
   `changed:true` on run 1 would be a guess. Detail says "ensured". Risk:
   none (the run-twice pin doesn't key off it).
8. **`resolve_slug` copied** into routes/provision.rs (private in the F1
   route file, which F3 must not touch) with a keep-in-sync comment — the
   prompt's "reuse the approach". Risk: drift; ~20 lines.
9. **Pre-run consent names the branch generically** ("the project's current
   branch") — the repo store carries no branch field and adding a
   branch-read would grow scope; the AUTHORITATIVE branch name is reported
   post-run from `CommitReport.branch` (displayed in the inline report).
   The file list is exact per D8. Risk: cosmetic.
10. **Goal-step header copy updated** ("…file a tracker issue, and provision
    the repo next — all optional") to match the now-four optional steps the
    list below it renders. Risk: none (copy only).

## Gate results (F3)

| Gate | Command | Result |
|---|---|---|
| Unit | `cargo test -p agentum-server --lib` | **616 passed, 0 failed, 5 ignored** (604 baseline + 12 new; re-run green after fmt) |
| Test-first proof | run-twice + red-push run RED against the stubbed commit step (2 failures), then green after implementing it | honored |
| Deletion audit | `git diff -U0` over task_sink.rs/lib.rs/routes/mod.rs | exactly 2 deleted lines = the two fn signatures replaced by their `pub(crate)` versions; **zero test edits, zero other Rust files touched** (`github_projects.rs`, `harness/types.rs`, seam call sites all clean in `git status`) |
| Fmt | `cargo fmt --all` then `--check` | clean |
| Clippy | `cargo clippy --workspace` | 0 warnings, exit 0 |
| UI build | `NODE_OPTIONS=--max-old-space-size=3072 npm run build --prefix crates/agentum-desktop/ui` | green (3m 26s; chunk-size warning pre-existing) |
| Vitest | `npx vitest run src/lib/workspace-provision-step.test.ts src/lib/workspace-goal-step.test.ts src/lib/github-projects-binding.test.ts` | **37 passed** (15 new + 12 goal-step with ONLY the steps pin updated three→four + 10 F1 binding held) |
| tsc parity | `npx tsc --noEmit` | 1642 errors = the recorded pre-F3 baseline exactly; the only F3-file hits are the pre-existing `shared/types` bare-tsc resolution misses (import lines renumbered, same category) — **zero new** |
