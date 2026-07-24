# Architecture Notes — 008a (Browser Verification Loop, Local)

> Grounded by a codebase sweep of three subsystems: the Agents & Automation UI,
> the orchestration/session-launch mechanism, and the issue-tracker integration.
> Key constraint discovered: **agentum runs no MCP and no loop** — the loop is a
> Claude-Code skill the agent runs; agentum installs + launches it and handles result
> reporting. The in-pane `agentum` binary + `AGENTUM_API_URL` are **local-only**, which
> is why this spec is local and 008b owns remote.

## Components

**A. Launch surface (desktop UI)** — a new subsection on Agents & Automation.
- NEW `crates/agentum-desktop/ui/src/components/settings/BrowserVerificationLoopPane.tsx` — installs the skill via the existing `AgentSkillSetupPanel.tsx` + a "Launch verification loop" action (choose/confirm a session+repo, pick the **linked issue**, set a stop cap, kick off).
- NEW `crates/agentum-desktop/ui/src/lib/browser-verification-loop-install-command.ts` — the skill-install command constant (pattern: `orchestration-install-command.ts`).
- NEW `crates/agentum-desktop/ui/src/components/settings/browser-verification-loop-search.ts` — Cmd+J search entries (pattern: orchestration search entries).
- EDIT `crates/agentum-desktop/ui/src/components/settings/Settings.tsx` (the `agents-automation` pane, ~837–876) — render the new pane beside `OrchestrationPane`/Computer Use.
- EDIT `crates/agentum-desktop/ui/src/hooks/useSettingsNavigationMetadata.ts` (~100–112) — add the search entries to the `agents-automation` section.
- REUSE `AgentSkillSetupPanel.tsx`, `useInstalledAgentSkill()`, and `lib/linked-work-item-context.ts` (already source-prefixes issue context against prompt injection).

**B. The agentic skill (loop logic — authored content, not Rust)** — NEW `browser-verification-loop` `SKILL.md`, installed to `~/.claude/skills/…` and discovered by `crates/agentum-desktop/src/commands/skills.rs`. The skill owns: a **preflight** (are the Playwright MCP tools present? else fail), the per-task in-browser drive, the **stop condition** (max iterations / task budget), per-task **evidence capture** (Playwright screenshot/snapshot), and the structured result emit. Developer authors it; agentum never reimplements the loop.

**C. Result → issue comment (backend — the net-new lift)**.
- EDIT `crates/agentum-desktop/src/commands/gh.rs` — unstub `gh_add_issue_comment()` (and `gh_add_issue_comment_by_slug()`) via the already-authed `gh issue comment <n> --body <…>`.
- EDIT `crates/agentum-desktop/src/commands/linear.rs` — unstub `linear_add_issue_comment()` via a GraphQL `commentCreate` mutation using the token in `linear.json`.
- Verify both commands are registered in the desktop Tauri command list (`crates/agentum-desktop/src/lib.rs`) and exposed in the generated `tauri/contract.ts`.
- GitLab comment write is **deferred** (spec names GitHub/Linear only).

---

## APIs / Interfaces

**Reused, unchanged** (this is why launch is local/remote-agnostic and 008b is cheap):
- `POST /api/sessions`, `POST /api/sessions/{id}/start`, `POST /api/sessions/{id}/send`, `WS /api/sessions/{id}/stream` (`routes/sessions.rs`).
- `/api/orchestration/*` task create/update/check (`routes/orchestration.rs`, `store/orchestration.rs`) — used as the **agent→agentum structured result channel**, honoring the existing in-pane `agentum orchestration` handoff pattern.

**New (Tauri commands, unstubbed — not new routes):**
- `gh_add_issue_comment(issue_number, body, repo_slug?)`
- `linear_add_issue_comment(issue_id, body)`

No new HTTP/WS server route is required for 008a.

---

## Data Flow

