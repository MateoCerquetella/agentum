# Spec 024 — Architecture

- **Spec:** `024-sdd-toolbar-session-fidelity`
- **Phase:** Architect → Developer
- **Author:** Mateo Cerquetella (autonomous SDD loop)
- **Date:** 2026-07-21
- **Tracker:** https://github.com/MateoCerquetella/agentum/issues/412

## System boundary

This is one terminal/session-fidelity change with two thin sides:

1. The desktop derives the SDD toolbar from the terminal pane's bound server
   session plus live agent evidence. It keeps one toolbar per active terminal
   group and sends every action to that binding.
2. The existing SDD injection route waits for the existing `inject_prompt`
   result and returns the real delivery outcome instead of acknowledging before
   delivery.

The Harness Engine, loop driver, playbook bodies, global status bar, terminal
stream, and agent adapters stay unchanged.

## Components and files

### Desktop UI

- `crates/agentum-desktop/ui/src/components/sdd/SddBar.tsx`
  - Replace the transient-agent-only `SddBarGate` decision at lines 156–162 with
    a stable bound-session-aware decision.
  - Keep `useServerSessionId` (lines 82–92) as the action address.
  - Rearrange the existing button array/render at lines 34–39 and 292–325.
  - Add explicit `sending` state and consume the delivered response.
- `crates/agentum-desktop/ui/src/components/tab-group/TabGroupPanel.tsx`
  - Keep the existing per-group mount at lines 381–392 byte-for-byte except for
    any prop required by the revised gate.
- `crates/agentum-desktop/ui/src/components/terminal-pane/server-pane-connection.ts`
  - After `ensureWorkspaceSession` returns at lines 347–351, validate a newly
    requested tool before binding.
  - After a pinned session is loaded, hydrate the tab from the returned
    `Session.tool` when it is a `TuiAgent`.
  - Keep the `server:<sessionId>:<leafId>` registration at lines 381–383 as the
    canonical session address.
- `crates/agentum-desktop/ui/src/store/slices/terminals.ts`
  - Add the inverse of `clearTabLaunchAgent` (lines 1141–1160):
    `setTabLaunchAgent(tabId, agent)`, updating only the owning tab and scheduling
    runtime graph sync.
- `crates/agentum-desktop/ui/src/runtime/workspace-session.ts`
  - Strengthen find-or-create compatibility at lines 111–137.
  - Fix `sessionName` at lines 80–86 so truncation preserves the tool and stable
    hash rather than truncating them away.
- `crates/agentum-desktop/ui/src/runtime/sdd-client.ts`
  - Extend the injection response from `{mode}` to `{mode, ready}`; no new client
    transport.

### Server

- `crates/agentum-server/src/routes/sdd.rs`
  - Keep route validation and `prompt_for` at lines 108–139 and 159–183.
  - Make one-shot `inject` await `harness::inject_prompt` instead of spawning a
    detached task; return `200 {mode, ready}` only after both tmux send steps
    succeed.
  - Emit `sdd.injected` after success. On an injection error, emit
    `sdd.inject_failed` with playbook/mode and return the existing API error
    envelope.
  - Do not change `drive_sdd_loop`; Loop steps already await the same primitive.
- `crates/agentum-server/src/harness/drive.rs::inject_prompt` (lines 1126–1175)
  - Reuse unchanged. Its `Ok(bool)` already means both sends succeeded and
    reports whether readiness was confirmed.
- `crates/agentum-server/src/mcp_provision.rs::tool_supports_mcp` /
  `agent_mcp_file` (lines 27–76)
  - Remain the single delivery-mode registry. Unknown/unwired tools continue to
    receive the full playbook through the generic fallback.

## Data model and identity rules

No server schema changes are needed. The existing fields have distinct jobs:

- `TerminalTab.launchAgent`: requested/display identity for a tab.
- `TerminalTab.serverSessionId` or registered `server:<id>:<leaf>` PTY: binding
  identity.
- `Session.tool`: process launch and MCP-provisioning truth.
- `useTabAgent(tab)`: live evidence, including manually started agents and
  confirmed exits.

Add a small pure UI resolver near `SddBar.tsx` (or `lib/sdd-toolbar-agent.ts` if
needed by tests):

```ts
resolveSddToolbarAgent({
  sessionTool,       // Session.tool when bound/known
  requestedAgent,    // tab.launchAgent
  liveAgent           // useTabAgent(tab)
}): TuiAgent | null
```

Precedence:

