# Handoff 04 — Tester → Reviewer

- **Spec:** 020-ssh-host-tracker-plumbing
- **Date:** 2026-07-13
- **From:** Tester (independent; did not write the code)
- **To:** Reviewer
- **Commits under test:** F1 `09726c46`, F2 `e8fb31a8`, F3 `820712d9`
  (base = spec 015's `3ec6f028`)
- **Full evidence:** `../verification.md`

## Verdict: **PASS-WITH-DEFERRALS** — no defects

All five gates independently reproduced at HEAD: `cargo test -p
agentum-server --lib` **701/0/5**; fmt clean; clippy clean under a forced
recompile; UI build green; targeted vitest **5 files / 53 tests** green
(015's 26 intent-model cases unmodified — 0 deletions in that test file's
diff). ACs 1–10 all PASS (AC 8 graded against the amended text); the only
deferrals are the handoff's own live-SSH legs (qa.sh/staging).

## Sacred surfaces — all proven untouched

Empty `3ec6f028..HEAD` diffs: `start-work-repo-match.ts` (+ test), native
`gh.rs`, **the whole `board_goals.rs`** (so `resolve_github_slug`/`SlugReason`
trivially unchanged), `task_sink.rs`, `auth.rs`. Both duplicate `resolve_slug`
copies really deleted (repo-wide grep = 0). Wizard gate unrelaxed
(`trackerRepoId` uses the byte-same gate). Zero serde-alias / `is_public`
code changes (all 11 grep hits are doc prose). Deviation audit: **15/15
ACCURATE** (F1: 7, F2: 3, F3: 5).

## Reviewer focus points

1. **The §2.3 host-choice subtleties** — the architecture itself flags these
   as the likeliest silent-regression spot. I verified: create/labels `gh`
   stays LOCAL (explicit `resolve_tracker_host(&state, None)` + load-bearing
   "why local" comments), fetch's `gh issue view` runs on the RESOLVED host
   (documented behavior choice — SSH repoId needs `gh` authed on that host).
   Sanity-check you agree with the fetch-on-remote-host product call; it is
   architecture-sanctioned, not a developer improvisation.
2. **Unconditional `repoId` on the intake file leg** (nit 3 in
   verification.md): a stale local repo id now 404s loud where pre-020 the
   workdir would have resolved. Consistent with D1; confirm you're happy with
   the UX for that (rare) stale-state case.
3. **"Byte-identical arms" wording** (nit 1): the env-RPC/native arm *bodies*
   are byte-identical; the shared ternary's condition expression changed to
   route through the pure arm-picker. Equivalent, but the tasks.md phrasing
   slightly overstates.
4. **`create_issue` failure-ordering micro-delta** (nit 4): missing-local-host
   now surfaces at/after slug resolution instead of before it — still always
   before any `gh` call.

## Open risks (all deferred to qa.sh/staging, per the spec's gate split)

- No live SSH proof yet: real dyaus binding, SSH filing, Start-work direct
  launch, and the host-down **502 `host_unreachable`** flavor (the new slug
  route's status is the qa.sh key — wire-distinguishable from the binding
  family's 422 `no_github_repo`).
- `gh`-on-remote-host requirement for SSH-repoId issue *fetch* is untested
  live (needs `gh` installed/authed on the host).
- Full vitest (~139) and bare tsc remain pre-broken non-gates; no touched
  suite regressed (no other suite imports the touched modules).
