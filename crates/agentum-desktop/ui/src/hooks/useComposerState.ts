import { api } from '@/tauri'
/* eslint-disable max-lines -- Why: this hook co-locates every piece of state
the NewWorkspaceComposerCard reads or mutates, so both the full-page composer
and the global quick-composer modal can consume a single unified source of
truth without duplicating effects, derivation, or the create side-effect. */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { useShallow } from 'zustand/react/shallow'
import { useAppStore } from '@/store'
import { AGENT_CATALOG } from '@/lib/agent-catalog'
import {
  parseGitHubIssueOrPRNumber,
  parseGitHubIssueOrPRLink,
  normalizeGitHubLinkQuery
} from '@/lib/github-links'
import { openCreatedWorkspace } from '@/lib/open-created-workspace'
import { gatedRunResultOwnsWorktree } from '@/lib/gated-run-ownership'
import { filterEnabledTuiAgents, isTuiAgentEnabled } from '../../../shared/tui-agent-selection'
import { isGitRepoKind } from '../../../shared/repo-kind'
import { callRuntimeRpc, getActiveRuntimeTarget } from '@/runtime/runtime-rpc-client'
import { connectSshTargetViaServer } from '@/runtime/server-host-client'
import type {
  GitHubWorkItem,
  GitHubPrStartPoint,
  GitPushTarget,
  GitLabWorkItem,
  LinearIssue,
  AgentumHooks,
  CreateWorktreeResult,
  SetupDecision,
  SetupRunPolicy,
  SparsePreset,
  TuiAgent,
  WorktreeMeta,
  WorkspaceStatus,
  WorkspaceCreateTelemetrySource
} from '../../../shared/types'
import { isWorkspaceStatusId } from '../../../shared/workspace-statuses'
import {
  DEFAULT_ISSUE_COMMAND_TEMPLATE,
  buildAgentPromptWithContext,
  getAttachmentLabel,
  getLinkedWorkItemSuggestedName,
  getSetupConfig,
  getWorkspaceSeedName,
  isGitLabIssueUrl,
  PER_REPO_FETCH_LIMIT,
  renderIssueCommandTemplate,
  type LinkedWorkItemSummary,
  type SetupConfig
} from '@/lib/new-workspace'
import {
  getLinkedWorkItemPromptContext,
  resolveQuickCreateLinkedWorkItemPrompt
} from '@/lib/linked-work-item-context'
import { buildGithubIssueContextSnapshot } from '@/lib/github-linked-work-item'
import { composeIssueContextBody, STATIC_FALLBACK_LABELS } from '@/lib/issue-context-body'
import {
  createGithubIssue,
  draftGithubIssueBody,
  fetchGithubRepoLabels,
  scaffoldSpecFromIssue
} from '@/runtime/github-issue-client'
import {
  deriveIssueSideEffectGate,
  describeIssueSideEffectSkip
} from '@/lib/issue-side-effect-gate'
import { firstStartGatedRunBlocker } from '@/lib/start-gated-run-precondition'
import {
  getHarnessSettings,
  startGatedWork,
  subscribeHarnessRunErrors
} from '@/runtime/harness-client'
import { buildLinearIssueLinkedWorkItem } from '@/lib/linear-linked-work-item'
import {
  getFullComposerCreateDisabled,
  getQuickComposerCreateDisabled
} from '@/lib/new-workspace-create-gates'
import {
  lookupSmartGitHubSubmitItem,
  getSmartGitHubSubmitIntent,
  getSmartGitHubSubmitResolution,
  type SmartGitHubSubmitResolution
} from '@/lib/smart-github-submit'
import {
  canUseRepoBackedComposerSources,
  getSelectedRepoSshGate,
  isSshConnectInProgress
} from '@/lib/new-workspace-ssh-gate'
import { getSuggestedCreatureName } from '@/components/sidebar/worktree-name-suggestions'
import type { SmartWorkspaceNameSelection } from '@/components/new-workspace/SmartWorkspaceNameField'
import { deriveTrackerBindCoords } from '@/components/new-workspace/work-item-picker-model'
import { ensureHooksConfirmed } from '@/lib/ensure-hooks-confirmed'
import { normalizeSparseDirectoryLines, sparseDirectoriesMatch } from '@/lib/sparse-paths'
import { joinPath } from '@/lib/path'
import type {
  ExecutionMode,
  NewWorkCheckpoint,
  NewWorkStage,
  NewWorkStageStatus
} from '@/components/new-workspace/new-work-launch-model'
import { importExternalPathsToRuntime } from '@/runtime/runtime-file-client'
import {
  checkRuntimeHooks,
  readRuntimeIssueCommand,
  type HookCheckResult
} from '@/runtime/runtime-hooks-client'
import {
  formatWorkspaceCreateError,
  getWorkspaceCreateErrorToastMessage,
  type WorkspaceCreateErrorDisplay
} from '@/lib/workspace-create-error-format'
import type { SshConnectionStatus } from '../../../shared/ssh-types'
import {
  resolveComposerBranchNameOverrideForCreate,
  resolveComposerBranchSelection
} from './composer-branch-selection'
import {
  type ComposerHostOption,
  deriveEligibleHosts,
  filterReposForHost,
  gitOnHostCacheKey,
  resolveDefaultHostKey,
  resolveRepoIdForHost
} from './composer-host-scoping'

export type UseComposerStateOptions = {
  initialRepoId?: string
  initialName?: string
  initialPrompt?: string
  initialLinkedWorkItem?: LinkedWorkItemSummary | null
  initialWorkspaceStatus?: WorkspaceStatus
  /** Spec 005 F1 (AC 3): open with the "Start gated run" toggle armed — the
   *  Tasks page row action pre-fills the composer this way. */
  initialStartGatedRun?: boolean
  /** Seed the Start-from selection when the composer opens. Used by the
   *  Create-from → Quick fallback path so a PR pick that needs a setup
   *  decision still lands with the resolved PR head as the base branch. */
  initialBaseBranch?: string
  /** Why: the full-page composer persists drafts so users can navigate away
   *  without losing work; the quick-composer modal is transient and must not
   *  clobber or leak that long-running draft. */
  persistDraft: boolean
  /** Invoked after a successful createWorktree. The caller usually closes its
   *  surface here (palette modal, full page, etc.). */
  onCreated?: () => void
  /** Optional external repoId override — used by TaskPage's work-item list
   *  which drives repo selection from the page header, not the card. */
  repoIdOverride?: string
  onRepoIdOverrideChange?: (value: string) => void
  /** Telemetry surface that opened this composer. Threaded into
   *  `createWorktree` so `workspace_created.source` reflects the actual
   *  entry point (Cmd+J palette → `command_palette`, sidebar buttons →
   *  `sidebar`, keyboard shortcut → `shortcut`). Omitted callers default
   *  to `unknown` at the IPC boundary. */
  telemetrySource?: WorkspaceCreateTelemetrySource
  /** Quick-create launches a blank/draft agent session and does not run
   *  issueCommand automation, so it can skip the issue-command probe that the
   *  full composer needs for linked-item prompt previews. */
  enableIssueAutomation?: boolean
  createGateMode?: 'full' | 'quick'
}

type ComposerCardProps = {
  eligibleRepos: ReturnType<typeof useAppStore.getState>['repos']
  /** Host selector (spec 006): local + each configured SSH host with repos.
   *  Empty when driven by `repoIdOverride` (TaskPage/JumpPalette), which keeps
   *  their existing repo-first behavior and hides the host row. */
  eligibleHosts: ComposerHostOption[]
  selectedHostKey: string
  onHostChange: (hostKey: string) => void
  /** `eligibleRepos` filtered to `selectedHostKey` — the repo picker shows only
   *  this host's repos. Equals `eligibleRepos` when driven by `repoIdOverride`. */
  hostScopedRepos: ReturnType<typeof useAppStore.getState>['repos']
  /** repoId → reason for repos that aren't a git repo on the selected host;
   *  the combobox renders these disabled with the reason as a hint. */
  disabledRepoIds: Map<string, string>
  repoId: string
  selectedRepoIsGit: boolean
  onRepoChange: (value: string) => void
  name: string
  onNameValueChange: (value: string) => void
  onSmartGitHubItemSelect: (item: GitHubWorkItem) => void
  onSmartGitLabItemSelect: (item: GitLabWorkItem) => void
  onSmartBranchSelect: (refName: string, localBranchName: string) => void
  onSmartLinearIssueSelect: (issue: LinearIssue) => void
  /** GitLab parallel of onBaseBranchPrSelect. */
  onBaseBranchMrSelect?: (
    baseBranch: string,
    item: GitLabWorkItem,
    pushTarget?: GitPushTarget
  ) => void
  smartNameSelection: SmartWorkspaceNameSelection | null
  onClearSmartNameSelection: () => void
  agentPrompt: string
  onAgentPromptChange: (value: string) => void
  /** Rendered issueCommand template to preview inside the empty prompt
   *  textarea when the user has linked a work item but not typed anything. */
  linkedOnlyTemplatePreview: string | null
  attachmentPaths: string[]
  getAttachmentLabel: (pathValue: string) => string
  onAddAttachment: () => void
  onRemoveAttachment: (pathValue: string) => void
  linkedWorkItem: LinkedWorkItemSummary | null
  onRemoveLinkedWorkItem: () => void
  /** Bind an existing work item (spec 012 New Workspace issue picker). The one
   *  attach seam — setting the composer's `linkedWorkItem` so the create path
   *  persists the tracker bind (see `deriveTrackerBindCoords`). */
  applyLinkedWorkItem: (
    item: GitHubWorkItem,
    options?: { preserveBranchNameOverride?: boolean }
  ) => void
  /** True when the composer should offer "Create GitHub issue": nothing is
   *  linked yet and the selected repo is a local git repo (spec 004 F3). */
  canCreateGithubIssue: boolean
  createIssueOpen: boolean
  onCreateIssueOpenChange: (open: boolean) => void
  createIssueTitle: string
  onCreateIssueTitleChange: (value: string) => void
  createIssueBody: string
  onCreateIssueBodyChange: (value: string) => void
  createIssueSubmitting: boolean
  createIssueError: string | null
  onCreateIssueSubmit: () => Promise<LinkedWorkItemSummary | null>
  /** Spec 007: "Generate description" — drafts an SDD-shaped body from the
   *  typed title + repo context into the textarea (review before filing). */
  createIssueGenerating: boolean
  onGenerateIssueBody: () => void
  /** Spec 006 F1: label picker selection for the create-issue form. */
  createIssueLabels: string[]
  /** Pickable label names — `null` while the fetch is in flight; the static
   *  fallback set when the fetch errored. */
  createIssueLabelOptions: string[] | null
  onToggleCreateIssueLabel: (label: string) => void
  /** True when the "Scaffold spec" toggle applies: a github.com issue is
   *  linked and the target is a local git repo (spec 004 F4, D5). */
  canScaffoldSpec: boolean
  /** Opt-in, off by default (D5): after the worktree is created, write
   *  `.agentum-harness/specs/<n>-<slug>/spec.md` from the linked issue. */
  scaffoldSpec: boolean
  onScaffoldSpecChange: (value: boolean) => void
  /** Spec 005 F1: "Start gated run" — same eligibility gate as the scaffold
   *  toggle (linked github.com issue + local repo). When armed the linked
   *  issue becomes the spec and the Harness Engine drives the worktree; the
   *  scaffold toggle hides (subsumed — the server converge-scaffolds). */
  canStartGatedRun: boolean
  startGatedRun: boolean
  onStartGatedRunChange: (value: boolean) => void
  /** Spec 006 F3 (AC 8): whether gated runs use the SDD role loop — read from
   *  the harness settings when the toggle becomes available; optimistic `true`
   *  on failure (the server default is ON). Drives the armed copy only. */
  sddRolesEnabled: boolean
  linkPopoverOpen: boolean
  onLinkPopoverOpenChange: (open: boolean) => void
  linkQuery: string
  onLinkQueryChange: (value: string) => void
  filteredLinkItems: GitHubWorkItem[]
  linkItemsLoading: boolean
  linkDirectLoading: boolean
  normalizedLinkQuery: { query: string }
  onSelectLinkedItem: (item: GitHubWorkItem) => void
  tuiAgent: TuiAgent
  onTuiAgentChange: (value: TuiAgent) => void
  detectedAgentIds: Set<TuiAgent> | null
  onOpenAgentSettings: () => void
  advancedOpen: boolean
  onToggleAdvanced: () => void
  createDisabled: boolean
  creating: boolean
  onCreate: () => void
  note: string
  onNoteChange: (value: string) => void
  /** When true, create the worktree only — no tmux session/agent is launched. */
  skipSession: boolean
  onSkipSessionChange: (value: boolean) => void
  baseBranch: string | undefined
  onBaseBranchChange: (next: string | undefined) => void
  /** Called when a PR is selected in the Start-from picker. Updates both
   *  baseBranch and linkedWorkItem/linkedPR in one pass. */
  onBaseBranchPrSelect: (
    baseBranch: string,
    item: GitHubWorkItem,
    pushTarget?: GitPushTarget,
    branchNameOverride?: string
  ) => void
  /** PR number selected via the Start-from picker (when applicable). Used so the
   *  field can render "PR #N" copy. */
  baseBranchLinkedPrNumber: number | null
  /** Absolute path of the selected repo, used by Start-from picker for SWR. */
  selectedRepoPath: string | null
  /** True when the selected repo is a remote SSH repo. */
  selectedRepoIsRemote: boolean
  selectedRepoConnectionId: string | null
  selectedRepoSshStatus: SshConnectionStatus | null
  selectedRepoRequiresConnection: boolean
  selectedRepoConnectInProgress: boolean
  onConnectSelectedRepo: () => Promise<void>
  /** Transient inline hint shown next to the Start-from trigger after a repo
   *  switch resets a prior selection (e.g. "was PR #8778"). Null when none. */
  startFromResetHint: string | null
  setupConfig: SetupConfig | null
  requiresExplicitSetupChoice: boolean
  setupDecision: 'run' | 'skip' | null
  onSetupDecisionChange: (value: 'run' | 'skip') => void
  shouldWaitForSetupCheck: boolean
  resolvedSetupDecision: 'run' | 'skip' | null
  createError: WorkspaceCreateErrorDisplay | null
  canUseSparseCheckout: boolean
  /** Saved presets for the currently-selected repo. Empty array when no
   *  presets exist or when the repo is remote. */
  sparsePresets: SparsePreset[]
  /** ID of the selected sparse preset. Null means sparse checkout is off. */
  sparseSelectedPresetId: string | null
  onSparseSelectPreset: (preset: SparsePreset | null) => void
}

export type UseComposerStateResult = {
  cardProps: ComposerCardProps
  /** Ref the consumer should attach to the composer wrapper so the global
   *  Enter-to-submit handler can scope its behavior to the visible composer. */
  composerRef: React.RefObject<HTMLDivElement | null>
  onComposerNodeChange: (node: HTMLDivElement | null) => void
  promptTextareaRef: React.RefObject<HTMLTextAreaElement | null>
  nameInputRef: React.RefObject<HTMLInputElement | null>
  submit: () => Promise<void>
  submitQuick: (agent: TuiAgent | null, options?: QuickSubmitOptions) => Promise<void>
  /** Invoked by the Enter handler to re-check whether submission should fire. */
  createDisabled: boolean
}

