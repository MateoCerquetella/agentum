import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  ArrowLeft,
  ArrowRight,
  Check,
  ChevronsUpDown,
  FolderOpen,
  FolderPlus,
  GitBranch,
  KanbanSquare,
  Laptop,
  Loader2,
  PlugZap,
  Plus,
  RefreshCw,
  Search,
  Server,
  Sparkles,
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
  deriveTrackerBindingTarget,
  deriveUnifiedTrackerStatus,
  deriveWizardComposerSeed,
  filterRepoList,
  resolveWizardAgentOptions,
  wizardBaseBranchTriggerLabel,
  wizardPrimaryLabel,
  type CreateWorkspaceWizardData,
  type UnifiedTrackerStatus,
  type WizardStep
} from '@/components/new-workspace/create-workspace-wizard-model'
import {
  buildBindPayload,
  deriveTrackerIssueViewModel,
  pickerBindingTargetKey,
  pickerProjectKey,
  pickerScopeKey,
  resolvePickerProject,
  type PickerBindingResolution,
  type PickerProjectRef,
  type WorkItemOption
} from '@/components/new-workspace/work-item-picker-model'
import {
  isCurrentTrackerSectionScope,
  trackerConfigureActionLabel,
  trackerSectionAfterSuccessfulUnbind,
  trackerSectionTableForScope
} from '@/components/new-workspace/tracker-section-scope'
import {
  canFileIssue,
  deriveCreateIssueIntentPhase,
  resolveCreateIssueProvider,
  type CreateIssueProvider
} from '@/components/new-workspace/create-issue-intent-model'
import {
  linearCreateIssue,
  linearListTeams,
  linearStatus,
  type RuntimeLinearSettings
} from '@/runtime/runtime-linear-client'
import { ProjectBindingEditor } from '@/components/github-projects/ProjectBindingEditor'
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
import type { DraftLlmChoice } from '@/runtime/github-issue-client'
import {
  getProjectBinding,
  GithubProjectsBindingError
} from '@/runtime/github-projects-client'
import type {
  GetProjectViewTableArgs,
  GetProjectViewTableResult,
  GitHubProjectSettings,
  GitHubProjectTable
} from '@/shared/github-project-types'
import type {
  GitHubWorkItem,
  LinearIssue,
  LinearTeam,
  Repo,
  TuiAgent
} from '../../../../shared/types'
import {
  NEW_WORK_STAGES,
  canLaunchNewWork,
  deriveNewWorkEligibility,
  initialNewWorkProgress,
  newWorkPrimaryLabel,
  resolveLaunchIssue,
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
  // Spec 011 F2: the issue picker can use the globally-active Project only
  // when there is no selected git repo. A selected repo is a closed scope and
  // must resolve through its own binding.
  const activeProject = settings?.githubProjects?.activeProject ?? null

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
    // Spec 013 F4: the gated-run toggle — the SAME seams the composer card used,
    // so `submitQuick` inherits the `start_work` precondition set unchanged.
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
  const initialWorkSource: WorkSource = linkedWorkItem ? 'existing' : 'new'
  const [workSource, setWorkSource] = useState<WorkSource>(initialWorkSource)
  const [executionMode, setExecutionMode] = useState<ExecutionMode>('autopilot')
  const [launchCheckpoint, setLaunchCheckpoint] = useState<NewWorkCheckpoint>({})
  const [launchProgress, setLaunchProgress] = useState(() =>
    initialNewWorkProgress(linkedWorkItem ? { linkedWorkItem } : {}, initialWorkSource)
  )
  const [launchAttempted, setLaunchAttempted] = useState(false)
  const launchInFlightRef = useRef(false)
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
  const canStageNewGithubIssue = selectedRepoIsGit
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
    isLocal: !selectedRepo?.connectionId,
    isGit: selectedRepoIsGit,
    source: workSource,
    linkedWorkItem,
    selectedAgentInstalled: Boolean(quickAgent && (!detectedAgentIds || detectedAgentIds.has(quickAgent))),
    setupBlocked:
      selectedRepoRequiresConnection || (requiresExplicitSetupChoice && !setupDecision)
  })

  const handleWorkSourceChange = useCallback(
    (source: WorkSource): void => {
      if (launchCheckpoint.linkedWorkItem) return
      setWorkSource(source)
      setLaunchProgress(initialNewWorkProgress({}, source))
      setLaunchAttempted(false)
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
    if (
      step === 3 &&
      selectedRepo &&
      !launchCheckpoint.linkedWorkItem &&
      workSource === 'new' &&
      !canStageNewGithubIssue
    ) {
      handleWorkSourceChange(selectedRepoIsGit ? 'existing' : 'none')
    }
  }, [canStageNewGithubIssue, handleWorkSourceChange, launchCheckpoint.linkedWorkItem, selectedRepo, selectedRepoIsGit, step, workSource])

  const launchAllowed = canLaunchNewWork({
    source: workSource,
    executionMode,
    eligibility,
    hasSelectedAgent: Boolean(quickAgent),
    canStageNewIssue: canStageNewGithubIssue,
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
      try {
        setLaunchAttempted(true)
        let checkpoint = launchCheckpoint
        let issue = checkpoint.linkedWorkItem ?? null
        if (!issue && workSource !== 'none') {
          setLaunchProgress((current) => updateNewWorkProgress(current, 'issue', 'active'))
          const resolved = await resolveLaunchIssue({
            source: workSource,
            selectedIssue: linkedWorkItem,
            checkpoint,
            createIssue: onCreateIssueSubmit
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
      }
      return
    }
    if (step === 2 && !canLeaveRepoStep) {
      return
    }
    goNext()
  }, [canLeaveRepoStep, createIssueSubmitting, creating, executionMode, goNext, launchAllowed, launchCheckpoint, linkedWorkItem, onCreateIssueSubmit, quickAgent, step, submitQuick, workSource])

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

  const primaryLabel = step === 3
    ? creating ? 'Preparing work…' : newWorkPrimaryLabel(workSource, launchAttempted)
    : wizardPrimaryLabel(step, creating)
  const primaryDisabled =
    step === 3
      ? creating || createIssueSubmitting || !launchAllowed
      : step === 2 ? !canLeaveRepoStep : false

  // Spec 013 F1: the tracker section reads SOLELY from the Project the picker
  // resolves (per-repo binding ∨ global activeProject) — no git-remote heuristic
  // that could disagree with the picker's issue list. Any GIT repo carries a
  // binding target now (#356, shipped v0.75.1): SSH repos resolve their slug
  // on their own host via spec 020's `repoId` (#359's hostId param, migrated
  // to the repoId wire at the develop merge). Reading and configuration are
  // both host-aware. A non-git selection resolves nothing and may use the
  // legacy global activeProject.
  const trackerTarget = deriveTrackerBindingTarget({
    repo: selectedRepo,
    isGit: selectedRepoIsGit
  })

  return (
    <Dialog open onOpenChange={onOpenChange}>
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
              onClick={onClose}
              aria-label="Close"
              className="inline-flex size-6 flex-none items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
            >
              <X className="size-3.5" />
            </button>
          </div>
          <StepDots step={step} onJump={(target) => setStep(target)} />
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
              trackerWorkdir={trackerTarget?.workdir}
              trackerRepoId={trackerTarget?.repoId}
              trackerLocal={trackerTarget?.local}
              activeProject={activeProject}
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
              locked={Boolean(launchCheckpoint.linkedWorkItem)}
              canStageNewIssue={canStageNewGithubIssue}
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
          ) : null}
        </div>

        {/* Footer */}
        <div className="flex flex-none items-center gap-2.5 border-t border-border bg-muted/40 px-[18px] py-3">
          {step > 1 && !launchCheckpoint.linkedWorkItem ? (
            <button
              type="button"
              onClick={goBack}
              className="inline-flex items-center gap-1.5 rounded-md border border-border px-3 py-1.5 text-[12.5px] text-muted-foreground transition-colors hover:border-muted-foreground/40 hover:text-foreground"
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
            className="inline-flex items-center gap-2 rounded-full bg-primary px-[18px] py-2 text-[13px] font-medium text-primary-foreground transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {creating && step === 3 ? <Loader2 className="size-3.5 animate-spin" /> : null}
            {primaryLabel}
            {!creating || step !== 3 ? <ArrowRight className="size-3.5" aria-hidden /> : null}
          </button>
        </div>
      </DialogContent>
    </Dialog>
  )
}

/** Segmented step indicator — completed steps are clickable to jump back. */
function StepDots({
  step,
  onJump
}: {
  step: WizardStep
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
            disabled={!done}
            onClick={() => done && onJump(n)}
            className={cn(
              'inline-flex items-center gap-2',
              done ? 'cursor-pointer' : 'cursor-default'
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
  /** True when "Create issue" applies: nothing linked yet + a local git repo. */
  canCreate: boolean
  title: string
  onTitleChange: (value: string) => void
  body: string
  onBodyChange: (value: string) => void
  labels: string[]
  labelOptions: string[] | null
  onToggleLabel: (label: string) => void
  generating: boolean
  onGenerate: (choice?: DraftLlmChoice) => void
  submitting: boolean
  error: string | null
  onSubmit: () => void
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
  repoDisplayName,
  name,
  onNameValueChange,
  nameInputRef,
  trackerWorkdir,
  trackerRepoId,
  trackerLocal,
  activeProject,
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
  locked,
  canStageNewIssue,
  worktreeLocked,
  repoIssuePicker
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
  trackerWorkdir?: string
  /** Spec 020 F3: the repo's registry id — the binding resolves on the repo's
   *  own host (#356: SSH repos included). */
  trackerRepoId?: string
  /** False when the repo lives on an SSH host; the same repoId-aware binding
   *  read/write path supports both host kinds. */
  trackerLocal?: boolean
  activeProject: GitHubProjectSettings['activeProject']
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
  locked: boolean
  canStageNewIssue: boolean
  worktreeLocked: boolean
  repoIssuePicker: RepoIssuePickerSeams
}): React.JSX.Element {
  useEffect(() => {
    repoIssuePicker.onOpenChange(workSource === 'existing')
  }, [repoIssuePicker.onOpenChange, workSource])

  const repoIssues = repoIssuePicker.items.filter((item) => item.type === 'issue')

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

      <div className="grid grid-cols-3 gap-2">
        {(['new', 'existing', 'none'] as const).map((source) => (
          <button key={source} type="button" disabled={locked || (source === 'new' && !canStageNewIssue)} onClick={() => onWorkSourceChange(source)}
            className={cn('rounded-lg border px-3 py-2 text-left text-[12px]', workSource === source ? 'border-primary/55 bg-primary/8 text-foreground' : 'border-border text-muted-foreground', locked && 'opacity-60')}>
            <span className="block font-medium">
              {source === 'new' ? 'New issue' : source === 'existing' ? 'Existing issue' : 'No issue'}
            </span>
            <span className="block text-[10.5px] text-muted-foreground">
              {source === 'new'
                ? 'Filed when work starts'
                : source === 'existing'
                  ? 'Choose from this project'
                  : 'Start untracked work'}
            </span>
          </button>
        ))}
      </div>

      {workSource === 'existing' ? (
        <div className="overflow-hidden rounded-lg border border-border bg-card/30">
          <div className="flex items-center gap-2 border-b border-border px-2.5 py-2">
            <Search className="size-3.5 flex-none text-muted-foreground" />
            <input
              value={repoIssuePicker.query}
              onChange={(event) => repoIssuePicker.onQueryChange(event.target.value)}
              aria-label="Search repository issues"
              placeholder="Search issues or paste #number / URL"
              className="min-w-0 flex-1 bg-transparent text-[12px] text-foreground outline-none placeholder:text-muted-foreground/70"
            />
            {repoIssuePicker.loading ? (
              <Loader2 className="size-3.5 animate-spin text-muted-foreground" aria-label="Loading issues" />
            ) : null}
          </div>
          <div className="max-h-36 overflow-y-auto p-1">
            {repoIssues.length ? (
              repoIssues.map((item) => {
                const selected = linkedWorkItem?.url === item.url
                return (
                  <button
                    key={item.id ?? item.url}
                    type="button"
                    aria-pressed={selected}
                    onClick={() => {
                      repoIssuePicker.onSelect(item)
                      // The shared selector normally closes its popover after
                      // a pick. This inline picker stays mounted, so keep its
                      // host-aware search effects live for a changed selection.
                      repoIssuePicker.onOpenChange(true)
                    }}
                    className={cn(
                      'flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[12px] transition-colors',
                      selected
                        ? 'bg-secondary text-foreground'
                        : 'text-muted-foreground hover:bg-secondary/60 hover:text-foreground'
                    )}
                  >
                    <Check className={cn('size-3 flex-none', selected ? 'opacity-70' : 'opacity-0')} />
                    <span className="flex-none font-mono text-[11px]">#{item.number}</span>
                    <span className="truncate">{item.title}</span>
                  </button>
                )
              })
            ) : (
              <p className="px-3 py-4 text-center text-[11.5px] text-muted-foreground">
                {repoIssuePicker.loading ? 'Loading repository issues…' : 'No matching open issues.'}
              </p>
            )}
          </div>
        </div>
      ) : null}

      {/* Tracker FIRST: linking/creating the issue auto-fills the name field
          below (via applyLinkedWorkItem's title slug) while it's still blank. */}
      {workSource !== 'none' ? <TrackerSection
        workdir={trackerWorkdir}
        repoId={trackerRepoId}
        local={trackerLocal}
        activeProject={activeProject}
        fetchProjectViewTable={fetchProjectViewTable}
        getCachedProjectViewTable={getCachedProjectViewTable}
        linkedWorkItem={linkedWorkItem}
        onPickWorkItem={onPickWorkItem}
        createIssue={createIssue}
        linear={linear}
        source={workSource}
        canStageNewIssue={canStageNewIssue}
        showLinkedSelection={workSource === 'existing' || locked}
      /> : null}

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

      <div className="grid grid-cols-2 gap-2">
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

/**
 * Spec 011 F2 / 012 F1 / 013 F1: the New Workspace tracker section — ONE honest
 * section. A selected git repo resolves only its repo-owned binding; absent or
 * failed resolution exposes no Project rows. Legacy callers without a selected
 * git repo may still use the globally-active Project. It then lists that
 * Project's OPEN issues (PRs/closed excluded, via `deriveIssueOptions`) so the
 * operator binds the card they're about to work.
 *
 * Spec 013 F1: the "Change / Configure tracker" control lives in this section's
 * TOP header, and the status line is driven SOLELY by the resolved Project
 * (`deriveUnifiedTrackerStatus`) — the same value that seeds the issue list — so
 * the section can never claim "no tracker" while it lists issues (AC 3). The
 * old git-remote heuristic (`deriveWizardTracker`) is gone. The
 * `ProjectBindingEditor` popover is the SAME editor the hub / Settings use, so
 * the Project can be picked or switched right here.
 *
 * Picking is OPTIONAL and non-fatal (AC 3): no resolved Project / an unreachable
 * fetch shows an honest empty state and never blocks the step. Binding flows
 * through the composer's `applyLinkedWorkItem` seam (via `onPickWorkItem`), so
 * the workspace persists its tracker coords on create.
 */
function TrackerSection({
  workdir,
  repoId,
  local,
  activeProject,
  fetchProjectViewTable,
  getCachedProjectViewTable,
  linkedWorkItem,
  onPickWorkItem,
  createIssue,
  linear,
  source,
  canStageNewIssue,
  showLinkedSelection
}: {
  /** The selected GIT repo's workdir (local path, or the path on the repo's
   *  own host for an SSH repo). The slug is resolved server-side from this
   *  workdir's git remote — on the right host (#356). */
  workdir?: string
  /** Spec 020 F3: the repo's registry id — the server resolves the slug on
   *  the repo's own host (the read leg that makes SSH repos resolve at all;
   *  #359's hostId, migrated to the repoId wire). Gated exactly like
   *  `workdir`; also threaded to the binding editor. */
  repoId?: string
  /** False for SSH repos. Reading and configuration both remain host-aware via
   *  the selected registry `repoId`. */
  local?: boolean
  activeProject: GitHubProjectSettings['activeProject']
  fetchProjectViewTable: (
    args: GetProjectViewTableArgs,
    options?: { force?: boolean }
  ) => Promise<GetProjectViewTableResult>
  getCachedProjectViewTable: (args: GetProjectViewTableArgs) => GitHubProjectTable | null
  linkedWorkItem: LinkedWorkItemSummary | null
  onPickWorkItem: (option: WorkItemOption) => void
  createIssue: CreateIssueSeams
  linear: LinearCreateSeams
  source: WorkSource
  canStageNewIssue: boolean
  showLinkedSelection: boolean
}): React.JSX.Element {
  const [binding, setBinding] = useState<PickerBindingResolution | null>(null)
  const [tableState, setTableState] = useState<{
    scopeKey: string
    table: GitHubProjectTable
  } | null>(null)
  const [status, setStatus] = useState<'idle' | 'loading' | 'refreshing' | 'failed'>('idle')
  const [configureOpen, setConfigureOpen] = useState(false)
  const [query, setQuery] = useState('')

  const bindingTargetKey = workdir ? pickerBindingTargetKey({ workdir, repoId }) : null
  const currentBinding: PickerBindingResolution | null = bindingTargetKey
    ? binding?.targetKey === bindingTargetKey
      ? binding
      : { kind: 'loading', targetKey: bindingTargetKey }
    : null
  const latestBindingTargetRef = useRef(bindingTargetKey)
  latestBindingTargetRef.current = bindingTargetKey

  // Read the selected repo's per-repo Projects binding — host-aware via the
  // repo's registry id (spec 020: the server resolves the repo's own host, so
  // SSH repos resolve the same slug-keyed binding their local clone
  // configured). Fail-closed — a missing binding, an unreachable host, or gh
  // being unavailable leaves the selected repo fail-closed with no Project.
  useEffect(() => {
    if (!workdir) {
      setBinding(null)
      return
    }
    const targetKey = pickerBindingTargetKey({ workdir, repoId })
    setBinding({ kind: 'loading', targetKey })
    let cancelled = false
    void getProjectBinding({ workdir, repoId })
      .then((res) => {
        if (cancelled || latestBindingTargetRef.current !== targetKey) return
        setBinding(
          res.binding
            ? {
                kind: 'resolved',
                targetKey,
                repositorySlug: res.slug,
                binding: res.binding
              }
            : { kind: 'absent', targetKey }
        )
      })
      .catch((error: unknown) => {
        if (!cancelled && latestBindingTargetRef.current === targetKey) {
          setBinding({
            kind: 'failed',
            targetKey,
            ...(error instanceof GithubProjectsBindingError && error.code
              ? { errorCode: error.code }
              : {})
          })
        }
      })
    return () => {
      cancelled = true
    }
  }, [workdir, repoId])

  // Selected repos resolve only their own binding. The global activeProject is
  // retained solely for legacy callers without a selected git repo.
  const resolved = useMemo(
    () =>
      resolvePickerProject({
        binding: currentBinding,
        activeProject,
        selectedGitRepo: Boolean(workdir)
      }),
    [currentBinding, activeProject, workdir]
  )
  const repositorySlug = currentBinding?.kind === 'resolved' ? currentBinding.repositorySlug : null
  const scopeKey = resolved
    ? bindingTargetKey && repositorySlug
      ? pickerScopeKey({ targetKey: bindingTargetKey, repositorySlug, project: resolved })
      : `global:${pickerProjectKey(resolved)}`
    : null
  const latestScopeKeyRef = useRef(scopeKey)
  latestScopeKeyRef.current = scopeKey
  // Read the matching cache during render so the first frame after binding
  // resolution already contains rows. The effect below owns revalidation, not
  // first paint; a table from another Project remains ineligible by key.
  const cachedTable = useMemo(
    () =>
      resolved
        ? getCachedProjectViewTable({
            owner: resolved.owner,
            ownerType: resolved.ownerType,
            projectNumber: resolved.number
          })
        : null,
    [resolved, getCachedProjectViewTable]
  )
  const table = trackerSectionTableForScope(tableState, scopeKey, cachedTable)

  useEffect(() => {
    if (!resolved) {
      setStatus('idle')
      return
    }
    if (!scopeKey) return
    const capturedKey = scopeKey
    const args = {
      owner: resolved.owner,
      ownerType: resolved.ownerType,
      projectNumber: resolved.number
    }
    // Paint a fresh cached table synchronously — re-entering step 3 within the
    // cache TTL no longer re-fires the RPC or flashes the "loading the Project's
    // issues…" spinner over data that's seconds old (#385). A miss falls
    // through to the normal fetch below.
    const cached = getCachedProjectViewTable(args)
    let cancelled = false
    if (cached) {
      setTableState({ scopeKey: capturedKey, table: cached })
      setStatus('refreshing')
    } else {
      setStatus('loading')
    }
    void fetchProjectViewTable(args, cached ? { force: true } : undefined)
      .then((res) => {
        if (cancelled || !isCurrentTrackerSectionScope(capturedKey, latestScopeKeyRef.current)) return
        if (res.ok) {
          setTableState({ scopeKey: capturedKey, table: res.data })
          setStatus('idle')
        } else {
          setStatus('failed')
        }
      })
      .catch(() => {
        if (!cancelled && isCurrentTrackerSectionScope(capturedKey, latestScopeKeyRef.current)) {
          setStatus('failed')
        }
      })
    return () => {
      cancelled = true
    }
  }, [resolved, scopeKey, fetchProjectViewTable, getCachedProjectViewTable])

  useEffect(() => setQuery(''), [scopeKey])

  const issueView = useMemo(
    () => deriveTrackerIssueViewModel(table, query, repositorySlug ?? undefined),
    [table, query, repositorySlug]
  )
  const selectedUrl = linkedWorkItem?.type === 'issue' ? linkedWorkItem.url : null

  const refresh = useCallback(() => {
    if (!resolved || !scopeKey) return
    const args = {
      owner: resolved.owner,
      ownerType: resolved.ownerType,
      projectNumber: resolved.number
    }
    setStatus(table ? 'refreshing' : 'loading')
    void fetchProjectViewTable(args, { force: true })
      .then((res) => {
        if (!isCurrentTrackerSectionScope(scopeKey, latestScopeKeyRef.current)) return
        if (res.ok) {
          setTableState({ scopeKey, table: res.data })
          setStatus('idle')
        } else {
          setStatus('failed')
        }
      })
      .catch(() => {
        if (isCurrentTrackerSectionScope(scopeKey, latestScopeKeyRef.current)) setStatus('failed')
      })
  }, [fetchProjectViewTable, scopeKey, resolved, table])

  // Spec 013 F1: the ONE status, from the ONE resolved Project the list reads.
  const trackerStatus = useMemo<UnifiedTrackerStatus>(
    () =>
      deriveUnifiedTrackerStatus({
        resolved,
        binding: currentBinding,
        selectedGitRepo: Boolean(workdir),
        status,
        optionCount: issueView.issueCount,
        hasTable: Boolean(table)
      }),
    [resolved, currentBinding, workdir, status, issueView.issueCount, table]
  )

  // The compact repo-scoped configure/switch affordance. The shared editor and
  // server resolve both local and SSH repositories through workdir + repoId.
  const configureControl = workdir ? (
    <Popover open={configureOpen} onOpenChange={setConfigureOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="inline-flex items-center gap-1.5 rounded-md border border-border px-2 py-1 text-[11px] text-muted-foreground transition-colors hover:border-muted-foreground/40 hover:text-foreground"
        >
          <KanbanSquare className="size-3" />
          {trackerConfigureActionLabel(Boolean(resolved))}
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" className="max-h-[420px] w-[360px] overflow-y-auto p-3">
        <ProjectBindingEditor
          workdir={workdir}
          repoId={repoId}
          onBound={(next, nextRepositorySlug) => {
            if (bindingTargetKey) {
              setBinding({
                kind: 'resolved',
                targetKey: bindingTargetKey,
                repositorySlug: nextRepositorySlug,
                binding: next
              })
            }
            setConfigureOpen(false)
          }}
          onUnbound={() => {
            if (!bindingTargetKey) return
            const unbound = trackerSectionAfterSuccessfulUnbind(bindingTargetKey)
            latestScopeKeyRef.current = unbound.scopeKey
            setBinding(unbound.binding)
            setTableState(null)
            setStatus('idle')
            setQuery('')
            setConfigureOpen(false)
          }}
        />
      </PopoverContent>
    </Popover>
  ) : null

  return (
    <div className="flex flex-col gap-2.5">
      {/* Spec 013 F1 (AC 1): one section header, the control at the TOP. */}
      {source === 'existing' ? <div className="flex items-center justify-between gap-2">
        <span className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-muted-foreground">
          Tracker
        </span>
        {configureControl}
      </div> : null}

      {/* Status line — driven SOLELY by the resolved Project (AC 2). */}
      {source === 'existing' ? (
        <TrackerStatusLine
          status={trackerStatus}
          hasWorkdir={Boolean(workdir)}
          onRetry={resolved ? refresh : undefined}
        />
      ) : null}

      {source === 'existing' && table ? (
        <div className="overflow-hidden rounded-lg border border-border bg-card/30">
          <div className="flex items-center gap-2 border-b border-border px-2.5 py-2">
            <Search className="size-3.5 flex-none text-muted-foreground" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              aria-label="Filter Project issues"
              placeholder="Filter by title or #number"
              className="min-w-0 flex-1 bg-transparent text-[12px] text-foreground outline-none placeholder:text-muted-foreground/70"
            />
            <span className="flex-none font-mono text-[10.5px] text-muted-foreground">
              {issueView.issueCount} {issueView.issueCount === 1 ? 'issue' : 'issues'}
            </span>
            <button
              type="button"
              onClick={refresh}
              disabled={status === 'refreshing'}
              aria-label="Refresh Project issues"
              className="inline-flex size-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground disabled:opacity-50"
            >
              <RefreshCw className={cn('size-3.5', status === 'refreshing' && 'animate-spin')} />
            </button>
          </div>
          <div className="max-h-52 overflow-y-auto p-1">
            {issueView.groups.length ? (
              issueView.groups.map((group) => (
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
                      <span className="font-mono text-[10px] text-muted-foreground/70">
                        {group.options.length}
                      </span>
                    </div>
                  ) : null}
                  {group.options.map((option) => {
                    const selected = selectedUrl === option.url
                    return (
                      <button
                        key={option.itemId}
                        type="button"
                        aria-pressed={selected}
                        aria-label={`Link issue #${option.number}: ${option.title}`}
                        onClick={() => onPickWorkItem(option)}
                        className={cn(
                          'flex w-full items-center gap-2 rounded-md border-l-2 px-2.5 py-1.5 text-left text-[12px] transition-colors',
                          selected
                            ? 'border-l-primary bg-secondary text-foreground'
                            : 'border-l-transparent text-muted-foreground hover:bg-secondary/60 hover:text-foreground'
                        )}
                      >
                        <Check
                          className={cn('size-3 flex-none', selected ? 'opacity-70' : 'opacity-0')}
                        />
                        <span className="flex-none font-mono text-[11px] text-muted-foreground">
                          #{option.number}
                        </span>
                        <span className="truncate">{option.title}</span>
                        {group.label ? (
                          <span className="ml-auto inline-flex max-w-28 flex-none items-center gap-1 truncate rounded-full border border-border px-1.5 py-0.5 text-[9.5px] text-muted-foreground">
                            <span
                              className="size-1.5 flex-none rounded-full"
                              style={{ backgroundColor: trackerStatusColor(group.color) }}
                            />
                            <span className="truncate">{group.label}</span>
                          </span>
                        ) : null}
                      </button>
                    )
                  })}
                </div>
              ))
            ) : (
              <div className="px-3 py-5 text-center text-[11.5px] text-muted-foreground">
                {query.trim()
                  ? `No open issues match “${query.trim()}”.`
                  : 'No open issues in this Project.'}
              </div>
            )}
          </div>
          <div className="flex items-center justify-between border-t border-border px-2.5 py-1.5 text-[10.5px] text-muted-foreground">
            <span className="truncate">
              {table.project.title || `${table.project.owner} · Project ${table.project.number}`}
            </span>
            <span className="flex-none font-mono">
              {table.project.owner} · #{table.project.number}
            </span>
          </div>
        </div>
      ) : null}

      {/* Spec 013 F2/F3: create an issue from a short intent, then bind it. Only
          when nothing is linked yet and the repo is a local git repo. The file
          arm targets GitHub or Linear per the resolved tracker (F3). */}
      {source === 'new' && canStageNewIssue ? (
        <CreateIssuePanel createIssue={createIssue} linear={linear} resolved={resolved} deferred />
      ) : null}

      {linkedWorkItem && showLinkedSelection ? (
        <div className="flex items-start gap-2 rounded-lg border border-emerald-500/35 bg-emerald-500/8 px-3 py-2">
          <Check className="mt-0.5 size-3.5 flex-none text-emerald-500" aria-hidden />
          <span className="min-w-0 text-[11.5px] text-foreground">
            <span className="font-semibold text-emerald-500">Selected for this workspace</span>
            <span className="ml-1.5 font-mono text-muted-foreground">
              #{linkedWorkItem.number}
            </span>{' '}
            {linkedWorkItem.title}
          </span>
        </div>
      ) : null}
    </div>
  )
}

/**
 * Spec 013 F2/F3: "Create issue from intent" — a thin surface over the
 * composer's EXISTING create-issue seams. The operator types "what do you want
 * to do?", Draft calls `onGenerateIssueBody` (wiki+codebase-grounded
 * server-side), then Create files the drafted title+body.
 *
 * F3: the *file* step branches by provider (`resolveCreateIssueProvider`): a
 * GitHub Project resolves ⇒ GitHub (`onCreateIssueSubmit` → `createGithubIssue`
 * → `applyLinkedWorkItem`); no Project but Linear connected ⇒ Linear
 * (`linearCreateIssue`, then bind via the SAME `onSmartLinearIssueSelect` seam);
 * BOTH ⇒ a provider toggle. The drafted body is provider-agnostic.
 *
 * Errors render inline; the wizard's Create workspace primary is never gated on
 * this panel (inv. 7).
 */
function CreateIssuePanel({
  createIssue,
  linear,
  resolved,
  deferred = false
}: {
  createIssue: CreateIssueSeams
  linear: LinearCreateSeams
  resolved: PickerProjectRef | null
  deferred?: boolean
}): React.JSX.Element {
  const [open, setOpen] = useState(deferred)
  const savedChatAgent = useAppStore((state) => state.settings?.chatAgent)
  const updateSettings = useAppStore((state) => state.updateSettings)
  const { detectedIds: detectedChatAgentIds } = useDetectedAgents()
  const preferredDraftAgent = pickChatAgent(savedChatAgent, detectedChatAgentIds)
  const [draftAgent, setDraftAgent] = useState<ChatAgentId>(preferredDraftAgent)
  const [draftModel, setDraftModel] = useState(
    () => resolveChatModel(readChatModelPreference()).id
  )
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
  // F3 Linear arm — probed lazily when the panel opens (best-effort; a failure
  // leaves `linearConnected` false so the panel stays GitHub-only).
  const [linearConnected, setLinearConnected] = useState(false)
  const [teams, setTeams] = useState<LinearTeam[]>([])
  const [teamId, setTeamId] = useState<string | null>(null)
  const [providerChoice, setProviderChoice] = useState<CreateIssueProvider | null>(null)
  const [linearFiling, setLinearFiling] = useState(false)
  const [linearError, setLinearError] = useState<string | null>(null)

  // Probe Linear once the panel is open: is it connected, and what teams exist?
  // Non-fatal — any failure keeps the GitHub-only default.
  useEffect(() => {
    if (!open || deferred) {
      return
    }
    let cancelled = false
    void linearStatus(linear.settings)
      .then((status) => {
        if (cancelled || !status.connected) {
          return
        }
        setLinearConnected(true)
        return linearListTeams(linear.settings).then((list) => {
          if (cancelled) {
            return
          }
          setTeams(list)
          // Default to the sole team when there's exactly one (open question 2).
          if (list.length === 1) {
            setTeamId(list[0].id)
          }
        })
      })
      .catch(() => {
        /* best-effort: stay GitHub-only */
      })
    return () => {
      cancelled = true
    }
  }, [deferred, open, linear.settings])

  const provider = resolveCreateIssueProvider({ resolved, linearConnected })
  const effectiveProvider: CreateIssueProvider = deferred
    ? 'github'
    : provider === 'ambiguous'
      ? (providerChoice ?? 'github')
      : provider

  const busy = createIssue.generating || createIssue.submitting || linearFiling
  const phase = deriveCreateIssueIntentPhase({
    generating: createIssue.generating,
    submitting: createIssue.submitting || linearFiling,
    error: createIssue.error ?? linearError,
    hasBody: createIssue.body.trim().length > 0
  })
  const inlineError = effectiveProvider === 'linear' ? linearError : createIssue.error

  // Draft an SDD-shaped body from the TITLE (optional helper — the title is the
  // one required field now; a blank body files fine). Provider-agnostic markdown
  // that files to either tracker.
  const handleDraft = useCallback(() => {
    if (!canFileIssue(createIssue.title, busy)) {
      return
    }
    setLinearError(null)
    const choice: DraftLlmChoice = {
      agent: effectiveDraftAgent,
      ...(effectiveDraftAgent === 'claude' ? { model: draftModel } : {})
    }
    createIssue.onGenerate(choice)
  }, [busy, createIssue, draftModel, effectiveDraftAgent])

  const handleDraftAgentChange = useCallback(
    (agent: ChatAgentId) => {
      draftPreferenceTouched.current = true
      setDraftAgent(agent)
      void updateSettings({ chatAgent: agent })
    },
    [updateSettings]
  )

  const handleDraftModelChange = useCallback((model: string) => {
    setDraftModel(model)
    writeChatModelPreference(model)
  }, [])

  // F3 Linear file arm: create the issue in Linear, then bind it through the
  // SAME composer seam (`onSmartLinearIssueSelect`) the Linear @-picker uses, so
  // the workspace persists `trackerProvider:'linear'` on create (spec 012).
  const handleLinearFile = useCallback(async () => {
    const title = createIssue.title.trim()
    if (!canFileIssue(title, busy)) {
      return
    }
    if (!teamId) {
      setLinearError('Pick a Linear team to file into.')
      return
    }
    const team = teams.find((t) => t.id === teamId)
    setLinearFiling(true)
    setLinearError(null)
    try {
      const result = await linearCreateIssue(linear.settings, {
        teamId,
        title,
        description: createIssue.body.trim() || undefined
      })
      if (!result.ok) {
        setLinearError(result.error || 'Could not create the Linear issue.')
        return
      }
      const issue: LinearIssue = {
        id: result.id,
        identifier: result.identifier,
        title: result.title,
        description: createIssue.body.trim() || undefined,
        url: result.url,
        state: { name: 'Todo', type: 'unstarted', color: '#9ca3af' },
        team: team
          ? { id: team.id, name: team.name, key: team.key }
          : { id: teamId, name: teamId, key: teamId },
        labels: [],
        labelIds: [],
        priority: 0,
        updatedAt: new Date().toISOString()
      }
      linear.onBind(issue)
    } catch (error) {
      setLinearError(error instanceof Error ? error.message : 'Could not create the Linear issue.')
    } finally {
      setLinearFiling(false)
    }
  }, [busy, createIssue.body, createIssue.title, linear, teamId, teams])

  const handleFile = useCallback(() => {
    if (effectiveProvider === 'linear') {
      void handleLinearFile()
    } else {
      createIssue.onSubmit()
    }
  }, [createIssue, effectiveProvider, handleLinearFile])

  if (!open && !deferred) {
    return (
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="inline-flex items-center gap-1.5 self-start rounded-md border border-dashed border-border px-2.5 py-1.5 text-[11.5px] text-muted-foreground transition-colors hover:border-muted-foreground/40 hover:text-foreground"
      >
        <Plus className="size-3.5" />
        Create an issue for this work
      </button>
    )
  }

  const hasBody = createIssue.body.trim().length > 0
  const canFile = canFileIssue(createIssue.title, busy)

  return (
    // Enter inside the panel files the issue (or inserts a newline in the
    // description) — it must NOT bubble to the wizard's global key handler, which
    // would otherwise create the whole workspace out from under a half-typed
    // issue. Stopping propagation here is the fix for the "creates it double"
    // report: the title <input>'s Enter used to trigger "Create workspace".
    <div
      className="flex flex-col gap-2.5 rounded-lg border border-border p-3"
      onKeyDown={(event) => {
        if (event.key === 'Enter' && !event.shiftKey) {
          event.stopPropagation()
        }
      }}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="text-[12px] font-medium text-foreground">New GitHub issue</span>
        {!deferred ? <button
          type="button"
          onClick={() => setOpen(false)}
          aria-label="Cancel"
          className="inline-flex size-5 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        >
          <X className="size-3.5" />
        </button> : null}
      </div>

      {/* F3: only when a repo has BOTH a GitHub Project and Linear connected. */}
      {!deferred && provider === 'ambiguous' ? (
        <div className="flex items-center gap-1.5">
          <span className="text-[11px] text-muted-foreground">File into</span>
          {(['github', 'linear'] as const).map((p) => (
            <button
              key={p}
              type="button"
              onClick={() => setProviderChoice(p)}
              className={cn(
                'rounded-md border px-2 py-0.5 text-[11px] capitalize transition-colors',
                effectiveProvider === p
                  ? 'border-muted-foreground/40 bg-secondary text-foreground'
                  : 'border-border text-muted-foreground hover:border-muted-foreground/25'
              )}
            >
              {p}
            </button>
          ))}
        </div>
      ) : null}

      {/* Single required field: the title. Enter files the issue right away — no
          separate "draft" round-trip stands between typing and creating. */}
      <label className="flex flex-col gap-1.5">
        <span className="text-[11px] text-muted-foreground">Title</span>
        <input
          autoFocus
          value={createIssue.title}
          onChange={(event) => createIssue.onTitleChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.shiftKey) event.stopPropagation()
          }}
          placeholder="What needs doing?"
          className="h-[34px] rounded-md border border-input bg-secondary px-2.5 text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
        />
      </label>

      {/* Description is OPTIONAL. "Draft with AI" fills an SDD-shaped body from
          the title; the user can also just type, or leave it blank. */}
      <div className="flex flex-col gap-1.5">
        <div className="flex flex-wrap items-end justify-between gap-2">
          <span className="text-[11px] text-muted-foreground">Description (optional)</span>
          <div className="flex flex-wrap items-end gap-1.5">
            <label className="flex flex-col gap-0.5">
              <span className="font-mono text-[9px] uppercase tracking-[0.1em] text-muted-foreground">
                Engine
              </span>
              <select
                aria-label="Drafting engine"
                value={effectiveDraftAgent}
                disabled={availableDraftAgents.length === 0 || busy}
                onChange={(event) => handleDraftAgentChange(event.target.value as ChatAgentId)}
                className="h-6 rounded-md border border-border bg-secondary px-1.5 text-[10.5px] text-foreground outline-none focus-visible:border-ring disabled:opacity-50"
              >
                {availableDraftAgents.length === 0 ? (
                  <option value={effectiveDraftAgent}>No detected engine</option>
                ) : null}
                {availableDraftAgents.map((agent) => (
                  <option key={agent.id} value={agent.id}>
                    {agent.label}
                  </option>
                ))}
              </select>
            </label>
            {effectiveDraftAgent === 'claude' ? (
              <label className="flex flex-col gap-0.5">
                <span className="font-mono text-[9px] uppercase tracking-[0.1em] text-muted-foreground">
                  Model
                </span>
                <select
                  aria-label="Drafting model"
                  value={draftModel}
                  disabled={busy}
                  onChange={(event) => handleDraftModelChange(event.target.value)}
                  className="h-6 max-w-44 rounded-md border border-border bg-secondary px-1.5 text-[10.5px] text-foreground outline-none focus-visible:border-ring disabled:opacity-50"
                >
                  {CHAT_MODELS.map((model) => (
                    <option key={model.id} value={model.id}>
                      {model.label}
                    </option>
                  ))}
                </select>
              </label>
            ) : (
              <span className="inline-flex h-6 items-center rounded-md border border-border bg-secondary px-2 text-[10.5px] text-muted-foreground">
                default model
              </span>
            )}
            <button
              type="button"
              onClick={handleDraft}
              disabled={!canFile || availableDraftAgents.length === 0}
              className="inline-flex h-6 items-center gap-1 rounded-md border border-border px-2 text-[11px] text-muted-foreground transition-colors hover:border-muted-foreground/40 hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
            >
              {phase === 'drafting' ? (
                <Loader2 className="size-3 animate-spin" />
              ) : (
                <Sparkles className="size-3" />
              )}
              {phase === 'drafting' ? 'Drafting…' : hasBody ? 'Redraft' : 'Draft with AI'}
            </button>
          </div>
        </div>
        <textarea
          value={createIssue.body}
          onChange={(event) => createIssue.onBodyChange(event.target.value)}
          rows={hasBody ? 6 : 3}
          placeholder="Add details, or let AI draft an SDD-shaped description from the title."
          className="resize-none rounded-md border border-input bg-secondary px-2.5 py-2 font-mono text-[11.5px] leading-relaxed text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
        />
      </div>

      {deferred && createIssue.labelOptions?.length ? (
        <div className="flex flex-col gap-1.5">
          <span className="text-[11px] text-muted-foreground">Labels (optional)</span>
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
                    'rounded-full border px-2 py-0.5 text-[10.5px] transition-colors',
                    selected
                      ? 'border-primary/50 bg-primary/10 text-foreground'
                      : 'border-border text-muted-foreground hover:border-muted-foreground/40'
                  )}
                >
                  {label}
                </button>
              )
            })}
          </div>
        </div>
      ) : null}

      {/* F3: pick the Linear team when filing into Linear and >1 exists. */}
      {!deferred && effectiveProvider === 'linear' && teams.length > 1 ? (
        <label className="flex flex-col gap-1.5">
          <span className="text-[11px] text-muted-foreground">Linear team</span>
          <select
            value={teamId ?? ''}
            onChange={(event) => setTeamId(event.target.value || null)}
            className="h-[34px] rounded-md border border-input bg-secondary px-2 text-[12.5px] text-foreground outline-none focus-visible:border-ring"
          >
            <option value="">Select a team…</option>
            {teams.map((team) => (
              <option key={team.id} value={team.id}>
                {team.name} ({team.key})
              </option>
            ))}
          </select>
        </label>
      ) : null}

      {!deferred ? <button
        type="button"
        onClick={handleFile}
        disabled={!canFile}
        className="inline-flex items-center gap-1.5 self-start rounded-full bg-primary px-3.5 py-1.5 text-[12px] font-medium text-primary-foreground transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-50"
      >
        {phase === 'filing' ? <Loader2 className="size-3.5 animate-spin" /> : null}
        {phase === 'filing'
          ? 'Creating…'
          : effectiveProvider === 'linear'
            ? 'Create Linear issue'
            : 'Create issue'}
      </button> : null}

      {inlineError ? <span className="text-[11px] text-destructive">{inlineError}</span> : null}
    </div>
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

