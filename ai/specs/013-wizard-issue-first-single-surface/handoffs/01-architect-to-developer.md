# Handoff 01 — Architect → Developer

- **Spec:** 013-wizard-issue-first-single-surface
- **Date:** 2026-07-08
- **From:** Architect (autonomous /sdd-loop iteration 1)
- **To:** Developer
- **Artifacts:** `architecture.md` (complete blueprint, 11 sections)

## Gate result

Architect gate: **PASS** — every AC has a design home (§3: F1 AC1–3, F2 AC4–7,
F3 AC8, F4 AC9–11); seams grounded in real code; 8 invariants numbered (§2);
tradeoffs + rejected alternatives stated (§10); per-slice first-failing tests
map to the gate (§8); the 4 open questions resolved with decisive defaults +
non-blocking carry-forwards flagged (§9, §11).

## ⚠️ Environment note (read FIRST — the #1 rule)

This worktree is **76 commits behind `origin/develop`**. The wizard
(`CreateWorkspaceWizard.tsx`), the work-item picker (`work-item-picker-model.ts`,
`WorkItemPicker`), spec 012's bind (`applyLinkedWorkItem` / `buildBindPayload` /
`deriveTrackerBindCoords`), and the issue draft/create routes are **stale or
entirely ABSENT locally**. Every `:line` in the blueprint is **approximate** —
**re-ground each seam on fresh `origin/develop`** (`git show origin/develop:…` /
`git grep origin/develop`) and confirm the reuse target still exists BEFORE
writing code. **Reuse-shipped-seams-over-rebuild is invariant #1.**

**Number-collision note (cosmetic, out of scope):** `origin/develop` already
carries a *different* released spec 013 (`013-mission-control-and-browser-fixes`,
v0.64.0). This spec's directory (`013-wizard-issue-first-single-surface`) is new
and local. When the Developer branches/PRs, expect the "013" number to already
be used by a released spec — pick a non-colliding branch/issue identity; don't
try to reconcile the two.

## Build order + first move

F1 → F2 → F3 → F4, each an independently gated slice. **Shared gate:** backend
`cargo test -p agentum-server --lib`; UI build `bun run build --prefix
crates/agentum-desktop/ui`; UI model `bunx vitest run`. **No `tsc` gate**
(`shared/*` is a vite alias — grep-pin, don't typecheck; jsdom-free pure-model
tests only). Commit per green slice; stage only your files (concurrent-checkout
rule).

**First failing test (Slice F1):** `create-workspace-wizard-model.test.ts` →
`deriveUnifiedTrackerStatus never reports "none" when a Project resolves` (pure,
jsdom-free) — asserts `resolved != null` ⇒ never `{kind:'none'}`, `resolved ==
null` ⇒ `{kind:'none'}`, so the AC3 contradiction is structurally impossible.

## Non-negotiables (from the blueprint §2)

1. **Reuse shipped seams, never rebuild** — F1 reuses `resolvePickerProject` /
   `deriveIssueOptions` / `buildBindPayload` untouched; F2 reuses
   `useComposerState`'s create-issue seams (`onGenerateIssueBody`→
   `draftGithubIssueBody`, `onCreateIssueSubmit`→`createGithubIssue`) +
   `applyLinkedWorkItem`; F4 reuses `submitQuick` / `maybeStartGatedRun` /
   `firstStartGatedRunBlocker`.
2. **One source of truth for F1 honesty** — after F1 no second detection path can
   disagree with the picker. `deriveUnifiedTrackerStatus` reads SOLELY from
   `resolvePickerProject`'s `resolved` (+ status/optionCount). `deriveWizardTracker`
   is **deleted as the display driver** (may not gate any visible tracker text).
3. **Serde-alias-FREE bind** — a *created* issue binds via the existing
   `applyLinkedWorkItem`/`buildBindPayload`/`deriveTrackerBindCoords` shapes. NO
   new persisted `Worktree`/linked field, NO `#[serde(alias)]` (registry-wipe
   hazard).
4. **Gated-run preserved (005/008)** — the wizard's "Start gated run" toggle binds
   to the SAME `cardProps.{canStartGatedRun,startGatedRun,onStartGatedRunChange}`,
   keeps `createGateMode:'quick'` + `enableIssueAutomation:false`, seeds
   `initialStartGatedRun` via `initialStartGatedRunProp`, and submits via the SAME
   `submitQuick(quickAgent)`. No new submit path — inherit the precondition set by
   calling it.
