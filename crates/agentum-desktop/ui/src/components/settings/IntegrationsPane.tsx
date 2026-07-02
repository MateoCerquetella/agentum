import { api } from '@/tauri'
import { LinearIcon } from '@/components/icons/LinearIcon'
/* eslint-disable max-lines -- Why: this pane co-locates source-host and
   Linear integration cards so the preflight-check + status-badge +
   install/auth-prompt scaffolding lives in one place rather than fanning
   out across per-integration files that would each repeat the same
   pattern. Splitting buys nothing while the surface stays this narrow. */
import { useEffect, useState } from 'react'
import {
  Github,
  Gitlab,
  GitPullRequestArrow,
  ExternalLink,
  LoaderCircle,
  Terminal,
  Unlink,
  CheckCircle2,
  AlertCircle
} from 'lucide-react'
import { useAppStore } from '../../store'
import { Button } from '../ui/button'
import { Input } from '../ui/input'
import { useMountedRef } from '@/hooks/useMountedRef'
import { getHarnessSettings, setHarnessSettings } from '@/runtime/harness-client'
import { LinearApiKeyDialog } from '@/components/linear-api-key-dialog'
import {
  getPreflightIntegrationStatuses,
  type PreflightRefreshProvider
} from './integrations-pane-status'
export { INTEGRATIONS_PANE_SEARCH_ENTRIES } from './integrations-search'

/** Map of pipeline phase → Linear workflow-state name (spec 012). */
type LinearStateMap = { todo: string; inProgress: string; readyToTest: string; done: string }

/**
 * Editor for the harness pipeline → Linear workflow-state names. The embedded
 * server resolves these by name when it transitions a ticket (Todo → In Progress
 * → Ready to Test → Done); a name that doesn't exist on the team is skipped, so
 * these only need to match the user's actual Linear columns.
 */
function LinearStateMapEditor(): React.JSX.Element {
  const mounted = useMountedRef()
  const [map, setMap] = useState<LinearStateMap | null>(null)
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)

  useEffect(() => {
    void (async () => {
      const m = (await api.linear.getStateMap()) as Partial<LinearStateMap> | null
      if (!mounted.current || !m) return
      setMap({
        todo: m.todo ?? 'Todo',
        inProgress: m.inProgress ?? 'In Progress',
        readyToTest: m.readyToTest ?? 'Ready to Test',
        done: m.done ?? 'Done'
      })
    })()
  }, [mounted])

  if (!map) return <></>

  const field = (key: keyof LinearStateMap, label: string) => (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-muted-foreground">{label}</span>
      <Input
        value={map[key]}
        onChange={(e) => {
          setSaved(false)
          setMap({ ...map, [key]: e.target.value })
        }}
        className="h-8 text-sm"
      />
    </label>
  )

  const handleSave = async (): Promise<void> => {
    setSaving(true)
    // Param names are snake_case to match the Tauri command signature.
    await api.linear.setStateMap({
      todo: map.todo,
      in_progress: map.inProgress,
      ready_to_test: map.readyToTest,
      done: map.done
    })
    if (!mounted.current) return
    setSaving(false)
    setSaved(true)
  }

  return (
    <div className="mt-3 rounded-md border border-border/50 bg-background/60 p-3">
      <p className="text-sm font-medium text-foreground">Pipeline workflow states</p>
      <p className="mt-0.5 text-[11px] text-muted-foreground/70">
        Names the harness moves a ticket through as it codes and verifies a feature. Match these to
        your Linear team's columns; an unmatched name is skipped (never fails the run).
      </p>
      <div className="mt-2.5 grid grid-cols-2 gap-2.5">
        {field('todo', 'Backlog / Todo')}
        {field('inProgress', 'Coding')}
        {field('readyToTest', 'Ready to Test')}
        {field('done', 'Done')}
      </div>
      <div className="mt-2.5 flex items-center gap-2">
        <Button variant="outline" size="sm" onClick={() => void handleSave()} disabled={saving}>
          {saving ? (
            <>
              <LoaderCircle className="mr-1.5 size-3.5 animate-spin" />
              Saving…
            </>
          ) : (
            'Save states'
          )}
        </Button>
        {saved ? (
          <span className="flex items-center gap-1 text-xs text-emerald-600 dark:text-emerald-400">
            <CheckCircle2 className="size-3.5" />
            Saved
          </span>
        ) : null}
      </div>
    </div>
  )
}

