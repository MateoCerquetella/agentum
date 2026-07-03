# Spec 008 — Finish the Loop

- **Number:** 008
- **Status:** Architect
- **Surface:** `crates/agentum-server` (harness, chat routes) + `crates/agentum-desktop/ui` (Chat, composer, workspace creation)
- **Author:** Mateo (Socratic interview, 5 passes, 2026-07-03)
- **Date:** 2026-07-03

## Problem

The end-to-end loop that IS agentum's product — chat → spec → GitHub/Linear
issue → gated agent run — breaks at the last step: clicking **Start gated
run** on a GitHub issue usually opens no session at all, and when a session
does open the agent never gets driven (no prompt, no loop, no gate). The
failure is silent, so the one person using this daily (Mateo) falls back to
hand-driving terminals — the exact toil agentum exists to remove — and the
product cannot be demoed or shown to anyone.

## Goal

Make the core loop complete end-to-end, hands-off and never-silently, in the
installed app: a goal described in Chat becomes a spec, becomes filed issues,
becomes a one-click gated run that drives to a green (or visibly blocked)
gate with live tracker updates.

## Users / personas

- **Mateo, solo dogfooder** — drives agentum daily on real projects. Feels the
  break in three moments:
  1. Clicking **Start gated run** on an issue → dead click or inert session.
  2. Right after Chat files issues → issues exist, but no agent picks them up.
  3. Creating a new workspace → the flow doesn't set up the pipeline (spec
     scaffold, tracker binding) that a later run needs to stand on.
- Secondary (why-it-matters, not a design target): anyone Mateo demos agentum
  to — a visibly broken core flow blocks showing/selling the product.

## Acceptance criteria

Ordered slices F1 → F3; each criterion is independently gateable.

**F1 — the run actually runs (the spine):**

1. Clicking **Start gated run** on a GitHub issue (Tasks-page action →
   pre-armed composer → submit, and the composer toggle directly) yields, in
   the installed app: (a) a visible in-app acknowledgment (pending state /
   event-log entry) within **2 s** of the click, and (b) within **15 s** on the
   demo project (D7), either a **visible session** or a **visible, actionable
   error stating why**. The 200-`alreadyRunning` friendly state renders visibly
   too. A silent no-op — including a silent skip by `deriveIssueSideEffectGate`
   — is a failed criterion in itself.
2. The spawned agent receives the spec/issue-grounded prompt (the injected
   prompt text is visible in the pane or transcript) and produces non-empty
   pane output within **60 s** of session spawn (D7) — well inside the settle
   window (`settle_grace_secs` default 8 s / `settle_timeout_secs` default
   1800 s, `harness/types.rs:97–102`).
3. The loop **drives**: settle → verify gate → advance/retry/block, and the
   issue's `status/*` labels flip live at each transition: `status/todo` at
   plan, `status/in-progress` at agent spawn, `status/ready-to-test` at
   unit-gate green, `status/done` at QA green (spec 004 D3 canon, via
   `apply_tracker_transition`).
4. A red gate or blocked feature is **loudly surfaced** in-app (event log /
   board state) **and on the issue** (a comment carrying retry count +
   gate-output tail, and the `status/blocked` label per D6) — never silent.

**F2 — Chat embeds real SDD intake (Fast / Complex):**

5. The Chat composer **renders two entry buttons**, labeled **Fast feature**
   and **Complex feature** (glyphs non-normative).
6. **Fast feature** routes to the existing single-prompt intake unchanged: the
   fast-mode system prompt is byte-identical to today's (pinned by a unit test,
   same technique as the pre-006 body pin), with no added mandatory turns.
7. **Complex feature** runs a staged Socratic interview of five passes (WHO →
   WHAT → WHY → done-criteria → risks): the stage **advances exactly one pass
   per user turn and never skips** (unit-tested progression per D1), each
   stage's system prompt covers exactly one pass topic and instructs reflecting
   the previous answer back before advancing (per-stage prompts pinned by unit
   test; the reflect-back behavior itself is QA-observed), and after the fifth
   pass the interview converges to the same draft/preview endpoint as Fast.
