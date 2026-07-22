# Handoff 02 — Architect → Developer

- **Spec:** 024-restore-project-colors-in-sidebar
- **Date:** 2026-07-21
- **From:** Architect (autonomous SDD loop, generation 6)
- **To:** Developer
- **Artifact:** `ai/specs/024-restore-project-colors-in-sidebar/architecture.md`

## Gate result

Architect gate: **PASS**. Every component/API boundary exists and was read;
the swatch-vs-glyph tradeoff is resolved; all six ACs map to named tests and
verification commands; risks have concrete mitigations.

## Mandatory developer sequence

1. Treat the current uncommitted Harness patch in `WorktreeList.tsx`,
   `project-header-color.test.ts`, and `worktree-list-groups.test.ts` as the
   implementation candidate. Audit it against `architecture.md`; do not rebuild
   an already green path.
2. Preserve unrelated dirty files, including `.agentum-harness/*`,
   `crates/agentum-executor/src/adapters.rs`, `spec.md`, and generated Vitest
   cache state.
3. Confirm `RepoBadgeMark` is repo-only and consumes normalized
   `repoHeaderColor`; retain the existing `RepoIconGlyph` color prop and
   theme-owned active/selected/drag classes.
4. Run the exact focused Vitest command, the desktop UI build, and
   `git diff --check`. Record fresh results in `tasks.md` and write
   `handoffs/03-developer-to-tester.md`.

No open decisions remain.
