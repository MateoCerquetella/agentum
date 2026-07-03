/* eslint-disable max-lines -- Why: shared type definitions for all runtime RPC methods live in one file for discoverability and import simplicity. */
import type { AgentStatusEntry } from './agent-status-types'
import type {
  BaseRefSearchResult,
  BrowserCookieImportResult,
  BrowserSessionProfile,
  BrowserSessionProfileSource,
  GitWorktreeInfo,
  RemoveWorktreeResult,
  Repo,
  TabGroupLayoutNode,
  TerminalColorOverrides,
  TerminalLayoutSnapshot,
  TuiAgent,
  Worktree,
  WorktreeLineage,
  WorktreeLineageWarning
} from './types'
import type { TerminalPaneLayoutNode } from './types'
import type { RuntimeCapability } from './protocol-version'

type RuntimeGraphStatus = 'ready' | 'reloading' | 'unavailable'

export type RuntimeStatus = {
  runtimeId: string
  rendererGraphEpoch: number
  graphStatus: RuntimeGraphStatus
  authoritativeWindowId: number | null
  liveTabCount: number
  liveLeafCount: number
  // Why: optional so clients can read both new and pre-contract runtimes.
  // Absence is treated as protocol 0 by the compat evaluator.
  runtimeProtocolVersion?: number
  minCompatibleRuntimeClientVersion?: number
  capabilities?: RuntimeCapability[]
  hostPlatform?: NodeJS.Platform
  // COMPAT(runtimeStatusMobileAliases): added 2026-05-15 for mobile builds
  // that still read these names; new desktop/CLI code uses the fields above.
  protocolVersion?: number
  minCompatibleMobileVersion?: number
}

type CliRuntimeState =
  | 'not_running'
  | 'starting'
  | 'ready'
  | 'graph_not_ready'
  | 'stale_bootstrap'

type CliStatusResult = {
  app: {
    running: boolean
    pid: number | null
  }
  runtime: {
    state: CliRuntimeState
    reachable: boolean
    runtimeId: string | null
  }
  graph: {
    state: RuntimeGraphStatus | 'not_running' | 'starting'
  }
}

type RuntimeSyncedTab = {
  tabId: string
  worktreeId: string
  title: string | null
  activeLeafId: string | null
  layout: TerminalPaneLayoutNode | null
}

type RuntimeSyncedLeaf = {
  tabId: string
  worktreeId: string
  leafId: string
  paneRuntimeId: number
  ptyId: string | null
  paneTitle?: string | null
  title?: string | null
}

export type RuntimeSyncWindowGraph = {
  tabs: RuntimeSyncedTab[]
  leaves: RuntimeSyncedLeaf[]
  mobileSessionTabs?: RuntimeMobileSessionTabsSnapshot[]
}

type RuntimeMobileSessionTerminalTab = {
  type: 'terminal'
  id: string
  title: string
  parentTabId: string
  leafId: string
  ptyId?: string | null
  terminalTheme?: RuntimeMobileTerminalTheme
  agentStatus?: AgentStatusEntry | null
  launchAgent?: TuiAgent
  parentLayout?: TerminalLayoutSnapshot
  isActive: boolean
}

export type RuntimeMobileTerminalTheme = {
  mode: 'dark' | 'light'
  theme: TerminalColorOverrides
}

export type RuntimeMobileSessionMarkdownTab = {
  type: 'markdown'
  id: string
  title: string
  filePath: string
  relativePath: string
  language: 'markdown'
  mode: 'edit' | 'markdown-preview'
  isDirty: boolean
  isActive: boolean
  sourceFileId: string
  sourceFilePath: string
  sourceRelativePath: string
  documentVersion: string
}

export type RuntimeMobileSessionFileTab = {
  type: 'file'
  id: string
  title: string
  filePath: string
  relativePath: string
  language: string
  mode?: 'edit' | 'diff'
  diffSource?: 'staged' | 'unstaged'
  isDirty: boolean
  isActive: boolean
}

export type RuntimeMobileSessionBrowserTab = {
  type: 'browser'
  id: string
  title: string
  browserWorkspaceId: string
  browserPageId: string | null
  url: string
  loading: boolean
  canGoBack: boolean
  canGoForward: boolean
  isActive: boolean
}

export type RuntimeMobileSessionSnapshotTab =
  | RuntimeMobileSessionTerminalTab
  | RuntimeMobileSessionMarkdownTab
  | RuntimeMobileSessionFileTab
  | RuntimeMobileSessionBrowserTab