/**
 * Toggle for the harness browser-QA capability (spec 005 F3, D3 — default OFF).
 * When on, the QA gate's `Auto` arm spawns an `agentum_browser`-driven QA agent
 * even without `AGENTUM_BROWSER_VERIFY`; when off, projects with no `qa.sh` keep
 * today's skip-pass. Mirrors the LinearStateMapEditor load flow + the McpPane
 * optimistic-write toggle.
 */
function BrowserQaGateToggle(): React.JSX.Element {
  const mounted = useMountedRef()
  const [enabled, setEnabled] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    void getHarnessSettings()
      .then((s) => {
        if (mounted.current) setEnabled(s.browserQaAgentEnabled)
      })
      .catch(() => {
        // Leave the default-OFF value; the toggle still works and surfaces a
        // write error if the server is unreachable.
      })
  }, [mounted])

  const toggle = (value: boolean): void => {
    // Optimistic: flip the UI, write the server flag, revert if it fails.
    setEnabled(value)
    setError(null)
    void setHarnessSettings({ browserQaAgentEnabled: value }).catch((err: unknown) => {
      if (!mounted.current) return
      setEnabled(!value)
      setError(err instanceof Error ? err.message : 'Could not update the browser QA setting.')
    })
  }

  return (
    <div className="rounded-md border border-border/50 bg-muted/30 px-4 py-3">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 flex-1 space-y-0.5">
          <p className="text-sm font-medium">Harness browser QA agent</p>
          <p className="text-xs text-muted-foreground">
            Let gated runs verify features by spawning a QA agent that drives the in-app browser
            (the <span className="font-mono text-[11px]">agentum_browser</span> tool). Off (the
            default), projects without a <span className="font-mono text-[11px]">qa.sh</span> skip
            the browser gate — turn this on for web projects you want QA&apos;d automatically.
          </p>
        </div>
        <button
          role="switch"
          aria-checked={enabled}
          onClick={() => toggle(!enabled)}
          className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border border-transparent transition-colors ${
            enabled ? 'bg-foreground' : 'bg-muted-foreground/30'
          }`}
        >
          <span
            className={`inline-block h-3.5 w-3.5 transform rounded-full bg-background shadow-sm transition-transform ${
              enabled ? 'translate-x-4' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>
      {error ? <p className="mt-1.5 text-xs text-destructive">{error}</p> : null}
    </div>
  )
}

