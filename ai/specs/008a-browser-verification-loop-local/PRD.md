# PRD — Browser Verification Loop, Local (spec 008a)

> **Autonomous-execution PRD for Ralph mode.** This file is the single source of
> truth. It is self-contained: you do not need to read other files to start, though
> `spec.md` and `architecture.md` in this folder have extra rationale. Work
> top-to-bottom, phase order. Check off each task **in this file** as you complete it.
> Keep the build green at every step.

---

## 0. Ralph loop protocol (read every iteration)

Each iteration:
1. Open this file. Find the **first unchecked `[ ]` task**, respecting phase order (1 → 2 → 3 → 4).
2. Do exactly that one task (or the smallest coherent sub-unit). **Read each target file fresh** before editing — paths/line numbers are from a snapshot and may have drifted; match on real content.
3. **Verify** with the task's verify command(s). The build MUST be green before you check the box. If red, fix what your change broke until green.
4. Edit this file: flip the task `[ ]`→`[x]`, append a one-line note (what you did + build status). Update §1 Progress.
5. Commit: `git add -A && git commit -m "feat(008a): <task-id> <summary>"`. One commit per task, each green. **Do NOT push and do NOT cut a release** until every Phase 1–4 task is `[x]`/`[!]` AND the human explicitly says to (see §7).
6. If a task is **blocked** (ambiguous / needs a decision / balloons), mark it `[!]`, write a `BLOCKED:` note with the specific question, skip to the next unblocked task, keep going. Surface all `[!]` at the end.
7. Stop when every Phase 1–4 task is `[x]` or `[!]`. Then write a final summary and a single line: `READY FOR HUMAN: push + release?`.

**Never** leave the tree red. **Never** touch the §4 keep-list or the pre-existing-WIP files in §4.

---

## 1. Progress (update each iteration)

```
Phase 1 (Backend: issue-comment write):   0 / 3
Phase 2 (UI: Agents & Automation pane):   0 / 3
Phase 3 (Skill + result→comment wiring):  0 / 2
Phase 4 (Local end-to-end verify):        0 / 1
Blocked [!]: none yet
Last green build: (none yet)
```

---

## 2. Mission

Ship the **local** half of the Browser Verification Loop: from **Agents & Automation**, a developer launches an autonomous Claude-Code skill that drives the **Playwright MCP** browser to verify a task list, and the result is posted as a **comment on the linked GitHub/Linear issue** — with **strict evidence** (no all-pass comment is posted without a Playwright screenshot/snapshot per task).

agentum runs **no** browser and **no** loop: it *installs* the skill, *launches* it into a session, and *posts* the result. Remote-host parity is **out of scope** (spec 008b).

Acceptance anchor: reproduce the user's proven localhost run (Ralph + Playwright MCP, ~10 tasks) — but launched from Agents & Automation and reporting to the issue.

---

## 3. Environment & build commands

Repo root: `/Users/mateocerquetella/Developer/projects/agentum`.

```sh
# UI typecheck + build (gate for any ui/src change)
npm run build --prefix crates/agentum-desktop/ui
npx tsc --noEmit -p crates/agentum-desktop/ui          # faster inner loop
npm run test --prefix crates/agentum-desktop/ui -- run # vitest (affected specs)
# Rust shell build (after any crates/agentum-desktop/src/**.rs change)
cargo build -p agentum-desktop
```
If `crates/agentum-desktop/ui/node_modules` is missing, run `npm install --prefix crates/agentum-desktop/ui` once.

---

## 4. HARD BOUNDARIES — do NOT touch