export type RuntimeMobileSessionTerminalClientTab =
  | (RuntimeMobileSessionTerminalTab & {
      status: 'pending-handle'
      terminal: null
    })
  | (RuntimeMobileSessionTerminalTab & {
      status: 'ready'
      terminal: string
    })

type RuntimeMobileSessionClientTab =
  | RuntimeMobileSessionTerminalClientTab
  | RuntimeMobileSessionMarkdownTab
  | RuntimeMobileSessionFileTab
  | RuntimeMobileSessionBrowserTab

export type RuntimeMobileSessionTabGroup = {
  id: string
  activeTabId: string | null
  tabOrder: string[]
  recentTabIds?: string[]
}

type RuntimeMobileSessionTabMoveBase = {
  tabId: string
  targetGroupId: string
}

export type RuntimeMobileSessionTabMove =
  | (RuntimeMobileSessionTabMoveBase & {
      kind: 'reorder'
      tabOrder: string[]
    })
  | (RuntimeMobileSessionTabMoveBase & {
      kind: 'move-to-group'
      index?: number
    })
  | (RuntimeMobileSessionTabMoveBase & {
      kind: 'split'
      splitDirection: 'left' | 'right' | 'up' | 'down'
    })

export type RuntimeMobileSessionTabMoveResult = {
  moved: true
}

export type RuntimeMobileSessionTabsSnapshot = {
  worktree: string
  publicationEpoch: string
  snapshotVersion: number
  activeGroupId: string | null
  activeTabId: string | null
  activeTabType: 'terminal' | 'markdown' | 'file' | 'browser' | null
  tabGroups?: RuntimeMobileSessionTabGroup[]
  tabGroupLayout?: TabGroupLayoutNode | null
  tabs: RuntimeMobileSessionSnapshotTab[]
}

export type RuntimeMobileSessionTabsResult = {
  worktree: string
  publicationEpoch: string
  snapshotVersion: number
  activeGroupId: string | null
  activeTabId: string | null
  activeTabType: 'terminal' | 'markdown' | 'file' | 'browser' | null
  tabGroups?: RuntimeMobileSessionTabGroup[]
  tabGroupLayout?: TabGroupLayoutNode | null
  tabs: RuntimeMobileSessionClientTab[]
}

export type RuntimeMobileSessionCreateTerminalResult = {
  tab: RuntimeMobileSessionTerminalClientTab
  publicationEpoch: string
  snapshotVersion: number
}

type RuntimeMobileSessionTabsRemovedResult = RuntimeMobileSessionTabsResult & {
  removed: true
  activeGroupId: null
  activeTabId: null
  activeTabType: null
  tabs: []
}

type RuntimeFileListEntry = {
  relativePath: string
  basename: string
  kind: 'text' | 'binary'
}

type RuntimeFileListResult = {
  worktree: string
  rootPath: string
  files: RuntimeFileListEntry[]
  totalCount: number
  truncated: boolean
}

type RuntimeFileOpenResult = {
  worktree: string
  relativePath: string
  kind: 'markdown' | 'text' | 'binary'
  opened: boolean
}

export type RuntimeFileReadResult = {
  worktree: string
  relativePath: string
  content: string
  truncated: boolean
  byteLength: number
}

export type RuntimeFilePreviewResult = {
  content: string
  isBinary: boolean
  isImage?: boolean
  mimeType?: string
}

type RuntimeTerminalSummary = {
  handle: string
  worktreeId: string
  worktreePath: string
  branch: string
  tabId: string
  leafId: string
  title: string | null
  connected: boolean
  writable: boolean
  lastOutputAt: number | null
  preview: string
}

export type RuntimeTerminalListResult = {
  terminals: RuntimeTerminalSummary[]
  totalCount: number
  truncated: boolean
}

type RuntimeTerminalShow = RuntimeTerminalSummary & {
  paneRuntimeId: number
  ptyId: string | null
  rendererGraphEpoch: number
}

type RuntimeTerminalState = 'running' | 'exited' | 'unknown'

type RuntimeTerminalRead = {
  handle: string
  status: RuntimeTerminalState
  tail: string[]
  truncated: boolean
  limited?: boolean
  oldestCursor?: string
  nextCursor: string | null
  latestCursor?: string
  returnedLineCount?: number
}

type RuntimeTerminalRename = {
  handle: string
  tabId: string
  title: string | null
}

export type RuntimeTerminalSend = {
  handle: string
  accepted: boolean
  bytesWritten: number
}

export type RuntimeTerminalCreate = {
  handle: string
  worktreeId: string
  title: string | null
  surface?: 'background' | 'visible'
}

