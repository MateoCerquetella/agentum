// Host-browser live view (spec 009a Phase 3, direct-WS transport).
//
// Renders the CDP screencast of a headless Chromium running ON a remote host:
// `startHostBrowser` launches/re-attaches it, `openHostBrowserScreencast` streams
// `0x62` JPEG frames here, and pointer/keyboard input is serialized straight back
// over the same WS (the scratch input protocol → CDP). No runtime-environments
// RPC broker — that path was removed in spec 007; this consumes the verified
// Phase-2 `/api/host-browser` routes directly.
import React, { useCallback, useEffect, useRef, useState } from 'react'

import type { BrowserScreencastFrameMetadata } from '../../shared/browser-screencast-protocol'
import {
  navigateHostBrowser,
  openHostBrowserScreencast,
  startHostBrowser,
  stopHostBrowser,
  type HostBrowserInput,
  type HostBrowserScreencast
} from '../../runtime/host-browser-client'
import { getRemoteBrowserKeypressKey } from '../browser-pane/remote-browser-keyboard'

type Props = {
  /** Host (SSH machine) UUID the browser runs on. */
  hostId: string
  /** Worktree dir on the host — scopes the browser (one per worktree). */
  workdir: string
  /** Optional URL to load on first launch (e.g. the host app's localhost:PORT). */
  initialUrl?: string
}

type Status = 'starting' | 'connecting' | 'live' | 'error'

function mouseButton(button: number): 'left' | 'middle' | 'right' | null {
  if (button === 0) return 'left'
  if (button === 1) return 'middle'
  if (button === 2) return 'right'
  return null
}

/** Map a pointer event on the displayed frame to the page's CSS-pixel viewport
 *  coordinates CDP expects (the frame is painted fill, so scale proportionally
 *  to the frame's reported device dimensions). */
function toViewportPoint(
  e: { clientX: number; clientY: number },
  img: HTMLImageElement,
  meta: BrowserScreencastFrameMetadata | null
): { x: number; y: number } {
  const rect = img.getBoundingClientRect()
  const fw = meta?.deviceWidth ?? img.naturalWidth ?? rect.width
  const fh = meta?.deviceHeight ?? img.naturalHeight ?? rect.height
  const x = rect.width > 0 ? ((e.clientX - rect.left) / rect.width) * fw : 0
  const y = rect.height > 0 ? ((e.clientY - rect.top) / rect.height) * fh : 0
  return { x: Math.round(x), y: Math.round(y) }
}

