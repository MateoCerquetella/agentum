<script lang="ts">
  /**
   * GoalComposer — persistent input bar above the kanban board.
   *
   * Placement: between .toolbar and .board on /board (UI-SPEC §Interaction
   * Contract — GoalComposer.svelte). Single textarea + "Plan it" button.
   * Heights: 56px compact, 220px when the board has zero cards (empty-state).
   * The empty-state mode shows the GOAL eyebrow, heading, and body copy
   * above the textarea to invite the first goal.
   *
   * Submit path: Cmd/Ctrl+Enter or button click → submitGoal(text) store
   * action → POST /api/board/goals → the new card lands via /api/events WS.
   * No toast, no optimistic card insertion — the WS event IS the feedback.
   *
   * Error handling mirrors BoardItemDialog.svelte §submit (lines 278-354):
   *   400 {missing, status} → parseGateRejection → "Your todo column needs: …"
   *   5xx / network        → "Couldn't reach the planner. …"
   *
   * Matches §3.2 of 01-UI-SPEC.md (Interaction Contract — GoalComposer).
   */
  import { ApiError } from '$lib/api';
  import { board, submitGoal } from '$stores/board';
  import { parseGateRejection, requiredFieldLabel } from '$lib/board-schema';

  // ---- local state (Svelte 5 runes) -------------------------------------

  let text = $state('');
  let submitting = $state(false);
  let error = $state<string | null>(null);
  // Tracks which required fields the server rejected — mirrors
  // BoardItemDialog.svelte so the pattern is consistent.
  let rejectedFields = $state<Set<string>>(new Set());
  let textareaEl: HTMLTextAreaElement | undefined = $state();

  // Empty-state mode: show the expanded 220px form with eyebrow, heading,
  // and body copy when the board has zero cards across all columns.
  // Per UI-SPEC §Mobile: empty-state is suppressed on ≤720px — CSS handles
  // that; the JS logic here is viewport-agnostic.
  const boardIsEmpty = $derived.by(() => {
    const data = $board.data;
    if (!data) return true;
    return data.column_order.every((col) => (data.columns[col]?.length ?? 0) === 0);
  });

  // ---- keyboard handler -------------------------------------------------

  function handleKey(e: KeyboardEvent) {
    // Cmd/Ctrl+Enter always submits, regardless of line count.
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault();
      void submit();
      return;
    }
    // Plain Enter on a single-line textarea (text contains no newline yet)
    // submits rather than inserting a newline. Once the goal has wrapped
    // beyond one line the user must use Cmd/Ctrl+Enter or the button.
    // This matches §Interaction Contract: "Enter only when single-line and
    // no shift" (01-UI-SPEC.md).
    if (e.key === 'Enter' && !e.shiftKey && !text.includes('\n')) {
      e.preventDefault();
      void submit();
    }
  }

  // ---- auto-grow handler ------------------------------------------------

  function handleInput() {
    if (!textareaEl) return;
    // Reset height to 'auto' so scrollHeight reflects content, then pin.
    textareaEl.style.height = 'auto';
    textareaEl.style.height = Math.min(textareaEl.scrollHeight, 160) + 'px';
  }

  // ---- submit -----------------------------------------------------------

  async function submit() {
    const t = text.trim();
    if (!t || submitting) return;
    submitting = true;
    error = null;
    rejectedFields = new Set();
    try {
      await submitGoal(t);
      text = '';
      // Reset textarea height after clear.
      if (textareaEl) textareaEl.style.height = 'auto';
      textareaEl?.focus();
    } catch (err) {
      if (err instanceof ApiError) {
        if (err.status === 400) {
          // Try to parse the {missing, status} gate-rejection envelope
          // (same shape as BoardItemDialog.svelte §parseRejectionFromMessage).
          const parsed = parseRejectionFromRawMessage(err.message);
          if (parsed) {
            rejectedFields = new Set(parsed.missing);
            const labels = parsed.missing.map(requiredFieldLabel).join(', ');
            // Verbatim copy from 01-UI-SPEC.md §Copywriting Contract.
            error = `Your todo column needs: ${labels}. Add them in Settings → Column rules.`;
          } else if (
            err.message.includes("isn't installed") ||
            err.message.includes('not available') ||
            err.message.includes('not found')
          ) {
            // Tool-not-installed shape from the executor. The daemon already
            // includes the tool name in its message body — surface as-is.
            error = err.message;
          } else {
            error = err.message;
          }
        } else {
          // 5xx and network failures (status 0 from a network-level throw
          // that wraps into ApiError in the request helper).
          // Verbatim copy from 01-UI-SPEC.md §Copywriting Contract.
          error = "Couldn't reach the planner. Check the daemon and try again.";
        }
      } else {
        // Non-ApiError (e.g. SyntaxError from a malformed response).
        error = "Couldn't reach the planner. Check the daemon and try again.";
      }
    } finally {
      submitting = false;
    }
  }

  // Mirrors the same helper in BoardItemDialog.svelte (lines 356-370).
  // ApiError.message is `HTTP 4xx: <raw body>` — find the first `{` and
  // parse from there. Defensive: returns null when the body isn't the
  // expected {missing, status} shape.
  function parseRejectionFromRawMessage(message: string) {
    const idx = message.indexOf('{');
    if (idx < 0) return null;
    try {
      return parseGateRejection(JSON.parse(message.slice(idx)));
    } catch {
      return null;
    }
  }
</script>

