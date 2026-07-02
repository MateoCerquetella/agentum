# Spec 004 — Architecture Blueprint: Workspace issue loop (issue-first workspaces)

**Self-check passed.** Load-bearing cites re-verified on the `fix-wiki` worktree
(pre-merge; see the drift warning below): `task_sink.rs:280-282` (the GitHub
no-op arm), `drive.rs:321` (the seam call), `board_goals.rs:602` (the second
caller), `worktrees.rs:351-353` (dropped metadata), `git_fs.rs:90`
(`gh_in_dir`), `types.rs:905` (`plan_from_spec`). Exactly **two** production
callers of `apply_tracker_transition` exist (`drive.rs:321`,
`board_goals.rs:602`) plus three tests in `task_sink.rs`.

> ⚠️ **Line-drift warning (orchestrator note):** after this blueprint was
> line-verified, `origin/develop` (+35 commits) was merged into the worktree —
> including spec 003's `task_sink.rs`/`chat.rs` changes (`NewFeature.labels`,
> `gh --label`). The design stands; treat every `:line` as approximate and
> re-locate before editing. 003's labels-on-create is adjacent to (not
> overlapping) F1's labels-on-transition.

**Status:** Architect → ready for Developer. All D1–D5 honored; five
spec-vs-code corrections below (C1–C5) — none blocks the build, all change
*where* work lands.

---

## 0. TL;DR — the four features, one sentence each

1. **F1** — widen `apply_tracker_transition` with `tracker_url: Option<&str>`;
   the GitHub arm parses `owner/repo` **and the issue number** from the URL,
   ensure-creates the 4 canonical labels (`gh label create … --force`,
   idempotent), then one `gh issue edit --add-label <target>
   --remove-label ×3`; every failure → `Ok(Skipped(reason))`.
2. **F2** — widen `CreateBody` (serde-compatible, `alias = "linkedPR"`),
   persist the three linked fields into the registry row, fix the two UI-client
   layers that currently **strip** them, and fix the `linkedPr`→`linkedPR`
   wire-key mismatch on the read path the UI actually consumes.
3. **F3** — new `POST /api/github/issues` (thin over
   `TaskSink::Github::create_feature` + `resolve_github_slug`); the composer
   files pre-create and the response becomes `linkedWorkItem`. (The Tasks-page
   `api.gh.createIssue` path is a local **stub** — C1 — so reuse is impossible.)
4. **F4** — new `POST /api/harness/spec-from-issue`: server-fetches the issue
   (`gh issue view --json title,body,url`, shared with `GET /api/github/issue`),
   pure `spec_md_from_issue` transform (checkboxes preserved, fallback AC
   synthesized), `scaffold_harness` + `plan_from_spec_with_tracker` stamp
   `tracker_provider`/`tracker_url` onto every derived feature.

---

## 1. Spec-vs-code corrections (read before building)

- **C1 — the Tasks-page create path does not work locally.** The spec's
  "Reuse vs build" offers "reuse of the Tasks-page create path" for F3
  (`TaskPage.tsx:2452` → `api.gh.createIssue`). On the local desktop that
  resolves to the native Tauri command `gh_create_issue`
  (`tauri/gh.ts:15` → `agentum-desktop/src/commands/gh.rs:476-478`), which is a
  stub: `not_available()` → `{ok:false, "The GitHub API isn't available in this
  build."}` (`gh.rs:436-438`). It only functions via `callRuntimeRpc` against a
  hosted runtime. **F3 must be the server endpoint; there is no local path to
  reuse.** (Follow-up, not in scope: point the Tasks dialog's local branch at
  the new endpoint.)
- **C2 — "the UI already sends the linked fields" is only true one layer
  deep.** `store/slices/worktrees.ts:1055-1073` builds
  `linkedIssue`/`linkedPR`/`linkedLinearIssue` into `createArgs`, but
  `tauri/worktrees.ts:18-25` **strips everything** except
  `repoId/name/baseBranch/branchNameOverride/displayName` before
  `worktreesCreate` (`runtime/server-worktree-client.ts:26-34`) posts. F2 has
  **two TS files to widen**, not zero. Also `useComposerState.ts:2034`'s
  comment ("linked source metadata is already included in createWorktree")
  documents an assumption that is currently false end-to-end.
