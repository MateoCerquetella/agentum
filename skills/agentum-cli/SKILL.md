---
name: agentum-cli
description: >-
  Use the `agentum` CLI to drive a running agentum editor — manage agentum worktrees;
  create and manage scheduled automations; create, read, and run shell commands
  in agentum-managed terminals; and automate agentum's built-in browser
  (snapshot/click/fill/screenshot/tabs). Use this
  instead of raw `git worktree`, ad hoc shell PTYs, or Playwright whenever the
  task touches agentum state. Coding agents inside an agentum worktree should also use
  it to keep the worktree comment fresh at meaningful checkpoints. Boundary with
  `orchestration`: if the recipient of a terminal write is another AI agent
  (Claude Code, Gemini, Codex, a worker), use `orchestration` — it is the only
  correct way to send messages, nudges, replies, or task hand-offs to agents.
  agentum-cli writes are for non-agent terminals (shells, build/test commands);
  reading or `wait`ing on any terminal — including agent terminals — stays in
  agentum-cli.
---

# agentum CLI

Use this skill when the task should go through agentum's control plane rather than directly through `git`, shell PTYs, or ad hoc filesystem access.

## When To Use

Use `agentum` for:

- worktree orchestration inside a running agentum app
- updating the current worktree comment with meaningful progress checkpoints
- reading agentum-managed terminals and sending input to non-agent terminals
- stopping or waiting on agentum-managed terminals
- creating and managing scheduled agentum automations
- accessing repos known to agentum
  Do not use `agentum` when plain shell tools are simpler and agentum state does not matter.

Examples:

- creating one agentum worktree per GitHub issue
- updating the current worktree comment after a significant checkpoint, such as reproducing a bug, validating a fix, or handing off for review
- finding the Claude Code terminal for a worktree and reading its status
- checking which agentum worktrees have live terminal activity
- creating a scheduled automation that runs a prompt against a known repo or worktree

## Preconditions

- Prefer the public `agentum` command first
- agentum editor/runtime should already be running, or the agent should start it with `agentum open`
- Do not begin by inspecting agentum source files just to decide how to invoke the CLI. The first step is to check whether the installed `agentum` command exists.
- Do not assume a generic shell environment variable proves the agent is "inside agentum". For normal agent flows, the public CLI is the supported surface, but avoid wasting a round trip on probe-only checks when a direct agentum action would answer the question.

First verify the public CLI is installed:

```bash
command -v agentum
```

Then use the public command:

```bash
agentum status --json
```

If the task is about agentum worktrees or agentum terminals, do this before any codebase exploration:

```bash
command -v agentum
agentum status --json
```

If the agent truly needs to confirm that the current directory is inside an agentum-managed worktree, use:

```bash
agentum worktree current --json
```

If `agentum` / `agentum` is not on PATH, say so explicitly and stop or ask the user to install/register the CLI before continuing.

## Core Workflow

1. Confirm agentum runtime availability:

```bash
agentum status --json
```

If agentum is not running yet:

```bash
agentum open --json
agentum status --json
```

2. Discover current agentum state:

```bash
agentum worktree ps --json
agentum terminal list --json
```

3. Resolve a target worktree or terminal handle.

4. Act through agentum:

- `worktree create/set/rm`
- `automations list/show/create/edit/remove/run/runs`
- `terminal read/send/wait/stop`

5. When the agent reaches a significant checkpoint in the current worktree, update the agentum worktree comment so the UI reflects the latest work-in-progress:

```bash
agentum worktree set --worktree active --comment "reproduced auth failure with aws sts; testing credential-chain fix" --json
```

Why: the worktree comment is agentum's lightweight, agent-writable status field. Keeping it current gives the user an at-a-glance summary of what the agent most recently proved, changed, or is waiting on.

## Command Surface

### Repo

```bash
agentum repo list --json
agentum repo show --repo id:<repoId> --json
agentum repo add --path /abs/repo --json
agentum repo set-base-ref --repo id:<repoId> --ref origin/main --json
agentum repo search-refs --repo id:<repoId> --query main --limit 10 --json
```

### Worktree

