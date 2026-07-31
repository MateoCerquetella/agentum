import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  ArrowLeft,
  ArrowRight,
  Check,
  ChevronDown,
  ChevronsUpDown,
  FolderOpen,
  FolderPlus,
  GitBranch,
  Laptop,
  Loader2,
  PlugZap,
  Search,
  Server,
  X
} from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle
} from '@/components/ui/dialog'
import {
  Command,
  CommandEmpty,
  CommandInput,
  CommandItem,
  CommandList
} from '@/components/ui/command'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { api } from '@/tauri'
import { toast } from 'sonner'
import { searchRuntimeRepoBaseRefs } from '@/runtime/runtime-repo-client'
import { cn } from '@/lib/utils'
import { useAppStore } from '@/store'
import { useMountedRef } from '@/hooks/useMountedRef'
import { useDetectedAgents } from '@/hooks/useDetectedAgents'
import { RemoteFileBrowser } from '@/components/sidebar/RemoteFileBrowser'
import { useComposerState } from '@/hooks/useComposerState'
import {
  pickQuickWorkspaceAgent,
  resolveQuickWorkspaceAgentSelection
} from '@/lib/quick-workspace-agent-selection'
import { AGENT_CATALOG, AgentIcon } from '@/lib/agent-catalog'
import { isFolderRepo, isGitRepoKind } from '@/shared/repo-kind'
import { filterReposForHost } from '@/hooks/composer-host-scoping'
import { LOCAL_HOST_KEY } from '@/components/sidebar/worktree-list-groups'
import {
  REPO_LIST_COLLAPSED_CAP,
  WIZARD_STEP_LABELS,
  buildWizardRecap,
  canLeaveRepoStep as canLeaveRepoStepModel,
  capRepoList,
  deriveWizardComposerSeed,
  deriveWizardSddTitle,
  filterRepoList,
  resolveWizardAgentOptions,
  selectAddedRepoBeforeHydration,
  wizardBaseBranchTriggerLabel,
  wizardPrimaryLabel,
  type CreateWorkspaceWizardData,
  type WizardStep
} from '@/components/new-workspace/create-workspace-wizard-model'
import type { Repo, TuiAgent } from '@/shared/types'
import {
  NEW_WORK_STAGES,
  canLaunchNewWork,
  initialNewWorkProgress,
  isNewWorkRetryAvailable,
  newWorkBusyLabel,
  newWorkPrimaryLabel,
  updateNewWorkProgress,
  type NewWorkCheckpoint,
  type NewWorkProgress,
  type WorkSource
} from './new-work-launch-model'
import {
  createWorkspaceSpec,
  selectRunInRunCenter
} from '@/runtime/sdd-client'
import {
  clampDialogOffset,
  type DialogBaseRect,
  type DialogOffset
} from './movable-dialog'

/**
 * The "Create Workspace" wizard — a three-step front-end (Host → Repo &
 * branch → Name & agent) over the shared `useComposerState` creation engine.
 * Tracker-originated work belongs to New Spec; this wizard remains the
 * explicit, tracker-neutral path for creating a manual workspace.
 * Spec 013 F4: it is the SINGLE front door for `new-workspace-composer`
 * — it never becomes a state machine inside the engine, it drives the same
 * host/repo/name/baseBranch/agent state the composer card drove and calls the
 * same `submitQuick`, so agent translation, SSH gating, setup hooks, and the
 * post-create launch stay centralized (no new paths).
 */
