<script lang="ts">
  import { api, ApiError, type AgentInfo, type BoardComment, type BoardItem, type BoardPatch, type NewBoardItem, type Session, type TicketLbl, type Tool } from '$lib/api';
  import { actorId } from '$stores/actor';
  import { sessions, loadSessions } from '$stores/sessions';
  import { fleet } from '$stores/fleet';
  import { profiles, activeProfileId, type Profile } from '$lib/profiles';
  import {
    parseGateRejection,
    requiredFieldLabel,
    requiredFieldsFor,
    validateTransition,
    type RequiredField
  } from '$lib/board-schema';
  import DirPicker from './DirPicker.svelte';
  import StatusPill from './StatusPill.svelte';

  /**
   * Create + edit dialog for board items. Visual language follows
   * NewSessionDialog so the chrome is consistent across the dashboard.
   * Each ticket carries enough execution context (workdir, agent,
   * optional model) that turning it into a session is a one-button
   * action — no re-asking for the same answers.
   *
   *   mode === 'create' — POSTs a fresh ticket against the selected
   *     paired daemon. `defaultStatus` pre-fills the column the user
   *     clicked `+` on; `defaultProfileId` seeds the Servers picker.
   *     `onCreated(profileId, item)` fires with the server-assigned
   *     row so the parent can avoid waiting on the WS refetch.
   *   mode === 'edit'   — PATCHes the existing `item` on whichever
   *     profile originally owned it. Profile is fixed (visible but not
   *     reassignable — moving a ticket between servers is a copy/
   *     migrate flow, not an inline edit). Exposes claim / unclaim /
   *     delete via `onUpdated` / `onDeleted`.
   */
  type Props = {
    open: boolean;
    mode: 'create' | 'edit';
    item?: BoardItem | null;
    defaultStatus?: string | null;
    /** When set, pre-selects the Servers tile. Edit mode supplies the
     *  ticket's home profile so the PATCH routes back to the same one. */
    defaultProfileId?: string | null;
    /** Seed for the Workdir field on create. Per-lane "+ Ticket" passes
     *  the lane's workdir so the new ticket inherits the project. */
    defaultWorkdir?: string | null;
    /** Server-rejected fields to highlight on open — used when the page
     *  reopens the dialog after a drag-drop snap-back so the user sees
     *  the red borders without having to re-submit first. */
    initialMissing?: RequiredField[];
    columns?: string[];
    onClose: () => void;
    onCreated?: (profileId: string, it: BoardItem) => void;
    onUpdated?: (profileId: string, it: BoardItem) => void;
    onDeleted?: (profileId: string, id: number) => void;
  };
  let {
    open,
    mode,
    item = null,
    defaultStatus = null,
    defaultProfileId = null,
    defaultWorkdir = null,
    initialMissing = [],
    columns = [],
    onClose,
    onCreated,
    onUpdated,
    onDeleted
  }: Props = $props();

  const LBLS: { id: TicketLbl; label: string }[] = [
    { id: 'feat',  label: 'feat'  },
    { id: 'bug',   label: 'bug'   },
    { id: 'chore', label: 'chore' },
    { id: 'spike', label: 'spike' }
  ];

  /// Same palette as NewSessionDialog so the agent picker on a ticket
  /// reads identically to the agent picker on a fresh session. Keep
  /// the two in sync when adding a new adapter.
  type ToolTile = {
    id: Tool;
    label: string;
    desc: string;
    dot: string;
    /// First-class tools appear in /api/agents and get gated on whether
    /// the binary resolves on the daemon's PATH. Non-first-class entries
    /// are always shown.
    firstClass: boolean;
    /// Suggested model string — surfaces as the Model field placeholder
    /// when this tool is picked.
    modelHint: string;
  };
  const TOOLS: ToolTile[] = [
    { id: 'claude', label: 'Claude', desc: 'Anthropic',    dot: 'var(--tool-claude)', firstClass: true,  modelHint: 'claude-opus-4-8' },
    { id: 'codex',  label: 'Codex',  desc: 'OpenAI',       dot: 'var(--tool-codex)',  firstClass: true,  modelHint: 'gpt-5'           },
    { id: 'cursor', label: 'Cursor', desc: 'cursor-agent', dot: 'var(--tool-cursor, var(--cta))', firstClass: true, modelHint: 'default' },
    { id: 'gemini', label: 'Gemini', desc: 'Google',       dot: 'var(--tool-gemini)', firstClass: true,  modelHint: 'default' },
    { id: 'hermes', label: 'Hermes', desc: 'hermes-cli',   dot: 'var(--tool-hermes)', firstClass: false, modelHint: 'default' }
  ];

  let title   = $state('');
  let body    = $state('');
  let status  = $state('todo');
  let lbl     = $state<TicketLbl | ''>('');
  let tool    = $state<Tool | ''>('');
  let workdir = $state('');
  let model   = $state('');
  /// Optional pre-bind: when set, the create payload sends `session_id`
  /// and the server's dual-write attaches the new card to this running
  /// tmux session instead of auto-spawning one. Picker only renders in
  /// create mode against the same target profile.
  let pickedSessionId = $state<string>('');
  /// When true, after a successful create we immediately PATCH the card
  /// to status="doing" so the server's auto-spawn (or attach-to-picked
  /// branch) fires without the user having to drag the card. Defaults
  /// to true because the user's mental model is "I'm starting this work
  /// now, not making a draft" — they can untick for true drafts.
  let startNow = $state(true);
  /// Target daemon for the ticket. In create mode the user can pick;
  /// in edit mode it's locked to the row's home profile.
  let targetProfileId = $state<string>('');

  let submitting = $state(false);
  let deleting   = $state(false);
  let confirmDelete = $state(false);
  let error = $state<string | null>(null);

  /// Server-rejected fields highlighted from the last 400 response.
  /// Seeded either by the page (drag-drop snap-back via `initialMissing`)
  /// or by this dialog's own submit handler. Cleared on the next user-
  /// driven status change so the red borders don't linger after the
  /// user fixes the problem.
  let rejectedFields = $state<Set<RequiredField>>(new Set());
  /// Tracks the last status the dialog rendered so the clear-on-status-
  /// change effect can distinguish a user-driven column change from the
  /// initial seed (where status flips from '' to its real value).
  let priorStatus = $state<string>('');

  /// Agent installation gating against the *target* daemon. Keyed by
  /// profile id so flipping the Servers tile re-uses already-fetched
  /// data instead of re-probing every time.
  let availabilityByProfile = $state<Record<string, Record<string, AgentInfo>>>({});

  function availabilityFor(profileId: string): Record<string, AgentInfo> | null {
    return availabilityByProfile[profileId] ?? null;
  }

  async function refreshAvailability(profileId: string) {
    if (availabilityByProfile[profileId]) return; // cached
    try {
      const list = await api.listAgentsOn(profileId);
      const map: Record<string, AgentInfo> = {};
      for (const a of list) map[a.name] = a;
      availabilityByProfile = { ...availabilityByProfile, [profileId]: map };
    } catch {
      availabilityByProfile = { ...availabilityByProfile, [profileId]: {} };
    }
  }

  /// Servers tiles — every configured profile gets a tile so the user
  /// can pin the ticket to whichever daemon owns the work. Mirrors the
  /// `Servers` block in NewSessionDialog. For loopback (empty baseUrl)
  /// we show the *real* hostname from `/api/health` via the fleet
  /// store — same as Sidebar — so the user sees "omarchy" / "mateo-mac"
  /// instead of a generic placeholder.
  function serverLabel(p: Profile): string {
    if (p.baseUrl) return p.label;
    const host = $fleet[p.id]?.hostname?.trim();
    return host || 'this machine';
  }
  function serverHost(p: Profile): string {
    if (p.baseUrl) {
      try { return new URL(p.baseUrl).host; } catch { return p.baseUrl; }
    }
    // Loopback: the label already carries the real hostname (from the
    // fleet store), so the host hint stays empty — matches the pattern
    // in `profileHostHint`/EndpointSwitcher.
    return '';
  }

  function toolAvailable(t: ToolTile): boolean {
    if (!t.firstClass) return true;
    const map = availabilityFor(targetProfileId);
    if (!map) return true;            // probe pending — fail open
    const info = map[t.id];
    if (!info) return true;           // unknown id — fail open
    return info.available;
  }

  function toolUnavailableReason(t: ToolTile): string | null {
    if (!t.firstClass) return null;
    const map = availabilityFor(targetProfileId);
    if (!map) return null;
    const info = map[t.id];
    if (!info || info.available) return null;
    return `${info.binary} not found on the daemon's PATH`;
  }

  const currentToolHint = $derived(
    (TOOLS.find((t) => t.id === tool)?.modelHint) ?? 'default'
  );

  /// Preference order for the auto-assign fallback. First entry that
  /// shows up as `available` on the target daemon wins.
  const AUTO_PREF: Tool[] = ['claude', 'codex', 'cursor', 'gemini', 'opencode', 'aider'];

  /// What we'd auto-pick *right now* given the current target profile's
  /// availability map. Returns null when the probe hasn't landed yet
  /// (so the UI can show "auto-assigning…" instead of guessing).
  function autoAssignCandidate(profileId: string): Tool | null {
    const map = availabilityFor(profileId);
    if (!map) return null;
    for (const id of AUTO_PREF) {
      const info = map[id];
      if (info?.available) return id;
    }
    // Fall back to any available entry in case the user has only an
    // exotic agent installed.
    const any = Object.values(map).find((a) => a.available);
    return (any?.name as Tool) ?? null;
  }

  /// Hint string for the "Agent" eyebrow row when nothing is picked.
  const autoHint = $derived.by(() => {
    if (tool !== '') return '';
    const pick = autoAssignCandidate(targetProfileId);
    if (!pick) return 'agent will be auto-assigned at submit';
    return `auto-assigns to ${pick} (first installed)`;
  });

  /// Seed local state whenever the dialog re-opens or the bound item
  /// changes. Without this guard, typing into a field would get clobbered
  /// every time the parent re-renders.
  let lastSeeded = $state<string>('');
  $effect(() => {
    if (!open) {
      lastSeeded = '';
      return;
    }
    const key = mode === 'edit' && item
      ? `edit:${item.id}:${item.updated_at}`
      : `create:${defaultStatus ?? ''}:${defaultProfileId ?? ''}:${defaultWorkdir ?? ''}`;
    if (key === lastSeeded) return;
    lastSeeded = key;
    error = null;
    confirmDelete = false;
    if (mode === 'edit' && item) {
      title   = item.title;
      body    = item.body ?? '';
      status  = item.status;
      lbl     = (item.lbl ?? '') as TicketLbl | '';
      tool    = (item.tool ?? '') as Tool | '';
      workdir = item.workdir ?? '';
      model   = item.model ?? '';
      targetProfileId = defaultProfileId || $activeProfileId;
    } else {
      title   = '';
      body    = '';
      status  = defaultStatus || 'todo';
      lbl     = '';
      tool    = '';
      workdir = defaultWorkdir ?? '';
      model   = '';
      targetProfileId = defaultProfileId || $activeProfileId;
      // Always reset the picker on a fresh open so a previously-picked
      // session doesn't leak across dialog instances.
      pickedSessionId = '';
      // Reset start-now per open; sticky-default-true matches user intent.
      startNow = true;
    }
    // Seed server-rejection highlights from the parent (drag-drop
    // snap-back path). Setting `priorStatus = status` before the
    // clear-on-status-change effect runs prevents it from wiping out
    // the seed on the same tick.
    priorStatus = status;
    rejectedFields = new Set(initialMissing);
    // Probe agent availability for the target profile. Subsequent
    // server tile clicks re-trigger via pickServer().
    void refreshAvailability(targetProfileId);
    // Refresh the sessions store so the existing-session picker shows
    // an up-to-date list of attachable sessions for the target profile.
    // Best-effort — failure leaves the picker empty, no UX break.
    if (mode === 'create') {
      void loadSessions();
    }
  });

  /// Sessions on the target profile that are *available* to bind: must
  /// be running, must be on the same profile we're creating against,
  /// and must not already be bound to a card. Filters out planner
  /// sessions (lbl="goal"-spawned) by name convention — they're
  /// owned by their goal card.
  const attachableSessions = $derived.by<Session[]>(() => {
    const list = $sessions.items.filter((s) => {
      const profileId = (s as Session & { profile?: string }).profile ?? '';
      return (
        profileId === targetProfileId &&
        s.status === 'running' &&
        s.card_id == null &&
        !s.name.startsWith('planner-')
      );
    });
    // Stable order — newest activity first so freshly-spawned panes
    // surface at the top.
    return list.slice().sort((a, b) => {
      const av = a.last_activity_at ?? a.updated_at;
      const bv = b.last_activity_at ?? b.updated_at;
      return (bv ?? '').localeCompare(av ?? '');
    });
  });

  /// Toggle a session pick. When picked, prefill workdir + tool from the
  /// session so the visible context matches what will land server-side
  /// (the server preserves the bind even if the user changes these — but
  /// the prefill makes the panel honest about the current state).
  function toggleSession(s: Session) {
    if (pickedSessionId === s.id) {
      pickedSessionId = '';
      return;
    }
    pickedSessionId = s.id;
    // Only prefill empty fields so we don't clobber what the user typed.
    if (!workdir.trim()) workdir = s.workdir;
    const sTool = s.tool as Tool | '';
    if (tool === '' && sTool) tool = sTool;
  }

  /// Switching the Servers tile re-probes /api/agents on the chosen
  /// daemon. Cached results in `availabilityByProfile` make repeats
  /// instant; first-time probes are a single fetch.
  async function pickServer(id: string) {
    if (mode === 'edit') return; // profile is locked in edit mode
    if (id === targetProfileId) return;
    targetProfileId = id;
    await refreshAvailability(id);
  }

  function close() {
    if (submitting || deleting) return;
    onClose();
  }

  function onBackdrop(e: MouseEvent) {
    if (e.target === e.currentTarget) close();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') close();
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    const t = title.trim();
    if (!t) {
      error = 'title is required';
      return;
    }
    // If the client-side gate sees something missing, surface it as a
    // visible banner instead of silently doing nothing. The button is
    // no longer disabled on gate failure, so this is the user's
    // feedback path. The server gate is still authoritative — we
    // never block a submit that the client thinks is fine.
    if (!clientGatePasses) {
      const labels = missingFields.map(requiredFieldLabel).join(', ');
      error = `move to ${status} needs: ${labels}`;
      rejectedFields = new Set(missingFields);
      return;
    }
    submitting = true;
    error = null;
    try {
      const cleanWorkdir = workdir.trim().replace(/\/+$/, '') || '';
      const cleanModel   = model.trim();
      if (mode === 'create') {
        // Auto-assign if the user left the agent unpicked. The probe
        // is normally cached from dialog open; if it raced and isn't
        // ready, fetch it synchronously here so submit never lands an
        // unassigned ticket.
        let effectiveTool: Tool | '' = tool;
        if (effectiveTool === '') {
          if (!availabilityFor(targetProfileId)) {
            await refreshAvailability(targetProfileId);
          }
          const pick = autoAssignCandidate(targetProfileId);
          if (pick) effectiveTool = pick;
        }
        const payload: NewBoardItem = {
          title: t,
          body: body.trim() ? body : null,
          status: status || null,
          lbl: lbl || null,
          tool: (effectiveTool || null) as string | null,
          workdir: cleanWorkdir || null,
          model:   cleanModel || null,
          // When the user picked an existing session, send it so the
          // server's create_board_item dual-write attaches the card to
          // that running pane instead of auto-spawning a new one.
          session_id: pickedSessionId || null
        };
        const created = await api.createBoardItemOn(targetProfileId, payload);

        // Start-now: PATCH the freshly-created card to status=doing so
        // the server's auto-spawn (or attach-to-picked-session) branch
        // fires immediately. Without this, the user had to drag the
        // card or open it and change the column manually — the v0.8.4
        // user feedback was "I created the ticket and it didn't trigger
        // at start, we should have an option for that".
        //
        // Only PATCH when the create didn't already land in doing, and
        // when the user has agent context (workdir+tool) so the server
        // gate would pass. If client gate would fail for doing, surface
        // the error inline instead of issuing a doomed PATCH.
        let final = created;
        if (startNow && created.status !== 'doing') {
          // The start path runs as: claim → PATCH status=doing. The doing
          // gate requires claimed_by — but we're about to set it ourselves
          // in the very next call, so the pre-check uses the actor id we
          // intend to claim with, not `null` (the row's current value).
          // The previous version always tripped "missing claimed_by" and
          // the user never got to the start path.
          const willClaimAs = actorId();
          const doingMissing = validateTransition('doing', {
            title: t,
            lbl: created.lbl ?? null,
            workdir: created.workdir ?? '',
            tool: created.tool ?? null,
            claimed_by: willClaimAs,
            session_id: created.session_id ?? null
          });
          if (doingMissing.length > 0) {
            // Ticket exists; we just can't start it. Tell the user which
            // field(s) to fill in by re-opening the ticket.
            onCreated?.(targetProfileId, created);
            const labels = doingMissing.map(requiredFieldLabel).join(', ');
            error = `created — to start, set: ${labels}`;
            submitting = false;
            return;
          }
          try {
            await api.claimBoardItemOn(targetProfileId, created.id, willClaimAs);
            final = await api.patchBoardItemOn(targetProfileId, created.id, { status: 'doing' });
          } catch (startErr) {
            // Ticket exists — surface the start failure but don't lose the create.
            onCreated?.(targetProfileId, created);
            const msg = startErr instanceof Error ? startErr.message : String(startErr);
            error = `created — start failed: ${msg}`;
            submitting = false;
            return;
          }
        }

        onCreated?.(targetProfileId, final);
        onClose();
      } else if (item) {
        const patch: BoardPatch = {};
        if (t !== item.title) patch.title = t;
        const nextBody = body.trim() ? body : null;
        if (nextBody !== (item.body ?? null)) patch.body = nextBody;
        if (status !== item.status) patch.status = status;
        const nextLbl = lbl || null;
        if (nextLbl !== (item.lbl ?? null)) patch.lbl = nextLbl;
        const nextTool = tool || null;
        if (nextTool !== (item.tool ?? null)) patch.tool = nextTool;
        const nextWorkdir = cleanWorkdir || null;
        if (nextWorkdir !== (item.workdir ?? null)) patch.workdir = nextWorkdir;
        const nextModel = cleanModel || null;
        if (nextModel !== (item.model ?? null)) patch.model = nextModel;
        // Server returns the row even when the patch is empty, so no
        // need to short-circuit here — keeps the UX simple.
        const updated = await api.patchBoardItemOn(targetProfileId, item.id, patch);
        onUpdated?.(targetProfileId, updated);
        onClose();
      }
    } catch (err) {
      // Server gate rejection? The payload carries {missing, status}.
      // Map it onto rejectedFields so the inputs render red borders,
      // and rewrite the error message to the friendly hint.
      if (err instanceof ApiError && err.status === 400) {
        const parsed = parseRejectionFromMessage(err.message);
        if (parsed) {
          rejectedFields = new Set(parsed.missing);
          const labels = parsed.missing.map(requiredFieldLabel).join(', ');
          error = `move to ${parsed.status} needs: ${labels}`;
        } else {
          error = err.message;
        }
      } else {
        error = err instanceof Error ? err.message : String(err);
      }
    } finally {
      submitting = false;
    }
  }

  /// `api.ts::request` throws `ApiError(status, message)` where the
  /// message is the raw response body. For 400 gate rejections that
  /// body is JSON `{missing, status}` — parse defensively and fall
  /// back to the generic error path on shape mismatch.
  function parseRejectionFromMessage(message: string): ReturnType<typeof parseGateRejection> {
    // The `ApiError` constructor prefixes with `HTTP 400: ` — strip
    // before parsing.
    const idx = message.indexOf('{');
    if (idx < 0) return null;
    try {
      return parseGateRejection(JSON.parse(message.slice(idx)));
    } catch {
      return null;
    }
  }

  async function claim() {
    if (!item || submitting) return;
    submitting = true;
    error = null;
    try {
      const updated = await api.claimBoardItemOn(targetProfileId, item.id, actorId());
      onUpdated?.(targetProfileId, updated);
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      submitting = false;
    }
  }

  async function unclaim() {
    if (!item || submitting) return;
    submitting = true;
    error = null;
    try {
      const updated = await api.releaseBoardItemOn(targetProfileId, item.id, actorId());
      onUpdated?.(targetProfileId, updated);
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      submitting = false;
    }
  }

  async function destroy() {
    if (!item || deleting) return;
    if (!confirmDelete) {
      confirmDelete = true;
      return;
    }
    deleting = true;
    error = null;
    try {
      await api.deleteBoardItemOn(targetProfileId, item.id);
      onDeleted?.(targetProfileId, item.id);
      onClose();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
      deleting = false;
    }
  }

  /// Spawn an agentum session from the ticket. Reuses the fields the
  /// user already provided (workdir / tool / model) so this is a single
  /// click rather than re-asking. The ticket gets stamped with the new
  /// session id so subsequent visits jump straight in.
  let spawning = $state(false);
  async function startSession() {
    if (!item || spawning) return;
    if (!workdir.trim()) {
      error = 'workdir is required to start a session';
      return;
    }
    const toolId = (tool || 'claude').toString();
    spawning = true;
    error = null;
    try {
      // Deterministic name = ticket key, so re-spawning the same row
      // collides against the per-daemon UNIQUE constraint and the
      // server returns its error — which is what we want, not a second
      // pane for the same ticket.
      const name = item.key.toLowerCase();
      const created = await api.createSessionOn(targetProfileId, {
        name,
        workdir: workdir.trim().replace(/\/+$/, ''),
        tool: toolId,
        model: model.trim() || null,
        flags: []
      });
      try { await api.startSessionOn(targetProfileId, created.id); } catch (e) {
        // Started-but-couldn't-attach is rare; surface it but still
        // stamp the session id since the row exists.
        console.warn('start failed, session still created:', e);
      }
      const updated = await api.patchBoardItemOn(targetProfileId, item.id, {
        session_id: created.id
      });
      onUpdated?.(targetProfileId, updated);
      // Jump into the new pane. The /sessions/{id} page reads against
      // the *active* profile, so flip first when targeting a remote
      // server.
      if (targetProfileId !== $activeProfileId) {
        const { setActiveProfile } = await import('$lib/profiles');
        setActiveProfile(targetProfileId);
      }
      if (typeof location !== 'undefined') {
        location.href = `/sessions/${created.id}`;
      }
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
      spawning = false;
    }
  }

  async function openSession() {
    if (!item?.session_id) return;
    if (targetProfileId !== $activeProfileId) {
      const { setActiveProfile } = await import('$lib/profiles');
      setActiveProfile(targetProfileId);
    }
    if (typeof location !== 'undefined') {
      location.href = `/sessions/${item.session_id}`;
    }
  }

  /* -- bound-session panel ----------------------------------------- */

  let boundSession = $state<Session | null>(null);
  let boundSessionError = $state<string | null>(null);
  let paneLines = $state<string[]>([]);
  let paneError = $state<string | null>(null);
  let paneErrorCount = $state(0);
  let panePollId: ReturnType<typeof setInterval> | null = null;
  let paneController: AbortController | null = null;
  let unbinding = $state(false);

  async function refreshBoundSession() {
    if (!item?.session_id) return;
    try {
      const s = await api.getSession(item.session_id);
      boundSession = s;
      boundSessionError = null;
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) {
        boundSessionError = 'bound session no longer exists. unbind to clear the link.';
      }
      boundSession = null;
    }
  }

  async function refreshPane() {
    if (!boundSession || boundSession.status !== 'running' || !item?.session_id) return;
    // Abort any in-flight fetch before starting a new one.
    paneController?.abort();
    paneController = new AbortController();
    try {
      const result = await api.getSessionPane(
        item.session_id,
        20,
        { signal: paneController.signal },
      );
      paneLines = result.lines;
      paneError = null;
      paneErrorCount = 0;
    } catch (e) {
      // AbortError is benign — the effect teardown cancelled the fetch.
      if (e instanceof Error && e.name === 'AbortError') return;
      paneErrorCount += 1;
      if (paneErrorCount >= 3) {
        const msg = e instanceof Error ? e.message : String(e);
        paneError = msg.slice(0, 60);
        if (panePollId) { clearInterval(panePollId); panePollId = null; }
      }
    }
  }

  $effect(() => {
    // Teardown previous poll before starting a new one.
    if (panePollId) { clearInterval(panePollId); panePollId = null; }
    paneController?.abort();
    paneController = null;
    paneLines = [];
    paneError = null;
    paneErrorCount = 0;
    boundSession = null;
    boundSessionError = null;

    if (!open || !item || !item.session_id) return;
    void refreshBoundSession();
    // Poll only when session is running — detected after refreshBoundSession lands,
    // but we start the interval now and refreshPane guards on status itself.
    const tick = () => {
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
      void refreshPane();
    };
    tick();
    panePollId = setInterval(tick, 2000);
    const onVis = () => { if (typeof document !== 'undefined' && document.visibilityState === 'visible') tick(); };
    if (typeof document !== 'undefined') document.addEventListener('visibilitychange', onVis);
    return () => {
      if (panePollId) { clearInterval(panePollId); panePollId = null; }
      paneController?.abort();
      paneController = null;
      if (typeof document !== 'undefined') document.removeEventListener('visibilitychange', onVis);
    };
  });

  async function unbindSession() {
    if (!item || unbinding) return;
    unbinding = true;
    try {
      const updated = await api.patchBoardItemOn(targetProfileId, item.id, { session_id: null });
      // Optimistic local clear — the parent will also re-emit a board.updated event.
      item = updated;
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      console.error('unbind failed', msg);
    } finally {
      unbinding = false;
    }
  }

  /* -- comments thread --------------------------------------------- */

  let comments = $state<BoardComment[]>([]);
  let commentDraft = $state('');
  let commentLoading = $state(false);
  let postingComment = $state(false);
  let commentError = $state<string | null>(null);

  /// Last (item.id, profile) we fetched for. Guards against repeated
  /// fetches on every re-render — only refetch when the dialog opens
  /// onto a different ticket.
  let lastCommentsFor = $state<string>('');

  async function refreshComments() {
    if (mode !== 'edit' || !item) return;
    const key = `${targetProfileId}:${item.id}`;
    if (key === lastCommentsFor) return;
    lastCommentsFor = key;
    commentLoading = true;
    commentError = null;
    try {
      const list = await api.listBoardCommentsOn(targetProfileId, item.id);
      // Only adopt if the dialog is still on the same ticket — otherwise
      // a slow request would clobber the user's current view.
      if (lastCommentsFor === key) comments = list;
    } catch (e) {
      if (lastCommentsFor === key) {
        commentError = e instanceof Error ? e.message : String(e);
      }
    } finally {
      if (lastCommentsFor === key) commentLoading = false;
    }
  }

  $effect(() => {
    if (open && mode === 'edit' && item) {
      void refreshComments();
    } else if (!open) {
      // Reset on close so the next open shows a clean state.
      comments = [];
      commentDraft = '';
      commentError = null;
      lastCommentsFor = '';
    }
  });

  /// Subscribe to the WS event bus so peer-posted comments (and our
  /// own — convenient when the POST flies before the optimistic
  /// applyComment lands) appear in the thread live.
  $effect(() => {
    if (!open || mode !== 'edit' || !item) return;
    let cancelled = false;
    let unsub: (() => void) | null = null;
    void import('$stores/events').then(({ onEvent }) => {
      if (cancelled) return;
      unsub = onEvent((ev) => {
        if (ev.kind !== 'board.commented') return;
        const targetId = (ev.payload?.board_id as number | undefined) ?? -1;
        if (!item || targetId !== item.id) return;
        // Force a refetch — payload doesn't carry the body. Cheap:
        // the route just SELECTs from a small per-ticket index.
        lastCommentsFor = '';
        void refreshComments();
      });
    });
    return () => { cancelled = true; unsub?.(); };
  });

  function fmtCommentTime(iso: string): string {
    try {
      return new Date(iso).toLocaleString();
    } catch {
      return iso;
    }
  }

  async function postComment(e?: Event) {
    e?.preventDefault();
    if (!item || postingComment) return;
    const body = commentDraft.trim();
    if (!body) return;
    postingComment = true;
    commentError = null;
    try {
      const created = await api.createBoardCommentOn(targetProfileId, item.id, {
        author: actorId(),
        body
      });
      // Optimistic apply — the WS event will refresh too but this
      // makes the textarea-to-thread roundtrip feel instant.
      comments = [...comments, created];
      const { bumpCommentCount } = await import('$stores/fleet-board');
      bumpCommentCount(targetProfileId, item.id, 1);
      commentDraft = '';
    } catch (err) {
      commentError = err instanceof Error ? err.message : String(err);
    } finally {
      postingComment = false;
    }
  }

  const claimedByMe = $derived(
    item?.claimed_by != null && item.claimed_by === actorId()
  );

  /// Distinct status names we know about — backend's defaults + any
  /// custom column the parent is currently rendering. Falls back to the
  /// canonical three when the parent didn't pass anything.
  const statusOptions = $derived.by(() => {
    const set = new Set<string>(['todo', 'doing', 'done']);
    for (const c of columns) set.add(c);
    if (status) set.add(status);
    return Array.from(set);
  });

  /* -- per-status validation gate ---------------------------------- */

  /// Required fields for the currently-selected target status. Empty
  /// when the column is custom (no rule) — keeps the matrix mirroring
  /// the Rust source.
  const requiredFields = $derived(requiredFieldsFor(status));

  /// What's *currently* missing given the in-flight form state. The
  /// dialog uses this to disable submit and paint inline hints. The
  /// `session_id_or_comment` check here is client-side best-effort —
  /// the server is authoritative for the OR-clause's comment fallback.
  const missingFields = $derived(
    validateTransition(status, {
      title: title.trim(),
      lbl: lbl || null,
      workdir: workdir.trim(),
      tool: tool || null,
      // claimed_by: edit mode reads from the row; create mode never
      // sets it (the dedicated /claim endpoint owns this column).
      claimed_by: mode === 'edit' ? (item?.claimed_by ?? null) : null,
      session_id: mode === 'edit' ? (item?.session_id ?? null) : null
    })
  );

  /// True iff every required field is satisfied client-side. The
  /// `session_id_or_comment` case is special: defensively, we let
  /// submit go through even when only `session_id` is missing —
  /// the server might still pass via the comments fallback that
  /// the client can't see.
  const clientGatePasses = $derived.by(() => {
    if (missingFields.length === 0) return true;
    // Allow submit when the only thing missing is the OR-clause —
    // server will resolve it via the comments check.
    return (
      missingFields.length === 1 &&
      missingFields[0] === 'session_id_or_comment'
    );
  });

  function isFieldMissing(field: RequiredField): boolean {
    return missingFields.includes(field) || rejectedFields.has(field);
  }

  function isFieldRejected(field: RequiredField): boolean {
    return rejectedFields.has(field);
  }

  /// Reset rejection highlights when the user changes status — moving
  /// the card to a new column starts a fresh gate evaluation. The
  /// `priorStatus` guard prevents this from wiping the seed during the
  /// initial render (where `status` flips from '' to its real value as
  /// part of the seed effect). `priorStatus` is declared above so the
  /// seed effect can preset it before this clears any seeded rejection.
  $effect(() => {
    if (status === priorStatus) return;
    if (priorStatus !== '' && rejectedFields.size > 0) {
      rejectedFields = new Set();
    }
    priorStatus = status;
  });
