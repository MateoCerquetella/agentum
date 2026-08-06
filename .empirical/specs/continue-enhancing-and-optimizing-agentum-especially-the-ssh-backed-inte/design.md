# Design: Continuously Responsive SSH Sessions

## Current failure surfaces

The terminal WebSocket client enables Nagle and uses a biased select that can
keep choosing outbound key frames while inbound pane frames are ready. The
remote pane watchdog probes log progress through the streaming ControlMaster;
that creates circular observation when the streaming pool itself is wedged.
Interactive SSH commands also use compression intended for bulk pane output.
Finally, the installed executable predates the host-aware upload implementation.

## Design

### Terminal WebSocket

Construct terminal WebSockets with TCP_NODELAY enabled. Use Tokio's fair select
for the established bidirectional pump so neither sustained input nor sustained
output starves the other. Preserve ownership of the outbound receiver across
reconnects and all existing reconnect/resume behavior.

### SSH pool roles

Extend `SshMux` with an `Observer` role backed by a distinct `cmo-` control
socket. Interactive input remains on `cm-`; pane tails remain on `cms-`; title
and log-progress probes move to `cmo-`. Host warmup, health repair, mutation
cleanup, and tests include all three revision-scoped roles.

The interactive and observer pools use `Compression=no` to minimize latency for
small messages. Streaming retains `Compression=yes` because terminal output is
compressible and throughput-sensitive. Unmultiplexed retries retain compression
for compatibility with their mixed workloads.

### Recovery

Keep the existing two-observation lag proof and atomic capture/log-offset tail
replacement. Only change the probe transport: observer failure is independently
repairable, while a successful observer probe can still prove that the
streaming tail is behind and trigger streaming-role repair.

### Image paste and executable

Retain the reviewed host-aware transaction: lifecycle leases, saved-host tmux
validation, private atomic byte write, and exact remote path injection. Add no
alternate upload path. After all verification and integration gates pass,
install the workspace binary so the user's next launch runs these changes.

## Failure handling

- Closing or editing a host retires all exact role sockets concurrently.
- Observer probe errors do not block the WebSocket loop and are retried.
- Persistent log lag repairs only streaming transport.
- Input writer timeouts retain the bounded per-exec fallback.
- Upload failures occur before success events and clipboard completion.

## Verification mapping

- AC-1: WebSocket connector argument and fair-selection regression checks.
- AC-2/3: SSH command/path tests for every role and observer probe routing.
- AC-4: remote tail progress/recovery state tests.
- AC-5/6: upload destination, binary write, clipboard decision, and host-aware
  route tests already present plus full regression suite.
- AC-7/8: focused packages, configured workspace checkpoints, review receipt,
  independent integration, and installed executable timestamp.
