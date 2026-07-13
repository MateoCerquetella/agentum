# Handoff 03 — Developer → Tester

- **Spec:** 017-pinned-chat-repo-context
- **Date:** 2026-07-13
- **From:** Developer (autonomous sdd-orchestrate)
- **To:** Tester
- **Artifacts:** `tasks.md` (full per-slice log), commits `e6b6798f`,
  `6d457545`, `6ea8e5f9`, `c5b22ce0` on this branch.

## What changed (one paragraph)

Chat's repo-context gather is now host-aware and honest: the local arm
expands `~` before the dir check (chat.rs was the only workdir route that
didn't); the client finally sends `repo_id` (it was dropped in
`chat-store.sendChatMessage`), letting the server resolve the repo's host and
gather over SSH via ONE 10s-bounded sentinel script; both arms feed a single
pure assembler (byte-identical local output); every request logs
workdir/repo_id/arm/grounded; and workspace-backed streams lead with a
`context` SSE event that raises an amber ChatPage banner when grounding
failed.

## Gates as run by the developer (re-run these independently)

| Gate | Command | Result |
| ---- | ------- | ------ |
| Server | `cargo test -p agentum-server --lib` (use `$HOME/.cargo/bin/cargo`) | **566 passed / 0 failed / 5 ignored** |
| fmt | `cargo fmt --all` | clean (committed) |
| New vitest | `cd crates/agentum-desktop/ui && bunx vitest run src/lib/chat-body.test.ts src/lib/chat-context-status.test.ts` | **10/10** |
| Existing chat suite | `bunx vitest run src/runtime/chat-client.test.ts` | **5/5** (pre-existing, no regression) |
| UI build | `npm run build --prefix crates/agentum-desktop/ui` (bun install first if node_modules missing) | ✓ 1m43s |

## What to verify beyond re-running gates (from architecture "tester probes")

1. **Bare tilde**: `local_repo_context(Some("~"), Some(tmp_home))` with a
   guide at the home root → grounds; trailing-slash workdir (`~/proj/`) also
   grounds (expand_with_home trims).
2. **Stale repo_id fallback**: `gather_repo_context_for` with a repo_id that
   doesn't resolve → falls back to local arm (the `Err` branch warn path) —
   verifiable by reading the code path + the existing repos tests; no store
   fixture needed if you assert via unit seams.
3. **`context: ok` clears a previous `missing`** — covered by
   `applyContextDelta` tests; confirm the store wiring calls it on BOTH send
   (clear) and delta (apply).
4. **Non-stream `/api/chat`** takes repo_id (grounds) but emits no event —
   confirm `context_event_json` is only referenced in `chat_stream`.
5. **Sacred surfaces intact**: `intake_grounding_blocks` literals untouched
   (byte-pin tests green); wiki path untouched; no new public routes
   (`auth.rs::is_public` unchanged).
6. **Full vitest baseline**: only compare NEW failures vs the known large
   pre-existing baseline; the four suites named above are the spec-scoped set.

## Deviations to sanity-check (3, in tasks.md)

whitespace-only tree → None (new `capped_tree` guard) · `ssh_stdout` +
`tokio::time::timeout` instead of private `ssh_output` · warning banner has
no dismiss (self-clearing state).

## Deferred (NOT tester scope)

Live SSH gather against a real host and the browser banner render = qa.sh /
staging (human-gated), per the spec's harness wiring.
