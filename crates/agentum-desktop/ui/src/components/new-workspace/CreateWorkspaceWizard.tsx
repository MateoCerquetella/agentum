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
  KanbanSquare,
  Laptop,
  Loader2,
  PlugZap,
  RefreshCw,
  Search,
  Server,
  Sparkles,
  WandSparkles,
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import { api } from '@/tauri'
import { toast } from 'sonner'
import { searchRuntimeRepoBaseRefs } from '@/runtime/runtime-repo-client'
import { cn } from '@/lib/utils'
import { useAppStore } from '@/store'
import { useMountedRef } from '@/hooks/useMountedRef'
import { useDetectedAgents } from '@/hooks/useDetectedAgents'
import { RemoteFileBrowser } from '@/components/sidebar/RemoteFileBrowser'
import { useComposerState } from '@/hooks/useComposerState'
import { IssueSpecInterviewDialog } from './IssueSpecInterviewDialog'
import {
  pickQuickWorkspaceAgent,
  resolveQuickWorkspaceAgentSelection
} from '@/lib/quick-workspace-agent-selection'
import { AGENT_CATALOG, AgentIcon } from '@/lib/agent-catalog'
import { isFolderRepo, isGitRepoKind } from '../../../../shared/repo-kind'
import { filterReposForHost } from '@/hooks/composer-host-scoping'
import { LOCAL_HOST_KEY } from '@/components/sidebar/worktree-list-groups'
import type { LinkedWorkItemSummary } from '@/lib/new-workspace'
import {
  REPO_LIST_COLLAPSED_CAP,
  WIZARD_STEP_LABELS,
  buildWizardRecap,
  canLeaveRepoStep as canLeaveRepoStepModel,
  capRepoList,
  deriveWizardComposerSeed,
  filterRepoList,
  resolveWizardAgentOptions,
  wizardBaseBranchTriggerLabel,
  wizardPrimaryLabel,
  type CreateWorkspaceWizardData,
  type WizardStep
} from '@/components/new-workspace/create-workspace-wizard-model'
import {
  buildBindPayload,
  deriveTrackerIssueViewModel,
  type PickerProjectRef,
  type WorkItemOption
} from '@/components/new-workspace/work-item-picker-model'
import {
  isCurrentTrackerSectionScope,
  trackerSectionTableForScope
} from '@/components/new-workspace/tracker-section-scope'
import {
  canFileIssue,
  deriveCreateIssueIntentPhase
} from '@/components/new-workspace/create-issue-intent-model'
import {
  linearCreateIssue,
  linearListCustomViewIssues,
  linearListIssues,
  linearListProjectIssues,
  linearListTeams,
  type RuntimeLinearSettings
} from '@/runtime/runtime-linear-client'
import {
  CHAT_AGENTS,
  CHAT_MODELS,
  pickChatAgent,
  resolveChatModel,
  type ChatAgentId
} from '@/runtime/chat-client'
import {
  readChatModelPreference,
  writeChatModelPreference
} from '@/runtime/chat-preferences'
import type { DraftIssueStyle, DraftLlmChoice } from '@/runtime/github-issue-client'
import type {
  GetProjectViewTableArgs,
  GetProjectViewTableResult,
  GitHubProjectTable
} from '@/shared/github-project-types'
import type {
  GitHubWorkItem,
  LinearIssue,
  LinearTeam,
  Repo,
  TuiAgent
} from '../../../../shared/types'
import type {
  ProjectTrackerConfig,
  ProjectTrackerLinearTarget,
  ProjectTrackerProvider
} from '@/shared/project-tracker-config'
import { buildLinearIssueLinkedWorkItem } from '@/lib/linear-linked-work-item'
import {
  NEW_WORK_STAGES,
  canLaunchNewWork,
  canSelectWorkSource,
  deriveNewWorkEligibility,
  initialNewWorkProgress,
  isNewWorkRetryAvailable,
  newWorkBusyLabel,
  newWorkPrimaryLabel,
  resolveLaunchIssue,
  shouldDefaultNewWorkToManual,
  updateNewWorkProgress,
  type ExecutionMode,
  type NewWorkCheckpoint,
  type NewWorkEligibility,
  type NewWorkProgress,
  type WorkSource
} from './new-work-launch-model'