- **C3 — wire-key mismatch `linkedPR` vs `linkedPr`.** The UI type reads
  `linkedPR` (`shared/types.ts:233`, comment at `:239` pins that spelling);
  the server registry struct camelCases `linked_pr` to `linkedPr` and the
  `detected` scan emits `"linkedPr"` (`worktrees.rs:739`) — a dead key no UI
  code reads (repo-wide grep: zero `linkedPr` readers). F2 must
  `alias = "linkedPR"` on `CreateBody`, emit `linkedPR` on the detected read
  path, and **must not** alias the registry `Worktree` struct itself (§3,
  duplicate-field wipe risk).
- **C4 — AC 3's "removes any other `status/*` label" conflicts with D3.**
  Taken literally it would strip `status/qa` / `status/qa-pass` /
  `status/qa-fail` — this very repo's human-QA labels (`.github/labels.sh`),
  which D3 explicitly says not to conflate. Resolved in D3's favor: the
  transition **deterministically removes only the other three canonical
  labels**. Invariant as built: *exactly one canonical harness `status/*`
  label*; foreign `status/*` labels are never touched. (This also matches the
  no-read-modify-write preference: no `gh issue view` before the edit.)
- **C5 (nuance, not a contradiction) — D2 says "via `gh_in_dir`", but the
  creation path it cites as precedent doesn't use it.**
  `TaskSink::Github::create_feature` runs a direct
  `tokio::process::Command("gh")` from `neutral_cwd()` (`task_sink.rs:155-176`);
  `gh_in_dir`'s `Local` arm (`git_fs.rs:92-104`) is byte-identical mechanics
  plus a `Host` parameter. Both transition callers are **structurally local**:
  the drive loop hard-codes `LOCAL_HOST_ID` (`drive.rs:349-352`) and
  `plan_goal_harness` gates on a local `wd.exists()` (`board_goals.rs:535`) —
  "features run in remote workdirs" is false for the harness today. So F1 runs
  local `gh` from `neutral_cwd()` with `--repo <slug>` — *exactly* the creation
  path's slug arm, honoring D2's rationale (same CLI, same auth surface). No
  `Host` threads through the seam. If a remote-host harness ever lands, the
  widening point is an `Option<&Host>` on the GitHub arm's runner — swap the
  exec to `gh_in_dir` then, not now.

Confirmed for the PM: **`board_sync` cannot strip `status/*` labels.** Its
GitHub PATCH body is exactly `{title, body, state}` (`board_sync.rs:463-469`);
a labels-less PATCH leaves labels untouched, and no label-writing code exists
anywhere in `agentum-server` (grep: zero matches for `--add-label` /
`label create` / `"labels"` — pre-merge; 003's merge added labels-on-CREATE,
which still never removes labels). With D1 (label-only Done) the two
authorities are fully disjoint: `board_sync` owns open/closed + title/body;
the harness owns the canonical labels.

---

## 2. F1 — `github-status-transition` (AC 3–5)

### Boundaries

| File | Change |
|---|---|
| `crates/agentum-server/src/task_sink.rs` | Widen the seam; replace the no-op arm (`:280-282`); add label table + pure argv builders + parser + runner; replace/extend tests |
| `crates/agentum-server/src/harness/drive.rs` | **One line** at `:321` — thread `feature.tracker_url.as_deref()` (the only permitted touch; control flow, transition points at `:129/:184/:240`, and autonomy mechanics untouched) |
| `crates/agentum-server/src/routes/board_goals.rs` | One line at `:602-608` — pass `url.as_deref()` (in scope since `:572-597`); optionally log the `Skipped` arm at `:610` (today `Ok(_) => {}` swallows skips silently) |

### Seam signature (serves BOTH callers)

```rust
// task_sink.rs — widened. board/linear arms ignore the new param.
pub async fn apply_tracker_transition(
    store: &Store,
    provider: &str,
    tracker_id: &str,
    tracker_url: Option<&str>,   // NEW: GitHub needs owner/repo (+ number) — from Feature.tracker_url
    phase: TrackerPhase,
) -> anyhow::Result<TransitionResult>
```

Call sites:
```rust
// drive.rs:321 (inside transition_tracker — the ONE drive.rs touch, AC 4)
apply_tracker_transition(&state.store, provider, &feature.id,
                         feature.tracker_url.as_deref(), phase).await
// board_goals.rs:602 (initial Todo at plan time)
apply_tracker_transition(&state.store, p, &id, url.as_deref(), TrackerPhase::Todo).await
```

