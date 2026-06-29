// In-pane element-annotate picker for the AGENT browser screencast (W2/W3).
//
// Self-contained on purpose: it layers OVER the screencast <canvas> and only
// intercepts pointer events while "armed", so the canvas's existing drive-the-agent
// handlers are untouched. Hovering hit-tests the element under the cursor (server
// `node_at_point`, clip only) and highlights it; clicking captures a sharp element
// screenshot + opens a comment card; committed annotations accumulate into a banner
// that sends them — comment + element context + screenshot path — to a chosen agent
// in the worktree (the same active-agent/new-agent menu diff-notes uses).
import { useCallback, useEffect, useRef, useState } from 'react'
import { MessageSquarePlus, Send, SquareTerminal, Trash2, X } from 'lucide-react'
import type { BrowserScreencastFrameMetadata } from '../../../../shared/browser-screencast-protocol'
import { cdpNodeAtPoint, type CdpNodeClip } from '../../runtime/cdp-screencast-client'
import { clipToOverlayRect, pointToDevice, type ElementClip } from './agent-browser-picker'
import {
  formatAgentBrowserAnnotationsAsMarkdown,
  type AgentBrowserAnnotation,
  type AgentBrowserAnnotationIntent
} from './agent-browser-annotation-output'
import {
  deriveWorktreeAgentSessions,
  type WorktreeAgentSession
} from '@/lib/worktree-agent-sessions'
import { listSessions, submitPromptToSession } from '@/runtime/agentum-server-client'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'

/** Debounce hover hit-tests: one server round-trip per ~90ms of cursor settle. */
const HOVER_DEBOUNCE_MS = 90

type DraftAnnotation = {
  clip: CdpNodeClip
  label: string
  intent: AgentBrowserAnnotationIntent
  comment: string
  screenshotPath?: string
  imageB64?: string
}

type AgentBrowserPickerOverlayProps = {
  /** The screencast canvas the picker layers over (for coordinate mapping). */
  canvasRef: { current: HTMLCanvasElement | null }
  /** Latest frame metadata (deviceWidth/Height = the page's CSS-viewport size). */
  getMetadata: () => BrowserScreencastFrameMetadata | null
  /** Worktree + group the Send menu targets (which agents are eligible). */
  worktreeId: string
  groupId: string
  /** Current page URL, for the annotation brief heading. */
  pageUrl: string
  /** Bumped by the pane on every navigation (address bar / back / forward / reload
   *  / attach). Markers are positioned in the OLD page's viewport coords, so the
   *  overlay drops them when this changes — otherwise they stay stuck over every
   *  later page. */
  navToken: number
}

// Monotonic id source for annotation keys/markers (no Date.now/Math.random needed).
let annotationSeq = 0

