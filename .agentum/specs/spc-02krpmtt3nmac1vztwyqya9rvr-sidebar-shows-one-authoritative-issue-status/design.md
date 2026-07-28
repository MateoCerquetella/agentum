# Architecture — Spec 024: Sidebar single authoritative issue status

- **Status:** Architect complete
- **Date:** 2026-07-21
- **Base:** `8cb2a502`
- **Surface:** Desktop UI only

## Current-state findings

1. `WorktreeCardDetailsHover` obtains the fetched GitHub labels from
   `issue.labels` and the authoritative Project option from
   `useIssueProjectStatus` (`WorktreeCardMeta.tsx:210-236`, `:255`).
2. The issue badge row renders `IssueProjectStatusChip` and then maps every
   `issueLabels` entry without classification (`WorktreeCardMeta.tsx:320-333`).
   This is the complete duplicate-render seam.
3. The Project-status hook already implements cache peeking,
   stale-while-revalidate, tracker-event invalidation, warnings, and worktree
   reconciliation (`IssueProjectStatusChip.tsx:92-183`). None of those behaviors
   needs to change.
4. The server owns six default Agentum lifecycle labels: five pipeline names at
   `task_sink.rs:348-357` plus fixed `status/blocked` at `:360-375`. Human
   `status/qa*` labels are explicitly outside that set (`:348-350`).
5. The existing static-render test already hoists a mutable Project-status mock
   and renders the full hover (`WorktreeCardMeta.test.tsx:30-159`), so the bug can
   be pinned without a new test harness or native runtime.

## Decisions

### D1 — Keep the classifier in the sidebar UI module

Add a module-scope `ReadonlySet<string>` containing exactly:

- `status/todo`
- `status/in-progress`
- `status/in-review`
- `status/ready-to-test`
- `status/done`
- `status/blocked`

Add/export a pure helper near `WorktreeCardDetailsHover`, conceptually:

```ts
visibleIssueLabels(labels, projectStatus): readonly string[]
```

When `projectStatus` is absent/blank, return the input labels unchanged. When it
is non-empty, return labels whose exact name is not in the set.

Why: this keeps the presentational policy beside its only consumer, avoids a
cross-crate dependency from TypeScript into Rust internals, and makes the
fallback/reference-preserving path explicit. A comment cites the server source
of truth so future lifecycle-label changes have a visible synchronization seam.

### D2 — Filter at render time, after Project status resolution

Derive `visibleIssueLabels` synchronously from `issueLabels` and
`projectStatus.status` in `WorktreeCardDetailsHover`. Use the derived array both
for the badge-row visibility condition and the label map.

Why: no state or effect is needed. A cached/resolved Project status immediately
suppresses canonical labels; a missing status leaves all labels visible; when a
background refresh changes the status, React recomputes the exact correct view.

### D3 — Exact, case-sensitive matching only

Do not use `startsWith('status/')`, regexes, case folding, or semantic mapping.
GitHub's returned canonical names are compared exactly. Consequently
`status/qa`, `status/qa-pass`, `status/qa-fail`, arbitrary user labels, and
case-distinct custom names remain visible.

Why: this is the narrowest policy that satisfies AC 1-3 and cannot swallow
human release state or user taxonomy.

### D4 — Preserve the existing authoritative chip and warning behavior

Do not modify `IssueProjectStatusChip.tsx`, `TrackerPhaseChip.tsx`, the project
binding client, the issue cache, or tracker events. The existing Project chip
remains first in the row and `projectStatus.warning` remains below it unchanged.

## Data and control flow

```text
issue cache --------------------------> issue.labels
GitHub Project status hook -----------> projectStatus.status
                                            |
                                            v
                              visibleIssueLabels(labels, status)
                                  | status missing: all labels
                                  | status present: remove exact six
                                            |
                                            v
                         Project chip + filtered ordinary/QA badges
```

There is no write path. No fetched data or cache entry is mutated; `Array.filter`
creates a render-local array only on the bound/status-present path.

## Race and error handling

- **Initial/loading/unbound/error:** `projectStatus.status` is null, so every
  fetched label remains visible as the fallback.
- **Cached status:** the hook initializes from cache, so canonical labels are
  suppressed on the first render when authoritative cached data exists.
- **Background resolution:** a null→status transition recomputes the filtered
  array in the same React render that paints the Project chip; there is no second
  effect or intermediate state to race.
- **Status invalidation/refetch:** stale-while-revalidate keeps the last-known
  Project status, matching current behavior. The filter follows that same value.
- **Warning:** warnings remain independently rendered and are never classified as
  labels.

## Exact files and seams

1. `crates/agentum-desktop/ui/src/components/sidebar/WorktreeCardMeta.tsx`
   - Add the exact canonical-name set and pure `visibleIssueLabels` helper.
   - Derive the displayed labels from `issueLabels` + `projectStatus.status`.
   - Use the displayed labels in the row predicate and `.map`.
2. `crates/agentum-desktop/ui/src/components/sidebar/WorktreeCardMeta.test.tsx`
   - Add the bound collision regression from AC 4.
   - Add the no-Project-status fallback regression from AC 3.

No other source file is required.

## Acceptance-criteria mapping

| AC | Implementation seam | Verification |
|---|---|---|
| 1 | Exact-name filter applied when `projectStatus.status` is non-empty | Static render asserts one Project chip/status and no canonical label text |
| 2 | Set contains only six Agentum names | Static render preserves `status/qa` and `area/desktop` |
| 3 | Helper returns all labels when status is null/blank | Static render with mocked null status preserves canonical labels |
| 4 | Pure helper/render derivation + existing hoisted mock | Focused Vitest collision fixture |
| 5 | Only render-local filtering; no hook/server edits | Diff-scope review and build |
| 6 | Existing test/build toolchain | Focused Vitest + `npm run build --prefix ...` |

## Build order

1. Add the helper and wire the derived labels into the existing row.
2. Extend the existing test with bound and fallback cases.
3. Run focused Vitest.
4. Run the production Vite build.
5. Inspect the final diff to confirm only UI presentation/tests plus SDD
   artifacts changed.

## Test strategy

- Focused unit/render gate from `crates/agentum-desktop/ui`:

  ```sh
  npx vitest run src/components/sidebar/WorktreeCardMeta.test.tsx
  ```

- Production build gate from repository root:

  ```sh
  npm run build --prefix crates/agentum-desktop/ui
  ```

- Browser QA remains the spec's `qa.sh` leg: bound issue with a canonical stale
  label shows one Project lifecycle chip; an unbound issue retains its label.

## Architecture gate

PASS. Every acceptance criterion has one implementation seam and one observable
verification method; all product choices are resolved; reuse is maximal; no
launch, streaming, MCP, harness, tracker, or cache invariant is touched.