8. Both modes **end the same way**: GitHub/Linear issues whose bodies carry the
   SDD-shaped spec content (existing `## Problem` / `## Goal` /
   acceptance-checklist shape, `compose_issue_body`), from which the existing
   `spec_md_from_issue` round-trip materializes the spec at start-work (D8) —
   no new chat-time file write.

**F3 — Create New Workspace, goal-first:**

9. The default create-workspace entry renders a goal text input as its first
   step; no repo/branch field is required before the goal is captured.
10. Fresh-worktree creation, SDD/spec scaffold, and tracker (GitHub/Linear)
    binding are offered as optional steps — each skippable (skipping worktree
    creation uses an existing folder/branch as-is, per D9); the goal plus a
    workdir target are the only required inputs.
11. A workspace created with scaffold + tracker steps accepted **can run
    criteria 1–8 without further setup**.

**Done bar (non-negotiable):**

12. One unbroken demo passes in the **installed release app** (tagged build,
    not dev): create a workspace goal-first → Complex-feature Socratic chat →
    spec → filed issues → one click → green gate with live label flips.
    **Runner: Mateo** (release stays human-gated per standing convention).
    **Evidence: the GitHub issue's label-flip + harness-comment trail** as the
    durable artifact, plus a demo-pass line appended to `ai/STATE.md`'s
    decision log.

## Scope & non-goals (YAGNI)

- **In:** F1 fix + hardening of the issue→run path; F2 Fast/Complex chat
  intake with a real staged Socratic mode; F3 goal-first workspace creation
  with optional steps; the end-to-end installed-app demo.
- **Out:**
  - Multi-user / team flows (solo dogfooder is the persona).
  - New Linear features beyond the existing transition map
    (`apply_tracker_transition` stays the seam).
  - TUI parity (separate repo; this spec is desktop-only).
  - New agent tool adapters, new MCP tools, marketing surfaces.
  - Rewriting the harness engine — F1 fixes the existing path, it does not
    redesign `drive`.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- `POST /api/harness/start-work` (route `harness.rs:46`, handler `start_work`
  at `:508`) — the spec-005 F1 orchestration route. F1 **fixes and hardens**
  this path; it does not add a second one.
- `spawn_agent_into_pane` (`routes/sessions/provision.rs`) — the ONE launch
  path (YOLO translation, pane_env, MCP wiring). Sacred.
- Autonomy mechanics in `harness/drive.rs` — YOLO marker push,
  `await_repl_ready` (workspace-trust dialog), `inject_prompt` two-step
  submit. Hard-won; F1 must diagnose against them, never reimplement them.
- Chat intake: `routes/chat.rs` (`/api/chat` + `/api/chat/stream`, Socratic
  system prompt at `chat.rs:335`) and SDD-shaped issue bodies
  (`chat.rs:981–1047`, `compose_issue_body`, spec 006 F2). F2 layers modes on
  this; the SDD body shape and `spec_md_from_issue` round-trip (spec 004 F4)
  are kept verbatim.
- Issue side-effect gating: `deriveIssueSideEffectGate`
  (`ui/src/lib/issue-side-effect-gate.ts:26`, spec 007) — the single execution
  gate for scaffold/start toggles; F1's UI fixes flow through it, with its
  skip-reason toasts.
