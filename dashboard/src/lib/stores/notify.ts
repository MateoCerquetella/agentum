import { writable } from 'svelte/store';

/**
 * Web Notifications API wrapper.
 *
 * Surfaces *OS-level* notifications for events the user wants to know
 * about even when the dashboard tab is hidden, the browser is in the
 * background, or the user is on another desktop. Distinct from the
 * in-app toast stack: toasts are visible while the tab is foregrounded;
 * these fire only when the user can't see a toast (hidden / unfocused).
 *
 * The decision tree:
 *   - permission denied  → no-op
 *   - permission default → no-op (the user has to opt in via Settings)
 *   - permission granted →
 *       - urgent (awaiting_input, crashed): always fire
 *       - normal (finished, compact):       fire only when page hidden
 *
 * The `tag` field deduplicates: a second `${sessionId}.awaiting_input`
 * notification replaces the first instead of stacking, so a chatty
 * agent can't pile up a row of duplicates in the OS notification
 * center.
 */

export type NotifyPermission = 'default' | 'granted' | 'denied' | 'unsupported';

/** Reactive permission state. Components watch this to gate the toggle. */
export const notifyPermission = writable<NotifyPermission>(currentPermission());

function currentPermission(): NotifyPermission {
  if (typeof Notification === 'undefined') return 'unsupported';
  return Notification.permission as NotifyPermission;
}

/** True when OS notifications are available AND the user has granted access. */
export function canNotify(): boolean {
  return currentPermission() === 'granted';
}

/** True when the API exists at all. Distinguishes `denied` (user said
 *  no) from `unsupported` (platform / browser doesn't ship the API). */
export function isSupported(): boolean {
  return typeof Notification !== 'undefined';
}

/**
 * Trigger the permission prompt. Resolves with the resulting state.
 * Safe to call from a non-user-gesture context — Safari requires a
 * gesture so callers should invoke this from a click handler.
 */
export async function requestPermission(): Promise<NotifyPermission> {
  if (!isSupported()) {
    notifyPermission.set('unsupported');
    return 'unsupported';
  }
  try {
    const result = await Notification.requestPermission();
    const next = result as NotifyPermission;
    notifyPermission.set(next);
    return next;
  } catch {
    return currentPermission();
  }
}

/** Whether the dashboard tab is in the foreground right now. */
function isPageActive(): boolean {
  if (typeof document === 'undefined') return true;
  if (document.visibilityState === 'hidden') return false;
  // hasFocus() is true for the active tab in the focused window only —
  // false when the user is on another window, another desktop, or
  // another browser tab. That's exactly the "user can't see a toast"
  // condition we want.
  if (typeof document.hasFocus === 'function' && !document.hasFocus()) return false;
  return true;
}

export interface NotifyOpts {
  title: string;
  body?: string;
  /** De-dup key. Subsequent notifications with the same tag replace the
   *  prior one instead of stacking up. */
  tag?: string;
  /** "Always alert" — fire even when the page is foregrounded. Use for
   *  permission prompts and crashes; the user can't miss those. */
  urgent?: boolean;
  /** Click handler. Default focuses the dashboard window. */
  onClick?: () => void;
}

/**
 * Fire an OS notification — best-effort, never throws.
 *
 * Returns `true` if a notification was actually shown, `false` if it
 * was skipped (no permission / page is foregrounded for a non-urgent
 * event / API call failed).
 */
export function notify(opts: NotifyOpts): boolean {
  if (!canNotify()) return false;
  if (!opts.urgent && isPageActive()) return false;
  try {
    const n = new Notification(opts.title, {
      body: opts.body,
      tag: opts.tag,
      // Always let the OS chime. The dashboard has no in-app audio
      // stack, so suppressing the OS sound for non-urgent kinds
      // (`agent.finished`, `watchdog.compact`) left the user with no
      // audible cue at all — which is exactly the moment they're
      // tabbed away and want to be poked. Urgent kinds
      // (`awaiting_input`, `crashed`) still get treated specially via
      // `urgent` (fire-when-foreground); audibility is separate.
      silent: false,
      // Use the favicon as the notification icon — modern browsers
      // auto-pick this when no `icon` is set; setting it explicitly
      // avoids a one-frame fallback to the browser default.
      icon: '/favicon.svg'
    });
    n.onclick = () => {
      try {
        window.focus();
        opts.onClick?.();
      } finally {
        n.close();
      }
    };
    // OS notifications auto-expire on most platforms; close defensively
    // after 12s so a stuck banner doesn't linger forever on Linux where
    // some notification daemons honour `requireInteraction` aggressively.
    if (!opts.urgent) {
      window.setTimeout(() => { try { n.close(); } catch { /* ignore */ } }, 12_000);
    }
    return true;
  } catch (e) {
    // Permission can flip mid-session (user revoked it in browser
    // settings). Refresh the cached state so the Settings UI re-renders
    // the toggle correctly on next paint.
    notifyPermission.set(currentPermission());
    if (typeof console !== 'undefined') console.warn('notify failed:', e);
    return false;
  }
}

/** Refresh the cached permission state. Call from layout onMount so the
 *  Settings page reflects whatever the user last toggled in browser
 *  preferences without forcing a hard reload. */
export function refreshPermission(): void {
  notifyPermission.set(currentPermission());
}
