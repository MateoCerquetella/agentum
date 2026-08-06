# Decisions

## D-001: Detect missing output by comparing durable log progress

Status: Accepted

### Evidence

- The pane log is the source consumed by `tail -f`, and connect /
  recovery already pair its byte offset with a pane snapshot. The current code
  only notices child EOF, which does not cover a live-but-silent SSH channel.

### Options

1. Send synthetic bytes through the pane.
2. Periodically reconnect all tails.
3. Compare remote log size with locally consumed tail bytes.

### Chosen approach

Choose option 3. Compare low-frequency remote log size with the locally consumed
  offset and require a persistent discrepancy before recovery.

### Trade-offs and risks

- Adds a bounded metadata round trip, but distinguishes healthy
  idle panes without injecting user-visible data or reconnecting them.

### Verification

- Test caught-up, idle, lagging, recovered, and truncated-log states.

## D-002: Combine liveness and title maintenance off the input path

Status: Accepted

### Evidence

- Title polling already creates periodic SSH traffic and currently
  awaits an interactive-master exec inside the WebSocket dispatch loop.

### Options

1. Add a second liveness poll.
2. Remove title propagation.
3. Combine both reads in one background streaming-master operation.

### Chosen approach

Choose option 3. Combine log-size and pane-title reads, run at most one task at a
  time on the streaming role, and consume results asynchronously.

### Trade-offs and risks

- A saturated streaming master can delay titles, which is
  preferable to delaying accepted input and is handled by existing streaming
  recovery.

### Verification

- Prove a pending maintenance task does not block input or pane dispatch.

## D-003: Recover the tail, not the whole session

Status: Accepted

### Evidence

- The SSH pool has separate interactive and streaming roles; input
  already has its own bounded writer/fallback recovery.

### Options

1. Restart the WebSocket/session.
2. Evict both masters.
3. Replace only the tail and validate/evict the streaming master if necessary.

### Chosen approach

Choose option 3. Reuse the existing in-place tail recovery and exact-role eviction.

### Trade-offs and risks

- Recovery code must carry its new log offset back to the caller,
  but the pane, WebSocket, accepted input, and unrelated sessions remain intact.

### Verification

- Assert recovery selects only the streaming role and preserves input state.

## D-004: Treat probe failures as uncertainty, not proof of a wedge

Status: Accepted

### Evidence

- Shared masters can be healthy but temporarily at channel capacity,
  and existing pooled-probe classification deliberately preserves that state.

### Options

1. Evict on any failed probe.
2. Ignore probe failures.
3. Retry and recover only from confirmed durable-log lag or existing EOF signals.

### Chosen approach

Choose option 3. Do not evict based solely on one maintenance failure.

### Trade-offs and risks

- A total transport outage follows existing timeout/retry bounds
  instead of being declared immediately, avoiding disruption of healthy peers.

### Verification

- Cover failed, busy, and successful probe classification without broad eviction.
