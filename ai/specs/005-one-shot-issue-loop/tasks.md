# Spec 005 — tasks (per-feature checklist)

Status legend: `[x]` done in this worktree · `[ ]` not started (later slice).
Developer slice 1 (iteration 3) scope: **F2, F3, F4 only**. Developer slice 2
(iteration 4) scope: **F1 only** — F5 is the next slice.

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

## F5 — github-state-map (AC 10) — NOT STARTED (later slice)

- [ ] `GithubStateMap` + `github.json`/env layering + `github_status_color(phase)`
- [ ] Widened `gh_set_status_label_argv` / `github_transition_with` (name-filtered remove-set)
- [ ] Desktop `github_labels.rs` flat-arg Tauri commands + `ui/src/tauri/github-labels.ts` + IntegrationsPane GitHub card
- [ ] §6 tests (incl. byte-identical default-map argv pin)

Notes for the F5 slice (from this one):
- `github_slug_and_number_from_issue_url` is now `pub(crate)` (F4) — no further widening needed.
- The F1-slice Todo assertions can keep using `github_status_label(phase)` as the default-name accessor per §6.

Notes for the F5 slice (from the F1 slice):
- `ensure_spec_and_plan_fires_todo_at_plan` (routes/harness.rs) asserts the DEFAULT
  label spelling `status/todo` in the fake-gh argv — when F5 makes names
  configurable, that assertion stays valid (the test env has no `github.json`/
  env overrides) but be aware it exercises `GithubStateMap::from_env` defaults.
- The same test mutates `AGENTUM_GH_BIN` under `crate::TEST_ENV_LOCK` — F5's
  `AGENTUM_GITHUB_CONFIG` tempdir isolation should take the same lock if it
  mutates env (prefer the pure `apply_layers` injection per §6).

## Gate results (this slice)

See the developer handoff / final report for exact counts. Gates run:
`cargo fmt --all` · `cargo test -p agentum-server --lib` ·
`npm run build --prefix crates/agentum-desktop/ui`.
