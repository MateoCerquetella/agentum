# Handoff 03 — Developer → Tester (spec 018)

Built the four commits (one cohesive slice):
1. `gh_issue_project_status` desktop command + pure `issue_project_status`
   mapper + 4 `#[cfg(test)]` cases + `lib.rs` registration.
2. `gh.ts` `issueProjectStatus` + `contract.ts` type.
3. `lib/issue-project-status.ts` (`parseIssueRef` / `statusCacheKey` /
   `resolveIssueProjectStatus`, never-throws) + `issue-project-status.test.ts`.
4. `IssueProjectStatusChip` + `useIssueProjectStatus` hook + badges-row slot +
   `workdir`/`repoId` threaded from `WorktreeCard` + `WorktreeCardMeta.test.tsx`.

Local gate: UI `bun run build` green; targeted vitest — all new tests pass
(13); standalone tsc on the pure model green; `cargo fmt --check` green.
`cargo check -p agentum-desktop` env-blocked on webkitgtk (Rust tests CI-gated).
The 1 red vitest case (`review`/"PR #456") is pre-existing on develop.
