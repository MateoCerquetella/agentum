/**
 * In-page audible cue for high-priority toasts (`agent.finished`,
 * `agent.awaiting_input`).
 *
 * Why a separate chime when the OS Notification API exists: the OS
 * banner needs a granted browser-permission AND a hidden/unfocused tab
 * to fire (per `stores/notify.ts`). The vast majority of users have
 * neither, so without an in-page cue a finished agent reads as silent
 * — exactly the "I didn't notice my agent was done" complaint the
 * sidebar dot tries to solve visually. This complements the dot with
 * a 250 ms beep that works regardless of permission state.
 *
 * Implementation: Web Audio API, ~250 ms of A5 → mute envelope, no
 * external asset (zero network, zero bundle). Browser autoplay rules
 * require a prior user gesture to unlock audio context — if the very
 * first event arrives before any click/keypress, the chime is a
 * silent no-op for that one event (subsequent events ring fine). We
 * lazily create + persist the context so we don't keep instantiating
 * one per event.
 */

let ctx: AudioContext | null = null;

function ensureCtx(): AudioContext | null {
  if (typeof window === 'undefined') return null;
  if (ctx) return ctx;
  // Older Safari shipped this on a `webkit` prefix; the cast keeps
  // tsc happy without dragging in lib.dom polyfills.
  const Ctor =
    (typeof AudioContext !== 'undefined' && AudioContext) ||
    ((window as unknown as { webkitAudioContext?: typeof AudioContext })
      .webkitAudioContext as typeof AudioContext | undefined);
  if (!Ctor) return null;
  try {
    ctx = new Ctor();
    return ctx;
  } catch {
    return null;
  }
}

export type ChimeKind = 'finished' | 'attention';

export function playChime(kind: ChimeKind = 'finished'): void {
  const ac = ensureCtx();
  if (!ac) return;
  // Some browsers leave the context suspended until a user gesture —
  // resume() returns a promise we deliberately ignore (best-effort).
  if (ac.state === 'suspended') void ac.resume().catch(() => {});
  try {
    const osc = ac.createOscillator();
    const gain = ac.createGain();
    // Finished: A5 (880 Hz) — pleasant, doesn't compete with system
    // chimes. Attention: a higher G6 (1568 Hz) to read as "more
    // urgent" without being a phone-ringing klaxon.
    osc.frequency.value = kind === 'attention' ? 1568 : 880;
    osc.type = 'sine';
    osc.connect(gain);
    gain.connect(ac.destination);
    const t0 = ac.currentTime;
    // Short attack so we don't get a click; quick decay so the chime
    // doesn't overstay its welcome on rapid-fire events.
    gain.gain.setValueAtTime(0, t0);
    gain.gain.linearRampToValueAtTime(0.18, t0 + 0.012);
    gain.gain.exponentialRampToValueAtTime(0.0001, t0 + 0.28);
    osc.start(t0);
    osc.stop(t0 + 0.3);
  } catch {
    // Audio is a nice-to-have, never load-bearing.
  }
}
