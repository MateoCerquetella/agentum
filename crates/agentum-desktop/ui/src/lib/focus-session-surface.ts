/**
 * Move keyboard focus onto the active session surface — the focused terminal's
 * hidden xterm textarea when a session is showing, otherwise the current view's
 * <main> region. Shared by the Cmd+E focus toggle (view.toggleSidebarFocus) and
 * the sidebar's Enter-to-open so "jump into the session" lives in one place
 * instead of being re-implemented per call site.
 *
 * Returns true when a surface was focused.
 */
export function focusActiveSessionSurface(): boolean {
  // Why scan, not querySelector: hidden terminal tabs/worktrees stay mounted as
  // display:none (and inert) slots, so the FIRST `.xterm-helper-textarea` in DOM
  // order is frequently an off-screen tab. Focusing a display:none / inert
  // element is a no-op, which would leave focus stuck in the sidebar. Pick the
  // first textarea that is actually laid out (offsetParent != null) and not
  // inside an inert subtree — i.e. the visible pane.
  const helpers = document.querySelectorAll<HTMLElement>('.xterm-helper-textarea')
  for (const helper of helpers) {
    if (helper.offsetParent !== null && helper.closest('[inert]') === null) {
      helper.focus()
      return true
    }
  }
  // Why: non-terminal views (Tasks, Activity, …) render their own <main>. These
  // regions are not natively focusable, so make the current one programmatically
  // focusable (VS Code's "focus editor group" trick) without adding it to the
  // Tab order.
  const main = document.querySelector<HTMLElement>('main')
  if (main) {
    if (!main.hasAttribute('tabindex')) {
      main.setAttribute('tabindex', '-1')
    }
    main.focus({ preventScroll: true })
    return true
  }
  return false
}

const WORKTREE_SIDEBAR_SELECTOR = '[data-worktree-sidebar]'

/**
 * Whether keyboard focus currently lives inside the worktree list. Used by the
 * focus toggle to decide which direction to flip.
 */
export function isFocusInWorktreeSidebar(): boolean {
  const active = document.activeElement
  return active instanceof Element && active.closest(WORKTREE_SIDEBAR_SELECTOR) !== null
}

/**
 * Move keyboard focus onto the worktree list container. The list is unmounted
 * while the sidebar is collapsed, so this retries across two animation frames to
 * outlast the reveal → React mount → DOM commit sequence.
 */
export function focusWorktreeSidebar(): void {
  const attempt = (): boolean => {
    const listbox = document.querySelector<HTMLElement>(WORKTREE_SIDEBAR_SELECTOR)
    if (listbox) {
      listbox.focus()
      return true
    }
    return false
  }
  if (attempt()) {
    return
  }
  requestAnimationFrame(() => {
    if (attempt()) {
      return
    }
    requestAnimationFrame(attempt)
  })
}
