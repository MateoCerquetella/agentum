# Spec 015 — Host-aware start-work + Tracker intent-to-gated-run intake

- **Number:** 015
- **Status:** Done       <!-- Draft | PM | Architect | In progress | Done -->
- **Surface:** `crates/agentum-server` (repos registry, worktree create, github/linear routes) + `crates/agentum-desktop/ui` (wizard, Projects board Start-work, Project Hub Tracker tab)
- **Author:** Mateo (via /sdd-spec)
- **Date:** 2026-07-13

> **Numbering note:** 013 and 014 are each claimed twice by concurrent drafts;
> the next free number is **015**. All file:line anchors below were verified on
> `origin/develop` @ `4f98453f` (2026-07-13).

## Problem

Working from the VPS (`dyaus`) is second-class in the start-work flow, and Mateo
hit all three walls in one session:

1. **Starting work from a board task silently lands on localhost.** The board's
   "Start work" creates the workspace on the local machine without ever asking
   where, even though the repo also lives on the VPS.
2. **Picking the VPS copy of a repo doesn't stick.** In "pick a repo" on host
   `dyaus`, the repo can be selected — but after confirm the flow proceeds with
   the *first* repo in the list (local `agentum`) instead of the one picked.
3. **The Tracker tab is a dead end.** It connects and loads the board binding,
   but it's a bare config form: there is no way to write down what you want to
   do, file it as a real GitHub/Linear issue, and start the gated run from there.

## Goal

Make start-work host-honest end to end: a repo registered on a remote host keeps
its host identity from picker to created workspace, the board's Start-work never
silently assumes localhost, and the Tracker tab turns written intent into a filed
GitHub/Linear issue with an optional gated run.

## Users / personas

- **Mateo (multi-host solo operator)** — laptop + a VPS (`dyaus`) over SSH, at
  three moments: (a) clicking **Start work** on a board item whose repo lives on
  the VPS; (b) adding/picking the VPS copy of a repo in the New Workspace wizard;
  (c) sitting in **Project Hub → Tracker** with a thought in his head and no way
  to make it a tracked, gated piece of work without leaving the tab.

## Acceptance criteria

Ordered increments F1 → F3; each independently gateable. F1 is the root-cause
fix and lands first — F2's "lands on the chosen host" depends on it.

**F1 — remote repo identity survives registration (root cause of bug 2)**

1. Registering a repo on an SSH host whose absolute path equals an existing
   *local* repo's path **persists a distinct repo entry** carrying
   `connection_id` + `host_id`: the dedupe in `append_repo`
   (`crates/agentum-server/src/routes/repos.rs:134`) keys on
   **(path, connection_id)** — not path alone — and `POST /api/repos` returns
   the remote entry, never the pre-existing local one. (Today the identical
   path — `/home/dyaus/...` exists on both machines — collapses the remote add
   into the local repo with `connection_id: None`.)
2. Re-adding the *same* repo on the *same* host stays idempotent: a second
   `POST /api/repos` with an already-registered (path, connection_id) returns
   the existing entry — no duplicate rows for either local or remote repos.
   Asserted by Rust unit tests in `repos.rs` (same path × two hosts → two
   entries; same path × same host → one entry).
3. The wizard's repo list under the SSH host **renders the remote repo with its
   ssh/remote badge** (the badge derives from `repo.connectionId`,
   `CreateWorkspaceWizard.tsx:1129-1133`) and the selection **survives confirm**:
   with the remote repo selected, the keep-selection-valid effect
   (`useComposerState.ts:1012-1020` → `resolveRepoIdForHost`,
   `hooks/composer-host-scoping.ts:89-94`) does not rewrite `repoId`, and
   `submitQuick`/`submit` (`useComposerState.ts:2602`/`2471`) pass that exact
   repo id to `createWorktree`.
4. The created workspace **lands on the SSH host**: the server resolves the
   worktree's host from the picked repo (`load_host_for_repo`,
   `routes/worktrees.rs:431` → `routes/repos.rs:371-378`) to the remote
   `host_id`, and the worktree registry entry + spawned session run on `dyaus`
   (pane over SSH), not locally.

**F2 — Start-work from a board item asks where (bug 1)**

