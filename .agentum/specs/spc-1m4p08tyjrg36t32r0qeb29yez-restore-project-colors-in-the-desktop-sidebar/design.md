# Architecture — Spec 024: restore project colors in the desktop sidebar

- **Status:** Architect PASS
- **Date:** 2026-07-21
- **Grounded at:** `8cb2a502` plus the current Spec 407 Harness patch

## Collision finding

The Agentum Harness Engine already implemented and review-gated issue #407 in
this worktree. Its artifacts at
`.agentum-harness/specs/407-fix-colors-in-sidebar-on-projects-now-th/` and
`.agentum-harness/decisions.md` record Architecture PASS, Reviewer PASS, and a
green UI build. Developer must audit and adopt that existing diff; it must not
build a parallel color path.

The PM handoff initially prohibited a second mark. Grounding showed that rule
was incorrect: `RepoIconGlyph` intentionally ignores `color` for image and
emoji icons (`components/repo/repo-icon.tsx:72-93`). The spec is amended to
require the existing `RepoBadgeMark` primitive as the reliable color cue for
every repo icon mode.

## Components

### Production change

- `crates/agentum-desktop/ui/src/components/sidebar/WorktreeList.tsx`
  (`row.type === 'header'`, around lines 2968-3190): keep resolving
  `repoHeaderColor` from `groupBy`, `row.key`, and `row.repo?.badgeColor`; keep
  passing it to `RepoIconGlyph`; render `RepoBadgeMark` only when `row.repo`
  exists, with the resolved color and a fixed `size-2 shrink-0 rounded-[2px]`
  footprint. Preserve `bg-sidebar-accent`, selection/drag rings, truncation,
  navigation, collapse, and virtualization. The header foreground explicitly
  inherits `text-sidebar-foreground` so content contrast stays theme-owned.

### Existing primitives reused unchanged

- `components/sidebar/project-header-color.ts:6-28` owns repo-header gating,
  normalization, and fallback.
- `shared/repo-badge-color.ts:5-28` accepts normalized hex values and rejects
  invalid persisted strings.
- `shared/constants.ts:119-130` owns `REPO_COLORS` and
  `DEFAULT_REPO_BADGE_COLOR`.
- `components/repo/RepoBadgeLabel.tsx:4-14` already supplies
  `RepoBadgeMark`; no new badge component or color parser is allowed.
- `components/repo/repo-icon.tsx:61-102` retains existing Lucide tint and
  authored emoji/image rendering.

### Tests

- `components/sidebar/project-header-color.test.ts`: table-test missing,
  `null`, empty, and invalid values at the exact resolver consumed by the mark;
  retain configured palette/custom hex and non-project exclusion coverage.
- `components/sidebar/worktree-list-groups.test.ts` under
  `WorktreeList header styles`: use the established source-contract pattern to
  pin the repo-only mark, `repoHeaderColor` wiring, active/selected neutral
  classes, semantic foreground, and label truncation.

## APIs and data flow

No wire, store, persistence, or public component API changes.

```text
Repo.badgeColor
  -> WorktreeList repo header
  -> resolveProjectGroupHeaderColor(groupBy, row.key, badgeColor)
  -> normalizeRepoBadgeColor
  -> configured canonical/custom hex OR DEFAULT_REPO_BADGE_COLOR
  -> RepoIconGlyph tint (Lucide/default only)
  -> RepoBadgeMark backgroundColor (every repo icon mode)
```

Non-repo headers receive `undefined` from the resolver and never enter the
`row.repo` mark branch.

## Important decisions and tradeoffs

1. **Persistent swatch over colored row/text.** A small mark exposes identity
   across every interaction state without contrast math or per-theme text
   colors. The existing state background and ring remain readable.
2. **Swatch plus existing glyph tint over glyph tint alone.** This duplicates a
   tiny color cue for Lucide icons, but covers emoji/image icons whose authored
   pixels cannot accept a CSS color prop.
3. **Existing source-contract test over extracting a test-only component.**
   `WorktreeList` is large and virtualized, and the current suite already tests
   its header JSX through source assertions. A new presentation abstraction
   solely for this mark would add speculative structure.
4. **Audit the current Harness patch over reimplementation.** The exact patch
   already passed its review/build gate. Developer should remove only deviations
   from this architecture and preserve unrelated dirty-worktree changes.

## Risks and mitigations

- **Invalid CSS input:** only the normalized/fallback resolver reaches inline
  style; table tests include all invalid/missing shapes.
- **Theme contrast regression:** project color is confined to the swatch and
  existing colorable glyph; semantic foreground/background/ring tokens remain
  authoritative and are source-pinned.
- **Long-label compression:** the fixed `size-2 shrink-0` mark is bounded and
  existing `min-w-0 truncate` remains unchanged.
- **Non-project leakage:** both resolver key/group gating and the `row.repo`
  render branch exclude pinned/status/folder/host headers.
- **Dirty-worktree collision:** Developer must inspect `git diff`, touch only
  the three sidebar files in this slice, and preserve all other changes.

## Acceptance criteria mapping

| AC | Plan | Verification |
| --- | --- | --- |
| Configured color renders | Repo-only `RepoBadgeMark` plus existing glyph tint | `renders the resolved project color mark in repo headers`; resolver configured-color test |
| Interaction states retain color and contrast | Unconditional mark; unchanged accent/ring classes | `keeps project color independent from active and selected header styling` |
| Light/dark themes | Semantic foreground and neutral state tokens remain owners | `uses the sidebar foreground token for readable light and dark theme content`; production build |
| Invalid/missing fallback | Existing normalization and `DEFAULT_REPO_BADGE_COLOR` | Resolver fallback table for missing/null/empty/invalid |
| Non-project exclusion | Resolver group/key guard plus `row.repo` branch | Existing pinned and alternate-group resolver tests |
| Build and focused tests | Existing package commands | Focused Vitest command below; Vite production build |

## Developer gate

```sh
(cd crates/agentum-desktop/ui && npx vitest run \
  src/components/sidebar/project-header-color.test.ts \
  src/components/sidebar/worktree-list-groups.test.ts)
npm run build --prefix crates/agentum-desktop/ui
git diff --check
```

Architect gate: **PASS**. Components and boundaries are explicit, the central
swatch tradeoff is grounded, every risk has a mitigation, every AC maps to a
named test, and no new abstraction or API is introduced.
