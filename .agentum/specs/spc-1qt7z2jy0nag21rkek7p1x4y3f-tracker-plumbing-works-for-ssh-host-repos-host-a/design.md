# Spec 020 — Architecture Blueprint: SSH-host tracker plumbing

**Self-check passed.** Every load-bearing cite re-verified line-by-line on this
worktree (`fixes-new-workspace` @ spec-015 commits, base develop `4f98453f`,
2026-07-13). D1–D5 honored. **Pre-design collision sweep ran clean**: no
`repoId`/`repo_id` field exists on any DTO in `github.rs` /
`github_projects.rs` / `provision.rs` / `chat.rs`, and no slug route exists
under `/api/repos/*` — nothing from specs 016–018 landed on these surfaces in
this worktree. Nothing here was already built.

- **Status:** Architect → ready for Developer.
- **Order:** F1 → F2 → F3. F1 is the keystone (F2's route and F3's threading
  both sit on its resolver); F2 and F3 are independently severable after F1.

---

## 0. TL;DR — three slices, one sentence each

1. **F1 (server):** one shared resolver pair in `routes/util.rs`
   (`resolve_tracker_host` + `resolve_tracker_slug`) replaces the five pinned
   `get_host(LOCAL_HOST_ID)` sites AND unifies the two admitted-duplicate
   `resolve_slug` copies (`github_projects.rs:45-85`, `provision.rs:39-61`);
   every `{workdir, slug?}` DTO gains an optional add-only `repoId`
   (absent = today's local path byte-for-byte on success; unknown = 4xx).
2. **F2 (server route + renderer):** `GET /api/repos/{id}/slug` in `repos.rs`
   (the `base-ref-default` pattern: `resolve_repo_path` +
   `load_host_for_repo` + `resolve_github_slug`), response `{slug}`, errors
   `404` / `422 no_github_remote` / `502 host_unreachable`; the renderer's
   `repo-slug-index.ts` grows a third arm — `connectionId`-bearing repos call
   the new route via `server-repo-client.ts`, failures stay excluded, the
   existing `slugByRepoId` cache covers it unchanged.
3. **F3 (UI + one server flag):** `repoId` threads through the binding
   read/file legs (`use-tracker-intake.ts`, `ProjectBindingEditor` + three
   feeders, the two runtime clients); the draft response gains an add-only
   `grounding: {repo, wiki}` flag (server-known, D4) that a new pure model fn
   turns into the honest "drafted without repo grounding — files live on
   \<host label\>" note.

---

## 1. Architect calls resolved (the PM's open calls)

### 1.1 Resolver home = `routes/util.rs` (two `pub(crate)` async fns)

`util.rs` is the repo's blessed home for shared route helpers (CLAUDE.md
"Shared route helpers live in `routes/util.rs`"; `expand_workdir` /
`parse_uuid` / `now_millis` already live there). Both new fns are sibling-safe
(`super::repos::load_host_for_repo` is `pub(crate)` at `repos.rs:410`;
`super::board_goals::resolve_github_slug` is `pub(crate)` at
`board_goals.rs:248`). `provision.rs`'s hand-copy (its `:35-38` comment
*admits* the duplication) and `github_projects.rs`'s original are both deleted
in favor of direct calls — the "keep the two in sync" liability disappears.

### 1.2 F2 route shape = `GET /api/repos/{id}/slug`, response `{ slug }`

- **Home:** `routes/repos.rs` — the route is repo-id-addressed and `repos.rs`
  already owns the id→path/host helpers and the exact sibling pattern
  (`base_ref_default`, `repos.rs:431-436`: `resolve_repo_path` +
  `load_host_for_repo` + host-aware git).
- **Response:** an object, not a bare string (future add-only fields), but
  **no `source` field** — the only source is the `origin` read (the route
  takes no hint by design: its caller is the index trying to *learn* the
  slug), so `source` would be a constant. YAGNI.