### The URL is authoritative for slug AND number (load-bearing decision)

The repo-slug parse lives in `task_sink.rs` (NOT `board_sync` — `task_sink` is
a crate-root seam and must not depend on a route module; `board_sync`'s
`parse_github_issue` at `:374-384` stays private and untouched):

```rust
/// `https://github.com/{owner}/{repo}/issues/{n}` → (slug, number). Rejects
/// /pull/ URLs, non-github hosts, and non-numeric tails. Pure.
fn github_slug_and_number_from_issue_url(url: &str) -> Option<(String, String)>
```

The GitHub arm takes the **number from the URL, not from `tracker_id`**. Why:
F4 derives N features (`F1..Fn`) from ONE issue's checkboxes —
`write_backlog_from_features` rejects duplicate ids (`types.rs:967-968`), so
feature ids cannot all be the issue number; only `tracker_url` reliably names
the issue. For `plan_goal_harness` features (1 issue : 1 feature,
`fref.id == number`, url matches — `board_goals.rs:595`) the two sources agree,
so this is uniform, not a special case. `tracker_id` remains the handle for
board/linear.

### Labels (D3) — fixed table + pure argv builders

```rust
/// Canonical, harness-owned status labels (D3). NOT .github/labels.sh's
/// status/qa* — that is the human-QA lifecycle (C4).
const GITHUB_STATUS_LABELS: [(TrackerPhase, &str, &str); 4] = [
    (TrackerPhase::Todo,        "status/todo",          "ededed"),
    (TrackerPhase::InProgress,  "status/in-progress",   "1d76db"),
    (TrackerPhase::ReadyToTest, "status/ready-to-test", "fbca04"),
    (TrackerPhase::Done,        "status/done",          "0e8a16"),
];
fn github_status_label(phase: TrackerPhase) -> &'static str;

