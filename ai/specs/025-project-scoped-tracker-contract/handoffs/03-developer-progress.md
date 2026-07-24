# Handoff — Developer progress

- **Spec:** 025-project-scoped-tracker-contract
- **From:** Developer
- **To:** Developer continuation
- **Date:** 2026-07-21
- **Gate:** OPEN — remain in Developer

## Implemented in F1

- Added a versioned provider-aware `ProjectTrackerConfig` domain contract.
- Added SQLite migration `0027_project_tracker_configs.sql` and transactional
  CAS store methods keyed by `Repo.id`, including deletion and exact GitHub
  target lookup.
- Added canonical repo-scoped GET/PUT/PATCH-preferences/DELETE routes with
  provider-target validation and structured `409` current-record responses.
- Added deterministic migration inputs from only the requested repo's legacy
  `trackerProvider` and exact host-resolved GitHub slug/binding. Global desktop
  selections are not read.
- Made repoId-aware legacy GitHub binding GET/PUT/DELETE calls project through
  the canonical row; repoId-less callers retain the legacy compatibility path.
- Added canonical tracker cleanup before registry deletion.

## Changed files

- `crates/agentum-core/src/lib.rs`
- `crates/agentum-store/migrations/0027_project_tracker_configs.sql`
- `crates/agentum-store/src/lib.rs`
- `crates/agentum-store/src/project_trackers.rs`
- `crates/agentum-server/src/lib.rs`
- `crates/agentum-server/src/routes/mod.rs`
- `crates/agentum-server/src/routes/repos.rs`
- `crates/agentum-server/src/routes/github_projects.rs`
- `crates/agentum-server/src/routes/project_trackers.rs`

All other pre-existing dirty/untracked files were preserved.

## Exact verification

- PASS: `cargo test -p agentum-store project_tracker --lib` — 1 passed, 0
  failed, 47 filtered out.
- PASS: `git diff --check` — no output.
- INCOMPLETE: `cargo test -p agentum-server project_trackers --lib` reached
  `Compiling agentum-server` and emitted only the existing duplicate attribute
  warning at `github_projects.rs:1064`, but was interrupted before producing a
  test result. It must be rerun; no pass is claimed.
- NOT RUN: Vite build, UI tests, full relevant Rust library gates, real desktop
  QA.

## Remaining work

1. Finish F1 route tests for GET migration idempotence, PUT/PATCH/DELETE CAS,
   two-repo isolation, local/SSH slug routing, unknown-field preservation, and
   ambiguous compatibility writes; rerun focused server and store gates.
2. Implement F2 shared repo/host/generation-guarded UI owner and both editor
   entry points.
3. Implement F3 canonical target consumption and repo-scoped preferences.
4. Implement F4 transition context/fail-closed resolution, immutable workspace
   coordinates, sole-binding fallback removal, and full deletion cache cleanup.
5. Run formatting, focused/full Rust and Vitest gates, Vite production build,
   and `git diff --check`; only then advance to Tester.

## Acceptance-criteria status

- AC 1, 2, 7, 8: F1 infrastructure is present but not fully route-tested, so
  these criteria are not yet passed.
- AC 3–6, 9–10: pending F2–F4; no criterion is deferred or claimed complete.
