# Architecture — Spec 379: Per-project tracker picker (GitHub / Linear / None)

> Grounded against the tree at `bb25a97d` (v0.78.0). Every seam below was read;
> citations are file:line at that revision. Note: `apply_tracker_transition`
> gained a required `TrackerEmit` param (spec 014) — the plan targets the
> current 6-arg signature, not the pre-014 one the spec text implies.

## Summary of the design

The choice is **UI-owned, request-threaded**: a new optional
`Repo.trackerProvider` field (mirroring the existing per-repo
`issueSourcePreference` pattern, `ui/src/shared/types.ts:75-99`) is persisted in
the desktop's repo metadata and threaded into the two plan requests the way
`agentTool`/`agentModel` already are (`StartWorkRequest`, spec 005 D2,
`routes/harness.rs:452-465`). Server-side, a tiny pure `TrackerChoice`
parse/dispatch lives in `task_sink.rs`, plus one new `"none"` provider arm in
the transition seam. No new storage, no new route, no changes to
`harness/drive.rs` or `linear.rs`.

## Components (files to touch)

### Desktop UI (`crates/agentum-desktop/ui/src`)

1. **`shared/types.ts`** — add
   `export type TrackerProviderPreference = 'auto' | 'github' | 'linear' | 'none'`
   and `Repo.trackerProvider?: TrackerProviderPreference` (undefined ⇒ `'auto'`,
   exactly the forward-compat convention documented on
   `issueSourcePreference`, types.ts:96-99).
2. **`components/settings/RepositoryPane.tsx`** — a new `SearchableSetting`
   section "Tracker" (gated `!isFolder`, like Default Worktree Base,
   RepositoryPane.tsx:251) with a 4-option select — "Auto (detect)" /
   "GitHub" / "Linear" / "None" — value `repo.trackerProvider ?? 'auto'`,
   persisted via the existing `updateRepo(repo.id, { trackerProvider })` prop
   (the same write path Display Name uses, RepositoryPane.tsx:220-228).
   Reopening renders the saved value because the control reads straight from
   the persisted `Repo` record (AC 2).
3. **`components/settings/repository-search.ts`** — add the "Tracker" entry to
   `getRepositoryPaneSearchEntries` so settings search finds the new section
   (the pane filters sections by these entries, RepositoryPane.tsx:120-144).
4. **`runtime/harness-client.ts`** — `startGatedWork` input gains
   `tracker?: TrackerProviderPreference`; body gains
   `...(input.tracker ? { tracker: input.tracker } : {})`
   (harness-client.ts:171-188).
5. **`runtime/github-issue-client.ts`** — same optional `tracker` field on
   `scaffoldSpecFromIssue` (github-issue-client.ts:238).
6. **`hooks/useComposerState.ts`** — thread `selectedRepo?.trackerProvider`
   into both call sites: `maybeStartGatedRun` (useComposerState.ts:2302) and
   `maybeScaffoldSpecFromIssue` (useComposerState.ts:2265). `selectedRepo` is
   already in scope at both.

### Server (`crates/agentum-server/src`)

7. **`task_sink.rs`** —
   - New pure `TrackerChoice` enum (`Auto | Github | Linear | None`) +
     `parse_tracker_choice(Option<&str>) -> Option<TrackerChoice>`:
     absent/`"auto"` → `Auto`; `"github"`/`"linear"`/`"none"` → their variant;
     anything else → `None` (the routes turn that into a 400 — never a silent
     fallback). Two immediate consumers (routes/harness.rs,
     routes/board_goals.rs), so this is shared code, not speculation.
   - `transition_inner` (task_sink.rs:878-885) gains an explicit `"none"` arm
     **before** the `other =>` fallthrough (task_sink.rs:951), returning
     `Ok(TransitionResult::Skipped("tracker disabled for this project"))`.
     Same arm added to `apply_blocked_transition`'s match (alongside its
     existing `"board" | "linear"` no-blocked-state skip).
8. **`routes/harness.rs`** —
   - `StartWorkRequest` (routes/harness.rs:452) and `SpecFromIssueRequest`
     (routes/harness.rs:273) each gain `#[serde(default)] tracker: Option<String>`.
   - `ensure_spec_and_plan` gains a `choice: TrackerChoice` param. Provider
     resolution for this (inherently GitHub-issue-driven) path:
     `Auto | Github → "github"` (byte-identical to today), `Linear → "linear"`,
     `None → "none"`. The resolved provider replaces both hardcoded `"github"`
     literals — the `plan_from_spec_with_tracker(workdir, &spec_id, "github",
     &issue.url)` call (routes/harness.rs:421) and the initial Todo transition
     (routes/harness.rs:438). `tracker_url` stays `issue.url` in every case
     (AC 3). `plan_from_spec_with_tracker` already takes the provider as a
     parameter (harness/types.rs:937-944) — **no change to harness/types.rs**.
