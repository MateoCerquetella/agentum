# Spec 020 — Tasks

## F1 — host-aware-slug-family (server) — BUILT ✅ (2026-07-13)

Increment of spec 020 F1 per `architecture.md` §2 and
`handoffs/02-architect-to-developer.md`. Server-only; no UI files touched
(F2/F3 surfaces untouched).

### Pre-build collision sweep (handoff step 0)

Re-ran at build time, clean:
- `grep -rn "repo_id\|repoId" crates/agentum-server/src/routes/{github,github_projects,provision}.rs` → no hits.
- `grep -n '"/api/repos' crates/agentum-server/src/routes/repos.rs` → no `/slug` route.

### What was built

1. **`routes/util.rs`** — the shared resolver family (§2.1):
   - `resolve_tracker_host(state, repo_id)` — explicit repoId →
     `repos::load_host_for_repo` (404 unknown id / 400 deleted host); absent
     or blank → local host. Blank-is-absent via trim/filter.
   - `resolve_tracker_slug(state, repo_id, workdir, slug_hint)` — the ONE
     `{workdir, slug?, repoId?}` → slug resolver; body = the former
     `github_projects.rs::resolve_slug` verbatim with the host line swapped.
     Ordering contract honored: workdir shape-check → expand → host
     (repoId-aware) → `resolve_github_slug` (hint short-circuits inside with
     zero git I/O). Unknown repoId 4xxes even with a valid hint.
   - Pure `no_github_repo_envelope(reason)` — the typed 422; code
     `no_github_repo` byte-identical, messages distinguish
     NoGithubRemote vs HostUnreachable (the github_projects precedent).
2. **`routes/repos.rs`** — pure `host_id_of(&[Repo], &str)` extracted from
   `resolve_repo_host_id` (now a thin `read_repos()?` wrapper).
3. **DTO widenings** (add-only, `#[serde(default)]`, NO aliases, NO
   `rename_all` additions; query structs use per-field `rename = "repoId"`):
   `BindingQuery`, `PutBindingRequest` (github_projects.rs); `IssueQuery`,
   `CreateIssueBody`, `LabelsQuery` (github.rs); `ProvisionRequest`
   (provision.rs).
4. **Site swaps** (§2.3):
   - `github_projects.rs::resolve_slug` **deleted**; get/put/delete binding
     handlers call `util::resolve_tracker_slug` with their `repo_id`.
   - `provision.rs::resolve_slug` **deleted**; `provision_workspace` calls the
     util resolver. The `workdir.is_dir()` local-only gate STAYS (module doc
     updated to say only the slug half is host-aware).
   - `github.rs::create_issue` — one util call for the slug;
     `TaskSink::Github` filing and `authenticated_github_login` stay LOCAL
     (§2.3.3 ruling; "why local" comments written). The local host is derived
     via `resolve_tracker_host(&state, None)`.
   - `github.rs::list_labels` — one util call for the slug; `gh label list`
     stays LOCAL (§2.3.4; "why local" comment written).
   - `github.rs::fetch_github_issue` — gains `repo_id: Option<&str>` (param
     after `state`); host via `resolve_tracker_host`; its `gh issue view`
     runs on the RESOLVED host (as before — `&host` was already threaded);
     the plain-400 slug-error contract kept. Callers: `get_issue` passes
     `q.repo_id.as_deref()`; `harness.rs` (spec-from-issue + sdd-loop) pass
     `None` with byte-identical-pin comments.
5. **Untouched by contract (D5)**: `board_goals::resolve_github_slug`,
   `SlugReason`, `is_valid_slug`, `task_sink.rs`, hint semantics,
   `wiki.rs`'s local pin, `auth.rs::is_public`.

### Test-first evidence

Tests written BEFORE implementation; red run (`cargo test -p agentum-server
--lib`) failed with 28 compile errors pinning exactly the missing surface:
`cannot find function resolve_tracker_host/resolve_tracker_slug/host_id_of`,
`no field repo_id on type …` for all six DTOs.

New tests (9):
- `util::tests::resolve_tracker_host_absent_repo_id_is_local` (+ blank-id arm)
- `util::tests::resolve_tracker_host_unknown_repo_id_is_4xx`
- `util::tests::resolve_tracker_slug_hint_short_circuits_with_unreadable_workdir` (AC 2)
- `util::tests::resolve_tracker_slug_unknown_repo_id_beats_valid_hint` (ordering contract)
- `util::tests::no_github_repo_envelope_distinguishes_reasons` (pure)
- `repos::tests::host_id_of_known_local_is_none`
- `repos::tests::host_id_of_known_remote_is_its_host`
- `repos::tests::host_id_of_unknown_id_is_not_found`
- `github::tests::issue_and_labels_queries_accept_repo_id`

