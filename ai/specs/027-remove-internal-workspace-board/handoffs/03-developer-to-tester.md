# Handoff — Developer to Tester

- **Spec:** 027-remove-internal-workspace-board
- **From:** Developer
- **To:** Tester
- **Date:** 2026-07-23
- **Gate:** PASS

## Delivered

- Reconciled the already-landed implementation at commit `84c5a1c7` against all
  five architecture slices on the current `02255b08` worktree.
- Corrected `.agentum-harness/verify.sh` so F1 no longer invokes the deleted
  `SidebarHeader.test.tsx`; the surviving Tasks regression now runs cache-disabled.
- Marked F1–F5 complete in `tasks.md` with exact focused and full-gate evidence.
- No product-code correction was required: later commits preserve the removal and
  compatibility boundaries established by the landed implementation.

## Implementation files and boundaries verified

- Desktop: `TaskPage.tsx`, `ProjectTasksPage.tsx`, and
  `ProjectTasksPage.test.tsx`; `runtime/board-client.ts` remains deleted.
- Server boundary/helpers: `lib.rs`, `routes/mod.rs`, `routes/util.rs`, `routes/chat.rs`,
  `routes/github.rs`, and `routes/repos.rs`; all five internal board route modules and
  `rules.rs` remain deleted, as do the three retired board integration suites.
- External tracker lifecycle: `task_sink.rs`, `harness/drive.rs`,
  `harness/orchestrated.rs`, `routes/harness.rs`, `routes/mcp.rs`,
  `tracker_sync.rs`, and `tracker_attention.rs`. Creation is a closed GitHub/Linear
  enum, transition seams have no `Store` parameter, and legacy `board` metadata is a
  bounded non-writing skip.
- Persistence/runtime compatibility: `agentum-store/src/lib.rs`, `sessions.rs`,
  `agentum-core/src/lib.rs`, `agentum-watchdog/src/lib.rs`, `docs/API.md`, and
  `docs/DATA-MODEL.md`. Board CRUD/binding/core/reconciler modules remain deleted,
  migrations and `Session.card_id` remain compatibility-only, and ordinary workers
  do not reconcile legacy board rows.

## Commands and results

- `env HARNESS_FEATURE_ID=F1 bash .agentum-harness/verify.sh` — PASS; 1 file,
  4 tests.
- `env HARNESS_FEATURE_ID=F2 CARGO=/Users/mateocerquetella/.cargo/bin/cargo bash .agentum-harness/verify.sh`
  — PASS; route 404 matrix 1/1.
- `env HARNESS_FEATURE_ID=F3 CARGO=/Users/mateocerquetella/.cargo/bin/cargo bash .agentum-harness/verify.sh`
  — PASS; two focused sink tests, 1/1 each.
- `/Users/mateocerquetella/.cargo/bin/cargo test -p agentum-server --lib pinned_provider_dispatches_to_matching_tracker_arm`
  — PASS; 1/1.
- `/Users/mateocerquetella/.cargo/bin/cargo test -p agentum-server --lib resolve_tracker_pin_maps_d4`
  — PASS; 1/1.
- `/Users/mateocerquetella/.cargo/bin/cargo test -p agentum-server --lib report_status`
  — PASS; 7/7.
- `env HARNESS_FEATURE_ID=F4 CARGO=/Users/mateocerquetella/.cargo/bin/cargo bash .agentum-harness/verify.sh`
  — PASS; legacy-row regression 1/1 and watchdog 11/11.
- `/Users/mateocerquetella/.cargo/bin/cargo test -p agentum-server --lib current_docs_and_sdd_playbooks_keep_internal_board_compatibility_only`
  — PASS; 1/1.
- `/Users/mateocerquetella/.cargo/bin/cargo fmt --all -- --check` — PASS.
- `/Users/mateocerquetella/.cargo/bin/cargo test --workspace --lib` — PASS;
  901 passed, 6 ignored, 0 failed.
- `npm run build --prefix crates/agentum-desktop/ui` — PASS; 7,253 modules,
  1m29s.
- `git diff --check` — PASS.

## Acceptance-criteria coverage

- **AC 1:** focused structural Tasks test proves only external provider views or the
  settings-linked empty state remain and the global sync affordance/client is absent;
  production build passes.
- **AC 2:** the real router returns 404 for every retired family root and representative
  nested paths.
- **AC 3:** creation sinks are GitHub/Linear only; provider dispatch, MCP behavior, and
  store-free transition signatures pass focused and workspace tests.
- **AC 4:** legacy rows survive reopen byte-for-byte across ordinary session work;
  board runtime APIs/workers remain absent and legacy provider input is non-writing.
- **AC 5:** all focused gates, formatting, the required workspace library suite, the UI
  production build, and the whitespace check pass.

## Known risks and Tester focus

- Browser/runtime QA was not run in the Developer phase. Exercise bound GitHub, bound
  Linear, unbound, and unavailable project Tasks states and confirm there is no internal
  board sync action.
- Live external tracker writes were not attempted; focused fake-`gh`, isolated Linear,
  and best-effort seam tests cover dispatch without mutating a real tracker.
- The build retains pre-existing non-fatal warnings: `forge_send` is dead code, and Vite
  reports static/dynamic import overlap plus oversized chunks. None is introduced by
  this spec.
- Release promotion and tracker actions remain human-gated and were not performed.
