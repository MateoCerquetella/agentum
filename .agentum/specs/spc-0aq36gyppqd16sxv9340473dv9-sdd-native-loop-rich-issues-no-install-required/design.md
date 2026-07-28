# Spec 006 — Architecture

**Self-check passed.** Load-bearing cites re-verified on this worktree (`finish-the-loop`, HEAD `47ef487e`): `routes/github.rs:32-36` (router), `:69-75` (`FetchedIssue`), `:82-148` (`fetch_github_issue` — slug via `resolve_github_slug`, `gh_in_dir` from `neutral_cwd`), `:164-176` (`CreateIssueBody`, no labels field), `:178-185` (`CreateIssueResponse`), `:236-240` (the hardcoded `labels: Vec::new()` — the #232 root cause), `:222-234` (typed 422 `no_github_repo`), `:265-313` (tests mod incl. `create_issue_rejects_blank_title`), `task_sink.rs:24-32` (`NewFeature.labels`), `:156-194` (the GitHub create arm), `:645-691` (`gh_create_argv`/`_with_repo`/`push_label_args`), `:823-850` (`gh_create_argv_appends_labels`), `routes/chat.rs:866` (`EXTRACT_INSTRUCTIONS`), `:911-928` (`SubTask`/`FeaturePlan`, Deserialize-only), `:973-1001` (`compose_issue_body` — exact composition), `:1031-1057` (`plan_to_features`), `:1182-1215` (`chat_issues_preview` returns an EXPLICIT `{title,summary,tasks,body}` json — drops unknown fields), `:1621-1660` (the existing composition test), `harness/types.rs:108-170` (`FeatureList` incl. `spec_id:157`, `roles:163`), `:185-203` (defaults, `roles:false`), `:211-239` (`copy_knobs_from`), `:860` (`pub fn derive_backlog_from_spec`), `:905-960` (`plan_from_spec`/`_with_tracker`/`_inner` — tracker stamping at `:948-953`), `:963-983` (`update_backlog_knobs`), `:1042-1075` (`spec_md_from_issue` — body verbatim, checkbox fallback), `harness/drive.rs:78-89` (the `roles && spec_id.is_some()` gate), `:151-161` (spec-steer prompt), `:326-338` (`transition_tracker` — **silent no-op when `tracker_provider` is `None`**), `:589-594` (`read_spec_md` ← `<harness_dir>/specs/<spec_id>/spec.md`), `:647-808` (`run_role_gate` — spawn, verdict file, fail-closed), `:813-852` (`run_pre_feature_phases` — decompose calls **tracker-less** `plan_from_spec` at `:846`), `harness/helpers.rs:39-71` (`build_feature_prompt`), `:76-99` (`RoleVerdict` + `parse_role_verdict`), `:105-128` (`build_role_prompt` — the "HOW TO RECORD YOUR VERDICT" block at `:116-121`), `harness_roles/{pm,architect,reviewer}.md` (read fully), `harness.rs:1946-1998` (verdict/brief tests incl. `role_briefs_are_embedded_and_demand_a_verdict`), `routes/harness.rs:31` (`BROWSER_QA_ENABLED_SETTING`), `:33-48` (router incl. `/api/harness/settings` and `/api/harness/start-work`), `:50-84` (`HarnessSettings` + get/put), `:410-439` (`StartWorkRequest/Response`), `:448-566` (`start_work` — knob write at `:521-530`), `:774-814` (settings tests incl. the **exact-string** camelCase pin at `:810`), `agentum-store/src/settings.rs:37-52` (`setting_get_bool(key, default)`), `.github/labels.sh:9-33` (the canonical `type/*`+`priority/*` set), UI: `useComposerState.ts:495-498` (`agentPrompt`/`note` state + `agentPromptRef:667`/`noteRef:656`), `:1415-1417` (`canCreateGithubIssue`), `:1437-1510` (`handleCreateIssueSubmit` — the author-less snapshot at `:1464-1492`), `:1275-1300` (`applyLinkedWorkItem` stores `{type,number,title,url}` only), `lib/new-workspace.ts:51-63` (`LinkedWorkItemSummary` — no author/labels), `shared/types.ts:1003-1012` (`GitHubWorkItem.author: string | null`), `runtime/github-issue-client.ts:60-108` (`CreatedGithubIssue` + `createGithubIssue`), `GitHubItemDialog.tsx:892/:938` (`workItem.author ?? 'unknown'`), `agentum-desktop/src/commands/gh.rs:255-263` (LIST `--json` fields **include `author`**), `:190-203` (`map_issue` maps `author.login`), `HarnessEngine.tsx:277-318` (`PhaseStrip` keys on `status.features.roles` at `:292`), `NewWorkspaceComposerCard.tsx:585-644` (create-issue form), `:668-689` (gated-run toggle + armed copy), `IntegrationsPane.tsx:233-290` (`BrowserQaGateToggle` — the pattern to clone), `runtime/harness-client.ts:245-258` (`HarnessSettings` client), `ChatPage.tsx:418-455` (draft plan round-trip: preview → `setDraftPlan` → Confirm posts `plan` verbatim via spread), `ai/skills/validate_handoff.md` + `ai/skills/write_spec.md` (read fully for the AC 7 deltas).

**Status:** Architect → ready for Developer. D1–D3 honored; four corrections below — C1 is material (a latent spec-013 bug that F3 would activate), the rest narrow or re-route work.

---

## 0. TL;DR — four features, one sentence each

1. **F1** — `CreateIssueBody` gains serde-default `labels: Vec<String>` threaded into `NewFeature.labels` (argv plumbing exists); new thin `GET /api/github/labels` (same slug resolution as create, `gh label list --json name`, static `type/*`+`priority/*` fallback client-side); composer form gains a chip-toggle label picker + a pure `composeIssueContextBody(agentPrompt, note)` blank-body auto-fill under `## Context`.
2. **F2** — `FeaturePlan` gains optional serde-default `problem`/`goal`; `compose_issue_body` renders `## Problem` / `## Goal` / `## Acceptance criteria` when either is present and is **byte-identical** when both absent (pinned); the preview endpoint + UI `DraftPlan` pass the two fields through Confirm; an SDD-shaped fixture round-trips through `spec_md_from_issue` → `derive_backlog_from_spec`.
3. **F3** — `SDD_ROLES_ENABLED_SETTING` (default **ON**, read exactly once in `start_work`'s post-plan knob write via a testable `apply_start_work_knobs`); `HarnessSettings` widens (GET full / PUT patch — keeps old clients valid); role briefs get exact gate-checklist deltas (content below) with the verdict contract pinned character-identical; **plus C1's fix**: decompose preserves tracker provenance so the label trail survives roles-on runs.
4. **F4** — `CreateIssueResponse` gains `author` (best-effort `gh api user --jq .login`, `None` on any failure); the composer snapshot at `useComposerState.ts:1464-1492` populates it; the Tasks LIST payload already carries author (verified — no fix needed there).

---

## 1. Corrections & contradictions (read before building)

- **C1 — Flipping `roles: true` (F3) would silently kill the spec-004/005 tracker label trail; the fix is in-scope for F3.** With roles on, `run_pre_feature_phases` re-derives the backlog at Decompose via tracker-less `plan_from_spec` (`drive.rs:846`), producing features with `tracker_provider: None`; `copy_knobs_from` (`types.rs:211-239`) copies only list-level knobs, never per-feature stamps; `transition_tracker` (`drive.rs:336-338`) then no-ops silently — so a start-work run (whose backlog `ensure_spec_and_plan` tracker-stamped) would stop flipping `status/*` labels the moment F3 defaults roles on. This is a latent spec-013 gap, dormant until now because `roles` was only ever true on non-tracker SDD-intake runs. **Fix (small, F3):** a helper `shared_tracker_provenance(&FeatureList) -> Option<(String, String)>` in `types.rs`; Decompose calls `plan_from_spec_with_tracker` when it returns `Some` (§4). The verdict-file contract and phase machine stay untouched — this changes what Decompose *feeds* the existing planner, not how any gate runs.
- **C2 — `HarnessSettings` widening collides with two existing contracts; resolved by a GET-struct / PUT-patch split.** The wire pin test asserts the **exact string** `{"browserQaAgentEnabled":true}` (`routes/harness.rs:810`), and the PUT struct requires every field — so naively adding a required `sdd_roles_enabled` breaks the pin AND makes the existing UI call `setHarnessSettings({ browserQaAgentEnabled: value })` a 422. Resolved: `HarnessSettings` (GET + PUT response) gains the field and the pin test updates to the new exact two-field string; PUT deserializes a new `HarnessSettingsPatch` with two `Option<bool>`s (serde-default) and writes only the present keys, returning the full effective settings. Existing one-field PUTs stay valid (pinned), and two Settings toggles can't clobber each other.
- **C3 — AC 9's open question is answered: the Tasks LIST payload does NOT lack author.** `gh_list_work_items` requests `author` in `--json` for both issues and PRs (`agentum-desktop/src/commands/gh.rs:258/:262`) and `map_issue`/`map_pr` map `author.login` (`:200/:227`). F4 therefore lands **only** in the create response + the composer snapshot (D3 as locked) — no list-side change, and the `?? 'unknown'` fallback stays for genuinely unknown authors.
- **C4 — F2 needs a UI passthrough or Confirm drops the new fields.** `chat_issues_preview` builds its response **explicitly** (`chat.rs:1209-1214` — `{title, summary, tasks, body}`), and the Chat UI stores that object as `draftPlan` and posts it back verbatim as `plan` on Confirm (`ChatPage.tsx:423/:455`; task edits spread `...p`, preserving siblings). So: the preview JSON must include `problem`/`goal` (nullable) and the UI `DraftPlan` type must declare them optional — pure passthrough, **no editing UI** (YAGNI; the composed `body` preview already shows them rendered). Without this, only the no-`plan` (direct-extraction) path would ever file SDD-shaped bodies.

Confirmed for the PM: `run_pre_feature_phases` **works against a start-work-written spec** — `ensure_spec_and_plan` writes `<workdir>/.agentum-harness/specs/<spec_id>/spec.md` and stamps `FeatureList.spec_id`; `run_role_gate` reads exactly `config.harness_dir.join("specs").join(spec_id).join("spec.md")` (`drive.rs:591-593`, `:667`), embeds it in the role prompt, and the PM agent (spawned with cwd = the same workdir, `drive.rs:620`) refines that file in place; Decompose re-derives from the refined file. Same workdir, same paths — no gap beyond C1.

---

## 2. F1 — `rich-issue-create` (AC 1–3)

### Boundaries

| File | Change |
|---|---|
| `crates/agentum-server/src/routes/github.rs` | `CreateIssueBody` gains `#[serde(default)] labels: Vec<String>` (thread into `NewFeature`); new `GET /api/github/labels` route + `list_labels` handler + pure `parse_label_names`; tests |
| `crates/agentum-desktop/ui/src/runtime/github-issue-client.ts` | `createGithubIssue` input gains `labels?: string[]`; new `fetchGithubRepoLabels` |
| `crates/agentum-desktop/ui/src/lib/issue-context-body.ts` (new) | pure `composeIssueContextBody` + `STATIC_FALLBACK_LABELS` + vitest |
| `crates/agentum-desktop/ui/src/lib/new-workspace.ts` | `LinkedWorkItemSummary` gains `labels?: string[]` (optional — additive, no consumer breaks) |
| `crates/agentum-desktop/ui/src/hooks/useComposerState.ts` | `createIssueLabels`/`createIssueLabelOptions` state; fetch options on form open; blank-body fallback in `handleCreateIssueSubmit`; labels on both snapshot objects |
| `crates/agentum-desktop/ui/src/components/NewWorkspaceComposerCard.tsx` | chip-toggle label row in the create-issue form (`:597-644` area); props threaded |

### Server seams

```rust
// routes/github.rs — the widening (AC 1). Absent/empty labels deserialize to
// Vec::new(), and push_label_args() no-ops on empty — wire and argv stay
// byte-identical (the existing task_sink pins already cover the argv half).
struct CreateIssueBody {
    title: String,
    #[serde(default)] body: Option<String>,
    workdir: String,
    #[serde(default)] slug: Option<String>,
    /// Spec 006 F1: labels applied at creation via the existing `gh --label`
    /// plumbing (task_sink.rs). Absent = today's behavior, byte-identical.
    #[serde(default)] labels: Vec<String>,
}
// in create_issue: NewFeature { title, body: …, labels: body.labels.clone() }
```

```rust
// routes/github.rs — D2's thin new seam. Same shape as fetch_github_issue:
// host → resolve_github_slug (typed 422 `no_github_repo` on miss, mirroring
// create_issue so the UI branches on one code) → gh from neutral cwd.
.route("/api/github/labels", get(list_labels))

#[derive(Debug, Deserialize)]
pub struct LabelsQuery { pub workdir: String, pub slug: Option<String> }

#[derive(Debug, Serialize)]
pub struct LabelsResponse { pub labels: Vec<String> }

async fn list_labels(State(state): State<AppState>, Query(q): Query<LabelsQuery>)
    -> Result<Json<LabelsResponse>, ApiError>
// argv: ["label","list","--repo",slug,"--json","name","--limit","100"]
// via crate::host_runtime::gh_in_dir(&host, &neutral_cwd, …). gh failure →
// 400 ("`gh label list` failed", stderr warn-logged) — the picker treats ANY
// error as "use the static fallback", so no typed envelope needed there.

/// Pure: map `gh label list --json name` output ([{"name": …}]) to names —
/// skip nameless entries, sort case-insensitively, dedup. Unit-tested.
fn parse_label_names(stdout: &[u8]) -> anyhow::Result<Vec<String>>
```

No label *creation* (spec 004 D3 stands): the picker offers existing names only; `--label` with an unknown name makes `gh` fail loudly — acceptable because the picker is seeded from the live set (or the static set, which `.github/labels.sh` keeps synced).

### UI — picker, fallback, auto-fill

- **Client:** `fetchGithubRepoLabels(input: { workdir: string; slug?: string; timeoutMs?: number }): Promise<string[]>` in `github-issue-client.ts` (mirror `fetchGithubIssueBody`'s abort-budget shape, 6 s default).
- **Static fallback (D2), grounded in `.github/labels.sh:9-33`:**

```ts
export const STATIC_FALLBACK_LABELS = [
  'type/feat', 'type/fix', 'type/perf', 'type/refactor', 'type/docs', 'type/test', 'type/chore',
  'priority/p0', 'priority/p1', 'priority/p2', 'priority/p3',
] as const
```

- **State (useComposerState):** `createIssueLabels: string[]` (selection, reset to `[]` on submit-success and on form close) and `createIssueLabelOptions: string[] | null` (null = loading). In `handleCreateIssueOpenChange(true)`: fire `fetchGithubRepoLabels({ workdir: selectedRepo.path })`, `.catch(() => [...STATIC_FALLBACK_LABELS])` — fetch once per open; a repo change while open refetches (keyed effect or refetch in the handler — developer's choice, behavior pinned by QA not unit tests).
- **Form UI:** a wrap row of toggleable chips between the body `<textarea>` and the error row (`NewWorkspaceComposerCard.tsx:615` area); selected chips render filled. Empty selection = no row of applied labels later.
- **Blank-body auto-fill (AC 3), exact format** — pure helper, `lib/issue-context-body.ts`:

```ts
/** Deterministic assembly (spec 006 F1, AC 3) — NO agent call. Context is
 *  DEFINED as the composer's typed agent-prompt field + note field. */
export function composeIssueContextBody(agentPrompt: string, note: string): string | undefined
// p = agentPrompt.trim(); n = note.trim()
// both empty            → undefined                    (no body — today's path)
// sections = ['## Context', ...(p ? [p] : []), ...(n ? [`**Note:** ${n}`] : [])]
// return sections.join('\n\n')                         (no trailing newline)
```

  So both present renders exactly: `## Context\n\n<prompt>\n\n**Note:** <note>`. In `handleCreateIssueSubmit`: `const body = createIssueBody.trim() || (composeIssueContextBody(agentPromptRef.current, noteRef.current) ?? '')` — use the **existing refs** (`:656`, `:667`) so the callback's deps don't grow per keystroke. The `linkedContext` snapshot (`:1483`) uses this same effective body — what got filed is what's snapshotted.
- **Created-issue chip (AC 2):** thread the applied selection into both snapshot objects — the `applyLinkedWorkItem(… as unknown as GitHubWorkItem)` cast gains `labels: createIssueLabels` (`GitHubWorkItem.labels` is already a required field, `shared/types.ts:1010`), and `LinkedWorkItemSummary` gains optional `labels?: string[]` set on the summary at `:1470`. Render: follow `linkedWorkItem` from `useComposerState`'s return to the chip that shows `#<number> <title>` after a create (the linked-item affordance in the composer) and append a compact label-name row when `labels?.length`. The data seam is fixed here; the render is a one-component addition at that site.

### Unit-test plan (F1)

1. `create_issue_body_labels_default_empty` (routes/github.rs, extend the existing deserialization test): body JSON without `labels` → `Vec::new()`; with `["type/feat","priority/p1"]` → carried verbatim. (The argv byte-identity for empty labels is already pinned by `gh_create_argv_is_noninteractive` + `gh_create_argv_appends_labels`, `task_sink.rs:789/:823` — do not duplicate.)
2. `parse_label_names_maps_sorts_and_skips_nameless` — `[{"name":"b"},{"name":"A"},{}]` → `["A","b"]`.
3. UI vitest (`lib/issue-context-body.test.ts` — colocate under `lib/`, no xterm imports): both blank → `undefined`; prompt only → `'## Context\n\nfix the parser'`; note only → `'## Context\n\n**Note:** from PR #7'`; both → the exact two-section string above; whitespace-only inputs count as blank.
4. `npm run build --prefix crates/agentum-desktop/ui` green.

---

## 3. F2 — `chat-sdd-shape` (AC 4–5)

### Boundaries

| File | Change |
|---|---|
| `crates/agentum-server/src/routes/chat.rs` | `FeaturePlan` gains `problem`/`goal`; `EXTRACT_INSTRUCTIONS` names them; `compose_issue_body` three-section rendering; `chat_issues_preview` returns them (C4); tests |
| `crates/agentum-desktop/ui/src` (Chat draft type) | `DraftPlan` (follow `previewIssuesFromChat`'s return type) gains `problem?: string | null; goal?: string | null` — passthrough only, no editor |

### Seams

```rust
// chat.rs — serde-default so a terse model reply, an old client's plan, and
// every existing fixture still parse; Deserialize-only stays (the preview
// response is hand-built json!, C4).
struct FeaturePlan {
    title: String,
    #[serde(default)] summary: String,
    /// Spec 006 F2: SDD framing. Optional — absent keeps compose_issue_body
    /// byte-identical to the pre-006 body (pinned).
    #[serde(default)] problem: Option<String>,
    #[serde(default)] goal: Option<String>,
    tasks: Vec<SubTask>,
}
```

**`EXTRACT_INSTRUCTIONS` (AC 4):** extend the JSON schema in the prompt to `{"title": string, "summary": string, "problem": string, "goal": string, "tasks": […]}` and append two field specs after the existing ones: `problem = 1–3 sentences naming the user-felt problem this feature solves (no solution language); goal = ONE sentence naming the concrete user outcome.` Keep every existing sentence otherwise (the "Output ONLY the raw JSON" tail is load-bearing for `extract_feature_plan`). The parser tolerates omission by construction (serde-default).

**`compose_issue_body` — exact rendering rule (AC 4):**

```rust
// Present-but-blank is absent: a model that emits "" must not flip the shape.
let problem = plan.problem.as_deref().map(str::trim).filter(|s| !s.is_empty());
let goal    = plan.goal.as_deref().map(str::trim).filter(|s| !s.is_empty());
let sdd = problem.is_some() || goal.is_some();
```

- `!sdd` → **today's exact composition**, character for character (summary lead, `## Sub-tasks (priority order)`, the same task-line format, the same footer).
- `sdd` → in order: summary lead (unchanged, when non-blank) → `## Problem\n\n{problem}\n\n` (when present) → `## Goal\n\n{goal}\n\n` (when present) → `## Acceptance criteria\n\n` followed by the **identical** task-line rendering (`- [ ] **[{Priority}]** {title}` + optional ` — {detail}`, same stable priority sort) → the same `\n_Created from an agentum Chat feature breakdown._` footer. The only delta versus today inside the checklist block is the heading name — the `- [ ]` lines are shared code, which is what makes AC 5 hold by construction.

**Preview + UI passthrough (C4):** add `"problem": plan.problem, "goal": plan.goal` to the `json!` at `chat.rs:1209-1214` (nullable); widen the UI `DraftPlan` type with the two optional fields. Confirm's POST already spreads the stored object (`ChatPage.tsx:449-455`), and task-edit patches spread `...p` (`:501`), so the fields survive editing untouched. `plan_to_features`, `compose_task_body`, `sanitize_messages`, labels threading, one-issue semantics: **zero diffs**.

### Unit-test plan (F2) — in chat.rs's tests mod

1. `compose_issue_body_without_problem_goal_is_byte_identical` — a plan with summary + 3 prioritized tasks, `problem: None, goal: None`; assert **full-string equality** against the written-out pre-change literal (the AC 4 pin; write it before touching the function).
2. `compose_issue_body_blank_problem_goal_falls_back_to_today` — `problem: Some("  ")`, `goal: Some("")` → equals the test-1 output exactly.
3. `compose_issue_body_renders_problem_goal_and_acceptance_criteria` — both present: contains `## Problem`, `## Goal`, `## Acceptance criteria` in that index order; contains the same `- [ ] **[High]** …` lines; does **not** contain `## Sub-tasks (priority order)`; footer present.
4. `feature_plan_json_defaults_problem_and_goal` — deserialize with and without the fields.
5. `sdd_issue_body_round_trips_through_spec_md_to_backlog` (AC 5) — compose an SDD-shaped body (3 tasks) → `crate::harness::spec_md_from_issue("42", "T", &body, "https://github.com/o/r/issues/42")` → `crate::harness::derive_backlog_from_spec(&spec)` → exactly 3 features whose names contain the task titles, and the spec does NOT contain the fallback `- [ ] T` line (the checkboxes were found). Both helpers are `pub` (`types.rs:860/:1042`).
6. `extract_instructions_names_problem_and_goal` — `EXTRACT_INSTRUCTIONS.contains("\"problem\"")` + `"\"goal\""` (guards a prompt regression without pinning prose).
7. Existing `compose_issue_body_sorts_by_priority_keeps_order_and_renders_checklist` (`:1621`) and all `plan_to_features_*` tests: untouched-green.

---

## 4. F3 — `roles-inherited` (AC 6–8)

### Boundaries

| File | Change |
|---|---|
| `crates/agentum-server/src/routes/harness.rs` | `SDD_ROLES_ENABLED_SETTING` const; `HarnessSettings` + `HarnessSettingsPatch` (C2); `get_settings`/`put_settings` widened; `apply_start_work_knobs` helper called from `start_work`'s knob-write closure (`:521-530`); tests updated/added |
| `crates/agentum-server/src/harness/types.rs` | `shared_tracker_provenance` helper (C1) |
| `crates/agentum-server/src/harness/drive.rs` | Decompose (`:846`) uses `plan_from_spec_with_tracker` when provenance exists (C1) |
| `crates/agentum-server/src/harness_roles/{pm,architect,reviewer}.md` | the exact gate-checklist deltas below (AC 7) |
| `crates/agentum-server/src/harness.rs` (tests mod) | `role_prompt_verdict_contract_is_character_identical` pin |
| `crates/agentum-desktop/ui/src/runtime/harness-client.ts` | `HarnessSettings` gains `sddRolesEnabled`; `setHarnessSettings` takes `Partial<HarnessSettings>` |
| `crates/agentum-desktop/ui/src/components/settings/IntegrationsPane.tsx` | `SddRoleLoopToggle` beside `BrowserQaGateToggle` (clone its load/optimistic-write shape; initial `useState(true)` — the default is ON) |
| `crates/agentum-desktop/ui/src/hooks/useComposerState.ts` + `NewWorkspaceComposerCard.tsx` | fetch `getHarnessSettings()` when `canStartGatedRun` (best-effort, optimistic `true`); armed copy names the role loop (AC 8) |

### The knob (D1 — default ON, read once, start-work only)

```rust
// routes/harness.rs
/// Spec 006 F3 (D1): when true, start-work-planned backlogs run the SDD role
/// loop (PM gate → Architect gate → Decompose → Execute → Review gate).
/// Default ON — the loop is the product working as designed; this is the
/// global opt-out. Read EXACTLY ONCE, in start_work's post-plan knob write:
/// `roles` is a backlog knob stamped into feature_list.json, never a
/// per-drive-tick read — manually registered runs are untouched.
pub const SDD_ROLES_ENABLED_SETTING: &str = "harness.sdd.roles.enabled";
```

```rust
/// start_work's post-plan knobs in one pure, pinned place (spec 006 F3).
/// `sdd_roles` only ever SETS roles (plan resets the list to defaults, so
/// false is already the resting state — never write `false` explicitly).
fn apply_start_work_knobs(
    list: &mut crate::harness::FeatureList,
    agent_tool: Option<&str>,
    agent_model: Option<&str>,
    sdd_roles: bool,
) {
    if let Some(t) = agent_tool { list.agent_tool = t.to_string(); }
    if let Some(m) = agent_model { list.agent_model = Some(m.to_string()); }
    if sdd_roles { list.roles = true; }
}
```

In `start_work`, immediately before the existing `update_backlog_knobs` call (`:521`):

```rust
// D1: the one and only read. A store hiccup falls back to the default (ON).
let sdd_roles = state.store
    .setting_get_bool(SDD_ROLES_ENABLED_SETTING, true)
    .await.unwrap_or(true);
let list = crate::harness::update_backlog_knobs(&workdir, |list| {
    apply_start_work_knobs(list, req.agent_tool.as_deref(), req.agent_model.as_deref(), sdd_roles);
}).await…;
```

`spec_id` is already stamped by the plan; `drive_inner`'s existing gate (`drive.rs:78-81`) then turns the phases on with **zero drive-loop diffs**. `Decompose` re-applies knobs via `copy_knobs_from`, so `roles`/`spec_id` survive the mid-run re-plan (verified, `types.rs:236-237`).

### Settings wire (C2)

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HarnessSettings {
    browser_qa_agent_enabled: bool,
    sdd_roles_enabled: bool,          // ← new; declaration order = wire order
}

/// PUT body: partial by design so a caller flipping one knob can't clobber the
/// other (and the pre-006 one-field PUT stays valid — pinned).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HarnessSettingsPatch {
    #[serde(default)] browser_qa_agent_enabled: Option<bool>,
    #[serde(default)] sdd_roles_enabled: Option<bool>,
}
```

`get_settings` reads both (`setting_get_bool(BROWSER_QA…, false)` / `setting_get_bool(SDD_ROLES…, true)` — note the **different defaults**). `put_settings` takes the patch, writes only `Some` fields, then re-reads and returns the full `HarnessSettings`. Client: `setHarnessSettings(patch: Partial<HarnessSettings>): Promise<HarnessSettings>` — the existing `BrowserQaGateToggle` call compiles unchanged.

### C1 fix — Decompose keeps the label trail

```rust
// types.rs — spec 006 C1. All spec-planners stamp provider/url uniformly
// (one issue per spec), so the first stamped feature speaks for the backlog.
pub(crate) fn shared_tracker_provenance(list: &FeatureList) -> Option<(String, String)> {
    list.features.iter()
        .find_map(|f| Some((f.tracker_provider.clone()?, f.tracker_url.clone()?)))
}
```

```rust
// drive.rs:846 — Decompose must not drop tracker provenance (spec 006 C1):
// re-deriving via the tracker-less planner turned every later
// transition_tracker call into a silent no-op on roles-on runs.
let mut derived = match shared_tracker_provenance(&config.features) {
    Some((provider, url)) => plan_from_spec_with_tracker(workdir, &spec_id, &provider, &url).await?,
    None => plan_from_spec(workdir, &spec_id).await?,
};
```

### Brief deltas (AC 7 — exact content; everything not shown stays verbatim)

Brief bytes change by design (`include_str!`, `types.rs:384-386`); the pins are the verdict contract + the `role_briefs_are_embedded_and_demand_a_verdict` invariants ("verdict"/"passed" appear — the retained Output-contract sections keep them true).

**`pm.md`** — replace the six bullets under `## PM gate — the spec passes only if all hold` with (sourced from `ai/skills/validate_handoff.md` + `write_spec.md`; the STATE.md item is deliberately dropped — the engine has no `ai/` scaffold, and `decisions.md` is written by the engine itself):

```markdown
- **One slice.** The goal is one sentence naming a concrete user action, and it is a
  single shippable increment — no hidden "and". If it needs an "and", fail and say split.
- **Problem before solution.** The Problem section names a user-felt pain, not a
  feature or a mechanism.
- **Persona named.** At least one concrete user and the moment they feel the pain.
- **Acceptance criteria are testable.** 3–6 criteria, every one with an observable verb
  (returns, renders, persists, emits, blocks — never "support", "handle", "works"),
  each one checkable by the verification gate.
- **Non-goals stated.** In-scope and out-of-scope are explicit; the spec says what it
  will NOT do.
- **Grounded in code.** Claims about what already exists cite real files/modules of
  THIS project; reuse before build.
- **Invariants respected.** No criterion forces breaking a rule stated in the harness
  instructions (AGENTS.md) above.
- User value is stated in one line.
- The spec fits on one screen; if it doesn't, say it must be split.
- No duplicate or conflict with an existing spec.
```

**`architect.md`** — append three bullets to the existing gate list:

```markdown
- The plan is grounded: every seam it names (file / function / route) was actually
  read and exists — cite it. Never design against an imagined API.
- Every acceptance criterion maps to a named part of the plan AND a named test; a
  criterion with no test is a gap.
- Reuse before build: existing primitives are the default; anything new is justified.
```

**`reviewer.md`** — append two bullets to the existing gate list:

```markdown
- The implementation matches `architecture.md`; any deviation is named and justified
  in your review note, not silent.
- Every judgment cites evidence (files, tests, the diff) — never a verdict from memory.
```

### AC 8 — armed copy + strip

`PhaseStrip` already renders for roles-on runs (`HarnessEngine.tsx:292` — verified, untouched). Composer: when `canStartGatedRun` first becomes true, `getHarnessSettings()` best-effort (optimistic default `true` on failure — honest, since the server default is ON); thread `sddRolesEnabled: boolean` to the card and switch the armed copy (`NewWorkspaceComposerCard.tsx:684`) to: *"Your typed prompt won't be sent — the linked issue becomes the spec and the SDD role loop (PM gate → Architect → Build → Review gate) drives gated agents in the worktree. The selected agent runs inside the engine."* Setting off → today's copy unchanged.

### Unit-test plan (F3)

1. `sdd_roles_setting_defaults_on_and_round_trips` — tempdir Store; unset → `setting_get_bool(SDD_ROLES_ENABLED_SETTING, true)` is `true` (the D1 pin — note the default arg differs from the QA knob's); set `false` → reads `false`.
2. `start_work_knobs_stamp_roles_only_when_enabled` — `apply_start_work_knobs` on a default list: `sdd_roles=true` → `roles == true`; `sdd_roles=false` → `roles == false`; agent/model set only when `Some`; `spec_id`/features untouched.
3. `harness_settings_wire_shape_is_camel_case` **updated** — exact string `{"browserQaAgentEnabled":true,"sddRolesEnabled":true}`.
4. `harness_settings_patch_accepts_partial_puts` — `{"browserQaAgentEnabled":false}` parses with `sdd_roles_enabled == None`; `{}` parses (both None); `{"sddRolesEnabled":false}` parses (the old-client + one-toggle compat pin).
5. `role_prompt_verdict_contract_is_character_identical` — `build_role_prompt(RoleKind::Pm, "I", "s", "S", "roles/authoring.json")` **contains the exact rendered literal**: `=== HOW TO RECORD YOUR VERDICT ===\nWhen finished, WRITE your verdict to `roles/authoring.json` (relative to the project root) as exactly this JSON:\n{"passed": true|false, "summary": "one line on what passed or the single most important gap"}\nSet passed=false if the gate does not pass. Do not stop until the file is written. Do not ask the human anything.` — written BEFORE the brief edits (the AC 7 pin; `parse_role_verdict_*` tests at `harness.rs:1946-1968` already pin the wire shape).
6. `shared_tracker_provenance_reads_stamped_backlog` + `shared_tracker_provenance_none_when_unstamped` (C1; the tracker-stamped re-plan itself is covered by the existing `plan_from_spec_with_tracker` tests).
7. Existing untouched-green nets: `role_briefs_are_embedded_and_demand_a_verdict`, all `parse_role_verdict_*`, `build_role_prompt_includes_brief_spec_and_verdict_contract`, `harness_qa_setting_defaults_off_and_round_trips`, spec-005's start-work tests.

---

## 5. F4 — `author-hydration` (AC 9, D3)

### Boundaries

| File | Change |
|---|---|
| `crates/agentum-server/src/routes/github.rs` | `CreateIssueResponse` gains `author: Option<String>`; best-effort `authenticated_github_login` + pure `parse_gh_login`; tests |
| `crates/agentum-desktop/ui/src/runtime/github-issue-client.ts` | `CreatedGithubIssue` gains `author: string | null` |
| `crates/agentum-desktop/ui/src/lib/new-workspace.ts` | `LinkedWorkItemSummary` gains `author?: string | null` (shared with F1's `labels` widening) |
| `crates/agentum-desktop/ui/src/hooks/useComposerState.ts` | both snapshot objects at `:1464-1492` populate `author: created.author ?? null` (+ F1's labels) |

### Mechanism (the design question, answered)

No existing `gh api user` usage anywhere server-side (verified). The issue's author **is** the authenticated login by definition (we just created it with the user's own `gh` auth), and `gh issue create`'s stdout is only the URL — so: one best-effort `gh api user --jq .login` per create, run through the same `gh_in_dir(&host, &neutral_cwd, …)` runner the route already uses. No cache (a create is click-frequency; a cache would go stale across `gh auth switch` — YAGNI). Any failure (offline, jq missing from old gh — `api --jq` is core, unauthenticated) → `None` → the dialog's `?? 'unknown'` fallback, which **stays** (D3).

```rust
// routes/github.rs
struct CreateIssueResponse {
    provider: &'static str,
    number: i64,
    url: String,
    slug: String,
    /// Spec 006 F4 (D3): the authenticated `gh` login — the creator. Best
    /// effort: None on any failure, never an error (additive; serializes as
    /// `"author":null`, which old clients ignore).
    author: Option<String>,
}

/// Pure: a login is the trimmed, non-empty single token of stdout.
fn parse_gh_login(stdout: &[u8]) -> Option<String>

/// `gh api user --jq .login` from the neutral cwd — best-effort by contract.
async fn authenticated_github_login(host: &agentum_core::Host) -> Option<String>
```

Called in `create_issue` **after** the successful create (a login failure must never fail a created issue; ordering also means no wasted call on a failed create). UI: `applyLinkedWorkItem` cast object and the `LinkedWorkItemSummary` both gain `author: created.author ?? null` — so whichever object the detail view hydrates from carries the login. Per **C3**, the Tasks LIST already carries author; no other change.

### Unit-test plan (F4)

1. `create_issue_response_serializes_author_present_and_null` — `Some("mateo")` → contains `"author":"mateo"`; `None` → `"author":null` (the additive-wire pin).
2. `parse_gh_login_trims_and_rejects_empty` — `b"mateo\n"` → `Some("mateo")`; `b"  \n"`/`b""` → `None`.
3. `npm run build --prefix crates/agentum-desktop/ui` green (type widenings compile everywhere the response/summary is consumed).

---

## 6. Cross-cutting risks & invariants

- **The gate is sacred / verdict contract untouched:** F3 changes *when* the role gates run (a knob at plan time) and what Decompose *feeds* the planner (C1) — `parse_role_verdict`, fail-closed missing/garbled behavior, `decide_gate`, and the phase machine have **zero diffs**. The character-identical prompt pin (§4 test 5) is written before any brief edit.
- **One launch path:** nothing in 006 spawns anything new — role agents already flow through `spawn_agent_into_pane` (`drive.rs:634`).
- **Best-effort tracker (sacred):** C1's fix re-enables transitions, it never adds a failure mode (`transition_tracker` still logs-and-continues); F4's login read is `Option`-typed best-effort; the labels route failure is a client-side fallback, never a blocked create.
- **Byte-identical pins, written first:** F2 absent-fields body (full-literal), F1 absent-labels wire (serde-default + existing argv pins), F3 verdict contract, F3 patch-PUT compat. These are the regression net for the three riskiest string/wire edits.
- **Wire-compat:** all response widenings are additive (`author`, `sddRolesEnabled`, preview `problem`/`goal`); the one request-shape change (PUT settings) becomes *more* lenient (patch semantics, pinned). No `is_public` additions — `/api/github/labels` sits behind the token like its siblings.
- **Chat contract stability:** `sanitize_messages`, one-issue semantics, `plan_to_features`, labels threading: zero diffs; every existing chat test stays green by construction (F2 only adds optional fields + a conditional heading swap).
- **Default-ON divergence (D1) is contained:** `roles` is stamped only inside `start_work`'s knob write — the `start` route, MCP register/plan, and hand-written backlogs can never inherit it; `setting_get_bool(…, true)` makes absence mean ON (mind the different default from the QA knob when copying code).
- **Registry hazard:** no `Worktree`/`CreateBody`/registry change anywhere in 006 — the serde-alias-free rule holds by construction.
- **Developer-gate constraints (from handoff 01, carry into every slice):** `cargo clippy --workspace --all-targets -- -D warnings` green per slice (v0.51.0 tag lesson); any tests mod added/moved goes at **EOF** of its file (`items_after_test_module`); env-lock tests need `#[allow(clippy::await_holding_lock)]` + justification — note: **no test in this plan mutates env** (the settings tests use tempdir Stores; label/argv tests are pure), so the allow should not be needed — if a developer reaches for env mutation, redesign the test instead.

---

## 7. Build order and gates

F1 → F2 → F3 → F4 (per the spec's harness wiring; F2 and F4 are independently shippable; F3 depends on nothing from F1/F2 but is the largest slice — value-first order stands). Per-slice gate: `cargo test -p agentum-server --lib` + `cargo clippy --workspace --all-targets -- -D warnings` + `npm run build --prefix crates/agentum-desktop/ui`.

| # | Feature | Done when |
|---|---|---|
| F1 | `rich-issue-create` | §2 tests green; absent-labels wire pinned; `/api/github/labels` mounted with static fallback client-side; blank-body create with a typed prompt files a `## Context` body; both-blank still files bodyless (pinned) |
| F2 | `chat-sdd-shape` | §3 tests green; `compose_issue_body_without_problem_goal_is_byte_identical` passes against the pre-change literal; preview + `DraftPlan` round-trip the two fields (C4); round-trip fixture green |
| F3 | `roles-inherited` | §4 tests green; verdict-contract pin passes against the new briefs; `shared_tracker_provenance` wired into Decompose (C1); settings PUT accepts partial bodies; PhaseStrip shows phases on a start-work run (QA) |
| F4 | `author-hydration` | §5 tests green; created-issue detail shows the login (QA); `?? 'unknown'` fallback untouched |

`qa.sh` (browser gate, from the spec): composer create-issue with labels + blank body → GitHub shows description + labels, detail shows the author; chat-created issue body has the three sections; Start-gated-run with the roles knob ON shows PM/Architect/Review in the strip, blocks on a failing PM verdict, **and the issue's `status/*` label still flips at InProgress** (the C1 regression check).

## 8. Handoff to Developer (sdd-developer)

- **Completed:** all seams line-verified on `47ef487e`; D1–D3 honored; C1–C4 corrections; every design question from handoff 01 answered (labels seam = `GET /api/github/labels` + `parse_label_names`; auto-fill = client-side pure `composeIssueContextBody`, exact format §2; F2 rendering rule §3 with the preview/DraftPlan passthrough; F3 = `apply_start_work_knobs` + GET/patch-PUT split + exact brief deltas + C1's provenance fix; F4 = `gh api user --jq .login` best-effort, no cache, LIST already has author).
- **Key decisions to not re-litigate:** present-but-blank `problem`/`goal` counts as absent (byte-identity safety); the SDD checklist heading is `## Acceptance criteria` and the `- [ ]` line format is shared code with the legacy branch; PUT is a patch, GET is full (C2); `roles` is only ever *set*, never written false; Decompose's tracker fix takes the first stamped feature's pair; login fetched after the create, never before.
- **First failing tests to write:** `compose_issue_body_without_problem_goal_is_byte_identical` (F2 — guards the riskiest string edit) and `role_prompt_verdict_contract_is_character_identical` (F3 — guards the brief refresh), both against the current code before any edit.
- **Reviewer focus:** zero diffs in `run_role_gate`/`decide_gate`/`parse_role_verdict` and in `drive_inner`'s control flow beyond the one Decompose planner call; the four byte/wire pins; the two different `setting_get_bool` defaults; no `is_public` additions; tests mod at EOF; C4's passthrough actually reaching the Confirm POST.

**Key files:** `crates/agentum-server/src/routes/github.rs`, `crates/agentum-server/src/routes/chat.rs`, `crates/agentum-server/src/routes/harness.rs`, `crates/agentum-server/src/harness/types.rs`, `crates/agentum-server/src/harness/drive.rs`, `crates/agentum-server/src/harness/helpers.rs`, `crates/agentum-server/src/harness_roles/{pm,architect,reviewer}.md`, `crates/agentum-server/src/harness.rs` (tests), `crates/agentum-desktop/ui/src/runtime/github-issue-client.ts`, `crates/agentum-desktop/ui/src/runtime/harness-client.ts`, `crates/agentum-desktop/ui/src/hooks/useComposerState.ts`, `crates/agentum-desktop/ui/src/components/NewWorkspaceComposerCard.tsx`, `crates/agentum-desktop/ui/src/components/settings/IntegrationsPane.tsx`, `crates/agentum-desktop/ui/src/lib/new-workspace.ts`, `crates/agentum-desktop/ui/src/lib/issue-context-body.ts` (new), `crates/agentum-desktop/ui/src/components/harness/ChatPage.tsx`.
