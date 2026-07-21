# Architecture — Spec 013: Wizard as the honest, issue-first, single create surface

- **Spec:** `ai/specs/013-wizard-issue-first-single-surface/spec.md`
- **Status:** Architect
- **Surfaces:** `crates/agentum-desktop/ui` (New Workspace wizard step-3 + the modal that hosts it) · `crates/agentum-server` (thread `retrieve_wiki` into `draft_issue_body`). Almost entirely a **UI re-shaping over already-shipped seams** — the picker/bind (spec 012), the composer's create-issue flow (specs 006/007), the gated-run machinery (specs 005/008), and the draft/create routes all exist on `origin/develop`. The only server change is one grounding thread; the only genuinely new UI code is two small pure models + a merged section render.

> **Grounding caveat — re-ground on `origin/develop`, do NOT trust this working tree.** This blueprint was written in the `new-chat-refresh` worktree, which is **76 commits behind `origin/develop`**. The wizard, the work-item picker, spec 012's bind, and the issue draft/create routes this spec builds on are **stale or entirely ABSENT locally**. Every symbol below was read via `git show origin/develop:<path>` / `git grep origin/develop`; all `:line` numbers are **approximate** — cite the SYMBOL and grep. The Developer MUST re-ground each seam on fresh `origin/develop` before writing code, and confirm the reuse target still exists there. (Note: `origin/develop` already carries a *different* spec 013, `013-mission-control-and-browser-fixes`, RELEASED v0.64.0 — this spec's directory is new and local; the number collision is cosmetic and out of scope.)

---

## 1. Design overview — one section, one source, one door

Three thin re-shapings over existing engines. **No new create path, no new submit path, no new bind path.**

```
F1  HONEST UNIFIED TRACKER (UI only) ────────────────────────────────────────
    AgentStep today renders TWO blocks that read from TWO sources:
       "Tracker"  ← deriveWizardTracker(remoteUrl)      ← git-remote heuristic  ← LIES
       "Work item"← resolvePickerProject(binding∨active) ← the real Project     ← TRUTH
    → collapse to ONE section, status driven SOLELY by resolvePickerProject's
      result (the picker's own source), control moved to the TOP header.
      deriveWizardTracker is demoted (deleted as the display driver).

F2  CREATE ISSUE FROM INTENT (GitHub) — surface EXISTING composer seams ───────
    The composer already owns the whole flow inside useComposerState:
       onCreateIssueTitleChange / onGenerateIssueBody (→ draftGithubIssueBody)
       / onCreateIssueSubmit (→ createGithubIssue) → applyLinkedWorkItem
    → surface it in the wizard's unified section as a "what do you want to do?"
      sub-panel; server-side, thread retrieve_wiki into draft_issue_body so the
      draft grounds on wiki + codebase (today: codebase only).

F3  CREATE ISSUE (Linear) — one provider branch at the FILE step ──────────────
    Same drafted body (provider-agnostic markdown); the create call branches to
    linearCreateIssue; bind via the same applyLinkedWorkItem seam.

F4  SINGLE FRONT DOOR — remove the goal step + the composer card ──────────────
    NewWorkspaceComposerModal today: phase machine {wizard | goal | provision |
    details(=QuickTabBody→NewWorkspaceComposerCard)}. An opinionated open routes
    to the card, bypassing the wizard.
    → modal ALWAYS renders CreateWorkspaceWizard; widen the wizard to honor every
      opinionated field (startGatedRun / initialBaseBranch / initialWorkspaceStatus
      / linkedWorkItem) by seeding the SAME useComposerState + calling the SAME
      submitQuick; delete the goal/provision/details branches + the card usage.
```

**The load-bearing realization:** the wizard already calls `useComposerState({ createGateMode: 'quick', enableIssueAutomation: false, … })` and submits via `submitQuick(quickAgent)` — the *exact* configuration and submit entry the composer card (`QuickTabBody`) uses. `submitQuick` already honors `startGatedRun` (computes `submitGatedRun` via `deriveIssueSideEffectGate`, calls `maybeStartGatedRun`). So F4's "migrate the card's powers" is **plumbing existing `cardProps` through the wizard's JSX + widening its data type** — not re-implementing anything. F2's create-issue flow is likewise already in `cardProps` (`onGenerateIssueBody`, `onCreateIssueSubmit`, …); the wizard uses the same hook, it just doesn't render those seams yet.

---

## 2. Non-negotiable invariants (numbered — regressing any one reintroduces a paid-for bug)