export default function AgentBrowserPickerOverlay({
  canvasRef,
  getMetadata,
  worktreeId,
  pageUrl,
  navToken
}: AgentBrowserPickerOverlayProps): React.JSX.Element {
  const [armed, setArmed] = useState(false)
  const [hover, setHover] = useState<{ clip: CdpNodeClip; label: string } | null>(null)
  const [draft, setDraft] = useState<DraftAnnotation | null>(null)
  const [annotations, setAnnotations] = useState<AgentBrowserAnnotation[]>([])
  const [sendOpen, setSendOpen] = useState(false)
  // "Send to an agent" targets come from the SERVER's session list — every pane the
  // daemon owns (tmux/MCP/board-spawned agents), not just ones open as desktop terminal
  // tabs. That renderer-tab-only source was why the menu showed "No running agents here"
  // even with an agent running on the worktree. Captured when the menu opens so the
  // list is stable while it's up; delivery goes through the server's robust two-step
  // REPL injection (`submitPromptToSession`).
  const [sendTargets, setSendTargets] = useState<WorktreeAgentSession[]>([])
  const [sendError, setSendError] = useState<string | null>(null)
  const hoverTimerRef = useRef<number | null>(null)
  // One hit-test in flight at a time: hover fires often; don't pile up requests.
  const busyRef = useRef(false)

  // Current canvas display box + the frame's CSS-viewport size, for the coordinate
  // map. Null until a frame has arrived (no metadata) — overlays then don't render.
  const frameDims = useCallback((): { rect: DOMRect; dw: number; dh: number } | null => {
    const canvas = canvasRef.current
    const md = getMetadata()
    if (!canvas) {
      return null
    }
    const rect = canvas.getBoundingClientRect()
    const dw = md?.deviceWidth ?? canvas.width
    const dh = md?.deviceHeight ?? canvas.height
    if (rect.width <= 0 || rect.height <= 0 || dw <= 0 || dh <= 0) {
      return null
    }
    return { rect, dw, dh }
  }, [canvasRef, getMetadata])

  const disarm = useCallback((): void => {
    setArmed(false)
    setHover(null)
    setDraft(null)
    if (hoverTimerRef.current != null) {
      window.clearTimeout(hoverTimerRef.current)
      hoverTimerRef.current = null
    }
  }, [])

  const onMouseMove = useCallback(
    (e: React.MouseEvent): void => {
      // While a draft card is open we freeze the selection; don't re-hit-test.
      if (!armed || draft) {
        return
      }
      const f = frameDims()
      if (!f) {
        return
      }
      const p = pointToDevice(e.clientX, e.clientY, f.rect, f.dw, f.dh)
      if (!p) {
        return
      }
      if (hoverTimerRef.current != null) {
        window.clearTimeout(hoverTimerRef.current)
      }
      hoverTimerRef.current = window.setTimeout(() => {
        if (busyRef.current) {
          return
        }
        busyRef.current = true
        void cdpNodeAtPoint(p.x, p.y, false, { worktreeId })
          .then((r) => {
            setHover(r.ok ? { clip: r.clip, label: r.label } : null)
          })
          .finally(() => {
            busyRef.current = false
          })
      }, HOVER_DEBOUNCE_MS)
    },
    [armed, draft, frameDims, worktreeId]
  )

  const onClick = useCallback(
    (e: React.MouseEvent): void => {
      // Ignore clicks while a draft card is open — the user must Add or Cancel first.
      if (!armed || draft) {
        return
      }
      e.preventDefault()
      e.stopPropagation()
      const f = frameDims()
      if (!f) {
        return
      }
      const p = pointToDevice(e.clientX, e.clientY, f.rect, f.dw, f.dh)
      if (!p) {
        return
      }
      setHover(null)
      // capture:true → a sharp element PNG in the same round-trip for the thumbnail.
      void cdpNodeAtPoint(p.x, p.y, true, { worktreeId }).then((r) => {
        if (r.ok) {
          setDraft({
            clip: r.clip,
            label: r.label,
            intent: 'change',
            comment: '',
            screenshotPath: r.path,
            imageB64: r.image_b64
          })
        }
      })
    },
    [armed, draft, frameDims, worktreeId]
  )

  const commitDraft = useCallback((): void => {
    setDraft((d) => {
      if (!d || d.comment.trim().length === 0) {
        return d
      }
      annotationSeq += 1
      const annotation: AgentBrowserAnnotation = {
        id: `agent-annotation-${annotationSeq}`,
        label: d.label,
        intent: d.intent,
        comment: d.comment,
        clip: { x: d.clip.x, y: d.clip.y, width: d.clip.width, height: d.clip.height },
        ...(d.screenshotPath ? { screenshotPath: d.screenshotPath } : {})
      }
      setAnnotations((prev) => [...prev, annotation])
      return null
    })
  }, [])

  // Esc: cancel the in-progress draft → else clear any pending markers + disarm.
  // Active whenever there's ANYTHING to clear (not just while armed) so a stuck
  // marker is always dismissable with one keypress. Gated so we don't capture Esc
  // app-wide when the picker is idle.
  useEffect(() => {
    if (!armed && !draft && annotations.length === 0) {
      return
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') {
        return
      }
      if (draft) {
        setDraft(null)
        return
      }
      setAnnotations([])
      setHover(null)
      disarm()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [armed, draft, annotations.length, disarm])

  // Drop markers/draft/hover when the page navigates: their clips are in the OLD
  // page's viewport coordinate space, so without this they float, stuck, over every
  // subsequent page. Keeps annotations bound to the page they were made on.
  useEffect(() => {
    setAnnotations([])
    setDraft(null)
    setHover(null)
  }, [navToken, pageUrl])

  const f = frameDims()
  const overlayRectFor = (clip: ElementClip): ReturnType<typeof clipToOverlayRect> => {
    if (!f) {
      return null
    }
    return clipToOverlayRect(clip, { left: 0, top: 0, width: f.rect.width, height: f.rect.height }, f.dw, f.dh)
  }

  const prompt = formatAgentBrowserAnnotationsAsMarkdown(annotations, pageUrl)
  const hoverRect = armed && hover && !draft ? overlayRectFor(hover.clip) : null
  const draftRect = draft ? overlayRectFor(draft.clip) : null

  return (
    <>
      {/* Pointer-intercepting highlight layer — active ONLY while armed, so the
          canvas keeps driving the agent the rest of the time. */}
      <div
        className={
          armed
            ? 'absolute inset-0 cursor-crosshair'
            : 'pointer-events-none absolute inset-0'
        }
        onMouseMove={onMouseMove}
        onClick={onClick}
      >
        {hoverRect ? (
          <div
            className="pointer-events-none absolute rounded-sm border-2 border-violet-400 bg-violet-400/10"
            style={{ left: hoverRect.left, top: hoverRect.top, width: hoverRect.width, height: hoverRect.height }}
          />
        ) : null}
        {annotations.map((annotation, index) => {
          const r = overlayRectFor(annotation.clip)
          if (!r) {
            return null
          }
          return (
            <div
              key={annotation.id}
              className="pointer-events-none absolute rounded-sm border border-violet-500/70 bg-violet-500/5"
              style={{ left: r.left, top: r.top, width: r.width, height: r.height }}
            >
              <span className="absolute -left-2 -top-2 flex h-4 w-4 items-center justify-center rounded-full bg-violet-500 text-[9px] font-semibold text-white">
                {index + 1}
              </span>
            </div>
          )
        })}
      </div>

      {/* Annotate toggle — always interactive, top-left (clear of the right-side
          "You're controlling" badge). */}
      <button
        type="button"
        onClick={() => (armed ? disarm() : setArmed(true))}
        className={`absolute left-2 top-2 z-10 flex items-center gap-1 rounded px-2 py-0.5 text-[11px] font-medium shadow-sm ${
          armed
            ? 'bg-violet-500 text-white'
            : 'bg-background/80 text-muted-foreground hover:text-foreground'
        }`}
      >
        <MessageSquarePlus className="size-3" />
        {armed ? 'Annotating — Esc to stop' : 'Annotate'}
      </button>

      {/* Comment card for the in-progress draft. */}
      {draft ? (
        <div
          className="absolute z-20 w-64 rounded-md border border-border bg-popover p-2 shadow-lg"
          style={{
            left: Math.max(8, Math.min(draftRect?.left ?? 12, (f?.rect.width ?? 280) - 264)),
            top: Math.max(
              8,
              Math.min((draftRect ? draftRect.top + draftRect.height + 6 : 12), (f?.rect.height ?? 180) - 168)
            )
          }}
        >
          <div className="mb-1 flex items-center justify-between gap-2">
            <span className="truncate text-[11px] font-medium text-muted-foreground">
              {draft.label || 'element'}
            </span>
            <button
              type="button"
              onClick={() => setDraft(null)}
              className="text-muted-foreground hover:text-foreground"
              aria-label="Cancel annotation"
            >
              <X className="size-3" />
            </button>
          </div>
          {draft.imageB64 ? (
            <img
              src={`data:image/png;base64,${draft.imageB64}`}
              alt="Selected element"
              className="mb-1 max-h-24 w-full rounded border border-border object-contain"
            />
          ) : null}
          <div className="mb-1 flex gap-1">
            {(['change', 'question'] as const).map((intent) => (
              <button
                key={intent}
                type="button"
                onClick={() => setDraft((d) => (d ? { ...d, intent } : d))}
                className={`rounded px-1.5 py-0.5 text-[10px] capitalize ${
                  draft.intent === intent ? 'bg-violet-500 text-white' : 'bg-muted text-muted-foreground'
                }`}
              >
                {intent}
              </button>
            ))}
          </div>
          <textarea
            autoFocus
            value={draft.comment}
            onChange={(e) => setDraft((d) => (d ? { ...d, comment: e.target.value } : d))}
            placeholder="What should change here?"
            className="mb-1 h-14 w-full resize-none rounded border border-border bg-input p-1 text-[11px] outline-none"
          />
          <div className="flex justify-end gap-1">
            <button
              type="button"
              onClick={() => setDraft(null)}
              className="rounded px-2 py-0.5 text-[11px] text-muted-foreground hover:text-foreground"
            >
              Cancel
            </button>
            <button
              type="button"
              disabled={draft.comment.trim().length === 0}
              onClick={commitDraft}
              className="rounded bg-violet-500 px-2 py-0.5 text-[11px] font-medium text-white disabled:opacity-40"
            >
              Add
            </button>
          </div>
        </div>
      ) : null}

      {/* Pending-annotations banner with the Send menu. */}
      {annotations.length > 0 ? (
        <div className="absolute bottom-2 left-1/2 z-10 flex -translate-x-1/2 items-center gap-2 rounded-md border border-border bg-popover px-2 py-1 shadow-lg">
          <span className="text-[11px] text-muted-foreground">
            {sendError ? (
              <span className="text-red-400">{sendError}</span>
            ) : (
              `${annotations.length} annotation${annotations.length === 1 ? '' : 's'} ready`
            )}
          </span>
          <DropdownMenu
            modal={false}
            open={sendOpen}
            onOpenChange={(open) => {
              setSendOpen(open)
              if (open) {
                setSendError(null)
                // Pull the worktree's running agents from the SERVER (so tmux/MCP-spawned
                // agents appear, not only open terminal tabs); captured for the menu's life.
                void listSessions()
                  .then((sessions) =>
                    setSendTargets(deriveWorktreeAgentSessions(sessions, worktreeId))
                  )
                  .catch(() => setSendTargets([]))
              }
            }}
          >
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                className="flex items-center gap-1 rounded bg-violet-500 px-2 py-0.5 text-[11px] font-medium text-white"
              >
                <Send className="size-3" />
                Send
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent
              align="end"
              className="min-w-[220px]"
              onInteractOutside={preventAgentSendTargetOutsideDismiss}
              onPointerDownOutside={preventAgentSendTargetOutsideDismiss}
            >
              <DropdownMenuLabel>Send to an agent in this worktree</DropdownMenuLabel>
              {sendTargets.length === 0 ? (
                <DropdownMenuItem disabled className="text-[11px] text-muted-foreground">
                  No running agents here — start an agent in this worktree.
                </DropdownMenuItem>
              ) : (
                sendTargets.map((target) => (
                  <DropdownMenuItem
                    key={target.sessionId}
                    data-agent-send-target="eligible"
                    onSelect={() => {
                      setSendOpen(false)
                      // Deliver through the server's robust two-step REPL injection so a
                      // multi-line prompt actually executes; clearing the banner is the
                      // "sent" confirmation, an error keeps the annotations and shows why.
                      void submitPromptToSession(target.sessionId, prompt)
                        .then(() => setAnnotations([]))
                        .catch((e: unknown) =>
                          setSendError(
                            e instanceof Error ? e.message : 'Could not send to the agent.'
                          )
                        )
                    }}
                    className="gap-2 text-[12px]"
                  >
                    <SquareTerminal className="size-3.5 shrink-0" />
                    <span className="truncate">{target.label}</span>
                  </DropdownMenuItem>
                ))
              )}
            </DropdownMenuContent>
          </DropdownMenu>
          <button
            type="button"
            onClick={() => setAnnotations([])}
            className="text-muted-foreground hover:text-foreground"
            aria-label="Clear annotations"
          >
            <Trash2 className="size-3" />
          </button>
        </div>
      ) : null}
    </>
  )
}

/** Keep the send dropdown open when an outside interaction lands on a mirrored
 *  agent-send-target row (the sidebar rows `revealWorktreeInSidebar` renders when the
 *  menu opens). Even a `modal={false}` Radix menu dismisses on that interaction
 *  without this guard, so no target could be selected. Mirrors the same guard in
 *  NotesSendMenu / BrowserPane (the two send-menu surfaces that already work). */
function preventAgentSendTargetOutsideDismiss(event: CustomEvent<{ originalEvent: Event }>) {
  const target = event.detail.originalEvent.target
  if (!(target instanceof Element)) {
    return
  }
  if (
    target.closest(
      '[data-agent-send-target="eligible"], [data-agent-send-target="disabled"], [data-agent-send-target="sending"]'
    )
  ) {
    event.preventDefault()
  }
}
