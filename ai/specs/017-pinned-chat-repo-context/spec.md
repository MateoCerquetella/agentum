# Spec 017 — Pinned chat gets its project's repo context

- **Number:** 017
- **Status:** Done
- **Surface:** `crates/agentum-server/src/routes/chat.rs` + `crates/agentum-desktop/ui`
- **Author:** Claude (from GitHub issue #361, filed by Mateo)
- **Date:** 2026-07-13
- **Tracker:** GitHub #361 (`type/fix`, `priority/p1`, `area/server` + `area/desktop`)

## Problem

The chat inside a project (Project Hub → Chat tab) is blind to the very repo it
is pinned to: the assistant answers "this chat runs without a workspace
selected — I can't read files" and asks the user to paste code. Turn after turn
of the model apologizing for a wiring bug makes the product look broken — a
project-scoped chat that can't see its project defeats its purpose.

## Goal

A pinned hub chat always grounds in its project's repo context — local **or**
SSH — and when context gathering genuinely fails, the UI says so explicitly
instead of leaving the model to apologize.

## Users / personas

- **Mateo (multi-project operator)** — opens a project's hub, asks the pinned
  chat "how does the sidebar/board code work?", and expects a grounded answer
  about *that* repo. Today he gets "no workspace loaded" for the exact repo the
  chat is pinned to (real transcript against the agentum project itself).
- **SSH-project user** — same moment, but the project lives on a remote host.
  Today their hub chat is blind **by construction** (see diagnosis).

## Diagnosis (verified in code, 2026-07-13)

- Server context comes from `gather_repo_context(workdir)`
  (`crates/agentum-server/src/routes/chat.rs:235`) — documented "all LOCAL —
  Chat never SSHes". It checks `root.is_dir()` on the **raw string** and
  returns `None` otherwise; the honest-blind access rule at `chat.rs:338`
  ("no repo snapshot for this chat") then produces exactly the observed
  behavior.
- **Local root cause (confirmed candidate):** `chat.rs` never calls
  `routes::util::expand_workdir` (`util.rs:19`) — every other workdir-taking
  route does (sessions, harness, board_goals, mcp, wiki). A `~/…` repo path
  fails `is_dir()` silently → blind, even for a perfectly valid local repo.
- **SSH root cause (by construction):** `Repo.path` for a remote project is a
  path on the remote host; a local `is_dir()` can never see it.
- **Server can't even try:** the client sends only the raw `workdir` string.
  `ChatRequest` (`chat.rs:120`) has no repo/host identity, and
  `chat-store.ts::sendChatMessage` receives `repoId` but does **not** pass it
  to `streamChat` (`chat-store.ts:219` sends `workdir` only). So the server
  has no way to resolve the repo's host and gather remotely.
- The UI pinning itself is correct: `ProjectHubPage.tsx:180` renders
  `<ChatPage pinnedRepo={repo} />`; pinned `ChatPage` resolves
  `workspace = pinnedRepo` and sends `workdir: workspace?.path`
  (`ChatPage.tsx:302`). Don't re-suspect a missing param / separate route.

## Acceptance criteria

1. **Local `~` paths ground.** `gather_repo_context` (or its new wrapper)
   expands the workdir via `routes::util::expand_workdir` before the dir
   check: a unit test asserts a `~`-prefixed path to a real repo fixture
   returns `Some` with guide + file tree (and the existing
   `gather_repo_context_reads_guide_and_manifests` test still passes).
2. **Pinned requests carry repo identity.** `ChatRequest` accepts an optional
   `repo_id`; the pinned `ChatPage` → `chat-store` → `streamChat` path sends
   it on both `/api/chat` and `/api/chat/stream`. A vitest asserts the request
   body of a pinned-mode send includes both `workdir` and `repo_id`; absent
   `repo_id` deserializes as `None` (old clients unchanged).
3. **SSH projects ground.** When `repo_id` resolves (via
   `repos::load_host_for_repo`) to an SSH host, the server gathers context
   over the existing host runtime (guide file + root manifests +
   `git ls-files` tree, same section headers and budgets as the local arm) and
   the system prompt contains the grounded repo block, not the blind access
   rule. Unit-testable seam: the remote arm is a pure function of the fetched
   file bodies + tree text.
4. **Failure is loud, never silent.** Every `/api/chat/stream` request logs the
   received `workdir` + gather outcome (`Some(len)`/`None` + reason). When a
   request carries `repo_id` but gathering returns `None`, the stream emits a
   `context` SSE event before the first token and the pinned `ChatPage`
   renders a visible warning banner — the model's system prompt still gets the
   honest-blind rule, but the user sees the wiring problem, not an apology.
5. **A remote-gather failure never breaks chat.** SSH gather errors/timeouts
   degrade to criterion 4's warning path; the reply still streams. (No test
   may leave chat hanging on a wedged SSH connection — the gather is bounded
   by a timeout.)

## Scope & non-goals (YAGNI)

- **In:** local `~`-expansion fix; `repo_id` threading; host-aware context
  gather (one remote arm); context-status SSE event + pinned-chat warning
  banner; the diagnostic log line; regression tests.
