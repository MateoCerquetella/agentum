# Verification — Spec 009 pinned-chat-repo-context

- **Spec:** `ai/specs/009-pinned-chat-repo-context/spec.md`
- **Date:** 2026-07-13
- **Tester:** independent re-run (autonomous sdd-orchestrate)
- **Code under test:** `e6b6798f`, `6d457545`, `6ea8e5f9`, `c5b22ce0`, `e56f1fea` on top of base `d957eefd`
- **Verdict:** **PASS-WITH-DEFERRALS** (all CI-runnable criteria green; live SSH
  gather + browser banner render deferred to qa.sh/staging by design)

## Independent gate counts (re-run by the tester, not copied)

| Gate | Command | Tester's result | Dev claimed |
| ---- | ------- | --------------- | ----------- |
| Server tests | `$HOME/.cargo/bin/cargo test -p agentum-server --lib` | **566 passed / 0 failed / 5 ignored** (89.4s) | 566/0/5 ✓ match |
| fmt | `cargo fmt --all --check` | **clean** | clean ✓ match |
| New vitest | `bunx vitest run src/lib/chat-body.test.ts src/lib/chat-context-status.test.ts src/runtime/chat-client.test.ts` | **15/15** (chat-body 4 + chat-context-status 6 + chat-client 5) | 10/10 + 5/5 ✓ match |
| Adjacent vitest | `bunx vitest run src/lib/socratic-intake.test.ts` | **5/5** (unregressed) | n/a |
| UI build | `npm run build --prefix crates/agentum-desktop/ui` (`vite build`) | **✓ built in 1m16s** | ✓ 1m43s |

Vitest baseline note: only 3 test files in the whole UI import any touched
module (`chat-client.test.ts`, `chat-body.test.ts`, `chat-context-status.test.ts`
— verified by grep; `chat-store.ts` / `ChatPage.tsx` have no test files), so the
scoped run covers the full regression surface. **Zero new vitest failures.**

## Per-AC verdicts

| AC | Verdict | Evidence |
| -- | ------- | -------- |
| AC-1 local `~` paths ground | **PASS** | `local_repo_context` runs `super::util::expand_with_home(wd, home).ok()?` BEFORE `is_dir()` (`routes/chat.rs:344-349`); test `local_repo_context_expands_tilde_workdir` (`chat.rs:~2830`) uses an explicit temp home — **no env mutation** (whole 009 diff greps clean for `set_var`; the only `std::env` touch is the production `var_os("HOME")` READ in `gather_repo_context`, `chat.rs:334`). Pre-existing `gather_repo_context_reads_guide_and_manifests` + `gather_repo_context_none_for_missing_or_empty_workdir` still pass. |
| AC-2 requests carry repo identity | **PASS** | `ChatRequest.repo_id` `#[serde(default)]` (`chat.rs:130-131`) + `chat_request_repo_id_is_serde_default` (absent → None). Client path verified end-to-end: `ChatPage.tsx:309` `repoId: workspace?.id` (workspace = pinnedRepo in pinned mode, `ChatPage.tsx:194-197`) → `chat-store.ts:231` `repoId: opts.repoId` → `streamChat` → pure `buildChatStreamBody` (`lib/chat-body.ts`) sends `repo_id`. Vitest `chat-body.test.ts` asserts body includes both `workdir` + `repo_id` and that absent opts keep the pre-009 wire shape (JSON.stringify drops undefined) — pure model test, no fetch mock, per architecture S6. |
| AC-3 SSH projects ground | **PASS (unit seams) / live SSH DEFERRED** | Remote arm exists: `remote_context_script` (pure, `shlex::try_quote`d workdir, `exit 42` cd guard) + `parse_remote_context_output` (pure) + `gather_repo_context_ssh` → ONE `sh -c` round trip via `host_runtime::ssh_stdout`. Both arms feed the SAME `assemble_repo_context` (owns ALL budgets + section headers, `chat.rs:255-303`), so headers/budgets can't drift — proven by `remote_context_output_round_trips_to_snapshot` asserting the local headers (`## Repo guide (…)`, `## Root manifests`, `### Cargo.toml`, `Repo file tree (git-tracked)`) on parsed remote output. Host resolution: `gather_repo_context_for` → `repos::load_host_for_repo` (`repos.rs:363`), `HostKind::Local` → local arm, `Ssh` → remote arm. |
| AC-4 failure is loud | **PASS (logic) / pixel render DEFERRED** | Log line: `log_repo_context_outcome` called from BOTH `chat` (`chat.rs:~822`) and `chat_stream` (`chat.rs:~941`) with workdir/repo_id/arm/context_len/grounded. Event: `context_event_json(repo_id_present, has_context)` returns None without repo_id (test `context_event_only_for_repo_backed_requests`); yielded FIRST in the stream generator (`chat.rs:1036-1041`), before the redacted-thinking notice. Banner: `ChatPage.tsx:655-657` renders `WarningBanner` from `chat.contextMissing[activeId]`; store sets it via `applyContextDelta` on the `context` delta (`chat-store.ts:240-245`). |
| AC-5 remote failure never breaks chat | **PASS (unit seams) / live wedged-SSH DEFERRED** | `SSH_CONTEXT_TIMEOUT = 10s` (`chat.rs:~371`); `gather_repo_context_ssh` wraps the one call in `tokio::time::timeout`; ALL failure legs (transport Err, non-zero exit incl. `exit 42`, timeout) → `tracing::warn!` + `None` — grep of `local_repo_context` / `gather_repo_context_ssh` / `gather_repo_context_for` finds **zero** `ApiError` returns; the handler proceeds and the reply streams regardless. Inner `ssh_stdout` timeout is 12s so the outer 10s bound dominates. |

