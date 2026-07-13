# Handoff 01 — PM → Architect

- **Spec:** 017-pinned-chat-repo-context
- **Date:** 2026-07-13
- **From:** PM (autonomous sdd-orchestrate iteration 1)
- **To:** Architect
- **Artifact:** `ai/specs/017-pinned-chat-repo-context/spec.md` (PM-gated)
- **Tracker:** GitHub #361 (p1, `type/fix`) — keep the issue updated on every
  feature state transition (architecture-principles rule).

## Gate result

PM gate (`ai/skills/validate_handoff.md`): **PASS** — all nine items green.
Every code citation verified against THIS worktree's tree on 2026-07-13:
`gather_repo_context` `chat.rs:235` (doc-comment says "all LOCAL — Chat never
SSHes"); blind access rule `chat.rs:338`; `ChatRequest` `chat.rs:120` (no
repo/host field); `expand_workdir` `util.rs:19` — grep shows **every** other
workdir-taking route uses it (sessions/harness/board_goals/mcp/wiki/uploads),
chat.rs alone does not; `resolve_repo_host_id` `repos.rs:350` +
`load_host_for_repo` `repos.rs:363` (both `pub(crate)`, async, AppState-based);
`host_runtime::ssh_stdout` `host_runtime.rs:318` + `read_remote_file`
`host_runtime.rs:298`; client sends workdir only (`chat-store.ts:219`;
`chat-client.ts` `sendChat`/`streamChat` bodies have no repoId);
`ChatStreamDelta` `chat-client.ts:35` (`text|thinking|error|done`); pinned
wiring correct (`ProjectHubPage.tsx:180` → `ChatPage.tsx:302`).

## Decisions locked (PM)

- **D1 — repo_id on every workspace-selected send.** Not just pinned: the
  un-pinned chat with a selected workspace sends `repo_id` too (same send
  path, `workspaceId` in hand). Zero extra cost, makes SSH-repo chats work
  outside the hub. (Spec open-Q 2 → yes.)
- **D2 — banner is event-driven, not mode-driven.** The warning banner renders
  wherever a `context: missing` SSE event arrives (pinned mode is the required
  AC; un-pinned-with-workspace gets the same banner for free). The server
  emits the `context` event **only** when the request carries `repo_id`.
- **D3 — remote transport is the architect's call** (spec open-Q 1), under two
  constraints: minimize SSH round trips (one `ssh_stdout` script preferred)
  and a hard timeout with degrade-to-warning (AC 5). The output contract is
  fixed: same section headers + budgets as the local arm.
- **D4 — timeout is a 10 s constant.** No env knob, no config surface (YAGNI).
  Tune the constant at build time if evidence demands.
- **D5 — sacred surface:** `intake_grounding_blocks` (`chat.rs:308`) string
  literals are byte-pinned by spec 008 tests (`chat.rs:2332`, `chat.rs:2470`).
  The fix changes only whether `repo_context` is `Some` — never the block
  text. Any architecture that rewords them is wrong.
- **D6 — context event is stream-only.** `/api/chat` (non-stream) gets
  `repo_id` grounding but no context signal (it has no SSE channel); the hub
  chat uses `/api/chat/stream`. Don't invent a response-envelope field for the
  non-stream route.
- **D7 — no remote-context cache in this slice.** One gather per turn. If
  latency hurts, that's a follow-up issue, not scope creep here.

## Material PM findings

1. **The local bug is `~`-expansion, full stop.** chat.rs is the only
   workdir-taking route that never calls `expand_workdir`; a `~/…` `Repo.path`
   silently fails `root.is_dir()` → `None` → the exact observed transcript.
   Don't chase the "pinnedRepo resolution race" theory from the issue — in
   pinned mode `workspace = pinnedRepo` unconditionally (`ChatPage.tsx:194`).
2. **The server literally cannot try SSH today** — no repo identity arrives.
   `repoId` exists client-side on the `Conversation` but `sendChatMessage`
   drops it before `streamChat` (`chat-store.ts:219`).
3. **Existing tests to keep green:** `gather_repo_context_reads_guide_and_manifests`
   (`chat.rs:2519`) and `gather_repo_context_none_for_missing_or_empty_workdir`
   (`chat.rs:2540`) — note the second asserts `None` for a *nonexistent* path;
   `~`-expansion must not break that. Plus the two byte-pin tests (D5).
4. **Vitest here is pure-model only** (no jsdom). The banner AC needs the repo
   pattern: a pure derivation helper in `ui/src/lib/` with vitest coverage +
   a thin component change (precedent: `socratic-intake.ts`,
   `workspace-goal-step.ts`). Don't blueprint a DOM-render test.
5. **Verify commands for this repo** (memory-confirmed): `cargo test -p
   agentum-server --lib`, `bun run build` / `npm run build --prefix
   crates/agentum-desktop/ui`, `bunx vitest run` scoped to the new suites
   (full vitest has a large pre-existing failing baseline — count NEW
   failures only). Bare `tsc` cannot resolve `shared/*` — never gate on it.

## What to blueprint (F1 → F3 order, riskiest first is F2's remote arm)

1. **F1 `local-workdir-expansion`** — where exactly `expand_workdir` applies
   (inside `gather_repo_context` vs a wrapper), the diagnostic log line's
   shape and level in `/api/chat/stream` (workdir received → arm chosen →
   outcome), and the tilde-expansion unit test (needs `AGENTUM_HOME`-style
   isolation? `expand_workdir` expands `~` via home-dir — check how other
   routes' tests handle it).
2. **F2 `repo-id-threading-and-ssh-gather`** — `ChatRequest.repo_id` (serde
   default) + client threading (`chat-client.ts` both bodies, `chat-store.ts`
   pass-through, `ChatPage.tsx` supplies `workspace?.id`); the host-aware
   gather seam (who resolves the host: handler vs helper; `load_host_for_repo`
   is async + needs `AppState` — confirm no layering cycle chat.rs→repos.rs);
   the remote arm as a **pure function** of fetched bodies (unit-testable
   without SSH) + the one-script fetch + 10 s timeout + degrade path.
3. **F3 `blind-context-warning`** — the `context` SSE event's emission point
   in the stream handler (before first token), the `ChatStreamDelta` variant,
   client parser tolerance for unknown event types (verify — else guard), the
   pure banner-state helper + minimal ChatPage render.

## Open architect calls (flag to Mateo only if genuinely blocked)

- D3 transport shape (one script vs N `read_remote_file`s).
- Whether the remote arm includes `.harness/*` files (local arm does; a remote
  repo rarely has them — include if the one-script fetch makes it free).

## Expected architect artifact

`ai/specs/017-pinned-chat-repo-context/architecture.md` — boundaries, seam
signatures, the remote-arm purity split, test plan per feature, risks —
matching prior specs' `architecture.md` shape.