```bash
agentum worktree list --repo id:<repoId> --json
agentum worktree ps --json
agentum worktree current --json
agentum worktree show --worktree id:<worktreeId> --json
agentum worktree create --repo id:<repoId> --name my-task --issue 123 --comment "seed" --json
agentum worktree create --repo id:<repoId> --name related-task --parent-worktree active --json
agentum worktree create --repo id:<repoId> --name independent-task --no-parent --json
agentum worktree set --worktree id:<worktreeId> --display-name "My Task" --json
agentum worktree set --worktree active --comment "reproduced bug; collecting logs from staging" --json
agentum worktree set --worktree active --comment "waiting on review" --json
agentum worktree rm --worktree id:<worktreeId> --force --json
```

Worktree selectors supported in focused v1:

- `id:<worktree-id>`
- `path:<absolute-path>`
- `branch:<branch-name>`
- `issue:<number>`
- `active` / `current` to resolve the enclosing agentum-managed worktree from the shell `cwd`

### Worktree Lineage

Worktree lineage records intent; it is not a required flag sequence. When creating a worktree from inside an agentum-managed worktree, decide whether the new work is related to the current work or independent of it.

For related work, rely on agentum's inferred parent. Use `--parent-worktree active` when the current worktree relationship should be explicit or when the shell context might not make the intended parent obvious.

```bash
agentum worktree create --repo id:<repoId> --name related-task --json
agentum worktree create --repo id:<repoId> --name related-task --parent-worktree active --json
```

For independent work, pass `--no-parent`.

```bash
agentum worktree create --repo id:<repoId> --name independent-task --no-parent --json
```

A different branch, issue, or name is not enough by itself to make the work independent. Treat lineage as a record of why the workspace exists, not as a property of the branch name.

### Automations

```bash
agentum automations list --json
agentum automations show <automationId> --json
agentum automations create --name "Daily review" --trigger daily --time 09:00 --prompt "Review open changes" --provider codex --repo id:<repoId> --json
agentum automations create --name "Weekday triage" --trigger "0 9 * * 1-5" --prompt "Triage issues" --provider claude --repo path:/abs/repo --disabled --json
agentum automations create --name "Inbox digest" --trigger hourly --prompt "Summarize unread mail" --provider codex --workspace active --reuse-session --json
agentum automations edit <automationId> --name "Weekday review" --trigger weekdays --time 09:30 --fresh-session --json
agentum automations run <automationId> --json
agentum automations runs --id <automationId> --json
agentum automations remove <automationId> --json
```

Automation schedules accept `hourly`, `daily`, `weekdays`, `weekly`, a 5-field cron expression, or an RRULE string. Use `--time <HH:MM>` with `daily`, `weekdays`, or `weekly`; use `--day <0-6>` only with `weekly`, where Sunday is `0`.

Use `--repo <selector>` for a new worktree per run, or `--workspace <selector>` / `--workspace-mode existing` when the automation should run in an existing agentum worktree. `--repo` and `--workspace` are mutually exclusive.

Use `--reuse-session` only for existing-workspace automations when later runs should submit into the previous live automation terminal. Use `--fresh-session` to turn reuse back off. If the previous live terminal is gone, agentum falls back to a fresh session.

Why: automations are persisted through the running agentum runtime, so use the CLI instead of editing automation storage files directly. Prefer `--disabled` when creating an automation during tests or setup so it cannot run before the user reviews it.

### Terminal

Use selectors to discover terminals, then use the returned handle for repeated live interaction.

```bash
agentum terminal list --worktree id:<worktreeId> --json
agentum terminal show --terminal <handle> --json
agentum terminal read --terminal <handle> --json
agentum terminal read --terminal <handle> --cursor <oldestCursor> --limit 1000 --json
agentum terminal send --terminal <handle> --text "continue" --enter --json
agentum terminal wait --terminal <handle> --for exit --timeout-ms 5000 --json
agentum terminal wait --terminal <handle> --for tui-idle --timeout-ms 30000 --json
agentum terminal stop --worktree id:<worktreeId> --json
agentum terminal create --json
agentum terminal create --title "My Terminal" --json
agentum terminal create --worktree path:/projects/myapp --command "npm test" --json
agentum terminal split --terminal <handle> --direction vertical --json
agentum terminal split --terminal <handle> --direction horizontal --command "npm run dev" --json
agentum terminal rename --terminal <handle> --title "New Name" --json
agentum terminal switch --terminal <handle> --json
agentum terminal close --terminal <handle> --json
agentum terminal send --text "echo hello" --enter --json
agentum terminal read --json
```

