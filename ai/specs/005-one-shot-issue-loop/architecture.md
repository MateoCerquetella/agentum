# Spec 005 — Architecture

**Self-check passed.** Load-bearing cites re-verified on this worktree (develop tip `7e9afaa4`): `routes/harness.rs:27-38` (router), `:220-292` (`spec_from_issue`, 400-on-existing at `:247-251`), `harness.rs:75` (`HarnessEngine::start`), `:440` (`claim_driver`), `:452` (`release_driver`), `:503` (`stop`), `harness/types.rs:108-203` (`FeatureList` + defaults), `:211-239` (`copy_knobs_from`), `:895-898` (`derive_backlog_from_spec` returns `..FeatureList::default()`), `:905-957` (`plan_from_spec` / `_with_tracker` / `_inner`), `harness/drive.rs:126-135` (spawn + InProgress transition), `:147` (`build_feature_prompt` call), `:184-208` (ReadyToTest + `resolve_qa_mode` call), `:240-246` (Done), `:396` (`spawn_agent_into_pane` — the one launch path), `:407-423` (`resolve_qa_mode`), `:508-514` (stale `AGENTUM_BROWSER_VERIFY` warning), `:521-546` (QA agent + verdict), `task_sink.rs:214-231` (`TrackerPhase`/`TransitionResult`), `:247-298` (labels + argv builders), `:304-320` (URL parser), `:324-326` (`gh_bin`), `:365-378` (`github_transition_with`), `:391-449` (`apply_tracker_transition`), `linear.rs:182-257` (`LinearStateMap` layering), `agentum-desktop/src/commands/linear.rs:467-498` (`linear_get/set_state_map` — the Settings write precedent), `routes/mcp.rs:68-114` (`MCP_ENABLED_SETTING` + GET/PUT settings route precedent), `:252-258` (`orchestration_enabled` store-setting read), `:263` (`tool_specs`), `:623-648` (`call_tool` dispatch + never-panic outcome mapping), `:1219-1270` (catalog tests — the delegation-test pattern), `routes/board_goals.rs:601-619` (Todo-at-plan precedent), `playwright_mcp.rs:80-84` (`feature_enabled`), `agentum-store/src/settings.rs:10-55` (`setting_get/set` + bool variants), UI: `lib/open-created-workspace.ts:40-67` (all three plain-delivery paths), `lib/worktree-activation.ts:413-425` (issueCommand split), `hooks/useComposerState.ts:542` (`scaffoldSpec` state), `:2065-2091` (`maybeScaffoldSpecFromIssue`), `:2214/:2231` and `:2404/:2428` (both submit paths' call sites), `components/NewWorkspaceComposerCard.tsx:637-656` (the D5 toggle to mirror), `components/TaskPage.tsx:2349-2400` (`openComposerForItem` + row action), `runtime/harness-client.ts:148-219` (`startHarness`/`runHarness`), `runtime/github-issue-client.ts:125-158` (`scaffoldSpecFromIssue`), `components/settings/IntegrationsPane.tsx:48/:78` (Linear state-map Settings UI).

**Status:** Architect → ready for Developer. All D1–D4 honored; five corrections/nuances below — none blocks the build, all change *where or in what order* work lands.

---

## 0. TL;DR — five features, one sentence each

1. **F1** — new `POST /api/harness/start-work` (server-orchestrated, one failure surface, serialized by an engine-level lock): converge-scaffold + plan from the linked issue → Todo transition → post-plan knob write (`agent_tool`/`agent_model`) → register → claim → spawn `drive`; the composer gains a "Start gated run" toggle that calls it and suppresses **all three** plain-delivery paths; the Tasks page row action pre-fills the composer with the toggle armed.
2. **F2** — `plan_from_spec_inner` stamps `FeatureList.spec_id`; `build_feature_prompt` widens with `spec_rel_path: Option<&str>` and tells the agent to read the spec first; no-spec and explicit-`prompt` cases pinned byte-identical.
3. **F3** — `build_qa_prompt` steers the QA agent at the `agentum_browser` MCP tool (verdict contract untouched); `resolve_qa_mode` becomes pure (`agent_qa_capable: bool` param) fed by `AGENTUM_BROWSER_VERIFY` **or** a new store setting `harness.qa.agent_browser.enabled` (default OFF, D3) behind `GET/PUT /api/harness/settings`.
4. **F4** — new MCP tool `agentum_report_status` ({provider, id, url?, phase}) as a thin arm over `apply_tracker_transition`, mapping every tracker failure to a normal text result (never `isError` for a hiccup).
5. **F5** — `GithubStateMap` in `task_sink.rs` mirroring `LinearStateMap` (defaults → `github.json` `state_map` → `AGENTUM_GITHUB_STATUS_*` env); argv builders take the map, filter the remove-set **by name**, colors stay keyed by phase; Settings writes `github.json` via new flat-arg Tauri commands mirroring `linear_set_state_map`.

---

## 1. Corrections & contradictions (read before building)

- **C1 — AC 5's "toast + harness event" is only half-satisfiable before registration.** Every `HarnessEvent` variant requires a `harness_id`; a failure in fetch/scaffold/plan happens before any run exists. Resolved: pre-registration failures surface as the HTTP error → composer toast (exactly the `maybeScaffoldSpecFromIssue` non-fatal pattern, `useComposerState.ts:2085-2088`); post-registration failures ride the existing `drive` error path (`drive.rs:33-45` emits `HarnessEvent::Error` + `Failed`). No `Uuid::nil()` pseudo-events. The workspace is never rolled back in either case.
- **C2 — AC 1's `status/in-progress`-at-first-spawn needs zero new code.** `drive_inner` already fires `TrackerPhase::InProgress` right after `spawn_feature_agent` (`drive.rs:126-135`), and the backlog F1 plans is tracker-stamped by `plan_from_spec_with_tracker`. Do **not** add a second InProgress call in the start-work seam — the AC is satisfied by wiring INTO the engine (the invariant, verbatim).
- **C3 — F3's knob persistence: store setting, not a file.** The handoff floated "a Settings-writable file, like `linear.json`". The stronger, closer precedent is the SQLite settings table + a GET/PUT route — exactly how `MCP_ENABLED_SETTING` (`routes/mcp.rs:68-114`) and `ORCHESTRATION_ENABLED_SETTING` (`routes/orchestration.rs:63-76`) already work, readable from `AppState.store` inside `drive_inner` with no new file format. `linear.json`/`github.json` exist because the *desktop* owns Linear/GitHub creds; this knob is server-owned run behavior. (F5 keeps the file, because D4 locks it.)
- **C4 — F2's stamp lands in `plan_from_spec_inner`, which also widens the MCP `agentum_harness_plan` tool's output.** AC 6 says "the spec-from-issue plan step stamps `spec_id`". Stamping in `_inner` (one line) covers the 004 opt-in route, F1's start-work, **and** the MCP plan tool — every backlog planned from a spec records its spec. This deliberately retires the 004 comment "the `tracker: None` path is byte-for-byte the pre-004 `plan_from_spec`" (`types.rs:926`) — update it. Safe because spec-013's role gates key on `roles && spec_id.is_some()` (`drive.rs:78-81`) and `roles` stays false. The `plan_from_spec_delegation_unchanged` test (`harness.rs:1433-1451`) gains one assertion (`spec_id == Some("s1")`), it does not fight this.
- **C5 — F1 re-entry ordering is load-bearing and not in the spec text: the already-running check must precede ALL filesystem mutation, and start-work must be self-serialized.** Re-planning overwrites `feature_list.json` (states re-derived from spec checkboxes, knobs reset to defaults then rewritten) — done against a *driving* run it clobbers mid-run state; done concurrently by two retries it can register two runs on one worktree (per-run `claim_driver` can't see that). Resolved: an engine-level `start_work_lock: tokio::sync::Mutex<()>` held across the whole orchestration + "existing run for this workdir" resolution first (§2). This is the friendly-state requirement made structural.

Confirmed for the PM: **the registry `Worktree` struct is untouched** — F1 touches `useComposerState.ts`, `open-created-workspace.ts`, `TaskPage.tsx`, `harness-client.ts`, and `routes/harness.rs` only; no `CreateBody`/registry change, so the serde-alias-free rule (spec 004 wipe hazard) holds by construction.

---

## 2. F1 — `start-work-gated-run` (AC 1–5)

### Route decision (D1)

**`POST /api/harness/start-work`**, in `crates/agentum-server/src/routes/harness.rs`. Every sequenced verb is a harness verb; the router, auth layer, engine handle, and UI client already exist there. `/api/workflows/*` would add a namespace + client module for one route with no second verb in sight (YAGNI). **Rejected:** client-side sequencing (D1's own rationale: triplicated partial-failure handling across composer, Tasks page, and the named chat-card follow-up).

### Boundaries

| File | Change |
|---|---|
| `crates/agentum-server/src/routes/harness.rs` | `start_work` handler + route; refactor `spec_from_issue`'s core into a shared `ensure_spec_and_plan` (converge flag); the Todo-at-plan call lives in that shared core (AC 4) |
| `crates/agentum-server/src/harness.rs` | `start_work_lock: tokio::sync::Mutex<()>` field on `HarnessEngine` + `pub async fn find_by_workdir(&self, workdir: &Path) -> Option<Uuid>` |
| `crates/agentum-server/src/harness/types.rs` | `pub async fn update_backlog_knobs(workdir, f: impl FnOnce(&mut FeatureList)) -> anyhow::Result<FeatureList>` — the post-plan knob-write seam (PM finding 5) |
| `crates/agentum-desktop/ui/src/runtime/harness-client.ts` | `startGatedWork()` |
| `crates/agentum-desktop/ui/src/lib/open-created-workspace.ts` | `gatedRun?: boolean` option — all three plain-delivery skips live HERE so every caller inherits |
| `crates/agentum-desktop/ui/src/hooks/useComposerState.ts` | `startGatedRun` state (+ `initialStartGatedRun` option at `:103`-area); `maybeStartGatedRun` helper; wire both submit paths |
| `crates/agentum-desktop/ui/src/components/NewWorkspaceComposerCard.tsx` + `NewWorkspaceComposerModal.tsx` | the toggle + undelivered-prompt copy; thread `startGatedRun` modal data |
| `crates/agentum-desktop/ui/src/components/TaskPage.tsx` | issue-row dropdown "Start gated run" → `openComposerForItem(item, { startGatedRun: true })` (widen its signature; today `:2349`) |

### HTTP contract

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartWorkRequest {
    workdir: String,                     // the freshly created worktree (local)
    number: String,                      // issue number, digits-only
    #[serde(default)] slug: Option<String>,       // owner/repo fast path
    #[serde(default)] agent_tool: Option<String>, // composer's selected agent (D2)
    #[serde(default)] agent_model: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartWorkResponse {
    harness_id: Uuid,
    spec_id: String,
    spec_existed: bool,   // converged on an existing spec (retry / D5 overlap)
    planned: usize,       // feature count
    run_started: bool,
    already_running: bool, // friendly state, NOT an error (200)
}
```

(Casing follows the newer `SpecFromIssueResponse` camelCase precedent, not `StartResponse`'s legacy snake_case.)

### Orchestration sequence (the handler, straight-line)

All under `let _g = state.harness.start_work_lock.lock().await;` (C5):

1. `expand_workdir` + `is_dir` → 400.
2. **Friendly already-running check first** (before any fs write): `engine.find_by_workdir(&workdir)` → if `Some(existing)`: `claim_driver(existing)?` — `false` → return `200 { harnessId: existing, alreadyRunning: true, runStarted: false, specExisted: true, planned: <status().features.len()>, specId: <from status/config or ""> }`. `true` → we own an *idle* stale run: `engine.stop(existing)` (removes it; note: `stop` emits `HarnessCompleted{success:false}` — cosmetic, acceptable) and fall through to a fresh registration. This is what makes retries converge instead of clobbering a live run.
3. `fetch_github_issue(&state, &workdir_str, &req.number, req.slug.as_deref())` (existing shared fetch — needed even when the spec exists, because `spec_id = issue_spec_id(number, title)` needs the title).
4. `ensure_spec_and_plan(...)` — the shared core (below) with `converge_existing: true`, `plan: true` (forced ON, AC 1).
5. **Post-plan knob write** (AC 2 — "the plan itself writes defaults"): `crate::harness::update_backlog_knobs(&workdir, |list| { if let Some(t)=&req.agent_tool { list.agent_tool = t.clone(); } if let Some(m)=&req.agent_model { list.agent_model = Some(m.clone()); } }).await` — `spec_id` is already stamped by F2's plan change; do not re-stamp here.
6. `let harness_id = engine.start(workdir.clone()).await?` (register AFTER the plan so the run's in-memory snapshot is the real backlog, not the scaffold stub).
7. `engine.claim_driver(harness_id)` (fresh run → `true`); `tokio::spawn(crate::harness::drive(st, harness_id))` — byte-identical to the `run` route's spawn (`routes/harness.rs:103-104`).
8. Respond `200 { runStarted: true, alreadyRunning: false, ... }`.

If any step after a successful claim errors, `release_driver` before returning (only step 7's spawn follows the fresh claim, so in practice this is the step-6→7 error path).

### The shared core (also serves the 004 opt-in route — AC 4)

Refactor the body of `spec_from_issue` (`routes/harness.rs:220-292`) into:

```rust
struct EnsuredSpec {
    spec_id: String,
    spec_path: String,       // relative
    written: Vec<String>,
    spec_existed: bool,
    features: Option<crate::harness::FeatureList>,
}

/// Scaffold + write spec.md + optionally plan, from an ALREADY-FETCHED issue.
/// `converge_existing: false` keeps the route's never-overwrite 400 contract;
/// `true` (start-work) plans from the existing spec instead (AC 1 convergence).
/// On a successful plan, fires the initial `TrackerPhase::Todo` transition
/// (best-effort, logged — mirrors board_goals.rs:601-619) so BOTH callers
/// inherit the label-trail start (AC 4).
async fn ensure_spec_and_plan(
    store: &agentum_store::Store,
    workdir: &Path,
    number: &str,
    issue: &super::github::FetchedIssue,
    plan: bool,
    converge_existing: bool,
) -> Result<EnsuredSpec, ApiError>
```

The Todo call, verbatim shape:

```rust
if let Some(list) = &features {
    let _ = list; // planned OK → start the label trail at Todo (idempotent flip)
    match crate::task_sink::apply_tracker_transition(
        store, "github", number, Some(&issue.url), crate::task_sink::TrackerPhase::Todo,
    ).await {
        Ok(_) => {}
        Err(e) => tracing::warn!(number, error = %e, "initial Todo transition failed (non-fatal)"),
    }
}
```

Placement rationale: `apply_tracker_transition` needs `&Store`; `plan_from_spec_with_tracker` lives in fs-only `types.rs` — so the transition sits in the route-layer core, which is exactly "the spec-from-issue plan branch" AC 4 names. The existing `spec_from_issue` handler becomes: validate → fetch → `ensure_spec_and_plan(store, …, req.plan, /*converge*/ false)` → response (wire shape unchanged; it now also fires Todo when it plans — AC 4 says that inheritance is intentional).

Taking `issue: &FetchedIssue` (not fetching inside) is what makes the core unit-testable with a synthetic issue + tempdir and no `gh`.

### Engine additions

```rust
// harness.rs — HarnessEngine
pub struct HarnessEngine {
    runs: RwLock<HashMap<Uuid, Arc<RwLock<HarnessRun>>>>,
    event_tx: broadcast::Sender<HarnessEvent>,
    /// Serializes POST /api/harness/start-work end-to-end (C5): the
    /// already-running check, the feature_list.json rewrite, and the fresh
    /// register+claim must be atomic per workdir or two retries can register
    /// two drivers on one worktree.
    start_work_lock: tokio::sync::Mutex<()>,
}

/// The registered run (if any) whose workdir == `workdir`. First match wins;
/// start-work keeps the map at ≤1 run per workdir going forward.
pub async fn find_by_workdir(&self, workdir: &Path) -> Option<Uuid>
```

```rust
// types.rs — the post-plan knob-write seam (PM finding 5). Load → mutate →
// persist feature_list.json; returns the saved list. Knob writes only — the
// features vector passes through untouched (contrast copy_knobs_from).
pub async fn update_backlog_knobs(
    workdir: &Path,
    apply: impl FnOnce(&mut FeatureList),
) -> anyhow::Result<FeatureList>
```

### UI — the toggle, the three skips (D2), the copy

- **State:** `const [startGatedRun, setStartGatedRun] = useState(Boolean(initialStartGatedRun))` in `useComposerState` (option `initialStartGatedRun?: boolean` beside `initialRepoId`, `useComposerState.ts:103`; threaded from `NewWorkspaceComposerModal.tsx:113-118` from modal data). Rendered in `NewWorkspaceComposerCard.tsx` directly below the D5 scaffold toggle (`:637-656`), gated by the **same** eligibility (linked `github.com` issue + local repo — reuse `canScaffoldSpec`'s derivation). When armed, **hide** the "Scaffold spec from issue" toggle (subsumed — the server scaffolds) and render the AC 2 copy: *"Your typed prompt won't be sent — the linked issue becomes the spec and drives the gated agents. The selected agent runs inside the engine."*
- **Submit (both paths):** in `submit` (`:2093`) and `submitQuick` (`:2297`), when armed:
  - force `submitShouldRunIssueAutomation = false` (no issueCommand is built, no trust prompt — one of the three skips at the source);
  - skip `maybeScaffoldSpecFromIssue` (AC 1: the D5 call is skipped when armed);
  - after `applyWorktreeMeta`, call a new shared `maybeStartGatedRun(worktree, submitLinkedWorkItem)` (mirrors `maybeScaffoldSpecFromIssue`'s shape and non-fatal contract): re-derive the gate from the submitted item (`parseGitHubIssueOrPRLink`, local-only), then `startGatedWork({ workdir: worktree.path, number: item.number, slug, agentTool: tuiAgent?.id, agentModel: undefined })`; catch → `toast.error('Workspace created, but the gated run could not start.')` — never roll back (AC 5). When the response says `alreadyRunning`, toast an *info* ("A gated run is already driving this workspace"), not an error (the friendly state, C5).
  - call `openCreatedWorkspace({ …, gatedRun: true })` and pass `issueCommand: undefined`.
- **The three skips, centralized** in `open-created-workspace.ts`:

```ts
export type OpenCreatedWorkspaceOptions = {
  // …existing…
  /** Spec 005 F1 (D2): a gated engine run owns the worktree's agents. Skip all
   *  three plain-delivery paths — the draft-open, the picker prompt stash, and
   *  the issueCommand automation — so exactly one (engine-spawned) agent runs. */
  gatedRun?: boolean
}
```
  In the body: when `gatedRun`, call `activateAndRevealWorktree` **without** `issueCommand` (belt-and-braces with the composer-side suppression) and return before the `if (agent)` / `else if (prompt)` branches. Extract the decision into a pure, testable helper in the same file:

```ts
export function planCreatedWorkspaceOpen(opts: {
  gatedRun?: boolean; agent: TuiAgent | null; prompt?: string; hasIssueCommand: boolean
}): { launchAgent: boolean; stashPrompt: boolean; runIssueCommand: boolean }
```
  (`gatedRun` → all three `false`; otherwise today's behavior — the "suppression flag round-trips" unit pin.)
- **Tasks page (AC 3):** widen `openComposerForItem(item, opts?: { startGatedRun?: boolean })` (`TaskPage.tsx:2349`) to spread `...(opts?.startGatedRun ? { startGatedRun: true } : {})` into the `openModal('new-workspace-composer', …)` data; add a **"Start gated run"** entry to the issue-row dropdown (the same menu hosting "Use", near `:2388-2400`) that calls it with the flag. The modal type (`NewWorkspaceComposerModal.tsx:21-23` + the `ui.ts:450` modal-data type) gains `startGatedRun?: boolean`.
- **Client:**

```ts
// runtime/harness-client.ts
export type StartGatedWorkResult = {
  harnessId: string; specId: string; specExisted: boolean
  planned: number; runStarted: boolean; alreadyRunning: boolean
}
export function startGatedWork(input: {
  workdir: string; number: number; slug?: string
  agentTool?: string; agentModel?: string
}): Promise<StartGatedWorkResult> {
  return request('/api/harness/start-work', {
    method: 'POST',
    body: JSON.stringify({
      workdir: input.workdir, number: String(input.number),
      ...(input.slug ? { slug: input.slug } : {}),
      ...(input.agentTool ? { agentTool: input.agentTool } : {}),
      ...(input.agentModel ? { agentModel: input.agentModel } : {}),
    }),
  })
}
```

**Tradeoff taken:** stale-idle runs are stopped + re-registered (fresh id) rather than refreshed in place — `drive_inner` iterates the run's *in-memory* feature snapshot, and an in-place refresh method (`reload_features`) is more surface for the same result. **Rejected:** registering before planning (snapshots the stub backlog); a per-workdir claim table (the mutex is sufficient at click frequency).

### Unit-test plan (F1)

1. `ensure_spec_and_plan_writes_and_plans_fresh` — tempdir + synthetic `FetchedIssue`; spec.md written, backlog planned, every feature tracker-stamped, `spec_id` stamped (F2), `spec_existed == false`.
2. `ensure_spec_and_plan_converges_on_existing_spec` — pre-write the spec; `converge_existing: true` → no error, re-plans from the existing file, `spec_existed == true`; `converge_existing: false` → the 400 (existing route contract pinned).
3. `update_backlog_knobs_preserves_features_and_writes_knobs` — plan, then knob-write `agent_tool`/`agent_model`; feature vector + tracker stamps + `spec_id` untouched, knobs persisted (reload proves it).
4. `find_by_workdir_resolves_registered_run` + `claim_driver_second_claim_is_friendly_false` (engine-level; the latter mostly exists — extend to assert re-claim after `release_driver`).
5. Todo-at-plan: in test 1, point `AGENTUM_GH_BIN` at a fake `gh` (the `task_sink.rs` fake-script pattern, `#[cfg(unix)]`) and assert the transition argv was invoked once with `status/todo` — or, minimally, assert the code path is reached via the `github_transition_without_url_is_skipped`-style seam. (The handler itself is a reviewed straight-line composition; the order is pinned by these seam tests, not an AppState harness.)
6. UI: `planCreatedWorkspaceOpen` vitest — gated → `{false,false,false}`; agent+prompt → launch only; no-agent+prompt → stash only (colocate under `lib/`; avoid xterm imports — known vitest loader noise).
7. `npm run build --prefix crates/agentum-desktop/ui` green.

---

## 3. F2 — `spec-aware-feature-prompt` (AC 6)

### Boundaries

| File | Change |
|---|---|
| `crates/agentum-server/src/harness/types.rs` | one line in `plan_from_spec_inner` (`:937`-area): `list.spec_id = Some(spec_id.to_string());` + update the `:924-926` doc comment (C4) |
| `crates/agentum-server/src/harness/helpers.rs` | widen `build_feature_prompt` (`:33`) |
| `crates/agentum-server/src/harness/drive.rs` | compute + thread the spec path at the one call site (`:147`) |

### Seams

```rust
// helpers.rs — the explicit-prompt short-circuit stays FIRST and unconditional.
pub(crate) fn build_feature_prompt(
    instructions: &str,
    feature: &Feature,
    spec_rel_path: Option<&str>,  // e.g. ".agentum-harness/specs/42-add-widget/spec.md"
) -> String
```

When `Some(path)`, insert one section between the AGENTS.md block and the task block:

```
=== THE SPEC ===
This feature comes from the spec at `{path}` (relative to the project root).
Read that file BEFORE coding — it carries the full acceptance criteria and context.
```

When `None`, the output is **byte-identical** to today's format string (`helpers.rs:37-51`).

Call site (`drive.rs:147`):

```rust
// Only steer at a spec that actually exists on disk — a stale spec_id must not
// send the agent hunting for a missing file. Handles the legacy `.harness` dir
// by deriving the dir name from config.harness_dir, not the HARNESS_DIR const.
let spec_rel = config.features.spec_id.as_deref().and_then(|sid| {
    let dir = config.harness_dir.file_name()?.to_string_lossy().into_owned();
    let rel = format!("{dir}/specs/{sid}/spec.md");
    workdir.join(&rel).exists().then_some(rel)
});
let prompt = build_feature_prompt(&config.agent_instructions, &feature, spec_rel.as_deref());
```

The stamp in `plan_from_spec_inner` covers all three planners-from-spec (004 route, F1 start-work, MCP `agentum_harness_plan`) — C4 records the deliberate MCP widening. `copy_knobs_from` (`types.rs:211-239`) already preserves `spec_id` through decompose; no change there.

### Unit-test plan (F2) — in `harness.rs`'s test mod (where the prompt/plan tests live)

1. `feature_prompt_without_spec_is_byte_identical` — `build_feature_prompt(i, f, None)` equals the exact pre-change literal (write the expected string out in full — this is the regression pin, AC 6).
2. `feature_prompt_with_spec_names_the_path_and_says_read_first` — contains the rel path + "BEFORE coding"; still contains the gate contract text.
3. `feature_prompt_explicit_override_wins_even_with_spec` — `feature.prompt = Some(p)` → returns exactly `p` with `spec_rel_path = Some(..)` (the second byte-identical pin, `helpers.rs:34-36`).
4. `plan_from_spec_stamps_spec_id` — extend `plan_from_spec_delegation_unchanged` (`harness.rs:1433`): planned list has `spec_id == Some("s1")`, still no tracker stamps; and extend `plan_from_spec_with_tracker_stamps_provider_and_url` (`:1402`) with the same `spec_id` assert.

---

## 4. F3 — `qa-agentum-browser` (AC 7–8)

### Boundaries

| File | Change |
|---|---|
| `crates/agentum-server/src/harness/helpers.rs` | rewrite `build_qa_prompt`'s "WHAT TO DO" step 1 (`:154-155`); verdict contract lines untouched |
| `crates/agentum-server/src/harness/drive.rs` | `resolve_qa_mode` widens (pure); caller computes capability; replace the stale env warning (`:508-514`) |
| `crates/agentum-server/src/routes/harness.rs` | `BROWSER_QA_ENABLED_SETTING` const + `GET/PUT /api/harness/settings` |
| `crates/agentum-desktop/ui/src/runtime/harness-client.ts` | `getHarnessSettings`/`setHarnessSettings` |
| `crates/agentum-desktop/ui/src/components/settings/IntegrationsPane.tsx` | the toggle (lands beside the Linear state-map pipeline config — same "pipeline behavior" pane) |

### Prompt rewrite (AC 7 — wording direction, verdict contract byte-compatible)

Replace step 1 of `build_qa_prompt` with:

```
1. Use the `agentum_browser` MCP tool to QA this feature against the running
   app: start with op `open` and the app URL (add `split:"right"` to place the
   browser beside you — it is the VISIBLE in-app browser), then drive it with
   navigate/click/fill/snapshot, and capture a `screenshot` per check as
   evidence. Do NOT use the browser-verification-loop skill, claude-in-chrome,
   or Playwright here — `agentum_browser` is the browser surface for this app.
```

Step 2 (the verdict-file JSON contract) is **unchanged, character for character** — `parse_qa_verdict`, `qa_verdict_path`, and the missing/garbled-fails behavior in `run_qa_agent_gate` (`drive.rs:532-546`) are untouched. Also update the log line at `drive.rs:518` ("browser-verification-loop" → "agentum_browser").

### The knob (D3 — default OFF, second opt-in door)

```rust
// routes/harness.rs
/// Opt-in capability switch for the Auto QA arm (spec 005 F3, D3): when true,
/// `resolve_qa_mode`'s Auto arm treats agent-QA as capable WITHOUT
/// AGENTUM_BROWSER_VERIFY. Default OFF — Auto + no qa.sh + no env stays the
/// Script skip-pass, so non-web projects and headless/CI are byte-identical.
pub const BROWSER_QA_ENABLED_SETTING: &str = "harness.qa.agent_browser.enabled";
```

Routes (mirror `routes/mcp.rs:91-114` exactly; `/api/harness/settings` coexists with `/api/harness/{id}` the same way `/api/harness/events` already does — matchit static-over-capture):

```rust
.route("/api/harness/settings", get(get_settings).put(put_settings))
// GET  → { "browserQaAgentEnabled": bool }   (setting_get_bool(.., false))
// PUT  { "browserQaAgentEnabled": bool } → persists via setting_set_bool
```

### `resolve_qa_mode` — now fully pure (the matrix test's enabler)

```rust
// drive.rs — the env read moves to the caller; the function becomes a pure
// decision table (mode × qa.sh-present × capable).
pub(super) fn resolve_qa_mode(config: &HarnessConfig, agent_qa_capable: bool) -> QaMode {
    match config.features.qa_mode {
        QaMode::Script => QaMode::Script,
        QaMode::Agent => QaMode::Agent,
        QaMode::Auto => {
            if config.qa_script.is_some() { QaMode::Script }
            else if agent_qa_capable { QaMode::Agent }
            else { QaMode::Script } // skip-pass: non-web projects advance
        }
    }
}
```

Caller (`drive.rs:194`):

```rust
let agent_qa_capable =
    crate::playwright_mcp::feature_enabled() || browser_qa_agent_enabled(state).await;
let qa_mode = resolve_qa_mode(&config, agent_qa_capable);
```

```rust
/// Best-effort read (mirrors routes/mcp.rs::orchestration_enabled): a store
/// error falls back to OFF — never a run failure, and OFF is the D3 default.
async fn browser_qa_agent_enabled(state: &AppState) -> bool {
    state.store
        .setting_get_bool(crate::routes::harness::BROWSER_QA_ENABLED_SETTING, false)
        .await.unwrap_or(false)
}
```

(`AppState` reaches `drive_inner` already — the handoff's signature question resolves as: the *capability bit* is computed where `state` lives and passed in; `HarnessConfig` stays fs-only.)

Replace the stale warning in `run_qa_agent_gate` (`drive.rs:508-514`): the QA agent now needs the **agentum MCP** (wired by default), not Playwright — warn when the master switch is off:

```rust
if !state.store.setting_get_bool(crate::routes::mcp::MCP_ENABLED_SETTING, true).await.unwrap_or(true) {
    engine.log(harness_id, Some(&feature.id),
        "QA agent: the agentum MCP master switch is OFF (Settings → Agent MCP) — the agent has no `agentum_browser` tool and the QA gate will likely fail.");
}
```

### Unit-test plan (F3)

1. `qa_prompt_steers_agentum_browser` — contains `agentum_browser`, `open`, `split`; does **not** contain `browser-verification-loop`; still contains the verdict rel path and the exact `{"passed": true|false, ...}` contract line (the contract-identical pin).
2. `resolve_qa_mode_matrix` — pure, no env mutation: all 12 cells of {Script,Agent,Auto} × {qa.sh present,absent} × {capable,not}: explicit modes ignore both dimensions; Auto+qa.sh → Script always; Auto+no-qa.sh → capable?Agent:Script. The `capable=false` column IS the AC 8 byte-identical pin (D3).
3. Update `resolve_qa_mode_honors_explicit_and_auto` (`harness.rs:882`) — pass `false` explicitly; the old `Script | Agent` tolerance (env leakage) tightens to exact asserts.
4. Settings round-trip: `harness_qa_setting_defaults_off_and_round_trips` (mirror `mcp_master_switch_defaults_on_and_round_trips`, `mcp.rs:1192` — tempdir Store, default `false`, set/read `true`).
5. Existing QA-gate tests (`run_qa_once`, verdict parsing) untouched-green — the gate contract regression net.

---

## 5. F4 — `mcp-report-status` (AC 9)

### Boundaries

| File | Change |
|---|---|
| `crates/agentum-server/src/routes/mcp.rs` | `tool_specs` entry + `call_tool` arm + `tool_report_status` |
| `crates/agentum-server/src/task_sink.rs` | `pub fn parse_tracker_phase(&str) -> Option<TrackerPhase>` (pure); `github_slug_and_number_from_issue_url` → `pub(crate)` (id-from-url derivation) |

### Tool spec (append to `tool_specs()`, `mcp.rs:263`)

```json
{
  "name": "agentum_report_status",
  "description": "Report a work item's pipeline phase to its tracker: GitHub = flip the status/* label, Linear = move the workflow state, board = move the card column. Best-effort by contract — a tracker hiccup returns a 'skipped' note, never a tool error — so call it freely on every phase change (todo, in_progress, ready_to_test, done).",
  "inputSchema": {
    "type": "object",
    "properties": {
      "provider": { "type": "string", "enum": ["github", "linear", "board"] },
      "id": { "type": "string", "description": "The tracker's stable handle: board card key (AG-12), Linear identifier (ENG-42), or GitHub issue number. For github it may be omitted when `url` is given (derived from the URL)." },
      "url": { "type": "string", "description": "The ticket URL. Required for github — owner/repo and the issue number are parsed from it. Ignored by linear/board." },
      "phase": { "type": "string", "enum": ["todo", "in_progress", "ready_to_test", "done"] }
    },
    "required": ["provider", "phase"],
    "additionalProperties": false
  }
}
```

**Not** in `ORCHESTRATION_TOOLS` (it is a status verb, not the mailbox/DAG surface) — advertised and callable regardless of that gate, like `agentum_list_sessions`.

### Implementation — split for testability, never-`Err` for hiccups

```rust
// mcp.rs — dispatch arm:
"agentum_report_status" => tool_report_status(state, &args).await,

/// Parsed+validated inputs. Pure → unit-testable without AppState.
/// Errors here are CALLER bugs (missing/unknown args) and DO surface as
/// isError:true — the best-effort contract covers tracker failures, not typos.
fn parse_report_status_args(args: &Value)
    -> anyhow::Result<(String /*provider*/, String /*id*/, Option<String> /*url*/, TrackerPhase)>
// - provider, phase required; phase via task_sink::parse_tracker_phase.
// - id required, EXCEPT provider=="github" with a parseable issue `url`
//   (id := the URL's number — task_sink::github_slug_and_number_from_issue_url).

/// Map the seam's outcome to the tool's text. Pure. NEVER an Err for a
/// tracker failure (AC 9 / best-effort invariant): transport errors from the
/// linear/board arms come back as a "skipped" note the agent can read.
fn report_status_text(outcome: anyhow::Result<TransitionResult>, provider: &str, phase: TrackerPhase) -> String
// Ok(Applied)     → "applied: {provider} → {phase:?}"
// Ok(Skipped(w))  → "skipped: {w}"
// Err(e)          → "skipped (tracker error, non-fatal): {e:#}"

async fn tool_report_status(state: &AppState, args: &Value) -> anyhow::Result<String> {
    let (provider, id, url, phase) = parse_report_status_args(args)?;
    let outcome = crate::task_sink::apply_tracker_transition(
        &state.store, &provider, &id, url.as_deref(), phase).await;
    Ok(report_status_text(outcome, &provider, phase))
}
```

Delegation is thin by construction — the arm never reimplements label/state mechanics (the same rule every existing tool follows). An unknown provider string flows to the seam's `Ok(Skipped("unknown tracker provider …"))` — visible, non-fatal.

### Unit-test plan (F4) — the same pattern the catalog tests use

1. `parse_tracker_phase_accepts_the_four_and_rejects_junk` (task_sink.rs).
2. `report_status_args_require_id_except_github_url` — id-less linear → Err; id-less github + issue URL → Ok with derived number; id-less github + garbage URL → Err.
3. `report_status_text_never_errs_on_tracker_failure` — all three outcome shapes map to strings; `Err` input yields "skipped (tracker error…" (the AC 9 pin).
4. `report_status_is_in_the_catalog` + `report_status_survives_orchestration_gate_off` (mirror `list_sessions_is_in_the_catalog` / gate tests, `mcp.rs:1243-1270`; update the `off.len() + ORCHESTRATION_TOOLS.len() == on.len()` arithmetic — it holds automatically since the tool is ungated).
5. Wire-level delegation (board arm, no subprocess): tempdir Store + a board card → `tool_report_status` moves the card column (the `board_transition_moves_card_status` fixture pattern, `task_sink.rs:822`).

---

## 6. F5 — `github-state-map` (AC 10)

### Boundaries

| File | Change |
|---|---|
| `crates/agentum-server/src/task_sink.rs` | `GithubStateMap` + file/env layering; `github_status_color(phase)`; widen `gh_set_status_label_argv` + `github_transition_with`; the github arm resolves the map |
| `crates/agentum-desktop/src/commands/github_labels.rs` (new) + `src/lib.rs` handler registration | `github_get_state_map` / `github_set_state_map` — **flat args** (Tauri convention), writing `<data_local_dir|data_dir>/Agentum/github.json` |
| `crates/agentum-desktop/ui/src/tauri/github-labels.ts` (new) + `components/settings/IntegrationsPane.tsx` | four-input GitHub card mirroring the Linear state-map UI (`:48/:78`) |

### The map (mirrors `LinearStateMap`, `linear.rs:182-257`)

```rust
/// The four pipeline phases → GitHub *label names* (spec 005 F5, D4). Teams
/// with their own status vocabulary configure names here; the transport
/// (ensure-create + one edit) is unchanged from spec 004.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubStateMap {
    pub todo: String, pub in_progress: String, pub ready_to_test: String, pub done: String,
}
impl Default for GithubStateMap { /* the canonical four from GITHUB_STATUS_LABELS */ }

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoredGithubStateMap {
    #[serde(default)] todo: Option<String>,
    #[serde(default)] in_progress: Option<String>,
    #[serde(default)] ready_to_test: Option<String>,
    #[serde(default)] done: Option<String>,
}
#[derive(Debug, Default, Serialize, Deserialize)]
struct GithubConfigFile { #[serde(default)] state_map: Option<StoredGithubStateMap> }

/// `AGENTUM_GITHUB_CONFIG` override (tests/CI — mirrors AGENTUM_LINEAR_CREDS),
/// else `<data_local_dir|data_dir>/Agentum/github.json` (the linear.json sibling).
fn github_config_path() -> Option<PathBuf>;
fn read_github_config() -> GithubConfigFile;   // unreadable/absent → Default

impl GithubStateMap {
    /// defaults → github.json state_map → AGENTUM_GITHUB_STATUS_{TODO,IN_PROGRESS,
    /// READY_TO_TEST,DONE} (highest). Partial overrides keep lower layers —
    /// byte-for-byte the LinearStateMap::from_env layering.
    pub fn from_env() -> Self { Self::apply_layers(read_github_config().state_map, |k| std::env::var(k).ok()) }
    /// Pure core: precedence is tested by injection, never by env mutation
    /// (parallel tests; no ENV_HOME_TEST_LOCK needed).
    fn apply_layers(file: Option<StoredGithubStateMap>, env: impl Fn(&str) -> Option<String>) -> Self;
    pub fn label_for(&self, phase: TrackerPhase) -> &str;
    fn labels(&self) -> [&str; 4];
}
```

### The color story (decision)

Colors key off the **phase**, never the name: keep `GITHUB_STATUS_LABELS` as the canonical *(phase, default-name, color)* table and add `fn github_status_color(phase: TrackerPhase) -> &'static str`. A custom-named label inherits its phase's canonical color (grey/blue/yellow/green) via the same `--force` ensure-create — so a renamed pipeline still reads at a glance, and `--force` self-heals a manually recolored label. Per-name custom colors are a follow-up nobody asked for (YAGNI).

### Widened builders — the invariant under custom names

```rust
/// Set-one/remove-others against the CONFIGURED set. The remove-filter is by
/// NAME (not phase): if a user maps two phases to one name, the target must
/// never appear in its own remove list. Dedup so no name repeats.
fn gh_set_status_label_argv<'a>(
    number: &'a str, slug: &'a str, phase: TrackerPhase, map: &'a GithubStateMap,
) -> Vec<&'a str>
// ["issue","edit",number,"--repo",slug,"--add-label",map.label_for(phase),
//  "--remove-label",<each OTHER configured name, name != target, deduped>]

async fn github_transition_with(
    program: &str, slug: &str, number: &str, phase: TrackerPhase, map: &GithubStateMap,
) -> TransitionResult
// ensure loop: for each phase p → gh_label_ensure_argv(map.label_for(p), slug,
// github_status_color(p)), skipping duplicate names; then the single edit.
// Ok/Skipped semantics unchanged — still never Err.
```

The arm (`task_sink.rs:432-444`): resolve the map **after** the URL parse succeeds (so the no-url skip never touches the config file — keeps `github_transition_without_url_is_skipped` hermetic):

```rust
let map = GithubStateMap::from_env();
Ok(github_transition_with(&gh_bin(), &slug, &number, phase, &map).await)
```

`github_status_label(phase)` stays as the *default* name accessor (delegating to `GithubStateMap::default()`), used by F1's Todo assertions and back-compat tests.

**Mid-flight map change (the invariant, precisely):** after any transition, **exactly one label from the currently-configured set** is present. A label applied under an *older* map whose name is no longer configured is foreign by definition and is never touched — same protection class as `status/qa*` (004 C4). Removing it would require read-modify-write over arbitrary `status/*` names, which is exactly the foreign-label hazard the deterministic remove-set exists to prevent. Documented on the builder.

### Settings write path (D4)

`crates/agentum-desktop/src/commands/github_labels.rs`, mirroring `linear.rs:467-498` — including the flat-args rule (a `request: Struct` param silently rejects the invoke):

```rust
#[tauri::command] pub fn github_get_state_map() -> Value
// stored overrides filled with the canonical defaults → {"todo": "...", "inProgress": "...", "readyToTest": "...", "done": "..."}
#[tauri::command] pub fn github_set_state_map(
    todo: Option<String>, in_progress: Option<String>,
    ready_to_test: Option<String>, done: Option<String>,
) -> Value  // blank clears the override; returns the effective map
```

Register both in the `generate_handler![]` list; `ui/src/tauri/github-labels.ts` binds them; `IntegrationsPane.tsx` renders a GitHub card with four inputs directly mirroring the Linear one (load via `getStateMap`-equivalent at mount, save on blur/Save — copy the `:48/:78` flow). The server reads the file fresh per transition (`from_env` in the arm), so Settings edits apply on the next transition with no restart — same freshness contract as Linear.

### Unit-test plan (F5)

1. `github_state_map_defaults_are_canonical` — equals the `GITHUB_STATUS_LABELS` names.
2. `github_state_map_precedence_file_then_env` — via pure `apply_layers` with injected closures: file overrides defaults; env overrides file; blank/whitespace layers keep the lower layer. **No env mutation.**
3. `gh_set_status_label_argv_uses_configured_names` — fully-renamed map: target = custom name, removes = the other three custom names, **no canonical default appears anywhere in the argv** (the mid-flight/foreign-label pin at argv level).
4. `gh_set_status_label_argv_never_removes_the_target_on_name_collision` — two phases share one name: the target is added and absent from every `--remove-label`.
5. Update `gh_set_status_label_argv_adds_one_removes_exactly_the_other_three` (`task_sink.rs:764`) — pass `&GithubStateMap::default()`; the argv must be **byte-identical** to today's (the F5-changes-nothing-by-default regression pin).
6. `github_transition_with_custom_map_flips_configured_names` (`#[cfg(unix)]`, fake-`gh` script pattern from `task_sink.rs:822+`) — 5 invocations, ensure-creates carry custom names + canonical colors, the edit adds/removes only configured names; arity-update the existing fake-gh tests.
7. Desktop: `npm run build` green (the Tauri commands compile under `cargo build -p agentum-desktop`).

---

## 7. Cross-cutting risks & invariants

- **One launch path:** F1 spawns nothing itself — `drive` → `spawn_feature_agent` → `spawn_agent_into_pane` (`drive.rs:396`) is untouched; the composer's plain launch is *suppressed*, not replaced. `inject_prompt`, settle detection, YOLO, trust-dialog handling: zero diffs.
- **The gate is sacred:** F1 wires into `drive` as-is; F3 changes prompt wording and the Auto *capability input* only — the verdict-file fail-closed contract and both gate loops are untouched.
- **Best-effort tracker (sacred):** the Todo-at-plan call swallows to a `warn!` (board_goals pattern); F4 maps even seam `Err`s to text; F5's arm still returns only `Ok(Applied|Skipped)`. No caller can be halted by a tracker.
- **Registry serde hazard:** no registry/`CreateBody` change anywhere in 005 — the `Worktree` struct stays alias-free by construction (confirmed §1).
- **Double-driver:** per-run `claim_driver` + the new `start_work_lock` + fs-mutation-after-guard ordering (C5). The friendly state is a `200` with `alreadyRunning: true`, never a 400 toast loop.
- **Prompt regressions:** three byte/contract pins (F2 no-spec, F2 explicit-prompt, F3 verdict contract) are named tests, written FIRST.
- **D3 safety:** the knob default-OFF keeps `Auto`'s truth table byte-identical (pinned by the matrix test's `capable=false` column); headless/CI unchanged by construction.
- **Config-file test hygiene:** `GithubStateMap::from_env` is only reachable in tests through the URL-parse-guarded arm; precedence tests use `apply_layers` injection; anything that must touch the file sets `AGENTUM_GITHUB_CONFIG` to a tempdir path (mirror `board_goals.rs:1068`'s `AGENTUM_LINEAR_CREDS` isolation; take the env lock if mutating env).
- **Composer skips must not leak to non-gated flows:** the three-path skip keys on one explicit `gatedRun` flag threaded from the armed toggle; `planCreatedWorkspaceOpen` pins the default path unchanged.

## 8. Build order and gates

Order per the spec's harness wiring (F1 → F5; **F2, F4, F5 independently shippable**; F3's prompt half is independent, its knob half depends on nothing else either — the listed order is value-first, not a dependency chain). `verify.sh` = `cargo test -p agentum-server --lib` + `npm run build --prefix crates/agentum-desktop/ui`.

| # | Feature | Done when |
|---|---|---|
| F1 | `start-work-gated-run` | §2 tests green; `spec_from_issue` still 400s on existing (converge only via start-work); the run route's spawn line and `drive_inner` show **zero** diffs; composer/Tasks QA flow per qa.sh |
| F2 | `spec-aware-feature-prompt` | §3 tests green; `feature_prompt_without_spec_is_byte_identical` passes against the pre-change literal |
| F3 | `qa-agentum-browser` | §4 tests green; `resolve_qa_mode_matrix` full 12 cells; setting defaults OFF |
| F4 | `mcp-report-status` | §5 tests green; catalog arithmetic test updated; tool ungated |
| F5 | `github-state-map` | §6 tests green; default-map argv byte-identical; Settings card writes `github.json` |

`qa.sh` (browser gate, from the spec): chat→issue→Tasks row "Start gated run"→composer submit→workspace has spec+backlog, **exactly one agent** (the engine's), issue shows `status/in-progress`; repeat with a custom-named map; green feature → `status/ready-to-test`; `agentum_browser`-written verdict → `status/done`, issue still open.

## 9. Handoff to Developer (sdd-developer)

- **Completed:** all seams line-verified on `7e9afaa4`; D1–D4 honored; C1–C5 corrections; every design question from the PM handoff answered (route = `/api/harness/start-work`; stamp = `plan_from_spec_inner`; knob = store setting + pure `resolve_qa_mode`; F4 arg/outcome split; F5 name-filtered remove-set + phase-keyed colors).
- **Key decisions to not re-litigate:** already-running check before any fs write, under the engine `start_work_lock`; stale-idle runs stopped + re-registered (never refreshed in place); Todo transition in `ensure_spec_and_plan` (route layer, `&Store` in scope), not in `types.rs`; the three plain-delivery skips centralized in `open-created-workspace.ts` behind one `gatedRun` flag; `github.json` map resolved after the URL parse.
- **First failing test to write:** `feature_prompt_without_spec_is_byte_identical` (F2 — cheapest pin, guards the riskiest string edit), then `resolve_qa_mode_matrix`.
- **Reviewer focus:** zero diffs in `drive_inner`'s control flow and `spawn_*`; `Ok(Skipped)`-never-`Err` in every tracker path incl. the F4 text mapping; the byte-identical pins; no `is_public` additions; flat Tauri args on the two new commands; the C5 ordering.

**Key files:** `crates/agentum-server/src/routes/harness.rs`, `crates/agentum-server/src/harness.rs`, `crates/agentum-server/src/harness/types.rs`, `crates/agentum-server/src/harness/drive.rs`, `crates/agentum-server/src/harness/helpers.rs`, `crates/agentum-server/src/task_sink.rs`, `crates/agentum-server/src/routes/mcp.rs`, `crates/agentum-desktop/src/commands/github_labels.rs` (new), `crates/agentum-desktop/ui/src/lib/open-created-workspace.ts`, `ui/src/hooks/useComposerState.ts`, `ui/src/components/TaskPage.tsx`, `ui/src/components/NewWorkspaceComposerCard.tsx`, `ui/src/runtime/harness-client.ts`, `ui/src/components/settings/IntegrationsPane.tsx`.