## Sacred invariants (all verified)

- **`intake_grounding_blocks` byte-identical to base:** SHA-1 of the function
  body at `d957eefd` vs HEAD → `79c1d995…` both. The 009 diff never mentions the
  function or its literals. Byte-pin tests green in the run:
  `interviewer_is_honest_blind_when_no_context` (base `chat.rs:2332` area),
  `socratic_stage_reuses_the_shared_grounding_blocks` (base `:2470` area),
  `interviewer_grounds_when_context_present`.
- **No env/HOME mutation in any new test:** diff greps clean for
  `set_var`/`remove_var`; the tilde test passes an explicit temp home
  (the S1 seam, `util.rs:27` `expand_with_home` now `pub(crate)`).
- **No new public routes:** `git diff d957eefd..HEAD -- crates/agentum-server/src/auth.rs`
  is 0 lines; `is_public` untouched.
- **Gather is soft-None everywhere:** expansion errors → `.ok()?`; SSH
  errors/timeout → warn + None; stale `repo_id` → warn + local-arm fallthrough.

## Architecture probes (tester-run)

| Probe | Result | How |
| ----- | ------ | --- |
| Bare `~` workdir grounds | **PASS** | Throwaway test (run then reverted, not committed): `local_repo_context(Some("~"), Some(tmp_home))` with CLAUDE.md at home root → Some containing the guide body. Also backed by pre-existing `util.rs::expands_bare_tilde`. |
| Trailing-slash workdir grounds | **PASS** | Same throwaway test: `~/proj/` and `/abs/path/` both ground (`expand_with_home` trims, `util.rs:29-33`). |
| Stale `repo_id` falls back to local arm | **PASS (code-verified)** | `gather_repo_context_for`'s `Err(e)` arm warns WITHOUT returning and falls through to the local-arm tail (`chat.rs:~490-505`). Covers both unknown repo (`resolve_repo_host_id` NotFound) and deleted host (`load_host_for_repo` BadRequest). No direct unit test — needs a full `AppState`; see nits. |
| `context: ok` clears a prior `missing` | **PASS** | `applyContextDelta` vitest "sets on missing and clears on ok" + independence across conversations; store wiring clears on send (`clearContextMissing`, `chat-store.ts:221`) AND applies deltas (`:242`). |
| Non-stream `/api/chat` grounds, no event | **PASS** | `chat` handler calls `gather_repo_context_for` + logs (`chat.rs:~822`); `context_event_json` is referenced ONLY inside `chat_stream` (`chat.rs:1030`, `:1039`) — grep-verified. |

## Deviation audit (3 documented in tasks.md — all accurate, all harmless)

1. **Whitespace-only tree → None** (`capped_tree`, `chat.rs:311-315`): accurate.
   Old code returned None only for zero-line output; a pathological
   whitespace-only `git ls-files` would have rendered a header-only tree
   section. Tested (`remote_context_output_round_trips_to_snapshot`'s empty-tree
   assertion). Behavior change is confined to that pathological input — harmless.
2. **`ssh_stdout` + `tokio::time::timeout` instead of `ssh_output`:** accurate.
   `ssh_output` lives in `agentum_tmux::ssh` and is `use`d, not re-exported, by
   `host_runtime` (`host_runtime.rs:21-26`) — not reachable from `routes/`. The
   architecture named this exact fallback (S3). Bonus: `ssh_stdout` errors on
   non-zero exit (`host_runtime.rs:318-329`), which is what makes the script's
   `exit 42` a clean None.
3. **No banner dismiss:** accurate — `WarningBanner` (`ChatPage.tsx:1332-1339`)
   has no dismiss; the flag self-clears on the next send and on `context: ok`.
   `ErrorBanner` keeps its dismiss. Reasonable: hiding live server state by hand
   would hide a real problem.

## Defects

**None.** No red gate, no failed AC, no invariant violation.

## Nits (non-blocking, for the reviewer)

1. **`gather_repo_context_for` has no direct unit test** — it takes `&AppState`
   so the stale-repo_id fallthrough is code-verified only. A future seam
   (e.g. taking `Result<Host, ApiError>` instead of resolving inside) would make
   it a pure test target. Not worth blocking: the function is 30 lines of glue
   and every branch is covered by tested collaborators.
2. **Test-comment overclaim** in `remote_context_output_round_trips_to_snapshot`
   (`chat.rs:~2884`): the comment says "A tree-only output (empty repo dir)
   still grounds on the tree alone" but no assertion exercises a tree-only-Some
   case (only the empty→None edge). The claim is true by code reading; the
   comment just isn't backed by its own assertion.
3. **`context_event_json` keys off `repo_id.is_some()`** while
   `gather_repo_context_for` filters empty/whitespace `repo_id` — a client
   sending `repo_id: ""` would get an event for a request treated as
   repo-less. Our client can never send it (undefined is dropped by
   JSON.stringify — asserted in `chat-body.test.ts`), so purely theoretical.

## Deferred (by design — qa.sh / staging, not CI-runnable)

- Live SSH gather against a real remote host (AC-3/AC-5 end-to-end; unit seams
  fully covered).
- Browser render of the warning banner + live SSE `context` round trip (AC-4
  pixel half).
