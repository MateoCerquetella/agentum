# Spec 010 — Developer tasks (F1 ONLY: board bind)

- **Spec:** 010-end-to-end-autonomous-flow
- **Feature:** **F1** — bind: discover a board, resolve the mapping. AC 1–3.
- **Role:** Developer (sdd-developer)
- **Date:** 2026-07-06
- **Base:** worktree `prd-agentum-end-to-end-autonomous` (tip `07ea5f53`,
  origin/develop v0.59.0 merged)

> **Scope guardrail:** this iteration implements **F1 only**. **F2** (the
> `github_transition_with_board` seam arm + board writes + id cache) and **F3**
> (repo-from-template + `provision_repo` + wizard provision step) are
> **deferred to later, separate developer iterations** — no F2/F3 code was
> written. `task_sink.rs` is byte-identical (zero lines changed — the allowed
> `neutral_cwd` widening wasn't even needed; it was already `pub(crate)`).

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

## F2 — drive (PENDING)

Not started (per the one-slice ruling, D6). Next developer iteration:
pure builders + `run_gh_capture` + close/reopen/state argv (test-first) →
`board_write_with` + id cache + invalidate-retry + probe-gated close/reopen
(fake-gh suite) → `github_transition_with_board` + the two task_sink arm
hooks LAST (AC 8: existing label tests stay green **unmodified**).

## F3 — provision (PENDING)

Not started. After F2: template-create argv + `provision_repo` (run-twice
test FIRST, before the commit step) → routes → wizard provision step
(`OPTIONAL_WORKSPACE_STEPS` 4th entry + modal `'provision'` phase;
`useComposerState` untouched).
