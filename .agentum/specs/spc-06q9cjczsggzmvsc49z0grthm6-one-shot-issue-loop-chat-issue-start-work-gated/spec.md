---
schema: 1
id: SPC-06Q9CJCZSGGZMVSC49Z0GRTHM6
revision: 1
title: One-shot issue loop (chat → issue → Start work → gated run → Done)
source: legacy-import:ai/specs/005-one-shot-issue-loop/spec.md@sha256:3b525b21158ae460c74dd773fb4027766a37fcfb3e9ce10cbf2423409df75f6c
---

# One-shot issue loop (chat → issue → Start work → gated run → Done)

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

> # Spec 005 — One-shot issue loop (chat → issue → Start work → gated run → Done)
>
> - **Number:** 005
> - **Status:** Done             <!-- Draft | PM | Architect | In progress | Done -->  (Reviewer sign-off 2026-07-02; release human-gated)
> - **Surface:** `crates/agentum-server` (routes/harness, task_sink, harness prompts, routes/mcp) + `crates/agentum-desktop/ui` (composer, TaskPage)
> - **Author:** Mateo Cerquetella (drafted with Claude)
> - **Date:** 2026-07-02
>
> ## Problem
>
> Every piece of the intended loop exists, but the loop itself still needs four
> manual hops and two blind spots. Today: Chat files the issue (✅), "Use" opens
> the composer and a workspace is born with the issue linked — and, if the user
> found the opt-in toggle, with a spec + tracker-stamped backlog already inside
> it (✅ spec 004). Then it stalls: **nothing registers or runs the Harness
> Engine in that workspace**, so the agent that spawns is a plain, ungated
> session; the issue's status never moves; the engine's feature prompt (when the
> engine IS used, from its separate page) never mentions the spec sitting beside
> it; QA is steered at a Playwright skill instead of the in-app
> `agentum_browser`; and an agent iterating outside the engine (e.g. an
> `/sdd-loop` session) has **no way at all** to move a ticket — no MCP tool, no
> API route. GitHub statuses are also locked to four hardcoded label names,
> while Linear already honors custom state names.
>
> ## Goal
>
> One click on a linked issue starts a verification-gated run in its own
> workspace and the external board tracks it live to Done — no further human
> hops.
>
> ## Users / personas
>
> - **Mateo (solo multi-agent operator):** describes a feature in Chat, gets the
>   issue, clicks "Start work" — and expects to next look at the workspace only
>   when the board says Ready to Test. Today he must open the Harness page, type
>   the worktree path, register, run, and hope the composer's plain agent isn't
>   also editing the same tree.
> - **Anyone watching the board (GitHub/Linear):** wants live status — including
>   teams whose GitHub repos use their own status label names, not agentum's
>   canonical four.
> - **An agent driving itself with `/sdd-loop`:** does real gated work but is
>   invisible on every board because it has no status verb to call.
>
> ## Acceptance criteria
>
> *Increment F1 — Start work = gated run (the headline):*
>
> 1. The composer, when a GitHub issue is linked, renders a **"Start gated run"**
>    affordance (alongside the existing plain-agent path); submitting with it ON
>    performs, server-orchestrated after worktree creation: spec scaffold + plan
>    (existing `POST /api/harness/spec-from-issue` with `plan: true` — forced ON
>    for this path; the composer's 004 D5 scaffold call is skipped when the
>    toggle is armed, and an already-existing spec for this issue is not a
>    failure: the orchestration plans from the existing spec instead of
>    surfacing the route's never-overwrite 400 — retries and the D5-toggle
>    overlap must both converge, not error), harness registration
>    (`HarnessEngine::start` seam behind `POST /api/harness`), and run kick-off
>    (`POST /api/harness/{id}/run` seam) — and the workspace's linked issue
>    carries `status/in-progress` (or the mapped equivalent) once the first
>    feature agent spawns, with no clicks after the composer submit.
> 2. In the same submission, the composer does **not** double-spawn: today's
>    post-create `openCreatedWorkspace` opens the selected agent with the prompt
>    as an **editable draft** (never submitted — `promptDelivery: 'draft'`,
>    `lib/open-created-workspace.ts:56-61`, mirroring the picker path
>    `WorkspaceAgentLauncher.tsx:88`); with "Start gated run" ON, all three
>    plain-delivery paths are skipped — the draft-open, the no-agent
>    `stashPendingSessionPrompt` fallback (`open-created-workspace.ts:65`), and
>    the `issueCommand` automation launch — and the engine's sessions (which
>    auto-submit via the two-step `inject_prompt`) are the only agents in the
>    worktree. The composer's selected agent/model are written into the run's
>    `FeatureList.agent_tool`/`agent_model` knobs by the start-work seam after
>    the plan step (the plan itself writes defaults). The typed prompt is not
>    delivered in this mode; the UI says so.
> 3. The Tasks page issue-row dropdown gains the same "Start gated run" action
>    (it pre-fills the composer with the toggle armed — reusing
>    `openComposerForItem`), so the chat→Tasks→click path reaches the loop.
> 4. The plan step fires the initial `TrackerPhase::Todo` transition (parity
>    with `plan_goal_harness`, `routes/board_goals.rs:604-616`) so the label
>    trail starts at plan time, not first-spawn. The call lives in the
>    spec-from-issue plan branch, so the 004 opt-in scaffold path inherits it —
>    intentional (best-effort, idempotent label flip; a second Todo from any
>    other path is a no-op).
> 5. A failure in scaffold/register/run surfaces as a toast + harness event but
>    never rolls back the created workspace (spec 004's non-fatal contract).
>
> *Increment F2 — the agent sees the spec:*
>
> 6. The spec-from-issue plan step stamps `FeatureList.spec_id = <spec_id>` on
>    the backlog it writes (today `plan_from_spec_inner`, `harness/types.rs:927-957`,
>    writes `..FeatureList::default()` — `spec_id: None` — so the condition below
>    can never fire without this); then `build_feature_prompt`
>    (`harness/helpers.rs:33`), when the run's backlog carries a `spec_id`,
>    includes the spec's relative path (`.agentum-harness/specs/<id>/spec.md`)
>    and an instruction to read it before coding. Two cases stay byte-identical,
>    pinned by test: a backlog without `spec_id`, and a feature with an explicit
>    `prompt` override (the `helpers.rs:34-36` short-circuit).
>
> *Increment F3 — QA drives the in-app browser:*
>
> 7. `build_qa_prompt` (`harness/helpers.rs:141`) steers the QA agent to the
>    `agentum_browser` MCP tool (open/split = visible in-app browser) instead of
>    the `browser-verification-loop` skill; the verdict-file contract
>    (`qa/<feature_id>.json`, missing/garbled = FAIL) is unchanged.
> 8. `resolve_qa_mode`'s `Auto` arm (`harness/drive.rs:407-423`; today gated on
>    `AGENTUM_BROWSER_VERIFY` via `playwright_mcp::feature_enabled`) gains a
>    persisted config knob (Settings-writable) that, when explicitly enabled,
>    makes agent-QA capable without the env var. The knob is **default OFF**
>    (D3): with it off and no `AGENTUM_BROWSER_VERIFY`, the Auto matrix is
>    byte-identical to today — non-web projects keep the Script skip-pass and
>    headless/CI is unchanged. Pinned by a `resolve_qa_mode` matrix test
>    (mode × qa.sh-present × env × knob).
>
> *Increment F4 — a status verb for out-of-engine agents:*
>
> 9. A new MCP tool `agentum_report_status` (routes/mcp.rs `tool_specs` +
>    `call_tool` arm) accepts `{provider, id, url?, phase}` — `id` is the
>    provider's stable handle (board key / Linear identifier / GitHub issue
>    number; the GitHub arm may derive it from `url`) — and delegates to
>    `apply_tracker_transition` (`task_sink.rs`), returning the
>    `Applied`/`Skipped` outcome; it is best-effort by contract (never an MCP
>    error for a tracker hiccup) — so an `/sdd-loop`-driven session can keep the
>    board live.
>
> *Increment F5 — custom GitHub status names:*
>
> 10. A `GithubStateMap` mirroring `LinearStateMap` (`linear.rs:182-257`)
>     resolves the four phase→label names with precedence defaults →
>     `github.json` `state_map` (Settings-written) → `AGENTUM_GITHUB_STATUS_*`
>     env; `apply_tracker_transition`'s GitHub arm ensure-creates and flips the
>     **configured** names while preserving the invariants: exactly one
>     configured status label per issue after any transition, foreign `status/*`
>     labels (e.g. `status/qa*`) never touched, every failure `Ok(Skipped)`.
>
> ## Scope & non-goals (YAGNI)
>
> - **In:** the five increments above; GitHub + the existing Linear/board arms;
>   local worktrees only (the spec-from-issue seam is already local-only).
> - **Out:**
>   - GitHub ProjectV2 column sync (`gh_projects.rs` stays read-only — custom
>     status = configurable **labels**, same transport as spec 004 D2/D3).
>   - Auto-closing the issue on Done (spec 004 D1 stands: label-only).
>   - Chat auto-starting work with **zero** clicks (the click is the consent
>     gate; a "Start work" button on the chat result card is a named follow-up).
>   - Replacing or changing the plain issueCommand path — it remains the
>     non-gated lightweight alternative when "Start gated run" is off.
>   - Remote (SSH-host) start-work orchestration.
>   - LLM-authored spec prose (the deterministic `spec_md_from_issue` transform
>     stands).
>   - Changing `/sdd-loop` itself (a Claude-Code-side command; F4 only gives
>     such sessions a verb — adopting it is prompt/docs territory).
>   - The default-OFF spec-013 role gates (`FeatureList.roles`) stay default-OFF.
>
> ## Reuse vs build (ground in code)
>
> ### Already exists — do NOT rebuild
>
> - **Issue → spec → tracker-stamped backlog, one call:**
>   `POST /api/harness/spec-from-issue` (`routes/harness.rs:220-295`) — fetches
>   the issue server-side, writes `spec.md` (never overwrites), and with
>   `plan: true` (server default) runs `plan_from_spec_with_tracker`
>   (`harness/types.rs:915`) stamping `tracker_provider`/`tracker_url` on every
>   feature. The UI client exists (`runtime/github-issue-client.ts:125`,
>   composer hook `useComposerState.ts:2064-2090`).
> - **Register + run routes:** `POST /api/harness` and `POST /api/harness/{id}/run`
>   with UI clients `startHarness`/`runHarness`
>   (`runtime/harness-client.ts:149,216-218`) — today called only from the
>   standalone Harness page (`HarnessEngine.tsx:588,602`).
> - **The engine loop itself:** drive → per-feature prompt injection →
>   `verify.sh` gate → QA gate → retries → `transition_tracker` at
>   InProgress/ReadyToTest/Done — all shipped; F1 adds **zero** control-flow
>   changes there.
> - **GitHub label machinery:** ensure-create + exactly-one-canonical-label flip
>   in `task_sink.rs` (`GITHUB_STATUS_LABELS` `:247-252`, argv builders
>   `:265-298`, github arm `github_transition_with` `:365-378`) — F5
>   parameterizes the names, it does not re-plumb the transport.
> - **Custom-state precedent:** `LinearStateMap::from_env` + `linear.json`
>   `state_map` + env overrides (`linear.rs:182-257`) — F5 mirrors it.
> - **Composer plumbing:** linked-item state, `openComposerForItem`
>   (TaskPage → composer prefill, declared `TaskPage.tsx:2349`, row-action
>   caller `:2397`), issueCommand template suppression logic
>   (`useComposerState.ts:820-878`) — F1/F2 hang off these seams.
> - **MCP tool pattern:** `tool_specs()` + `call_tool` arm (`routes/mcp.rs`);
>   `agentum_browser` is already first-class there and every local Claude/Codex
>   launch is MCP-wired by default (`mcp_provision.rs`).
>
> ### Build new
>
> - A server-side start-work orchestration seam (new route
>   `POST /api/workflows/start-work` **or** composer-driven sequencing of the
>   three existing calls — architect decides; the Tasks-page caller argues for
>   server-side).
> - The composer "Start gated run" affordance + plain-agent suppression +
>   agent/model threading into `FeatureList` knobs.
> - The `Todo`-at-plan transition call inside the spec-from-issue plan step.
> - The two prompt-builder edits (F2, F3) + `resolve_qa_mode` knob.
> - `agentum_report_status` MCP tool (thin delegation, like every other tool).
> - `GithubStateMap` + `github.json` `state_map` read/write (Settings pane
>   field) + env overrides.
>
> ## Risks & invariants
>
> - **One launch path:** the engine already spawns via `spawn_agent_into_pane`;
>   F1 must not add a second spawn path — suppressing the composer's plain agent
>   is what keeps one agent per worktree.
> - **The gate is sacred:** F1 wires INTO the engine, never around it; a red
>   gate still blocks; QA verdict missing/garbled still fails (F3 keeps the
>   contract).
> - **Best-effort tracker (sacred):** F4's MCP tool and F5's custom names must
>   keep `Ok(Skipped)`-never-`Err`; a tracker hiccup never halts a run or
>   errors an MCP call.
> - **Exactly-one-status-label invariant** must hold under **custom** names,
>   including transition safety when the map changes mid-flight (old-name labels
>   should be removed if they are among the configured set; foreign labels
>   stay untouched).
> - **Registry serde hazard:** F1 touches composer submit, not the worktree
>   registry shape — the `Worktree` struct must stay serde-alias-free (spec 004
>   wipe hazard).
> - **Double-driver risk:** `claim_driver` already rejects a second run; the
>   start-work seam must surface that as a friendly state, not an error toast
>   loop.
> - **Prompt regression risk:** F2/F3 are string edits to load-bearing prompts —
>   pin the no-spec and existing-QA cases byte-identical / contract-identical in
>   tests.
>
> ## Harness wiring (the gate)
>
> - **feature_list.json entries (build order — value first, deps before
>   dependents):**
>   - `F1 start-work-gated-run` (AC 1–5)
>   - `F2 spec-aware-feature-prompt` (AC 6)
>   - `F3 qa-agentum-browser` (AC 7–8)
>   - `F4 mcp-report-status` (AC 9)
>   - `F5 github-state-map` (AC 10)
> - **`verify.sh` asserts (unit gate):** `cargo test -p agentum-server --lib`
>   green with new tests: start-work orchestration calls scaffold→plan→register→
>   run in order and fires `Todo` (seam-level, stubbed engine); plain-agent
>   suppression flag round-trips; `build_feature_prompt` with/without `spec_id`
>   (no-spec case byte-identical); `build_qa_prompt` names `agentum_browser`;
>   `resolve_qa_mode` knob matrix; `agentum_report_status` delegation +
>   never-`Err`; `GithubStateMap` precedence (defaults/json/env) + the
>   exactly-one-configured-label invariant under custom names. Plus
>   `npm run build --prefix crates/agentum-desktop/ui` green.
> - **`qa.sh` asserts (browser QA gate):** against a scratch repo: Chat files an
>   issue → Tasks row "Start gated run" → composer submits → workspace appears
>   with spec + backlog inside, exactly one agent running, issue shows
>   `status/in-progress` (then a custom-named map variant of the same check) →
>   a feature going green flips `status/ready-to-test` → QA verdict file written
>   by an `agentum_browser`-driven agent → `status/done`, issue still open.
>
> ## Decisions (PM-locked)
>
> > Auto-resolved defaults (autonomous run, 2026-07-02): recommendations locked as
> > scope decisions when the loop was armed; overridable by a human note in
> > `ai/STATE.md`. D3 deliberately overrides the draft's AC 8 leaning (see AC 8).
>
> 1. **D1 — Orchestration is server-side, one route.** A single start-work route
>    (the architect picks the path — `POST /api/harness/start-work` keeps it in
>    the existing namespace; `/api/workflows/*` acceptable if more verbs are
>    foreseen) sequences scaffold+plan → register (`HarnessEngine::start`,
>    `harness.rs:75`) → run, and owns the one failure surface. Every step is
>    already a server seam, the Todo-at-plan transition and the `FeatureList`
>    knob writes are filesystem/server work a browser client can't do, and the
>    Tasks-page action + the named chat-card follow-up must share it.
>    Client-side sequencing would triplicate partial-failure handling.
> 2. **D2 — Adoption, not co-existence.** With "Start gated run" ON, the
>    composer's selected agent/model become the run's
>    `FeatureList.agent_tool`/`agent_model` (written post-plan by the start-work
>    seam); the composer skips **all three** plain-delivery paths in
>    `open-created-workspace.ts` — the `launchAgentInNewTab` draft-open
>    (`:56-61`), the no-agent `stashPendingSessionPrompt` fallback (`:65`), and
>    the `issueCommand` automation launch (it runs a user-configured command in
>    a terminal in the same worktree — typically an agent invocation on the same
>    issue — which would reintroduce the double-agent problem AC 2 exists to
>    prevent; see `worktree-activation.ts:413-423`). The typed prompt is not
>    delivered anywhere in this mode (the issue is the sole prompt source); the
>    UI communicates that. "Exactly one agent running" is a qa.sh assert and the
>    one-launch-path invariant's UX twin. Repo `setup`/`defaultTabs` still apply
>    (project config, not agents).
> 3. **D3 — QA capability stays opt-in; the knob is a second opt-in door,
>    default OFF.** Keep `AGENTUM_BROWSER_VERIFY` honored; add a persisted
>    config knob (Settings-writable) that makes `resolve_qa_mode`'s `Auto` arm
>    treat agent-QA as capable **when explicitly enabled** — do NOT make the
>    embedded desktop default-capable in this spec. Evidence: today `Auto` + no
>    `qa.sh` + no env resolves to `Script` skip-pass (`harness/drive.rs:411-420`),
>    which is what lets non-web projects advance; flipping the default converts
>    that pass into a spawned QA agent whose missing/garbled verdict **fails the
>    gate by contract** — every non-web run would block at QA. Default-capable
>    is a follow-up once `agentum_browser` QA has earned trust (mirrors 004 D5's
>    "earned trust" pattern). Headless/CI unchanged by construction.
> 4. **D4 — Global `github.json` `state_map`, mirroring `linear.json`.** Exact
>    precedent: `LinearStateMap::from_env` (`linear.rs:206-245` — defaults →
>    creds-file → env). Per-repo adds schema + resolution surface with no
>    persona demanding it; per-repo overrides are a named follow-up
>    (`tracker_url` already carries the slug if that ever lands).
