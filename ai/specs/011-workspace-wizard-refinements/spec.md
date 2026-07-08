# Spec 011 — New Workspace wizard refinements

- **Number:** 011
- **Status:** PM              <!-- Draft | PM | Architect | In progress | Done -->
- **Surface:** `crates/agentum-desktop/ui/src/components/new-workspace`
- **Author:** Mateo (via /sdd-spec)
- **Date:** 2026-07-08

## Problem

The Create Workspace wizard (shipped v0.61.0) still carries clutter and one
misleading control. Mateo, opening the wizard to spin up a workspace, sees: an
"Advanced options" escape hatch he doesn't want on the default front door;
noisy muted "what's next" labels beside the primary button ("Next: repos on
local", "Next: agent & tracker", "Lands you in a fresh session"); a raw
worktree path preview (`→ /home/.../.worktrees/worktree`) that adds nothing;
a base-branch field that's a free-text box (he has to know and type the branch
name); and a Tracker card that always claims "auto-detected from origin" with a
green "detected" chip **regardless of the repo** — a hardcoded, non-functional
label that lies for repos with no remote and isn't actually per-project.

## Goal

Declutter the wizard and make its two "live" controls honest and per-repo:
drop Advanced options + the next-step hints + the worktree-path line, turn the
base branch into a selectable combobox, and make the tracker reflect the
selected repo's actual remote instead of a hardcoded label.

## Users / personas

- **Mateo (primary operator)** — creating a workspace via the default wizard.
  He wants the fewest, truest controls: pick a branch from a list (not type
  it), and see the real tracker for *this* repo (or an honest "none"), not a
  cosmetic "detected" badge.

## Acceptance criteria

1. **No Advanced options affordance.** The wizard footer (step 1) no longer
   renders the "Advanced options" button (`CreateWorkspaceWizard.tsx:320-326`).
   The composer card remains reachable for *opinionated opens* (linked item /
   gated run) via the modal's existing routing — only the manual escape hatch
   from the plain wizard is removed.
2. **No next-step hint labels.** The muted hint beside the primary button
   (`nextHint` span, `CreateWorkspaceWizard.tsx:339`) is removed, and the
   `wizardNextHint` helper (`create-workspace-wizard-model.ts:79-83`) is deleted
   along with its unit assertions. No "Next: repos on …", "Next: agent &
   tracker", or "Lands you in a fresh session" text renders anywhere in the
   wizard.
3. **Base branch is a combobox.** Step 2 replaces the free-text base-branch
   `<input>` (`CreateWorkspaceWizard.tsx:615-628`) with a searchable combobox
   that lists the selected repo's branches. Opening it lists candidate refs;
   typing filters them; picking one calls `onBaseBranchChange(<ref>)`. Leaving
   it empty keeps the repo default (the trigger shows the default-branch
   placeholder, `selectedRepo.worktreeBaseRef` or "default branch"). The
   branch data comes from the existing `searchRuntimeRepoBaseRefs` /
   `getRuntimeRepoBaseRefDefault` runtime client — **no new backend route**.
4. **No worktree-path preview.** Step 2 no longer renders the
   `→ {worktreePath}` line (`CreateWorkspaceWizard.tsx:640-644`); the `slug` /
   `worktreePath` computation used only by it is removed.
5. **Per-repo, honest tracker.** Step 3's Tracker section
   (`CreateWorkspaceWizard.tsx:743-769`) is derived from the **selected repo's
   own remote** (`Repo.remoteUrl`), not a hardcoded string:
   - When `remoteUrl` resolves to a host + slug, it renders **host · owner/repo**
     (e.g. "GitHub · owner/repo", or "git.mycorp.com · team/app" for a
     self-hosted remote) and may show "detected".
   - When the repo isn't connected, has no remote, or the remote can't be
     parsed, it renders the honest "No tracker — link one later" (or
     "not connected") state and shows **no** "detected" chip and **no**
     "auto-detected from origin" text. (Per Mateo: show host/owner/repo when
     available; if it's not connected, say so — don't fabricate.)
   The literal strings "auto-detected from origin" and an unconditional
   "detected" chip no longer appear.
6. **Pure model covered.** New derivation logic (tracker-from-repo, combobox
   trigger label) lives in `create-workspace-wizard-model.ts` with unit tests
   in `create-workspace-wizard-model.test.ts`; the removed `wizardNextHint`
   test is deleted. `bunx vitest run create-workspace-wizard-model` is green.

## Scope & non-goals (YAGNI)

- **In:** The five UI edits above, entirely within the two wizard files
  (`CreateWorkspaceWizard.tsx`, `create-workspace-wizard-model.ts`) plus its
  test; reuse of the existing branch-search client and combobox primitives;
  a pure tracker-derivation helper over `Repo.remoteUrl`.