</script>

<svelte:window onkeydown={open ? onKey : undefined} />

{#if open}
  <div class="backdrop" onmousedown={onBackdrop} role="presentation">
    <form class="dialog" onsubmit={submit}>
      <header>
        <div class="hd">
          {#if mode === 'edit' && item}
            <span class="key">{item.key}</span>
          {/if}
          <h3>{mode === 'create' ? 'New ticket' : 'Edit ticket'}</h3>
          <p class="sub">
            {mode === 'create'
              ? 'A board item — title required, everything else optional.'
              : 'Update the ticket. Changes save when you submit.'}
          </p>
        </div>
        <button type="button" class="x" onclick={close} aria-label="close">×</button>
      </header>

      <label class="field title-field">
        <span class="lbl">Title</span>
        <!-- svelte-ignore a11y_autofocus -->
        <input
          type="text"
          class="title-input"
          bind:value={title}
          placeholder="What is the agent going to do?"
          autocomplete="off"
          spellcheck="false"
          required
          autofocus={mode === 'create'}
        />
      </label>

      <label class="field">
        <span class="lbl">
          Body
          {#if mode === 'create' && startNow}
            <span class="hint-prompt">— sent to the agent as its first prompt</span>
          {:else}
            <span class="opt">optional</span>
          {/if}
        </span>
        <textarea
          class="body-input"
          bind:value={body}
          rows="6"
          placeholder={mode === 'create' && startNow
            ? 'Tell the agent what you want done.\n\nE.g. "Add a `/api/status` endpoint that returns build SHA + uptime, with tests."'
            : 'Notes, links, acceptance criteria…'}
          spellcheck="false"
        ></textarea>
      </label>

      <section>
        <span class="eyebrow">
          Server
          {#if mode === 'edit'}<span class="opt" style="text-transform:none; letter-spacing:0;">— locked once created</span>{/if}
        </span>
        <div class="agents">
          {#each $profiles as p (p.id)}
            <button
              type="button"
              class="agent"
              class:on={targetProfileId === p.id}
              class:off={mode === 'edit' && targetProfileId !== p.id}
              disabled={mode === 'edit' && targetProfileId !== p.id}
              onclick={() => pickServer(p.id)}
              title={p.baseUrl || 'http://current-origin'}
            >
              <span
                class="dot"
                style:background={p.baseUrl ? 'var(--cta)' : 'var(--green, #2ea043)'}
              ></span>
              <span class="a-name">{serverLabel(p)}</span>
              <span class="a-desc">{serverHost(p)}</span>
            </button>
          {/each}
        </div>
      </section>

      <section class="grid">
        <label class="field">
          <span class="lbl">Column</span>
          <select bind:value={status}>
            {#each statusOptions as s (s)}
              <option value={s}>{s}</option>
            {/each}
          </select>
        </label>
        <label class="field">
          <span class="lbl">
            Label
            {#if isFieldMissing('lbl')}
              <span class="req">required for {status}</span>
            {:else}
              <span class="opt">optional</span>
            {/if}
          </span>
          <select bind:value={lbl} class:bad={isFieldRejected('lbl')}>
            <option value="">—</option>
            {#each LBLS as l (l.id)}
              <option value={l.id}>{l.label}</option>
            {/each}
          </select>
        </label>
      </section>

      {#if requiredFields.length > 0 && missingFields.length > 0}
        <div class="gate-hint">
          <span class="gate-pill">required for <em>{status}</em>:</span>
          {#each missingFields as f (f)}
            <span class="gate-key">{requiredFieldLabel(f)}</span>
          {/each}
        </div>
      {/if}

      <!--
        Existing-session picker (create mode only). Lets the user attach
        the new card to a running tmux pane instead of letting the
        auto-spawn branch start a fresh one. Picking prefills workdir +
        tool from the session so the visible context is honest.
      -->
      {#if mode === 'create'}
        <section>
          <span class="eyebrow">
            Existing session
            <span class="opt" style="text-transform:none; letter-spacing:0;">
              {#if attachableSessions.length === 0}
                — none available on this server (a fresh pane will spawn when you move the card to doing)
              {:else if pickedSessionId}
                — this card will attach to the picked pane on submit; no new session is spawned
              {:else}
                — optional: attach to a running pane instead of auto-spawning
              {/if}
            </span>
          </span>
          {#if attachableSessions.length > 0}
            <div class="sessions">
              {#each attachableSessions as s (s.id)}
                <button
                  type="button"
                  class="session-tile"
                  class:on={pickedSessionId === s.id}
                  onclick={() => toggleSession(s)}
                  title={`${s.name} · ${s.tool} · ${s.workdir}`}
                >
                  <span class="s-dot" style:background="var(--tool-{s.tool}, var(--cta))"></span>
                  <span class="s-name">{s.name}</span>
                  <span class="s-meta">{s.tool} · {s.workdir}</span>
                </button>
              {/each}
              {#if pickedSessionId}
                <button
                  type="button"
                  class="session-tile session-clear"
                  onclick={() => (pickedSessionId = '')}
                  title="Forget the picked session — auto-spawn will fire on move to doing"
                >
                  <span class="s-name">none</span>
                  <span class="s-meta">auto-spawn instead</span>
                </button>
              {/if}
            </div>
          {/if}
        </section>
      {/if}

      <section>
        <span class="eyebrow">
          Agent
          {#if autoHint}<span class="opt" style="text-transform:none; letter-spacing:0;">— {autoHint}</span>{/if}
        </span>
        <div class="agents">
          <button
            type="button"
            class="agent"
            class:on={tool === ''}
            onclick={() => (tool = '')}
            title="No specific agent assignment"
          >
            <span class="dot" style:background="var(--fg-3)"></span>
            <span class="a-name">unassigned</span>
            <span class="a-desc">no agent</span>
          </button>
          {#each TOOLS as t (t.id)}
            {@const avail = toolAvailable(t)}
            {@const reason = toolUnavailableReason(t)}
            <button
              type="button"
              class="agent"
              class:on={tool === t.id}
              class:off={!avail}
              disabled={!avail}
              title={reason ?? ''}
              onclick={() => avail && (tool = t.id)}
            >
              <span class="dot" style:background={t.dot}></span>
              <span class="a-name">{t.label}</span>
              <span class="a-desc">{avail ? t.desc : 'not installed'}</span>
            </button>
          {/each}
        </div>
      </section>

      <section class="grid">
        <label class="field" class:bad={isFieldRejected('workdir')}>
          <span class="lbl">
            Working directory
            {#if isFieldMissing('workdir')}
              <span class="req">required for {status}</span>
            {:else}
              <span class="opt">optional</span>
            {/if}
          </span>
          <DirPicker
            bind:value={workdir}
            onChange={(v) => (workdir = v)}
            placeholder="~/projects/foo"
          />
        </label>
        <label class="field">
          <span class="lbl">Model <span class="opt">optional</span></span>
          <input
            type="text"
            bind:value={model}
            placeholder={currentToolHint}
            autocomplete="off"
            spellcheck="false"
          />
        </label>
      </section>

      {#if mode === 'edit' && item}
        <section class="claim-row">
          <div class="claim-meta">
            <span class="lbl" title="Who is currently working on this ticket — claim it to mark it as yours, release to hand off">Owner</span>
            <span class="claim-text">
              {#if item.claimed_by}
                <span class="actor" class:me={claimedByMe}>{item.claimed_by}</span>
                {#if claimedByMe}<span class="sub-tag">— you</span>{/if}
              {:else}
                <span class="actor unclaimed">no one yet</span>
              {/if}
            </span>
          </div>
          {#if item.claimed_by}
            <button
              type="button"
              class="ghost"
              onclick={unclaim}
              disabled={submitting || !claimedByMe}
              title={!claimedByMe ? 'only the current holder can release it' : 'Release the ticket so someone else can pick it up'}
            >Release</button>
          {:else}
            <button
              type="button"
              class="ghost"
              onclick={claim}
              disabled={submitting}
              title="Mark this ticket as yours"
            >Take it</button>
          {/if}
        </section>

        <section class="claim-row">
          <div class="claim-meta">
            <span class="lbl" title="The tmux pane (and the agent running in it) that this ticket is attached to">Agent pane</span>
            <span class="claim-text">
              {#if item.session_id}
                <span class="actor" title={item.session_id}>{item.session_id.slice(0, 8)}…</span>
              {:else}
                <span class="actor unclaimed">not attached yet</span>
              {/if}
            </span>
          </div>
          {#if item.session_id}
            <div class="session-actions">
              <button
                type="button"
                class="ghost"
                onclick={openSession}
                disabled={submitting || spawning}
                title="Open the pane in a new dashboard tab"
              >Open ↗</button>
              <button
                type="button"
                class="ghost"
                onclick={unbindSession}
                disabled={submitting || spawning || unbinding}
                title={`Detach this ticket from ${item.session_id.slice(0, 8)}… (the pane keeps running)`}
              >
                {#if unbinding}Detaching…{:else}Detach{/if}
              </button>
            </div>
          {:else}
            <button
              type="button"
              class="ghost"
              onclick={startSession}
              disabled={submitting || spawning || !workdir.trim()}
              title={!workdir.trim() ? 'set a working directory first' : 'Spawn a fresh tmux pane bound to this ticket'}
            >
              {#if spawning}
                <span class="spin"></span> starting…
              {:else}
                Start a pane
              {/if}
            </button>
          {/if}
        </section>

        {#if item.session_id && boundSession}
          <section class="bound-session-panel">
            <div class="bsp-head">
              <span class="eyebrow">BOUND SESSION</span>
              <StatusPill status={boundSession.status} />
            </div>
            <pre class="pane-tail" aria-live="polite">{#if paneError}couldn't fetch pane: {paneError}{:else if paneLines.length === 0 && boundSession.status === 'running'}waiting for output…{:else if paneLines.length === 0}pane not active.{:else}{paneLines.join('\n')}{/if}</pre>
            <a class="open-session-link" href={`/sessions/${boundSession.id}`}>Open session →</a>
          </section>
        {:else if boundSessionError}
          <section class="bound-session-panel error">
            <p class="muted">{boundSessionError}</p>
          </section>
        {/if}

        <section class="comments">
          <span class="eyebrow">Comments {#if comments.length > 0}<span class="count-chip">{comments.length}</span>{/if}</span>
          {#if commentLoading && comments.length === 0}
            <div class="cmt-empty mono">loading…</div>
          {:else if comments.length === 0}
            <div class="cmt-empty mono">no comments yet — be the first.</div>
          {:else}
            <ol class="cmt-list">
              {#each comments as c (c.id)}
                <li
                  class="cmt-item"
                  class:system={c.author === 'system'}
                  class:crash={c.author === 'system' && c.body.startsWith('[system] session crashed:')}
                >
                  <div class="cmt-head">
                    <span class="cmt-author">{c.author}</span>
                    <span class="cmt-time" title={c.created_at}>{fmtCommentTime(c.created_at)}</span>
                  </div>
                  <div class="cmt-body">{c.body}</div>
                </li>
              {/each}
            </ol>
          {/if}
          {#if commentError}
            <div class="error" style="margin-top: 6px;">{commentError}</div>
          {/if}
          <!-- Nested <form> inside the dialog's outer form is invalid
               HTML, so use a div + button click + Cmd/Ctrl+Enter
               shortcut. The textarea explicitly stops `keydown` from
               reaching the outer form's accidental Enter-submit. -->
          <div class="cmt-form">
            <textarea
              bind:value={commentDraft}
              rows="2"
              placeholder="Add a comment… (⌘/Ctrl+Enter to post)"
              spellcheck="false"
              disabled={postingComment}
              onkeydown={(e) => {
                if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                  e.preventDefault();
                  void postComment();
                }
              }}
            ></textarea>
            <button
              type="button"
              class="primary"
              disabled={postingComment || commentDraft.trim().length === 0}
              onclick={() => void postComment()}
            >
              {#if postingComment}
                <span class="spin"></span> posting…
              {:else}
                Post
              {/if}
            </button>
          </div>
        </section>
      {/if}

      {#if error}
        <div class="error">{error}</div>
      {/if}

      <footer>
        {#if mode === 'edit'}
          <button
            type="button"
            class="danger"
            onclick={destroy}
            disabled={deleting || submitting}
          >
            {#if deleting}
              <span class="spin"></span> deleting…
            {:else if confirmDelete}
              Confirm delete
            {:else}
              Delete
            {/if}
          </button>
        {/if}
        <span class="spacer"></span>
        {#if mode === 'create'}
          <!--
            Start-now toggle. On (default) → after create, the dialog
            claims the card and PATCHes status=doing, which fires the
            server's auto-spawn branch (or attaches the picked existing
            session). Off → card lands in its column unstarted, like a
            classic draft. Direct response to user feedback: "I created
            the ticket and it didn't trigger at start, we should have
            an option for that".
          -->
          <label
            class="start-now"
            class:on={startNow}
            title={startNow
              ? 'On submit: claim, move to doing, spawn an agent pane, and send the body as its first prompt.'
              : 'Drafts the ticket without spawning an agent. Move the card to doing later to start.'}
          >
            <input type="checkbox" bind:checked={startNow} disabled={submitting} />
            <span class="dot" aria-hidden="true"></span>
            <span class="sn-label">Start now</span>
          </label>
        {/if}
        <button type="button" class="ghost" onclick={close} disabled={submitting || deleting}>Cancel</button>
        <!--
          Submit stays enabled even when the client-side gate fails so
          clicking always produces feedback (either the create succeeds,
          the server gate returns a 400 with `{missing, status}`, or our
          own preview-check above surfaces the missing fields as a
          banner). Disabling the button was the silent-submit trap that
          made users think the dialog was broken.
        -->
        <button
          type="submit"
          class="primary"
          disabled={submitting || deleting}
          title={clientGatePasses ? '' : `move to ${status} needs: ${missingFields.map(requiredFieldLabel).join(', ')}`}
        >
          {#if submitting}
            <span class="spin"></span>
            {#if mode === 'create'}
              {startNow ? 'starting…' : 'creating…'}
            {:else}
              saving…
            {/if}
          {:else if mode === 'create'}
            {startNow ? 'Create + start' : 'Create ticket'}
          {:else}
            Save changes
          {/if}
        </button>
      </footer>
    </form>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.62);
    z-index: 80;
    display: grid;
    place-items: center;
    padding: 1rem;
    backdrop-filter: blur(3px);
  }
  .dialog {
    background: var(--bg);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-lg);
    padding: 22px 22px 16px;
    width: min(580px, 100%);
    max-height: 90vh;
    overflow-y: auto;
    box-shadow:
      0 0 0 1px rgba(255, 255, 255, 0.02) inset,
      0 24px 64px rgba(0, 0, 0, 0.55);
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
  }
  .hd { display: flex; flex-direction: column; gap: 4px; }
  .key {
    font-family: var(--mono);
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-3);
  }
  h3 {
    margin: 0;
    font-family: var(--display);
    font-size: 18px;
    font-weight: 500;
    letter-spacing: -0.02em;
    color: var(--fg);
  }
  .sub {
    margin: 2px 0 0;
    color: var(--fg-3);
    font-size: 12.5px;
  }
  .x {
    background: none;
    border: 0;
    color: var(--fg-3);
    font-size: 22px;
    line-height: 1;
    cursor: pointer;
    padding: 0 6px;
    transition: color var(--t-hover);
  }
  .x:hover { color: var(--fg); }

  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }

  .field { display: flex; flex-direction: column; gap: 6px; }
  .lbl {
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-3);
  }
  .opt { color: var(--fg-3); text-transform: none; letter-spacing: 0; font-size: 10px; }
  /* Per-status required-field hint. Renders next to a field's label
     when the validator's missing[] includes it. Same shape as .opt
     so the row stays balanced when the badge swaps mid-edit. */
  .req {
    color: var(--cta);
    text-transform: none;
    letter-spacing: 0;
    font-size: 10px;
  }
  /* Highlight a field the server flagged in the most recent 400's
     `missing[]`. The CSS variable `--crash` is the same red the .error
     box uses, so the dialog's failure language reads consistently.
     `.field.bad` applies to the workdir picker (a custom component, so
     we colour the wrapping label's children); `select.bad` covers the
     lbl picker which gets the class directly. */
  .field.bad :global(input),
  .field.bad :global(button),
  select.bad {
    border-color: var(--crash);
  }
  /* "Required for *<status>*" summary strip beneath the column picker —
     gives the user a single glance of what's missing before they try
     to submit. Suppressed when the form already satisfies the gate. */
  .gate-hint {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    background: color-mix(in srgb, var(--cta) 8%, var(--surface));
    border: 1px solid color-mix(in srgb, var(--cta) 40%, var(--border-2));
    border-radius: var(--radius-md);
    font-family: var(--mono);
    font-size: 11px;
    color: var(--fg-2);
  }
  .gate-pill {
    color: var(--fg-3);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-size: 9.5px;
  }
  .gate-pill em { color: var(--cta); font-style: normal; }
  .gate-key {
    padding: 1px 6px;
    border-radius: var(--radius-pill);
    background: var(--bg-2);
    border: 1px solid var(--border-2);
    color: var(--fg);
  }

  input[type='text'],
  textarea,
  select {
    padding: 8px 10px;
    background: var(--bg-2);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    color: var(--fg);
    font-family: var(--mono);
    font-size: 12.5px;
    transition: border-color var(--t-hover);
  }
  textarea {
    resize: vertical;
    min-height: 76px;
    line-height: 1.45;
  }
  input[type='text']:focus,
  textarea:focus,
  select:focus {
    outline: none;
    border-color: var(--cta);
  }
  input[type='text']::placeholder,
  textarea::placeholder { color: var(--fg-3); }

  .eyebrow {
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--fg-3);
  }

  .agents {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 6px;
  }

  /* Existing-session picker — same visual rhythm as .agent tiles so
     the dialog reads as one consistent set of "pick something" rows. */
  .sessions {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 6px;
  }
  .session-tile {
    display: grid;
    grid-template-columns: auto 1fr;
    grid-template-rows: auto auto;
    align-items: center;
    column-gap: 8px;
    row-gap: 2px;
    padding: 9px 10px;
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    color: var(--fg-2);
    cursor: pointer;
    transition: border-color var(--t-hover), background var(--t-hover), color var(--t-hover);
    text-align: left;
  }
  .session-tile:hover { border-color: var(--fg-3); color: var(--fg); }
  .session-tile.on {
    border-color: var(--cta);
    background: color-mix(in srgb, var(--cta) 10%, var(--surface));
    color: var(--fg);
  }
  .session-tile .s-dot {
    width: 7px;
    height: 7px;
    border-radius: var(--radius-pill);
    grid-row: 1 / span 2;
  }
  .session-tile .s-name {
    font-size: 13px;
    letter-spacing: -0.01em;
    color: inherit;
  }
  .session-tile .s-meta {
    grid-column: 2;
    font-family: var(--mono);
    font-size: 10px;
    color: var(--fg-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .session-clear {
    border-style: dashed;
    grid-template-columns: 1fr;
  }
  .session-clear .s-meta { grid-column: 1; }

  .agent {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 3px;
    padding: 9px 10px;
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    color: var(--fg-2);
    cursor: pointer;
    transition: border-color var(--t-hover), background var(--t-hover), color var(--t-hover);
    text-align: left;
  }
  .agent:hover:not(:disabled) { border-color: var(--fg-3); color: var(--fg); }
  .agent.on {
    border-color: var(--cta);
    background: color-mix(in srgb, var(--cta) 10%, var(--surface));
    color: var(--fg);
  }
  .agent.off {
    opacity: 0.45;
    cursor: not-allowed;
    filter: grayscale(0.6);
  }
  .agent.off .a-desc { color: var(--crash); }
  .agent .dot {
    width: 7px;
    height: 7px;
    border-radius: var(--radius-pill);
  }
  .agent .a-name {
    font-size: 13px;
    letter-spacing: -0.01em;
    color: inherit;
  }
  .agent .a-desc {
    font-family: var(--mono);
    font-size: 9.5px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-3);
  }

  .claim-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
  }

  /* Comments thread + reply form. Sits inside the dialog as a tall
     scrollable section so long threads don't blow out the modal. */
  .comments {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .count-chip {
    margin-left: 6px;
    padding: 1px 6px;
    border-radius: var(--radius-pill);
    background: var(--bg-2);
    border: 1px solid var(--border-2);
    color: var(--fg-3);
    font-size: 9.5px;
    text-transform: none;
    letter-spacing: 0;
  }
  .cmt-empty {
    padding: 10px 12px;
    color: var(--fg-3);
    font-size: 11.5px;
    text-align: center;
    border: 1px dashed var(--border-2);
    border-radius: var(--radius-md);
  }
  .cmt-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-height: 240px;
    overflow-y: auto;
  }
  .cmt-item {
    padding: 8px 10px;
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
  }
  .cmt-head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    margin-bottom: 4px;
  }
  .cmt-author {
    font-family: var(--mono);
    font-size: 10.5px;
    color: var(--cta);
  }
  .cmt-time {
    font-family: var(--mono);
    font-size: 9.5px;
    color: var(--fg-3);
  }
  .cmt-body {
    font-size: 12.5px;
    color: var(--fg);
    line-height: 1.4;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .cmt-form {
    display: flex;
    gap: 8px;
    align-items: flex-end;
  }
  .cmt-form textarea {
    flex: 1;
    padding: 8px 10px;
    background: var(--bg-2);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
    color: var(--fg);
    font-family: var(--mono);
    font-size: 12.5px;
    resize: vertical;
    min-height: 48px;
  }
  .cmt-form textarea:focus { outline: none; border-color: var(--cta); }
  .cmt-form button.primary {
    padding: 8px 14px;
    border-radius: var(--radius-md);
    border: 1px solid var(--cta);
    background: var(--cta);
    color: #fff;
    font-family: var(--mono);
    font-size: 11.5px;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    transition: filter var(--t-hover);
  }
  .cmt-form button.primary:hover:not(:disabled) { filter: brightness(1.05); }
  .cmt-form button.primary:disabled { opacity: 0.5; cursor: not-allowed; }
  .claim-meta { display: flex; flex-direction: column; gap: 2px; flex: 1; min-width: 0; }
  .claim-text {
    display: inline-flex;
    align-items: baseline;
    gap: 6px;
    font-family: var(--mono);
    font-size: 12px;
  }
  .actor { color: var(--fg); }
  .actor.me { color: var(--cta); }
  .actor.unclaimed { color: var(--fg-3); }
  .sub-tag { color: var(--fg-3); font-size: 10.5px; }

  .error {
    padding: 8px 10px;
    border: 1px solid var(--crash);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--crash) 10%, transparent);
    color: var(--crash);
    font-size: 12px;
    font-family: var(--mono);
    word-break: break-word;
  }

  footer {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 4px;
    padding-top: 12px;
    border-top: 1px solid var(--border);
  }
  footer .spacer { flex: 1; }
  footer button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 7px 14px;
    border-radius: var(--radius-md);
    border: 1px solid var(--border-2);
    background: var(--surface);
    color: var(--fg);
    font-family: var(--mono);
    font-size: 11.5px;
    cursor: pointer;
    transition: border-color var(--t-hover), background var(--t-hover), color var(--t-hover);
  }
  footer .ghost:hover:not(:disabled) { border-color: var(--fg-3); }
  footer .primary {
    background: var(--cta);
    color: #fff;
    border-color: var(--cta);
  }
  footer .primary:hover:not(:disabled) { filter: brightness(1.05); }
  footer .danger {
    color: var(--crash);
    border-color: color-mix(in srgb, var(--crash) 50%, var(--border-2));
  }
  footer .danger:hover:not(:disabled) {
    background: color-mix(in srgb, var(--crash) 12%, var(--surface));
    border-color: var(--crash);
  }
  footer button:disabled { opacity: 0.55; cursor: not-allowed; }

  /* Start-now toggle — pill-style modifier on the primary submit. When
     ON (default), the dialog claims the card + moves to doing right
     after create so the server's auto-spawn fires. Visually prominent
     because users were missing it. */
  footer .start-now {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    margin-right: 6px;
    padding: 6px 11px;
    border-radius: var(--radius-pill);
    border: 1px solid var(--border-2);
    background: var(--surface);
    color: var(--fg-2);
    font-family: var(--mono);
    font-size: 11px;
    cursor: pointer;
    user-select: none;
    transition: border-color var(--t-hover), background var(--t-hover), color var(--t-hover);
  }
  footer .start-now input[type='checkbox'] {
    position: absolute;
    opacity: 0;
    pointer-events: none;
    width: 0;
    height: 0;
  }
  footer .start-now .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--fg-3);
    transition: background var(--t-hover), box-shadow var(--t-hover);
  }
  footer .start-now.on {
    border-color: var(--cta);
    background: color-mix(in srgb, var(--cta) 10%, var(--surface));
    color: var(--fg);
  }
  footer .start-now.on .dot {
    background: var(--cta);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--cta) 22%, transparent);
  }
  footer .start-now:hover { color: var(--fg); border-color: var(--fg-3); }
  footer .start-now.on:hover { border-color: var(--cta); }
  footer .start-now:focus-within {
    outline: 2px solid var(--cta);
    outline-offset: 2px;
  }

  /* "this body becomes the agent's first prompt" hint next to the Body
     label — only shown in create + start-now mode so users learn what
     the field actually does at the moment it matters. */
  .hint-prompt {
    color: var(--cta);
    text-transform: none;
    letter-spacing: 0;
    font-size: 10px;
    font-weight: 500;
  }
  /* Bigger body textarea — body is the agent's brief; gets visual
     weight to match its importance. */
  .body-input {
    min-height: 110px;
    line-height: 1.5;
  }
  /* Title gets a larger, display-font input so the most-prominent
     field reads as the heading of the dialog. */
  .title-input {
    font-family: var(--display);
    font-size: 15.5px;
    font-weight: 500;
    letter-spacing: -0.01em;
    padding: 10px 12px;
  }

  .spin {
    width: 11px;
    height: 11px;
    border-radius: 50%;
    border: 1.5px solid rgba(255,255,255,0.45);
    border-top-color: #fff;
    animation: spin 0.7s linear infinite;
  }
  @keyframes spin { to { transform: rotate(360deg); } }

  @media (max-width: 720px) {
    .backdrop { align-items: flex-end; padding: 0; }
    .dialog {
      width: 100%;
      max-width: 100%;
      max-height: 92dvh;
      border-radius: 18px 18px 0 0;
      padding: 18px 16px calc(18px + env(safe-area-inset-bottom, 0px));
      animation: sheet-in 220ms cubic-bezier(0.2, 0.7, 0.2, 1);
    }
    @keyframes sheet-in {
      from { transform: translateY(100%); }
      to   { transform: translateY(0); }
    }
    input[type='text'],
    textarea,
    select {
      padding: 12px;
      font-size: 16px !important;
      border-radius: 10px;
    }
    .agents { grid-template-columns: repeat(2, 1fr); }
    footer {
      flex-wrap: wrap;
      padding-top: 14px;
    }
    footer button {
      padding: 12px 16px;
      font-size: 13px;
      border-radius: 10px;
      min-height: 44px;
    }
  }
  @media (max-width: 540px) {
    .grid { grid-template-columns: 1fr; }
  }

  /* ---- Bound-session panel ---- */
  .bound-session-panel {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 16px;
    margin-bottom: 0;
    background: var(--surface);
    border: 1px solid var(--border-2);
    border-radius: var(--radius-md);
  }
  .bound-session-panel.error { padding: 8px 16px; }
  .bsp-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .pane-tail {
    font-family: var(--mono);
    font-size: 11px;
    line-height: 1.4;
    color: var(--fg-2);
    background: var(--bg-2);
    padding: 8px;
    margin: 0;
    max-height: calc(20 * 1.4em + 16px);
    overflow-y: auto;
    white-space: pre;
  }
  @media (max-width: 720px) {
    .pane-tail { max-height: calc(12 * 1.4em + 16px); font-size: 16px; }
  }
  .open-session-link {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--fg-2);
    text-decoration: none;
  }
  .open-session-link:hover { color: var(--link, var(--cta)); }
  .muted { color: var(--fg-3); font-size: 12px; margin: 0; }
  .session-actions { display: flex; gap: 8px; align-items: center; }
  /* ---- System comment rows ---- */
  .cmt-item.system .cmt-author { color: var(--fg-3); }
  .cmt-item.system.crash { border-left: 2px solid var(--crash); padding-left: 8px; }
</style>
