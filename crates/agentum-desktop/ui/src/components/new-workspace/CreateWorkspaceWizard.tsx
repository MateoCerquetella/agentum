import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  ArrowLeft,
  ArrowRight,
  Check,
  ChevronsUpDown,
  GitBranch,
  KanbanSquare,
  Laptop,
  Loader2,
  PlugZap,
  Plus,
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
import { searchRuntimeRepoBaseRefs } from '@/runtime/runtime-repo-client'
import { cn } from '@/lib/utils'
import { useAppStore } from '@/store'
import { useComposerState } from '@/hooks/useComposerState'
import {
  pickQuickWorkspaceAgent,
  resolveQuickWorkspaceAgentSelection
} from '@/lib/quick-workspace-agent-selection'
import { AGENT_CATALOG, AgentIcon } from '@/lib/agent-catalog'
import { isFolderRepo } from '../../../../shared/repo-kind'
import { filterReposForHost } from '@/hooks/composer-host-scoping'
import { LOCAL_HOST_KEY } from '@/components/sidebar/worktree-list-groups'
import type { LinkedWorkItemSummary } from '@/lib/new-workspace'
import {
  REPO_LIST_COLLAPSED_CAP,
  WIZARD_STEP_LABELS,
  buildWizardRecap,
  canLeaveRepoStep as canLeaveRepoStepModel,
  capRepoList,
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
  deriveIssueOptions,
  resolvePickerProject,
  type PickerProjectRef,
  type WorkItemOption
} from '@/components/new-workspace/work-item-picker-model'
import {
  canDraftIssue,
  canFileIssue,
  deriveCreateIssueIntentPhase,
  deriveIntentTitle,
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
  getProjectBinding,
  type ProjectBindingDto
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

/**
 * The "Create Workspace" wizard — a three-step front-end (Host → Repo &
 * worktree → Agent & tracker) over the shared `useComposerState` creation
 * engine. Spec 013 F4: it is the SINGLE front door for `new-workspace-composer`
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
  // Spec 011 F2: the issue picker resolves its Project from the selected repo's
  // per-repo binding first; the globally-active Project
  // (settings.githubProjects.activeProject) is the fallback (spec 012 behavior,
  // preserved when there's no per-repo binding).
  const activeProject = settings?.githubProjects?.activeProject ?? null

  // Spec 013 F4: seed the SAME `useComposerState` from the full modal-open data
  // (pure `deriveWizardComposerSeed` — every opinionated field honored). The
  // gate mode / issue-automation / submit path are unchanged, so the gated run
  // is inherited byte-identically (inv. 4).
  const { cardProps, submitQuick, nameInputRef } = useComposerState({
    ...deriveWizardComposerSeed(modalData),
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
    createDisabled,
    selectedRepoRequiresConnection,
    selectedRepoConnectInProgress,
    onConnectSelectedRepo,
    applyLinkedWorkItem,
    linkedWorkItem,
    // Spec 013 F2: the composer's EXISTING create-issue seams — the wizard
    // shares the hook, so it renders them rather than rebuilding the flow.
    canCreateGithubIssue,
    createIssueTitle,
    onCreateIssueTitleChange,
    createIssueBody,
    onCreateIssueBodyChange,
    createIssueGenerating,
    onGenerateIssueBody,
    createIssueSubmitting,
    createIssueError,
    onCreateIssueSubmit,
    // Spec 013 F3: bind a filed Linear issue through the SAME composer seam the
    // Linear @-picker uses (`setLinkedWorkItem(buildLinearIssueLinkedWorkItem)`).
    onSmartLinearIssueSelect,
    // Spec 013 F4: the gated-run toggle — the SAME seams the composer card used,
    // so `submitQuick` inherits the `start_work` precondition set unchanged.
    canStartGatedRun,
    startGatedRun,
    onStartGatedRunChange,
    sddRolesEnabled
  } = cardProps

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
  // Badge the host we opened on ("last used") — captured once so re-selecting
  // doesn't move the badge around.
  const lastUsedHostKeyRef = useRef(selectedHostKey)

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

  const handlePrimary = useCallback(() => {
    if (step === 3) {
      if (!createDisabled && !creating) {
        void submitQuick(quickAgent)
      }
      return
    }
    if (step === 2 && !canLeaveRepoStep) {
      return
    }
    goNext()
  }, [canLeaveRepoStep, createDisabled, creating, goNext, quickAgent, step, submitQuick])

  // Enter advances / creates (there are no multi-line inputs here, so a bare
  // Enter is unambiguous); Radix handles Esc → close on its own.
  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (event.key !== 'Enter' || event.shiftKey) {
        return
      }
      const target = event.target as HTMLElement | null
      if (target instanceof HTMLTextAreaElement) {
        return
      }
      event.preventDefault()
      handlePrimary()
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

  const primaryLabel = wizardPrimaryLabel(step)
  const primaryDisabled =
    step === 3 ? createDisabled || creating : step === 2 ? !canLeaveRepoStep : false

  // Spec 013 F1: the tracker section reads SOLELY from the Project the picker
  // resolves (per-repo binding ∨ global activeProject) — no git-remote heuristic
  // that could disagree with the picker's issue list. The per-repo binding
  // resolves through the local `gh`, so only a LOCAL git repo can carry (or
  // configure) one; a remote/folder repo leaves `trackerWorkdir` undefined and
  // the picker falls back to the global activeProject.
  const trackerWorkdir =
    selectedRepo && !selectedRepo.connectionId && selectedRepoIsGit ? selectedRepo.path : undefined

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        onKeyDown={handleKeyDown}
        className="flex max-h-[min(680px,calc(100dvh-4rem))] w-full flex-col gap-0 overflow-hidden p-0 sm:max-w-[640px]"
      >
        <DialogTitle className="sr-only">New workspace</DialogTitle>
        <DialogDescription className="sr-only">
          Create a workspace in three steps: choose a host, a repo and worktree, then the agent.
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
              disabledRepoIds={disabledRepoIds}
              repoId={repoId}
              onRepoChange={onRepoChange}
              selectedRepo={selectedRepo}
              selectedRepoIsGit={selectedRepoIsGit}
              name={name}
              onNameValueChange={onNameValueChange}
              nameInputRef={nameInputRef}
              baseBranch={baseBranch}
              onBaseBranchChange={onBaseBranchChange}
              requiresConnection={selectedRepoRequiresConnection}
              connectInProgress={selectedRepoConnectInProgress}
              onConnect={onConnectSelectedRepo}
            />
          ) : null}

          {step === 3 ? (
            <AgentStep
              agents={agentOptions}
              detectedAgentIds={detectedAgentIds}
              quickAgent={quickAgent}
              onPick={setQuickAgentOverride}
              trackerWorkdir={trackerWorkdir}
              activeProject={activeProject}
              fetchProjectViewTable={fetchProjectViewTable}
              linkedWorkItem={linkedWorkItem}
              onPickWorkItem={onPickWorkItem}
              createIssue={{
                canCreate: canCreateGithubIssue,
                title: createIssueTitle,
                onTitleChange: onCreateIssueTitleChange,
                body: createIssueBody,
                onBodyChange: onCreateIssueBodyChange,
                generating: createIssueGenerating,
                onGenerate: onGenerateIssueBody,
                submitting: createIssueSubmitting,
                error: createIssueError,
                onSubmit: onCreateIssueSubmit
              }}
              linear={{ settings, onBind: onSmartLinearIssueSelect }}
              gatedRun={{
                canStart: canStartGatedRun,
                enabled: startGatedRun,
                onChange: onStartGatedRunChange,
                sddRolesEnabled
              }}
            />
          ) : null}
        </div>

        {/* Footer */}
        <div className="flex flex-none items-center gap-2.5 border-t border-border bg-muted/40 px-[18px] py-3">
          {step > 1 ? (
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
            onClick={handlePrimary}
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

// ---------- Step 2: Repo & worktree ----------

function RepoStep({
  hostLabel,
  repos,
  disabledRepoIds,
  repoId,
  onRepoChange,
  selectedRepo,
  selectedRepoIsGit,
  name,
  onNameValueChange,
  nameInputRef,
  baseBranch,
  onBaseBranchChange,
  requiresConnection,
  connectInProgress,
  onConnect
}: {
  hostLabel: string
  repos: Repo[]
  disabledRepoIds: Map<string, string>
  repoId: string
  onRepoChange: (value: string) => void
  selectedRepo: Repo | undefined
  selectedRepoIsGit: boolean
  name: string
  onNameValueChange: (value: string) => void
  nameInputRef: React.RefObject<HTMLInputElement | null>
  baseBranch: string | undefined
  onBaseBranchChange: (next: string | undefined) => void
  requiresConnection: boolean
  connectInProgress: boolean
  onConnect: () => Promise<void>
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
        <div className="rounded-lg border border-dashed border-border px-4 py-6 text-center text-[12.5px] text-muted-foreground">
          No repos on {hostLabel} yet. Add one from the sidebar, then come back.
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
        </div>
      )}

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

      {selectedRepoIsGit ? (
        <div className="flex flex-col gap-2.5">
          <span className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-muted-foreground">
            Worktree
          </span>
          <div className="grid grid-cols-[160px_1fr] gap-2.5">
            <label className="flex flex-col gap-1.5">
              <span className="text-[11.5px] text-muted-foreground">Base branch</span>
              <BaseBranchCombobox
                repoId={repoId}
                baseBranch={baseBranch}
                defaultRef={selectedRepo?.worktreeBaseRef ?? null}
                onChange={onBaseBranchChange}
              />
            </label>
            <label className="flex flex-col gap-1.5">
              <span className="text-[11.5px] text-muted-foreground">Worktree name</span>
              <input
                ref={nameInputRef}
                value={name}
                onChange={(event) => onNameValueChange(event.target.value)}
                placeholder="auto"
                className="h-[34px] rounded-md border border-input bg-secondary px-2.5 font-mono text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
              />
            </label>
          </div>
        </div>
      ) : selectedRepo ? (
        <div className="flex flex-col gap-2.5">
          <span className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-muted-foreground">
            Workspace name
          </span>
          <input
            ref={nameInputRef}
            value={name}
            onChange={(event) => onNameValueChange(event.target.value)}
            placeholder="auto"
            className="h-[34px] rounded-md border border-input bg-secondary px-2.5 font-mono text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
          />
          <span className="text-[11px] text-muted-foreground">
            {selectedRepo.displayName} isn&apos;t a git repo — the workspace opens the folder as-is.
          </span>
        </div>
      ) : null}
    </div>
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

// ---------- Step 3: Agent & tracker ----------

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
  generating: boolean
  onGenerate: () => void
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

/** Spec 013 F4: the composer's gated-run seams, migrated into the wizard. Maps
 *  1:1 onto `cardProps` — no new state or submit path. */
type GatedRunSeams = {
  /** Eligible when a github.com issue is linked and the repo is local git. */
  canStart: boolean
  enabled: boolean
  onChange: (value: boolean) => void
  /** Whether gated runs use the SDD role loop (drives the armed copy only). */
  sddRolesEnabled: boolean
}

function AgentStep({
  agents,
  detectedAgentIds,
  quickAgent,
  onPick,
  trackerWorkdir,
  activeProject,
  fetchProjectViewTable,
  linkedWorkItem,
  onPickWorkItem,
  createIssue,
  linear,
  gatedRun
}: {
  agents: TuiAgent[]
  detectedAgentIds: Set<TuiAgent> | null
  quickAgent: TuiAgent | null
  onPick: (agent: TuiAgent) => void
  trackerWorkdir?: string
  activeProject: GitHubProjectSettings['activeProject']
  fetchProjectViewTable: (args: GetProjectViewTableArgs) => Promise<GetProjectViewTableResult>
  linkedWorkItem: LinkedWorkItemSummary | null
  onPickWorkItem: (option: WorkItemOption) => void
  createIssue: CreateIssueSeams
  linear: LinearCreateSeams
  gatedRun: GatedRunSeams
}): React.JSX.Element {
  return (
    <div className="flex animate-in flex-col gap-[18px] fade-in-0 slide-in-from-bottom-1">
      <div className="flex flex-col gap-0.5">
        <span className="text-[15px] font-semibold tracking-[-0.01em] text-foreground">
          Who drives — and where is it tracked?
        </span>
        <span className="text-[12px] text-muted-foreground">
          Remembered from last time. Confirm or change.
        </span>
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
                onClick={() => onPick(agent)}
                title={installed ? undefined : 'Not detected on PATH'}
                className={cn(
                  'inline-flex items-center gap-2 rounded-full border px-3 py-1.5 text-[12.5px] transition-colors',
                  selected
                    ? 'border-muted-foreground/40 bg-secondary text-foreground'
                    : 'border-border text-muted-foreground hover:border-muted-foreground/25',
                  !installed && !selected ? 'opacity-55' : ''
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
            No agents detected on PATH — install one, or pick after the workspace opens.
          </span>
        ) : null}
      </div>

      <TrackerSection
        workdir={trackerWorkdir}
        activeProject={activeProject}
        fetchProjectViewTable={fetchProjectViewTable}
        linkedWorkItem={linkedWorkItem}
        onPickWorkItem={onPickWorkItem}
        createIssue={createIssue}
        linear={linear}
      />

      {/* Spec 013 F4: the migrated "Start gated run" toggle — eligible only when
          a github.com issue is linked to a local git repo. Bound to the SAME
          cardProps seams, so submitting arms `start_work` unchanged (inv. 4). */}
      {gatedRun.canStart ? (
        <label className="flex cursor-pointer items-start gap-2.5 rounded-lg border border-border bg-secondary px-3 py-2.5">
          <input
            type="checkbox"
            checked={gatedRun.enabled}
            onChange={(event) => gatedRun.onChange(event.target.checked)}
            className="mt-0.5 size-3.5 flex-none accent-primary"
          />
          <span className="flex min-w-0 flex-col gap-0.5">
            <span className="text-[12.5px] font-medium text-foreground">Start gated run</span>
            <span className="text-[11px] text-muted-foreground">
              {gatedRun.enabled
                ? gatedRun.sddRolesEnabled
                  ? 'The linked issue becomes the spec; the SDD role loop drives the worktree behind a verify gate.'
                  : 'The linked issue becomes the spec; the Harness Engine drives the worktree behind a verify gate.'
                : 'Turn the linked issue into a spec and let the Harness Engine drive the worktree behind a verify gate.'}
            </span>
          </span>
        </label>
      ) : null}
    </div>
  )
}

/**
 * Spec 011 F2 / 012 F1 / 013 F1: the New Workspace tracker section — ONE honest
 * section. It resolves its Project from the selected repo's per-repo binding
 * first, falling back to the globally-active Project (spec 012, no regression),
 * then lists that Project's OPEN issues (PRs/closed excluded, via
 * `deriveIssueOptions`) so the operator binds the card they're about to work.
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
  activeProject,
  fetchProjectViewTable,
  linkedWorkItem,
  onPickWorkItem,
  createIssue,
  linear
}: {
  /** The selected repo's local workdir — present only for a LOCAL git repo,
   *  which is the only kind that can carry/configure a per-repo binding. The
   *  slug is resolved server-side from this workdir's git remote. */
  workdir?: string
  activeProject: GitHubProjectSettings['activeProject']
  fetchProjectViewTable: (args: GetProjectViewTableArgs) => Promise<GetProjectViewTableResult>
  linkedWorkItem: LinkedWorkItemSummary | null
  onPickWorkItem: (option: WorkItemOption) => void
  createIssue: CreateIssueSeams
  linear: LinearCreateSeams
}): React.JSX.Element {
  const [binding, setBinding] = useState<ProjectBindingDto | null>(null)
  const [table, setTable] = useState<GitHubProjectTable | null>(null)
  const [status, setStatus] = useState<'idle' | 'loading' | 'failed'>('idle')
  const [configureOpen, setConfigureOpen] = useState(false)

  // Read the selected repo's per-repo Projects binding. Fail-closed — a missing
  // binding, a remote repo, or gh being unavailable leaves `binding` null so
  // resolution falls back to the global activeProject (spec 012).
  useEffect(() => {
    if (!workdir) {
      setBinding(null)
      return
    }
    let cancelled = false
    void getProjectBinding({ workdir })
      .then((res) => {
        if (!cancelled) setBinding(res.binding)
      })
      .catch(() => {
        if (!cancelled) setBinding(null)
      })
    return () => {
      cancelled = true
    }
  }, [workdir])

  // Per-repo binding wins; else the global activeProject; else null.
  const resolved = useMemo(
    () => resolvePickerProject({ binding, activeProject }),
    [binding, activeProject]
  )

  useEffect(() => {
    if (!resolved) {
      setTable(null)
      setStatus('idle')
      return
    }
    let cancelled = false
    setStatus('loading')
    void fetchProjectViewTable({
      owner: resolved.owner,
      ownerType: resolved.ownerType,
      projectNumber: resolved.number
    })
      .then((res) => {
        if (cancelled) return
        if (res.ok) {
          setTable(res.data)
          setStatus('idle')
        } else {
          setTable(null)
          setStatus('failed')
        }
      })
      .catch(() => {
        if (!cancelled) {
          setTable(null)
          setStatus('failed')
        }
      })
    return () => {
      cancelled = true
    }
  }, [resolved, fetchProjectViewTable])

  const options = useMemo(() => deriveIssueOptions(table), [table])
  const selectedUrl = linkedWorkItem?.type === 'issue' ? linkedWorkItem.url : null

  // Spec 013 F1: the ONE status, from the ONE resolved Project the list reads.
  const trackerStatus = useMemo<UnifiedTrackerStatus>(
    () => deriveUnifiedTrackerStatus({ resolved, status, optionCount: options.length }),
    [resolved, status, options.length]
  )

  // The compact configure/switch affordance — only a LOCAL git repo (a
  // resolvable workdir) can carry a binding, so gate the control on it. Reuses
  // the SAME editor as the hub / Settings; onBound refreshes the section so it
  // re-resolves to the freshly-bound Project.
  const configureControl = workdir ? (
    <Popover open={configureOpen} onOpenChange={setConfigureOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="inline-flex items-center gap-1.5 rounded-md border border-border px-2 py-1 text-[11px] text-muted-foreground transition-colors hover:border-muted-foreground/40 hover:text-foreground"
        >
          <KanbanSquare className="size-3" />
          {resolved ? 'Change tracker' : 'Configure tracker'}
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" className="max-h-[420px] w-[360px] overflow-y-auto p-3">
        <ProjectBindingEditor
          workdir={workdir}
          onBound={(next) => {
            setBinding(next)
            setConfigureOpen(false)
          }}
        />
      </PopoverContent>
    </Popover>
  ) : null

  return (
    <div className="flex flex-col gap-2.5">
      {/* Spec 013 F1 (AC 1): one section header, the control at the TOP. */}
      <div className="flex items-center justify-between gap-2">
        <span className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-muted-foreground">
          Tracker
        </span>
        {configureControl}
      </div>

      {/* Status line — driven SOLELY by the resolved Project (AC 2). */}
      <TrackerStatusLine status={trackerStatus} hasWorkdir={Boolean(workdir)} />

      {/* Issue list — only when a Project resolved and has open issues. */}
      {trackerStatus.kind === 'connected' ? (
        <div className="flex max-h-44 flex-col gap-1 overflow-y-auto rounded-lg border border-border p-1">
          {options.map((option) => {
            const selected = selectedUrl === option.url
            return (
              <button
                key={option.itemId}
                type="button"
                onClick={() => onPickWorkItem(option)}
                className={cn(
                  'flex items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[12px] transition-colors',
                  selected
                    ? 'bg-secondary text-foreground'
                    : 'text-muted-foreground hover:bg-secondary/60 hover:text-foreground'
                )}
              >
                <Check className={cn('size-3 flex-none', selected ? 'opacity-70' : 'opacity-0')} />
                <span className="flex-none font-mono text-[11px] text-muted-foreground">
                  #{option.number}
                </span>
                <span className="truncate">{option.title}</span>
              </button>
            )
          })}
        </div>
      ) : null}

      {/* Spec 013 F2/F3: create an issue from a short intent, then bind it. Only
          when nothing is linked yet and the repo is a local git repo. The file
          arm targets GitHub or Linear per the resolved tracker (F3). */}
      {createIssue.canCreate ? (
        <CreateIssuePanel createIssue={createIssue} linear={linear} resolved={resolved} />
      ) : null}

      {linkedWorkItem ? (
        <span className="text-[11px] text-emerald-500">
          Linked · #{linkedWorkItem.number} {linkedWorkItem.title}
        </span>
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
  resolved
}: {
  createIssue: CreateIssueSeams
  linear: LinearCreateSeams
  resolved: PickerProjectRef | null
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [intent, setIntent] = useState('')
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
    if (!open) {
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
  }, [open, linear.settings])

  const provider = resolveCreateIssueProvider({ resolved, linearConnected })
  const effectiveProvider: CreateIssueProvider =
    provider === 'ambiguous' ? (providerChoice ?? 'github') : provider

  const busy = createIssue.generating || createIssue.submitting || linearFiling
  const phase = deriveCreateIssueIntentPhase({
    generating: createIssue.generating,
    submitting: createIssue.submitting || linearFiling,
    error: createIssue.error ?? linearError,
    hasBody: createIssue.body.trim().length > 0
  })
  const inlineError = effectiveProvider === 'linear' ? linearError : createIssue.error

  const handleDraft = useCallback(() => {
    if (!canDraftIssue(intent, busy)) {
      return
    }
    setLinearError(null)
    // Seed the title from the intent (reuse), then draft the body server-side
    // (provider-agnostic markdown — the same body files to either tracker).
    createIssue.onTitleChange(deriveIntentTitle(intent))
    createIssue.onGenerate()
  }, [busy, createIssue, intent])

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

  if (!open) {
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

  const hasDraft = createIssue.body.trim().length > 0 || createIssue.title.trim().length > 0

  return (
    <div className="flex flex-col gap-2.5 rounded-lg border border-border p-3">
      <div className="flex items-center justify-between gap-2">
        <span className="text-[12px] font-medium text-foreground">Create an issue</span>
        <button
          type="button"
          onClick={() => setOpen(false)}
          aria-label="Cancel"
          className="inline-flex size-5 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        >
          <X className="size-3.5" />
        </button>
      </div>

      {/* F3: only when a repo has BOTH a GitHub Project and Linear connected. */}
      {provider === 'ambiguous' ? (
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

      <label className="flex flex-col gap-1.5">
        <span className="text-[11px] text-muted-foreground">What do you want to do?</span>
        <textarea
          value={intent}
          onChange={(event) => setIntent(event.target.value)}
          rows={2}
          placeholder="Describe the work — a title and an SDD-shaped body get drafted from it."
          className="resize-none rounded-md border border-input bg-secondary px-2.5 py-2 text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
        />
      </label>

      <button
        type="button"
        onClick={handleDraft}
        disabled={!canDraftIssue(intent, busy)}
        className="inline-flex items-center gap-1.5 self-start rounded-md border border-border px-2.5 py-1.5 text-[11.5px] text-foreground transition-colors hover:border-muted-foreground/40 disabled:cursor-not-allowed disabled:opacity-50"
      >
        {phase === 'drafting' ? (
          <Loader2 className="size-3.5 animate-spin" />
        ) : (
          <Sparkles className="size-3.5" />
        )}
        {phase === 'drafting' ? 'Drafting…' : hasDraft ? 'Redraft from intent' : 'Draft issue'}
      </button>

      {hasDraft ? (
        <>
          <label className="flex flex-col gap-1.5">
            <span className="text-[11px] text-muted-foreground">Title</span>
            <input
              value={createIssue.title}
              onChange={(event) => createIssue.onTitleChange(event.target.value)}
              placeholder="Issue title"
              className="h-[34px] rounded-md border border-input bg-secondary px-2.5 text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
            />
          </label>
          <label className="flex flex-col gap-1.5">
            <span className="text-[11px] text-muted-foreground">Description</span>
            <textarea
              value={createIssue.body}
              onChange={(event) => createIssue.onBodyChange(event.target.value)}
              rows={6}
              placeholder="Drafted description — review and edit before filing."
              className="resize-none rounded-md border border-input bg-secondary px-2.5 py-2 font-mono text-[11.5px] leading-relaxed text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
            />
          </label>

          {/* F3: pick the Linear team when filing into Linear and >1 exists. */}
          {effectiveProvider === 'linear' && teams.length > 1 ? (
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

          <button
            type="button"
            onClick={handleFile}
            disabled={!canFileIssue(createIssue.title, busy)}
            className="inline-flex items-center gap-1.5 self-start rounded-full bg-primary px-3.5 py-1.5 text-[12px] font-medium text-primary-foreground transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {phase === 'filing' ? <Loader2 className="size-3.5 animate-spin" /> : null}
            {phase === 'filing'
              ? 'Creating…'
              : effectiveProvider === 'linear'
                ? 'Create Linear issue'
                : 'Create issue'}
          </button>
        </>
      ) : null}

      {inlineError ? <span className="text-[11px] text-destructive">{inlineError}</span> : null}
    </div>
  )
}

/** The single status line for the unified tracker section — a pure view of
 *  `deriveUnifiedTrackerStatus`. "none" is the ONLY state that reads "no
 *  tracker", and it renders only when no Project resolved (AC 3). */
function TrackerStatusLine({
  status,
  hasWorkdir
}: {
  status: UnifiedTrackerStatus
  hasWorkdir: boolean
}): React.JSX.Element {
  switch (status.kind) {
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
        </div>
      )
    case 'connected-empty':
      return (
        <div className="flex items-center gap-3 rounded-lg border border-border bg-secondary px-3 py-2.5">
          <KanbanSquare className="size-[15px] flex-none text-muted-foreground" />
          <span className="min-w-0 flex-1 text-[11.5px] text-muted-foreground">
            Tracker connected — no open issues in this Project. Link one later (optional).
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
            Tracker connected — pick the issue you're about to work (optional).
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
