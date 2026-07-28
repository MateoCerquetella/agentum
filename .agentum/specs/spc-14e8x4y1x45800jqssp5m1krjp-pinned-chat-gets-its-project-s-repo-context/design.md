# Architecture — Spec 017 pinned-chat-repo-context

- **Spec:** `ai/specs/017-pinned-chat-repo-context/spec.md` (PM handoff 01)
- **Date:** 2026-07-13
- **Author:** Architect (autonomous sdd-orchestrate)
- **Verdict:** Buildable as one spec, three gated slices (F1 → F2 → F3). No
  new crates, no new routes, no schema changes. All seams verified against
  this worktree's tree.

## Shape of the change

One sentence: make `gather_repo_context` host-aware behind a shared pure
assembler, thread `repo_id` client→server, and signal context status over the
existing SSE channel.

```
ChatPage (workspace = pinnedRepo | selected)
  └─ chat-store.sendChatMessage{ workdir, repoId }          ← F2 (repoId was dropped here)
       └─ chat-client.streamChat body{ workdir, repo_id }   ← F2
            └─ POST /api/chat/stream (ChatRequest.repo_id)  ← F2
                 ├─ gather_repo_context_for(state, workdir, repo_id)
                 │    ├─ no repo_id / Local host → LOCAL arm (expand_workdir-first)   ← F1
                 │    └─ Ssh host → REMOTE arm (one script, 10 s cap)                 ← F2
                 │         both → assemble_repo_context(parts)  [ONE format/budget fn]
                 ├─ tracing::info! workdir/arm/outcome                                 ← F1
                 └─ SSE: yield context event FIRST (only when repo_id present)         ← F3
                      └─ chat-client 'context' delta → chat-store.contextMissing
                           └─ ChatPage warning banner (streamError pattern)            ← F3
```

## Seams (signatures the developer builds to)

### S1 — local arm expands (F1)

- `routes/util.rs`: make `expand_with_home(raw: &str, home: Option<&Path>)`
  `pub(crate)` (today private; already unit-tested with explicit home).
- `chat.rs`: `gather_repo_context(workdir)` keeps its public signature but
  delegates to a new `fn local_repo_context(workdir: Option<&str>, home:
  Option<&Path>) -> Option<String>` that runs `expand_with_home(...).ok()?`
  before the `is_dir()` check. Production passes `std::env::var_os("HOME")`.
  **Never mutate `HOME`/env in tests** (the AGENTUM_HOME race, memory
  `ci-tag-gated-fmt-and-agentum-home-test-race`) — the explicit-home
  parameter IS the test seam. Soft failure only: an expansion error (empty,
  no HOME) → `None`, never an `ApiError` (gather is best-effort by contract).
- Existing tests `chat.rs:2519` / `:2540` stay green unchanged — `:2540`'s
  "None for /nonexistent" survives because expansion of an absolute path is a
  pass-through.

### S2 — one pure assembler, two arms (F2 refactor, byte-identical output)

Extract the string-building of today's `gather_repo_context` body
(`chat.rs:242–298`) into:

```rust
struct RepoContextParts {
    guide: Option<(String, String)>,      // (filename, body)
    harness_agents: Option<String>,
    feature_list: Option<String>,
    manifests: Vec<(String, String)>,     // (filename, body), manifest order
    tree: Option<String>,                 // RAW `git ls-files` text, uncapped
}
fn assemble_repo_context(parts: RepoContextParts) -> Option<String>
```

- The assembler owns ALL budgets (`GUIDE_BUDGET` … `CONTEXT_BUDGET`) and the
  `TREE_MAX_FILES` cap + `…(+N more files)` suffix (moved from
  `git_tracked_tree`, which now returns the raw text). Headers byte-identical
  to today (`## Repo guide ({name})`, `## .harness/AGENTS.md`, …).
- Shared name constants: `GUIDE_CANDIDATES: [&str; 3]`,
  `MANIFEST_NAMES: [&str; 10]` (used by the local arm, the remote script
  builder, and nothing else).
- The local arm becomes: collect parts from fs (existing `read_first_file`
  logic) → `assemble_repo_context`. Existing gather tests are the
  byte-compat gate for this refactor.

### S3 — remote arm (F2)

Three functions, two of them pure:

```rust
fn remote_context_script(workdir: &str) -> Result<String, _>   // pure; q()-quoted
fn parse_remote_context_output(out: &str) -> RepoContextParts   // pure
async fn gather_repo_context_ssh(host: &Host, workdir: &str) -> Option<String>
```

