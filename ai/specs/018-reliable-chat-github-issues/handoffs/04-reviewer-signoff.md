# Reviewer Sign-off — Spec 018 (DONE)

**Date:** 2026-06-23 · **From:** Reviewer · **Verdict:** ✅ **SIGN-OFF (DONE)** · **Commit:** `58a406e` (worktree `/private/tmp/agentum-work`, branch `spec/reliable-chat-github-issues`)

---

## Reviewer gate (DONE) — all pass

| Gate item | Y/N | Reason |
|---|---|---|
| All AC pass per Tester (6/6) | Yes | Handoff trail 01→02→03 intact; each AC maps to a real test/code site; Tester reproduced 347/0/4 + green UI build (not re-run, per role). |
| No spec risk R1–R6 unaddressed | Yes | R1/R2 accepted+documented; R3 → typed `gh_failed`/`classify_gh_stderr`; R4 → `tokio::process` async preserved; R5 → **zero** `planner_session_id` UI consumers (grep); R6 → Board fallback `Ok(provider:"board")` + explicit UI copy. |
| Code maintainable, no dead/commented code | Yes | `create_feature_for_goal` clean Local/Ssh match; `map_sink_error`/`classify_gh_stderr`/`create_error` small + why-commented; `ApiError::Custom` used once per error, no double-wrap; groks in <10 min. |
| `gh_in_dir` mirrors `git_in_dir` without drift | Yes | Same `q()` quoting, same `ssh_output(host,&script,GIT_TIMEOUT)`, same `map_ssh_io`; only intentional delta `current_dir(cwd)` (gh has no `-C`), documented. |
| No undocumented technical debt | Yes | Dormant `spawn_planner_session` (`#[allow(dead_code)]`, right call per §5 Non-goals) + vestigial account picker both disclosed; `board_failed` falls through to `default → err.message` (shown, not swallowed). |
| `tasks.md` honest + handoff trail intact | Yes | tasks.md S1/S2/S3 match committed reality; all four handoffs present + consistent. |

## Decisions on the Tester's 3 observations
1. **Vestigial account picker → KEEP + DOCUMENT** (no send-back). Correctness-neutral dead UX; removing it is surgery outside spec 018's scope. Documented follow-up.
2. **Stale-`host_id` window → acceptable.** `backfillRemoteRepoHostIds` runs on store load; worst case fails loudly as `not_github_repo` (AC-3-compliant).
3. **`goal.created` after `board.created` → not a bug.** Two distinct event kinds, deliberate (watchdog filtering; commented at `board_goals.rs:158`).

## Non-blocking follow-ups (documented, NOT ship-blockers)
1. Remove the vestigial account picker in a small UI follow-up (`ChatPage.tsx` account-option state + `:483–498` `<select>` + `accountSwapPending` banner).
2. Optional: add an `#[ignore]` live-SSH smoke test for `create_github_issue_remote` mirroring `github_sink_creates_a_real_issue`, giving AC-6's remote leg the same live documentation as the local path.

## User value
Spec 018 delivers its promise: describing a feature in desktop **Chat** now **deterministically** creates a real GitHub issue server-side via `TaskSink` (or a typed, actionable error) in one round trip — the autonomous planner is off the critical path, so the "planning…" hang is gone.

## Release note (HUMAN-GATED — outside the autonomous loop)
Work is committed on `spec/reliable-chat-github-issues` (`58a406e`). Promotion
`spec branch → develop → staging → main` is a human decision and was NOT
performed by the loop. The two follow-ups above can be folded into that PR or a
later one.