1. **Reuse the shipped seams, never rebuild them.** F1 reuses `resolvePickerProject` / `deriveIssueOptions` / `buildBindPayload` (work-item-picker-model.ts) untouched. F2 reuses `useComposerState`'s create-issue seams + `draftGithubIssueBody` / `createGithubIssue` clients. F4 reuses `submitQuick` + `maybeStartGatedRun` + `firstStartGatedRunBlocker`. Re-ground on `origin/develop` before writing any create/bind/draft code.
2. **One source of truth for F1 honesty.** After F1 there is **no** second detection path that can disagree with the picker. The section's connected/empty status and the issue list both read from **one** `resolvePickerProject` result. `deriveWizardTracker`'s remote heuristic is removed as the display driver — it may not gate any visible "connected/detected/none" text.
3. **Serde-alias-FREE bind, via `buildBindPayload`.** Binding a *created* issue reuses the existing `applyLinkedWorkItem` seam and the `buildBindPayload`/`deriveTrackerBindCoords` shapes (spec 012). **No** new aliased `Worktree`/linked-work-item field — a `#[serde(alias)]` there is a known registry-wipe hazard. F2/F3 add zero new persisted fields; they route the created issue through the same attach seam.
4. **Gated-run preserved (specs 005/008).** F4 must not regress `start_work`. The wizard's "Start gated run" toggle binds to the **same** `cardProps.canStartGatedRun` / `startGatedRun` / `onStartGatedRunChange`, keeps `createGateMode: 'quick'` + `enableIssueAutomation: false`, seeds `initialStartGatedRun` via the existing `initialStartGatedRunProp`, and submits via the **same** `submitQuick(quickAgent)`. No new submit path; the precondition set is whatever `submitQuick` enforces today (inherited verbatim by calling it).
5. **Card removal only after every `openModal('new-workspace-composer')` caller is re-homed.** Enumerate all callers; verify each opinionated field (`startGatedRun`, `linkedWorkItem`, `prefilledName`, `initialRepoId`, `initialBaseBranch`, `initialWorkspaceStatus`, `telemetrySource`) reaches an equivalent create through the wizard **before** deleting `QuickTabBody` / `NewWorkspaceComposerCard`. (§7 table.)
6. **Wiki grounding stays best-effort / non-fatal.** Threading `retrieve_wiki` into `draft_issue_body` must remain async + optional: a wiki miss (or a slow/failed retrieval) still drafts from repo context and must never wedge or fail the draft. Mirror `chat()`'s existing `wiki_context: Option<String>` handling.
7. **Create-issue is fail-loud, non-blocking (silent-failure invariant).** A missing credential / `no_github_repo` / draft failure / Linear-down surfaces an **inline** error (`createIssueError` / a wizard-local error) and **never** blocks creating the workspace. The wizard's "Create workspace" primary stays live even when the create-issue sub-panel is erroring.
8. **Telemetry parity.** Every caller passes `telemetrySource`; the wizard already threads it into `useComposerState` → `workspace_created.source`. Making the wizard the single door must preserve each caller's `telemetrySource` value (no caller collapses to `unknown`).

---

## 3. Per-feature design home (every AC → concrete seam)

Legend: **[reuse]** = call/extend existing code; **[build]** = new code; `~:` = approximate line, re-ground on develop. All UI paths under `crates/agentum-desktop/ui/src/`.

### F1 — honest unified tracker (UI only)

| AC | Home | Reuse / build |
|----|------|---------------|
| **1** step-3 renders **one** section; the Change/Configure control is at the **top** header (not the lower Work-item header) | `components/new-workspace/CreateWorkspaceWizard.tsx` `AgentStep` (`~:891`) — merge the `Tracker` block (`~:967-1010`) and the `WorkItemPicker` block (`~:1032`) into one titled section; hoist the `configureControl` popover (`ProjectBindingEditor`, `~:1120`) into that section's top header. | **[build]** merged JSX; **[reuse]** `ProjectBindingEditor`, `getProjectBinding`, the popover. |
| **2** status text driven by the **same** resolved Project the picker lists from (`resolvePickerProject`) | Replace the `tracker.kind` branch with a status computed from the picker's `resolved` (+ `status`, `options.length`) via **[build]** `deriveUnifiedTrackerStatus` (see §4). `resolvePickerProject` (`work-item-picker-model.ts`) is unchanged. | **[build]** pure helper; **[reuse]** picker resolution. |
| **3** **no** code path shows "No tracker" while the picker lists ≥1 issue | Structural: both the status label and the issue list read from **one** `resolved`. `resolved == null` ⇒ `deriveIssueOptions([])` ⇒ zero issues ⇒ "no tracker" honest state; `resolved != null` ⇒ "connected". They cannot disagree. `deriveWizardTracker` no longer contributes to the decision. | **[build]** delete `deriveWizardTracker`/`WizardTracker` as the display driver (§4). |

### F2 — create issue from intent (GitHub)

