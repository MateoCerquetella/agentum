# New workspace → "Start a session" picker (no auto-launch) + tmux toggle

## Problem
Adding a new workspace auto-launches an agent (defaults to Claude). The user
wants creation to never launch an agent: create the worktree, select that branch
in the sidebar, and land on the existing "Start a session" picker
(`WorkspaceAgentLauncher`). The "Run in tmux (persist)" toggle should also be on
that picker.

## Changes
1. **`lib/worktree-activation.ts`** — add opt `skipCreatedAgentStartup?: boolean`.
   When true, the createdAgent reopen fallback (`buildCreatedAgentReopenStartup`)
   is bypassed so a freshly-created worktree does not relaunch its
   `createdWithAgent`. Reopen-from-sidebar behavior is unchanged.

2. **`hooks/useComposerState.ts`** (`submit` + `submitQuick`) — stop building the
   agent `startupPlan`; stop passing `startup`; remove `ensureAgentStartupInTerminal`
   calls. Still activate + reveal the worktree (branch selected, user lands on it)
   with `skipCreatedAgentStartup: true`. Keep `setup`/`defaultTabs`/`issueCommand`
   (repo config, not the agent). A plain repo → zero tabs → picker renders.
   - Preserve a typed agent prompt: stash it as a pending draft keyed by worktreeId
     so the picker delivers it when the user chooses an agent (no silent loss).

3. **`components/WorkspaceAgentLauncher.tsx`** — add a "Run in tmux (persist)"
   checkbox bound to `getPersistTmuxDefault()` / `setPersistTmuxDefault()` (the
   same persisted default `createTab` already reads). Hidden on web clients.
   Consume the pending draft prompt when launching an agent.

## Non-goals
- No change to reopen-from-sidebar auto-launch of `createdWithAgent`.
- No removal of the composer agent dropdown or the "Don't start a session" checkbox.

## Verification
- `npm run build` + `tsc` typecheck + vitest (UI), accounting for known pre-existing
  noise (see memory: desktop-ui-verification-gotchas).
