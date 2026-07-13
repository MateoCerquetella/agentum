# Handoff 01 — PM → Architect (spec 015-workspace-harness-autostart)

- **Date:** 2026-07-13
- **From:** PM (sdd-orchestrate, autonomous — worktree `i-want-to-…-twe-h`, issue #301)
- **To:** Architect
- **Verdict:** PM gate **PASS** (all 9 `validate_handoff.md` boxes). Spec
  amended in place after a full fact-check against the ff-merged tree
  (`40030d8b`): 3 citation fixes, 1 surface re-decision (D1), 6 locked
  decisions, 2 delegated questions.

## What the Architect receives

`ai/specs/015-workspace-harness-autostart/spec.md` — one slice, three ordered
gateable features:

1. **f1-detect-helper** — pure `lib/workspace-harness-detect.ts`
   (`{found, harnessDir, offer}` from fs entries + registered workdirs +
   creation context) + vitest.
2. **f2-banner** — `HarnessSpecBanner`, workspace-view mount for the
   just-created worktree (AC 1, 2, 6).
3. **f3-register-run** — export `startHarness`/`runHarness`/`listHarnesses`;
   accept → `POST /api/harness` + `POST /api/harness/{id}/run`, dedupe +
   error toast (AC 3, 4, 5).

## Overlap verdict (checked, not assumed)

**No duplicate.** The two adjacent mechanisms on develop are (a)
scaffold-a-new-harness on create (spec 010 F3, `workspace-provision-step.ts`
— *writes* `.agentum-harness/`, excludes `feature_list.json`) and (b) the
issue-first gated run (`start_work` via the wizard toggle,
`useComposerState.ts:2293/2711` → `harness-client.ts:171`) — neither reads a
pre-existing `feature_list.json`. Specs 010/011/013 promise neither. 015 is
the missing complementary half.

## PM-locked decisions (D1–D6 in spec.md; do NOT re-open)

D1 workspace-view mount (auto-launch reality: `CreateWorkspaceWizard.tsx:344`
auto-launches; launcher only mounts for `agent === null`/gated runs) ·
D2 creation-moment trigger only · D3 hide-never-link · D4 canonical names
(`listHarnesses`, not the draft's `getHarnessStatuses`) · D5 local-only
(engine has no host plumbing: `routes/harness.rs` `StartRequest{workdir}`) ·
D6 gated-run suppression.

## Delegated to YOU (the only two)

- **Q1** — exact banner mount in the workspace view (Terminal.tsx strip vs a
  shared slot) that survives auto-launch WITHOUT touching `useComposerState`
  internals (008 invariant: props only).
- **Q2** — how creation context (worktreeId, workdir, hostId, gatedRun)
  reaches the banner: store slice vs module-level pending signal (precedent:
  `lib/pending-session-prompt.ts`).

## Verified line map (fact-checked 2026-07-13 on `cac37bab`)

- `fsListEntries` `server-fs-client.ts:52` (exported, host-aware) → handler
  `routes/fs.rs:180`, route `fs.rs:24`.
- `startHarness` `harness-client.ts:148`, `runHarness` `:286`,
  `listHarnesses` `:276` — all module-private today; `HarnessStatus.workdir`
  `:78` (server `harness.rs:696`). `subscribeHarnessRunErrors` `:378`
  (exported).
- `HARNESS_DIR`/`LEGACY_HARNESS_DIR` `harness/types.rs:16,19`; fallback
  `resolve_harness_dir` `:25-35`.
- `WorkspaceAgentLauncher` mount `Terminal.tsx:1578` (conditional `:1576`);
  its docstring's "no longer auto-launches" claim is OUTDATED — don't trust
  it.
- Post-create seam: `lib/open-created-workspace.ts` (`planCreatedWorkspaceOpen`;
  `gatedRun` suppresses plain deliveries `:40-46`, launcher fallback
  `agent === null` `:44-46,92-96`).

## Standing obligations

- **Invariants:** one launch path (UI never spawns agents); one-shot check,
  never poll; gate is sacred (accept = register + run only);
  `useComposerState` internals untouched; no new server routes.
- **Test rhythm:** UI uses bun — `bunx vitest run <files>` targeted; full
  vitest (~139 fails) + bare tsc are pre-broken baselines on develop; gates
  are targeted suites + `npm run build --prefix crates/agentum-desktop/ui`.
- **Deliverable:** `architecture.md` — Q1/Q2 seam choices with tradeoffs,
  module placement, per-feature test strategy (pure models, jsdom-free),
  build order f1→f3, and the AC→assertion map for verify.sh/qa.sh.

## Known accepted residuals

- Dismissal is session-memory only (re-offer on next *creation* of a
  workspace on the same dir is acceptable; D2 prevents mount-loop re-offers).
- A spec file appearing AFTER creation is not detected (creation-moment
  trigger only) — future slice if wanted.