/** The single status line for the unified tracker section — a pure view of
 *  `deriveUnifiedTrackerStatus`. "none" is the ONLY state that reads "no
 *  tracker", and it renders only when no Project resolved (AC 3). */
function TrackerStatusLine({
  status,
  hasWorkdir,
  onRetry
}: {
  status: UnifiedTrackerStatus
  hasWorkdir: boolean
  onRetry?: () => void
}): React.JSX.Element {
  switch (status.kind) {
    case 'resolving':
      return (
        <div className="flex items-center gap-2 rounded-lg border border-border px-3 py-2.5 text-[11.5px] text-muted-foreground">
          <Loader2 className="size-3.5 animate-spin" />
          Resolving this repository&apos;s Project…
        </div>
      )
    case 'binding-unavailable':
      return (
        <div className="flex items-center gap-3 rounded-lg border border-border bg-secondary px-3 py-2.5">
          <KanbanSquare className="size-[15px] flex-none text-muted-foreground" />
          <span className="min-w-0 flex-1 text-[11.5px] text-muted-foreground">
            Couldn&apos;t resolve this repository&apos;s tracker. Workspace creation is still available.
          </span>
        </div>
      )
    case 'binding-mismatch':
      return (
        <div className="flex items-center gap-3 rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2.5">
          <KanbanSquare className="size-[15px] flex-none text-destructive" />
          <span className="min-w-0 flex-1 text-[11.5px] text-muted-foreground">
            This tracker belongs to a different repository. Reconfigure it before selecting an issue.
          </span>
        </div>
      )
    case 'connecting':
      return (
        <div className="flex items-center gap-2 rounded-lg border border-border px-3 py-2.5 text-[11.5px] text-muted-foreground">
          <Loader2 className="size-3.5 animate-spin" />
          Connecting — loading the Project's issues…
        </div>
      )
    case 'unavailable':
      return (
        <div className="flex items-center gap-3 rounded-lg border border-border bg-secondary px-3 py-2.5">
          <KanbanSquare className="size-[15px] flex-none text-muted-foreground" />
          <span className="min-w-0 flex-1 text-[11.5px] text-muted-foreground">
            Tracker connected, but its issues couldn't load — pick or link one later (optional).
          </span>
          {onRetry ? (
            <button
              type="button"
              onClick={onRetry}
              className="rounded-md border border-border px-2 py-1 text-[10.5px] hover:text-foreground"
            >
              Retry
            </button>
          ) : null}
        </div>
      )
    case 'connected-empty':
      return (
        <div className="flex items-center gap-3 rounded-lg border border-border bg-secondary px-3 py-2.5">
          <KanbanSquare className="size-[15px] flex-none text-muted-foreground" />
          <span className="min-w-0 flex-1 text-[11.5px] text-muted-foreground">
            {status.refreshing
              ? 'Refreshing Project issues…'
              : status.stale
                ? 'Refresh failed — showing the last saved Project data.'
                : 'Tracker connected — no open issues in this Project. Link one later (optional).'}
          </span>
          <span className="flex-none rounded-full bg-emerald-500/15 px-2 py-0.5 font-mono text-[10.5px] text-emerald-500">
            connected
          </span>
        </div>
      )
    case 'connected':
      return (
        <div className="flex items-center gap-3 rounded-lg border border-border bg-secondary px-3 py-2.5">
          <KanbanSquare className="size-[15px] flex-none text-muted-foreground" />
          <span className="min-w-0 flex-1 text-[11.5px] text-muted-foreground">
            {status.refreshing
              ? 'Refreshing Project issues — cached rows remain available.'
              : status.stale
                ? 'Refresh failed — showing the last saved Project data.'
                : "Tracker connected — pick the issue you're about to work (optional)."}
          </span>
          <span className="flex-none rounded-full bg-emerald-500/15 px-2 py-0.5 font-mono text-[10.5px] text-emerald-500">
            {status.issueCount} open
          </span>
        </div>
      )
    case 'none':
    default:
      return (
        <div className="rounded-lg border border-dashed border-border px-3 py-2.5 text-[11.5px] text-muted-foreground">
          {hasWorkdir
            ? 'No tracker bound to this repo yet — configure one above to pick an issue (optional).'
            : 'No tracker — configure one from the Board view to pick an issue here (optional).'}
        </div>
      )
  }
}
