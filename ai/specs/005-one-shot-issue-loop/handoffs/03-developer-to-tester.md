# Handoff 03 — Developer → Tester

- **Spec:** 005-one-shot-issue-loop
- **Date:** 2026-07-02
- **From:** Developer (autonomous /sdd-loop, iterations 3–5, three gated slices)
- **To:** Tester
- **Artifacts:** commits `197a7bea` (F2+F3+F4), `ae8bf467` (F1), `3b0a00d0` (F5)
  on branch `finish-the-loop`; `tasks.md` carries the full per-feature
  checklist + every deviation.

## Gate result (developer phase)

All three slices green, orchestrator-verified per slice (fmt --check, file
surface vs blueprint, sacred-file diff checks):

- Slice 1 (F2+F3+F4): 507/0 lib tests, vite green.
- Slice 2 (F1): 512/0 lib tests, vite 1m03s, vitest open-created-workspace 10/0.
- Slice 3 (F5): **518/0 lib tests (5 ignored)**, `cargo check -p agentum-desktop`
  green, vite green.
- `drive_inner` control flow, the run route's spawn line, and both gate loops:
  zero structural diffs across all slices (checked per slice).

## What the tester must verify (ACs 1–10, spec.md)

Reproduce the gates first (`export PATH="$HOME/.cargo/bin:$PATH"`;
`cargo test -p agentum-server --lib`; `npm run build --prefix
crates/agentum-desktop/ui`; `npx vitest run src/lib/open-created-workspace.test.ts`
from the ui dir), then verify each AC against code + tests, exactness over
vibes:

- **AC 1–5 (F1):** the 8-step sequence order in `start_work`
  (routes/harness.rs) — already-running check BEFORE any fs write, under
  `start_work_lock`; converge-on-existing (no 400 via start-work; the plain
  route still 400s — test-pinned); Todo fired at plan in `ensure_spec_and_plan`
  (fake-gh test asserts exactly one `--add-label status/todo` edit); knob write
  after plan (no spec_id re-stamp); C2 — NO new InProgress call.
- **AC 2 specifically:** all three plain-delivery skips
  (`planCreatedWorkspaceOpen` — vitest) + composer forces issue automation off
  and skips the D5 scaffold when armed.
- **AC 6 (F2):** byte-identical pins (`feature_prompt_without_spec_is_byte_identical`,
  explicit-override) + spec_id stamped by `plan_from_spec_inner` (both plan
  tests extended).
- **AC 7–8 (F3):** QA prompt steers `agentum_browser`, verdict contract
  character-identical; `resolve_qa_mode_matrix` all 12 cells; setting defaults
  OFF (`harness_qa_setting_defaults_off_and_round_trips`).
- **AC 9 (F4):** `report_status_text_never_errs_on_tracker_failure` (the pin),
  arg matrix incl. github-id-from-url, catalog presence, ungated by
  orchestration, board-card wire test.
- **AC 10 (F5):** default-map argv byte-identical pin; precedence via pure
  `apply_layers` (no env mutation); name-filtered remove-set (collision +
  no-canonical-in-argv tests); fake-gh custom-map flip; phase-keyed colors.

## Env & test hygiene (will bite you if ignored)

- Only `ensure_spec_and_plan_fires_todo_at_plan` mutates env — under
  `crate::TEST_ENV_LOCK` (it pins BOTH `AGENTUM_GH_BIN` and
  `AGENTUM_GITHUB_CONFIG`). Don't add env mutation elsewhere.
- Fresh-worktree: `bun install` in `crates/agentum-desktop/ui` before vite;
  `NODE_OPTIONS=--max-old-space-size=3072` if vite OOMs.
- `cargo check -p agentum-desktop` needed sherpa/onnx dylibs copied into
  `target/release/` (build-env only, not committed) — if you re-run it and it
  fails on resource validation, that's the cause, not the code.
- vitest suites importing xterm fail to load — pre-existing noise; the
  open-created-workspace file loads fine.

## Explicitly NOT verified (out of tester scope, goes to qa.sh/staging)

- GUI: composer toggle rendering, Tasks-row dropdown flow, Settings
  GitHub-labels card, toasts — vite-build + unit-pinned only, no installed-app
  run.
- Live end-to-end: real issue → Start gated run → real label flips on GitHub.

## Known issues logged for later (do NOT fix in this spec)

- Pre-existing: IntegrationsPane's LINEAR editor sends snake_case invoke keys
  (`in_progress`, `ready_to_test`) that never bind (camelCase-only, no
  fallback) — every Linear save silently clears those two overrides. File an
  issue at review/ship time.
- Stale `FeatureList.qa_mode`/`qa_agent_tool` doc comments still mention
  AGENTUM_BROWSER_VERIFY/browser-verification-loop (flagged in tasks.md,
  slice 1 deviation 4).

## Expected tester artifact

`ai/specs/005-one-shot-issue-loop/verification.md` — per-AC PASS/FAIL verdict
with the exact evidence (test name / code cite / reproduced command output),
independently re-run suite numbers, and Info findings weighted like spec 004's
verification.md.