1. A bound `Session.tool` that passes `isTuiAgent` is stable authority.
2. A `terminal` session may use `liveAgent`; this enables a manually started
   recognized agent while keeping delivery in safe full-playbook mode.
3. Before the first bind resolves, `requestedAgent` keeps the launch-time bar
   stable.
4. No recognized signal returns `null`; a true shell gets no actionable bar.

Do **not** PATCH a manual shell session's `Session.tool`. That field also means
“this process was launched/provisioned as this tool.” Relabeling a manually
started Codex session to `codex` would falsely select the MCP bootstrap even
though Codex was not launched with Agentum MCP. Live evidence affects toolbar
eligibility only; the server's persisted tool continues to select delivery
mode.

## Session create/attach compatibility

### Generated names

Change `sessionName` from truncating the completed string to truncating only the
human-readable base:

```text
<truncated-base>-<clean-tool>-<hash(workdir + tab-id)>
```

The tool and hash suffix must always survive within the 64-byte ASCII limit.
This prevents long worktree names from collapsing Claude and Codex per-tab
session names.

### Matching and 409 recovery

For every `ensureWorkspaceSession` request, compatibility always requires:

```text
normalized host + exact workdir + exact tool
```

When `name` is present, exact name is an additional requirement, not a
replacement for workdir/tool. The initial list and the post-409 recovery use the
same predicate. If a 409 exists but no compatible row appears, rethrow the 409;
the pane falls through the existing visible error/local-fallback behavior
instead of attaching another agent.

After `startSession`, validate the returned row with the same predicate before
returning it. This closes the final race without changing `freshPane` semantics.

### Pinned/restored sessions

A pinned `serverSessionId` expresses “open this existing session,” not “launch
the tab's stale requested agent.” Fetch it with existing `getSession`, then call
`setTabLaunchAgent` from its actual `Session.tool` when recognized. No mismatch
rejection applies to pinned inspection because the server ID is explicit; the
UI is corrected to reality before SDD controls enable.

## Toolbar layout and lifecycle

- Split `SDD_BUTTONS` into `LEFT_ACTIONS = [Spec, Spec Socratic, Status]` and
  the existing Continue action rendered in the right cluster.
- Remove only the expanded standalone `SDD` span. Keep accessible names and the
  collapsed restore control (“Show the SDD bar”) so issue #349 does not regress.
- Layout: left actions → flexible notice/sending slot → Continue → Loop → Hide.
- `SddBarGate` renders while `resolveSddToolbarAgent` is non-null. Bound
  `Session.tool` prevents idle title/hook gaps from unmounting it; confirmed
  shell exit on a `terminal` session still removes it through `useTabAgent`.
- `useServerSessionId` stays the address for inject/read/toggle/event filtering.
  The bar's `key={tab.id}` in `TabGroupPanel` prevents state crossing tabs.

## Injection API and data flow

### One-shot action

1. User confirms a preview; `SddBar` captures its current `sessionId`, clears
   preview, sets `sending`, and calls `injectSddPlaybook(sessionId, name)`.
2. Server reloads and validates that exact session and chooses mode from
   `Session.tool`:
   - launch-arg/file-provisioned tool → `bootstrap`;
   - everything else, including a manual agent inside `terminal` → `full`.
3. Server awaits unchanged `inject_prompt`.
4. Success: emit `sdd.injected`, return `200 {mode, ready}`. The UI renders
   “sent via MCP” / “sent (full text)”; `ready:false` may append “sent before
   readiness was confirmed” but is not a failed tmux delivery.
5. Failure: emit `sdd.inject_failed`, return an error; the client promise rejects
   and the toolbar renders the existing “Could not inject …” notice with the
   server message where available.

### Loop action

Unchanged: toggle/read/event calls already use `sessionId`, and
`drive_sdd_loop` already awaits `inject_prompt`. This spec adds no request ID,
loop event, worker, or client-local loop state.

## Important decisions and tradeoffs

### D1 — Await one-shot delivery over a correlated event

Choose a synchronous response. The primitive already returns the exact success
and readiness result, and toolbar injections are low-frequency user actions.
This may hold the request during the existing readiness window, but the toolbar
can show `Sending…`. A correlated event would add request IDs, reconnect replay,
and timeout state for no additional correctness.

### D2 — Session tool over title as stable authority

Choose persisted `Session.tool` for server-launched agents because titles and
hooks are intentionally transient. Use live evidence only for a manual agent in
a `terminal` session. This preserves correct MCP provisioning and still supports
manual starts.

