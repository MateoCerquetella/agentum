# Tasks — Spec 024

## F1 — Sidebar single authoritative issue status

- **Acceptance criteria:** AC 1-6
- **Files:**
  - `crates/agentum-desktop/ui/src/components/sidebar/WorktreeCardMeta.tsx`
  - `crates/agentum-desktop/ui/src/components/sidebar/WorktreeCardMeta.test.tsx`

### Implementation

1. [x] Add the exact six-name Agentum tracker-label set and pure display-label
   helper in `WorktreeCardMeta.tsx`.
2. [x] Filter canonical labels only when `projectStatus.status` is non-empty.
3. [x] Render the derived labels and use their length in the badge-row predicate.
4. [x] Add bound-project collision coverage preserving `status/qa` and
   `area/desktop`.
5. [x] Add null-Project-status fallback coverage preserving canonical labels.

### Green gate

- `npx vitest run src/components/sidebar/WorktreeCardMeta.test.tsx` from the UI
  directory.
- `npm run build --prefix crates/agentum-desktop/ui` from the repository root.
- `git diff --check`.

### Done evidence

- F1 implementation complete in the two architect-approved UI files.
- Focused Vitest — PASS: 1 test file, 7 tests passed (7), duration 6.80s.
- Production UI build — PASS: 7,222 modules transformed; Vite built in 6m 2s.
- `git diff --check` — PASS.
- AC mapping and exact gate evidence recorded in
  `handoffs/03-developer-to-tester.md`.
