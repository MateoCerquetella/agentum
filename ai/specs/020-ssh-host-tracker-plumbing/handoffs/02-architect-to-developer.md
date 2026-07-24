# Handoff 02 — Architect → Developer

- **Spec:** 020-ssh-host-tracker-plumbing
- **Date:** 2026-07-13
- **Artifact:** `ai/specs/020-ssh-host-tracker-plumbing/architecture.md`
  (read it in full first — §2.3 and §1.5 carry the subtle rulings; this file
  is only the build order + gates + drift map).
- **Base:** this worktree (`fixes-new-workspace`, spec-015 commits on develop
  `4f98453f`). 015 is a hard prerequisite and is already here. Collision
  sweep @ design time was clean (no `repoId` DTO fields, no `/api/repos/{id}/slug`)
  — **re-run it before coding** (specs 016–018 land from another worktree):
  `grep -rn "repo_id\|repoId" crates/agentum-server/src/routes/{github,github_projects,provision}.rs`
  and `grep -n '"/api/repos' crates/agentum-server/src/routes/repos.rs`.
  If either hits, stop and reconcile.

## Build order

### F1 — host-aware-slug-family (server only)

1. ▲ Tests first (red): the six util/repos tests of architecture §2.4
   (items 1–6) + the DTO serde pins (item 7). Copy `fresh_state()` from
   `board_goals.rs::tests:22` into `routes/util.rs::tests`.
2. `routes/util.rs`: add `resolve_tracker_host`, `resolve_tracker_slug`,
   pure `no_github_repo_envelope` (§2.1 — signatures + ordering contract:
   host resolves BEFORE the hint short-circuit; unknown repoId 4xxes even
   with a valid hint).
3. `routes/repos.rs`: extract pure `host_id_of(&[Repo], &str)` under
   `resolve_repo_host_id` (`:397-403`).
4. DTO widenings (§2.2 table — query structs use per-field
   `rename = "repoId"`, body structs are already camelCase; `#[serde(default)]`
   everywhere; NO aliases, NO `rename_all` additions).
5. Site swaps (§2.3, in this order):
   - delete `github_projects.rs::resolve_slug` (`:45-85`); swap the 3 binding
     handlers (`:314`, `:368`, `:401`);
   - delete `provision.rs::resolve_slug` (`:39-61`); swap `:298` (its `:292`
     `is_dir` gate STAYS);
   - `github.rs::create_issue` `:232-254` → one util call; sink + `authenticated_github_login`
     stay LOCAL (§2.3.3 ruling — write the "why local" comment);
   - `github.rs::list_labels` `:381-401` → one util call; `gh label list`
     stays LOCAL (§2.3.4);
   - `github.rs::fetch_github_issue` gains `repo_id: Option<&str>` param;
     `:102-106` → `resolve_tracker_host`; its `gh` RUNS ON THE RESOLVED HOST
     (already does, `:125`); keep the plain-400 error contract `:115-119`.
     Callers: `get_issue:163` passes `q.repo_id.as_deref()`;
     `harness.rs:324` and `:572` pass `None` (byte-identical pin).
6. Green: `cargo test -p agentum-server --lib && cargo fmt && cargo clippy -p agentum-server -- -D warnings`.

### F2 — slug-index-ssh (route + renderer)

1. ▲ Rust pure test for `slug_reason_wire` + `RepoSlugResponse` serde (§3.5).
2. `routes/repos.rs`: `.route("/api/repos/{id}/slug", get(repo_slug))` +
   handler per §3.1 (registry path, no hint/workdir params; 404 unknown id,
   422 `no_github_remote`, 502 `host_unreachable`). No `is_public` change.
3. ▲ Vitest first (red): new `ui/src/lib/repo-slug-arm.test.ts` (§3.5 cases).
4. `ui/src/lib/repo-slug-arm.ts` (pure, type-only imports) +
   `getServerRepoSlug` in `runtime/server-repo-client.ts` (§3.2) + the
   three-arm switch inside `resolveRepoSlug`'s existing try
   (`repo-slug-index.ts:59-91`; lowercase the server slug like `:82`; the
   `:85-90` catch keeps fail-closed exclusion; cache untouched).
   The runtime-environment RPC arm is UNTOUCHED (spec non-goal).
5. Do NOT touch `start-work-repo-match.ts` / `ProjectViewWrapper` — the e2e
   pins already exist (`start-work-repo-match.test.ts:26,:32`); just run them.
6. Gates: Rust trio + `bunx vitest run src/lib/repo-slug-arm.test.ts
   src/components/github-project/start-work-repo-match.test.ts` +
   `bun run build` (cwd `crates/agentum-desktop/ui`; ui uses **bun**).

### F3 — intake-ssh-honest (UI + one server flag)