Why: `--terminal` is optional for most commands. When omitted, agentum auto-resolves to the active terminal in the current worktree (same as browser commands target the active tab). Use explicit `--terminal <handle>` when operating on a specific pane.

Why: `terminal create` creates a background session unless `--focus` is explicit. Interactive local agent commands such as bare `codex` or bare `claude` use agentum's renderer-backed terminal path so they can start at the app's measured terminal geometry without stealing focus from the user.

Why: long terminal transcripts should be read with cursors. After a limited tail preview without an input cursor, page retained transcript from `oldestCursor`; in that case `nextCursor` already equals `latestCursor` and would skip omitted output. After a cursor read, if `limited` remains true and `nextCursor !== latestCursor`, continue with the returned `nextCursor`. Cursor reads default to the retained transcript size; `--limit` can request a smaller page. If `truncated` is true, older output has already fallen out of the retained buffer; use `oldestCursor` as the earliest available cursor.

Why: terminal handles are runtime-scoped and may go stale after reloads. If agentum returns `terminal_handle_stale`, reacquire a fresh handle with `terminal list`.

Why: `--direction horizontal` splits the pane **left and right** (new pane appears to the right). `--direction vertical` splits the pane **top and bottom** (new pane appears below). This matches VS Code's split convention. Default is horizontal.

## Agent Guidance

- If the user says to create/manage an agentum worktree, use `agentum worktree ...`, not raw `git worktree ...`.
- If the user says to create/manage a scheduled agentum automation, use `agentum automations ...`, not direct persistence edits.
- Treat agentum as the source of truth for agentum worktree and terminal tasks. Do not mix agentum-managed state with ad hoc git worktree commands unless agentum explicitly cannot perform the requested action.
- Prefer `--json` for all machine-driven use.
- Use `worktree ps` as the first summary view when many worktrees may exist.
- Use `worktree current` or `--worktree active` when the agent is already running inside the target worktree.
- When creating a worktree from an existing workspace, choose lineage based on intent: related work should keep parent context, independent work should use `--no-parent`.
- Let agentum infer the parent when the current/caller workspace is the right parent; use `--parent-worktree active` when making that relationship explicit is useful.
- Treat `agentum worktree set --worktree active --comment ... --json` as a default coding-agent behavior whenever the agent reaches a meaningful checkpoint in the current agentum-managed worktree; the user does not need to explicitly ask for each update.
- Update the worktree comment at significant checkpoints, not every trivial command. Good checkpoints include reproducing a bug, confirming a hypothesis, starting a risky migration, finishing a meaningful implementation slice, switching from investigation to fix, or blocking on external input.
- Write comments as short status snapshots of the current state, for example `debugging AWS CLI profile resolution`, `confirmed flaky test is caused by temp-dir race`, or `fix implemented; running integration tests`.
- Prefer optimistic execution over probe-first flows for checkpoint updates: if `agentum` is on `PATH`, call `agentum worktree set --worktree active --comment ... --json` directly at the checkpoint instead of spending an extra cycle on `agentum worktree current`.
- If that direct update fails because agentum is unavailable or the shell is not inside an agentum-managed worktree, continue the main task and treat the comment update as best-effort unless the user explicitly made agentum state part of the task.
- Use `agentum worktree current --json` only when the agent actually needs the worktree identity for later logic, not as a preflight before every comment update.
- agentum only injects `AGENTUM_WORKTREE_PATH`-style variables for some setup-hook flows, so they are not a general detection contract for agents.
- Use `terminal list` to reacquire handles after agentum reloads.
- Use `terminal read` before `terminal send` unless the next input is obvious.
- For long agent responses, use `terminal read --json` with `oldestCursor`, `nextCursor`, `--cursor`, and `--limit` instead of relying on the default human preview. After a limited tail preview, start at `oldestCursor`; after a cursor read, continue with `nextCursor` only while `limited` is true and `nextCursor !== latestCursor`. Treat `truncated` as a signal that the requested cursor was older than the retained output.
- Use `terminal wait --terminal <handle> --for exit` only when the task actually depends on process completion.
- Use `terminal wait --terminal <handle> --for tui-idle` to wait for an agent CLI (Claude Code, Gemini, Codex, etc.) to finish its current task. This detects the working→idle OSC title transition. Always pass `--timeout-ms` as a safety net — unsupported CLIs will hang until timeout.
- Use `terminal create` to spin up new terminal tabs programmatically, optionally with a `--command` for startup (e.g. `--command "claude"` to launch Claude Code) and `--title` for labeling. In local agentum sessions, `--command "codex"` is routed through agentum's visible terminal path automatically so Codex does not start as a headless/background PTY. After creating a `--command` terminal, use `terminal wait --for tui-idle` to wait for the agent to boot before dispatching.
- Use `terminal split` to create split panes within an existing terminal tab. Pass `--command` to run a command in the new pane.
- Prefer agentum worktree selectors over hardcoded paths when agentum identity already exists.
- If the user asks for CLI UX feedback, test the public `agentum` / `agentum` command first. Only inspect `src/cli` or use `node out/cli/index.js` if the public command is missing or the task is explicitly about implementation internals.
- If a command fails, prefer retrying with the public `agentum` / `agentum` command before concluding the CLI is broken, unless the failure already came from the CLI itself.

