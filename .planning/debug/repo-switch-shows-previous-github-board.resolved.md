# Repo switch shows previous GitHub board

## Symptoms

Opening Agentum's bound GitHub Project and switching the Project Hub to the
unbound Freebee repo did not show Freebee's project picker/empty state.

## Root cause

The per-repo resolver correctly returned `{ source: 'none', project: null }`
for Freebee, but `TaskPage` translated that result to `githubMode = 'items'`.
That hid the repo-scoped `ProjectViewWrapper` instead of allowing it to render
the honest unbound state, making the switch appear to retain the prior board.

Live-state evidence confirmed that only Agentum has a server binding and the
legacy desktop setting still points at Agentum's project; Freebee has neither.

## Resolution

Embedded Project Hubs now stay in Project mode for both bound and unbound repo
resolutions. Bound repos render their board; unbound repos render their picker.
A regression test models the Agentum-bound → Freebee-unbound transition.

## Verification

`embedded-github-mode.test.ts` and `board-project-resolution.test.ts`: 17 tests
passed, 0 failed. `git diff --check` passed.
