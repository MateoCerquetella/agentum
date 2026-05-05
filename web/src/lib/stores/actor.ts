/**
 * Per-browser actor identity used for board claims.
 *
 * Generated lazily on first access, persisted in localStorage. Two
 * different browsers (or two tabs after clearing storage) get different
 * IDs so the atomic-claim CAS test can run with both clients open.
 */

const KEY = 'agentum_actor';
const PREFIX = 'web-';

function makeId(): string {
  // 12 hex chars from crypto.getRandomValues — no UUID dep required.
  const buf = new Uint8Array(6);
  if (typeof crypto !== 'undefined' && crypto.getRandomValues) {
    crypto.getRandomValues(buf);
  } else {
    for (let i = 0; i < buf.length; i++) buf[i] = Math.floor(Math.random() * 256);
  }
  return PREFIX + Array.from(buf, (b) => b.toString(16).padStart(2, '0')).join('');
}

export function actorId(): string {
  if (typeof localStorage === 'undefined') return makeId();
  let id = localStorage.getItem(KEY);
  if (!id) {
    id = makeId();
    localStorage.setItem(KEY, id);
  }
  return id;
}