### D3 — Fail closed instead of repairing incompatible rows

Do not PATCH or rename a mismatched running session. Reusing it risks controlling
the wrong process; restarting it risks killing user work. Rejecting visibly is
safer and lets the existing pane fallback/error surface explain the failure.

### D4 — Preserve one launch path

Do not pass desktop launch commands through the SDD layer and do not add agent
special-cases. Server-created agent sessions continue through
`spawn_agent_into_pane`; generic adapters and full-playbook fallback remain the
extension mechanism.

## Acceptance criteria → implementation and tests

| AC | Plan | Named verification |
| --- | --- | --- |
| 1 | Split toolbar actions and remove expanded label in `SddBar.tsx`. | Update `TabGroupPanel.sdd-bar.test.tsx`: assert no standalone label, DOM order `Status < Continue < Loop`, and restore control remains. |
| 2 | `resolveSddToolbarAgent` uses bound session tool before transient live evidence. | New pure resolver tests: agent session + live null remains visible; terminal + live agent visible; terminal + live null hidden; collapsed preference regression. |
| 3 | Keep bound `sessionId` as every request/event address and key state per tab. | Component/client test with two tab IDs/session IDs: click each action and assert only its bound route; existing split-group render test stays green. |
| 4 | Preserve tool/hash in `sessionName`; require host+workdir+tool(+name); hydrate pinned tab from `Session.tool`. | `workspace-session.test.ts`: long-name Claude/Codex names differ; incompatible initial/409/start rows reject; compatible race still reuses. `server-pane-connection` pure/helper test pins requested Codex and returned Claude to visible failure, while pinned Claude hydrates truthfully. |
| 5 | Server `prompt_for` keeps MCP registry + generic full fallback; manual terminal eligibility remains UI-only. | Rust `bootstrap_only_for_tools_with_mcp_wiring` expands to `agent` and generic unknown; UI table test iterates `Object.keys(TUI_AGENT_CONFIG)` and proves each recognized agent can make the toolbar actionable, with `terminal` absent unless live. |
| 6 | Await `inject_prompt`; success event/200 after send, failure event/error on send failure; UI sending/success/failure states. | Extract `inject_with` seam in `sdd.rs` for scripted delivery: success returns `{mode,ready}` and emits `sdd.injected`; error emits `sdd.inject_failed` and never success. Focused SddBar client-state test covers pending then success/reject. |

## Risks and mitigations

- **Long HTTP wait:** `await_repl_ready` can take ~56 seconds. Mitigation: only
  one-shot toolbar calls become synchronous, UI renders `Sending…`, Loop stays a
  worker. Accepted over event-correlation complexity (D1).
- **Manual-agent false MCP claim:** patching `Session.tool` would lie about
  provisioning. Mitigation: never patch it; manual agents receive full text.
- **Wrong-session race:** a row can change between list/create/start. Mitigation:
  one compatibility predicate is applied to initial lookup, 409 recovery, and
  start response; every mismatch rejects.
- **Pinned stale UI identity:** old `launchAgent` may disagree with the explicit
  server ID. Mitigation: pinned server record hydrates the tab; actual session
  wins because no new launch was requested.
- **Agent registry drift:** desktop and server have different registries.
  Mitigation: no duplicate “supported agent” list is added; UI iterates
  `TUI_AGENT_CONFIG`, server uses existing MCP registry plus generic full
  fallback.
- **Remote regression:** process inspection cannot be local for SSH. Mitigation:
  stable authority comes from the host-aware session returned by the server;
  injection continues through `load_host_for_session` and `host_runtime`.
- **Sacred prompt mechanics:** changing response timing could tempt a rewrite.
  Mitigation: `inject_prompt` body and its two-step byte sequence stay untouched
  and remain covered by existing tests.

## Developer build order

1. **Identity foundation:** `sessionName` suffix preservation, compatibility
   predicate, tests; add `setTabLaunchAgent` and pinned hydration/mismatch guard.
2. **Stable toolbar:** pure identity resolver, layout, per-tab targeting tests.
3. **Truthful delivery:** synchronous route/result events, client response,
   sending/success/failure tests.
4. Run focused Vitest, `npm run build --prefix crates/agentum-desktop/ui`,
   `cargo test -p agentum-server --lib`, and `cargo fmt --check`; then browser QA
   per the spec.

## Architect verdict

**PASS.** All six acceptance criteria map to existing seams and named tests;
boundaries, tradeoffs, risks, and invariants are explicit. No PM send-back or
new abstraction is required.
