# Handoff 02 — Architect → Developer

- **Spec:** 009-pinned-chat-repo-context
- **Date:** 2026-07-13
- **From:** Architect (autonomous sdd-orchestrate)
- **To:** Developer
- **Artifacts:** `spec.md` (ACs), `architecture.md` (seams S1–S6, build order,
  per-slice gates), `handoffs/01-pm-to-architect.md` (decisions D1–D7)

## Gate result

Architect gate: **PASS.** Every seam verified against this worktree on
2026-07-13 (handler signatures `chat.rs:538/643` take `State<AppState>` as
`_state`; `q()`+`sh -c` precedent `git_fs.rs:221`; client parser tolerance
confirmed at `chat-client.ts:175-188`; `errors`-map lifecycle to mirror at
`chat-store.ts:31-58` + `ChatPage.tsx:231/652`). No invariant conflicts: no
launch-path/YOLO/streaming changes; SSE event is additive; no new public
routes; spec-008 byte-pins untouched by construction.

## Build exactly this, in this order

1. **F1** — S1 (expand-first local arm, explicit-home test seam) + the
   diagnostic log line. Gate: `cargo test -p agentum-server --lib` +
   `cargo fmt --all`; tests `chat.rs:2519/2540/2332/2470` must stay green.
2. **F2** — S2 (pure assembler refactor FIRST, byte-identical under existing
   tests) → S3 (remote arm: script + parser pure fns, one `sh -c {q(script)}`
   round trip, 10 s timeout) → S4 (resolver + `ChatRequest.repo_id`) + client
   body threading with the pure `buildChatBody` extraction. Gate: cargo tests
   (round-trip, quoting-with-spaces, serde-default) + vitest `buildChatBody` +
   vite build.
3. **F3** — S5 (`context` event, lead-in yield like `redacted_thinking_notice`,
   pure `context_event_json`) + S6 (delta variant, `contextMissing` store map,
   `lib/chat-context-status.ts` pure helper, ChatPage banner). Gate: cargo +
   vitest + vite build.

## Hard rules (from PM D1–D7 + memory)

- Never edit `intake_grounding_blocks` literals (byte-pinned, `chat.rs:2332/2470`).
- Never mutate `HOME`/env in tests — use the explicit-home seam.
- Gather is best-effort: every failure → `None` + log, never an `ApiError`.
- One SSH round trip, 10 s constant timeout, degrade to warning (never hang).
- Count only NEW vitest failures (pre-existing baseline is large); never gate
  on bare `tsc`; UI build = `npm run build --prefix crates/agentum-desktop/ui`.
- Use `$HOME/.cargo/bin/cargo` if bare `cargo` is missing from PATH.
- Commits: conventional style, reference #361, NO AI attribution trailers.
- Keep GitHub #361 updated per slice (coding → gate green).

## Expected artifacts

`tasks.md` (per-slice log + gate results + deviations),
`handoffs/03-developer-to-tester.md`, committed code on this branch.
