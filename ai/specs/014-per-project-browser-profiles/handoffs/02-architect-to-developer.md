# Handoff 02 — Architect → Developer

- **Spec:** 014-per-project-browser-profiles
- **Date:** 2026-07-09
- **From:** Architect (autonomous /sdd-loop iteration 2)
- **To:** Developer
- **Artifact:** `ai/specs/014-per-project-browser-profiles/architecture.md`

## Gate result

Architect gate: **PASS** (6/6). Boundaries defined (`BrowserScope`
Shared/Project/Adhoc; token-keyed registry); every decision A–H has a chosen
design + rejected alternative grounded in file:line; risks + sacred list cover
all spec invariants; the one researchable unknown (native data-store removal)
resolved against docs.rs (wry 0.55.1 / tauri 2.11.2: no remove-by-identifier →
`Webview::clear_all_browsing_data()` via a live webview + AC 5 observable
degradation); build order = 10 steps, each with gate commands, mapped to the 3
harness features; all 7 ACs covered (AC 1→step 3, AC 2→step 4, AC 3→step 6,
AC 4→step 7, AC 5→steps 8–10, AC 6→sweep/reap design, AC 7→named tests).

## Design in one paragraph

Resolve every caller-supplied browser context to a `BrowserScope`
(`Shared` / `Project{repo_id}` / `Adhoc{key}`); registry re-keyed by the
profile token (`project-<repoId>` — prefix applied AFTER sanitization);
teardown split into a scope-aware *release* (project browsers stop only at
attach-refcount 0 — ground truth = live screencast WS lifecycle, guard moved
into `run()`'s future — and NEVER delete the dir), while Adhoc keeps today's
kill+delete; shared profile relocates to `cdp-browser/shared/` so its existing
delete-on-stop is structurally safe; every-boot idempotent sweep (after the
reap) deletes top-level non-`project-*`/non-`shared` entries; native WKWebView
store id re-keys via `project_store_token`; clear action =
`POST /api/cdp-browser/clear-project-data` + `browser_clear_project_data`
Tauri command (flat args) + screencast-toolbar menu item.

## Build contract

- Follow **architecture.md §4 build order exactly** (steps 1–10, smallest
  first; step 4 is the riskiest and lands AC 2 — do it only after 1–3 pin the
  scope/token layer). Each step independently green before the next.
- Gates: **R** = `cargo test -p agentum-server --lib` (+ desktop:
  `cargo test -p agentum-desktop --lib && cargo build -p agentum-desktop`);
  **U** = `npm run build --prefix crates/agentum-desktop/ui` + vitest. Also
  run `cargo fmt --all` + clippy per repo conventions before finishing.
  NOTE (worktree gotcha, from project memory): fresh-worktree
  `cargo check -p agentum-desktop` may fail on `libsherpa-onnx-*.dylib` — copy
  the sherpa + onnxruntime dylibs from the main checkout's `target/release/`;
  use `$HOME/.cargo/bin/cargo` if bare `cargo` is missing; UI deps via
  `bun install` if node_modules is absent (then `npm run build` or
  `bun run build` both work).
- Write/update `tasks.md` in the spec folder as you go (per-step status +
  deviations, spec-008 style).
- **Top risks (architecture.md §5, do not regress):** std mutexes never held
  across `.await`; the attach guard must live inside `run()`'s future;
  sweep strictly AFTER reap; pkill test paths must sit under a temp
  `AGENTUM_HOME` (assert the containment, `routes/profiles.rs:154-177`
  pattern + `crate::TEST_ENV_LOCK`); worktree remove/prune callers must pass
  the full `<repoId>::<path>` id (bare paths stop resolving after the row is
  deregistered).
- **Sacred/untouched (architecture.md §6):** `pkill_by_signature` containment;
  hermetic CDP self-test (`cdp_driver.rs:1286-1310`); reap stays a
  process-killer; push-based streaming (no polling); server API-only; remote
  SSH browser path byte-identical; `canonical_worktree_key` + its `:804` test
  retained green; screencast remains the default browser surface.
- Deviations from the architecture are allowed only with a documented reason
  in tasks.md (and must not cross a sacred item).

## Open empirical checks for the developer

1. Step 6: verify which id plain-workspace / project-hub browser tabs
   actually carry at runtime (the design accepts both `<repoId>::<path>` and
   bare `repoId`; fix any surface passing `undefined` where a project exists).
2. Step 9: if `clear_all_browsing_data()` errors on macOS WKWebView at
   runtime, return it as the AC 5 warning — never swallow.