export type RuntimeTerminalSplit = {
  handle: string
  tabId: string
  paneRuntimeId: number
}

type RuntimeTerminalFocus = {
  handle: string
  tabId: string
  worktreeId: string
}

export type RuntimeTerminalClose = {
  handle: string
  tabId: string
  ptyKilled: boolean
}

type RuntimeTerminalWaitCondition = 'exit' | 'tui-idle'
type RuntimeTerminalWaitBlockedReason =
  | 'codex-update-prompt'
  | 'codex-trust-workspace'
  | 'codex-cwd-prompt'
  | 'codex-model-migration-prompt'
  | 'codex-hooks-review-prompt'
  | 'codex-interactive-prompt'

export type RuntimeTerminalWait = {
  handle: string
  condition: RuntimeTerminalWaitCondition
  satisfied: boolean
  status: RuntimeTerminalState
  exitCode: number | null
  blockedReason?: RuntimeTerminalWaitBlockedReason
}

type RuntimeWorktreePsSummary = {
  worktreeId: string
  repoId: string
  repo: string
  path: string
  branch: string
  parentWorktreeId: string | null
  childWorktreeIds: string[]
  displayName: string
  linkedIssue: number | null
  linkedPR: { number: number; state: string } | null
  isPinned: boolean
  unread: boolean
  liveTerminalCount: number
  hasAttachedPty: boolean
  lastOutputAt: number | null
  preview: string
  status: RuntimeWorktreeStatus
}

type RuntimeWorktreeStatus = 'active' | 'working' | 'permission' | 'done' | 'inactive'

type RuntimeWorktreeRecord = Worktree & {
  parentWorktreeId: string | null
  childWorktreeIds: string[]
  lineage: WorktreeLineage | null
  git: GitWorktreeInfo
}

type RuntimeWorktreeCreateResult = {
  worktree: RuntimeWorktreeRecord
  lineage: WorktreeLineage | null
  warnings: WorktreeLineageWarning[]
  warning?: string
}

type RuntimeWorktreeRemoveResult = RemoveWorktreeResult & {
  removed: boolean
  warning?: string
}

type RuntimeWorktreePsResult = {
  worktrees: RuntimeWorktreePsSummary[]
  totalCount: number
  truncated: boolean
}

type RuntimeRepoList = {
  repos: Repo[]
}

type RuntimeRepoSearchRefs = {
  refs: string[]
  refDetails?: BaseRefSearchResult[]
  truncated: boolean
}

export type RuntimeWorktreeListResult = {
  worktrees: RuntimeWorktreeRecord[]
  totalCount: number
  truncated: boolean
}

// ── Browser automation types ──

type BrowserSnapshotRef = {
  ref: string
  role: string
  name: string
}

type BrowserSnapshotResult = {
  browserPageId: string
  snapshot: string
  refs: BrowserSnapshotRef[]
  url: string
  title: string
}

type BrowserClickResult = {
  clicked: string
}

export type BrowserGotoResult = {
  url: string
  title: string
}

type BrowserFillResult = {
  filled: string
}

type BrowserTypeResult = {
  typed: boolean
}

type BrowserSelectResult = {
  selected: string
}

type BrowserScrollResult = {
  scrolled: 'up' | 'down'
}

export type BrowserBackResult = {
  url: string
  title: string
}

export type BrowserReloadResult = {
  url: string
  title: string
}

type BrowserScreenshotResult = {
  data: string
  format: 'png' | 'jpeg'
}

type BrowserScreencastReadyResult = {
  type: 'ready'
  subscriptionId: string
  browserPageId: string
  format: 'jpeg' | 'png'
  tab: BrowserTabInfo
}

type BrowserScreencastEndResult = {
  type: 'end'
  subscriptionId: string
}

type BrowserScreencastDialogResult = {
  type: 'dialog'
  dialogType: string
  message: string
}

type BrowserScreencastDialogClosedResult = {
  type: 'dialogClosed'
}

type BrowserScreencastErrorResult = {
  type: 'error'
  message: string
}

export type BrowserScreencastResult =
  | BrowserScreencastReadyResult
  | BrowserScreencastEndResult
  | BrowserScreencastDialogResult
  | BrowserScreencastDialogClosedResult
  | BrowserScreencastErrorResult

type BrowserEvalResult = {
  result: string
  origin: string
}

export type BrowserTabInfo = {
  browserPageId: string
  index: number
  url: string
  title: string
  active: boolean
  worktreeId?: string | null
  profileId?: string | null
  profileLabel?: string | null
}

