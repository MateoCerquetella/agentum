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
//
// Frames are painted to a <canvas> via `createImageBitmap` (decodes off the main
// thread) on a `requestAnimationFrame` tick, keeping only the newest bitmap. This
// avoids the per-frame `URL.createObjectURL` churn + React `setState` the first
// cut used — both of which caused flicker, decode backlog, and GC pressure that
// made the stream feel like a laggy VPS. Mouse-move is likewise coalesced to one
// send per frame so a drag doesn't flood the input channel.
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
import AgentBrowserPickerOverlay from './AgentBrowserPickerOverlay'

/** Device scale the page lays out at, sent via `sendViewport`. Kept at 1× to match
 *  the server's 1× capture (`cdp_device_scale` → `--force-device-scale-factor`):
 *  capturing at 1× makes every JPEG frame ~4× smaller, so the stream is fast — the
 *  speed-over-sharpness default. MUST stay in lockstep with the server's launch
 *  scale, else the page lays out at one DPR but is captured at another (soft AND
 *  wasteful). Override both via `AGENTUM_CDP_DEVICE_SCALE=2` for a sharp, slower capture. */
const SCREENCAST_DEVICE_SCALE = 1

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
  /** Worktree + group the annotate picker's Send menu targets (which agents are
   *  eligible). Omit to hide the picker (no agent to send to). */
  worktreeId?: string
  groupId?: string
}

/** Map a client point onto the CDP device coordinate space the page expects.
 *  Mirrors the legacy pane's formula against the rendered <canvas> rect
 *  (`x = round(((clientX-rectLeft)/rectWidth)*deviceWidth)`). */
