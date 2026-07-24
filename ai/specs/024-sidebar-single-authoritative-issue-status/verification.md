# Spec 024 — Verification (Tester)

- **Date:** 2026-07-21
- **Base under test:** `8cb2a502` plus the uncommitted Spec 024 implementation
  and SDD artifacts
- **Tester:** independent (did not implement the change)
- **Verdict:** **PASS-WITH-QA-DEFERRAL** — 0 reproducible defects

The implementation passes every local gate required by AC 6. The only deferral
is the spec's separate `qa.sh` browser leg: this tester session has no
Playwright MCP connection and no relevant running desktop/browser fixture, so
no live UI interaction or screenshot was run and no browser pass is claimed.
No tracker data was mutated.

## Gates independently run

| Gate | Result |
| --- | --- |
| `npx vitest run src/components/sidebar/WorktreeCardMeta.test.tsx` from `crates/agentum-desktop/ui` | **PASS** — 1 file, 7 tests passed, 0 failed; duration 3.33s. |
| `npm run build --prefix crates/agentum-desktop/ui` from repository root | **PASS** — 7,222 modules transformed; production Vite build completed in 1m 29s. Existing dynamic-import and large-chunk warnings were non-fatal. |
| `git diff --check` | **PASS** — no whitespace errors. |
| Browser `qa.sh` behavior | **DEFERRED / NOT RUN** — Playwright MCP tools and a relevant live Agentum desktop/browser fixture were unavailable. No screenshot evidence exists, therefore no browser result is marked passed. |

## Acceptance-criteria evidence

| AC | Verdict | Independent evidence |
| --- | --- | --- |
| 1 | **PASS** | The bound collision render sets Project status to `In progress`, asserts exactly one `data-project-status` element and one rendered `In progress`, and asserts that `status/blocked` and `status/in-progress` are absent. The render path maps `displayedIssueLabels`, not the fetched array. |
| 2 | **PASS** | The same bound render preserves `status/qa` and `area/desktop`. Diff inspection confirms the classifier is an exact six-member set rather than a `status/` prefix rule; therefore `status/qa-pass`, `status/qa-fail`, and arbitrary labels are not filtered. Matching is case-sensitive. |
| 3 | **PASS** | The null-status render asserts both canonical labels, the QA label, and the ordinary label remain visible. `visibleIssueLabels` also returns the original array for `null`, `undefined`, empty, or whitespace-only Project status, covering missing URL/loading/unbound/error-shaped absence without mutating labels. |
| 4 | **PASS** | `visibleIssueLabels` is an exported pure helper. Focused static-render coverage exercises both conditional branches and the exact AC 4 collision fixture; the focused suite is independently green at 7/7. |
| 5 | **PASS** | The implementation diff changes only `WorktreeCardMeta.tsx` and its focused test. The source change adds a constant set, a pure filter, and render-local derivation. No GitHub, Project, tracker event, cache, hook, server, or worktree metadata path is changed. `Array.filter` creates a derived array only in the bound case. |
| 6 | **PASS** | Focused Vitest, production UI build, and `git diff --check` all exit 0 in this tester run. |

## Negative, error, and race audit

- **Unbound/loading/error/missing URL:** all present as an absent
  `projectStatus.status`; the helper preserves every fetched label. The null
  render regression pins the observable fallback.
- **Blank status:** the helper uses `projectStatus?.trim()` only as the presence
  check, so empty and whitespace-only values cannot accidentally suppress the
  fallback.
- **Exact-name boundary:** only the six names mirrored from
  `task_sink.rs` (`todo`, `in-progress`, `in-review`, `ready-to-test`, `done`,
  and `blocked`) are members. QA labels, prefix-sharing user labels, and
  case-distinct labels survive.
- **Resolution/invalidation race:** the displayed labels are derived
  synchronously during the same render from the same `projectStatus.status`
  passed to `IssueProjectStatusChip`. A null-to-status transition therefore
  paints the chip and removes canonical fallback labels atomically at this
  component seam; there is no added effect, state, subscription, or request to
  lag behind. Existing stale-while-revalidate behavior is untouched.
- **Warnings:** `projectStatus.warning` remains on its independent render path
  and is covered by the existing focused warning test.

## Diff-scope audit

The production diff is limited to:

- `crates/agentum-desktop/ui/src/components/sidebar/WorktreeCardMeta.tsx`
- `crates/agentum-desktop/ui/src/components/sidebar/WorktreeCardMeta.test.tsx`

Other working-tree changes are SDD state/spec artifacts. This tester added only
`verification.md` and the Tester-to-Reviewer handoff and did not update
`ai/STATE.md`, tracker/spec status, implementation, commits, or remotes.

## Reviewer focus

Confirm the exact six-name UI mirror remains intentionally synchronized with
the server defaults, and confirm suppression remains conditional on the
non-blank authoritative Project status. Live bound/unbound screenshot proof
remains the later `qa.sh`/runtime leg.