- **One SSH round trip** (PM D3): the script emits sentinel-delimited
  sections; run it as `sh -c {q(script)}` — the exact `git_fs.rs:221`
  precedent (`q` = `shlex::try_quote`, `host_runtime.rs:348`; the `sh -c`
  wrap is mandatory: the login shell may be fish). Script sketch:

  ```sh
  cd <q(workdir)> 2>/dev/null || exit 42
  for f in CLAUDE.md AGENTS.md README.md; do
    [ -f "$f" ] && { printf '===AGENTUM-CTX guide %s===\n' "$f"; head -c 80000 "$f"; printf '\n'; break; }
  done
  [ -f .harness/AGENTS.md ] && { printf '===AGENTUM-CTX harness-agents===\n'; head -c 40000 .harness/AGENTS.md; printf '\n'; }
  [ -f .harness/feature_list.json ] && { printf '===AGENTUM-CTX feature-list===\n'; head -c 24000 .harness/feature_list.json; printf '\n'; }
  for f in Cargo.toml package.json … tsconfig.json; do
    [ -f "$f" ] && { printf '===AGENTUM-CTX manifest %s===\n' "$f"; head -c 16000 "$f"; printf '\n'; }
  done
  printf '===AGENTUM-CTX tree===\n'; git ls-files 2>/dev/null | head -c 60000
  ```

  `head -c` caps are ~2× the char budgets (bytes ≥ chars) — coarse transport
  caps; the assembler enforces the real budgets. `.harness/*` included (it's
  free in one script — PM's open call, resolved yes). `exit 42` (cd failed)
  or empty output → `None`.
- **Timeout:** use the explicit-timeout transport (`ssh_output(host, script,
  timeout)` as `git_fs.rs` does with `GIT_TIMEOUT`; if it isn't visible from
  `routes/`, wrap `ssh_stdout` in `tokio::time::timeout`) with
  `const SSH_CONTEXT_TIMEOUT: Duration = Duration::from_secs(10)` (PM D4).
  Any error/timeout → `warn!` + `None` (degrades to the F3 warning — AC 5).
- Parser: split on `^===AGENTUM-CTX (.+)===$` lines; unknown section names
  skipped. Sentinel collision inside a file garbles that snapshot section at
  worst — never an error (accepted risk, note in code).

### S4 — host resolution (F2)

```rust
async fn gather_repo_context_for(
    state: &AppState, workdir: Option<&str>, repo_id: Option<&str>,
) -> (Option<String>, &'static str /* arm: "local"|"ssh"|"none" */)
```

- `repo_id` present → `super::repos::load_host_for_repo(state, rid)`
  (`repos.rs:363`, `pub(crate)`, async — same routes tree, no layering
  issue). `HostKind::Local` → local arm; `HostKind::Ssh` → remote arm.
- `load_host_for_repo` `Err` (repo/host deleted) → `warn!` and fall through
  to the local arm attempt — a stale `repo_id` must not make a valid local
  `workdir` blind.
- No `repo_id` → local arm (today's behavior + F1 expansion).
- Both handlers (`chat` `:538`, `chat_stream` `:643`) call this; rename
  `State(_state)` → `State(state)`. Non-stream gets grounding only, no event
  (PM D6). `retrieve_wiki` untouched (workdir-keyed; returns `None` for
  remote paths harmlessly — wiki-RAG is out of scope).
- The log line (F1, both handlers):
  `tracing::info!(workdir, repo_id, arm, context_len, "chat repo-context")` —
  one line per request, the AC-4 diagnostic.

### S5 — context SSE event (F3)

- `ChatRequest` gains `#[serde(default)] repo_id: Option<String>` (F2).
- In `chat_stream` only: compute before the upstream call
  `let context_event: Option<String> = body.repo_id.is_some().then(|| json!({"type":"context","state": if repo_context.is_some() {"ok"} else {"missing"}}).to_string());`
  and yield it as the FIRST event inside the `async_stream` generator —
  exactly the `redacted_thinking_notice` lead-in pattern (`chat.rs:~765`).
  Emitted only when the request carried `repo_id` (PM D2), so non-workspace
  chats see zero wire change. If the upstream call 502s the event is never
  sent — fine, the typed error dominates.
- Keep the decision logic as a tiny pure fn
  `fn context_event_json(repo_id_present: bool, has_context: bool) -> Option<String>`
  for a direct unit test.

### S6 — client threading + banner (F2 + F3)

- `chat-client.ts`: add `| { type: 'context'; state: 'ok' | 'missing' }` to
  `ChatStreamDelta` (`:35`); `apply` gains an `else if (ev.type ===
  'context') opts.onDelta?.(ev)` branch (no accumulation). Old clients are
  already tolerant — verified: unmatched types fall through `apply` silently.
  Add `repoId?: string` to both `sendChat` and `streamChat` opts and send
  `repo_id` in both JSON bodies. **Extract the body construction into a pure
  `buildChatBody(messages, opts)`** (exported for vitest) so AC-2's "request
  body includes workdir + repo_id" is a pure-model test — no fetch mock, no
  jsdom (repo convention).
- `chat-store.ts`: `sendChatMessage` passes `repoId: opts.repoId` through to
  `streamChat` (the one-line bug); `ChatSnapshot` gains
  `contextMissing: Readonly<Record<string, true>>` — set when a
  `context: missing` delta arrives, cleared on the next send into that
  conversation AND on a `context: ok` (same lifecycle as `errors`).
- New pure helper `ui/src/lib/chat-context-status.ts`:
  `applyContextDelta(map, convoId, state)` → next map (immutable), and
  `contextWarningText(repoName: string | null)` → the banner copy
  ("agentum couldn't read this project's files for chat — …"). Vitest-able,
  zero DOM (precedent: `socratic-intake.ts`).
- `ChatPage.tsx`: read `chat.contextMissing[activeId]`, render a warning
  banner adjacent to the existing `streamError` surface (`:652`); shown in
  pinned AND un-pinned-with-workspace modes (PM D2). Banner copy uses
  `workspace?.name`.

## Build order & per-slice gates

| Slice | Contents | Gate |
| ----- | -------- | ---- |
| **F1** `local-workdir-expansion` | S1 + the S4 log line (local-only degenerate form is fine until F2 lands the full resolver) | `cargo test -p agentum-server --lib` (new tilde test w/ explicit home; `:2519`/`:2540` + byte-pins `:2332`/`:2470` green), `cargo fmt --all` |
| **F2** `repo-id-threading-and-ssh-gather` | S2 + S3 + S4 + `ChatRequest.repo_id` + client body threading | cargo tests (assembler byte-compat via existing gather tests; script/parse round-trip; quoting w/ spaces; serde-default `repo_id`) + `bunx vitest run` on `buildChatBody` + `npm run build --prefix crates/agentum-desktop/ui` |
| **F3** `blind-context-warning` | S5 + S6 (delta type, store map, pure helper, banner) | cargo test (`context_event_json`), vitest (`chat-context-status`, delta handling), vite build |

Riskiest first within F2: land S2 (refactor under existing tests) before S3.

## Risks & mitigations

1. **The S2 refactor drifts the local output** → existing gather tests are
   the gate; do S2 as a pure move-and-delegate commit before any remote code.
2. **Byte-pinned grounding blocks** (`chat.rs:2332`/`:2470`, spec 008) — no
   change to `intake_grounding_blocks` at all; only `repo_context`'s
   `Some`-ness changes. Any diff touching those literals is a defect.
3. **Env mutation in tests** — forbidden (HOME race); the explicit-home and
   pure-fn seams exist precisely so no test sets env vars.
4. **Wedged SSH** — hard 10 s timeout on the one remote call; failure path is
   `None` + warning event, the reply always streams (AC 5).
5. **Stale `repo_id`** — resolver falls back to the local arm, never errors
   the chat request.
6. **Sentinel collision** — garbled section at worst, never a failure;
   documented in code.
7. **`bun`/`npm` + vitest baseline** — count only NEW vitest failures (large
   pre-existing baseline); never gate on bare `tsc`.

## What the tester should probe (beyond the gates)

- `gather_repo_context(Some("~"))` (bare tilde) and a trailing-slash workdir.
- A `repo_id` whose host record was deleted (resolver fallback path).
- `context: ok` after a previous `missing` clears the banner state.
- Non-stream `/api/chat` with `repo_id` — grounds, and emits NO event field.

## Expected developer artifact

`ai/specs/017-pinned-chat-repo-context/tasks.md` — per-slice task list with
gate results, plus handoff `03-developer-to-tester.md`. Update GitHub #361 on
each slice transition (coding / gate green).
