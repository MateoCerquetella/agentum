# Spec 010 — Architecture Blueprint: End-to-End Autonomous Flow (Projects v2)

**Self-check passed.** Load-bearing cites line-verified at v0.58.3
(`388eaa66`); the worktree merged origin/develop (v0.59.0) mid-phase →
base is now `664ee365`, and all cited seam anchors were re-checked
unchanged (`board_goals.rs:605`, `mcp.rs:1201`, `routes/harness.rs:425`,
`drive.rs:388`). D1–D8 honored. **Nine architect calls resolved (§7)** —
none reopens product scope; all decide *how/where* work lands.

- **Status:** Architect → ready for Developer.
- **Order:** F1 → F2 → F3 (D6). F1+F2 ship the headline value on a hand-bound
  repo; F3 may ship separately.

---

## 0. TL;DR — three slices, one sentence each

1. **F1 (foundation):** a new crate-root module `github_projects.rs` (the
   `linear.rs` precedent) owning `BoardBinding` (five required option-ID
   fields — an unmapped phase is *unrepresentable*), a one-call
   `gh api graphql` Status-field discovery, a pure exact-normalized fuzzy
   mapper with the two locked fallbacks, persistence in a **server-owned
   sibling file `github_projects.json`** (D2 = a2), CRUD routes in a new
   `routes/github_projects.rs`, and one shared mapping component mounted in
   Settings → Integrations (D7).
2. **F2 (headline value):** a `github_transition_with_board` wrapper INSIDE
   `task_sink.rs`'s github arm — the untouched `github_transition_with` label
   path plus an additive `github_projects::board_write_with` (ensure item →
   set option ID → knob-gated probe-then-close/reopen), with **zero call-site
   edits**, board failures folded into the *existing* `Skipped(String)`
   return so the run log stays loud through today's plumbing.
3. **F3 (born ready):** `POST /api/github/repo-from-template` +
   `POST /api/workspace/provision` (ONE injectable, idempotent
   `provision_repo` core: 5-label ensure, project link-**or**-create guarded
   by "binding exists ⇒ never create", F1 bind, `scaffold_harness`,
   consent-gated contract-files commit + plain push), wired into the wizard
   as a 4th `OPTIONAL_WORKSPACE_STEPS` entry + a modal-level `'provision'`
   phase — `useComposerState` untouched.

---

## 1. Boundaries & seams

| Feature | May touch | Must NOT touch |
|---|---|---|
| **F1** | NEW `crates/agentum-server/src/github_projects.rs`; NEW `routes/github_projects.rs` (+ one `.merge` in `lib.rs::router`, region `lib.rs:270–310`); NEW `ui/src/runtime/github-projects-client.ts`; NEW `ui/src/components/github-projects/ProjectBindingEditor.tsx`; NEW `ui/src/lib/github-projects-binding.ts` (pure); `components/settings/IntegrationsPane.tsx` (mount only) | `task_sink.rs` (F1 adds nothing there); `routes/github.rs`; the desktop `gh_projects.rs` read commands (reused as-is, registered at `agentum-desktop/src/lib.rs:497–498`); `github_labels.rs` (the clobber file — untouched by construction) |
| **F2** | `task_sink.rs` — ONLY: the github arm of `apply_tracker_transition` (:733–749), the github arm of `apply_blocked_transition` (:783–806), one new private `github_transition_with_board`, a doc-widening on `TransitionResult::Skipped`; `github_projects.rs` (the write machinery + id cache) | `github_transition_with` (:621 — byte-identical, AC 8), `github_mark_blocked_with` (:654 — byte-identical), `gh_set_status_label_argv`/all argv builders, ALL four call sites (`drive.rs:378–411` wrapper + :129/:207/:268/:321; `board_goals.rs:605`; `routes/harness.rs:425`; `mcp.rs:1201`), `TrackerPhase` (4 variants), `TransitionResult` variants, any spawn-path code |
| **F3** | NEW `routes/provision.rs` (+ `.merge`); NEW `ui/src/lib/workspace-provision-step.ts`; `ui/src/lib/workspace-goal-step.ts` (the `OPTIONAL_WORKSPACE_STEPS` extension + template-mode pure fns); `NewWorkspaceGoalStep.tsx` (mode toggle); `NewWorkspaceComposerModal.tsx` (phase machine `'goal'→'provision'→'details'`); NEW `NewWorkspaceProvisionStep.tsx`; `github-projects-client.ts` (+2 fns); two `pub(crate)` visibility widenings in `task_sink.rs` (`gh_label_ensure_argv` :452, `github_status_color` :305) | `useComposerState.ts` internals (props-only, the 008 F3 contract — the modal already feeds it via `initialName/initialRepoId/…`, `NewWorkspaceComposerModal.tsx:180–202`); `scaffold_harness` (`types.rs:678` — wrapped, not edited); `harness/types.rs` `FeatureList` (NO new knobs — `done_closes_issue` lives in the binding); the composer submit flow; `isGoalStepReady` (:81) |

**Untouchable everywhere:** `spawn_agent_into_pane` and all autonomy
mechanics; the label canon (exactly-one-`status/*`, five names, `status/qa*`
never touched); push-based streaming; `routes/mcp.rs` (gains F2 free through
the seam); the desktop write stubs `gh_update_project_item_field` /
`gh_clear_project_item_field` (`gh.rs:1046/:1051` stay `not_available()` —
writes are server-side so harness/MCP work headless in the installed app).

---

## 2. D2 — the persistence decision: (a2) sibling `github_projects.json`

**Chosen: a server-owned, single-writer sibling file** at
`<data_local_dir|data_dir>/Agentum/github_projects.json`, env-overridable via
`AGENTUM_GITHUB_PROJECTS_CONFIG` — the exact `linear.rs::creds_path` /
`task_sink::github_config_path` (:366) pattern.

**Why not (a1) `github.json` + passthrough:** the hazard is verified —
desktop `github_labels.rs::update_config` (:60–71) round-trips a typed
`GithubConfig { state_map }`; serde drops unknown keys, and its `STORE_LOCK`
(:38) is module-local. Fixing it needs a desktop passthrough field, a
preserves-bindings regression test, AND still leaves two uncoordinated
writers (the desktop Settings saver + the server bind route) doing
read-modify-write on one file. Most moving parts, real residual race,
touches the one file the constraint says must never destroy a binding.