/// Idempotent ensure-create: --force updates an existing label's color to
/// canonical instead of failing. One argv token per value — never a shell.
fn gh_label_ensure_argv<'a>(name: &'a str, slug: &'a str, color: &'a str) -> [&'a str; 8]
// ["label","create",name,"--repo",slug,"--color",color,"--force"]

/// Set-one/remove-others in ONE edit: add the target, deterministically remove
/// the other THREE canonical labels (no read-modify-write; gh treats removing
/// an absent label as a no-op). Foreign status/* labels untouched (C4).
fn gh_set_status_label_argv<'a>(number: &'a str, slug: &'a str, phase: TrackerPhase) -> Vec<&'a str>
// ["issue","edit",number,"--repo",slug,"--add-label",target,
//  "--remove-label",a,"--remove-label",b,"--remove-label",c]
```

### Execution — best-effort by construction (AC 5)

```rust
/// `gh` binary override — a real knob (server with gh off PATH), and the docs
/// hook for tests (which pass the program explicitly; no env mutation).
fn gh_bin() -> String { std::env::var("AGENTUM_GH_BIN").unwrap_or_else(|_| "gh".into()) }

/// One gh call from neutral_cwd() (task_sink.rs:313 — same cwd discipline as
/// creation). Ok on exit 0; Err(bounded stderr, ~240 chars) otherwise.
async fn run_gh(program: &str, args: &[&str]) -> Result<(), String>;

/// Ensure 4 labels (failures NON-fatal — a shared repo without label-create
/// permission can still add an existing label), then the single edit decides:
/// exit 0 → Applied; anything else → Skipped(reason). Never returns Err.
async fn github_transition_with(program: &str, slug: &str, number: &str,
                                phase: TrackerPhase) -> TransitionResult;
```

The arm:
```rust
"github" => {
    let Some(url) = tracker_url.map(str::trim).filter(|u| !u.is_empty()) else {
        return Ok(TransitionResult::Skipped("feature has no tracker_url; owner/repo unknown".into()));
    };
    let Some((slug, number)) = github_slug_and_number_from_issue_url(url) else {
        return Ok(TransitionResult::Skipped(format!("cannot parse a GitHub issue from {url}")));
    };
    Ok(github_transition_with(&gh_bin(), &slug, &number, phase).await)
}
```

**Every** failure path is `Ok(Skipped(reason))` — the arm never produces `Err`,
so both callers' existing logging (`drive.rs:327-337` logs Skipped as a
`HarnessEvent`; `board_goals.rs:610` warns on Err) keeps the best-effort
contract with zero caller changes beyond the one-line widenings. Cost note: 5
`gh` calls per transition (~15 per feature), inline in the drive loop but
dwarfed by settle waits (minutes); a per-run ensure-memo is a named follow-up,
not v1 (premature optimization).

**Tradeoff taken:** unconditional ensure-create every transition (idempotent,
self-heals a mid-run label deletion) over lazy ensure-on-edit-failure (fewer
calls, but requires stderr classification — fragile). **Rejected:** `forge_send`
/PAT (D2), issue close on Done (D1), `gh_in_dir` + `Host` threading (C5).

### Unit-test plan (verify.sh: `cargo test -p agentum-server --lib`)

1. `github_status_label_covers_all_phases_uniquely` — 4 distinct labels.
2. `gh_label_ensure_argv_is_idempotent_shape` — exact 8-token argv incl. `--force`.
3. `gh_set_status_label_argv_adds_one_removes_exactly_the_other_three` — per
   phase: target added, other 3 removed, target never removed, **no
   non-canonical name appears** (the C4 invariant at argv level).
4. `github_slug_and_number_from_issue_url_parses_and_rejects` — ok URL;
   `/pull/`, GitLab host, non-numeric tail rejected.
5. `github_transition_without_url_is_skipped` — **replaces**
   `github_transition_is_a_logged_noop` (`task_sink.rs:496-502`).
6. `github_transition_applies_with_fake_gh` (`#[cfg(unix)]`) — a tempdir
   `gh`-fake script that logs `"$@"` and exits 0, passed as `program` (no env
   mutation, no lock needed); assert `Applied` + 5 invocations, last one the
   `issue edit`.
7. `github_transition_maps_gh_failure_to_skipped` (`#[cfg(unix)]`) — fake exits
   1 with stderr; assert `Skipped` contains the stderr.
8. Mechanical arity update of the existing board-arm tests (`:472/:489`).

---

## 3. F2 — `worktree-linked-metadata` (AC 2)

### Boundaries

| File | Change |
|---|---|
| `crates/agentum-server/src/routes/worktrees.rs` | `CreateBody` +3 fields (`:249-260`); `create()` persists them (`:351-353`); `scan_git_worktrees` key fix (`:739`); `update_meta` key canonicalization (`:233-238`) |
| `crates/agentum-desktop/ui/src/tauri/worktrees.ts` | `create` forwards `linkedIssue`/`linkedPR`/`linkedLinearIssue` (`:18-25`) |
| `crates/agentum-desktop/ui/src/runtime/server-worktree-client.ts` | `worktreesCreate` arg type widened (`:26-34`) |

### Exact seams

```rust
// worktrees.rs — old clients unchanged: Option fields deserialize None when
// absent; unknown extra keys (pushTarget, workspaceStatus, …) stay ignored
// (no deny_unknown_fields).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody {
    repo_id: String,
    name: String,
    #[serde(default)] base_branch: Option<String>,
    #[serde(default)] branch_name_override: Option<String>,
    #[serde(default)] display_name: Option<String>,
    #[serde(default)] linked_issue: Option<i64>,                    // wire: linkedIssue
    #[serde(default, alias = "linkedPR")] linked_pr: Option<i64>,   // UI sends linkedPR (C3)
    #[serde(default)] linked_linear_issue: Option<String>,          // wire: linkedLinearIssue
}
```

`create()` at `:351-353`: `linked_issue: body.linked_issue`, etc. The
`{ worktree }` response then serializes them (`linkedIssue` / `linkedPr` /
`linkedLinearIssue`) — AC 2's "returns them in the response" is satisfied; note
the response's `linkedPr` casing is **inert** for the UI (both submit paths
read only `result.worktree.id/setup/defaultTabs`, `useComposerState.ts:2031-2060
/:2219-2222`; the sidebar hydrates from `detected`).

Read-path fix (what the UI actually consumes — `fetchWorktrees` →
`listDetected`, `worktrees.ts:764-767`): change the hand-built key at
`worktrees.rs:739` from `"linkedPr"` to `"linkedPR"` (a `json!` literal, not a
serde struct — no disk format involved).

`update_meta` canonicalization: translate incoming key `"linkedPR"` →
`"linkedPr"` before the insert loop (`:233-238`) so post-create edits hit the
typed field instead of shadowing it in `extra` — a pure
`fn canonical_meta_key(key: &str) -> &str`.