function toDevicePoint(
  event: { clientX: number; clientY: number },
  canvas: HTMLCanvasElement | null,
  metadata: BrowserScreencastFrameMetadata | null
): { x: number; y: number } | null {
  if (!canvas) {
    return null
  }
  const rect = canvas.getBoundingClientRect()
  // The canvas intrinsic size IS the frame's device size (we set it from the
  // decoded bitmap), so it's the natural fallback when metadata is absent.
  const deviceWidth = metadata?.deviceWidth ?? canvas.width
  const deviceHeight = metadata?.deviceHeight ?? canvas.height
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
  cdpPort,
  worktreeId,
  groupId
}: AgentBrowserScreencastPaneProps): React.JSX.Element {
  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  // The pane wrapper — what we measure for the render viewport. NEVER measure the
  // <canvas>: its intrinsic size IS the frame's pixel size (e.g. 402×9200), and a
  // replaced element with such an extreme aspect collapses in flex layout, so its
  // getBoundingClientRect reports a bogus width — which sendViewport then fed back
  // as the page's render width (the "renders at phone width, smeared across the
  // pane" bug). The container div has no intrinsic size; it's always the true pane.
  const containerRef = useRef<HTMLDivElement | null>(null)
  const subRef = useRef<CdpScreencastSubscription | null>(null)
  const metadataRef = useRef<BrowserScreencastFrameMetadata | null>(null)
  // Stable getter so the picker overlay's memoized handlers don't churn each render.
  const getScreencastMetadata = useCallback(() => metadataRef.current, [])
  // Bumped on every navigation so the annotate overlay drops markers whose clips
  // belong to the previous page (else they stay stuck over every later page).
  const [navToken, setNavToken] = useState(0)
  const bumpNav = useCallback((): void => setNavToken((n) => n + 1), [])
  // Newest decoded-but-not-yet-painted frame; the rAF tick draws and frees it.
  const pendingBitmapRef = useRef<ImageBitmap | null>(null)
  const drawRafRef = useRef<number | null>(null)
  // First-frame latch: flips the "Connecting…" overlay off exactly once (no
  // per-frame React render).
  const hasFrameRef = useRef(false)
  const [hasFrame, setHasFrame] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [addressBar, setAddressBar] = useState(toDisplayUrl(page.url))
  // Latest tab URL, read inside the open effect without making it a dep — which
  // would needlessly tear down and re-open the stream on every navigation.
  const pageUrlRef = useRef(page.url)
  pageUrlRef.current = page.url

  // Coalesced mouse-move: keep only the latest position and emit one send per
  // animation frame, so a fast drag doesn't spam the input channel (and the CDP
  // browser) with dozens of moves per frame.
  const pendingMoveRef = useRef<{ x: number; y: number } | null>(null)
  const moveRafRef = useRef<number | null>(null)

  // Send one browser.* interaction; a no-op while the stream is down.
  const sendInput = useCallback((method: string, params?: Record<string, unknown>): void => {
    subRef.current?.sendInput(method, params)
  }, [])

  // Match the headless page's LAYOUT viewport to the pane so the frame fills it
  // edge-to-edge (no letterbox/cropping). Without this the page lays out at the
  // launcher's fixed `--window-size=1280,800` and gets letterboxed in a differently-
  // shaped pane — this is what keeps the browser correctly sized as the pane resizes.
  // The deviceScaleFactor we send is SCREENCAST_DEVICE_SCALE so the page's DPR matches
  // the server's capture scale (`--force-device-scale-factor`); 1× keeps frames small
  // and fast. No-op until a subscription exists.
  const sendViewport = useCallback((): void => {
    // Measure the CONTAINER, never the <canvas> (see containerRef): the canvas box
    // tracks the frame's intrinsic pixel size and collapses in flex, which fed a
    // wrong width back as the render viewport.
    const el = containerRef.current
    if (!el) {
      return
    }
    const rect = el.getBoundingClientRect()
    const width = Math.round(rect.width)
    const height = Math.round(rect.height)
    if (width <= 0 || height <= 0) {
      return
    }
    sendInput('browser.setViewport', { width, height, deviceScaleFactor: SCREENCAST_DEVICE_SCALE })
  }, [sendInput])

  // Co-browse banner (F12): flips on briefly after the human interacts. The same
  // human input also gives the human the wheel server-side, so the agent's input
  // ops yield for a few seconds — this badge tells the user they're in control.
  const [driving, setDriving] = useState(false)
  const drivingTimerRef = useRef<number | null>(null)
  const markDriving = useCallback((): void => {
    setDriving(true)
    if (drivingTimerRef.current != null) {
      clearTimeout(drivingTimerRef.current)
    }
    drivingTimerRef.current = window.setTimeout(() => setDriving(false), 2500)
  }, [])

  // Paint the newest decoded frame on the next animation frame. Latest-wins: if
  // more frames decode before the tick fires, only the freshest is drawn.
  const scheduleDraw = useCallback((): void => {
    if (drawRafRef.current != null) {
      return
    }
    drawRafRef.current = requestAnimationFrame(() => {
      drawRafRef.current = null
      const bmp = pendingBitmapRef.current
      if (!bmp) {
        return
      }
      pendingBitmapRef.current = null
      const canvas = canvasRef.current
      if (!canvas) {
        bmp.close()
        return
      }
      if (canvas.width !== bmp.width || canvas.height !== bmp.height) {
        canvas.width = bmp.width
        canvas.height = bmp.height
      }
      // `drawImage` copies the pixels synchronously, so the bitmap can be freed
      // immediately afterward — no need to retain it across ticks.
      canvas.getContext('2d')?.drawImage(bmp, 0, 0)
      bmp.close()
      if (!hasFrameRef.current) {
        hasFrameRef.current = true
        setHasFrame(true)
      }
    })
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
      // quality 75 + 1× capture (SCREENCAST_DEVICE_SCALE) keep each frame small so
      // the stream stays fast — the speed-over-sharpness default the user asked for.
      // (Raise AGENTUM_CDP_DEVICE_SCALE to 2 for a sharper, heavier capture.)
      // everyNthFrame stays 1: dropping frames blanks a static page until its next
      // repaint. worktreeId attaches to THIS worktree's own browser (per-worktree
      // isolation) — the same instance its agent drives. cdpPort (SSH host) wins
      // over it when set.
      { cdpPort, worktreeId, format: 'jpeg', quality: 75, everyNthFrame: 1 },
      {
        onBinary: (bytes) => {
          if (disposed) {
            return
          }
          const frame = decodeBrowserScreencastFrame(bytes)
          if (!frame) {
            return
          }
          metadataRef.current = frame.metadata
          // Copy out of the shared buffer before handing it to createImageBitmap.
          const image = frame.image.slice()
          void createImageBitmap(new Blob([image], { type: `image/${frame.format}` }))
            .then((bmp) => {
              if (disposed) {
                bmp.close()
                return
              }
              // Latest-wins: a decoded frame not yet painted is now stale — free it.
              pendingBitmapRef.current?.close()
              pendingBitmapRef.current = bmp
              scheduleDraw()
            })
            .catch(() => {
              // A corrupt frame fails to decode — skip it, keep the stream alive.
            })
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
        // v1 single shared CDP page: drive it to THIS tab's URL on attach (the
        // page may sit on about:blank or a previous tab's URL). Subsequent in-tab
        // navigation flows through the address bar's browser.goto.
        const target = normalizeBrowserNavigationUrl(pageUrlRef.current)
        if (target) {
          sub.sendInput('browser.goto', { url: target })
        }
        // Size the page to the pane immediately so the first frames already fill
        // it (the ResizeObserver below keeps it in sync afterward).
        sendViewport()
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
      if (drawRafRef.current != null) {
        cancelAnimationFrame(drawRafRef.current)
        drawRafRef.current = null
      }
      if (moveRafRef.current != null) {
        cancelAnimationFrame(moveRafRef.current)
        moveRafRef.current = null
      }
      pendingBitmapRef.current?.close()
      pendingBitmapRef.current = null
      pendingMoveRef.current = null
    }
  }, [isActive, cdpPort, worktreeId, scheduleDraw, sendViewport])

  // Keep the page's layout viewport in sync with the pane size. Reuses the input
  // channel (no socket re-subscribe), rAF-coalesced so a drag-resize sends at most
  // one update per frame.
  useEffect(() => {
    if (!isActive) {
      return
    }
    const el = containerRef.current
    if (!el || typeof ResizeObserver === 'undefined') {
      return
    }
    let raf: number | null = null
    const observer = new ResizeObserver(() => {
      if (raf != null) {
        return
      }
      raf = requestAnimationFrame(() => {
        raf = null
        sendViewport()
      })
    })
    observer.observe(el)
    return () => {
      observer.disconnect()
      if (raf != null) {
        cancelAnimationFrame(raf)
      }
    }
  }, [isActive, sendViewport])

  // --- input handlers (forward to the same CDP instance the agent drives) -----

  // Emit the queued move immediately (cancelling the coalesce tick) — used before
  // discrete events so a press/scroll lands at the current cursor without a
  // one-frame lag.
  const sendMoveNow = useCallback(
    (p: { x: number; y: number }) => {
      pendingMoveRef.current = null
      if (moveRafRef.current != null) {
        cancelAnimationFrame(moveRafRef.current)
        moveRafRef.current = null
      }
      sendInput('browser.mouseMove', { x: p.x, y: p.y })
    },
    [sendInput]
  )

  const onMouseMove = useCallback(
    (e: React.MouseEvent) => {
      const p = toDevicePoint(e, canvasRef.current, metadataRef.current)
      if (!p) {
        return
      }
      pendingMoveRef.current = p
      if (moveRafRef.current == null) {
        moveRafRef.current = requestAnimationFrame(() => {
          moveRafRef.current = null
          const latest = pendingMoveRef.current
          if (latest) {
            sendInput('browser.mouseMove', { x: latest.x, y: latest.y })
          }
        })
      }
    },
    [sendInput]
  )

  const buttonName = (e: React.MouseEvent): 'left' | 'middle' | 'right' =>
    e.button === 1 ? 'middle' : e.button === 2 ? 'right' : 'left'

  const onMouseDown = useCallback(
    (e: React.MouseEvent) => {
      markDriving()
      const p = toDevicePoint(e, canvasRef.current, metadataRef.current)
      if (p) {
        // Move first so the press lands at the cursor (the server tracks the last
        // moved position for button events).
        sendMoveNow(p)
        sendInput('browser.mouseDown', { button: buttonName(e) })
      }
    },
    [sendInput, sendMoveNow, markDriving]
  )

  const onMouseUp = useCallback(
    (e: React.MouseEvent) => {
      sendInput('browser.mouseUp', { button: buttonName(e) })
    },
    [sendInput]
  )

  const onWheel = useCallback(
    (e: React.WheelEvent) => {
      markDriving()
      const p = toDevicePoint(e, canvasRef.current, metadataRef.current)
      if (p) {
        sendMoveNow(p)
        sendInput('browser.mouseWheel', { dx: e.deltaX, dy: e.deltaY })
      }
    },
    [sendInput, sendMoveNow, markDriving]
  )

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      markDriving()
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
    [sendInput, markDriving]
  )

  const navigate = useCallback(() => {
    const url = normalizeBrowserNavigationUrl(addressBar)
    if (url) {
      bumpNav()
      sendInput('browser.goto', { url })
    }
  }, [addressBar, sendInput, bumpNav])

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col bg-background">
      <div className="flex items-center gap-1 border-b border-border px-2 py-1">
        <button
          type="button"
          className="rounded px-2 py-0.5 text-xs text-muted-foreground hover:bg-muted"
          onClick={() => {
            bumpNav()
            sendInput('browser.back', {})
          }}
          aria-label="Back"
        >
          ←
        </button>
        <button
          type="button"
          className="rounded px-2 py-0.5 text-xs text-muted-foreground hover:bg-muted"
          onClick={() => {
            bumpNav()
            sendInput('browser.forward', {})
          }}
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

      <div
        ref={containerRef}
        className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden"
      >
        {error ? (
          <div className="px-4 text-center text-xs text-muted-foreground">{error}</div>
        ) : (
          <>
            {/* eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions */}
            <canvas
              ref={canvasRef}
              tabIndex={0}
              // Absolutely fill the (relative) container so the canvas box is ALWAYS the
              // pane size — decoupled from the frame's intrinsic pixel size, which
              // otherwise collapses the canvas in flex. object-contain preserves the
              // frame's aspect (never stretch/smear); since the render viewport is now
              // matched to the pane (sendViewport measures the container), the frame
              // fills the pane with no letterbox.
              className="absolute inset-0 h-full w-full object-contain outline-none"
              onMouseMove={onMouseMove}
              onMouseDown={onMouseDown}
              onMouseUp={onMouseUp}
              onWheel={onWheel}
              onKeyDown={onKeyDown}
              onContextMenu={(e) => e.preventDefault()}
            />
            {!hasFrame ? (
              <div className="absolute text-xs text-muted-foreground">
                Connecting to the agent browser…
              </div>
            ) : null}
            {driving ? (
              <div className="pointer-events-none absolute right-2 top-2 rounded bg-primary/80 px-2 py-0.5 text-[10px] font-medium text-primary-foreground">
                You're controlling
              </div>
            ) : null}
            {hasFrame && worktreeId ? (
              <AgentBrowserPickerOverlay
                canvasRef={canvasRef}
                getMetadata={getScreencastMetadata}
                worktreeId={worktreeId}
                groupId={groupId ?? worktreeId}
                pageUrl={page.url}
                navToken={navToken}
              />
            ) : null}
          </>
        )}
      </div>
    </div>
  )
}