<!-- outer wrapper applies height via CSS; empty-state class gates the 220px form -->
<div class="composer" class:empty-state={boardIsEmpty}>
  {#if boardIsEmpty}
    <!-- Empty-state copy — shown only on desktop (CSS suppresses on ≤720px). -->
    <div class="eyebrow">GOAL</div>
    <!-- Heading uses .display-2-like tracking via inline style — inherits
         the existing 600-weight from the design system rather than
         introducing a new weight (UI-SPEC §Typography third-weight allowance). -->
    <h2 class="es-heading">Drop a goal in.</h2>
    <p class="es-body">The planner spawns a tmux session and writes 3–7 cards within ~2 min. Cards land in todo, ready to claim.</p>
  {/if}

  <div class="composer-row">
    <textarea
      aria-label="Goal description"
      placeholder="Drop a goal in. The planner will turn it into 3–7 cards."
      bind:value={text}
      bind:this={textareaEl}
      onkeydown={handleKey}
      oninput={handleInput}
      rows={1}
      disabled={submitting}
      class="composer-ta"
    ></textarea>
    <button
      type="button"
      class="tb-btn primary composer-btn"
      onclick={() => void submit()}
      disabled={text.trim().length === 0 || submitting}
    >{submitting ? 'Planning…' : 'Plan it'}</button>
  </div>
</div>

{#if error}
  <!-- Error block below the composer. 4px --crash left border + role="alert"
       per UI-SPEC §Interaction Contract and §Quality Bar. -->
  <div class="composer-error" role="alert" aria-live="polite">
    <span class="composer-error-msg">{error}</span>
    <button
      type="button"
      class="composer-error-dismiss"
      aria-label="Dismiss error"
      onclick={() => (error = null)}
    >×</button>
  </div>
{/if}

<style>
  /* ---- outer wrapper -------------------------------------------------- */
  .composer {
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    gap: 8px;
    padding: 10px 16px;
    background: var(--bg-chrome);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
    /* Compact height matches the .toolbar strip (56px) so the composer
       feels like an extension of the top chrome, not a separate panel. */
    min-height: 56px;
  }

  /* Empty-state: expand to 220px when the board has no cards. The extra
     space holds the eyebrow + heading + body that invite the first goal.
     Suppressed on ≤720px (phone) where vertical real estate is precious. */
  @media (min-width: 721px) {
    .composer.empty-state {
      min-height: 220px;
      justify-content: center;
    }
  }

  /* ---- empty-state copy ----------------------------------------------- */
  .eyebrow {
    font-family: var(--mono);
    font-size: 11px;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--fg-3);
  }

  .es-heading {
    margin: 0;
    font-family: var(--display);
    font-size: 18px;
    font-weight: 600;
    line-height: 1.2;
    letter-spacing: -0.02em;
    color: var(--fg);
  }

  .es-body {
    margin: 0;
    font-family: var(--display);
    font-size: 14px;
    font-weight: 400;
    line-height: 1.5;
    color: var(--fg-3);
  }

  /* Hide empty-state copy on phone — the compact bar stays visible
     but only the textarea + button render. */
  @media (max-width: 720px) {
    .eyebrow,
    .es-heading,
    .es-body {
      display: none;
    }
  }

  /* ---- composer row --------------------------------------------------- */
  .composer-row {
    display: flex;
    align-items: flex-end;
    gap: 8px;
  }

  /* Mobile: stack textarea above button (UI-SPEC §Mobile / PWA Behavior). */
  @media (max-width: 720px) {
    .composer-row {
      flex-direction: column;
      align-items: stretch;
    }
  }

  /* ---- textarea ------------------------------------------------------- */
  .composer-ta {
    flex: 1;
    min-width: 0;
    /* Single-line by default; auto-grow via JS up to 160px. */
    height: auto;
    max-height: 160px;
    padding: 6px 10px;
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    color: var(--fg);
    font-family: var(--display);
    font-size: 14px;
    line-height: 1.5;
    resize: none;
    overflow-y: auto;
    transition: border-color var(--t-hover);
  }

  /* iOS: font-size ≥16px suppresses the focus-zoom on older Safari.
     app.css already enforces this globally for inputs; stated here
     explicitly so the rule is co-located with the component. */
  @media (max-width: 720px) {
    .composer-ta {
      font-size: 16px;
    }
  }

  .composer-ta::placeholder {
    color: var(--fg-3);
  }

  .composer-ta:focus-visible {
    outline: 2px solid var(--link);
    outline-offset: 2px;
    border-color: var(--link);
  }

  .composer-ta:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  /* ---- submit button -------------------------------------------------- */
  /* Inherits .tb-btn.primary from _design.css (coral bg, blue hover).
     Explicit height so it aligns with a single-line textarea. */
  .composer-btn {
    height: 34px;
    white-space: nowrap;
    flex-shrink: 0;
  }

  .composer-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
    pointer-events: none;
  }

  @media (max-width: 720px) {
    .composer-btn {
      /* Full-width on mobile; min-height 38px matches .tb-btn.primary
         mobile rule in _design.css (UI-SPEC §Spacing Scale exception). */
      width: 100%;
      min-height: 38px;
      height: auto;
    }
  }

  /* ---- error block ---------------------------------------------------- */
  /* Rendered below the composer (between composer and board), full-width.
     4px --crash left border + muted bg per UI-SPEC §Interaction Contract. */
  .composer-error {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 8px 12px;
    background: var(--bg-2);
    border-left: 4px solid var(--crash);
    color: var(--fg-2);
    font-family: var(--display);
    font-size: 13px;
    line-height: 1.5;
  }

  .composer-error-msg {
    flex: 1;
    min-width: 0;
  }

  .composer-error-dismiss {
    background: transparent;
    border: 0;
    color: var(--fg-3);
    cursor: pointer;
    font-size: 16px;
    line-height: 1;
    padding: 0;
    flex-shrink: 0;
    transition: color var(--t-hover);
  }

  .composer-error-dismiss:hover {
    color: var(--fg);
  }
</style>