## Browser Automation

The `agentum` CLI also drives the built-in agentum browser. The core workflow is a **snapshot-interact-re-snapshot** loop:

1. **Snapshot** the page to see interactive elements and their refs.
2. **Interact** using refs (`@e1`, `@e3`, etc.) to click, fill, or select.
3. **Re-snapshot** after interactions to see the updated page state.

```bash
agentum goto --url https://example.com --json
agentum snapshot --json
# Read the refs from the snapshot output
agentum click --element @e3 --json
agentum snapshot --json
```

### Element Refs

Refs like `@e1`, `@e5` are short identifiers assigned to interactive page elements during a snapshot. They are:

- **Assigned by snapshot**: Run `agentum snapshot` to get current refs.
- **Scoped to one tab**: Refs from one tab are not valid in another.
- **Invalidated by navigation**: If the page navigates after a snapshot, refs become stale. Re-snapshot to get fresh refs.
- **Invalidated by tab switch**: Switching tabs with `agentum tab switch` invalidates refs. Re-snapshot after switching.

If a ref is stale, the command returns `browser_stale_ref` — re-snapshot and retry.

### Worktree Scoping

Browser commands default to the **current worktree** — only tabs belonging to the agent's worktree are visible and targetable. Tab indices are relative to the filtered tab list.

```bash
# Default: operates on tabs in the current worktree
agentum snapshot --json

# Explicitly target all worktrees (cross-worktree access)
agentum snapshot --worktree all --json

# Tab indices are relative to the worktree-filtered list
agentum tab list --json         # Shows tabs [0], [1], [2] for this worktree
agentum tab switch --index 1 --json   # Switches to tab [1] within this worktree
```

If no tabs are open in the current worktree, commands return `browser_no_tab`.

### Stable Page Targeting

For single-agent flows, bare browser commands are fine: agentum will target the active browser tab in the current worktree.

For concurrent or multi-process browser automation, prefer a stable page id instead of ambient active-tab state:

1. Run `agentum tab list --json`.
2. Read `tabs[].browserPageId` from the result.
3. Pass `--page <browserPageId>` to follow-up commands like `snapshot`, `click`, `goto`, `screenshot`, `tab switch`, or `tab close`.

Why: active-tab state and tab indices can change while another agentum CLI process is working. `browserPageId` pins the command to one concrete tab.

```bash
agentum tab list --json
agentum snapshot --page page-123 --json
agentum click --page page-123 --element @e3 --json
agentum screenshot --page page-123 --json
agentum tab switch --page page-123 --json
agentum tab close --page page-123 --json
```

If you also pass `--worktree`, agentum treats it as extra scoping/validation for that page id. Without `--page`, commands still fall back to the current worktree's active tab.

### Navigation

```bash
agentum goto --url <url> [--json]           # Navigate to URL, waits for page load
agentum back [--json]                       # Go back in browser history
agentum forward [--json]                    # Go forward in browser history
agentum reload [--json]                     # Reload the current page
```

### Observation

```bash
agentum snapshot [--page <browserPageId>] [--json]                   # Accessibility tree snapshot with element refs
agentum screenshot [--page <browserPageId>] [--format <png|jpeg>] [--json]  # Viewport screenshot (base64)
agentum full-screenshot [--page <browserPageId>] [--format <png|jpeg>] [--json]  # Full-page screenshot (base64)
agentum pdf [--page <browserPageId>] [--json]                        # Export page as PDF (base64)
```

