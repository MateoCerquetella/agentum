# Spec 020 — Reviewer Sign-off

- **Spec:** 020-ssh-host-tracker-plumbing
- **Date:** 2026-07-13
- **Reviewer:** independent (did not write the code; did not re-run the
  tester's gates — built on `verification.md`'s PASS-WITH-DEFERRALS, 0 defects)
- **Commits reviewed:** F1 `09726c46`, F2 `e8fb31a8`, F3 `820712d9`
  (base = spec 015's `3ec6f028`; range also carries 015's two docs-only
  commits, excluded from code claims). Every code hunk in
  `git diff 3ec6f028..820712d9` was read.

## Verdict: **SIGN-OFF**

All 10 focus items PASS. No blockers. Two should-fixes (both follow-up
tickets, neither in this spec's ACs) and a handful of leave-as-is nits.
Live-SSH legs stay deferred to qa.sh/staging per the spec's own gate split —
release checklist at the bottom.

---

## Per-focus rulings

### 1. Cross-repo repoId/workdir mismatch — **PASS (contract ACCEPTED)**

The §2.1 ruling ("the client's `workdir` is used as-sent, on the repo's
host — only the *host* swaps") is sound. Three independent guards:

- **Every real caller passes a coherent pair from one `Repo` object.**
  Intake hook: `const workdir = repo.path` … `getProjectBinding({ workdir,
  repoId: repo.id })` (`use-tracker-intake.ts:102,111`). ProjectHubPage:
  `workdir={repo.path} repoId={repo.id}`; IntegrationsPane:
  `workdir={selected.path} repoId={selected.id}`; the wizard computes
  `trackerWorkdir`/`trackerRepoId` from the same `selectedRepo` with the
  byte-same ternary gate.
- **The pair cannot drift after mount**: `PATCH /api/repos/{id}` refuses
  `id`/`path`/`addedAt`/`connectionId` edits (`repos.rs:244`, pinned by
  `update_refuses_connection_id_edit` incl. the null-edit arm), and a re-add
  under a different connection is a NEW id (`register_repo` keys identity on
  `(path, connection_id)`, `repos.rs:152`).
- **A deliberately incoherent pair fails loud or answers literally.** Remote
  repoId + local-only path → honest 422 (`NoGithubRemote`/`HostUnreachable`),
  never silent; a hand-crafted pair naming repo B's path with repo A's id
  resolves whatever lives at that path on that host — exactly what the wire
  asked for, and no wider than the pre-020 trust model where any client could
  already send any workdir. With a valid hint, workdir is ignored entirely
  (zero I/O); with a garbage repoId, 404 beats the hint (pinned by
  `resolve_tracker_slug_unknown_repo_id_beats_valid_hint`).
- The F2 route side-steps the class: `resolve_repo_path(&repo_id)` — registry
  path, no workdir param ("the server owns id→path consistency").

Future-caller check: the only live wires with no caller yet are
`fetchGithubIssueBody`'s `repoId?` (both existing callers pass slug only;
`ProjectViewWrapper.tsx:502` even gates its fetch to
`connectionId == null`) and `ProvisionRequest.repo_id` (no UI sender; an SSH
repoId dies at the handler's `is_dir` gate first, per the module doc). No
wrong-repo slug or wrong-host git read with real consequences exists.

### 2. File-leg unconditional `repoId` — **PASS (D1-correct, ruled ACCEPT)**

`createGithubIssue({ …, repoId: repo.id })` is unconditional in the intake
hook. The stale-registry case (repo removed mid-session) now 404s
`repo not found: <id>` where pre-020 the workdir would have resolved — that is
D1's letter ("an identity error is a client bug that must be loud"), the error
lands in the AC 10 inline path (`setError`, non-fatal), the window is a
seconds-long race (the hook's `repo` comes from `useAppStore.repos`, which
updates on removal), and retry self-heals. The only other
`createGithubIssue` caller (`hooks/useComposerState.ts:1544`) is untouched and
passes no repoId — no legacy caller regressed. Conditioning on `repo.id`
truthiness would not help (a store repo always has an id); no change wanted.

### 3. `create_issue` failure-ordering move — **PASS (no observable contract change)**

Pre-020: missing-local-host 500'd before slug resolution. Post-020, absent
repoId: the resolver's `None` arm loads the same
`get_host(LOCAL_HOST_ID)` + identical `Internal("local host missing")` —
merely after the workdir expand. Present repoId: the local host (for
attribution) derives at `github.rs:266`, after slug resolution but before
`create_feature` at `:273` — the comment pins the invariant: "Derived before
the create so a missing local host still fails before any `gh` call, as it
always has." The only theoretically observable delta — bad workdir + missing
local host now yields 422 instead of 500 — requires a corrupted store (the
local host row is seeded on `Store::open`; the util test's comment says so).
No real client can see a difference.

### 4. D1 contract integrity — **PASS**

Verified per-route by reading the no-repoId paths:

- **Bindings (get/put/delete)**: `resolve_tracker_slug` body = the deleted
  `github_projects.rs::resolve_slug` verbatim (trim → expand → local host →
  resolve → the SAME two 422 message strings, now in
  `no_github_repo_envelope`). Byte-identical.
- **create_issue / list_labels**: same flow; the only deltas are the two
  architecture-sanctioned ones (§1.5.2 message distinction — `code:
  "no_github_repo"` unchanged, no UI string-matching, grep-verified by the
  tester; and the trivial expand/host-load order swap).
- **fetch_github_issue**: `resolve_tracker_host(state, None)` ≡ the deleted
  `get_host(LOCAL_HOST_ID)` block incl. the `Internal` fallback; the plain-400
  `could not resolve a GitHub repo: {reason:?}` contract kept (`github.rs:122`)
  — the desktop's "Use" fallback never sees a new envelope. Harness's two
  callers pass `None` with byte-identical-pin comments.
- **provision**: shared resolver adds a tilde-expand the old copy lacked —
  no-op behind the handler's own expand+`is_dir` gate, which still runs first.
- **Unknown repoId**: `Some(id)` arm → `load_host_for_repo` →
  `host_id_of` → `Err(NotFound)` — pinned three ways
  (`resolve_tracker_host_unknown_repo_id_is_4xx`,
  `host_id_of_unknown_id_is_not_found`, the ordering test in focus 1). The
  ONLY `Some(id)`→local path is `load_host_for_repo`'s
  `unwrap_or(LOCAL_HOST_ID)` for a **known** repo lacking `host_id`
  (`repos.rs:422`) — the pre-existing, 015-pinned legacy edge, documented in
  architecture §2.1; not an unknown-id fallback. **No silent-local fallback
  anywhere.**

### 5. 502/422/404 wire contract (F2 route) — **PASS**

`slug_reason_wire` (`repos.rs`): `NoGithubRemote` → `422 no_github_remote`,
`HostUnreachable` → `502 host_unreachable`, messages are `&'static str`
literals — nothing interpolated. Leakage is structurally impossible on the
slug leg: `SlugReason` is a payload-free two-variant enum
(`board_goals.rs:225-232`), so SSH stderr/hostname/token cannot ride it. The
handler's other errors carry only client-supplied/registry ids:
`resolve_repo_path` 404 `repo not found: {repo_id}`, `load_host_for_repo`
400 `repo host is missing: {host_id}` (a UUID). F1's 422 messages in
`no_github_repo_envelope` are likewise static. The 422/502 split is pinned
pure (`slug_reason_wire_distinguishes_transport_from_semantic`), and the F1
family's message split by `no_github_repo_envelope_distinguishes_reasons`.

### 6. Fail-closed renderer — **PASS**

- **Arm ordering**: `slugResolutionArm` is `environmentTarget ?
  'environment-rpc' : (connectionId ? 'server' : 'native')` — env-RPC wins
  over server wins over native; all four combinations pinned in
  `repo-slug-arm.test.ts` (incl. env-RPC winning even for SSH repos, and
  null vs undefined connectionId both native).
- **Exclusion on failure**: the server arm sits inside the pre-existing `try`
  (`repo-slug-index.ts:74-85`); `getServerRepoSlug` throws on any non-2xx →
  the `catch` → `slugByRepoId.set(cacheKey, null)` → excluded, identical to a
  native miss. A slug enters the index only from a 2xx `{slug}` — no phantom
  slugs.
- **No stale arm decision**: the cache keys `(runtime-scope, repo.id)`, and a
  repo id's `connectionId` is immutable — `PATCH` refuses the edit
  (`repos.rs:244`, test-pinned incl. null), and re-adding under another
  connection mints a new id (`register_repo`'s `(path, connection_id)`
  identity). No edit path exists; confirmed post-015 unchanged (empty
  `repos.rs` diff on those lines apart from the additive `host_id_of` split).

### 7. Grounding honesty (D4 / AC 9) — **PASS**

- **Always present**: `DraftBodyResponse.grounding` is a non-`Option` struct
  field; the booleans are captured from `.is_some()` **before** the contexts
  move into the prompt (`chat.rs`: "Captured before the contexts move into
  the prompt"); the empty-model-body case 400s before the response is built —
  no success response can lack the flag. Both shapes exact-JSON pinned
  (`draft_body_response_serializes_body_and_grounding`).
- **Note only when `repo === false`**: `if (!grounding || grounding.repo)
  return null` (`create-issue-intent-model.ts`) — null flag (pre-020 server)
  silent, wiki-only miss silent (sanctioned §1.5.3), both pinned by the 6 new
  model tests with exact strings.
- **No ungrounded-presented-as-grounded path**: the note is derived from the
  server's flag only ("never inferred from connectionId"); the host label is
  presentation-only (`hostLabel` explains WHY; a repo-miss with no label still
  notes "the project folder wasn't readable here").
- **Error/empty paths**: `setGrounding(null)` resets beside `setFiled(null)`
  at draft start ("a stale note must never describe a fresh draft"); a failed
  draft leaves grounding null → no note over an error. Rendered muted
  (`text-[11px] text-muted-foreground`), never destructive.

### 8. D5 sacred machinery — **PASS**

Independently re-verified the empty diffs myself: `board_goals.rs` **0
lines**, `task_sink.rs` **0**, `auth.rs` **0**, native `gh.rs` **0**,
`start-work-repo-match.ts` + test **0**. Caller semantics preserved:
`resolve_tracker_slug` hands `slug_hint` straight to `resolve_github_slug`,
whose hint fast-path still runs before any git (proven by the
unreadable-workdir test returning `Ok("acme/widgets")`); the only ordering
addition — host-load before hint — is the documented D1 ruling, and for
absent repoId every deleted copy already loaded the host before resolving.
`create_issue` still always passes `slug: Some(&slug)` into `SinkCtx`, so the
sink's explicit-slug arm (`neutral_cwd()`, `--repo <slug>`,
`task_sink.rs:170-177`) is the only arm reachable from this route — the
`ctx.workdir` cwd exists solely in the slug-`None` legacy arm (`:184`),
making F1's trimmed-not-expanded `SinkCtx.workdir` deviation genuinely inert.

### 9. Serde/wire hygiene — **PASS**

All six DTO widenings are add-only `#[serde(default)] Option<String>`; the
three query structs use the surgical per-field `rename = "repoId"` (no
`rename_all` added), the three camelCase body structs need none. Diff-wide
grep for `serde(alias` in code: **zero** (all hits are doc prose — verified
myself). Absent → `None` pinned on all six DTOs. Responses are add-only:
`grounding` is a new key next to `body` (old readers like the wizard's
generate path ignore it; the client type keeps it optional for old-server
skew); `RepoSlugResponse` is a new route. Legacy binding get/put/delete with
no repoId: `bindingQuery` appends `repoId` only when present (pinned:
"carries workdir alone … pre-020 wire shape"), `createIssuePayload` likewise
("carries only title + workdir when nothing optional is supplied").

### 10. Principles sweep — **PASS**

- **No new `is_public`**: `auth.rs` 0-line diff; the new `/api/repos/{id}/slug`
  rides `routes::repos::router()` inside the `require_token` layer
  (`lib.rs:336,349` — read directly).
- **No polling**: the index still rebuilds only on `repos` change; the route
  is plain request/response; nothing schedules.
- **One launch path / no new spawn**: the diff contains no session/spawn
  code — routes touched are slug/DTO/threading only; provision's launch-free
  ensure changed only its slug half; harness changes are two `None` params.
- **Comments why-not-what**: the load-bearing "why local" comments the
  architecture demanded are present at both §2.3 sites (create:
  "Author attribution … must come from the SAME local credential that
  filed"; labels: "the local `gh` … reaches any slug it is authed for") and
  on the fetch-on-resolved-host choice.
- **No AI attribution** on the three code commits (subject-only messages,
  read with `git log --format=%B`).

The tester's focus 1 (the §2.3 host-choice product call): I **agree** with
fetch-on-resolved-host — fetch already threaded `&host` into `gh_in_dir`
pre-020, so keeping the resolved host is the non-surprising continuation, the
remote host owns the repo, and repoId-absent callers are byte-identical. See
should-fix 2 for the one operational caveat.

---

## Should-fix (non-blocking follow-ups; file as tickets)

1. **`ProjectHubPage.tsx:86` Tasks-tab binding read is not repoId-threaded**
   (`getProjectBinding({ workdir: repo.path })`). For a bound SSH repo the
   read 422s (fail-closed `.catch` → default Tasks view), so the hub's Tasks
   tab never auto-enters the bound board's project mode even though the
   Tracker tab (F3) can now create that binding. Not in this spec's
   enumerated legs, but it is the next dead-end a user hits after binding an
   SSH repo. One-line thread + dep, exactly like the intake hook's leg.
2. **SSH-repoId issue fetch composes `neutral_cwd()` (the daemon's LOCAL
   `$HOME`) with the remote host's `gh`**: `gh_in_dir`'s SSH arm runs
   `cd '<local home>' && gh …` (`git_fs.rs:111`), so a cross-OS pair (macOS
   daemon → Linux host, `/Users/…` absent remotely) fails the `cd` and every
   fetch 400s even with `gh` authed on the host. Fail-loud (the desktop's
   "Use" falls back to title+URL) and currently caller-less
   (`fetchGithubIssueBody`'s `repoId?` has no sender; both existing callers
   pass slug only), but the wire is live — the deferred "gh on remote host"
   QA leg will trip this exactly. Fix: a remote-valid neutral cwd (e.g. `~`)
   in the SSH arm, or document the same-layout requirement.
3. **tasks.md wording** (tester nit 1): "environment-RPC and native arms
   byte-identical" is true of the arm *bodies*; the shared ternary's
   condition changed (`target.kind === 'environment'` →
   `arm === 'environment-rpc'`). Equivalent by construction — docs-only
   overstatement, correct it whenever tasks.md is next touched.

## Leave-as-is nits

- `util.rs` test harness `std::mem::forget(dir)` leaks a tempdir per run —
  inherited from the board_goals `fresh_state` pattern; test-only.
- `repo_slug_unknown_id_is_not_found` pins `resolve_repo_path` (the
  handler's first gate) rather than driving the 3-line handler (tester nit 2)
  — acceptable indirection.
- Unconditional `repoId` on the intake file leg (tester nit 3) — ruled
  D1-correct in focus 2.
- `create_issue` missing-local-host ordering micro-delta (tester nit 4) —
  ruled unobservable in focus 3.
- expand-vs-host-load order swap in create/labels moves a hypothetical
  `Internal("local host missing")` behind a `BadRequest` for a malformed
  `~user` workdir — same corrupted-store territory, unobservable.
- IntegrationsPane empty-state copy "Add a repo first" — coherent with the
  sanctioned filter drop.

## Human release checklist (deferred — qa.sh/staging; ships AFTER/WITH 015, same branch)

- [ ] **Live dyaus binding**: Project Hub → Tracker on the SSH repo binds a
      board — no `no_github_repo` (the original screenshot bug).
- [ ] **Live SSH filing**: intake panel drafts (grounding note renders with
      the host label, muted) and files a real GitHub issue;
      provider-confirmed `filed` chip.
- [ ] **Start-work direct launch** on an SSH-only repo (classifies `direct`,
      launches; both-hosts repo still `choose`).
- [ ] **Host-down flavors**: binding family shows the 422 `no_github_repo`
      "could not read" message (not the no-origin one); the slug route
      returns **502 `host_unreachable`** (qa.sh's wire key) and the repo
      drops from the Start-work index (fail-closed, no phantom match).
- [ ] **`gh` on the remote host** for any SSH-repoId issue *fetch* — install
      + auth required there; note should-fix 2's cross-OS neutral-cwd caveat
      before relying on this leg.
- [ ] Do not cherry-pick 020 without 015 — this branch builds on 015's
      commits (classifier, intake panel, dual-entry registry).