5. Clicking **Start work** on a Projects-board item
   (`components/github-project/ProjectViewWrapper.tsx:503` `handleStartWork` →
   `lib/launch-work-item-direct.ts:194`) **never silently assumes local**: when
   the item's repo is registered on **more than one host** (e.g. local `agentum`
   + `dyaus` `agentum`), the flow surfaces a host/repo choice *before* any
   worktree is created; the created workspace lands on the chosen host's repo.
6. When the item's repo matches **exactly one** registered repo, Start-work
   proceeds directly as today (no new friction), using that repo's host — a
   VPS-only repo starts on the VPS, a local-only repo starts locally.
7. The wizard's Host step governs the outcome end to end: host `dyaus` + a
   `dyaus` repo selected in the wizard produces a worktree + session on `dyaus`
   (this is F1 doing the work — asserted here as the observable end-to-end).
8. No new spawn path: whatever surfaces the choice reuses the existing create
   flows (`store.createWorktree` → `POST /api/worktrees/create`; sessions keep
   spawning via `routes::sessions::spawn_agent_into_pane`). No parallel
   worktree-create or session-spawn code is introduced.

**F3 — Tracker tab: written intent → issue → gated run (ask 3)**

9. **Project Hub → Tracker** (`ProjectHubPage.tsx:238` → `ProjectTrackerConfig`
   `:253-277`, today only `ProjectBindingEditor`) additionally renders, whenever
   a binding/tracker resolves, a **"New issue" intake panel**: a free-text
   "what do you want to do?" field that **drafts** a reviewable issue
   (title + body) via the existing draft seam
   (`POST /api/github/issues/draft-body` → `draft_issue_body`,
   `routes/github.rs:302`; client `github-issue-client.ts:189`).
10. **Filing creates a real issue** with the resolved provider: GitHub via
    `POST /api/github/issues` (`routes/github.rs:212`; client
    `createGithubIssue`, `github-issue-client.ts:115`); **Linear** via the
    existing `linearCreateIssue` client (`runtime-linear-client.ts:153` — the
    same native-command path the wizard already uses; the webview never talks
    GraphQL). *(Amended at architect grounding 2026-07-13: the originally
    planned thin HTTP seam is VOID — spec 013 F3 already shipped this client;
    adding a route would create the fork this spec forbids.)* Provider
    resolution reuses `resolveCreateIssueProvider`
    (`create-issue-intent-model.ts:69`); when ambiguous (both connected), the
    panel shows a provider toggle.
11. On file success the new issue is **visible on the bound board** (the Tasks
    tab's project-mode board view lists it after its normal refresh) and the
    panel offers **"Start gated run"** for a filed **GitHub** issue: it routes
    through the spec-008 pre-armed composer hop (filed issue linked, gated-run
    toggle armed), landing on the SAME `start_work` seam and precondition set
    the wizard uses (`POST /api/harness/start-work`, `routes/harness.rs:508`)
    — no second gated-run entry path, and never a direct `startGatedWork` call
    (a gated run requires the fresh worktree the composer creates; amended at
    architect grounding 2026-07-13).
12. **Errors are inline and non-fatal**: missing credentials, no binding, a
    draft failure, or a provider error render an inline message in the panel and
    never wedge the Tracker tab or file a half-issue. Inconclusive never files.
13. **Pure model covered:** the panel's state machine (idle → drafting → review
    → filing → filed/error, provider resolution, gated-run eligibility) lives in
    a jsdom-free model module (reusing/extending `create-issue-intent-model.ts`)
    with `bunx vitest run` green.

## Scope & non-goals (YAGNI)

- **In:** the (path, connection_id) repo-identity fix + its UI fallout (F1);
  a host choice on the board's direct Start-work when ambiguous (F2); the
  Tracker-tab intake panel reusing the existing draft/create/start-work seams
  — including the existing `linearCreateIssue` client (F3; no new server
  route, amended at architect grounding).