export default function CreateWorkspaceWizard({
  modalData,
  onClose,
  onOpenChange
}: {
  modalData: CreateWorkspaceWizardData
  onClose: () => void
  onOpenChange: (open: boolean) => void
}): React.JSX.Element {
  const settings = useAppStore((s) => s.settings)
  const repos = useAppStore((s) => s.repos)
  const hostMetaByKey = useAppStore((s) => s.hostMetaByKey)
  const sshConnectionStates = useAppStore((s) => s.sshConnectionStates)
  const addRepoFromStore = useAppStore((s) => s.addRepo)
  const fetchWorktrees = useAppStore((s) => s.fetchWorktrees)
  // Seed the normal workspace fields from modal-open data. Specification
  // authoring is deliberately deferred to Run Center.
  const { cardProps, submitQuick, nameInputRef } = useComposerState({
    ...deriveWizardComposerSeed(modalData),
    // Fail closed if a stale caller still supplies a legacy tracker payload.
    initialLinkedWorkItem: null,
    requiredProjectTaskScope: modalData.requiredProjectTaskScope,
    initialPrompt: '',
    persistDraft: false,
    onCreated: onClose,
    enableIssueAutomation: false,
    createGateMode: 'quick'
  })

  const {
    eligibleHosts,
    selectedHostKey,
    onHostChange,
    hostScopedRepos,
    disabledRepoIds,
    repoId,
    onRepoChange,
    selectedRepoIsGit,
    name,
    onNameValueChange,
    baseBranch,
    onBaseBranchChange,
    detectedAgentIds,
    creating,
    selectedRepoRequiresConnection,
    selectedRepoConnectInProgress,
    onConnectSelectedRepo,
    requiresExplicitSetupChoice,
    setupDecision,
    onSetupDecisionChange,
  } = cardProps
  const scopeLockedDisabledRepoIds = useMemo(() => {
    if (!modalData.requiredProjectTaskScope) return disabledRepoIds
    const next = new Map(disabledRepoIds)
    for (const candidate of repos) if (candidate.id !== modalData.requiredProjectTaskScope.repoId) next.set(candidate.id, 'Repository locked to the active Project Tasks scope.')
    return next
  }, [disabledRepoIds, modalData.requiredProjectTaskScope, repos])

  // Quick-agent selection mirrors the composer modal's `QuickTabBody`: the
  // user's pick wins, else the preferred/detected default. Derived during
  // render (no mirror effect) so detection landing doesn't clobber a choice.
  const [quickAgentOverride, setQuickAgentOverride] = useState<TuiAgent | null | undefined>(
    undefined
  )
  const preferredQuickAgent = useMemo(
    () =>
      pickQuickWorkspaceAgent(
        settings?.defaultTuiAgent,
        detectedAgentIds,
        settings?.disabledTuiAgents
      ),
    [detectedAgentIds, settings?.defaultTuiAgent, settings?.disabledTuiAgents]
  )
  const resolvedAgent = resolveQuickWorkspaceAgentSelection({
    quickAgentOverride,
    preferredQuickAgent,
    detectedAgentIds,
    disabledTuiAgents: settings?.disabledTuiAgents
  })
  if (resolvedAgent.quickAgentOverride !== quickAgentOverride) {
    setQuickAgentOverride(resolvedAgent.quickAgentOverride)
  }
  const quickAgent = resolvedAgent.quickAgent

  const [step, setStep] = useState<WizardStep>(1)
  const dialogContentRef = useRef<HTMLDivElement>(null)
  const [dialogOffset, setDialogOffset] = useState<DialogOffset>({ x: 0, y: 0 })
  const [dialogDragging, setDialogDragging] = useState(false)
  const dialogDragSessionRef = useRef<{
    pointerId: number
    startClientX: number
    startClientY: number
    startOffset: DialogOffset
    baseRect: DialogBaseRect
  } | null>(null)
  const workSource: WorkSource = 'none'
  const [useSdd, setUseSdd] = useState(false)
  const [sddDescription, setSddDescription] = useState('')
  const [launchCheckpoint, setLaunchCheckpoint] = useState<NewWorkCheckpoint>({})
  const [launchProgress, setLaunchProgress] = useState(() =>
    initialNewWorkProgress({}, useSdd ? 'sdd' : workSource)
  )
  const [launchInFlight, setLaunchInFlight] = useState(false)
  const launchInFlightRef = useRef(false)
  const [addingRepo, setAddingRepo] = useState(false)
  // Spec: SSH/remote "Add project" is inline in the wizard (not a separate
  // dialog) — this toggles the inline remote-add panel in step 2. Reset on host
  // switch so it never lingers over a host that can't use it.
  const [remoteAddOpen, setRemoteAddOpen] = useState(false)
  // Badge the host we opened on ("last used") — captured once so re-selecting
  // doesn't move the badge around.
  const lastUsedHostKeyRef = useRef(selectedHostKey)

  const constrainDialogToViewport = useCallback((): void => {
    const content = dialogContentRef.current
    if (!content) return
    const rect = content.getBoundingClientRect()
    setDialogOffset((current) => {
      const next = clampDialogOffset({
        desiredOffset: current,
        baseRect: {
          left: rect.left - current.x,
          top: rect.top - current.y,
          right: rect.right - current.x,
          bottom: rect.bottom - current.y
        },
        viewportWidth: window.innerWidth,
        viewportHeight: window.innerHeight
      })
      return next.x === current.x && next.y === current.y ? current : next
    })
  }, [])

  useEffect(() => {
    const frame = window.requestAnimationFrame(constrainDialogToViewport)
    window.addEventListener('resize', constrainDialogToViewport)
    return () => {
      window.cancelAnimationFrame(frame)
      window.removeEventListener('resize', constrainDialogToViewport)
    }
  }, [constrainDialogToViewport, step])

  const handleDialogDragStart = useCallback(
    (event: React.PointerEvent<HTMLDivElement>): void => {
      if (event.button !== 0) return
      const target = event.target
      if (
        target instanceof Element &&
        target.closest('button, a, input, textarea, select, [data-dialog-drag-exclude]')
      ) {
        return
      }
      const content = dialogContentRef.current
      if (!content) return
      event.preventDefault()
      const rect = content.getBoundingClientRect()
      dialogDragSessionRef.current = {
        pointerId: event.pointerId,
        startClientX: event.clientX,
        startClientY: event.clientY,
        startOffset: dialogOffset,
        baseRect: {
          left: rect.left - dialogOffset.x,
          top: rect.top - dialogOffset.y,
          right: rect.right - dialogOffset.x,
          bottom: rect.bottom - dialogOffset.y
        }
      }
      setDialogDragging(true)
      try {
        event.currentTarget.setPointerCapture(event.pointerId)
      } catch {
        // Pointer capture can fail if Chromium detaches the handle mid-event.
      }
    },
    [dialogOffset]
  )

  const handleDialogDragMove = useCallback(
    (event: React.PointerEvent<HTMLDivElement>): void => {
      const session = dialogDragSessionRef.current
      if (!session || session.pointerId !== event.pointerId) return
      setDialogOffset(
        clampDialogOffset({
          desiredOffset: {
            x: session.startOffset.x + event.clientX - session.startClientX,
            y: session.startOffset.y + event.clientY - session.startClientY
          },
          baseRect: session.baseRect,
          viewportWidth: window.innerWidth,
          viewportHeight: window.innerHeight
        })
      )
    },
    []
  )

  const finishDialogDrag = useCallback(
    (event: React.PointerEvent<HTMLDivElement>): void => {
      const session = dialogDragSessionRef.current
      if (!session || session.pointerId !== event.pointerId) return
      dialogDragSessionRef.current = null
      setDialogDragging(false)
      try {
        if (event.currentTarget.hasPointerCapture(event.pointerId)) {
          event.currentTarget.releasePointerCapture(event.pointerId)
        }
      } catch {
        // Chromium may already have released capture on pointer cancellation.
      }
    },
    []
  )

  const selectedRepo = repos.find((repo) => repo.id === repoId)
  const selectedHostLabel =
    eligibleHosts.find((host) => host.key === selectedHostKey)?.label ??
    (selectedHostKey === LOCAL_HOST_KEY ? 'This machine' : 'SSH host')

  const eligibleRepos = useMemo(() => repos.filter((repo) => Boolean(repo.path)), [repos])
  const repoCountForHost = useCallback(
    (hostKey: string) => filterReposForHost(eligibleRepos, hostKey).length,
    [eligibleRepos]
  )

  const disabledAgents = settings?.disabledTuiAgents
  const agentOptions = useMemo<TuiAgent[]>(
    () => resolveWizardAgentOptions({ detectedAgentIds, disabledTuiAgents: disabledAgents }),
    [detectedAgentIds, disabledAgents]
  )

  const canLeaveRepoStep = canLeaveRepoStepModel({
    repoId,
    requiresConnection: selectedRepoRequiresConnection,
    // Keep the old selection from acting as a valid Continue target while a
    // replacement project is being picked/registered.
    selectionPending: addingRepo || remoteAddOpen
  })

  const goNext = useCallback(() => {
    setStep((prev) => (prev < 3 ? ((prev + 1) as WizardStep) : prev))
  }, [])
  const goBack = useCallback(() => {
    setStep((prev) => (prev > 1 ? ((prev - 1) as WizardStep) : prev))
  }, [])

  // The SSH connection behind the selected host (`ssh:<id>` → id), or '' for
  // the local machine. Drives whether "Add project" adds locally or remotely.
  const selectedConnectionId = selectedHostKey.startsWith('ssh:')
    ? selectedHostKey.slice('ssh:'.length)
    : ''

  // Step 2 "Add project": register a repo without leaving the wizard, so the
  // user never has to bail to the sidebar and lose their place. Both host kinds
  // stay in the wizard and end by selecting the new repo — one consistent flow.
  // Local uses the native OS folder picker (`store.addRepo`). An SSH host's
  // filesystem isn't reachable by that picker, so it opens the inline
  // remote-add panel (`AddRemoteProjectPanel`) — the host is already chosen in
  // step 1, so no separate dialog/target-picker is needed.
  const handleAddRepo = useCallback(async () => {
    if (addingRepo) {
      return
    }
    if (selectedConnectionId) {
      setRemoteAddOpen(true)
      return
    }
    setAddingRepo(true)
    try {
      const repo = await addRepoFromStore()
      if (!repo) {
        return
      }
      // Select first. Previously Agentum (or whichever project opened the
      // wizard) stayed selected until this worktree scan completed, while the
      // Continue button remained actionable. Advancing in that window loaded
      // the old project's tracker and ultimately created the worktree there.
      await selectAddedRepoBeforeHydration({
        repoId: repo.id,
        selectRepo: onRepoChange,
        ...(isGitRepoKind(repo) ? { hydrateRepo: fetchWorktrees } : {})
      })
    } finally {
      setAddingRepo(false)
    }
  }, [addingRepo, addRepoFromStore, fetchWorktrees, onRepoChange, selectedConnectionId])

  // Collapse the inline remote-add panel whenever the host changes — it's
  // connection-specific and must not carry over to a different (or local) host.
  useEffect(() => {
    setRemoteAddOpen(false)
  }, [selectedHostKey])

  // A remote repo landed: select it and collapse the panel, mirroring the
  // local add's "auto-select and stay in the wizard" outcome.
  const handleRemoteRepoAdded = useCallback(
    (repoId: string) => {
      onRepoChange(repoId)
      setRemoteAddOpen(false)
    },
    [onRepoChange]
  )

  const launchAllowed = canLaunchNewWork({
    source: useSdd ? 'sdd' : workSource,
    hasSelectedAgent: Boolean(quickAgent),
    canStageNewIssue: false,
    hasNewIssueTitle: false,
    hasSelectedIssue: false,
    hasIssueCheckpoint: false,
    hasSddDescription: Boolean(sddDescription.trim())
  })

  const handlePrimary = useCallback(async () => {
    if (step === 3) {
      if (
        launchInFlightRef.current ||
        creating ||
        !launchAllowed
      ) {
        return
      }
      launchInFlightRef.current = true
      setLaunchInFlight(true)
      try {
        setLaunchProgress((current) => updateNewWorkProgress(current, 'issue', 'done'))

        if (useSdd && sddDescription.trim() && repoId) {
          // Start SDD run
          setLaunchProgress((current) => updateNewWorkProgress(current, 'sdd', 'active'))
          const sddResult = await createWorkspaceSpec(repoId, {
            requestId: `wizard-${Date.now()}`,
            title: deriveWizardSddTitle(sddDescription, name),
            description: sddDescription.trim(),
            provider: quickAgent || 'codex',
            baseRef: baseBranch || 'HEAD'
          })
          setLaunchCheckpoint((current) => ({ ...current, sddResult }))
          setLaunchProgress((current) => updateNewWorkProgress(current, 'sdd', 'done'))

          // Create workspace and link to SDD run
          await submitQuick(quickAgent, {
            linkedWorkItem: null,
            checkpoint: launchCheckpoint,
            onCheckpoint: setLaunchCheckpoint,
            onProgress: (stage, status) =>
              setLaunchProgress((current) => updateNewWorkProgress(current, stage, status))
          })

          // Notify Run Center to open the SDD run
          if (sddResult.specId && sddResult.runId) {
            selectRunInRunCenter({
              repoId,
              specId: sddResult.specId,
              runId: sddResult.runId,
              workspaceId: '' // Will be filled by workspace activation
            })
          }
        } else {
          // Normal workspace creation without SDD
          await submitQuick(quickAgent, {
            linkedWorkItem: null,
            checkpoint: launchCheckpoint,
            onCheckpoint: setLaunchCheckpoint,
            onProgress: (stage, status) =>
              setLaunchProgress((current) => updateNewWorkProgress(current, stage, status))
          })
        }
      } finally {
        launchInFlightRef.current = false
        setLaunchInFlight(false)
      }
      return
    }
    if (step === 2 && !canLeaveRepoStep) {
      return
    }
    goNext()
  }, [canLeaveRepoStep, creating, goNext, launchAllowed, launchCheckpoint, quickAgent, step, submitQuick, useSdd, sddDescription, repoId, name, baseBranch])

  // Enter advances / creates (there are no multi-line inputs here, so a bare
  // Enter is unambiguous); Radix handles Esc → close on its own.
  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (event.key !== 'Enter' || event.shiftKey) {
        return
      }
      // Only a plain text input (or an empty focus) should let Enter drive the
      // wizard's primary action. Elements that own their own Enter semantics —
      // textareas (newline), buttons (their onClick, e.g. "Draft with AI" or an
      // agent tile), and selects — must NOT also advance/create the workspace,
      // or Enter double-fires (the "creates it double" report). The create-issue
      // panel additionally stops Enter propagation so its title field cannot
      // reach the final launch action while the draft is being edited.
      const target = event.target as HTMLElement | null
      if (
        target instanceof HTMLTextAreaElement ||
        target instanceof HTMLButtonElement ||
        target instanceof HTMLSelectElement
      ) {
        return
      }
      event.preventDefault()
      void handlePrimary()
    },
    [handlePrimary]
  )

  const recap = useMemo(
    () =>
      buildWizardRecap({
        step,
        hostLabel: selectedHostLabel,
        repoDisplayName: selectedRepo?.displayName,
        worktreeName: name,
        agent: quickAgent
      }),
    [name, quickAgent, selectedHostLabel, selectedRepo, step]
  )

  // Keep progress model in sync with the SDD toggle
  useEffect(() => {
    setLaunchProgress((current) => ({
      ...current,
      sdd: useSdd ? (launchCheckpoint.sddResult ? 'done' : 'pending') : 'done'
    }))
  }, [useSdd, launchCheckpoint.sddResult])

  const launchBusy = launchInFlight || creating
  const launchScopeLocked = launchBusy || Boolean(launchCheckpoint.worktreeResult)
  const retryAvailable = isNewWorkRetryAvailable(launchProgress, launchBusy)
  const primaryLabel = step === 3
    ? newWorkBusyLabel(launchProgress) ?? (launchBusy ? 'Preparing work…' : newWorkPrimaryLabel(workSource, retryAvailable))
    : wizardPrimaryLabel(step, creating)
  const primaryDisabled =
    step === 3
      ? launchBusy || !launchAllowed
      : step === 2 ? !canLeaveRepoStep : false
  const handleDialogOpenChange = useCallback(
    (open: boolean): void => {
      // Keep the staged launch owner mounted until its current checkpoint
      // settles. Closing mid-request would hide an operation that can still
      // create an issue or worktree in the selected project.
      if (!open && launchBusy) return
      onOpenChange(open)
    },
    [launchBusy, onOpenChange]
  )

  // Step 3 reads only the selected repo's canonical tracker record. Both the
  // issue source and the final launch path therefore share one authority for
  // local and SSH repositories.
  return (
    <Dialog open onOpenChange={handleDialogOpenChange}>
      <DialogContent
        ref={dialogContentRef}
        showCloseButton={false}
        onKeyDown={handleKeyDown}
        style={{
          left: `calc(50% + ${dialogOffset.x}px)`,
          top: `calc(50% + ${dialogOffset.y}px)`
        }}
        className={cn(
          'flex max-h-[min(680px,calc(100dvh-4rem))] w-full flex-col gap-0 overflow-hidden p-0',
          step === 3 ? 'sm:max-w-[720px]' : 'sm:max-w-[640px]'
        )}
      >
        <DialogTitle className="sr-only">New workspace</DialogTitle>
        <DialogDescription className="sr-only">
          Create a manual workspace in three steps: choose a host, a repo and base branch,
          then its name and agent.
        </DialogDescription>

        {/* Native-style title bar: the whole non-interactive top surface moves the dialog. */}
        <div
          data-dialog-drag-handle
          onPointerDown={handleDialogDragStart}
          onPointerMove={handleDialogDragMove}
          onPointerUp={finishDialogDrag}
          onPointerCancel={finishDialogDrag}
          onLostPointerCapture={finishDialogDrag}
          className={cn(
            'flex h-10 flex-none touch-none select-none items-center gap-2.5 border-b border-border bg-muted/30 px-[14px]',
            dialogDragging ? 'cursor-grabbing' : 'cursor-grab'
          )}
        >
          <span className="text-[14px] font-semibold tracking-[-0.01em] text-foreground">
            New workspace
          </span>
          <span className="font-mono text-[11px] text-muted-foreground">step {step} / 3</span>
          <span className="flex-1" />
          {recap ? (
            <span className="max-w-[320px] truncate font-mono text-[11px] text-muted-foreground">
              {recap}
            </span>
          ) : null}
          <button
            type="button"
            disabled={launchBusy}
            onClick={onClose}
            aria-label="Close"
            className="inline-flex size-7 flex-none cursor-pointer items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
          >
            <X className="size-3.5" />
          </button>
        </div>

        {/* Wizard navigation */}
        <div className="flex flex-none flex-col gap-3 border-b border-border px-[18px] py-3">
          <StepDots step={step} locked={launchScopeLocked} onJump={(target) => setStep(target)} />
        </div>

        {/* Body */}
        <div className="min-h-0 flex-1 overflow-y-auto">
          <div className="px-[18px] py-4">
            {step === 1 ? (
              <HostStep
                hosts={eligibleHosts}
                selectedHostKey={selectedHostKey}
                lastUsedHostKey={lastUsedHostKeyRef.current}
                hostMetaByKey={hostMetaByKey}
                sshConnectionStates={sshConnectionStates}
                repoCountForHost={repoCountForHost}
                onPick={onHostChange}
              />
            ) : null}

            {step === 2 ? (
              <RepoStep
                hostLabel={selectedHostLabel}
                repos={hostScopedRepos}
                disabledRepoIds={scopeLockedDisabledRepoIds}
                repoId={repoId}
                onRepoChange={onRepoChange}
                selectedRepo={selectedRepo}
                selectedRepoIsGit={selectedRepoIsGit}
                baseBranch={baseBranch}
                onBaseBranchChange={onBaseBranchChange}
                requiresConnection={selectedRepoRequiresConnection}
                connectInProgress={selectedRepoConnectInProgress}
                onConnect={onConnectSelectedRepo}
                onAddRepo={handleAddRepo}
                addingRepo={addingRepo}
                connectionId={selectedConnectionId}
                remoteAddOpen={remoteAddOpen}
                onCloseRemoteAdd={() => setRemoteAddOpen(false)}
                onRemoteRepoAdded={handleRemoteRepoAdded}
              />
            ) : null}

            {step === 3 ? (
              <fieldset
                disabled={launchScopeLocked}
                className="m-0 min-w-0 border-0 p-0 disabled:cursor-wait disabled:opacity-80"
              >
                <AgentStep
                  agents={agentOptions}
                  detectedAgentIds={detectedAgentIds}
                  quickAgent={quickAgent}
                  onPick={setQuickAgentOverride}
                  selectedRepoIsGit={selectedRepoIsGit}
                  repoDisplayName={selectedRepo?.displayName}
                  name={name}
                  onNameValueChange={onNameValueChange}
                  nameInputRef={nameInputRef}
                  requiresExplicitSetupChoice={requiresExplicitSetupChoice}
                  setupDecision={setupDecision}
                  onSetupDecisionChange={onSetupDecisionChange}
                  worktreeLocked={launchScopeLocked}
                  useSdd={useSdd}
                  onUseSddChange={setUseSdd}
                  sddDescription={sddDescription}
                  onSddDescriptionChange={setSddDescription}
                />
              </fieldset>
            ) : null}
          </div>
        </div>

        {step === 3 ? (
          <NewWorkProgressPanel
            progress={launchProgress}
            workSource={workSource}
            selectedRepoIsGit={selectedRepoIsGit}
            busy={launchBusy}
            onCancel={onClose}
          />
        ) : null}

        {/* Footer */}
        <div
          className="flex flex-none flex-col-reverse gap-2.5 border-t border-border bg-muted/40 px-3 py-3 sm:flex-row sm:items-center sm:px-[18px]"
          aria-busy={step === 3 && launchBusy}
        >
          {step > 1 && !launchCheckpoint.worktreeResult ? (
            <button
              type="button"
              disabled={launchBusy}
              onClick={goBack}
              className="inline-flex w-full items-center justify-center gap-1.5 rounded-md border border-border px-3 py-1.5 text-[12.5px] text-muted-foreground transition-colors hover:border-muted-foreground/40 hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50 sm:w-auto"
            >
              <ArrowLeft className="size-3.5" />
              Back
            </button>
          ) : null}
          <span className="flex-1" />
          <button
            type="button"
            onClick={() => void handlePrimary()}
            disabled={primaryDisabled}
            className="inline-flex w-full items-center justify-center gap-2 rounded-full bg-primary px-[18px] py-2 text-[13px] font-medium text-primary-foreground transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-50 sm:w-auto"
          >
            {launchBusy && step === 3 ? <Loader2 className="size-3.5 animate-spin" /> : null}
            {primaryLabel}
            {!launchBusy || step !== 3 ? <ArrowRight className="size-3.5" aria-hidden /> : null}
          </button>
        </div>
      </DialogContent>
    </Dialog>
  )
}

