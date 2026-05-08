/**
 * Theme synchronisation bridge between the dashboard and the TUI.
 *
 * Source of truth lives on the server in `<data_dir>/preferences.json`,
 * exposed by `/api/preferences`. The TUI mirrors its theme name into the
 * legacy `<data_dir>/theme` file too, so older `agentum terminal`
 * launches still pick up the user's pick on next start.
 *
 * Direction of flow:
 *
 * - On dashboard mount we GET preferences. If the server already knows
 *   a theme (set by another tab or by the TUI), the dashboard adopts
 *   it before paint.
 * - Whenever the user changes theme in the dashboard we PUT the new
 *   value (and the mapped TUI name).
 * - The server fans out a `preferences.changed` event over the bus on
 *   every PUT. The events store applies it live so a theme switch in
 *   the TUI reflects in this tab without a refresh.
 */

import { get } from 'svelte/store';
import { api, type Preferences } from '$lib/api';
import { tweaks, setTheme } from '$stores/tweaks';
import { THEMES } from '$stores/themes';

/** Best-fit pairing between dashboard theme ids and TUI theme names.
 *  The TUI ships six built-ins (system, midnight, dusk, slate, paper,
 *  mono); the dashboard ships ~14. We pair on visual mood — Dracula /
 *  Tokyo Night / One Dark all feel "midnight"-ish, Solarized Light and
 *  GitHub Light land on "paper", etc. The reverse map below converts
 *  TUI names back to a representative dashboard theme. */
const DASHBOARD_TO_TUI: Record<string, string> = {
  default: 'midnight',
  dracula: 'midnight',
  'tokyo-night': 'midnight',
  monokai: 'dusk',
  nord: 'slate',
  'gruvbox-dark': 'dusk',
  'one-dark': 'dusk',
  matrix: 'slate',
  retro: 'slate',
  synthwave: 'midnight',
  'github-light': 'paper',
  'solarized-light': 'paper',
  'gruvbox-light': 'paper',
  'one-light': 'paper'
};

const TUI_TO_DASHBOARD: Record<string, string> = {
  system: 'default',
  midnight: 'tokyo-night',
  dusk: 'one-dark',
  slate: 'nord',
  paper: 'github-light',
  mono: 'default'
};

export function tuiThemeForDashboard(id: string): string {
  return DASHBOARD_TO_TUI[id] ?? 'midnight';
}

export function dashboardThemeForTui(name: string): string {
  return TUI_TO_DASHBOARD[name] ?? 'default';
}

let lastSent: string | null = null;

/** Fetch the persisted preferences and apply the theme locally if the
 *  server's value differs from what we currently have. Safe to call
 *  before auth — the wrapper just rejects and we ignore the error. */
export async function pullPreferences(): Promise<void> {
  let prefs: Preferences;
  try {
    prefs = await api.getPreferences();
  } catch {
    return; // unauthenticated, offline, or older daemon — fail silent.
  }
  applyServerPrefs(prefs);
}

/** Apply a Preferences payload received from either the GET response
 *  or an SSE/WS `preferences.changed` event. Adopts the dashboard
 *  theme verbatim when present, otherwise falls back to the mapped
 *  TUI theme — that covers the case where the TUI wrote first and we
 *  never persisted a dashboard id. */
export function applyServerPrefs(prefs: Preferences): void {
  const desired = prefs.theme
    ?? (prefs.tui_theme ? dashboardThemeForTui(prefs.tui_theme) : null);
  if (!desired) return;
  if (!THEMES.some(t => t.id === desired)) return;
  const cur = get(tweaks).theme;
  if (cur === desired) return;
  // Mark this id as "received" so the immediate $tweaks subscription
  // doesn't bounce it back to the server in a tight loop.
  lastSent = desired;
  setTheme(desired);
}

/** Push the current dashboard theme to the server. Called from a
 *  tweaks-store subscription so every user action propagates. */
export async function pushTheme(themeId: string): Promise<void> {
  if (lastSent === themeId) {
    // We just adopted this from the server — don't echo it right back.
    lastSent = null;
    return;
  }
  lastSent = themeId;
  try {
    await api.putPreferences({
      theme: themeId,
      tui_theme: tuiThemeForDashboard(themeId)
    });
  } catch {
    // Silent: the TUI cache is best-effort. localStorage still has
    // the latest pick so the dashboard itself stays correct.
  }
}

let started = false;

/** Wire a one-shot subscription that pushes every subsequent theme
 *  change to the server. Idempotent — calling more than once is a
 *  no-op. Pair with `pullPreferences()` at startup. */
export function startThemeBridge(): void {
  if (started) return;
  started = true;
  let prev = get(tweaks).theme;
  tweaks.subscribe((t) => {
    if (t.theme !== prev) {
      prev = t.theme;
      void pushTheme(t.theme);
    }
  });
}