**Do NOT alias the registry `Worktree` struct** (`:48-63`): legacy rows carry
both `"linkedPr": null` (typed) and `"linkedPR": 7` (in `extra`, written by
old `update_meta` calls); an alias would make serde see the field twice →
`duplicate field` error → `read_worktrees`'s `unwrap_or_default()` (`:79`)
returns `[]` → the next write **wipes the registry**. The struct stays
byte-identical; migration of legacy shadowed rows is explicitly out of scope.

```ts
// tauri/worktrees.ts — forward what the store already sends (C2)
create: (...args: any[]) =>
  worktreesCreate({
    repoId: args[0]?.repoId, name: args[0]?.name, baseBranch: args[0]?.baseBranch,
    branchNameOverride: args[0]?.branchNameOverride, displayName: args[0]?.displayName,
    linkedIssue: args[0]?.linkedIssue, linkedPR: args[0]?.linkedPR,
    linkedLinearIssue: args[0]?.linkedLinearIssue,
  }),
// server-worktree-client.ts — args type += linkedIssue?: number; linkedPR?: number;
// linkedLinearIssue?: string
```

**Tradeoff taken:** scope = the three AC-2 fields only. `pushTarget`,
`createdWithAgent`, `workspaceStatus`, GitLab links remain stripped (they
predate this spec and have their own persistence gaps) — widening them here
would balloon F2 past its gate. **Rejected:** renaming the registry struct's
serialization to `linkedPR` (the wipe risk above).

### Unit-test plan

1. `create_body_accepts_ui_linked_keys` — payload with `linkedIssue`/`linkedPR`
   /`linkedLinearIssue` populates all three; `linkedPr` variant also accepted.
2. `create_body_defaults_absent_linked_fields` — an old-client payload (5
   original keys) parses with all three `None`.
3. `canonical_meta_key_maps_linkedPR` — and passes other keys through.
4. Extend `worktree_serializes_camel_case_and_flattens_extra` (`:959-984`) to
   pin the registry struct's on-disk keys (regression guard for the no-alias rule).
5. `npm run build --prefix crates/agentum-desktop/ui` green.

---

## 4. F3 — `composer-create-issue` (AC 1)

### Reuse decision

**Build the thin server endpoint** over `TaskSink::Github::create_feature` —
mandated by AC 1's own wording ("through the existing `TaskSink::Github` path")
and forced by C1 (the Tasks-page path is a local stub). **Rejected:**
`api.gh.createIssue` (stub, C1); `POST /api/chat/issues` (runs an LLM
extraction over a transcript — wrong contract for a typed title+body form).

### Boundaries

| File | Change |
|---|---|
| `crates/agentum-server/src/routes/github.rs` | `POST /api/github/issues` handler + route (`router()` at `:23-25`); already merged+authed via `lib.rs:298` (do not touch `is_public`) |
| `crates/agentum-server/src/routes/board_goals.rs` | `map_sink_error` (`:450`) → `pub(crate)` (one word) |
| `crates/agentum-desktop/ui/src/runtime/github-issue-client.ts` | `createGithubIssue()` |
| `crates/agentum-desktop/ui/src/hooks/useComposerState.ts` + `components/NewWorkspaceComposerModal.tsx` (`:112`) | the affordance + wiring |

### HTTP contract

```rust
#[derive(Deserialize)] #[serde(rename_all = "camelCase")]
struct CreateIssueBody {
    title: String,                       // trimmed non-empty, else 400
    #[serde(default)] body: Option<String>,
    workdir: String,                     // for the origin read when no slug hint
    #[serde(default)] slug: Option<String>, // owner/repo fast path
}
#[derive(Serialize)] #[serde(rename_all = "camelCase")]
struct CreateIssueResponse { provider: &'static str, number: i64, url: String, slug: String }
```

