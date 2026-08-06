# Design: Self-Healing SSH Pane Streams

## Diagnosis

The remote stream treats EOF from the persistent `ssh tail -f` child as its
only liveness signal. A half-open TCP path, wedged multiplexed channel, or live
local SSH child that no longer forwards bytes produces no EOF, so the recovery
state machine never runs. Input can provoke a remote redraw and make the stream
appear to wake, but output progress is not independently verified.

The same WebSocket `select!` loop also awaits title polling over the interactive
ControlMaster. That periodic round trip temporarily stops dispatching pane
output and input and consumes the pool intended for latency-sensitive work.

## Components

### Combined background pane-state probe

Add one bounded SSH operation that returns the remote pane-log byte size and
pane title. Run it on the streaming ControlMaster, never the interactive one.
Only one probe may be in flight per attached stream. The main WebSocket loop
starts and consumes probe tasks but never awaits network I/O inside a ticker
branch, so tail and keyboard traffic remain dispatchable.

### Progress accounting

Initialize a local next-log-offset from the atomic connect snapshot. Increment
it for every raw byte read from the tail before WebSocket framing/coalescing.
Compare this consumed offset with the size returned by the background probe:

- equal size means healthy idle or caught-up streaming;
- a smaller remote size resets the baseline because the log was replaced or
  truncated;
- a larger remote size indicates data exists that the tail has not delivered.

Require the discrepancy to remain after a short grace/probe boundary before
declaring the channel wedged, preventing a normal probe-versus-read race from
causing churn. Any intervening tail progress clears the candidate.

### In-place recovery

On confirmed lag, stop and reap only the pane-tail child. Enter the existing
`reestablish_remote_tail` path, which obtains the current saved-host revision,
captures a pane snapshot and log offset atomically, reconnects the tail, then
repaints the WebSocket. Carry the new offset back to progress accounting.

Repeated pooled failures retain the existing recovery plan: validate/evict only
the streaming master and then use `SshMux::Off` as the escape path. The input
writer and interactive master are never evicted by output-liveness recovery.

### Reduced background contention

Use the combined pane-state probe for title changes, eliminating the separate
interactive-master title call. Keep activity-sensitive cadence: output can make
status refresh due promptly, while healthy idle sessions use the bounded safety
cadence. Probe results inject OSC title bytes exactly as today.

## Failure Handling

- Probe timeouts/errors do not by themselves prove the tail is wedged. Record
  them and retry on the normal bounded cadence.
- Host lifecycle cancellation wins over probe, recovery, input, and cleanup.
- A failed recovery retains the existing reconnect notice and WebSocket
  teardown semantics after bounded attempts.
- Offset parsing failure is conservative and cannot authorize silent skipping.

## Verification

- Pure tests for progress classification: caught up, healthy idle, first lag,
  persistent lag, intervening delivery, and log truncation.
- Script/argv tests proving the combined probe uses the streaming role and emits
  size plus title without modifying the pane.
- Async tests proving a pending maintenance probe does not block input/tail
  dispatch and confirmed lag enters recovery without keyboard input.
- Existing exact-byte input, tail reconnect, pooled-probe, and workspace suites.
