# Spec 020 — Tester Verification

- **Spec:** 020-ssh-host-tracker-plumbing
- **Date:** 2026-07-13
- **Tester:** independent (did not write the code)
- **Commits under test:** F1 `09726c46`, F2 `e8fb31a8`, F3 `820712d9`
  on `fixes-new-workspace`, diffed against spec 015's `3ec6f028`.
  (The `3ec6f028..HEAD` range also contains 015's two docs-only commits
  `fed20898`/`aa8ce9e3` — verified docs-only, excluded from all code claims.)

## Verdict: **PASS-WITH-DEFERRALS**

No defects. All 10 ACs verified against code and test bodies (AC 8 graded
against the amended text). The deferred items are exactly the handoff's
live-SSH legs (real dyaus binding/filing/Start-work, host-down 502) —
qa.sh/staging/human territory, per contract.

## Gate table (all independently re-run at HEAD `820712d9`)

| Gate | Claimed | Reproduced | Notes |
|---|---|---|---|
| `cargo test -p agentum-server --lib` | 701 / 0 / 5 | **701 passed / 0 failed / 5 ignored** | Delta arithmetic corroborated: 9 new F1 tests + 5 new F2 tests counted in the diff; F3 replaced one serde pin one-for-one → 687 + 14 = 701. |
| `cargo fmt --all --check` | clean | **clean** | |
| `cargo clippy -p agentum-server --lib --tests -- -D warnings` | clean | **clean** | Forced recompile (`touch src/lib.rs` → "Checking agentum-server v0.71.0" observed) — not cache-trusted. |
| `npm run build --prefix crates/agentum-desktop/ui` | green | **green** (built in 39.3s; pre-existing chunk-size warnings only) | |
| Targeted `bunx vitest run` (5 files) | 5 files / 53 tests | **5 passed (5), 53 passed (53)** | intent-model 32 (015's 26 + 6 new), issue-client 6, projects-client 4, repo-slug-arm 4, start-work-repo-match 7. |

Baseline corroboration: no other vitest suite imports any touched module
(grep over `*.test.ts{,x}` for use-tracker-intake / ProjectBindingEditor /
IntegrationsPane / repo-slug-index / CreateWorkspaceWizard /
server-repo-client / TrackerIntakePanel / ProjectHubPage = zero hits), so the
pre-broken full-vitest/tsc baselines cannot have gained NEW failures from this
work. Rust suite is 0-failed outright.

## Sacred-surface proofs (`git diff 3ec6f028..HEAD -- <path>`)

| Surface | Proof |
|---|---|
| `components/github-project/start-work-repo-match.ts` (+ its test) | **empty diff** (0 lines, both files) |
| Native Tauri `gh_repo_slug` (`agentum-desktop/src/commands/gh.rs`) | **empty diff** |
| `board_goals.rs` (`resolve_github_slug`, `SlugReason`, `is_valid_slug`) | **empty diff on the whole file** — F1's report claim "no board_goals.rs edits" confirmed; bodies trivially unchanged |
| `task_sink.rs` | **empty diff** |
| `auth.rs` (`is_public`) | **empty diff** |
| `use-tracker-intake.ts` provider resolution + `filed`-from-confirmed | diff read line-by-line: threading + grounding capture only; `setFiled` still only from the provider-confirmed `created` response; `deriveFiledGatedRunGate` call untouched |
| `repo-slug-index.ts` env-RPC + native arms | arm **bodies** byte-identical vs base (`callRuntimeRpc(...)` and `api.gh.repoSlug(...)` expressions unchanged); the shared ternary's condition swapped `target.kind === 'environment'` → `arm === 'environment-rpc'` — semantically equivalent by construction of the pure arm fn (see nit 1). `slugByRepoId` cache key/eviction untouched. |
| Wizard `trackerWorkdir` gating | NOT relaxed — `trackerRepoId` computed with the byte-same gate (`selectedRepo && !selectedRepo.connectionId && selectedRepoIsGit`); comment rewrite only |
| serde aliases / `is_public` in the full diff | grep hits = 11, **all doc prose/comments** (tasks.md, 015 verification docs, module doc comments); zero code hits |

## AC-by-AC verdicts

**AC 1 — repoId → host at all pinned sites: PASS.**
`util::resolve_tracker_slug` body = the former `github_projects.rs::resolve_slug`
verbatim (trim/empty-check → expand → host → `resolve_github_slug` → identical
422 envelope incl. both message strings) with only the host line swapped to
`resolve_tracker_host(state, repo_id)`. All five sites verified in the diff:
`github_projects` get/put/delete binding, `github.rs::create_issue`,
`github.rs::fetch_github_issue` (param `repo_id: Option<&str>` →
`resolve_tracker_host`), `github.rs::list_labels`, `provision.rs` (slug-half
only). Both duplicate resolvers really deleted: `grep -rn 'fn resolve_slug'
crates/agentum-server/src/` = **zero hits**. Absent repoId →
`get_host(LOCAL_HOST_ID)` + the same `Internal("local host missing")` — local
path byte-identical. Unknown repoId → `load_host_for_repo` → `NotFound` (never
a fallback), pinned by `resolve_tracker_host_unknown_repo_id_is_4xx` and pure
`host_id_of_unknown_id_is_not_found`.

**AC 2 — hint short-circuit = zero git I/O: PASS.**
Test body read: `resolve_tracker_slug(&state, None, "/path/does/not/exist",
Some("acme/widgets"))` → `Ok("acme/widgets")`. The proof is sound: had any git
run, `git -C /path/does/not/exist …` fails (spawn error → HostUnreachable, or
non-zero → NoGithubRemote), either of which is an `Err` → the test would fail.
`resolve_github_slug`'s hint fast-path (`board_goals.rs`, unchanged) returns
before the `git_in_dir` call.

**AC 3 — SSH binding routes: PASS (live SSH leg deferred).**
All three binding handlers thread `q.repo_id`/`body.repo_id`; the store stays
slug-keyed (`binding_for_slug`/`remove_binding` untouched); the 422 messages
keep the HostUnreachable ≠ NoGithubRemote split (`no_github_repo_envelope`,
pinned pure by `no_github_repo_envelope_distinguishes_reasons`: same 422 +
`no_github_repo` code, `assert_ne!` on messages). Real remote binding = qa.sh.

**AC 4 — Rust unit tests: PASS.**
All 9 new F1 tests read and confirmed to test what they claim (incl. the
ordering test `resolve_tracker_slug_unknown_repo_id_beats_valid_hint`, which
genuinely pins ordering: hint-first would return `Ok("acme/widgets")`, the
test demands `NotFound`). Wire pins: repoId present deserializes / absent →
`None` on all six DTOs (BindingQuery, PutBindingRequest, IssueQuery,
CreateIssueBody, LabelsQuery, ProvisionRequest). Existing suites green
unmodified (701/0, add-only diffs everywhere except the sanctioned F3 serde
pin replacement).

**AC 5 — renderer slug via server for SSH repos: PASS.**
`GET /api/repos/{id}/slug` (repos.rs): registry path → `load_host_for_repo` →
`resolve_github_slug(&host, &path, None)`; behind the top-level
`require_token` (auth.rs untouched, route not in `is_public`).
`getServerRepoSlug` in server-repo-client.ts; `repo-slug-index.ts`'s `server`
arm lowercases + caches. Local repos keep the native arm
(`slugResolutionArm(false, null|undefined) === 'native'`, pinned).

**AC 6 — Start-work classifies SSH repo `direct`: PASS (live leg deferred).**
`classifyStartWorkRepoMatches` + its 7 test pins byte-untouched (empty diffs)
and re-run green — sole-remote `direct`, both-hosts `choose` exactly as 015
defines. The new behavior (SSH repo *enters* the index) is pinned by the arm
tests; the live launch = qa.sh.

**AC 7 — fail-closed exclusion: PASS.**
The `server` arm sits inside the pre-existing `try`; `getJson` throws on any
non-2xx (`server-http.ts:23`) → the pre-existing `catch` → `slugByRepoId.set(cacheKey,
null)` → excluded, identical to a native miss. Negative caching semantics
unchanged. Wire side: `slug_reason_wire` pins 422 `no_github_remote` vs **502**
`host_unreachable` (pure test asserts statuses, codes, and distinct messages) —
transport can never masquerade as no-origin.

**AC 8 (amended) — intake threading: PASS (live SSH filing deferred).**
Binding read: `getProjectBinding({ workdir, repoId: repo.id })`, deps gain
`repo.id`. File: `createGithubIssue({ …, repoId: repo.id })`. Draft leg:
payload unchanged (workdir/title/slug-first) — `DraftBodyRequest` has **no**
`repo_id` field (verified in github.rs), exactly the amended text. Create is
slug-only downstream: `TaskSink::Github`'s explicit-slug arm runs `gh` from
`neutral_cwd()` with `--repo <slug>` (task_sink.rs:165-178, unchanged), and
`create_issue` always passes `slug: Some(&slug)`.

**AC 9 — honest grounding note: PASS (live SSH draft deferred).**
Server: `DraftedIssue { body, grounded_repo, grounded_wiki }` captured from
`.is_some()` BEFORE the contexts move into the prompt; response carries an
**always-present** `grounding: {repo, wiki}` (plain struct field, not
`Option`), pinned by the exact-JSON serde test for both shapes. Empty-model
body 400s before serialization, so no success response can lack the flag; the
client tolerates old-server absence via `res.grounding ?? null`. Note derives
ONLY when `grounding.repo === false` (`if (!grounding || grounding.repo)
return null`); null flag (pre-020 server) silent; wiki-only miss silent; exact
host-label / unreadable-folder strings pinned by 6 new model tests. Rendered
muted (`text-[11px] text-muted-foreground`), never destructive. Per-draft
reset (`setGrounding(null)` beside `setFiled(null)`) — no stale note.

**AC 10 — inline errors, provider-confirmed `filed`: PASS.**
`draft`/`fileGithub` error paths untouched (`setError(...)` inline);
`setFiled` only from the provider-confirmed `created` response (the AC 12
comment and code unchanged); panel error rendering untouched.

## Deviation audit (all 15 numbered deviations)

**F1 (7):**
1. 422 messages distinguish reasons on create/labels/provision — **ACCURATE**.
   Old generic string removed in the diff; `code: "no_github_repo"` identical;
   UI greps confirm no message string-matching (`no GitHub repo resolved` = 0
   hits in ui/src; `no_github_repo` appears only in code-branching comments).
2. provision slug read now tilde-expands — **ACCURATE** (old copy passed the
   trimmed-unexpanded workdir; no-op post the handler's own expand+`is_dir`
   gate, which is intact and runs first).
3. `SinkCtx.workdir` = trimmed not expanded — **ACCURATE and safe**: the route
   always passes `slug: Some(_)`, and the explicit-slug arm uses
   `neutral_cwd()`, never `ctx.workdir` (`.current_dir(ctx.workdir)` exists
   only in the slug-`None` legacy arm).
4. Local-host derivation before the sink create — **ACCURATE**; failure
   ordering preserved in both arms (absent repoId: the resolver itself loads
   the local host; present repoId: line 266 fails before `create_feature`).
5. `repo_id` param after `state` — **ACCURATE** (trivial; position unpinned).
6. Line drift cosmetic — accepted (not independently re-measured; no
   functional drift found in any diff).
7. `ai/STATE.md` concurrent drift left uncommitted — **ACCURATE**:
   `git status` shows `M ai/STATE.md`; absent from all three commit stats.

**F2 (3):**
1. `slug_on_host` handler-core split — **ACCURATE**; wire behavior identical
   (handler = `resolve_repo_path` → `load_host_for_repo` → `slug_on_host`);
   enables the temp-repo tests without touching `~/.agentum/repos.json`.
2. Line drift cosmetic — accepted.
3. STATE.md — same as F1-7, **ACCURATE**.

**F3 (5):**
1. Serde pin replaced one-for-one — **ACCURATE**: old
   `draft_body_response_serializes_body_field` asserted the exact pre-020 JSON
   of the struct F3 widens; renamed to `…_body_and_grounding`; count unchanged
   (701). No other existing test touched (intent-model test diff = 41
   insertions, **0 deletions**).
2. IntegrationsPane empty-state "Add a repo first" — **ACCURATE**, coherent
   with the sanctioned filter drop.
3. `trackerRepoId` hops through `AgentStep` — **ACCURATE**; the mount lives
   inside `AgentStep`, gate unchanged.
4. Line drift cosmetic — accepted.
5. STATE.md — **ACCURATE**.

## Adversarial spot-checks (6)

1. **repoId + workdir for DIFFERENT repos:** the git read runs at the client's
   workdir on the repoId's host (architecture §2.1's explicit ruling — "workdir
   as-sent, only the host swaps"). A mismatched pair either 422s honestly or
   resolves whatever repo lives at that path on that host — a documented,
   sanctioned contract, and every real caller (intake hook, editor feeders)
   passes coherent `(repo.path, repo.id)` pairs from one repo object. Not a
   defect. (The F2 route side-steps this class entirely by using the registry
   path and taking no workdir.)
2. **502 body hygiene:** `slug_reason_wire` returns `&'static str` messages;
   `SlugReason` is a payload-free enum, so no SSH stderr/hostname/token can
   reach the wire. `load_host_for_repo`'s 404/400 carry only the repo id.
   Clean.
3. **Stale `connectionId=""`:** `'' ? 'server' : 'native'` → falsy → native
   arm; null/undefined both pinned native by tests. No misroute.
4. **`grounding` on empty/error drafts:** an empty model body 400s before the
   response is built; error paths return the error envelope; every success
   response carries the non-optional struct field. Client `?? null` covers
   old-server skew. Cannot be absent-on-success.
5. **Legacy callers with NO repoId:** resolver body is the old body verbatim;
   `None` arm loads `LOCAL_HOST_ID` with the identical `Internal` fallback;
   wire pins assert absent → `None` on all six DTOs; harness's two
   `fetch_github_issue` callers pass `None` with byte-identical-pin comments.
   Byte-identical local behavior confirmed.
6. **IntegrationsPane filter drop end-to-end:** an SSH repo now reaches
   `ProjectBindingEditor` with `workdir={selected.path}` (remote absolute
   path) + `repoId={selected.id}` → `bindingQuery` appends both → server
   resolves the host from the id and reads origin remotely. No dead-end path
   remains in the code; live proof = qa.sh.

## Nits (no action required to pass)

1. The "environment-RPC and native arms **byte-identical**" claim (tasks.md
   F2 §4 / handoff) is true of the arm *bodies*; the shared ternary's
   condition expression did change (`target.kind === 'environment'` →
   `arm === 'environment-rpc'`). Equivalent by construction of the pure arm
   fn — but the literal wording overstates slightly.
2. `repo_slug_unknown_id_is_not_found` pins `resolve_repo_path` (the
   handler's first gate) rather than driving the 3-line handler itself —
   acceptable, mildly indirect.
3. The intake **file** leg now sends `repoId: repo.id` unconditionally — for
   a local repo whose registry entry vanished mid-session, filing 404s loud
   where pre-020 it would have resolved via workdir. This is D1's intent
   (identity errors must be loud) applied to a stale-state client; worth one
   line in the reviewer's head, not a defect.
4. `create_issue`'s failure ordering for a *missing local host* moved from
   before-slug-read to inside-the-resolver (absent repoId) / after-slug-read
   (present repoId) — still always before any `gh` call, which is the
   invariant that matters.

## Deferred (record, not failed)

- Live SSH legs: real dyaus binding read/write, SSH issue filing, Start-work
  direct launch on an SSH-only repo, host-down 502 flavor in the UI —
  qa.sh/staging/human per the spec's own gate split.
- Full vitest (~139 fails) and bare tsc — pre-broken baselines, non-gates;
  corroborated no new exposure (no other suite imports touched modules).
