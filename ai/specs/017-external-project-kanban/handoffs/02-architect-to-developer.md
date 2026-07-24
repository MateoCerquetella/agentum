# Handoff: Architect → Developer — 017a (GitHub Project v2 read-kanban)

**From:** sdd-architect · **To:** sdd-developer · **Spec:** 017a · 2026-06-23

## State
`spec.md` + `architecture.md` written; **architect gate PASSED (5/5)**.
017a = **GitHub only, read-only, live view** (fetch the project board; do NOT
persist its columns/cards into `board_items`).

## Build (architecture.md build order)
1. `github_graphql(token, query, vars)` helper (reuse `token_for(Github)`; mirror `linear.rs::graphql`; manual JSON body, no reqwest `json` feature).
2. `projectV2` query consts + **pure** `parse_project_columns` / `parse_project_items` → `ProjectBoard { columns:[{id,name}], cards:[{id,title,url,column_id}] }`, with unit tests on a sample response.
3. `GET /api/board/projects/board?binding_id=<id>` (auth-gated): resolve binding → `github_graphql` fetch → `ProjectBoard`. **Fail loud** on GraphQL error / missing `read:project` scope / project-not-found (non-2xx + message).
4. `runtime/board-projects-client.ts` (mirror `board-client.ts`) — `getProjectBoard(bindingId)` + types.
5. `components/board/ProjectKanbanBoard.tsx` — **dynamic columns** from the fetched board (don't reuse the fixed todo/doing/done); bind-project input + Refresh; card → GitHub deep-link; wire into `BoardPage`/nav.
6. Verify: `cargo test -p agentum-server --lib` green; `bun run build` green.

## Boundaries (do NOT cross)
- No `board_items` writes / no 016 reconcile / `/api/board/sync` untouched.
- **Reuse `board_tracker_bindings`** — `provider="github_project"`, `project="<owner>/<number>"`. **No new migration.**
- Read-only (no write-back → 017c); GitHub only (Linear → 017b).
- Build in an **isolated worktree off `origin/develop`** (the 016 pattern); commit per slice; **do NOT push** (promotion human-gated).

## Risks to honor
`read:project` scope (fail-loud + clear msg) · org-vs-user project (try `organization` then `user`) · Status-field discovery (first single-select / "Status"; documented) · item pagination (cap 100 + `log`, no silent truncation) · dynamic-column UI must be **additive** (don't break the internal board).
