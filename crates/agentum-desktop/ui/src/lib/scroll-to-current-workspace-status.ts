export const SCROLL_TO_CURRENT_WORKSPACE_REVEAL_REQUEST_EVENT =
  'agentum-scroll-to-current-workspace-reveal-request'

export function requestScrollToCurrentWorkspaceReveal(): void {
  if (typeof window === 'undefined') {
    return
  }
  window.dispatchEvent(new Event(SCROLL_TO_CURRENT_WORKSPACE_REVEAL_REQUEST_EVENT))
}
