/**
 * Per-session panel layout for the /terminals canvas.
 *
 * State is keyed by session id and persisted in localStorage so the user's
 * arrangement survives reloads. We persist {x, y, w, h, z} only — anything
 * more would couple us too tightly to the panel UI.
 */
import { writable } from 'svelte/store';

export interface PanelLayout {
  x: number;
  y: number;
  w: number;
  h: number;
  z: number;
}

const KEY = 'agentum_canvas_layout_v1';

type LayoutMap = Record<string, PanelLayout>;

function load(): LayoutMap {
  if (typeof localStorage === 'undefined') return {};
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return {};
    const v = JSON.parse(raw);
    return v && typeof v === 'object' ? (v as LayoutMap) : {};
  } catch {
    return {};
  }
}

function save(map: LayoutMap) {
  if (typeof localStorage === 'undefined') return;
  try { localStorage.setItem(KEY, JSON.stringify(map)); } catch { /* quota */ }
}

export const layouts = writable<LayoutMap>(load());

if (typeof window !== 'undefined') {
  layouts.subscribe(save);
}

/** Apply a partial patch to one session's layout. */
export function patchLayout(id: string, patch: Partial<PanelLayout>) {
  layouts.update((m) => {
    const cur = m[id];
    if (!cur) return m;
    return { ...m, [id]: { ...cur, ...patch } };
  });
}

/** Make sure every session id has a layout entry. New ones get tiled. */
export function ensureLayouts(ids: string[], opts?: { tileCols?: number; tileW?: number; tileH?: number }) {
  const cols = opts?.tileCols ?? 2;
  const w = opts?.tileW ?? 520;
  const h = opts?.tileH ?? 360;
  layouts.update((m) => {
    let next = m;
    let mutated = false;
    let placed = Object.keys(m).length;
    for (const id of ids) {
      if (next[id]) continue;
      const col = placed % cols;
      const row = Math.floor(placed / cols);
      next = {
        ...next,
        [id]: {
          x: 16 + col * (w + 16),
          y: 16 + row * (h + 16),
          w,
          h,
          z: placed + 1
        }
      };
      mutated = true;
      placed++;
    }
    return mutated ? next : m;
  });
}

/** Bring `id` to the top of the z-stack. */
export function bringToFront(id: string) {
  layouts.update((m) => {
    const max = Object.values(m).reduce((acc, l) => Math.max(acc, l.z), 0);
    const cur = m[id];
    if (!cur || cur.z === max) return m;
    return { ...m, [id]: { ...cur, z: max + 1 } };
  });
}

/** Reset to a fresh tiled arrangement for the given ids. */
export function resetLayout(ids: string[], opts?: { tileCols?: number; tileW?: number; tileH?: number }) {
  layouts.set({});
  ensureLayouts(ids, opts);
}
