import { writable } from 'svelte/store';
import { api, ApiError, type BoardItem, type GroupedBoard } from '$lib/api';
import { parseGateRejection, requiredFieldLabel, type RequiredField } from '$lib/board-schema';
import { showToast } from './events';

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

/// Outcome of a server-side gate rejection that the snap-back helper
/// surfaces back to the caller — `+page.svelte` (or any future drag
/// integration) uses it to open the edit dialog pre-focused on the
/// first missing field.
export interface MoveRejection {
  /** `{missing, status}` from the server's 400 response. */
  missing: RequiredField[];
  /** Status the user attempted to move into. */
  target: string;
  /** Item id that was rejected. */
  id: number;
}

/// Drag-drop snap-back. Wraps the PATCH so the store reverts the
/// optimistic move and emits a toast when the server rejects the
/// transition. Returns the parsed `{missing, status}` on rejection so
/// the caller can open the edit dialog with the first missing field
/// focused. Returns `null` on success.
///
/// Pattern: `+page.svelte` (or any view) captures the origin column
/// before mutating, calls `moveLocal(id, target)` for the optimistic
/// flip, then awaits `patchStatusWithSnapBack(...)`. On a non-null
/// return value it reopens the edit dialog with the missing fields
/// pre-highlighted.
export async function patchStatusWithSnapBack(
  id: number,
  origin: string,
  target: string
): Promise<MoveRejection | null> {
  try {
    await api.patchBoardItem(id, { status: target });
    return null;
  } catch (err) {
    if (err instanceof ApiError && err.status === 400) {
      const parsed = parseRejectionFromMessage(err.message);
      if (parsed) {
        // Revert the optimistic move before surfacing the rejection so
        // the card returns to its origin column without waiting on the
        // safety-net loadBoard() refresh.
        moveLocal(id, origin);
        const labels = parsed.missing.map(requiredFieldLabel).join(', ');
        showToast({
          kind: 'warn',
          title: `Move to ${parsed.status} needs:`,
          body: labels,
          ttl_ms: 6000
        });
        return { missing: parsed.missing, target: parsed.status, id };
      }
    }
    // Non-gate failure (network, server 5xx, etc.) — still revert the
    // optimistic move so the UI doesn't get stuck in the wrong column.
    moveLocal(id, origin);
    throw err;
  }
}

/// `ApiError` carries the raw response body in its `.message`. For 400
/// gate rejections the body is `{missing, status}` JSON — strip the
/// `HTTP 400: ` prefix and parse defensively.
function parseRejectionFromMessage(message: string): ReturnType<typeof parseGateRejection> {
  const idx = message.indexOf('{');
  if (idx < 0) return null;
  try {
    return parseGateRejection(JSON.parse(message.slice(idx)));
  } catch {
    return null;
  }
}
