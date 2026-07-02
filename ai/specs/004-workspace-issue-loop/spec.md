# Spec 004 — Workspace issue loop (issue-first workspaces)

- **Number:** 004
- **Status:** Done              <!-- Draft | PM | Architect | In progress | Done -->  (reviewer sign-off 2026-07-01; release human-gated)
- **Surface:** `crates/agentum-server` (task_sink, routes/worktrees, harness types) + `crates/agentum-desktop/ui` (New Workspace composer)
- **Author:** Mateo Cerquetella (drafted with Claude)
- **Date:** 2026-07-01

## Problem

Starting work in agentum doesn't follow the issue-first workflow the product
preaches: creating a workspace files no GitHub issue and writes no spec, so the
user must hop to the Tasks page or Chat first; and even when an issue IS linked,
its status never moves — the harness fires tracker transitions at every phase,
but the GitHub arm is a no-op, so the issue sits untouched from "created" until
a human closes it.

## Goal

Work started in agentum is tracked on GitHub from birth to done: the New
Workspace composer can file the issue (and scaffold its spec) at creation, and
the harness's existing tracker transitions become real on GitHub.

## Users / personas

- **Mateo (solo multi-agent operator):** has the same project open twice, runs
  several agents concurrently, and lives by "every change starts as an issue".
  Feels the pain the moment he opens the composer with an idea in hand — today
  he must detour to the Tasks page's "New issue" dialog or Chat, then come back
  and click "Use".
- **Anyone watching an autonomous run from GitHub:** the issue is supposed to be
  the live status board, but its state (labels/open-closed) never changes even
  as the harness moves `coding → verifying → ready_to_test → done`.

## Acceptance criteria

*Increment A — issue at workspace birth (composer + persistence):*

1. The New Workspace composer, when no work item is linked, renders a "Create
   GitHub issue" affordance; submitting it files an issue through the existing
   `TaskSink::Github` path and the created issue becomes the workspace's
   `linkedWorkItem` — the composer renders the issue number + URL before the
   worktree is created.
2. `POST /api/worktrees/create` persists `linkedIssue` / `linkedPR` /
   `linkedLinearIssue` from the request body into the worktree registry and
   returns them in the `{ worktree }` response (today `routes/worktrees.rs`
   hard-codes them to `None`).

*Increment B — GitHub status movement:*

3. `apply_tracker_transition(store, "github", issue_no, phase)` returns
   `Applied`: via the `gh` CLI (D2) it ensure-creates the four canonical labels
   idempotently with fixed colors (D3), then sets the label matching the phase
   (`Todo`→`status/todo`, `InProgress`→`status/in-progress`,
   `ReadyToTest`→`status/ready-to-test`, `Done`→`status/done`) and removes the
   other three canonical labels — foreign `status/*` labels (e.g. this repo's
   own `status/qa*` human-QA set) are never touched (architecture C4) — so the
   issue carries exactly one canonical label after every transition. `Done` is label-only — the issue stays open (D1). The harness
   drive log records `Applied`, not
   `skipped: github issue state sync not implemented`.
4. A harness run whose features carry `tracker_provider = "github"` moves the
   real issue on each existing transition point (`InProgress` on agent spawn,
   `ReadyToTest` on unit-gate green, `Done` on QA green) **without changing the
   drive loop's control flow, transition points, or autonomy mechanics** — the
   seam is already called there (`transition_tracker`, `drive.rs:311-338`). One
   mechanical widening is allowed: the call at `drive.rs:321` passes only
   `feature.id`; threading `feature.tracker_url` through the seam (the GitHub
   arm needs the repo slug) is in scope.
5. Tracker sync stays best-effort: an unreachable GitHub / missing label yields
   `Skipped(reason)` and a logged `HarnessEvent`, and the run advances anyway
   (existing contract, must not regress).

*Increment C — spec at workspace birth:*

6. Creating a workspace with a linked GitHub issue and the composer's
   "Scaffold spec" toggle enabled (opt-in, **off by default** — D5) writes
   `.agentum-harness/specs/<issue>-<slug>/spec.md` into the new
   worktree, populated from the issue title + body, preserving `- [ ]` checklist
   lines as acceptance criteria — exposed over a new HTTP seam wrapping the
   existing scaffold helpers (today they are MCP-only).
7. `plan_from_spec` over that generated spec emits `feature_list.json` entries
   whose features carry the issue's `tracker_provider`/`tracker_url` — closing
   the loop: composer-created issue → spec → backlog → status movement (AC 3–4).

## Scope & non-goals (YAGNI)

- **In:** GitHub only for the new write paths; the composer affordance; the
  worktree-registry persistence fix; the canonical
  `status/todo|in-progress|ready-to-test|done` label scheme (D3 —
  ensure-created idempotently with fixed colors, exactly one `status/*` label
  per issue); the spec-scaffold-from-issue seam.
