# Spec 025 — Issue-first New Work

- **Number:** 025
- **Status:** Done             <!-- Draft | PM | Architect | In progress | Done -->
- **Surface:** `crates/agentum-desktop/ui` + `crates/agentum-server`
- **Author:** Mateo Cerquetella (drafted with Codex)
- **Date:** 2026-07-22

## Problem

An operator can draft and file a new issue inside the New Workspace wizard, but
filing the issue does not establish runnable work: the worktree may be created
without any spec because both spec scaffolding and the gated run are separate,
default-off choices. The nearby rainbow Loop drives the incompatible repo-native
`ai/STATE.md` workflow, so a reasonable "create an issue, create its worktree,
then loop it" journey strands the operator between two execution systems.

## Goal

Make one New Work submission turn a new or existing local GitHub issue into an
issue-linked worktree with a prepared Harness spec and an explicit automatic or
manual execution outcome.

## Users / personas

- **Mateo, operating several coding agents:** while creating a worktree for work
  that has not been filed yet, he wants to describe the issue once and press one
  launch button, then either let Agentum drive the SDD run or open the prepared
  workspace himself without remembering a separate spec step.

## Acceptance criteria

1. The final New Work step renders one mutually exclusive work source:
   **New issue** or **Existing issue**. New-issue title, description, labels, and
   the existing AI description-draft action remain editable in the wizard and
   do not file anything before the final primary action; Existing issue keeps
   the current project-scoped issue picker.
2. For a new issue, the final primary action renders **Create issue & start
   work**; it files exactly one GitHub issue, binds the returned issue identity
   to the new worktree, and derives the editable worktree name from the issue
   title. For an existing issue it renders **Create worktree & start work** and
   does not create another issue.
3. The launch step renders two mutually exclusive execution outcomes:
   **SDD Autopilot** (default for an eligible local GitHub issue) and **Open
   manually**. Its user-facing copy names the PM → Architect → Build → Verify →
   Review progression; it does not expose "Harness", "scaffold", or "gated
   run" as primary-workflow terminology.
4. Every successfully created local GitHub issue-backed worktree receives
   `.agentum-harness/specs/<issue-derived-id>/spec.md` before the workflow is
   reported ready, regardless of execution outcome. The standalone
   `scaffoldSpec` opt-in and its checkbox are removed from this path.
5. SDD Autopilot reuses `POST /api/harness/start-work` to converge the spec,
   plan the backlog, stamp the selected agent/model and tracker provenance,
   register the run, and start the existing Harness drive loop. The composer
   opens no competing plain agent when the run takes ownership; all agent
   spawns continue through `spawn_agent_into_pane`.
6. Open manually reuses `POST /api/harness/spec-from-issue` to converge the
   issue-derived spec without starting the Harness driver, then opens the
   selected plain agent through the existing workspace activation path. The
   prepared workspace can later be adopted by `start-work` without overwriting
   a human-edited spec.
7. The submission surface renders ordered progress for issue, worktree, spec,
   and run preparation. If issue creation succeeds and a later step fails, the
   filed issue remains bound in the still-open wizard and Retry resumes from
   the first incomplete step; it never files a duplicate issue during that
   wizard lifetime. A created worktree is never rolled back or silently hidden.
8. An ineligible selection (remote/SSH repo, non-GitHub issue, non-git folder,
   or unavailable agent) renders the precise unsupported outcome before
   submission and falls back only to an explicitly selected compatible manual
   path; SDD Autopilot never silently degrades into a plain session.

## Scope & non-goals (YAGNI)

- **In:** the local GitHub New Work path; new-versus-existing issue intent;
  deferred filing; automatic Harness-spec preparation; SDD Autopilot versus
  manual execution; resumable in-wizard partial-failure feedback; copy changes
  on this surface.
- **Out:** removing the standalone Chat page or project Chat tab; porting its
  multi-turn Socratic interviewer into the wizard (the existing one-shot
  description draft remains); redesigning the repo-native `ai/STATE.md` rainbow
  Loop; Linear/board issue creation parity; SSH Harness execution; durable
  recovery after the app or wizard is closed; changing Harness gates, roles, or
  feature-state semantics.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- `CreateWorkspaceWizard` and `AgentStep`
  (`crates/agentum-desktop/ui/src/components/new-workspace/CreateWorkspaceWizard.tsx`)
  — the single three-step workspace front door already renders issue selection,
  issue creation, agent selection, and the current gated-run toggle.
- `useComposerState::handleCreateIssueSubmit` and `submitQuick`
  (`crates/agentum-desktop/ui/src/hooks/useComposerState.ts`) — existing issue
  payload construction, linked-item binding, worktree creation, setup policy,
  agent launch suppression, and failure toasts; refactor these seams rather than
  introduce a second composer.
