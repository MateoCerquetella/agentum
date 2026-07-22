---
tracker: https://github.com/MateoCerquetella/agentum/issues/407
---

# Spec 024 — Restore project colors in the desktop sidebar

- **Number:** 024
- **Status:** Done
- **Surface:** `crates/agentum-desktop/ui`
- **Author:** Mateo Cerquetella
- **Date:** 2026-07-21
- **Tracker:** https://github.com/MateoCerquetella/agentum/issues/407

## Problem

A workspace operator scanning projects in the desktop sidebar sees neutral
grey/white project icons instead of each project's configured color. Projects
with similar names are therefore slower to distinguish during routine workspace
navigation.

## Goal

Expose each Projects-group header's valid configured project color as a persistent identity cue.

## User value

Project colors restore the workspace operator's quickest visual cue when navigating projects.

## Users / personas

- **Workspace operator** — scans the Projects-group headers before opening a
  project or workspace and relies on color as the quickest project-identity cue.

## Acceptance criteria

- [x] A project header rendered while the sidebar is grouped by Projects renders
   its valid configured `Repo.badgeColor` in an always-visible repo-only color
   mark; colorable default/Lucide icons retain their existing matching tint.
- [x] The project color mark retains the same configured color while its header is
   hovered, selected, dragged, or opened, while the existing interaction-state
   background and selection ring remain visible.
- [x] Switching between the existing light and dark themes preserves the configured
   project color mark; theme foreground tokens do not replace it with a neutral
   color.
- [x] A project with a missing, empty, or invalid `badgeColor` renders
   `DEFAULT_REPO_BADGE_COLOR`, and the invalid persisted value never reaches
   inline CSS.
- [x] Pinned headers and headers in non-project grouping modes retain their current
   theme-owned icon colors rather than receiving a project color.
- [x] `npm run build --prefix crates/agentum-desktop/ui` completes successfully,
   and focused sidebar tests cover valid, missing, invalid, interaction-state,
   and theme-independent project colors.

## Scope & non-goals (YAGNI)

- **In:** A persistent configured-color identity cue beside the existing
  project/repository header icon in
  the desktop sidebar; normal, hover, selected/opened, drag, light-theme,
  dark-theme, and fallback states; focused regression coverage.
- **Out:** Recoloring project names or workspace cards; adding a second color
  component; changing header typography, navigation, selection, collapse,
  drag behavior, the color picker, persisted metadata, project-group folder
  colors, or theme palettes.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- `VirtualizedWorktreeViewport` project-header rendering
  (`crates/agentum-desktop/ui/src/components/sidebar/WorktreeList.tsx:2970`) —
  already owns project grouping, active/selected/drag backgrounds, navigation,
  collapse, and the existing `RepoIconGlyph` call.
- `RepoIconGlyph`
  (`crates/agentum-desktop/ui/src/components/repo/repo-icon.tsx:61`) — the shared
  project icon renderer and the focused seam through which a validated color can
  reach the Lucide project glyph.
- `normalizeRepoBadgeColor` and `resolveRepoBadgeColor`
  (`crates/agentum-desktop/ui/src/shared/repo-badge-color.ts:5`) — validate
  persisted hex colors and provide the existing fallback behavior.
- `REPO_COLORS` and `DEFAULT_REPO_BADGE_COLOR`
  (`crates/agentum-desktop/ui/src/shared/constants.ts:119`) — remain the
  authoritative palette and fallback; no new color token is introduced.
- Existing header interaction classes in `WorktreeList.tsx` — active, selected,
  drag, status-drop, and pin-drop backgrounds/rings stay theme-owned and do not
  become project-colored.

### Build new

- An always-visible `RepoBadgeMark` composition in real `repo:*` headers that
  consumes the existing resolved color without recoloring the row or creating a
  new badge primitive.
- Focused unit/source regression tests proving valid and fallback resolution,
  non-project exclusion, icon wiring, and independence from interaction and
  theme foreground classes.

## Risks & invariants

- Persisted color strings are untrusted: only a value accepted by the shared
  normalizer, or `DEFAULT_REPO_BADGE_COLOR`, may reach inline CSS.
- The project color is an identity cue owned by the repo-only mark and existing
  colorable glyph tint. Header text,
  backgrounds, rings, and non-project icons remain owned by existing semantic
  theme classes so light/dark contrast and selection affordances do not regress.
- Project-header click-to-open, chevron collapse, multi-selection, drag reorder,
  sticky positioning, and virtualization behavior remain unchanged.
- Image and emoji project icons retain their authored appearance; the adjacent
  mark supplies their configured color cue without tinting or replacing custom
  image/emoji content.
- This UI-only slice does not touch agent launching, streaming, adapters, or the
  Harness Engine's execution semantics.

## Harness wiring (the gate)

- **feature_list.json entries:** `restore-project-header-colors`
- **`verify.sh` asserts:** focused Vitest coverage for project-header color
  normalization/fallback and `WorktreeList` icon wiring, followed by
  `npm run build --prefix crates/agentum-desktop/ui`.
- **`qa.sh` asserts:** in the desktop app, configure two projects with distinct
  colors and capture the Projects-group sidebar in normal, hover, selected/open,
  and drag states in both light and dark themes; confirm each color stays on the
  project icon while the existing state background remains visible. Set one
  persisted color to an invalid value and confirm the default neutral color is
  rendered instead of the invalid value.

## Open questions

- None. The tracker ask and existing project-icon/color seams define the slice.