/** Segmented step indicator — completed steps are clickable to jump back. */
function StepDots({
  step,
  locked,
  onJump
}: {
  step: WizardStep
  locked: boolean
  onJump: (target: WizardStep) => void
}): React.JSX.Element {
  return (
    <div className="flex items-center gap-3.5">
      {WIZARD_STEP_LABELS.map((label, index) => {
        const n = (index + 1) as WizardStep
        const done = step > n
        const active = step === n
        return (
          <button
            key={label}
            type="button"
            disabled={!done || locked}
            onClick={() => done && onJump(n)}
            className={cn(
              'inline-flex items-center gap-2',
              done && !locked ? 'cursor-pointer' : 'cursor-default',
              locked && 'opacity-60'
            )}
          >
            <span
              className={cn(
                'inline-flex size-[19px] items-center justify-center rounded-full border font-mono text-[10.5px] font-semibold',
                done
                  ? 'border-transparent bg-emerald-500/15 text-emerald-500'
                  : active
                    ? 'border-transparent bg-primary text-primary-foreground'
                    : 'border-border text-muted-foreground'
              )}
            >
              {done ? <Check className="size-3" strokeWidth={3} /> : n}
            </span>
            <span
              className={cn(
                'whitespace-nowrap text-[12px]',
                active
                  ? 'font-semibold text-foreground'
                  : done
                    ? 'text-muted-foreground'
                    : 'text-muted-foreground/70'
              )}
            >
              {label}
            </span>
          </button>
        )
      })}
    </div>
  )
}

