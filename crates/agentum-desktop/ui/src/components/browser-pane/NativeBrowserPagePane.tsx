import { useCallback, useEffect, useRef, useState } from 'react'
import { ArrowLeft, ArrowRight, Globe, Loader2, MessageSquarePlus, RefreshCw } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { api } from '@/tauri'
import { useAppStore } from '@/store'
import { AGENTUM_BROWSER_BLANK_URL } from '../../../../shared/constants'
import type { BrowserPage as BrowserPageState } from '../../../../shared/types'
import {
  normalizeBrowserNavigationUrl,
  redactKagiSessionToken
} from '../../../../shared/browser-url'
import BrowserAddressBar from './BrowserAddressBar'
import {
  isNativeBrowserOverlayOpen,
  useNativeBrowserOverlayOpen
} from './native-browser-overlay-suppression'

type BrowserTabPageState = Partial<
  Pick<
    BrowserPageState,
    'title' | 'loading' | 'faviconUrl' | 'canGoBack' | 'canGoForward' | 'loadError'
  >
>

// Native browser pane: the page itself renders in a Tauri child webview the
// Rust shell overlays on the main window (commands in
// crates/agentum-desktop/src/commands/browser_native.rs). This pane owns the
// toolbar chrome and keeps the native webview aligned with the container div.
// It serves both local and SSH-host worktrees — the page always renders on
// this machine; remote-localhost URLs need a port forward to resolve.

type NativeBrowserApi = {
  webviewOpen: (args: {
    browserPageId: string
    worktreeId: string
    url: string
    bounds: { x: number; y: number; width: number; height: number }
  }) => Promise<void>
  webviewNavigate: (args: { browserPageId: string; url: string }) => Promise<void>
  webviewHistory: (args: {
    browserPageId: string
    action: 'back' | 'forward' | 'reload'
  }) => Promise<void>
  webviewSetBounds: (args: {
    browserPageId: string
    bounds: { x: number; y: number; width: number; height: number }
  }) => Promise<void>
  webviewSetVisible: (args: { browserPageId: string; visible: boolean }) => Promise<void>
  webviewClose: (args: { browserPageId: string }) => Promise<void>
  onPageLoad: (
    callback: (payload: { browserPageId: string; event: 'started' | 'finished'; url: string }) => void
  ) => () => void
}

// The api.browser namespace synthesizes any non-explicit method through the
// defineNamespace proxy (webviewOpen -> browser_webview_open, onPageLoad ->
// "browser-page-load" event), so the native commands need no contract entry.
const nativeBrowser = api.browser as unknown as NativeBrowserApi

function toDisplayUrl(url: string): string {
  return url === AGENTUM_BROWSER_BLANK_URL ? '' : redactKagiSessionToken(url)
}

function titleForUrl(url: string): string {
  try {
    return new URL(url).host || url
  } catch {
    return url
  }
}

function isNavigableUrl(url: string): boolean {
  return Boolean(url) && url !== AGENTUM_BROWSER_BLANK_URL && url !== 'about:blank'
}

const BOUNDS_POLL_MS = 250

