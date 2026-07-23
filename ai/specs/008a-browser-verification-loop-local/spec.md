# Spec: MCP Browser Verification Loop — Local (008a)

> Child of **008** (umbrella). This is the engine slice: prove the whole vertical on a
> **local** session. **008b** extends it to remote hosts.

## Goal

A developer can **launch**, from **Agents & Automation**, an autonomous agent loop that drives the **Playwright MCP** browser to verify its task list on a **local** session, and posts pass/fail as a comment on the linked GitHub/Linear issue.

---

## User Value

**In one line:** locally, the agent verifies its own work in a real browser and reports pass/fail to the issue — unattended — so the developer stops doing that check by hand.

This proves the entire loop → MCP-drive → issue-posting vertical with near-zero environment risk, and becomes the engine 008b reuses on remote hosts.

---

## Requirements

- A launch point on **Agents & Automation** that starts the loop, bound to a local session/repo and a linked issue.
- The loop drives the **Playwright MCP** browser to execute **and** verify each task **entirely in-browser**.
- **Bounded autonomy:** an explicit stop condition (max iterations / task budget) so it runs unattended without running away.
- On completion (or per task), the loop posts pass/fail as a **comment on the linked GitHub/Linear issue** via agentum's existing integration.
- **Loud failure:** if the Playwright MCP browser can't start locally, report a stated reason instead of hanging.

---

## Acceptance Criteria

- [ ] From **Agents & Automation**, a developer can launch the loop bound to a local session/repo and a tracked issue
- [ ] The loop drives the **Playwright MCP** browser to execute and verify every task **in-browser** (no stubbed/non-browser shortcut)
- [ ] The loop runs **unattended to completion** and halts at an **explicit stop condition** (max iterations / task budget)
- [ ] On completion (or per task), the loop **posts the pass/fail result as a comment** on the linked **GitHub/Linear** issue
- [ ] If the Playwright MCP browser can't start, the loop **fails with a stated reason** — no silent hang

---

## Dependencies

- **External issue-tracker integration (GitHub/Linear)** — existing (gh/glab CLI + Linear token store).
- **Playwright MCP config support** — existing `.mcp.json` inspector (`McpConfigSection.tsx`) + the agent CLIs that read it and launch the server.
- **Autonomous-loop pattern** — Ralph-style loop already exercised in spec 007's execution.
- Parent: **008**. No prior-spec blocker.

---

## Risks

- **Agent reports green without actually driving** (silent failure / hallucinated pass). *Architect to decide:* trust the self-report vs. require hard evidence (screenshot/trace per task) before posting.
- **Unattended loop runs away** — mitigated by the explicit stop-condition criterion.
- **External tracker dependency** — auth, rate limits, posting to the wrong issue.

---

## Notes

- **Out of scope (parent-level, parked):** authoring tests for the user; visual-regression/screenshot diffing; non-browser MCP servers; external CI integration; cron-scheduling. **Plus: remote-host execution → deferred to 008b.**
- **Anchor:** the user's proven localhost run — Ralph + Playwright MCP, ~10 tasks, perfect. 008a = the same run, but launched from Agents & Automation and reporting to the issue.
