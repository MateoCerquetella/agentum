// In-agentum CDP screencast pane (009c-3): renders the agent-driven headless
// CDP-Chromium LIVE inside agentum, and lets the user click/type/scroll in it.
// The agent drives the SAME instance over the bound Playwright MCP, so what the
// user watches and what the agent controls are one browser.
//
// Transport is the embedded server's `WS /api/cdp-browser/screencast` bridge via
// `openCdpScreencast` (NOT the stubbed native runtime_environments path) — one
// path for local AND SSH-host browsers (only `cdpPort` differs; a host's port is
// the 009a `ssh -L` tunnel on 127.0.0.1). Reuses the FIXED `0x62` decoder and the
// `remote-browser-keyboard` serializer so it matches the server byte-for-byte.
import { useCallback, useEffect, useRef, useState } from 'react'
import type { BrowserPage } from '../../../../shared/types'
import {
  decodeBrowserScreencastFrame,
  type BrowserScreencastFrameMetadata
} from '../../../../shared/browser-screencast-protocol'
import {
  openCdpScreencast,
  type CdpScreencastSubscription
} from '../../runtime/cdp-screencast-client'
import {
  getRemoteBrowserKeypressKey,
  getRemoteBrowserKeyboardShortcut
} from './remote-browser-keyboard'
import { normalizeBrowserNavigationUrl } from '../../../../shared/browser-url'

/** Strip the `about:blank` placeholder so the address bar starts empty. */
function toDisplayUrl(url: string): string {
  return url === 'about:blank' ? '' : url
}

type AgentBrowserScreencastPaneProps = {
  page: BrowserPage
  isActive: boolean
  /** CDP port to attach to. Omit for the shared local browser (server default
   *  9300); set to the tunneled port for an SSH-host browser (009a). */
  cdpPort?: number
}

/** Map a value normalized to the rendered <img> rect onto the CDP device
 *  coordinate space the page expects. Mirrors the legacy pane's formula
 *  (`x = round(((clientX-rectLeft)/rectWidth)*deviceWidth)`). */
function toDevicePoint(
  event: { clientX: number; clientY: number },
  img: HTMLImageElement | null,
  metadata: BrowserScreencastFrameMetadata | null
): { x: number; y: number } | null {
  if (!img) {
    return null
  }
  const rect = img.getBoundingClientRect()
  const deviceWidth = metadata?.deviceWidth ?? img.naturalWidth
  const deviceHeight = metadata?.deviceHeight ?? img.naturalHeight
  if (rect.width <= 0 || rect.height <= 0 || deviceWidth <= 0 || deviceHeight <= 0) {
    return null
  }
  return {
    x: Math.round(((event.clientX - rect.left) / rect.width) * deviceWidth),
    y: Math.round(((event.clientY - rect.top) / rect.height) * deviceHeight)
  }
}