type BrowserTabListResult = {
  tabs: BrowserTabInfo[]
}

type BrowserTabSwitchResult = {
  switched: number
  browserPageId: string
}

type BrowserTabSetProfileResult = {
  browserPageId: string
  profileId: string | null
  profileLabel: string | null
}

type BrowserTabShowResult = {
  tab: BrowserTabInfo
}

type BrowserTabCurrentResult = {
  tab: BrowserTabInfo
}

type BrowserTabProfileShowResult = {
  browserPageId: string
  worktreeId: string | null
  profileId: string | null
  profileLabel: string | null
}

type BrowserTabProfileCloneResult = {
  browserPageId: string
  sourceBrowserPageId: string
  profileId: string | null
  profileLabel: string | null
}

export type BrowserProfileListResult = {
  profiles: BrowserSessionProfile[]
}

export type BrowserProfileCreateResult = {
  profile: BrowserSessionProfile | null
}

export type BrowserProfileDeleteResult = {
  deleted: boolean
  profileId: string
}

type BrowserDetectedProfileInfo = {
  name: string
  directory: string
}

type BrowserDetectedInfo = {
  family: BrowserSessionProfileSource['browserFamily']
  label: string
  profiles: BrowserDetectedProfileInfo[]
  selectedProfile: string
}

export type BrowserDetectProfilesResult = {
  browsers: BrowserDetectedInfo[]
}

export type BrowserProfileImportFromBrowserResult = BrowserCookieImportResult

export type BrowserProfileClearDefaultCookiesResult = {
  cleared: boolean
}

type BrowserHoverResult = {
  hovered: string
}

type BrowserDragResult = {
  dragged: { from: string; to: string }
}

type BrowserUploadResult = {
  uploaded: number
}

type BrowserWaitResult = {
  waited: boolean
}

type BrowserCheckResult = {
  checked: boolean
}

type BrowserFocusResult = {
  focused: string
}

type BrowserClearResult = {
  cleared: string
}

type BrowserSelectAllResult = {
  selected: string
}

type BrowserKeypressResult = {
  pressed: string
}

type BrowserPdfResult = {
  data: string
}

// ── Cookie management types ──

type BrowserCookie = {
  name: string
  value: string
  domain: string
  path: string
  expires: number
  httpOnly: boolean
  secure: boolean
  sameSite: string
}

type BrowserCookieGetResult = {
  cookies: BrowserCookie[]
}

type BrowserCookieSetResult = {
  success: boolean
}

type BrowserCookieDeleteResult = {
  deleted: boolean
}

// ── Viewport emulation types ──

type BrowserViewportResult = {
  width: number
  height: number
  deviceScaleFactor: number
  mobile: boolean
}

// ── Geolocation types ──

type BrowserGeolocationResult = {
  latitude: number
  longitude: number
  accuracy: number
}

// ── Request interception types ──

type BrowserInterceptedRequest = {
  id: string
  url: string
  method: string
  headers: Record<string, string>
  resourceType: string
}

type BrowserInterceptEnableResult = {
  enabled: boolean
  patterns: string[]
}

type BrowserInterceptDisableResult = {
  disabled: boolean
}

// ── Console/network capture types ──

type BrowserConsoleEntry = {
  level: string
  text: string
  timestamp: number
  url?: string
  line?: number
}

type BrowserConsoleResult = {
  entries: BrowserConsoleEntry[]
  truncated: boolean
}

type BrowserNetworkEntry = {
  url: string
  method: string
  status: number
  mimeType: string
  size: number
  timestamp: number
}

type BrowserNetworkLogResult = {
  entries: BrowserNetworkEntry[]
  truncated: boolean
}

type BrowserCaptureStartResult = {
  capturing: boolean
}

type BrowserCaptureStopResult = {
  stopped: boolean
}

type BrowserExecResult = {
  output: unknown
}

export type BrowserTabCreateResult = {
  browserPageId: string
}

type BrowserTabCloseResult = {
  closed: boolean
}

type BrowserErrorCode =
  | 'browser_no_tab'
  | 'browser_tab_not_found'
  | 'browser_tab_closed'
  | 'browser_stale_ref'
  | 'browser_ref_not_found'
  | 'browser_navigation_failed'
  | 'browser_element_not_interactable'
  | 'browser_eval_error'
  | 'browser_cdp_error'
  | 'browser_debugger_detached'
  | 'browser_timeout'
  | 'browser_error'

// Computer-use types (see docs/computer-use/plan.md §4 and §12.6).

