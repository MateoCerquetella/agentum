# Handoff 01 — PM → Architect

- **Spec:** `024-sdd-toolbar-session-fidelity`
- **Date:** 2026-07-21
- **From:** PM (autonomous SDD loop, iteration 1)
- **To:** Architect
- **Tracker:** https://github.com/MateoCerquetella/agentum/issues/412

## Verdict

PM gate: **PASS** after one refinement pass.

| Gate item | Verdict |
| --- | --- |
| One slice | PASS — one user action: operate the visible terminal's SDD toolbar against that pane's real server session. Layout, visibility, identity, and delivery share this boundary. |
| Problem before solution | PASS — the user cannot trust which live pane the toolbar represents or targets. |
| Persona and value | PASS — Mateo using mixed agent terminals; one-line user value is explicit. |
| Acceptance criteria | PASS — six checkbox criteria with observable render, mount, target, reject, inject, and report outcomes. |
| Scope / non-goals | PASS — loop mechanics, playbooks, Harness gates, global status bar, new agents, and true-shell actions are excluded. |
| Grounded in code | PASS — the existing toolbar, split-group mount, pane binding, launch/attach paths, and server injection route are cited. |
| Invariants | PASS — one launch path, bound-session addressing, MCP-first fallback, remote routing, and two-step injection are preserved. |
| Harness wiring | PASS — one feature entry with unit/build/browser gates. |
| Size | PASS — the initial architecture-heavy nine-criterion draft was reduced to six product outcomes and implementation choices were deferred. |
| Duplicate/conflict | PASS — issue #395 owns the read-only gated-run phase strip; spec 016 owns loop completion/check-in; spec 399 owns broader gated-run surfacing. This spec owns interactive terminal toolbar/session fidelity. |

## PM decisions locked

1. A genuine plain shell has no actionable SDD toolbar; “sometimes disappears”
   means transient identity loss must not unmount a still-running agent's bar.
2. `Continue` sits directly before `Loop` in the right action group. The
   standalone expanded “SDD” label is removed; the deliberate Hide/Show control
   remains discoverable.
3. Codex→Claude substitution fails closed. The UI must never attach or relabel
   an incompatible session merely to keep a terminal open.
4. “All agents” means every recognized desktop `TuiAgent`: MCP bootstrap where
   provisioned, full-playbook fallback otherwise. It does not mean injecting
   prose into a shell.
5. A queued HTTP response is not a delivered result. The UI may show an
   intermediate queued state but may show “sent” only after pane delivery.

## Architect must pin

- The single identity precedence/reconciliation model across requested
  `launchAgent`, bound `Session.tool`, and live foreground/hook evidence.
- Whether injection confirmation is synchronous or event-correlated, including
  failure and reconnect behavior.
- The exact compatibility guard for new per-tab sessions versus pinned/restored
  sessions, without perturbing `freshPane` or double-launch protection.
- A testable supported-agent delivery matrix derived from existing registries,
  not a second hand-maintained list.