**Why not (a3) store table:** reusing `agentum_core::TrackerBinding`
(`lib.rs:610`) is verified NOT clean — `board_tracker_bindings` is consumed
by the board-sync pull engine (`routes/board_sync.rs:34–45` hangs
`POST /bindings/{id}/sync` off that resource; `list_bindings` feeds its UI).
Stuffing Projects bindings into it makes them appear as pull-able
board-mirror bindings — a behavioral regression. So (a3) degrades to "new
migration + repository methods + core type + wire DTOs" — cost without the
reuse. The PM's "seam already has `&Store`" is true but not differentiating:
the github arm today reads config *inside the arm*
(`GithubStateMap::from_env()` at :747) without the store; the binding read is
the same move.

**Why (a2) wins:** clobber-immune **by construction** (Settings label saves
touch `github.json`; nothing else writes `github_projects.json`); exact match
to the strongest precedent (defaults → file → env layering, absent/garbled →
default, fresh read per transition = the documented "applies on the next
transition, no restart" contract); zero migration; hermetic tests by
injection (the `apply_layers` technique — never env mutation); and
human-debuggable (`cat`/delete the file), which matters for a solo dogfooder
diagnosing a board that didn't move. Known residual (§6.5): two embedded
servers (desktop + TUI) could theoretically race a write — out-of-profile
(the TUI has no bind UI; all writes flow through the desktop's embedded
server or a single networked daemon), documented, with "promote to a store
table" as the named escalation if multi-client binding ever materializes.

**The API (in `github_projects.rs`):**

```rust
fn github_projects_config_path() -> Option<PathBuf>;          // env override → data dir
fn read_bindings_at(path: &Path) -> GithubProjectsFile;       // absent/garbled → Default (empty)
pub fn binding_for_slug(slug: &str) -> Option<BoardBinding>;  // fresh read; slug lowercased
pub fn upsert_binding(slug: &str, b: BoardBinding) -> Result<(), String>;  // WRITE_LOCK'd RMW
pub fn remove_binding(slug: &str) -> Result<bool, String>;

#[derive(Debug, Default, Serialize, Deserialize)]
struct GithubProjectsFile {
    #[serde(default)]
    bindings: std::collections::BTreeMap<String, BoardBinding>, // key = lowercase "owner/repo"
}
static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(()); // github_labels.rs pattern, server-side
```

File format (snake_case, server-owned; routes expose camelCase DTOs):

```json
{ "bindings": { "acme/widgets": {
    "project_id": "PVT_kwDO...", "status_field_id": "PVTSSF_...",
    "status_mapping": { "todo": "f75ad846", "in_progress": "47fc9ee4",
      "ready_to_test": "aba1c2e6", "done": "98236657", "blocked": "47fc9ee4" },
    "done_closes_issue": true,
    "project_title": "Widgets", "project_owner": "acme",
    "project_owner_type": "organization", "project_number": 7,
    "option_names": { "todo": "Backlog", "in_progress": "Building",
      "ready_to_test": "QA", "done": "Shipped", "blocked": "Building" } } } }
```

---

## 3. F1 seam design — bind

### 3.1 Types + the constructor invariant

```rust
// github_projects.rs (new crate-root module — the linear.rs precedent:
// domain logic at root, task_sink calls in, routes layer separate).

/// The five-phase board vocabulary. LOCAL to the projects layer —
/// `TrackerPhase` stays four variants (008 D-A stands).
pub enum BoardPhase { Todo, InProgress, ReadyToTest, Done, Blocked }
impl From<crate::task_sink::TrackerPhase> for BoardPhase { /* 4 arms */ }

/// One single-select OPTION ID per canonical phase. Five REQUIRED String
/// fields — an unmapped phase is unrepresentable by type. A stored file
/// missing any phase fails deserialization → reads as "no binding", so a
/// partial binding can never exist on disk either (AC 2d).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusMapping {
    pub todo: String, pub in_progress: String, pub ready_to_test: String,
    pub done: String, pub blocked: String,
}
impl StatusMapping { pub fn option_id(&self, phase: BoardPhase) -> &str; }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardBinding {
    pub project_id: String,
    pub status_field_id: String,
    pub status_mapping: StatusMapping,
    /// D1 knob. THE default materializes HERE (serde default = true): a
    /// binding written without it reads ON; the UI renders the stored value
    /// and writes it explicitly — one definition site.
    #[serde(default = "default_true")]
    pub done_closes_issue: bool,
    // Display/round-trip metadata (names are NEVER used at write time — IDs only):
    #[serde(default)] pub project_title: Option<String>,
    #[serde(default)] pub project_owner: Option<String>,
    #[serde(default)] pub project_owner_type: Option<String>, // "user"|"organization"
    #[serde(default)] pub project_number: Option<i64>,
    #[serde(default)] pub option_names: Option<StatusNames>,   // same 5-field shape, names
}
```

### 3.2 Discovery — ONE `gh api graphql` call

Server-side, from `task_sink::neutral_cwd()`, explicit `program: &str`
(finding 6). The picker always supplies `ownerType`, so the happy path is
exactly one call (probe org-then-user only when it's absent — the
`gh_resolve_project_ref` candidates loop, `gh_projects.rs:608–654`).

```rust
pub struct StatusOption { pub id: String, pub name: String }
pub struct DiscoveredStatusField {
    pub project_id: String, pub project_title: String,
    pub status_field_id: String, pub options: Vec<StatusOption>,
}
pub async fn discover_status_field(
    program: &str, owner: &str, owner_type: &str, number: i64,
) -> Result<DiscoveredStatusField, ProjectsError>;
```

The query (owner node = validated `"organization"`/`"user"`, interpolated as
the root field exactly like `gh_projects.rs::owner_node` :182; the login is
ALWAYS a `$var`):

```graphql
query($owner: String!, $number: Int!) {
  organization(login: $owner) {        # or user(login: $owner)
    projectV2(number: $number) {
      id
      title
      field(name: "Status") {
        __typename
        ... on ProjectV2SingleSelectField { id name options { id name } }
      }
    }
  }
}
```

argv (pure builder `gh_graphql_argv`, pinned by test — the `-f`/`-F`
discipline copied from `gh_projects.rs::graphql` :136–149):

```
["api","graphql","-f","query=<Q>","-f","owner=<owner>","-F","number=<n>"]
```

Runner: `run_gh_graphql(program, query, vars) -> Result<Value, ProjectsError>`
— `tokio::process::Command`, `neutral_cwd()`, **30 s timeout** (mirror
`run_gh` :585–611), parse stdout JSON; a top-level `errors[]` → classifier;
no JSON → classify stderr. `parse_discovery(&Value) -> Result<…>` is pure
(fixture-tested). A `field` that is null / not single-select →
`ProjectsError { kind: "no_status_field", … }`.

### 3.3 The scope probe + actionable error

No separate probe call — **the discovery call IS the probe**; classification
does the work (mirrors `gh_projects.rs::classify_stderr` :100–123 +
`classify_graphql_errors`):

```rust
pub struct ProjectsError { pub kind: &'static str, pub message: String }
// kinds: scope_missing | auth_required | not_found | no_status_field
//        | network_error | unknown
```

The `scope_missing` message is CONSTRUCTED to contain the remedy:
`"GitHub Projects needs the `project` token scope. Run: gh auth refresh -s project"`.
The bind/discover routes return it as a typed envelope
(`ApiError::Custom(422, {"error":{"code":"scope_missing","message":…}})` —
the `no_github_repo` precedent, `routes/github.rs:239–244`) so the UI can
render the command verbatim. Mid-run (F2) the same classifier's message goes
to the log-and-continue path — never a silent skip.

### 3.4 The pure fuzzy mapper

**Normalization:** lowercase → keep only `[a-z0-9]` (spaces, dashes,
underscores, emoji, punctuation all stripped). `"🚧 In-Progress "` →
`"inprogress"`.

**Matching: exact-normalized synonym match only — NO substring** (substring
is unsafe: `"notstarted"` contains `"started"`, `"notdone"` contains
`"done"`). Misses go to fallback or D7's manual selects — the recovery path
exists precisely for this. Each phase scans options in discovery order and
takes the first synonym hit. The lists (normalized tokens, **disjoint by
construction** — pinned by a structural test):

| Phase | Synonyms (normalized) |
|---|---|
| Todo | `todo, backlog, new, triage, inbox, upnext, planned` |
| InProgress | `inprogress, doing, building, wip, started, active, development, indevelopment, coding` |
| ReadyToTest | `readytotest, readyfortest, qa, testing, test, review, inreview, readyforreview, verify, verification, staging` |
| Done | `done, shipped, complete, completed, finished, closed, merged, released` |
| Blocked | `blocked, stuck, onhold, hold, waiting, paused` |

**Fallbacks (the only two, per AC 1):** `ReadyToTest → InProgress`'s resolved
option; `Blocked → InProgress`'s resolved option. **Refusal:** if `Todo`,
`InProgress`, or `Done` has no match, return `Err` naming the unmapped
phase(s) AND the discovered option names — never a partial mapping; the UI
turns a refusal into manual per-phase selects (D7).

```rust
pub enum MatchVia { Matched, FellBack }   // FellBack ⇒ render the D5 hint
pub struct ResolvedPhase { pub option_id: String, pub option_name: String, pub via: MatchVia }
pub struct ResolvedMapping { /* five ResolvedPhase fields */ }
pub fn resolve_status_mapping(options: &[StatusOption]) -> Result<ResolvedMapping, String>;
```

AC 2's four fixtures are the contract: (a) `Todo/In Progress/Done` → three
`Matched` + RTT/Blocked `FellBack` to the In Progress option; (b)
`Backlog/Building/QA/Shipped` → `backlog→Todo`, `building→InProgress`,
`qa→ReadyToTest`, `shipped→Done`, Blocked `FellBack`; (c) no RTT-like column
→ `FellBack`; (d) gh failure / missing Status field / missing scope →
`ProjectsError`, never a partial binding.

### 3.5 Routes — NEW `routes/github_projects.rs`

Resolved call: a **new file**, not `routes/github.rs` — github.rs is ~590
lines of issue surface; the projects domain gets its own module (the
`git.rs`-decomposition precedent). Registered with one `.merge` next to
`routes::github::router()` (`lib.rs:308`). All routes authed (no `is_public`
changes). Slug resolution reuses `board_goals::resolve_github_slug` via
`{workdir, slug?}` exactly like every github.rs route.

```
POST   /api/github/project-binding/discover   { owner, ownerType, number }
       → 200 { projectId, title, statusFieldId, options: [{id,name}],
               resolved: { todo:{optionId,name,via}, … } | null,
               unmappedPhases: ["ready_to_test", …] }
       → 422 typed envelope on scope_missing / classified 400 otherwise
GET    /api/github/project-binding?workdir=…&slug=…
       → 200 { slug, binding: BindingDto | null }
PUT    /api/github/project-binding
       { workdir, slug?, projectId, statusFieldId,
         statusMapping: {todo,inProgress,readyToTest,done,blocked},
         doneClosesIssue?, projectTitle?, projectOwner?, projectOwnerType?,
         projectNumber?, optionNames? }
       → validates all five option IDs non-empty (constructor invariant at
         the wire too; 400 otherwise) → upsert → 200 { slug, binding }
DELETE /api/github/project-binding?workdir=…&slug=…  → 204
```

`BindingDto` is a camelCase wire twin of `BoardBinding` (the file stays
snake_case; the route maps).

### 3.6 The shared mapping UI (D7) + settings mount

- **Component:** `components/github-projects/ProjectBindingEditor.tsx`,
  props `{ workdir: string, slug?: string, onBound?: (b) => void }`. Flow:
  project picker (**reuse the registered desktop read commands** —
  `gh_list_accessible_projects` for the list, `gh_resolve_project_ref` for a
  pasted URL/`owner/number`; both real + registered at
  `agentum-desktop/src/lib.rs:497–498`) → `POST …/discover` → five per-phase
  selects populated with the discovered option names, pre-selected from
  `resolved`; a `FellBack` phase renders a visible hint chip (D5: "no
  'Ready to Test'-like column — falls back to In Progress; add one and
  re-discover"); a refusal renders empty selects + the error — a prompt to
  finish manually, never a dead end (D7). Plus the `done_closes_issue`
  toggle (renders the stored/serde default = ON), Save (PUT), Re-discover,
  Unbind.
- **Client:** NEW `runtime/github-projects-client.ts` — `discoverProjectStatus`,
  `getProjectBinding`, `putProjectBinding`, `deleteProjectBinding` — the
  `apiUrl` + `authHeaders` + typed-fetch pattern copied from
  `github-issue-client.ts:14–58`.
- **Pure module:** `lib/github-projects-binding.ts` — select-state reducer,
  `mappingComplete(selected): boolean`, fallback-hint derivation — vitest'able
  without jsdom (the UI package ships none).
- **Settings mount (D7 resolved):** `IntegrationsPane.tsx`'s GitHub card
  gains a "Projects v2 board" section: a repo selector over the app's repo
  list (a binding is per-repo; settings is global — the selector bridges
  that) → `ProjectBindingEditor` for the picked repo. This is the
  wizard-independent surface F2 dogfoods on agentum's own repo; F3's wizard
  step mounts the SAME component later.

---

## 4. F2 seam design — drive (zero call-site edits)

### 4.1 Where the arm hooks

`apply_tracker_transition`'s github arm (`task_sink.rs:733–749`) becomes:

```rust
"github" => {
    // (unchanged) tracker_url guard → github_slug_and_number_from_issue_url
    // (unchanged comment) map resolved only AFTER the parse succeeds…
    let map = GithubStateMap::from_env();
    // Spec 010 F2: the binding read follows the SAME hermeticity discipline —
    // only after the parse, so the no-url/unparseable skip tests never touch
    // the config files.
    let binding = crate::github_projects::binding_for_slug(&slug);
    Ok(github_transition_with_board(&gh_bin(), &slug, &number, phase, &map, binding.as_ref()).await)
}
```

```rust
/// Label transition + (when bound) the ADDITIVE Projects write. The label
/// path is the byte-identical `github_transition_with` (AC 8); a board
/// failure can only append to the report, never alter label behavior, and
/// NEVER becomes an `Err` (AC 7).
async fn github_transition_with_board(
    program: &str, slug: &str, number: &str, phase: TrackerPhase,
    map: &GithubStateMap, binding: Option<&crate::github_projects::BoardBinding>,
) -> TransitionResult {
    let label = github_transition_with(program, slug, number, phase, map).await;
    let Some(b) = binding else { return label };            // unbound = today, byte-for-byte
    match crate::github_projects::board_write_with(program, b, slug, number, phase.into()).await {
        Ok(()) => label,
        Err(reason) => {
            tracing::warn!(slug, number, ?phase, %reason, "Projects board write failed (non-fatal)");
            match label {
                TransitionResult::Applied =>
                    TransitionResult::Skipped(format!("status label applied; Projects board write failed: {reason}")),
                TransitionResult::Skipped(why) =>
                    TransitionResult::Skipped(format!("{why}; Projects board write failed: {reason}")),
            }
        }
    }
}
```

**Best-effort logging shape (resolved call):** the seam is workdir-less AND
engine-less — it cannot emit `HarnessEvent::Log` without a signature change,
which AC 4 forbids. Resolution: (i) `tracing::warn` ALWAYS (the daemon log);
(ii) run-context visibility rides the **existing** return-value plumbing — a
board failure folds into `Skipped(reason)`, which `drive.rs`'s
`transition_tracker` (:400–404) already turns into
`engine.log(… "ticket transition to X skipped: status label applied; Projects
board write failed: …")` and the MCP tool turns into its `skipped:` text
(`mcp.rs:1191`). Zero call-site edits, zero new variants, loud in the run
log (the 008 never-silent doctrine). The one semantic bend — `Skipped` when
the label DID apply — is self-describing in the reason string; widen
`TransitionResult::Skipped`'s docstring from "nothing to do" to "not fully
applied; the reason names what did and didn't land" (doc-only; no test
asserts the docstring).

`apply_blocked_transition`'s github arm (:783–806) gets the same pattern:
after `github_mark_blocked_with` (untouched), read the binding (post-parse),
call `board_write_with(…, BoardPhase::Blocked)`, combine identically (AC 5).
No close/reopen on Blocked.

### 4.2 The board write (`github_projects::board_write_with`)

```rust
/// One transition's whole board side. Best-effort: Ok(()) or a reason string
/// the caller folds into TransitionResult — never Err-propagates, never panics.
pub async fn board_write_with(
    program: &str, binding: &BoardBinding, slug: &str, number: &str, phase: BoardPhase,
) -> Result<(), String>;
```

Steps:
1. **ids** — cache lookup `(slug.lower(), number) → (issue_node_id, item_id)`;
   miss ⇒ resolve + ensure (below), populate cache.
2. **issue node id** (cold only):
   `query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){issue(number:$number){id}}}`
   — GraphQL (not REST) so ONE runner + ONE classifier serve every call.
3. **ensure-on-board + item id, one call** (cold only; PM finding 3):
   `mutation($project:ID!,$content:ID!){addProjectV2ItemById(input:{projectId:$project,contentId:$content}){item{id}}}`
   — idempotent by API contract; re-adding returns the existing item's id.
   This is also what makes a chat-filed issue "land in the Todo column"
   (AC 11 / PRD §5) — the Todo transition's lazy ensure.
4. **the option write** (every call):
   `mutation($project:ID!,$item:ID!,$field:ID!,$option:String!){updateProjectV2ItemFieldValue(input:{projectId:$project,itemId:$item,fieldId:$field,value:{singleSelectOptionId:$option}}){projectV2Item{id}}}`
   with `$option = binding.status_mapping.option_id(phase)` — **option IDs,
   never names** (PRD AC 6; renames after bind still land).
5. **stale-cache self-heal:** if step 4 fails on a *cached* item id →
   invalidate the entry and retry ONCE cold (steps 2–4). Correctness never
   depends on the cache (resolved call §7.3).
6. **close/reopen (D1/AC 6), gated on `binding.done_closes_issue`:**
   - `Done` → probe `gh issue view N --repo slug --json state --jq .state`;
     `OPEN` ⇒ `gh issue close N --repo slug`; `CLOSED` ⇒ no-op.
   - `InProgress` → probe; `CLOSED` ⇒ `gh issue reopen N --repo slug`;
     `OPEN` ⇒ no-op.
   - Probe failure ⇒ skip the close/reopen with a warn (best-effort).
   **Probe-then-act for BOTH directions** (resolved call §7.4): blind reopen
   would exit non-zero on every already-open InProgress (log spam — the PM's
   point); the symmetric probe also silences already-closed closes, and with
   the knob OFF neither probe runs (we never closed, so we never reopen —
   a human-closed issue on a knob-off binding is respected).

**The id cache (resolved call §7.3):** ship it —
`static ID_CACHE: Lazy<Mutex<HashMap<(String,String),(String,String)>>>`,
process-lifetime, no TTL (issue node ids are immutable; item ids die only if
a card is removed from the board, which step 5 heals). **Volume:** per
bound feature run ≈ Todo cold (3) + InProgress (1 + probe 1) + ReadyToTest
(1) + Done (1 + probe 1 + close 1) = **9 ≤ ~10** ✅; without the cache the
same run is ~14 — the spec's ceiling effectively requires it, so it ships
in F2 (not deferred), with the invalidate-retry rule keeping it
correctness-free.

All builders are pure and unit-pinned:
`issue_node_id_query_args(owner, name, number)`,
`add_item_mutation_args(project_id, content_id)`,
`update_status_mutation_args(project_id, item_id, field_id, option_id)`,
`gh_issue_close_argv / gh_issue_reopen_argv / gh_issue_state_argv`.
`run_gh_capture(program, args) -> Result<String, String>` (a stdout-carrying
sibling of `run_gh` — `task_sink::run_gh` :585 stays untouched since it
discards stdout).

### 4.3 What the six transition points get, with zero edits

| Call site | Phase(s) | Board effect (bound repo) |
|---|---|---|
| `drive.rs:129` via `transition_tracker` :378 | InProgress | card → InProgress option; reopen if closed (knob ON) |
| `drive.rs:207` | ReadyToTest | card → mapped option (custom "QA" columns included) |
| `drive.rs:268` | Done | card → Done option; issue closed (knob ON) |
| `drive.rs:321` `apply_blocked_transition` | Blocked | card → Blocked-mapped option + today's label+comment |
| `board_goals.rs:605`, `harness.rs:425` | Todo | lazy ensure-on-board + Todo option (chat-filed issue lands in Todo) |
| `mcp.rs:1201` `agentum_report_status` | any | same seam ⇒ same writes, free |

---

## 5. F3 seam design — provision

### 5.1 Server: two routes, one injectable core — NEW `routes/provision.rs`

```
POST /api/github/repo-from-template
  { owner, name, templateRepo, directory, visibility? ("private"|"public", default private) }
  → 200 { slug, path, created: bool }

POST /api/workspace/provision
  { workdir, slug?,
    project: { owner, ownerType, number } | { create: true, owner, ownerType, title },
    statusMapping?: {…5 ids…}, doneClosesIssue?: bool,
    commitScaffold: bool }                    // D8 consent — explicit on the wire
  → 200 ProvisionReport
```

**Template mode** (`create_repo_from_template(program, owner, name, template,
directory, visibility)`):
1. `target = directory/name`; `target/.git` exists ⇒ `{created:false}`
   (local idempotency).
2. probe `gh repo view <owner>/<name> --json nameWithOwner`:
   - missing ⇒ `["repo","create","<owner>/<name>","--template",template,
     "--private"|"--public","--clone"]` run with cwd = `directory`
     (gh clones into `./name`);
   - exists ⇒ `["repo","clone","<owner>/<name>"]` (cwd = directory) — the
     AC-10 "template-create skipped when the repo exists" rule.
3. return `{slug, path, created}`. D4: `templateRepo` defaults to
   `goempirical/empirical-sdd-ddd-starter` **in the UI constant**, editable;
   owner is the wizard's explicit choice.

**The ONE idempotent ensure** — everything injectable for the run-twice test
(the finding-6 discipline, extended to the bindings path):

```rust
pub(crate) struct ProvisionCtx<'a> {
    pub program: &'a str,                    // fake-gh injection
    pub bindings_path: Option<&'a Path>,     // None → default file; Some → test temp file
    pub workdir: &'a Path, pub slug: &'a str,
    pub project: ProjectChoice,              // Link{…} | Create{…}
    pub status_mapping: Option<StatusMapping>,
    pub done_closes_issue: bool, pub commit_scaffold: bool,
}
pub(crate) async fn provision_repo(ctx: ProvisionCtx<'_>) -> ProvisionReport;
// ProvisionReport { labels, project, binding, scaffold: StepReport, commit: CommitReport }
// StepReport { ok: bool, changed: bool, detail: String }
// CommitReport { committed: bool, pushed: bool, branch: String, error: Option<String> }
```

Steps (each independent, best-effort, per-step reported; only request-shape /
missing-workdir errors are hard 4xx):
1. **Labels:** a 5-ensure loop — the four configured pipeline names via
   `gh_label_ensure_argv` (colors by phase via `github_status_color`) + the
   fixed `GITHUB_BLOCKED_LABEL` ensure. **Deliberately its own loop** over
   `pub(crate)`-widened builders — refactoring `github_transition_with`'s
   4-ensure sequence would change the pinned 5-invocation fake-gh test
   (AC 8 forbids). `--force` makes re-runs no-ops (AC 10 "no duplicate
   labels" holds by `gh` contract).
2. **Project link-or-create, guarded by the binding:** if
   `binding_for_slug(slug)` (at `ctx.bindings_path`) is `Some` ⇒
   `changed:false, "already bound"` and **skip create+bind entirely** — THE
   idempotency rule ("no second project"). Else: `Link` ⇒ F1 discovery;
   `Create` ⇒ `["project","create","--owner",owner,"--title",title,
   "--format","json"]` → parse `{number,…}` → F1 discovery (the created
   board carries GitHub's defaults Todo/In Progress/Done; the mapper's two
   fallbacks resolve it, `FellBack` visible per D5) → apply
   `ctx.status_mapping` override if present → constructor → upsert binding.
3. **Scaffold:** `scaffold_harness(workdir)` (`types.rs:678`) untouched —
   keep-existing already idempotent; report the `written` list
   (empty on rerun ⇒ `changed:false`).
4. **Commit (only when `commit_scaffold`; D8):**
   - rewrite `.agentum-harness/.gitignore` from the scaffold's blanket `*`
     to a **state-only ignore** (write-if-different):
     `feature_list.json`, `handoff.md`, `qa/` — the engine-written runtime
     state stays out of commits (the exact noise the self-ignore exists to
     prevent, `types.rs:715–723`), while the CONTRACT files become
     committable;
   - `git -C workdir add .agentum-harness/.gitignore …/AGENTS.md …/init.sh
     …/verify.sh …/qa.sh` (plain add — the rewritten ignore no longer covers
     them);
   - `git status --porcelain -- .agentum-harness` empty ⇒ `changed:false`,
     **no commit** (the AC-10 unchanged-commit-count mechanism);
   - `git commit -m "chore: provision agentum harness scaffold"` — **no
     AI-attribution trailer** (D8 + the standing repo rule);
   - `git push origin HEAD` — **plain, never `--force`**; a red push ⇒
     `pushed:false` + `error` surfaced, workspace stays usable (non-fatal,
     D8). The commit lands on the workdir's **current branch**
     (`git rev-parse --abbrev-ref HEAD`, reported in `CommitReport.branch`
     and displayed by the consent UI) — provisioning never checks out or
     switches branches (the concurrent-agents rule); template clones are on
     the default branch by construction.

Local-host only (like the harness routes); `expand_workdir` + `is_dir`
guards; git via `tokio::process::Command` with `-C workdir`.

### 5.2 Wizard wiring — props-only, `useComposerState` untouched

**Resolved calls (handoff §"Open architect calls", items 6 + 8):**

- **Provision is a FOURTH `OPTIONAL_WORKSPACE_STEPS` entry**, not a mode of
  `tracker`/`worktree` (`workspace-goal-step.ts:114`): append
  `{ id: 'provision', label: 'Provision repo (labels, board, scaffold)',
  skippable: true, primitive: 'provisionWorkspace' }`; widen
  `OptionalWorkspaceStepId` (:103) with `'provision'`. The tracker step
  files an *issue*; provisioning ensures *repo infrastructure* — distinct
  concerns, and the typed data table is the designed extension seam
  (PM finding 5).
- **Template mode produces the repoId inside the goal step's Continue**,
  *before* `isGoalStepReady` is consulted — `isGoalStepReady` (:81) and
  `GoalStepInputs` (:78) stay untouched. `NewWorkspaceGoalStep.tsx` gains a
  mode toggle: **"Existing project"** (today's `RepoCombobox`, byte-identical)
  | **"New repo from template"** ({owner, name ← seeded by
  `deriveTemplateRepoName(goal)` = `slugifyGoalName`, template ← the D4
  default constant, editable, directory picker, visibility}). Template-mode
  Continue: `createRepoFromTemplate(…)` (spinner; inline error on failure —
  never silent) → register the cloned path as an app repo through the SAME
  underlying store action the existing `add-repo` modal uses
  (`RepoCombobox.tsx:194` opens `'add-repo'`; the developer traces its
  submit to the concrete action and reuses it) → `onContinue(goal,
  newRepoId)`. The composer receives the repoId via the existing
  `seedRepoId` prop path (`NewWorkspaceComposerModal.tsx:97–102, :191`).
- **The D8 consent renders in a new modal-level `'provision'` phase**
  between `'goal'` and `'details'` — `NewWorkspaceComposerModal.tsx`'s phase
  state (:95) widens to `'goal' | 'provision' | 'details'`. NEW
  `NewWorkspaceProvisionStep.tsx` mounts: the **shared**
  `ProjectBindingEditor` (link-or-create project + mapping, F1's component —
  D7's second mount) + the consent checklist (labels / project+bind /
  scaffold / **commit — default ON, explicitly toggleable, naming the target
  branch and listing the exact five committed paths** from the pure
  `provisionCommitFileList()`), a "Provision & continue" button (calls
  `provisionWorkspace`, renders per-step `ProvisionReport` results inline;
  failures are warnings, never blockers) and "Skip" (straight to details).
  The provision phase is offered only on the goal-first path; opinionated
  opens (`initialComposerPhase` :161 — untouched) still skip to details, and
  existing workspaces use the Settings mount. `QuickTabBody` and every
  `useComposerState` prop stay byte-identical.

New pure module `lib/workspace-provision-step.ts`:
`provisionCommitFileList()`, `mappingComplete(selected)`,
`summarizeProvisionReport(report)`, `deriveTemplateRepoName(goal)` — all
vitest'able without jsdom.

---

## 6. Tradeoffs, risks, invariants

1. **`gh` token scope (top risk):** bind-time = the discovery call fails →
   classified `scope_missing` → the actionable `gh auth refresh -s project`
   message on a typed 422 (AC 2d). Mid-run = the same classifier's reason
   folds into the `Skipped` note + `tracing::warn` (AC 7) — visible in the
   run log, never silent, never fatal. Residual: a user who revokes the
   scope after binding gets a noisy-but-correct run log; the fix is
   self-describing.
2. **GraphQL shape drift:** all five operations (`projectV2`,
   `field(name:)`, `ProjectV2SingleSelectField`, `addProjectV2ItemById`,
   `updateProjectV2ItemFieldValue`) are GA-stable since 2022 and already
   half-used by the desktop read surface. Every parse is a pure fn with
   canned-JSON fixture tests, so a drift = one fixture update; every runtime
   miss degrades to a classified, logged reason (best-effort holds).
3. **Embedded-app behavior:** ALL writes are server-side (`gh` from
   `neutral_cwd()` on the daemon/embedded process) — headless-identical for
   harness + MCP, the spec-007 stub lesson applied. The desktop read
   commands are used ONLY for picking. `gh`-on-PATH in the installed app is
   an existing exposure (issue creation already rides it), not a new one.
4. **Idempotency edges:** item-add idempotent by API; project-create guarded
   by binding-exists (the single rule the run-twice test pins); labels
   `--force`; scaffold keep-existing; commit gated on porcelain-empty;
   template-create skipped when the repo/clone exists. Edge: a bound project
   deleted on GitHub ⇒ writes fail logged; recovery = re-discover/re-bind in
   the editor. Edge: two worktrees of one repo share one binding (keyed by
   slug) — correct by design.
5. **Clobber residual under (a2):** zero by construction for the named
   hazard (Settings label saves write `github.json`; bindings live in
   `github_projects.json`, single-writer server-side behind `WRITE_LOCK`).
   Documented residual: two embedded servers (desktop + TUI) writing
   bindings concurrently could lose one RMW — out-of-profile (the TUI has no
   bind surface; a lost write is re-bindable); named escalation = promote to
   a store table if multi-client binding ever ships.
6. **The `Skipped`-with-label-applied semantic bend (§4.1):** deliberate —
   the only zero-call-site way to make board failures loud through existing
   plumbing. Self-describing reason strings; doc-widened variant docstring;
   the AC-7 test pins the exact shape.
7. **Human drags (D3):** overwritten at the next transition — no polling, no
   echo machinery, nothing to build (verified: no inbound sync exists,
   `board_sync.rs:14`).
8. **`.gitignore` rewrite (F3 commit path only):** contract files become
   tracked; engine-written state (`feature_list.json`, `handoff.md`, `qa/`)
   stays ignored — the worktree-noise property the blanket `*` protected is
   preserved for everything the engine writes. Repos provisioned without the
   commit step keep the blanket `*` untouched.

**Protected invariants confirmed untouched:** one launch path (no spawn-path
file is edited); YOLO translation; push-based streaming; the label canon
(`github_transition_with`, `github_mark_blocked_with`, and every argv builder
byte-identical — AC 8's "existing label tests stay green unmodified" holds
because no tested function changes); the best-effort tracker contract
(`Ok`-never-`Err` extended to every board write); `TrackerPhase` stays four
variants (`BoardPhase` is local to the projects layer); no `is_public`
additions; desktop write stubs stay dead.

---

## 7. Resolved architect calls (every call named in the handoff)

1. **D2 mechanism → (a2) sibling `github_projects.json`.** The verified
   clobber hazard (`github_labels.rs:60–71` drops unknown keys; module-local
   lock :38) makes (a1) the most-moving-parts option with residual race even
   after a passthrough; (a3)'s "reuse `TrackerBinding`" is disqualified by
   verified coupling (the board-sync pull engine consumes that table/resource,
   `board_sync.rs:34–45`), degrading it to a new migration + repository +
   DTO surface for a single-blob-per-slug lookup. The sibling file is
   clobber-immune by construction, matches the strongest precedent
   (`linear.rs:60–73` / `task_sink.rs:366–381` — path fn, env override,
   absent→default, fresh-read-per-transition freshness), costs zero
   migration, tests by injection, and is solo-dogfooder-debuggable.
2. **Fuzzy internals → strip-to-alphanumeric normalization + disjoint
   exact-match synonym tables + exactly the two locked fallbacks.** No
   substring matching (false positives like "not done"/"not started" are
   worse than a miss); a miss on Todo/InProgress/Done refuses with the
   unmapped phase + option names (never partial, AC 2d), and D7's manual
   selects are the designed recovery. AC 2's four fixtures all pass under
   exact matching; a structural test pins list disjointness.
3. **Id cache → ship it in F2** (process-lifetime `(slug, number) →
   (node_id, item_id)` map) **with invalidate-and-retry-once on a stale-id
   failure**, keeping correctness cache-independent as required. The spec's
   own ≤ ~10-calls-per-run ceiling is only met with it (9 warm vs ~14 cold).
4. **Reopen → probe-then-act, both directions, gated on
   `done_closes_issue`.** Adopts the PM's probe recommendation and extends
   it symmetrically to close: one `gh issue view --json state` probe kills
   all exit-nonzero noise (blind reopen would red-log every ordinary
   InProgress), and gating on the knob means a knob-OFF binding never
   probes, never closes, never reopens — a human-closed issue is respected.
5. **Route home → new `routes/github_projects.rs` (F1) + new
   `routes/provision.rs` (F3); domain logic in a crate-root
   `github_projects.rs`.** `routes/github.rs` stays an issue surface; the
   crate-root module mirrors `linear.rs` (task_sink calls a provider module,
   routes stay thin) — which is also what keeps the F2 arm a two-line hook.
6. **Template repoId flow → repo creation completes inside the goal step's
   template-mode Continue,** producing the registered repoId before
   `onContinue(goal, repoId)` fires — so `GoalStepInputs.repoId` (:78) stays
   required and `isGoalStepReady` (:81) is never edited. The clone registers
   through the same store action the existing add-repo modal uses; the
   composer receives the id via the existing `seedRepoId` prop
   (`NewWorkspaceComposerModal.tsx:191`).
7. **`done_closes_issue` default home → the binding type itself**
   (`#[serde(default = "default_true")]` on `BoardBinding`): a binding
   persisted or fetched without the knob reads ON, the editor renders the
   stored value and writes it explicitly, and D1's wizard-default-ON exists
   in exactly one definition site — no wizard-layer duplicate.
8. **Provision step placement → a 4th `OPTIONAL_WORKSPACE_STEPS` entry plus
   a modal-level `'provision'` phase** between goal and details, where the
   D8 consent (branch name + exact file list + default-ON commit toggle)
   renders. The modal is the 008-established editable seam; the composer
   engine and its props contract stay byte-identical.
9. **Board-failure visibility with no run handle in the seam → fold into
   the returned `Skipped(reason)` + always `tracing::warn`.** `HarnessEvent::
   Log` emission from inside the seam would require a signature or variant
   change — both forbidden by the zero-call-site-edit constraint — while the
   reason string rides `drive.rs:400`'s existing `engine.log` and
   `mcp.rs:1191`'s report text untouched. Loud where a run context exists,
   traced everywhere, `Ok` always (AC 7).

---

## 8. Per-feature build/test plan

**Gate commands (every feature):** `cargo test -p agentum-server --lib` ·
`cargo fmt --all --check` + `cargo clippy --workspace` ·
`npm run build --prefix crates/agentum-desktop/ui` + vitest (pure modules).
Harness entries per the spec: `010-f1-board-bind`, `010-f2-board-drive`,
`010-f3-workspace-provision`; AC 11 = the human/qa.sh release demo
(runner: Mateo; evidence: issue timeline + `ai/STATE.md` line), not a build
item.

### F1 — bind

**Steps (ordered):**
1. `github_projects.rs`: `BoardPhase`/`StatusMapping`/`BoardBinding` +
   persistence (path fn, `read_bindings_at`, `binding_for_slug`,
   `upsert_binding`, `remove_binding`, `WRITE_LOCK`).
2. The pure mapper (`normalize`, synonym tables, `resolve_status_mapping`,
   `ResolvedMapping`) — test-first against AC 2's four fixtures.
3. `gh_graphql_argv` + `run_gh_graphql` + `parse_discovery` + the error
   classifier (scope message embeds `gh auth refresh -s project`) +
   `discover_status_field`.
4. `routes/github_projects.rs` (discover/GET/PUT/DELETE) + `lib.rs` merge.
5. UI: `github-projects-client.ts` → `lib/github-projects-binding.ts` →
   `ProjectBindingEditor.tsx` → the `IntegrationsPane.tsx` mount.

**Tests:**
- `normalize_strips_case_space_punct_emoji` — normalization rules.
- `synonym_lists_are_disjoint` — structural guard (no token in two phases).
- `resolve_default_board_maps_three_and_falls_back_rtt_blocked` (fixture a).
- `resolve_custom_backlog_building_qa_shipped` (fixture b — QA→ReadyToTest,
  Shipped→Done, Blocked FellBack to Building).
- `resolve_no_rtt_column_falls_back_to_in_progress_option` (fixture c).
- `resolve_refuses_when_core_phase_unmappable_never_partial` (Err names the
  phase + lists options).
- `gh_graphql_argv_uses_f_for_strings_F_for_ints` (argv pin).
- `parse_discovery_extracts_field_and_options` /
  `parse_discovery_missing_status_field_is_actionable` (canned JSON).
- `classify_scope_missing_names_gh_auth_refresh` (fixture stderr →
  `scope_missing` + the command in the message; fixture d).
- `discover_status_field_with_fake_gh` (`#[cfg(unix)]` — canned-JSON fake gh;
  asserts ONE invocation + parsed result).
- `stored_binding_missing_phase_fails_deserialize_reads_as_no_binding`,
  `binding_for_slug_is_case_insensitive`, `upsert_preserves_other_slugs`,
  `done_closes_issue_defaults_true_when_absent`.
- Route pure: `put_binding_rejects_empty_phase_option`.
- vitest: `mappingComplete` gate; fallback-hint derivation; select-state
  reducer.

### F2 — drive

**Steps (ordered):**
1. Pure builders + `run_gh_capture` + close/reopen/state argv — test-first.
2. `board_write_with` + the id cache + invalidate-retry + probe-gated
   close/reopen; fake-gh suite.
3. `github_transition_with_board` + the two arm hooks (`task_sink.rs`
   :733–749 github arm; :783–806 blocked arm) — LAST, after the suite is
   green, verifying existing label tests untouched.

**Tests:**
- Argv pins: `add_item_mutation_args_shape`, `update_status_mutation_uses_option_id`,
  `issue_node_id_query_args_shape`, `gh_issue_close_reopen_state_argv_shapes`.
- `board_write_with_fake_gh_cold_is_three_graphql_calls` (`#[cfg(unix)]` —
  arg-switching fake gh with canned JSON per operation; asserts node-id →
  addItem → update order + the option-ID token).
- `board_write_second_call_hits_cache_one_call` (two writes, 4 total).
- `board_write_invalidates_stale_item_and_retries_once_cold`.
- `done_closes_open_issue_and_skips_closed` / `in_progress_reopens_closed_only`
  / `knob_off_never_probes_closes_or_reopens`.
- `github_transition_with_board_unbound_is_byte_identical` (binding `None` ⇒
  the exact 5-invocation log today's test pins).
- `github_transition_with_board_board_failure_is_skipped_note_still_ok`
  (**the AC-7 pin**: fake gh fails graphql, label edit succeeds ⇒
  `Skipped("status label applied; Projects board write failed: …")`, outer
  `Ok`).
- `blocked_arm_moves_card_to_blocked_option_with_fake_gh`.
- **AC 8 pin: zero edits to any existing test** — `github_transition_applies_with_fake_gh`
  (:1601), `gh_set_blocked_label_argv_…` (:1669), `github_mark_blocked_with_fake_gh`
  (:1751) et al. stay byte-identical and green.

### F3 — provision

**Steps (ordered):**
1. Argv pins + `create_repo_from_template` + `gh project create` parse.
2. `provision_repo` core (injectable `ProvisionCtx`) + the run-twice test —
   **test-first: write the run-twice test before the commit step**.
3. Routes + `lib.rs` merge.
4. UI: pure modules → goal-step template mode → provision step + modal
   phase → client fns.

**Tests:**
- `gh_repo_create_from_template_argv_shape` / `gh_repo_clone_argv_shape` /
  `gh_project_create_argv_shape` / `parse_project_create_output`.
- `provision_run_twice_changes_nothing` (`#[cfg(unix)]` — temp `git init`
  repo with an initial commit + a temp bare `origin`; fake gh logging with
  canned discovery + project-create JSON; injected `bindings_path`: run 1
  ensures labels, creates+binds project, scaffolds, commits (+1), pushes;
  run 2 shows **no `project create` in the gh log**, binding file unchanged,
  scaffold `changed:false`, `git rev-list --count HEAD` equal — AC 10).
- `provision_skips_commit_when_consent_off`.
- `provision_red_push_is_nonfatal_and_reported` (no-origin repo ⇒
  `pushed:false` + error, overall 200-shaped report).
- `gitignore_rewrite_is_write_if_different_and_keeps_state_ignored`.
- `provision_with_existing_binding_never_creates_a_project` (the guard in
  isolation).
- vitest: `OPTIONAL_WORKSPACE_STEPS` pin updated to four entries (each
  `skippable: true`); `deriveTemplateRepoName`; template-ready gating;
  `provisionCommitFileList` names branch + exactly five paths;
  `summarizeProvisionReport`; existing goal-step tests green unmodified
  (`isGoalStepReady` untouched).

**qa.sh (browser QA, `AGENTUM_BROWSER_VERIFY`-armed else vacuous per 005-F3):**
wizard renders template/adopt modes; the mapping step shows discovered
custom column names with per-phase selects + visible fallback hints; a
missing-`project`-scope bind surfaces the `gh auth refresh -s project`
error; AC 11 live demo on a custom-column board (Mateo).