Handler flow (mirrors `chat.rs:1086-1153`'s shape, minus the LLM):
LOCAL host → `board_goals::resolve_github_slug(&host, &workdir, slug_hint)`
(miss → typed 422 `no_github_repo`, exactly `chat.rs:1120-1124`) →
`TaskSink::Github.create_feature(SinkCtx{ slug: Some(&slug), workdir: shape-only,
parent_goal_id: None })` — runs `gh issue create --repo <slug>` from
`neutral_cwd()` — errors mapped via `map_sink_error` → parse
`FeatureRef.id` to `i64` (guaranteed digits by `parse_gh_issue_url`; a parse
miss is a 500). Auth: rides the global `require_token` layer.

### Sequencing + orphan analysis (AC 1)

Filing is its **own user action inside the composer, before Create**: click
"Create GitHub issue" (rendered only when `linkedWorkItem == null`) → inline
title/body mini-form (title pre-seeded from the workspace name/prompt) →
`createGithubIssue()` → on success:

```ts
setLinkedWorkItem({
  type: 'issue', number, title, url,
  ...(body.trim() ? { linkedContext: { provider: 'github', version: 1,
      renderedText: buildGithubIssueContextSnapshot({ number, title, url, body }) } } : {}),
})  // LinkedWorkItemSummary — lib/new-workspace.ts:51-63
setLinkedIssue(String(number))
```

The chip (number + URL) renders immediately — **before** any worktree exists;
the existing submit paths then thread `linkedIssue` into `createWorktree`
(F2). Failure modes: request fails → inline error, zero state change, no
orphan; issue created then composer abandoned → a real, deliberately-filed
GitHub issue exists (not orphan *app* state — the user asked for it; it shows
on the Tasks page). No rollback machinery needed. The body is **in hand** (the
user just typed it), so no refetch — `linkedContext` is seeded directly and the
prompt path's containment (`buildContainedLinkedContextBlock`,
`linked-work-item-context.ts:22`) applies unchanged.

### Unit-test plan

1. `create_issue_rejects_blank_title` (pure validation or handler-level 400).
2. Number parse: `FeatureRef.id → i64` helper test (digits ok, junk errors).
3. Existing sink/argv/slug tests already cover creation mechanics — no new
   subprocess tests.
4. `npm run build` green; the QA gate covers the rendered chip end-to-end.

---

## 5. F4 — `spec-from-issue-scaffold` (AC 6–7)

### Endpoint shape decision

**Standalone `POST /api/harness/spec-from-issue`** (in `routes/harness.rs`,
router at `:27-37`; authed by the same layer, `lib.rs:300`). **Rejected:** a
param on `POST /api/worktrees/create` — that route is host-aware (SSH repos via
`git_in_dir`) while every scaffold helper is local `tokio::fs`
(`types.rs:667-994`); coupling them would silently break remote repos or drag
host-aware fs work into scope. Standalone also matches the MCP wrappers'
workdir-param shape (`mcp.rs:1030-1086`) so an `agentum_harness_spec_from_issue`
MCP tool can wrap it later, and the composer needs the worktree path *first*
anyway (the spec is written INTO the new worktree).

### HTTP contract

```rust
#[derive(Deserialize)] #[serde(rename_all = "camelCase")]
struct SpecFromIssueRequest {
    workdir: String,                 // the NEW worktree path; expand_workdir + is_dir
    number: String,                  // digits-only (reuse is_numeric_issue_id discipline, github.rs:48)
    #[serde(default)] slug: Option<String>,
    #[serde(default = "default_true")] plan: bool,   // also write feature_list.json
}
#[derive(Serialize)] #[serde(rename_all = "camelCase")]
struct SpecFromIssueResponse {
    spec_id: String,                 // "<number>-<slug>"
    spec_path: String,               // ".agentum-harness/specs/<spec_id>/spec.md"
    written: Vec<String>,            // scaffold + spec files written
    features: Option<crate::harness::FeatureList>,   // when plan
}
```

### Server fetch (shared with the existing read)

Refactor `routes/github.rs::get_issue`'s core into:

```rust
pub(crate) struct FetchedIssue { pub title: String, pub body: String, pub url: String, pub slug: String }
/// gh issue view <n> --repo <slug> --json title,body,url from neutral_cwd —
/// identical mechanics to get_issue (:84-108); url comes from gh (authoritative,
/// GHES-correct), never string-assembled.
pub(crate) async fn fetch_github_issue(state: &AppState, workdir: &str,
    number: &str, slug_hint: Option<&str>) -> Result<FetchedIssue, ApiError>
```
`get_issue` delegates (adds `url` to its `--json`, ignores it — no wire change).
Server-side fetch (vs. trusting a client-supplied body) keeps the transform's
input authoritative and makes the endpoint usable by any future client.

### The deterministic transform (pure, in `harness/types.rs` next to `derive_backlog_from_spec`)