const COMPUTER_ERROR_CODES = {
  app_not_found: 'app_not_found',
  app_blocked: 'app_blocked',
  window_not_found: 'window_not_found',
  window_stale: 'window_stale',
  provider_incompatible: 'provider_incompatible',
  unsupported_capability: 'unsupported_capability',
  permission_denied: 'permission_denied',
  element_not_found: 'element_not_found',
  element_not_clickable: 'element_not_clickable',
  action_not_supported: 'action_not_supported',
  value_not_settable: 'value_not_settable',
  invalid_argument: 'invalid_argument',
  action_timeout: 'action_timeout',
  screenshot_failed: 'screenshot_failed',
  accessibility_error: 'accessibility_error'
} as const

type ComputerErrorCode = keyof typeof COMPUTER_ERROR_CODES

type ComputerAppQuery = string

type ComputerSessionTarget = {
  session?: string
  worktree?: string
  app?: ComputerAppQuery
}

type ComputerListAppsArgs = {
  worktree?: string
}

type ComputerAppInfo = {
  name: string
  bundleId: string | null
  pid: number
}

type ComputerWindowInfo = {
  id?: number | null
  title: string
  x?: number | null
  y?: number | null
  width: number
  height: number
  isMinimized?: boolean | null
  isOffscreen?: boolean | null
  screenIndex?: number | null
  platform?: Record<string, unknown>
}

type ComputerSnapshotData = {
  id: string
  app: ComputerAppInfo
  window: ComputerWindowInfo
  coordinateSpace: 'window'
  treeText: string
  elementCount: number
  focusedElementId: number | null
  truncation?: {
    truncated: boolean
    maxNodes?: number
    maxDepth?: number
    maxDepthReached?: boolean
  }
}

type ComputerScreenshotData = {
  data?: string
  format: 'png'
  width: number
  height: number
  scale: number
  path?: string
  dataOmitted?: boolean
  expiresAt?: string
}

type ComputerScreenshotMetadata = {
  engine?: 'screenCaptureKit' | 'cgWindowList' | 'unknown'
  windowId?: number | null
}

type ComputerScreenshotStatus =
  | { state: 'captured'; metadata?: ComputerScreenshotMetadata }
  | { state: 'skipped'; reason: 'no_screenshot_flag' }
  | {
      state: 'failed'
      code: ComputerErrorCode
      message: string
      metadata?: ComputerScreenshotMetadata
    }

type ComputerActionMetadata = {
  path: 'accessibility' | 'synthetic' | 'clipboard'
  actionName?: string | null
  fallbackReason?: string | null
  targetWindowId?: number | null
  verification?: ComputerActionVerification
}

type ComputerActionVerification =
  | {
      state: 'verified'
      property: 'focusedText' | 'selection'
      expected?: string | null
      actualPreview?: string | null
    }
  | {
      state: 'unverified'
      reason: 'synthetic_input' | 'clipboard_paste' | 'provider_unavailable' | 'window_changed'
    }

type ComputerSnapshotResult = {
  snapshot: ComputerSnapshotData
  screenshot: ComputerScreenshotData | null
  screenshotStatus: ComputerScreenshotStatus
}

type ComputerActionResult = ComputerSnapshotResult & {
  action?: ComputerActionMetadata
}

type ComputerProviderCapabilities = {
  platform: NodeJS.Platform
  provider: string
  providerVersion: string
  protocolVersion: number
  supports: {
    apps: {
      list: boolean
      bundleIds: boolean
      pids: boolean
    }
    windows: {
      list: boolean
      targetById: boolean
      targetByIndex: boolean
      focus: boolean
      moveResize: boolean
    }
    observation: {
      screenshot: boolean
      annotatedScreenshot: boolean
      elementFrames: boolean
      ocr: boolean
    }
    actions: {
      click: boolean
      typeText: boolean
      pressKey: boolean
      hotkey: boolean
      pasteText: boolean
      scroll: boolean
      drag: boolean
      setValue: boolean
      performAction: boolean
    }
    surfaces: {
      menus: boolean
      dialogs: boolean
      dock: boolean
      menubar: boolean
    }
  }
}

type ComputerWindowListWindow = ComputerWindowInfo & {
  app: ComputerAppInfo
  index: number
  isMain?: boolean | null
}

type ComputerListWindowsResult = {
  app: ComputerAppInfo
  windows: ComputerWindowListWindow[]
}

type ComputerListAppsResult = {
  apps: (ComputerAppInfo & {
    isRunning: boolean
    lastUsedAt: string | null
    useCount: number | null
  })[]
}