- **Out:**
  - **No new tracker providers** beyond GitHub + Linear.
  - **No change to the harness engine** or to `start_work`'s semantics — F3
    only adds a caller.
  - **No Linear → gated-run parity**: `start_work` is GitHub-issue-only today
    (`gh issue view` by number+slug); a filed Linear issue gets no "Start gated
    run" button in this spec (see Open questions).
  - **No rework of the Tasks board / project views** — F3's board visibility
    rides the existing refresh.
  - **No duplication of the wizard's create-issue panel** (spec 013 F2, already
    on develop): the Tracker panel reuses the same model/clients, not a fork.
  - **No repo-registry migration tool**: entries collapsed by the old dedupe
    stay as-is; the operator re-adds the remote repo once (document in the
    release notes). A doctor check is a possible follow-up, not this spec.
  - **No change to `worktrees.rs::CreateBody`** unless the architect finds
    repo-derived host insufficient (see Open questions).

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- **Repo registry + host resolution** — `routes/repos.rs` `append_repo:125-161`
  (the dedupe to fix), `resolve_repo_host_id:358` / `load_host_for_repo:371-378`
  (host-from-repo, `unwrap_or(LOCAL_HOST_ID)` at `:372`); worktree create
  consumes it at `routes/worktrees.rs:431`.
- **Remote repo add flows (UI)** — sidebar `AddRepoSteps.tsx:151`
  (`reposAddRemote`) and the wizard inline add
  (`CreateWorkspaceWizard.tsx:947-970`), both upserting the server's returned
  repo by id; client `runtime/server-repo-client.ts:26-36`
  (`POST /api/repos {path, connectionId, hostId}`).
- **Host scoping + picker plumbing (UI)** — `composer-host-scoping.ts`
  (`filterReposForHost:65`, `resolveRepoIdForHost:89`), `hostKeyForRepo`
  (`components/sidebar/worktree-list-groups.ts:246`), the wizard `HostStep`
  (`CreateWorkspaceWizard.tsx:595-670`) — all behave correctly once repo
  identity is fixed; F1 changes none of their logic.
- **Hosts registry** — `GET /api/hosts` + `runtime/server-host-client.ts:22`
  (`listServerHosts`), `resolveServerHostIdForConnection:395` — for any F2
  chooser UI.
- **Direct launch + wizard hop** — `lib/launch-work-item-direct.ts:194`
  (creates worktree, binds tracker coords, pastes context) and the existing
  item→wizard route `TaskPage.tsx:2345` (`openComposerForItem` →
  `openModal('new-workspace-composer', …)`) — F2 composes these; it does not
  build a third launcher.
- **Issue draft/create (GitHub)** — `routes/github.rs:212` `create_issue`,
  `:302` `draft_issue_body`; clients `github-issue-client.ts:115/189`; panel
  state precedent `create-issue-intent-model.ts` + `useComposerState.ts:1510`
  (`handleCreateIssueSubmit`) / `:1604` (`handleGenerateIssueBody`).
- **Linear create (server)** — `linear.rs:159` `create_issue` via
  `TaskSink::Linear` (`task_sink.rs:200`, provider pick `:93-110`). Exists;
  only the HTTP exposure is missing.
- **Issue → gated run** — `POST /api/harness/start-work`
  (`routes/harness.rs:508`), client `startGatedWork` (`harness-client.ts:171`),
  side-effect gate precedent `lib/issue-side-effect-gate.ts` +
  `useComposerState.ts:2268-2311`.
- **Tracker tab shell** — `ProjectHubPage.tsx:238/253-277` +
  `ProjectBindingEditor.tsx` (binding load `:94`, project list `:116`,
  discover `:146`, save `:229`) stays the config half of the tab.

### Build new

- **F1** — the (path, connection_id) dedupe key in `append_repo` + unit tests.
  Audit the two other `read_repos()` consumers that match by path for
  same-assumption bugs (`resolve_repo_host_id`, any path-keyed lookup).
- **F2** — a host/repo disambiguation step on the board Start-work path when
  multiple registered repos match the item (small: either an inline chooser in
  `ProjectViewWrapper` or a pre-seeded wizard hop; architect picks — the wizard
  hop reuses `openComposerForItem`).
- **F3** — a Tracker-tab intake panel (`TrackerIntakePanel` +
  `use-tracker-intake.ts`, a sibling of an untouched `ProjectBindingEditor`)
  + add-only pure-model extensions to `create-issue-intent-model.ts`; the
  pre-armed composer hop for "Start gated run". *(Amended at architect
  grounding: NO new server route — Linear create already exists client-side,
  `runtime-linear-client.ts:153`; F3 touches zero Rust.)*

