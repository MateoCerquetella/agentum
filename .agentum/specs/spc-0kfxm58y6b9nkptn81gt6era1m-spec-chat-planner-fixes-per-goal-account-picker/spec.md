---
schema: 1
id: SPC-0KFXM58Y6B9NKPTN81GT6ERA1M
revision: 1
title: Spec: Chat Planner Fixes + Per-Goal Account Picker
source: legacy-import:ai/specs/015-chat-planner-fixes-and-account-picker/spec.md@sha256:747c67a560a0568fe5d902955f2c788f3b23a0ee3b29485e0b5730a1d9d5e313
---

# Spec: Chat Planner Fixes + Per-Goal Account Picker

## Migration provenance

This historical specification was assigned a stable Agentum identity during the
v2 cutover. Its source is included below and its exact original bytes are also
preserved in the external recovery archive and accounted for by SHA-256.

## Requirements

- RQ-001 Preserve the historical specification's stable identity and source provenance.
- RQ-002 Treat this imported revision as historical context until a user explicitly reopens it.

## Acceptance criteria

- AC-001 The source path and SHA-256 match the migration inventory and recovery archive.
- AC-002 New work on this specification creates an immutable later revision through Agentum.

## Imported historical source

> # Spec: Chat Planner Fixes + Per-Goal Account Picker
>
> > Fixes the Chat → board-goals planner surface (#48 / spec 011) on three fronts
> > the user hit in practice: a hard 400 that orphaned goals, a chats sidebar with
> > no way to switch cleanly or delete, and a tool/agent dropdown that needs to
> > also choose **which Claude/Codex account** the planner runs as. Branch:
> > `feat/chat-agent-picker-remove-goals` (based on `main`, which already threads
> > the picker's `tool` through `/api/board/goals`).
>
> ## Goal
>
> Make the Chat front door reliably draft a backlog and let the user choose the
> agent **and account** the planner runs as — without leaving stuck/duplicate
> "planning…" chats behind and without silently clobbering their global Claude
> login.
>
> ---
>
> ## User Value
>
> The Chat surface is the "describe a feature → planner drafts board cards" front
> door. Today it is broken end-to-end for this user:
>
> - Every goal submit returned **400** (`planner.prompt_file must be an absolute
>   path: ../etc/passwd`) — a leaked test fixture in the real `planner.toml`. The
>   goal row was created *before* the planner config loaded, so each failed submit
>   **orphaned a goal** that sits at "planning…" forever with no delete affordance.
> - The agent dropdown picks a *tool* but offers no way to pick *which account*
>   the planner authenticates as, even though agentum already manages multiple
>   Claude/Codex accounts.
>
> Fixing this makes the front door trustworthy (no orphans, deletable chats) and
> gives power users running multiple subscriptions per-goal control over which
> account does the planning.
>
> ---
>
> ## Context already resolved (this session)
>
> - **400 root cause fixed (env-level):** the real config
>   `~/Library/Application Support/agentum/planner.toml` held the leaked fixture
>   `prompt_file = "../etc/passwd"`; moved aside to `planner.toml.leaked.bak` so
>   the loader falls back to the bundled default. (`profiles.toml` is similarly
>   polluted with `dup`/`vps` fixtures — noted, see Slice 3.)
> - **macOS credential constraint (verified vs docs + `claude` 2.1.186):** on
>   macOS Claude reads the **global Keychain** (`Claude Code-credentials`)
>   regardless of `CLAUDE_CONFIG_DIR`; `.credentials.json` only isolates on
>   Linux/Windows. ⇒ **No per-process subscription isolation on macOS.** The only
>   clean per-process override is `ANTHROPIC_API_KEY` (API billing, not a
>   subscription). This is why the account picker uses **global swap**, not
>   per-pane isolation.
>
> ---
>
> ## Requirements
>
> ### Slice 1 — Chat-selector reliability (ship first)
>
> - **No orphaned goals:** in `routes/board_goals.rs::create_goal`, load + validate
>   the planner config **before** `create_board_item`, so a planner-config error
>   returns 400 with **no goal row written**.
> - **Delete a chat/goal:** expose the existing `DELETE /api/board/{id}`
>   (`board.rs:422`) as `deleteGoal(id)` in `runtime/board-client.ts`; add a
>   hover-trash control to each sidebar chat in `ChatPage.tsx` with a confirm.
>   Deleting the selected goal clears selection.
> - **Truthful "planning…":** rollup label only shows for goals genuinely mid-draft
>   (0 children but a live planner) — a no-op once orphans can't accrue, but verify.
> - **Dropdown polish:** persist the last selected tool (localStorage); keep the
>   installed-agents filter; show the account the planner will run as (Slice 2).
>
> ### Slice 2 — Per-goal account picker (active-account swap)
>
> - **Picker lists managed accounts** for the selected tool via the existing
>   desktop commands (`claude_accounts_list` / `codex_accounts_list`); each entry
>   shows its email/label and which is currently **active**.
> - **Default = current active account** → submit performs **no swap** (zero risk).
> - **Choosing a different account** calls the existing swap
>   (`claude_accounts_select` / `codex_accounts_select`) **before** `createGoal`,
>   so the planner pane inherits it; surface a one-line warning that this changes
>   the active account for **all** sessions (the macOS truth).
> - **No new per-process isolation** is attempted on macOS (see constraint above).
>
> ### Slice 3 — Test-leak hardening (small, prevents recurrence)
>
> - Prevent tests writing to the **real** config dir: in the server test harness,
>   assert `AGENTUM_HOME` is set before any `config_dir()`-relative write (fail
>   loudly otherwise), so a future `planner.toml`/`profiles.toml` fixture can't
>   leak into `~/Library/Application Support/agentum/` again.
>
> ---
>
> ## Acceptance Criteria
>
> - [ ] Submitting a goal with a broken `planner.toml` returns 400 and creates
>       **no** board item (unit test on `create_goal` ordering).
> - [ ] With a valid/absent `planner.toml`, submitting a goal drafts cards and the
>       chat leaves "planning…" once the first card lands.
> - [ ] A sidebar chat can be deleted; the goal row is gone from `/api/board` and
>       the selection resets.
> - [ ] The agent dropdown remembers the last choice across app restarts.
> - [ ] The account picker defaults to the active account and performs no swap; an
>       explicit non-active choice swaps before spawn and shows the global-effect
>       warning.
> - [ ] A server test fails loudly if a config write is attempted without
>       `AGENTUM_HOME` set.
>
> ---
>
> ## Dependencies
>
> - Spec 011 (chat-to-features) — the board-goals planner + Chat surface this builds on.
> - `main`'s `board_goals.rs` already accepts `tool`/`model` on `CreateGoalBody`.
> - Desktop account commands in `crates/agentum-desktop/src/commands/accounts.rs`.
>
> ---
>
> ## Risks
>
> - **Account swap is global & racy.** Two goals created back-to-back with
>   different accounts fight over the live Keychain slot; live terminals keep their
>   old token until restart. Mitigation: default-to-current (most submits don't
>   swap) + explicit warning. Document; do not attempt unsupported isolation.
> - **Desktop ↔ server seam.** Account swap is a desktop Tauri command; goal create
>   is a server route. The swap must complete (await) **before** `createGoal`,
>   client-side, to avoid a race where the planner spawns under the old account.
> - **Codex differs from Claude.** Codex auth is a plain `~/.codex/auth.json` file
>   (no Keychain), so it *could* isolate per-process — but we keep symmetry (swap)
>   for v1; per-process Codex isolation is a possible later enhancement (YAGNI now).
>
> ---
>
> ## Notes
>
> - Work lands on `feat/chat-agent-picker-remove-goals` via a dedicated git
>   worktree (the current `fix/claude-session-id-worktree-encoding` checkout is
>   dirty with foreign WIP — never disturb it; stage only own hunks).
> - The branch name implies "remove goals" — out of scope here unless the user
>   confirms; this spec keeps the goal model and fixes/extends the picker. Confirm
>   intent before any goals removal.
> - UI file: `crates/agentum-desktop/ui/src/components/harness/ChatPage.tsx`;
>   client: `runtime/board-client.ts` (+ `agentum-server-client.ts` for agents).
