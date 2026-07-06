import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useAppStore } from '@/store'
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import NewWorkspaceComposerCard from '@/components/NewWorkspaceComposerCard'
import NewWorkspaceGoalStep from '@/components/NewWorkspaceGoalStep'
import NewWorkspaceProvisionStep from '@/components/NewWorkspaceProvisionStep'
import AgentSettingsDialog from '@/components/agent/AgentSettingsDialog'
import { useComposerState } from '@/hooks/useComposerState'
import {
  pickQuickWorkspaceAgent,
  resolveQuickWorkspaceAgentSelection
} from '@/lib/quick-workspace-agent-selection'
import type { LinkedWorkItemSummary } from '@/lib/new-workspace'
import { shouldAllowComposerEnterSubmitTarget } from '@/lib/new-workspace-enter-guard'
import { isScreenSubmitShortcut } from '@/lib/screen-submit-shortcut'
import { initialStartGatedRunProp } from '@/lib/composer-modal-props'
import {
  deriveGoalIssueDraft,
  deriveWorkspaceGoalSeed,
  initialComposerPhase,
  type ComposerModalPhase,
  type WorkspaceGoalSeed
} from '@/lib/workspace-goal-step'
import type {
  TuiAgent,
  WorkspaceCreateTelemetrySource,
  WorkspaceStatus
} from '../../../shared/types'

type ComposerModalData = {
  prefilledName?: string
  initialRepoId?: string
  linkedWorkItem?: LinkedWorkItemSummary | null
  initialBaseBranch?: string
  initialWorkspaceStatus?: WorkspaceStatus
  /** Spec 005 F1 (AC 3): open with the "Start gated run" toggle armed — set by
   *  the Tasks page issue-row action. */
  startGatedRun?: boolean
  /** Telemetry surface that opened the composer. Set by each
   *  `openModal('new-workspace-composer', ...)` site so
   *  `workspace_created.source` carries the right value. Falls back to
   *  `unknown` when omitted. */
  telemetrySource?: WorkspaceCreateTelemetrySource
}

export default function NewWorkspaceComposerModal(): React.JSX.Element | null {
  const visible = useAppStore((s) => s.activeModal === 'new-workspace-composer')
  const modalData = useAppStore((s) => s.modalData as ComposerModalData | undefined)
  const closeModal = useAppStore((s) => s.closeModal)

  // Why: Dialog open-state transitions must be driven by the store, not a
  // mirror useState, so palette/open-modal calls feel instantaneous and the
  // modal doesn't linger with stale data after close.
  const handleOpenChange = useCallback(
    (open: boolean) => {
      if (!open) {
        closeModal()
      }
    },
    [closeModal]
  )

  if (!visible) {
    return null
  }

  return (
    <ComposerModalBody
      modalData={modalData ?? {}}
      onClose={closeModal}
      onOpenChange={handleOpenChange}
    />
  )
}

