# Spec 024 — Tester Verification

- **Spec:** `024-sdd-toolbar-session-fidelity`
- **Date:** 2026-07-21
- **Role:** Tester (independent autonomous re-verification)
- **Verdict:** **PASS-WITH-QA-DEFERRALS — advance to Reviewer.** All six
  acceptance criteria pass by focused tests plus code inspection. No product
  defect or send-back was found.

## Independent gates

| Gate | Result |
| --- | --- |
| Focused Vitest: `TabGroupPanel.sdd-bar.test.tsx`, `SddBar.identity.test.ts`, `sdd-injection-state.test.ts`, `workspace-session.test.ts`, `sdd-client.test.ts` | **PASS — 5 files, 18/18 tests** |
| UI production build: `npm run build` | **PASS — 7,238 modules, built in 6m26s**; existing dynamic-import and large-chunk warnings only |
| `cargo test -p agentum-server --lib routes::sdd::tests` | **PASS — 13/13 tests** (774 filtered out) |
| `cargo fmt -p agentum-server -- --check` | **PASS** |
| `git diff --check` | **PASS** |
| `cargo fmt --all -- --check` | **Baseline-only failure** in untouched `crates/agentum-executor/src/adapters.rs`; the changed `agentum-server` package is clean |

The worktree's UI `node_modules` is only an empty cache stub, so Vitest and the
build were run with the existing main-checkout dependency tree temporarily
linked into the UI directory. The original directory was restored after the
commands; no dependency or cache artifacts remain in the diff.

## Acceptance criteria

| AC | Verdict | Evidence |
| --- | --- | --- |
| 1 — expanded layout and label removal | **PASS** | `SddBar.tsx` renders left actions `Spec`, `Spec Socratic`, `Status`, followed by the flexible notice and right-side `Continue`, `Loop`, Hide controls. The standalone expanded `SDD` span is gone. Static component tests assert label absence, `Status < Continue < Loop` DOM order, and the collapsed restore control. |
| 2 — stable visibility and explicit Hide/Show | **PASS** | `resolveSddToolbarAgent` gives a recognized bound `Session.tool` precedence over transient live evidence; a manual recognized agent is eligible only with live evidence in a `terminal` session, while a true shell remains hidden. Resolver and component tests cover persisted-agent/live-null, terminal/live-agent, terminal/live-null, pre-bind identity, lookup failure, and collapse/restore. Runtime reconnect/idle behavior remains in browser QA. |
| 3 — visible pane is the sole action/event target | **PASS** | `useServerSessionId(tabId)` derives only the active tab's `server:<sessionId>:<leaf>` binding. Inject and loop calls capture/use that ID, event filtering requires the same `session_id`, and `TabGroupPanel` keys the bar by `tab.id`. `sdd-client.test.ts` proves separate injection and loop calls retain their supplied session IDs. |
| 4 — launch/attach/restore agent fidelity | **PASS** | Generated names truncate only the base and retain `-tool-hash`; the single compatibility predicate requires normalized host + exact workdir + exact tool + optional name for initial lookup, create response, 409 recovery, and start response. New mismatches reject, while explicit pinned sessions hydrate the tab from actual `Session.tool`. Focused tests cover long Claude/Codex names and incompatible create/409/start responses. |
| 5 — all-agent server injection path | **PASS** | Toolbar eligibility iterates every `TUI_AGENT_CONFIG` entry without adding a second registry. Server `prompt_for` still uses existing MCP wiring and the generic full-playbook fallback; the Rust matrix covers `claude`, `codex`, `cursor`, `agent`, `gemini`, `opencode`, plus terminal/unwired/unknown tools. A plain shell has no actionable toolbar. Real non-MCP pane delivery is deferred to browser QA. |
| 6 — truthful sent/failure outcomes | **PASS** | One-shot injection now awaits unchanged `inject_prompt`, returns HTTP 200 with `{mode, ready}` only on successful delivery, then emits `sdd.injected`. Failure emits only `sdd.inject_failed` and returns an API error. Rust tests prove mutually exclusive events; UI tests prove pending state, confirmed success copy, detailed rejection copy, and pending cleanup. Existing stopped-session and unknown-playbook tests remain green. |

## Findings

- **Blockers:** none.
- **Should-fix:** none.
- **Informational:** repository-wide rustfmt remains red solely on the committed,
  untouched `agentum-executor/src/adapters.rs`; the affected server package
  passes its formatting gate.

## Deferred to `qa.sh` / staging

- Open Claude and Codex in separate live splits and capture the final toolbar
  layout, including `Continue` immediately before `Loop`.
- Exercise idle-title, process-hook gap, reconnect, and tab/split switching in
  the running desktop; confirm the bar stays available and no action crosses
  pane boundaries.
- Restore/pin an existing session and confirm its actual tool replaces stale UI
  intent without launching or attaching the wrong agent.
- Deliver one action through an MCP-provisioned agent and one through a real
  non-MCP/manual recognized agent; confirm bootstrap versus full-playbook mode
  and the final sent/failure notices.

These are the spec's declared live desktop/server screenshot legs, not Tester
gate failures.