export type QuickSubmitOptions = {
  linkedWorkItem?: LinkedWorkItemSummary
  executionMode?: ExecutionMode
  checkpoint?: NewWorkCheckpoint
  onCheckpoint?: (next: NewWorkCheckpoint) => void
  onProgress?: (stage: NewWorkStage, status: NewWorkStageStatus) => void
}

// Why: both the full-page TaskPage composer and the Cmd+J modal can be
// mounted simultaneously. Without instance scoping, a single native file
// drop fires every subscriber and duplicates attachments/prompt edits across
// the background draft and the visible modal. Route drops to the
// most-recently-mounted composer only — the modal stacks on top, so the
// modal wins when both are present, and the page takes over once the modal
// closes.
const composerDropStack: symbol[] = []
const EMPTY_SPARSE_PRESETS: SparsePreset[] = []

export function useComposerState(options: UseComposerStateOptions): UseComposerStateResult {
  const {
    initialRepoId,
    initialName = '',
    initialPrompt = '',
    initialLinkedWorkItem = null,
    initialWorkspaceStatus,
    initialStartGatedRun,
    initialBaseBranch,
    persistDraft,
    onCreated,
    repoIdOverride,
    onRepoIdOverrideChange,
    telemetrySource,
    enableIssueAutomation = true,
    createGateMode = 'full'
  } = options

  // Why: each `useAppStore(s => s.someAction)` registers its own equality
  // check that React has to re-run on every store mutation. Consolidating
  // all stable actions into a single useShallow subscription turns 11 checks
  // per store update into one.
  const actions = useAppStore(
    useShallow((s) => ({
      setNewWorkspaceDraft: s.setNewWorkspaceDraft,
      clearNewWorkspaceDraft: s.clearNewWorkspaceDraft,
      createWorktree: s.createWorktree,
      updateWorktreeMeta: s.updateWorktreeMeta,
      setSidebarOpen: s.setSidebarOpen,
      closeModal: s.closeModal,
      openSettingsPage: s.openSettingsPage,
      openSettingsTarget: s.openSettingsTarget,
      prefetchWorkItems: s.prefetchWorkItems,
      fetchSparsePresets: s.fetchSparsePresets,
      fetchDetectedWorktrees: s.fetchDetectedWorktrees
    }))
  )
  const {
    setNewWorkspaceDraft,
    clearNewWorkspaceDraft,
    createWorktree,
    updateWorktreeMeta,
    setSidebarOpen,
    closeModal,
    openSettingsPage,
    openSettingsTarget,
    prefetchWorkItems,
    fetchSparsePresets,
    fetchDetectedWorktrees
  } = actions

  const repos = useAppStore((s) => s.repos)
  const activeRepoId = useAppStore((s) => s.activeRepoId)
  const hostMetaByKey = useAppStore((s) => s.hostMetaByKey)
  const settings = useAppStore((s) => s.settings)
  const newWorkspaceDraft = useAppStore((s) => s.newWorkspaceDraft)
  const worktreesByRepo = useAppStore((s) => s.worktreesByRepo)
  const sparsePresetsByRepo = useAppStore((s) => s.sparsePresetsByRepo)
  const workspaceStatuses = useAppStore((s) => s.workspaceStatuses)
  const sshConnectionStates = useAppStore((s) => s.sshConnectionStates)
  const sshConnectedGeneration = useAppStore((s) => s.sshConnectedGeneration)
  const eligibleRepos = useMemo(() => repos.filter((repo) => Boolean(repo.path)), [repos])
  const draftRepoId = persistDraft ? (newWorkspaceDraft?.repoId ?? null) : null
  const resolvedInitialWorkspaceStatus = useMemo(
    () =>
      initialWorkspaceStatus && isWorkspaceStatusId(initialWorkspaceStatus, workspaceStatuses)
        ? initialWorkspaceStatus
        : undefined,
    [initialWorkspaceStatus, workspaceStatuses]
  )

  const resolvedInitialRepoId =
    draftRepoId && eligibleRepos.some((repo) => repo.id === draftRepoId)
      ? draftRepoId
      : initialRepoId && eligibleRepos.some((repo) => repo.id === initialRepoId)
        ? initialRepoId
        : activeRepoId && eligibleRepos.some((repo) => repo.id === activeRepoId)
          ? activeRepoId
          : (eligibleRepos[0]?.id ?? '')

  const [internalRepoId, setInternalRepoId] = useState<string>(resolvedInitialRepoId)
  const repoId = repoIdOverride ?? internalRepoId
  const selectedRepo = eligibleRepos.find((repo) => repo.id === repoId)
  const selectedRepoIsGit = selectedRepo ? isGitRepoKind(selectedRepo) : false
  const selectedRepoConnectionId = selectedRepo?.connectionId ?? null
  const selectedRepoSshState = selectedRepoConnectionId
    ? (sshConnectionStates.get(selectedRepoConnectionId) ?? null)
    : null
  const { selectedRepoSshStatus, selectedRepoRequiresConnection, selectedRepoConnectInProgress } =
    getSelectedRepoSshGate({
      connectionId: selectedRepoConnectionId,
      status: selectedRepoSshState?.status ?? null
    })
  const repoIdRef = useRef(repoId)
  repoIdRef.current = repoId
  const setRepoId = useCallback(
    (value: string) => {
      if (onRepoIdOverrideChange) {
        onRepoIdOverrideChange(value)
      } else {
        setInternalRepoId(value)
      }
    },
    [onRepoIdOverrideChange]
  )

  // Host-first New Workspace (spec 006). When the composer is driven by an
  // external `repoIdOverride` (TaskPage work-item list, WorktreeJumpPalette),
  // host selection is bypassed entirely — the host is implied by the overridden
  // repo and those surfaces keep their repo-first behavior unchanged.
  const hostScopingEnabled = repoIdOverride === undefined
  const eligibleHosts = useMemo<ComposerHostOption[]>(
    () => (hostScopingEnabled ? deriveEligibleHosts(eligibleRepos, hostMetaByKey) : []),
    [eligibleRepos, hostMetaByKey, hostScopingEnabled]
  )
  // Default to the active workspace/project's host, else the first eligible host
  // (local-first), else local — resolved once when the composer mounts.
  // Why: when initialRepoId is passed (e.g. from Add Project dialog opening a
  // workspace composer for an SSH project), use that repo's host, not activeRepoId.
  const [selectedHostKey, setSelectedHostKey] = useState<string>(() =>
    resolveDefaultHostKey(eligibleRepos, initialRepoId ?? activeRepoId, deriveEligibleHosts(eligibleRepos, {}))
  )
  const hostScopedRepos = useMemo(
    () => (hostScopingEnabled ? filterReposForHost(eligibleRepos, selectedHostKey) : eligibleRepos),
    [eligibleRepos, hostScopingEnabled, selectedHostKey]
  )

  const handleHostChange = useCallback(
    (nextHostKey: string): void => {
      if (nextHostKey === selectedHostKey) {
        return
      }
      setSelectedHostKey(nextHostKey)
      // The current repoId likely belongs to the previous host; reset it to the
      // new host's first repo (or clear it when that host has no repos).
      const nextRepoId = resolveRepoIdForHost(
        filterReposForHost(eligibleRepos, nextHostKey),
        repoIdRef.current
      )
      if (nextRepoId !== repoIdRef.current) {
        setRepoId(nextRepoId)
      }
    },
    [eligibleRepos, selectedHostKey, setRepoId]
  )

  // Per-`(hostKey, repoId)` cache of the `worktrees/detected` authoritative flag
  // (true ⇒ a real git repo on that host). Cached for the dialog's lifetime so a
  // remote host isn't re-probed over SSH on every keystroke/re-render. `null`
  // means "probe in flight / not yet resolved" — treated as enabled-pending so
  // the dialog never blocks on a slow SSH round trip.
  const [gitOnHostCache, setGitOnHostCache] = useState<Map<string, boolean>>(() => new Map())
  const gitProbeInFlightRef = useRef<Set<string>>(new Set())
  const isGitOnHost = useCallback(
    (targetRepoId: string): boolean | null => {
      const value = gitOnHostCache.get(gitOnHostCacheKey(selectedHostKey, targetRepoId))
      return value === undefined ? null : value
    },
    [gitOnHostCache, selectedHostKey]
  )
  // Lazily probe each scoped repo's git-ness on the selected host, once.
  useEffect(() => {
    if (!hostScopingEnabled) {
      return
    }
    let cancelled = false
    for (const repo of hostScopedRepos) {
      const cacheKey = gitOnHostCacheKey(selectedHostKey, repo.id)
      if (gitOnHostCache.has(cacheKey) || gitProbeInFlightRef.current.has(cacheKey)) {
        continue
      }
      gitProbeInFlightRef.current.add(cacheKey)
      void fetchDetectedWorktrees(repo.id)
        .then((result) => {
          gitProbeInFlightRef.current.delete(cacheKey)
          if (cancelled || !result) {
            return
          }
          setGitOnHostCache((prev) => {
            if (prev.has(cacheKey)) {
              return prev
            }
            const next = new Map(prev)
            next.set(cacheKey, result.authoritative)
            return next
          })
        })
        .catch(() => {
          gitProbeInFlightRef.current.delete(cacheKey)
        })
    }
    return () => {
      cancelled = true
    }
  }, [fetchDetectedWorktrees, gitOnHostCache, hostScopedRepos, hostScopingEnabled, selectedHostKey])

  const [name, setName] = useState<string>(
    persistDraft ? (newWorkspaceDraft?.name ?? initialName) : initialName
  )
  const [agentPrompt, setAgentPrompt] = useState<string>(
    persistDraft ? (newWorkspaceDraft?.prompt ?? initialPrompt) : initialPrompt
  )
  const [note, setNote] = useState<string>(persistDraft ? (newWorkspaceDraft?.note ?? '') : '')
  const [attachmentPaths, setAttachmentPaths] = useState<string[]>(
    persistDraft ? (newWorkspaceDraft?.attachments ?? []) : []
  )
  const [linkedWorkItem, setLinkedWorkItem] = useState<LinkedWorkItemSummary | null>(
    persistDraft
      ? (newWorkspaceDraft?.linkedWorkItem ?? initialLinkedWorkItem)
      : initialLinkedWorkItem
  )
  const [linkedIssue, setLinkedIssue] = useState<string>(() => {
    if (persistDraft && newWorkspaceDraft?.linkedIssue) {
      return newWorkspaceDraft.linkedIssue
    }
    if (
      initialLinkedWorkItem?.type === 'issue' &&
      !initialLinkedWorkItem.linearIdentifier &&
      !isGitLabIssueUrl(initialLinkedWorkItem.url)
    ) {
      return String(initialLinkedWorkItem.number)
    }
    return ''
  })
  const [linkedPR, setLinkedPR] = useState<number | null>(() => {
    if (persistDraft && newWorkspaceDraft?.linkedPR !== undefined) {
      return newWorkspaceDraft.linkedPR
    }
    return initialLinkedWorkItem?.type === 'pr' ? initialLinkedWorkItem.number : null
  })
  // Why: GitLab parallels of linkedIssue/linkedPR. Kept as separate state
  // (rather than reusing the GitHub slots with a provider discriminator) so
  // the existing GitHub auto-name / linked-badge / persistence code paths
  // stay untouched.
  const [linkedGitLabIssue, setLinkedGitLabIssue] = useState<number | null>(() => {
    if (persistDraft && newWorkspaceDraft?.linkedGitLabIssue !== undefined) {
      return newWorkspaceDraft.linkedGitLabIssue
    }
    return initialLinkedWorkItem?.type === 'issue' && isGitLabIssueUrl(initialLinkedWorkItem.url)
      ? initialLinkedWorkItem.number
      : null
  })
  const [linkedGitLabMR, setLinkedGitLabMR] = useState<number | null>(() => {
    if (persistDraft && newWorkspaceDraft?.linkedGitLabMR !== undefined) {
      return newWorkspaceDraft.linkedGitLabMR
    }
    return initialLinkedWorkItem?.type === 'mr' ? initialLinkedWorkItem.number : null
  })
  // Spec 004 F3: inline "Create GitHub issue" mini-form. Transient by design —
  // never persisted into the long-running draft (a half-typed issue is not
  // workspace state; the *linked* result is, via linkedWorkItem above).
  const [createIssueOpen, setCreateIssueOpen] = useState(false)
  const [createIssueTitle, setCreateIssueTitle] = useState('')
  const [createIssueBody, setCreateIssueBody] = useState('')
  const [createIssueSubmitting, setCreateIssueSubmitting] = useState(false)
  const [createIssueError, setCreateIssueError] = useState<string | null>(null)
  // Spec 007: "Generate description" — an LLM draft of the body from the
  // typed title + repo context. Fills the textarea for review, never files.
  const [createIssueGenerating, setCreateIssueGenerating] = useState(false)
  // Spec 006 F1: label picker for the create-issue form. Selection resets on
  // submit-success and on form close; options are `null` while loading and
  // fall back to the static `type/*`+`priority/*` set when the fetch errors.
  const [createIssueLabels, setCreateIssueLabels] = useState<string[]>([])
  const [createIssueLabelOptions, setCreateIssueLabelOptions] = useState<string[] | null>(null)
  // Spec 004 F4 (D5): opt-in, off by default. Not draft-persisted — trust in
  // the deterministic transform is earned per creation, not remembered.
  const [scaffoldSpec, setScaffoldSpec] = useState(false)
  // Spec 005 F1: "Start gated run" — the linked issue becomes the spec and the
  // Harness Engine drives gated agents in the new worktree. Armed up front by
  // the Tasks page row action (AC 3); like scaffoldSpec, never draft-persisted.
  const [startGatedRun, setStartGatedRun] = useState(Boolean(initialStartGatedRun))
  const [baseBranch, setBaseBranch] = useState<string | undefined>(
    persistDraft ? newWorkspaceDraft?.baseBranch : initialBaseBranch
  )
  const [branchNameOverride, setBranchNameOverride] = useState<string | undefined>(undefined)
  const [branchNameOverridePreservesNameEdits, setBranchNameOverridePreservesNameEdits] =
    useState(false)
  const [pushTarget, setPushTarget] = useState<GitPushTarget | undefined>(undefined)
  // Why: when a repo switch wipes a prior Start-from selection, surface the
  // reset inline (e.g. "was PR #8778") so the change is recoverable visually
  // instead of slipping past the user. Cleared on any subsequent selection.
  const [startFromResetHint, setStartFromResetHint] = useState<string | null>(null)
  const disabledTuiAgentKey = (settings?.disabledTuiAgents ?? []).join('\u0000')
  const disabledTuiAgents = useMemo<TuiAgent[]>(
    () => settings?.disabledTuiAgents ?? [],
    // Why: settings IPC round-trips clone arrays; agent availability only
    // changes when the disabled-agent content changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [disabledTuiAgentKey]
  )
  // Why: the long-form composer's agent selection is a required TuiAgent (not
  // null/blank), so 'blank' preferences from global settings must collapse to
  // the Claude default here — the blank-terminal affordance only lives in the
  // quick-create flow.
  const enabledCatalogAgents = useMemo(
    () =>
      filterEnabledTuiAgents(
        AGENT_CATALOG.map((agent) => agent.id),
        disabledTuiAgents
      ),
    [disabledTuiAgents]
  )
  const fallbackDefaultAgent: TuiAgent =
    settings?.defaultTuiAgent &&
    settings.defaultTuiAgent !== 'blank' &&
    isTuiAgentEnabled(settings.defaultTuiAgent, disabledTuiAgents)
      ? settings.defaultTuiAgent
      : (enabledCatalogAgents[0] ?? 'claude')
  const [tuiAgent, setTuiAgent] = useState<TuiAgent>(
    persistDraft ? (newWorkspaceDraft?.agent ?? fallbackDefaultAgent) : fallbackDefaultAgent
  )
  // Why: when the selected repo is remote (has a connectionId), read the
  // per-connection agent list instead of the local one. This ensures the
  // Create Workspace dialog shows agents installed on the SSH host, not the
  // local machine.
  const connectionId = selectedRepoConnectionId
  const isRemote = typeof connectionId === 'string'
  const detectedAgentList = useAppStore((s) => {
    if (isRemote) {
      return s.remoteDetectedAgentIds[connectionId] ?? null
    }
    return s.detectedAgentIds
  })
  const ensureDetectedAgents = useAppStore((s) => s.ensureDetectedAgents)
  const ensureRemoteDetectedAgents = useAppStore((s) => s.ensureRemoteDetectedAgents)
  const detectedAgentIds = useMemo<Set<TuiAgent> | null>(
    () => (detectedAgentList ? new Set(detectedAgentList) : null),
    [detectedAgentList]
  )

  const [yamlHooks, setYamlHooks] = useState<AgentumHooks | null>(null)
  const [checkedHooksRepoId, setCheckedHooksRepoId] = useState<string | null>(null)
  const [issueCommandTemplate, setIssueCommandTemplate] = useState('')
  const [hasLoadedIssueCommand, setHasLoadedIssueCommand] = useState(false)
  const [setupDecision, setSetupDecision] = useState<'run' | 'skip' | null>(null)
  const [creating, setCreating] = useState(false)
  const [createError, setCreateError] = useState<WorkspaceCreateErrorDisplay | null>(null)
  // Why: "create the worktree only, don't start a tmux session/agent right now".
  // When set, create + reveal the worktree but skip activation+launch; opening it
  // later runs the normal activation (its remembered agent, or the picker). Held
  // in a ref so the submit callbacks read the latest value without dep churn.
  const [skipSession, setSkipSession] = useState(false)
  const skipSessionRef = useRef(skipSession)
  skipSessionRef.current = skipSession
  const [advancedOpen, setAdvancedOpen] = useState(
    persistDraft ? Boolean((newWorkspaceDraft?.note ?? '').trim()) : false
  )
  const [sparseEnabled, setSparseEnabled] = useState(false)
  const [sparseDirectories, setSparseDirectories] = useState('')
  const [sparseSelectedPresetId, setSparseSelectedPresetId] = useState<string | null>(null)

  const [linkPopoverOpen, setLinkPopoverOpen] = useState(false)
  const [linkQuery, setLinkQuery] = useState('')
  const [linkDebouncedQuery, setLinkDebouncedQuery] = useState('')
  const [linkItems, setLinkItems] = useState<GitHubWorkItem[]>([])
  const [linkItemsLoading, setLinkItemsLoading] = useState(false)
  const [linkDirectItem, setLinkDirectItem] = useState<GitHubWorkItem | null>(null)
  const [linkDirectLoading, setLinkDirectLoading] = useState(false)

  const lastAutoNameRef = useRef<string>(
    persistDraft ? (newWorkspaceDraft?.name ?? initialName) : initialName
  )
  const branchAutoNameRef = useRef<string>('')
  // Why: tracks the note value we auto-prefilled from a Start-from PR pick, so
  // a subsequent PR change can replace it without clobbering user-typed text.
  const lastAutoNoteRef = useRef<string>('')
  // Why: read the latest note inside handleBaseBranchPrSelect without adding
  // `note` to its deps (which would rebuild the callback on every keystroke).
  const noteRef = useRef<string>(note)
  noteRef.current = note
  const composerRef = useRef<HTMLDivElement | null>(null)
  const promptTextareaRef = useRef<HTMLTextAreaElement | null>(null)
  const promptCaretFrameRef = useRef<number | null>(null)
  const nameInputRef = useRef<HTMLInputElement | null>(null)
  // Why: the native-file-drop effect below subscribes once on mount and must
  // read the latest agentPrompt when computing the caret-scoped insertion.
  // Mirror the value into a ref so the listener sees fresh state without
  // re-subscribing (which would reorder the composerDropStack and break
  // multi-instance routing).
  const agentPromptRef = useRef(agentPrompt)
  agentPromptRef.current = agentPrompt
  const connectionIdRef = useRef(connectionId)
  connectionIdRef.current = connectionId
  const selectedRepoConnectionIdRef = useRef(selectedRepoConnectionId)
  selectedRepoConnectionIdRef.current = selectedRepoConnectionId

  // Why: resolves the selected repo's owner/repo slug so a PR URL pasted
  // into the workspace name field can be matched against the current repo.
  // Pasting a PR URL from a different repo would otherwise recover only the
  // PR number, mislinking the worktree to an unrelated PR with the same
  // number in the selected repo.
  const [selectedRepoSlug, setSelectedRepoSlug] = useState<{ owner: string; repo: string } | null>(
    null
  )
  const selectedRepoPath = selectedRepo?.path
  const selectedRepoPathRef = useRef<string | undefined>(selectedRepoPath)
  selectedRepoPathRef.current = selectedRepoPath
  const settingsRef = useRef(settings)
  settingsRef.current = settings

  const cancelPromptCaretFrame = useCallback((): void => {
    if (promptCaretFrameRef.current === null) {
      return
    }
    cancelAnimationFrame(promptCaretFrameRef.current)
    promptCaretFrameRef.current = null
  }, [])

  const handleComposerNodeChange = useCallback(
    (node: HTMLDivElement | null): void => {
      // Why: the queued caret restoration targets composer descendants and
      // must be canceled as soon as the composer root leaves the DOM.
      if (!node) {
        cancelPromptCaretFrame()
      }
    },
    [cancelPromptCaretFrame]
  )

  const hookCheckRef = useRef<{
    key: string
    promise: Promise<HookCheckResult>
  } | null>(null)
  const loadHookCheckForRepo = useCallback((targetRepoId: string): Promise<HookCheckResult> => {
    const key = `${settingsRef.current?.activeRuntimeEnvironmentId ?? 'local'}:${targetRepoId}`
    const existing = hookCheckRef.current
    if (existing?.key === key) {
      return existing.promise
    }
    const promise = checkRuntimeHooks(settingsRef.current, targetRepoId)
    hookCheckRef.current = { key, promise }
    return promise
  }, [])
  const commitHookCheckIfCurrent = useCallback(
    (targetRepoId: string, hooks: AgentumHooks | null): boolean => {
      if (repoIdRef.current !== targetRepoId) {
        return false
      }
      setYamlHooks(hooks)
      setCheckedHooksRepoId(targetRepoId)
      return true
    },
    []
  )
  useEffect(() => {
    if (!selectedRepo || !selectedRepoPath || !selectedRepoIsGit) {
      setSelectedRepoSlug(null)
      return
    }
    let cancelled = false
    void (
      api.gh.repoSlug({ repoPath: selectedRepoPath, repoId }) as Promise<{
        owner: string
        repo: string
      } | null>
    )
      .then((result) => {
        if (cancelled) {
          return
        }
        setSelectedRepoSlug(result)
      })
      .catch(() => {
        if (!cancelled) {
          setSelectedRepoSlug(null)
        }
      })
    return () => {
      cancelled = true
    }
  }, [repoId, selectedRepo, selectedRepoIsGit, selectedRepoPath])
  const sparsePresetsForRepo = sparsePresetsByRepo[repoId]
  const sparsePresets = sparsePresetsForRepo ?? EMPTY_SPARSE_PRESETS
  const normalizedSparseDirectories = useMemo(
    () => normalizeSparseDirectoryLines(sparseDirectories),
    [sparseDirectories]
  )
  // Why: a preset attribution should only ride along if what's about to be
  // created actually equals the saved preset. If the user picked a preset and
  // then edited the textarea, we want the worktree to be a "Custom" sparse
  // checkout — not falsely tagged as the original preset.
  const effectivePresetId = useMemo(() => {
    if (!sparseSelectedPresetId) {
      return null
    }
    const selected = sparsePresets.find((preset) => preset.id === sparseSelectedPresetId)
    if (!selected) {
      return null
    }
    return sparseDirectoriesMatch(selected.directories, normalizedSparseDirectories)
      ? selected.id
      : null
  }, [normalizedSparseDirectories, sparsePresets, sparseSelectedPresetId])

  const sparseError = useMemo(() => {
    if (!sparseEnabled) {
      return null
    }
    if (!selectedRepoIsGit) {
      return null
    }
    if (selectedRepo?.connectionId) {
      return 'Sparse checkout is only supported for local repos right now.'
    }
    if (normalizedSparseDirectories.length === 0) {
      return 'Enter at least one repo-relative directory.'
    }
    if (
      normalizedSparseDirectories.some((entry) => entry === '.' || entry.split('/').includes('..'))
    ) {
      return 'Use repo-relative directories, not root or parent paths.'
    }
    return null
  }, [normalizedSparseDirectories, selectedRepo?.connectionId, selectedRepoIsGit, sparseEnabled])
  const parsedLinkedIssueNumber = useMemo(
    () => (linkedIssue.trim() ? parseGitHubIssueOrPRNumber(linkedIssue) : null),
    [linkedIssue]
  )
  // Why: when the user pastes a PR URL straight into the workspace name field
  // (without picking from the source picker), `linkedPR` stays null and the
  // worktree card has no PR strip. Recover the PR number from the name on
  // submit so create-from-PR worktrees always link back to their PR.
  const effectiveLinkedPR = useMemo<number | null>(() => {
    if (linkedPR !== null) {
      return linkedPR
    }
    const fromName = parseGitHubIssueOrPRLink(name)
    if (fromName && fromName.type === 'pr') {
      // Why: only adopt a number when the URL's owner/repo matches the
      // selected repo. Pasting `github.com/other/repo/pull/1234` must not
      // mislink the worktree to an unrelated PR #1234 in the current repo.
      // If the slug hasn't resolved yet, suppress recovery rather than
      // risking a cross-repo mislink.
      if (
        selectedRepoSlug &&
        fromName.slug.owner.toLowerCase() === selectedRepoSlug.owner.toLowerCase() &&
        fromName.slug.repo.toLowerCase() === selectedRepoSlug.repo.toLowerCase()
      ) {
        return fromName.number
      }
    }
    return null
  }, [linkedPR, name, selectedRepoSlug])
  const setupConfig = useMemo(
    () => (selectedRepoIsGit ? getSetupConfig(selectedRepo, yamlHooks) : null),
    [selectedRepo, selectedRepoIsGit, yamlHooks]
  )
  const setupPolicy: SetupRunPolicy = selectedRepo?.hookSettings?.setupRunPolicy ?? 'run-by-default'
  // Why: the "no prompt + linked item" path below rehydrates the issueCommand
  // template into the main startup prompt. When that happens we suppress the
  // separate split pane that would otherwise run the same command twice.
  const willApplyIssueCommandAsPrompt =
    enableIssueAutomation && !agentPrompt.trim() && Boolean(linkedWorkItem)
  const shouldWaitForIssueAutomationCheck =
    enableIssueAutomation &&
    (parsedLinkedIssueNumber !== null || willApplyIssueCommandAsPrompt) &&
    !hasLoadedIssueCommand
  const requiresExplicitSetupChoice = Boolean(setupConfig) && setupPolicy === 'ask'
  const resolvedSetupDecision =
    setupDecision ??
    (!setupConfig || setupPolicy === 'ask'
      ? null
      : setupPolicy === 'run-by-default'
        ? 'run'
        : 'skip')
  const isSetupCheckPending = Boolean(repoId) && checkedHooksRepoId !== repoId
  const shouldWaitForSetupCheck = Boolean(selectedRepo) && selectedRepoIsGit && isSetupCheckPending

  // Why: when the user leaves the workspace name blank and provides no other
  // seed source (prompt, linked issue/PR), pick a globally-unique marine
  // creature name so the workspace gets a distinct, readable identifier
  // instead of colliding on a literal "workspace" default — or on the same
  // creature already used in another repo.
  const fallbackCreatureName = useMemo(
    () => getSuggestedCreatureName(worktreesByRepo),
    [worktreesByRepo]
  )
  const workspaceSeedName = useMemo(
    () =>
      getWorkspaceSeedName({
        explicitName: name,
        prompt: agentPrompt,
        linkedIssueNumber: parsedLinkedIssueNumber,
        linkedPR,
        linkedTitle: linkedWorkItem?.title ?? null,
        fallbackName: fallbackCreatureName
      }),
    [agentPrompt, fallbackCreatureName, linkedPR, linkedWorkItem, name, parsedLinkedIssueNumber]
  )
  // Why: when the user links an issue/PR but has not typed any prompt text
  // (attachments don't count), swap the generic "Linked work items:" context
  // block for the repo's issueCommand template — or the built-in
  // "Complete {{artifact_url}}" default when none is configured. This makes
  // the common "paste a link and hit enter" flow produce a useful agent task
  // instead of a bare URL bullet.
  const shouldApplyLinkedOnlyTemplate =
    enableIssueAutomation && !agentPrompt.trim() && Boolean(linkedWorkItem) && hasLoadedIssueCommand
  const linkedOnlyTemplatePrompt = useMemo(() => {
    if (!shouldApplyLinkedOnlyTemplate || !linkedWorkItem) {
      return ''
    }
    const template = issueCommandTemplate.trim() || DEFAULT_ISSUE_COMMAND_TEMPLATE
    return renderIssueCommandTemplate(template, {
      issueNumber: linkedWorkItem.type === 'issue' ? linkedWorkItem.number : null,
      artifactUrl: linkedWorkItem.url
    })
  }, [issueCommandTemplate, linkedWorkItem, shouldApplyLinkedOnlyTemplate])
  const normalizedLinkQuery = useMemo(
    () => normalizeGitHubLinkQuery(linkDebouncedQuery),
    [linkDebouncedQuery]
  )

  const filteredLinkItems = useMemo(() => {
    if (normalizedLinkQuery.directNumber !== null) {
      return linkDirectItem ? [linkDirectItem] : []
    }

    const query = normalizedLinkQuery.query.trim().toLowerCase()
    if (!query) {
      return linkItems
    }

    return linkItems.filter((item) => {
      const text = [
        item.type,
        item.number,
        item.title,
        item.author ?? '',
        item.labels.join(' '),
        item.branchName ?? '',
        item.baseRefName ?? ''
      ]
        .join(' ')
        .toLowerCase()
      return text.includes(query)
    })
  }, [linkDirectItem, linkItems, normalizedLinkQuery.directNumber, normalizedLinkQuery.query])

  // Persist draft whenever relevant fields change (full-page only).
  useEffect(() => {
    if (!persistDraft) {
      return
    }
    setNewWorkspaceDraft({
      repoId: repoId || null,
      name,
      prompt: agentPrompt,
      note,
      attachments: attachmentPaths,
      linkedWorkItem,
      agent: tuiAgent,
      linkedIssue,
      linkedPR,
      linkedGitLabIssue,
      linkedGitLabMR,
      ...(baseBranch !== undefined ? { baseBranch } : {})
    })
  }, [
    persistDraft,
    agentPrompt,
    attachmentPaths,
    baseBranch,
    linkedIssue,
    linkedPR,
    linkedGitLabIssue,
    linkedGitLabMR,
    linkedWorkItem,
    note,
    name,
    repoId,
    setNewWorkspaceDraft,
    tuiAgent
  ])

  // Auto-pick a repo for the current host when none is selected (or the prior
  // selection belongs to a different host). Host scoping keeps the repo picker
  // and the host selector in sync; with `repoIdOverride` set this reduces to the
  // original "pick the first eligible repo" behavior (hostScopedRepos ===
  // eligibleRepos and the override controls repoId anyway).
  useEffect(() => {
    if (repoIdOverride !== undefined) {
      return
    }
    const nextRepoId = resolveRepoIdForHost(hostScopedRepos, repoId)
    if (nextRepoId !== repoId) {
      setRepoId(nextRepoId)
    }
  }, [hostScopedRepos, repoId, repoIdOverride, setRepoId])

  // Why: the compact sparse dropdown is always visible under Advanced, so
  // presets must load before sparse mode is enabled.
  useEffect(() => {
    if (!repoId || !selectedRepoIsGit || selectedRepo?.connectionId) {
      return
    }
    if (sparsePresetsByRepo[repoId] !== undefined) {
      return
    }
    void fetchSparsePresets(repoId)
  }, [
    fetchSparsePresets,
    repoId,
    selectedRepo?.connectionId,
    selectedRepoIsGit,
    sparsePresetsByRepo
  ])

  // Why: detect agents for the selected repo. For local repos this runs once
  // on mount (deduped by the store). For remote repos it re-runs when the
  // selected repo changes so the agent list matches the SSH host.
  useEffect(() => {
    if (isRemote && selectedRepoSshStatus !== 'connected') {
      return
    }
    let cancelled = false
    const detect = isRemote ? ensureRemoteDetectedAgents(connectionId) : ensureDetectedAgents()
    void detect.then((ids) => {
      if (cancelled) {
        return
      }
      const enabledIds = filterEnabledTuiAgents(ids, disabledTuiAgents)
      if (!newWorkspaceDraft?.agent && !settings?.defaultTuiAgent && enabledIds.length > 0) {
        const firstInCatalogOrder = AGENT_CATALOG.find((a) => enabledIds.includes(a.id))
        if (firstInCatalogOrder) {
          setTuiAgent(firstInCatalogOrder.id)
        }
      } else if (!isTuiAgentEnabled(tuiAgent, disabledTuiAgents)) {
        const firstEnabledDetected = AGENT_CATALOG.find((a) => enabledIds.includes(a.id))
        setTuiAgent(firstEnabledDetected?.id ?? fallbackDefaultAgent)
      }
    })
    return () => {
      cancelled = true
    }
    // Why: re-run when connectionId changes (user picks a different repo) so
    // detection targets the correct host. Draft/settings deps are intentionally
    // excluded — detection is a best-effort PATH snapshot.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connectionId, isRemote, selectedRepoSshStatus, disabledTuiAgents])

  // Per-repo: load yaml hooks + issue command template.
  useEffect(() => {
    if (!repoId) {
      return
    }

    let cancelled = false
    setHasLoadedIssueCommand(false)
    setIssueCommandTemplate('')
    setYamlHooks(null)
    setCheckedHooksRepoId(null)

    if (!selectedRepoIsGit) {
      setHasLoadedIssueCommand(true)
      setCheckedHooksRepoId(repoId)
      return () => {
        cancelled = true
      }
    }

    void loadHookCheckForRepo(repoId)
      .then((result) => {
        if (!cancelled) {
          commitHookCheckIfCurrent(repoId, result.hooks)
        }
      })
      .catch(() => {
        if (!cancelled) {
          commitHookCheckIfCurrent(repoId, null)
        }
      })

    if (!enableIssueAutomation) {
      setHasLoadedIssueCommand(true)
      return () => {
        cancelled = true
      }
    }

    void readRuntimeIssueCommand(settings, repoId)
      .then((result) => {
        if (!cancelled) {
          setIssueCommandTemplate(result.effectiveContent ?? '')
          setHasLoadedIssueCommand(true)
        }
      })
      .catch(() => {
        if (!cancelled) {
          setIssueCommandTemplate('')
          setHasLoadedIssueCommand(true)
        }
      })

    return () => {
      cancelled = true
    }
  }, [
    commitHookCheckIfCurrent,
    enableIssueAutomation,
    loadHookCheckForRepo,
    repoId,
    selectedRepoIsGit,
    settings
  ])

  const onConnectSelectedRepo = useCallback(async (): Promise<void> => {
    const targetId = selectedRepoConnectionIdRef.current
    if (!targetId) {
      return
    }
    const liveState = useAppStore.getState()
    const liveRepo = liveState.repos.find((repo) => repo.id === repoIdRef.current)
    if (liveRepo?.connectionId !== targetId) {
      return
    }
    const liveStatus = liveState.sshConnectionStates.get(targetId)?.status ?? null
    if (liveStatus === 'connected' || isSshConnectInProgress(liveStatus)) {
      return
    }

    try {
      // The native `ssh_connect` command is an unported no-op (returns null), so
      // the old call left the composer stuck on "Not connected". Use the same
      // server-host path every other Connect surface uses (status bar, add-repo
      // step): it registers the SSH target as a server host, probes it over SSH,
      // and updates sshConnectionStates — which flips this card to "connected".
      const result = await connectSshTargetViaServer(targetId)
      if (!result.ok) {
        toast.error(result.message || 'Failed to connect to project.')
      }
    } catch (error) {
      toast.error(error instanceof Error ? error.message : 'Failed to connect to project.')
    }
  }, [])

  // Why: warm the Start-from picker's PR cache on composer mount and whenever
  // the selected repo changes so opening the picker paints instantly from
  // cache.
  const canPrefetchSelectedRepoWorkItems = canUseRepoBackedComposerSources({
    connectionId: selectedRepoConnectionId,
    status: selectedRepoSshStatus
  })
  const prefetchSshConnectedGeneration =
    selectedRepoConnectionId && selectedRepoSshStatus === 'connected' ? sshConnectedGeneration : 0
  useEffect(() => {
    if (!selectedRepoIsGit || !selectedRepo?.path || !canPrefetchSelectedRepoWorkItems) {
      return
    }
    prefetchWorkItems(selectedRepo.id, selectedRepo.path, PER_REPO_FETCH_LIMIT, 'is:pr is:open')
  }, [
    canPrefetchSelectedRepoWorkItems,
    prefetchSshConnectedGeneration,
    prefetchWorkItems,
    selectedRepo?.id,
    selectedRepo?.path,
    selectedRepoIsGit
  ])

  // Reset setup decision when config / policy changes.
  useEffect(() => {
    if (shouldWaitForSetupCheck) {
      setSetupDecision(null)
      return
    }
    if (!setupConfig) {
      setSetupDecision(null)
      return
    }
    if (setupPolicy === 'ask') {
      setSetupDecision(null)
      return
    }
    setSetupDecision(setupPolicy === 'run-by-default' ? 'run' : 'skip')
  }, [setupConfig, setupPolicy, shouldWaitForSetupCheck])

  // Link popover: debounce + load recent items + resolve direct number.
  useEffect(() => {
    const timeout = window.setTimeout(() => setLinkDebouncedQuery(linkQuery), 250)
    return () => window.clearTimeout(timeout)
  }, [linkQuery])

  useEffect(() => {
    if (!linkPopoverOpen || !selectedRepo || !selectedRepoIsGit) {
      return
    }

    let cancelled = false
    setLinkItemsLoading(true)

    const lookupRepoId = selectedRepo.id
    void api.gh
      .listWorkItems({ repoPath: selectedRepo.path, repoId: selectedRepo.id, limit: 100 })
      .then((envelope) => {
        if (!cancelled) {
          // Why: IPC payload omits repoId — stamp it here from the repo we
          // queried so downstream consumers typed against GitHubWorkItem work.
          // Cast through unknown: spreading a discriminated union loses the
          // discriminant, so the union-preserving shape must be asserted.
          // Why: the link popover intentionally does NOT surface
          // `envelope.errors?.issues`. Per-surface error copy lives in the
          // Tasks view (TaskPage) and the smart workspace-name field — a
          // partial-failure banner inside the small
          // @-mention popover would crowd the input and the user would
          // already see the same error on the originating Tasks page. If a
          // future UX decision flips this, add an error row to the popover's
          // render output.
          // Why: surface partial issues-side failures via devtools even though the
          // popover intentionally omits a UI banner (see rationale above). A user
          // hitting a 403 on a private upstream would otherwise see an empty popover
          // and no diagnostic trail.
          if (envelope.errors?.issues) {
            console.warn(
              '[composer/link] issues-side partial failure in @-mention popover:',
              envelope.errors.issues
            )
          }
          setLinkItems(
            envelope.items.map((it) => ({
              ...it,
              repoId: lookupRepoId
            })) as unknown as GitHubWorkItem[]
          )
        }
      })
      .catch(() => {
        if (!cancelled) {
          setLinkItems([])
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLinkItemsLoading(false)
        }
      })

    return () => {
      cancelled = true
    }
  }, [linkPopoverOpen, selectedRepo, selectedRepoIsGit])

  useEffect(() => {
    if (
      !linkPopoverOpen ||
      !selectedRepo ||
      !selectedRepoIsGit ||
      normalizedLinkQuery.directNumber === null
    ) {
      setLinkDirectItem(null)
      setLinkDirectLoading(false)
      return
    }

    let cancelled = false
    setLinkDirectLoading(true)
    // Why: Superset lets users paste a full GitHub URL or type a raw issue/PR
    // number and still get a concrete selectable result. Agentum mirrors that by
    // resolving direct lookups against the selected repo instead of requiring a
    // text match in the recent-items list.
    const lookupRepoId = selectedRepo.id
    void api.gh
      .workItem({
        repoPath: selectedRepo.path,
        repoId: selectedRepo.id,
        number: normalizedLinkQuery.directNumber
      })
      .then((item) => {
        if (!cancelled) {
          setLinkDirectItem(
            item ? ({ ...item, repoId: lookupRepoId } as unknown as GitHubWorkItem) : null
          )
        }
      })
      .catch(() => {
        if (!cancelled) {
          setLinkDirectItem(null)
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLinkDirectLoading(false)
        }
      })

    return () => {
      cancelled = true
    }
  }, [linkPopoverOpen, normalizedLinkQuery.directNumber, selectedRepo, selectedRepoIsGit])

  const applyLinkedWorkItem = useCallback(
    (item: GitHubWorkItem, options: { preserveBranchNameOverride?: boolean } = {}): void => {
      if (item.type === 'issue') {
        setLinkedIssue(String(item.number))
        setLinkedPR(null)
      } else {
        setLinkedIssue('')
        setLinkedPR(item.number)
      }
      setLinkedWorkItem({
        type: item.type,
        number: item.number,
        title: item.title,
        url: item.url
      })
      const suggestedName = getLinkedWorkItemSuggestedName(item)
      if (suggestedName && (!name.trim() || name === lastAutoNameRef.current)) {
        setName(suggestedName)
        lastAutoNameRef.current = suggestedName
      }
      if (!options.preserveBranchNameOverride) {
        setBranchNameOverride(undefined)
      }
    },
    [name]
  )

  const resolvePendingSmartGitHubSubmit =
    useCallback(async (): Promise<SmartGitHubSubmitResolution | null> => {
      if (linkedWorkItem || !selectedRepo || !selectedRepoIsGit) {
        return null
      }

      const intent = getSmartGitHubSubmitIntent(name)
      if (!intent) {
        return null
      }

      const item = await lookupSmartGitHubSubmitItem({
        repoPath: selectedRepo.path,
        repoId: selectedRepo.id,
        intent,
        workItem: (args) => api.gh.workItem(args) as Promise<GitHubWorkItem | null>,
        workItemByOwnerRepo: (args) =>
          api.gh.workItemByOwnerRepo(args) as Promise<GitHubWorkItem | null>
      })
      if (!item) {
        throw new Error('Could not resolve the GitHub item before creating the workspace.')
      }

      const resolution = getSmartGitHubSubmitResolution(item)
      // Why: Create can be clicked before the debounced smart field commits
      // its selected source. Commit the resolved item here so failures leave
      // the form showing the title instead of the raw URL.
      setLinkedIssue(
        resolution.linkedIssueNumber !== null ? String(resolution.linkedIssueNumber) : ''
      )
      setLinkedPR(resolution.linkedPR)
      setLinkedGitLabIssue(null)
      setLinkedGitLabMR(null)
      setLinkedWorkItem(resolution.linkedWorkItem)
      setName(resolution.workspaceName)
      lastAutoNameRef.current = resolution.workspaceName
      setBranchNameOverride(undefined)
      branchAutoNameRef.current = ''
      setStartFromResetHint(null)
      return resolution
    }, [linkedWorkItem, name, selectedRepo, selectedRepoIsGit])

  // Why: parallel of applyLinkedWorkItem for GitLab. Touches the GitLab
  // state slots only — the GitHub linkedIssue/linkedPR remain unchanged
  // so a workspace can in principle reference items from both providers.
  // The auto-name logic mirrors the GitHub side (issue: number-and-title,
  // MR: branch name) via getLinkedWorkItemSuggestedName, which already
  // accepts both shapes structurally.
  const applyLinkedGitLabWorkItem = useCallback(
    (item: GitLabWorkItem): void => {
      if (item.type === 'issue') {
        setLinkedGitLabIssue(item.number)
        setLinkedGitLabMR(null)
      } else {
        setLinkedGitLabIssue(null)
        setLinkedGitLabMR(item.number)
      }
      setLinkedWorkItem({
        type: item.type,
        number: item.number,
        title: item.title,
        url: item.url
      })
      // Why: GitLabWorkItem.branchName lines up with GitHubWorkItem.branchName
      // structurally; cast to the suggested-name helper's input shape so we
      // reuse the existing naming heuristic without forking it.
      const suggestedName = getLinkedWorkItemSuggestedName({
        type: item.type === 'mr' ? 'pr' : 'issue',
        number: item.number,
        title: item.title,
        branchName: item.branchName
      } as unknown as GitHubWorkItem)
      if (suggestedName && (!name.trim() || name === lastAutoNameRef.current)) {
        setName(suggestedName)
        lastAutoNameRef.current = suggestedName
      }
      setBranchNameOverride(undefined)
    },
    [name]
  )

  const handleSelectLinkedItem = useCallback(
    (item: GitHubWorkItem): void => {
      applyLinkedWorkItem(item)
      setLinkPopoverOpen(false)
      setLinkQuery('')
      setLinkDebouncedQuery('')
      setLinkDirectItem(null)
    },
    [applyLinkedWorkItem]
  )

  const handleLinkPopoverChange = useCallback((open: boolean): void => {
    setLinkPopoverOpen(open)
    if (!open) {
      setLinkQuery('')
      setLinkDebouncedQuery('')
      setLinkDirectItem(null)
    }
  }, [])

  const handleRemoveLinkedWorkItem = useCallback((): void => {
    setLinkedWorkItem(null)
    setLinkedIssue('')
    setLinkedPR(null)
    // Spec 007 (bug 2): the issue side-effect toggles are gated on the linked
    // issue; armed state must not outlive its (now hidden) checkbox, or the
    // submit-time gate silently no-ops.
    setScaffoldSpec(false)
    setStartGatedRun(false)
    if (name === lastAutoNameRef.current) {
      lastAutoNameRef.current = ''
    }
  }, [name])

  // Spec 004 F3: the affordance only renders when nothing is linked yet and
  // the selected repo is a *local git* repo — issue creation resolves the slug
  // from the local origin and runs the local `gh`.
  const canCreateGithubIssue = Boolean(
    !linkedWorkItem && selectedRepo && selectedRepoIsGit && !selectedRepo.connectionId
  )

  const handleCreateIssueOpenChange = useCallback(
    (open: boolean): void => {
      setCreateIssueOpen(open)
      setCreateIssueError(null)
      if (open) {
        // Pre-seed the title from what the user already typed (workspace name,
        // else the prompt's first line) so filing is one review + click.
        setCreateIssueTitle((current) => {
          if (current.trim()) {
            return current
          }
          return name.trim() || (agentPrompt.trim().split('\n')[0]?.trim() ?? '')
        })
      } else {
        // Spec 006 F1: a half-picked label set is form state, not draft state.
        setCreateIssueLabels([])
      }
    },
    [agentPrompt, name]
  )

  // Spec 006 F1 (D2): seed the label picker while the form is open — once per
  // open, refetched when the selected repo changes mid-form (the effect key).
  // ANY fetch error falls back to the static set; labels must never block
  // filing an issue.
  useEffect(() => {
    if (!createIssueOpen || !selectedRepoPath) {
      return
    }
    let cancelled = false
    setCreateIssueLabelOptions(null)
    fetchGithubRepoLabels({ workdir: selectedRepoPath })
      .catch(() => [...STATIC_FALLBACK_LABELS])
      .then((labels) => {
        if (!cancelled) {
          setCreateIssueLabelOptions(labels)
        }
      })
    return () => {
      cancelled = true
    }
  }, [createIssueOpen, selectedRepoPath])

  const handleToggleCreateIssueLabel = useCallback((label: string): void => {
    setCreateIssueLabels((current) =>
      current.includes(label) ? current.filter((l) => l !== label) : [...current, label]
    )
  }, [])

  const handleCreateIssueSubmit = useCallback(async (): Promise<LinkedWorkItemSummary | null> => {
    const title = createIssueTitle.trim()
    const repoPath = selectedRepo?.path
    if (createIssueSubmitting) {
      return null
    }
    if (!title) {
      setCreateIssueError('Give the issue a title.')
      return null
    }
    if (!repoPath) {
      setCreateIssueError('Pick a project first.')
      return null
    }
    setCreateIssueSubmitting(true)
    setCreateIssueError(null)
    try {
      // Spec 006 F1 (AC 3): a blank body auto-fills from the composer's
      // context already in hand (agent prompt + note, via the existing refs so
      // this callback's deps don't grow per keystroke). Both blank keeps
      // today's bodyless create.
      const body =
        createIssueBody.trim() ||
        (composeIssueContextBody(agentPromptRef.current, noteRef.current) ?? '')
      const labels = createIssueLabels
      const created = await createGithubIssue({
        title,
        ...(body ? { body } : {}),
        workdir: repoPath,
        ...(labels.length ? { labels } : {})
      })
      // Reuse the standard linked-item application (linkedIssue slot, suggested
      // workspace name), then overwrite linkedWorkItem to attach the typed body
      // as linked context — the body is in hand, so no refetch (mirrors
      // buildGithubIssueLinkedWorkItem's snapshot shape).
      applyLinkedWorkItem({
        type: 'issue',
        number: created.number,
        title,
        url: created.url,
        labels,
        author: created.author ?? null
      } as unknown as GitHubWorkItem)
      const summary: LinkedWorkItemSummary = {
        type: 'issue',
        number: created.number,
        title,
        url: created.url,
        labels,
        author: created.author ?? null
      }
      const confirmedSummary: LinkedWorkItemSummary = body
        ? {
              ...summary,
              linkedContext: {
                provider: 'github',
                version: 1,
                renderedText: buildGithubIssueContextSnapshot({
                  number: created.number,
                  title,
                  url: created.url,
                  body
                })
              }
            }
        : summary
      setLinkedWorkItem(confirmedSummary)
      setCreateIssueOpen(false)
      setCreateIssueTitle('')
      setCreateIssueBody('')
      setCreateIssueLabels([])
      return confirmedSummary
    } catch (error) {
      // Zero state change on failure — the form stays filled for a retry.
      setCreateIssueError(
        error instanceof Error ? error.message : 'Could not create the GitHub issue.'
      )
      return null
    } finally {
      setCreateIssueSubmitting(false)
    }
  }, [
    applyLinkedWorkItem,
    createIssueBody,
    createIssueLabels,
    createIssueSubmitting,
    createIssueTitle,
    selectedRepo
  ])

  // Spec 007: draft an SDD-shaped body from the title + repo context and put
  // it in the TEXTAREA — the user reviews/edits before filing; nothing is
  // posted from here. Failures render inline (`createIssueError`) and leave
  // the form usable; a missing chat credential surfaces the server's
  // "set ANTHROPIC_API_KEY / sign in to Claude" message verbatim.
  const handleGenerateIssueBody = useCallback(async (): Promise<void> => {
    const title = createIssueTitle.trim()
    const repoPath = selectedRepo?.path
    if (createIssueGenerating || createIssueSubmitting) {
      return
    }
    if (!title) {
      setCreateIssueError('Give the issue a title first — the description is drafted from it.')
      return
    }
    if (!repoPath) {
      setCreateIssueError('Pick a project first.')
      return
    }
    setCreateIssueGenerating(true)
    setCreateIssueError(null)
    try {
      const { body } = await draftGithubIssueBody({
        workdir: repoPath,
        title,
        slug: selectedRepoSlug
          ? `${selectedRepoSlug.owner}/${selectedRepoSlug.repo}`
          : undefined
      })
      setCreateIssueBody(body)
    } catch (error) {
      setCreateIssueError(
        error instanceof Error ? error.message : 'Could not generate a description.'
      )
    } finally {
      setCreateIssueGenerating(false)
    }
  }, [
    createIssueGenerating,
    createIssueSubmitting,
    createIssueTitle,
    selectedRepo,
    selectedRepoSlug
  ])

  // Spec 004 F4 (D5): the toggle only applies to a linked *github.com issue*
  // targeting a local git repo — the scaffold endpoint writes into the new
  // worktree's local path.
  const linkedGithubIssueLink = useMemo(
    () => (linkedWorkItem?.type === 'issue' ? parseGitHubIssueOrPRLink(linkedWorkItem.url) : null),
    [linkedWorkItem]
  )
  const canScaffoldSpec = Boolean(
    linkedGithubIssueLink?.type === 'issue' && selectedRepoIsGit && !selectedRepo?.connectionId
  )

  // Spec 006 F3 (AC 8): fetch whether gated runs use the SDD role loop when
  // the gated-run toggle becomes available. Best-effort with an optimistic
  // `true` on failure — honest, since the server default is ON. Cancellation
  // guard so a stale response can't land after the toggle re-hid.
  const [sddRolesEnabled, setSddRolesEnabled] = useState(true)
  useEffect(() => {
    if (!canScaffoldSpec) {
      return
    }
    let cancelled = false
    void getHarnessSettings()
      .then((settings) => {
        if (!cancelled) {
          setSddRolesEnabled(settings.sddRolesEnabled)
        }
      })
      .catch(() => {
        if (!cancelled) {
          setSddRolesEnabled(true)
        }
      })
    return () => {
      cancelled = true
    }
  }, [canScaffoldSpec])

  const handleNameValueChange = useCallback(
    (nextName: string): void => {
      // Why: linked GitHub items should keep refreshing the suggested workspace
      // name only while the current value is still auto-managed. As soon as the
      // user edits the field by hand, later issue/PR selections must stop
      // clobbering it until they clear the field again.
      if (!nextName.trim()) {
        lastAutoNameRef.current = ''
      } else if (name !== lastAutoNameRef.current) {
        lastAutoNameRef.current = ''
      }
      if (
        branchNameOverride &&
        !branchNameOverridePreservesNameEdits &&
        nextName !== branchAutoNameRef.current
      ) {
        setBranchNameOverride(undefined)
        branchAutoNameRef.current = ''
      }
      setName(nextName)
      setCreateError(null)
    },
    [branchNameOverride, branchNameOverridePreservesNameEdits, name]
  )

  const addComposerAttachments = useCallback((paths: string[]): void => {
    if (paths.length === 0) {
      return
    }
    setAttachmentPaths((current) => {
      const next = [...current]
      for (const pathValue of paths) {
        if (!next.includes(pathValue)) {
          next.push(pathValue)
        }
      }
      return next
    })
  }, [])

  const insertComposerFolderPaths = useCallback(
    (folderPaths: string[]): void => {
      if (folderPaths.length === 0) {
        return
      }
      // Why: de-dup within a single drop — the OS occasionally delivers the
      // same folder twice when a user drags from a selection that includes both
      // the item and its parent, and we don't want to insert it multiple times.
      const uniqueFolderPaths = Array.from(new Set(folderPaths))
      // Why: wrap paths containing shell metacharacters in double quotes (and
      // escape embedded quotes) so inserted folder refs stay a single token if
      // pasted into a terminal. Simple paths stay unadorned to match OS drops.
      const formatPath = (p: string): string => {
        if (/[\s"'$`\\()[\]{}*?!;&|<>#~]/.test(p)) {
          return `"${p.replace(/(["\\$`])/g, '\\$1')}"`
        }
        return p
      }
      const insertion = uniqueFolderPaths.map(formatPath).join(' ')
      const textarea = promptTextareaRef.current
      // Why: compute selection, insertion, and caret target OUTSIDE the
      // setAgentPrompt updater so the updater stays pure. React Strict Mode
      // double-invokes updaters in dev, and batching can delay execution.
      const current = agentPromptRef.current
      const selStart = textarea?.selectionStart ?? current.length
      const selEnd = textarea?.selectionEnd ?? current.length
      const before = current.slice(0, selStart)
      const after = current.slice(selEnd)
      // Why: pad with single spaces when the caret sits directly against other
      // text so the folder path doesn't merge into an adjacent word.
      const needsLeadingSpace = before.length > 0 && !/\s$/.test(before)
      const needsTrailingSpace = after.length > 0 && !/^\s/.test(after)
      const padded = `${needsLeadingSpace ? ' ' : ''}${insertion}${needsTrailingSpace ? ' ' : ''}`
      const caret = before.length + padded.length
      if (textarea) {
        cancelPromptCaretFrame()
        promptCaretFrameRef.current = requestAnimationFrame(() => {
          promptCaretFrameRef.current = null
          if (promptTextareaRef.current !== textarea || !textarea.isConnected) {
            return
          }
          textarea.focus()
          textarea.setSelectionRange(caret, caret)
        })
      }
      // Why: pass a plain value (not an updater) since `before`/`after` were
      // already resolved from `agentPromptRef.current`; this keeps the state
      // write side-effect-free under Strict-Mode double-render.
      setAgentPrompt(before + padded + after)
    },
    [cancelPromptCaretFrame]
  )

  const uploadComposerPaths = useCallback(
    async (
      sourcePaths: string[],
      targetSettings = settings,
      targetConnectionId = connectionId,
      targetRepoPath = selectedRepoPath
    ): Promise<{ filePaths: string[]; folderPaths: string[] } | null> => {
      if (!targetSettings?.activeRuntimeEnvironmentId?.trim() && !targetConnectionId) {
        return null
      }
      if (!targetRepoPath) {
        toast.error('No remote project path is available for attachments.')
        return { filePaths: [], folderPaths: [] }
      }
      const destinationDir = joinPath(targetRepoPath, '.agentum/drops')
      const { results } = await importExternalPathsToRuntime(
        {
          settings: targetSettings,
          worktreeId: targetRepoPath,
          worktreePath: targetRepoPath,
          connectionId: targetConnectionId ?? undefined
        },
        sourcePaths,
        destinationDir,
        { ensureDestinationDir: true }
      )
      const filePaths: string[] = []
      const folderPaths: string[] = []
      let skippedOrFailed = 0
      for (const result of results) {
        if (result.status !== 'imported') {
          skippedOrFailed += 1
          continue
        }
        if (result.kind === 'directory') {
          folderPaths.push(result.destPath)
        } else {
          filePaths.push(result.destPath)
        }
      }
      if (skippedOrFailed > 0) {
        toast.error('Some attachments could not be uploaded.')
      }
      return { filePaths, folderPaths }
    },
    [connectionId, selectedRepoPath, settings]
  )

  const handleAddAttachment = useCallback(async (): Promise<void> => {
    try {
      const selectedPath = await api.shell.pickAttachment()
      if (!selectedPath) {
        return
      }
      const uploaded = await uploadComposerPaths([selectedPath])
      if (uploaded) {
        addComposerAttachments(uploaded.filePaths)
        insertComposerFolderPaths(uploaded.folderPaths)
        return
      }
      addComposerAttachments([selectedPath])
    } catch (error) {
      const message = error instanceof Error ? error.message : 'Failed to add attachment.'
      toast.error(message)
    }
  }, [addComposerAttachments, insertComposerFolderPaths, uploadComposerPaths])

  const applyLocalComposerDrop = useCallback(
    async (paths: string[]): Promise<void> => {
      const fileAttachments: string[] = []
      const folderPaths: string[] = []
      for (const filePath of paths) {
        try {
          await api.fs.authorizeExternalPath({ targetPath: filePath })
          const stat = await api.fs.stat({ filePath })
          if (stat.isDirectory) {
            folderPaths.push(filePath)
          } else {
            fileAttachments.push(filePath)
          }
        } catch {
          // Skip paths we cannot authorize or stat.
        }
      }

      addComposerAttachments(fileAttachments)
      insertComposerFolderPaths(folderPaths)
    },
    [addComposerAttachments, insertComposerFolderPaths]
  )
  const addComposerAttachmentsRef = useRef(addComposerAttachments)
  addComposerAttachmentsRef.current = addComposerAttachments
  const insertComposerFolderPathsRef = useRef(insertComposerFolderPaths)
  insertComposerFolderPathsRef.current = insertComposerFolderPaths
  const uploadComposerPathsRef = useRef(uploadComposerPaths)
  uploadComposerPathsRef.current = uploadComposerPaths
  const applyLocalComposerDropRef = useRef(applyLocalComposerDrop)
  applyLocalComposerDropRef.current = applyLocalComposerDrop

  // Why: native OS file drops onto the composer are captured by the preload
  // bridge (see `data-native-file-drop-target="composer"` markers) and relayed
  // as a gesture-scoped IPC event. Files become attachments (matching the
  // manual picker behavior); folders are pasted inline at the textarea caret
  // so the user can reference them as working directories in their prompt
  // without attaching a path we can't embed as file content.
  const instanceIdRef = useRef<symbol>(Symbol('composer'))
  useEffect(() => {
    const instanceId = instanceIdRef.current
    composerDropStack.push(instanceId)
    const unsubscribe = api.ui.onFileDrop((data) => {
      if (data.target !== 'composer') {
        return
      }
      // Why: only the top-of-stack composer (most recently mounted) owns the
      // drop. Earlier subscribers stay bound to keep their own cleanup tidy
      // but short-circuit so the event doesn't double-apply when page+modal
      // are both alive.
      if (composerDropStack.at(-1) !== instanceId) {
        return
      }
      void (async () => {
        const uploaded = await uploadComposerPathsRef.current(
          data.paths,
          settingsRef.current,
          connectionIdRef.current,
          selectedRepoPathRef.current
        )
        if (uploaded) {
          addComposerAttachmentsRef.current(uploaded.filePaths)
          insertComposerFolderPathsRef.current(uploaded.folderPaths)
          return
        }
        await applyLocalComposerDropRef.current(data.paths)
      })()
    })
    return () => {
      unsubscribe()
      const idx = composerDropStack.lastIndexOf(instanceId)
      if (idx !== -1) {
        composerDropStack.splice(idx, 1)
      }
    }
  }, [])

  const handleRepoChange = useCallback(
    (value: string): void => {
      if (value === repoId) {
        setRepoId(value)
        return
      }
      // Why: capture a short descriptor of the prior Start-from selection so
      // the field can render an inline reset (e.g. "was PR #8778") after the
      // repo changes and the selection is wiped.
      let hint: string | null = null
      if (linkedWorkItem?.type === 'pr' && baseBranch) {
        hint = `was PR #${linkedWorkItem.number}`
      } else if (linkedWorkItem?.type === 'mr' && baseBranch) {
        // Why: GitLab MR convention is `!N`, not `#N` — match the
        // upstream UI so the reset hint is recognizable.
        hint = `was MR !${linkedWorkItem.number}`
      } else if (baseBranch) {
        hint = `was ${baseBranch}`
      }
      setRepoId(value)
      setLinkedIssue('')
      setLinkedPR(null)
      setLinkedGitLabIssue(null)
      setLinkedGitLabMR(null)
      setLinkedWorkItem(null)
      // Spec 007 (bug 2): the repo switch just wiped the linked issue, so the
      // issue side-effect toggles lose their subject — disarm them instead of
      // leaving armed state behind a hidden checkbox (silent no-op at submit).
      setScaffoldSpec(false)
      setStartGatedRun(false)
      setSparseEnabled(false)
      setSparseDirectories('')
      // Why: presets are repo-scoped, so a stale selection from the prior
      // repo would be meaningless after a repo switch.
      setSparseSelectedPresetId(null)
      // Why: the Start-from picker is repo-scoped, so any prior branch/PR
      // selection is meaningless in the new repo. Resetting to undefined
      // makes the field fall back to the new repo's effective base ref.
      setBaseBranch(undefined)
      setPushTarget(undefined)
      setBranchNameOverride(undefined)
      setStartFromResetHint(hint)
    },
    [baseBranch, linkedWorkItem, repoId, setRepoId]
  )

  const handleSparseSelectPreset = useCallback((preset: SparsePreset | null): void => {
    if (preset) {
      setSparseEnabled(true)
      setSparseDirectories(preset.directories.join('\n'))
      setSparseSelectedPresetId(preset.id)
    } else {
      setSparseEnabled(false)
      setSparseDirectories('')
      setSparseSelectedPresetId(null)
    }
  }, [])

  const handleBaseBranchChange = useCallback((next: string | undefined): void => {
    setBaseBranch(next)
    setPushTarget(undefined)
    setBranchNameOverride(undefined)
    branchAutoNameRef.current = ''
    setStartFromResetHint(null)
  }, [])

  const handleBaseBranchPrSelect = useCallback(
    (
      nextBaseBranch: string,
      item: GitHubWorkItem,
      nextPushTarget?: GitPushTarget,
      nextBranchNameOverride?: string
    ): void => {
      setBaseBranch(nextBaseBranch)
      setPushTarget(nextPushTarget)
      setBranchNameOverride(nextBranchNameOverride)
      setBranchNameOverridePreservesNameEdits(Boolean(nextBranchNameOverride))
      branchAutoNameRef.current = ''
      setStartFromResetHint(null)
      // Why: per spec, a PR selection in the Start-from picker is also a
      // linkedWorkItem assignment. Reuse applyLinkedWorkItem so auto-name and
      // linkedPR state stay in a single code path.
      applyLinkedWorkItem(item, { preserveBranchNameOverride: Boolean(nextBranchNameOverride) })
      // Why: starting a worktree from a PR is a strong hint for what the
      // worktree's comment should surface (`agentum worktree current`, sidebar).
      // Prefill the note if it's empty or still equal to a prior auto-fill, so
      // we don't overwrite anything the user has typed.
      if (item.type === 'pr') {
        const suggestedNote = `PR #${item.number} — ${item.title}`
        const currentNote = noteRef.current
        if (!currentNote.trim() || currentNote === lastAutoNoteRef.current) {
          setNote(suggestedNote)
          lastAutoNoteRef.current = suggestedNote
        }
      }
    },
    [applyLinkedWorkItem]
  )

  // Why: GitLab parallel of handleBaseBranchPrSelect. Same shape, same
  // semantics — except the note prefill uses GitLab's `!N` MR convention
  // so a glance at the worktree sidebar makes the provider obvious.
  const handleBaseBranchMrSelect = useCallback(
    (nextBaseBranch: string, item: GitLabWorkItem, nextPushTarget?: GitPushTarget): void => {
      setBaseBranch(nextBaseBranch)
      setPushTarget(nextPushTarget)
      setBranchNameOverride(undefined)
      branchAutoNameRef.current = ''
      setStartFromResetHint(null)
      applyLinkedGitLabWorkItem(item)
      if (item.type === 'mr') {
        const suggestedNote = `MR !${item.number} — ${item.title}`
        const currentNote = noteRef.current
        if (!currentNote.trim() || currentNote === lastAutoNoteRef.current) {
          setNote(suggestedNote)
          lastAutoNoteRef.current = suggestedNote
        }
      }
    },
    [applyLinkedGitLabWorkItem]
  )

  const handleSmartGitHubItemSelect = useCallback(
    (item: GitHubWorkItem): void => {
      setStartFromResetHint(null)
      setBranchNameOverride(undefined)
      branchAutoNameRef.current = ''
      const repoForItem = eligibleRepos.find((repo) => repo.id === item.repoId) ?? selectedRepo
      applyLinkedWorkItem(item)
      if (item.type !== 'pr' || !repoForItem) {
        setPushTarget(undefined)
        return
      }
      setPushTarget(undefined)
      const target = getActiveRuntimeTarget(settings)
      const resolvePrBase =
        target.kind === 'local'
          ? api.worktrees.resolvePrBase({
              repoId: repoForItem.id,
              prNumber: item.number,
              ...(item.branchName ? { headRefName: item.branchName } : {}),
              ...(item.isCrossRepository !== undefined
                ? { isCrossRepository: item.isCrossRepository }
                : {})
            })
          : callRuntimeRpc<GitHubPrStartPoint | { error: string }>(
              target,
              'worktree.resolvePrBase',
              {
                repo: repoForItem.id,
                prNumber: item.number,
                ...(item.branchName ? { headRefName: item.branchName } : {}),
                ...(item.isCrossRepository !== undefined
                  ? { isCrossRepository: item.isCrossRepository }
                  : {})
              },
              { timeoutMs: 30_000 }
            )
      void resolvePrBase
        .then((result) => {
          if ('error' in result) {
            setBaseBranch(undefined)
            setPushTarget(undefined)
            toast.error(result.error)
            return
          }
          handleBaseBranchPrSelect(
            result.baseBranch,
            item,
            result.pushTarget,
            result.branchNameOverride
          )
        })
        .catch((error: unknown) => {
          setBaseBranch(undefined)
          setPushTarget(undefined)
          toast.error(error instanceof Error ? error.message : 'Failed to resolve PR base.')
        })
    },
    [applyLinkedWorkItem, eligibleRepos, handleBaseBranchPrSelect, selectedRepo, settings]
  )

  // Why: GitLab parallel of handleSmartGitHubItemSelect. For a picked
  // MR, resolves the base branch via worktrees:resolveMrBase (which uses
  // refs/merge-requests/<iid>/head for fork MRs the same way the gh side
  // uses refs/pull/<N>/head). Issue selections short-circuit since
  // there's no branch-resolution step to run.
  const handleSmartGitLabItemSelect = useCallback(
    (item: GitLabWorkItem): void => {
      applyLinkedGitLabWorkItem(item)
      setStartFromResetHint(null)
      setBranchNameOverride(undefined)
      branchAutoNameRef.current = ''
      const repoForItem = eligibleRepos.find((repo) => repo.id === item.repoId) ?? selectedRepo
      if (item.type !== 'mr' || !repoForItem) {
        return
      }
      void api.worktrees
        .resolveMrBase({
          repoId: repoForItem.id,
          mrIid: item.number,
          ...(item.branchName ? { sourceBranch: item.branchName } : {}),
          ...(item.isCrossRepository !== undefined
            ? { isCrossRepository: item.isCrossRepository }
            : {})
        })
        .then((result) => {
          if ('error' in result) {
            return
          }
          handleBaseBranchMrSelect(result.baseBranch, item, result.pushTarget)
        })
    },
    [applyLinkedGitLabWorkItem, eligibleRepos, handleBaseBranchMrSelect, selectedRepo]
  )

  const handleSmartBranchSelect = useCallback(
    (refName: string, localBranchName: string): void => {
      const selection = resolveComposerBranchSelection({
        refName,
        localBranchName,
        currentName: name,
        lastAutoName: lastAutoNameRef.current
      })
      setBaseBranch(selection.baseBranch)
      setPushTarget(undefined)
      setStartFromResetHint(null)
      setBranchNameOverridePreservesNameEdits(false)
      if (selection.name !== undefined && selection.lastAutoName !== undefined) {
        setName(selection.name)
        lastAutoNameRef.current = selection.lastAutoName
        branchAutoNameRef.current = selection.branchAutoName
        setBranchNameOverride(selection.branchNameOverride)
      } else {
        setBranchNameOverride(selection.branchNameOverride)
        branchAutoNameRef.current = selection.branchAutoName
      }
    },
    [name]
  )

  const handleSmartLinearIssueSelect = useCallback(
    (issue: LinearIssue): void => {
      setLinkedIssue('')
      setLinkedPR(null)
      setLinkedWorkItem(buildLinearIssueLinkedWorkItem(issue))
      const suggestedName = issue.title
      if (!name.trim() || name === lastAutoNameRef.current) {
        setName(suggestedName)
        lastAutoNameRef.current = suggestedName
      }
      setBranchNameOverride(undefined)
      branchAutoNameRef.current = ''
      // Why: match the GitHub issue/PR flow by drafting linked context for
      // review instead of auto-submitting. Auto-filling the note here would
      // turn a source selection into user-authored instructions.
    },
    [name]
  )

  const handleClearSmartNameSelection = useCallback((): void => {
    setLinkedIssue('')
    setLinkedPR(null)
    setLinkedWorkItem(null)
    setBaseBranch(undefined)
    setPushTarget(undefined)
    setBranchNameOverride(undefined)
    branchAutoNameRef.current = ''
    setStartFromResetHint(null)
    if (name === lastAutoNameRef.current) {
      setName('')
      lastAutoNameRef.current = ''
    }
    if (noteRef.current === lastAutoNoteRef.current) {
      setNote('')
      lastAutoNoteRef.current = ''
    }
  }, [name])

  const smartNameSelection = useMemo<SmartWorkspaceNameSelection | null>(() => {
    if (linkedWorkItem) {
      const isLinear = linkedWorkItem.number === 0 && !linkedWorkItem.url.includes('github.com')
      const kind: SmartWorkspaceNameSelection['kind'] = isLinear
        ? 'linear'
        : linkedWorkItem.type === 'pr'
          ? 'github-pr'
          : 'github-issue'
      return {
        kind,
        label:
          isLinear || linkedWorkItem.number === 0
            ? linkedWorkItem.title
            : `#${linkedWorkItem.number} ${linkedWorkItem.title}`,
        url: linkedWorkItem.url,
        // Spec 006 F1 (AC 2): the created-issue chip renders its applied
        // labels. Only the composer's create path populates the summary's
        // labels, so linked pre-existing items are unaffected.
        ...(linkedWorkItem.labels?.length ? { labels: linkedWorkItem.labels } : {})
      }
    }
    if (baseBranch) {
      return { kind: 'branch', label: baseBranch }
    }
    return null
  }, [baseBranch, linkedWorkItem])

  const handleOpenAgentSettings = useCallback((): void => {
    openSettingsTarget({ pane: 'agents', repoId: null })
    openSettingsPage()
    closeModal()
  }, [closeModal, openSettingsPage, openSettingsTarget])

  const applyWorktreeMeta = useCallback(
    async (worktreeId: string, meta: Partial<WorktreeMeta>): Promise<void> => {
      if (Object.keys(meta).length === 0) {
        return
      }
      try {
        await updateWorktreeMeta(worktreeId, meta)
      } catch {
        console.error('Failed to update worktree meta after creation')
      }
    },
    [updateWorktreeMeta]
  )

  // Spec 004 F4: after createWorktree succeeds (both submit paths), write the
  // linked issue's spec into the new worktree when the user opted in (D5).
  // Non-fatal by contract: a scaffold failure must never roll back or block
  // the freshly created workspace.
  const maybeScaffoldSpecFromIssue = useCallback(
    async (
      worktree: { path: string },
      item: LinkedWorkItemSummary | null | undefined
    ): Promise<void> => {
      if (!scaffoldSpec) {
        return
      }
      // Re-derive the gate from the submitted item: github.com issues only,
      // local targets only (the endpoint writes to a local path). Spec 007:
      // an ARMED toggle that skips must say so — the silent return was bug 2.
      const gate = deriveIssueSideEffectGate(item ?? null, selectedRepo?.connectionId)
      if (gate.eligible === false) {
        toast.warning(describeIssueSideEffectSkip('scaffold-spec', gate.reason))
        return
      }
      try {
        await scaffoldSpecFromIssue({
          workdir: worktree.path,
          number: gate.number,
          slug: `${gate.slug.owner}/${gate.slug.repo}`,
          // Spec 021 (#379): thread the repo's tracker pin so the planned
          // backlog stamps the right provider ('auto'/absent = GitHub).
          ...(selectedRepo?.trackerProvider ? { tracker: selectedRepo.trackerProvider } : {})
        })
      } catch (error) {
        console.error('Failed to scaffold a spec from the linked issue', error)
        toast.error('Workspace created, but the spec scaffold failed.')
      }
    },
    [scaffoldSpec, selectedRepo]
  )

  // Spec 005 F1: with "Start gated run" armed, after createWorktree succeeds
  // (both submit paths) drive the server-side orchestration — converge-scaffold
  // + plan + Todo + register + run — against the new worktree. Mirrors
  // maybeScaffoldSpecFromIssue's non-fatal contract (AC 5): a failure toasts
  // but never rolls back the created workspace. `agent` is the composer's
  // selection, written into the run's agent_tool knob post-plan (D2).
  const maybeStartGatedRun = useCallback(
    async (
      worktree: { path: string },
      item: LinkedWorkItemSummary | null | undefined,
      agent: TuiAgent | null
    ): Promise<boolean> => {
      // Returns whether the engine actually took ownership of the worktree. The
      // caller uses this to decide whether to suppress the plain agent delivery:
      // a run that never started must NOT suppress it, or the worktree strands
      // on the empty "Start a session" picker with nothing driving it.
      if (!startGatedRun) {
        return false
      }
      // Re-derive the gate from the submitted item: github.com issues only,
      // local targets only (the start-work route writes to a local path).
      // Spec 007: an ARMED toggle that skips must say so (bug 2).
      const gate = deriveIssueSideEffectGate(item ?? null, selectedRepo?.connectionId)
      if (gate.eligible === false) {
        toast.warning(describeIssueSideEffectSkip('start-gated-run', gate.reason))
        return false
      }
      try {
        const result = await startGatedWork({
          workdir: worktree.path,
          number: gate.number,
          slug: `${gate.slug.owner}/${gate.slug.repo}`,
          ...(agent ? { agentTool: agent } : {}),
          // Spec 021 (#379): thread the repo's tracker pin so the run's
          // features + transitions aim at the right provider.
          ...(selectedRepo?.trackerProvider ? { tracker: selectedRepo.trackerProvider } : {})
        })
        // Only suppress the plain session when the engine actually took
        // ownership (a live run, or a fresh one with ≥1 planned feature). A
        // zero-feature plan has nothing to drive and would strand the worktree.
        const owns = gatedRunResultOwnsWorktree(result)
        if (result.alreadyRunning) {
          // The friendly state (C5), not an error: a live run already owns
          // this worktree and was left untouched.
          toast.info('A gated run is already driving this workspace.')
        } else if (!owns) {
          // The plan produced no features, so the engine has nothing to spawn
          // and would leave the worktree stranded on the empty "Start a
          // session" picker with no error. Fall back to a normal agent instead
          // of a silent empty worktree.
          toast.warning(
            'The gated run planned no work from this issue — opening a normal session instead.'
          )
        } else {
          // Spec 008 F1 §B.5: the composer navigates to the session view after
          // a successful start, so a drive-phase failure on the harness event
          // bus (init.sh, spawn, the readiness/settle timeouts) would otherwise
          // go unseen. Subscribe filtered by this run and toast the FIRST early
          // error. Best-effort: self-closes on the first error or a bounded
          // window, so a healthy run never holds the socket open.
          void subscribeHarnessRunErrors(result.harnessId, (message) => {
            toast.error(`Gated run failed: ${message}`)
          })
        }
        return owns
      } catch (error) {
        console.error('Failed to start the gated run', error)
        // Spec 008 F1 #5: surface the server's ApiError detail — request()
        // already appends `— {detail}` (e.g. "workdir does not exist", "could
        // not plan from the spec"), which is actionable; the generic string hid
        // it.
        const detail = error instanceof Error ? error.message.trim() : ''
        toast.error(
          detail
            ? `Workspace created, but the gated run could not start — opening a normal session instead: ${detail}`
            : 'Workspace created, but the gated run could not start — opening a normal session instead.'
        )
        // The engine did NOT take ownership: fall back to a plain agent so the
        // worktree isn't stranded empty (the "stuck on Start a session" bug).
        return false
      }
    },
    [startGatedRun, selectedRepo]
  )

  const submit = useCallback(async (): Promise<void> => {
    if (
      !repoId ||
      !workspaceSeedName ||
      !selectedRepo ||
      selectedRepoRequiresConnection ||
      shouldWaitForSetupCheck ||
      shouldWaitForIssueAutomationCheck ||
      (requiresExplicitSetupChoice && !setupDecision) ||
      sparseError !== null
    ) {
      // Spec 008 F1 #2: an ARMED gated run that trips a precondition must say
      // WHY (the #226 chat-origin `repoId: ''` edge) — a bare return was silent.
      if (startGatedRun) {
        const blocker = firstStartGatedRunBlocker({
          repoId,
          workspaceSeedName,
          hasSelectedRepo: Boolean(selectedRepo),
          selectedRepoRequiresConnection,
          shouldWaitForSetupCheck,
          shouldWaitForIssueAutomationCheck,
          requiresExplicitSetupChoice,
          hasSetupDecision: Boolean(setupDecision),
          sparseError
        })
        if (blocker) {
          toast.error(blocker)
        }
      }
      return
    }
    if (!isTuiAgentEnabled(tuiAgent, disabledTuiAgents)) {
      setTuiAgent(fallbackDefaultAgent)
      toast.error('Selected agent is disabled. Choose an enabled agent before creating.')
      return
    }

    setCreateError(null)
    setCreating(true)
    try {
      const smartGitHubResolution = await resolvePendingSmartGitHubSubmit()
      const submitLinkedWorkItem = smartGitHubResolution?.linkedWorkItem ?? linkedWorkItem
      const submitLinkedIssueNumber =
        smartGitHubResolution?.linkedIssueNumber ?? parsedLinkedIssueNumber
      const submitLinkedPR = smartGitHubResolution?.linkedPR ?? effectiveLinkedPR
      const workspaceName = smartGitHubResolution?.workspaceName ?? workspaceSeedName
      if (!workspaceName) {
        return
      }
      const submitShouldApplyLinkedOnlyTemplate =
        enableIssueAutomation &&
        !agentPrompt.trim() &&
        Boolean(submitLinkedWorkItem) &&
        hasLoadedIssueCommand
      const submitLinkedOnlyTemplatePrompt =
        submitShouldApplyLinkedOnlyTemplate && submitLinkedWorkItem
          ? renderIssueCommandTemplate(
              issueCommandTemplate.trim() || DEFAULT_ISSUE_COMMAND_TEMPLATE,
              {
                issueNumber:
                  submitLinkedWorkItem.type === 'issue' ? submitLinkedWorkItem.number : null,
                artifactUrl: submitLinkedWorkItem.url
              }
            )
          : ''
      const linkedPromptContext = getLinkedWorkItemPromptContext(submitLinkedWorkItem)
      const submitStartupPrompt = submitShouldApplyLinkedOnlyTemplate
        ? buildAgentPromptWithContext(
            submitLinkedOnlyTemplatePrompt,
            attachmentPaths,
            [],
            linkedPromptContext.linkedContextBlocks
          )
        : buildAgentPromptWithContext(
            agentPrompt,
            attachmentPaths,
            linkedPromptContext.linkedUrls,
            linkedPromptContext.linkedContextBlocks
          )
      // Spec 005 F1: armed AND eligible for the submitted item (github.com
      // issue, local repo). Suppresses the issueCommand automation at its
      // source (one of D2's three skips) and routes the post-create side
      // effect to the gated-run orchestration instead of the D5 scaffold.
      // Spec 007: the same pure gate the post-create callbacks re-derive.
      const submitGatedRun =
        startGatedRun &&
        deriveIssueSideEffectGate(submitLinkedWorkItem ?? null, selectedRepo?.connectionId)
          .eligible
      const submitShouldRunIssueAutomation =
        !submitGatedRun &&
        enableIssueAutomation &&
        submitLinkedIssueNumber !== null &&
        issueCommandTemplate.length > 0 &&
        !submitShouldApplyLinkedOnlyTemplate

      const setupTrustDecision = selectedRepoIsGit
        ? await ensureHooksConfirmed(useAppStore.getState(), repoId, 'setup')
        : 'skip'
      const effectiveSetupDecision: SetupDecision =
        setupTrustDecision === 'skip'
          ? 'skip'
          : ((resolvedSetupDecision ?? 'inherit') as SetupDecision)

      let issueCommandTrustDecision: 'run' | 'skip' = 'run'
      if (selectedRepoIsGit && submitShouldRunIssueAutomation) {
        issueCommandTrustDecision =
          setupTrustDecision === 'skip'
            ? 'skip'
            : await ensureHooksConfirmed(useAppStore.getState(), repoId, 'issueCommand')
      }

      const linkedLinearIssue = submitLinkedWorkItem?.linearIdentifier
      const effectiveBranchNameOverride = resolveComposerBranchNameOverrideForCreate({
        branchNameOverride,
        branchAutoName: branchAutoNameRef.current,
        workspaceName,
        preserveWorkspaceNameEdits: branchNameOverridePreservesNameEdits
      })
      // Spec 012: persist the tracker bind so the session-start reactor + PR
      // poller can drive the linked item's status. GitHub issue → URL; Linear →
      // identifier; PR/MR-linked or unlinked create binds nothing.
      const trackerBind = deriveTrackerBindCoords(submitLinkedWorkItem)
      const result = await createWorktree(
        repoId,
        workspaceName,
        selectedRepoIsGit ? baseBranch : undefined,
        effectiveSetupDecision,
        selectedRepoIsGit && sparseEnabled
          ? {
              directories: normalizedSparseDirectories,
              ...(effectivePresetId ? { presetId: effectivePresetId } : {})
            }
          : undefined,
        telemetrySource,
        smartGitHubResolution?.displayName ?? submitLinkedWorkItem?.title,
        submitLinkedIssueNumber ?? undefined,
        submitLinkedPR ?? undefined,
        pushTarget,
        tuiAgent,
        linkedLinearIssue,
        effectiveBranchNameOverride,
        resolvedInitialWorkspaceStatus,
        linkedGitLabMR ?? undefined,
        linkedGitLabIssue ?? undefined,
        undefined,
        trackerBind?.trackerProvider,
        trackerBind?.trackerUrl
      )
      const worktree = result.worktree

      const trimmedNote = note.trim()
      // Why: linked source metadata is already included in createWorktree.
      // Re-saving it here can trigger slow post-create PR push-target lookups.
      await applyWorktreeMeta(worktree.id, trimmedNote ? { comment: trimmedNote } : {})
      // Track whether the engine actually took ownership. Only a real start
      // suppresses the plain delivery below — a failed start-work falls back to
      // a normal session so the worktree isn't stranded on "Start a session".
      let gatedRunStarted = false
      if (startGatedRun) {
        // Spec 005 F1 (AC 1): the server converge-scaffolds + plans + runs the
        // engine — the D5 scaffold call is skipped when the toggle is armed.
        // Spec 007: routed on the ARMED state (not eligibility) so an
        // ineligible-but-armed run surfaces its skip reason instead of
        // falling into the scaffold branch as a silent no-op (bug 2).
        gatedRunStarted = await maybeStartGatedRun(worktree, submitLinkedWorkItem, tuiAgent)
      } else {
        // Spec 004 F4 (opt-in): write the linked issue's spec into the new
        // worktree before the agent opens, so it can start from the spec.
        await maybeScaffoldSpecFromIssue(worktree, submitLinkedWorkItem)
      }

      const issueCommand =
        submitShouldRunIssueAutomation && issueCommandTrustDecision === 'run'
          ? {
              command: renderIssueCommandTemplate(issueCommandTemplate, {
                issueNumber: submitLinkedIssueNumber,
                artifactUrl: submitLinkedWorkItem?.url ?? null
              })
            }
          : undefined
      // Why: the composer already let the user pick the agent — open it directly
      // instead of bouncing them to the "Start a session" picker to pick again.
      // openCreatedWorkspace launches the selected agent (delivering the typed
      // prompt as an editable draft) and only falls back to the picker when no
      // agent was chosen. Repo `setup`/`defaultTabs`/`issueCommand` still apply —
      // those are project config, not the agent. With a gated run armed (spec
      // 005 D2) every plain delivery is suppressed — the engine's sessions are
      // the only agents in the worktree.
      openCreatedWorkspace({
        worktreeId: worktree.id,
        agent: tuiAgent,
        prompt: submitStartupPrompt,
        setup: result.setup,
        defaultTabs: result.defaultTabs,
        issueCommand: submitGatedRun ? undefined : issueCommand,
        gatedRun: gatedRunStarted
      })
      setSidebarOpen(true)
      if (persistDraft) {
        clearNewWorkspaceDraft()
      }
      onCreated?.()
    } catch (error) {
      const formattedError = formatWorkspaceCreateError(error)
      setCreateError(formattedError)
      toast.error(getWorkspaceCreateErrorToastMessage(formattedError))
    } finally {
      setCreating(false)
    }
  }, [
    agentPrompt,
    attachmentPaths,
    baseBranch,
    branchNameOverride,
    branchNameOverridePreservesNameEdits,
    clearNewWorkspaceDraft,
    createWorktree,
    applyWorktreeMeta,
    maybeScaffoldSpecFromIssue,
    maybeStartGatedRun,
    startGatedRun,
    enableIssueAutomation,
    issueCommandTemplate,
    effectiveLinkedPR,
    hasLoadedIssueCommand,
    linkedGitLabIssue,
    linkedGitLabMR,
    linkedWorkItem,
    normalizedSparseDirectories,
    note,
    onCreated,
    parsedLinkedIssueNumber,
    persistDraft,
    pushTarget,
    repoId,
    requiresExplicitSetupChoice,
    resolvePendingSmartGitHubSubmit,
    resolvedSetupDecision,
    resolvedInitialWorkspaceStatus,
    selectedRepo,
    selectedRepoIsGit,
    selectedRepoRequiresConnection,
    settings?.agentCmdOverrides,
    setSidebarOpen,
    setupDecision,
    sparseEnabled,
    sparseError,
    effectivePresetId,
    telemetrySource,
    fallbackDefaultAgent,
    disabledTuiAgents,
    tuiAgent,
    shouldWaitForIssueAutomationCheck,
    shouldWaitForSetupCheck,
    workspaceSeedName
  ])

  const submitQuick = useCallback(
    async (requestedAgent: TuiAgent | null, options?: QuickSubmitOptions): Promise<void> => {
      const agent =
        requestedAgent && isTuiAgentEnabled(requestedAgent, disabledTuiAgents)
          ? requestedAgent
          : null
      const workspaceNameSeed = getWorkspaceSeedName({
        explicitName: name,
        prompt: '',
        linkedIssueNumber: options?.linkedWorkItem?.number ?? parsedLinkedIssueNumber,
        linkedPR,
        linkedTitle: options?.linkedWorkItem?.title ?? linkedWorkItem?.title ?? null,
        fallbackName: fallbackCreatureName
      })
      if (
        !repoId ||
        !workspaceNameSeed ||
        !selectedRepo ||
        selectedRepoRequiresConnection ||
        (requiresExplicitSetupChoice && !setupDecision) ||
        sparseError !== null
      ) {
        return
      }

      setCreateError(null)
      setCreating(true)
      let activeStage: NewWorkStage = 'worktree'
      try {
        const smartGitHubResolution = await resolvePendingSmartGitHubSubmit()
        const submitLinkedWorkItem =
          options?.linkedWorkItem ?? smartGitHubResolution?.linkedWorkItem ?? linkedWorkItem
        const submitLinkedIssueNumber =
          options?.linkedWorkItem?.number ?? smartGitHubResolution?.linkedIssueNumber ?? parsedLinkedIssueNumber
        const submitLinkedPR = smartGitHubResolution?.linkedPR ?? effectiveLinkedPR
        const workspaceName = smartGitHubResolution?.workspaceName ?? workspaceNameSeed
        if (!workspaceName) {
          return
        }

        let submitSetupConfig = setupConfig
        let submitResolvedSetupDecision = resolvedSetupDecision
        if (selectedRepoIsGit && checkedHooksRepoId !== repoId) {
          let hookCheck: HookCheckResult
          try {
            hookCheck = await loadHookCheckForRepo(repoId)
          } catch {
            hookCheck = { hasHooks: false, hooks: null, mayNeedUpdate: false }
          }
          if (!commitHookCheckIfCurrent(repoId, hookCheck.hooks)) {
            return
          }
          submitSetupConfig = getSetupConfig(selectedRepo, hookCheck.hooks)
          submitResolvedSetupDecision =
            setupDecision ??
            (!submitSetupConfig || setupPolicy === 'ask'
              ? null
              : setupPolicy === 'run-by-default'
                ? 'run'
                : 'skip')
        }
        if (selectedRepoIsGit && submitSetupConfig && setupPolicy === 'ask' && !setupDecision) {
          setAdvancedOpen(true)
          return
        }

        const trustDecision = selectedRepoIsGit
          ? await ensureHooksConfirmed(useAppStore.getState(), repoId, 'setup')
          : 'skip'
        const effectiveSetupDecision: SetupDecision =
          trustDecision === 'skip'
            ? 'skip'
            : ((submitResolvedSetupDecision ?? 'inherit') as SetupDecision)

        const linkedLinearIssue = submitLinkedWorkItem?.linearIdentifier
        const effectiveBranchNameOverride = resolveComposerBranchNameOverrideForCreate({
          branchNameOverride,
          branchAutoName: branchAutoNameRef.current,
          workspaceName,
          preserveWorkspaceNameEdits: branchNameOverridePreservesNameEdits
        })
        // Spec 021: the wizard's quick create must persist the tracker bind just
        // like the full submit path — otherwise a wizard-created issue's worktree
        // shows no status chip in the sidebar (bind dropped on create).
        const trackerBind = deriveTrackerBindCoords(submitLinkedWorkItem)
        options?.onProgress?.('worktree', 'active')
        const result: CreateWorktreeResult = options?.checkpoint?.worktreeResult ?? (await createWorktree(
          repoId,
          workspaceName,
          selectedRepoIsGit ? baseBranch : undefined,
          effectiveSetupDecision,
          selectedRepoIsGit && sparseEnabled
            ? {
                directories: normalizedSparseDirectories,
                ...(effectivePresetId ? { presetId: effectivePresetId } : {})
              }
            : undefined,
          telemetrySource,
          smartGitHubResolution?.displayName ?? submitLinkedWorkItem?.title,
          submitLinkedIssueNumber ?? undefined,
          submitLinkedPR ?? undefined,
          pushTarget,
          agent ?? undefined,
          linkedLinearIssue,
          effectiveBranchNameOverride,
          resolvedInitialWorkspaceStatus,
          linkedGitLabMR ?? undefined,
          linkedGitLabIssue ?? undefined,
          undefined,
          trackerBind?.trackerProvider,
          trackerBind?.trackerUrl
        ))
        if (!options?.checkpoint?.worktreeResult) {
          options?.onCheckpoint?.({
            ...options?.checkpoint,
            ...(submitLinkedWorkItem ? { linkedWorkItem: submitLinkedWorkItem } : {}),
            worktreeResult: result
          })
        }
        options?.onProgress?.('worktree', 'done')
        const worktree = result.worktree

        const trimmedNote = note.trim()
        await applyWorktreeMeta(worktree.id, trimmedNote ? { comment: trimmedNote } : {})
        // Spec 005 F1: only a gated run that ACTUALLY started suppresses the
        // plain session — a failed/ineligible start falls back to a normal
        // agent so the worktree isn't stranded on "Start a session" with
        // nothing driving it (spec 007: the pure gate is re-derived inside
        // maybeStartGatedRun, which returns whether the engine took ownership).
        let gatedRunStarted = false
        if (options?.executionMode === 'autopilot') {
          activeStage = 'spec'
          options.onProgress?.('spec', 'active')
          const gate = deriveIssueSideEffectGate(submitLinkedWorkItem ?? null, selectedRepo.connectionId)
          if (!gate.eligible) throw new Error(describeIssueSideEffectSkip('start-gated-run', gate.reason))
          activeStage = 'run'
          const started = await startGatedWork({
            workdir: worktree.path,
            number: gate.number,
            slug: `${gate.slug.owner}/${gate.slug.repo}`,
            ...(agent ? { agentTool: agent } : {}),
            ...(selectedRepo.trackerProvider ? { tracker: selectedRepo.trackerProvider } : {})
          })
          gatedRunStarted = gatedRunResultOwnsWorktree(started)
          if (!gatedRunStarted) {
            throw new Error('SDD Autopilot planned no work and did not take ownership of this workspace.')
          }
          options.onProgress?.('spec', 'done')
          options.onProgress?.('run', 'done')
        } else if (options?.executionMode === 'manual') {
          activeStage = 'spec'
          options.onProgress?.('spec', 'active')
          const gate = deriveIssueSideEffectGate(submitLinkedWorkItem ?? null, selectedRepo.connectionId)
          if (gate.eligible) {
            await scaffoldSpecFromIssue({
              workdir: worktree.path,
              number: gate.number,
              slug: `${gate.slug.owner}/${gate.slug.repo}`,
              plan: false,
              converge: true,
              ...(selectedRepo.trackerProvider ? { tracker: selectedRepo.trackerProvider } : {})
            })
          }
          options.onProgress?.('spec', 'done')
        } else if (startGatedRun) {
          // The server converge-scaffolds + plans + runs the engine; the D5
          // scaffold call is skipped when the toggle is armed (AC 1). Runs
          // before the skip-session branch so the gated run starts either way.
          // Spec 007: routed on the ARMED state so an ineligible-but-armed
          // run warns instead of silently no-oping (bug 2).
          gatedRunStarted = await maybeStartGatedRun(worktree, submitLinkedWorkItem, agent)
        } else {
          // Spec 004 F4 (opt-in): shared with the full submit path — runs before
          // the skip-session branch so the spec lands either way.
          await maybeScaffoldSpecFromIssue(worktree, submitLinkedWorkItem)
        }

        // Why: "Don't start a session" — the worktree is created (and remembers
        // its agent via createdWithAgent) but no tmux session/agent is launched
        // and we don't switch to it. It just appears in the sidebar; opening it
        // later runs the normal activation (its agent, or the empty-state picker).
        if (skipSessionRef.current) {
          useAppStore.getState().revealWorktreeInSidebar(worktree.id, { behavior: 'auto' })
          setSidebarOpen(true)
          if (persistDraft) {
            clearNewWorkspaceDraft()
          }
          onCreated?.()
          return
        }

        // Why: the user picked the agent in the quick-create form — open it
        // directly instead of landing on the redundant "Start a session" picker.
        // openCreatedWorkspace launches the selected agent (delivering any
        // linked/typed prompt as an editable draft) and only falls back to the
        // picker when no agent was chosen. Rich linked context wins over URL
        // fallback; typed-only Linear entries use the note.
        const { prompt: quickPrompt, draftPrompt: quickDraftPrompt } =
          resolveQuickCreateLinkedWorkItemPrompt(submitLinkedWorkItem, trimmedNote)
        openCreatedWorkspace({
          worktreeId: worktree.id,
          agent,
          prompt: quickDraftPrompt || quickPrompt,
          setup: result.setup,
          defaultTabs: result.defaultTabs,
          // Spec 005 D2: a gated run that took ownership suppresses the plain
          // deliveries — the engine's sessions are the only agents in the
          // worktree. A failed start falls through to a normal session.
          gatedRun: gatedRunStarted
        })
        if (options?.executionMode === 'manual') options.onProgress?.('run', 'done')
        setSidebarOpen(true)
        if (persistDraft) {
          clearNewWorkspaceDraft()
        }
        onCreated?.()
      } catch (error) {
        if (options?.executionMode === 'autopilot' && activeStage === 'run') {
          options.onProgress?.('spec', 'error')
        }
        options?.onProgress?.(activeStage, 'error')
        const formattedError = formatWorkspaceCreateError(error)
        setCreateError(formattedError)
        toast.error(getWorkspaceCreateErrorToastMessage(formattedError))
      } finally {
        setCreating(false)
      }
    },
    [
      applyWorktreeMeta,
      maybeScaffoldSpecFromIssue,
      maybeStartGatedRun,
      startGatedRun,
      baseBranch,
      branchNameOverride,
      branchNameOverridePreservesNameEdits,
      clearNewWorkspaceDraft,
      createWorktree,
      fallbackCreatureName,
      effectiveLinkedPR,
      linkedGitLabIssue,
      linkedGitLabMR,
      linkedPR,
      linkedWorkItem,
      name,
      normalizedSparseDirectories,
      note,
      onCreated,
      parsedLinkedIssueNumber,
      persistDraft,
      pushTarget,
      repoId,
      requiresExplicitSetupChoice,
      resolvePendingSmartGitHubSubmit,
      resolvedSetupDecision,
      resolvedInitialWorkspaceStatus,
      selectedRepo,
      selectedRepoIsGit,
      selectedRepoRequiresConnection,
      settings?.agentCmdOverrides,
      disabledTuiAgents,
      setSidebarOpen,
      setupDecision,
      sparseEnabled,
      sparseError,
      effectivePresetId,
      telemetrySource,
      checkedHooksRepoId,
      commitHookCheckIfCurrent,
      loadHookCheckForRepo,
      setupConfig,
      setupPolicy
    ]
  )

  // Repos whose path isn't a git repo on the selected host (the `detected` probe
  // came back non-authoritative) are surfaced disabled with a reason hint rather
  // than hidden, so the user can see why they can't pick them. Repos whose probe
  // hasn't resolved yet stay enabled-pending (isGitOnHost === null).
  const selectedHostLabel =
    eligibleHosts.find((host) => host.key === selectedHostKey)?.label ??
    (selectedHostKey === 'local' ? 'this machine' : 'this host')
  const disabledRepoIds = useMemo<Map<string, string>>(() => {
    const map = new Map<string, string>()
    if (!hostScopingEnabled) {
      return map
    }
    for (const repo of hostScopedRepos) {
      if (gitOnHostCache.get(gitOnHostCacheKey(selectedHostKey, repo.id)) === false) {
        map.set(repo.id, `not a git repository on ${selectedHostLabel}`)
      }
    }
    return map
  }, [
    gitOnHostCache,
    hostScopedRepos,
    hostScopingEnabled,
    selectedHostKey,
    selectedHostLabel
  ])

  const createGateInput = {
    repoId,
    workspaceSeedName,
    creating,
    shouldWaitForSetupCheck,
    shouldWaitForIssueAutomationCheck,
    requiresExplicitSetupChoice,
    hasSetupDecision: Boolean(setupDecision),
    selectedRepoRequiresConnection,
    sparseError
  }
  const createDisabled =
    createGateMode === 'quick'
      ? getQuickComposerCreateDisabled(createGateInput)
      : getFullComposerCreateDisabled(createGateInput)
  const cardProps: ComposerCardProps = {
    eligibleRepos,
    eligibleHosts,
    selectedHostKey,
    onHostChange: handleHostChange,
    hostScopedRepos,
    disabledRepoIds,
    repoId,
    selectedRepoIsGit,
    onRepoChange: handleRepoChange,
    name,
    onNameValueChange: handleNameValueChange,
    onSmartGitHubItemSelect: handleSmartGitHubItemSelect,
    onSmartGitLabItemSelect: handleSmartGitLabItemSelect,
    onSmartBranchSelect: handleSmartBranchSelect,
    onSmartLinearIssueSelect: handleSmartLinearIssueSelect,
    smartNameSelection,
    onClearSmartNameSelection: handleClearSmartNameSelection,
    agentPrompt,
    onAgentPromptChange: setAgentPrompt,
    linkedOnlyTemplatePreview: shouldApplyLinkedOnlyTemplate ? linkedOnlyTemplatePrompt : null,
    attachmentPaths,
    getAttachmentLabel,
    onAddAttachment: () => void handleAddAttachment(),
    onRemoveAttachment: (pathValue) =>
      setAttachmentPaths((current) => current.filter((currentPath) => currentPath !== pathValue)),
    linkedWorkItem,
    onRemoveLinkedWorkItem: handleRemoveLinkedWorkItem,
    applyLinkedWorkItem,
    canCreateGithubIssue,
    createIssueOpen,
    onCreateIssueOpenChange: handleCreateIssueOpenChange,
    createIssueTitle,
    onCreateIssueTitleChange: setCreateIssueTitle,
    createIssueBody,
    onCreateIssueBodyChange: setCreateIssueBody,
    createIssueSubmitting,
    createIssueError,
    onCreateIssueSubmit: handleCreateIssueSubmit,
    createIssueGenerating,
    onGenerateIssueBody: () => void handleGenerateIssueBody(),
    createIssueLabels,
    createIssueLabelOptions,
    onToggleCreateIssueLabel: handleToggleCreateIssueLabel,
    canScaffoldSpec,
    scaffoldSpec,
    onScaffoldSpecChange: setScaffoldSpec,
    // Spec 005 F1: identical eligibility derivation to the D5 scaffold toggle.
    canStartGatedRun: canScaffoldSpec,
    startGatedRun,
    onStartGatedRunChange: setStartGatedRun,
    sddRolesEnabled,
    linkPopoverOpen,
    onLinkPopoverOpenChange: handleLinkPopoverChange,
    linkQuery,
    onLinkQueryChange: setLinkQuery,
    filteredLinkItems,
    linkItemsLoading,
    linkDirectLoading,
    normalizedLinkQuery,
    onSelectLinkedItem: handleSelectLinkedItem,
    tuiAgent,
    onTuiAgentChange: setTuiAgent,
    detectedAgentIds,
    onOpenAgentSettings: handleOpenAgentSettings,
    advancedOpen,
    onToggleAdvanced: () => setAdvancedOpen((current) => !current),
    createDisabled,
    creating,
    onCreate: () => void submit(),
    baseBranch,
    onBaseBranchChange: handleBaseBranchChange,
    onBaseBranchPrSelect: handleBaseBranchPrSelect,
    onBaseBranchMrSelect: handleBaseBranchMrSelect,
    baseBranchLinkedPrNumber:
      linkedWorkItem?.type === 'pr' && baseBranch ? linkedWorkItem.number : null,
    selectedRepoPath: selectedRepo?.path ?? null,
    selectedRepoIsRemote: Boolean(selectedRepo?.connectionId),
    selectedRepoConnectionId,
    selectedRepoSshStatus,
    selectedRepoRequiresConnection,
    selectedRepoConnectInProgress,
    onConnectSelectedRepo,
    startFromResetHint,
    note,
    onNoteChange: setNote,
    skipSession,
    onSkipSessionChange: setSkipSession,
    setupConfig,
    requiresExplicitSetupChoice,
    setupDecision,
    onSetupDecisionChange: setSetupDecision,
    shouldWaitForSetupCheck,
    resolvedSetupDecision,
    createError,
    canUseSparseCheckout: selectedRepoIsGit && !selectedRepo?.connectionId,
    sparsePresets,
    sparseSelectedPresetId,
    onSparseSelectPreset: handleSparseSelectPreset
  }

  return {
    cardProps,
    composerRef,
    onComposerNodeChange: handleComposerNodeChange,
    promptTextareaRef,
    nameInputRef,
    submit,
    submitQuick,
    createDisabled
  }
}
