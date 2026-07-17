# Spec 021 — Per-project tracker choice (GitHub / Linear) + one shared tracker section

- **Number:** 021
- **Status:** Draft             <!-- Draft | PM | Architect | In progress | Done -->
- **Surface:** `crates/agentum-desktop/ui` (TaskPage New Issue, ChatPage DraftReview, repos store) + `crates/agentum-server` (task_sink, chat, board_goals, harness planning)
- **Author:** Mateo (via /sdd-spec)
- **Date:** 2026-07-17
- **tracker:** https://github.com/MateoCerquetella/agentum/issues/379

> **Numbering note:** dirs run through `020`; memory records an *uncommitted*
> spec-021 lost to a worktree reap on 2026-07-17 (never landed in any branch).
> Next free on this tree is **021**; if the lost draft resurfaces, a number
> collision is cosmetic (precedent: the two `016-*` dirs).
> Anchors line-verified on `origin/develop` @ v0.78.0 (`bb25a97d`) after
> rebasing this worktree off its stale v0.57.0 base.

## Problem

A project has no say in which tracker its issues live in. The provider is
decided three inconsistent ways: Chat's DraftReview has a session-local toggle
that resets to GitHub every time (`ChatPage.tsx:136`), the New Issue surface
implies the provider from whichever Tasks tab happens to be active
(`TaskPage.tsx` — two separate bespoke dialogs), and the server falls back to
an availability heuristic that always prefers GitHub
(`task_sink.rs::pick_provider`). For someone with both GitHub and Linear
connected, issues and harness status transitions routinely aim at the wrong
tracker for the project at hand. On top of that, the Chat filing strip and the
New Issue dialog are near-duplicate UIs with zero shared code — the tracker
section exists twice, differently, and neither remembers anything.

## Goal

A project remembers its tracker: a per-repo GitHub/Linear/Auto choice persisted
on the `Repo`, surfaced through **one** shared, redesigned tracker section used
by both the New Issue dialog and Chat's DraftReview, and honored end-to-end by
the server when filing issues, planning goals, and stamping harness features.

## Users / personas

- **Mateo (multi-project operator)** — some projects track in Linear (client
  work), others in GitHub (agentum itself). The moment: he files an issue from
  Chat for a Linear-tracked project and it lands on GitHub because the toggle
  reset; or a harness run's Coding→InProgress transition no-ops because the
  feature was stamped with the wrong provider. He wants to set it once per
  project and stop thinking about it.

## UX direction (the tracker section, once)

One `TrackerSection` component, identical in both surfaces:

```
┌ Tracker ──────────────────────────────────────────────┐
│  Project: [acme-app ▾]     Provider: (Auto|GitHub|Linear) │
│  ── provider-specific fields swap below ──            │
│  GitHub:  repo target · issue source · labels · assignees │
│  Linear:  team · project · state · priority · assignee    │
│  [✓] Remember for this project                        │
└───────────────────────────────────────────────────────┘
```