5. **Card removal only after every `openModal('new-workspace-composer')` caller is
   re-homed** — §7 enumerates all 12; verify each opinionated field before
   deleting `QuickTabBody`/`NewWorkspaceComposerCard`.
6. **Wiki grounding best-effort/non-fatal** — `retrieve_wiki_for_query` into
   `draft_issue_body` stays async+optional; a `None` wiki still drafts from repo
   context and never wedges the draft.
7. **Create-issue fail-loud, non-blocking** — no-cred / `no_github_repo` / draft
   failure / Linear-down renders inline; the wizard's "Create workspace" primary
   is never gated on it.
8. **Telemetry parity** — every one of the 12 callers' `telemetrySource` reaches
   the wizard unchanged (none collapses to `unknown`).

## Developer confirmations to make on develop (blueprint §3, §5, §7)

- The exact `cardProps` create-issue seam names (`onGenerateIssueBody`,
  `onCreateIssueSubmit`, `createIssueTitle/Body/Generating/Submitting/Error`) and
  that `useComposerState` still exposes them.
- `draft_issue_body(workdir, repo_slug, title)` + `draft_body_instructions`
  signatures in `routes/chat.rs`, and `retrieve_wiki(workdir, messages)` (~:640)
  so you can extract `retrieve_wiki_for_query(workdir, query)` and delegate (zero
  behavior change for `chat()`).
- `linearCreateIssue`'s exact args — the **`teamId` wrinkle** (§6): it needs a
  team the wizard's GitHub-Projects-centric section doesn't resolve today.
  Default to the sole team when one exists, else a small `linearListTeams`
  picker. **F3 may trail F2/GitHub without blocking release** if this proves
  fiddly.
- The 12 `openModal('new-workspace-composer')` caller fields (§7 table) — verify
  before deleting the card.

## Key files (re-ground each on develop)

UI: `components/new-workspace/CreateWorkspaceWizard.tsx` (`AgentStep`, the
Tracker + `WorkItemPicker` blocks, `CreateWorkspaceWizardData`, the footer
"Start from a goal"), `components/new-workspace/work-item-picker-model.ts`
(`resolvePickerProject`/`deriveIssueOptions`/`buildBindPayload` — reuse), **new**
`create-workspace-wizard-model.ts::deriveUnifiedTrackerStatus`, **new**
`components/new-workspace/create-issue-intent-model.ts`,
`components/NewWorkspaceComposerModal.tsx` (collapse to wizard-only),
`hooks/useComposerState.ts` (`applyLinkedWorkItem`, `submitQuick`, create-issue
seams, `createGateMode`/`enableIssueAutomation`), `lib/composer-modal-props.ts`
(`initialStartGatedRunProp` — retain), `lib/start-gated-run-precondition.ts`
(retain), `lib/workspace-goal-step.ts` (keep `deriveGoalIssueDraft`; trim dead
exports), `runtime/github-issue-client.ts` + `runtime/runtime-linear-client.ts`.
Server: **only** `crates/agentum-server/src/routes/chat.rs` (`draft_issue_body`,
`draft_body_instructions`, `retrieve_wiki` → `retrieve_wiki_for_query`) — the
`github.rs` route + the `draftGithubIssueBody` client need NO signature change.

## Reviewer focus (carry forward — §11)

F1 single-source (`deriveWizardTracker` deleted as display driver; "none"
unreachable while issues list) · F2 wiki best-effort (None still drafts, no
signature change) · F2/F3 fail-loud non-blocking (inline error, primary never
disabled) · serde-alias-free bind · F4 gated-run parity (same `submitQuick`,
precondition inherited not re-implemented) · F4 caller re-homing (all 12,
telemetry preserved, orphan files deleted only when knip-clean).

## Carry-forwards genuinely needing Mateo (non-blocking — F2/GitHub ships regardless)

- **F3 Linear `teamId`** — create arm needs a team; default-to-sole-team or a
  small picker. F3 can trail F2 without blocking release.
- **Provision hop removed** from the create flow (capability remains in
  Settings/hub + the gated-run `start_work` path; §7, open question 3).
