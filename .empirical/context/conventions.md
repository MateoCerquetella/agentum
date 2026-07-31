# Conventions

## Code and structure

- Keep ownership in the existing crate/module boundary; update `CLAUDE.md` when
  architecture, crates, primitives, or non-obvious constraints change.
- Rust request contracts use closed typed inputs, deterministic hashes, bounded
  reads, and explicit error variants. Filesystem input is repository-anchored
  and no-follow where security depends on identity.
- The React UI uses typed runtime clients and server capability responses as the
  authority for whether a surface is selectable.
- Add focused unit tests beside pure logic and route/integration tests at the
  mutation boundary. Preserve existing tests and unrelated worktree changes.

## Testing and delivery

- Every repository change starts with a documented, labeled GitHub issue and a
  linked PR into `develop` containing `Closes #<issue>`.
- Promotion order is `develop` to `staging` for QA, then `staging` to `main` for
  release. Versioned, signed annotated tags must point at protected `main`.
- Run formatting, clippy, Rust tests, UI typecheck/tests/build, boundary scripts,
  and relevant policy tests before promotion.

## Repository-specific constraints

- Use a dedicated worktree; never switch branches in a shared checkout.
- SDD run phases are non-delivery. Commits, pushes, PRs, merges, issue updates,
  and releases are explicit delivery actions.
- Never track project-local `.codex`, `.cursor`, `.gemini`, `.opencode`, Hermes,
  or Aider configuration; `scripts/check-sdd-boundary.sh` fails such releases.
- Remote repositories must never fall through to local filesystem, provider,
  Git, or execution behavior.