- Provider control defaults from the selected repo's stored `trackerProvider`
  (`Auto` when unset — today's behavior).
- Switching provider swaps the field set below **without losing title/body**.
- A one-off override never silently rewrites the stored preference; only the
  explicit "Remember for this project" affordance persists it.

## Acceptance criteria

1. `Repo` carries an optional `trackerProvider: 'auto' | 'github' | 'linear'`
   (absent = auto): added to `shared/types.ts` and the `RepoUpdate` whitelist
   (`store/slices/repos.ts:77` area); `updateRepo` **persists** it to
   `repos.json` via the existing serde-flatten path (`routes/repos.rs::update`,
   zero server schema change — the `issueSourcePreference` precedent), and the
   value **renders** again after app relaunch.
2. A shared `TrackerSection` component **renders** in BOTH the New Issue
   surface (`TaskPage.tsx`, replacing the tab-implied provider split between
   the "New GitHub issue" and "New Linear issue" dialogs) and Chat's
   DraftReview filing strip (`ChatPage.tsx:1025-1033`, replacing the ad-hoc
   `SegButtons` provider toggle); selecting a provider swaps the
   provider-specific fields (GitHub: `IssueSourceSelector`, repo target,
   `GitHubIssueLabelSelector`, `GitHubIssueAssigneeSelector`; Linear: team /
   project / state / priority) while entered title/body persist.
3. When a repo is selected in either surface, the section **initializes** its
   provider from the repo's stored `trackerProvider`; a dialog-local override does NOT
   write the store, and the explicit "Remember for this project" affordance
   **persists** the choice through `updateRepo` (AC 1 path).
4. Server pinning: chat filing (`chat.rs::resolve_provider`, `chat.rs:1861`)
   and goal planning (`board_goals.rs::create_feature_for_goal`,
   `TaskSink::select`) **honor** the pinned provider — a `linear`-pinned
   project files to Linear even when GitHub is available (inverting the
   GitHub-first `pick_provider` heuristic for that project). Precedence per
   architecture D3: an explicit per-project pin **wins** over
   `AGENTUM_TASK_SINK`; the env pin keeps governing `auto`/absent, so every
   existing env-pinned hermetic test (none send the field) stays green.
   A pinned-but-unconnected provider **returns** the existing typed 422
   (`no_linear` / `no_github_repo`), never a silent fallback.
5. Harness stamping: features planned for a pinned project **carry** the pinned
   `tracker_provider` (`harness/types.rs:85`, threaded via the goal task-sink
   path), so `apply_tracker_transition` fires against the pinned provider with
   no call-site changes. The literal `"github"` in the spec-from-issue scaffold
   (`routes/harness.rs:421`/`438`) is CORRECT (the source is a GitHub issue)
   and stays.
6. Projects with `trackerProvider` unset or `auto` **behave** exactly as today
   (availability heuristic, GitHub-first) — regression-guarded by existing +
   new `pick_provider` / `resolve_provider` tests; `bun run build` (UI) and
   `cargo test --workspace --lib` are green.

## Scope & non-goals (YAGNI)

- **In:** the `Repo.trackerProvider` field + persistence; the shared `TrackerSection`
  in New Issue + Chat DraftReview; server-side pinning at the three seams
  (chat, goal planning, harness feature stamping).
- **Out:**
  - Merging Chat's DraftReview and the New Issue dialog into one surface
    (Mateo prefers New Issue's document style — that consolidation is its own
    follow-up spec; this spec only unifies the tracker *section*).
  - A "None"/Board option in the picker — Chat issues are GitHub/Linear only
    (standing directive); the internal-Board fallback remains a server-side
    auto-resolution detail, not a user choice.
  - Per-project issue-URL / tracker-URL fields — contradicts the per-feature
    `tracker_url` flow (dropped at the run-379 authoring gate).
  - Per-project Linear credentials — the Linear connection stays global
    (`linear.json`).
  - Changing the SSH-host slug resolution shipped by spec 020.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- `Repo.issueSourcePreference` (`shared/types.ts:99`; whitelist
  `store/slices/repos.ts:77`; server `routes/repos.rs` `update()` with
  `#[serde(flatten)] extra`) — the byte-for-byte precedent for a per-repo
  field that persists to `repos.json` with zero server schema change.
- `chat.rs:1861 resolve_provider` (+ tests at `chat.rs:2309+`) — already
  accepts an explicit `github`/`linear`, rejects `board`, types the
  unconnected 422s. Pinning = changing what the *default* resolves to.
- `task_sink.rs:95-118` — `pick_provider` + `TaskSink::select` with the
  `AGENTUM_TASK_SINK` env override; the single policy seam for goal planning.
- `board_goals.rs:325 create_feature_for_goal`, `:515 plan_goal_harness` —
  goal→feature path that already threads `tracker_provider`/`tracker_url`
  into harness features (`harness/types.rs:85`, `:937
  plan_from_spec_with_tracker`).
- `apply_tracker_transition` / `TrackerPhase` (`task_sink.rs`) — status sync
  already keys off `Feature.tracker_provider`; stamp it right and transitions
  follow with no call-site changes.
- UI field pickers: `IssueSourceSelector`
  (`components/github/IssueSourceSelector.tsx`), `GitHubIssueLabelSelector`,
  `GitHubIssueAssigneeSelector`, the Linear dialog's team/state/priority
  fields (`TaskPage.tsx:1720+`), `IssueProvider` type
  (`runtime/chat-client.ts:219`).
- Chat already sends `provider` + pinned repo context (`createIssuesFromChat`,
  spec 017 repo_id threading) — the transport for the pinned choice exists.
- Run-379 `architecture.md` (same spec dir in `.agentum-harness/`) — a
  line-verified server-seam plan at `bb25a97d` (`TrackerChoice` parse,
  transition arms, request threading, D1–D5). It predates this spec's
  amendment: reuse its server seams, but reconcile scope (add the shared
  `TrackerSection` + the chat `resolve_provider` seam; drop the `"none"`
  machinery; keep D3 precedence and the `trackerProvider` field name).

### Build new

- `TrackerSection` shared component (provider control + swap-in field sets +
  "Remember for this project") composed from the existing pickers.
- `trackerProvider` field on `Repo` + `RepoUpdate` whitelist entry.
- Pin resolution on the server: consult the repo's stored `trackerProvider` at the
  three seams (architect decides UI-sends vs server-reads-`repos.json`;
  recommendation: server reads the registry so there's one source of truth,
  clients just send `repo_id` — chat already does).

## Risks & invariants

- **Best-effort tracker contract** (architecture principle): transitions are
  logged-never-halting; pinning must not introduce a hard failure mid-run. A
  pinned-but-unavailable provider fails *filing* loudly (typed 422) but only
  *logs* during a harness run.
- **`AGENTUM_TASK_SINK` keeps governing `auto`/absent** — hermetic tests send
  no explicit field, so they stay green; an explicit per-project pin
  deliberately beats the env (architecture D3, documented at the seam).
- **Chat issues = GitHub/Linear only** — no Board option leaks into the UI.
- **Don't "fix" `routes/harness.rs:421`** — the spec-from-issue scaffold is
  inherently GitHub; stamping it with a Linear pin would mis-route
  transitions for a GitHub-sourced spec.
- Old app versions ignore the unknown `trackerProvider` key in `repos.json`
  (flatten round-trip) — forward/backward safe.
- Dual-surface regression risk: TaskPage's two dialogs have deep local state;
  replacing their provider plumbing must not break label/assignee fetches
  keyed on the target repo (`TaskPage.tsx:783-790`).

## Harness wiring (the gate)

- **feature_list.json entries** (re-plan of run 379's F1–F6, one gated slice
  each):
  - F1 — `Repo.trackerProvider` field + whitelist + persistence round-trip.
  - F2 — `TrackerSection` component + New Issue adoption.
  - F3 — Chat DraftReview adoption (replace `SegButtons` strip).
  - F4 — server pinning at chat + goal-planning seams (+ env-override guard).
  - F5 — harness feature stamping carries the pin; auto/unset regression
    guard.
- **`verify.sh` asserts:** `cargo test --workspace --lib` green (new:
  pinned-select, resolve_provider-pinned, stamp-threading, auto-unchanged) +
  `bun run build --prefix crates/agentum-desktop/ui` + targeted vitest for the
  repos-slice whitelist and `TrackerSection` pure logic.
- **`qa.sh` asserts (browser):** pin a project to Linear in the tracker
  section → reopen New Issue → section defaults to Linear with Linear fields;
  open Chat DraftReview for the same project → same default; file → the issue
  lands in Linear; an `auto` project still defaults GitHub-first.

## Open questions

1. Should the stored choice also render/edit in the Project Hub's Tracker tab
   (read-mostly today)? Recommended: follow-up, not this slice.
2. The bigger consolidation — one intake surface instead of Chat-vs-New-Issue
   (Mateo prefers New Issue's style) — needs its own spec; does it subsume the
   Tasks-tab split too?
3. Should `Auto` display its resolution ("Auto — GitHub detected") so the
   heuristic is legible in the picker?
4. Pin resolution seam: UI-sends-provider vs server-reads-registry
   (recommended: server reads `repos.json` by `repo_id`; architect to pin).