9. **`routes/board_goals.rs`** — `plan_goal_harness` (board_goals.rs:515) gains
   an `Option<Json<PlanGoalHarnessRequest { tracker: Option<String> }>>` body
   (optional ⇒ existing no-body callers are untouched). Resolution at the
   current `TaskSink::select` call site (board_goals.rs:567):
   - `Auto`/absent → `TaskSink::select(&wd).await` — env + probe, unchanged.
   - `Github`/`Linear` → the forced `TaskSink` variant, skipping select
     entirely (an unconfigured explicit sink fails `create_feature` loudly →
     500 with the provider named, matching the repo's never-silent ethos).
   - `None` → the Board-branch id shape without board mirroring:
     `(c.key.clone(), Some("none".into()), None)` — no external
     `create_feature`, and the plan-time Todo call runs with `"none"` and
     lands in the new logged-skip arm.

### Explicitly untouched (boundaries)

- `harness/drive.rs` — zero changes. `transition_tracker` already logs every
  `Skipped` (drive.rs:400-404), which is what makes "None → logged no-op"
  work; the roles-on decompose re-plan already re-threads provenance via
  `shared_tracker_provenance` (drive.rs:912-917).
- `harness/types.rs` — zero changes (the stamp mechanism exists; AC 3 names it
  only as where the stamp lands).
- `linear.rs`, `LinearStateMap`, `GithubStateMap` — untouched (non-goal).
- `IntegrationsPane.tsx` + the `gh`/`linear` Tauri commands — untouched
  (non-goal: auth config stays global).
- MCP plan tools (`agentum_harness_plan`) and hand-written backlogs — untouched
  (they plan without tracker stamps today and continue to).

## APIs (wire deltas)

| Route | Delta | Absent ⇒ |
| --- | --- | --- |
| `POST /api/harness/start-work` | `+ tracker?: "auto"\|"github"\|"linear"\|"none"` | today's `"github"` |
| `POST /api/harness/spec-from-issue` | same | today's `"github"` |
| `POST /api/board/goals/{id}/harness-plan` | optional JSON body `{ tracker? }` | today's `TaskSink::select` |

Unknown `tracker` values → 400 (`ApiError::BadRequest`), never a silent
fallback. Backward compatibility is structural: old clients send no field.

## Data flow

```
RepositoryPane picker ──updateRepo──▶ Repo.trackerProvider (persisted UI repo metadata)
        │
        ▼ (composer submit; selectedRepo in scope)
useComposerState ──tracker──▶ startGatedWork / scaffoldSpecFromIssue (HTTP)
        │
        ▼
routes/harness.rs: parse_tracker_choice ──▶ ensure_spec_and_plan(choice)
        ├─▶ plan_from_spec_with_tracker(wd, spec, resolved_provider, issue.url)
        │      └─▶ Feature.tracker_provider stamped per feature (types.rs:970-975)
        └─▶ initial Todo transition with resolved_provider
        ▼ (run time — unchanged)
drive.rs transition_tracker ──▶ apply_tracker_transition(provider, …, TrackerEmit)
        └─ "github" | "linear" | "board" | "none"(new, logged skip) | other(skip)
```

The goal path is the same shape with `plan_goal_harness` resolving
choice → sink before feature creation.

## Important decisions

- **D1 — UI-owned persistence + request threading, not a server-side KV.**
  Chose the `issueSourcePreference` pattern (per-repo choice in the UI `Repo`
  record) + the `agentTool`/`agentModel` request-knob pattern (spec 005 D2)
  over persisting in the server's `settings` KV keyed by project path,
  because path-keyed server storage needs worktree→root resolution plus macOS
  path canonicalization (`/var` vs `/private/var`, `~`-expansion) — a real
  silent-miss class — and a new read/write route pair, all machinery this
  spec doesn't need. Cost accepted: the choice is desktop-local (a TUI client
  wouldn't see it) — acceptable because the desktop is this repo's only
  client and the AC pins the picker to the desktop settings surface.
- **D2 — `"none"` is a stamped provider string, not an absent stamp.** A
  feature with `tracker_provider: None` short-circuits *silently* in
  `transition_tracker` (drive.rs:384-386), but AC 5 demands a **logged**
  no-op. Stamping the literal `"none"` routes through the existing
  Skipped-logging (drive.rs:400-404) with a purpose-built reason string
  instead of the misleading `unknown tracker provider` fallthrough copy.
- **D3 — explicit choice wins over `AGENTUM_TASK_SINK`.** AC 3 says the
  explicit tracker replaces "the env/auto probe in `TaskSink::select`", so a
  sent `tracker` field never consults env or probe; the env pin still governs
  `Auto`/absent (which keeps every existing env-pinning test hermetic — they
  send no field). Documented on `select`'s docstring.
- **D4 — the issue-driven path maps `Auto` to `"github"`, not to
  `TaskSink::select`.** `start-work`/`spec-from-issue` are inherently
  GitHub-issue-shaped (`FetchedIssue`, digits-only `number`,
  `deriveIssueSideEffectGate` requires a github.com issue URL) — probing and
  landing on `Linear` there would stamp a provider whose id space doesn't
  match the URL. Today's hardcoded `"github"` **is** the correct Auto
  behavior for this path; only an explicit choice overrides it.
- **D5 — reuse before build.** No new crate types beyond `TrackerChoice`
  (two immediate route consumers + a pure test target); the stamp
  (`plan_from_spec_with_tracker`), the dispatch (`transition_inner`), the
  logging (`transition_tracker`), and the UI persistence (`updateRepo`) all
  already exist and are reused as-is.

## Risks & mitigations

- **R1 — explicit Linear on a GitHub-issue-driven plan can't move the Linear
  ticket** (the stamped feature id is `F1…Fn` from spec checkboxes
  (types.rs:900-902), not a Linear identifier → `transition_issue` errors or
  skips). Mitigation: the seam is best-effort by contract — every failure is
  a logged non-fatal line, never a halt (drive.rs:405-409). **Accepted
  because** the honest fix (a per-project Linear team/issue mapping) is
  explicitly out of scope in the spec's non-goals ("No per-project issue/team
  URL fields").
- **R2 — goal-path `None` loses its stamp on a roles-on decompose re-plan**
  (`shared_tracker_provenance` needs provider *and* url, types.rs:250; the
  goal-path `"none"` has no url). The re-planned features then no-op silently
  instead of logging. **Accepted because** the observable AC-5 behavior (run
  completes; no tracker touched) is identical; the issue path keeps
  `issue.url` so its `"none"` provenance survives re-plans.
- **R3 — precedence flip for env-pinned users**: a project with an explicit
  choice now ignores `AGENTUM_TASK_SINK`. Mitigation: the env pin keeps full
  authority whenever no explicit choice is sent (the default for every
  existing repo — the field is undefined until a user touches the picker),
  and D3 documents the ordering at the seam.
- **R4 — hermetic Linear dispatch test needs creds isolation**: the AC-4 test
  points `AGENTUM_LINEAR_CREDS` at a missing file so `transition_issue` fails
  *before* any network call. `std::env::set_var` in tests can race parallel
  readers — mitigated by following the existing precedent that already does
  exactly this (board_goals.rs:1074); a missing-creds state is also the CI
  default, so a race degrades to the same observable result.
- **R5 — stale persisted repos**: existing `Repo` records lack the field →
  `undefined` → `Auto` → byte-identical behavior. Forward-compatible by the
  same serde/TS-optional convention as `issueSourcePreference`.

## Acceptance-criteria → plan → test map

| AC | Plan part | Test |
| --- | --- | --- |
| 1. Picker with Auto/GitHub/Linear/None in per-project settings | Component 2 (RepositoryPane section) + 3 (search entries) | New pure vitest next to `RepositoryPane.test.ts` (pure-model, no jsdom — repo convention): the Tracker search entry exists and the option-value model covers exactly the four values with `'auto'` default |
| 2. Choice persists; reopening renders it | Component 1 (`Repo.trackerProvider`) + the existing `updateRepo` persistence; control reads `repo.trackerProvider ?? 'auto'` | Same pure test asserts the default-resolution helper (`undefined → 'auto'`, saved value round-trips); UI build gate (AC 6) typechecks the field end-to-end |
| 3. Explicit tracker stamped into every feature; replaces hardcoded `"github"` + env/auto probe; `tracker_url` from issue | Components 7 (`parse_tracker_choice`), 8 (resolution in `ensure_spec_and_plan`), 9 (`plan_goal_harness` forced sink) | Extend the routes/harness tests (pattern at routes/harness.rs:766): `tracker:"linear"` → all features `Some("linear")` + url = issue.url; `tracker:"none"` → `Some("none")`; absent → `Some("github")` (unchanged). `parse_tracker_choice` unit tests in task_sink.rs. board_goals: `plan_goal_harness_explicit_tracker_overrides_detection` (`tracker:"linear"` + missing-creds env per board_goals.rs:1074 precedent → error names linear, proving select was bypassed) |
| 4. `apply_tracker_transition` dispatches per selection, unit-tested, no live creds | Component 7 (the dispatch is `transition_inner`'s existing match) | New task_sink test `linear_dispatch_reaches_linear_arm_without_creds` (`AGENTUM_LINEAR_CREDS` → missing file ⇒ `Err("no Linear token configured")` proves the linear arm, pre-network); GitHub arm already pinned by the fake-`gh` test (`github_transition_with_custom_map_flips_configured_names`) and `github_transition_without_url_is_skipped` |
| 5. `None` ⇒ run completes, transitions logged-skipped; Auto/no-choice unchanged | Component 7 (`"none"` arm) + D2; unchanged-Auto is structural (absent field never reaches new code) | New task_sink test `none_provider_transition_is_skipped_not_err` (both `apply_tracker_transition` and `apply_blocked_transition` return `Skipped("tracker disabled for this project")`); "logged" is drive.rs:400-404's existing Skipped log; existing suite staying green covers the unchanged-Auto half |
| 6. UI build + `cargo test -p agentum-server --lib` pass | — | The two commands are the gate, run as-is |
