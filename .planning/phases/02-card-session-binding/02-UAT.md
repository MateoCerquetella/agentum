---
phase: 02-card-session-binding
doc: UAT
linked_plan: 02-06-PLAN.md
roadmap_success_criteria: [1, 2, 3, 4, 5]
ui_spec_section: "Quality Bar (executor checks)"
---

# Phase 2: Card Session Binding — UAT Checklist

This document is the human-verify checklist for Phase 2 of the Agentum kanban orchestrator milestone. It covers what the automated integration test in 02-06-PLAN.md Task 1 cannot exercise: live-tmux session spawning observed in the browser, real-browser dashboard walkthrough, and real-TUI keybinding verification. The five ROADMAP success criteria and all 21 UI-SPEC Quality Bar checkboxes must be confirmed here before Phase 2 is considered complete.

---

## Before you start

**Rebuild and restart the daemon** (CLAUDE.md rebuild rhythm — skip this and you will be testing a stale binary):

```sh
npm run build --prefix dashboard
cargo build --release
pkill agentum
./target/release/agentum serve &
```

The daemon binds to `https://127.0.0.1:8822` by default (self-signed TLS; trust the fingerprint on first visit).

**Confirm prerequisites with agentum doctor:**

```sh
agentum doctor
```

Verify that tmux and at least one agent CLI (e.g. `claude`) are listed as available. If tmux is absent, the auto-spawn UAT steps (SC #1, SC #3, SC #4, SC #5) cannot be completed on this machine — mark them deferred and proceed per the operator-may-defer clause in the sign-off section.

---

## ROADMAP Success Criteria

The five criteria below are quoted verbatim from `.planning/ROADMAP.md` §Phase 2.

> 1. User drags a `todo` card to `doing` (or clicks "Start") and a tool session spawns automatically with `card.session_id` and `session.card_id` both set in a single atomic write
> 2. The card detail view shows a live tail snippet of the bound session's pane, a status pill, and an "open session" deep link; the session view reciprocally shows the bound card + parent goal with a "back to board" link
> 3. Watchdog `AwaitingInput` / `AgentFinished` / `Crashed` events appear as `[system]` comments on the bound card in real time
> 4. When a session crashes or is killed, the card stays in `doing` with a `[system]` crash comment (no auto-revert) — the user decides retry vs. move
> 5. User can manually re-bind or unbind a card-session pair from both dashboard and TUI; the binding survives daemon restart and profile switch

---

### SC #1 — Auto-spawn dual-write

**What to verify:** dragging a `todo` card to `doing` creates a tool session with both link columns set atomically.

1. Open `https://127.0.0.1:8822/board` in your browser.
2. Click "New card". Fill in: title `manual UAT auto-spawn`, workdir `/tmp`, tool `claude` (or whichever tool `agentum doctor` reports as installed), status `todo`. Save.
3. Drag the card to the `doing` column.
4. Within 1 second: confirm a session row appears when you run:
   ```sh
   agentum ls
   ```
   The session name should match the card title.
5. Click the card again. Confirm the Bound-session panel is now visible in the card detail dialog.
6. Confirm both link columns were written atomically:
   ```sh
   sqlite3 "$XDG_DATA_HOME/agentum/db.sqlite" \
     "SELECT session_id FROM board_items WHERE title='manual UAT auto-spawn'"
   ```
   Assert the value is NOT NULL.
   ```sh
   sqlite3 "$XDG_DATA_HOME/agentum/db.sqlite" \
     "SELECT card_id FROM sessions WHERE id='<sid from above>'"
   ```
   Assert the value equals the card's numeric id.

- [ ] SC #1 verified

---

### SC #2 — Card detail + session view linking

**What to verify:** the card detail shows the Bound-session panel; the session view shows the back-link chip; navigation works in both directions.

1. On the card from SC #1: confirm the Bound-session panel shows:
   - A status pill (uses `StatusPill.svelte` — check DevTools for class `status-pill`, NOT an inline `<span>`).
   - A pane-tail `<pre>` that updates every 2 seconds while the session is running.
   - An "Open session ->" deep link.
2. Click "Open session ->". Assert the browser navigates to `/sessions/<sid>`.
3. On the session view: assert the topbar shows a back-link chip. If the card has no parent goal, format is `← Card #<N>`. If it has a parent goal, format is `← Card #<N> (in "<goal_title>")` with the goal title truncated to 40 chars + `…`.
4. Click the back-link chip. Assert the browser navigates to `/board?focus=<N>`. Assert the target card row pulses visually for approximately 2 s. Assert the `?focus=` query param clears after the first user interaction.

- [ ] SC #2 verified

---

### SC #3 — Watchdog [system] comments in real time

**What to verify:** agent lifecycle events appear as `[system]` comments on the bound card within 1-2 seconds.

1. Keep the card from SC #1 in `doing` with its session running.
2. Open the card detail dialog. Scroll to the comment thread.
3. Trigger an agent-finished event: in the pane, type `/exit` or equivalent, or kill the agent process:
   ```sh
   kill <agent-pid>
   ```
4. Within 1-2 seconds: a new comment appears in the thread with author `system` and body `[system] agent finished`. Confirm the comment row carries class `.cmt-item.system` (check DevTools). The author cell should be muted (uses `var(--fg-3)`).
5. If the agent crashed (non-zero exit): the comment body should be `[system] session crashed: <signature>`. Confirm the row also carries `.crash` modifier class and has a 2px `var(--crash)` left border. Confirm the body text is NOT colorized.

- [ ] SC #3 verified

---

### SC #4 — Crash leaves card in doing, binding intact

**What to verify:** a crashed session does not auto-revert the card; the user manually decides retry vs. move.

1. After triggering a crash in SC #3: reload the board view.
2. Confirm the card is still in the `doing` column (no auto-revert to `todo`).
3. Open the card detail. Confirm `session_id` is still set — the Bound-session panel is still mounted. The pane tail should show `pane not active.` (single Unicode `.`, not `...`).
4. To retry: click "Unbind". Then drag the card back to `todo`. Then drag it back to `doing`. A fresh session spawns automatically.

- [ ] SC #4 verified

---

### SC #5 — Manual rebind/unbind survives restart and profile switch

**What to verify:** the user can unbind and rebind a card-session pair from both dashboard and TUI; the binding persists across daemon restart and profile switch.

1. With a bound card open in the dashboard: click "Unbind". Confirm the Bound-session panel disappears immediately (optimistic clear, no confirmation modal, no spinner — the button label flips to `Unbinding…` momentarily).
2. Rebind to a different session using the API:
   ```sh
   curl -s -k -X PATCH "https://127.0.0.1:8822/api/board/<card-id>" \
     -H "Authorization: Bearer <token>" \
     -H "Content-Type: application/json" \
     -d '{"session_id": "<other-session-uuid>"}'
   ```
   Confirm HTTP 200. Refresh the dashboard. Confirm the Bound-session panel reappears bound to the new session.
3. Test TUI rebind/unbind: open `agentum terminal`. Navigate to the session that now has `card_id` set. Confirm the status bar shows a chip ` c card #<id> `. Press `c`. Confirm a one-cell hint strip appears showing `card #<id> — <title>`. Press `c` or `Esc` to collapse it.
4. Test daemon restart:
   ```sh
   pkill agentum
   ./target/release/agentum serve &
   ```
   Refresh the dashboard. Confirm the card-session binding from step 2 is still in place.
5. Test profile switch: in the dashboard topbar, switch to a different named profile and back (or add a `local2` profile pointing at the same daemon URL). Confirm the card and its binding still render correctly after switching back.

- [ ] SC #5 verified

---

## UI-SPEC Quality Bar (executor checks)

The 21 checkboxes below are quoted verbatim from `.planning/phases/02-card-session-binding/02-UI-SPEC.md` §Quality Bar.

- [ ] Bound-session panel mounts above `.comments` and only when `card.session_id != null` AND the session GET resolves.
- [ ] `StatusPill.svelte` is used **verbatim** for the session status — no inline `<span class="status-pill">` duplicate.
- [ ] Pane-tail `<pre>` carries `aria-live="polite"` and `aria-atomic="false"` (or no `aria-atomic` set — defaults to `false`).
- [ ] Pane-tail poll runs every 2000 ms while dialog is open AND `session.status === "running"`. Stops on dialog close, status change, or 3 consecutive errors. Uses an `AbortController` per fetch.
- [ ] Pane-tail empty states render the exact copy: `waiting for output…` / `pane not active.` / `couldn't fetch pane: {reason}`. The ellipsis is a single Unicode `…`, NOT three ASCII dots.
- [ ] Back-link chip renders only when `session.card_id != null` AND the card GET resolves. Format strictly matches: `← Card #{id} (in "{goal_title}")` or `← Card #{id}` when no parent. Goal title truncated to 40 chars + `…`.
- [ ] Back-link chip navigates to `/board?focus={card.id}` via `goto()`. Hover state turns `--cta` (text + border).
- [ ] `[system]` comments render with `.cmt-item.system` class; author cell muted to `var(--fg-3)`; body inherits standard `.cmt-body` styling.
- [ ] Crash `[system]` comments add the `.crash` modifier class; rendered with a 2px `var(--crash)` left border; body text is NOT colorized.
- [ ] No edit/delete affordance is ever rendered on `.cmt-item.system` rows — even if the parent `.cmt-item` gains one in the future.
- [ ] Unbind button renders only when `card.session_id != null`. Click PATCHes `{ session_id: null }` and clears the panel optimistically.
- [ ] No confirmation modal on Unbind. No spinner mid-action — label flips to `Unbinding…`.
- [ ] TUI: status-bar acquires the ` c card #{id} ` chip when focused on a session with `card_id != null`. Chip painted `palette.muted` on `palette.chrome_bg`.
- [ ] TUI: pressing `c` while focused on such a session toggles a one-cell hint strip with `card #{id} — {title}`. Pressing `c` or `Esc` collapses it. `c` is a no-op for sessions with `card_id == null`.
- [ ] TUI: `s` is NOT touched. The existing "Stop session" binding stays intact.
- [ ] All TUI colors come from `Palette`. Zero hardcoded `Color::*` constants.
- [ ] `/board?focus={card.id}` scrolls the target ticket into view AND clears the query param after the first user interaction. Pulses the row for 2 s.
- [ ] Mobile: bound-session panel stacks vertically on `≤720px`; pane-tail max-height drops to 12 lines; back-link chip is the leftmost topbar element; goal-title parenthetical drops on `≤540px`.
- [ ] `npm run build --prefix dashboard && cargo build --release` rerun after every dashboard-side change, gated in CI.
- [ ] Accessibility: back-link `<a>` carries `aria-label="Back to Card #{id} on the board"`. Pane-tail `<pre>` carries `aria-live="polite"`. Unbind button is keyboard-focusable and announces state changes via the standard `.ghost` button styling.

(21st item below — listed separately because it spans the rebuild rhythm and Accessibility dimensions above)

- [ ] `npm run check --prefix dashboard` exits 0 after all dashboard changes for this phase.

**All 21 quality-bar items verified by:** _______________ (operator name + date)

---

## Operator Sign-Off

| Criterion | Status |
|-----------|--------|
| [ ] SC #1 — auto-spawn dual-write | |
| [ ] SC #2 — card detail + session view linking | |
| [ ] SC #3 — watchdog [system] comments in real time | |
| [ ] SC #4 — crash leaves binding intact, no auto-revert | |
| [ ] SC #5 — manual rebind/unbind survives restart + profile switch | |

**Approved by:** _______________ (operator) on _______________ (date)

**Operator may defer:** If automated test coverage in 02-06-PLAN.md Task 1 is sufficient and the operator opts to defer live-UI verification, write `Deferred — see <pointer-to-future-verify-session>` next to the approval line. Phase 2 will then close on automated coverage alone, with the live walkthrough scheduled for a separate `/gsd-verify-work 2` session (same pattern as Phase 1 plan 01-08 documented in `.planning/phases/01-goal-cards-planner-slice/01-08-SUMMARY.md` §"Manual UAT — Deferred").

---

## References

- `.planning/ROADMAP.md` §Phase 2 — the five success criteria quoted verbatim above
- `.planning/phases/02-card-session-binding/02-UI-SPEC.md` §Quality Bar — the 21 executor checkboxes quoted verbatim above
- `.planning/phases/02-card-session-binding/02-CONTEXT.md` — the locked decisions D-01..D-15 (body templates, dedupe, goal-card filter, atomic transfer, crash behavior)
- `.planning/phases/01-goal-cards-planner-slice/01-08-SUMMARY.md` — the Phase 1 UAT precedent establishing the operator-may-defer pattern
