---
schema: 1
id: SPC-114RCAHT0JCKPDG0F5TFMJATC0
revision: 1
title: Start an external ticket → the agent gets the spec (no internal board)
source: legacy-import:ai/specs/002-start-loads-spec/spec.md@sha256:98d6d1ab792f3fe06bab72c88c9a9db1dcc58a59fec09fcc289a8d3b491efc9b
---

# Start an external ticket → the agent gets the spec (no internal board)

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

> # Spec 002 — Start an external ticket → the agent gets the spec (no internal board)
>
> - **Number:** 002
> - **Status:** PM  <!-- Draft | PM | Architect | In progress | Done -->
> - **Surface:** `crates/agentum-server/src/routes/board_goals.rs` (Start path) + `routes/chat.rs` (issue body, already done) + desktop UI (Start surface)
> - **Author:** Claude (drafted with Mateo)
> - **Date:** 2026-06-30
> - **Base:** `feat/chat-spec-roundtrip` off `origin/feat/autowiki` (= current develop `fe1a2a6a` + AutoWiki + the `ai/` scaffold)
>
> ## Problem
>
> Chat already files a good external ticket: `chat_issues` (`routes/chat.rs:950`)
> extracts a `FeaturePlan` (grounded by `gather_repo_context`), and
> `compose_issue_body` (`chat.rs:914`) writes the issue **title + a body** (summary +
> priority-ordered sub-task checklist) via `create_github_issue` / `create_linear_issue`
> (`chat.rs:1050/1143`) — explicitly **GitHub/Linear only, never the internal board**
> (`chat.rs:1011/1022`).
>
> But that spec **dies at the door.** When the user clicks **Start**, the spawned
> agent's opening prompt is built from a board **card's own `title`/`body` columns**
> (`build_card_prompt`, `board_goals.rs:861`) — it **never fetches the external issue
> body** (the spec the user authored). The agent starts with
> `"Working on <key>: <title>"` and nothing else. Worse, Start is **hard-coupled to
> the internal board**: `spawn_card_session(card: &BoardItem)` (`board_goals.rs:737`)
> requires a card row — there is no way to Start an external GitHub/Linear ticket
> directly, even though the internal board is deprecated (external boards only).
>
> ## Goal
>
> Click **Start** on a GitHub/Linear ticket → the spawned agent's opening prompt is
> seeded with that ticket's full **body** (the spec), fetched live — and Start runs
> off the external ticket **without requiring an internal-board card**.
>
> ## Users / personas
>
> - **The operator who specs in Chat.** They describe a feature in Chat → a
>   GitHub/Linear issue with the plan is filed → they click **Start** expecting the
>   agent to build exactly that, and are surprised when it begins with only a
>   one-line title.
>
> ## Acceptance criteria
>
> 1. Clicking **Start** on a GitHub/Linear ticket fetches the ticket's current
>    **body** and includes it (with the title) in the spawned agent's opening prompt
>    — observable: the agent's first prompt **contains the issue body**, not just
>    `"Working on <key>: <title>"`.
> 2. Start works on an **external ticket directly** — no internal-board `BoardItem`
>    row must exist first for the spawn to happen.
> 3. The Start path **never reads or writes the internal Board** — no `TaskSink::Board`,
>    no board card minted (GitHub/Linear only).
> 4. **No regression on creation**: Chat-filed issues keep their title + body
>    (summary + checklist), external-only (`compose_issue_body` unchanged unless
>    enriched per Open Q3).
> 5. If the ticket body can't be fetched, the agent still **starts gracefully** with
>    the title + a clear note — never a silent bare prompt; any `gh`/Linear error is
>    redacted of secrets (reuse `redact`, `chat.rs`).
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** Start reads the external ticket's body → agent prompt; Start off an
>   external ticket (no card required); external-only.
> - **Out (deferred / separate):**
>   - **GitHub issue STATE transitions** (Todo→In Progress→Done) as the agent works
>     — the `task_sink.rs` "github issue state sync not implemented" no-op is a
>     **separate spec** (003).
>   - **Enriching the issue body** beyond the current summary+checklist (Open Q3).
>   - **Deleting the internal Board code** wholesale — this spec only removes it from
>     the Start path.
>   - Linear-only Start nuances beyond reading the body.
>
> ## Reuse vs build (grounded — current develop)
>
> ### Reuse — do NOT rebuild
> - **Issue creation (already title+body+external-only):** `chat_issues` +
>   `compose_issue_body` (`chat.rs:914`) + `create_github_issue`/`create_linear_issue`
>   (`chat.rs:1050/1143`), `NewFeature { title, body }` (`chat.rs:1103`).
> - **Spawn + prompt-inject:** `spawn_card_session` (`board_goals.rs:737`) →
>   `spawn_agent_into_pane` + `inject_prompt` (the one launch path).
> - **External-ticket linkage on a card:** `external_url`/`external_id`/
>   `external_provider` (core `BoardItem`) + the GitHub pull-sync (`board_sync.rs`)
>   — the existing issue↔card bridge.
> - **Reading an issue body:** the `gh` client (`gh issue view <n> --json body`) /
>   the Linear GraphQL client (`linear.rs`).
>
> ### Build new
> - A Start prompt builder that, given an external ticket (provider + id/url),
>   **fetches the body live** and seeds the agent prompt from it — replacing /
>   augmenting `build_card_prompt`'s card-columns-only prompt (`board_goals.rs:861`).
> - A way to **Start an external ticket without first minting an internal card** —
>   decouple the spawn from `BoardItem`, or add an external-ticket start entry.
> - The desktop **Start surface** for an external ticket (Open Q2).
>
> ## Risks & invariants
>
> - **One launch path** — Start MUST keep going through `spawn_agent_into_pane`.
> - **No internal board** — do not reintroduce `TaskSink::Board` / a minted card on
>   the Start path (AC-3).
> - **Graceful on fetch failure** (AC-5) — a missing/erroring body must not block the
>   spawn or leak the token.
> - **Live fetch cost** — a `gh`/Linear call per Start (vs reusing a synced
>   `card.body`); see Open Q4.
>
> ## Harness wiring (the gate)
>
> Proposed `.harness/feature_list.json` slices:
> 1. `start-reads-issue-body` — fetch the external ticket body + build the agent
>    prompt from it (the spec), graceful fallback.
> 2. `external-ticket-start` — Start off an external ticket without an internal card.
>
> - **`verify.sh` asserts:** a unit test that the Start prompt **contains a stubbed
>   issue body** (not just the title); `cargo test -p agentum-server --lib` green.
> - **`qa.sh` asserts:** in-app, file an issue from Chat → click Start → the agent's
>   first message reflects the issue's full spec (browser-verification-loop).
>
> ## Resolved (with Mateo, 2026-06-30)
>
> 1. **Creation is fine — this spec is Start-only.** Chat already writes title + body
>    external-only on develop; the empty issues were the **installed app being behind**
>    (~v0.42), not a bug. Spec 002 does **not** touch issue creation.
> 2. **Start runs directly off the external ticket — no internal-board card.** Start
>    takes a GitHub/Linear ticket (provider + id/url), **fetches its body live**, and
>    seeds the agent prompt from it.
> 3. **"Spec" = the issue body** (summary + checklist). No separate richer-spec store.
> 4. **Live fetch** at Start (there is no card to read from in the direct path).
>
> ## Open questions (for the architect)
>
> 1. **UI Start surface** — with no board card, where does "Start this ticket" live?
>    A button on the in-app GitHub/Linear ticket view? Confirm the desktop surface.
> 2. **Spawn decoupling** — fully decouple `spawn_card_session` from `BoardItem`, or
>    add a parallel `start_external_ticket` that shares the worktree + launch +
>    inject internals? And the worktree name / session FK when there is no card
>    `key` (use the issue number, e.g. `ticket-<provider>-<n>`?).
> 3. **Body fetch client** — `gh issue view <n> --json body` (GitHub) + the
>    `linear.rs` GraphQL fetch (Linear), mapped to one `fetch_ticket_body(provider, id)`.