## Risks & invariants

- **One launch path (invariant 1).** F2/F3 must create worktrees and spawn
  sessions only through the existing `createWorktree` → `spawn_agent_into_pane`
  chain. No bespoke spawn.
- **Repo-store dedupe change is a data-shape change.** The new key must not
  duplicate existing *local* entries on every add (idempotency per host,
  AC 2) and must not disturb `update()`'s refusal to change `path`
  (`repos.rs:215`). Pre-existing collapsed entries are left untouched — fixing
  them is the operator's one-time re-add, never an automatic rewrite of the
  registry.
- **Serde-alias hazard (spec 012 memory).** Any payload additions around the
  worktree registry / linked work item reuse existing shapes; no aliased fields.
- **Fail-closed honesty (011/013 precedent).** The remote badge and host
  bucketing derive from `connectionId` — after F1 they become truthful; F3's
  panel must never show "filed" without a provider-confirmed issue id/URL, and
  an unparseable draft never files.
- **Coordination with in-flight specs.** 013-wizard-issue-first (create-issue
  intent, partially landed) owns the *wizard* panel — F3 reuses its model and
  must not fork it. *(RESOLVED at architect grounding: 013 F3 already landed —
  the wizard files Linear via `linearCreateIssue`; 015 reuses that client and
  adds no seam.)* 012 (work-item picker + status sync) touches the same wizard
  step — re-ground line numbers before editing.
- **`unwrap_or(LOCAL_HOST_ID)` stays.** A repo with no host really is local;
  F1 fixes identity at registration, not the resolver's default.

## Harness wiring (the gate)

- **feature_list.json entries (ordered):**
  1. `remote-repo-identity` — (path, connection_id) dedupe + tests + badge /
     selection / created-host end-to-end (AC 1–4).
  2. `start-work-asks-where` — multi-host disambiguation on board Start-work
     (AC 5–8).
  3. `tracker-intent-intake` — Tracker-tab panel: draft → file (GitHub/Linear)
     → optional gated run (AC 9–13).
- **`verify.sh` asserts:** `cargo test -p agentum-server --lib` green (new
  `repos.rs` dedupe tests; the Linear-create route test is VOID — no route is
  built, see AC 10 amendment) AND
  `bun run build --prefix crates/agentum-desktop/ui` succeeds AND
  `bunx vitest run` on the touched pure models (host-scoping, intake panel
  model) is green. No bare `tsc` (pre-broken baseline; vite + targeted vitest
  is this repo's gate).
- **`qa.sh` asserts (browser QA, staging):** add the VPS copy of a repo whose
  path collides with a local one → it appears under host `dyaus` with the ssh
  badge → select it → confirm → the created workspace's session runs on the
  VPS; board Start-work on an item whose repo exists on two hosts → a host
  choice appears → choosing `dyaus` lands there; Tracker tab → type an intent →
  draft renders → file → the issue exists on GitHub/Linear and shows on the
  board → Start gated run kicks a harness run.

## Open questions

- **F2 chooser UX:** inline host dialog on the board vs routing through the
  wizard pre-seeded at the Host step? *Default:* the wizard hop
  (single-front-door direction of 013 F4); an inline chooser only if the hop
  proves too heavy for the "Start work" gesture.
- **Explicit `host_id` on worktree create?** Sessions and board-goal-start
  already thread `host_id`; worktree create derives host from the repo. After
  F1 the derivation is sound — does the architect still want the explicit field
  for symmetry/future-proofing? *Default:* no API change; repo-derived.
- **Linear gated run:** `start_work` is GitHub-only (fetches via `gh issue
  view`). Do we want a Linear-issue → gated-run path (needs a Linear fetch arm
  in `ensure_spec_and_plan`) or is GitHub-first acceptable? *Default:* defer;
  the panel files Linear issues but shows "gated run: GitHub issues only".
- **Old collapsed entries:** is a `doctor.rs` check ("repo path registered
  local but reachable on host X") worth a follow-up ticket? *Default:* yes as
  follow-up, not in this spec.
