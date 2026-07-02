# Spec 005 — tasks (per-feature checklist)

Status legend: `[x]` done in this worktree · `[ ]` not started (later slice).
Developer slice 1 (iteration 3) scope: **F2, F3, F4 only** — F1/F5 are later
slices by orchestrator instruction.

## F1 — start-work-gated-run (AC 1–5) — NOT STARTED (later slice)

- [ ] `POST /api/harness/start-work` handler + `ensure_spec_and_plan` shared core (Todo-at-plan lives there)
- [ ] `HarnessEngine.start_work_lock` + `find_by_workdir`
- [ ] `types.rs::update_backlog_knobs`
- [ ] UI: `startGatedWork` client, composer toggle + three-skip `gatedRun` flag, TaskPage row action
- [ ] §2 unit tests + `planCreatedWorkspaceOpen` vitest

Notes for the F1 slice (from this one):
- `/api/harness/settings` now coexists with `/api/harness/{id}` (static-over-capture) — `start-work` can be added the same way.
- Do NOT re-stamp `spec_id` in the post-plan knob write — `plan_from_spec_inner` stamps it now (F2, landed here).
- `routes/harness.rs` now has a `#[cfg(test)] mod tests` — put the `ensure_spec_and_plan` tests there.

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

## Gate results (this slice)

See the developer handoff / final report for exact counts. Gates run:
`cargo fmt --all` · `cargo test -p agentum-server --lib` ·
`npm run build --prefix crates/agentum-desktop/ui`.