export function HostBrowserPane({ hostId, workdir, initialUrl }: Props): React.JSX.Element {
  const imgRef = useRef<HTMLImageElement | null>(null)
  const screencastRef = useRef<HostBrowserScreencast | null>(null)
  const metaRef = useRef<BrowserScreencastFrameMetadata | null>(null)
  // Revoke the previous object URL when a new frame replaces it (no leak).
  const lastObjectUrl = useRef<string | null>(null)

  const [status, setStatus] = useState<Status>('starting')
  const [error, setError] = useState<string | null>(null)
  const [id, setId] = useState<string | null>(null)
  const [urlInput, setUrlInput] = useState(initialUrl ?? '')

  useEffect(() => {
    let disposed = false
    setStatus('starting')
    setError(null)

    startHostBrowser(hostId, workdir)
      .then(async (started) => {
        if (disposed) return
        setId(started.id)
        setStatus('connecting')
        const screencast = await openHostBrowserScreencast(started.id, {
          onOpen: () => {
            if (!disposed) setStatus('live')
            // Load the host app on first attach if a URL was provided.
            if (initialUrl) {
              screencastRef.current?.sendInput({ type: 'navigate', url: initialUrl })
            }
          },
          onClose: () => {
            if (!disposed) setStatus('connecting')
          },
          onFrame: (frame) => {
            if (disposed) return
            metaRef.current = frame.metadata
            const img = imgRef.current
            if (!img) return
            // Uint8Array is a valid BlobPart at runtime; the cast satisfies the
            // generic `Uint8Array<ArrayBufferLike>` typing in newer TS libs.
            const blob = new Blob([frame.image as BlobPart], {
              type: frame.format === 'png' ? 'image/png' : 'image/jpeg'
            })
            const objectUrl = URL.createObjectURL(blob)
            img.src = objectUrl
            if (lastObjectUrl.current) {
              URL.revokeObjectURL(lastObjectUrl.current)
            }
            lastObjectUrl.current = objectUrl
          }
        })
        if (disposed) {
          screencast.close()
          return
        }
        screencastRef.current = screencast
      })
      .catch((e: unknown) => {
        if (disposed) return
        setStatus('error')
        setError(e instanceof Error ? e.message : String(e))
      })

    return () => {
      disposed = true
      // Close the WS but DO NOT stop the browser — it lives on the host and must
      // survive the Mac sleeping / agentum closing (spec 009a). Reopening
      // re-attaches via the deterministic per-worktree session.
      screencastRef.current?.close()
      screencastRef.current = null
      if (lastObjectUrl.current) {
        URL.revokeObjectURL(lastObjectUrl.current)
        lastObjectUrl.current = null
      }
    }
  }, [hostId, workdir, initialUrl])

  const send = useCallback((msg: HostBrowserInput) => {
    screencastRef.current?.sendInput(msg)
  }, [])

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLImageElement>) => {
      const img = imgRef.current
      const button = mouseButton(e.button)
      if (!img || !button) return
      const { x, y } = toViewportPoint(e, img, metaRef.current)
      send({ type: 'mouse', action: 'move', x, y })
      send({ type: 'mouse', action: 'down', x, y, button })
    },
    [send]
  )

  const onPointerUp = useCallback(
    (e: React.PointerEvent<HTMLImageElement>) => {
      const img = imgRef.current
      const button = mouseButton(e.button)
      if (!img || !button) return
      const { x, y } = toViewportPoint(e, img, metaRef.current)
      send({ type: 'mouse', action: 'up', x, y, button })
    },
    [send]
  )

  const onPointerMove = useCallback(
    (e: React.PointerEvent<HTMLImageElement>) => {
      const img = imgRef.current
      if (!img || e.buttons === 0) return // only stream moves while dragging
      const { x, y } = toViewportPoint(e, img, metaRef.current)
      send({ type: 'mouse', action: 'move', x, y })
    },
    [send]
  )

  const onWheel = useCallback(
    (e: React.WheelEvent<HTMLImageElement>) => {
      const img = imgRef.current
      if (!img) return
      const { x, y } = toViewportPoint(e, img, metaRef.current)
      send({ type: 'wheel', x, y, dx: e.deltaX, dy: e.deltaY })
    },
    [send]
  )

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      // Printable chars + named keys (Enter/arrows/…). Modifier shortcuts are a
      // later refinement (the scratch backend has no modifier-combo mapping yet).
      const key = getRemoteBrowserKeypressKey({
        key: e.key,
        metaKey: e.metaKey,
        ctrlKey: e.ctrlKey,
        altKey: e.altKey,
        shiftKey: e.shiftKey
      })
      if (key) {
        e.preventDefault()
        send({ type: 'key', key })
      }
    },
    [send]
  )

  const onNavigate = useCallback(
    (e: React.FormEvent) => {
      e.preventDefault()
      const url = urlInput.trim()
      if (!url || !id) return
      // Prefer in-band navigate (one CDP connection); fall back to the REST route.
      if (screencastRef.current) {
        send({ type: 'navigate', url })
      } else {
        void navigateHostBrowser(id, url)
      }
    },
    [urlInput, id, send]
  )

  const onStop = useCallback(() => {
    if (id) void stopHostBrowser(id)
  }, [id])

  return (
    <div
      style={{ display: 'flex', flexDirection: 'column', height: '100%', outline: 'none' }}
      tabIndex={0}
      onKeyDown={onKeyDown}
    >
      <form
        onSubmit={onNavigate}
        style={{ display: 'flex', gap: 8, padding: 8, alignItems: 'center' }}
      >
        <input
          type="text"
          value={urlInput}
          onChange={(e) => setUrlInput(e.target.value)}
          placeholder="http://localhost:3000"
          spellCheck={false}
          style={{ flex: 1 }}
        />
        <button type="submit">Go</button>
        <button type="button" onClick={onStop}>
          Stop
        </button>
        <span style={{ fontSize: 12, opacity: 0.7 }}>
          {status === 'live' ? '● live' : status === 'error' ? '✕ error' : '… ' + status}
        </span>
      </form>
      {error ? (
        <div style={{ padding: 8, color: 'var(--error, #c00)' }}>{error}</div>
      ) : null}
      <div style={{ flex: 1, minHeight: 0, background: '#111', position: 'relative' }}>
        {/* Frame is painted fill; pointer coords are scaled to device dims. */}
        <img
          ref={imgRef}
          alt="host browser"
          draggable={false}
          onPointerDown={onPointerDown}
          onPointerUp={onPointerUp}
          onPointerMove={onPointerMove}
          onWheel={onWheel}
          onContextMenu={(e) => e.preventDefault()}
          style={{ width: '100%', height: '100%', objectFit: 'fill', display: 'block' }}
        />
      </div>
    </div>
  )
}
