---
name: browser-verification-loop
description: >-
  Use to autonomously verify a task list in a REAL browser via the Playwright
  MCP server and report pass/fail back to a linked GitHub/Linear issue. Drives
  the browser in-browser for each task, captures a screenshot per task as
  mandatory evidence (strict: no screenshot → cannot pass), enforces a stop cap
  so it never loops unbounded, and fails loudly (never silently) if the
  Playwright MCP browser can't start. Triggers: "verify in the browser",
  "run the browser checks", "browser verification loop", an agentum
  Agents & Automation launch, or a task list that must be confirmed against a
  running web app.
---

# Browser Verification Loop

Autonomously confirm a list of tasks **in a real browser** using the Playwright
MCP server, then report the result to the linked issue. This is the local half
of agentum spec 008a: the agent does the browser work; agentum launches this
skill and (for Linear, or strict enforcement) posts the comment.

## When To Use

- An agentum **Agents & Automation → Browser Verification Loop** launch.
- A task list (a GitHub/Linear issue checklist, or an explicit list) that must
  be verified against a running web app, unattended.
- You need a pass/fail report posted back to the issue, backed by evidence.

## When Not To Use

- There is no Playwright MCP server configured (see Preconditions — fail loudly).
- The work is non-browser (use a normal test runner instead).
- You only need a quick manual look (just drive the browser directly).

## Preconditions (check first — fail loudly if unmet)

1. **Playwright MCP present.** The Playwright MCP tools (e.g. `browser_navigate`,
   `browser_snapshot`, `browser_take_screenshot`) must be available this session.
   Confirm via `/mcp` or by checking the tool list. **If they are absent, STOP
   immediately**, emit a `failed` result naming the reason (e.g. "Playwright MCP
   not connected — add it to .mcp.json and `npx playwright install chromium`"),
   and post nothing green. Never hang waiting for a browser that won't start.
2. **A task list.** From the linked issue body (checkbox list) or passed in at
   launch. If empty, report `failed` with "no tasks to verify".
3. **A stop cap.** A max-iteration / task budget (passed at launch; default 25).
   You MUST honor it — never loop unbounded.
4. For direct GitHub posting: `gh` is authenticated (`gh auth status`). If not,
   skip the direct post and rely on the emitted result block (the desktop posts).

## The loop (one task per iteration)

For each task, up to the stop cap:

1. **Drive it in-browser.** Use the Playwright MCP tools to navigate, interact,
   and observe the running app — actually exercise the behavior the task
   describes. Do the work *in the browser*; do not infer pass/fail from code.
2. **Capture mandatory evidence.** Take a screenshot (`browser_take_screenshot`)
   — and/or a `browser_snapshot` — that shows the verified state. Record its
   path/ref. **This is required for the task to count as passed.**
3. **Decide pass/fail** from what the browser actually showed.
4. **Record** `{ task, status: pass|fail, evidence: <screenshot ref>, note }`.
5. Stop when every task is done OR the stop cap is reached (note any unrun tasks
   as `skipped: cap reached` — do not silently drop them).

## Strict evidence (non-negotiable)

A task may be reported **passed only if it carries a captured screenshot/snapshot
ref**. A "pass" with no evidence is invalid — downgrade it to `fail` with the
note "no evidence captured". The desktop also rejects an all-pass result that
carries no evidence, so a green report without screenshots will be blocked
either way.

## Report

Build a markdown summary:

```
## Browser verification — <passed>/<total> passed
- [x] <task> — evidence: <screenshot ref>
- [ ] <task> — FAILED: <why> (evidence: <ref>)
...
Stopped at: <completed N / cap M>. Tasks unrun: <list or none>.
```

Then deliver it two ways (do both when possible):

1. **Direct post (GitHub, when `gh` is authed):**
   `gh issue comment <number> --repo <owner/repo> --body-file <summary.md>`
   (PR conversation: `gh pr comment`). This is the standalone path.
2. **Structured result block** so the agentum desktop can enforce strict
   evidence and post for desktop-managed providers (Linear):

   ```
   <<<AGENTUM-VERIFY-RESULT>>>
   { "status": "completed" | "failed",
     "reason": "<only when failed>",
     "summary": "<the markdown above>",
     "tasks": [ { "task": "...", "status": "pass|fail|skipped", "evidence": "<ref>" } ] }
   <<<END>>>
   ```

   If launched through agentum orchestration, also report via:
   `agentum orchestration task-update <task-id> completed --result '<json above>'`
   (or `failed`), so the launching desktop pane can post the comment and surface
   the outcome.

## Fail-loud contract

- Playwright MCP missing / browser won't start → `failed` result with the reason,
  no green comment, stop. Never a silent hang.
- A task you cannot drive in the browser → mark it `fail` with the reason, keep
  going, do not fake a pass.
- Cap reached with tasks remaining → report the unrun tasks explicitly.