export function IntegrationsPane(): React.JSX.Element {
  const linearStatus = useAppStore((s) => s.linearStatus)
  const preflightStatus = useAppStore((s) => s.preflightStatus)
  const disconnectLinear = useAppStore((s) => s.disconnectLinear)
  const disconnectLinearWorkspace = useAppStore((s) => s.disconnectLinearWorkspace)
  const checkLinearConnection = useAppStore((s) => s.checkLinearConnection)
  const refreshPreflightStatus = useAppStore((s) => s.refreshPreflightStatus)
  const testLinearConnection = useAppStore((s) => s.testLinearConnection)
  const linearWorkspaces = linearStatus.workspaces ?? []
  const mountedRef = useMountedRef()

  const [refreshingPreflightProviders, setRefreshingPreflightProviders] = useState<
    Set<PreflightRefreshProvider>
  >(new Set())
  const [linearDialogOpen, setLinearDialogOpen] = useState(false)
  const [linearTestingWorkspaceId, setLinearTestingWorkspaceId] = useState<string | null>(null)
  const [linearTestResultByWorkspace, setLinearTestResultByWorkspace] = useState<
    Record<string, { state: 'ok' | 'error'; error?: string }>
  >({})

  useEffect(() => {
    void checkLinearConnection()
    void refreshPreflightStatus()
  }, [checkLinearConnection, refreshPreflightStatus])

  const {
    ghStatus,
    glabStatus,
    bitbucketStatus,
    bitbucketAccount,
    azureDevOpsStatus,
    azureDevOpsAccount,
    azureDevOpsBaseUrl,
    giteaStatus,
    giteaAccount,
    giteaBaseUrl
  } = getPreflightIntegrationStatuses(preflightStatus, refreshingPreflightProviders)

  const handleLinearDisconnect = async (workspaceId?: string): Promise<void> => {
    await (workspaceId ? disconnectLinearWorkspace(workspaceId) : disconnectLinear())
    if (!mountedRef.current) {
      return
    }
    setLinearTestResultByWorkspace({})
  }

  // Why: explicit user-triggered verification. This is the *only* path in
  // settings that decrypts the stored API key, so the macOS Keychain prompt
  // (if the app signature has changed since the item was stored) only
  // appears when the user clicks Test — not just for opening Settings.
  const handleLinearTest = async (workspaceId: string): Promise<void> => {
    setLinearTestingWorkspaceId(workspaceId)
    setLinearTestResultByWorkspace((prev) => {
      const next = { ...prev }
      delete next[workspaceId]
      return next
    })
    const result = await testLinearConnection(workspaceId)
    if (!mountedRef.current) {
      return
    }
    if (result.ok) {
      setLinearTestResultByWorkspace((prev) => ({
        ...prev,
        [workspaceId]: { state: 'ok' }
      }))
    } else {
      setLinearTestResultByWorkspace((prev) => ({
        ...prev,
        [workspaceId]: { state: 'error', error: result.error }
      }))
    }
    setLinearTestingWorkspaceId(null)
  }

  const refreshPreflightProvider = (provider: PreflightRefreshProvider): void => {
    setRefreshingPreflightProviders((prev) => new Set(prev).add(provider))
    void refreshPreflightStatus({ force: true }).finally(() => {
      if (!mountedRef.current) {
        return
      }
      setRefreshingPreflightProviders((prev) => {
        if (!prev.has(provider)) {
          return prev
        }
        const next = new Set(prev)
        next.delete(provider)
        return next
      })
    })
  }

  const handleRefreshGlab = (): void => refreshPreflightProvider('glab')

  const handleRefreshGh = (): void => refreshPreflightProvider('gh')

  const handleRefreshBitbucket = (): void => refreshPreflightProvider('bitbucket')

  const handleRefreshAzureDevOps = (): void => refreshPreflightProvider('azureDevOps')

  const handleRefreshGitea = (): void => refreshPreflightProvider('gitea')

  return (
    <div className="space-y-3">
      {/* GitHub */}
      <div className="rounded-md border border-border/50 bg-muted/30 px-4 py-3">
        <div className="flex items-center gap-3">
          <Github className="size-5 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1 space-y-0.5">
            <p className="text-sm font-medium">GitHub</p>
            <p className="text-xs text-muted-foreground">
              Pull requests, issues, and checks via the{' '}
              <span className="font-mono text-[11px]">gh</span> CLI.
            </p>
          </div>
          {ghStatus === 'checking' ? (
            <LoaderCircle className="size-4 shrink-0 animate-spin text-muted-foreground" />
          ) : ghStatus === 'connected' ? (
            <span className="shrink-0 rounded-full border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-1 text-[11px] font-medium text-emerald-700 dark:text-emerald-300">
              Connected
            </span>
          ) : (
            <span className="shrink-0 rounded-full border border-amber-500/30 bg-amber-500/10 px-2.5 py-1 text-[11px] font-medium text-amber-700 dark:text-amber-300">
              {ghStatus === 'not-installed' ? 'Not installed' : 'Not authenticated'}
            </span>
          )}
        </div>

        {ghStatus !== 'checking' && ghStatus !== 'connected' && (
          <div className="mt-3 rounded-md border border-border/30 bg-background/50 px-3 py-2.5 space-y-2">
            {ghStatus === 'not-installed' ? (
              <>
                <p className="text-xs text-muted-foreground">
                  Install the GitHub CLI to enable pull requests, issues, and checks.
                </p>
                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => api.shell.openUrl('https://cli.github.com')}
                  >
                    <ExternalLink className="size-3.5 mr-1.5" />
                    Install GitHub CLI
                  </Button>
                  <Button variant="ghost" size="sm" onClick={handleRefreshGh}>
                    Re-check
                  </Button>
                </div>
              </>
            ) : (
              <>
                <p className="text-xs text-muted-foreground">
                  The GitHub CLI is installed but not authenticated. Run this command in a terminal:
                </p>
                <div className="flex items-center gap-2 rounded-md bg-muted/50 px-2.5 py-1.5 font-mono text-xs">
                  <Terminal className="size-3.5 shrink-0 text-muted-foreground" />
                  gh auth login
                </div>
                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() =>
                      api.shell.openUrl('https://cli.github.com/manual/gh_auth_login')
                    }
                  >
                    <ExternalLink className="size-3.5 mr-1.5" />
                    Learn more
                  </Button>
                  <Button variant="ghost" size="sm" onClick={handleRefreshGh}>
                    Re-check
                  </Button>
                </div>
              </>
            )}
          </div>
        )}
      </div>

      {/* GitLab */}
      <div className="rounded-md border border-border/50 bg-muted/30 px-4 py-3">
        <div className="flex items-center gap-3">
          <Gitlab className="size-5 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1 space-y-0.5">
            <p className="text-sm font-medium">GitLab</p>
            <p className="text-xs text-muted-foreground">
              Merge requests, issues, todos, and pipelines via the{' '}
              <span className="font-mono text-[11px]">glab</span> CLI.
            </p>
          </div>
          {glabStatus === 'checking' ? (
            <LoaderCircle className="size-4 shrink-0 animate-spin text-muted-foreground" />
          ) : glabStatus === 'connected' ? (
            <span className="shrink-0 rounded-full border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-1 text-[11px] font-medium text-emerald-700 dark:text-emerald-300">
              Connected
            </span>
          ) : (
            <span className="shrink-0 rounded-full border border-amber-500/30 bg-amber-500/10 px-2.5 py-1 text-[11px] font-medium text-amber-700 dark:text-amber-300">
              {glabStatus === 'not-installed' ? 'Not installed' : 'Not authenticated'}
            </span>
          )}
        </div>

        {glabStatus !== 'checking' && glabStatus !== 'connected' && (
          <div className="mt-3 rounded-md border border-border/30 bg-background/50 px-3 py-2.5 space-y-2">
            {glabStatus === 'not-installed' ? (
              <>
                <p className="text-xs text-muted-foreground">
                  Install the GitLab CLI to enable merge requests, issues, and pipelines.
                </p>
                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() =>
                      api.shell.openUrl('https://gitlab.com/gitlab-org/cli#installation')
                    }
                  >
                    <ExternalLink className="size-3.5 mr-1.5" />
                    Install GitLab CLI
                  </Button>
                  <Button variant="ghost" size="sm" onClick={handleRefreshGlab}>
                    Re-check
                  </Button>
                </div>
              </>
            ) : (
              <>
                <p className="text-xs text-muted-foreground">
                  The GitLab CLI is installed but not authenticated. Run this command in a terminal:
                </p>
                <div className="flex items-center gap-2 rounded-md bg-muted/50 px-2.5 py-1.5 font-mono text-xs">
                  <Terminal className="size-3.5 shrink-0 text-muted-foreground" />
                  glab auth login
                </div>
                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() =>
                      api.shell.openUrl(
                        'https://gitlab.com/gitlab-org/cli/-/blob/main/docs/source/auth/login.md'
                      )
                    }
                  >
                    <ExternalLink className="size-3.5 mr-1.5" />
                    Learn more
                  </Button>
                  <Button variant="ghost" size="sm" onClick={handleRefreshGlab}>
                    Re-check
                  </Button>
                </div>
              </>
            )}
          </div>
        )}
      </div>

      {/* Bitbucket */}
      <div className="rounded-md border border-border/50 bg-muted/30 px-4 py-3">
        <div className="flex items-center gap-3">
          <GitPullRequestArrow className="size-5 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1 space-y-0.5">
            <p className="text-sm font-medium">Bitbucket</p>
            <p className="text-xs text-muted-foreground">
              {bitbucketStatus === 'connected'
                ? bitbucketAccount
                  ? `${bitbucketAccount} · Pull requests and build statuses`
                  : 'Pull requests and build statuses'
                : 'Pull requests and build statuses via Bitbucket Cloud API tokens.'}
            </p>
          </div>
          {bitbucketStatus === 'checking' ? (
            <LoaderCircle className="size-4 shrink-0 animate-spin text-muted-foreground" />
          ) : bitbucketStatus === 'connected' ? (
            <span className="shrink-0 rounded-full border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-1 text-[11px] font-medium text-emerald-700 dark:text-emerald-300">
              Connected
            </span>
          ) : (
            <span className="shrink-0 rounded-full border border-amber-500/30 bg-amber-500/10 px-2.5 py-1 text-[11px] font-medium text-amber-700 dark:text-amber-300">
              {bitbucketStatus === 'not-configured' ? 'Not configured' : 'Auth failed'}
            </span>
          )}
        </div>

        {bitbucketStatus !== 'checking' && bitbucketStatus !== 'connected' && (
          <div className="mt-3 rounded-md border border-border/30 bg-background/50 px-3 py-2.5 space-y-2">
            {bitbucketStatus === 'not-configured' ? (
              <>
                <p className="text-xs text-muted-foreground">
                  Set <span className="font-mono text-[11px]">AGENTUM_BITBUCKET_EMAIL</span> and{' '}
                  <span className="font-mono text-[11px]">AGENTUM_BITBUCKET_API_TOKEN</span>, or set{' '}
                  <span className="font-mono text-[11px]">AGENTUM_BITBUCKET_ACCESS_TOKEN</span>.
                </p>
                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() =>
                      api.shell.openUrl(
                        'https://support.atlassian.com/bitbucket-cloud/docs/using-api-tokens/'
                      )
                    }
                  >
                    <ExternalLink className="size-3.5 mr-1.5" />
                    Learn more
                  </Button>
                  <Button variant="ghost" size="sm" onClick={handleRefreshBitbucket}>
                    Re-check
                  </Button>
                </div>
              </>
            ) : (
              <>
                <p className="text-xs text-muted-foreground">
                  Bitbucket credentials are configured but could not authenticate. Check the token
                  and repository permissions, then restart Agentum if environment variables changed.
                </p>
                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() =>
                      api.shell.openUrl(
                        'https://support.atlassian.com/bitbucket-cloud/docs/using-api-tokens/'
                      )
                    }
                  >
                    <ExternalLink className="size-3.5 mr-1.5" />
                    Learn more
                  </Button>
                  <Button variant="ghost" size="sm" onClick={handleRefreshBitbucket}>
                    Re-check
                  </Button>
                </div>
              </>
            )}
          </div>
        )}
      </div>

      {/* Azure DevOps */}
      <div className="rounded-md border border-border/50 bg-muted/30 px-4 py-3">
        <div className="flex items-center gap-3">
          <GitPullRequestArrow className="size-5 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1 space-y-0.5">
            <p className="text-sm font-medium">Azure DevOps</p>
            <p className="text-xs text-muted-foreground">
              {azureDevOpsStatus === 'configured'
                ? azureDevOpsAccount
                  ? `${azureDevOpsAccount} · Pull requests and build statuses`
                  : azureDevOpsBaseUrl
                    ? `${azureDevOpsBaseUrl} · Pull requests and build statuses`
                    : 'Pull requests and build statuses for detected Azure Repos'
                : 'Pull requests and build statuses via Azure DevOps REST API tokens.'}
            </p>
          </div>
          {azureDevOpsStatus === 'checking' ? (
            <LoaderCircle className="size-4 shrink-0 animate-spin text-muted-foreground" />
          ) : azureDevOpsStatus === 'configured' ? (
            <span className="shrink-0 rounded-full border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-1 text-[11px] font-medium text-emerald-700 dark:text-emerald-300">
              {azureDevOpsAccount ? 'Connected' : 'Configured'}
            </span>
          ) : (
            <span className="shrink-0 rounded-full border border-amber-500/30 bg-amber-500/10 px-2.5 py-1 text-[11px] font-medium text-amber-700 dark:text-amber-300">
              {azureDevOpsStatus === 'not-configured' ? 'Not configured' : 'Auth failed'}
            </span>
          )}
        </div>

        {azureDevOpsStatus !== 'checking' && azureDevOpsStatus !== 'configured' && (
          <div className="mt-3 rounded-md border border-border/30 bg-background/50 px-3 py-2.5 space-y-2">
            {azureDevOpsStatus === 'not-configured' ? (
              <>
                <p className="text-xs text-muted-foreground">
                  Set <span className="font-mono text-[11px]">AGENTUM_AZURE_DEVOPS_TOKEN</span>, or set{' '}
                  <span className="font-mono text-[11px]">AGENTUM_AZURE_DEVOPS_ACCESS_TOKEN</span>. Set{' '}
                  <span className="font-mono text-[11px]">AGENTUM_AZURE_DEVOPS_API_BASE_URL</span> only
                  when Agentum cannot derive the API base URL from the git remote.
                </p>
                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() =>
                      api.shell.openUrl(
                        'https://learn.microsoft.com/en-us/azure/devops/organizations/accounts/use-personal-access-tokens-to-authenticate'
                      )
                    }
                  >
                    <ExternalLink className="size-3.5 mr-1.5" />
                    Learn more
                  </Button>
                  <Button variant="ghost" size="sm" onClick={handleRefreshAzureDevOps}>
                    Re-check
                  </Button>
                </div>
              </>
            ) : (
              <>
                <p className="text-xs text-muted-foreground">
                  Azure DevOps credentials are configured but could not authenticate. Check the
                  token, API base URL, and repository permissions, then restart Agentum if environment
                  variables changed.
                </p>
                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() =>
                      api.shell.openUrl(
                        'https://learn.microsoft.com/en-us/rest/api/azure/devops/git/pull-requests/get-pull-requests'
                      )
                    }
                  >
                    <ExternalLink className="size-3.5 mr-1.5" />
                    Learn more
                  </Button>
                  <Button variant="ghost" size="sm" onClick={handleRefreshAzureDevOps}>
                    Re-check
                  </Button>
                </div>
              </>
            )}
          </div>
        )}
      </div>

      {/* Gitea */}
      <div className="rounded-md border border-border/50 bg-muted/30 px-4 py-3">
        <div className="flex items-center gap-3">
          <GitPullRequestArrow className="size-5 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1 space-y-0.5">
            <p className="text-sm font-medium">Gitea</p>
            <p className="text-xs text-muted-foreground">
              {giteaStatus === 'configured'
                ? giteaAccount
                  ? `${giteaAccount} · Pull requests and commit statuses`
                  : giteaBaseUrl
                    ? `${giteaBaseUrl} · Pull requests and commit statuses`
                    : 'Pull requests and commit statuses for detected repositories'
                : 'Pull requests and commit statuses via the Gitea REST API.'}
            </p>
          </div>
          {giteaStatus === 'checking' ? (
            <LoaderCircle className="size-4 shrink-0 animate-spin text-muted-foreground" />
          ) : giteaStatus === 'configured' ? (
            <span className="shrink-0 rounded-full border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-1 text-[11px] font-medium text-emerald-700 dark:text-emerald-300">
              {giteaAccount ? 'Connected' : 'Configured'}
            </span>
          ) : (
            <span className="shrink-0 rounded-full border border-amber-500/30 bg-amber-500/10 px-2.5 py-1 text-[11px] font-medium text-amber-700 dark:text-amber-300">
              {giteaStatus === 'not-configured' ? 'Optional setup' : 'Auth failed'}
            </span>
          )}
        </div>

        {giteaStatus !== 'checking' && giteaStatus !== 'configured' && (
          <div className="mt-3 rounded-md border border-border/30 bg-background/50 px-3 py-2.5 space-y-2">
            {giteaStatus === 'not-configured' ? (
              <>
                <p className="text-xs text-muted-foreground">
                  Public repositories are detected from their git remote. Set{' '}
                  <span className="font-mono text-[11px]">AGENTUM_GITEA_TOKEN</span> for private
                  repositories, and set{' '}
                  <span className="font-mono text-[11px]">AGENTUM_GITEA_API_BASE_URL</span> only when
                  Agentum cannot derive the API URL from the remote.
                </p>
                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() =>
                      api.shell.openUrl('https://docs.gitea.com/next/development/api-usage')
                    }
                  >
                    <ExternalLink className="size-3.5 mr-1.5" />
                    Learn more
                  </Button>
                  <Button variant="ghost" size="sm" onClick={handleRefreshGitea}>
                    Re-check
                  </Button>
                </div>
              </>
            ) : (
              <>
                <p className="text-xs text-muted-foreground">
                  Gitea credentials are configured but could not authenticate. Check the token, API
                  base URL, and repository permissions, then restart Agentum if environment variables
                  changed.
                </p>
                <div className="flex items-center gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() =>
                      api.shell.openUrl('https://docs.gitea.com/next/development/api-usage')
                    }
                  >
                    <ExternalLink className="size-3.5 mr-1.5" />
                    Learn more
                  </Button>
                  <Button variant="ghost" size="sm" onClick={handleRefreshGitea}>
                    Re-check
                  </Button>
                </div>
              </>
            )}
          </div>
        )}
      </div>

      {/* Linear */}
      <div className="rounded-md border border-border/50 bg-muted/30 px-4 py-3">
        <div className="flex items-center gap-3">
          <LinearIcon className="size-5 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1 space-y-0.5">
            <p className="text-sm font-medium">Linear</p>
            <p className="text-xs text-muted-foreground">
              {linearStatus.connected
                ? `${linearWorkspaces.length} workspace${linearWorkspaces.length === 1 ? '' : 's'} connected`
                : 'Add Linear access to browse and link issues.'}
            </p>
          </div>
          {linearStatus.connected ? (
            <div className="flex shrink-0 items-center gap-1.5">
              <Button variant="outline" size="sm" onClick={() => setLinearDialogOpen(true)}>
                Add workspace access
              </Button>
              <span className="rounded-full border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-1 text-[11px] font-medium text-emerald-700 dark:text-emerald-300">
                Connected
              </span>
            </div>
          ) : (
            <button
              className="shrink-0 rounded-full border border-border/50 bg-muted/40 px-2.5 py-1 text-[11px] font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
              onClick={() => setLinearDialogOpen(true)}
            >
              Add Linear access
            </button>
          )}
        </div>

        {linearStatus.connected && (
          <div className="mt-3 space-y-2">
            {linearWorkspaces.map((workspace) => {
              const testResult = linearTestResultByWorkspace[workspace.id]
              const testing = linearTestingWorkspaceId === workspace.id
              return (
                <div
                  key={workspace.id}
                  className="flex items-center gap-3 rounded-md border border-border/50 bg-background/60 px-3 py-2"
                >
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium text-foreground">
                      {workspace.organizationName}
                    </p>
                    <p className="truncate text-xs text-muted-foreground">
                      {workspace.displayName}
                      {workspace.email ? ` · ${workspace.email}` : ''}
                    </p>
                  </div>
                  {testResult?.state === 'ok' ? (
                    <span className="flex shrink-0 items-center gap-1 text-xs text-emerald-600 dark:text-emerald-400">
                      <CheckCircle2 className="size-3.5" />
                      Verified
                    </span>
                  ) : null}
                  {testResult?.state === 'error' ? (
                    <span className="flex min-w-0 max-w-[220px] shrink items-center gap-1 truncate text-xs text-destructive">
                      <AlertCircle className="size-3.5 shrink-0" />
                      <span className="truncate">{testResult.error}</span>
                    </span>
                  ) : null}
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => void handleLinearTest(workspace.id)}
                    disabled={testing}
                  >
                    {testing ? (
                      <>
                        <LoaderCircle className="size-3.5 mr-1.5 animate-spin" />
                        Testing…
                      </>
                    ) : (
                      'Test'
                    )}
                  </Button>
                  <button
                    onClick={() => void handleLinearDisconnect(workspace.id)}
                    aria-label={`Disconnect ${workspace.organizationName}`}
                    className="rounded-md p-1 text-muted-foreground/50 transition-colors hover:text-destructive"
                  >
                    <Unlink className="size-3.5" />
                  </button>
                </div>
              )
            })}
            <p className="text-[11px] text-muted-foreground/70">
              Each connected Linear workspace has one key stored by the active runtime. Full-access
              keys can cover all teams the key owner can access; restricted keys can be replaced any
              time.
            </p>
            <LinearStateMapEditor />
          </div>
        )}
      </div>

      {/* Harness pipeline behavior (spec 005 F3) — sits beside the Linear
          state-map config: both configure how gated runs drive a tracker/QA. */}
      <BrowserQaGateToggle />

      <LinearApiKeyDialog
        open={linearDialogOpen}
        onOpenChange={setLinearDialogOpen}
        connectLabel="Add Linear access"
        onConnected={() => setLinearTestResultByWorkspace({})}
      />
    </div>
  )
}