function ComposerModalBody({
  modalData,
  onClose,
  onOpenChange
}: {
  modalData: ComposerModalData
  onClose: () => void
  onOpenChange: (open: boolean) => void
}): React.JSX.Element {
  const repos = useAppStore((s) => s.repos)
  // Mirror the composer's own eligibility filter (`useComposerState` uses the
  // same `Boolean(repo.path)`), so the goal step's workdir picker offers exactly
  // the repos the composer would.
  const eligibleRepos = useMemo(() => repos.filter((repo) => Boolean(repo.path)), [repos])

  // Spec 008 F3 (AC 9): the goal step is the DEFAULT first screen for a plain
  // create-workspace open; an opinionated open (Tasks-page gated-run hop, a
  // create-from item, a prefilled name, a pinned base branch) skips straight to
  // the mechanics-first composer (D3 — the composer stays reachable, and F1's
  // Tasks hop is byte-identical). Spec 010 F3 widens the phase with a
  // modal-LOCAL `'provision'` between goal and details — offered only on the
  // goal-first Continue path (opinionated opens and "Skip to details" never
  // see it; `initialComposerPhase` stays untouched).
  const [phase, setPhase] = useState<ComposerModalPhase | 'provision'>(() =>
    initialComposerPhase(modalData)
  )
  const [seed, setSeed] = useState<WorkspaceGoalSeed | null>(null)
  const [seedRepoId, setSeedRepoId] = useState<string | undefined>(undefined)

  const handleContinue = useCallback((goal: string, repoId: string) => {
    setSeed(deriveWorkspaceGoalSeed(goal))
    setSeedRepoId(repoId ? repoId : undefined)
    // Goal-first Continue → the provision step (skippable) → details.
    setPhase('provision')
  }, [])
  const handleSkip = useCallback(() => {
    // "Skip to details" (D3): no goal framing — byte-identical to today.
    setSeed(null)
    setSeedRepoId(undefined)
    setPhase('details')
  }, [])

  // Spec 010 F3: provisioning runs against the chosen project's ROOT path
  // (worktree creation happens later, in the composer submit). A missing path
  // simply skips the provision phase — never a dead end.
  const provisionWorkdir = useMemo(
    () => (seedRepoId ? (eligibleRepos.find((repo) => repo.id === seedRepoId)?.path ?? '') : ''),
    [eligibleRepos, seedRepoId]
  )

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent
        className="flex flex-col sm:max-w-lg"
        onOpenAutoFocus={(event) => {
          // Why: Radix's FocusScope fires this once the dialog has mounted.
          // preventDefault stops it from focusing whatever first-tabbable it
          // picks (close button). On the goal step we focus the goal box (the
          // first thing to capture, AC 9); on the details step, the repo picker
          // so the keyboard flow starts at the top of the create form.
          event.preventDefault()
          const content = event.currentTarget as HTMLElement
          const goalInput = content.querySelector<HTMLElement>('#workspace-goal')
          if (goalInput) {
            goalInput.focus({ preventScroll: true })
            return
          }
          const trigger = content.querySelector<HTMLElement>(
            '[data-repo-combobox-root="true"][role="combobox"]'
          )
          trigger?.focus({ preventScroll: true })
        }}
      >
        {phase === 'goal' ? (
          <NewWorkspaceGoalStep
            repos={eligibleRepos}
            initialRepoId={modalData.initialRepoId}
            primaryLabel="Create Workspace"
            onContinue={handleContinue}
            onSkip={handleSkip}
          />
        ) : phase === 'provision' && provisionWorkdir ? (
          <NewWorkspaceProvisionStep
            workdir={provisionWorkdir}
            onContinue={() => setPhase('details')}
            onSkip={() => setPhase('details')}
          />
        ) : (
          <QuickTabBody
            modalData={modalData}
            onClose={onClose}
            active
            seed={seed}
            seedRepoId={seedRepoId}
          />
        )}
      </DialogContent>
    </Dialog>
  )
}

