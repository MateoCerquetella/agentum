# Spec 004 — Tester Verification Report

- **Date:** 2026-07-01 (autonomous /sdd-loop iteration 5)
- **Worktree:** `.claude/worktrees/fix-wiki` (commits `85c48e0d` F1+F2, `02b92794` F3+F4)
- **Overall verdict:** **ADVANCE to reviewer** — all 7 ACs pass; 4 info findings, none blocking.

## Commands run

| # | Command | Result |
|---|---------|--------|
| 1 | `cargo test -p agentum-server --lib` | **494 passed / 0 failed / 5 ignored** (62.96s; ignored = pre-existing live-gh/agent tests) |
| 2 | `… --lib task_sink` | 19 passed / 1 ignored (live-gh) |
| 3 | `… --lib routes::worktrees` | 12 passed |
| 4 | `… --lib surface_tests` | 41 passed |
| 5 | `… --lib routes::github` | 3 passed |
| 6 | `… issue_spec_id_is_traversal_proof` | 1 passed |
| 7 | `NODE_OPTIONS=--max-old-space-size=6144 npm run build --prefix crates/agentum-desktop/ui` | green (`✓ built in 1m 11s`) |
| 8 | `git diff 203d0497..HEAD -- crates/agentum-server/src/auth.rs` | empty — no `is_public` additions |

## Per-AC verdicts

| AC | Verdict | Key evidence |
|----|---------|--------------|
| 1 composer create-issue | **PASS** (unit; chip render = browser QA) | `create_issue_rejects_blank_title` + 2 more; `POST /api/github/issues` thin over `TaskSink::Github` (422 `no_github_repo` on slug miss); `canCreateGithubIssue` renders only when unlinked+local; linkedWorkItem set pre-worktree; failure = inline error, zero state change |
| 2 worktree persists links | **PASS** | 4 tests incl. `linkedPR` alias + old-client defaults + on-disk-key pin (no-alias wipe guard); detected scan emits `"linkedPR"`; registry struct alias-free; both TS layers forward |
| 3 GitHub arm + labels | **PASS** (live flip = browser QA) | Label table exactly `status/todo|in-progress|ready-to-test|done` w/ blueprint colors; remove-set built from the canonical table minus target — can never name `status/qa*`; NO close/state mutation anywhere in the arm (D1); fake-gh test pins 4×ensure + 1×edit |
| 4 transitions at existing points | **PASS** | Commit-attributed diff: `85c48e0d` touches only `transition_tracker` (tracker_url threading; rustfmt reflow); the range's second drive.rs hunk is pre-spec branch work (`05abe6f1`); transition points at spawn/unit-green/QA-green intact |
| 5 best-effort, never Err | **PASS** | All 6 failure paths walked → `Ok(Skipped(reason))`; no `?`, no `Err` construction in the arm; pinned by no-url (3 variants) + fake-gh-failure tests |
| 6 spec scaffold opt-in | **PASS** (unit; toggle flow = browser QA) | Round-trip through the REAL `derive_backlog_from_spec` (checked→Done); control-strip/64KiB/fallback-AC/traversal tests; never-overwrite 400; `scaffoldSpec` default false, gated github.com+local, both submit paths, non-fatal |
| 7 tracker provenance | **PASS** | `plan_from_spec_with_tracker_stamps_provider_and_url` (returned + persisted); `plan_from_spec_delegation_unchanged` pins MCP behavior; route feeds the same URL F1 parses |

## Findings (all Info, none blocking)

1. **GHES issue URLs skip silently in the transition arm** — the parser hard-requires `https://github.com/`; a GHES-hosted issue degrades to `Skipped` per the best-effort contract. Consistent with GitHub-only scope; reviewer awareness.
2. **No handler-level unit test for `spec_from_issue`'s 400 gates** (needs AppState); pure pieces all pinned — same accepted class as the blank-title pure-gate pin.
3. **No dedicated 30s gh-timeout test** (wall-time cost); flows through the identical `Err → Skipped` seam that the fake-gh failure test pins.
4. **Attribution note:** the `203d0497..HEAD` drive.rs diff contains a pre-spec hunk (`inject_prompt` → `send_bytes`, wiki "command too long" fix) — verified NOT spec-004 work.

## Pending downstream (NOT this phase)

Browser QA (qa.sh/staging): AC 1 chip render, AC 6 toggle flow end-to-end,
AC 3/4 live `status/*` label movement on a real issue ending open with exactly
`status/done`.
