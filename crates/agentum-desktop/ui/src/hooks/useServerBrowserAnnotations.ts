import { useEffect } from 'react'
import { toast } from 'sonner'
import {
  formatHeadedAnnotationForAgent,
  openBrowserAnnotationStream,
  type AnnotationStream
} from '../runtime/headed-browser-client'

/**
 * App-wide subscriber for annotations beaconed from a persistent (headed) Chrome
 * window. The headed window has no in-app tab, so its annotations can't ride the
 * per-tab WKWebView tray; instead the server rebroadcasts them on `/api/events` as
 * `browser.annotation` and we surface each one as a toast with a "Copy for agent"
 * action (the formatted change request the user pastes into their agent).
 *
 * Mounted once near the app root (next to `useIpcEvents`). The clipboard hand-off is
 * deliberately simple + target-agnostic; auto-routing to a specific worktree agent is
 * a later refinement (Phase 1b+).
 */
export function useServerBrowserAnnotations(): void {
  useEffect(() => {
    let stream: AnnotationStream | null = null
    let cancelled = false

    void openBrowserAnnotationStream((annotation) => {
      const prompt = formatHeadedAnnotationForAgent(annotation)
      if (!prompt) return
      toast.message('New browser annotation', {
        description: prompt.slice(0, 160),
        action: {
          label: 'Copy for agent',
          onClick: () => {
            void navigator.clipboard?.writeText(prompt).catch(() => {})
          }
        }
      })
    }).then((s) => {
      if (cancelled) {
        s.close()
        return
      }
      stream = s
    })

    return () => {
      cancelled = true
      stream?.close()
    }
  }, [])
}
