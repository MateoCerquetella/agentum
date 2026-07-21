# Handoff 04 — Tester → Reviewer

- **Spec:** 015-host-aware-start-and-tracker-intake
- **Date:** 2026-07-13
- **From:** Tester (independent verification)
- **To:** Reviewer
- **Commits:** F1 `ff7290ee`, F2 `d7d64f33`, F3 `3ec6f028` (base `4f98453f`)

## Verdict: PASS-WITH-DEFERRALS — zero defects

All gates reproduced independently and match the developer's claims exactly:

- `cargo test -p agentum-server --lib`: **687 / 0 / 5** (routes::repos module: 10/0)
- `cargo fmt --all --check` clean; `cargo clippy -p agentum-server --lib --tests -- -D warnings`
  clean (tester **forced** a recompile — the first run was cache-only)
- `npm run build` (vite): green in 38.6s, pre-existing chunk-size warning only
- targeted `bunx vitest run` (9 files): **157 / 0**

Sacred surfaces all proved empty by git diff at their **real** paths
(useComposerState.ts lives at `ui/src/hooks/`, not the handoff's implied
location — verified there): checks-tabs ×2, launch-work-item-direct.ts,
ProjectBindingEditor.tsx (its `onBound` prop pre-exists at base; F3 only wires
it at the call site), worktrees.rs, the 013 wizard panel.
`git diff ff7290ee..HEAD -- crates/agentum-server` is empty — F2+F3 are zero Rust.

ACs 1, 2, 6, 8, 9, 10, 12, 13 PASS on code/test evidence; ACs 3, 4, 5, 7, 11
PASS with their live legs deferred (VPS host, real filing, browser QA —
qa.sh/staging per the 008/010 precedent). Full evidence table in
`../verification.md`.

Deviation audit: all 7 numbered deviations **ACCURATE** against the code. One
reporting miscount found: "8 new routes::repos tests" is actually **7 new test
functions** (module went 3 → 10) — the folded hostId assertion was counted as
an eighth. Substance intact; not a defect.

## What the reviewer should focus on

1. **The `onUse` behavior shift (F2 deviation 1)** — the item dialog's "Use"
   now re-classifies against the slug index, so a zero-match shows the
   missing-repo dialog instead of falling into `launchWorkItemDirect`'s `!repo`
   URL-open. I verified it's practically unreachable and arguably more honest,
   but it is the one *product-visible* behavior change outside the spec's
   letter — a reviewer judgment call on whether it needs a release note.
2. **`handleOpenDialog`'s choose arm** stamps the *seed* repo onto the
   repo-backed dialog (mutations are slug-addressed so any same-slug candidate
   is safe per the in-code comment). Sanity-check you agree that dialog
   mutations really are slug-addressed for every mutation the dialog exposes.
3. **F3 render policy superset** — the intake panel renders whenever the tab
   has a workdir (architecture §4.1), not only when a binding resolves (spec
   AC 9's narrower letter). Architect-sanctioned; confirm you accept the
   superset.
4. **Two residual path-fallback sites** (`GitHubItemDialog.tsx:365`,
   `PullRequestPage.tsx:344`) — benign today (the fallback arm's only consumer
   null-guards on the same condition), but they're the same shape the F1 audit
   swept elsewhere. Candidate for the doctor-check follow-up ticket.

## Open risks

- **Deferred live legs** (VPS end-to-end, real GitHub/Linear filing, board
  refresh visibility, gated run from the panel) are unexercised — they ride
  qa.sh/staging. The mechanisms are code-verified but nothing has driven a real
  browser or a real VPS in this verification.
- **Vite does not typecheck** and bare tsc is pre-broken; I hand-verified every
  cross-module signature the new F3 code consumes (result shapes, modal-data
  contract, settings pick) — all sound — but there is no automated type gate on
  the new files beyond vite's esbuild transform.
- **F1-without-F2 shipping hazard** (false "isn't added to Agentum" dialog) is
  moot on this branch since both landed, but promotion must keep them in the
  same release train (architecture §5).
