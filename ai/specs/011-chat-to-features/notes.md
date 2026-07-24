# 011 Chat-to-Features — implementation notes

Tracking issue: https://github.com/MateoCerquetella/agentum/issues/19

Worktree: `.claude/worktrees/011-chat-to-features` (branch `feat/011-chat-to-features`).
Built under a Ralph loop. Commits **6e1601b** (011a), **46bbbaf** (011b), **212eb38** (011c). NOT merged to main.

## Done

### 011a — foundational vertical slice (commit 6e1601b)
- `crates/agentum-server/src/task_sink.rs` (NEW): `TaskSink` closed-enum seam
  (style matches `mcp_provision::BrowserMcpEngine` — no `async-trait` dep),
  `NewFeature` / `FeatureRef` / `SinkCtx`, `BoardSink` `create_feature` (creates
  a `feat` card in `todo`), `select()`.
- `harness::write_backlog_from_features(workdir, &[(id,name,description)])`:
  derives a `Pending` `feature_list.json` from tracker features and leaves the
  harness **Idle** (human-gated Run). Rejects empty input + duplicate/blank ids.
- `POST /api/board/goals/{id}/harness-plan`: reads a goal's planner-produced
  child cards → writes the harness backlog; emits `goal.harness.planned`.

### 011b — GitHub sink + agnostic selection (commit 46bbbaf)
- `TaskSink::Github` → `gh issue create` (pure `gh_create_argv` /
  `parse_gh_issue_url`, unit-tested; live path is an `#[ignore]` test).
- Pure `TaskSink::pick_provider(github_available)` policy (external = source of
  truth, board = fallback) + async `select()` probing `gh repo view`.
- `harness-plan` endpoint now selects the sink: **board** reuses the planner
  card keys; an **external** sink mirrors each card out and uses the tracker id
  as the harness feature id. Response + event carry the chosen provider.

### 011c — Linear sink (commit 212eb38)
User-approved approach: "read desktop creds + sole-team".
- `crates/agentum-server/src/linear.rs` (NEW): reads the desktop's
  `<data_local_dir>/Agentum/linear.json` token (added `dirs = "6"`;
  `AGENTUM_LINEAR_CREDS` overrides the path for tests), resolves the
  workspace's **sole** team via a `teams()` query (errors on zero/many — never
  guesses), and creates issues via the GraphQL `issueCreate` mutation. Pure
  helpers `pick_token` / `parse_team_id` / `parse_issue_create` are unit-tested;
  the live API path is runtime-only (needs real Linear creds).
- `TaskSink::Linear` variant + `create_feature` arm; `pick_provider(github,
  linear)` precedence **github > linear > board**; `select()` honors
  `AGENTUM_TASK_SINK=board|github|linear` to pin a provider (also keeps the
  endpoint tests hermetic).

Verification: `cargo test -p agentum-server --lib` → 258 pass / 0 fail / 5 ignored;
`cargo clippy -p agentum-server --lib -- -D warnings` clean.

## Also pending
- Desktop UI trigger: `planGoalHarness()` client method added (commit 06623d3),
  but **there is no desktop surface that renders board goals** — the planner
  "New Goal" → cards flow is TUI/board-API only; `WorkspaceKanbanDrawer.tsx` is
  a *worktree* kanban, and the harness `ChatPage` shows harness features (no
  goal id). The button needs a board-goals view built first (a separate,
  larger piece). The pipeline is usable today via API / TUI / MCP.
- Merge `feat/011-chat-to-features` → main (human-gated).

## Deferred — 011d
Bidirectional status sync (external ↔ board ↔ harness) + GitLab sink + harness
re-plan idempotency (re-running an external sink currently re-creates issues).