- `draftGithubIssueBody` / `createGithubIssue`
  (`crates/agentum-desktop/ui/src/runtime/github-issue-client.ts`) — the existing
  reviewed AI-description draft and authoritative issue-creation clients.
- `POST /api/harness/spec-from-issue` and shared `ensure_spec_and_plan`
  (`crates/agentum-server/src/routes/harness.rs`) — deterministic issue-to-spec
  materialization with keep-existing semantics and tracker-stamped planning.
- `POST /api/harness/start-work` (`routes/harness.rs::start_work`) — serialized,
  convergent spec → plan → register → drive orchestration with an
  already-running result and no alternate spawn path.
- `createWorktree` plus the persisted `linkedIssue` / tracker bind coordinates
  in `useComposerState::submitQuick` — the existing authoritative worktree and
  metadata path.

### Build new

- A pure New Work intent/launch model covering New issue versus Existing issue,
  SDD Autopilot versus Open manually, eligibility, contextual CTA copy, ordered
  progress, and retry-resume state.
- A deferred issue-draft submit seam: the inline New issue editor stages data;
  final submit performs the existing create call once, binds its returned
  identity, and continues through the existing worktree pipeline.
- A required manual preparation branch that converge-scaffolds the spec before
  opening the plain agent, replacing the default-off `scaffoldSpec` behavior.
- Focused wizard presentation for the work source and execution outcome; remove
  the technical gated-run/scaffold controls and copy from the primary flow.

## Risks & invariants

- **One launch path is sacred:** Autopilot delegates spawning to Harness drive;
  manual mode delegates to the existing activation path. A submission must
  never do both.
- **The green gate is sacred:** the pivot changes intake and launch ownership,
  never the verify/QA gates or their fail-closed behavior.
- **Issue creation is externally irreversible:** after GitHub returns success,
  retain and reuse that identity for every retry; never simulate rollback by
  deleting the issue.
- **Worktree creation is locally durable:** a post-create failure must expose
  the surviving worktree and a recovery action rather than create a replacement.
- **Human edits are durable:** start-work convergence must retain the current
  keep-existing spec semantics and never overwrite an existing `spec.md`.
- **Eligibility must be honest:** remote and non-GitHub paths remain visible but
  cannot be mislabeled as SDD Autopilot-capable.
- **No registry wire widening:** preserve the canonical linked-work-item metadata
  keys; do not add serde aliases that can reintroduce registry rewrite loss.

## Harness wiring (the gate)

- **feature_list.json entries:** `F1 deferred-new-issue-intent` → `F2
  issue-backed-spec-invariant` → `F3 explicit-execution-and-recovery`.
- **`verify.sh` asserts:** focused UI model/hook tests prove contextual labels,
  default Autopilot eligibility, one issue-create call across a failed-step
  retry, new/existing branching, mandatory spec preparation in both modes,
  Autopilot plain-session suppression, manual single-session launch, and honest
  ineligible states; server tests keep `start-work` convergence/driver claims
  and `spec-from-issue` keep-existing behavior green; then
  `npm run build --prefix crates/agentum-desktop/ui` and
  `cargo test -p agentum-server --lib` pass.
- **`qa.sh` asserts:** in a scratch local GitHub repo, enter a not-yet-filed
  issue and confirm nothing exists before final submit; submit Autopilot and
  observe exactly one issue, one linked worktree, one issue-derived spec, one
  Harness-owned agent, and ordered progress. Repeat with Open manually and
  observe the same issue/worktree/spec invariant with exactly one plain agent
  and no active Harness driver. Force a post-issue failure, Retry, and verify no
  duplicate issue is filed.

## Product decisions (PM-locked)

1. **The issue is the canonical user-authored intake; the Harness spec is a
   generated execution artifact.** Users edit/review the issue draft and choose
   an execution outcome, but never make a separate "create spec" decision.
2. **SDD Autopilot is the eligible-path default, not an implicit mandate.** Open
   manually remains a first-class explicit choice and receives the same prepared
   issue-derived spec so the worktree is adoptable later.
3. **One final action owns filing and launch.** The New issue editor stages data;
   it does not create an externally durable issue early. After filing succeeds,
   the returned issue identity is the retry token for the remaining in-wizard
   steps.
4. **No silent execution fallback.** Autopilot failure or ineligibility stays
   visible and recoverable; Agentum never starts a plain agent merely because
   the requested autonomous driver failed.
5. **Chat and repo-native Loop are follow-ups.** This slice proves the canonical
   issue-first launch surface before navigation is removed or the separate
   `ai/STATE.md` loop is redesigned.

## Open questions

- None blocking. The standalone Chat/Loop navigation pivot is intentionally a
  follow-up after this single front door proves the issue-first launch model.
