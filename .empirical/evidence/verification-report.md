# Verification report

Captured at `2026-07-31T21:51:51Z` for verify revision 5.

## Passing automated gates

- `cargo fmt --all -- --check`
- `cargo test --locked -p agentum-server sdd::sources::tests --lib` — 20 passed
- `cargo test --locked -p agentum-server source_request_is_a_closed_discriminated_union --lib` — 1 passed
- `cargo test --locked -p agentum-server empirical_ --lib` — 8 passed
- `cargo clippy --locked -p agentum-server --all-targets --all-features -- -D warnings`
- Repository Python contract suite, including `tests.test_empirical_golden` — 28 passed
- `scripts/check-sdd-boundary.sh --boundary-only`
- `bunx tsc --noEmit`
- Focused New Workspace and SDD UI suite — 27 passed
- `bunx vitest run` — 796 files and 6,472 tests passed
- `bun run build:check` — production build passed; 2.11 MB entry under the 2.30 MB budget
- `git diff --check`

The Empirical importer tests exercise the pinned canonical fixture, stable
revision and provenance rendering, all delta operations, optional design/plan
artifacts, omission of evidence/runtime material, malformed schema/state/delta
input, traversal, symlinks, parent replacement, active locks, unknown shapes,
invalid UTF-8, and size limits. Route tests exercise capabilities, read-only
preview, source drift conflict before allocation, durable import content, and
local-only fail-closed behavior. The full UI suite protects existing source,
approval, evidence, and delivery surfaces in addition to the new source.

## Browser gate

Chromium `149.0.7827.55` mounted the exported production `NewSpecDialog`,
selected Empirical, entered `.empirical/specs/add-report-export`, and activated
Preview source. Nine visible-state assertions passed with zero console errors.
The structured interaction record is
`.empirical/evidence/empirical-new-spec-preview.json`; the 1440x1200 screenshot
is `.empirical/evidence/empirical-new-spec-preview.png` with SHA-256
`3002346a01acbed975c879e59814bc3d48d8caa2c11310b7d4e3e3c3d3cf1d52`.

## Environment-only broad-gate observations

The broader server library run reached all repository tests but one existing
live installed-Codex help probe because this machine's Codex build does not
advertise the expected `--sandbox` flag. Full-workspace clippy cannot discover
the system `javascriptcoregtk-4.1` development package; the changed server
crate's all-target/all-feature clippy gate passes. Neither condition exercises
or is changed by Empirical intake.
