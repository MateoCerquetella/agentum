import { useEffect, useState } from 'react'
import { Globe2, Import, Loader2, MousePointerClick } from 'lucide-react'
import { toast } from 'sonner'
import { BROWSER_USE_ENABLED_STORAGE_KEY } from '@/lib/browser-use-setup-state'
import { Button } from '../ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuPortal,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger
} from '../ui/dropdown-menu'
import { useAppStore } from '../../store'
import { BROWSER_FAMILY_LABELS } from '../../../../shared/constants'
import { SearchableSetting } from './SearchableSetting'
import { matchesSettingsSearch } from './settings-search'
import { BROWSER_USE_PANE_SEARCH_ENTRIES } from './browser-use-search'
import { BrowserUseExamples } from './BrowserUseExamples'

type BrowserUseSetupProps = {
  onConfigureMoreBrowsers?: () => void
  onOpenComputerUse?: () => void
}

export function BrowserUseSetup({
  onConfigureMoreBrowsers,
  onOpenComputerUse
}: BrowserUseSetupProps = {}): React.JSX.Element {
  const searchQuery = useAppStore((s) => s.settingsSearchQuery)
  const browserSessionProfiles = useAppStore((s) => s.browserSessionProfiles)
  const detectedBrowsers = useAppStore((s) => s.detectedBrowsers)
  const fetchBrowserSessionProfiles = useAppStore((s) => s.fetchBrowserSessionProfiles)
  const fetchDetectedBrowsers = useAppStore((s) => s.fetchDetectedBrowsers)
  const browserSessionImportState = useAppStore((s) => s.browserSessionImportState)

  // Why: the toggle gates only whether we show the cookie-import setup. Browser
  // control itself is always available through agentum's MCP (`agentum_browser`),
  // so this is a UI affordance persisted in localStorage — it has no functional
  // effect elsewhere in the app.
  const [browserUseEnabled, setBrowserUseEnabled] = useState<boolean>(() => {
    return localStorage.getItem(BROWSER_USE_ENABLED_STORAGE_KEY) === '1'
  })

  const toggleBrowserUse = (value: boolean): void => {
    setBrowserUseEnabled(value)
    localStorage.setItem(BROWSER_USE_ENABLED_STORAGE_KEY, value ? '1' : '0')
    if (value) {
      useAppStore.getState().recordFeatureInteraction('agent-browser-setup')
    }
  }

  useEffect(() => {
    // Why: skip IPC work when the feature is toggled off — the component
    // returns early below and none of this data is rendered.
    if (!browserUseEnabled) {
      return
    }
    void fetchBrowserSessionProfiles()
  }, [browserUseEnabled, fetchBrowserSessionProfiles])

  const defaultProfile = browserSessionProfiles.find((p) => p.id === 'default')
  // Why: agents reach authenticated pages through the default profile, so
  // completion tracks that profile only. Cookies on a non-default profile would
  // mislead users into thinking agents can reach their logins when the profile
  // agents actually use is still empty.
  const cookiesImported = !!defaultProfile?.source

  const handleImportFromBrowser = async (
    browserFamily: string,
    browserProfile?: string
  ): Promise<void> => {
    const profileId = 'default'
    const result = await useAppStore
      .getState()
      .importCookiesFromBrowser(profileId, browserFamily, browserProfile)
    if (result.ok) {
      const browser = detectedBrowsers.find((b) => b.family === browserFamily)
      toast.success(
        `Imported ${result.summary.importedCookies} cookies from ${browser?.label ?? browserFamily}${browserProfile ? ` (${browserProfile})` : ''}.`
      )
    } else {
      toast.error(result.reason)
    }
  }

  const handleImportFromFile = async (): Promise<void> => {
    const result = await useAppStore.getState().importCookiesToProfile('default')
    if (result.ok) {
      toast.success(`Imported ${result.summary.importedCookies} cookies from file.`)
    } else if (result.reason !== 'canceled') {
      toast.error(result.reason)
    }
  }

  const isImportingDefault =
    browserSessionImportState?.profileId === 'default' &&
    browserSessionImportState.status === 'importing'

  const showOverview = matchesSettingsSearch(searchQuery, [BROWSER_USE_PANE_SEARCH_ENTRIES[0]])
  const showCookies = matchesSettingsSearch(searchQuery, [BROWSER_USE_PANE_SEARCH_ENTRIES[1]])

  const sourceLabel = defaultProfile?.source
    ? `${BROWSER_FAMILY_LABELS[defaultProfile.source.browserFamily] ?? defaultProfile.source.browserFamily}${defaultProfile.source.profileName ? ` (${defaultProfile.source.profileName})` : ''}`
    : null

  const toggleSwitch = (
    <button
      role="switch"
      aria-checked={browserUseEnabled}
      aria-label="Enable Agent Browser Use"
      onClick={() => toggleBrowserUse(!browserUseEnabled)}
      className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border border-transparent transition-colors ${
        browserUseEnabled ? 'bg-foreground' : 'bg-muted-foreground/30'
      }`}
    >
      <span
        className={`inline-block h-3.5 w-3.5 transform rounded-full bg-background shadow-sm transition-transform ${
          browserUseEnabled ? 'translate-x-4' : 'translate-x-0.5'
        }`}
      />
    </button>
  )

  if (!browserUseEnabled) {
    return (
      <div className="flex items-center justify-between gap-4 py-2">
        <div className="space-y-0.5">
          <p className="text-sm font-medium">Agent Browser Use</p>
          <p className="text-xs text-muted-foreground">
            Let coding agents drive this browser with your logins.
          </p>
        </div>
        {toggleSwitch}
      </div>
    )
  }

  return (
    <div className="space-y-3 rounded-2xl border border-border/60 bg-card/30 p-4">
      <div className="flex items-center justify-between gap-3">
        <div className="space-y-0.5">
          <p className="text-sm font-semibold">Agent Browser Use</p>
          <p className="text-xs text-muted-foreground">
            Built into the agentum MCP — agents can already drive this browser. Import your logins
            so they can reach authenticated pages.
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {cookiesImported ? (
            <span className="rounded-full bg-emerald-500/15 px-2 py-0.5 text-[10px] font-medium text-emerald-700 dark:text-emerald-400">
              Logins ready
            </span>
          ) : null}
          {toggleSwitch}
        </div>
      </div>

      {showOverview ? (
        <div className="flex items-start gap-3 rounded-xl border border-border/60 bg-card/50 p-4">
          <div className="mt-0.5 text-muted-foreground">
            <Globe2 className="size-4" />
          </div>
          <p className="text-xs text-muted-foreground">
            Browser control ships with agentum&apos;s MCP server — every agent agentum launches can
            call the <code className="text-foreground">agentum_browser</code> tool to navigate,
            inspect, and act on pages. There&apos;s no skill to install.
          </p>
        </div>
      ) : null}

      {onOpenComputerUse ? (
        <div className="rounded-xl border border-border/60 bg-card/50 p-4">
          <div className="flex flex-col gap-3 sm:flex-row sm:items-start">
            <div className="min-w-0 flex-1 space-y-1">
              <p className="text-sm font-medium">Use an existing browser session</p>
              <p className="text-xs text-muted-foreground">
                If cookie import is not the right fit, Computer Use — also built into the agentum
                MCP — can control local apps and may use existing logged-in browser sessions where
                applicable. macOS requires privacy permissions.
              </p>
            </div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={onOpenComputerUse}
              className="shrink-0 gap-1.5 self-start"
            >
              <MousePointerClick className="size-3.5" />
              Open Computer Use
            </Button>
          </div>
        </div>
      ) : null}

      {showCookies ? (
        <SearchableSetting
          title="Import Browser Cookies"
          description="Import cookies from Chrome, Edge, or other browsers so agents can reuse your logins."
          keywords={BROWSER_USE_PANE_SEARCH_ENTRIES[1].keywords}
          className="rounded-xl border border-border/60 bg-card/50 p-4"
        >
          <div className="flex items-start gap-3">
            <div className="min-w-0 flex-1 space-y-1">
              <p className="text-sm font-medium">Import Browser Cookies</p>
              <p className="text-xs text-muted-foreground">
                Bring your existing logins into Agentum so agents can reach authenticated pages.
                Imports into the default profile.
              </p>
              {sourceLabel ? (
                <p className="text-[11px] text-muted-foreground">Last imported from {sourceLabel}</p>
              ) : null}
              {onConfigureMoreBrowsers ? (
                <button
                  type="button"
                  onClick={onConfigureMoreBrowsers}
                  className="text-[11px] text-muted-foreground underline underline-offset-2 hover:text-foreground"
                >
                  Manage profiles for separate logins
                </button>
              ) : null}
            </div>
            <DropdownMenu
              onOpenChange={(open) => {
                if (open) {
                  // Why: macOS treats other browsers' profile folders as app
                  // data. Only probe them when the user opens the import menu.
                  void fetchDetectedBrowsers()
                }
              }}
            >
              <DropdownMenuTrigger asChild>
                <Button
                  variant={cookiesImported ? 'outline' : 'default'}
                  size="sm"
                  disabled={isImportingDefault}
                  className="gap-1.5"
                >
                  {isImportingDefault ? (
                    <Loader2 className="size-3.5 animate-spin" />
                  ) : (
                    <Import className="size-3.5" />
                  )}
                  {cookiesImported ? 'Re-import' : 'Import'}
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                {detectedBrowsers.map((browser) =>
                  browser.profiles.length > 1 ? (
                    <DropdownMenuSub key={browser.family}>
                      <DropdownMenuSubTrigger>From {browser.label}</DropdownMenuSubTrigger>
                      <DropdownMenuPortal>
                        <DropdownMenuSubContent>
                          {browser.profiles.map((bp) => (
                            <DropdownMenuItem
                              key={bp.directory}
                              onSelect={() =>
                                void handleImportFromBrowser(browser.family, bp.directory)
                              }
                            >
                              {bp.name}
                            </DropdownMenuItem>
                          ))}
                        </DropdownMenuSubContent>
                      </DropdownMenuPortal>
                    </DropdownMenuSub>
                  ) : (
                    <DropdownMenuItem
                      key={browser.family}
                      onSelect={() => void handleImportFromBrowser(browser.family)}
                    >
                      From {browser.label}
                    </DropdownMenuItem>
                  )
                )}
                {detectedBrowsers.length > 0 ? <DropdownMenuSeparator /> : null}
                <DropdownMenuItem onSelect={() => void handleImportFromFile()}>
                  From File…
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </SearchableSetting>
      ) : null}

      <BrowserUseExamples />
    </div>
  )
}
