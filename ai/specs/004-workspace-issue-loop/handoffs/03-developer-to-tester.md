# Handoff 03 — Developer → Tester

- **Spec:** 004-workspace-issue-loop
- **Date:** 2026-07-01
- **From:** Developer (autonomous /sdd-loop iterations 3–4, two gated slices)
- **To:** Tester
- **Commits:** slice 1 `85c48e0d` (F1+F2); slice 2 uncommitted at handoff-write
  time — committed by the orchestrator with this note.

## Gate result at handoff

- `cargo test -p agentum-server --lib`: **494 passed / 0 failed / 5 ignored**
  (ignored = pre-existing live-gh/live-agent tests).
- `cargo fmt --all -- --check` + `cargo check -p agentum-server`: clean.
- `NODE_OPTIONS=--max-old-space-size=6144 npm run build --prefix
  crates/agentum-desktop/ui`: **green** (`✓ built in 1m 4s`; chunk-size
  warnings pre-existing).

## What was built (per AC)

- **AC 1 (F3):** `POST /api/github/issues` (routes/github.rs) over
  `TaskSink::Github::create_feature`; composer "Create GitHub issue" affordance
  (NewWorkspaceComposerCard.tsx + useComposerState.ts +
  runtime/github-issue-client.ts) — renders only when nothing is linked, on a
  local git repo; success → linkedWorkItem chip BEFORE worktree creation.
- **AC 2 (F2):** CreateBody widened (`linkedPR` alias), registry persistence,
  detected-scan emits `linkedPR`, `canonical_meta_key`, two TS client layers
  forward the fields. NO registry-struct alias (wipe hazard).
- **AC 3–5 (F1):** real GitHub arm in `apply_tracker_transition(…,
  tracker_url, …)` — 4 canonical labels ensure-created (`--force`, fixed
  colors), one `issue edit` sets target + removes the other three canonical
  only; Done = label-only; every failure `Ok(Skipped(reason))`, 30s gh timeout;
  drive.rs + board_goals.rs each one logical line.
- **AC 6 (F4):** `fetch_github_issue` (shared with GET), pure
  `spec_md_from_issue` (control-strip, 64 KiB cap, fallback AC via the real
  `derive_backlog_from_spec`), traversal-proof `issue_spec_id`,
  `POST /api/harness/spec-from-issue` (never-overwrite); composer
  `scaffoldSpec` toggle (default OFF, github.com issue + local only), shared
  `maybeScaffoldSpecFromIssue` in BOTH submit paths, non-fatal on failure.
- **AC 7:** `plan_from_spec_with_tracker` stamps provider+url on every derived
  feature; `plan_from_spec` delegates unchanged (MCP tool behavior pinned).

## Tester instructions

Verify per acceptance criterion in ai/specs/004-workspace-issue-loop/spec.md.
Evidence expected per AC: the pinning unit test(s) by name + a code-path walk;
where runtime verification is possible WITHOUT a GUI, do it (e.g. run the
argv-builder tests, inspect the fake-gh transcript assertions, POST to the new
routes on a scratch server only if practical). Browser QA (the composer chip,
the toggle, a live label flip on a real issue) is the qa.sh/staging gate per
repo flow — NOT this phase; mark those ACs "unit-verified, runtime pending QA"
rather than failing them.

Deviations already accepted by the orchestrator (do not re-flag): composer
markup in Card not Modal (modal delegates); `FetchedIssue.slug` documented
allow(dead_code); F4 tests in harness.rs::surface_tests (repo convention);
typed conditional instead of conditional spread; blank-title test pins the pure
gate not an HTTP round-trip.

## Watch-fors (from the blueprint's reviewer-focus list)

1. drive.rs diff = one logical change only.
2. GitHub arm: `Ok(Skipped)`-never-`Err` on every path.
3. No `is_public` additions.
4. `linkedPR` on the detected read path; alias only on CreateBody.
5. `- [ ]` lines bare in generated spec.md (derive round-trip).