export default function NativeBrowserPagePane({
  browserTab,
  isActive,
  onUpdatePageState,
  onSetUrl
}: {
  browserTab: BrowserPageState
  isActive: boolean
  onUpdatePageState: (tabId: string, updates: BrowserTabPageState) => void
  onSetUrl: (tabId: string, url: string) => void
}): React.JSX.Element {
  const addressBarInputRef = useRef<HTMLInputElement | null>(null)
  const containerRef = useRef<HTMLDivElement | null>(null)
  const [addressBarValue, setAddressBarValue] = useState(toDisplayUrl(browserTab.url))
  const [nativeError, setNativeError] = useState<string | null>(null)
  const createdRef = useRef(false)
  const lastBoundsRef = useRef<{ x: number; y: number; width: number; height: number } | null>(
    null
  )
  const browserTabUrlRef = useRef(browserTab.url)
  browserTabUrlRef.current = browserTab.url
  const hasPage = isNavigableUrl(browserTab.url)
  // A native child webview always paints ABOVE the DOM, so any open overlay
  // (dropdown/menu/dialog/popover/select) would be hidden behind the page. Hide
  // the webview while one is open and restore it when they all close.
  const overlayOpen = useNativeBrowserOverlayOpen()

  const measureBounds = useCallback(():
    | { x: number; y: number; width: number; height: number }
    | null => {
    const container = containerRef.current
    if (!container) {
      return null
    }
    const rect = container.getBoundingClientRect()
    if (rect.width <= 0 || rect.height <= 0) {
      return null
    }
    return {
      x: Math.round(rect.left),
      y: Math.round(rect.top),
      width: Math.round(rect.width),
      height: Math.round(rect.height)
    }
  }, [])

  const navigateToUrl = useCallback(
    (url: string): void => {
      setNativeError(null)
      onSetUrl(browserTab.id, url)
      onUpdatePageState(browserTab.id, {
        loading: true,
        loadError: null,
        title: titleForUrl(url)
      })
      const run = async (): Promise<void> => {
        if (createdRef.current) {
          await nativeBrowser.webviewNavigate({ browserPageId: browserTab.id, url })
          return
        }
        const bounds = measureBounds()
        if (!bounds) {
          return
        }
        await nativeBrowser.webviewOpen({
          browserPageId: browserTab.id,
          worktreeId: browserTab.worktreeId,
          url,
          bounds
        })
        createdRef.current = true
        lastBoundsRef.current = bounds
      }
      run().catch((error: unknown) => {
        const message = error instanceof Error ? error.message : String(error)
        setNativeError(message)
        onUpdatePageState(browserTab.id, {
          loading: false,
          loadError: { code: 0, description: message, validatedUrl: url }
        })
      })
    },
    [browserTab.id, measureBounds, onSetUrl, onUpdatePageState]
  )

  const submitAddressBar = useCallback((): void => {
    const searchEngine = useAppStore.getState().browserDefaultSearchEngine
    const kagiSessionLink = useAppStore.getState().browserKagiSessionLink
    const nextUrl = normalizeBrowserNavigationUrl(addressBarValue, searchEngine, {
      kagiSessionLink
    })
    if (!nextUrl) {
      setNativeError('Enter a valid http(s) or localhost URL.')
      return
    }
    navigateToUrl(nextUrl)
  }, [addressBarValue, navigateToUrl])

  const runHistory = useCallback(
    (action: 'back' | 'forward' | 'reload'): void => {
      if (!createdRef.current) {
        return
      }
      void nativeBrowser.webviewHistory({ browserPageId: browserTab.id, action }).catch(() => {})
    },
    [browserTab.id]
  )

  // Mirror native page-load progress into the tab model (loading dot, title,
  // canonical URL after redirects).
  useEffect(() => {
    return nativeBrowser.onPageLoad((payload) => {
      if (payload.browserPageId !== browserTab.id) {
        return
      }
      const safeUrl = redactKagiSessionToken(payload.url)
      if (payload.event === 'started') {
        onUpdatePageState(browserTab.id, { loading: true })
        return
      }
      onSetUrl(browserTab.id, safeUrl)
      onUpdatePageState(browserTab.id, {
        loading: false,
        loadError: null,
        title: titleForUrl(safeUrl)
      })
      if (document.activeElement !== addressBarInputRef.current) {
        setAddressBarValue(toDisplayUrl(safeUrl))
      }
    })
  }, [browserTab.id, onSetUrl, onUpdatePageState])

  useEffect(() => {
    if (document.activeElement === addressBarInputRef.current) {
      return
    }
    setAddressBarValue(toDisplayUrl(browserTab.url))
  }, [browserTab.url])

  // Lifecycle: while this pane is the visible page, keep the native webview
  // shown and glued to the container; on unmount/tab-switch, hide it. Bounds
  // are polled because layout shifts (sidebar resize, panel toggles) move the
  // container without firing ResizeObserver on it.
  useEffect(() => {
    if (!isActive || !hasPage) {
      return
    }
    let cancelled = false
    const syncBounds = (): void => {
      if (cancelled || !createdRef.current) {
        return
      }
      const bounds = measureBounds()
      if (!bounds) {
        return
      }
      const last = lastBoundsRef.current
      if (
        last &&
        last.x === bounds.x &&
        last.y === bounds.y &&
        last.width === bounds.width &&
        last.height === bounds.height
      ) {
        return
      }
      lastBoundsRef.current = bounds
      void nativeBrowser
        .webviewSetBounds({ browserPageId: browserTab.id, bounds })
        .catch(() => {})
    }

    const ensureOpen = async (): Promise<void> => {
      const bounds = measureBounds()
      if (!bounds || cancelled) {
        return
      }
      await nativeBrowser.webviewOpen({
        browserPageId: browserTab.id,
        worktreeId: browserTab.worktreeId,
        url: browserTabUrlRef.current,
        bounds
      })
      if (cancelled) {
        return
      }
      createdRef.current = true
      lastBoundsRef.current = bounds
      // If an overlay opened during the async open, honor it immediately so the
      // freshly-shown webview doesn't flash over the menu/dialog.
      if (isNativeBrowserOverlayOpen()) {
        void nativeBrowser
          .webviewSetVisible({ browserPageId: browserTab.id, visible: false })
          .catch(() => {})
      }
    }
    void ensureOpen().catch((error: unknown) => {
      if (!cancelled) {
        setNativeError(error instanceof Error ? error.message : String(error))
      }
    })

    const interval = window.setInterval(syncBounds, BOUNDS_POLL_MS)
    const observer = new ResizeObserver(syncBounds)
    if (containerRef.current) {
      observer.observe(containerRef.current)
    }
    return () => {
      cancelled = true
      window.clearInterval(interval)
      observer.disconnect()
      if (createdRef.current) {
        void nativeBrowser
          .webviewSetVisible({ browserPageId: browserTab.id, visible: false })
          .catch(() => {})
      }
    }
  }, [browserTab.id, hasPage, isActive, measureBounds])

  // Toggle the native webview's visibility as overlays open/close. Separate from
  // the lifecycle effect above (which owns mount/unmount) so a menu or dialog
  // opening over the page hides the webview — DOM overlays can't render above a
  // native webview — and restores it when the last overlay closes.
  useEffect(() => {
    if (!createdRef.current) {
      // Not opened yet; the lifecycle effect's ensureOpen applies the correct
      // initial visibility (incl. the overlay-open guard) when it creates it.
      return
    }
    const shouldShow = !overlayOpen && isActive && hasPage
    void nativeBrowser
      .webviewSetVisible({ browserPageId: browserTab.id, visible: shouldShow })
      .catch(() => {})
  }, [overlayOpen, isActive, hasPage, browserTab.id])

  useEffect(() => {
    if (isActive && !hasPage) {
      addressBarInputRef.current?.focus()
    }
  }, [hasPage, isActive])

  return (
    <div className="relative flex h-full min-h-0 flex-1 flex-col">
      <div className="relative z-10 flex items-center gap-2 border-b border-border/70 bg-background/95 px-3 py-1.5">
        <Button
          size="icon"
          variant="ghost"
          className="h-7 w-7"
          disabled={!hasPage}
          onClick={() => runHistory('back')}
        >
          <ArrowLeft className="size-4" />
        </Button>
        <Button
          size="icon"
          variant="ghost"
          className="h-7 w-7"
          disabled={!hasPage}
          onClick={() => runHistory('forward')}
        >
          <ArrowRight className="size-4" />
        </Button>
        <Button
          size="icon"
          variant="ghost"
          className="h-7 w-7"
          disabled={!hasPage}
          onClick={() => runHistory('reload')}
        >
          {browserTab.loading ? (
            <Loader2 className="size-4 animate-spin" />
          ) : (
            <RefreshCw className="size-4" />
          )}
        </Button>
        <BrowserAddressBar
          value={addressBarValue}
          onChange={setAddressBarValue}
          onSubmit={submitAddressBar}
          onNavigate={navigateToUrl}
          inputRef={addressBarInputRef}
        />
        {/* Annotate: inject the in-page picker (orca-style). Lives on the native
            toolbar because this — not BrowserPane — is the rendered pane. */}
        <Button
          size="icon"
          variant="ghost"
          className="h-7 w-7"
          disabled={!hasPage || !isNavigableUrl(browserTab.url)}
          title="Annotate page element"
          aria-label="Annotate page element"
          onClick={() =>
            void api.browser.inpageAnnotate({ browserPageId: browserTab.id, enabled: true })
          }
        >
          <MessageSquarePlus className="size-4" />
        </Button>
      </div>
      <div ref={containerRef} className="relative min-h-0 flex-1 overflow-hidden bg-background">
        {!hasPage ? (
          <div className="absolute inset-0 flex items-center justify-center px-6 text-center">
            <div className="flex max-w-sm flex-col items-center gap-2">
              <Globe className="size-5 text-muted-foreground" />
              <div className="text-sm font-medium text-foreground">New Tab</div>
              <div className="text-xs leading-5 text-muted-foreground">
                Search or enter a URL to start browsing.
              </div>
            </div>
          </div>
        ) : null}
        {nativeError ? (
          <div className="absolute bottom-4 left-1/2 z-10 max-w-md -translate-x-1/2 rounded-md border border-border bg-popover px-3 py-2 text-xs text-popover-foreground shadow-md">
            {nativeError}
          </div>
        ) : null}
      </div>
    </div>
  )
}
