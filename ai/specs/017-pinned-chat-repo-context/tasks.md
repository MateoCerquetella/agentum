# Tasks — Spec 017 pinned-chat-repo-context (Developer log)

- **Date:** 2026-07-13
- **Branch:** `project-hub-chat-is-blind-pinned-chat-has-no-rep` (this worktree)
- **Commits:** F1 `e6b6798f` · F2 `6d457545` (refactor) + `6ea8e5f9` (feat) · F3 `c5b22ce0`

## F1 `local-workdir-expansion` — DONE, gate GREEN

- `routes/util.rs`: `expand_with_home` → `pub(crate)` (the explicit-home test
  seam; doc comment says why — env mutation races the parallel suite).
- `routes/chat.rs`: `gather_repo_context` = thin env-HOME wrapper over new
  `local_repo_context(workdir, home)`; expansion via
  `super::util::expand_with_home(...).ok()?` BEFORE `is_dir()` (soft-None).
- `log_repo_context_outcome(route, workdir, …)` called from BOTH `chat` and
  `chat_stream` — `tracing::info!` with workdir/grounded/context_len (F2
  extended it with repo_id + arm).
- New test `local_repo_context_expands_tilde_workdir` (explicit temp home,
  `~/proj` fixture with CLAUDE.md → Some; absolute-nonexistent → None).
- **Gate:** `cargo test -p agentum-server --lib` **562/0/5** + fmt clean.
  `gather_repo_context_reads_guide_and_manifests`,
  `gather_repo_context_none_for_missing_or_empty_workdir`, byte-pins — green.

## F2 `repo-id-threading-and-ssh-gather` — DONE, gate GREEN

**S2 refactor (`6d457545`, byte-compat gated):**
- `RepoContextParts` + `assemble_repo_context` own ALL budgets, section
  headers, and the `TREE_MAX_FILES` cap (+`…(+N more files)`); collectors
  return raw text (`read_first_file` lost its budget param — double
  `truncate_chars` would stack markers); `git_tracked_tree` returns raw
  ls-files text; shared `GUIDE_CANDIDATES` / `MANIFEST_NAMES` consts.
- Local arm = fs collectors → assembler. Existing gather tests green
  unchanged = byte-compat proof.

**S3 + S4 + client (`6ea8e5f9`):**
- `remote_context_script(workdir)` — one sentinel-delimited
  (`===AGENTUM-CTX <section>===`) POSIX script; workdir `shlex::try_quote`d;
  `head -c` coarse caps (80k/40k/24k/16k/120k); `.harness/*` included;
  `exit 42` on cd fail. `parse_remote_context_output` splits sentinels,
  unknown sections skipped. `gather_repo_context_ssh` = ONE
  `sh -c {q(script)}` via `host_runtime::ssh_stdout` wrapped in
  `tokio::time::timeout(SSH_CONTEXT_TIMEOUT = 10s)`; every failure →
  `tracing::warn!` + None.
- `gather_repo_context_for(state, workdir, repo_id) -> (Option<String>, arm)`:
  repo_id → `repos::load_host_for_repo`; Local → local arm; Ssh → remote arm;
  lookup Err → warn + local-arm fallback (stale repo_id never blinds a valid
  local workdir). Both handlers use it (`State(_state)` → `State(state)`).
- `ChatRequest.repo_id` serde-default.
- Client: pure `ui/src/lib/chat-body.ts` (`buildChatBody` +
  `buildChatStreamBody`, no runtime imports) used by both fetches in
  `chat-client.ts`; `repoId` opt added to `sendChat`/`streamChat`;
  `chat-store.sendChatMessage` now passes `repoId: opts.repoId` (the
  original one-line drop); ChatPage already supplied `repoId: workspace?.id`.
- Tests: `chat_request_repo_id_is_serde_default`,
  `remote_context_script_quotes_workdir_and_guards_cd` (shlex round-trip on a
  spaced path), `remote_context_output_round_trips_to_snapshot` (+ tree-only
  and empty-output edges). Vitest `chat-body.test.ts` 4/4.
- **Gate:** cargo **565/0/5** + fmt · vitest new suite **4/4** · vite build ✓.

## F3 `blind-context-warning` — DONE, gate GREEN

- Server: pure `context_event_json(repo_id_present, has_context)` (None
  without repo_id; `{"state":"ok"|"missing","type":"context"}` with) — yielded
  FIRST in the stream generator, before the redacted-thinking notice.
  Stream-only (non-stream `/api/chat` grounds, no event — PM D6). Test
  `context_event_only_for_repo_backed_requests`.
- Client: `ChatStreamDelta` + `{ type: 'context'; state: 'ok' | 'missing' }`;
  `apply` forwards it to onDelta (no accumulation). `chat-store`:
  `ChatSnapshot.contextMissing` (errors-map lifecycle — cleared on send via
  `clearContextMissing`, cleared by `ok`, set by `missing` via
  `applyContextDelta`). New pure `ui/src/lib/chat-context-status.ts`
  (reducer + `contextWarningText`). `ChatPage`: `WarningBanner` (amber
  ErrorBanner sibling, no dismiss — state self-clears) rendered above the
  error banners whenever `chat.contextMissing[activeId]`, named via
  `workspace?.displayName`.
- **Gate:** cargo **566/0/5** + fmt · vitest new suites **10/10**
  (chat-body 4 + chat-context-status 6) · pre-existing
  `chat-client.test.ts` **5/5** (no regression) · vite build ✓ 1m43s.

## Deviations from architecture.md (3, all minor)

1. **`capped_tree` treats whitespace-only trees as None** (not in the
   blueprint): a blank-lines-only tree body would have rendered a header-only
   "file tree" section — grounding-shaped without grounding. Caught by the
   round-trip test.
2. **`ssh_output` not used directly** — it's private to `host_runtime`;
   used pub `ssh_stdout` (which also fails on non-zero exit — wanted for
   `exit 42`) wrapped in `tokio::time::timeout`, exactly the architecture's
   named fallback.
3. **Banner has no dismiss button** (architecture didn't specify one): the
   flag reflects live server state and self-clears on the next grounded
   send; a manual dismiss would hide a real problem. ErrorBanner keeps its
   dismiss (errors are one-shot).

## Not runtime-verified (for the tester / QA)

- Live SSH gather against a real remote host (unit-tested at the pure seams).
- The banner pixel-render + live SSE round trip (qa.sh / staging browser QA).