Extended (3, wire pins: `repoId` present deserializes, absent → `None`):
`github_projects::tests::wire_shapes_are_camel_case` (PutBindingRequest +
BindingQuery), `github::tests::create_issue_rejects_blank_title`
(CreateIssueBody), `provision::tests::provision_request_wire_shape_and_partial_mapping_rejected`.
`util::tests` gained the `fresh_state()` clone from board_goals tests.

### Gates

| Gate | Result |
|---|---|
| `cargo test -p agentum-server --lib` | **696 passed / 0 failed / 5 ignored** (baseline 687/0/5 → +9; all existing tests green unmodified) |
| `cargo fmt --all` (`--check` clean) | ✅ |
| `cargo clippy -p agentum-server --lib --tests -- -D warnings` | ✅ 0 warnings |
| `npm run build --prefix crates/agentum-desktop/ui` | ✅ (no wire-type breakage; no UI files touched → no vitest gate) |

### Deviations (numbered)

1. **(architecture-sanctioned, §1.5.2)** `create_issue` / `list_labels` /
   `provision` 422 *messages* now carry the HostUnreachable ≠ NoGithubRemote
   distinction (previously the generic "no GitHub repo resolved for this
   project"). `code: "no_github_repo"` byte-identical; UI branches on code
   only (verified by the architect's grep).
2. **(architecture-sanctioned, §2.3.2b)** `provision`'s slug read now
   tilde-expands its workdir like every sibling — a no-op for the absolute
   paths its own `is_dir` gate already requires.
3. **`create_issue`'s `SinkCtx.workdir` now receives the client's trimmed
   workdir, not the tilde-expanded one.** The expand happens inside the shared
   resolver, so the handler no longer holds the expanded path. No behavior
   change: this route always passes `slug: Some(_)`, and `TaskSink::Github`'s
   explicit-slug arm runs `gh` from `$HOME` — `ctx.workdir` is shape-only
   there (task_sink.rs, "workdir is never used as cwd").
4. **Placement of the local-host derivation in `create_issue`:** the
   architecture's consistency note says re-derive the local host for
   `authenticated_github_login` but not where; it is derived BEFORE the sink
   create so a missing local-host row still fails before any `gh` call —
   preserving today's failure ordering (a post-create failure would 500 a
   request whose issue was already filed).
5. **`fetch_github_issue`'s new param position**: `repo_id` sits after
   `state` (identity-first, matching the util resolver's signature); the
   architecture didn't pin a position.
6. **Line drift (cosmetic):** `provision.rs::resolve_slug` spanned `:35-61`
   including its doc comment (architecture cited `:39-61`); all other anchors
   matched.
7. **`ai/STATE.md` was modified concurrently by another agent** during this
   build — left unstaged/uncommitted (only F1 files + this tasks.md are in
   the commit).

### Blocking notes for F2/F3

None. `resolve_tracker_host` is in place for F2's `repo_slug` route
(`load_host_for_repo` + `resolve_repo_path` are already `pub(crate)`), and the
DTO wire fields F3's clients will target (`repoId` on binding/create/fetch)
are live and pinned by serde tests.

## F2 — slug-index-ssh (route + renderer) — BUILT ✅ (2026-07-13)

Increment of spec 020 F2 per `architecture.md` §3 and
`handoffs/02-architect-to-developer.md`. F3 surfaces untouched
(use-tracker-intake.ts, ProjectBindingEditor, the runtime github clients);
`start-work-repo-match.ts` / `ProjectViewWrapper` untouched by contract.

### Pre-build collision sweep (handoff step 0)

Re-ran at build time, clean:
- `grep -n '"/api/repos' crates/agentum-server/src/routes/repos.rs` → no
  `/slug` route (base-ref family only).
- No `getServerRepoSlug` / `repo-slug-arm` anywhere under `ui/src`.

### What was built

1. **`routes/repos.rs`** — `GET /api/repos/{id}/slug` (§3.1, exact shape):
   - `RepoSlugResponse { slug }` — object, add-only-friendly, no `source`
     constant.
   - Pure `slug_reason_wire(reason)` — `NoGithubRemote` → 422
     `no_github_remote` (semantic); `HostUnreachable` → **502**
     `host_unreachable` (transport must never masquerade as no-origin).
   - `repo_slug` handler: `resolve_repo_path` (404 unknown id) →
     `load_host_for_repo` → `board_goals::resolve_github_slug(&host, &path,
     None)` on the **registry path** (no hint/workdir params by design);
     errors wrapped as `ApiError::Custom` with the
     `{"error":{"code","message"}}` envelope. Slug case passed through
     (client lowercases). Auth: behind the existing top-level `require_token`;
     **no `is_public` change**.
2. **`ui/src/runtime/server-repo-client.ts`** — `getServerRepoSlug(repoId)`
   (§3.2 verbatim); `server-http`'s `run()` throws on non-2xx, so callers
   fail closed.
3. **`ui/src/lib/repo-slug-arm.ts`** (NEW, pure, import-free) —
   `slugResolutionArm(environmentTarget, connectionId)`:
   environment-RPC > server-for-connectionId-repos > native-local.
4. **`ui/src/lib/repo-slug-index.ts`** — `resolveRepoSlug` switches on the
   arm inside the EXISTING `try`: the `server` arm calls `getServerRepoSlug`,
   lowercases (parity with the native arm), caches, returns; any throw falls
   to the existing catch → cache `null` → repo excluded (AC 7 fail-closed by
   construction). The environment-RPC and native arms are byte-identical;
   the module-scope `slugByRepoId` cache and its eviction are untouched
   (§1.4 — the arm is a pure function of the immutable `connectionId` +
   runtime scope, so a cache key can never silently change arms). Module doc
   updated to describe the three arms.
5. **Untouched by contract**: `start-work-repo-match.ts` (+ its tests),
   `ProjectViewWrapper` wiring, the native `gh_repo_slug` Tauri command, the
   environment-RPC branch, `auth.rs::is_public`, F3's intake/binding files.

### Test-first evidence

- **Rust red:** tests written first; `cargo test -p agentum-server --lib`
  failed with 10 compile errors pinning exactly the missing surface
  (`cannot find function slug_reason_wire`/`slug_on_host`, `cannot find
  struct RepoSlugResponse`, `StatusCode` unresolved).
- **Vitest red:** `bunx vitest run src/lib/repo-slug-arm.test.ts` failed —
  module `./repo-slug-arm` does not exist.

New Rust tests (5, in `repos::tests`):
- `slug_reason_wire_distinguishes_transport_from_semantic` (422/502, codes,
  distinct messages — the §3.5 error-shape pin)
- `repo_slug_response_serializes_slug_only` (`{"slug":"Owner/Repo"}` serde pin)
- `slug_on_host_reads_github_origin` (local temp repo WITH GitHub origin →
  slug; handler core split out so the registry file is never touched)
- `slug_on_host_without_origin_is_no_github_remote_422` (local repo WITHOUT
  origin → Custom 422, code `no_github_remote`, never the 502)
- `repo_slug_unknown_id_is_not_found` (random-uuid 404 via the handler's
  first gate; env-tolerant, no env mutation — the 015 house rule)

New vitest (4, `src/lib/repo-slug-arm.test.ts`): environment target wins over
connectionId (RPC untouched even for SSH repos); environment + local;
connectionId → `server`; null vs undefined connectionId both `native`.
Regression pins re-run unmodified: `start-work-repo-match.test.ts` (7 tests —
sole-remote `direct` at `:26`, both-hosts `choose` at `:32`, AC 6).

### Gates

| Gate | Result |
|---|---|
| `cargo test -p agentum-server --lib` | **701 passed / 0 failed / 5 ignored** (F1 baseline 696/0/5 → +5; all existing tests green unmodified) |
| `cargo fmt --all` (`--check` clean) | ✅ |
| `cargo clippy -p agentum-server --lib --tests -- -D warnings` | ✅ 0 warnings |
| `bunx vitest run src/lib/repo-slug-arm.test.ts src/components/github-project/start-work-repo-match.test.ts` | ✅ 2 files, 11 tests |
| `npm run build --prefix crates/agentum-desktop/ui` | ✅ |

### Deviations (numbered)

1. **Handler core split (`slug_on_host`)**: the architecture's §3.1 sketch
   inlines the resolve+map in `repo_slug`; the mapping is extracted into an
   async `slug_on_host(&Host, &str)` so the with/without-origin contract
   (the task's required tests) is testable against a temp git repo without
   mutating the real `~/.agentum/repos.json`. Wire behavior identical.
2. **Line drift (cosmetic):** `resolveRepoSlug`'s try body sat at
   `repo-slug-index.ts:67-91` (architecture cited `:59-91` for the whole fn —
   matched); no functional drift found anywhere.
3. **`ai/STATE.md` concurrent drift** (same as F1 deviation 7): modified by
   another agent during this build — left uncommitted; only F2 files + this
   tasks.md are in the commit.

### Blocking notes for F3

None. The `repoId` DTO fields F3 threads are live (F1), and
`DraftedGithubIssueBody`'s widening point (`runtime/github-issue-client.ts`)
plus `ProjectBindingEditor`/`use-tracker-intake.ts` are untouched as promised.
Note for the tester: the new route's 502 (`host_unreachable`) is
wire-distinguishable from the binding family's 422 `no_github_repo` envelope —
qa.sh's "host-down shows the unreachable-flavored error" can key on the status.

## F3 — intake-ssh-honest (UI + one server flag) — BUILT ✅ (2026-07-13)

Increment of spec 020 F3 per `architecture.md` §4 and
`handoffs/02-architect-to-developer.md`. Untouched by contract:
`start-work-repo-match.ts`, `repo-slug-arm.ts`/`repo-slug-index.ts` (F2's),
`ProjectBindingEditor`'s binding/discover logic beyond the repoId threading,
the native `gh` commands, the wizard's `trackerWorkdir` gate (threaded but NOT
relaxed — §1.5.5 named follow-up), `fetchGithubRepoLabels` (unwidened per
§4.2), `task_sink.rs`, `auth.rs::is_public`.

### What was built

1. **Server grounding flag (D4, §4.1)** —
   `chat.rs::draft_issue_body` returns
   `pub(crate) struct DraftedIssue { body, grounded_repo, grounded_wiki }`
   (facts captured from `gather_repo_context(...).is_some()` /
   `retrieve_wiki_for_query(...).is_some()` BEFORE the values move into the
   prompt); sole caller `github.rs::draft_issue_body` (verified sole by grep)
   serializes an **always-present, add-only**
   `grounding: {repo: bool, wiki: bool}` next to `body`
   (`DraftGroundingDto`, single-word fields, no renames/aliases).
   `DraftBodyRequest` gets NO `repoId` (deliberate — §1.5.1: the route
   resolves no slug and touches no host).
2. **Client widenings (§4.2)** —
   `runtime/github-projects-client.ts`: pure exported `bindingQuery({workdir,
   slug?, repoId?})` (used by GET + DELETE); `getProjectBinding` /
   `putProjectBinding` / `deleteProjectBinding` inputs gain `repoId?`
   (PUT body spreads it conditionally, the `:176` slug pattern).
   `runtime/github-issue-client.ts`: pure exported `createIssuePayload`
   (extracted from the create body literal); `createGithubIssue` +
   `fetchGithubIssueBody` gain `repoId?`; `DraftedGithubIssueBody` widens to
   `{body, grounding?}` (optional client-side for old-server skew).
3. **Editor + feeders (§4.3)** — `ProjectBindingEditor` gains a `repoId?`
   prop, threaded into its 4 client calls + dep arrays (binding-load effect,
   handleSave, handleToggleDoneCloses, handleUnbind). Feeders:
   ProjectHubPage passes `repoId={repo.id}` (the hub's `repo.path ?` gate
   already passes SSH repos); IntegrationsPane passes `repoId={selected.id}`
   **and drops the `localRepos` filter** (§1.5.4 — comment rewritten to say
   the limitation it documented is what 020 removed); CreateWorkspaceWizard
   computes `trackerRepoId` with the SAME local-only gate as `trackerWorkdir`
   (comment updated: the gate is now a product choice, not technical),
   threads it through `AgentStep` → `TrackerSection` (new `repoId?` prop) →
   the editor mount.
4. **Intake hook + panel (§4.4)** — `use-tracker-intake.ts`: binding read
   threads `repoId: repo.id` (the leg that un-dead-ends SSH repos; deps gain
   `repo.id`); file threads `repoId: repo.id` (no-hint robustness when the
   binding read failed and `slug` is null); draft leg unchanged payload
   (slug-first) + captures `grounding` from the response (reset beside the
   `setFiled(null)` per-draft reset); new derived `groundingNote` via
   `deriveDraftGroundingNote(grounding, hostLabel)` with `hostLabel` from the
   `sshTargetLabels` store selector (WorktreeCard precedent, `?? 'a remote
   host'` per §4.4). `TrackerIntakePanel` renders the note after the
   Description field, muted (`text-[11px] text-muted-foreground`), never
   destructive-styled. `filed`/error handling untouched (AC 10).
5. **Pure model (add-only)** — `create-issue-intent-model.ts` gains
   `DraftGrounding` + `deriveDraftGroundingNote`: null flag (pre-020 server)
   → null; `repo: true` → null (wiki-only miss silent — §1.5.3); repo miss →
   the §4.4 exact strings (host-label vs unreadable-folder flavors, "or
   wiki" folded in when both missed). 015's exports untouched.

### Test-first evidence

Tests written BEFORE implementation; red runs pinned exactly the missing
surface:
- **Vitest red:** 13 failures — `deriveDraftGroundingNote is not a function`,
  `bindingQuery`/`createIssuePayload` not exported (module for
  github-projects-client.test.ts had no export).
- **Rust red:** 4 compile errors — `DraftBodyResponse` has no field
  `grounding`, `DraftGroundingDto` not found.

New/extended tests:
- `create-issue-intent-model.test.ts` (+6, add-only describe): null-flag
  silence, grounded silence, wiki-only-miss silence, repo-miss + label
  (wiki true/false — "or wiki" wording), repo-miss without label (both wiki
  variants). **015's 26 existing cases pass unmodified.**
- `runtime/github-projects-client.test.ts` (NEW, 4): `bindingQuery` param
  present iff supplied (workdir-only pre-020 shape, slug, repoId, all three).
- `runtime/github-issue-client.test.ts` (+3): `createIssuePayload` minimal =
  exactly `{title, workdir}`; all optionals present when supplied; empty
  labels array omitted (pre-006/020 wire pins).
- Rust: `github::tests::draft_body_response_serializes_body_and_grounding` —
  exact JSON for grounded and ungrounded shapes. The
  `None`-for-non-local-dir grounding facts stay pinned by the EXISTING
  `chat.rs` tests (`gather_repo_context_none_for_missing_or_empty_workdir`,
  `retrieve_wiki_for_query_is_none_without_a_workdir`) — no LLM-call test
  needed.

### Gates

| Gate | Result |
|---|---|
| `cargo test -p agentum-server --lib` | **701 passed / 0 failed / 5 ignored** (F2 baseline 701/0/5; the F3 serde pin replaced the old `DraftBodyResponse` pin one-for-one — see deviation 1; all other existing tests green unmodified) |
| `cargo fmt --all` (`--check` clean) | ✅ |
| `cargo clippy -p agentum-server --lib --tests -- -D warnings` | ✅ 0 warnings |
| `bunx vitest run` (intent-model + issue-client + projects-client + repo-slug-arm + start-work-repo-match) | ✅ 5 files, **53 tests** (32 + 6 + 4 + 4 + 7) — 015 model cases and F2 arm-picker suite green unmodified |
| `npm run build --prefix crates/agentum-desktop/ui` | ✅ |

### Deviations (numbered)

1. **Existing Rust test amended, not added-beside:**
   `github::tests::draft_body_response_serializes_body_field` asserted the
   exact pre-020 JSON (`{"body":"…"}`) of the struct F3 changes — it IS the
   response's wire-shape pin, so it was renamed/updated to
   `draft_body_response_serializes_body_and_grounding` with the new
   always-present shape. Test count unchanged (701). Every other existing
   test is untouched.
2. **IntegrationsPane empty-state string updated** ("Add a local repo first"
   → "Add a repo first") — follows the sanctioned filter drop (§1.5.4): with
   SSH repos now listed, "local" in the empty-state would be wrong.
3. **`trackerRepoId` threads through `AgentStep`:** the architecture's §4.3
   names the mount (`:1383`) and `TrackerSection`, but the mount lives inside
   the `AgentStep` subcomponent — `trackerRepoId` is passed wizard →
   `AgentStep` (new prop) → `TrackerSection` → editor. Same gate, one extra
   hop the blueprint's line-anchors didn't show.
4. **Line drift (cosmetic):** `chat.rs::draft_issue_body` sat at
   `:1871-1903` as cited; `github.rs`'s draft route at `:323-347`
   (architecture cited `:311-335` — F1's DTO comments shifted it); editor
   calls at `:94/:229(+2)/:264(+2)/:292(+3)`; hub mount `:274`; pane filter
   `:238`; wizard gate `:392-393`, mounts `:1383(+5)/:1608(+8)`. No
   functional drift.
5. **`ai/STATE.md` concurrent drift** (same as F1 dev. 7 / F2 dev. 3):
   modified by another agent during this build — left uncommitted; only F3
   files + this tasks.md are in the commit.

### Developer-phase verdict

All three slices (F1 `09726c46`, F2 `e8fb31a8`, F3 this commit) built and
green on every gate. Spec 020 ACs 1–10 implemented; ready for the tester.
