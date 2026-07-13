# Review — Spec 009 pinned-chat-repo-context

- **Spec:** `ai/specs/009-pinned-chat-repo-context/spec.md` (GitHub #361, p1)
- **Date:** 2026-07-13
- **Reviewer:** autonomous sdd-orchestrate (main session), on top of the
  independent tester verdict (`verification.md`, PASS-WITH-DEFERRALS)
- **HEAD reviewed:** `e56f1fea` (code: `e6b6798f` + `6d457545` + `6ea8e5f9` +
  `c5b22ce0` on base `d957eefd`)
- **Verdict:** **SIGN-OFF — SHIP-READY.** 0 blockers, 0 must-fix. 4
  leave-as-is nits. Release remains human-gated (see Deferrals).

## What was independently re-verified by the reviewer (with evidence)

1. **Sacred surface** — `intake_grounding_blocks` (`chat.rs:559`) literals:
   zero diff lines touch them (grep over the full base..HEAD diff of chat.rs);
   tester additionally proved SHA-1-identical body. Byte-pin tests green in
   both the developer's and tester's runs (566/0/5 twice, independently).
2. **Resolver correctness** (`gather_repo_context_for`, `chat.rs:487`) —
   trims/filters empty `repo_id`; `Local` host → local arm; `Ssh` → remote arm
   only with a non-empty workdir (else `(None,"ssh")` → warning, correct);
   lookup `Err` → `warn!` + fall through to the local arm, so a stale
   `repo_id` can never blind a valid local workdir. Matches architecture S4
   exactly.
3. **Remote arm safety** (`chat.rs:388–480`) — workdir shell-quoted
   (`shlex::try_quote`), constant-name loops carry no quoting hazard, script
   wrapped `sh -c {q(script)}` (the `git_fs.rs:221` fish-safe precedent),
   `exit 42` on cd-fail surfaces as `ssh_stdout` error → `warn!` + `None`;
   `tokio::time::timeout(SSH_CONTEXT_TIMEOUT=10s)` bounds the one round trip;
   every failure leg is soft-`None` — no `ApiError` exists in any gather path
   (tester grep + my read concur). AC-5 honored by construction.
4. **Event position & scope** — `context_event_json` (`chat.rs:522`, pure,
   4-case tested) is computed pre-stream and yielded FIRST in the generator
   (`chat.rs:1036–1041`), before the redacted-thinking notice; referenced
   only in `chat_stream` (stream-only, PM D6). Non-workspace requests see
   zero wire change.
5. **Client threading end-to-end** — `ChatPage.tsx:309` (`repoId:
   workspace?.id`) → `chat-store.ts:231` pass-through → pure
   `buildChatStreamBody`/`buildChatBody` (`lib/chat-body.ts`, both carry
   `repo_id`, old shape preserved when absent — vitest'd). The store's
   `contextMissing` mirrors the `errors` lifecycle precisely: cleared on send
   (`clearContextMissing`) and by `context: ok` (`applyContextDelta`, with a
   same-reference no-change optimization). Banner renders from
   `chat.contextMissing[activeId]` beside the streamError surface
   (`ChatPage.tsx:655`), pinned and un-pinned alike (PM D2).
6. **`Repo.displayName` is real** — pre-existing field used at
   `ChatPage.tsx:394/585` before this spec; the banner's name lookup cannot
   silently `undefined` (checked because vite build does not typecheck).
7. **Deviation audit concurrence** — all 3 developer deviations are accurate
   and sound; notably `ssh_output` genuinely isn't reachable from `routes/`
   (lives in `agentum_tmux::ssh`), and `ssh_stdout` + outer tokio timeout is
   the architecture's own named fallback with the outer 10 s dominating.

## Gates (three independent runs agree)

| Gate | Developer | Tester (independent) |
| ---- | --------- | -------------------- |
| `cargo test -p agentum-server --lib` | 566/0/5 | 566/0/5 |
| `cargo fmt --all --check` | clean | clean |
| vitest (new + touched suites) | 10/10 new, 5/5 held, 4/4 chat-body | 15/15 (+ socratic-intake 5/5); only-importing-suites grep-proven |
| vite build | ✓ 1m43s | ✓ 1m16s |

## Leave-as-is nits (recorded, not blocking)

1. `chat.rs:1030` uses `body.repo_id.is_some()` while the resolver filters
   empty strings — a hypothetical `repo_id: ""` emits an event for a
   local-arm fallback. Unreachable from our client (uuid or absent). Fine.
2. `ChatPage.tsx:~1331`: the pre-existing doc line "A dismissible inline
   error strip for the transcript." now sits above `WarningBanner`'s doc
   instead of `ErrorBanner`'s. Cosmetic comment placement.
3. `gather_repo_context_for` glue is code-verified, not unit-tested (needs
   `AppState`); both arms and the event decision it composes are tested.
4. One test comment overstates a tree-only-grounds assertion (tester's find,
   documented in `verification.md`).

## Deferrals (human / staging — NOT CI-runnable)

- **Live SSH gather** against a real remote host (AC-3's live leg).
- **Browser render** of the warning banner over a live SSE round trip (AC-4's
  pixel leg) — the qa.sh checklist: local hub chat answers a codebase
  question grounded; simulated bad workdir shows the banner.
- Release: promote develop → staging → main per branch flow. **RELEASE =
  HUMAN** (repo convention).

## Disposition

`spec.md` Status → Done. Phase → done. Branch carries code + full SDD trail
(spec, architecture, 4 handoffs, tasks, verification, review). PR into
`develop` with `Closes #361` is the next human (or /ship) step.
