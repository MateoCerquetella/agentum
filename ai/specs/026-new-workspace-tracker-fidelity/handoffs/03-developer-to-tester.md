# Handoff — Developer to Tester (Reviewer retry 2)

- **Spec:** 026-new-workspace-tracker-fidelity
- **From:** Developer
- **To:** Tester
- **Date:** 2026-07-21
- **Gate:** PASS after Reviewer send-back iteration 1

## Delivered in Reviewer retry 2

- Added the shared editor's typed success-only `onUnbound` callback.
- TrackerSection immediately projects the current target to `absent`, nulls its
  current scope ref, clears table/status/query state, and closes the editor.
- Added production-seam coverage proving Configure tracker, no connected badge,
  zero eligible rows, and rejection of deleted-scope late completions.
- Updated stale selected-repo/global-fallback and SSH-configuration comments.

## Reviewer blocker disposition

- **B1: FIXED.** A successful inline unbind can no longer leave the deleted
  Project connected or selectable in New Workspace.

## Verification for Reviewer retry 2

- Focused unbind/scope test — **PASS** (2/2).
- `HARNESS_FEATURE_ID=wizard-closed-tracker-scope bash .harness/verify.sh` —
  **PASS** (71 focused tests, exact coordinate test, Vite build 1m11s, diff).
- Final `git diff --check` — **PASS**.

## Prior iteration coverage retained

- Preserved the existing AutoWiki harness features and added both Spec 026
  feature IDs with exact focused verification routes.
- Added QA routes that explicitly remain pending/nonzero until the real desktop
  local/SSH matrix is run; no live evidence is claimed.
- Added a TrackerSection commit-boundary regression that defers A, switches to B
  on the same Project, commits B, then proves late A cannot alter B's status,
  issue count, or rows.
- Added repo-switch linked-item clearing and linked/unlinked create payload
  regressions proving exact versus absent persisted tracker coordinates.

## Cumulative changed files

- `.harness/feature_list.json`
- `.harness/verify.sh`
- `.harness/qa.sh`
- `crates/agentum-desktop/ui/src/components/github-projects/ProjectBindingEditor.tsx`
- `crates/agentum-desktop/ui/src/components/new-workspace/tracker-section-scope.ts`
- `crates/agentum-desktop/ui/src/components/new-workspace/tracker-section-scope.test.ts`
- `crates/agentum-desktop/ui/src/components/new-workspace/CreateWorkspaceWizard.tsx`
- `crates/agentum-desktop/ui/src/components/new-workspace/create-workspace-wizard-model.ts`
- `crates/agentum-desktop/ui/src/components/new-workspace/create-workspace-wizard-model.test.ts`
- `crates/agentum-desktop/ui/src/hooks/useComposerState.ts`
- `crates/agentum-desktop/ui/src/store/slices/worktrees.test.ts`

## Acceptance-criteria evidence added

- **AC 4:** The exact async guard used by TrackerSection now has a deferred
  same-Project A→B test asserting status/count/rows remain B after A resolves.
- **AC 8:** A real repo change returns no linked item, and create-store tests
  assert the selected canonical URL is sent only for the linked create.
- **Harness contract:** Both Spec 026 IDs execute their promised focused gates;
  both QA routes refuse to pass unrun live QA.

## Verification

- `HARNESS_FEATURE_ID=binding-identity-fidelity bash .harness/verify.sh` —
  **PASS** (5 tracker + 4 resolver tests).
- `HARNESS_FEATURE_ID=wizard-closed-tracker-scope bash .harness/verify.sh` —
  **PASS** (70 focused tests, 1 exact worktree test, Vite build in 1m42s,
  `git diff --check`).
- Harness JSON/shell syntax — **PASS**.
- Both new QA routes — correctly report `PENDING` and exit 2.
- Final `git diff --check` — **PASS**.

## Remaining risk / Tester action

Independently rerun the wizard harness route and reassess B1/AC 2/AC 6.
Real Agentum/xcode-theme, repeated switching, SSH, and persisted-coordinate QA
remains explicitly unrun until a current-build desktop and named safe fixtures
are available.