- **Out:**
  - No changes to `useComposerState` (props-only consumer, per the wizard's
    contract — comment at `CreateWorkspaceWizard.tsx:53-60`).
  - No new backend routes / no new git-branch API (reuse the runtime client).
  - No change to how the composer *card* renders base branch or tracker.
  - Not building tracker **selection/editing** in the wizard — this only makes
    the *display* honest and per-repo. Configuring the source stays in Tasks.
  - Not removing "Start from a goal" (the goal-first entry, spec 008) — see
    Open questions.

## Reuse vs build (ground in code)

### Already exists — do NOT rebuild

- **Branch data source:** `searchRuntimeRepoBaseRefs`,
  `getRuntimeRepoBaseRefDefault`
  (`runtime/runtime-repo-client.ts:31,50`) — already used by
  `settings/BaseRefPicker.tsx` (debounced search + default ref + remote count).
- **Combobox primitives:** `components/ui/command.tsx`, `components/ui/popover.tsx`,
  and the working combobox pattern in `components/ui/repo-multi-combobox.tsx`
  (Command-in-Popover). Use these rather than a bespoke dropdown.
- **Provider/URL parsing:** `shared/hosted-review-github.ts` /
  `shared/hosted-review-gitlab.ts` parse a remote-ish URL into
  `{ host, owner, repo }`; `Repo.remoteUrl` (`shared/types.ts:277`) is the
  per-repo input. Reuse the parsing precedent; do not hand-roll a new regex if
  a shared helper fits.
- **Wizard pure-model seam:** `create-workspace-wizard-model.ts` (+ its test)
  is the established home for gradeable, jsdom-free logic.

### Build new

- A small combobox wrapper for the base branch inside `RepoStep` (wires the
  runtime search client to Command/Popover; default = empty).
- A pure `deriveWizardTracker({ remoteUrl })` helper returning the display
  state (`{ provider, slug, detected }` or a "none" variant) — unit-tested.

## Risks & invariants

- **Don't touch `useComposerState`.** The wizard is a props-only front-end; all
  state (host/repo/name/baseBranch/agent) stays in the engine so YOLO
  translation, SSH gating, and post-create launch remain centralized.
- **Combobox on SSH/remote repos:** branch search hits the runtime client for
  the active environment; on a repo still needing a connection, degrade
  gracefully (empty list / disabled), never block the step or throw.
- **Honest-by-default tracker:** the fix must fail *closed* — an unparseable or
  missing `remoteUrl` shows "no tracker", never a fabricated "detected". This
  mirrors `BaseRefPicker`'s fail-closed remote-count rule.
- **Removing Advanced options** must not orphan the opinionated-open path: the
  modal still routes linked-item / gated-run opens to the composer card
  directly. Verify the card path still works before deleting the `onAdvanced`
  prop wiring.

## Harness wiring (the gate)

- **feature_list.json entries** (one shippable slice; can split if needed):
  1. `wizard-declutter` — remove Advanced options + next-step hints +
     worktree-path line (AC 1, 2, 4) and drop `wizardNextHint` + its test.
  2. `wizard-base-branch-combobox` — base-branch combobox over the runtime
     branch-search client (AC 3).
  3. `wizard-honest-tracker` — per-repo tracker derivation from `remoteUrl`
     (AC 5) + pure helper & tests (AC 6).
- **`verify.sh` asserts:** `bun run build --prefix crates/agentum-desktop/ui`
  succeeds AND `bunx vitest run create-workspace-wizard-model` is green (pure
  model — the UI package ships no jsdom, so gradeable logic is model-tested,
  not component-mounted). No `tsc` gate (shared/* is a vite alias).
- **`qa.sh` asserts (browser):** open New Workspace → step 1 shows no "Advanced
  options" and no hint text next to the primary button → step 2 shows a
  base-branch combobox that opens a branch list and no `→ path` line → step 3's
  tracker reflects the selected repo (real slug for a repo with a remote; an
  honest "no tracker" for one without), with no "auto-detected from origin"
  string.

## Open questions (all RESOLVED with Mateo, 2026-07-08)

- ~~**"open please, etc."**~~ — **RESOLVED: mistype.** No such label exists;
  it's shorthand for the muted next-step hints, all removed in AC 2. Nothing
  extra to do.
- ~~**"Start from a goal" link**~~ — **RESOLVED: keep it.** AC 1 removes only
  Advanced options; the goal-first entry (spec 008) stays as an alternate start.
- ~~**Tracker beyond GitHub/GitLab**~~ — **RESOLVED: show host · owner/repo when
  derivable; if not connected, say so.** Render the parsed host + slug for any
  remote (GitHub/GitLab/self-hosted alike); fall to the honest "no tracker /
  not connected" state only when there's no remote, the repo isn't connected,
  or the URL won't parse. Folded into AC 5.