function NewWorkProgressPanel({
  progress,
  workSource,
  selectedRepoIsGit,
  busy,
  onCancel
}: {
  progress: NewWorkProgress
  workSource: WorkSource
  selectedRepoIsGit: boolean
  busy: boolean
  onCancel: () => void
}): React.JSX.Element {
  const stageDetails = {
    issue:
      workSource === 'new'
        ? 'Create the tracker issue'
        : workSource === 'existing'
          ? 'Link the selected issue'
          : workSource === 'sdd'
            ? 'No tracker issue required'
            : 'No tracker issue requested',
    sdd: workSource === 'sdd' ? 'Create the spec and start its guarded run' : 'No SDD run requested',
    worktree: selectedRepoIsGit ? 'Create the Git worktree and open the agent' : 'Open the project workspace and agent'
  } satisfies Record<(typeof NEW_WORK_STAGES)[number], string>

  return (
    <section
      aria-label="Workspace creation progress"
      aria-live="polite"
      className="flex flex-none flex-col border-t border-border bg-muted/20 px-[18px] py-3"
    >
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <span className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-muted-foreground">
            Creation flow
          </span>
          <span className="ml-2 hidden text-[11px] text-muted-foreground sm:inline">
            These stages stay visible while your workspace is prepared.
          </span>
        </div>
        <button
          type="button"
          disabled={busy}
          onClick={onCancel}
          title={busy ? 'The current stage must finish before this window can close' : undefined}
          className="inline-flex flex-none items-center justify-center gap-1.5 rounded-md border border-border bg-background px-2.5 py-1 text-[11.5px] text-muted-foreground transition-colors hover:border-muted-foreground/40 hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
        >
          <X className="size-3" />
          Cancel
        </button>
      </div>

      <ol className="mt-2.5 grid grid-cols-2 gap-2 sm:grid-cols-4">
        {NEW_WORK_STAGES.map((stage, index) => {
          const status = progress[stage]
          const done = status === 'done'
          const active = status === 'active'
          const error = status === 'error'
          const statusLabel = done
            ? 'Complete'
            : active
              ? 'In progress'
              : error
                ? 'Needs attention'
                : 'Waiting'
          return (
            <li
              key={stage}
              aria-current={active ? 'step' : undefined}
              className={cn(
                'relative flex min-w-0 gap-2 rounded-md border border-border/70 bg-background/55 p-2',
                active && 'border-primary/45 bg-primary/5',
                error && 'border-destructive/45 bg-destructive/5'
              )}
            >
              <span
                className={cn(
                  'inline-flex size-5 flex-none items-center justify-center rounded-full border bg-background font-mono text-[9.5px] font-semibold',
                  done && 'border-emerald-500/45 bg-emerald-500/10 text-emerald-500',
                  active && 'border-primary/60 bg-primary/10 text-primary',
                  error && 'border-destructive/55 bg-destructive/10 text-destructive',
                  status === 'pending' && 'border-border text-muted-foreground'
                )}
              >
                {done ? (
                  <Check className="size-2.5" strokeWidth={3} />
                ) : active ? (
                  <Loader2 className="size-2.5 animate-spin" />
                ) : error ? (
                  <X className="size-2.5" strokeWidth={2.5} />
                ) : (
                  index + 1
                )}
              </span>
              <span className="min-w-0">
                <span
                  className={cn(
                    'block text-[11.5px] font-medium capitalize',
                    active || done ? 'text-foreground' : 'text-muted-foreground',
                    error && 'text-destructive'
                  )}
                >
                  {stage}
                </span>
                <span className="mt-0.5 block text-[9.5px] leading-3.5 text-muted-foreground">
                  {statusLabel} · {stageDetails[stage]}
                </span>
              </span>
            </li>
          )
        })}
      </ol>
      {busy ? (
        <p className="mt-2 text-right text-[10px] leading-4 text-muted-foreground">
          Finish the current stage before closing.
        </p>
      ) : null}
    </section>
  )
}

