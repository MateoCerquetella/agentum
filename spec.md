# Spec 407 — Restore project colors in the desktop sidebar

- **Status:** PM ready
- **Surface:** `crates/agentum-desktop/ui`
- **Tracker:** https://github.com/MateoCerquetella/agentum/issues/407

## Problem

A workspace operator scanning projects in the desktop sidebar sees only neutral grey/white
project marks, so projects with similar names are slower to distinguish. The pain occurs
during routine workspace navigation, before the operator opens a project or workspace.

## Goal

A workspace operator identifies a desktop-sidebar project by its configured color.

## User value

Project colors restore the operator's quickest visual cue when navigating workspaces.

## Acceptance criteria

- [ ] A project header grouped under Projects renders its valid configured `Repo.badgeColor` on the project icon in the normal state.
- [ ] The same project header retains that configured icon color when hovered, selected, or opened while the existing interaction-state background remains visible.
- [ ] Switching between the existing light and dark themes renders the same configured project icon color without replacing it with a neutral theme color.
- [ ] A project with a missing or invalid `badgeColor` renders `DEFAULT_REPO_BADGE_COLOR` and does not render the invalid value.
- [ ] `npm run build --prefix crates/agentum-desktop/ui` completes successfully, and focused sidebar color tests pass.

## Scope and non-goals

- **In scope:** the project/repository header icon color in the desktop sidebar, including
  normal, hover, selected/opened, light-theme, dark-theme, and fallback states.
- **Out of scope:** recoloring project names or workspace cards; changing the color picker,
  stored metadata, project-group folder colors, theme palettes, layout, or interactions.

## Existing code to reuse

- `WorktreeList` owns project-header rendering and interaction states in
  `crates/agentum-desktop/ui/src/components/sidebar/WorktreeList.tsx`; reuse this path.
- `Repo.badgeColor` is the configured project color in
  `crates/agentum-desktop/ui/src/shared/types.ts`.
- `normalizeRepoBadgeColor` and `DEFAULT_REPO_BADGE_COLOR` validate and default colors in
  `crates/agentum-desktop/ui/src/shared/repo-badge-color.ts` and `shared/constants.ts`.
- `RepoIconGlyph` is the existing project icon; its focused color seam and tests live in
  `components/sidebar/project-header-color.ts` and `project-header-color.test.ts`.

## Invariants

- Existing project-header navigation, selection, collapse, drag, and theme behavior persist.
- Untrusted persisted color strings never reach inline CSS without color normalization.

No existing spec in `ai/specs` targets project-header badge-color restoration; tracker-status
chips and other sidebar color work are separate surfaces.