export default function AgentBrowserScreencastPane({
  page,
  isActive,
  cdpPort
}: AgentBrowserScreencastPaneProps): React.JSX.Element {
  const imgRef = useRef<HTMLImageElement | null>(null)
  const subRef = useRef<CdpScreencastSubscription | null>(null)
  const frameUrlRef = useRef<string | null>(null)
  const metadataRef = useRef<BrowserScreencastFrameMetadata | null>(null)
  const [frameUrl, setFrameUrl] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [addressBar, setAddressBar] = useState(toDisplayUrl(page.url))

  // Send one browser.* interaction; a no-op while the stream is down.
  const sendInput = useCallback((method: string, params?: Record<string, unknown>): void => {
    subRef.current?.sendInput(method, params)
  }, [])

  // Open the screencast when the pane is the active surface; tear it down when it
  // backgrounds or unmounts so a parked pane holds no socket or CDP screencast.
  useEffect(() => {
    if (!isActive) {
      return
    }
    let disposed = false
    setError(null)

    void openCdpScreencast(
      { cdpPort, format: 'jpeg', quality: 70, everyNthFrame: 1 },
      {
        onBinary: (bytes) => {
          if (disposed) {
            return
          }
          const frame = decodeBrowserScreencastFrame(bytes)
          if (!frame) {
            return
          }
          // Copy out of the shared buffer before wrapping in a Blob.
          const image = frame.image.slice()
          const url = URL.createObjectURL(new Blob([image], { type: `image/${frame.format}` }))
          const prev = frameUrlRef.current
          frameUrlRef.current = url
          metadataRef.current = frame.metadata
          setFrameUrl(url)
          if (prev) {
            URL.revokeObjectURL(prev)
          }
        },
        onError: (message) => {
          if (!disposed) {
            setError(message)
          }
        },
        onClose: () => {
          if (!disposed) {
            setError((e) => e ?? 'Agent browser stream closed.')
          }
        }
      }
    )
      .then((sub) => {
        if (disposed) {
          sub.close()
          return
        }
        subRef.current = sub
      })
      .catch((e: unknown) => {
        if (!disposed) {
          setError(e instanceof Error ? e.message : 'Could not open the agent browser stream.')
        }
      })

    return () => {
      disposed = true
      subRef.current?.close()
      subRef.current = null
      if (frameUrlRef.current) {
        URL.revokeObjectURL(frameUrlRef.current)
        frameUrlRef.current = null
      }
    }
  }, [isActive, cdpPort])

  // --- input handlers (forward to the same CDP instance the agent drives) -----

  const onMouseMove = useCallback(
    (e: React.MouseEvent) => {
      const p = toDevicePoint(e, imgRef.current, metadataRef.current)
      if (p) {
        sendInput('browser.mouseMove', { x: p.x, y: p.y })
      }
    },
    [sendInput]
  )

  const buttonName = (e: React.MouseEvent): 'left' | 'middle' | 'right' =>
    e.button === 1 ? 'middle' : e.button === 2 ? 'right' : 'left'

  const onMouseDown = useCallback(
    (e: React.MouseEvent) => {
      const p = toDevicePoint(e, imgRef.current, metadataRef.current)
      if (p) {
        // Move first so the press lands at the cursor (the server tracks the last
        // moved position for button events).
        sendInput('browser.mouseMove', { x: p.x, y: p.y })
        sendInput('browser.mouseDown', { button: buttonName(e) })
      }
    },
    [sendInput]
  )

  const onMouseUp = useCallback(
    (e: React.MouseEvent) => {
      sendInput('browser.mouseUp', { button: buttonName(e) })
    },
    [sendInput]
  )

  const onWheel = useCallback(
    (e: React.WheelEvent) => {
      const p = toDevicePoint(e, imgRef.current, metadataRef.current)
      if (p) {
        sendInput('browser.mouseMove', { x: p.x, y: p.y })
        sendInput('browser.mouseWheel', { dx: e.deltaX, dy: e.deltaY })
      }
    },
    [sendInput]
  )

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      const shortcut = getRemoteBrowserKeyboardShortcut(e)
      if (shortcut === 'Meta+r' || shortcut === 'Control+r') {
        e.preventDefault()
        sendInput('browser.reload', {})
        return
      }
      const key = getRemoteBrowserKeypressKey(e)
      if (key == null) {
        return
      }
      e.preventDefault()
      // The serializer emits 'Space' for the spacebar; the CDP bridge wants the
      // literal character for printable input.
      sendInput('browser.keypress', { key: key === 'Space' ? ' ' : key })
    },
    [sendInput]
  )

  const navigate = useCallback(() => {
    const url = normalizeBrowserNavigationUrl(addressBar)
    if (url) {
      sendInput('browser.goto', { url })
    }
  }, [addressBar, sendInput])

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col bg-background">
      <div className="flex items-center gap-1 border-b border-border px-2 py-1">
        <button
          type="button"
          className="rounded px-2 py-0.5 text-xs text-muted-foreground hover:bg-muted"
          onClick={() => sendInput('browser.back', {})}
          aria-label="Back"
        >
          ←
        </button>
        <button
          type="button"
          className="rounded px-2 py-0.5 text-xs text-muted-foreground hover:bg-muted"
          onClick={() => sendInput('browser.forward', {})}
          aria-label="Forward"
        >
          →
        </button>
        <button
          type="button"
          className="rounded px-2 py-0.5 text-xs text-muted-foreground hover:bg-muted"
          onClick={() => sendInput('browser.reload', {})}
          aria-label="Reload"
        >
          ⟳
        </button>
        <input
          className="min-w-0 flex-1 rounded border border-border bg-input px-2 py-0.5 text-xs"
          value={addressBar}
          onChange={(e) => setAddressBar(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              navigate()
            }
          }}
          spellCheck={false}
          aria-label="Agent browser address"
        />
      </div>

      <div className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden">
        {error ? (
          <div className="px-4 text-center text-xs text-muted-foreground">
            {error}
          </div>
        ) : frameUrl ? (
          // eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions
          <img
            ref={imgRef}
            src={frameUrl}
            alt="Agent browser"
            tabIndex={0}
            draggable={false}
            className="h-full w-full object-contain outline-none"
            onMouseMove={onMouseMove}
            onMouseDown={onMouseDown}
            onMouseUp={onMouseUp}
            onWheel={onWheel}
            onKeyDown={onKeyDown}
            onContextMenu={(e) => e.preventDefault()}
          />
        ) : (
          <div className="text-xs text-muted-foreground">Connecting to the agent browser…</div>
        )}
      </div>
    </div>
  )
}