- UI start seam (AC 1's click path): `startGatedWork`
  (`ui/src/runtime/harness-client.ts:171`), the Tasks-page pre-armed entry
  `openComposerForItem(item, {startGatedRun:true})` (`TaskPage.tsx:4527–4535`),
  and the composer toggle → `startGatedWork`-after-`createWorktree` with
  `alreadyRunning` handling (`useComposerState.ts:2273–2313`, esp. `:2303`).
  Start gated run is a **two-hop** path, not one button — F1 covers both hops.
- Tracker transitions: `task_sink::apply_tracker_transition` (+ GitHub label
  arm from spec 004 F2) — F1 criterion 3's label flips are this seam,
  best-effort by contract.
- Composer surface: `NewWorkspaceComposerCard.tsx` (~1k lines),
  `NewWorkspaceComposerModal.tsx`, `useComposerState.ts` (~2.8k lines) — F3
  **fronts** this surface with a goal-first entry (D3); the composer primitives
  and the underlying worktree/session creation APIs stay.
- Live end-to-end proof: `tests/harness_live_agent.rs` (`#[ignore]`, real
  Claude agent, asserts green gate) — the pattern F1's regression test
  extends.

### Build new

- **F1:** instrumentation + fixes along start-work → plan → spawn → settle →
  gate, so every failure point emits a visible `HarnessEvent`/toast instead
  of dying silently; a live regression test covering the **issue → start-work
  → session opens → prompt lands** leg (the leg `harness_live_agent.rs`
  doesn't cover today).
- **F2:** an intake-mode parameter on the chat API (`fast` | `socratic`);
  a stateless staged-interview mechanism for the five-pass mode (one pass per
  turn, reflect-back, then draft) — stage travels in the request per D1, server
  owns the per-stage system prompts; the two composer buttons.
- **F3:** goal-first creation flow (describe goal → optional repo/worktree,
  scaffold, tracker steps) reusing the existing composer primitives and
  `POST /api/github/issues` / scaffold routes.

## Risks & invariants

- **Silent regression (top risk, chosen in interview):** the run path has
  broken before without anyone noticing. Mitigation: criterion 1 makes
  silence itself a failure; the new live test covers the issue→run leg; every
  gate in the path must emit an event.
- **Never cache a failed fetch as success** (spec 007 lesson) — applies to
  any new hydration F1/F3 adds.
- **One launch path** — all spawns stay on `spawn_agent_into_pane`; no
  bespoke spawn for the wizard or chat-started runs.
- **Autonomy mechanics are sacred** — YOLO marker, trust-dialog acceptance,
  two-step prompt submit (`drive.rs`). Any F1 fix that touches these needs
  the live test green before merge.
- **Fast must stay fast** — the Socratic mode is opt-in via the Complex
  button only; five passes must never be forced on a small ask, or Chat stops
  being used.
- **Works-in-dev vs installed gap** — history shows Tauri `gh_*` stubs and
  release-only breakage. Done bar (criterion 12) is pinned to the installed
  release app.
- **Scope balloon** — three slices, independently gated; F1 ships alone if
  F2/F3 slip.
- **Tauri `gh_*` stubs on the Start path** — several `gh_*` desktop commands
  remain stubs (spec 007 lesson); the AC 12 demo runs installed, so F1 must
  audit every Tauri command in the TaskPage → composer → start chain and
  surface (not swallow) stub returns.
- **ChatPage `repoId: ''` degenerate edge (#226)** — a Chat-filed issue with no
  pinnedRepo AND no workspaceId silently early-returns armed side effects: an
  AC 1 "never silent" case for the chat-origin start path. F1 either fixes it
  or explicitly re-defers to #226 with a visible skip toast — never silent.
- **No-credentials chat path** — F2's Complex mode must surface the existing
  `NO_CREDS_MSG` (`chat.rs:76`) visibly on its first turn, same as today —
  never a silent dead button.
- **`start_work_lock` serializes the whole orchestration** (architect note) —
  including the network `gh` fetch, so a double-click waits on the lock; AC 1's
  2 s acknowledgment must come from the UI pending state, not the HTTP response.

## Harness wiring (the gate)

- **feature_list.json entries:** `008-f1-run-path` (criteria 1–4),
  `008-f2-chat-intake` (5–8), `008-f3-workspace-goal-first` (9–11); demo
  criterion 12 is the release gate, not a feature entry.
- **`verify.sh` asserts:** workspace lib tests green (incl. new start-work
  leg tests + staged-interview unit tests); `cargo fmt --check` + clippy;
  vite build + vitest for the UI surfaces.
- **`qa.sh` asserts (browser QA):** Start-gated-run click → session visible
  or error visible (never nothing); Fast/Complex buttons render and route to
  distinct behaviors; goal-first wizard completes with all optional steps
  skipped AND with all accepted; label flip visible on a driven issue; a
  blocked feature shows `status/blocked` + comment on the issue (D6).
  ⚠️ The browser-QA assertions require the browser-QA knob armed
  (`AGENTUM_BROWSER_VERIFY` / `browserQaAgentEnabled`, default OFF per spec 005
  F3) — else they pass vacuously.

## Decisions (PM-locked, 2026-07-03)

- **D1 — Interview state is client-side; the server stays stateless.** The
  staged Socratic mode rides the existing `ChatRequest` (client sends full turn
  history; server owns the system prompt — `chat.rs:107–123`) plus an
  intake-mode/stage indicator in the request. Persistence = the existing
  localStorage chat history (survives reload/unmount, PR #240) — reload-survival
  is free, zero new store tables, no second source of truth. Explicit stage vs
  server-derived-from-turn-count is the architect's call. A cleared localStorage
  mid-interview restarts the interview — accepted for a solo dogfooder.
- **D2 — Complex mode uses the same model/config as today's chat; extended
  thinking is NOT required.** The quality lift comes from *staging* (one pass
  per turn, reflect-back), not raw effort. Forcing thinking would break
  OAuth-token users (encrypted-thinking limitation; `resolve_auth`,
  `chat.rs:89–97`) or drag `ANTHROPIC_API_KEY` onboarding into the core loop.
  The existing `thinking` opt-in + model picker apply to both modes. Follow-up
  (out of scope): a per-mode effort knob.
- **D3 — Goal-first is a parallel entry that becomes the DEFAULT; the existing
  composer is NOT deleted.** `NewWorkspaceComposerModal`/`Card`/`useComposerState`
  (~3.8k lines, load-bearing: issue linking, host scoping, scaffold/start
  gating) stay the creation engine and stay reachable ("Skip to details"). F3
  layers the goal step in front and reuses the primitives. Removing the
  mechanics-first entry is a named follow-up once goal-first earns trust
  (mirrors 004-D5).
- **D4 — Fast/Complex is chosen per-feature, every time. No sticky preference.**
  The choice is inherently per-ask (small fix and big feature coexist in one
  workspace); a sticky Complex would force five passes on a small ask, breaking
  the "Fast must stay fast" invariant. A remembered default is a YAGNI follow-up.
- **D5 — F1's `drive.rs` boundary.** F1 may add behavior-preserving
  instrumentation (HarnessEvent emission, error propagation) anywhere including
  `drive.rs`, and may fix bugs anywhere on the start-work orchestration path
  (`start_work`, plan/issue-fetch, spawn call sites). The three autonomy
  mechanics (YOLO marker push, `await_repl_ready`, two-step `inject_prompt`) may
  be *fixed in place* ONLY with `harness_live_agent.rs` **plus** the new
  start-work-leg live test green before merge — never reimplemented or bypassed.
  No new spawn path.
- **D6 — Blocked gets a real label: `status/blocked` joins the canonical set**
  (spec 004 D3 mechanics: ensure-created idempotently, fixed color,
  exactly-one-`status/*` invariant now over five labels). AC 4 demands "comment
  + label"; leaving `status/in-progress` on a blocked issue makes the board lie.
  This is the ONLY extension to the 004 label canon in this spec.
- **D7 — QA numbers (AC 1/2) are demo-project pins, not universal SLAs** —
  asserted by `qa.sh` against the reference demo project (fast `init.sh`), not a
  promise for arbitrary repos with slow init.
- **D8 — "Persisted spec" = the existing round-trip, not a new chat-time file
  write.** Chat's obligation ends at SDD-shaped issue bodies
  (`compose_issue_body`); the spec file is materialized at start-work via
  `spec_md_from_issue` (spec 004 F4), kept verbatim. Preserves the standing
  directive that Chat works with **no local workdir** (GitHub/Linear only).
- **D9 — F3's "optional repo step" = worktree creation is optional, not the
  workdir.** A session is a `(name, workdir, …)` tuple — a workspace without a
  workdir is not a domain object. The three skippable steps are (a) fresh
  worktree creation (skip → use an existing folder/branch as-is), (b) spec
  scaffold, (c) tracker binding; goal + a workdir target are the required inputs.
