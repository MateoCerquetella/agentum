# Verification — Spec 024: restore project colors in the desktop sidebar

- **Status:** Tester PASS
- **Date:** 2026-07-21
- **Scope:** Independent audit of the existing three-file implementation candidate

## Automated results

| Gate | Result |
| --- | --- |
| `(cd crates/agentum-desktop/ui && npx vitest run src/components/sidebar/project-header-color.test.ts src/components/sidebar/worktree-list-groups.test.ts)` | **PASS:** 2 files, 108 tests, 0 failures; Vitest duration 4.06s. |
| `npm run build --prefix crates/agentum-desktop/ui` | **PASS:** Vite transformed 7,236 modules and built in 4m36s. Existing dynamic/static import and chunk-size warnings were non-fatal. |
| `git diff --check` | **PASS:** no output. |

The focused tests were intentionally run from the UI package so its Vite/Vitest
configuration and `@` alias were loaded. This is the package-context equivalent
specified by the Developer handoff; the previously documented repository-root
`npx --prefix ... vitest` resolution failure was not treated as a product defect.

## Acceptance-criteria evidence

| Acceptance criterion | Independent evidence | Result |
| --- | --- | --- |
| Valid configured project color is always visible in Projects grouping | `resolveProjectGroupHeaderColor` preserves palette and normalized custom hex values. `WorktreeList` passes the result to `RepoIconGlyph` and renders an unconditional repo-only `RepoBadgeMark`. Focused tests passed. | **PASS** |
| Color survives hover, selected/open, and drag states while semantic state styling remains | The mark is outside every interaction-state conditional. Existing `bg-sidebar-accent`, selection ring, and drag ring/shadow classes remain on the header row. Source-contract assertions cover active/selected separation; static inspection confirms drag separation and hover inheritance. | **PASS (automated/source)** |
| Light and dark themes do not replace the configured color | Inline mark/glyph color is independent of theme foreground classes; header content uses `text-sidebar-foreground` and state backgrounds remain semantic tokens. Source-contract tests and production build passed. | **PASS (automated/source)** |
| Missing, empty, or invalid color falls back before inline CSS | Resolver table tests cover `undefined`, `null`, empty, and invalid values. All flow through `normalizeRepoBadgeColor` and resolve to `DEFAULT_REPO_BADGE_COLOR` before reaching `RepoBadgeMark`. | **PASS** |
| Pinned and non-project modes retain theme-owned colors | Resolver tests reject pinned headers and repo-looking keys in alternate grouping modes. The mark render branch additionally requires `row.repo`; non-repo icons continue to render without the project color prop. | **PASS** |
| Focused tests and production build succeed | 108 focused tests passed and the production UI build completed successfully. | **PASS** |

## Scope and regression review

- Production behavior changed only in `WorktreeList.tsx`; the shared normalizer,
  badge primitive, icon renderer, persistence, grouping, selection, navigation,
  collapse, drag mechanics, and virtualization were not modified.
- The implementation adds a fixed `size-2 shrink-0` mark and retains the
  existing `min-w-0 truncate` label path.
- No invalid inline-CSS path, non-project color leakage, or architecture
  deviation was found. No production or test code was modified by Tester.
- Unrelated dirty-worktree changes were preserved.

## Manual QA deferrals

The following visual checks require a running desktop app and are deferred to
Reviewer/manual QA; they are not contradicted by the automated or source-level
evidence:

- compare two distinct project colors in normal, hover, selected/open, and drag
  states in both light and dark themes;
- visually confirm the adjacent mark remains visible for emoji and image repo
  icons while those authored icons remain untinted;
- inject an invalid persisted color in a live session and visually confirm the
  default neutral mark.

Tester gate: **PASS**. The implementation satisfies all machine-verifiable
acceptance criteria, the requested build/test gates are green, and the remaining
items are explicit visual QA deferrals rather than reproducible defects.
