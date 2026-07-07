import React, { useCallback, useMemo, useRef, useState } from 'react'
import {
  ArrowLeft,
  ArrowRight,
  Check,
  GitBranch,
  KanbanSquare,
  Laptop,
  Loader2,
  PlugZap,
  Server,
  X
} from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle
} from '@/components/ui/dialog'
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
  WIZARD_STEP_LABELS,
  buildWizardRecap,
  canLeaveRepoStep as canLeaveRepoStepModel,
  resolveWizardAgentOptions,
  wizardNextHint,
  wizardPrimaryLabel,
  type WizardStep
} from '@/components/new-workspace/create-workspace-wizard-model'
import type { Repo, TuiAgent, WorkspaceCreateTelemetrySource } from '../../../../shared/types'

/** The modal-data slice the wizard honors. A superset lives on the composer
 *  modal; the wizard only reads the plain-open fields (opinionated opens route
 *  to the advanced card instead). */
export type CreateWorkspaceWizardData = {
  prefilledName?: string
  initialRepoId?: string
  linkedWorkItem?: LinkedWorkItemSummary | null
  telemetrySource?: WorkspaceCreateTelemetrySource
}

/**
 * The "Create Workspace" wizard — a three-step front-end (Host → Repo &
 * worktree → Agent & tracker) over the shared `useComposerState` creation
 * engine. Like `NewWorkspaceGoalStep`, it never becomes a state machine inside
 * the engine: it drives the same host/repo/name/baseBranch/agent state the
 * composer card drives and calls `submitQuick`, so YOLO translation, SSH
 * gating, setup hooks and post-create launch stay centralized.
 */
export default function CreateWorkspaceWizard({
  modalData,
  onClose,
  onOpenChange,
  onAdvanced,
  onUseGoal
}: {
  modalData: CreateWorkspaceWizardData
  onClose: () => void
  onOpenChange: (open: boolean) => void
  /** Switch to the full composer card (linked items, gated run, sparse, …). */
  onAdvanced: () => void
  /** Switch to the goal-first step (spec 008), when the caller offers it. */
  onUseGoal?: () => void
}): React.JSX.Element {
  const settings = useAppStore((s) => s.settings)
  const repos = useAppStore((s) => s.repos)
  const hostMetaByKey = useAppStore((s) => s.hostMetaByKey)
  const sshConnectionStates = useAppStore((s) => s.sshConnectionStates)

  const { cardProps, submitQuick, nameInputRef } = useComposerState({
    initialName: modalData.prefilledName ?? '',
    initialPrompt: '',
    initialLinkedWorkItem: null,
    initialRepoId: modalData.initialRepoId,
    persistDraft: false,
    onCreated: onClose,
    ...(modalData.telemetrySource ? { telemetrySource: modalData.telemetrySource } : {}),
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
    selectedRepoPath,
    selectedRepoRequiresConnection,
    selectedRepoConnectInProgress,
    onConnectSelectedRepo
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
  const nextHint = wizardNextHint(step, selectedHostLabel)

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
              selectedRepoPath={selectedRepoPath}
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
              selectedRepo={selectedRepo}
              selectedRepoIsGit={selectedRepoIsGit}
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
          ) : (
            <div className="flex items-center gap-3">
              <button
                type="button"
                onClick={onAdvanced}
                className="text-[11.5px] text-muted-foreground underline decoration-dotted underline-offset-2 transition-colors hover:text-foreground"
              >
                Advanced options
              </button>
              {onUseGoal ? (
                <button
                  type="button"
                  onClick={onUseGoal}
                  className="text-[11.5px] text-muted-foreground underline decoration-dotted underline-offset-2 transition-colors hover:text-foreground"
                >
                  Start from a goal
                </button>
              ) : null}
            </div>
          )}
          <span className="flex-1" />
          <span className="hidden text-[11.5px] text-muted-foreground sm:inline">{nextHint}</span>
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
  selectedRepoPath,
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
  selectedRepoPath: string | null
  name: string
  onNameValueChange: (value: string) => void
  nameInputRef: React.RefObject<HTMLInputElement | null>
  baseBranch: string | undefined
  onBaseBranchChange: (next: string | undefined) => void
  requiresConnection: boolean
  connectInProgress: boolean
  onConnect: () => Promise<void>
}): React.JSX.Element {
  const slug = (name.trim() || 'worktree').replace(/\//g, '-')
  const worktreePath = selectedRepoPath ? `${selectedRepoPath}/.worktrees/${slug}` : null
  const defaultBranchPlaceholder = selectedRepo?.worktreeBaseRef?.trim() || 'default branch'

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
          {repos.map((repo) => {
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
          })}
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
              <span className="flex h-[34px] items-center gap-2 rounded-md border border-input bg-secondary px-2.5">
                <GitBranch className="size-3.5 flex-none text-muted-foreground" />
                <input
                  value={baseBranch ?? ''}
                  placeholder={defaultBranchPlaceholder}
                  onChange={(event) =>
                    onBaseBranchChange(event.target.value.trim() ? event.target.value : undefined)
                  }
                  className="w-full min-w-0 bg-transparent font-mono text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground/70"
                />
              </span>
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
          {worktreePath ? (
            <span className="truncate font-mono text-[11px] text-muted-foreground">
              → {worktreePath}
            </span>
          ) : null}
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

// ---------- Step 3: Agent & tracker ----------

function AgentStep({
  agents,
  detectedAgentIds,
  quickAgent,
  onPick,
  selectedRepo,
  selectedRepoIsGit
}: {
  agents: TuiAgent[]
  detectedAgentIds: Set<TuiAgent> | null
  quickAgent: TuiAgent | null
  onPick: (agent: TuiAgent) => void
  selectedRepo: Repo | undefined
  selectedRepoIsGit: boolean
}): React.JSX.Element {
  const trackerName = selectedRepo?.displayName ?? 'this repo'
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

      <div className="flex flex-col gap-2.5">
        <span className="font-mono text-[10.5px] uppercase tracking-[0.14em] text-muted-foreground">
          Tracker
        </span>
        {selectedRepoIsGit ? (
          <>
            <div className="flex items-center gap-3 rounded-lg border border-border bg-secondary px-3 py-3">
              <KanbanSquare className="size-[15px] flex-none text-muted-foreground" />
              <span className="flex min-w-0 flex-1 flex-wrap items-baseline gap-2.5">
                <span className="text-[13px] font-medium text-foreground">{trackerName}</span>
                <span className="font-mono text-[11px] text-muted-foreground">
                  auto-detected from origin
                </span>
              </span>
              <span className="flex-none rounded-full bg-emerald-500/15 px-2 py-0.5 font-mono text-[10.5px] text-emerald-500">
                detected
              </span>
            </div>
            <span className="text-[11.5px] text-muted-foreground">
              Link only — issues stay in their tracker. Configure the source in the Tasks view.
            </span>
          </>
        ) : (
          <div className="rounded-lg border border-dashed border-border px-3 py-3 text-[12px] text-muted-foreground">
            No tracker — link one later from the Tasks view (optional).
          </div>
        )}
      </div>
    </div>
  )
}