```rust
/// Deterministic spec.md from an issue — no LLM (spec non-goal). Body is
/// verbatim except: C0/C1 control chars stripped (\n kept, \t → two spaces —
/// mirrors escapeLinkedContextControlChars, linked-work-item-context.ts:52),
/// capped at 64 KiB with a "[truncated]" marker. When the body contains no
/// `- [ ]`/`- [x]` line, appends "## Acceptance criteria\n\n- [ ] <title>" so
/// plan_from_spec ALWAYS round-trips (types.rs:912 would otherwise bail).
/// Header: "# Spec <number> — <title>" + a provenance line naming the issue
/// URL and stating the body below is verbatim issue content.
pub fn spec_md_from_issue(number: &str, title: &str, body: &str, url: &str) -> String;

/// "<number>-<slug>": number is digits-validated by the route; slug = title
/// lowercased, [a-z0-9]+ runs joined by '-', capped at 40 chars, fallback
/// "issue". Both atoms server-constructed → the specs/ join cannot traverse
/// (same guard class as helpers.rs:170's sanitize).
pub fn issue_spec_id(number: &str, title: &str) -> String;

/// plan_from_spec + stamp tracker provenance on every derived feature (AC 7).
/// Implemented as plan_from_spec_inner(workdir, spec_id, tracker: Option<(&str,&str)>);
/// the existing plan_from_spec (types.rs:905) delegates with None — the MCP
/// tool (mcp.rs:1073) is behaviorally unchanged.
pub async fn plan_from_spec_with_tracker(workdir: &Path, spec_id: &str,
    provider: &str, url: &str) -> anyhow::Result<FeatureList>;
```

Going through `derive_backlog_from_spec` (not `write_backlog_from_features`)
preserves checked-box → `Done` mapping (`types.rs:883-887`);
`write_backlog_from_features` would reset everything to `Pending`
(`types.rs:974`) and reject the duplicate ids a per-issue backlog would need.
Feature ids are `F1..Fn`; every feature carries
`tracker_provider: Some("github")` + `tracker_url: Some(<issue url>)` — which
is exactly why F1's GitHub arm reads the number from the URL. **Documented
semantics:** with N features on one issue, the label reflects the
currently-driven feature's phase (some mid-run churn), ending at `status/done`
when the last feature goes green.

### Handler flow

`expand_workdir` + `is_dir` → validate `number` digits → `fetch_github_issue`
→ `scaffold_harness(&wd)` (idempotent, keeps existing files —
`types.rs:667-680`) → if `specs/<spec_id>/spec.md` exists → typed 409-style
`BadRequest("spec <id> already exists")` (**never overwrite** a possibly
human-edited spec; keep-existing matches the scaffold ethos) → write it → if
`plan`: `plan_from_spec_with_tracker(...)` (note: this overwrites
`feature_list.json` — the same semantics `plan_from_spec` already has; in the
composer flow the fresh worktree only holds the scaffold stub, so the
overwrite is the intended replacement).

### Untrusted-content containment (the honest framing)

The spec.md is *meant* to become agent instructions — that is the feature
(D5 opt-in). Containment therefore means: (1) path atoms are fully
server-constructed and validated (no traversal via a crafted title);
(2) control characters are stripped so an issue body cannot smuggle terminal
escapes into files/panes; (3) size is capped; (4) provenance is stamped so a
reviewer sees the origin; (5) the transform never interpolates the body into a
shell or argv (pure file write). Full "treat as data, not instructions"
framing remains the **prompt layer's** job
(`buildContainedLinkedContextBlock`) and is not duplicated here — prefixing
body lines would break the `- [ ]` strip-prefix parse in
`derive_backlog_from_spec` (`types.rs:864`).

### UI (D5)

