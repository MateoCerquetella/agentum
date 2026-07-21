# Handoff 03 — Developer → Tester

- **Spec:** 015-host-aware-start-and-tracker-intake
- **Date:** 2026-07-13
- **From:** Developer (three slice sub-agents, orchestrator-gated)
- **To:** Tester
- **Commits under test:** F1 `ff7290ee`, F2 `d7d64f33`, F3 `3ec6f028`
  (on `fixes-new-workspace`, based on `origin/develop` `4f98453f`)
- **Artifacts:** `tasks.md` (F1/F2/F3 sections, all deviations numbered),
  `architecture.md`, spec ACs 1–13 (note the in-place "architect grounding"
  amendments to AC 10/11 — grade against the amended text)

## Developer-claimed gate results (re-run ALL independently)

1. `cargo test -p agentum-server --lib` — 687/0/5 (8 new `routes::repos` tests).
2. `cargo fmt --all --check` + `cargo clippy -p agentum-server --lib --tests -- -D warnings` — clean.
3. `npm run build --prefix crates/agentum-desktop/ui` — green (pre-existing
   chunk-size warning only).
4. Targeted vitest (9 files: `find-repo-by-path`, `github`, `github-checks`,
   `hosted-review*`, `start-work-repo-match`, `project-dialog-state`,
   `create-issue-intent-model`) — 157/0. The 12 pre-existing
   `create-issue-intent-model` cases must pass UNMODIFIED (add-only contract).

## Sacred surfaces (assert empty diffs base→HEAD)

- `ProjectBindingEditor.tsx` internals (F3 only wires its existing `onBound`).
- `hooks/useComposerState.ts` (modal-data/props consumers only; NO edits).
- `github-item-checks-tab.tsx`, `pull-request-checks-tab.tsx` (architect ruling:
  repo predetermined by surface).
- `lib/launch-work-item-direct.ts` create path.
- `routes/worktrees.rs` `CreateBody` (D2: no host field), the
  `unwrap_or(LOCAL_HOST_ID)` resolver default.
- F2+F3 combined touch ZERO Rust (`git diff ff7290ee..HEAD -- crates/agentum-server` empty).
- The wizard's own create-issue panel (013's surface).

## Behavior pins to verify by reading test bodies + code

- F1: dedupe key (path, connection_id) None==None; same path × same connection
  idempotent; `apply_repo_updates` refuses `connectionId` (incl. explicit null)
  while `hostId`/`displayName` still apply; `scope_pairs_locals_first` stable
  partition; `findRepoByPathPreferLocal` prefers the local entry.
- F2: classifier none/direct/choose; sole-remote → direct (VPS-only starts on
  VPS, AC 6); local-first seed; direct path byte-equivalent for single match;
  choose-hop payload has NO `startGatedRun` / NO `initialBaseBranch`.
- F3: `filed` only from provider-confirmed responses; gated-run gate composes
  `deriveIssueSideEffectGate` (Linear → not-github-url, remote → remote-repo);
  gated run = pre-armed composer hop, never direct `startGatedWork`; errors
  inline/non-fatal.

## Deferred (NOT the tester's job — record as deferred, don't fail on them)

- AC 3/4/5/7/11 live legs (real VPS host, real GitHub/Linear filing, browser
  QA) = qa.sh/staging/human, 008/010 precedent.
- The `bunx vitest run` FULL suite (~139 fails) and bare tsc (~1650 errors) are
  pre-broken develop baselines — corroborate no NEW failures in touched areas
  only.

## Expected tester artifacts

`verification.md` (verdict + evidence per AC + deviation-accuracy audit) and
`handoffs/04-tester-to-reviewer.md`, then STATE phase → reviewer (orchestrator
applies the transition on gate pass).