- **Out (explicitly):**
  - Issue **#360** (per-project tracker binding / sidebar Board removal) — a
    separate p2 feature, separate spec.
  - Wiki-RAG changes (`wiki_rag.rs`) — retrieval keying stays as is.
  - Any remote-context **caching** layer (gather per turn; note a follow-up if
    latency demands it).
  - Live file access / command execution from chat — the snapshot stays
    static; the access-rule wording (byte-pinned by spec 008) is untouched.
  - Un-pinning or re-designing the hub chat UI beyond the warning banner.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- `gather_repo_context` (`routes/chat.rs:235`) — the local reader (guide,
  `.harness/*`, manifests, git tree, budgets). Becomes the **local arm**;
  section format and budgets are the contract the remote arm mirrors.
- `routes::util::expand_workdir` (`routes/util.rs:19`) — the `~`/relative
  expansion every other route already uses. Apply it; don't re-implement.
- `repos::resolve_repo_host_id` (`routes/repos.rs:350`) +
  `repos::load_host_for_repo` (`routes/repos.rs:363`) — repoId → `Host`
  (local host when `host_id` absent). Already `pub(crate)` for cross-route use.
- `host_runtime::ssh_stdout(host, script)` (`host_runtime.rs:318`) and
  `host_runtime::read_remote_file` (`host_runtime.rs:298`) — the remote
  transport. Prefer **one** `ssh_stdout` script (single round trip) that cats
  the guide/manifests with delimiters and runs `git ls-files`.
- `intake_grounding_blocks` (`routes/chat.rs:308`) — consumes
  `Option<repo_context>`; **unchanged**. The Fast-mode byte-identical pin
  (spec 008 AC 6) depends on these block strings — the fix only changes
  whether `repo_context` is `Some`, never the strings.
- SSE delta protocol (`chat-client.ts:35` `ChatStreamDelta`, server's compact
  one-line `data:` events) — extend with one new variant; the parser already
  frames on blank lines and ignores nothing silently (`error`/`done` handled).
- `ChatPage` pinned mode (`ChatPage.tsx:108/194/302`) and
  `chat-store.sendChatMessage` (`chat-store.ts:153`) — the send path; add
  `repoId` pass-through, don't restructure.

### Build new

- `ChatRequest.repo_id: Option<String>` (serde-default) + threading in
  `chat-client.ts` (`sendChat` + `streamChat` bodies) and
  `chat-store.ts` (`streamChat(history, { …, repoId: opts.repoId })`).
- A host-aware gather wrapper (e.g. `gather_repo_context_for(state, workdir,
  repo_id)`): local host → existing local arm (now `expand_workdir`-first);
  SSH host → remote arm building the same sections from one scripted fetch,
  bounded by a timeout.
- `ChatStreamDelta::context` SSE event (`{ type: "context", state: "ok" |
  "missing" }`, emitted only when the request carries `repo_id`) + a warning
  banner in pinned `ChatPage` driven by it.
- The one-line diagnostic log in `/api/chat/stream` (workdir received, arm
  chosen, outcome).

## Risks & invariants

- **Spec 008 Fast byte-identical pin.** `intake_grounding_blocks` strings are
  pinned by test; this spec must not edit them. Risk: a well-meaning reword of
  the blind rule breaks the pin — the fix is upstream (make `repo_context`
  `Some`), never in the block text.
- **Wedged SSH must not hang chat** (v0.56.1 lesson: wedged ControlMaster).
  The remote gather gets a hard timeout and failure degrades to the warning
  path (AC 5); the reply always streams.
- **No new public routes.** `/api/chat*` stays behind `require_token`; the
  remote arm reuses the authed store/host plumbing — nothing added to
  `auth.rs::is_public`.
- **Don't poll, don't loop.** One gather per turn. No background refresher,
  no per-keystroke fetches.
- **Old clients keep working.** `repo_id` is serde-default; the `context`
  event is additive (unknown SSE types are already skipped client-side —
  verify, else guard).
- **`load_host_for_repo` reads the repos registry** — confirm it's reachable
  from chat.rs without a layering violation (it's `pub(crate)` in the same
  routes tree; architect to confirm no cycle).

## Harness wiring (the gate)

- **feature_list.json entries:**
  1. `local-workdir-expansion` — `expand_workdir` in the chat gather + the
     diagnostic log line + unit tests (AC 1, log half of AC 4).
  2. `repo-id-threading-and-ssh-gather` — `ChatRequest.repo_id`, client
     threading, host-aware gather with remote arm + timeout (AC 2, 3, 5).
  3. `blind-context-warning` — `context` SSE event + pinned ChatPage banner
     (AC 4 UI half).
- **`verify.sh` asserts:** `cargo test -p agentum-server --lib` green
  (new: tilde-expansion test, remote-arm pure-function test, `repo_id`
  serde-default test, context-event emission test) + `bunx vitest` green
  (new: pinned send body includes `workdir`+`repo_id`; banner renders on
  `context: missing`) + `npm run build --prefix crates/agentum-desktop/ui`.
- **`qa.sh` asserts (browser QA):** open a local project's hub chat, ask a
  codebase question → answer references real files (no "no workspace"
  apology); simulate a bad workdir → warning banner visible. SSH leg is a
  human/staging check (needs a live SSH host).

## Open questions

1. **Remote arm transport:** one `ssh_stdout` script with delimiters (single
   round trip, recommended) vs. N `read_remote_file` calls (simpler, chattier
   over SSH)? Architect decides; spec only fixes the output contract.
2. **Should the un-pinned chat with a selected workspace also send `repo_id`?**
   Recommended yes (same send path, `workspaceId` is in hand) — it makes
   SSH-repo chats work outside the hub too at zero extra cost. Confirm at
   architect phase.
3. **Remote gather timeout value** — propose 10 s (matches the input-write
   timeout family); tune at build time.