### Interaction

```bash
agentum click --element <ref> [--page <browserPageId>] [--json]      # Click an element by ref
agentum dblclick --element <ref> [--page <browserPageId>] [--json]   # Double-click an element
agentum fill --element <ref> --value <text> [--page <browserPageId>] [--json]  # Clear and fill an input
agentum type --input <text> [--page <browserPageId>] [--json]        # Type at current focus (no element targeting)
agentum select --element <ref> --value <value> [--page <browserPageId>] [--json]  # Select dropdown option
agentum check --element <ref> [--page <browserPageId>] [--json]      # Check a checkbox
agentum uncheck --element <ref> [--page <browserPageId>] [--json]    # Uncheck a checkbox
agentum scroll --direction <up|down> [--amount <pixels>] [--page <browserPageId>] [--json]  # Scroll viewport
agentum scrollintoview --element <ref> [--page <browserPageId>] [--json]  # Scroll element into view
agentum hover --element <ref> [--page <browserPageId>] [--json]      # Hover over an element
agentum focus --element <ref> [--page <browserPageId>] [--json]      # Focus an element
agentum drag --from <ref> --to <ref> [--page <browserPageId>] [--json]  # Drag from one element to another
agentum clear --element <ref> [--page <browserPageId>] [--json]      # Clear an input field
agentum select-all --element <ref> [--page <browserPageId>] [--json] # Select all text in an element
agentum keypress --key <key> [--page <browserPageId>] [--json]       # Press a key (Enter, Tab, Escape, etc.)
agentum upload --element <ref> --files <paths> [--page <browserPageId>] [--json]  # Upload files to a file input
```

### Tab Management

```bash
agentum tab list [--json]                   # List open browser tabs
agentum tab switch (--index <n> | --page <browserPageId>) [--json]     # Switch active tab (invalidates refs)
agentum tab create [--url <url>] [--json]   # Open a new browser tab
agentum tab close [--index <n> | --page <browserPageId>] [--json]    # Close a browser tab
```

### Wait / Synchronization

```bash
agentum wait [--timeout <ms>] [--json]                        # Wait for timeout (default 1000ms)
agentum wait --selector <css> [--state <visible|hidden>] [--timeout <ms>] [--json]  # Wait for element
agentum wait --text <string> [--timeout <ms>] [--json]        # Wait for text to appear on page
agentum wait --url <substring> [--timeout <ms>] [--json]      # Wait for URL to contain substring
agentum wait --load <networkidle|load|domcontentloaded> [--timeout <ms>] [--json]   # Wait for load state
agentum wait --fn <js-expression> [--timeout <ms>] [--json]   # Wait for JS condition to be truthy
```

After any page-changing action, pick one:

- Wait for specific content: `agentum wait --text "Dashboard" --json`
- Wait for URL change: `agentum wait --url "/dashboard" --json`
- Wait for network idle (catch-all for SPA navigation): `agentum wait --load networkidle --json`
- Wait for an element: `agentum wait --selector ".results" --json`

Avoid bare `agentum wait --timeout 2000` except when debugging — it makes scripts slow and flaky.

### Data Extraction

```bash
agentum exec --command "get text @e1" [--json]   # Get visible text of an element
agentum exec --command "get html @e1" [--json]   # Get innerHTML
agentum exec --command "get value @e1" [--json]  # Get input value
agentum exec --command "get attr @e1 href" [--json]  # Get element attribute
agentum exec --command "get title" [--json]      # Get page title
agentum exec --command "get url" [--json]        # Get current URL
agentum exec --command "get count .item" [--json]      # Count matching elements
```

### State Checks

```bash
agentum exec --command "is visible @e1" [--json]  # Check if element is visible
agentum exec --command "is enabled @e1" [--json]  # Check if element is enabled
agentum exec --command "is checked @e1" [--json]  # Check if checkbox is checked
```

### Page Inspection

```bash
agentum eval --expression <js> [--json]     # Evaluate JS in page context
```

### Cookie Management

```bash
agentum cookie get [--url <url>] [--json]   # List cookies
agentum cookie set --name <n> --value <v> [--domain <d>] [--json]  # Set a cookie
agentum cookie delete --name <n> [--domain <d>] [--json]  # Delete a cookie
```

### Emulation

```bash
agentum viewport --width <w> --height <h> [--scale <n>] [--mobile] [--json]
agentum geolocation --latitude <lat> --longitude <lng> [--accuracy <m>] [--json]
```