1. Server flag (§4.1): `chat::draft_issue_body` returns `DraftedIssue
   { body, grounded_repo, grounded_wiki }` (sole caller `github.rs:325`);
   `DraftBodyResponse` gains always-present `grounding: {repo, wiki}`.
   ▲ serde-shape test.
2. ▲ Vitest first (red): `deriveDraftGroundingNote` matrix in
   `create-issue-intent-model.test.ts` (exact strings in §4.4);
   `bindingQuery` pins in new `runtime/github-projects-client.test.ts`;
   `createIssuePayload` pins added to `runtime/github-issue-client.test.ts`.
3. Client widenings (§4.2): pure exported `bindingQuery` +
   `createIssuePayload` builders; `repoId?` on getProjectBinding /
   putProjectBinding / deleteProjectBinding / createGithubIssue /
   fetchGithubIssueBody; `grounding?` on `DraftedGithubIssueBody`.
   **No `repoId` on `draftGithubIssueBody`** (deliberate — §1.5.1) and
   `fetchGithubRepoLabels` unwidened.
4. `ProjectBindingEditor` `repoId?` prop + 4 call sites + dep arrays; feeders:
   ProjectHubPage `:274` (`repoId={repo.id}`), IntegrationsPane `:266`
   (`repoId={selected.id}` + drop the `localRepos` filter `:238` + comment),
   wizard `TrackerSection` prop + `trackerRepoId` at `:392` with the SAME
   local-only gate (do NOT relax it — §1.5.5).
5. `use-tracker-intake.ts`: `repoId` on binding read (`:104`, + `repo.id` in
   deps) and file (`:212`); `grounding` state from the draft response (reset
   beside `:187`'s `setFiled(null)`); `groundingNote` via the model fn +
   `sshTargetLabels` selector (WorktreeCard `:206` precedent).
   `TrackerIntakePanel`: render the note muted after the Description field
   (`:102`), never destructive-styled. `filed`/error handling untouched.
6. Gates: Rust trio + `bunx vitest run
   src/components/new-workspace/create-issue-intent-model.test.ts
   src/runtime/github-issue-client.test.ts
   src/runtime/github-projects-client.test.ts` + `bun run build`.

## Hard invariants (break = regression we already paid for)

- Absent `repoId` ⇒ success-path behavior byte-for-byte; unknown `repoId` ⇒
  4xx, never silent-local (D1). Unknown-id beats valid-hint.
- DO NOT touch `resolve_github_slug` / `SlugReason` / `is_valid_slug` /
  `task_sink.rs` / hint semantics (D5). DO NOT touch `wiki.rs:410`'s
  local pin (out of scope). No `is_public` additions. No serde aliases.
- 422 `code: "no_github_repo"` stays byte-identical everywhere; only messages
  gain the HostUnreachable/NoGithubRemote split (UI branches on code — verified).
- F2 fail-closed: any slug-route error = repo excluded from the index.
- F3: `filed` only from provider-confirmed responses; errors inline/non-fatal;
  the grounding note comes ONLY from the server flag, never inferred from
  `connectionId` (D4) — the connection label is presentation only.

## Line-drift warnings (re-ground before each edit)

Verified @ design time, but 016–018 merge into these files:

| Anchor | Verified at |
|---|---|
| `github_projects.rs::resolve_slug` / hardcode | `:45-85` / `:63-67`; handlers `:314/:368/:401` |
| `github.rs` hardcodes | fetch `:102-106`, create `:232-236`, labels `:381-385`; draft route `:311-335`; `IssueQuery:41-52`, `CreateIssueBody:172-188`, `LabelsQuery:337-343` |
| `provision.rs` copy / call / is_dir gate | `:39-61` / `:298` / `:292` |
| `repos.rs` helpers | `resolve_repo_path:386`, `resolve_repo_host_id:397`, `load_host_for_repo:410` |
| `chat.rs` | `draft_issue_body:1871-1903`, `gather_repo_context:235` (None-for-non-dir `:238`) |
| harness callers of fetch | `harness.rs:324`, `:572` |
| `repo-slug-index.ts` | cache `:33-41`, native call `:77`, catch `:85-90` |
| clients | projects `:122-146/:153-196/:288-312`; issue `:26-58/:115-151/:189-221` |
| editor + feeders | props `:59-67`, calls `:94/:229/:264/:292`; hub `:274`, pane `:266` (+filter `:238`), wizard gate `:392`, mount `:1383`, editor `:1608` |
| intake hook | binding `:104`, slug cache `:108`, draft `:189-193`, file `:212-217`, filed-reset `:187` |

Toolchain: ui = bun (`bunx vitest run <files>`, `bun run build`); full vitest
(~139 fails) and bare tsc are pre-broken baselines — never gate on them.
Desktop crate itself won't compile in this env (no webkitgtk) — `bun run build`
+ server-crate gates are the verification surface.
