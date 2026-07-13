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