// ---------- Step 1: Host ----------

function HostStep({
  hosts,
  selectedHostKey,
  lastUsedHostKey,
  hostMetaByKey,
  sshConnectionStates,
  repoCountForHost,
  onPick
}: {
  hosts: { key: string; kind: 'local' | 'ssh'; label: string }[]
  selectedHostKey: string
  lastUsedHostKey: string
  hostMetaByKey: Record<string, { detail?: string }>
  sshConnectionStates: ReadonlyMap<string, { status?: string }>
  repoCountForHost: (hostKey: string) => number
  onPick: (hostKey: string) => void
}): React.JSX.Element {
  return (
    <div className="flex animate-in flex-col gap-3.5 fade-in-0 slide-in-from-bottom-1">
      <div className="flex flex-col gap-0.5">
        <span className="text-[15px] font-semibold tracking-[-0.01em] text-foreground">
          Where should it run?
        </span>
        <span className="text-[12px] text-muted-foreground">
          The next step lists the repos that live on this host. Your last setup is pre-filled.
        </span>
      </div>
      <div className="flex flex-col gap-2.5">
        {hosts.map((host) => {
          const selected = host.key === selectedHostKey
          const count = repoCountForHost(host.key)
          const meta = hostMetaByKey[host.key]
          const connectionId = host.kind === 'ssh' ? host.key.replace(/^ssh:/, '') : null
          const live =
            host.kind === 'local' ||
            (connectionId
              ? sshConnectionStates.get(connectionId)?.status === 'connected'
              : false)
          const detail = meta?.detail ?? (host.kind === 'local' ? 'localhost' : 'ssh host')
          const sub = `${detail} · ${count} repo${count === 1 ? '' : 's'}`
          const Icon = host.kind === 'local' ? Laptop : Server
          return (
            <button
              key={host.key}
              type="button"
              onClick={() => onPick(host.key)}
              className={cn(
                'flex items-center gap-3 rounded-lg border px-3 py-3 text-left transition-colors',
                selected
                  ? 'border-muted-foreground/40 bg-secondary'
                  : 'border-border hover:border-muted-foreground/25 hover:bg-secondary/50'
              )}
            >
              <Icon className="size-[17px] flex-none text-muted-foreground" />
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="inline-flex items-center gap-2 text-[13px] font-medium text-foreground">
                  {host.label}
                  {live ? (
                    <span className="inline-block size-[7px] rounded-full bg-emerald-500 shadow-[0_0_0_3px] shadow-emerald-500/20" />
                  ) : null}
                </span>
                <span className="truncate font-mono text-[11px] text-muted-foreground">{sub}</span>
              </span>
              {host.key === lastUsedHostKey ? (
                <span className="flex-none rounded-full border border-border px-2 py-0.5 font-mono text-[10.5px] text-muted-foreground">
                  last used
                </span>
              ) : null}
            </button>
          )
        })}
      </div>
    </div>
  )
}

// ---------- Step 2: Repo & branch ----------

function RepoStep({
  hostLabel,
  repos,
  disabledRepoIds,
  repoId,
  onRepoChange,
  selectedRepo,
  selectedRepoIsGit,
  baseBranch,
  onBaseBranchChange,
  requiresConnection,
  connectInProgress,
  onConnect,
  onAddRepo,
  addingRepo,
  connectionId,
  remoteAddOpen,
  onCloseRemoteAdd,
  onRemoteRepoAdded
}: {
  hostLabel: string
  repos: Repo[]
  disabledRepoIds: Map<string, string>
  repoId: string
  onRepoChange: (value: string) => void
  selectedRepo: Repo | undefined
  selectedRepoIsGit: boolean
  baseBranch: string | undefined
  onBaseBranchChange: (next: string | undefined) => void
  requiresConnection: boolean
  connectInProgress: boolean
  onConnect: () => Promise<void>
  onAddRepo: () => void | Promise<void>
  addingRepo: boolean
  /** SSH connection behind the host (`ssh:<id>` → id), or '' for local. */
  connectionId: string
  /** Whether the inline remote-add panel is open (SSH hosts only). */
  remoteAddOpen: boolean
  onCloseRemoteAdd: () => void
  onRemoteRepoAdded: (repoId: string) => void
}): React.JSX.Element {
  // Many-project hosts render a wall of repo rows the operator has to scroll
  // past — collapse to the first few, with a search field + "show all" expander
  // to recover the rest fast. Filtering + capping are pure (unit-tested); the
  // component owns only the query + expanded local state.
  const [repoQuery, setRepoQuery] = useState('')
  const [reposExpanded, setReposExpanded] = useState(false)
  const filteredRepos = useMemo(() => filterRepoList(repos, repoQuery), [repos, repoQuery])
  const { visible: visibleRepos, hiddenCount } = useMemo(
    () => capRepoList({ repos: filteredRepos, expanded: reposExpanded, selectedId: repoId }),
    [filteredRepos, reposExpanded, repoId]
  )
  const showRepoSearch = repos.length > REPO_LIST_COLLAPSED_CAP

  return (
    <div className="flex animate-in flex-col gap-3.5 fade-in-0 slide-in-from-bottom-1">
      <div className="flex flex-col gap-0.5">
        <span className="text-[15px] font-semibold tracking-[-0.01em] text-foreground">
          Pick a repo on {hostLabel}
        </span>
        <span className="font-mono text-[11px] text-muted-foreground">
          {repos.length} repo{repos.length === 1 ? '' : 's'} · scanned just now
        </span>
      </div>

      {repos.length === 0 ? (
        <div className="flex flex-col items-center gap-3 rounded-lg border border-dashed border-border px-4 py-6 text-center">
          <span className="text-[12.5px] text-muted-foreground">No repos on {hostLabel} yet.</span>
          {!remoteAddOpen ? (
            <AddProjectButton onAddRepo={onAddRepo} addingRepo={addingRepo} variant="solid" />
          ) : null}
        </div>
      ) : (
        <div className="flex flex-col gap-2.5">
          {showRepoSearch ? (
            <div className="flex items-center gap-2 rounded-md border border-input bg-secondary px-2.5 focus-within:border-ring">
              <Search className="size-3.5 flex-none text-muted-foreground" />
              <input
                value={repoQuery}
                onChange={(event) => setRepoQuery(event.target.value)}
                placeholder={`Search ${repos.length} repos…`}
                className="h-[34px] flex-1 bg-transparent font-mono text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground/70"
              />
            </div>
          ) : null}

          {visibleRepos.length === 0 ? (
            <div className="rounded-lg border border-dashed border-border px-4 py-5 text-center text-[12px] text-muted-foreground">
              No repos match &ldquo;{repoQuery.trim()}&rdquo;.
            </div>
          ) : (
            visibleRepos.map((repo) => {
              const selected = repo.id === repoId
              const disabledReason = disabledRepoIds.get(repo.id)
              const isDisabled = Boolean(disabledReason)
              return (
                <button
                  key={repo.id}
                  type="button"
                  disabled={isDisabled}
                  onClick={() => onRepoChange(repo.id)}
                  title={disabledReason}
                  className={cn(
                    'grid grid-cols-[14px_1fr_auto] items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors',
                    isDisabled
                      ? 'cursor-not-allowed border-border opacity-50'
                      : selected
                        ? 'border-muted-foreground/40 bg-secondary'
                        : 'border-border hover:border-muted-foreground/25 hover:bg-secondary/50'
                  )}
                  style={{ boxShadow: `inset 3px 0 0 ${repo.badgeColor}` }}
                >
                  <span
                    className={cn(
                      'box-border size-[14px] rounded-full',
                      selected
                        ? 'border-[4px] border-foreground bg-background'
                        : 'border-[1.5px] border-muted-foreground/60'
                    )}
                  />
                  <span className="flex min-w-0 items-baseline gap-2.5">
                    <span className="text-[13.5px] font-medium text-foreground">
                      {repo.displayName}
                    </span>
                    <span className="truncate font-mono text-[11.5px] text-muted-foreground">
                      {repo.path}
                    </span>
                  </span>
                  <span className="rounded-full border border-border px-2 py-0.5 font-mono text-[10.5px] text-muted-foreground">
                    {disabledReason ? disabledReason : selectedRepoBadge(repo, selected)}
                  </span>
                </button>
              )
            })
          )}

          {hiddenCount > 0 ? (
            <button
              type="button"
              onClick={() => setReposExpanded(true)}
              className="inline-flex items-center gap-1.5 self-start rounded-md px-1 py-1 text-[12px] text-muted-foreground transition-colors hover:text-foreground"
            >
              <span className="font-mono leading-none">…</span>
              Show all {filteredRepos.length} repos
            </button>
          ) : null}

          {!remoteAddOpen ? (
            <AddProjectButton onAddRepo={onAddRepo} addingRepo={addingRepo} variant="row" />
          ) : null}
        </div>
      )}

      {/* SSH/remote "Add project" stays in the wizard: the host is already
          chosen, so we collect just a remote path (with a remote file browser)
          and select the new repo — no separate dialog, no "choose how to start"
          fork. Only rendered for SSH hosts (connectionId set). */}
      {connectionId && remoteAddOpen ? (
        <AddRemoteProjectPanel
          connectionId={connectionId}
          hostLabel={hostLabel}
          onAdded={onRemoteRepoAdded}
          onCancel={onCloseRemoteAdd}
        />
      ) : null}

      {requiresConnection ? (
        <button
          type="button"
          onClick={() => void onConnect()}
          disabled={connectInProgress}
          className="inline-flex items-center gap-2 self-start rounded-md border border-border px-3 py-1.5 text-[12.5px] text-foreground transition-colors hover:border-muted-foreground/40 disabled:opacity-60"
        >
          {connectInProgress ? (
            <Loader2 className="size-3.5 animate-spin" />
          ) : (
            <PlugZap className="size-3.5" />
          )}
          {connectInProgress ? 'Connecting…' : `Connect to ${hostLabel}`}
        </button>
      ) : null}

      {/* The worktree/workspace name lives in step 3 alongside agent choice. */}
      {selectedRepoIsGit ? (
        <div className="flex flex-col gap-2.5">
          <span className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-muted-foreground">
            Worktree
          </span>
          <label className="flex max-w-[240px] flex-col gap-1.5">
            <span className="text-[11.5px] text-muted-foreground">Base branch</span>
            <BaseBranchCombobox
              repoId={repoId}
              baseBranch={baseBranch}
              defaultRef={selectedRepo?.worktreeBaseRef ?? null}
              onChange={onBaseBranchChange}
            />
          </label>
          <span className="text-[11px] text-muted-foreground">
            The worktree is named in the next step.
          </span>
        </div>
      ) : null}
    </div>
  )
}

