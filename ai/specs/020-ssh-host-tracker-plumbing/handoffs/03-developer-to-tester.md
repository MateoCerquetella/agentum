# Handoff 03 — Developer → Tester

- **Spec:** 020-ssh-host-tracker-plumbing
- **Date:** 2026-07-13
- **From:** Developer (three slice sub-agents, orchestrator-gated)
- **To:** Tester
- **Commits under test:** F1 `09726c46`, F2 `e8fb31a8`, F3 `820712d9`
  (on `fixes-new-workspace`, on top of spec 015's `3ec6f028`; base develop
  `4f98453f`)
- **Artifacts:** `tasks.md` (F1/F2/F3, all deviations numbered),
  `architecture.md`, spec ACs 1–10 (grade against the amended AC 8 text)

## Developer-claimed gates (re-run ALL independently)

1. `cargo test -p agentum-server --lib` — 701/0/5 (F1 +9, F2 +5; F3 holds).
2. `cargo fmt --all --check` + `cargo clippy -p agentum-server --lib --tests -- -D warnings` — clean.
3. `npm run build --prefix crates/agentum-desktop/ui` — green.
4. Targeted vitest — F3 ran 5 files / 53 tests: `create-issue-intent-model`
   (015's 26 cases UNMODIFIED), `github-issue-client`,
   `github-projects-client` (new), `repo-slug-arm`, `start-work-repo-match`.

## Sacred surfaces (assert empty diffs `3ec6f028..HEAD`)

- `components/github-project/start-work-repo-match.ts` (015's classifier).
- `board_goals::resolve_github_slug` + `SlugReason` (old-019 machinery; D5) —
  the function bodies, not the file (board_goals.rs had caller edits? it
  should NOT — F1's report lists no board_goals.rs edits; verify).
- The native Tauri `gh_repo_slug` command (`agentum-desktop/src/commands/gh.rs`).
- `use-tracker-intake.ts`'s provider resolution + `filed`-only-from-confirmed
  logic (threading edits only).
- `lib/repo-slug-index.ts` environment-RPC + native arms byte-identical
  (F2's claim); `slugByRepoId` cache logic unchanged.
- Wizard `trackerWorkdir` gating NOT relaxed (F3 threaded `trackerRepoId`
  only).
- No `is_public` changes; no serde aliases anywhere in the diff.

## Behavior pins to verify by reading tests + code

- F1: unknown `repoId` → 4xx even WITH a valid hint; hint short-circuit =
  zero git I/O; absent `repoId` = byte-identical local behavior (regression
  pins); both duplicate resolvers really deleted (grep `fn resolve_slug` in
  github_projects.rs/provision.rs = gone); provision's `:292` `is_dir` gate
  intact; 422 messages carry the HostUnreachable/NoGithubRemote split, codes
  byte-identical.
- F2: route errors 404/422-`no_github_remote`/502-`host_unreachable`; a
  transport failure can never surface as no-origin; renderer failures →
  `null` cached → repo EXCLUDED (fail-closed); arm order env-RPC >
  server-for-connectionId > native.
- F3: `grounding` always present on the draft response; note derives ONLY
  when `grounding.repo === false`; draft leg threads slug NOT repoId
  (amended AC 8); `filed` still provider-confirmed-only; errors inline.

## Deferred (record, don't fail)

- Live SSH legs (real dyaus binding/filing/Start-work, host-down 502) =
  qa.sh/staging/human. Full vitest (~139 fails) + bare tsc = pre-broken
  baselines; corroborate no NEW failures in touched suites only.

## Expected tester artifacts

`verification.md` + `handoffs/04-tester-to-reviewer.md`, committed as
`docs(sdd): 020 tester verification` (artifacts only, no push).