1. User opens **Agents & Automation → Browser Verification Loop**; installs the skill via `AgentSkillSetupPanel` if `useInstalledAgentSkill()` reports it missing.
2. User clicks **Launch**: confirms the target session+repo and the **linked issue** (`linked-work-item-context`), sets a stop cap. Desktop creates one **orchestration task** whose spec carries the task list, the cap, and the issue ref.
3. Desktop starts/reuses the session and `send`s the skill invocation (e.g. `/browser-verification-loop <task-id>`) into the pane.
4. In the pane, the agent runs the skill → **preflight**: if the Playwright MCP tools aren't present, emit a `FAILED` result with the reason (AC#5) and stop. Otherwise, for each task: drive the browser **in-browser**, capture an evidence artifact, enforce the **stop cap** (AC#3).
5. Skill reports via `agentum orchestration task-update <task-id> <completed|failed> --result '{summary, per-task pass/fail, evidence refs}'`.
6. Desktop, watching that task, reads the result and **posts a comment** on the linked issue via `gh_add_issue_comment` / `linear_add_issue_comment` (pass/fail + evidence) (AC#4). It **refuses to post a green result that carries no evidence**.
7. Desktop surfaces final status (and any preflight failure reason) in the pane.

---

## Important Decisions

- **D1 — Skill-as-loop, not a Rust loop engine.** *Chose* an installed Claude-Code skill *over* a native Rust coordinator *because* agentum's model is "primitives + the agent does the work" (architecture_principles: agent-agnostic), and the user already proved the skill approach (Ralph + Playwright MCP, 10 tasks).
- **D2 — Result handoff via orchestration `task-update --result`, not pane-output parsing.** *Chose* the existing structured channel *over* scraping the stream *because* honoring the established in-pane handoff pattern avoids the fragile-parsing footgun that the silent-failure risk warns about.
- **D3 — Desktop posts the comment (unstub the two Tauri commands), not a unified server route.** *Chose* desktop-side posting *because* the **Linear token lives in desktop `linear.json`**, unreachable from the server; a single server route would force duplicating the credential. GitHub stays on the already-authed `gh` CLI; Linear on its stored-token GraphQL.
- **D4 — GitHub comment via `gh issue comment`, not a REST client.** *Chose* the CLI *because* auth is already delegated to `gh` — zero new auth surface.
- **D5 — Evidence required in the comment** (per-task Playwright screenshot/snapshot ref). *Chose* mandatory evidence *because* it is the concrete mitigation for "agent reports green without driving."

---

## Boundaries (what this spec does NOT touch)

- **No changes** to `executor/adapters.rs`, `host_runtime.rs`, `agentum-tmux` — the launch path is reused verbatim (this keeps 008b's remote work additive).
- agentum still **runs no Playwright/MCP** — unchanged.
- **No GitLab** comment write (deferred).
- **No remote-host execution** — the orchestration/`AGENTUM_API_URL` result channel is local-only by construction; 008b owns the remote-result problem.

---

## Risks

- **R1 — Agent reports green without actually driving.** *Mitigation (D5):* the skill must attach per-task Playwright evidence; the desktop **rejects an all-pass result with no evidence** before posting. *Residual (accepted):* evidence could be stale/misattributed — full hardening (e.g. trace assertion) deferred.
- **R2 — Unattended loop runs away.** *Mitigation:* the skill enforces a max-iteration / task-budget stop condition (AC#3); the desktop passes the cap at launch.
- **R3 — Comment-write is net-new for every provider.** *Mitigation:* scope to the two providers the spec names — GitHub first (lowest risk, `gh` CLI), then Linear (GraphQL mutation). GitLab deferred.
- **R4 — Coupling result handoff to orchestration.** *Mitigation / fallback:* if the orchestration dependency proves heavy, a delimited sentinel result block in the pane stream is the documented fallback; orchestration chosen for robustness.
- **R5 — Local-only result channel is a hard boundary for 008b.** *Mitigation:* called out explicitly so the Developer does not bake local-only assumptions (in-pane `agentum` CLI, `AGENTUM_API_URL`) into shared code in a way that blocks the remote slice.