| AC | Home | Reuse / build |
|----|------|---------------|
| **4** a "Create issue" affordance: free-text "what do you want to do?" → drafted title + SDD-body, reviewed/edited before filing | In the unified section, a sub-panel bound to `cardProps` seams: **[reuse]** `onCreateIssueTitleChange` / `createIssueTitle`, `onGenerateIssueBody` / `createIssueGenerating` / `createIssueBody`, `onCreateIssueSubmit` / `createIssueSubmitting` / `createIssueError` (all already on `useComposerState`, `~:2887-2907`). Derive the seed **title** from the description via **[reuse]** `deriveGoalIssueDraft(description)` (`lib/workspace-goal-step.ts`). Sub-panel phase gating = **[build]** `create-issue-intent-model.ts` (§5). | Wizard destructures these from `cardProps` (it already has the hook). |
| **5** draft body grounded in **both** `gather_repo_context` **and** `retrieve_wiki` | `crates/agentum-server/src/routes/chat.rs::draft_issue_body` (`~:1844`): **[build]** add `let wiki = retrieve_wiki_for_query(workdir, title).await;` and widen **[build]** `draft_body_instructions(repo_slug, repo_context, wiki)` (`~:1807`) to append a wiki block when present. Add **[build]** `retrieve_wiki_for_query(workdir, query: &str)` and refactor the existing `retrieve_wiki(workdir, messages)` (`~:640`) to delegate to it. | **[reuse]** `gather_repo_context`, `wiki_rag::retrieve_context`; the github.rs route (`~:37`) is unchanged. |
| **6** filing calls `POST /api/github/issues` and binds via `applyLinkedWorkItem` | Already the behavior of `handleCreateIssueSubmit` (`useComposerState ~:1518`) → `createGithubIssue` → `applyLinkedWorkItem` + `setLinkedWorkItem`. Persistence of tracker coords on create is spec 012's `buildBindPayload`/`deriveTrackerBindCoords` path, unchanged. | **[reuse]** entirely. |
| **7** optional + non-fatal: no-cred / no-repo / draft failure inline, never blocks create | `createIssueError` renders inline (spec 006/007 behavior); the wizard primary is independent of the sub-panel. `draftGithubIssueBody` throws the server's verbatim `NO_CREDS_MSG` / `no_github_repo` for inline display. | **[reuse]** `extractServerErrorMessage`, inline error. |

### F3 — create issue (Linear)

| AC | Home | Reuse / build |
|----|------|---------------|
| **8** when the resolved tracker is Linear, "Create issue" files via `linear.createIssue` using the **same** drafted body, and binds it the same way | **[build]** a provider branch at the *file* step only: `resolveCreateIssueProvider(...)` (§6) selects `'github' | 'linear'`; the Linear arm calls **[reuse]** `linearCreateIssue(settings, { teamId, title, description })`, then binds via **[reuse]** `applyLinkedWorkItem` (Linear item → `deriveTrackerBindCoords` → `trackerProvider:'linear'`, `trackerUrl:identifier`). The draft body is the same provider-agnostic markdown from `draftGithubIssueBody`. | **[reuse]** `linearCreateIssue`, `buildLinearIssueLinkedWorkItem`, `deriveTrackerBindCoords`. **[build]** provider resolution + the Linear `teamId` selection (see §6 wrinkle). |

### F4 — single front door

