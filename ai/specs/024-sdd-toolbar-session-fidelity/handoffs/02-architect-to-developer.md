# Handoff 02 — Architect → Developer

- **Spec:** `024-sdd-toolbar-session-fidelity`
- **Date:** 2026-07-21
- **From:** Architect (autonomous SDD loop)
- **To:** Developer
- **Artifact:** `architecture.md`
- **Tracker:** https://github.com/MateoCerquetella/agentum/issues/412

## Verdict

Architect gate: **PASS**.

- Files and existing seams were read and cited.
- Every AC maps to a named implementation part and test.
- Identity precedence, mismatch behavior, injection confirmation, and the
  supported-agent matrix are pinned.
- Every risk has a mitigation; one-launch-path, remote-host, MCP-first, and
  two-step injection invariants remain intact.

## Decisions the developer must preserve

1. `Session.tool` is launch/provisioning truth. Never PATCH a manually started
   shell agent to an MCP-capable tool; it must receive the full playbook.
2. Per-tab matching always requires normalized host + workdir + tool, with name
   as an additional key. Validate initial, 409-recovery, and start responses.
3. `sessionName` truncates only the base so tool + hash always survive.
4. Pinned session ID means inspect that actual session and hydrate the tab;
   newly requested incompatible tool means reject visibly.
5. Await one-shot `inject_prompt`; emit/return success only after both send
   steps. Do not edit `inject_prompt` or `drive_sdd_loop`.
6. Keep a true shell toolbar-free; live recognized agents in terminal sessions
   are eligible but use full-playbook delivery.

## Build order

1. Workspace-session name/match hardening and tests.
2. Tab identity setter, bind hydration/guard, stable toolbar resolver/layout.
3. Synchronous injection outcome/events/client state.
4. Focused tests, UI build, server lib tests, fmt, then QA.
