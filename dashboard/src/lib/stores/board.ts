import { writable } from 'svelte/store';
import { api, type BoardItem, type GroupedBoard } from '$lib/api';

interface State {
  loading: boolean;
  error: string | null;
  data: GroupedBoard | null;
}

const initial: State = { loading: false, error: null, data: null };
export const board = writable<State>(initial);

export async function loadBoard() {
  board.update((s) => ({ ...s, loading: true, error: null }));
  try {
    const data = await api.listBoard();
    board.set({ loading: false, error: null, data });
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    board.update((s) => ({ ...s, loading: false, error: msg }));
  }
}

/** Optimistic local update so drag-drop feels instant; the next loadBoard() reconciles. */
export function moveLocal(itemId: number, toStatus: string) {
  board.update((s) => {
    if (!s.data) return s;
    const next: GroupedBoard = {
      column_order: [...s.data.column_order],
      columns: {}
    };
    let moving: BoardItem | undefined;
    for (const [col, items] of Object.entries(s.data.columns)) {
      next.columns[col] = items.filter((it) => {
        if (it.id === itemId) {
          moving = it;
          return false;
        }
        return true;
      });
    }
    if (moving) {
      const updated: BoardItem = { ...moving, status: toStatus };
      if (!next.columns[toStatus]) {
        next.columns[toStatus] = [];
        if (!next.column_order.includes(toStatus)) next.column_order.push(toStatus);
      }
      next.columns[toStatus].push(updated);
    }
    return { ...s, data: next };
  });
}
