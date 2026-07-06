# Spec 010 — Developer tasks (F1 board bind + F2 board drive)

- **Spec:** 010-end-to-end-autonomous-flow
- **Features:** **F1** — bind (AC 1–3, committed `474cfd12`); **F2** — drive
  (AC 4–8, this iteration).
- **Role:** Developer (sdd-developer)
- **Date:** 2026-07-06
- **Base:** worktree `prd-agentum-end-to-end-autonomous` (F1 on tip `07ea5f53`
  / origin/develop v0.59.0; F2 on tip `e271d833`, origin/develop v0.59.1
  merged)

> **Scope guardrail:** two gated slices so far — F1 (below, committed) then
> **F2 only** in this iteration. **F3** (repo-from-template + `provision_repo`
> + wizard provision step) is **deferred to a later, separate developer
> iteration** — no F3 code was written. F2 is server-only: zero UI files
> touched, zero seam-call-site files touched (`harness/drive.rs`,
> `routes/board_goals.rs`, `routes/harness.rs`, `routes/mcp.rs` unmodified).

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

## F3 — provision (PENDING)

Not started. After F2: template-create argv + `provision_repo` (run-twice
test FIRST, before the commit step) → routes → wizard provision step
(`OPTIONAL_WORKSPACE_STEPS` 4th entry + modal `'provision'` phase;
`useComposerState` untouched).