`scaffoldSpec` boolean in `useComposerState` (default **false**), rendered in
`NewWorkspaceComposerModal.tsx` only when `linkedWorkItem?.type === 'issue'`,
the URL is github.com, and the runtime target is local. After `createWorktree`
succeeds in **both** submit paths (`submit` `:2008`, `submitQuick` `:2196` —
extract one shared `maybeScaffoldSpecFromIssue(worktree, submitLinkedWorkItem)`
helper), call a new `scaffoldSpecFromIssue({ workdir: worktree.path, number,
slug? })` in `runtime/github-issue-client.ts` (or a small harness-client
addition). Failure → toast, **non-fatal** (the workspace stays usable; the
worktree is never rolled back). Remote-host worktrees are out of scope (the
toggle is hidden; the server's `is_dir` check 400s cleanly if reached) — same
class of local-first gap as `pane_env`.

### Unit-test plan

1. `spec_md_from_issue_preserves_checkboxes` — body checkboxes appear verbatim;
   round-trip: `derive_backlog_from_spec(spec_md_from_issue(...))` yields
   exactly the body's boxes (unchecked → Pending, checked → Done).
2. `spec_md_from_issue_synthesizes_fallback_ac` — checkbox-free body → one
   `- [ ] <title>` and the round-trip yields exactly one Pending feature.
3. `spec_md_from_issue_strips_control_chars_and_caps` — ESC/C1 removed, 64 KiB
   cap + marker.
4. `issue_spec_id_is_traversal_proof` — title `"../../etc/passwd"` →
   `"42-etc-passwd"`-style safe id; empty/symbol-only title → `"42-issue"`.
5. `plan_from_spec_with_tracker_stamps_provider_and_url` (tempdir): write a
   spec via the transform, plan, assert every feature carries
   `tracker_provider == Some("github")` + the URL — **the AC 7 closer**.
6. `plan_from_spec_delegation_unchanged` — existing `plan_from_spec` tests
   still green after the inner refactor.

---

## 6. Cross-cutting risks and invariants

- **Best-effort tracker contract (sacred, AC 5):** the GitHub arm returns only
  `Ok(Applied | Skipped)`, never `Err`; both callers already log every outcome
  (`drive.rs:324-337`). The drive loop's structure, transition points, and
  autonomy mechanics receive exactly one one-line diff.
- **One launch path:** nothing here touches `spawn_agent_into_pane`,
  `inject_prompt`, or settle detection.
- **Registry wipe hazard (F2):** `read_worktrees` collapses any parse error to
  `[]` (`worktrees.rs:79`) and the next write persists it — hence the hard
  rule: no serde aliases on the registry `Worktree` struct.
- **Auth:** all new routes ride the existing `require_token` merges
  (`lib.rs:296-300`); `is_public` untouched.
- **gh latency/rate:** 5 calls per transition, serialized, inline; a failed or
  slow `gh` degrades to `Skipped` + log, never a stall (each call is a bounded
  child process; consider `tokio::time::timeout(30s)` around `run_gh` — cheap
  insurance, recommended).
- **`status/qa*` coexistence (C4):** by construction the transition can never
  remove non-canonical `status/*` labels — pinned by test F1-3.
- **Serde compat:** `CreateBody` widening is purely additive with
  `#[serde(default)]` Options; old clients and the hosted-runtime RPC arm
  (`worktrees.ts:1078-1108`) keep working unchanged.

## 7. Build order (D4) and gates

| # | Feature | Done when (`verify.sh` = `cargo test -p agentum-server --lib` + `npm run build --prefix crates/agentum-desktop/ui`) |
|---|---|---|
| F1 | `github-status-transition` | Tests §2 green; `github_transition_is_a_logged_noop` replaced; both call sites compile with the widened arity; drive.rs diff is one line |
| F2 | `worktree-linked-metadata` | Tests §3 green; a `POST /api/worktrees/create` with linked fields round-trips into the registry + `{worktree}`; `detected` emits `linkedPR` |
| F3 | `composer-create-issue` | Tests §4 green; composer files an issue and renders the chip before any worktree exists (QA gate) |
| F4 | `spec-from-issue-scaffold` | Tests §5 green; QA: composer with linked issue + toggle → worktree contains `.agentum-harness/specs/<n>-<slug>/spec.md` + a backlog whose features carry the tracker URL; a demo run flips the labels and ends open with exactly `status/done` |

## Handoff to Developer (sdd-developer)

- **Completed:** all seams grounded and line-verified (pre-merge); C1–C5
  corrections; board_sync label-safety confirmed; D1–D5 honored.
- **Pending:** implementation F1 → F4 (each independently shippable).
- **Key decisions:** URL-authoritative slug+number in the GitHub arm; no
  `Host`/`gh_in_dir` in the seam (C5); no registry-struct aliases (wipe risk);
  F3 = new `POST /api/github/issues`; F4 = standalone
  `POST /api/harness/spec-from-issue` with keep-existing spec semantics.
- **First failing test to write:**
  `gh_set_status_label_argv_adds_one_removes_exactly_the_other_three`.
- **Reviewer focus:** the one-line drive.rs diff; `Ok(Skipped)`-never-`Err` in
  the GitHub arm; no `is_public` additions; the C3 wire keys.