- `crates/agentum-executor/**`, `crates/agentum-server/src/host_runtime.rs`, `crates/agentum-tmux/**` — the session-launch path is reused **verbatim**. (This keeps 008b additive.)
- `crates/agentum-server/**` session/orchestration routes — reuse, do not modify.
- agentum must **not** spawn or manage any MCP/Playwright process — the agent CLI in the pane owns that.
- **No GitLab** comment write (deferred — `gl_add_issue_comment` stays stubbed).
- **No remote-host work** — the orchestration/`AGENTUM_API_URL` result channel is local-only by construction; 008b owns remote reporting. Do not bake remote assumptions out.
- **Pre-existing-modified WIP files — do NOT sweep into your commits** (they are the user's in-flight work, unrelated to 008a):
  `crates/agentum-cli/src/cli.rs`, `crates/agentum-cli/src/commands/hosts.rs`,
  `crates/agentum-cli/src/commands/worktree.rs`, `crates/agentum-server/src/host_runtime.rs`,
  `crates/agentum-server/src/routes/repos.rs`, `crates/agentum-server/src/routes/worktrees.rs`,
  `crates/agentum-desktop/src/lib.rs`, `crates/agentum-desktop/src/menu.rs`.
  **`lib.rs` is the snag:** registering the unstubbed Tauri commands (Task 1c) requires editing `lib.rs`, which is already WIP. Land your one-line registration via a temp-revert (like 007 did) so the user's WIP stays intact, or stage **only** your hunk (`git add -p`). Never commit the whole file.

---

## 5. Global method

- **Read fresh, match content**, not line numbers. One task = one commit = one green build.
- **No new abstractions** (YAGNI). Reuse the Orchestration pane / `AgentSkillSetupPanel` / `gh` CLI / orchestration handoff patterns the architecture names.
- Paths under §6 are relative to `crates/agentum-desktop/ui/src/` unless they start with `crates/`.

---

## 6. Tasks

### PHASE 1 — Backend: post a comment to an issue (the net-new lift)

#### [ ] Task 1a — GitHub: unstub `gh_add_issue_comment`
Implement (currently a `not_available()` stub at ~`crates/agentum-desktop/src/commands/gh.rs:440`, plus `gh_add_issue_comment_by_slug` ~`:524`) by shelling the **already-authed** `gh` CLI: `gh issue comment <number> --repo <owner/repo> --body <body>`. Follow the existing `gh_list_work_items`/`gh_repo_slug` patterns for owner/repo resolution, `gh` invocation, and error classification (return the same auth-aware envelope on `gh` missing / not-logged-in).
**AC:** with an authenticated `gh`, the command posts a comment to a real issue and returns success; unauth returns the classified auth-required envelope (no panic, no silent success).
**Verify:** `cargo build -p agentum-desktop` green. Manual (needs `gh`): note for the human — post to a scratch issue, confirm the comment appears.

#### [ ] Task 1b — Linear: unstub `linear_add_issue_comment`
Implement (currently returns `None` at ~`crates/agentum-desktop/src/commands/linear.rs:662`) via a GraphQL `commentCreate` mutation against `https://api.linear.app/graphql` using the token in `linear.json` (reuse the existing read-path token load + HTTP client in this file). Input: issue id + body. Return success/typed error.
**AC:** with a connected Linear workspace, posts a comment to a real issue and returns success; missing token returns a typed not-connected error.
**Verify:** `cargo build -p agentum-desktop` green. Manual (needs Linear token): note for the human.

#### [ ] Task 1c — Register + expose the two commands
Confirm both commands are in the Tauri `invoke_handler` list in `crates/agentum-desktop/src/lib.rs` and surfaced in the generated `tauri/contract.ts` so the UI can call them. **`lib.rs` is pre-existing-WIP (§4): stage only your registration hunk.** If the stubs were already registered, no `lib.rs` change is needed — just verify the contract.
**AC:** `api.gh.addIssueComment(...)` and `api.linear.addIssueComment(...)` (or the existing names) are callable from the UI per `contract.ts`.
**Verify:** `cargo build -p agentum-desktop` + `npm run build --prefix crates/agentum-desktop/ui` green.

---

### PHASE 2 — UI: Browser Verification Loop pane on Agents & Automation

#### [ ] Task 2a — Install-command constant + search entries
CREATE `lib/browser-verification-loop-install-command.ts` (pattern: `lib/orchestration-install-command.ts`) with the skill-install command. CREATE `components/settings/browser-verification-loop-search.ts` (pattern: orchestration search entries) exporting `BROWSER_VERIFICATION_LOOP_PANE_SEARCH_ENTRIES`.
**AC:** both modules exist and typecheck; install command targets the `browser-verification-loop` skill.
**Verify:** `npx tsc --noEmit -p crates/agentum-desktop/ui` green.

#### [ ] Task 2b — `BrowserVerificationLoopPane.tsx`
CREATE `components/settings/BrowserVerificationLoopPane.tsx` (sibling of `OrchestrationPane.tsx`). Two parts: (1) skill install via `AgentSkillSetupPanel` gated on `useInstalledAgentSkill(...)`; (2) a **Launch** action — pick/confirm the target session+repo, select the **linked issue** (reuse `lib/linked-work-item-context.ts`), set a **stop cap** (max iterations / task budget), then create an orchestration task (spec = task list + cap + issue ref) and `send` the skill invocation into the session.
**AC:** the pane renders; install flow works (reuses the proven panel); Launch is disabled until a linked issue + a session are chosen and the skill is installed.
**Verify:** `npm run build --prefix crates/agentum-desktop/ui` green.

#### [ ] Task 2c — Register the pane on Agents & Automation
EDIT `components/settings/Settings.tsx` (the `agents-automation` pane, ~837–876) to render `BrowserVerificationLoopPane` beside `OrchestrationPane`/Computer Use. EDIT `hooks/useSettingsNavigationMetadata.ts` (~100–112) to include `BROWSER_VERIFICATION_LOOP_PANE_SEARCH_ENTRIES` in the `agents-automation` section so Cmd+J finds it.
**AC:** Agents & Automation shows the new pane; Cmd+J finds "Browser Verification Loop".
**Verify:** `npm run build` green; if a metadata test exists, extend it and run the affected vitest specs.

---

### PHASE 3 — The agentic skill + strict-evidence result→comment

#### [ ] Task 3a — Author the `browser-verification-loop` skill
CREATE the skill (`SKILL.md` installed to `~/.claude/skills/browser-verification-loop/`, discoverable by `crates/agentum-desktop/src/commands/skills.rs`). The skill must:
- **Preflight:** confirm the Playwright MCP tools are present (e.g. `browser_navigate`/`browser_snapshot` exist). If not, emit a `failed` result with the reason and stop — **this is acceptance criterion #5 (fail loudly).**
- **Per task:** drive the browser **in-browser** to execute/verify the task, and **capture a Playwright screenshot/snapshot as evidence (mandatory — strict).**
- **Stop condition:** enforce the max-iteration / task-budget cap passed at launch (#3).
- **Report:** call `agentum orchestration task-update <task-id> <completed|failed> --result '<json: summary, per-task pass/fail, evidence ref per task>'`.
**AC:** running the skill in a repo with a Playwright MCP `.mcp.json` drives the browser, stops at the cap, and emits a structured result with per-task evidence; with no Playwright MCP it emits a `failed` result naming the reason.
**Verify:** manual (note for human) — invoke in a scratch repo; confirm the result JSON shape + evidence refs.

#### [ ] Task 3b — Desktop: enforce strict evidence + post the comment
In the launch flow (Task 2b), watch the orchestration task. On completion: parse the result; **STRICT EVIDENCE GUARDRAIL — refuse to post an all-pass result that carries no per-task evidence** (surface "result rejected: missing evidence" in the pane instead). Otherwise post the pass/fail summary (+ evidence refs) to the linked issue via Task 1a/1b commands by provider. Surface a `failed`/preflight result's reason in the pane (#5).
**AC:** a result with evidence posts a comment to the linked issue (#4); an all-pass result with **no** evidence is **blocked** from posting and the reason is shown; a `failed` result shows its reason and posts no green.
**Verify:** `npm run build` green; manual (needs a linked issue + `gh`/Linear) — note for human.

---

### PHASE 4 — Local end-to-end verify (the acceptance anchor)

#### [ ] Task 4 — Reproduce the 10-task run, locally, end-to-end
With a local session on a repo whose `.mcp.json` defines Playwright MCP and a linked GitHub (or Linear) issue: launch from Agents & Automation, let the loop run unattended to the cap, confirm it drives the browser in-browser, posts one comment with per-task pass/fail + evidence, and that pulling the Playwright MCP server makes it fail loudly (no silent hang, no green).
**AC:** all five 008a acceptance criteria observed locally (launch · in-browser drive · unattended+stop cap · comment with evidence · fail-loud). **This task is mostly human-driven (live GUI + real issue) — record results; mark `[!]` the parts the loop can't drive headless.**
**Verify:** end-to-end checklist filled in this file.

---

## 7. Definition of done

- Phases 1–3: code complete, each task one green commit; UI + `cargo build -p agentum-desktop` green.
- Phase 4: local end-to-end observed (or its un-automatable parts `[!]` with notes for the human).
- Strict-evidence guardrail enforced (Task 3b): no all-pass comment without per-task evidence.
- Pre-existing-WIP files (§4) untouched / excluded from every commit.
- A final summary listing every `[x]` and `[!]`, then: `READY FOR HUMAN: push + release?`
- **Push + release happen only on explicit human go-ahead** (see §8), never inside the loop.

## 8. Push & release (human-gated, OUTSIDE the loop)

`ai/` is **gitignored** — this PRD and the spec docs are **not** committed by the loop; only the 008a *code* commits are. When the human approves:
- Push the 008a branch/worktree.
- A "release" cuts a new version tag → triggers `.github/workflows/release.yml` (the multi-platform desktop build). Only do this once 008a code is green **and** the human confirms a release is actually intended (a feature release, not a docs/no-op).

## 9. Verify matrix

| Touched | Must run |
|---|---|
| any `ui/src/**` | `npm run build --prefix crates/agentum-desktop/ui` |
| tests changed/affected | `npm run test --prefix crates/agentum-desktop/ui -- run` |
| any `crates/agentum-desktop/src/**.rs` | `cargo build -p agentum-desktop` |
