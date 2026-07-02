# Spec 005 — tasks (per-feature checklist)

Status legend: `[x]` done in this worktree · `[ ]` not started (later slice).
Developer slice 1 (iteration 3) scope: **F2, F3, F4 only**. Developer slice 2
(iteration 4) scope: **F1 only**. Developer slice 3 (iteration 5) scope: **F5**
— the developer phase is COMPLETE (see "ready for tester" below).

## F1 — start-work-gated-run (AC 1–5) — DONE (slice 2)

- [x] `ensure_spec_and_plan(store, workdir, number, issue, plan, converge_existing)` shared core refactored out of `spec_from_issue` (routes/harness.rs) — never-overwrite 400 pinned via `converge_existing: false`; converge path re-plans from the existing spec without touching its body; Todo-at-plan fires on any successful plan (§2 verbatim `warn!` shape), so the 004 opt-in route inherits AC 4.
- [x] `POST /api/harness/start-work` handler + route (static route added beside `/settings`, above the `{id}` captures). The 8-step §2 sequence under `state.harness.start_work_lock` (C5): already-running check FIRST (claim fails → 200 `alreadyRunning` friendly state; claim succeeds on an idle stale run → `engine.stop` + fresh registration; claim `Err` (run vanished concurrently) → fresh registration), fetch, `ensure_spec_and_plan(plan: true, converge: true)`, `update_backlog_knobs` (agent_tool/agent_model only — `spec_id` NOT re-stamped), `engine.start` AFTER the plan, `claim_driver` + `tokio::spawn(drive)` byte-identical to the run route. `StartWorkRequest`/`StartWorkResponse` camelCase per §2.
- [x] `HarnessEngine.start_work_lock: tokio::sync::Mutex<()>` (`pub(crate)` — routes access it directly per the §2 handler snippet) + `pub async fn find_by_workdir`.
- [x] `types.rs::update_backlog_knobs(workdir, apply)` — load → mutate → persist via `resolve_harness_dir` (handles legacy `.harness/` like the load path).
- [x] UI: `startGatedWork` + `StartGatedWorkResult` in `runtime/harness-client.ts` (§2 verbatim shape).
- [x] UI: `open-created-workspace.ts` — `gatedRun?: boolean` option; pure `planCreatedWorkspaceOpen` extracted and used by the body (gated → all three plain deliveries false; default path pinned unchanged).
- [x] UI: `useComposerState.ts` — `initialStartGatedRun` option, `startGatedRun` state, `maybeStartGatedRun(worktree, item, agent)` (mirrors `maybeScaffoldSpecFromIssue`'s non-fatal shape; `alreadyRunning` → info toast, failure → error toast, never a rollback), BOTH submit paths derive `submitGatedRun` (armed AND eligible), force issue automation off at the source, skip the D5 scaffold call, pass `gatedRun` + `issueCommand: undefined` to `openCreatedWorkspace`; cardProps expose `canStartGatedRun` (= `canScaffoldSpec`) + `startGatedRun` + `onStartGatedRunChange`.
- [x] UI: `NewWorkspaceComposerCard.tsx` — "Start gated run" toggle below the D5 toggle, same eligibility gate; armed → the scaffold toggle hides and the §2 undelivered-prompt copy renders.
- [x] UI: `NewWorkspaceComposerModal.tsx` — `ComposerModalData.startGatedRun?: boolean` threaded to `initialStartGatedRun`.
- [x] UI: `TaskPage.tsx` — `openComposerForItem(item, opts?)` widened; "Start gated run" entry in the issue-row dropdown (beside "Start new workspace"/"Open in browser") → `openComposerForItem(item, { startGatedRun: true })`.
- [x] Tests: `ensure_spec_and_plan_writes_and_plans_fresh`, `ensure_spec_and_plan_converges_on_existing_spec` (both 400- and converge-arms), `ensure_spec_and_plan_fires_todo_at_plan` (fake-gh via `AGENTUM_GH_BIN` under `crate::TEST_ENV_LOCK`, asserts one `issue edit … --add-label status/todo`), `find_by_workdir_resolves_registered_run`, `update_backlog_knobs_preserves_features_and_writes_knobs`; `claim_release_driver_round_trips` already covered the claim/release round-trip (unchanged). Vitest: `planCreatedWorkspaceOpen` (7 cases) + a wire-level `openCreatedWorkspace` gated-run case in the EXISTING `lib/open-created-workspace.test.ts`.

### F1 deviations

1. **`ensure_spec_and_plan`'s `workdir` param is `&std::path::Path` fully qualified** — a top-level `use std::path::Path` collides with the axum `Path` extractor already imported in routes/harness.rs (37 compile errors); the §2 signature is otherwise verbatim.
2. **Existing-run claim `Err` falls through to a fresh registration** (not an error): the run can vanish between `find_by_workdir` and `claim_driver` via a concurrent `DELETE /api/harness/{id}` — nothing is driving the worktree, so registering fresh is the correct friendly behavior. (§2 only specified the `false`/`true` arms.)
3. **No `release_driver` call landed**: nothing fallible sits between the fresh claim and `tokio::spawn` (§2's "in practice this is the step-6→7 error path" — step 6 errors happen *before* the claim in the final ordering). The unreachable `!claimed`-on-fresh arm returns 500 without releasing (we don't own the slot when a claim fails).
4. **Hermetic tracker-arm tests**: the fresh/converge tests use a NON-github-host URL (`https://example.com/...`) so the Todo transition resolves to the URL-parse `Skipped` (no `gh` spawn, no env mutation); only the dedicated Todo-at-plan test wires the fake `gh` (env-locked). Tracker stamps still assert the URL round-trip.
5. **Vitest extended, not created**: `lib/open-created-workspace.test.ts` already existed (wire-level tests through the real store) — the `planCreatedWorkspaceOpen` cases and a gated-run wire case were added there instead of a new file. It loads fine (no xterm noise).
6. **`initialStartGatedRun` modal-data threading needed no store change** — `store/slices/ui.ts`'s `modalData` is `Record<string, unknown>`; the typed `ComposerModalData` lives in `NewWorkspaceComposerModal.tsx` (the blueprint's "ui.ts ~:450 modal-data type" doesn't exist as a typed slot in this tree).
7. **Toggle copy when un-armed**: the §2 copy ("Your typed prompt won't be sent…") renders once ARMED; un-armed the toggle shows a neutral one-liner ("Plan the linked issue into a spec and drive it with verification-gated agents.") so the warning doesn't fire before it applies.
8. **`maybeStartGatedRun` takes the agent as a param** (`agent: TuiAgent | null`) rather than closing over `tuiAgent` — the quick path's agent is a function argument, not hook state; `TuiAgent` is a plain string union so `agentTool: agent` (no `.id`).

## F2 — spec-aware-feature-prompt (AC 6) — DONE

- [x] Pin FIRST: `feature_prompt_without_spec_is_byte_identical` (harness.rs tests mod) — written against the pre-change 2-arg `build_feature_prompt`, run to green (1 passed / 499 filtered), THEN the function was widened and the test call updated to pass `None` with the literal unchanged.
- [x] `plan_from_spec_inner` stamps `list.spec_id = Some(spec_id.to_string())` (harness/types.rs) + the stale ":924-926" doc comment rewritten per C4 (records the deliberate MCP `agentum_harness_plan` widening + roles-stay-false safety).
- [x] `build_feature_prompt(instructions, feature, spec_rel_path: Option<&str>)` (harness/helpers.rs) — explicit-`prompt` short-circuit stays FIRST; `=== THE SPEC ===` section (§3 exact wording) inserted between the AGENTS.md block and the task block only when `Some`.
- [x] Call site (harness/drive.rs, feature loop step 4) — `spec_rel` computed via `config.harness_dir.file_name()` + on-disk existence check, exactly the §3 snippet (handles legacy `.harness`, stale spec_id → None).
- [x] `feature_prompt_with_spec_names_the_path_and_says_read_first`
- [x] `feature_prompt_explicit_override_wins_even_with_spec` (second byte pin)
- [x] `plan_from_spec_delegation_unchanged` extended: `spec_id == Some("s1")` + `!list.roles`; `plan_from_spec_with_tracker_stamps_provider_and_url` extended with the same `spec_id` assert. (§3 item 4 was phrased both as a named test `plan_from_spec_stamps_spec_id` and as "extend the two existing tests" — implemented as the extensions, per the item's own instruction.)

## F3 — qa-agentum-browser (AC 7–8) — DONE

- [x] Pin FIRST: `resolve_qa_mode_matrix` designed against today's noted behavior (Auto + no qa.sh + no env → Script skip-pass); the `capable=false` column reproduces it exactly (the D3 byte-identical pin).
- [x] `build_qa_prompt` step 1 rewritten to the §4 `agentum_browser` wording (open + split:"right" + navigate/click/fill/snapshot + screenshot evidence + "Do NOT use the browser-verification-loop skill…"); step 2 (verdict contract) character-for-character untouched.
- [x] `resolve_qa_mode(config, agent_qa_capable: bool)` — fully pure decision table (env read moved to the caller).
- [x] Caller in `drive_inner`: `agent_qa_capable = playwright_mcp::feature_enabled() || browser_qa_agent_enabled(state).await` (+ the best-effort `browser_qa_agent_enabled` helper mirroring `orchestration_enabled`).
- [x] Stale `AGENTUM_BROWSER_VERIFY`/Playwright warning in `run_qa_agent_gate` replaced with the MCP-master-switch warning (§4 snippet, keyed on `MCP_ENABLED_SETTING` default-true).
- [x] Log line "spawning browser QA agent (browser-verification-loop)" → "(agentum_browser)".
- [x] `pub const BROWSER_QA_ENABLED_SETTING: &str = "harness.qa.agent_browser.enabled"` in routes/harness.rs (§4 doc comment verbatim).
- [x] `GET/PUT /api/harness/settings` mirroring routes/mcp.rs:91-114; wire shape `{"browserQaAgentEnabled": bool}`.
- [x] `resolve_qa_mode_honors_explicit_and_auto` tightened: passes `false`, old `Script | Agent` env-leakage tolerance → exact asserts.
- [x] `resolve_qa_mode_matrix` — all 12 cells, no env mutation.
- [x] `harness_qa_setting_defaults_off_and_round_trips` (routes/harness.rs new tests mod, mirrors the mcp.rs settings test).
- [x] UI: `getHarnessSettings`/`setHarnessSettings` in `runtime/harness-client.ts`; `BrowserQaGateToggle` in `components/settings/IntegrationsPane.tsx` (load-on-mount like `LinearStateMapEditor`, optimistic write + revert like `McpPane`).

### F3 deviations

1. **Test wording vs. prompt wording conflict resolved in favor of the verbatim prompt.** §4's test plan item 1 says the QA prompt "does not contain `browser-verification-loop`", but §4's own verbatim step-1 wording contains "Do NOT use the browser-verification-loop skill". Kept the verbatim prompt; the test asserts the prompt no longer *instructs* the skill (`"Use the `browser-verification-loop`"` absent) while the negative steer is present.
2. **Toggle placement.** §4 says the toggle "lands beside the Linear state-map pipeline config", but `LinearStateMapEditor` is nested inside the Linear card and only rendered when Linear is *connected*. The browser-QA knob is provider-agnostic, so gating it on a Linear connection would hide it for GitHub-only users — rendered as a standalone card at the end of `IntegrationsPane` instead (same pane, sibling card).
3. **Doc-comment touch-up** on `build_qa_prompt` (its header described the browser-verification-loop) — doc-only, same function the blueprint rewrites.
4. **Extra test** `harness_settings_wire_shape_is_camel_case` (not in the §4 plan) — pins the `browserQaAgentEnabled` camelCase wire contract the TS client depends on.
5. **Left stale-but-out-of-scope docs untouched:** `FeatureList.qa_mode` / `qa_agent_tool` doc comments in types.rs still mention `AGENTUM_BROWSER_VERIFY`/browser-verification-loop. Blueprint's F3 file table doesn't include types.rs; flagging for the reviewer rather than widening the diff. (Behavior is unaffected — docs only.)

## F4 — mcp-report-status (AC 9) — DONE

- [x] Tool spec (§5 JSON, verbatim) appended to `tool_specs()`; NOT in `ORCHESTRATION_TOOLS`.
- [x] Dispatch arm `"agentum_report_status" => tool_report_status(state, &args).await` in `call_tool`.
- [x] `parse_report_status_args` (pure): provider + phase required; `id` required except github-with-parseable-issue-URL (id := the URL's number via `github_slug_and_number_from_issue_url`).
- [x] `report_status_text` (pure): `Ok(Applied)` → `"applied: {provider} → {phase:?}"`; `Ok(Skipped(w))` → `"skipped: {w}"`; `Err(e)` → `"skipped (tracker error, non-fatal): {e:#}"`.
- [x] `tool_report_status` thin: parse → `apply_tracker_transition` → text. An unknown provider flows to the seam's `Ok(Skipped(..))` — visible, non-fatal.
- [x] `task_sink.rs`: `pub fn parse_tracker_phase(&str) -> Option<TrackerPhase>` (pure) + `github_slug_and_number_from_issue_url` widened to `pub(crate)`.
- [x] `parse_tracker_phase_accepts_the_four_and_rejects_junk` (task_sink.rs).
- [x] `report_status_args_require_id_except_github_url`.
- [x] `report_status_text_never_errs_on_tracker_failure` (the AC 9 pin).
- [x] `report_status_is_in_the_catalog` + `report_status_survives_orchestration_gate_off`.
- [x] Catalog arithmetic test (`off.len() + ORCHESTRATION_TOOLS.len() == on.len()`) needed **no** update — the tool is ungated so it adds 1 to both sides, exactly as §5 predicted.
- [x] Wire-level delegation: `report_status_moves_a_board_card` — tempdir-Store `AppState` fixture (the `board_sync.rs::fresh_state` pattern) + a real board card moved todo→doing through the REAL `tool_report_status`; also asserts the unknown-provider Skipped path.

### F4 deviations

1. **AppState fixture, not the bare task_sink fixture.** §5 item 5 cites the `board_transition_moves_card_status` fixture pattern; since `tool_report_status` takes `&AppState` (per the §5 signature), the test uses the established `fresh_state()` AppState-over-tempdir-Store fixture (copied from `routes/board_sync.rs` tests) so it drives the real tool fn end-to-end. No production-code change was made to accommodate the test.

## F5 — github-state-map (AC 10) — DONE (slice 3)

- [x] Pin FIRST: `gh_set_status_label_argv_adds_one_removes_exactly_the_other_three`
  extended with per-phase **byte-exact** expected argv literals against the
  PRE-change 3-arg builder, run to green (1 passed / 516 filtered), THEN the
  builder was widened and the call updated to `&GithubStateMap::default()`
  with the literals unchanged (the F5-changes-nothing-by-default pin).
- [x] `GithubStateMap` (+ `Default` delegating to `github_status_label`, so
  defaults and the `GITHUB_STATUS_LABELS` table can't drift), `StoredGithubStateMap`
  / `GithubConfigFile` serde shapes, `github_config_path()` (`AGENTUM_GITHUB_CONFIG`
  override else `<data_local_dir|data_dir>/Agentum/github.json`, mirroring
  `linear.rs::creds_path`), `read_github_config()` (absent/garbled → Default),
  `from_env()` = `apply_layers(file, env)` with `AGENTUM_GITHUB_STATUS_{TODO,
  IN_PROGRESS,READY_TO_TEST,DONE}`, `label_for(phase)`, `labels()`;
  `github_status_color(phase)` (colors keyed by PHASE, never name).
- [x] `gh_set_status_label_argv(number, slug, phase, map)` — remove-set = the
  other configured names, filtered BY NAME (target never in its own remove
  list), deduped; foreign/stale-map labels never appear in the argv (doc'd).
- [x] `github_transition_with(program, slug, number, phase, map)` — ensure-loop
  dedupes names (first phase in canonical order wins the color); Applied/Skipped
  semantics unchanged. The github arm resolves `GithubStateMap::from_env()`
  AFTER the URL parse succeeds (no-url skips never touch the config file).
- [x] Desktop: new `commands/github_labels.rs` — `github_get_state_map()` /
  `github_set_state_map(todo, in_progress, ready_to_test, done)` (FLAT
  `Option<String>` args; blank clears the override; both return the effective
  map with camelCase keys), `STORE_LOCK`ed read-modify-write on `github.json`
  beside `linear.json`. Registered in `commands/mod.rs` + the
  `generate_handler![]` list in `src/lib.rs`.
- [x] UI: new `ui/src/tauri/github-labels.ts` (`githubGetStateMap` /
  `githubSetStateMap`); `IntegrationsPane.tsx` gains `GithubStatusLabelsEditor`
  — four inputs mirroring `LinearStateMapEditor`'s load/save flow, rendered
  **unconditionally** inside the GitHub card (gh is the default tracker).
- [x] Tests: `github_state_map_defaults_are_canonical`,
  `github_state_map_precedence_file_then_env` (pure `apply_layers` injection —
  NO env mutation), `gh_set_status_label_argv_uses_configured_names`,
  `gh_set_status_label_argv_never_removes_the_target_on_name_collision`, the
  byte-pin above, `github_transition_with_custom_map_flips_configured_names`
  (`#[cfg(unix)]` fake-gh, explicit program + explicit map — no env, no lock),
  plus `github_transition_ensures_duplicate_names_once` (extra, pins the
  documented ensure-dedup/color-precedence); arity-updated
  `github_transition_applies_with_fake_gh` and
  `github_transition_maps_gh_failure_to_skipped`.

### F5 deviations

1. **`ensure_spec_and_plan_fires_todo_at_plan` hardened, not just left green:**
   the github arm now calls `GithubStateMap::from_env()`, so a real
   `<data_dir>/Agentum/github.json` on a dev machine would rename the asserted
   `status/todo`. The test now also sets `AGENTUM_GITHUB_CONFIG` to an ABSENT
   tempdir file (defaults apply) under the same `crate::TEST_ENV_LOCK` it
   already holds for `AGENTUM_GH_BIN` — exactly the isolation the F1-slice
   note prescribed.
2. **UI payload keys are camelCase (`inProgress`, `readyToTest`)** — verified
   against tauri-macros 2.6.2: flat command args are looked up by the
   camelCase key with NO snake_case fallback, and a missing key deserializes
   an `Option<String>` as `None`. NOTE (pre-existing, out of scope): the
   Linear editor's `api.linear.setStateMap({ in_progress, ready_to_test })`
   sends snake_case keys, so those two fields silently bind as `None` (=
   "clear override") on every Linear save. Flagged for a follow-up issue.
3. **Extra test** `github_transition_ensures_duplicate_names_once` (not in the
   §6 7-item plan) — pins the ensure-dedup + first-phase-color-wins behavior
   §6 specifies in prose.
4. **`GithubStatusLabelsEditor` re-renders from the save response** (the
   effective map), so a blanked input visibly snaps back to its canonical
   `status/*` default — the Linear editor keeps the local state instead;
   the returned-effective-map contract §6 specifies is exercised.
5. **Build-env note (not a code change):** `cargo check -p agentum-desktop`
   fails at tauri-build's bundle-resource validation unless
   `target/release/libsherpa-onnx-{c,cxx}-api.dylib` exist in the worktree's
   own target dir (known sherpa gotcha). Copied from the main checkout's
   `target/release/` — nothing committed.

Notes carried from earlier slices (resolved here):
- `github_slug_and_number_from_issue_url` stayed `pub(crate)` (F4) — no
  further widening was needed.
- `github_status_label(phase)` stays the default-name accessor; the F1 Todo
  assertions and back-compat tests are untouched.

## Developer phase COMPLETE — ready for tester

All five features (F1–F5) are implemented on this branch. Gate results for
slice 3 (F5): `cargo fmt --all` clean · `cargo test -p agentum-server --lib`
**518 passed / 0 failed / 5 ignored** (~132s) · `cargo check -p agentum-desktop`
green (~80s warm) · `npm run build --prefix crates/agentum-desktop/ui` green
(~3m55s, `NODE_OPTIONS=--max-old-space-size=3072`).

Tester must know:

- **Env-locked tests:** `ensure_spec_and_plan_fires_todo_at_plan`
  (routes/harness.rs) is the ONLY test that mutates env (`AGENTUM_GH_BIN` +
  `AGENTUM_GITHUB_CONFIG`), under `crate::TEST_ENV_LOCK`. Every other F5 test
  is pure-injection (`apply_layers` closures) or passes the fake `gh` program
  + map explicitly. Do not add env-mutating tests without that lock.
- **Fake-gh pattern:** `write_fake_gh` in task_sink.rs tests (argv-logger
  script, `#[cfg(unix)]`); custom-map behavior is pinned at both argv level
  and process level.
- **Byte-pins to protect:** default-map argv literals in
  `gh_set_status_label_argv_adds_one_removes_exactly_the_other_three` were
  captured against pre-F5 code — never regenerate them from the code under
  test. Same class: F2's `feature_prompt_without_spec_is_byte_identical`,
  F3's verdict-contract assertions.
- **NOT GUI-verified:** the Settings → Integrations GitHub labels card, the
  composer "Start gated run" toggle (F1), the Tasks-page row action, and the
  end-to-end custom-label flip on a real repo (`qa.sh` flow in §8) — all only
  build-verified (`vite build` + `cargo check`). No installed-app run.
- **Known pre-existing bug found (not fixed, out of F5 scope):** Linear
  state-map saves silently clear `in_progress`/`ready_to_test` (snake_case
  invoke keys vs camelCase binding — see F5 deviation 2). Worth its own issue.
- **Freshness contract:** the server re-reads `github.json` on every github
  transition (`from_env` in the arm), so Settings edits apply on the next
  transition with no restart. Mid-flight map changes leave stale-named labels
  behind BY DESIGN (foreign-label protection — see the builder doc comment).

## Gate results (slice 2)

See the developer handoff / final report for exact counts. Gates run:
`cargo fmt --all` · `cargo test -p agentum-server --lib` ·
`npm run build --prefix crates/agentum-desktop/ui`.