function QuickTabBody({
  modalData,
  onClose,
  active,
  seed,
  seedRepoId
}: {
  modalData: ComposerModalData
  onClose: () => void
  active: boolean
  /** Spec 008 F3: the goal-step seed (name/prompt from the goal) when the user
   *  came via "Continue"; null/undefined when they opened straight to details
   *  or chose "Skip to details" (D3 — byte-identical to today). */
  seed?: WorkspaceGoalSeed | null
  seedRepoId?: string
}): React.JSX.Element {
  const settings = useAppStore((s) => s.settings)
  const {
    cardProps,
    composerRef,
    onComposerNodeChange,
    nameInputRef,
    submitQuick,
    createDisabled
  } = useComposerState({
    // Spec 008 F3 (AC 9): seed the workspace name from the goal when present;
    // otherwise the prefilled name (Tasks/palette) or blank.
    initialName: seed ? seed.name : (modalData.prefilledName ?? ''),
    // Why: the modal is quick-create only now, so prompt-prefill state is
    // intentionally ignored on the mechanics-first path even if older callers
    // still send it. On the goal-first path the goal seeds the prompt.
    initialPrompt: seed ? seed.prompt : '',
    initialLinkedWorkItem: modalData.linkedWorkItem ?? null,
    // Spec 008 F3 (D9): the goal step's chosen workdir target wins as the
    // composer's initial repo; fall back to any modal-provided repo.
    initialRepoId: seedRepoId ?? modalData.initialRepoId,
    initialWorkspaceStatus: modalData.initialWorkspaceStatus,
    // Spec 008 F1 #1: the Tasks-page pre-armed hop arms the toggle via
    // `modalData.startGatedRun` → `initialStartGatedRun` (pure, unit-pinned).
    ...initialStartGatedRunProp(modalData),
    ...(modalData.initialBaseBranch ? { initialBaseBranch: modalData.initialBaseBranch } : {}),
    persistDraft: false,
    onCreated: onClose,
    ...(modalData.telemetrySource ? { telemetrySource: modalData.telemetrySource } : {}),
    enableIssueAutomation: false,
    createGateMode: 'quick'
  })
  // Spec 008 F3 (AC 11): when arriving from the goal step, pre-fill the
  // composer's EXISTING create-issue form (title + body) from the goal via its
  // public callbacks — reuse, not rebuild. This lets the tracker step (c) →
  // scaffold (b) → gated run reach `start_work`'s precondition set without
  // retyping. One-shot; the form stays closed and fully skippable (AC 10), so
  // "Skip to details" is untouched (seed is null there).
  const { onCreateIssueTitleChange, onCreateIssueBodyChange } = cardProps
  const seededIssueRef = useRef(false)
  useEffect(() => {
    if (!seed || seededIssueRef.current) {
      return
    }
    seededIssueRef.current = true
    const draft = deriveGoalIssueDraft(seed.goal)
    if (draft.title) {
      onCreateIssueTitleChange(draft.title)
    }
    if (draft.body) {
      onCreateIssueBodyChange(draft.body)
    }
  }, [seed, onCreateIssueTitleChange, onCreateIssueBodyChange])
  // Why: the composer's built-in `onOpenAgentSettings` handler navigates to
  // the settings page and closes the modal. For the quick-create flow we want
  // a less disruptive affordance — a nested dialog layered over the composer
  // so the user can tweak agents without losing their in-progress workspace
  // name/repo selection.
  const [agentSettingsOpen, setAgentSettingsOpen] = useState(false)
  // Why: once the user picks an agent, their choice wins and must not be
  // overwritten when the derived "preferred" value changes (e.g. detection
  // finishes and adds more installed agents to the set). Track that with an
  // override rather than an effect that mirrors a prop into state — deriving
  // during render keeps the selection in sync with the detected set without
  // triggering an extra commit.
  const [quickAgentOverride, setQuickAgentOverride] = useState<TuiAgent | null | undefined>(
    undefined
  )
  const preferredQuickAgent = useMemo<TuiAgent | null>(() => {
    const pref = settings?.defaultTuiAgent
    // Why: detection can still be pending when quick-create submits; keep the
    // prior catalog fallback while filtering disabled agents out of that choice.
    return pickQuickWorkspaceAgent(pref, cardProps.detectedAgentIds, settings?.disabledTuiAgents)
  }, [cardProps.detectedAgentIds, settings?.defaultTuiAgent, settings?.disabledTuiAgents])
  const resolvedQuickAgentSelection = resolveQuickWorkspaceAgentSelection({
    quickAgentOverride,
    preferredQuickAgent,
    detectedAgentIds: cardProps.detectedAgentIds,
    disabledTuiAgents: settings?.disabledTuiAgents
  })
  if (resolvedQuickAgentSelection.quickAgentOverride !== quickAgentOverride) {
    // Why: detection/settings changes can invalidate a user-picked agent; repair
    // before the child selector renders an unavailable option for one commit.
    setQuickAgentOverride(resolvedQuickAgentSelection.quickAgentOverride)
  }
  const quickAgent = resolvedQuickAgentSelection.quickAgent

  const handleQuickAgentChange = useCallback((agent: TuiAgent | null) => {
    setQuickAgentOverride(agent)
  }, [])

  const handleCreate = useCallback(async (): Promise<void> => {
    await submitQuick(quickAgent)
  }, [quickAgent, submitQuick])
  const primaryActionLabel = cardProps.selectedRepoIsGit ? 'Create Worktree' : 'Create Workspace'

  // Cmd/Ctrl+Enter submits, Esc first blurs the focused input (like the full page).
  useEffect(() => {
    if (!active) {
      return
    }
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== 'Enter' && event.key !== 'Escape') {
        return
      }
      const target = event.target
      if (!(target instanceof HTMLElement)) {
        return
      }

      if (event.key === 'Escape') {
        if (
          target instanceof HTMLInputElement ||
          target instanceof HTMLTextAreaElement ||
          target instanceof HTMLSelectElement ||
          target.isContentEditable
        ) {
          event.preventDefault()
          target.blur()
          return
        }
        event.preventDefault()
        onClose()
        return
      }

      // Why: workspace creation is screen-local submit behavior, not a
      // user-configurable app command.
      if (!isScreenSubmitShortcut(event)) {
        return
      }
      if (!shouldAllowComposerEnterSubmitTarget(target, composerRef.current)) {
        return
      }
      if (createDisabled) {
        return
      }
      event.preventDefault()
      void handleCreate()
    }
    window.addEventListener('keydown', onKeyDown, { capture: true })
    return () => window.removeEventListener('keydown', onKeyDown, { capture: true })
  }, [active, composerRef, createDisabled, handleCreate, onClose])

  return (
    <>
      <DialogHeader className="gap-1">
        <DialogTitle className="text-base font-semibold">{primaryActionLabel}</DialogTitle>
      </DialogHeader>
      <NewWorkspaceComposerCard
        composerRef={composerRef}
        onComposerNodeChange={onComposerNodeChange}
        nameInputRef={nameInputRef}
        quickAgent={quickAgent}
        onQuickAgentChange={handleQuickAgentChange}
        {...cardProps}
        primaryActionLabel={primaryActionLabel}
        onOpenAgentSettings={() => setAgentSettingsOpen(true)}
        onCreate={() => void handleCreate()}
      />
      <AgentSettingsDialog open={agentSettingsOpen} onOpenChange={setAgentSettingsOpen} />
    </>
  )
}
