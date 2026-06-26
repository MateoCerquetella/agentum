//! Per-session pane streaming: the WebSocket handlers that tail a tmux pane's
//! output to a client — local (on-disk pipe-pane log) and remote (SSH `tail -f`)
//! — plus the burst-coalescing + settle-repaint helpers. `use super::*` pulls in
//! the parent route module's imports + helpers (e.g. `save_checkpoint`); the two
//! session entry points are `pub(super)` so the `stream` handler can call them.

use super::*;

fn coalesce_queued(first: Bytes, rx: &mut tokio::sync::mpsc::Receiver<Bytes>) -> Bytes {
    use tokio::sync::mpsc::error::TryRecvError;
    match rx.try_recv() {
        // Nothing else waiting → forward the lone chunk as-is (zero-copy).
        Err(_) => first,
        Ok(second) => {
            let mut buf = Vec::with_capacity(first.len() + second.len());
            buf.extend_from_slice(&first);
            buf.extend_from_slice(&second);
            while buf.len() < COALESCE_MAX {
                match rx.try_recv() {
                    Ok(more) => buf.extend_from_slice(&more),
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
            Bytes::from(buf)
        }
    }
}

/// Block until the embedded process's post-SIGWINCH repaint burst has
/// settled, so a `capture-pane` taken afterwards reflects a complete frame
/// rather than a half-painted one. The pane log file (the pipe-pane sink)
/// is a cheap activity probe: bytes the process emits are appended in real
/// time, so file-size growth is direct evidence of repaint work. Wait for
/// activity to start, then for it to quiet (two no-growth polls). Bail
/// early when no activity ever shows (a no-op resize that propagated no
/// SIGWINCH) and hard-cap so an actively-streaming agent can't pin the
/// connect open. Shared by the resize-settle and redraw-nudge paths.
async fn settle_repaint_burst(file: &tokio::fs::File) {
    let mut last_size = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let mut activity_seen = false;
    let mut quiet_streak: u32 = 0;
    let start = tokio::time::Instant::now();
    let max_deadline = start + POST_RESIZE_SETTLE_MAX;
    loop {
        sleep(SETTLE_POLL_INTERVAL).await;
        let now_size = file.metadata().await.map(|m| m.len()).unwrap_or(last_size);
        if now_size != last_size {
            activity_seen = true;
            quiet_streak = 0;
            last_size = now_size;
        } else {
            quiet_streak = quiet_streak.saturating_add(1);
        }
        let now = tokio::time::Instant::now();
        // Activity → quiet: repaint burst is over, capture is safe.
        if activity_seen && quiet_streak >= 2 {
            break;
        }
        // No activity within the bail window: nothing is going to repaint.
        if !activity_seen && now >= start + POST_RESIZE_NO_ACTIVITY_BAIL {
            break;
        }
        // Hard cap against an agent that never goes quiet.
        if now >= max_deadline {
            break;
        }
    }
}

pub(super) async fn stream_session(
    mut socket: WebSocket,
    id: Uuid,
    target: String,
    positions: Arc<std::sync::Mutex<std::collections::HashMap<Uuid, StreamCheckpoint>>>,
    resume_requested: bool,
    redraw_requested: bool,
) {
    let log_path = match paths::pane_log(&id.to_string()) {
        Ok(p) => p,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("[path error: {e}]").into()))
                .await;
            return;
        }
    };

    // Wait briefly for pipe-pane to create the file (it appears milliseconds
    // after `agentum up` returns).
    let mut waited = 0;
    while !log_path.exists() && waited < 50 {
        sleep(Duration::from_millis(100)).await;
        waited += 1;
    }
    if !log_path.exists() {
        let _ = socket
            .send(Message::Text("[no pane log — session not running]".into()))
            .await;
        return;
    }

    let mut file = match tokio::fs::File::open(&log_path).await {
        Ok(f) => f,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("[open error: {e}]").into()))
                .await;
            return;
        }
    };

    // Resize tmux to match the client's viewport BEFORE we snapshot. Without
    // this we capture-pane at tmux's stale pane size (80×24 default for fresh
    // detached sessions, or whatever the previous client sat at), the embedded
    // TUI keeps emitting cursor-position escapes against that size, and the
    // client's vt100 parser — sized to the actual viewport — places the
    // characters in the wrong cells. Symptom: status-line text like "esc to
    // interrupt" overpaints scrollback content and you end up with artefacts
    // like `okterrupt` permanently baked into the scrollback buffer.
    //
    // Modern clients (TUI ≥ 0.6.7, dashboard ≥ 0.6.7) push a `{"resize":...}`
    // text frame within milliseconds of WS open, so this wait almost never
    // reaches the timeout in practice. Old clients fall through to the
    // existing capture-at-current-size path.
    let mut early_input: Vec<Bytes> = Vec::new();
    let mut got_resize = false;
    // Captured on the first resize frame so the resume-replay path can
    // bail out when the client's viewport changed during a disconnect
    // — replaying bytes emitted at a different grid produces visible
    // layout corruption (cursor moves target stale cells).
    let mut current_size: Option<(u16, u16)> = None;
    let resize_deadline = tokio::time::Instant::now() + INITIAL_RESIZE_WAIT;
    loop {
        let remaining = resize_deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, socket.recv()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                if let Some((cols, rows)) = parse_resize(&t) {
                    let _ = agentum_tmux::resize_window(&target, cols, rows).await;
                    got_resize = true;
                    current_size = Some((cols, rows));
                    break;
                }
                // Non-resize text frame — preserve the legacy "treat as raw
                // input" behaviour by buffering for replay after the snapshot.
                early_input.push(Bytes::copy_from_slice(t.as_bytes()));
            }
            Ok(Some(Ok(Message::Binary(b)))) if !b.is_empty() => {
                early_input.push(b);
            }
            Ok(Some(Ok(_))) => {}                  // ping/pong/etc.
            Ok(Some(Err(_))) | Ok(None) => return, // socket already gone
            Err(_) => break,                       // timeout — fall through with no resize
        }
    }
    if got_resize {
        // Wait for the embedded process's post-SIGWINCH repaint burst to
        // settle before snapshotting. Fixed sleeps don't work: idle panes
        // are quiet immediately, but a ratatui-based agent (claude code,
        // codex, opencode) reacting to a real size change can take well
        // over 100 ms to start emitting its full repaint, then several
        // dozen ms more to finish it. Capturing during that window
        // returned a half-painted frame — tool indicator drawn but
        // input box / footer missing, or status-line characters
        // overpainting scrollback content because cursor moves still
        // referenced the old grid.
        //
        // The pane log file (pipe-pane sink) gives us a cheap activity
        // probe: bytes the embedded process emits are appended in real
        // time, so file-size growth is direct evidence of repaint
        // activity. Wait for activity to start, then for it to quiet.
        // Fall back to a "no activity" bail-out if the resize was a
        // no-op (size already matched), so connect doesn't pay the full
        // budget for a settle that will never come.
        settle_repaint_burst(&file).await;
    }

    // Redraw heal: force the agent to repaint EVERY cell before we snapshot.
    // The reconnect after a system suspend (or any foreign write into the
    // pane grid — an OS `wall` broadcast lands on top of the input box and
    // footer) leaves stale bytes a ratatui app won't overpaint on its own,
    // and a same-size reconnect emits no SIGWINCH so the agent never
    // repaints. We provoke one with a momentary 1-row shrink-then-restore:
    // two SIGWINCHs, each making the agent clear its buffer and redraw in
    // full, netting to the original geometry. The intermediate settle gives
    // the app time to observe the smaller size and start repainting before
    // we restore — two resizes delivered too close together can collapse
    // into a single read of the final (unchanged) size and skip the redraw.
    // The post-restore settle lets the clean frame land in the log before
    // the snapshot below captures it. No-op when we never learned the
    // client's size or the pane is too short to shrink.
    if redraw_requested && let Some((cols, rows)) = current_size {
        let shrunk = rows.saturating_sub(1);
        if shrunk >= 1 && shrunk != rows {
            let _ = agentum_tmux::resize_window(&target, cols, shrunk).await;
            settle_repaint_burst(&file).await;
            let _ = agentum_tmux::resize_window(&target, cols, rows).await;
            settle_repaint_burst(&file).await;
        }
    }

    // Replay path. Two modes:
    //
    // 1. RESUME: client has cached parser state and just wants the bytes
    //    it missed during the WS gap. Look up the saved log position and
    //    forward `log[saved..end]` as binary. The client's parser was
    //    not reset, so playing back those bytes brings it from the
    //    pre-disconnect state to the live tail without clobbering
    //    anything. Without this, switching agents and switching back
    //    used to wipe all visible chat history because we'd send a
    //    `capture-pane` snapshot reflecting whatever the embedded TUI's
    //    UI looks like *now* — which after a task completes can be
    //    almost empty.
    //
    // 2. FRESH SNAPSHOT (default): client has no cached state (or its
    //    cached state is invalid because the pane size changed during
    //    the disconnect), so we `capture-pane -e` the current visible
    //    grid and ship it after an `ESC c` (RIS) full parser reset.
    let mut snapshot_sent = false;
    // Resume only if we have a saved checkpoint AND its pane size matches
    // the current viewport. Two guards:
    //
    //  1. `unwrap_or(0)` after a daemon restart wiped `stream_positions`
    //     (in-memory only) made the daemon ship the ENTIRE log as
    //     "delta" on top of the client's existing parser state —
    //     duplicate footer/content baked into scrollback every time
    //     the daemon was bounced. v0.6.26 fixed this by falling through
    //     to the fresh snapshot path when no checkpoint exists.
    //
    //  2. Replaying bytes emitted at a stale grid size (e.g., user
    //     dragged their tmux window during the disconnect) places
    //     cursor moves and line wraps against the wrong cells, so the
    //     visible layout ends up corrupted in ways that survive in the
    //     vt100 parser's history. Mismatch → fall through to a fresh
    //     snapshot at the new size and let the client's parser reset.
    let saved_checkpoint: Option<StreamCheckpoint> =
        positions.lock().ok().and_then(|map| map.get(&id).copied());
    let resume_size_matches = match (saved_checkpoint, current_size) {
        (Some(cp), Some((cols, rows))) => cp.cols == cols && cp.rows == rows,
        // No first-resize frame from the client, or no checkpoint — let
        // the existing "resume only with checkpoint" gate handle it.
        (Some(_), None) => true,
        _ => false,
    };
    // A redraw heal always wants the fresh snapshot of the repainted grid,
    // never a delta replay (which would just re-feed the corrupting bytes).
    // Clients pair `redraw` with omitting `resume`, but gate here too so a
    // client sending both still heals.
    if let (true, Some(cp), true) = (
        resume_requested && !redraw_requested,
        saved_checkpoint,
        resume_size_matches,
    ) {
        if let Ok(end) = file.seek(std::io::SeekFrom::End(0)).await
            && end >= cp.pos
        {
            let delta = end - cp.pos;
            if delta > 0 && file.seek(std::io::SeekFrom::Start(cp.pos)).await.is_ok() {
                let mut buf = vec![0u8; delta as usize];
                if file.read_exact(&mut buf).await.is_ok()
                    && socket
                        .send(Message::Binary(Bytes::from(buf)))
                        .await
                        .is_err()
                {
                    return;
                }
            }
            // Position file at end so tail picks up only post-delta bytes.
            let _ = file.seek(std::io::SeekFrom::End(0)).await;
            snapshot_sent = true;
        }
    }
    if !snapshot_sent
        && let Ok(snap) = agentum_tmux::capture_pane_ansi(&target).await
        && !snap.is_empty()
    {
        // Pin the tail's replay point AFTER capturing, BEFORE sending: the
        // snapshot reflects pane state at capture time, so the tail must resume
        // just past it. Bytes emitted during the (possibly slow) socket send
        // land after this offset and stream through the tail exactly once. The
        // earlier order pinned End BEFORE the capture, replaying the
        // capture-window bytes on top of the snapshot — harmless for an
        // alt-screen app, but for a normal-screen agent that redraws with
        // RELATIVE cursor motion (cursor-agent: ESC[1A + ESC[2K) that duplicate
        // desynced the cursor and stacked spinner lines ("Composing…
        // Composing…"). The trade is a sub-ms gap (bytes emitted *during*
        // capture-pane), self-healed by the agent's next redraw — far cheaper
        // than permanent stacking.
        let _ = file.seek(std::io::SeekFrom::End(0)).await;
        // Reset the client parser before painting the snapshot so EVERY
        // bit of stale state from the previous session is discarded —
        // not just the visible grid.
        //
        //   ESC c (RIS, "Reset to Initial State")
        //
        // This is more thorough than the previous `\x1b[2J\x1b[H`
        // (clear-screen + cursor-home), which left SGR colors, saved
        // cursor positions, scroll regions, alternate-screen state,
        // application keypad/cursor mode, and mouse-tracking modes
        // intact. Carrying any of those across a session-switch (or
        // a crash-and-resume on a different agent type) showed up as
        // hard-to-pin-down corruption: text in the wrong color long
        // after the agent stopped emitting that SGR, scroll regions
        // clipping vt100-parser updates to a strip of the screen,
        // mouse events firing on a session that never asked for them.
        let mut payload = Vec::with_capacity(snap.len() + 4);
        payload.extend_from_slice(b"\x1bc");
        payload.extend_from_slice(&snap);
        if socket
            .send(Message::Binary(Bytes::from(payload)))
            .await
            .is_err()
        {
            return;
        }
        snapshot_sent = true;
        // The file cursor was pinned at the post-capture end above; no re-seek —
        // anything appended since then replays through the tail exactly once.
    }

    // Fallback: if capture-pane didn't yield anything (early in session
    // life, before tmux has rendered, or for non-tmux sessions), keep the
    // old 4 KB tail behaviour so users still see *something* on connect.
    if !snapshot_sent && let Ok(end) = file.seek(std::io::SeekFrom::End(0)).await {
        let backfill = end.min(BACKFILL_BYTES);
        if backfill > 0
            && file
                .seek(std::io::SeekFrom::End(-(backfill as i64)))
                .await
                .is_ok()
        {
            let mut backfill_buf = vec![0u8; backfill as usize];
            if file.read_exact(&mut backfill_buf).await.is_ok()
                && socket
                    .send(Message::Binary(Bytes::from(backfill_buf)))
                    .await
                    .is_err()
            {
                return;
            }
        }
    }

    // Replay any non-resize input that arrived during the resize-wait window.
    // Rare in practice (a fast typer connecting and hammering keys before the
    // first frame fires), but preserves the previous "every byte forwarded"
    // contract so we never silently drop a keystroke at connect time.
    for chunk in early_input {
        let _ = agentum_tmux::send_bytes(&target, &chunk).await;
    }

    // Tail the pane log on a dedicated task and pipe chunks through an mpsc.
    // The main loop multiplexes `tail_rx` (output) and `socket.recv()` (input)
    // so a chatty pane never starves keystrokes — and vice versa.
    //
    // We also remember the log position the tail starts from so the outer
    // loop can save where the client left off on disconnect — that's what
    // makes `{"resume":true}` reconnects deliver only the missed delta.
    let tail_start_pos = file.stream_position().await.unwrap_or(0);
    let (tail_tx, mut tail_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
    let tail_handle = tokio::spawn(async move {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            match file.read(&mut buf).await {
                Ok(0) => sleep(Duration::from_millis(80)).await,
                Ok(n) => {
                    if tail_tx
                        .send(Bytes::copy_from_slice(&buf[..n]))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut bytes_forwarded: u64 = 0;
    // Why: tmux consumes the agent's OSC title (set-titles off) so it never
    // reaches the client through the pane byte stream — the desktop's
    // title-derived agent-status (working/idle/needs-attention) had no input.
    // Poll the captured pane_title and re-inject it as a synthetic OSC title
    // whenever it changes. Invisible to the terminal grid; the desktop's
    // title pipeline extracts it. ~400ms keeps the sidebar dot responsive
    // without hammering tmux.
    let mut title_ticker = tokio::time::interval(Duration::from_millis(400));
    let mut last_pane_title = String::new();
    loop {
        tokio::select! {
            _ = title_ticker.tick() => {
                if let Ok(title) = agentum_tmux::pane_title(&target).await
                    && !title.is_empty()
                    && title != last_pane_title
                {
                    last_pane_title = title.clone();
                    let mut osc = Vec::with_capacity(title.len() + 5);
                    osc.extend_from_slice(b"\x1b]0;");
                    osc.extend_from_slice(title.as_bytes());
                    osc.push(0x07);
                    if socket.send(Message::Binary(Bytes::from(osc))).await.is_err() {
                        break;
                    }
                }
            }
            chunk = tail_rx.recv() => match chunk {
                Some(bytes) => {
                    // Coalesce any backlog into one frame (no added latency).
                    // Byte total is unchanged, so the checkpoint stays accurate.
                    let frame = coalesce_queued(bytes, &mut tail_rx);
                    let len = frame.len() as u64;
                    if socket.send(Message::Binary(frame)).await.is_err() {
                        break;
                    }
                    bytes_forwarded += len;
                    // Keep the checkpoint live so a concurrent reconnect
                    // can take the (cheap) delta path instead of the
                    // (destructive) snapshot path. Without this, the
                    // checkpoint only updates at disconnect — and the
                    // disconnect write loses the race against any
                    // reconnect that arrives in the same millisecond.
                    save_checkpoint(
                        &positions,
                        id,
                        tail_start_pos.saturating_add(bytes_forwarded),
                        current_size,
                    );
                }
                None => break, // tail task ended (file error / eof on dead pane)
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Binary(b))) if !b.is_empty() => {
                    if let Err(e) = agentum_tmux::send_bytes(&target, &b).await
                        && socket
                            .send(Message::Text(format!("[input dropped: {e}]").into()))
                            .await
                            .is_err()
                    {
                        break;
                    }
                }
                Some(Ok(Message::Text(t))) => {
                    // Text frames double as a side-channel for control
                    // messages — `{"resize":{"cols":N,"rows":N}}` and
                    // `{"refresh":true}`. Anything that isn't a recognised
                    // JSON envelope is forwarded as raw input bytes
                    // (preserves the old behaviour for clients that send
                    // keystrokes as text).
                    if parse_refresh(&t) {
                        // Client asked for a clean re-snapshot. Same
                        // payload shape as the initial fresh-snapshot
                        // path: parser reset (RIS) + current visible
                        // grid. Cheap and side-effect-free on tmux.
                        if let Ok(snap) = agentum_tmux::capture_pane_ansi(&target).await
                            && !snap.is_empty()
                        {
                            let mut payload = Vec::with_capacity(snap.len() + 4);
                            payload.extend_from_slice(b"\x1bc");
                            payload.extend_from_slice(&snap);
                            if socket
                                .send(Message::Binary(Bytes::from(payload)))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    } else if let Some((cols, rows)) = parse_resize(&t) {
                        // Track every successful resize so the disconnect
                        // checkpoint records the size at the time the
                        // client actually left, not the size we captured
                        // in the early-input window.
                        current_size = Some((cols, rows));
                        // Refresh the live checkpoint with the new size
                        // so a concurrent reconnect's size-match gate
                        // doesn't fall back to a fresh snapshot just
                        // because the saved size is stale.
                        save_checkpoint(
                            &positions,
                            id,
                            tail_start_pos.saturating_add(bytes_forwarded),
                            current_size,
                        );
                        if let Err(e) = agentum_tmux::resize_window(&target, cols, rows).await
                            && socket
                                .send(Message::Text(format!("[resize dropped: {e}]").into()))
                                .await
                                .is_err()
                        {
                            break;
                        }
                    } else if let Err(e) = agentum_tmux::send_bytes(&target, t.as_bytes()).await
                        && socket
                            .send(Message::Text(format!("[input dropped: {e}]").into()))
                            .await
                            .is_err()
                    {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
    tail_handle.abort();
    // Final save on disconnect — typically redundant now that we stamp
    // the checkpoint live during the forward loop, but keeps the
    // invariant that the last byte forwarded is reflected even if the
    // very last `save_checkpoint` call lost a race with the abort.
    save_checkpoint(
        &positions,
        id,
        tail_start_pos.saturating_add(bytes_forwarded),
        current_size,
    );
}

/// Remote (SSH) session stream — the push-based mirror of [`stream_session`].
///
/// Previously this polled `capture-pane` over SSH every 700 ms and re-sent a
/// full-screen snapshot (RIS + whole pane) on any change, which made remote
/// terminals lag up to 700 ms behind and flicker as the client cleared and
/// repainted on every tick. Now we follow the remote pane log incrementally:
/// `pipe_pane` (armed at session start, re-armed here for safety) appends raw
/// pane bytes to a per-session log on the host, and a single persistent
/// `ssh tail -f` streams those bytes as they appear — the same incremental
/// model as the local file tail, just sourced over one long-lived SSH channel.
pub(super) async fn stream_remote_session(
    mut socket: WebSocket,
    host: Host,
    id: Uuid,
    target: String,
    redraw_requested: bool,
) {
    let log = match paths::pane_log(&id.to_string()) {
        Ok(p) => p,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("[path error: {e}]").into()))
                .await;
            return;
        }
    };

    // Current screen state: pipe-pane only carries output produced *after* it was
    // armed, so a fresh connect (or an idle pane) needs one snapshot to paint
    // what's already there. RIS (`\x1bc`) resets the client parser first — same
    // payload shape as a fresh local connect.
    //
    // `capture_pane_with_log_offset` ALSO re-arms pipe-pane (idempotent `-o`) in
    // the same remote exec, so the old separate arm round-trip is gone — one
    // SSH call at connect instead of two, halving the time-to-first-paint on a
    // distant host.
    //
    // The log's byte size is sampled in the SAME remote exec as the snapshot, and
    // the tail below replays from that offset. Previously the tail started at EOF
    // *at attach time*, and its (deliberately unmultiplexed) SSH connection takes
    // a full handshake to attach — every byte a busy agent emitted in that
    // multi-second window was silently dropped, and a chunk lost mid-escape-
    // sequence left the terminal corrupted until a manual refresh.
    // Capture the snapshot, RETRYING while it comes back empty. A just-launched
    // agent paints its first frame slowly — Claude Code spins up node and draws
    // a "trust this folder?" prompt, which measured >3s to appear — and that
    // first frame is often a STATIC screen (it waits for input, emitting nothing
    // more), so the live tail has nothing to stream and the snapshot is the only
    // way to paint it. A single capture at connect therefore returned a blank
    // grid and the pane sat BLANK ("opened an agent, no response") until a manual
    // refresh. Re-capturing every ~300ms until the pane has drawn something (or a
    // 12s budget elapses — generous enough for a cold node/agent boot) makes a
    // freshly-opened agent paint as soon as it renders. The loop breaks the
    // instant a frame is non-empty, so a fast pane isn't delayed; each retry
    // re-samples the offset so the tail still resumes exactly past what we paint.
    // Redraw heal (see the local path and the `redraw` query doc): force the
    // remote agent to fully repaint before we snapshot, so a corrupted grid
    // (e.g. an OS `wall` broadcast written over the pane on the host) is
    // overpainted rather than re-captured. We don't learn the pane's absolute
    // size at connect here, so provoke the SIGWINCH with a RELATIVE 1-row
    // shrink-then-restore that nets to the original geometry. No file probe on
    // a remote host, so use a fixed inter-resize pause long enough for the
    // agent to observe the smaller size and start repainting; the snapshot
    // retry loop below then captures the clean frame.
    if redraw_requested {
        const REMOTE_NUDGE_PAUSE: Duration = Duration::from_millis(150);
        let _ = crate::host_runtime::resize_window_relative(&host, &target, -1).await;
        sleep(REMOTE_NUDGE_PAUSE).await;
        let _ = crate::host_runtime::resize_window_relative(&host, &target, 1).await;
        sleep(REMOTE_NUDGE_PAUSE).await;
    }

    const SNAPSHOT_RETRY_BUDGET: Duration = Duration::from_millis(12_000);
    const SNAPSHOT_RETRY_GAP: Duration = Duration::from_millis(300);
    let snap_deadline = tokio::time::Instant::now() + SNAPSHOT_RETRY_BUDGET;
    let mut log_offset: Option<u64> = None;
    loop {
        let out_of_budget = tokio::time::Instant::now() >= snap_deadline;
        match crate::host_runtime::capture_pane_with_log_offset(&host, &target, &log).await {
            Ok((offset, snap)) => {
                log_offset = Some(offset);
                if !snap.is_empty() {
                    let mut payload = Vec::with_capacity(snap.len() + 2);
                    payload.extend_from_slice(b"\x1bc");
                    payload.extend_from_slice(&snap);
                    if socket
                        .send(Message::Binary(Bytes::from(payload)))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    break;
                }
                // Empty grid: the agent hasn't painted yet. Keep retrying.
                if out_of_budget {
                    break;
                }
                sleep(SNAPSHOT_RETRY_GAP).await;
            }
            // A capture error early in a session's life is usually transient —
            // the tmux pane is still being set up, or the streaming SSH channel
            // is cold. RETRY within the budget instead of bailing to a blank
            // pane; only give up (fall back to tail-from-EOF) once time's up.
            Err(_) => {
                if out_of_budget {
                    break;
                }
                sleep(SNAPSHOT_RETRY_GAP).await;
            }
        }
    }

    // Persistent `ssh tail -f` of the remote pane log. Its stdout is pumped
    // through an mpsc so the select loop below multiplexes output against
    // keystrokes — a chatty pane never starves input, and vice versa.
    let mut child = match crate::host_runtime::spawn_remote_pane_tail(&host, &log, log_offset) {
        Ok(c) => c,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("[remote tail error: {e}]").into()))
                .await;
            return;
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill().await;
        return;
    };
    // Drain + log the tail's stderr so a transport failure that ends the stream
    // (e.g. the remote sshd refusing the channel under MaxSessions pressure:
    // "Session open refused by peer") is recorded instead of silently surfacing
    // as a bare "[session stream closed]". Also keeps the pipe from filling.
    if let Some(mut stderr) = child.stderr.take() {
        let label = log
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        tokio::spawn(async move {
            let mut buf = Vec::new();
            if stderr.read_to_end(&mut buf).await.is_ok() && !buf.is_empty() {
                tracing::warn!(
                    session = %label,
                    "remote pane tail ended: {}",
                    String::from_utf8_lossy(&buf).trim()
                );
            }
        });
    }
    let (tail_tx, mut tail_rx) = tokio::sync::mpsc::channel::<Bytes>(64);
    let tail_handle = tokio::spawn(async move {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            match stdout.read(&mut buf).await {
                // 0 = ssh/tail exited (channel/host dropped). Unlike a local file
                // tail this never means "caught up"; end the task so the loop
                // tears the connection down.
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tail_tx
                        .send(Bytes::copy_from_slice(&buf[..n]))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });

    // Input writer: keystrokes leave the select loop through an mpsc and a
    // dedicated task delivers them. The fast path is a PERSISTENT SSH channel
    // ([`spawn_remote_input_writer`]): each keystroke is a one-way write down an
    // already-open stream (~1 RTT, ~150 ms to a distant host) instead of the old
    // exec-per-keystroke (a fresh `tmux send-keys` channel open + round-trip,
    // ~450 ms measured — which made typing into a remote agent unusable). If the
    // persistent writer can't spawn (or dies), we fall back to the per-exec
    // `send_bytes` so input still works, just slower.
    //
    // Either way the loop coalesces whatever queued while the previous write was
    // in flight (fast typing, paste) into one write, and delivery failures are
    // logged rather than echoed — the pane not echoing already signals a drop.
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let input_handle = {
        let host = host.clone();
        let target = target.clone();
        tokio::spawn(async move {
            // Persistent keystroke channel + its stdin; None → use per-exec fallback.
            let mut writer = crate::host_runtime::spawn_remote_input_writer(&host, &target).ok();
            let mut stdin = writer.as_mut().and_then(|c| c.stdin.take());
            while let Some(first) = input_rx.recv().await {
                let mut buf = first;
                // Drain whatever queued while the previous write was in flight,
                // up to the send-keys 4 KB chunk size.
                while buf.len() < 4096 {
                    match input_rx.try_recv() {
                        Ok(more) => buf.extend_from_slice(&more),
                        Err(_) => break,
                    }
                }
                let mut delivered = false;
                if let Some(si) = stdin.as_mut() {
                    let line = crate::host_runtime::encode_input_hex_line(&buf);
                    if si.write_all(&line).await.is_ok() && si.flush().await.is_ok() {
                        delivered = true;
                    } else {
                        // Persistent channel broke — drop the child (kills the
                        // dead ssh) and fall back to per-exec for this and every
                        // subsequent keystroke.
                        stdin = None;
                        drop(writer.take());
                    }
                }
                if !delivered
                    && let Err(e) = crate::host_runtime::send_bytes(&host, &target, &buf).await
                {
                    tracing::warn!(target = %target, error = ?e, "remote input send failed");
                }
            }
        })
    };

    // Why: like the local stream, the remote pane's OSC title is consumed by tmux
    // on the host and never crosses the pane byte stream — so the desktop's
    // title-derived agent status would be blank for SSH sessions. Poll the remote
    // pane_title and re-inject it as a synthetic OSC title on change. Each poll is
    // a round-trip SSH exec over the *shared* ControlMaster, and that master is
    // also what carries keystroke `send_keys` — so a too-fast cadence across many
    // open sessions churns the master's limited channels (remote MaxSessions) and
    // can starve input. 2.5 s keeps agent-status lag imperceptible while leaving
    // the master headroom.
    let mut title_ticker = tokio::time::interval(Duration::from_millis(2500));
    let mut last_pane_title = String::new();
    loop {
        tokio::select! {
            _ = title_ticker.tick() => {
                if let Ok(title) = crate::host_runtime::pane_title(&host, &target).await
                    && !title.is_empty()
                    && title != last_pane_title
                {
                    last_pane_title = title.clone();
                    let mut osc = Vec::with_capacity(title.len() + 5);
                    osc.extend_from_slice(b"\x1b]0;");
                    osc.extend_from_slice(title.as_bytes());
                    osc.push(0x07);
                    if socket.send(Message::Binary(Bytes::from(osc))).await.is_err() {
                        break;
                    }
                }
            }
            chunk = tail_rx.recv() => match chunk {
                Some(bytes) => {
                    // Coalesce a backlog of small SSH-tail reads into one frame
                    // (no added latency) so a weak client isn't woken once per
                    // tiny chunk of a chatty remote agent.
                    let frame = coalesce_queued(bytes, &mut tail_rx);
                    if socket.send(Message::Binary(frame)).await.is_err() {
                        break;
                    }
                }
                None => break, // tail task ended (ssh died / host unreachable)
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Binary(b))) if !b.is_empty() => {
                    // Channel-full (writer wedged on a dead host for 256
                    // frames) drops the key; the 12 s ssh timeout unwedges
                    // the writer long before that backlog accrues.
                    let _ = input_tx.try_send(b.to_vec());
                }
                Some(Ok(Message::Text(t))) => {
                    if let Some((cols, rows)) = parse_resize(&t) {
                        if let Err(e) = crate::host_runtime::resize_window(&host, &target, cols, rows).await
                            && socket.send(Message::Text(format!("[resize dropped: {e}]").into())).await.is_err()
                        {
                            break;
                        }
                    } else if parse_refresh(&t) {
                        // Re-paint the current screen on demand (same shape as the
                        // initial snapshot). Heals any bytes missed at connect.
                        if let Ok(snap) = crate::host_runtime::capture_pane_ansi(&host, &target).await
                            && !snap.is_empty()
                        {
                            let mut payload = Vec::with_capacity(snap.len() + 2);
                            payload.extend_from_slice(b"\x1bc");
                            payload.extend_from_slice(&snap);
                            if socket.send(Message::Binary(Bytes::from(payload))).await.is_err() {
                                break;
                            }
                        }
                    } else {
                        let _ = input_tx.try_send(t.as_bytes().to_vec());
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
    tail_handle.abort();
    input_handle.abort();
    let _ = child.kill().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn coalesce_queued_forwards_lone_chunk_unchanged() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(8);
        drop(tx); // empty + closed: nothing to drain
        let out = coalesce_queued(Bytes::from_static(b"abc"), &mut rx);
        assert_eq!(&out[..], b"abc");
    }

    #[tokio::test]
    async fn coalesce_queued_merges_backlog_into_one_frame() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(8);
        // Simulate a weak-client backlog: several tiny chunks already queued.
        tx.send(Bytes::from_static(b"two")).await.unwrap();
        tx.send(Bytes::from_static(b"three")).await.unwrap();
        let out = coalesce_queued(Bytes::from_static(b"one"), &mut rx);
        // Byte total is preserved and ordered — only the framing changes.
        assert_eq!(&out[..], b"onetwothree");
        // Drained everything that was waiting.
        assert!(rx.try_recv().is_err());
    }
}
