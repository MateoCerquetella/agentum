# Handoff 04 — Tester → Reviewer

- **Spec:** 009-pinned-chat-repo-context (#361)
- **Date:** 2026-07-13
- **From:** Tester (autonomous sdd-orchestrate)
- **To:** Reviewer
- **Verdict:** **PASS-WITH-DEFERRALS** — full report in `../verification.md`.

## What I verified (independently, not trusting the dev log)

- **Gates re-run from scratch:** cargo `566/0/5`, `cargo fmt --all --check`
  clean, vitest **15/15** on the three chat suites (the ONLY test files that
  import any touched module — grep-verified), adjacent `socratic-intake` 5/5,
  `vite build` ✓ 1m16s. Every number matches the developer's claims.
- **All 5 ACs** against code with file:line evidence (see verification.md
  table). The full AC-2 wire path was traced end-to-end:
  `ChatPage.tsx:309` → `chat-store.ts:231` → `buildChatStreamBody` → `repo_id`.
- **Sacred invariants:** `intake_grounding_blocks` is SHA-1-identical to base
  `d957eefd`; byte-pin tests green; zero env mutation in new tests; `auth.rs`
  diff is 0 lines; no gather path can return `ApiError` (all soft-None).
- **All 5 architecture probes:** bare-`~` and trailing-slash grounding were
  exercised with a throwaway test (reverted after run, tree left clean);
  `ok`-clears-`missing` and non-stream-no-event are test/grep-verified; the
  stale-`repo_id` fallback is code-verified only (see focus item 1).
- **The 3 documented deviations** are accurately described and harmless
  (audited against `host_runtime.rs` / `capped_tree` / `WarningBanner`).

## Defects found

None.

## What the reviewer should focus on

1. **`gather_repo_context_for` is the one untested function** (needs
   `&AppState`). Its stale-repo_id `Err` arm — warn WITHOUT return, falling
   through to the local arm — is load-bearing for "a deleted host must not
   blind a valid local workdir". I read it as correct; a second pair of eyes
   on that control flow (`chat.rs:~480-510`) is the highest-value review.
2. **The S2 refactor's byte-compat claim** rests on the two pre-existing gather
   tests. They pass, and I audited the truncation layering (per-section budget
   at assemble + final CONTEXT_BUDGET — same two layers as before), but a
   skeptical diff-read of `assemble_repo_context` vs the old inline body
   (`git diff d957eefd..HEAD -- crates/agentum-server/src/routes/chat.rs`,
   hunk at old `:233-298`) is worth 10 minutes.
3. **Remote script quoting** (`remote_context_script`): `shlex::try_quote` on
   the workdir + the whole script re-quoted for `sh -c`. Tested for spaces;
   review for any shell-metachar corner the test misses.
4. Three cosmetic nits in verification.md (untestable glue fn, a test comment
   that overclaims, `is_some()` vs non-empty `repo_id` in the event decision) —
   none blocking, reviewer may fold them into follow-ups.

## Deferred to qa.sh / staging (NOT covered here)

- Live SSH gather against a real host (AC-3/AC-5 end-to-end).
- Browser banner render + live `context` SSE round trip (AC-4 pixel half).

## Recommended next step

Proceed to review. Nothing here blocks the PR; the deferred items belong to
the staging QA checklist, not CI.
