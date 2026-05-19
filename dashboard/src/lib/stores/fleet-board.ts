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
import { profiles, activeProfileId } from '$lib/profiles';
import { projectOf } from '$lib/dashboard';

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
  /** Per-ticket comment count keyed by `${profile_id}:${id}`. */
  commentCounts: Record<string, number>;
}

const initial: FleetState = {
  loading: false,
  errors: {},
  items: [],
  columnOrder: ['todo', 'doing', 'done'],
  commentCounts: {}
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
  const commentCounts: Record<string, number> = {};

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
      // Splice per-profile comment counts into the global map. Key is
      // `${profile_id}:${id}` to match the dashboard's tagging scheme.
      if (r.value.data.comment_counts) {
        for (const [idStr, n] of Object.entries(r.value.data.comment_counts)) {
          commentCounts[`${r.value.profile_id}:${idStr}`] = n;
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
  fleetBoard.set({ loading: false, errors, items, columnOrder, commentCounts });
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
  fleetBoard.update((s) => {
    const key = `${profileId}:${id}`;
    const { [key]: _dropped, ...remaining } = s.commentCounts;
    return {
      ...s,
      items: s.items.filter((it) => !(it.profile_id === profileId && it.id === id)),
      commentCounts: remaining
    };
  });
}

/// Bump the local comment-count cache by `delta` for a given ticket.
/// Used by the dialog after a successful POST so the card's chip
/// updates immediately, ahead of the next loadFleetBoard refresh.
export function bumpCommentCount(profileId: string, id: number, delta: number): void {
  fleetBoard.update((s) => {
    const key = `${profileId}:${id}`;
    const next = Math.max(0, (s.commentCounts[key] ?? 0) + delta);
    return { ...s, commentCounts: { ...s.commentCounts, [key]: next } };
  });
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

/// Swimlane shape: one row per (server, project) pair. Within each
/// lane the items are bucketed by status so the page renders a 3-column
/// strip beneath each lane header. The user picked "server first, then
/// directory" as the grouping order, so we sort accordingly.
export interface Lane {
  profile_id: string;
  profile_label: string;
  project: string;
  workdir: string | null;
  total: number;
  byStatus: Record<string, FleetItem[]>;
}

const NO_PROJECT = '(no workdir)';

/// Lane derivation. Active profile floats to the top, then alphabetical
/// by label. Within a profile, projects are alphabetical with the
/// no-workdir bucket pinned last so it doesn't dominate the visual top.
export const fleetLanes = derived(
  [fleetBoard, profiles, activeProfileId],
  ([$s, $profiles, $active]): Lane[] => {
    const byProfileProj = new Map<string, Lane>();
    const labelFor = (id: string): string => {
      const p = $profiles.find((x) => x.id === id);
      if (!p) return id;
      return p.baseUrl ? p.label : 'local';
    };
    for (const it of $s.items) {
      const proj = it.workdir ? projectOf(it.workdir) : NO_PROJECT;
      const key = `${it.profile_id}::${proj}`;
      let lane = byProfileProj.get(key);
      if (!lane) {
        const byStatus: Record<string, FleetItem[]> = {};
        for (const c of $s.columnOrder) byStatus[c] = [];
        lane = {
          profile_id: it.profile_id,
          profile_label: labelFor(it.profile_id),
          project: proj,
          workdir: it.workdir ?? null,
          total: 0,
          byStatus
        };
        byProfileProj.set(key, lane);
      }
      // The status column may be a non-default one introduced after we
      // initialised the lane; create the bucket on the fly.
      if (!lane.byStatus[it.status]) lane.byStatus[it.status] = [];
      lane.byStatus[it.status].push(it);
      lane.total += 1;
    }

    // Synthesize an empty lane per profile so adding the very first
    // ticket on a fresh server doesn't make the lane appear out of
    // nowhere — the user sees a placeholder + "(no workdir)" lane.
    for (const p of $profiles) {
      const placeholderKey = `${p.id}::${NO_PROJECT}`;
      if (!byProfileProj.has(placeholderKey)) {
        const byStatus: Record<string, FleetItem[]> = {};
        for (const c of $s.columnOrder) byStatus[c] = [];
        byProfileProj.set(placeholderKey, {
          profile_id: p.id,
          profile_label: p.baseUrl ? p.label : 'local',
          project: NO_PROJECT,
          workdir: null,
          total: 0,
          byStatus
        });
      }
    }

    const profileRank = (id: string): number => {
      if (id === $active) return 0;
      const idx = $profiles.findIndex((p) => p.id === id);
      return idx < 0 ? 1000 : idx + 1;
    };
    return Array.from(byProfileProj.values()).sort((a, b) => {
      const pa = profileRank(a.profile_id);
      const pb = profileRank(b.profile_id);
      if (pa !== pb) return pa - pb;
      // Pin the "(no workdir)" bucket below real projects within a profile.
      if (a.project === NO_PROJECT && b.project !== NO_PROJECT) return 1;
      if (b.project === NO_PROJECT && a.project !== NO_PROJECT) return -1;
      return a.project.localeCompare(b.project);
    });
  }
);