/**
 * The "Create Workspace" wizard — a three-step front-end (Host → Repo &
 * branch → Issue & agent) over the shared `useComposerState` creation
 * engine. The issue is linked/created BEFORE the worktree is named — step 3
 * renders tracker → worktree name → agent, so `applyLinkedWorkItem`'s
 * title-derived auto-name lands in a visible, editable field.
 * Spec 013 F4: it is the SINGLE front door for `new-workspace-composer`
 * — it never becomes a state machine inside the engine, it drives the same
 * host/repo/name/baseBranch/agent state the composer card drove and calls the
 * same `submitQuick`, so YOLO translation, SSH gating, setup hooks, the gated
 * run (`start_work`) and post-create launch stay centralized (no new paths).
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
  const fetchProjectViewTable = useAppStore((s) => s.fetchProjectViewTable)
  const getCachedProjectViewTable = useAppStore((s) => s.getCachedProjectViewTable)
  const addRepoFromStore = useAppStore((s) => s.addRepo)
  const fetchWorktrees = useAppStore((s) => s.fetchWorktrees)
  // Spec 013 F4: seed the SAME `useComposerState` from the full modal-open data
  // (pure `deriveWizardComposerSeed` — every opinionated field honored). The
  // gate mode / issue-automation / submit path are unchanged, so the gated run
  // is inherited byte-identically (inv. 4).
  const { cardProps, submitQuick, nameInputRef } = useComposerState({
    ...deriveWizardComposerSeed(modalData),
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
    applyLinkedWorkItem,
    linkedWorkItem,
    onRemoveLinkedWorkItem,
    onLinkPopoverOpenChange,
    linkQuery,
    onLinkQueryChange,
    filteredLinkItems,
    linkItemsLoading,
    linkDirectLoading,
    onSelectLinkedItem,
    // Spec 013 F2: the composer's EXISTING create-issue seams — the wizard
    // shares the hook, so it renders them rather than rebuilding the flow.
    canCreateGithubIssue,
    createIssueTitle,
    onCreateIssueTitleChange,
    createIssueBody,
    onCreateIssueBodyChange,
    onApplyCreateIssueDraft,
    createIssueLabels,
    createIssueLabelOptions,
    onToggleCreateIssueLabel,
    createIssueGenerating,
    onGenerateIssueBody,
    createIssueSubmitting,
    createIssueError,
    onCreateIssueSubmit,
    // Spec 013 F3: bind a filed Linear issue through the SAME composer seam the
    // Linear @-picker uses (`setLinkedWorkItem(buildLinearIssueLinkedWorkItem)`).
    onSmartLinearIssueSelect,
    requiresExplicitSetupChoice,
    setupDecision,
    onSetupDecisionChange,
    // Spec 013 F4: the gated-run toggle — the SAME seams the composer card used,
    // so `submitQuick` inherits the `start_work` precondition set unchanged.
  } = cardProps
  const trackerConfig = useAppStore((state) =>
    repoId ? state.projectTrackerConfigByRepo[repoId] : undefined
  )
  const trackerConfigLoadStatus = useAppStore((state) =>
    repoId ? (state.projectTrackerLoadStatusByRepo[repoId] ?? 'idle') : 'idle'
  )
  const trackerConfigError = useAppStore((state) =>
    repoId ? state.projectTrackerErrorByRepo[repoId] : undefined
  )
  const loadProjectTrackerConfig = useAppStore((state) => state.loadProjectTrackerConfig)
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
  const initialWorkSource: WorkSource = linkedWorkItem ? 'existing' : 'new'
  const [workSource, setWorkSource] = useState<WorkSource>(initialWorkSource)
  const [executionMode, setExecutionMode] = useState<ExecutionMode>('autopilot')
  const [launchCheckpoint, setLaunchCheckpoint] = useState<NewWorkCheckpoint>({})
  const [launchProgress, setLaunchProgress] = useState(() =>
    initialNewWorkProgress(linkedWorkItem ? { linkedWorkItem } : {}, initialWorkSource)
  )
  const [launchInFlight, setLaunchInFlight] = useState(false)
  const launchInFlightRef = useRef(false)
  const configuredIssueCreatorRef = useRef<() => Promise<LinkedWorkItemSummary | null>>(
    onCreateIssueSubmit
  )
  const [addingRepo, setAddingRepo] = useState(false)
  // Spec: SSH/remote "Add project" is inline in the wizard (not a separate
  // dialog) — this toggles the inline remote-add panel in step 2. Reset on host
  // switch so it never lingers over a host that can't use it.
  const [remoteAddOpen, setRemoteAddOpen] = useState(false)
  // Badge the host we opened on ("last used") — captured once so re-selecting
  // doesn't move the badge around.
  const lastUsedHostKeyRef = useRef(selectedHostKey)

  const selectedRepo = repos.find((repo) => repo.id === repoId)
  // Creation is slug-addressed and the server resolves an SSH repo's origin
  // through its registry id, so remote git repos support the same source card.
  const configuredTrackerReady =
    trackerConfigLoadStatus === 'loaded' && Boolean(trackerConfig?.provider)
  const canStageConfiguredIssue = selectedRepoIsGit && configuredTrackerReady
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
    requiresConnection: selectedRepoRequiresConnection
  })

  // Spec 012 F1: binding a picked issue routes through the composer's one
  // attach seam (`applyLinkedWorkItem`) so `submitQuick` persists the tracker
  // bind on create. Widen the picked option to a GitHubWorkItem — the seam
  // ignores the inert fields (precedent: TaskPage / WorktreeCard stubs).
  const onPickWorkItem = useCallback(
    (option: WorkItemOption) => {
      const { summary } = buildBindPayload(option)
      const item: GitHubWorkItem = {
        id: option.itemId,
        type: 'issue',
        number: summary.number,
        title: summary.title,
        state: 'open',
        url: summary.url,
        labels: [],
        updatedAt: new Date().toISOString(),
        author: null
      }
      applyLinkedWorkItem(item)
    },
    [applyLinkedWorkItem]
  )

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
      // Populate worktrees for git repos so the worktree/base-branch fields have
      // data the moment the row is selected (matches RepoCombobox's add flow).
      if (isGitRepoKind(repo)) {
        await fetchWorktrees(repo.id)
      }
      onRepoChange(repo.id)
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

  const eligibility = deriveNewWorkEligibility({
    isGit: selectedRepoIsGit,
    source: workSource,
    newIssueProvider:
      trackerConfigLoadStatus === 'loaded' ? (trackerConfig?.provider ?? null) : undefined,
    linkedWorkItem,
    selectedAgentInstalled: Boolean(quickAgent && (!detectedAgentIds || detectedAgentIds.has(quickAgent))),
    setupBlocked:
      selectedRepoRequiresConnection || (requiresExplicitSetupChoice && !setupDecision)
  })

  const structurallyManual = shouldDefaultNewWorkToManual({
    isGit: selectedRepoIsGit,
    source: workSource,
    trackerConfigLoaded: trackerConfigLoadStatus === 'loaded',
    newIssueProvider: trackerConfig?.provider ?? null,
    linkedWorkItem
  })

  // Demote only structural incompatibilities. A loading tracker, missing
  // Existing selection, unavailable agent, or pending setup choice is
  // transient and must not strand a later valid GitHub selection in Manual.
  useEffect(() => {
    if (step === 3 && structurallyManual && executionMode === 'autopilot') {
      setExecutionMode('manual')
    }
  }, [executionMode, step, structurallyManual])

  const handleWorkSourceChange = useCallback(
    (source: WorkSource): void => {
      if (launchCheckpoint.linkedWorkItem) return
      setWorkSource(source)
      setLaunchProgress(initialNewWorkProgress({}, source))
      if (source !== 'existing') {
        onRemoveLinkedWorkItem()
      }
      if (source === 'none') {
        setExecutionMode('manual')
      }
    },
    [launchCheckpoint.linkedWorkItem, onRemoveLinkedWorkItem]
  )

  useEffect(() => {
    if (step !== 3 || !selectedRepo?.id) return
    void loadProjectTrackerConfig(selectedRepo.id).catch(() => undefined)
  }, [loadProjectTrackerConfig, selectedRepo?.id, step])

  useEffect(() => {
    if (
      step === 3 &&
      selectedRepo &&
      !launchCheckpoint.linkedWorkItem &&
      trackerConfigLoadStatus === 'loaded' &&
      !trackerConfig?.provider &&
      workSource !== 'none'
    ) {
      handleWorkSourceChange('none')
    }
  }, [handleWorkSourceChange, launchCheckpoint.linkedWorkItem, selectedRepo, step, trackerConfig?.provider, trackerConfigLoadStatus, workSource])

  const launchAllowed = canLaunchNewWork({
    source: workSource,
    executionMode,
    eligibility,
    hasSelectedAgent: Boolean(quickAgent),
    canStageNewIssue: canStageConfiguredIssue,
    hasNewIssueTitle: Boolean(createIssueTitle.trim()),
    hasSelectedIssue: Boolean(linkedWorkItem),
    hasIssueCheckpoint: Boolean(launchCheckpoint.linkedWorkItem)
  })

  const handlePrimary = useCallback(async () => {
    if (step === 3) {
      if (
        launchInFlightRef.current ||
        creating ||
        createIssueSubmitting ||
        !launchAllowed
      ) {
        return
      }
      launchInFlightRef.current = true
      setLaunchInFlight(true)
      try {
        let checkpoint = launchCheckpoint
        let issue = checkpoint.linkedWorkItem ?? null
        if (!issue && workSource !== 'none') {
          setLaunchProgress((current) => updateNewWorkProgress(current, 'issue', 'active'))
          const resolved = await resolveLaunchIssue({
            source: workSource,
            selectedIssue: linkedWorkItem,
            checkpoint,
            createIssue: () => configuredIssueCreatorRef.current()
          })
          if (!resolved.issue) {
            setLaunchProgress((current) => updateNewWorkProgress(current, 'issue', 'error'))
            return
          }
          checkpoint = resolved.checkpoint
          issue = resolved.issue
          setLaunchCheckpoint(checkpoint)
          setLaunchProgress((current) => updateNewWorkProgress(current, 'issue', 'done'))
        } else if (workSource === 'none') {
          setLaunchProgress((current) => updateNewWorkProgress(current, 'issue', 'done'))
        }
        await submitQuick(quickAgent, {
          linkedWorkItem: workSource === 'none' ? null : issue,
          executionMode,
          checkpoint,
          onCheckpoint: setLaunchCheckpoint,
          onProgress: (stage, status) =>
            setLaunchProgress((current) => updateNewWorkProgress(current, stage, status))
        })
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
  }, [canLeaveRepoStep, createIssueSubmitting, creating, executionMode, goNext, launchAllowed, launchCheckpoint, linkedWorkItem, quickAgent, step, submitQuick, workSource])

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

  const launchBusy = launchInFlight || creating || createIssueSubmitting
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
        showCloseButton={false}
        onKeyDown={handleKeyDown}
        className="flex max-h-[min(680px,calc(100dvh-4rem))] w-full flex-col gap-0 overflow-hidden p-0 sm:max-w-[640px]"
      >
        <DialogTitle className="sr-only">New workspace</DialogTitle>
        <DialogDescription className="sr-only">
          Create a workspace in three steps: choose a host, a repo and base branch, then the
          issue, worktree name, and agent.
        </DialogDescription>

        {/* Header: title, step chip, recap, close */}
        <div className="flex flex-none flex-col gap-3 px-[18px] pt-4">
          <div className="flex items-center gap-2.5">
            <span className="text-[15px] font-semibold tracking-[-0.01em] text-foreground">
              New workspace
            </span>
            <span className="font-mono text-[11px] text-muted-foreground">step {step} / 3</span>
            <span className="flex-1" />
            {recap ? (
              <span className="max-w-[260px] truncate font-mono text-[11px] text-muted-foreground">
                {recap}
              </span>
            ) : null}
            <button
              type="button"
              disabled={launchBusy}
              onClick={onClose}
              aria-label="Close"
              className="inline-flex size-6 flex-none items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
            >
              <X className="size-3.5" />
            </button>
          </div>
          <StepDots step={step} locked={launchBusy} onJump={(target) => setStep(target)} />
          <div className="h-px bg-border" />
        </div>

        {/* Body */}
        <div className="min-h-0 flex-1 overflow-y-auto px-[18px] py-4">
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
              disabled={launchBusy}
              className="m-0 min-w-0 border-0 p-0 disabled:cursor-wait disabled:opacity-80"
            >
              <AgentStep
              agents={agentOptions}
              detectedAgentIds={detectedAgentIds}
              quickAgent={quickAgent}
              onPick={setQuickAgentOverride}
              selectedRepoIsGit={selectedRepoIsGit}
              repo={selectedRepo}
              repoDisplayName={selectedRepo?.displayName}
              name={name}
              onNameValueChange={onNameValueChange}
              nameInputRef={nameInputRef}
              trackerConfig={trackerConfig}
              trackerConfigLoadStatus={trackerConfigLoadStatus}
              trackerConfigError={trackerConfigError}
              onRetryTrackerConfig={() => selectedRepo ? void loadProjectTrackerConfig(selectedRepo.id, { force: true }).catch(() => undefined) : undefined}
              fetchProjectViewTable={fetchProjectViewTable}
              getCachedProjectViewTable={getCachedProjectViewTable}
              linkedWorkItem={linkedWorkItem}
              onPickWorkItem={onPickWorkItem}
              createIssue={{
                canCreate: canCreateGithubIssue,
                title: createIssueTitle,
                onTitleChange: onCreateIssueTitleChange,
                body: createIssueBody,
                onBodyChange: onCreateIssueBodyChange,
                onApplyDraft: onApplyCreateIssueDraft,
                labels: createIssueLabels,
                labelOptions: createIssueLabelOptions,
                onToggleLabel: onToggleCreateIssueLabel,
                generating: createIssueGenerating,
                onGenerate: onGenerateIssueBody,
                submitting: createIssueSubmitting,
                error: createIssueError,
                onSubmit: onCreateIssueSubmit
              }}
              linear={{ settings, onBind: onSmartLinearIssueSelect }}
              workSource={workSource}
              onWorkSourceChange={handleWorkSourceChange}
              executionMode={executionMode}
              onExecutionModeChange={setExecutionMode}
              eligibility={eligibility}
              progress={launchProgress}
              requiresExplicitSetupChoice={requiresExplicitSetupChoice}
              setupDecision={setupDecision}
              onSetupDecisionChange={onSetupDecisionChange}
              locked={Boolean(launchCheckpoint.linkedWorkItem)}
              canStageNewIssue={canStageConfiguredIssue}
              onRegisterIssueCreator={(creator) => {
                configuredIssueCreatorRef.current = creator
              }}
              worktreeLocked={Boolean(launchCheckpoint.worktreeResult)}
              repoIssuePicker={{
                onOpenChange: onLinkPopoverOpenChange,
                query: linkQuery,
                onQueryChange: onLinkQueryChange,
                items: filteredLinkItems,
                loading: linkItemsLoading || linkDirectLoading,
                onSelect: onSelectLinkedItem
              }}
              />
            </fieldset>
          ) : null}
        </div>

        {/* Footer */}
        <div
          className="flex flex-none flex-col-reverse gap-2.5 border-t border-border bg-muted/40 px-3 py-3 sm:flex-row sm:items-center sm:px-[18px]"
          aria-busy={step === 3 && launchBusy}
        >
          {step > 1 && !launchCheckpoint.linkedWorkItem ? (
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

      {/* The worktree/workspace NAME deliberately lives in step 3, after the
          tracker section — linking or creating the issue first lets the name
          derive from the issue title instead of being fixed before it exists. */}
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
            The worktree is named in the next step — from the issue you link or create.
          </span>
        </div>
      ) : null}
    </div>
  )
}