- **Out:**
  - Linear/GitLab changes (Linear transitions already work via
    `linear::transition_issue`; GitLab stays read/link-only).
  - GitHub ProjectV2 column sync (`gh_projects.rs` is read-only today; the
    `updateProjectV2ItemFieldValue` mutation is a future refinement).
  - A spec-browsing/authoring UI (the orphaned `HarnessEngine.tsx` board stays
    orphaned; `scan_board`'s `specs[]` stays MCP-only for now).
  - Chat auto-creating workspaces from its "Create issues" result (stays a
    manual "Use" hop; only noted as a follow-up).
  - Internal Board behavior changes (`board_status_for` mapping untouched).
  - LLM-authored spec prose — Increment C is a deterministic transform of the
    issue body, not an agent call.
  - Auto-closing the issue on `Done` (D1: label-only; close stays with the
    PR's `Closes #N` convention). A per-repo "close on Done" toggle is a
    named follow-up, not in this spec.
  - The REST/PAT write path (`forge_send`) for transitions (D2: `gh` CLI only,
    matching creation) and any reuse of `.github/labels.sh`'s `status/qa*`
    labels (D3: that is a different, human-QA lifecycle — conflating them
    would corrupt both).

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- **Issue creation:** `TaskSink::Github::create_feature`
  (`crates/agentum-server/src/task_sink.rs:119-198`) — `gh issue create
  [--repo <slug>]` from `neutral_cwd()`, URL parsed by `parse_gh_issue_url`
  (`:322`). The Tasks page already drives a create dialog
  (`TaskPage.tsx:2452 handleCreateNewIssue`); Chat files via
  `POST /api/chat/issues` (`routes/chat.rs:986`).
- **Transition seam + call sites:** `TrackerPhase` + `apply_tracker_transition`
  (`task_sink.rs:204-287`); the GitHub arm to replace is `task_sink.rs:278-282`.
  Call sites already correct in `harness/drive.rs` (`transition_tracker`
  `:311-338`, invoked at `:129` InProgress, `:184` ReadyToTest, `:240` Done);
  initial `Todo` fired by `plan_goal_harness`
  (`routes/board_goals.rs:601-615`), which also threads
  `tracker_provider`/`tracker_url` onto features (`harness/types.rs:81-88`,
  `:978-979`).
- **GitHub write plumbing (two candidates):** REST `forge_send`
  (`routes/forge.rs:272-306`, PAT from `forge.json` via `token_for`) — already
  moves GitHub issue state in `board_sync.rs:456-478` (`github_state`
  `:369-371`, `parse_github_issue` `:374-384`); or the `gh` CLI runner
  `gh_in_dir` (`host_runtime/git_fs.rs:90`) that creation/read already use.
- **Issue-body fetch + linking:** `GET /api/github/issue`
  (`routes/github.rs:53`), `fetchGithubIssueBody`
  (`runtime/github-issue-client.ts:26`), `buildGithubIssueLinkedWorkItem`
  (`lib/github-linked-work-item.ts:57`), composer `linkedWorkItem` state
  (`hooks/useComposerState.ts`), untrusted-content containment
  (`lib/linked-work-item-context.ts:22`).
- **Worktree create route:** `routes/worktrees.rs::create` (`:265`,
  `CreateBody` `:249-260`) — widen, don't replace; the UI already sends the
  linked fields (`store/slices/worktrees.ts:1012`).
- **Spec/harness scaffolding:** `scaffold_harness` + `scaffold_files`
  (`harness/types.rs:667-725`, idempotent), `plan_from_spec` +
  `derive_backlog_from_spec` (`:905-925`, `:860-899` — checkbox → feature),
  `write_backlog_from_features` (`:953-994`), `HARNESS_DIR =
  ".agentum-harness"` with legacy `.harness` fallback (`:16-25`). The MCP tools
  (`routes/mcp.rs:472-568`) stay as-is.

### Build new

- Composer "Create GitHub issue" affordance (UI) + a small server endpoint or
  reuse of the Tasks-page create path to file it pre-worktree.
- The real GitHub arm of `apply_tracker_transition`: label set/remove via the
  `gh` CLI runner (`gh_in_dir`, `host_runtime/git_fs.rs:90` — D2), repo slug
  parsed from the feature's `tracker_url`, plus idempotent label ensure-exists
  with fixed colors (D3). No close on `Done` (D1).
- `CreateBody` widening + registry persistence of linked metadata.
- A deterministic issue-body → `spec.md` transform + an HTTP seam
  (e.g. `POST /api/harness/spec-from-issue` or a param on worktree create)
  wrapping the existing scaffold helpers.

## Risks & invariants

- **Best-effort tracker contract (sacred):** a tracker hiccup is logged, never
  halts a run (`transition_tracker` only logs). AC 5 pins this.
- **One launch path untouched:** nothing here touches
  `spawn_agent_into_pane` or the drive loop's autonomy mechanics.
- **Auth split (resolved by D2):** creation uses the user's `gh` login;
  transitions use the same `gh` CLI, so any environment that can create can
  also transition — no half-configured state. `board_sync`/`forge`'s PAT path
  is untouched.
- **Label drift:** `gh issue edit --add-label` fails if the label doesn't
  exist; the transition must ensure-create labels idempotently or degrade to
  `Skipped(reason)`.
- **Double status authority:** `board_sync` already closes GitHub issues when a
  mirrored card hits `done`. With `Done` label-only (D1) the two paths cannot
  race on open/closed state; the architect should still confirm `board_sync`
  never strips `status/*` labels.
- **Convention clash (resolved by D1):** this repo's own workflow closes issues
  via `Closes #N` reaching `main` — harness `Done` is therefore label-only;
  auto-close is deferred to a per-repo toggle follow-up.
- **Serde compat:** widening `CreateBody` is backward-compatible (unknown
  fields already ignored); old clients keep working.

## Harness wiring (the gate)

- **feature_list.json entries (build order per D4 — highest value first,
  dependencies before dependents):**
  - `F1 github-status-transition` (AC 3–5) — independently shippable, zero UI,
    pure-Rust unit gate; lands the headline value even if a later feature
    blocks.
  - `F2 worktree-linked-metadata` (AC 2) — small backend widening; must precede
    the composer so its QA can assert end-to-end persistence.
  - `F3 composer-create-issue` (AC 1)
  - `F4 spec-from-issue-scaffold` (AC 6–7)
- **`verify.sh` asserts (unit gate):** `cargo test -p agentum-server --lib` —
  new tests: GitHub transition `gh` argv builders per phase, incl. label
  ensure-create and the exactly-one-`status/*`-label invariant (mirroring the
  existing `gh_create_argv` tests), `github_transition_is_a_logged_noop`
  replaced by an `Applied` + failure→`Skipped` pair, `CreateBody` round-trips
  linked fields into the registry, issue-body→spec transform + `plan_from_spec`
  round trip preserves checkboxes and threads `tracker_provider`/`tracker_url`.
  Plus `npm run build --prefix crates/agentum-desktop/ui` green.
- **`qa.sh` asserts (browser QA gate):** open the composer against a scratch
  repo → "Create GitHub issue" renders and files (issue chip with number/URL
  visible) → workspace created with spec file present in the worktree → a
  demo harness run flips the issue's `status/*` labels at each phase
  transition, ends with exactly `status/done`, and the issue is still **open**.

## Decisions (PM-locked)

> Auto-resolved defaults (autonomous run, 2026-07-01): the recommendations
> shown to the human were adopted as scope decisions when the loop was armed.
> Any of these can be overridden later by a human note in `ai/STATE.md`.

1. **D1 — `Done` is label-only.** It applies `status/done` and never closes the
   issue; closing stays with the PR's `Closes #N` reaching `main` (this repo's
   own convention). A per-repo "close on Done" toggle is deferred to a
   follow-up spec (out of scope here).
2. **D2 — Transitions write via the `gh` CLI** (direct local `gh` from
   `neutral_cwd()`, exactly like creation — architecture C5; `gh_in_dir` only
   if a remote-host harness ever lands), matching the creation path: every
   environment that can create issues can also edit them,
   and no PAT onboarding exists in the composer flow. `forge_send`/PAT stays
   untouched for `board_sync`.
3. **D3 — Canonical label set:** `status/todo`, `status/in-progress`,
   `status/ready-to-test`, `status/done` — self-describing and harness-owned.
   NOT `.github/labels.sh`'s `status/qa*` names (a different, human-QA
   lifecycle). Labels are ensure-created idempotently with fixed colors;
   exactly one `status/*` label per issue after any transition.
4. **D4 — One spec, reordered build.** Increments A/B/C stay in this spec (the
   user's ask is the connected loop), but the backlog builds highest-value
   first: `F1 github-status-transition` → `F2 worktree-linked-metadata` →
   `F3 composer-create-issue` → `F4 spec-from-issue-scaffold`. F1 is
   independently shippable with a pure-Rust gate and no dependency on the
   composer; F2 precedes F3 because the composer's end-to-end QA needs the
   registry to persist what it creates.
5. **D5 — Spec scaffold is opt-in, off by default** (composer toggle, shown
   only when a GitHub issue is linked). Default-on is a follow-up once the
   transform has earned trust.
