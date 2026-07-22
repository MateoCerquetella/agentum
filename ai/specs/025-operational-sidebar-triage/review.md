# Review — Spec 025: operational sidebar triage

- **Status:** SEND-BACK TO DEVELOPER
- **Date:** 2026-07-22
- **Reviewer:** autonomous SDD Reviewer

## Verdict

`passed: false`

The implementation is close and the Tester evidence is accepted, but source review found two
contained presentation defects that prevent final sign-off. Neither requires an architecture or
PM change.

## What worked well

- The pure operational model is small, deterministic, and keeps status precedence, search,
  ordering, counts, and disclosure outside React.
- The implementation reuses the existing status resolver, virtual row path, and `WorktreeCard`
  interaction owner. No backend, polling, persistence-schema, or duplicated interaction path was
  introduced.
- The two Tester send-backs were fixed at the shallowest layer with focused regressions. The final
  Tester record is internally honest: 21/21 focused tests and the production build pass, while
  browser screenshots and interaction QA are explicitly deferred rather than claimed.
- `tasks.md` and the eight-role handoff trail accurately record completed work, rework, and the
  remaining staging evidence.

## Areas for improvement

### 1. Preserve the three operational sections when filters match no workspaces

`WorktreeList.tsx:5187-5195` takes the legacy `filtersHideAllRows` empty-state return whenever an
active project/default-branch filter leaves `worktrees` empty. In operational mode the model has
already emitted the three required zero-count headers, but this return replaces them with “No
workspaces found.” That contradicts AC 1 and architecture D3, which require the operational queue
to always render exactly Needs You, Active, and Settled with truthful full counts.

**Required fix:** keep the legacy recovery empty state for alternate groupings, but allow the
operational rows to render when their filtered set is empty. Add a focused regression for a
selected project (or other active filter) with zero matching workspaces that asserts the three
ordered `0` headers remain visible.

### 2. Make `operational-rich` the authoritative card presentation

`WorktreeCard.tsx:139-141` lets `experimentalCompactWorktreeCards` make an
`operational-rich` row compact. In the usual no-badge/no-cache case, `hasMetaRow` then becomes
false (`:562-580`), so the required visible branch line is omitted. Separately,
`WorktreeCard.tsx:947-965` still renders the configured inline-agent list and then the new
operational agent summary, duplicating agent presentation and violating architecture D4’s
explicit suppression rule.

**Required fix:** when `operationalMeta` exists, derive density from its presentation (`rich` or
`settled`) rather than the legacy experimental compact preference, and suppress
`WorktreeCardAgents` for operational rows. Add focused render coverage proving a rich operational
card still visibly contains branch and only one agent summary when legacy compact and
inline-agent preferences are enabled.

## Risks

- Browser-only QA remains residual release/staging evidence: 220/500 px light/dark screenshots,
  keyboard traversal, focus/contrast checks, and runtime drag/context behavior have not been
  observed in a Playwright-enabled surface. This is documented debt, not fabricated pass evidence,
  and is not the reason for this send-back.
- No other maintainability, dead-code, scope, or architecture blocker was found in the four
  feature commits.

## Recommendation

Route to Developer for the two localized fixes above, rerun the existing focused suite plus the
new regressions, and return directly to Tester for a narrow retest. Reviewer can sign off after
that evidence is green; no PM or Architect loop is warranted.

Reviewer gate: **FAIL — targeted Developer send-back**.
