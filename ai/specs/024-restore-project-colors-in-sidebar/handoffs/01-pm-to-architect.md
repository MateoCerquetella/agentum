# Handoff 01 — PM → Architect

- **Spec:** 024-restore-project-colors-in-sidebar
- **Date:** 2026-07-21
- **From:** PM (autonomous SDD loop; canonical harness PM role used because the legacy `ai/roles/pm.md` scaffold is absent)
- **To:** Architect
- **Artifact:** `ai/specs/024-restore-project-colors-in-sidebar/spec.md`
- **Tracker:** https://github.com/MateoCerquetella/agentum/issues/407

## Gate result

PM gate: **PASS** after one format-only refinement. All acceptance criteria are
now harness checkboxes and the user value is explicit. The product scope and
technical intent did not change.

- One slice: one existing sidebar project icon receives one validated identity
  color.
- Problem first: the spec describes slower project recognition before naming
  the rendering seam.
- Persona: workspace operator scanning Projects-group headers.
- Testability: six observable criteria cover normal, interaction, theme,
  fallback, exclusion, and build/test outcomes.
- Non-goals: no project-name/workspace-card recoloring, second mark, layout,
  behavior, persistence, picker, or theme changes.
- Code grounding: `WorktreeList`, `RepoIconGlyph`, shared badge-color
  normalization, and palette constants were verified in this worktree.
- Invariants: untrusted color strings are normalized; navigation, selection,
  drag, collapse, virtualization, and theme-owned state styling remain intact.
- Harness wiring: one feature with focused Vitest/build and desktop QA gates.
- Duplicate check: no existing `ai/specs/*/spec.md` targets project-header icon
  color restoration.

## Decisions locked for architecture

1. Color belongs on the existing colorable default/Lucide project glyph only.
   Do not add a second badge mark or recolor the project name.
2. Resolve color only for real `repo:*` headers while grouped by Projects;
   pinned and other grouping headers retain their existing semantic tones.
3. Normalize every persisted color before inline rendering and fall back to
   `DEFAULT_REPO_BADGE_COLOR` for missing or invalid values.
4. Existing active, selected, drag, hover, sticky, and light/dark theme classes
   remain the owners of row background, ring, and text contrast.

## Architect output expected

Create `architecture.md` with the smallest component/helper/test change set,
line-verified seams, and exact focused verification commands. Then write the
Architect→Developer handoff.