- **Error contract (wire-distinguishable, D2):**
  - unknown id → `404` (`resolve_repo_path`'s existing `NotFound`),
  - `SlugReason::NoGithubRemote` → `422 {"error":{"code":"no_github_remote", …}}`,
  - `SlugReason::HostUnreachable` → `502 {"error":{"code":"host_unreachable", …}}`
    (a transport failure is a gateway problem, not a semantic one — and qa.sh's
    "host-down shows the unreachable-flavored error" gets a real status to key on).
- **Auth:** nothing to do — `require_token` wraps every `/api/*` at the
  `lib.rs:349` merge and the route is not in `auth.rs::is_public` (`:74-97`).
  **No `is_public` change** (invariant).

### 1.3 D4 flag shape = `grounding: { repo: bool, wiki: bool }` on the draft response

`chat::draft_issue_body` (`chat.rs:1871-1903`) already computes both facts —
`repo_context = gather_repo_context(workdir)` (`:1884`; `None` for a non-local
dir, `:238`) and `wiki = retrieve_wiki_for_query(…)` (`:1888`). The fn's
return widens from `String` to a small struct carrying the two booleans; the
route (`github.rs:311-335`) serializes them as an **always-present, add-only**
`grounding` object next to `body`. Old clients ignore it; no serde aliases,
no renamed fields (invariant).

### 1.4 Renderer cache covers the new F2 arm — **yes** (PM's expectation confirmed)

`slugByRepoId` (`repo-slug-index.ts:33-41`) keys by
`(runtime-scope, repo.id)`. The new arm is chosen by `repo.connectionId`,
which is **immutable** post-015 (`update()` refuses `connectionId` edits) —
so a cache key can never silently change arms. Negative caching (host down →
`null` until repos change or `clearRepoSlugCacheEntry`) matches today's
native-failure semantics exactly, which is what AC 7 pins.

### 1.5 Deviations from the spec letter (LOUD — none change product scope)

1. **No `repoId` on the draft-body request** (spec AC 8 lists the draft leg
   in the threading). Grounded reality: `POST /api/github/issues/draft-body`
   never resolves a slug and never touches a host — its only workdir use is
   the *local* `gather_repo_context`/wiki read that D4 flags. A `repoId` there
   would be a dead wire field the server ignores. The draft leg instead
   threads the **learned slug** (already does, `use-tracker-intake.ts:192`)
   and gains the **grounding flag on the response** — AC 8's intent (SSH
   filing works, AC 9's honesty) is fully met. PM ping optional; no scope change.
2. **`create_issue` / `list_labels` / `provision` 422 messages gain the
   HostUnreachable ≠ NoGithubRemote distinction** (today they emit the generic
   "no GitHub repo resolved for this project", `github.rs:251/:398`,
   `provision.rs:58`). D1 says "byte-for-byte" — this changes error-path
   *message* bytes while keeping the `code: "no_github_repo"` envelope
   byte-identical. Verified safe: no UI code string-matches these messages
   (grep clean; `GithubProjectsBindingError` branches on `code` only,
   `github-projects-client.ts:52-60`; `extractServerErrorMessage` just
   displays). This is the task mandate "keep the distinction on every path"
   and the exact precedent of the `github_projects.rs:74-84` fix.
3. **The grounding note renders only when `grounding.repo === false`.**
   A local repo with no wiki sidecar has `wiki: false` on every draft today —
   noting that would spam the common case with noise 015 shipped silently.
   The wiki word appears *inside* the note when both are false. AC 9's target
   (the SSH degradation) always trips `repo: false`.
4. **`IntegrationsPane`'s local-only filter drops** (`:238`
   `repos.filter((r) => !r.connectionId)`) — its own comment cites exactly the
   limitation F1 removes ("Bindings resolve through the server's LOCAL host…
   remote (SSH) repos are out of scope here"). Optional-but-recommended; one
   line + comment rewrite.
5. **The wizard's `trackerWorkdir` gate is NOT relaxed**
   (`CreateWorkspaceWizard.tsx:392` excludes `connectionId` repos from the
   whole TrackerSection). Relaxing it would light up the wizard's tracker
   section for SSH repos — correct post-F1, but it's a wider product surface
   shared with in-flight specs 012/013/016–018. Threaded but gated; named
   follow-up.

---

## 2. F1 — host-aware-slug-family (server)

### 2.1 New shared helpers (`routes/util.rs`)

```rust
/// Host a `{workdir, slug?, repoId?}` tracker request reads git state on
/// (spec 020 D1): an explicit repoId wins — unknown id is a 4xx, NEVER a
/// silent local fallback; absent repoId is the local host, i.e. today's
/// behavior for every existing caller.
pub(crate) async fn resolve_tracker_host(
    state: &AppState,
    repo_id: Option<&str>,
) -> Result<Host, ApiError> {
    match repo_id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => super::repos::load_host_for_repo(state, id).await, // 404 unknown id / 400 deleted host
        None => state
            .store
            .get_host(LOCAL_HOST_ID)
            .await?
            .ok_or_else(|| ApiError::Internal("local host missing".into())),
    }
}

/// The ONE `{workdir, slug?, repoId?}` → slug resolver with the typed
/// `no_github_repo` 422 (spec 020: unifies routes::github_projects's and
/// routes::provision's admitted copies). Order: workdir shape-check →
/// expand → host (repoId-aware) → resolve_github_slug (hint short-circuits
/// with zero git I/O inside it).
pub(crate) async fn resolve_tracker_slug(
    state: &AppState,
    repo_id: Option<&str>,
    workdir: &str,
    slug_hint: Option<&str>,
) -> Result<String, ApiError>
```

`resolve_tracker_slug` body = today's `github_projects.rs::resolve_slug`
(`:45-85`) verbatim, with `resolve_tracker_host(state, repo_id)` replacing the
`get_host(LOCAL_HOST_ID)` block. The `SlugReason` → message mapping moves into
a **pure** fn so it's unit-testable:

```rust
/// Pure: the typed 422 body for a slug miss. Message distinguishes
/// HostUnreachable from NoGithubRemote (the github_projects precedent);
/// the `no_github_repo` code the UI branches on is unchanged.
pub(crate) fn no_github_repo_envelope(reason: SlugReason) -> (StatusCode, serde_json::Value)
```

(`SlugReason` needs no changes — it is already `pub(crate)`, `Copy`, at
`board_goals.rs:224-232`. D5 honored: `resolve_github_slug`, `SlugReason`,
`is_valid_slug`, the sink's explicit-slug arm, hint semantics — all untouched.)

**Contract decisions inside the helper:**

- **Unknown repoId beats a valid hint.** Order is host-then-resolve, so a
  garbage `repoId` 4xxes even when the hint would have short-circuited: an
  identity error is a client bug that must be loud, and honoring the hint
  would mask it. AC 2's "zero git I/O" still holds on the hint path (the
  repoId branch reads the JSON registry + the host row — no git, no SSH).
- **The client's `workdir` is used as-sent, on the repo's host** — the spec's
  literal instruction (only the *host* swaps). A mismatched pair
  (remote repoId + local-only workdir) fails as an honest
  `NoGithubRemote`/`HostUnreachable` 422, never a silent wrong answer.
- **Tilde note:** `expand_workdir` expands `~` against the *daemon's* HOME —
  wrong for a remote path, but remote registry paths are absolute by
  construction (`reposAddRemote` sends `remotePath`) and `git_in_dir`'s SSH
  arm quotes the cwd (no remote expansion) so this is a pre-existing
  edge shared with every `base_ref_*` route. Documented, not fixed here.
- **Known edge (pre-existing, keep):** a repo with `connectionId` but no
  `host_id` (added pre-host_id) resolves via `load_host_for_repo`'s
  `unwrap_or(LOCAL_HOST_ID)` (`repos.rs:411`) → local read → 422. Spec 015
  pinned that default as sacred; unchanged.

### 2.2 DTO widenings (add-only, no aliases — serde-alias hazard honored)

| DTO | File:line | Add | Wire key |
|---|---|---|---|
| `BindingQuery` (GET+DELETE binding) | `github_projects.rs:295-299` | `#[serde(default, rename = "repoId")] pub repo_id: Option<String>` | `repoId` query param |
| `PutBindingRequest` | `github_projects.rs:322-345` (camelCase) | `#[serde(default)] repo_id: Option<String>` | `repoId` body field |
| `IssueQuery` | `github.rs:41-52` | `#[serde(default, rename = "repoId")] pub repo_id: Option<String>` | `repoId` query param |
| `CreateIssueBody` | `github.rs:172-188` (camelCase) | `#[serde(default)] repo_id: Option<String>` | `repoId` body field |
| `LabelsQuery` | `github.rs:337-343` | `#[serde(default, rename = "repoId")] pub repo_id: Option<String>` | `repoId` query param |
| `ProvisionRequest` | `provision.rs:~263-282` (camelCase) | `#[serde(default)] repo_id: Option<String>` | `repoId` body field |

(Query structs have no `rename_all`; per-field `rename` is the surgical form —
do NOT add `rename_all` to them. `DraftBodyRequest` gets **no** repoId —
deviation §1.5.1.)

### 2.3 The five site swaps

1. **`github_projects.rs::resolve_slug` (`:45-85`) — deleted.** The three
   binding handlers call
   `super::util::resolve_tracker_slug(&state, q.repo_id.as_deref() /* or body */, &q.workdir, q.slug.as_deref())`
   directly (`get_binding:314`, `put_binding:368`, `delete_binding:401`).
   Behavior for repoId-absent requests is byte-identical (the helper body IS
   this fn's body).
2. **`provision.rs::resolve_slug` (`:39-61`) — deleted**; swap at `:298` to
   the util call with `req.repo_id.as_deref()`. Two behavior deltas, both
   deliberate: (a) the distinguished 422 message (§1.5.2); (b) the slug read
   now tilde-expands its workdir like every sibling (no-op for the absolute
   paths the handler's own `:291` expand+`is_dir` gate already required).
   The `workdir.is_dir()` local-only gate at `:292` **stays** (spec non-goal:
   no remote provisioning) — an SSH `repoId` here dies at the gate first,
   which is correct.
3. **`github.rs::create_issue` (`:232-254`)** — replace the host-load block
   (`:232-236`) + expand + `resolve_github_slug` match (`:240-254`) with one
   `let slug = super::util::resolve_tracker_slug(&state, body.repo_id.as_deref(), &body.workdir, body.slug.as_deref()).await?;`
   (the fn does trim/expand internally). Everything downstream —
   `NewFeature`, `SinkCtx { slug: Some(&slug) … }`, `map_sink_error`,
   `authenticated_github_login(&host)`… **wait**: `authenticated_github_login`
   (`:279`) and the sink run the **local** `gh` today. Keep them local?
   **No** — thread the resolved `host` so `gh` runs where the repo's auth
   lives: fetch/labels already run `gh_in_dir(&host, …)` with the resolved
   host, and `gh_in_dir` is host-aware by design (`git_fs.rs:90-…`, built for
   "the remote GitHub-issue path, spec 018"). But `SinkCtx` has no host
   field and the sink's explicit-slug arm runs the local `gh` from `$HOME`
   **by old-019 contract** (`board_goals.rs:320-324`: "Filing always runs the
   local gh"; D5 forbids sink changes). **Ruling:** `create_issue` keeps
   `TaskSink::Github` local (filing is slug-only and works from any machine
   with `gh` auth — the PM-verified AC 8 mechanics), and
   `authenticated_github_login` keeps the same host the sink used = keep it on
   the **local** host explicitly (`resolve_tracker_host(&state, None)`), so
   author attribution matches the credential that filed. Net: in
   `create_issue` only the *slug resolution* becomes host-aware — exactly the
   spec's "slug-half" language. Add a comment citing this paragraph.
4. **`github.rs::list_labels` (`:381-401`)** — same one-call swap as
   create_issue for the slug; **but** `gh label list` (`:405`) reads the
   repo's labels — slug-addressed, so keep it on the **local** host too
   (labels are a GitHub-API read; the local `gh` reaches any slug it's authed
   for — this is exactly today's behavior for every reachable case, and
   matches create's local-file contract). Only `:381-390` collapses into the
   util call.
5. **`github.rs::fetch_github_issue` (`:85-157`)** — signature gains
   `repo_id: Option<&str>` (param, not DTO — it's a `pub(crate)` helper);
   `:102-106` becomes `let host = super::util::resolve_tracker_host(state, repo_id).await?;`.
   Its **error contract stays** the plain 400 with `{reason:?}` (`:115-119`)
   — the desktop's "Use" path treats any error as fall-back-to-URL and must
   not start receiving a 422 envelope it doesn't parse. As with 3/4, the
   `gh issue view` at `:125` stays on the resolved host? **No — keep the
   resolved host here**: fetch already passes `&host` to `gh_in_dir` today
   (`:125-127`), so the swap makes fetch run `gh` on the repo's host. That is
   a real behavior choice: for an SSH repoId, `gh` must be installed/authed on
   that host. Acceptable and honest (the remote host owns the repo; spec 018
   built `gh_in_dir` for exactly this), and repoId-absent callers are
   byte-identical. Callers: `get_issue` (`:163`) passes
   `q.repo_id.as_deref()`; the two harness callers (`harness.rs:324`, `:572`)
   pass `None` — their workdirs are `is_dir`-gated local worktrees, behavior
   pinned unchanged.

> **Consistency note for the developer:** after the swaps, `create_issue` and
> `list_labels` no longer bind a `host` variable from the resolver — create
> re-derives the local host for `authenticated_github_login` (one extra store
> read per create; click-frequency, fine), and labels needs the local host for
> `gh_in_dir` — derive it the same way. Keep the "why local" comments; this is
> the subtle part of F1.

### 2.4 F1 unit tests (write FIRST where marked ▲)

Rust, `cargo test -p agentum-server --lib`:

1. ▲ `util::tests::resolve_tracker_host_absent_repo_id_is_local` — fresh
   AppState (clone the ~25-line `fresh_state()` from `board_goals.rs` tests
   `:22` into `util.rs::tests`; the store seeds the local host on open) →
   host id == `LOCAL_HOST_ID`.
2. ▲ `util::tests::resolve_tracker_host_unknown_repo_id_is_4xx` —
   `Some("020-no-such-repo-<uuid>")` → `ApiError::NotFound`. (Env-tolerant:
   a random id misses whatever `~/.agentum/repos.json` holds; no env mutation.)
3. ▲ `util::tests::resolve_tracker_slug_hint_short_circuits_with_unreadable_workdir`
   — repoId `None`, workdir `/path/does/not/exist`, hint `acme/widgets` →
   `Ok("acme/widgets")` (AC 2; zero git I/O proven by the unreadable path;
   the resolver-level twin already exists at `board_goals.rs` tests `:484` —
   keep both, this one pins the *route family* path incl. host-load ordering).
4. ▲ `util::tests::resolve_tracker_slug_unknown_repo_id_beats_valid_hint` —
   unknown repoId + valid hint → 4xx (the §2.1 ordering contract).
5. ▲ `util::tests::no_github_repo_envelope_distinguishes_reasons` — pure:
   both reasons → `UNPROCESSABLE_ENTITY`, code `no_github_repo`, messages
   differ (`origin`-flavored vs `could not read`-flavored).
6. ▲ `repos::tests::host_id_of_*` — extract the pure core
   `fn host_id_of(repos: &[Repo], repo_id: &str) -> Result<Option<Uuid>, ApiError>`
   from `resolve_repo_host_id` (`:397-403`, now a thin `read_repos()?` +
   `host_id_of` wrapper) and pin: known-local → `Ok(None)`, known-remote →
   `Ok(Some(uuid))`, unknown → `Err(NotFound)` (repoId→host threading, pure,
   no HOME override — the 015 house rule).
7. DTO serde pins (extend the existing wire-shape tests in each module —
   `github_projects.rs::wire_shapes_are_camel_case`,
   `github.rs::create_issue_rejects_blank_title` + a labels/issue-query twin,
   provision's request tests): `repoId` present deserializes; absent → `None`
   (the local regression pin at the wire).
8. Regression pins = the existing suites stay green untouched
   (`create_goal_not_a_github_repo_returns_typed_error`,
   `resolve_github_slug_*`, binding validation tests…).

Gates: `cargo test -p agentum-server --lib` · `cargo fmt` ·
`cargo clippy -p agentum-server -- -D warnings`.

---

## 3. F2 — slug-index-ssh (route + renderer)

### 3.1 Route (`routes/repos.rs`)

```rust
// router(): .route("/api/repos/{id}/slug", get(repo_slug))

#[derive(Debug, Serialize)]
struct RepoSlugResponse { slug: String }

/// Pure: SlugReason → (status, code, message). NoGithubRemote is semantic
/// (422); HostUnreachable is transport (502) — the wire must never let an
/// SSH failure masquerade as "no origin" (spec 020 invariant).
fn slug_reason_wire(reason: SlugReason) -> (StatusCode, &'static str, &'static str)

/// `GET /api/repos/{id}/slug` — the repo's GitHub `owner/repo`, resolved by
/// reading `origin` ON THE REPO'S HOST (spec 020 F2/D2). The renderer's slug
/// index uses this for SSH repos; no hint param — this route IS how a client
/// learns the slug. Fail-closed: any error excludes the repo client-side.
async fn repo_slug(State(state): State<AppState>, Path(repo_id): Path<String>)
    -> Result<Json<RepoSlugResponse>, ApiError>
{
    let path = resolve_repo_path(&repo_id)?;              // 404 unknown id
    let host = load_host_for_repo(&state, &repo_id).await?;
    let slug = super::board_goals::resolve_github_slug(&host, &path, None)
        .await
        .map_err(|reason| { let (s, code, msg) = slug_reason_wire(reason);
            ApiError::Custom(s, json!({ "error": { "code": code, "message": msg } })) })?;
    Ok(Json(RepoSlugResponse { slug }))
}
```

Uses the **registry path**, not a client workdir — the server owns id→path
consistency (D1 rationale), same as every `base_ref_*` sibling. Workdir/hint
params deliberately absent. Auth: covered by the top-level `require_token`
(§1.2). Slug case: returned as resolved; the client lowercases (parity with
the native arm's `:82`).

### 3.2 Client (`ui/src/runtime/server-repo-client.ts` — the existing `/api/repos/*` client)

```ts
/** `GET /api/repos/{id}/slug` — owner/repo resolved on the repo's OWN host
 *  (spec 020 F2). Throws on any non-2xx (unknown repo / no GitHub origin /
 *  host unreachable) — callers fail closed. */
export function getServerRepoSlug(repoId: string): Promise<{ slug: string }> {
  return getJson<{ slug: string }>(`/api/repos/${encodeURIComponent(repoId)}/slug`)
}
```

### 3.3 Renderer branch (`lib/repo-slug-index.ts` + new pure arm module)

New **pure** module `ui/src/lib/repo-slug-arm.ts` (import-free except types,
so vitest never drags in `@/tauri`/store):

```ts
export type SlugResolutionArm = 'environment-rpc' | 'server' | 'native'
/** Which resolver a repo's slug uses: an active runtime environment keeps the
 *  existing RPC arm (spec non-goal: untouched); else an SSH repo
 *  (connectionId) resolves via the server's host-aware route (spec 020 F2);
 *  else the local native `gh_repo_slug`. */
export function slugResolutionArm(
  environmentTarget: boolean,
  connectionId: string | null | undefined
): SlugResolutionArm
```

`resolveRepoSlug` (`repo-slug-index.ts:59-91`) switches on it inside the
existing `try` (so every failure keeps hitting the `:85-90` catch → cache
`null` → excluded — AC 7 fail-closed by construction):

- `'environment-rpc'` → existing `callRuntimeRpc('github.repoSlug', …)` arm,
  byte-identical (**untouched**, spec non-goal);
- `'server'` → `const { slug } = await getServerRepoSlug(repo.id); const s = slug.toLowerCase(); cache; return s;`
- `'native'` → existing `api.gh.repoSlug({repoPath, repoId})` arm, untouched
  (`commands/gh.rs:305-321` stays local-only by design).

Cache: unchanged (§1.4). Module doc comment (`:2-16`) updated to describe the
three arms.

### 3.4 Start-work e2e pins — already in the tree, enumerate don't rewrite

`classifyStartWorkRepoMatches` and `ProjectViewWrapper`'s wiring are
**UNTOUCHED** (015 shipped them; `ProjectViewWrapper.tsx:539/:590` feed it
from `lookupSlug`). The pins the spec asks for already exist in
`start-work-repo-match.test.ts`: sole remote → `direct` (`:26`, "VPS-only repo
starts on the VPS"), local+remote → `choose` seeded local (`:32`). F2's new
behavior (an SSH-only repo now *enters* the index at all) is pinned by the arm
test + qa.sh; the classifier tests run in verify.sh as the e2e half of the pin.
Direct-launch's issue-body fetch is already SSH-safe: it always passes the
board row's slug hint (`ProjectViewWrapper.tsx:505-509`) → zero-I/O path.

### 3.5 F2 tests

- ▲ Rust: `slug_reason_wire` pure test (statuses 422/502, codes, distinct
  messages); `RepoSlugResponse` serde shape (`{"slug":"o/r"}`).
- ▲ Vitest: `src/lib/repo-slug-arm.test.ts` — environment target wins over
  connectionId (RPC untouched even for SSH repos); connectionId → `server`;
  neither → `native`; null vs undefined connectionId both native.
- Existing `start-work-repo-match.test.ts` re-run (regression pin).
- Gates: Rust trio + `bunx vitest run src/lib/repo-slug-arm.test.ts
  src/components/github-project/start-work-repo-match.test.ts` +
  `bun run build` (ui). Never full vitest / bare tsc (pre-broken).

---

## 4. F3 — intake-ssh-honest (UI + one server flag)

### 4.1 Server flag (D4)

- `chat.rs`: `draft_issue_body` (`:1871-1903`) returns
  `pub(crate) struct DraftedIssue { pub body: String, pub grounded_repo: bool, pub grounded_wiki: bool }`
  — set from `repo_context.is_some()` / `wiki.is_some()` captured before the
  values move into `draft_body_instructions`. Sole caller: `github.rs:325`.
- `github.rs`: `DraftBodyResponse` becomes
  `{ body: String, grounding: DraftGroundingDto }` with
  `#[derive(Serialize)] struct DraftGroundingDto { repo: bool, wiki: bool }`
  (single-word fields — no rename needed). **Add-only**: old readers of
  `{body}` (the wizard's `handleGenerateIssueBody`) are unaffected.
- ▲ Rust test: response serde shape
  (`{"body":"…","grounding":{"repo":false,"wiki":false}}`). The
  `None`-for-non-local-dir fact is already pinned
  (`chat.rs` tests `:2760`, `wiki` at `:3019`).

### 4.2 Runtime-client widenings (add-only inputs; two exported pure builders for the payload pins)

`runtime/github-projects-client.ts`:

- Extract + export pure
  `bindingQuery(input: { workdir: string; slug?: string; repoId?: string }): URLSearchParams`
  — used by `getProjectBinding` (`:127-131`) and `deleteProjectBinding`
  (`:293-297`); appends `repoId` only when present.
- `getProjectBinding` / `deleteProjectBinding` inputs gain `repoId?: string`.
- `putProjectBinding` input gains `repoId?: string`; body spread
  `...(input.repoId ? { repoId: input.repoId } : {})` (the `:176` slug
  pattern).

`runtime/github-issue-client.ts`:

- Extract + export pure
  `createIssuePayload(input): Record<string, unknown>` from the `:132-139`
  body literal; add `repoId?: string` to `createGithubIssue`'s input, spread
  into the payload the same conditional way.
- `fetchGithubIssueBody` input gains `repoId?: string` → query param
  (add-only; its two existing callers keep passing slug and need no change).
- `DraftedGithubIssueBody` widens to
  `{ body: string; grounding?: { repo: boolean; wiki: boolean } }`
  (optional on the client to tolerate an older server skew; the embedded
  server ships lockstep so it's effectively always present).
- `fetchGithubRepoLabels` NOT widened (spec cites `:26-58/:115-151/:189-221`
  only; the server route accepts `repoId` for a future need — YAGNI on the
  client).

### 4.3 `ProjectBindingEditor` + the three feeders

- Props (`ProjectBindingEditor.tsx:59-67`) gain `repoId?: string`; thread into
  `getProjectBinding` (`:94`), both `putProjectBinding` calls (`:229`, `:264`),
  `deleteProjectBinding` (`:292`); add `repoId` to the four dep arrays and to
  the `:85-109` effect's deps. The `:83-84` "keyed by slug" comment stands.
- **ProjectHubPage** (`:274-277`): `repoId={repo.id}` (the hub's `repo.path ?`
  gate at `:273` already passes SSH repos — remote paths are truthy — so the
  hub Tracker tab lights up for SSH repos with no further change; the stale
  `:279-283` else-comment applies only to path-less repos, leave it).
- **IntegrationsPane** (`:266`): `repoId={selected.id}`; drop the
  `localRepos` filter (`:238`) → iterate `repos`, rewrite the `:236-237`
  comment (the limitation it documents is what 020 removes). §1.5.4.
- **CreateWorkspaceWizard**: `TrackerSection` (`:1497`) gains
  `repoId?: string`; the mount (`:1383-1391`) passes a new `trackerRepoId`
  computed at `:392` with the **same** gate as `trackerWorkdir`
  (`selectedRepo && !selectedRepo.connectionId && selectedRepoIsGit ? selectedRepo.id : undefined`);
  the editor mount (`:1608-1614`) threads it. Gate deliberately unrelaxed
  (§1.5.5) — update the `:386-391` comment to say the remaining local-only
  gate is a product choice, no longer a technical one.

### 4.4 Intake hook + panel (`components/project-hub/`)

`use-tracker-intake.ts`:

- Binding read (`:104`): `getProjectBinding({ workdir, repoId: repo.id })` —
  **this is the leg that un-dead-ends SSH repos** (it's how the slug at `:108`
  gets learned at all; PM finding 1). Effect deps gain `repo.id`.
- File (`:212-217`): `createGithubIssue({ …, repoId: repo.id })` — the
  no-hint robustness path when the binding read failed earlier (host was down)
  and `slug` is still null.
- Draft (`:189-193`): payload unchanged (slug-first, §1.5.1); capture the
  flag — new state `grounding: DraftGrounding | null`, set
  `setGrounding(res.grounding ?? null)` on success, reset to `null` at draft
  start (beside the `:187` `setFiled(null)`).
- New derived output on `TrackerIntake`: `groundingNote: string | null` =
  `deriveDraftGroundingNote(grounding, hostLabel)` where
  `hostLabel = useAppStore((s) => repo.connectionId ? (s.sshTargetLabels.get(repo.connectionId) ?? 'a remote host') : null)`
  (the `WorktreeCard.tsx:206` selector precedent).

Pure model (`components/new-workspace/create-issue-intent-model.ts`,
**add-only** — 015's exports untouched):

```ts
export type DraftGrounding = { repo: boolean; wiki: boolean }

/** The honest grounding note for a drafted body (spec 020 AC 9, D4): null =
 *  no note. Notes ONLY when repo grounding was skipped — a wiki-only miss is
 *  the normal local no-wiki case and stays silent (015 behavior). `grounding`
 *  null = a pre-020 server response: silent, exactly today. Never inferred
 *  from connectionId — the flag is the server's word; the host label only
 *  explains WHY. */
export function deriveDraftGroundingNote(
  grounding: DraftGrounding | null,
  hostLabel: string | null
): string | null
```

Exact returns (tests pin these):
- `grounding == null` or `grounding.repo` → `null`;
- `!repo && hostLabel` →
  `` `Drafted without repo${wiki ? '' : ' or wiki'} grounding — the repo's files live on ${hostLabel}.` ``
- `!repo && !hostLabel` →
  `` `Drafted without repo${wiki ? '' : ' or wiki'} grounding — the project folder wasn't readable here.` ``

`TrackerIntakePanel.tsx`: render the note inside the `hasDraft` block after
the Description textarea (`:102`), muted styling
(`text-[11px] text-muted-foreground`), **not** the destructive error style —
it's honesty, not failure. Everything else (inline errors `:140-142`,
provider-confirmed `filed` `:146-190`, the D3/remote-repo gate notes) is
untouched — 015's AC 12 invariants preserved by not touching them.

### 4.5 F3 tests (write FIRST where ▲)

- ▲ `create-issue-intent-model.test.ts` (add-only describe):
  `deriveDraftGroundingNote` matrix — null flag → null (old-server silence);
  grounded → null; repo-miss + label (wiki true/false variants — "or wiki"
  wording); repo-miss without label; wiki-only miss → null.
- ▲ `runtime/github-projects-client.test.ts` (new file, pure-export pattern
  like `github-issue-client.test.ts`): `bindingQuery` with/without
  slug/repoId (param present iff supplied; order-insensitive assertions).
- ▲ extend `runtime/github-issue-client.test.ts`: `createIssuePayload`
  includes `repoId`/`slug`/`labels` only when supplied; absent → absent keys
  (the byte-identical pre-020 wire pin).
- Rust: §4.1's serde test rides F3's server commit.
- Gates: `cargo test -p agentum-server --lib` (the flag) · fmt · clippy ·
  `bunx vitest run src/components/new-workspace/create-issue-intent-model.test.ts
  src/runtime/github-issue-client.test.ts src/runtime/github-projects-client.test.ts` ·
  `bun run build`.

---

## 5. Risks & coordination

- **Specs 016–018 (sdd-tracker-status worktree) touch tracker surfaces.**
  This blueprint's grep found none of their code here, but they may land
  first in `develop`: the merge hotspots are `github.rs` (route family),
  `task_sink.rs` (untouched here — keep it that way), and the Tracker-tab UI.
  Developer: re-grep every `:line` anchor at build time (see handoff drift
  table); if a `repoId` field or slug route has appeared, STOP and reconcile
  before writing a duplicate.
- **015 ships first** — this branch already contains it (classifier, intake
  panel, `findRepoByPathPreferLocal` all verified present). Do not rebase
  onto bare develop.
- **The §2.3 host-choice subtleties** (sink stays local, login stays local,
  fetch/labels `gh` host choices) are the likeliest place for a silent
  behavior regression — the "why local" comments are load-bearing; clippy
  won't catch a wrong host.
- **Error-message deltas** (§1.5.2) are contract-safe (code-keyed UI) but
  will show up in any snapshot-style assertions — none exist today; don't add
  message-matching tests except the two envelope tests specified.
- **No new auth surface**: one new route, behind the existing top-level
  `require_token`; `is_public` untouched (pinned by reading `auth.rs:74-97` —
  no test needed, the existing `goals_route_requires_auth_verified_at_router_merge`
  comment pattern applies).
- **One launch path / no polling / YOLO invariants**: untouched — 020 spawns
  nothing and streams nothing.
- **wiki.rs:410 also pins `get_host(LOCAL_HOST_ID)`** — deliberately NOT in
  scope (wiki is local-by-design, spec non-goal). Don't "fix" it in passing.

## 6. Gate summary (per increment)

| Gate | F1 | F2 | F3 |
|---|---|---|---|
| `cargo test -p agentum-server --lib` | ✅ | ✅ | ✅ (flag serde) |
| `cargo fmt` + `cargo clippy -p agentum-server -- -D warnings` | ✅ | ✅ | ✅ |
| `bunx vitest run <targeted>` | — | `lib/repo-slug-arm.test.ts` + `github-project/start-work-repo-match.test.ts` | `new-workspace/create-issue-intent-model.test.ts` + `runtime/github-issue-client.test.ts` + `runtime/github-projects-client.test.ts` |
| `bun run build` (in `crates/agentum-desktop/ui`) | — (no UI change) | ✅ | ✅ |

ui uses **bun**; never gate on full vitest (~139 pre-broken) or bare tsc.
