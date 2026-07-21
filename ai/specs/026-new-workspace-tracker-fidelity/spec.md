# Spec 026 — New Workspace tracker fidelity

- **Number:** 026
- **Status:** Done
- **Surface:** `crates/agentum-server/src/routes`, `crates/agentum-desktop/ui/src/components/new-workspace`
- **Author:** Codex (from Mateo's screenshot and direct ask via Agentum SDD)
- **Date:** 2026-07-21

## Problem

After selecting `xcode-theme` in New Workspace, step 3 reports “Tracker
connected” and renders issues from Agentum's board. The UI presents valid-looking
data from the wrong project, so an operator can link a workspace to an unrelated
ticket without any visible warning.

## Goal

Make New Workspace step 3 render only tracker configuration and work items that
belong to the currently selected Agentum project.

## Users / personas

- **Multi-project operator:** selects a repository in New Workspace and expects
  the issue picker to be a closed scope for that project before creating a
  worktree or launching an agent.
- **Local/SSH project maintainer:** changes a project's tracker and expects that
  choice to affect only that project on the next visit to step 3.

## Acceptance criteria

1. For a selected git project, `GET /api/github/project-binding` returns a
   binding only when the canonical config's `Repo.id` and normalized GitHub
   repository slug both match the selected repo's server-resolved origin;
   mismatches never return another project's binding.
2. A selected project with no matching tracker binding renders “Configure
   tracker,” zero issue rows, and no “connected” badge. It never reads
   `settings.githubProjects.activeProject` or any globally last-used tracker as
   a fallback.
3. A stale **migrated** canonical row whose target slug differs from the selected
   repo is removed and re-migrated from that repo's exact origin. A mismatched
   explicitly **configured** row is preserved, returns an actionable
   `tracker_target_mismatch` error, and renders a reconfigure affordance without
   rendering its issues.
4. After switching from project A to project B, binding state and issue rows for
   A disappear synchronously. Late binding/table responses for A are rejected
   and cannot change B's status, count, or list.
5. When a matching GitHub Project is configured, the picker renders only open
   issue rows whose repository slug matches the selected project's configured
   repository. Pull requests, drafts, closed/redacted rows, and issues belonging
   to other repositories in the same GitHub Project are excluded.
6. “Change tracker” persists through the selected `Repo.id` write path and the
   selected repo's resolved slug. Changing project A leaves project B's binding,
   cached rows, and task preferences byte-unchanged.
7. Local and SSH projects follow the same identity checks. SSH slug resolution
   runs through the registered repo host; a missing/unreachable host fails
   closed and never retries against the local filesystem or a global tracker.
8. Creating a workspace without selecting an issue remains allowed. When an
   issue is selected, the persisted `trackerProvider` and `trackerUrl` match the
   visible row and cannot originate from a previously selected project.

## Scope & non-goals (YAGNI)

- **In:** New Workspace step-3 binding resolution, migrated/configured mismatch
  handling, repo-filtered GitHub Project rows, repo-switch race protection,
  configure/change persistence, local/SSH behavior, and exact worktree linkage.
- **Out:** the broader Spec 025 persistence redesign; new tracker providers;
  GitHub Project visual redesign; changing GitHub/Linear credentials; polling or
  webhooks; changing agent launch, worktree creation, or harness gate behavior.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- `deriveTrackerBindingTarget` and `pickerBindingTargetKey`
  (`create-workspace-wizard-model.ts:225-233`,
  `work-item-picker-model.ts:116-127`) — selected `Repo.id` + workdir identity.
- `resolvePickerProject` (`work-item-picker-model.ts:141-163`) — current
  fail-closed rule for selected git repos; strengthen its integration tests,
  do not add another resolver.
- `TrackerSection` request/project generation guards
  (`CreateWorkspaceWizard.tsx:1581-1730`) — reuse for synchronous invalidation
  and late-response rejection.
- `resolve_tracker_slug` and the repo-aware GitHub binding route
  (`routes/util.rs:110`, `routes/github_projects.rs:281-304`) — authoritative
  local/SSH origin resolution.
- Spec 025's canonical compatibility helpers
  (`routes/project_trackers.rs`) — validate/repair the stored target behind the
  existing binding API rather than adding a wizard-only endpoint.
- `deriveIssueOptions` / `deriveTrackerIssueViewModel`
  (`work-item-picker-model.ts`) and existing table cache/fetch actions — extend
  eligibility with the selected repository slug; do not refetch via a parallel
  client.
- Existing `applyLinkedWorkItem` and worktree create coordinates
  (`CreateWorkspaceWizard.tsx:270-290`, `store/slices/worktrees.ts:1081-1110`)
  — preserve the single exact-ticket linkage path.

### Build new

- Canonical binding projection that compares stored target slug to the
  server-resolved selected origin before returning a binding.
- Safe repair for mismatched migrated rows and typed fail-closed handling for
  mismatched explicitly configured rows.
- Repository-slug eligibility filtering for GitHub Project issue rows.
- Focused route/model/component regressions reproducing `xcode-theme` displaying
  Agentum issues, including repo switches and SSH resolution failures.

## Risks & invariants

- **False success is the primary hazard:** a wrong tracker must render as an
  error/unconfigured state, never as connected data.
- **Configured data is user-owned:** automatic repair may replace only rows
  marked migrated; explicit configuration is preserved for human correction.
- **Cross-repository Projects:** Project membership alone does not make an issue
  eligible; row repository identity must match the selected project.
- **Race safety:** cache and response acceptance use both repo identity and
  tracker target identity, not display name or currently active global state.
- **Host isolation:** repo IDs resolve SSH execution; never interpret a remote
  path locally.
- The change does not touch `spawn_agent_into_pane`, YOLO translation,
  per-session UUIDs, or push-based streaming.

## Harness wiring (the gate)

- **feature_list.json entries:**
  1. `binding-identity-fidelity` — validate canonical Repo.id + slug, repair only
     migrated mismatches, and type configured mismatches.
  2. `wizard-closed-tracker-scope` — no global fallback, repo-filtered rows,
     synchronous repo-switch invalidation, and exact selected-ticket linkage.
- **`verify.sh` asserts:** focused Rust tests cover matching, migrated mismatch
  repair, configured mismatch preservation/error, two-repo isolation, and SSH
  failure; focused Vitest covers selected-unbound → zero rows, mixed-repository
  Project filtering, stale response rejection, and selected-row bind payload;
  Vite production build, relevant server library tests, and `git diff --check`
  are green.
- **`qa.sh` asserts:** in the real desktop, select unbound `xcode-theme` after
  viewing Agentum and capture Configure tracker with zero Agentum rows; bind
  xcode-theme, confirm only xcode-theme issues render; switch repeatedly between
  both projects; repeat the unbound/bound flow for an SSH repo; create one linked
  and one unlinked workspace and inspect their persisted tracker coordinates.

## Open questions

- None blocking. The selected `Repo.id` and its server-resolved origin are the
  authority; global tracker state and cross-repository Project rows are never
  eligible in this wizard.
