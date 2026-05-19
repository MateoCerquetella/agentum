/**
 * Aggregated kanban board across every configured profile.
 *
 * The dashboard's single-server board store (`board.ts`) only knows
 * about whichever profile the topbar EndpointSwitcher has active. The
 * TUI cycles through profiles via Ctrl-O and shows each server's work;
 * here we fan out `listBoardOn(p.id)` in parallel for every profile so
 * the user sees one unified board across all paired daemons. Each item
 * carries a `profile_id` tag so the page can route subsequent mutations
 * (drag, claim, edit, delete) back to the originating server.
 */
import { writable, get, derived } from 'svelte/store';
import { api, type BoardItem } from '$lib/api';
import { profiles } from '$lib/profiles';

export interface FleetItem extends BoardItem {
  /** Dashboard-side tag — names the paired daemon that owns this row. */
  profile_id: string;
}

interface FleetState {
  loading: boolean;
  /** Per-profile error messages; absent key ⇒ that profile loaded cleanly. */
  errors: Record<string, string>;
  /** Every board row from every profile, tagged with its source. */
  items: FleetItem[];
  /** Union of `column_order` across every profile; preserves first-seen order. */
  columnOrder: string[];
}

const initial: FleetState = {
  loading: false,
  errors: {},
  items: [],
  columnOrder: ['todo', 'doing', 'done']
};

export const fleetBoard = writable<FleetState>(initial);

/// Last load completed at — used by the page's safety-net interval to
/// avoid double-fetching when the event bridge already covered us.
let lastLoadedAt = 0;
export function fleetBoardLastLoadedAt(): number {
  return lastLoadedAt;
}

/**
 * Fan out `listBoardOn(p.id)` over every configured profile in
 * parallel. A failure on one profile doesn't drop the others — the
 * error is recorded under `errors[profile_id]` so the page can surface
 * it without blanking the rest of the board.
 */
export async function loadFleetBoard(): Promise<void> {
  const list = get(profiles);
  fleetBoard.update((s) => ({ ...s, loading: true }));

  const settled = await Promise.allSettled(
    list.map(async (p) => {
      const data = await api.listBoardOn(p.id);
      return { profile_id: p.id, data };
    })
  );

  const items: FleetItem[] = [];
  const errors: Record<string, string> = {};
  const orderSeen = new Set<string>();
  const columnOrder: string[] = [];

  for (let i = 0; i < settled.length; i++) {
    const r = settled[i];
    const p = list[i];
    if (r.status === 'fulfilled') {
      for (const col of r.value.data.column_order) {
        if (!orderSeen.has(col)) {
          orderSeen.add(col);
          columnOrder.push(col);
        }
      }
      for (const [col, rows] of Object.entries(r.value.data.columns)) {
        for (const row of rows) {
          items.push({ ...row, status: col, profile_id: r.value.profile_id });
        }
      }
    } else {
      errors[p.id] =
        r.reason instanceof Error ? r.reason.message : String(r.reason);
    }
  }

  // Guarantee the canonical three are always present in the chrome
  // even if no profile returned them (empty fleet, all errors, …).
  for (const c of ['todo', 'doing', 'done']) {
    if (!orderSeen.has(c)) columnOrder.push(c);
  }

  lastLoadedAt = Date.now();
  fleetBoard.set({ loading: false, errors, items, columnOrder });
}

/**
 * Optimistic local move. Mirrors `board.ts::moveLocal` but pins to the
 * originating profile so a drag onto a column that exists on a
 * *different* server doesn't blast the row across endpoints.
 */
export function moveLocalAcross(
  profileId: string,
  itemId: number,
  toStatus: string
): void {
  fleetBoard.update((s) => {
    let touched = false;
    const next = s.items.map((it) => {
      if (it.profile_id === profileId && it.id === itemId) {
        touched = true;
        return { ...it, status: toStatus };
      }
      return it;
    });
    if (!touched) return s;
    const columnOrder = s.columnOrder.includes(toStatus)
      ? s.columnOrder
      : [...s.columnOrder, toStatus];
    return { ...s, items: next, columnOrder };
  });
}

/**
 * Splice a server-confirmed item back into local state — used by the
 * dialog's `onCreated` / `onUpdated` callbacks so the column updates
 * before the ~250 ms WS-refetch lands.
 */
export function applyItem(profileId: string, item: BoardItem): void {
  fleetBoard.update((s) => {
    const tagged: FleetItem = { ...item, profile_id: profileId };
    const idx = s.items.findIndex(
      (it) => it.profile_id === profileId && it.id === item.id
    );
    const items =
      idx >= 0
        ? [...s.items.slice(0, idx), tagged, ...s.items.slice(idx + 1)]
        : [...s.items, tagged];
    const columnOrder = s.columnOrder.includes(tagged.status)
      ? s.columnOrder
      : [...s.columnOrder, tagged.status];
    return { ...s, items, columnOrder };
  });
}

export function removeItem(profileId: string, id: number): void {
  fleetBoard.update((s) => ({
    ...s,
    items: s.items.filter((it) => !(it.profile_id === profileId && it.id === id))
  }));
}

/**
 * Group items by status for the page's column renderer. Derived rather
 * than recomputed inline so it stays cheap when the page re-renders for
 * unrelated reasons (e.g. session strip ticks).
 */
export const fleetColumns = derived(fleetBoard, ($s) => {
  const byStatus = new Map<string, FleetItem[]>();
  for (const c of $s.columnOrder) byStatus.set(c, []);
  for (const it of $s.items) {
    const bucket = byStatus.get(it.status) ?? [];
    bucket.push(it);
    byStatus.set(it.status, bucket);
  }
  return $s.columnOrder.map((key) => ({
    key,
    items: byStatus.get(key) ?? []
  }));
});