| AC | Home | Reuse / build |
|----|------|---------------|
| **9** goal + provision + details phases removed; opening `new-workspace-composer` always renders `CreateWorkspaceWizard` | `NewWorkspaceComposerModal.tsx`: **[build]** collapse `ComposerModalBody` to render `CreateWorkspaceWizard` unconditionally (drop the `phase` state machine, `handleContinue`/`handleSkip`/`provisionWorkdir`, the goal/provision/details branches). Drop the wizard's `onUseGoal` prop + footer "Start from a goal" button (`CreateWorkspaceWizard.tsx ~:407`). | **[build]** delete; **[reuse]** the wizard. |
| **10** the card's unique powers migrated: (a) "Start gated run" toggle + precondition, (b) `initialBaseBranch`, (c) create-from opinionated opens | **[build]** widen `CreateWorkspaceWizardData` (§7) to `startGatedRun?` / `initialBaseBranch?` / `initialWorkspaceStatus?`; seed `useComposerState` with `...initialStartGatedRunProp(modalData)`, `initialBaseBranch`, `initialWorkspaceStatus`, and `initialLinkedWorkItem: modalData.linkedWorkItem ?? null` (today hardcoded `null`, `~:120`); render the "Start gated run" toggle in step 3 bound to `cardProps.{canStartGatedRun,startGatedRun,onStartGatedRunChange}`. | **[reuse]** `initialStartGatedRunProp`, `submitQuick`, the base-branch combobox (`~:740`), all `cardProps`. |
| **11** every opinionated open reaches an equivalent create; a wizard gated run hits the **same** `start_work` precondition set | Because the wizard already submits via `submitQuick(quickAgent)` (the card's exact submit) and F4 only seeds `startGatedRun` into the same hook, the gated-run path (`submitQuick` guard → `maybeStartGatedRun` → `firstStartGatedRunBlocker`) is inherited byte-identically. §7 enumerates each caller. | **[reuse]** `submitQuick` / `maybeStartGatedRun`. |

---

## 4. F1 unification design (`deriveUnifiedTrackerStatus`)

**What the merged section renders (top → bottom):**

1. **Header row:** the label `Tracker` + the **Change/Configure tracker** control (the `ProjectBindingEditor` popover), hoisted from the current Work-item header to the top. Gate the control on a resolvable `workdir` exactly as today (only a LOCAL git repo can carry a binding).
2. **Status line:** driven by `deriveUnifiedTrackerStatus` — connected (Project identity + N issues available) or the honest "no tracker (optional)" empty state.
3. **Issue list / states:** the existing `WorkItemPicker` loading / failed / empty / options list, unchanged (still `deriveIssueOptions(table)`).
4. **Create-issue sub-panel** (F2, §5), below the picker.
5. **Linked confirmation** ("Linked · #N …"), unchanged.

**The pure helper (new, in `create-workspace-wizard-model.ts`, unit-tested jsdom-free):**

```ts
export type UnifiedTrackerStatus =
  | { kind: 'connected'; issueCount: number }   // resolved Project, issues loaded
  | { kind: 'connecting' }                       // resolved, status==='loading'
  | { kind: 'connected-empty' }                  // resolved, loaded, 0 issues
  | { kind: 'unavailable' }                       // resolved, status==='failed' (still connected, couldn't load)
  | { kind: 'none' }                              // resolved == null → honest "no tracker (optional)"

export function deriveUnifiedTrackerStatus(input: {
  resolved: PickerProjectRef | null            // from resolvePickerProject — the ONLY source
  status: 'idle' | 'loading' | 'failed'
  optionCount: number
}): UnifiedTrackerStatus
```

Rules: `resolved == null` ⇒ `none` (regardless of git remote); else `loading` ⇒ `connecting`; `failed` ⇒ `unavailable`; else `optionCount === 0` ⇒ `connected-empty`, else `connected`. **AC3 is structural:** `none` is the only state that renders "no tracker", and it is reachable only when `resolved == null`, which forces `deriveIssueOptions` to `[]`. There is no input under which "no tracker" coexists with a non-empty list.

**`deriveWizardTracker` disposition — DELETE as the display driver.** It is used only in `CreateWorkspaceWizard.tsx` (grep confirms: no other importer; the `providerLabel` hits elsewhere are unrelated local functions). Keeping it wired would reintroduce the exact divergent path AC3 forbids, and leaving it unused would fail knip. **Decisive default:** delete `deriveWizardTracker`, `WizardTracker`, and their cases in `create-workspace-wizard-model.test.ts`. Keep `parseRemoteSlug` **only if** `deriveUnifiedTrackerStatus`/the render uses it for a provider-brand decoration label (e.g. "GitHub · owner/repo"); if not consumed, delete it and its tests too so knip stays green. (The Project identity from `resolved` + each row's `repository` already give the section everything it needs to display; a remote-slug decoration is optional polish, not a status source.)

---

## 5. F2 create-issue design (draft → review/edit → file → bind)

**The flow is almost entirely existing composer machinery** — the wizard shares the `useComposerState` instance, so `cardProps` already carries the whole create-issue surface. F2 = render it + one server grounding thread + a thin pure phase model.

**UI flow (reuse the hook seams):**
1. Operator types a short intent into a "what do you want to do?" field.
2. On draft: `deriveGoalIssueDraft(intent)` → `{ title, body: intent }`; call `onCreateIssueTitleChange(title)`, then `onGenerateIssueBody()` (→ `draftGithubIssueBody({ workdir, title, slug })`, now wiki-grounded server-side) which fills `createIssueBody`.
3. Operator reviews/edits `createIssueTitle` + `createIssueBody`.
4. On file: `onCreateIssueSubmit()` (→ `createGithubIssue` → `applyLinkedWorkItem` + `setLinkedWorkItem`), which binds the new issue as the linked work item (persisted on create via spec 012's coords).
5. `createIssueError` renders inline; the wizard primary is never gated on it.

**The pure model (new, `components/new-workspace/create-issue-intent-model.ts`, unit-tested):** a thin selector over the hook's existing flags so the sub-panel's gating is jsdom-free and grade-able:

```ts
export type CreateIssueIntentPhase = 'idle' | 'drafting' | 'review' | 'filing' | 'error'

export function deriveCreateIssueIntentPhase(s: {
  generating: boolean; submitting: boolean; error: string | null; hasBody: boolean
}): CreateIssueIntentPhase

/** can we draft? (non-blank intent, not already busy) / can we file? (title present, not busy) */
export function canDraftIssue(intent: string, busy: boolean): boolean
export function canFileIssue(title: string, busy: boolean): boolean
/** intent → seed title (reuses deriveGoalIssueDraft) so the description alone produces a titled draft */
export function deriveIntentTitle(intent: string): string
```

The model owns **no** network/DOM — it maps the hook's flags to a phase and gates the two buttons. The heavy lifting stays in `useComposerState`.

**Server change (the only backend work):** in `crates/agentum-server/src/routes/chat.rs`:
- Add `async fn retrieve_wiki_for_query(workdir: Option<&str>, query: &str) -> Option<String>` = the `spawn_blocking` → `wiki_rag::retrieve_context(workdir, query, DEFAULT_TOP_K)` core.
- Refactor `retrieve_wiki(workdir, messages)` to extract the last user message → query, then delegate to `retrieve_wiki_for_query` (zero behavior change for `chat()`).
- In `draft_issue_body(workdir, repo_slug, title)`: compute `let wiki = retrieve_wiki_for_query(workdir, title).await;` and pass it to a widened `draft_body_instructions(repo_slug, repo_context, wiki)`.
- Widen `draft_body_instructions` to append a `=== WIKI CONTEXT === … === END WIKI ===` block when `Some`, mirroring the existing `REPO CONTEXT` block; `None` = today's string (no wiki mention). **Best-effort (inv. 6):** a `None` wiki is normal; nothing throws.

The `POST /api/github/issues/draft-body` route (`github.rs`) and the `draftGithubIssueBody` client need **no** signature change — the wiki is internal to `draft_issue_body`. The **first-failing test** is the pure `draft_body_instructions` string builder (see §8).

---

## 6. F3 Linear branch (provider selection at the file step)

The draft body is **provider-agnostic markdown** (open question 1 resolved: reuse the GitHub `draft-body` route; only the *create* call branches). F3 adds one branch at the file step:

```ts
export function resolveCreateIssueProvider(input: {
  resolved: PickerProjectRef | null      // GitHub Project resolves ⇒ prefer github
  linearConnected: boolean               // settings.activeRuntimeEnvironmentId / linear status
}): 'github' | 'linear' | 'ambiguous'
```

- Resolved GitHub Project ⇒ `github` (follow the resolved tracker's provider, open question 2 default).
- No GitHub Project but Linear connected ⇒ `linear`.
- Both ⇒ `ambiguous` ⇒ the sub-panel shows a small provider toggle (open question 2's escape hatch).

The Linear file arm calls `linearCreateIssue(settings, { teamId, title, description: body })`, then feeds the result (`{ identifier, url, title }`) to `applyLinkedWorkItem` via `buildLinearIssueLinkedWorkItem`, so `deriveTrackerBindCoords` binds `trackerProvider:'linear'` / `trackerUrl:identifier` — the same spec-012 bind path.

**Wrinkle to flag (non-blocking, §9/§11):** `linearCreateIssue` **requires a `teamId`** (and optionally `projectId`), which the wizard's tracker section — GitHub-Projects-centric today — does not currently resolve. F3 therefore needs a Linear team selection in the create-issue sub-panel (a small `linearListTeams` picker, defaulting to the single team when only one exists) OR to reuse a persisted default team from settings. This is the least-grounded slice; the GitHub path (F2) fully satisfies the spec's core, and F3 layers on top. See §9 open question 2.

---

## 7. F4 single front door — the caller enumeration + widening

**`openModal('new-workspace-composer', …)` callers on `origin/develop` (12 sites) and their opinionated fields:**

| # | Caller (file · symbol, ~line) | Fields passed | Wizard home |
|---|-------------------------------|---------------|-------------|
| 1 | `TaskPage.tsx` · `openComposerForItem` (GitHub) `~:2406` | `linkedWorkItem`, `prefilledName`, `initialRepoId`, `startGatedRun?`, `telemetrySource:'sidebar'` | **needs** create-from seed + gated-run toggle |
| 2 | `TaskPage.tsx` · `openComposerForGitLabItem` `~:2465` | `linkedWorkItem`, `prefilledName`, `initialRepoId`, `telemetrySource:'sidebar'` | **needs** create-from seed |
| 3 | `TaskPage.tsx` · `openComposerForLinearItem` `~:3136` | `linkedWorkItem` (Linear), `prefilledName`, `telemetrySource:'sidebar'` | **needs** create-from seed (Linear) |
| 4 | `WorktreeJumpPalette.tsx` · `handleCreateWorktree` (6 `openModal` sites `~:490,919,986,994,1047,1055`) | `prefilledName?`, `initialRepoId?`, `linkedWorkItem?`, `telemetrySource:'command_palette'` | **needs** create-from seed |
| 5 | `MissionControlPage.tsx` `~:139` | `telemetrySource:'unknown'` | plain — already OK |
| 6 | `sidebar/AddProjectFromFolderDialog.tsx` `~:212` | `initialRepoId`, `prefilledName?`, `telemetrySource:'sidebar'` | already OK |
| 7 | `sidebar/AddRepoDialog.tsx` `~:721` | `initialRepoId`, `prefilledName?`, `telemetrySource:'sidebar'` | already OK |
| 8 | `sidebar/ProjectAddedDialog.tsx` `~:77` | `initialRepoId`, `prefilledName?`, `telemetrySource?` | already OK |
| 9 | `sidebar/SidebarHeader.tsx` `~:135` | `telemetrySource:'sidebar'` | plain — already OK |
| 10 | `sidebar/WorktreeList.tsx` · `handleCreateForRepo` `~:4680` | `initialRepoId`, `telemetrySource:'sidebar'` | already OK |
| 11 | `sidebar/use-workspace-kanban-create-worktree.ts` `~:14` | `initialWorkspaceStatus`, `telemetrySource:'sidebar'` | **needs** `initialWorkspaceStatus` |
| 12 | `hooks/useIpcEvents.ts` `~:797` | `telemetrySource:'shortcut'` | plain — already OK |

**Result: 12 caller sites, ALL re-homeable cleanly — none blocks card removal.** The wizard already honors `prefilledName` / `initialRepoId` / `telemetrySource`. To become the single door it must newly honor: `linkedWorkItem` (create-from — currently hardcoded `initialLinkedWorkItem: null`), `startGatedRun` (only caller #1), `initialWorkspaceStatus` (only caller #11), and `initialBaseBranch` (no caller passes it today, but `ComposerModalData` declares it — keep parity for callers that may add it).

**Widen `CreateWorkspaceWizardData`** (`CreateWorkspaceWizard.tsx ~:80`) to the full `ComposerModalData` shape:
```ts
export type CreateWorkspaceWizardData = {
  prefilledName?: string
  initialRepoId?: string
  linkedWorkItem?: LinkedWorkItemSummary | null
  initialBaseBranch?: string
  initialWorkspaceStatus?: WorkspaceStatus
  startGatedRun?: boolean
  telemetrySource?: WorkspaceCreateTelemetrySource
}
```
and thread them into the wizard's `useComposerState({ … })` call: add `...initialStartGatedRunProp(modalData)`, `...(modalData.initialBaseBranch ? { initialBaseBranch } : {})`, `initialWorkspaceStatus: modalData.initialWorkspaceStatus`, and `initialLinkedWorkItem: modalData.linkedWorkItem ?? null`. Render the "Start gated run" toggle in step 3 (bound to `cardProps.canStartGatedRun/startGatedRun/onStartGatedRunChange`) and seed the base-branch combobox from `initialBaseBranch` (the combobox already reads `baseBranch`).

**What gets deleted from `NewWorkspaceComposerModal.tsx`:** the `phase` state machine and `useState`, `handleContinue`, `handleSkip`, `provisionWorkdir`, `seed`/`seedRepoId`, the `Dialog`/`QuickTabBody` render branch, and the imports of `NewWorkspaceComposerCard` / `NewWorkspaceGoalStep` / `NewWorkspaceProvisionStep` / `useComposerState` / goal-step helpers. `ComposerModalBody` becomes a thin pass-through to `CreateWorkspaceWizard`.

**Component-file & orphan disposition** (spec non-goal: don't force-delete files; delete only if no other referrer):
- `NewWorkspaceProvisionStep.tsx` — only referrer is the modal ⇒ orphaned ⇒ **delete file**.
- `NewWorkspaceGoalStep.tsx` — only *import* referrer is the modal (the wizard + `workspace-goal-step.ts` references are comments) ⇒ orphaned ⇒ **delete file**.
- `NewWorkspaceComposerCard.tsx` — only real importer is the modal's `QuickTabBody` (the `useComposerState.ts` / `useDetectedAgents.ts` mentions are comments; `ComposerCardProps` is defined **inside** `useComposerState.ts`, not imported from the card) ⇒ orphaned ⇒ **delete file**. The `cardProps` contract type is unaffected.
- `lib/workspace-goal-step.ts` — after F4, `initialComposerPhase` / `shouldStartAtGoalStep` / `deriveWorkspaceGoalSeed` lose their callers. Remove the dead exports + their test cases to stay knip-clean, **but keep `deriveGoalIssueDraft`** (F2 reuses it for intent→title). If knip flags residuals, trim them in the F4 slice.
- `lib/composer-modal-props.ts` (`initialStartGatedRunProp`) and `lib/start-gated-run-precondition.ts` — **retained**, now consumed by the wizard.

**Provision-step capability note (non-blocking):** removing the provision phase drops the *inline* "provision repo (labels/board/harness)" step from the create flow. That capability remains reachable in Settings/the project hub and in the gated-run (`start_work`) path, so it is not lost app-wide — only the optional pre-create hop goes. Matches open question 3's default. Flag to Mateo.

---

## 8. Build order — four independently gated slices

Each is a `feature_list.json` entry (matching the spec's harness wiring). The FIRST failing test to write is named so the Developer starts red.

**Shared gate vocabulary:** backend = `cargo test -p agentum-server --lib`; UI build = `bun run build --prefix crates/agentum-desktop/ui` (vite); UI model = `bunx vitest run`. **No `tsc` gate** (`shared/*` is a vite alias, unresolvable by bare tsc — grep-pin instead of typecheck; jsdom-free pure-model tests only).

### Slice F1 — `honest-unified-tracker` (AC 1–3)
- **First failing test:** `create-workspace-wizard-model.test.ts` → `deriveUnifiedTrackerStatus never reports "none" when a Project resolves` (pure): asserts `resolved != null` ⇒ never `{kind:'none'}`, and `resolved == null` ⇒ `{kind:'none'}` — the AC3 contradiction is impossible.
- Then: the per-branch cases (`connecting`/`connected`/`connected-empty`/`unavailable`); update/remove the `deriveWizardTracker` test cases.
- **Gate:** `bunx vitest run` green + `bun run build --prefix crates/agentum-desktop/ui` succeeds.

### Slice F2 — `create-issue-from-intent` (AC 4–7)
- **First failing test:** `chat::tests::draft_body_instructions_includes_wiki_block_when_present` (pure Rust): asserts the wiki block appears with `Some(wiki)` and is absent (still drafts from repo context) with `None`.
- Then: `create-issue-intent-model.test.ts` → `deriveCreateIssueIntentPhase` state transitions (idle → drafting → review → filing → error) + `canDraftIssue`/`canFileIssue`/`deriveIntentTitle` gating; a Rust `retrieve_wiki_for_query` smoke (workdir-less ⇒ `None`, non-fatal).
- **Gate:** `cargo test -p agentum-server --lib` + `bunx vitest run` + `bun run build …` all green.

### Slice F3 — `create-issue-linear` (AC 8)
- **First failing test:** `create-issue-intent-model.test.ts` → `resolveCreateIssueProvider prefers the resolved GitHub Project, falls back to Linear, flags ambiguous` (pure).
- Then: a bind-shape assertion that a filed Linear issue routes through `deriveTrackerBindCoords` → `{trackerProvider:'linear', trackerUrl:identifier}` (reuse the existing `work-item-picker-model.test.ts` Linear case).
- **Gate:** `bunx vitest run` + `bun run build …` green.

### Slice F4 — `single-front-door` (AC 9–11)
- **First failing test:** a modal-routing pure assertion — extract the modal's render decision into a tiny pure helper (or assert on the widened `CreateWorkspaceWizardData` honoring): `CreateWorkspaceWizardData` opinionated fields (`startGatedRun`, `initialBaseBranch`, `initialWorkspaceStatus`, `linkedWorkItem`) each map to a `useComposerState` seed (a pure `deriveWizardComposerSeed(modalData)` test, jsdom-free).
- Then: knip-clean assertion (no orphaned card/goal/provision files or dead goal-step exports); a `initialStartGatedRunProp` regression that the wizard arms the toggle.
- **Gate:** `cargo test -p agentum-server --lib` + `bunx vitest run` + `bun run build …` all green + knip clean.

---

## 9. Open questions — resolved (decisive defaults; carry-forwards flagged)

1. **Linear draft body (F3):** → **Reuse the GitHub `draft-body` route.** The body is provider-agnostic SDD markdown; only the *create* call branches by provider. `draft_body_instructions` must not leak GitHub-specific framing (e.g. "GitHub issue") into the body — the slug hint stays optional and the SDD template is provider-neutral. *Reviewer confirm: no "GitHub"-specific phrasing in `DRAFT_BODY_INSTRUCTIONS`.*
2. **Provider ambiguity (F2/F3):** → **Follow the resolved tracker's provider** (`resolveCreateIssueProvider`): GitHub Project resolves ⇒ GitHub; else Linear connected ⇒ Linear; **both ⇒ a provider toggle** in the create-issue sub-panel. *Carry-forward to Mateo (non-blocking): the Linear arm needs a `teamId` — default to the sole team when one exists, else a small team picker. If Linear team resolution proves fiddly, F3 can ship behind the GitHub-first F2 without blocking the release.*
3. **Provision-step orphan (F4):** → **Remove from this modal; delete the file** (only referrer is the modal). The provision capability survives in Settings/hub and the gated-run path (§7). *Non-blocking; flag to Mateo that the inline pre-create provision hop is gone.*
4. **Telemetry (F4):** → **No `workspace_created.source` values change.** Every caller still passes `telemetrySource`; the wizard already threads it into `useComposerState`. The F4 slice must verify each of the 12 callers' `telemetrySource` reaches the wizard unchanged (inv. 8). *Default ships; verify in the caller sweep.*

---

## 10. Tradeoffs / rejected alternatives

- **F1: one source vs reconciling two sources.** Rejected "make `deriveWizardTracker` agree with the picker": two detection paths can always drift (the exact bug). Collapsing to `resolvePickerProject` as the sole source makes the contradiction structurally impossible (AC3), at the cost of no longer showing a git-remote-derived provider chip for a repo with no bound Project — which is correct (no Project = no work-item tracker).
- **F2: surface existing hook seams vs build a fresh create-issue panel.** Rejected rebuilding: the composer's create-issue flow (draft/generate/file/bind, labels, error handling, spec 006/007) is battle-tested and already on `cardProps`. A parallel implementation would duplicate the fail-loud/non-blocking contract and drift. The pure model is a thin selector, not a re-implementation.
- **F2: thread wiki inside `draft_issue_body` vs a new wiki-aware route.** Chose internal threading: one grounding change, zero wire/client change, and `chat()` already proves the `wiki_context: Option` pattern. A new route would fork the drafter.
- **F3: reuse the GitHub draft body vs a Linear-specific drafter.** Chose reuse (open question 1) — the body is markdown; forking the drafter for Linear buys nothing and risks divergence.
- **F4: wizard reuses `submitQuick` vs a bespoke gated-run entry.** Chose reuse: `submitQuick` already honors `startGatedRun` end-to-end (`maybeStartGatedRun`, precondition, `deriveIssueSideEffectGate`). A bespoke path would re-derive the precondition set and risk regressing `start_work` (inv. 4). The wizard becomes the single door with **zero** new submit logic.
- **F4: delete orphaned component files vs leave them.** Chose delete (with the no-other-referrer guard) to keep knip green; `deriveGoalIssueDraft` is the one goal-step export kept alive (F2 reuses it).

---

## 11. Reviewer focus (verify specifically)

1. **F1 single source (inv. 2, AC3).** Confirm the merged section's status reads **only** from `resolvePickerProject`'s `resolved` (+ status/optionCount) via `deriveUnifiedTrackerStatus`, that `deriveWizardTracker` no longer gates any visible tracker text, and that the named test proves "none" is unreachable while issues list.
2. **F2 wiki grounding best-effort (inv. 6).** Confirm `draft_issue_body` threads `retrieve_wiki_for_query(workdir, title)`, that a `None` wiki still drafts (no throw, no wedge), and that `draft_body_instructions` gains a wiki block only when `Some`. Confirm the client/route signatures are unchanged.
3. **F2/F3 fail-loud non-blocking (inv. 7).** Confirm create-issue errors render inline (`createIssueError` / wizard-local) and the wizard's "Create workspace" primary is never disabled by a create-issue failure.
4. **Serde-alias-free bind (inv. 3).** Confirm the created issue binds through `applyLinkedWorkItem`/`buildBindPayload`/`deriveTrackerBindCoords` with **no** new persisted `Worktree`/linked field and no `#[serde(alias)]`.
5. **F4 gated-run parity (inv. 4, AC11).** Confirm the wizard submits via the **same** `submitQuick(quickAgent)` with `createGateMode:'quick'`/`enableIssueAutomation:false`, seeds `initialStartGatedRun` via `initialStartGatedRunProp`, and that the precondition set is inherited (not re-implemented).
6. **F4 caller re-homing (inv. 5, 8).** Confirm all 12 `openModal('new-workspace-composer')` callers reach an equivalent create through the wizard, each opinionated field is honored, each `telemetrySource` is preserved, and the card/goal/provision files are deleted only after they are orphaned (knip clean).

**Carry-forwards genuinely needing Mateo (non-blocking — F2/GitHub ships regardless):**
- **F3 Linear `teamId`** — the create arm needs a team; default-to-sole-team or a small picker. If fiddly, F3 can trail F2 without blocking the release (§6, open question 2).
- **Provision hop removed** from the create flow (capability remains in Settings/hub + gated run; §7, open question 3).
- **Distinct "In Review"/tracker column** is out of scope here (that was spec 012); no action.