/** Canonical repo-keyed tracker control. The provider choice, source cards,
 * provider-specific issue list, and new-issue form are one visual/runtime
 * unit, all driven by `/api/repos/{repoId}/tracker-config`. */
function CanonicalTrackerSection({
  config,
  configLoadStatus,
  configError,
  onRetryConfig,
  fetchProjectViewTable,
  getCachedProjectViewTable,
  linkedWorkItem,
  repo,
  onPickWorkItem,
  createIssue,
  linear,
  source,
  onSourceChange,
  locked,
  canStageNewIssue,
  showLinkedSelection,
  repoIssuePicker,
  onRegisterIssueCreator
}: {
  config: ProjectTrackerConfig | null | undefined
  configLoadStatus: 'idle' | 'loading' | 'loaded' | 'error'
  configError?: string
  onRetryConfig: () => void
  fetchProjectViewTable: (
    args: GetProjectViewTableArgs,
    options?: { force?: boolean }
  ) => Promise<GetProjectViewTableResult>
  getCachedProjectViewTable: (args: GetProjectViewTableArgs) => GitHubProjectTable | null
  linkedWorkItem: LinkedWorkItemSummary | null
  repo?: Repo
  onPickWorkItem: (option: WorkItemOption) => void
  createIssue: CreateIssueSeams
  linear: LinearCreateSeams
  source: WorkSource
  onSourceChange: (source: WorkSource) => void
  locked: boolean
  canStageNewIssue: boolean
  showLinkedSelection: boolean
  repoIssuePicker: RepoIssuePickerSeams
  onRegisterIssueCreator: (creator: () => Promise<LinkedWorkItemSummary | null>) => void
}): React.JSX.Element {
  const provider = config?.provider ?? null
  const configured = configLoadStatus === 'loaded' && provider !== null
  const githubTarget = provider === 'github' ? config?.github : undefined
  const githubBinding = githubTarget?.projectBinding
  const resolvedProject = useMemo<PickerProjectRef | null>(
    () =>
      githubBinding?.projectOwner &&
      (githubBinding.projectOwnerType === 'user' ||
        githubBinding.projectOwnerType === 'organization') &&
      githubBinding.projectNumber != null
        ? {
            owner: githubBinding.projectOwner,
            ownerType: githubBinding.projectOwnerType,
            number: githubBinding.projectNumber
          }
        : null,
    [
      githubBinding?.projectNumber,
      githubBinding?.projectOwner,
      githubBinding?.projectOwnerType
    ]
  )
  const usesGithubRepositoryFallback = provider === 'github' && !resolvedProject

  const [projectTableState, setProjectTableState] = useState<{
    scopeKey: string
    table: GitHubProjectTable
  } | null>(null)
  const [projectStatus, setProjectStatus] = useState<
    'idle' | 'loading' | 'refreshing' | 'failed'
  >('idle')
  const [query, setQuery] = useState('')
  const projectScopeKey =
    resolvedProject && githubBinding
      ? JSON.stringify([
          config?.repoId,
          config?.revision,
          githubTarget?.repositorySlug,
          githubBinding.projectId
        ])
      : null
  const latestProjectScopeKeyRef = useRef(projectScopeKey)
  latestProjectScopeKeyRef.current = projectScopeKey
  const cachedProjectTable = useMemo(
    () =>
      resolvedProject
        ? getCachedProjectViewTable({
            owner: resolvedProject.owner,
            ownerType: resolvedProject.ownerType,
            projectNumber: resolvedProject.number
          })
        : null,
    [getCachedProjectViewTable, resolvedProject]
  )
  const projectTable = trackerSectionTableForScope(
    projectTableState,
    projectScopeKey,
    cachedProjectTable
  )

  const readProject = useCallback(
    (force = false): void => {
      if (!resolvedProject || !projectScopeKey) return
      const capturedKey = projectScopeKey
      const args = {
        owner: resolvedProject.owner,
        ownerType: resolvedProject.ownerType,
        projectNumber: resolvedProject.number
      }
      const cached = getCachedProjectViewTable(args)
      setProjectStatus(cached ? 'refreshing' : 'loading')
      if (cached) setProjectTableState({ scopeKey: capturedKey, table: cached })
      void fetchProjectViewTable(args, force || cached ? { force: true } : undefined)
        .then((result) => {
          if (!isCurrentTrackerSectionScope(capturedKey, latestProjectScopeKeyRef.current)) return
          if (result.ok) {
            setProjectTableState({ scopeKey: capturedKey, table: result.data })
            setProjectStatus('idle')
          } else {
            setProjectStatus('failed')
          }
        })
        .catch(() => {
          if (isCurrentTrackerSectionScope(capturedKey, latestProjectScopeKeyRef.current)) {
            setProjectStatus('failed')
          }
        })
    },
    [fetchProjectViewTable, getCachedProjectViewTable, projectScopeKey, resolvedProject]
  )

  useEffect(() => {
    setQuery('')
    if (source === 'existing' && resolvedProject) readProject()
    else if (!resolvedProject) {
      setProjectStatus('idle')
      setProjectTableState(null)
    }
  }, [projectScopeKey, readProject, resolvedProject, source])

  useEffect(() => {
    repoIssuePicker.onOpenChange(
      source === 'existing' && configured && usesGithubRepositoryFallback
    )
  }, [configured, repoIssuePicker.onOpenChange, source, usesGithubRepositoryFallback])

  const projectIssueView = useMemo(
    () => deriveTrackerIssueViewModel(projectTable, query, githubTarget?.repositorySlug),
    [githubTarget?.repositorySlug, projectTable, query]
  )
  const repoIssues = repoIssuePicker.items.filter((item) => item.type === 'issue')

  const linearTarget = provider === 'linear' ? config?.linear : undefined
  const [linearIssues, setLinearIssues] = useState<LinearIssue[]>([])
  const [linearStatusValue, setLinearStatusValue] = useState<
    'idle' | 'loading' | 'failed'
  >('idle')
  const [linearError, setLinearError] = useState<string | null>(null)
  const [linearRefresh, setLinearRefresh] = useState(0)
  useEffect(() => {
    if (source !== 'existing' || provider !== 'linear' || !linearTarget) {
      setLinearIssues([])
      setLinearStatusValue('idle')
      setLinearError(null)
      return
    }
    let cancelled = false
    setLinearStatusValue('loading')
    setLinearError(null)
    const request =
      linearTarget.scope?.kind === 'project'
        ? linearListProjectIssues(
            linear.settings,
            linearTarget.scope.id,
            100,
            linearTarget.workspaceId
          ).then((result) => result.items)
        : linearTarget.scope?.kind === 'view'
          ? linearListCustomViewIssues(
              linear.settings,
              linearTarget.scope.id,
              100,
              linearTarget.workspaceId
            ).then((result) => result.items)
          : linearListIssues(linear.settings, 'all', 100, linearTarget.workspaceId)
    void request
      .then((issues) => {
        if (cancelled) return
        setLinearIssues(
          issues.filter(
            (issue) =>
              issue.workspaceId === linearTarget.workspaceId &&
              (!linearTarget.teamId || issue.team.id === linearTarget.teamId) &&
              (linearTarget.scope?.kind !== 'project' ||
                (issue.project?.id === linearTarget.scope.id &&
                  issue.project.workspaceId === linearTarget.workspaceId))
          )
        )
        setLinearStatusValue('idle')
      })
      .catch((cause) => {
        if (cancelled) return
        setLinearError(cause instanceof Error ? cause.message : 'Could not load Linear issues.')
        setLinearStatusValue('failed')
      })
    return () => {
      cancelled = true
    }
  }, [linear.settings, linearRefresh, linearTarget, provider, source])

  const visibleLinearIssues = useMemo(() => {
    const needle = query.trim().toLowerCase()
    if (!needle) return linearIssues
    return linearIssues.filter(
      (issue) =>
        issue.title.toLowerCase().includes(needle) ||
        issue.identifier.toLowerCase().includes(needle)
    )
  }, [linearIssues, query])

  const openTrackerSettings = (): void => {
    if (!repo) return
    const store = useAppStore.getState()
    store.openSettingsTarget({
      pane: 'repo',
      repoId: repo.id,
      sectionId: 'project-integrations'
    })
    store.openSettingsPage()
    // Settings replaces the wizard as the active surface. Leaving the modal
    // active would keep this blocking dialog above the requested repo pane.
    store.closeModal()
  }

  const targetLabel =
    provider === 'github'
      ? githubBinding?.projectTitle ?? githubTarget?.repositorySlug ?? 'GitHub'
      : provider === 'linear'
        ? linearTarget?.scope
          ? `Linear ${linearTarget.scope.kind}`
          : 'Linear workspace'
        : 'Not configured'
  const selectedUrl = linkedWorkItem?.url ?? null

  return (
    <section className="overflow-hidden rounded-lg border border-border bg-card/30">
      <div className="flex flex-col gap-2 border-b border-border px-3 py-2.5 sm:flex-row sm:items-center">
        <div className="min-w-0 flex-1">
          <p className="font-mono text-[10px] uppercase tracking-[0.14em] text-muted-foreground">
            Tracker
          </p>
          <p className="truncate text-[12px] font-medium text-foreground">{targetLabel}</p>
        </div>
        {repo ? (
          <button
            type="button"
            onClick={openTrackerSettings}
            className="inline-flex items-center justify-center gap-1.5 rounded-md border border-border px-2.5 py-1.5 text-[11px] text-muted-foreground transition-colors hover:border-muted-foreground/40 hover:text-foreground"
          >
            <KanbanSquare className="size-3" />
            Configure
          </button>
        ) : null}
      </div>

      <div className="space-y-3 p-3">
        <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
          {(['new', 'existing', 'none'] as const).map((option) => {
            const disabled = !canSelectWorkSource({
              source: option,
              trackerConfigured: configured,
              canStageNewIssue,
              locked
            })
            return (
              <button
                key={option}
                type="button"
                disabled={disabled}
                onClick={() => onSourceChange(option)}
                className={cn(
                  'rounded-lg border px-3 py-2 text-left text-[12px] transition-colors',
                  source === option
                    ? 'border-primary/55 bg-primary/8 text-foreground'
                    : 'border-border text-muted-foreground hover:border-muted-foreground/30',
                  disabled && 'cursor-not-allowed opacity-45'
                )}
              >
                <span className="block font-medium">
                  {option === 'new'
                    ? 'New issue'
                    : option === 'existing'
                      ? 'Existing issue'
                      : 'No issue'}
                </span>
                <span className="block text-[10.5px] text-muted-foreground">
                  {option === 'new'
                    ? 'File with configured provider'
                    : option === 'existing'
                      ? 'Choose from configured provider'
                      : 'Start untracked work'}
                </span>
              </button>
            )
          })}
        </div>

        {!configured && source !== 'none' ? (
          <div className="flex flex-col gap-2 rounded-md border border-dashed border-border px-3 py-2.5 text-[11.5px] text-muted-foreground sm:flex-row sm:items-center">
            <span className="min-w-0 flex-1">
              {configLoadStatus === 'error'
                ? configError ?? 'Could not load this project\'s tracker.'
                : configLoadStatus === 'loading' || configLoadStatus === 'idle'
                  ? 'Loading this project\'s tracker configuration…'
                  : 'Configure one tracker to create or choose an issue. No issue remains available.'}
            </span>
            {configLoadStatus === 'error' ? (
              <button
                type="button"
                onClick={onRetryConfig}
                className="rounded-md border border-border px-2 py-1 text-[10.5px] text-foreground"
              >
                Retry
              </button>
            ) : null}
          </div>
        ) : null}

        {configured && source === 'new' && repo && provider ? (
          <CanonicalCreateIssuePanel
            createIssue={createIssue}
            linear={linear}
            provider={provider}
            linearTarget={linearTarget}
            repo={repo}
            onRegisterIssueCreator={onRegisterIssueCreator}
          />
        ) : null}

        {configured && source === 'existing' && provider === 'github' && resolvedProject ? (
          <div className="overflow-hidden rounded-lg border border-border bg-background/35">
            <div className="flex items-center gap-2 border-b border-border px-2.5 py-2">
              <Search className="size-3.5 flex-none text-muted-foreground" />
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                aria-label="Filter GitHub Project issues"
                placeholder="Filter by title or #number"
                className="min-w-0 flex-1 bg-transparent text-[12px] outline-none placeholder:text-muted-foreground/70"
              />
              <span className="font-mono text-[10.5px] text-muted-foreground">
                {projectIssueView.issueCount}
              </span>
              <button
                type="button"
                onClick={() => readProject(true)}
                disabled={projectStatus === 'loading' || projectStatus === 'refreshing'}
                aria-label="Refresh GitHub Project issues"
                className="inline-flex size-6 items-center justify-center rounded-md text-muted-foreground hover:bg-secondary disabled:opacity-50"
              >
                <RefreshCw
                  className={cn(
                    'size-3.5',
                    (projectStatus === 'loading' || projectStatus === 'refreshing') &&
                      'animate-spin'
                  )}
                />
              </button>
            </div>
            <div className="max-h-52 overflow-y-auto p-1">
              {projectIssueView.groups.length ? (
                projectIssueView.groups.map((group) => (
                  <div key={group.key} className="mb-1 last:mb-0">
                    {group.label ? (
                      <div className="sticky top-0 z-[1] flex items-center gap-2 bg-card/95 px-2.5 py-1.5 backdrop-blur-sm">
                        <span
                          className="size-2 rounded-full"
                          style={{ backgroundColor: trackerStatusColor(group.color) }}
                        />
                        <span className="text-[10.5px] font-semibold uppercase tracking-[0.08em] text-muted-foreground">
                          {group.label}
                        </span>
                      </div>
                    ) : null}
                    {group.options.map((option) => (
                      <button
                        key={option.itemId}
                        type="button"
                        aria-pressed={selectedUrl === option.url}
                        onClick={() => onPickWorkItem(option)}
                        className={cn(
                          'flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[12px] transition-colors',
                          selectedUrl === option.url
                            ? 'bg-secondary text-foreground'
                            : 'text-muted-foreground hover:bg-secondary/60 hover:text-foreground'
                        )}
                      >
                        <Check
                          className={cn(
                            'size-3 flex-none',
                            selectedUrl === option.url ? 'opacity-70' : 'opacity-0'
                          )}
                        />
                        <span className="font-mono text-[11px]">#{option.number}</span>
                        <span className="truncate">{option.title}</span>
                      </button>
                    ))}
                  </div>
                ))
              ) : (
                <p className="px-3 py-4 text-center text-[11.5px] text-muted-foreground">
                  {projectStatus === 'failed'
                    ? 'GitHub Project issues could not load. Use refresh to retry.'
                    : projectStatus === 'loading'
                      ? 'Loading GitHub Project issues…'
                      : 'No matching open issues in this Project.'}
                </p>
              )}
            </div>
          </div>
        ) : null}

        {configured && source === 'existing' && usesGithubRepositoryFallback ? (
          <div className="overflow-hidden rounded-lg border border-border bg-background/35">
            <div className="flex items-center gap-2 border-b border-border px-2.5 py-2">
              <Search className="size-3.5 flex-none text-muted-foreground" />
              <input
                value={repoIssuePicker.query}
                onChange={(event) => repoIssuePicker.onQueryChange(event.target.value)}
                aria-label="Search repository issues"
                placeholder="Search issues or paste #number / URL"
                className="min-w-0 flex-1 bg-transparent text-[12px] outline-none placeholder:text-muted-foreground/70"
              />
              {repoIssuePicker.loading ? (
                <Loader2 className="size-3.5 animate-spin text-muted-foreground" />
              ) : null}
            </div>
            <div className="max-h-52 overflow-y-auto p-1">
              {repoIssues.length ? (
                repoIssues.map((item) => (
                  <button
                    key={item.id ?? item.url}
                    type="button"
                    aria-pressed={selectedUrl === item.url}
                    onClick={() => {
                      repoIssuePicker.onSelect(item)
                      repoIssuePicker.onOpenChange(true)
                    }}
                    className={cn(
                      'flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[12px] transition-colors',
                      selectedUrl === item.url
                        ? 'bg-secondary text-foreground'
                        : 'text-muted-foreground hover:bg-secondary/60 hover:text-foreground'
                    )}
                  >
                    <Check
                      className={cn(
                        'size-3 flex-none',
                        selectedUrl === item.url ? 'opacity-70' : 'opacity-0'
                      )}
                    />
                    <span className="font-mono text-[11px]">#{item.number}</span>
                    <span className="truncate">{item.title}</span>
                  </button>
                ))
              ) : (
                <p className="px-3 py-4 text-center text-[11.5px] text-muted-foreground">
                  {repoIssuePicker.loading
                    ? 'Loading repository issues…'
                    : 'No matching open repository issues.'}
                </p>
              )}
            </div>
            <div className="border-t border-border px-2.5 py-1.5 font-mono text-[10.5px] text-muted-foreground">
              {githubTarget?.repositorySlug} · repository fallback
            </div>
          </div>
        ) : null}

        {configured && source === 'existing' && provider === 'linear' ? (
          <div className="overflow-hidden rounded-lg border border-border bg-background/35">
            <div className="flex items-center gap-2 border-b border-border px-2.5 py-2">
              <Search className="size-3.5 flex-none text-muted-foreground" />
              <input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                aria-label="Filter Linear issues"
                placeholder="Filter by title or identifier"
                className="min-w-0 flex-1 bg-transparent text-[12px] outline-none placeholder:text-muted-foreground/70"
              />
              <span className="font-mono text-[10.5px] text-muted-foreground">
                {visibleLinearIssues.length}
              </span>
              <button
                type="button"
                onClick={() => setLinearRefresh((value) => value + 1)}
                disabled={linearStatusValue === 'loading'}
                aria-label="Refresh Linear issues"
                className="inline-flex size-6 items-center justify-center rounded-md text-muted-foreground hover:bg-secondary disabled:opacity-50"
              >
                <RefreshCw
                  className={cn(
                    'size-3.5',
                    linearStatusValue === 'loading' && 'animate-spin'
                  )}
                />
              </button>
            </div>
            <div className="max-h-52 overflow-y-auto p-1">
              {visibleLinearIssues.length ? (
                visibleLinearIssues.map((issue) => (
                  <button
                    key={issue.id}
                    type="button"
                    aria-pressed={selectedUrl === issue.url}
                    onClick={() => linear.onBind(issue)}
                    className={cn(
                      'flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[12px] transition-colors',
                      selectedUrl === issue.url
                        ? 'bg-secondary text-foreground'
                        : 'text-muted-foreground hover:bg-secondary/60 hover:text-foreground'
                    )}
                  >
                    <Check
                      className={cn(
                        'size-3 flex-none',
                        selectedUrl === issue.url ? 'opacity-70' : 'opacity-0'
                      )}
                    />
                    <span className="font-mono text-[11px]">{issue.identifier}</span>
                    <span className="truncate">{issue.title}</span>
                  </button>
                ))
              ) : (
                <p className="px-3 py-4 text-center text-[11.5px] text-muted-foreground">
                  {linearStatusValue === 'loading'
                    ? 'Loading Linear issues…'
                    : linearError ?? 'No matching Linear issues.'}
                </p>
              )}
            </div>
          </div>
        ) : null}

        {source === 'none' ? (
          <p className="rounded-md border border-dashed border-border px-3 py-2 text-[11.5px] text-muted-foreground">
            This workspace will start without creating or linking an external issue.
          </p>
        ) : null}

        {linkedWorkItem && showLinkedSelection ? (
          <div className="flex items-start gap-2 rounded-lg border border-emerald-500/35 bg-emerald-500/8 px-3 py-2">
            <Check className="mt-0.5 size-3.5 flex-none text-emerald-500" />
            <span className="min-w-0 text-[11.5px] text-foreground">
              <span className="font-semibold text-emerald-500">Selected</span>
              <span className="ml-1.5 font-mono text-muted-foreground">
                {linkedWorkItem.linearIdentifier ?? `#${linkedWorkItem.number}`}
              </span>{' '}
              {linkedWorkItem.title}
            </span>
          </div>
        ) : null}
      </div>
    </section>
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

// ---------- Step 3: Issue & agent ----------

/** The composer's create-issue seams, bundled for threading into the tracker
 *  section (spec 013 F2). Every field maps 1:1 onto a `useComposerState`
 *  `cardProps` entry — no new state, no rebuilt flow. */
type CreateIssueSeams = {
  /** True when "Create issue" applies: nothing linked yet + a git repo. */
  canCreate: boolean
  title: string
  onTitleChange: (value: string) => void
  body: string
  onBodyChange: (value: string) => void
  onApplyDraft: (draft: { title: string; body: string }) => void
  labels: string[]
  labelOptions: string[] | null
  onToggleLabel: (label: string) => void
  generating: boolean
  onGenerate: (choice?: DraftLlmChoice, style?: DraftIssueStyle) => void
  submitting: boolean
  error: string | null
  onSubmit: () => Promise<LinkedWorkItemSummary | null>
}

/** Spec 013 F3: the seams the Linear create arm needs — the runtime settings
 *  (routes the RPC to local or the active remote) and the bind callback (the
 *  same `onSmartLinearIssueSelect` the Linear @-picker uses). */
type LinearCreateSeams = {
  settings: RuntimeLinearSettings
  onBind: (issue: LinearIssue) => void
}

type RepoIssuePickerSeams = {
  onOpenChange: (open: boolean) => void
  query: string
  onQueryChange: (value: string) => void
  items: GitHubWorkItem[]
  loading: boolean
  onSelect: (item: GitHubWorkItem) => void
}

/** Spec 013 F4: the composer's gated-run seams, migrated into the wizard. Maps
 *  1:1 onto `cardProps` — no new state or submit path. */
function AgentStep({
  agents,
  detectedAgentIds,
  quickAgent,
  onPick,
  selectedRepoIsGit,
  repo,
  repoDisplayName,
  name,
  onNameValueChange,
  nameInputRef,
  trackerConfig,
  trackerConfigLoadStatus,
  trackerConfigError,
  onRetryTrackerConfig,
  fetchProjectViewTable,
  getCachedProjectViewTable,
  linkedWorkItem,
  onPickWorkItem,
  createIssue,
  linear,
  workSource,
  onWorkSourceChange,
  executionMode,
  onExecutionModeChange,
  eligibility,
  progress,
  requiresExplicitSetupChoice,
  setupDecision,
  onSetupDecisionChange,
  locked,
  canStageNewIssue,
  onRegisterIssueCreator,
  worktreeLocked,
  repoIssuePicker
}: {
  agents: TuiAgent[]
  detectedAgentIds: Set<TuiAgent> | null
  quickAgent: TuiAgent | null
  onPick: (agent: TuiAgent) => void
  selectedRepoIsGit: boolean
  repo?: Repo
  repoDisplayName?: string
  name: string
  onNameValueChange: (value: string) => void
  nameInputRef: React.RefObject<HTMLInputElement | null>
  trackerConfig: ProjectTrackerConfig | null | undefined
  trackerConfigLoadStatus: 'idle' | 'loading' | 'loaded' | 'error'
  trackerConfigError?: string
  onRetryTrackerConfig: () => void
  fetchProjectViewTable: (
    args: GetProjectViewTableArgs,
    options?: { force?: boolean }
  ) => Promise<GetProjectViewTableResult>
  getCachedProjectViewTable: (args: GetProjectViewTableArgs) => GitHubProjectTable | null
  linkedWorkItem: LinkedWorkItemSummary | null
  onPickWorkItem: (option: WorkItemOption) => void
  createIssue: CreateIssueSeams
  linear: LinearCreateSeams
  workSource: WorkSource
  onWorkSourceChange: (source: WorkSource) => void
  executionMode: ExecutionMode
  onExecutionModeChange: (mode: ExecutionMode) => void
  eligibility: NewWorkEligibility
  progress: NewWorkProgress
  requiresExplicitSetupChoice: boolean
  setupDecision: 'run' | 'skip' | null
  onSetupDecisionChange: (value: 'run' | 'skip') => void
  locked: boolean
  canStageNewIssue: boolean
  onRegisterIssueCreator: (creator: () => Promise<LinkedWorkItemSummary | null>) => void
  worktreeLocked: boolean
  repoIssuePicker: RepoIssuePickerSeams
}): React.JSX.Element {
  return (
    <div className="flex animate-in flex-col gap-[18px] fade-in-0 slide-in-from-bottom-1">
      <div className="flex flex-col gap-0.5">
        <span className="text-[15px] font-semibold tracking-[-0.01em] text-foreground">
          What&apos;s the work — and who drives it?
        </span>
        <span className="text-[12px] text-muted-foreground">
          Link or create the issue first — the {selectedRepoIsGit ? 'worktree' : 'workspace'} is
          named after it. Then pick the agent.
        </span>
      </div>

      {/* The single tracker control sits immediately below the step heading.
          Source selection and the provider-specific list/form live inside it,
          so there is no second repository picker that can disagree. */}
      <CanonicalTrackerSection
        config={trackerConfig}
        configLoadStatus={trackerConfigLoadStatus}
        configError={trackerConfigError}
        onRetryConfig={onRetryTrackerConfig}
        fetchProjectViewTable={fetchProjectViewTable}
        getCachedProjectViewTable={getCachedProjectViewTable}
        linkedWorkItem={linkedWorkItem}
        repo={repo}
        onPickWorkItem={onPickWorkItem}
        createIssue={createIssue}
        linear={linear}
        source={workSource}
        onSourceChange={onWorkSourceChange}
        locked={locked}
        canStageNewIssue={canStageNewIssue}
        showLinkedSelection={workSource === 'existing' || locked}
        repoIssuePicker={repoIssuePicker}
        onRegisterIssueCreator={onRegisterIssueCreator}
      />

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

      <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
        <button type="button" disabled={!eligibility.eligible || progress.worktree === 'done'} onClick={() => onExecutionModeChange('autopilot')}
          className={cn('rounded-lg border px-3 py-2 text-left', executionMode === 'autopilot' ? 'border-primary/55 bg-primary/8' : 'border-border', !eligibility.eligible && 'opacity-50')}>
          <span className="block text-[12px] font-medium">SDD Autopilot</span>
          <span className="block text-[10.5px] text-muted-foreground">PM → Architect → Build → Verify → Review</span>
        </button>
        <button type="button" disabled={progress.worktree === 'done'} onClick={() => onExecutionModeChange('manual')}
          className={cn('rounded-lg border px-3 py-2 text-left', executionMode === 'manual' ? 'border-primary/55 bg-primary/8' : 'border-border')}>
          <span className="block text-[12px] font-medium">Open manually</span>
          <span className="block text-[10.5px] text-muted-foreground">
            {eligibility.eligible
              ? 'Prepare the spec, then open one agent'
              : 'Open one agent · no generated SDD spec'}
          </span>
        </button>
      </div>
      {!eligibility.eligible ? <span className="text-[11px] text-amber-500">{eligibility.message}</span> : null}
      {Object.values(progress).some((status) => status !== 'pending') ? (
        <div className="grid grid-cols-4 gap-1.5 rounded-lg border border-border p-2">
          {NEW_WORK_STAGES.map((stage) => <span key={stage} className={cn('text-center text-[10px] capitalize text-muted-foreground', progress[stage] === 'done' && 'text-emerald-500', progress[stage] === 'error' && 'text-destructive')}>{progress[stage] === 'done' ? '✓ ' : progress[stage] === 'active' ? '● ' : progress[stage] === 'error' ? '! ' : ''}{stage}</span>)}
        </div>
      ) : null}
    </div>
  )
}

/** Deferred issue form for the canonical provider. The final wizard CTA owns
 * filing, so this component registers exactly one provider-specific creator
 * instead of rendering a second provider toggle or submit path. */
function CanonicalCreateIssuePanel({
  createIssue,
  linear,
  provider,
  linearTarget,
  repo,
  onRegisterIssueCreator
}: {
  createIssue: CreateIssueSeams
  linear: LinearCreateSeams
  provider: ProjectTrackerProvider
  linearTarget?: ProjectTrackerLinearTarget
  repo: Repo
  onRegisterIssueCreator: (creator: () => Promise<LinkedWorkItemSummary | null>) => void
}): React.JSX.Element {
  const savedChatAgent = useAppStore((state) => state.settings?.chatAgent)
  const updateSettings = useAppStore((state) => state.updateSettings)
  const { detectedIds: detectedChatAgentIds } = useDetectedAgents()
  const preferredDraftAgent = pickChatAgent(savedChatAgent, detectedChatAgentIds)
  const [draftAgent, setDraftAgent] = useState<ChatAgentId>(preferredDraftAgent)
  const [draftModel, setDraftModel] = useState(
    () => resolveChatModel(readChatModelPreference()).id
  )
  const [teams, setTeams] = useState<LinearTeam[]>([])
  const [teamId, setTeamId] = useState<string | null>(linearTarget?.teamId ?? null)
  const [linearFiling, setLinearFiling] = useState(false)
  const [linearError, setLinearError] = useState<string | null>(null)
  const [specOpen, setSpecOpen] = useState(false)
  const [specResetVersion, setSpecResetVersion] = useState(0)
  const draftPreferenceTouched = useRef(false)

  useEffect(() => {
    if (!draftPreferenceTouched.current) setDraftAgent(preferredDraftAgent)
  }, [preferredDraftAgent])

  const availableDraftAgents = useMemo(
    () =>
      detectedChatAgentIds === null
        ? CHAT_AGENTS
        : CHAT_AGENTS.filter((agent) => detectedChatAgentIds.includes(agent.id)),
    [detectedChatAgentIds]
  )
  const effectiveDraftAgent =
    availableDraftAgents.find((agent) => agent.id === draftAgent)?.id ??
    availableDraftAgents[0]?.id ??
    draftAgent

  useEffect(() => {
    setTeams([])
    setTeamId(linearTarget?.teamId ?? null)
    setLinearError(null)
    if (provider !== 'linear' || !linearTarget) return
    let cancelled = false
    void linearListTeams(linear.settings, linearTarget.workspaceId)
      .then((items) => {
        if (cancelled) return
        const exact = items.filter(
          (team) => !team.workspaceId || team.workspaceId === linearTarget.workspaceId
        )
        setTeams(exact)
        if (!linearTarget.teamId && exact.length === 1) setTeamId(exact[0].id)
      })
      .catch((cause) => {
        if (!cancelled) {
          setLinearError(cause instanceof Error ? cause.message : 'Could not load Linear teams.')
        }
      })
    return () => {
      cancelled = true
    }
  }, [linear.settings, linearTarget, provider])

  const busy = createIssue.generating || createIssue.submitting || linearFiling
  const phase = deriveCreateIssueIntentPhase({
    generating: createIssue.generating,
    submitting: createIssue.submitting || linearFiling,
    error: provider === 'linear' ? linearError : createIssue.error,
    hasBody: createIssue.body.trim().length > 0
  })

  const createLinear = useCallback(async (): Promise<LinkedWorkItemSummary | null> => {
    const title = createIssue.title.trim()
    if (!linearTarget) {
      setLinearError('The configured Linear target is incomplete.')
      return null
    }
    if (!teamId) {
      setLinearError('Pick a Linear team to file into.')
      return null
    }
    if (!canFileIssue(title, busy)) return null
    const team = teams.find((candidate) => candidate.id === teamId)
    setLinearFiling(true)
    setLinearError(null)
    try {
      const result = await linearCreateIssue(linear.settings, {
        teamId,
        title,
        description: createIssue.body.trim() || undefined,
        workspaceId: linearTarget.workspaceId,
        projectId:
          linearTarget.scope?.kind === 'project' ? linearTarget.scope.id : undefined
      })
      if (!result.ok) {
        setLinearError(result.error || 'Could not create the Linear issue.')
        return null
      }
      if (result.teamId !== teamId) {
        setLinearError('Linear returned an issue outside the configured team.')
        return null
      }
      if (
        linearTarget.scope?.kind === 'project' &&
        result.projectId !== linearTarget.scope.id
      ) {
        setLinearError('Linear returned an issue outside the configured project.')
        return null
      }
      const issue: LinearIssue = {
        id: result.id,
        workspaceId: linearTarget.workspaceId,
        identifier: result.identifier,
        title: result.title,
        description: createIssue.body.trim() || undefined,
        url: result.url,
        state: { name: 'Todo', type: 'unstarted', color: '#9ca3af' },
        team: team
          ? { id: team.id, name: team.name, key: team.key }
          : { id: teamId, name: teamId, key: teamId },
        ...(linearTarget.scope?.kind === 'project'
          ? {
              project: {
                id: linearTarget.scope.id,
                workspaceId: linearTarget.workspaceId,
                name: linearTarget.scope.id
              }
            }
          : {}),
        labels: [],
        labelIds: [],
        priority: 0,
        updatedAt: new Date().toISOString()
      }
      linear.onBind(issue)
      return buildLinearIssueLinkedWorkItem(issue)
    } catch (cause) {
      setLinearError(cause instanceof Error ? cause.message : 'Could not create the Linear issue.')
      return null
    } finally {
      setLinearFiling(false)
    }
  }, [busy, createIssue.body, createIssue.title, linear, linearTarget, teamId, teams])

  useEffect(() => {
    onRegisterIssueCreator(provider === 'linear' ? createLinear : createIssue.onSubmit)
  }, [createIssue.onSubmit, createLinear, onRegisterIssueCreator, provider])

  const handleDraft = useCallback(() => {
    if (!canFileIssue(createIssue.title, busy)) return
    setLinearError(null)
    const choice: DraftLlmChoice = {
      agent: effectiveDraftAgent,
      ...(effectiveDraftAgent === 'claude' ? { model: draftModel } : {})
    }
    createIssue.onGenerate(choice, 'concise')
  }, [busy, createIssue, draftModel, effectiveDraftAgent])

  const handleOpenSpec = useCallback(() => {
    if (!canFileIssue(createIssue.title, busy)) return
    setSpecResetVersion((value) => value + 1)
    setSpecOpen(true)
  }, [busy, createIssue.title])

  const inlineError = provider === 'linear' ? linearError : createIssue.error
  const specSeed = [createIssue.title.trim(), createIssue.body.trim()].filter(Boolean).join('\n\n')

  return (
    <>
      <div
        className="flex flex-col gap-2.5 rounded-lg border border-border bg-background/35 p-3"
        onKeyDown={(event) => {
          if (event.key === 'Enter' && !event.shiftKey) event.stopPropagation()
        }}
      >
        <div className="flex flex-wrap items-center justify-between gap-2">
          <span className="text-[12px] font-medium text-foreground">
            New {provider === 'linear' ? 'Linear' : 'GitHub'} issue
          </span>
          <span className="rounded-full border border-border px-2 py-0.5 font-mono text-[9.5px] uppercase text-muted-foreground">
            configured provider
          </span>
        </div>

        <label className="flex flex-col gap-1.5">
          <span className="text-[11px] text-muted-foreground">Title</span>
          <input
            value={createIssue.title}
            onChange={(event) => createIssue.onTitleChange(event.target.value)}
            placeholder="What needs doing?"
            className="h-[34px] rounded-md border border-input bg-secondary px-2.5 text-[12.5px] outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
          />
        </label>

        <div className="flex flex-col gap-1.5">
          <div className="flex flex-col gap-2 sm:flex-row sm:items-end sm:justify-between">
            <span className="text-[11px] text-muted-foreground">Description (optional)</span>
            <div className="flex flex-wrap items-end gap-1.5">
              <select
                aria-label="Drafting engine"
                value={effectiveDraftAgent}
                disabled={availableDraftAgents.length === 0 || busy}
                onChange={(event) => {
                  const agent = event.target.value as ChatAgentId
                  draftPreferenceTouched.current = true
                  setDraftAgent(agent)
                  void updateSettings({ chatAgent: agent })
                }}
                className="h-6 rounded-md border border-border bg-secondary px-1.5 text-[10.5px] outline-none"
              >
                {availableDraftAgents.map((agent) => (
                  <option key={agent.id} value={agent.id}>
                    {agent.label}
                  </option>
                ))}
              </select>
              {effectiveDraftAgent === 'claude' ? (
                <select
                  aria-label="Drafting model"
                  value={draftModel}
                  disabled={busy}
                  onChange={(event) => {
                    setDraftModel(event.target.value)
                    writeChatModelPreference(event.target.value)
                  }}
                  className="h-6 max-w-44 rounded-md border border-border bg-secondary px-1.5 text-[10.5px] outline-none"
                >
                  {CHAT_MODELS.map((model) => (
                    <option key={model.id} value={model.id}>
                      {model.label}
                    </option>
                  ))}
                </select>
              ) : null}
              <div className="flex">
                <button
                  type="button"
                  onClick={handleDraft}
                  disabled={!canFileIssue(createIssue.title, busy) || availableDraftAgents.length === 0}
                  className="inline-flex h-6 items-center gap-1 rounded-l-md border border-border px-2 text-[11px] text-muted-foreground hover:text-foreground disabled:opacity-50"
                >
                  {phase === 'drafting' ? (
                    <Loader2 className="size-3 animate-spin" />
                  ) : (
                    <Sparkles className="size-3" />
                  )}
                  {phase === 'drafting' ? 'Drafting…' : 'Draft simple issue'}
                </button>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <button
                      type="button"
                      disabled={!canFileIssue(createIssue.title, busy)}
                      aria-label="More drafting options"
                      className="-ml-px inline-flex h-6 w-6 items-center justify-center rounded-r-md border border-border text-muted-foreground hover:text-foreground disabled:opacity-50"
                    >
                      <ChevronDown className="size-3" />
                    </button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end" className="w-60">
                    <DropdownMenuItem onSelect={handleDraft}>
                      <Sparkles className="size-3.5" />
                      <span>Draft simple issue</span>
                    </DropdownMenuItem>
                    <DropdownMenuItem onSelect={handleOpenSpec}>
                      <WandSparkles className="size-3.5" />
                      <span>Shape into spec…</span>
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            </div>
          </div>
          <textarea
            value={createIssue.body}
            onChange={(event) => createIssue.onBodyChange(event.target.value)}
            rows={createIssue.body.trim() ? 6 : 3}
            placeholder="Add details or draft an SDD-shaped description."
            className="resize-none rounded-md border border-input bg-secondary px-2.5 py-2 font-mono text-[11.5px] leading-relaxed outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
          />
        </div>

        {provider === 'github' && createIssue.labelOptions?.length ? (
          <div className="flex flex-wrap gap-1.5">
            {createIssue.labelOptions.map((label) => {
              const selected = createIssue.labels.includes(label)
              return (
                <button
                  key={label}
                  type="button"
                  aria-pressed={selected}
                  onClick={() => createIssue.onToggleLabel(label)}
                  className={cn(
                    'rounded-full border px-2 py-0.5 text-[10.5px]',
                    selected
                      ? 'border-primary/50 bg-primary/10 text-foreground'
                      : 'border-border text-muted-foreground'
                  )}
                >
                  {label}
                </button>
              )
            })}
          </div>
        ) : null}

        {provider === 'linear' && (teams.length > 1 || !teamId) ? (
          <label className="flex flex-col gap-1.5">
            <span className="text-[11px] text-muted-foreground">Linear team</span>
            <select
              value={teamId ?? ''}
              onChange={(event) => setTeamId(event.target.value || null)}
              className="h-[34px] rounded-md border border-input bg-secondary px-2 text-[12.5px] outline-none"
            >
              <option value="">Select a configured workspace team…</option>
              {teams.map((team) => (
                <option key={team.id} value={team.id}>
                  {team.name} ({team.key})
                </option>
              ))}
            </select>
          </label>
        ) : null}

        <p className="text-[10.5px] text-muted-foreground">
          The issue is filed once when you start work; a failed later stage retries from its
          checkpoint without filing a duplicate.
        </p>
        {inlineError ? <span className="text-[11px] text-destructive">{inlineError}</span> : null}
      </div>
      <IssueSpecInterviewDialog
        open={specOpen}
        onOpenChange={setSpecOpen}
        repo={repo}
        seedIntent={specSeed}
        resetVersion={specResetVersion}
        onApplyDraft={(draft) => {
          createIssue.onApplyDraft(draft)
          setSpecOpen(false)
        }}
      />
    </>
  )
}

const TRACKER_STATUS_COLORS: Record<string, string> = {
  GRAY: '#8b949e',
  RED: '#f85149',
  ORANGE: '#db6d28',
  YELLOW: '#d29922',
  GREEN: '#3fb950',
  BLUE: '#58a6ff',
  PURPLE: '#bc8cff',
  PINK: '#db61a2'
}

function trackerStatusColor(color: string | null): string {
  if (!color) return 'var(--muted-foreground)'
  const keyword = TRACKER_STATUS_COLORS[color.toUpperCase()]
  if (keyword) return keyword
  if (/^#?[0-9a-fA-F]{6}$/.test(color)) return color.startsWith('#') ? color : `#${color}`
  return 'var(--muted-foreground)'
}