### Request Interception

```bash
agentum intercept enable [--patterns <list>] [--json]  # Start intercepting requests
agentum intercept disable [--json]          # Stop intercepting
agentum intercept list [--json]             # List paused requests
```

> **Note:** Per-request `intercept continue` and `intercept block` are not yet supported.
> They will be added once agent-browser supports per-request interception decisions.

### Console / Network Capture

```bash
agentum capture start [--json]              # Start capturing console + network
agentum capture stop [--json]               # Stop capturing
agentum console [--limit <n>] [--json]      # Read captured console entries
agentum network [--limit <n>] [--json]      # Read captured network entries
```

### Mouse Control

```bash
agentum exec --command "mouse move 100 200" [--json]   # Move mouse to coordinates
agentum exec --command "mouse down left" [--json]      # Press mouse button
agentum exec --command "mouse up left" [--json]        # Release mouse button
agentum exec --command "mouse wheel 100" [--json]      # Scroll wheel
```

### Keyboard

```bash
agentum exec --command "keyboard inserttext \"text\"" [--json]  # Insert text bypassing key events
agentum exec --command "keyboard type \"text\"" [--json]        # Raw keystrokes
agentum exec --command "keydown Shift" [--json]                 # Hold key down
agentum exec --command "keyup Shift" [--json]                   # Release key
```

### Frames (Iframes)

Iframes are auto-inlined in snapshots — refs inside iframes work transparently. For scoped interaction:

```bash
agentum exec --command "frame @e3" [--json]        # Switch to iframe by ref
agentum exec --command "frame \"#iframe\"" [--json] # Switch to iframe by CSS selector
agentum exec --command "frame main" [--json]       # Return to main frame
```

### Semantic Locators (alternative to refs)

When refs aren't available or you want to skip a snapshot:

```bash
agentum exec --command "find role button click --name \"Submit\"" [--json]
agentum exec --command "find text \"Sign In\" click" [--json]
agentum exec --command "find label \"Email\" fill \"user@test.com\"" [--json]
agentum exec --command "find placeholder \"Search\" type \"query\"" [--json]
agentum exec --command "find testid \"submit-btn\" click" [--json]
```

### Dialogs

`alert` and `beforeunload` are auto-accepted. For `confirm` and `prompt`:

```bash
agentum exec --command "dialog status" [--json]        # Check for pending dialog
agentum exec --command "dialog accept" [--json]        # Accept
agentum exec --command "dialog accept \"text\"" [--json]  # Accept with prompt input
agentum exec --command "dialog dismiss" [--json]       # Dismiss/cancel
```

### Extended Commands (Passthrough)

```bash
agentum exec --command "<agent-browser command>" [--json]
```

The `exec` command provides access to agent-browser's full command surface. Useful for commands without typed agentum handlers:

```bash
agentum exec --command "set device \"iPhone 14\"" --json   # Emulate device
agentum exec --command "set offline on" --json             # Toggle offline mode
agentum exec --command "set media dark" --json             # Emulate color scheme
agentum exec --command "network requests" --json           # View tracked network requests
agentum exec --command "help" --json                       # See all available commands
```

**Important:** Do not use `agentum exec --command "tab ..."` for tab management. Use `agentum tab list/create/close/switch` instead — those operate at the agentum level and keep the UI synchronized.

### `fill` vs `type`

- **`fill`** targets a specific element by ref, clears its value first, then enters text. Use for form fields.
- **`type`** types at whatever currently has focus. Use for search boxes or after clicking into an input.

If neither works on a custom input component, try:

```bash
agentum focus --element @e1 --json
agentum exec --command "keyboard inserttext \"text\"" --json   # bypasses key events
```

### Browser Error Codes

| Error Code              | Meaning                                      | Recovery                                                                                     |
| ----------------------- | -------------------------------------------- | -------------------------------------------------------------------------------------------- |
| `browser_no_tab`        | No browser tab is open in this worktree      | Open a tab, or use `--worktree all` to check other worktrees                                 |
| `browser_stale_ref`     | Ref is invalid (page changed since snapshot) | Run `agentum snapshot` to get fresh refs                                                        |
| `browser_tab_not_found` | Tab index does not exist                     | Run `agentum tab list` to see available tabs                                                    |
| `browser_error`         | Error from the browser automation engine     | Read the message for details; common causes: element not found, navigation timeout, JS error |