/**
 * Inline SSH/remote "Add project" — the in-wizard counterpart to the local
 * native folder picker. The SSH host is already chosen in step 1, so this only
 * needs a remote path (typed, or picked via the shared `RemoteFileBrowser`),
 * then registers the repo through `api.repos.addRemote` and hands the id back so
 * the wizard selects it. No separate dialog, no "choose how to start" setup step
 * — the user stays in the wizard exactly like the local flow.
 *
 * `api.repos.addRemote` doesn't touch the store, so we upsert the returned repo
 * ourselves (mirrors `AddRepoDialog`/`AddProjectFromFolderDialog`) so it appears
 * in the host-scoped list and is selectable.
 */
function AddRemoteProjectPanel({
  connectionId,
  hostLabel,
  onAdded,
  onCancel
}: {
  connectionId: string
  hostLabel: string
  onAdded: (repoId: string) => void
  onCancel: () => void
}): React.JSX.Element {
  const sshConnectionStates = useAppStore((s) => s.sshConnectionStates)
  const fetchWorktrees = useAppStore((s) => s.fetchWorktrees)
  const [remotePath, setRemotePath] = useState('~/')
  const [browsing, setBrowsing] = useState(false)
  const [adding, setAdding] = useState(false)
  const [connecting, setConnecting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const mountedRef = useMountedRef()

  const connected = sshConnectionStates.get(connectionId)?.status === 'connected'

  const handleConnect = useCallback(async () => {
    setConnecting(true)
    setError(null)
    try {
      // Dynamic import: server-host-client pulls in `@/store`; a static import
      // from this wizard chunk risks the known boot-time TDZ cycle. Deferring to
      // call time keeps the module graph acyclic (mirrors the repo store slice).
      const { connectSshTargetViaServer } = await import('@/runtime/server-host-client')
      const result = await connectSshTargetViaServer(connectionId)
      if (!result.ok && mountedRef.current) {
        setError(result.message)
      }
    } catch (err) {
      if (mountedRef.current) {
        setError(err instanceof Error ? err.message : String(err))
      }
    } finally {
      if (mountedRef.current) {
        setConnecting(false)
      }
    }
  }, [connectionId, mountedRef])

  const handleAdd = useCallback(async () => {
    const path = remotePath.trim()
    if (!path || adding) {
      return
    }
    setAdding(true)
    setError(null)
    try {
      const result = await api.repos.addRemote({ connectionId, remotePath: path })
      if ('error' in result) {
        if (mountedRef.current) {
          setError(result.error)
        }
        return
      }
      const repo = result.repo
      const state = useAppStore.getState()
      const existingIdx = state.repos.findIndex((r) => r.id === repo.id)
      if (existingIdx === -1) {
        useAppStore.setState({ repos: [...state.repos, repo] })
      } else {
        state.clearAgentumHookTrustForRepo(repo.id)
        const updated = [...state.repos]
        updated[existingIdx] = repo
        useAppStore.setState({ repos: updated })
      }
      toast.success('Remote project added', { description: repo.displayName })
      await fetchWorktrees(repo.id)
      if (!mountedRef.current) {
        return
      }
      onAdded(repo.id)
    } catch (err) {
      if (mountedRef.current) {
        setError(err instanceof Error ? err.message : String(err))
      }
    } finally {
      if (mountedRef.current) {
        setAdding(false)
      }
    }
  }, [adding, connectionId, fetchWorktrees, mountedRef, onAdded, remotePath])

  if (browsing) {
    return (
      <div className="rounded-lg border border-border bg-secondary/40 p-2">
        <RemoteFileBrowser
          targetId={connectionId}
          initialPath={remotePath || '~'}
          onSelect={(path) => {
            setRemotePath(path)
            setBrowsing(false)
          }}
          onCancel={() => setBrowsing(false)}
        />
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-2.5 rounded-lg border border-border bg-secondary/40 p-3">
      <span className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-muted-foreground">
        Add a project on {hostLabel}
      </span>

      {!connected ? (
        <div className="flex items-center justify-between gap-3">
          <span className="text-[12px] text-muted-foreground">
            Connect to browse this host&apos;s folders.
          </span>
          <button
            type="button"
            onClick={() => void handleConnect()}
            disabled={connecting}
            className="inline-flex flex-none items-center gap-2 rounded-md border border-border px-3 py-1.5 text-[12.5px] text-foreground transition-colors hover:border-muted-foreground/40 disabled:opacity-60"
          >
            {connecting ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <PlugZap className="size-3.5" />
            )}
            {connecting ? 'Connecting…' : 'Connect'}
          </button>
        </div>
      ) : (
        <>
          <div className="flex gap-2">
            <input
              value={remotePath}
              onChange={(event) => {
                setRemotePath(event.target.value)
                setError(null)
              }}
              onKeyDown={(event) => {
                if (event.key === 'Enter' && remotePath.trim() && !adding) {
                  event.preventDefault()
                  void handleAdd()
                }
              }}
              placeholder="/home/user/project"
              spellCheck={false}
              disabled={adding}
              className="h-[34px] min-w-0 flex-1 rounded-md border border-input bg-secondary px-2.5 font-mono text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
            />
            <button
              type="button"
              onClick={() => setBrowsing(true)}
              disabled={adding}
              aria-label="Browse remote folders"
              className="inline-flex size-[34px] flex-none items-center justify-center rounded-md border border-border text-muted-foreground transition-colors hover:border-muted-foreground/40 hover:text-foreground disabled:opacity-60"
            >
              <FolderOpen className="size-3.5" />
            </button>
          </div>

          {error ? <span className="text-[11px] text-destructive">{error}</span> : null}

          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={onCancel}
              className="rounded-md px-2 py-1 text-[12px] text-muted-foreground transition-colors hover:text-foreground"
            >
              Cancel
            </button>
            <span className="flex-1" />
            <button
              type="button"
              onClick={() => void handleAdd()}
              disabled={!remotePath.trim() || adding}
              className="inline-flex items-center gap-2 rounded-md bg-primary px-3.5 py-1.5 text-[12.5px] font-medium text-primary-foreground transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-50"
            >
              {adding ? (
                <Loader2 className="size-3.5 animate-spin" />
              ) : (
                <FolderPlus className="size-3.5" />
              )}
              {adding ? 'Adding…' : 'Add project'}
            </button>
          </div>
        </>
      )}
    </div>
  )
}

/**
 * The step-2 "Add project" affordance. `solid` is the primary CTA shown in the
 * empty state; `row` is the dashed, list-aligned entry rendered under an
 * existing repo list. Both call `onAddRepo` (local: native folder picker →
 * auto-select; SSH: the inline remote-add panel).
 */
function AddProjectButton({
  onAddRepo,
  addingRepo,
  variant
}: {
  onAddRepo: () => void | Promise<void>
  addingRepo: boolean
  variant: 'solid' | 'row'
}): React.JSX.Element {
  const Icon = addingRepo ? Loader2 : FolderPlus
  const icon = <Icon className={cn('size-3.5 flex-none', addingRepo && 'animate-spin')} />
  if (variant === 'solid') {
    return (
      <button
        type="button"
        onClick={() => void onAddRepo()}
        disabled={addingRepo}
        className="inline-flex items-center gap-2 rounded-md border border-border px-3 py-1.5 text-[12.5px] text-foreground transition-colors hover:border-muted-foreground/40 disabled:opacity-60"
      >
        {icon}
        {addingRepo ? 'Adding…' : 'Add project'}
      </button>
    )
  }
  return (
    <button
      type="button"
      onClick={() => void onAddRepo()}
      disabled={addingRepo}
      className="flex items-center gap-2.5 rounded-lg border border-dashed border-border px-3 py-2.5 text-left text-[12.5px] text-muted-foreground transition-colors hover:border-muted-foreground/40 hover:text-foreground disabled:opacity-60"
    >
      {icon}
      {addingRepo ? 'Adding…' : 'Add project'}
    </button>
  )
}

/** The right-side chip on a repo row: local git/folder, or the remote marker. */
function selectedRepoBadge(repo: Repo, selected: boolean): string {
  if (repo.connectionId) {
    return selected ? 'remote' : 'ssh'
  }
  return isFolderRepo(repo) ? 'folder' : 'git'
}

/**
 * Base-branch picker: a searchable combobox over the repo's refs (spec 011).
 * Replaces the old free-text input — the user picks a branch from a list rather
 * than having to know and type its name. Reuses `searchRuntimeRepoBaseRefs` (the
 * same client Settings' BaseRefPicker uses); an empty query returns the repo's
 * full ref list, so the list is populated the moment the popover opens. Picking
 * "Default branch" clears the pin (`baseBranch = undefined`); a typed-but-unlisted
 * value stays committable so arbitrary refs (e.g. `upstream/main`) still work.
 */
function BaseBranchCombobox({
  repoId,
  baseBranch,
  defaultRef,
  onChange
}: {
  repoId: string
  baseBranch: string | undefined
  defaultRef: string | null
  onChange: (next: string | undefined) => void
}): React.JSX.Element {
  const settings = useAppStore((s) => s.settings)
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [refs, setRefs] = useState<string[]>([])
  const [loading, setLoading] = useState(false)

  // Fetch (debounced) whenever the popover is open and the query changes. An
  // empty query returns the repo's full ref list (backend filters on
  // `needle.is_empty() || contains`), so the list fills as soon as it opens.
  useEffect(() => {
    if (!open || !repoId) {
      return
    }
    let stale = false
    setLoading(true)
    const timer = window.setTimeout(() => {
      void searchRuntimeRepoBaseRefs(settings, repoId, query.trim(), 50)
        .then((results) => {
          if (!stale) {
            setRefs(results)
          }
        })
        .catch((err) => {
          // Degrade to an empty list (Default + free-text options still let the
          // user proceed) rather than blocking the step on a search hiccup.
          console.error('[BaseBranchCombobox] searchBaseRefs failed', err)
          if (!stale) {
            setRefs([])
          }
        })
        .finally(() => {
          if (!stale) {
            setLoading(false)
          }
        })
    }, 180)
    return () => {
      stale = true
      window.clearTimeout(timer)
    }
  }, [open, query, repoId, settings])

  const usingDefault = !baseBranch?.trim()
  const triggerLabel = wizardBaseBranchTriggerLabel(baseBranch, defaultRef)
  const trimmedQuery = query.trim()
  const showRawOption =
    trimmedQuery.length > 0 && !refs.some((ref) => ref.toLowerCase() === trimmedQuery.toLowerCase())

  const commit = useCallback(
    (ref: string | undefined) => {
      const trimmed = ref?.trim()
      onChange(trimmed ? trimmed : undefined)
      setOpen(false)
      setQuery('')
    },
    [onChange]
  )

  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next)
        if (!next) {
          setQuery('')
        }
      }}
    >
      <PopoverTrigger asChild>
        <button
          type="button"
          role="combobox"
          aria-expanded={open}
          className="flex h-[34px] items-center gap-2 rounded-md border border-input bg-secondary px-2.5 text-left focus-visible:border-ring focus-visible:outline-none"
        >
          <GitBranch className="size-3.5 flex-none text-muted-foreground" />
          <span
            className={cn(
              'min-w-0 flex-1 truncate font-mono text-[12.5px]',
              usingDefault ? 'text-muted-foreground/80' : 'text-foreground'
            )}
          >
            {triggerLabel}
          </span>
          <ChevronsUpDown className="size-3.5 flex-none opacity-50" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className="w-[var(--radix-popover-trigger-width)] min-w-[220px] p-0"
      >
        <Command shouldFilter={false}>
          <CommandInput
            autoFocus
            placeholder="Search branches…"
            value={query}
            onValueChange={setQuery}
            className="text-xs"
          />
          <CommandList>
            {loading ? (
              <div className="px-3 py-2 text-[11.5px] text-muted-foreground">Searching…</div>
            ) : null}
            <CommandEmpty>No matching branches.</CommandEmpty>
            <CommandItem value="__default__" onSelect={() => commit(undefined)} className="gap-2 text-xs">
              <Check className={cn('size-3', usingDefault ? 'opacity-70' : 'opacity-0')} />
              <span className="text-muted-foreground">
                Default branch{defaultRef ? ` (${defaultRef})` : ''}
              </span>
            </CommandItem>
            {showRawOption ? (
              <CommandItem
                value={`__raw__:${trimmedQuery}`}
                onSelect={() => commit(trimmedQuery)}
                className="gap-2 text-xs"
              >
                <Check className="size-3 opacity-0" />
                <span className="truncate">
                  Use <span className="font-mono">{trimmedQuery}</span>
                </span>
              </CommandItem>
            ) : null}
            {refs.map((ref) => {
              const selected = baseBranch?.trim() === ref
              return (
                <CommandItem key={ref} value={ref} onSelect={() => commit(ref)} className="gap-2 text-xs">
                  <Check className={cn('size-3', selected ? 'opacity-70' : 'opacity-0')} />
                  <span className="truncate font-mono">{ref}</span>
                </CommandItem>
              )
            })}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  )
}

