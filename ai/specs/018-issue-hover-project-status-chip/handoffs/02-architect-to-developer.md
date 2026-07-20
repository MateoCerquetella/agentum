# Handoff 02 — Architect → Developer (spec 018)

- **Blueprint:** `ai/specs/018-issue-hover-project-status-chip/architecture.md`
- **Self-check:** PASS — all cites re-verified on this worktree (develop
  `d31314b3`); collision sweep clean (nothing built yet).

## Open questions — RESOLVED (build against these)

- **Q1 read path → desktop Tauri command** `gh_issue_project_status` beside
  `gh_get_project_view_table`. Not a server route (gh auth + popover are
  desktop-side; server route buys nothing since unreadable = silent no-chip).
- **Q2 binding source → fresh `getProjectBinding`, cached per slug** for the
  app session. Not Project Hub store reuse (unreliable until that page opens).

## Build order (four commits, §5)

1. Rust command `gh_issue_project_status` + **pure** `issue_project_status`
   mapper (`fn(&Value, &str, &str) -> Option<String>`) + `#[cfg(test)]` cases
   + `lib.rs` registration.
2. Tauri client: `gh.ts` `issueProjectStatus` + `contract.ts` type.
3. Pure model `lib/issue-project-status.ts` (`parseIssueRef`, `statusCacheKey`,
   `resolveIssueProjectStatus` — never throws) + `.test.ts`.
4. `IssueProjectStatusChip` + `useIssueProjectStatus` hook + badges-row slot
   (`WorktreeCardMeta.tsx:314–321`) + `workdir`/`repoId` props threaded from
   `WorktreeCard.tsx` (:586, :602; `repo.path`/`repo.id` already in scope) +
   `WorktreeCardMeta.test.tsx` presence/absence.

## Non-negotiables

- **D6 never throw:** `resolveIssueProjectStatus` try/catch → null; chip
  `status == null → return null`. A throw takes the whole hover down.
- **AC 2 silent absence:** unbound / not-on-project / error all → no chip.
- **AC 3 no poll:** fetch only in the `open`-gated effect + module caches. No
  `/api/events`, no interval.
- **Injection:** owner/repo/number stay `$vars` in the GraphQL string.
- **repoId** passed to `getProjectBinding` for SSH-repo bindings (spec 020).

## ⚠️ Build-gate reality (§7)

No `webkitgtk` in this env → `cargo build/test -p agentum-desktop` won't
compile locally. Author the Rust mapper tests (CI-runnable, pure) but verify
locally on the **UI side**: `bun run build` (`crates/agentum-desktop/ui`) +
targeted `bunx vitest run issue-project-status.test.ts WorktreeCardMeta.test.tsx`.
Full vitest + full tsc are a known pre-broken develop baseline — pin the two
targeted files. A local cargo webkitgtk failure is environmental, not a defect.