### Browser Worked Example

Agent fills a login form and verifies the dashboard loads:

```bash
# Navigate to the login page
agentum goto --url https://app.example.com/login --json

# See what's on the page
agentum snapshot --json
# Output includes:
#   [@e1] text input "Email"
#   [@e2] text input "Password"
#   [@e3] button "Sign In"

# Fill the form
agentum fill --element @e1 --value "user@example.com" --json
agentum fill --element @e2 --value "s3cret" --json

# Submit
agentum click --element @e3 --json

# Verify the dashboard loaded
agentum snapshot --json
# Output should show dashboard content, not the login form
```

### Browser Troubleshooting

**"Ref not found" / `browser_stale_ref`**
Page changed since the snapshot. Run `agentum snapshot --json` again, then use the new refs.

**Element exists but not in snapshot**
It may be off-screen or not yet rendered. Try:

```bash
agentum scroll --direction down --amount 1000 --json
agentum snapshot --json
# or wait for it:
agentum wait --text "..." --json
agentum snapshot --json
```

**Click does nothing / overlay swallows the click**
Modals or cookie banners may be blocking. Snapshot, find the dismiss button, click it, then re-snapshot.

**Fill/type doesn't work on a custom input**
Some components intercept key events. Use `keyboard inserttext`:

```bash
agentum focus --element @e1 --json
agentum exec --command "keyboard inserttext \"text\"" --json
```

**`browser_no_tab` error**
No browser tab is open in the current worktree. Open one with `agentum tab create --url <url> --json`.

### Auto-Switch Worktree

Browser commands automatically activate the target worktree in the agentum UI when needed. If the agent issues a browser command targeting a worktree that isn't currently active, agentum will switch to that worktree before executing the command.

### Tab Create Auto-Activation

When `agentum tab create` opens a new tab, it is automatically set as the active tab for the worktree. Subsequent commands (`snapshot`, `click`, etc.) will target the newly created tab without needing an explicit `tab switch`.

### Browser Agent Guidance

- Always snapshot before interacting with elements.
- After navigation (`goto`, `back`, `reload`, clicking a link), re-snapshot to get fresh refs.
- After switching tabs, re-snapshot.
- If you get `browser_stale_ref`, re-snapshot and retry with the new refs.
- Use `agentum tab list` before `agentum tab switch` to know which tabs exist.
- For concurrent browser workflows, prefer `agentum tab list --json` and reuse `tabs[].browserPageId` with `--page` on later commands.
- Use `agentum wait` to synchronize after actions that trigger async updates (form submits, SPA navigation, modals) instead of arbitrary sleeps.
- Use `agentum eval` as an escape hatch for interactions not covered by other commands.
- Use `agentum exec --command "help"` to discover extended commands.
- Worktree scoping is automatic — you'll only see tabs from your worktree by default.
- Bare browser commands without `--page` still target the current worktree's active tab, which is convenient but less robust for multi-process automation.
- Tab creation auto-activates the new tab — no need for `tab switch` after `tab create`.
- Browser commands auto-switch the active worktree if needed — no manual worktree activation required.

## Important Constraints

- agentum CLI only talks to a running agentum editor.
- Terminal handles are ephemeral and tied to the current agentum runtime. If agentum restarts, handles change.
- `terminal wait` supports `--for exit` (wait for process exit) and `--for tui-idle` (wait for a recognized agent CLI like Claude Code, Gemini, or Codex to finish its current task, detected via OSC title transitions). `tui-idle` defaults to a 5-minute timeout if `--timeout-ms` is not specified. Real coding tasks routinely take 15-60 minutes — always pass `--timeout-ms` explicitly.
- agentum is the source of truth for worktree/terminal state; do not duplicate that state with manual assumptions.
- The public `agentum` command is the interface users experience. Agents should validate and use that surface, not repo-local implementation entrypoints.
- The default bounded `terminal read` preview is for status monitoring. For retained transcript extraction, use `terminal read --json` with `oldestCursor`/`nextCursor`, `--cursor`, and `--limit`.

## References

When behavior is unclear, prefer the live CLI help over inspecting source:

- `agentum --help` and `agentum <subcommand> --help`
- `CLAUDE.md` — agentum architecture overview (crates, daemon/TUI/desktop split)