// ---------- Step 3: Workspace & agent ----------

/** Tracker-neutral manual workspace controls. */
function AgentStep({
  agents,
  detectedAgentIds,
  quickAgent,
  onPick,
  selectedRepoIsGit,
  repoDisplayName,
  name,
  onNameValueChange,
  nameInputRef,
  requiresExplicitSetupChoice,
  setupDecision,
  onSetupDecisionChange,
  worktreeLocked,
  useSdd,
  onUseSddChange,
  sddDescription,
  onSddDescriptionChange,
}: {
  agents: TuiAgent[]
  detectedAgentIds: Set<TuiAgent> | null
  quickAgent: TuiAgent | null
  onPick: (agent: TuiAgent) => void
  selectedRepoIsGit: boolean
  repoDisplayName?: string
  name: string
  onNameValueChange: (value: string) => void
  nameInputRef: React.RefObject<HTMLInputElement | null>
  requiresExplicitSetupChoice: boolean
  setupDecision: 'run' | 'skip' | null
  onSetupDecisionChange: (value: 'run' | 'skip') => void
  worktreeLocked: boolean
  useSdd: boolean
  onUseSddChange: (value: boolean) => void
  sddDescription: string
  onSddDescriptionChange: (value: string) => void
}): React.JSX.Element {
  return (
    <div className="flex animate-in flex-col gap-[18px] fade-in-0 slide-in-from-bottom-1">
      <div className="flex flex-col gap-0.5">
        <span className="text-[15px] font-semibold tracking-[-0.01em] text-foreground">
          Name the workspace — and choose its agent
        </span>
        <span className="text-[12px] text-muted-foreground">
          This manual path creates a {selectedRepoIsGit ? 'worktree' : 'workspace'} without a
          tracker source. Use New Spec for GitHub, Linear, Jira, or imported work.
        </span>
      </div>

      <div className="flex flex-col gap-2.5">
        <span className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-muted-foreground">
          {selectedRepoIsGit ? 'Worktree name' : 'Workspace name'}
        </span>
        <input
          ref={nameInputRef}
          value={name}
          disabled={worktreeLocked}
          onChange={(event) => onNameValueChange(event.target.value)}
          placeholder="auto"
          className="h-[34px] rounded-md border border-input bg-secondary px-2.5 font-mono text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
        />
        {!selectedRepoIsGit && repoDisplayName ? (
          <span className="text-[11px] text-muted-foreground">
            {repoDisplayName} isn&apos;t a git repo — the workspace opens the folder as-is.
          </span>
        ) : null}
      </div>

      {/* SDD Toggle */}
      <div className="flex flex-col gap-2.5">
        <div className="flex items-center gap-2.5">
          <button
            type="button"
            role="switch"
            aria-checked={useSdd}
            disabled={worktreeLocked}
            onClick={() => onUseSddChange(!useSdd)}
            className={cn(
              'relative inline-flex h-5 w-9 flex-none cursor-pointer rounded-full border-2 border-transparent transition-colors',
              useSdd ? 'bg-primary' : 'bg-muted',
              worktreeLocked && 'cursor-not-allowed opacity-50'
            )}
          >
            <span
              className={cn(
                'pointer-events-none inline-block size-4 rounded-full bg-white shadow-sm transition-transform',
                useSdd ? 'translate-x-4' : 'translate-x-0'
              )}
            />
          </button>
          <div className="flex flex-col gap-0.5">
            <span className="text-[12.5px] font-medium text-foreground">
              Start with SDD spec
            </span>
            <span className="text-[10.5px] text-muted-foreground">
              Generate a specification and run the full workflow
            </span>
          </div>
        </div>

        {useSdd ? (
          <div className="flex flex-col gap-1.5 rounded-lg border border-border bg-muted/25 p-3">
            <span className="text-[11px] font-medium text-muted-foreground">
              Feature description
            </span>
            <textarea
              value={sddDescription}
              disabled={worktreeLocked}
              onChange={(event) => onSddDescriptionChange(event.target.value)}
              rows={4}
              placeholder="Describe what you want to build...

## Requirements
- RQ-001 ...

## Acceptance Criteria
- AC-001 ..."
              className="resize-none rounded-md border border-input bg-secondary px-2.5 py-2 font-mono text-[11.5px] leading-relaxed outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
            />
            <span className="text-[10px] text-muted-foreground">
              The first line becomes the spec title. Markdown is supported.
            </span>
          </div>
        ) : null}
      </div>

      {requiresExplicitSetupChoice ? (
        <div className="flex flex-col gap-2.5">
          <div>
            <span className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-muted-foreground">
              Project setup
            </span>
            <p className="mt-1 text-[11px] text-muted-foreground">
              This project asks whether its setup script should run in the new workspace.
            </p>
          </div>
          <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
            {(['run', 'skip'] as const).map((decision) => (
              <button
                key={decision}
                type="button"
                disabled={worktreeLocked}
                aria-pressed={setupDecision === decision}
                onClick={() => onSetupDecisionChange(decision)}
                className={cn(
                  'rounded-lg border px-3 py-2 text-left text-[12px] transition-colors',
                  setupDecision === decision
                    ? 'border-primary/55 bg-primary/8 text-foreground'
                    : 'border-border text-muted-foreground hover:border-muted-foreground/30',
                  worktreeLocked && 'cursor-not-allowed opacity-50'
                )}
              >
                <span className="block font-medium">
                  {decision === 'run' ? 'Run setup' : 'Skip setup'}
                </span>
                <span className="block text-[10.5px] text-muted-foreground">
                  {decision === 'run'
                    ? 'Prepare dependencies before the agent starts'
                    : 'Open the workspace without running the script'}
                </span>
              </button>
            ))}
          </div>
        </div>
      ) : null}

      <div className="flex flex-col gap-2.5">
        <span className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-muted-foreground">
          Agent
        </span>
        <div className="flex flex-wrap gap-2">
          {agents.map((agent) => {
            const selected = agent === quickAgent
            const installed = detectedAgentIds ? detectedAgentIds.has(agent) : true
            const label = AGENT_CATALOG.find((entry) => entry.id === agent)?.label ?? agent
            return (
              <button
                key={agent}
                type="button"
                disabled={worktreeLocked}
                onClick={() => onPick(agent)}
                title={installed ? undefined : 'Not detected on PATH'}
                className={cn(
                  'inline-flex items-center gap-2 rounded-full border px-3 py-1.5 text-[12.5px] transition-colors',
                  selected
                    ? 'border-muted-foreground/40 bg-secondary text-foreground'
                    : 'border-border text-muted-foreground hover:border-muted-foreground/25',
                  !installed && !selected ? 'opacity-55' : '',
                  worktreeLocked ? 'cursor-not-allowed opacity-60' : ''
                )}
              >
                <AgentIcon agent={agent} size={13} />
                {label}
                {!installed ? (
                  <span className="font-mono text-[10px] text-muted-foreground/70">·</span>
                ) : null}
              </button>
            )
          })}
        </div>
        {agents.length === 0 ? (
          <span className="text-[11.5px] text-muted-foreground">
            No agents detected on PATH — install or detect one before starting work.
          </span>
        ) : null}
      </div>

      <p className="rounded-lg border border-border bg-muted/25 px-3 py-2 text-[11px] text-muted-foreground">
        {useSdd
          ? 'The workspace opens with an SDD run in Run Center. The provider writes the spec, you approve it, then implementation starts.'
          : 'This opens one agent in the workspace. Specification authoring starts explicitly from Run Center.'}
      </p>
    </div>
  )
}
